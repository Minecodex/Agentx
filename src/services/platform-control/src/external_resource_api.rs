use agentx_api_types::PageResponse;
use agentx_runtime_contracts::RuntimeResourceProbeV1;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, mysql::MySqlRow};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    control_helpers::required_name,
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/knowledge/connections",
            get(list_rag_connections).post(create_rag_connection),
        )
        .route(
            "/api/v1/knowledge/connections/{id}/test-connection",
            post(test_rag_connection),
        )
        .route(
            "/api/v1/knowledge/resources",
            get(list_knowledge).post(create_knowledge),
        )
        .route(
            "/api/v1/knowledge/resources/{id}",
            get(get_knowledge)
                .patch(update_knowledge)
                .delete(delete_knowledge),
        )
        .route(
            "/api/v1/memory/connections",
            get(list_memory_connections).post(create_memory_connection),
        )
        .route(
            "/api/v1/memory/connections/{id}/test-connection",
            post(test_memory_connection),
        )
        .route(
            "/api/v1/memory/namespaces",
            get(list_memory).post(create_memory),
        )
        .route(
            "/api/v1/memory/namespaces/{id}",
            get(get_memory).patch(update_memory).delete(delete_memory),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    search: Option<String>,
    status: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionResponse {
    id: Uuid,
    name: String,
    provider: Option<String>,
    endpoint: String,
    health_path: String,
    credential_id: Option<Uuid>,
    owner_department_id: Uuid,
    configuration: Value,
    status: String,
    version: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateConnectionRequest {
    name: String,
    provider: Option<String>,
    endpoint: String,
    health_path: Option<String>,
    credential_id: Option<Uuid>,
    owner_department_id: Uuid,
    configuration: Value,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KnowledgeResponse {
    id: Uuid,
    connection_id: Uuid,
    connection_name: String,
    name: String,
    external_resource_id: String,
    owner_department_id: Uuid,
    sync_status: String,
    status: String,
    grant_count: u64,
    version: u64,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateKnowledgeRequest {
    connection_id: Uuid,
    name: String,
    external_resource_id: String,
    owner_department_id: Uuid,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryResponse {
    id: Uuid,
    connection_id: Uuid,
    connection_name: String,
    name: String,
    external_namespace: String,
    access_mode: String,
    owner_department_id: Uuid,
    status: String,
    grant_count: u64,
    version: u64,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateMemoryRequest {
    connection_id: Uuid,
    name: String,
    external_namespace: String,
    access_mode: String,
    owner_department_id: Uuid,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    name: String,
    status: String,
    version: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    status: String,
    latency_ms: Option<u64>,
    error_code: Option<String>,
    error_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    checked_at: OffsetDateTime,
}

async fn list_rag_connections(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<Json<Vec<ConnectionResponse>>> {
    actor.require("knowledge:view")?;
    Ok(Json(
        list_connections(&state, &actor, "rag_connections").await?,
    ))
}
async fn create_rag_connection(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateConnectionRequest>,
) -> ApiResult<(StatusCode, Json<ConnectionResponse>)> {
    actor.require("knowledge:manage")?;
    create_connection(&state, &actor, input, "rag_connections").await
}
async fn test_rag_connection(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<HealthResponse>> {
    actor.require("knowledge:manage")?;
    test_connection(&state, &actor, "rag_connections", "rag", id).await
}
async fn list_memory_connections(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<Json<Vec<ConnectionResponse>>> {
    actor.require("memory:view")?;
    Ok(Json(
        list_connections(&state, &actor, "memory_connections").await?,
    ))
}
async fn create_memory_connection(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateConnectionRequest>,
) -> ApiResult<(StatusCode, Json<ConnectionResponse>)> {
    actor.require("memory:manage")?;
    create_connection(&state, &actor, input, "memory_connections").await
}
async fn test_memory_connection(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<HealthResponse>> {
    actor.require("memory:manage")?;
    test_connection(&state, &actor, "memory_connections", "memory", id).await
}

async fn list_knowledge(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<KnowledgeResponse>>> {
    actor.require("knowledge:view")?;
    let (page, page_size, search, status) = list_values(query);
    let (rows, total) = knowledge_rows(&state, &actor, &search, &status, page, page_size).await?;
    Ok(Json(PageResponse {
        items: rows
            .into_iter()
            .map(knowledge_from_row)
            .collect::<Result<_, _>>()?,
        page,
        page_size,
        total: total.try_into().unwrap_or_default(),
    }))
}
async fn create_knowledge(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateKnowledgeRequest>,
) -> ApiResult<(StatusCode, Json<KnowledgeResponse>)> {
    actor.require("knowledge:manage")?;
    require_department_scope(&state, &actor, input.owner_department_id).await?;
    require_connection(&state, &actor, "rag_connections", input.connection_id).await?;
    let provider: String =
        sqlx::query_scalar("SELECT provider FROM rag_connections WHERE tenant_id=? AND id=?")
            .bind(actor.tenant_id)
            .bind(input.connection_id)
            .fetch_one(&state.pool)
            .await?;
    if provider == "lightrag" {
        agentx_runtime_contracts::rag::validate_lightrag_workspace(&input.external_resource_id)
            .map_err(|error| ApiError::bad_request(error.code, error.message))?;
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO rag_resources(id,tenant_id,connection_id,name,external_resource_id,owner_department_id) VALUES(?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(input.connection_id).bind(required_name(&input.name)?).bind(required_external(&input.external_resource_id)?).bind(input.owner_department_id).execute(&state.pool).await.map_err(|error|map_external_error(error,"KNOWLEDGE_EXTERNAL_RESOURCE_ID_EXISTS"))?;
    Ok((
        StatusCode::CREATED,
        Json(load_knowledge(&state, actor.tenant_id, id).await?),
    ))
}
async fn get_knowledge(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<KnowledgeResponse>> {
    actor.require("knowledge:view")?;
    require_resource(&state, &actor, "rag_resources", id).await?;
    Ok(Json(load_knowledge(&state, actor.tenant_id, id).await?))
}
async fn update_knowledge(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateRequest>,
) -> ApiResult<Json<KnowledgeResponse>> {
    actor.require("knowledge:manage")?;
    require_resource(&state, &actor, "rag_resources", id).await?;
    validate_status(&input.status)?;
    let changed=sqlx::query("UPDATE rag_resources SET name=?,status=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?").bind(required_name(&input.name)?).bind(input.status).bind(actor.tenant_id).bind(id).bind(input.version).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "KNOWLEDGE_VERSION_CONFLICT",
            "Knowledge resource changed",
        ));
    }
    Ok(Json(load_knowledge(&state, actor.tenant_id, id).await?))
}
async fn delete_knowledge(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("knowledge:manage")?;
    delete_resource(&state, &actor, "rag", "rag_resources", id).await
}

async fn list_memory(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<MemoryResponse>>> {
    actor.require("memory:view")?;
    let (page, page_size, search, status) = list_values(query);
    let (rows, total) = memory_rows(&state, &actor, &search, &status, page, page_size).await?;
    Ok(Json(PageResponse {
        items: rows
            .into_iter()
            .map(memory_from_row)
            .collect::<Result<_, _>>()?,
        page,
        page_size,
        total: total.try_into().unwrap_or_default(),
    }))
}
async fn create_memory(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateMemoryRequest>,
) -> ApiResult<(StatusCode, Json<MemoryResponse>)> {
    actor.require("memory:manage")?;
    require_department_scope(&state, &actor, input.owner_department_id).await?;
    require_connection(&state, &actor, "memory_connections", input.connection_id).await?;
    if !matches!(input.access_mode.as_str(), "read" | "read_write") {
        return Err(ApiError::bad_request(
            "INVALID_MEMORY_ACCESS",
            "Memory access mode is invalid",
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO memory_namespaces(id,tenant_id,connection_id,name,external_namespace,access_mode,owner_department_id) VALUES(?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(input.connection_id).bind(required_name(&input.name)?).bind(required_external(&input.external_namespace)?).bind(input.access_mode).bind(input.owner_department_id).execute(&state.pool).await.map_err(|error|map_external_error(error,"MEMORY_EXTERNAL_NAMESPACE_EXISTS"))?;
    Ok((
        StatusCode::CREATED,
        Json(load_memory(&state, actor.tenant_id, id).await?),
    ))
}
async fn get_memory(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<MemoryResponse>> {
    actor.require("memory:view")?;
    require_resource(&state, &actor, "memory_namespaces", id).await?;
    Ok(Json(load_memory(&state, actor.tenant_id, id).await?))
}
async fn update_memory(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateRequest>,
) -> ApiResult<Json<MemoryResponse>> {
    actor.require("memory:manage")?;
    require_resource(&state, &actor, "memory_namespaces", id).await?;
    validate_status(&input.status)?;
    let changed=sqlx::query("UPDATE memory_namespaces SET name=?,status=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?").bind(required_name(&input.name)?).bind(input.status).bind(actor.tenant_id).bind(id).bind(input.version).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "MEMORY_VERSION_CONFLICT",
            "Memory namespace changed",
        ));
    }
    Ok(Json(load_memory(&state, actor.tenant_id, id).await?))
}
async fn delete_memory(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("memory:manage")?;
    delete_resource(&state, &actor, "memory", "memory_namespaces", id).await
}

async fn create_connection(
    state: &ControlApiState,
    actor: &Actor,
    input: CreateConnectionRequest,
    table: &str,
) -> ApiResult<(StatusCode, Json<ConnectionResponse>)> {
    require_department_scope(state, actor, input.owner_department_id).await?;
    require_credential(state, actor.tenant_id, input.credential_id).await?;
    let endpoint = Url::parse(&input.endpoint)
        .map_err(|_| ApiError::bad_request("INVALID_ENDPOINT", "Connection endpoint is invalid"))?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(
            "INVALID_ENDPOINT",
            "Connection endpoint must use HTTP or HTTPS",
        ));
    }
    let health = input.health_path.unwrap_or_else(|| "/health".into());
    if !health.starts_with('/') || health.len() > 512 {
        return Err(ApiError::bad_request(
            "INVALID_HEALTH_PATH",
            "Health path must begin with /",
        ));
    }
    let id = Uuid::now_v7();
    match table {
        "rag_connections" => {
            let provider = input.provider.as_deref().unwrap_or("lightrag");
            if !matches!(provider, "lightrag" | "ragflow") {
                return Err(ApiError::bad_request(
                    "INVALID_PROVIDER",
                    "Knowledge provider must be lightrag or ragflow",
                ));
            }
            sqlx::query(
                "INSERT INTO rag_connections(id,tenant_id,name,provider,endpoint,health_path,credential_id,owner_department_id,configuration_json) VALUES(?,?,?,?,?,?,?,?,?)",
            )
            .bind(id)
            .bind(actor.tenant_id)
            .bind(required_name(&input.name)?)
            .bind(provider)
            .bind(input.endpoint)
            .bind(health)
            .bind(input.credential_id)
            .bind(input.owner_department_id)
            .bind(input.configuration)
            .execute(&state.pool)
            .await?;
        }
        "memory_connections" => {
            sqlx::query(
                "INSERT INTO memory_connections(id,tenant_id,name,endpoint,health_path,credential_id,owner_department_id,configuration_json) VALUES(?,?,?,?,?,?,?,?)",
            )
            .bind(id)
            .bind(actor.tenant_id)
            .bind(required_name(&input.name)?)
            .bind(input.endpoint)
            .bind(health)
            .bind(input.credential_id)
            .bind(input.owner_department_id)
            .bind(input.configuration)
            .execute(&state.pool)
            .await?;
        }
        _ => return Err(ApiError::internal("unsupported connection table")),
    }
    Ok((
        StatusCode::CREATED,
        Json(load_connection(state, actor.tenant_id, table, id).await?),
    ))
}
async fn list_connections(
    state: &ControlApiState,
    actor: &Actor,
    table: &str,
) -> ApiResult<Vec<ConnectionResponse>> {
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let sql = match (table, administrator) {
        ("rag_connections", true) => {
            "SELECT id,name,provider,endpoint,health_path,credential_id,owner_department_id,configuration_json,status,version FROM rag_connections WHERE tenant_id=? ORDER BY name,id"
        }
        ("memory_connections", true) => {
            "SELECT id,name,NULL provider,endpoint,health_path,credential_id,owner_department_id,configuration_json,status,version FROM memory_connections WHERE tenant_id=? ORDER BY name,id"
        }
        ("rag_connections", false) => {
            "SELECT c.id,c.name,c.provider,c.endpoint,c.health_path,c.credential_id,c.owner_department_id,c.configuration_json,c.status,c.version FROM rag_connections c JOIN department_closure dc ON dc.tenant_id=c.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=c.owner_department_id WHERE c.tenant_id=? ORDER BY c.name,c.id"
        }
        ("memory_connections", false) => {
            "SELECT c.id,c.name,NULL provider,c.endpoint,c.health_path,c.credential_id,c.owner_department_id,c.configuration_json,c.status,c.version FROM memory_connections c JOIN department_closure dc ON dc.tenant_id=c.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=c.owner_department_id WHERE c.tenant_id=? ORDER BY c.name,c.id"
        }
        _ => return Err(ApiError::internal("unsupported connection table")),
    };
    let rows = if administrator {
        sqlx::query(sql)
            .bind(actor.tenant_id)
            .fetch_all(&state.pool)
            .await?
    } else {
        sqlx::query(sql)
            .bind(actor.department_id)
            .bind(actor.tenant_id)
            .fetch_all(&state.pool)
            .await?
    };
    Ok(rows
        .into_iter()
        .map(connection_from_row)
        .collect::<Result<_, _>>()?)
}
async fn load_connection(
    state: &ControlApiState,
    tenant: Uuid,
    table: &str,
    id: Uuid,
) -> ApiResult<ConnectionResponse> {
    let sql = match table {
        "rag_connections" => {
            "SELECT id,name,provider,endpoint,health_path,credential_id,owner_department_id,configuration_json,status,version FROM rag_connections WHERE tenant_id=? AND id=?"
        }
        "memory_connections" => {
            "SELECT id,name,NULL provider,endpoint,health_path,credential_id,owner_department_id,configuration_json,status,version FROM memory_connections WHERE tenant_id=? AND id=?"
        }
        _ => return Err(ApiError::internal("unsupported connection table")),
    };
    let row = sqlx::query(sql)
        .bind(tenant)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Connection"))?;
    Ok(connection_from_row(row)?)
}
async fn test_connection(
    state: &ControlApiState,
    actor: &Actor,
    table: &str,
    resource_type: &str,
    id: Uuid,
) -> ApiResult<Json<HealthResponse>> {
    require_connection(state, actor, table, id).await?;
    let connection = load_connection(state, actor.tenant_id, table, id).await?;
    let check = crate::model_api::execute_runtime_resource_check(
        state,
        actor.tenant_id,
        connection.endpoint,
        connection.credential_id,
        RuntimeResourceProbeV1::HttpGet {
            path: connection.health_path,
        },
    )
    .await?;
    record_health(
        state,
        actor,
        resource_type,
        id,
        &check.status,
        check.latency_ms,
        check.error_code.as_deref(),
        check.error_message.as_deref(),
    )
    .await?;
    Ok(Json(HealthResponse {
        status: check.status,
        latency_ms: Some(check.latency_ms),
        error_code: check.error_code,
        error_message: check.error_message,
        checked_at: check.checked_at,
    }))
}

async fn knowledge_rows(
    state: &ControlApiState,
    actor: &Actor,
    search: &str,
    status: &str,
    page: u32,
    page_size: u32,
) -> ApiResult<(Vec<MySqlRow>, i64)> {
    if actor.roles.iter().any(|role| role == "company_admin") {
        let rows=sqlx::query("SELECT r.id,r.connection_id,c.name connection_name,r.name,r.external_resource_id,r.owner_department_id,r.sync_status,r.status,r.version,r.updated_at,(SELECT COUNT(*) FROM resource_grants g WHERE g.tenant_id=r.tenant_id AND g.resource_type='rag' AND g.resource_id=r.id AND g.subject_type='workflow_service_identity') grant_count FROM rag_resources r JOIN rag_connections c ON c.tenant_id=r.tenant_id AND c.id=r.connection_id WHERE r.tenant_id=? AND (?='' OR r.status=?) AND (?='%%' OR r.name LIKE ?) ORDER BY r.updated_at DESC,r.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(status).bind(status).bind(search).bind(search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total=sqlx::query_scalar("SELECT COUNT(*) FROM rag_resources r WHERE r.tenant_id=? AND (?='' OR r.status=?) AND (?='%%' OR r.name LIKE ?)").bind(actor.tenant_id).bind(status).bind(status).bind(search).bind(search).fetch_one(&state.pool).await?;
        Ok((rows, total))
    } else {
        let rows=sqlx::query("SELECT r.id,r.connection_id,c.name connection_name,r.name,r.external_resource_id,r.owner_department_id,r.sync_status,r.status,r.version,r.updated_at,(SELECT COUNT(*) FROM resource_grants g WHERE g.tenant_id=r.tenant_id AND g.resource_type='rag' AND g.resource_id=r.id AND g.subject_type='workflow_service_identity') grant_count FROM rag_resources r JOIN rag_connections c ON c.tenant_id=r.tenant_id AND c.id=r.connection_id WHERE r.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=r.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=r.owner_department_id) AND (?='' OR r.status=?) AND (?='%%' OR r.name LIKE ?) ORDER BY r.updated_at DESC,r.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(actor.department_id).bind(status).bind(status).bind(search).bind(search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total=sqlx::query_scalar("SELECT COUNT(*) FROM rag_resources r WHERE r.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=r.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=r.owner_department_id) AND (?='' OR r.status=?) AND (?='%%' OR r.name LIKE ?)").bind(actor.tenant_id).bind(actor.department_id).bind(status).bind(status).bind(search).bind(search).fetch_one(&state.pool).await?;
        Ok((rows, total))
    }
}
async fn memory_rows(
    state: &ControlApiState,
    actor: &Actor,
    search: &str,
    status: &str,
    page: u32,
    page_size: u32,
) -> ApiResult<(Vec<MySqlRow>, i64)> {
    if actor.roles.iter().any(|role| role == "company_admin") {
        let rows=sqlx::query("SELECT n.id,n.connection_id,c.name connection_name,n.name,n.external_namespace,n.access_mode,n.owner_department_id,n.status,n.version,n.updated_at,(SELECT COUNT(*) FROM resource_grants g WHERE g.tenant_id=n.tenant_id AND g.resource_type='memory' AND g.resource_id=n.id AND g.subject_type='workflow_service_identity') grant_count FROM memory_namespaces n JOIN memory_connections c ON c.tenant_id=n.tenant_id AND c.id=n.connection_id WHERE n.tenant_id=? AND (?='' OR n.status=?) AND (?='%%' OR n.name LIKE ?) ORDER BY n.updated_at DESC,n.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(status).bind(status).bind(search).bind(search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total=sqlx::query_scalar("SELECT COUNT(*) FROM memory_namespaces n WHERE n.tenant_id=? AND (?='' OR n.status=?) AND (?='%%' OR n.name LIKE ?)").bind(actor.tenant_id).bind(status).bind(status).bind(search).bind(search).fetch_one(&state.pool).await?;
        Ok((rows, total))
    } else {
        let rows=sqlx::query("SELECT n.id,n.connection_id,c.name connection_name,n.name,n.external_namespace,n.access_mode,n.owner_department_id,n.status,n.version,n.updated_at,(SELECT COUNT(*) FROM resource_grants g WHERE g.tenant_id=n.tenant_id AND g.resource_type='memory' AND g.resource_id=n.id AND g.subject_type='workflow_service_identity') grant_count FROM memory_namespaces n JOIN memory_connections c ON c.tenant_id=n.tenant_id AND c.id=n.connection_id WHERE n.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=n.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=n.owner_department_id) AND (?='' OR n.status=?) AND (?='%%' OR n.name LIKE ?) ORDER BY n.updated_at DESC,n.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(actor.department_id).bind(status).bind(status).bind(search).bind(search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total=sqlx::query_scalar("SELECT COUNT(*) FROM memory_namespaces n WHERE n.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=n.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=n.owner_department_id) AND (?='' OR n.status=?) AND (?='%%' OR n.name LIKE ?)").bind(actor.tenant_id).bind(actor.department_id).bind(status).bind(status).bind(search).bind(search).fetch_one(&state.pool).await?;
        Ok((rows, total))
    }
}

async fn require_connection(
    state: &ControlApiState,
    actor: &Actor,
    table: &str,
    id: Uuid,
) -> ApiResult<()> {
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let sql = match (table, administrator) {
        ("rag_connections", true) => {
            "SELECT EXISTS(SELECT 1 FROM rag_connections WHERE tenant_id=? AND id=?)"
        }
        ("memory_connections", true) => {
            "SELECT EXISTS(SELECT 1 FROM memory_connections WHERE tenant_id=? AND id=?)"
        }
        ("rag_connections", false) => {
            "SELECT EXISTS(SELECT 1 FROM rag_connections c JOIN department_closure dc ON dc.tenant_id=c.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=c.owner_department_id WHERE c.tenant_id=? AND c.id=?)"
        }
        ("memory_connections", false) => {
            "SELECT EXISTS(SELECT 1 FROM memory_connections c JOIN department_closure dc ON dc.tenant_id=c.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=c.owner_department_id WHERE c.tenant_id=? AND c.id=?)"
        }
        _ => return Err(ApiError::internal("unsupported connection table")),
    };
    let visible: bool = if administrator {
        sqlx::query_scalar(sql)
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_one(&state.pool)
            .await?
    } else {
        sqlx::query_scalar(sql)
            .bind(actor.department_id)
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_one(&state.pool)
            .await?
    };
    if visible {
        Ok(())
    } else {
        Err(ApiError::not_found("Connection"))
    }
}
pub(crate) async fn require_resource(
    state: &ControlApiState,
    actor: &Actor,
    table: &str,
    id: Uuid,
) -> ApiResult<()> {
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let sql = match (table, administrator) {
        ("rag_resources", true) => {
            "SELECT EXISTS(SELECT 1 FROM rag_resources WHERE tenant_id=? AND id=?)"
        }
        ("memory_namespaces", true) => {
            "SELECT EXISTS(SELECT 1 FROM memory_namespaces WHERE tenant_id=? AND id=?)"
        }
        ("rag_resources", false) => {
            "SELECT EXISTS(SELECT 1 FROM rag_resources r JOIN department_closure dc ON dc.tenant_id=r.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=r.owner_department_id WHERE r.tenant_id=? AND r.id=?)"
        }
        ("memory_namespaces", false) => {
            "SELECT EXISTS(SELECT 1 FROM memory_namespaces r JOIN department_closure dc ON dc.tenant_id=r.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=r.owner_department_id WHERE r.tenant_id=? AND r.id=?)"
        }
        _ => return Err(ApiError::internal("unsupported resource table")),
    };
    let visible: bool = if administrator {
        sqlx::query_scalar(sql)
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_one(&state.pool)
            .await?
    } else {
        sqlx::query_scalar(sql)
            .bind(actor.department_id)
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_one(&state.pool)
            .await?
    };
    if visible {
        Ok(())
    } else {
        Err(ApiError::not_found("Resource"))
    }
}
async fn delete_resource(
    state: &ControlApiState,
    actor: &Actor,
    resource_type: &str,
    table: &str,
    id: Uuid,
) -> ApiResult<StatusCode> {
    require_resource(state, actor, table, id).await?;
    let mut tx = state.pool.begin().await?;
    let lock_sql = match table {
        "rag_resources" => "SELECT id FROM rag_resources WHERE tenant_id=? AND id=? FOR UPDATE",
        "memory_namespaces" => {
            "SELECT id FROM memory_namespaces WHERE tenant_id=? AND id=? FOR UPDATE"
        }
        _ => return Err(ApiError::internal("unsupported resource table")),
    };
    sqlx::query(lock_sql)
        .bind(actor.tenant_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::not_found("Resource"))?;
    let references:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM workflow_draft_resources WHERE tenant_id=? AND resource_type=? AND resource_id=?)+(SELECT COUNT(*) FROM workflow_version_resources WHERE tenant_id=? AND resource_type=? AND resource_id=?)").bind(actor.tenant_id).bind(resource_type).bind(id).bind(actor.tenant_id).bind(resource_type).bind(id).fetch_one(&mut *tx).await?;
    if references != 0 {
        return Err(ApiError::conflict(
            "RESOURCE_REFERENCED",
            "Resource is referenced",
        ));
    }
    if table == "rag_resources" {
        let documents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM knowledge_documents WHERE tenant_id=? AND rag_resource_id=?",
        )
        .bind(actor.tenant_id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if documents != 0 {
            return Err(ApiError::conflict(
                "RESOURCE_REFERENCED",
                "Delete knowledge documents before deleting their resource",
            ));
        }
    }
    let sql = match table {
        "rag_resources" => "DELETE FROM rag_resources WHERE tenant_id=? AND id=?",
        "memory_namespaces" => "DELETE FROM memory_namespaces WHERE tenant_id=? AND id=?",
        _ => return Err(ApiError::internal("unsupported resource table")),
    };
    sqlx::query(sql)
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn require_department_scope(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<()> {
    if actor.roles.iter().any(|role| role == "company_admin") {
        return Ok(());
    }
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM department_closure WHERE tenant_id=? AND ancestor_id=? AND descendant_id=?)").bind(actor.tenant_id).bind(actor.department_id).bind(id).fetch_one(&state.pool).await?;
    if allowed {
        Ok(())
    } else {
        Err(ApiError::forbidden("Department is outside the actor scope"))
    }
}
async fn require_credential(
    state: &ControlApiState,
    tenant: Uuid,
    id: Option<Uuid>,
) -> ApiResult<()> {
    let Some(id) = id else { return Ok(()) };
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM credentials WHERE tenant_id=? AND id=? AND status='active')",
    )
    .bind(tenant)
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    if active {
        Ok(())
    } else {
        Err(ApiError::unprocessable(
            "CREDENTIAL_UNAVAILABLE",
            "Credential is disabled or unavailable",
        ))
    }
}
#[allow(clippy::too_many_arguments)]
async fn record_health(
    state: &ControlApiState,
    actor: &Actor,
    resource_type: &str,
    id: Uuid,
    status: &str,
    latency: u64,
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> ApiResult<()> {
    let next:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(check_sequence),0)+1 AS UNSIGNED) FROM resource_health_checks WHERE tenant_id=? AND resource_type=? AND resource_id=?").bind(actor.tenant_id).bind(resource_type).bind(id).fetch_one(&state.pool).await?;
    sqlx::query("INSERT INTO resource_health_checks(id,tenant_id,resource_type,resource_id,check_sequence,status,latency_ms,error_code,error_message,checked_by) VALUES(?,?,?,?,?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(resource_type).bind(id).bind(next).bind(status).bind(latency).bind(error_code).bind(error_message).bind(actor.user_id).execute(&state.pool).await?;
    Ok(())
}
async fn load_knowledge(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<KnowledgeResponse> {
    let row=sqlx::query("SELECT r.id,r.connection_id,c.name connection_name,r.name,r.external_resource_id,r.owner_department_id,r.sync_status,r.status,r.version,r.updated_at,(SELECT COUNT(*) FROM resource_grants g WHERE g.tenant_id=r.tenant_id AND g.resource_type='rag' AND g.resource_id=r.id AND g.subject_type='workflow_service_identity') grant_count FROM rag_resources r JOIN rag_connections c ON c.tenant_id=r.tenant_id AND c.id=r.connection_id WHERE r.tenant_id=? AND r.id=?").bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Knowledge resource"))?;
    Ok(knowledge_from_row(row)?)
}
async fn load_memory(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<MemoryResponse> {
    let row=sqlx::query("SELECT n.id,n.connection_id,c.name connection_name,n.name,n.external_namespace,n.access_mode,n.owner_department_id,n.status,n.version,n.updated_at,(SELECT COUNT(*) FROM resource_grants g WHERE g.tenant_id=n.tenant_id AND g.resource_type='memory' AND g.resource_id=n.id AND g.subject_type='workflow_service_identity') grant_count FROM memory_namespaces n JOIN memory_connections c ON c.tenant_id=n.tenant_id AND c.id=n.connection_id WHERE n.tenant_id=? AND n.id=?").bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Memory namespace"))?;
    Ok(memory_from_row(row)?)
}
fn connection_from_row(row: MySqlRow) -> Result<ConnectionResponse, sqlx::Error> {
    Ok(ConnectionResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        provider: row.try_get("provider")?,
        endpoint: row.try_get("endpoint")?,
        health_path: row.try_get("health_path")?,
        credential_id: row.try_get("credential_id")?,
        owner_department_id: row.try_get("owner_department_id")?,
        configuration: row.try_get("configuration_json")?,
        status: row.try_get("status")?,
        version: row.try_get("version")?,
    })
}
fn knowledge_from_row(row: MySqlRow) -> Result<KnowledgeResponse, sqlx::Error> {
    Ok(KnowledgeResponse {
        id: row.try_get("id")?,
        connection_id: row.try_get("connection_id")?,
        connection_name: row.try_get("connection_name")?,
        name: row.try_get("name")?,
        external_resource_id: row.try_get("external_resource_id")?,
        owner_department_id: row.try_get("owner_department_id")?,
        sync_status: row.try_get("sync_status")?,
        status: row.try_get("status")?,
        grant_count: row.try_get::<i64, _>("grant_count")? as u64,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn memory_from_row(row: MySqlRow) -> Result<MemoryResponse, sqlx::Error> {
    Ok(MemoryResponse {
        id: row.try_get("id")?,
        connection_id: row.try_get("connection_id")?,
        connection_name: row.try_get("connection_name")?,
        name: row.try_get("name")?,
        external_namespace: row.try_get("external_namespace")?,
        access_mode: row.try_get("access_mode")?,
        owner_department_id: row.try_get("owner_department_id")?,
        status: row.try_get("status")?,
        grant_count: row.try_get::<i64, _>("grant_count")? as u64,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn list_values(query: ListQuery) -> (u32, u32, String, String) {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    (
        page,
        page_size,
        format!("%{}%", query.search.unwrap_or_default().trim()),
        query.status.unwrap_or_default(),
    )
}
fn validate_status(value: &str) -> ApiResult<()> {
    if matches!(value, "active" | "disabled") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "INVALID_STATUS",
            "Resource status is invalid",
        ))
    }
}
fn required_external(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 512 {
        Err(ApiError::bad_request(
            "INVALID_EXTERNAL_REFERENCE",
            "External reference must contain 1 to 512 bytes",
        ))
    } else {
        Ok(value.to_owned())
    }
}
fn map_external_error(error: sqlx::Error, code: &'static str) -> ApiError {
    match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            let field = match code {
                "KNOWLEDGE_EXTERNAL_RESOURCE_ID_EXISTS" => "externalResourceId",
                "MEMORY_EXTERNAL_NAMESPACE_EXISTS" => "externalNamespace",
                _ => "name",
            };
            ApiError::conflict(code, "External resource reference already exists").with_field_error(
                field,
                code,
                "External resource reference already exists",
            )
        }
        _ => ApiError::from(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_external_contract() {
        assert!(required_external("namespace").is_ok());
        assert!(required_external("").is_err());
        assert!(validate_status("active").is_ok());
    }
}
