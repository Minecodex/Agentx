use super::*;
use crate::application_batches::{self as service, BatchCaller, BatchScope};
use agentx_runtime_contracts::*;
use serde::Deserialize;

pub(super) fn routes() -> Router<RuntimeState> {
    Router::new()
        .route("/applications/{slug}/batches", get(list).post(create))
        .route("/pages/{slug}/batches", post(create_page))
        .route("/applications/{slug}/batch-imports/parse", post(parse))
        .route("/batches/{id}", get(get_batch))
        .route("/batches/{id}/items", get(items))
        .route("/batches/{id}/stop", post(stop))
        .route("/batches/{id}/cancel", post(cancel))
        .route("/batches/{id}/rerun", post(rerun))
        .route("/batches/{id}/exports", post(export))
        .route("/batches/{id}/exports/{export_id}", get(get_export))
        .route("/batches/{id}/exports/{export_id}/file", get(download))
}

fn scope(caller: Caller) -> BatchScope {
    BatchScope::owner(BatchCaller {
        tenant_id: caller.tenant_id,
        application_id: caller.application_id,
        caller_type: caller.caller_type.into(),
        caller_id: caller.subject_id,
        token_version: caller.token_version,
        origin: caller.origin,
    })
}
async fn object(state: &RuntimeState, headers: &HeaderMap, id: Uuid) -> RuntimeResult<BatchScope> {
    if bearer_token(headers)?.starts_with("ory_at_") {
        return Err(RuntimeError::Unauthorized);
    }
    Ok(scope(
        authenticate_resource(state, headers, "batch", id).await?,
    ))
}

