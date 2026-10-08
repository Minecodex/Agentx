//! Knowledge document management and hit-testing (plan7 P7-E): upload →
//! external index → status tracking → retrieval test. Indexing and retrieval
//! talk to the provider directly from Control over the dependencies network
//! path using the shared RAG protocol, with credentials read from the frozen
//! Vault snapshot.

use std::time::Instant;

use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::StatusCode,
};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use agentx_application::{ArtifactStore, ArtifactWrite};
use agentx_control_infrastructure::artifact::MySqlControlArtifactStore;
use agentx_domain::TenantId;

use crate::api_error::{ApiError, ApiResult};
use crate::control_api::{Actor, ControlApiState};

const MAX_DOCUMENT_SIZE: usize = 8 * 1024 * 1024;
const MAX_DOCUMENTS_PER_RESOURCE: u64 = 200;
const MAX_TOTAL_BYTES_PER_RESOURCE: u64 = 256 * 1024 * 1024;
const CONTENT_TYPE_ALLOWLIST: &[&str] = &[
    "text/plain",
    "text/markdown",
    "text/csv",
    "application/json",
];

pub fn routes() -> axum::Router<ControlApiState> {
    axum::Router::new()
        .route(
            "/api/v1/knowledge/resources/{id}/documents",
            axum::routing::get(list_documents).post(upload_document),
        )
        .route(
            "/api/v1/knowledge/resources/{id}/documents/{document_id}",
            axum::routing::delete(delete_document),
        )
        .route(
            "/api/v1/knowledge/resources/{id}/retrieval-test",
            axum::routing::post(retrieval_test),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_DOCUMENT_SIZE + 1024 * 1024,
        ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KnowledgeDocumentResponse {
    id: Uuid,
    rag_resource_id: Uuid,
    name: String,
    content_type: String,
    size_bytes: u64,
    external_document_id: Option<String>,
    status: String,
    error_code: Option<String>,
    error_message: Option<String>,
    indexed_at: Option<String>,
    version: u64,
    created_at: String,
}

async fn require_resource(
    state: &ControlApiState,
    actor: &Actor,
    resource_id: Uuid,
) -> ApiResult<sqlx::mysql::MySqlRow> {
    actor.require("knowledge:view")?;
    crate::external_resource_api::require_resource(state, actor, "rag_resources", resource_id)
        .await?;
    let row = sqlx::query(
        "SELECT r.id,r.name,r.external_resource_id,r.sync_status,r.version,c.provider,c.endpoint,c.health_path,c.credential_id FROM rag_resources r JOIN rag_connections c ON c.tenant_id=r.tenant_id AND c.id=r.connection_id WHERE r.tenant_id=? AND r.id=?",
    )
    .bind(actor.tenant_id)
    .bind(resource_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Knowledge resource"))?;
    Ok(row)
}

async fn upload_document(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(resource_id): Path<Uuid>,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<KnowledgeDocumentResponse>)> {
    actor.require("knowledge:manage")?;
    let resource = require_resource(&state, &actor, resource_id).await?;
    let provider: String = resource.try_get("provider")?;
    if provider != "lightrag" {
        return Err(ApiError::unprocessable(
            "RAG_OPERATION_UNSUPPORTED",
            "RAGFlow knowledge resources manage documents externally",
        ));
    }
    let mut upload = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad_request("INVALID_MULTIPART", "Upload is invalid"))?
    {
        if field.name() == Some("file") && upload.is_none() {
            let name = field
                .file_name()
                .unwrap_or("document")
                .chars()
                .take(255)
                .collect::<String>();
            let content_type = field.content_type().unwrap_or("text/plain").to_owned();
            let bytes = field.bytes().await.map_err(ApiError::internal)?.to_vec();
            upload = Some((name, content_type, bytes));
        }
    }
    let (name, content_type, bytes) = upload
        .ok_or_else(|| ApiError::bad_request("FILE_REQUIRED", "Multipart file is required"))?;
    if !CONTENT_TYPE_ALLOWLIST.contains(&content_type.as_str()) {
        return Err(ApiError::unprocessable(
            "KNOWLEDGE_DOCUMENT_TYPE_UNSUPPORTED",
            "Knowledge documents accept text, markdown, CSV and JSON",
        ));
    }
    if std::str::from_utf8(&bytes)
        .ok()
        .is_none_or(|text| text.trim().is_empty())
    {
        return Err(ApiError::bad_request(
            "KNOWLEDGE_DOCUMENT_INVALID",
            "Knowledge documents must contain non-empty UTF-8 text",
        ));
    }
    if bytes.len() > MAX_DOCUMENT_SIZE {
        return Err(ApiError::unprocessable(
            "KNOWLEDGE_DOCUMENT_TOO_LARGE",
            "Knowledge documents are limited to 8 MiB",
        ));
    }
    let store = MySqlControlArtifactStore::new(state.pool.clone(), state.control_objects.clone());
    let mut snapshot =
        crate::knowledge_indexing::snapshot(&state, actor.tenant_id, &resource, 0).await?;
    let stored = ArtifactStore::put(
        &store,
        ArtifactWrite {
            tenant_id: TenantId::from_uuid(actor.tenant_id),
            content_type: content_type.clone(),
            content: bytes.clone(),
        },
    )
    .await
    .map_err(ApiError::internal)?;
    let document_id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    let current_version: u64 = sqlx::query_scalar(
        "SELECT version FROM rag_resources WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(resource_id)
    .fetch_one(&mut *tx)
    .await?;
    let counts = sqlx::query(
        "SELECT COUNT(*) document_count,CAST(COALESCE(SUM(size_bytes),0) AS SIGNED) total_bytes FROM knowledge_documents WHERE tenant_id=? AND rag_resource_id=?",
    )
    .bind(actor.tenant_id)
    .bind(resource_id)
    .fetch_one(&mut *tx)
    .await?;
    if counts.try_get::<i64, _>("document_count")? >= MAX_DOCUMENTS_PER_RESOURCE as i64 {
        return Err(ApiError::unprocessable(
            "KNOWLEDGE_DOCUMENT_LIMIT",
            "Knowledge resources hold at most 200 documents",
        ));
    }
    if counts.try_get::<i64, _>("total_bytes")? + bytes.len() as i64
        > MAX_TOTAL_BYTES_PER_RESOURCE as i64
    {
        return Err(ApiError::unprocessable(
            "KNOWLEDGE_DOCUMENT_LIMIT",
            "Knowledge resources hold at most 256 MiB of documents",
        ));
    }
    let duplicate: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM knowledge_documents WHERE tenant_id=? AND rag_resource_id=? AND sha256=?",
    )
    .bind(actor.tenant_id)
    .bind(resource_id)
    .bind(sha256_hex(&bytes))
    .fetch_optional(&mut *tx)
    .await?;
    if duplicate.is_some() {
        return Err(ApiError::unprocessable(
            "KNOWLEDGE_DOCUMENT_DUPLICATED",
            "The same document content already exists on this knowledge resource",
        ));
    }
    snapshot["index_version"] = json!(current_version + 1);
    sqlx::query(
        "INSERT INTO knowledge_documents(id,tenant_id,rag_resource_id,name,content_type,size_bytes,sha256,artifact_id,index_snapshot_json,status,created_by) VALUES(?,?,?,?,?,?,?,?,?, 'uploading',?)",
    )
    .bind(document_id)
    .bind(actor.tenant_id)
    .bind(resource_id)
    .bind(&name)
    .bind(&content_type)
    .bind(bytes.len() as u64)
    .bind(stored.sha256.trim_start_matches("sha256:"))
    .bind(stored.id.as_uuid())
    .bind(&snapshot)
    .bind(actor.user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'knowledge_document',?,'source') ON DUPLICATE KEY UPDATE reference_role=VALUES(reference_role)",
    )
    .bind(actor.tenant_id)
    .bind(stored.id.as_uuid())
    .bind(document_id.to_string())
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE rag_resources SET sync_status='syncing',version=version+1 WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(resource_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let document = load_document(&state, actor.tenant_id, document_id).await?;
    Ok((StatusCode::ACCEPTED, Json(document)))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

async fn load_document(
    state: &ControlApiState,
    tenant_id: Uuid,
    document_id: Uuid,
) -> ApiResult<KnowledgeDocumentResponse> {
    let row = sqlx::query(
        "SELECT id,rag_resource_id,name,content_type,size_bytes,external_document_id,status,error_code,error_message,DATE_FORMAT(indexed_at,'%Y-%m-%dT%H:%i:%s.%fZ') indexed_at,version,DATE_FORMAT(created_at,'%Y-%m-%dT%H:%i:%s.%fZ') created_at FROM knowledge_documents WHERE tenant_id=? AND id=?",
    )
    .bind(tenant_id)
    .bind(document_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Knowledge document"))?;
    document_from_row(row)
}

fn document_from_row(row: sqlx::mysql::MySqlRow) -> ApiResult<KnowledgeDocumentResponse> {
    Ok(KnowledgeDocumentResponse {
        id: row.try_get("id")?,
        rag_resource_id: row.try_get("rag_resource_id")?,
        name: row.try_get("name")?,
        content_type: row.try_get("content_type")?,
        size_bytes: row.try_get("size_bytes")?,
        external_document_id: row.try_get("external_document_id")?,
        status: row.try_get("status")?,
        error_code: row.try_get("error_code")?,
        error_message: row.try_get("error_message")?,
        indexed_at: row.try_get::<Option<String>, _>("indexed_at")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn list_documents(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(resource_id): Path<Uuid>,
) -> ApiResult<Json<Vec<KnowledgeDocumentResponse>>> {
    require_resource(&state, &actor, resource_id).await?;
    let rows = sqlx::query(
        "SELECT id,rag_resource_id,name,content_type,size_bytes,external_document_id,status,error_code,error_message,DATE_FORMAT(indexed_at,'%Y-%m-%dT%H:%i:%s.%fZ') indexed_at,version,DATE_FORMAT(created_at,'%Y-%m-%dT%H:%i:%s.%fZ') created_at FROM knowledge_documents WHERE tenant_id=? AND rag_resource_id=? ORDER BY created_at DESC",
    )
    .bind(actor.tenant_id)
    .bind(resource_id)
    .fetch_all(&state.pool)
    .await?;
    let items = rows
        .into_iter()
        .map(document_from_row)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(items))
}

async fn delete_document(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((resource_id, document_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    actor.require("knowledge:manage")?;
    require_resource(&state, &actor, resource_id).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM rag_resources WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id)
        .bind(resource_id)
        .fetch_one(&mut *tx)
        .await?;
    let status: String = sqlx::query_scalar("SELECT status FROM knowledge_documents WHERE tenant_id=? AND rag_resource_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id).bind(resource_id).bind(document_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Knowledge document"))?;
    if matches!(status.as_str(), "uploading" | "indexing") {
        return Err(ApiError::conflict(
            "KNOWLEDGE_DOCUMENT_BUSY",
            "Wait for indexing to finish before removing its source record",
        ));
    }
    sqlx::query("DELETE FROM artifact_references WHERE tenant_id=? AND owner_type='knowledge_document' AND owner_id=?")
        .bind(actor.tenant_id).bind(document_id.to_string()).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM knowledge_documents WHERE tenant_id=? AND rag_resource_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(resource_id)
        .bind(document_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE rag_resources SET version=version+1 WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(resource_id)
        .execute(&mut *tx)
        .await?;
    crate::knowledge_indexing::refresh_resource_status(&mut tx, actor.tenant_id, resource_id)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetrievalTestRequest {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: u32,
}

fn default_top_k() -> u32 {
    5
}

async fn retrieval_test(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(resource_id): Path<Uuid>,
    Json(request): Json<RetrievalTestRequest>,
) -> ApiResult<Json<Value>> {
    actor.require("knowledge:manage")?;
    let resource = require_resource(&state, &actor, resource_id).await?;
    let query = request.query.trim();
    if query.is_empty() || query.len() > 2000 {
        return Err(ApiError::bad_request(
            "INVALID_RETRIEVAL_QUERY",
            "Query must be 1..=2000 characters",
        ));
    }
    let top_k = request.top_k.clamp(1, 20);
    let provider: String = resource.try_get("provider")?;
    let endpoint: String = resource.try_get("endpoint")?;
    let namespace: String = resource.try_get("external_resource_id")?;
    let index_version: u64 = resource.try_get("version")?;
    let input = json!({"query": query, "top_k": top_k});
    let (path, body, secret_header) = agentx_runtime_contracts::rag::rag_query_request(
        &provider,
        "retrieve",
        &namespace,
        &index_version.to_string(),
        &input,
    )
    .map_err(|error| ApiError::unprocessable(error.code, error.message))?;
    let secret = vault_secret(&state, &actor, resource.try_get("credential_id")?).await?;
    let started = Instant::now();
    let url = format!("{}/{}", endpoint.trim_end_matches('/'), path);
    validate_dependencies_http(&url)?;
    let mut builder = state
        .http
        .post(&url)
        .timeout(std::time::Duration::from_secs(20))
        .json(&body);
    if provider == "lightrag" {
        builder = builder.header("LIGHTRAG-WORKSPACE", &namespace);
    }
    if let Some(secret) = secret.as_deref() {
        builder = match secret_header {
            "authorization" => builder.bearer_auth(secret),
            _ => builder.header("x-api-key", secret),
        };
    }
    let response = builder.send().await.map_err(|_| {
        ApiError::unavailable("PROVIDER_UNAVAILABLE", "Knowledge provider is unreachable")
    })?;
    let status = response.status();
    let payload: Value = response.json().await.map_err(|_| {
        ApiError::unprocessable(
            "PROVIDER_REJECTED",
            "Knowledge provider returned an unreadable response",
        )
    })?;
    let duration_ms = started.elapsed().as_millis() as u64;
    if !status.is_success() {
        record_retrieval_test(
            &state,
            &actor,
            resource_id,
            query,
            top_k,
            0,
            Some("PROVIDER_REJECTED"),
            duration_ms,
        )
        .await;
        return Err(ApiError::unprocessable(
            "PROVIDER_REJECTED",
            format!("Knowledge provider returned HTTP {status}"),
        ));
    }
    let normalized = agentx_runtime_contracts::rag::finalize_retrieval_value(&provider, payload)
        .map_err(|error| ApiError::unprocessable(error.code, error.message))?;
    let hits = normalized
        .get("documents")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0) as u32;
    record_retrieval_test(
        &state,
        &actor,
        resource_id,
        query,
        top_k,
        hits,
        None,
        duration_ms,
    )
    .await;
    Ok(Json(normalized))
}

#[allow(clippy::too_many_arguments)]
async fn record_retrieval_test(
    state: &ControlApiState,
    actor: &Actor,
    resource_id: Uuid,
    query: &str,
    top_k: u32,
    hit_count: u32,
    error_code: Option<&str>,
    duration_ms: u64,
) {
    let _ = sqlx::query(
        "INSERT INTO rag_retrieval_tests(id,tenant_id,rag_resource_id,query,top_k,hit_count,error_code,duration_ms,created_by) VALUES(?,?,?,?,?,?,?,?,?)",
    )
    .bind(Uuid::now_v7())
    .bind(actor.tenant_id)
    .bind(resource_id)
    .bind(query.chars().take(2000).collect::<String>())
    .bind(top_k)
    .bind(hit_count)
    .bind(error_code)
    .bind(duration_ms)
    .bind(actor.user_id)
    .execute(&state.pool)
    .await;
}

async fn vault_secret(
    state: &ControlApiState,
    actor: &Actor,
    credential_id: Option<Uuid>,
) -> ApiResult<Option<String>> {
    let reference =
        crate::knowledge_indexing::credential_snapshot(state, actor.tenant_id, credential_id)
            .await?;
    crate::knowledge_indexing::read_secret(state, &reference)
        .await
        .map_err(|_| ApiError::unavailable("VAULT_UNAVAILABLE", "Credential could not be read"))
}

/// Control may only reach cluster-internal dependency endpoints over plain
/// HTTP; the NetworkPolicy carries the enforced allowlist.
fn validate_dependencies_http(url: &str) -> ApiResult<()> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| ApiError::bad_request("INVALID_ENDPOINT", "Knowledge endpoint is invalid"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ApiError::bad_request(
            "INVALID_ENDPOINT",
            "Knowledge endpoint must use http or https",
        ));
    }
    Ok(())
}
