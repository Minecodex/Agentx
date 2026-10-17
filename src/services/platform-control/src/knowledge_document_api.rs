//! Read-only external documents and query-scoped retrieval tests.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    knowledge_provider::{Provider, protocol_error},
};
use agentx_runtime_contracts::{
    rag::{RagScope, rag_query_request, validate_instance_scope, validate_retrieval_input},
    retrieval::{QueryScope, resolve_sources, retrieve},
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

pub fn routes() -> axum::Router<ControlApiState> {
    axum::Router::new()
        .route(
            "/api/v1/knowledge/queries/{id}/documents",
            axum::routing::get(list_documents),
        )
        .route(
            "/api/v1/knowledge/queries/{id}/retrieval-test",
            axum::routing::post(retrieval_test),
        )
}

async fn resource(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<(Provider, QueryScope)> {
    actor.require("knowledge:view")?;
    crate::external_resource_api::require_resource(state, actor, "rag_resources", id).await?;
    let row = sqlx::query("SELECT r.scope_json,c.id connection_id,c.version connection_version,c.provider,c.endpoint,c.credential_id FROM rag_resources r JOIN rag_connections c ON c.tenant_id=r.tenant_id AND c.id=r.connection_id WHERE r.tenant_id=? AND r.id=? AND r.status='active' AND c.status='active'")
        .bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::unprocessable("RESOURCE_UNAVAILABLE", "Query or connection is disabled"))?;
    let scope: RagScope =
        serde_json::from_value(row.try_get("scope_json")?).map_err(ApiError::internal)?;
    crate::resource_api::require_knowledge_credential(state, actor, row.try_get("credential_id")?)
        .await?;
    let provider = Provider::new(
        state,
        actor.tenant_id,
        row.try_get("provider")?,
        row.try_get("endpoint")?,
        row.try_get("credential_id")?,
    )
    .await?;
    scope.validate(&provider.kind).map_err(protocol_error)?;
    Ok((
        provider,
        QueryScope {
            query_id: id,
            connection_id: row.try_get("connection_id")?,
            connection_version: row.try_get("connection_version")?,
            scope,
        },
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentsQuery {
    #[serde(default = "first_page")]
    page: u32,
    #[serde(default = "page_size")]
    page_size: u32,
    dataset_id: Option<String>,
}
fn first_page() -> u32 {
    1
}
fn page_size() -> u32 {
    20
}

async fn list_documents(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(input): Query<DocumentsQuery>,
) -> ApiResult<Json<Value>> {
    let (provider, query) = resource(&state, &actor, id).await?;
    if query.scope.provider() == "lightrag" {
        let health = provider
            .request(&state, reqwest::Method::GET, "health", None)
            .await
            .map_err(protocol_error)?;
        validate_instance_scope(&query.scope, &health).map_err(protocol_error)?;
    }
    let page = input.page.max(1);
    let page_size = input.page_size.clamp(10, 100);
    let (value, scope) = match &query.scope {
        RagScope::Ragflow { dataset_ids } => {
            let dataset = input.dataset_id.as_ref().unwrap_or(&dataset_ids[0]);
            if !dataset_ids.contains(dataset) {
                return Err(ApiError::forbidden("Dataset is outside this query scope"));
            }
            let mut url = reqwest::Url::parse("http://provider/api/v1/datasets/")
                .map_err(ApiError::internal)?;
            url.path_segments_mut()
                .unwrap()
                .pop_if_empty()
                .push(dataset)
                .push("documents");
            let path = format!("{}?page={page}&page_size={page_size}", url.path());
            (
                provider
                    .request(&state, reqwest::Method::GET, &path, None)
                    .await
                    .map_err(protocol_error)?,
                RagScope::Ragflow {
                    dataset_ids: vec![dataset.clone()],
                },
            )
        }
        scope => (
            provider
                .request(
                    &state,
                    reqwest::Method::POST,
                    "documents/paginated",
                    Some(&json!({"page":page,"page_size":page_size})),
                )
                .await
                .map_err(protocol_error)?,
            scope.clone(),
        ),
    };
    let docs = if provider.kind == "ragflow" {
        value.pointer("/data/docs")
    } else {
        value.get("documents")
    }
    .and_then(Value::as_array)
    .ok_or_else(|| {
        ApiError::unprocessable("PROVIDER_REJECTED", "Provider omitted document list")
    })?;
    let items = docs.iter().map(|doc|json!({"id":doc["id"],"name":doc.get("name").or_else(||doc.get("file_path")),"status":doc.get("status").or_else(||doc.get("run")),"scope":scope,"connectionId":query.connection_id})).collect::<Vec<_>>();
    let total = if provider.kind == "ragflow" {
        value.pointer("/data/total")
    } else {
        value.pointer("/pagination/total_count")
    }
    .and_then(Value::as_u64)
    .unwrap_or(items.len() as u64);
    Ok(Json(
        json!({"items":items,"page":page,"pageSize":page_size,"total":total}),
    ))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RetrievalTestRequest {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
}
fn default_top_k() -> usize {
    5
}
async fn retrieval_test(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<RetrievalTestRequest>,
) -> ApiResult<Json<Value>> {
    actor.require("knowledge:manage")?;
    validate_retrieval_input(&input.query, input.top_k as u64).map_err(protocol_error)?;
    let (provider, query) = resource(&state, &actor, id).await?;
    let sources = resolve_sources(&[query]).map_err(protocol_error)?;
    let started = std::time::Instant::now();
    let result = retrieve(sources, input.top_k, |_, source| {
        let provider = &provider;
        let state = &state;
        let query = &input.query;
        async move {
            if source.scope.provider() == "lightrag" {
                let health = provider
                    .request(state, reqwest::Method::GET, "health", None)
                    .await?;
                validate_instance_scope(&source.scope, &health)?;
            }
            let (path, body, _) = rag_query_request(&source.scope, query)?;
            provider
                .request(state, reqwest::Method::POST, &path, Some(&body))
                .await
        }
    })
    .await;
    let elapsed = started.elapsed().as_millis() as u64;
    let hits = result
        .as_ref()
        .ok()
        .and_then(|v| v["documents"].as_array())
        .map_or(0, Vec::len);
    sqlx::query("INSERT INTO rag_retrieval_tests(id,tenant_id,rag_resource_id,query,top_k,hit_count,error_code,duration_ms,created_by) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(&input.query).bind(input.top_k as u32).bind(hits as u32).bind(result.as_ref().err().map(|e|e.code)).bind(elapsed).bind(actor.user_id).execute(&state.pool).await?;
    let mut result = result.map_err(|e| {
        ApiError::unprocessable(e.code, e.message).with_details(json!({"sources":e.sources}))
    })?;
    result["elapsedMs"] = json!(elapsed);
    Ok(Json(result))
}
