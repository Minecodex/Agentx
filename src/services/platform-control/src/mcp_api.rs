use agentx_api_types::{PageRequest, PageResponse};
use agentx_domain::{ResourceOperation, ResourceReference, ResourceType, ResourceVersionSnapshot};
use agentx_runtime_contracts::{
    RuntimeMcpTransportV2, RuntimeResourceBindingV1, RuntimeResourceConfigurationV1,
    RuntimeResourceOperationV1, RuntimeResourceProbeV1,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, mysql::MySqlRow};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    control_helpers::required_name,
};

macro_rules! tool_select {
    ($tail:literal) => {
        concat!(
            "SELECT t.id,t.server_id,t.name,t.title,t.description,t.availability,t.current_version_number,t.version,tv.id current_version_id,tv.input_schema,tv.output_schema,tv.annotations_json,tv.schema_hash,p.enabled,p.debug_enabled,p.timeout_seconds,p.side_effect,p.version policy_version FROM mcp_tools t JOIN mcp_servers s ON s.tenant_id=t.tenant_id AND s.id=t.server_id JOIN mcp_tool_versions tv ON tv.tenant_id=t.tenant_id AND tv.tool_id=t.id AND tv.version_number=t.current_version_number JOIN mcp_tool_policies p ON p.tenant_id=t.tenant_id AND p.tool_id=t.id ",
            $tail
        )
    };
}

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/mcp/servers", get(list_servers).post(create_server))
        .route(
            "/api/v1/mcp/servers/{id}",
            get(get_server).patch(update_server).delete(delete_server),
        )
        .route("/api/v1/mcp/servers/{id}/tools", get(list_tools))
        .route("/api/v1/mcp/tools", get(list_all_tools))
        .route(
            "/api/v1/mcp/servers/{id}/test-connection",
            post(test_connection),
        )
        .route("/api/v1/mcp/servers/{id}/discover", post(discover_tools))
        .route(
            "/api/v1/mcp/tools/{id}/policy",
            axum::routing::patch(update_tool_policy),
        )
        .route("/api/v1/mcp/tools/{id}/debug-invoke", post(debug_invoke))
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
struct ServerResponse {
    id: Uuid,
    name: String,
    description: Option<String>,
    owner_department_id: Uuid,
    transport: McpTransportInput,
    status: String,
    current_version_number: u64,
    version: u64,
    tool_count: u64,
    #[serde(with = "time::serde::rfc3339::option")]
    last_discovered_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum McpTransportInput {
    StreamableHttp {
        endpoint: String,
        bearer_credential_id: Option<Uuid>,
    },
    Sse {
        endpoint: String,
        bearer_credential_id: Option<Uuid>,
    },
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        environment_credential_refs: Vec<EnvironmentCredentialInput>,
        runtime_sandbox: RuntimeSandboxInput,
    },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvironmentCredentialInput {
    name: String,
    credential_id: Uuid,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeSandboxInput {
    resource_id: Uuid,
    resource_version_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateServerRequest {
    name: String,
    description: Option<String>,
    owner_department_id: Uuid,
    transport: McpTransportInput,
    #[serde(default)]
    configuration: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateServerRequest {
    name: String,
    description: Option<String>,
    status: String,
    transport: McpTransportInput,
    #[serde(default)]
    configuration: Value,
    version: u64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolResponse {
    id: Uuid,
    server_id: Uuid,
    name: String,
    title: Option<String>,
    description: Option<String>,
    availability: String,
    current_version_id: Uuid,
    current_version_number: u64,
    input_schema: Value,
    output_schema: Option<Value>,
    annotations: Value,
    schema_hash: String,
    enabled: bool,
    debug_enabled: bool,
    timeout_seconds: u32,
    side_effect: String,
    version: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyRequest {
    enabled: bool,
    debug_enabled: bool,
    timeout_seconds: u32,
    side_effect: String,
    version: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DebugRequest {
    arguments: Value,
    expected_tool_version_id: Uuid,
    confirmed: bool,
    confirmation_text: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugResponse {
    result: Value,
    duration_ms: u64,
    tool_version_id: Uuid,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryResponse {
    run_id: Uuid,
    discovered_count: u32,
    tools: Vec<ToolResponse>,
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

#[derive(Clone)]
struct ServerConfig {
    version_id: Uuid,
    transport: RuntimeMcpTransportV2,
    credential: Option<agentx_runtime_contracts::VaultSecretReferenceV1>,
    runtime_sandbox_profile: Option<RuntimeResourceBindingV1>,
}
struct RemoteTool {
    name: String,
    title: Option<String>,
    description: Option<String>,
    input_schema: Value,
    output_schema: Option<Value>,
    annotations: Value,
}

async fn list_servers(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<ServerResponse>>> {
    actor.require("mcp:view")?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let status = query.status.unwrap_or_default();
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let (rows, total) = if administrator {
        let rows=sqlx::query("SELECT s.id,s.name,s.description,s.owner_department_id,s.status,s.current_version_number,s.version,s.last_discovered_at,s.updated_at,sv.transport,sv.endpoint,sv.credential_id,sv.configuration_json,(SELECT COUNT(*) FROM mcp_tools t WHERE t.tenant_id=s.tenant_id AND t.server_id=s.id AND t.availability='available') tool_count FROM mcp_servers s JOIN mcp_server_versions sv ON sv.tenant_id=s.tenant_id AND sv.server_id=s.id AND sv.version_number=s.current_version_number WHERE s.tenant_id=? AND (?='' OR s.status=?) AND (?='%%' OR s.name LIKE ?) ORDER BY s.updated_at DESC,s.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_servers s WHERE s.tenant_id=? AND (?='' OR s.status=?) AND (?='%%' OR s.name LIKE ?)").bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    } else {
        let rows=sqlx::query("SELECT s.id,s.name,s.description,s.owner_department_id,s.status,s.current_version_number,s.version,s.last_discovered_at,s.updated_at,sv.transport,sv.endpoint,sv.credential_id,sv.configuration_json,(SELECT COUNT(*) FROM mcp_tools t WHERE t.tenant_id=s.tenant_id AND t.server_id=s.id AND t.availability='available') tool_count FROM mcp_servers s JOIN mcp_server_versions sv ON sv.tenant_id=s.tenant_id AND sv.server_id=s.id AND sv.version_number=s.current_version_number WHERE s.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=s.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=s.owner_department_id) AND (?='' OR s.status=?) AND (?='%%' OR s.name LIKE ?) ORDER BY s.updated_at DESC,s.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM mcp_servers s WHERE s.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=s.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=s.owner_department_id) AND (?='' OR s.status=?) AND (?='%%' OR s.name LIKE ?)").bind(actor.tenant_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    };
    Ok(Json(PageResponse {
        items: rows
            .into_iter()
            .map(server_from_row)
            .collect::<Result<_, _>>()?,
        page,
        page_size,
        total: total.try_into().unwrap_or_default(),
    }))
}

async fn create_server(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateServerRequest>,
) -> ApiResult<(StatusCode, Json<ServerResponse>)> {
    actor.require("mcp:manage")?;
    require_department_scope(&state, &actor, input.owner_department_id).await?;
    let frozen = validate_transport(&state, actor.tenant_id, &input.transport).await?;
    ensure_transport_dependencies_authorized(
        &state,
        actor.tenant_id,
        input.owner_department_id,
        &input.transport,
    )
    .await?;
    validate_configuration(&input.configuration)?;
    let id = Uuid::now_v7();
    let hash = config_hash(&input.transport, &input.configuration)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO mcp_servers(id,tenant_id,name,description,owner_department_id,created_by) VALUES(?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(required_name(&input.name)?).bind(input.description).bind(input.owner_department_id).bind(actor.user_id).execute(&mut *tx).await.map_err(map_name_error)?;
    sqlx::query("INSERT INTO mcp_server_versions(id,tenant_id,server_id,version_number,transport,endpoint,credential_id,runtime_sandbox_profile_id,runtime_sandbox_profile_version_id,configuration_json,configuration_hash,created_by) VALUES(?,?,?,1,?,?,?,?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(frozen.kind).bind(frozen.endpoint)
        .bind(frozen.credential_id).bind(frozen.runtime_sandbox_profile_id).bind(frozen.runtime_sandbox_profile_version_id)
        .bind(json!({"transport":input.transport,"options":input.configuration})).bind(hash).bind(actor.user_id)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_server(&state, actor.tenant_id, id).await?),
    ))
}
async fn get_server(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ServerResponse>> {
    actor.require("mcp:view")?;
    require_server(&state, &actor, id).await?;
    Ok(Json(load_server(&state, actor.tenant_id, id).await?))
}
async fn update_server(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateServerRequest>,
) -> ApiResult<Json<ServerResponse>> {
    actor.require("mcp:manage")?;
    require_server(&state, &actor, id).await?;
    let owner_department_id: Uuid = sqlx::query_scalar(
        "SELECT owner_department_id FROM mcp_servers WHERE tenant_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    let frozen = validate_transport(&state, actor.tenant_id, &input.transport).await?;
    ensure_transport_dependencies_authorized(
        &state,
        actor.tenant_id,
        owner_department_id,
        &input.transport,
    )
    .await?;
    validate_configuration(&input.configuration)?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_STATUS",
            "MCP server status is invalid",
        ));
    }
    let hash = config_hash(&input.transport, &input.configuration)?;
    let mut tx = state.pool.begin().await?;
    let current=sqlx::query("SELECT current_version_number,version FROM mcp_servers WHERE tenant_id=? AND id=? FOR UPDATE").bind(actor.tenant_id).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("MCP server"))?;
    if current.try_get::<u64, _>("version")? != input.version {
        return Err(ApiError::conflict(
            "MCP_SERVER_VERSION_CONFLICT",
            "MCP server changed",
        ));
    }
    let existing:Option<u64>=sqlx::query_scalar("SELECT version_number FROM mcp_server_versions WHERE tenant_id=? AND server_id=? AND configuration_hash=?").bind(actor.tenant_id).bind(id).bind(&hash).fetch_optional(&mut *tx).await?;
    let target = if let Some(version) = existing {
        version
    } else {
        let next = current
            .try_get::<u64, _>("current_version_number")?
            .checked_add(1)
            .ok_or_else(|| ApiError::internal("MCP server version exhausted"))?;
        sqlx::query("INSERT INTO mcp_server_versions(id,tenant_id,server_id,version_number,transport,endpoint,credential_id,runtime_sandbox_profile_id,runtime_sandbox_profile_version_id,configuration_json,configuration_hash,created_by) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(next).bind(frozen.kind).bind(frozen.endpoint)
            .bind(frozen.credential_id).bind(frozen.runtime_sandbox_profile_id).bind(frozen.runtime_sandbox_profile_version_id)
            .bind(json!({"transport":input.transport,"options":input.configuration})).bind(hash).bind(actor.user_id)
            .execute(&mut *tx).await?;
        next
    };
    let changed=sqlx::query("UPDATE mcp_servers SET name=?,description=?,status=?,current_version_number=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?").bind(required_name(&input.name)?).bind(input.description).bind(input.status).bind(target).bind(actor.tenant_id).bind(id).bind(input.version).execute(&mut *tx).await.map_err(map_name_error)?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "MCP_SERVER_VERSION_CONFLICT",
            "MCP server changed",
        ));
    }
    tx.commit().await?;
    Ok(Json(load_server(&state, actor.tenant_id, id).await?))
}

async fn list_tools(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<ToolResponse>>> {
    actor.require("mcp:view")?;
    require_server(&state, &actor, id).await?;
    Ok(Json(load_tools(&state, actor.tenant_id, Some(id)).await?))
}
async fn list_all_tools(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<ToolResponse>>> {
    actor.require("mcp:view")?;
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let needle = query.search.unwrap_or_default().trim().to_lowercase();
    let status = query.status.unwrap_or_default();
    let mut visible = Vec::new();
    for tool in load_tools(&state, actor.tenant_id, None).await? {
        if !administrator
            && require_server(&state, &actor, tool.server_id)
                .await
                .is_err()
        {
            continue;
        }
        if !status.is_empty() && tool.availability != status {
            continue;
        }
        if !needle.is_empty()
            && !tool.name.to_lowercase().contains(&needle)
            && !tool
                .title
                .as_deref()
                .is_some_and(|value| value.to_lowercase().contains(&needle))
        {
            continue;
        }
        visible.push(tool);
    }
    let spec = PageRequest {
        page: query.page,
        page_size: query.page_size,
    }
    .normalized();
    let total = visible.len() as u64;
    let start = (spec.offset() as usize).min(visible.len());
    Ok(Json(PageResponse {
        items: visible
            .into_iter()
            .skip(start)
            .take(spec.page_size as usize)
            .collect(),
        page: spec.page,
        page_size: spec.page_size,
        total,
    }))
}

async fn test_connection(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<HealthResponse>> {
    actor.require("mcp:manage")?;
    require_server(&state, &actor, id).await?;
    let config = load_config(&state, actor.tenant_id, id).await?;
    let checked_at = OffsetDateTime::now_utc();
    if let RuntimeMcpTransportV2::StreamableHttp { endpoint }
    | RuntimeMcpTransportV2::Sse { endpoint } = &config.transport
    {
        let check = crate::model_api::execute_runtime_resource_check_with_reference(
            &state,
            actor.tenant_id,
            endpoint.clone(),
            config.credential.clone(),
            RuntimeResourceProbeV1::McpInitialize,
        )
        .await?;
        record_health(
            &state,
            &actor,
            id,
            &check.status,
            check.latency_ms,
            check.error_code.as_deref(),
            check.error_message.as_deref(),
        )
        .await?;
        return Ok(Json(HealthResponse {
            status: check.status,
            latency_ms: Some(check.latency_ms),
            error_code: check.error_code,
            error_message: check.error_message,
            checked_at,
        }));
    }
    let check = crate::model_api::execute_runtime_resource_operation(
        &state,
        actor.tenant_id,
        config.version_id,
        config.transport,
        config.credential,
        config.runtime_sandbox_profile,
        30,
        RuntimeResourceOperationV1::McpInitialize,
    )
    .await?;
    record_health(&state, &actor, id, "healthy", check.duration_ms, None, None).await?;
    Ok(Json(HealthResponse {
        status: "healthy".into(),
        latency_ms: Some(check.duration_ms),
        error_code: None,
        error_message: None,
        checked_at,
    }))
}
async fn discover_tools(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<DiscoveryResponse>> {
    actor.require("mcp:discover")?;
    require_server(&state, &actor, id).await?;
    let config = load_config(&state, actor.tenant_id, id).await?;
    let run_id = Uuid::now_v7();
    sqlx::query("INSERT INTO mcp_discovery_runs(id,tenant_id,server_id,server_version_id,status,started_by) VALUES(?,?,?,?,'running',?)").bind(run_id).bind(actor.tenant_id).bind(id).bind(config.version_id).bind(actor.user_id).execute(&state.pool).await?;
    let discovered = match crate::model_api::execute_runtime_resource_operation(
        &state,
        actor.tenant_id,
        config.version_id,
        config.transport.clone(),
        config.credential.clone(),
        config.runtime_sandbox_profile.clone(),
        30,
        RuntimeResourceOperationV1::McpDiscover,
    )
    .await
    .and_then(|response| remote_tools_from_result(response.result))
    {
        Ok(value) => value,
        Err(error) => {
            sqlx::query("UPDATE mcp_discovery_runs SET status='failed',error_code='MCP_DISCOVERY_FAILED',error_message=?,finished_at=UTC_TIMESTAMP(6) WHERE id=?").bind(format!("{error:?}").chars().take(512).collect::<String>()).bind(run_id).execute(&state.pool).await?;
            return Err(error);
        }
    };
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE mcp_tools SET availability='unavailable' WHERE tenant_id=? AND server_id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    for tool in &discovered {
        upsert_tool(&mut tx, &actor, id, config.version_id, run_id, tool).await?;
    }
    sqlx::query("UPDATE mcp_discovery_runs SET status='succeeded',discovered_count=?,finished_at=UTC_TIMESTAMP(6) WHERE id=?").bind(discovered.len() as u32).bind(run_id).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE mcp_servers SET last_discovered_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let tools = load_tools(&state, actor.tenant_id, Some(id)).await?;
    Ok(Json(DiscoveryResponse {
        run_id,
        discovered_count: tools.len().try_into().unwrap_or(u32::MAX),
        tools,
    }))
}

async fn update_tool_policy(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<PolicyRequest>,
) -> ApiResult<Json<ToolResponse>> {
    actor.require("mcp:manage")?;
    require_tool(&state, &actor, id).await?;
    if !(1..=120).contains(&input.timeout_seconds)
        || !matches!(
            input.side_effect.as_str(),
            "unknown" | "none" | "read_only" | "idempotent" | "non_idempotent" | "irreversible"
        )
    {
        return Err(ApiError::bad_request(
            "INVALID_MCP_POLICY",
            "MCP tool policy is invalid",
        ));
    }
    let changed=sqlx::query("UPDATE mcp_tool_policies SET enabled=?,debug_enabled=?,timeout_seconds=?,side_effect=?,version=version+1,updated_by=? WHERE tenant_id=? AND tool_id=? AND version=?").bind(input.enabled).bind(input.debug_enabled).bind(input.timeout_seconds).bind(input.side_effect).bind(actor.user_id).bind(actor.tenant_id).bind(id).bind(input.version).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "MCP_TOOL_POLICY_CONFLICT",
            "MCP tool policy changed",
        ));
    }
    Ok(Json(load_tool(&state, actor.tenant_id, id).await?))
}
async fn debug_invoke(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<DebugRequest>,
) -> ApiResult<Json<DebugResponse>> {
    actor.require("mcp:debug")?;
    require_tool(&state, &actor, id).await?;
    let tool = load_tool(&state, actor.tenant_id, id).await?;
    if !tool.enabled || !tool.debug_enabled || tool.availability != "available" {
        return Err(ApiError::unprocessable(
            "MCP_DEBUG_DISABLED",
            "MCP tool debugging is disabled",
        ));
    }
    if input.expected_tool_version_id != tool.current_version_id {
        return Err(ApiError::conflict(
            "MCP_TOOL_VERSION_CONFLICT",
            "MCP tool schema changed",
        ));
    }
    if !input.confirmed {
        return Err(ApiError::bad_request(
            "MCP_CONFIRMATION_REQUIRED",
            "MCP tool invocation requires confirmation",
        ));
    }
    if matches!(
        tool.side_effect.as_str(),
        "unknown" | "non_idempotent" | "irreversible"
    ) && input.confirmation_text.as_deref() != Some(tool.name.as_str())
    {
        return Err(ApiError::bad_request(
            "MCP_CONFIRMATION_TEXT_INVALID",
            "Type the tool name to confirm this invocation",
        ));
    }
    validate_arguments(&tool.input_schema, &input.arguments)?;
    let config = load_config_for_tool(&state, actor.tenant_id, id).await?;
    let response = crate::model_api::execute_runtime_resource_operation(
        &state,
        actor.tenant_id,
        config.version_id,
        config.transport,
        config.credential,
        config.runtime_sandbox_profile,
        tool.timeout_seconds,
        RuntimeResourceOperationV1::McpCall {
            tool_name: tool.name,
            arguments: input.arguments,
        },
    )
    .await?;
    Ok(Json(DebugResponse {
        result: response.result,
        duration_ms: response.duration_ms,
        tool_version_id: tool.current_version_id,
    }))
}

async fn delete_server(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("mcp:manage")?;
    require_server(&state, &actor, id).await?;
    let mut tx = crate::workflow_revision_resources::begin_delete(
        &state.pool,
        actor.tenant_id,
        "mcp_server",
        id,
    )
    .await?;
    let references:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM workflow_draft_resources WHERE tenant_id=? AND resource_type IN ('mcp_server','mcp_tool') AND (resource_id=? OR resource_id IN (SELECT id FROM mcp_tools WHERE tenant_id=? AND server_id=?)))+(SELECT COUNT(*) FROM workflow_version_resources WHERE tenant_id=? AND resource_type IN ('mcp_server','mcp_tool') AND (resource_id=? OR resource_id IN (SELECT id FROM mcp_tools WHERE tenant_id=? AND server_id=?)))").bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if references != 0 {
        return Err(ApiError::conflict(
            "MCP_SERVER_REFERENCED",
            "MCP server or tool is referenced",
        ));
    }

    let tools = sqlx::query("SELECT id FROM mcp_tools WHERE tenant_id=? AND server_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
    for row in tools {
        let tool: Uuid = row.try_get("id")?;
        sqlx::query("DELETE FROM mcp_tool_policies WHERE tenant_id=? AND tool_id=?")
            .bind(actor.tenant_id)
            .bind(tool)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM mcp_tool_versions WHERE tenant_id=? AND tool_id=?")
            .bind(actor.tenant_id)
            .bind(tool)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM mcp_tools WHERE tenant_id=? AND server_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM mcp_discovery_runs WHERE tenant_id=? AND server_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM mcp_server_versions WHERE tenant_id=? AND server_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM mcp_servers WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn remote_tools_from_result(value: Value) -> ApiResult<Vec<RemoteTool>> {
    let tools = value
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiError::unavailable(
                "MCP_TOOLS_INVALID",
                "MCP server returned an invalid tools list",
            )
        })?;
    if tools.len() > 500 {
        return Err(ApiError::unprocessable(
            "MCP_TOOL_LIMIT",
            "MCP server exposes more than 500 tools",
        ));
    }
    tools.iter().map(remote_tool).collect()
}

async fn upsert_tool(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    actor: &Actor,
    server: Uuid,
    server_version: Uuid,
    run: Uuid,
    tool: &RemoteTool,
) -> ApiResult<()> {
    let hash=format!("{:x}",Sha256::digest(serde_json::to_vec(&json!({"serverVersionId":server_version,"input":tool.input_schema,"output":tool.output_schema,"annotations":tool.annotations})).map_err(ApiError::internal)?));
    let existing=sqlx::query("SELECT id,current_version_number FROM mcp_tools WHERE tenant_id=? AND server_id=? AND name=? FOR UPDATE").bind(actor.tenant_id).bind(server).bind(&tool.name).fetch_optional(&mut **tx).await?;
    let (id, current) = if let Some(row) = existing {
        (row.try_get("id")?, row.try_get("current_version_number")?)
    } else {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO mcp_tools(id,tenant_id,server_id,name,title,description,current_version_number) VALUES(?,?,?,?,?,?,1)").bind(id).bind(actor.tenant_id).bind(server).bind(&tool.name).bind(&tool.title).bind(&tool.description).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO mcp_tool_policies(tenant_id,tool_id,updated_by) VALUES(?,?,?)")
            .bind(actor.tenant_id)
            .bind(id)
            .bind(actor.user_id)
            .execute(&mut **tx)
            .await?;
        (id, 1)
    };
    let known:Option<u64>=sqlx::query_scalar("SELECT version_number FROM mcp_tool_versions WHERE tenant_id=? AND tool_id=? AND schema_hash=?").bind(actor.tenant_id).bind(id).bind(&hash).fetch_optional(&mut **tx).await?;
    let version = known.unwrap_or_else(|| if current == 1 { 1 } else { current + 1 });
    if known.is_none() {
        sqlx::query("INSERT INTO mcp_tool_versions(id,tenant_id,tool_id,discovery_run_id,server_version_id,version_number,input_schema,output_schema,annotations_json,schema_hash) VALUES(?,?,?,?,?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(run).bind(server_version).bind(version).bind(&tool.input_schema).bind(&tool.output_schema).bind(&tool.annotations).bind(hash).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE mcp_tools SET title=?,description=?,availability='available',current_version_number=?,version=version+1,last_seen_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=?").bind(&tool.title).bind(&tool.description).bind(version).bind(actor.tenant_id).bind(id).execute(&mut **tx).await?;
    Ok(())
}
fn remote_tool(value: &Value) -> ApiResult<RemoteTool> {
    Ok(RemoteTool {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::unavailable("MCP_TOOL_INVALID", "MCP tool name is missing"))?
            .to_owned(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        input_schema: value
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object"})),
        output_schema: value.get("outputSchema").cloned(),
        annotations: value
            .get("annotations")
            .cloned()
            .unwrap_or_else(|| json!({})),
    })
}
async fn load_config(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<ServerConfig> {
    let version_id: Uuid = sqlx::query_scalar("SELECT sv.id FROM mcp_servers s JOIN mcp_server_versions sv ON sv.tenant_id=s.tenant_id AND sv.server_id=s.id AND sv.version_number=s.current_version_number WHERE s.tenant_id=? AND s.id=? AND s.status='active'")
        .bind(tenant)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("MCP server"))?;
    let mut reference = ResourceReference {
        binding_role: None,
        resource_type: ResourceType::McpServer,
        resource_id: id,
        resource_version_id: Some(version_id),
        operation: ResourceOperation::Use,
    };
    let snapshot =
        crate::workflow_resources::resource_snapshot(state, tenant, &mut reference).await?;
    let binding = binding_from_snapshot(reference, snapshot)?;
    let RuntimeResourceConfigurationV1::Mcp {
        transport,
        credential,
        ..
    } = binding.configuration
    else {
        return Err(ApiError::unprocessable(
            "MCP_CONFIGURATION_INVALID",
            "MCP Server Version has an invalid Runtime binding",
        ));
    };
    let runtime_sandbox_profile = if let RuntimeMcpTransportV2::Stdio {
        runtime_sandbox, ..
    } = &transport
    {
        let mut reference = ResourceReference {
            binding_role: None,
            resource_type: ResourceType::SandboxProfile,
            resource_id: runtime_sandbox.resource_id,
            resource_version_id: Some(runtime_sandbox.resource_version_id),
            operation: ResourceOperation::Use,
        };
        let snapshot =
            crate::workflow_resources::resource_snapshot(state, tenant, &mut reference).await?;
        Some(binding_from_snapshot(reference, snapshot)?)
    } else {
        None
    };
    Ok(ServerConfig {
        version_id,
        transport,
        credential,
        runtime_sandbox_profile,
    })
}

fn binding_from_snapshot(
    reference: ResourceReference,
    snapshot: Value,
) -> ApiResult<RuntimeResourceBindingV1> {
    let snapshot_hash = agentx_runtime_contracts::content_hash(&snapshot)
        .map_err(ApiError::internal)?
        .to_string();
    crate::runtime_resource_binding::from_snapshot(&ResourceVersionSnapshot {
        node_id: "mcp-control-diagnostic".into(),
        reference,
        snapshot_hash,
        snapshot,
    })
    .map_err(ApiError::internal)
}
async fn load_config_for_tool(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<ServerConfig> {
    let server: Uuid =
        sqlx::query_scalar("SELECT server_id FROM mcp_tools WHERE tenant_id=? AND id=?")
            .bind(tenant)
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("MCP tool"))?;
    load_config(state, tenant, server).await
}
async fn load_server(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<ServerResponse> {
    let row=sqlx::query("SELECT s.id,s.name,s.description,s.owner_department_id,s.status,s.current_version_number,s.version,s.last_discovered_at,s.updated_at,sv.transport,sv.endpoint,sv.credential_id,sv.configuration_json,(SELECT COUNT(*) FROM mcp_tools t WHERE t.tenant_id=s.tenant_id AND t.server_id=s.id AND t.availability='available') tool_count FROM mcp_servers s JOIN mcp_server_versions sv ON sv.tenant_id=s.tenant_id AND sv.server_id=s.id AND sv.version_number=s.current_version_number WHERE s.tenant_id=? AND s.id=?").bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("MCP server"))?;
    Ok(server_from_row(row)?)
}
async fn load_tools(
    state: &ControlApiState,
    tenant: Uuid,
    server: Option<Uuid>,
) -> ApiResult<Vec<ToolResponse>> {
    let rows = if let Some(server) = server {
        sqlx::query(tool_select!(
            "WHERE t.tenant_id=? AND t.server_id=? ORDER BY t.name"
        ))
        .bind(tenant)
        .bind(server)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query(tool_select!("WHERE t.tenant_id=? ORDER BY s.name,t.name"))
            .bind(tenant)
            .fetch_all(&state.pool)
            .await?
    };
    Ok(rows
        .into_iter()
        .map(tool_from_row)
        .collect::<Result<_, _>>()?)
}
async fn load_tool(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<ToolResponse> {
    let row = sqlx::query(tool_select!("WHERE t.tenant_id=? AND t.id=?"))
        .bind(tenant)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("MCP tool"))?;
    Ok(tool_from_row(row)?)
}
fn server_from_row(row: MySqlRow) -> Result<ServerResponse, sqlx::Error> {
    let configuration: Value = row.try_get("configuration_json")?;
    let transport = serde_json::from_value::<McpTransportInput>(
        configuration
            .get("transport")
            .cloned()
            .unwrap_or(Value::Null),
    )
    .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    Ok(ServerResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        owner_department_id: row.try_get("owner_department_id")?,
        transport,
        status: row.try_get("status")?,
        current_version_number: row.try_get("current_version_number")?,
        version: row.try_get("version")?,
        tool_count: row
            .try_get::<i64, _>("tool_count")?
            .try_into()
            .unwrap_or_default(),
        last_discovered_at: row.try_get("last_discovered_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn tool_from_row(row: MySqlRow) -> Result<ToolResponse, sqlx::Error> {
    Ok(ToolResponse {
        id: row.try_get("id")?,
        server_id: row.try_get("server_id")?,
        name: row.try_get("name")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        availability: row.try_get("availability")?,
        current_version_id: row.try_get("current_version_id")?,
        current_version_number: row.try_get("current_version_number")?,
        input_schema: row.try_get("input_schema")?,
        output_schema: row.try_get("output_schema")?,
        annotations: row.try_get("annotations_json")?,
        schema_hash: row.try_get("schema_hash")?,
        enabled: row.try_get("enabled")?,
        debug_enabled: row.try_get("debug_enabled")?,
        timeout_seconds: row.try_get("timeout_seconds")?,
        side_effect: row.try_get("side_effect")?,
        version: row.try_get("policy_version")?,
    })
}
async fn require_server(state: &ControlApiState, actor: &Actor, id: Uuid) -> ApiResult<()> {
    let visible: bool = if actor.roles.iter().any(|role| role == "company_admin") {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE tenant_id=? AND id=?)")
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_one(&state.pool)
            .await?
    } else {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mcp_servers s JOIN department_closure dc ON dc.tenant_id=s.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=s.owner_department_id WHERE s.tenant_id=? AND s.id=?)").bind(actor.department_id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?
    };
    if visible {
        Ok(())
    } else {
        Err(ApiError::not_found("MCP server"))
    }
}
async fn require_tool(state: &ControlApiState, actor: &Actor, id: Uuid) -> ApiResult<()> {
    let server: Uuid =
        sqlx::query_scalar("SELECT server_id FROM mcp_tools WHERE tenant_id=? AND id=?")
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("MCP tool"))?;
    require_server(state, actor, server).await
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
async fn record_health(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
    status: &str,
    latency: u64,
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> ApiResult<()> {
    let next:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(check_sequence),0)+1 AS UNSIGNED) FROM resource_health_checks WHERE tenant_id=? AND resource_type='mcp_server' AND resource_id=?").bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    sqlx::query("INSERT INTO resource_health_checks(id,tenant_id,resource_type,resource_id,check_sequence,status,latency_ms,error_code,error_message,checked_by) VALUES(?,?,'mcp_server',?,?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(next).bind(status).bind(latency).bind(error_code).bind(error_message).bind(actor.user_id).execute(&state.pool).await?;
    Ok(())
}
struct FrozenMcpTransport {
    kind: &'static str,
    endpoint: Option<String>,
    credential_id: Option<Uuid>,
    runtime_sandbox_profile_id: Option<Uuid>,
    runtime_sandbox_profile_version_id: Option<Uuid>,
}

async fn validate_transport(
    state: &ControlApiState,
    tenant_id: Uuid,
    transport: &McpTransportInput,
) -> ApiResult<FrozenMcpTransport> {
    match transport {
        McpTransportInput::StreamableHttp {
            endpoint,
            bearer_credential_id,
        }
        | McpTransportInput::Sse {
            endpoint,
            bearer_credential_id,
        } => {
            validate_http_endpoint(endpoint)?;
            require_credential(state, tenant_id, *bearer_credential_id).await?;
            Ok(FrozenMcpTransport {
                kind: if matches!(transport, McpTransportInput::StreamableHttp { .. }) {
                    "streamable_http"
                } else {
                    "sse"
                },
                endpoint: Some(endpoint.clone()),
                credential_id: *bearer_credential_id,
                runtime_sandbox_profile_id: None,
                runtime_sandbox_profile_version_id: None,
            })
        }
        McpTransportInput::Stdio {
            command,
            args,
            environment_credential_refs,
            runtime_sandbox,
        } => {
            if command.is_empty()
                || !command.starts_with('/')
                || command.contains(['\n', '\r', '\0'])
                || args.iter().any(|arg| arg.contains(['\n', '\r', '\0']))
                || args.len() > 128
            {
                return Err(ApiError::bad_request(
                    "INVALID_MCP_STDIO_COMMAND",
                    "stdio MCP command and args are invalid",
                ));
            }
            let mut names = std::collections::BTreeSet::new();
            for reference in environment_credential_refs {
                if !valid_environment_name(&reference.name) || !names.insert(&reference.name) {
                    return Err(ApiError::bad_request(
                        "INVALID_MCP_STDIO_ENVIRONMENT",
                        "stdio MCP environment Credential names must be unique safe names",
                    ));
                }
                require_credential(state, tenant_id, Some(reference.credential_id)).await?;
            }
            let valid_sandbox: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sandbox_profile_versions v JOIN sandbox_profiles p ON p.tenant_id=v.tenant_id AND p.id=v.profile_id WHERE v.tenant_id=? AND v.profile_id=? AND v.id=? AND p.status='active')",
            )
            .bind(tenant_id)
            .bind(runtime_sandbox.resource_id)
            .bind(runtime_sandbox.resource_version_id)
            .fetch_one(&state.pool)
            .await?;
            if !valid_sandbox {
                return Err(ApiError::unprocessable(
                    "MCP_STDIO_SANDBOX_REQUIRED",
                    "stdio MCP requires an active exact Runtime Sandbox version",
                ));
            }
            Ok(FrozenMcpTransport {
                kind: "stdio",
                endpoint: None,
                credential_id: None,
                runtime_sandbox_profile_id: Some(runtime_sandbox.resource_id),
                runtime_sandbox_profile_version_id: Some(runtime_sandbox.resource_version_id),
            })
        }
    }
}

async fn ensure_transport_dependencies_authorized(
    state: &ControlApiState,
    tenant_id: Uuid,
    owner_department_id: Uuid,
    transport: &McpTransportInput,
) -> ApiResult<()> {
    match transport {
        McpTransportInput::StreamableHttp {
            bearer_credential_id,
            ..
        }
        | McpTransportInput::Sse {
            bearer_credential_id,
            ..
        } => {
            if let Some(credential_id) = bearer_credential_id {
                crate::resource_api::ensure_department_resource_authorized(
                    state,
                    tenant_id,
                    owner_department_id,
                    "credential",
                    *credential_id,
                    None,
                )
                .await?;
            }
        }
        McpTransportInput::Stdio {
            environment_credential_refs,
            runtime_sandbox,
            ..
        } => {
            for reference in environment_credential_refs {
                crate::resource_api::ensure_department_resource_authorized(
                    state,
                    tenant_id,
                    owner_department_id,
                    "credential",
                    reference.credential_id,
                    None,
                )
                .await?;
            }
            crate::resource_api::ensure_department_resource_authorized(
                state,
                tenant_id,
                owner_department_id,
                "sandbox_profile",
                runtime_sandbox.resource_id,
                Some(runtime_sandbox.resource_version_id),
            )
            .await?;
        }
    }
    Ok(())
}

fn validate_http_endpoint(endpoint: &str) -> ApiResult<()> {
    let url = Url::parse(endpoint)
        .map_err(|_| ApiError::bad_request("INVALID_ENDPOINT", "MCP endpoint is invalid"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(
            "INVALID_ENDPOINT",
            "MCP endpoint must use HTTP or HTTPS",
        ));
    }
    if url
        .host_str()
        .and_then(|host| {
            host.trim_matches(&['[', ']'][..])
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(blocked_literal_address)
    {
        return Err(ApiError::forbidden(
            "MCP endpoint targets a blocked network address",
        ));
    }
    Ok(())
}

fn validate_configuration(configuration: &Value) -> ApiResult<()> {
    if !configuration.is_object() {
        return Err(ApiError::bad_request(
            "INVALID_MCP_CONFIGURATION",
            "MCP configuration must be an object",
        ));
    }
    Ok(())
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| matches!(byte, b'A'..=b'Z' | b'0'..=b'9' | b'_'))
        && name.len() <= 128
}

fn blocked_literal_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(address) => {
            address.is_loopback()
                || address.is_private()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_unspecified()
                || address.is_multicast()
        }
        std::net::IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local()
                || address.is_multicast()
        }
    }
}
fn config_hash(transport: &McpTransportInput, configuration: &Value) -> ApiResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({"transport":transport,"configuration":configuration}))
                .map_err(ApiError::internal)?
        )
    ))
}
fn validate_arguments(schema: &Value, args: &Value) -> ApiResult<()> {
    let object = args.as_object().ok_or_else(|| {
        ApiError::bad_request(
            "MCP_ARGUMENTS_INVALID",
            "MCP tool arguments must be an object",
        )
    })?;
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for key in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(key) {
                return Err(ApiError::bad_request(
                    "MCP_ARGUMENTS_INVALID",
                    format!("Required argument '{key}' is missing"),
                ));
            }
        }
    }
    Ok(())
}
fn map_name_error(error: sqlx::Error) -> ApiError {
    match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => ApiError::conflict(
            "MCP_SERVER_NAME_EXISTS",
            "An MCP server with this name already exists",
        ),
        _ => ApiError::from(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_validation_rejects_literal_internal_networks() {
        for endpoint in [
            "http://169.254.169.254/latest/meta-data",
            "http://127.0.0.1:8090/mcp",
            "http://10.0.0.8/mcp",
            "http://[::1]/mcp",
        ] {
            assert!(validate_http_endpoint(endpoint).is_err());
        }
        assert!(
            validate_http_endpoint("http://echo-mcp.agentx-deps.svc.cluster.local:8090/mcp")
                .is_ok()
        );
    }
    #[test]
    fn validates_mcp_contract() {
        assert!(validate_http_endpoint("http://mcp.test/mcp").is_ok());
        assert!(validate_http_endpoint("stdio://mcp.test").is_err());
        assert!(validate_configuration(&json!({})).is_ok());
        assert!(validate_configuration(&json!([])).is_err());
        assert!(validate_arguments(&json!({"required":["text"]}), &json!({"text":"ok"})).is_ok());
    }
}