async fn create(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateBatchV1>,
) -> RuntimeResult<(StatusCode, Json<BatchViewV1>)> {
    if bearer_token(&headers)?.starts_with("ory_at_") {
        return Err(RuntimeError::Unauthorized);
    }
    let scope = scope(authenticate(&state, &headers, Some(&slug)).await?);
    let view = service::create(
        &state,
        &scope.caller,
        request,
        idempotency_key(&headers)?,
        None,
        None,
        &[],
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(view)))
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubmitBatchPage {
    pub publication_id: Uuid,
    pub revision: u64,
    pub name: String,
    pub items: Vec<BatchInputV1>,
}
async fn create_page(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SubmitBatchPage>,
) -> RuntimeResult<(StatusCode, Json<BatchViewV1>)> {
    let claims = state.trust.user(bearer_token(&headers)?)?;
    let application: Uuid = sqlx::query_scalar(
        "SELECT application_id FROM application_web_heads WHERE tenant_id=? AND slug=?",
    )
    .bind(claims.tenant_id)
    .bind(slug)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(RuntimeError::NotFound)?;
    let scope = scope(authorize_user_application(&state, claims, application).await?);
    let view = service::create(
        &state,
        &scope.caller,
        CreateBatchV1 {
            name: request.name,
            items: request.items,
        },
        idempotency_key(&headers)?,
        Some(ApplicationAccessBindingV1::Web {
            publication_id: request.publication_id,
            revision: request.revision,
        }),
        None,
        &[],
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(view)))
}
async fn list(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Query(query): Query<BatchQueryV1>,
) -> RuntimeResult<Json<BatchesPageV1>> {
    if bearer_token(&headers)?.starts_with("ory_at_") {
        return Err(RuntimeError::Unauthorized);
    }
    let scope = scope(authenticate(&state, &headers, Some(&slug)).await?);
    Ok(Json(service::list(&state, &scope, &query).await?))
}
async fn get_batch(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<Json<BatchViewV1>> {
    let scope = object(&state, &headers, id).await?;
    Ok(Json(service::get(&state, &scope, id).await?))
}
async fn items(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<BatchQueryV1>,
) -> RuntimeResult<Json<BatchItemsPageV1>> {
    let scope = object(&state, &headers, id).await?;
    Ok(Json(service::items(&state, &scope, id, &query).await?))
}
async fn stop(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<Json<BatchViewV1>> {
    idempotency_key(&headers)?;
    let scope = object(&state, &headers, id).await?;
    Ok(Json(service::stop(&state, &scope, id).await?))
}
async fn cancel(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<Json<BatchViewV1>> {
    let scope = object(&state, &headers, id).await?;
    Ok(Json(
        service::cancel(&state, &scope, id, idempotency_key(&headers)?).await?,
    ))
}
async fn rerun(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<BatchRerunV1>,
) -> RuntimeResult<(StatusCode, Json<BatchViewV1>)> {
    let scope = object(&state, &headers, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(service::rerun(&state, &scope, id, request, idempotency_key(&headers)?).await?),
    ))
}
async fn parse(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ReadTableV1>,
) -> RuntimeResult<Json<TableDataV1>> {
    if bearer_token(&headers)?.starts_with("ory_at_") {
        return Err(RuntimeError::Unauthorized);
    }
    let scope = scope(authenticate(&state, &headers, Some(&slug)).await?);
    Ok(Json(
        service::parse_table(&state, &scope.caller, request).await?,
    ))
}
async fn export(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<(StatusCode, Json<BatchExportV1>)> {
    let scope = object(&state, &headers, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(service::request_export(&state, &scope, id, idempotency_key(&headers)?).await?),
    ))
}
async fn get_export(
    State(state): State<RuntimeState>,
    Path((id, export)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> RuntimeResult<Json<BatchExportV1>> {
    let scope = object(&state, &headers, id).await?;
    Ok(Json(service::get_export(&state, &scope, id, export).await?))
}
async fn download(
    State(state): State<RuntimeState>,
    Path((id, export)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> RuntimeResult<Response> {
    let scope = object(&state, &headers, id).await?;
    let bytes = service::download_export(&state, &scope, id, export).await?;
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"batch.csv\"",
            ),
            (axum::http::header::CACHE_CONTROL, "no-store"),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response())
}

pub(super) fn schemas() -> Map<String, Value> {
    let mut values = Map::new();
    macro_rules! add{($($t:ty),*)=>{$(insert_public_schema::<$t>(&mut values,stringify!($t));)*};}
    add!(
        CreateBatchV1,
        SubmitBatchPage,
        BatchViewV1,
        BatchesPageV1,
        BatchItemsPageV1,
        BatchRerunV1,
        BatchExportV1,
        ReadTableV1,
        TableDataV1
    );
    values
}
pub(super) fn paths() -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("/gateway/v1/applications/{slug}/batches".into(),json!({"get":public_get("listMyBatches","BatchesPageV1"),"post":public_post("createBatch","CreateBatchV1","BatchViewV1","202")}));
    p.insert(
        "/gateway/v1/pages/{slug}/batches".into(),
        json!({"post":public_post("createPageBatch","SubmitBatchPage","BatchViewV1","202")}),
    );
    for (path, op, result) in [
        ("/gateway/v1/batches/{id}", "getBatch", "BatchViewV1"),
        (
            "/gateway/v1/batches/{id}/items",
            "listBatchItems",
            "BatchItemsPageV1",
        ),
        (
            "/gateway/v1/batches/{id}/exports/{export_id}",
            "getBatchExport",
            "BatchExportV1",
        ),
    ] {
        p.insert(path.into(), json!({"get":public_get(op,result)}));
    }
    for (op, result) in [
        ("stop", "BatchViewV1"),
        ("cancel", "BatchViewV1"),
        ("exports", "BatchExportV1"),
    ] {
        let status = if op == "exports" { "202" } else { "200" };
        p.insert(format!("/gateway/v1/batches/{{id}}/{op}"),json!({"post":{"operationId":format!("batch_{op}"),"parameters":[idempotency_parameter()],"responses":public_responses(result,status),"security":[{"runtimeBearer":[]}]}}));
    }
    p.insert(
        "/gateway/v1/batches/{id}/rerun".into(),
        json!({"post":public_post("rerunFailedItems","BatchRerunV1","BatchViewV1","202")}),
    );
    p.insert(
        "/gateway/v1/applications/{slug}/batch-imports/parse".into(),
        json!({"post":public_post("readTable","ReadTableV1","TableDataV1","200")}),
    );
    p.insert("/gateway/v1/batches/{id}/exports/{export_id}/file".into(),json!({"get":{"operationId":"downloadBatchExport","responses":{"200":{"description":"Authorized CSV export","content":{"text/csv":{"schema":{"type":"string","format":"binary"}}}}},"security":[{"runtimeBearer":[]}]}}));
    for (path, methods) in &mut p {
        for (_, operation) in methods.as_object_mut().into_iter().flatten() {
            let params = operation
                .as_object_mut()
                .expect("operation")
                .entry("parameters")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .expect("parameters");
            for name in ["slug", "id", "export_id"] {
                if path.contains(&format!("{{{name}}}")) {
                    let schema = if name == "slug" {
                        json!({"type":"string"})
                    } else {
                        json!({"type":"string","format":"uuid"})
                    };
                    params.push(json!({"name":name,"in":"path","required":true,"schema":schema}));
                }
            }
            if operation["operationId"] == "listMyBatches"
                || operation["operationId"] == "listBatchItems"
            {
                // Query fields are shared with the bounded management views.
                for (name, schema) in [
                    ("limit", json!({"type":"integer","minimum":1,"maximum":50})),
                    ("cursor", json!({"type":"string"})),
                    ("state", json!({"type":"string"})),
                    ("source", json!({"type":"string","enum":["web","api"]})),
                    ("requestKey", json!({"type":"string","maxLength":192})),
                ] {
                    operation["parameters"]
                        .as_array_mut()
                        .expect("parameters")
                        .push(json!({"name":name,"in":"query","schema":schema}));
                }
            }
        }
    }
    p
}
