use std::collections::{BTreeSet, HashMap};

use agentx_runtime_contracts::{
    ApplyReceiptV1, ContentHash, DELEGATION_TOKEN_TTL_SECONDS, ExecutionCheckpointV1,
    ExecutionCollectionPageV1, ExecutionCommandV1, ExecutionDetailV1, ExecutionEventPageV1,
    ExecutionNodeV1, ExecutionRuntimeDetailsV1, ExecutionSearchPageV1, ExecutionSearchRequestV1,
    ExecutionSessionModeV1, PartialExecutionModeV1, RuntimeCommandApplyRequestV1,
    SideEffectResolutionV1, content_hash, issue_delegation_token, now_unix,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::Response,
    routing::{get, post},
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{MySql, QueryBuilder, Row};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState, execution_origin},
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/executions", get(search_executions))
        .route("/api/v1/executions/{id}", get(get_execution))
        .route("/api/v1/executions/{id}/nodes", get(get_nodes))
        .route("/api/v1/executions/{id}/nodes/{node_id}", get(get_node))
        .route("/api/v1/executions/{id}/events", get(get_events))
        .route("/api/v1/executions/{id}/checkpoints", get(get_checkpoints))
        .route(
            "/api/v1/executions/{id}/runtime-details",
            get(get_runtime_details),
        )
        .route(
            "/api/v1/executions/{id}/artifacts/{artifact_id}",
            get(get_artifact),
        )
        .route(
            "/api/v1/executions/{id}/plugin-manifests/{node_type}/versions/{version}",
            get(get_execution_plugin_manifest),
        )
        .route("/api/v1/executions/{id}/cancel", post(cancel_execution))
        .route("/api/v1/executions/{id}/fork", post(fork_execution))
        .route(
            "/api/v1/executions/{id}/side-effect-confirmations",
            post(confirm_side_effect),
        )
        .route("/api/v1/executions/{id}/trace", get(get_trace))
        .route(
            "/api/v1/executions/{id}/trace/spans/{span_id}",
            get(get_trace_span),
        )
        .route("/api/v1/agent-sessions", get(search_agent_sessions))
        .route(
            "/api/v1/agent-sessions/{session_key}/{node_key}",
            get(get_agent_session),
        )
        .route("/api/v1/agent-sessions/clear", post(clear_agent_session))
        .route(
            "/api/v1/agent-subject-memory/audit",
            post(search_agent_subject_memory),
        )
        .route(
            "/api/v1/agent-subject-memory/clear",
            post(clear_agent_subject_memory),
        )
        .route("/api/v1/deliveries", get(search_deliveries))
        .route("/api/v1/deliveries/{delivery_id}", get(get_delivery))
        .route(
            "/api/v1/deliveries/{delivery_id}/retry",
            post(retry_delivery),
        )
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSessionListQuery {
    application_id: Option<Uuid>,
    session_key: Option<String>,
    stable_agent_node_key: Option<String>,
    limit: Option<u32>,
    after: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutionPluginManifestQuery {
    bundle_digest: String,
}

async fn get_execution_plugin_manifest(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, node_type, version)): Path<(Uuid, String, u32)>,
    Query(query): Query<ExecutionPluginManifestQuery>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:view")?;
    let detail = runtime_execution_detail(&state, &actor, id).await?;
    let (manifest, manifest_hash) = crate::canvas_plugin_api::plugin_manifest_for_execution_bundle(
        &state,
        actor.tenant_id,
        detail.summary.bundle_id,
        &node_type,
        version,
        &query.bundle_digest,
    )
    .await?;
    Ok(Json(json!({
        "nodeType":node_type,
        "version":version,
        "manifestHash":manifest_hash,
        "manifest":manifest,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSessionClearInput {
    session_key: String,
    stable_agent_node_key: String,
    idempotency_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSubjectMemoryInput {
    application_id: Uuid,
    memory_resource_version_id: Uuid,
    limit: Option<u32>,
    after: Option<String>,
    idempotency_key: Option<String>,
}

async fn search_agent_sessions(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<AgentSessionListQuery>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:view")?;
    let (tenant_wide, application_ids, workflow_ids) =
        execution_query_scope(&state, &actor).await?;
    if let Some(application_id) = query.application_id
        && !tenant_wide
        && !application_ids.contains(&application_id)
    {
        return Err(ApiError::forbidden(
            "The application is outside your execution scope",
        ));
    }
    let request = agentx_runtime_contracts::AgentSessionSearchRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        application_id: query.application_id,
        session_key: query.session_key,
        stable_agent_node_key: query.stable_agent_node_key,
        limit: query.limit.unwrap_or(50).clamp(1, 100),
        after: query.after,
    };
    let request_hash = content_hash(&json!({"operation":"agent-session-search","request":request}))
        .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.query.agent_sessions",
        application_ids,
        workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/query/agent-sessions:search",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json::<Value>(response).await.map(Json)
}

async fn get_agent_session(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((session_key, node_key)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:view")?;
    let (tenant_wide, application_ids, workflow_ids) =
        execution_query_scope(&state, &actor).await?;
    let request_hash = content_hash(&json!({
        "operation":"agent-session-detail",
        "sessionKey":session_key,
        "stableAgentNodeKey":node_key,
    }))
    .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.query.agent_sessions",
        application_ids,
        workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )?;
    let response = state
        .http
        .get(format!(
            "{}/internal/runtime/v1/query/agent-sessions/{}/{}",
            state.runtime_query_url, session_key, node_key
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json::<Value>(response).await.map(Json)
}

async fn clear_agent_session(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<AgentSessionClearInput>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:view")?;
    let (tenant_wide, application_ids, workflow_ids) =
        execution_query_scope(&state, &actor).await?;
    let request = agentx_runtime_contracts::AgentSessionClearRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        session_key: input.session_key,
        stable_agent_node_key: input.stable_agent_node_key,
        idempotency_key: input.idempotency_key,
    };
    let request_hash = content_hash(&json!({"operation":"agent-session-clear","request":request}))
        .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.sessions.clear",
        application_ids,
        workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/agent-sessions:clear",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json::<Value>(response).await.map(Json)
}

async fn search_agent_subject_memory(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<AgentSubjectMemoryInput>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:view")?;
    let (tenant_wide, application_ids, workflow_ids) =
        execution_query_scope(&state, &actor).await?;
    if !tenant_wide && !application_ids.contains(&input.application_id) {
        return Err(ApiError::forbidden(
            "The application is outside your execution scope",
        ));
    }
    let request = agentx_runtime_contracts::AgentSubjectMemorySearchRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        application_id: input.application_id,
        memory_resource_version_id: input.memory_resource_version_id,
        limit: input.limit.unwrap_or(50).clamp(1, 100),
        after: input.after,
    };
    let request_hash =
        content_hash(&json!({"operation":"agent-subject-memory-search","request":request}))
            .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.query.agent_subject_memory",
        application_ids,
        workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/query/agent-subject-memory:search",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json::<Value>(response).await.map(Json)
}

async fn clear_agent_subject_memory(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<AgentSubjectMemoryInput>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:view")?;
    let (tenant_wide, application_ids, workflow_ids) =
        execution_query_scope(&state, &actor).await?;
    if !tenant_wide && !application_ids.contains(&input.application_id) {
        return Err(ApiError::forbidden(
            "The application is outside your execution scope",
        ));
    }
    let request = agentx_runtime_contracts::AgentSubjectMemoryClearRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        application_id: input.application_id,
        memory_resource_version_id: input.memory_resource_version_id,
        idempotency_key: input
            .idempotency_key
            .unwrap_or_else(|| format!("web:{}", Uuid::now_v7())),
    };
    let request_hash =
        content_hash(&json!({"operation":"agent-subject-memory-clear","request":request}))
            .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.memory.clear",
        application_ids,
        workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/agent-subject-memory:clear",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json::<Value>(response).await.map(Json)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionListQuery {
    limit: Option<u32>,
    application_ids: Option<String>,
    workflow_ids: Option<String>,
    tool_ids: Option<String>,
    initiator_user_ids: Option<String>,
    initiator_department_ids: Option<String>,
    trigger_types: Option<String>,
    trigger_name: Option<String>,
    statuses: Option<String>,
    session_mode: Option<String>,
    created_after: Option<String>,
    created_before: Option<String>,
    search: Option<String>,
    cursor: Option<String>,
}

async fn search_executions(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ExecutionListQuery>,
) -> ApiResult<Json<Value>> {
    let limit = query.limit.unwrap_or(8);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request(
            "INVALID_LIMIT",
            "limit must be between 1 and 100",
        ));
    }
    let application_ids = parse_uuid_filter(query.application_ids.as_deref(), "applicationIds")?;
    let can_view_executions = actor
        .permissions
        .iter()
        .any(|permission| permission == "execution:view");
    let can_query_invokable_applications = actor
        .permissions
        .iter()
        .any(|permission| permission == "application:invoke")
        && !application_ids.is_empty()
        && query.session_mode.as_deref() == Some("stateless");
    if !can_view_executions && !can_query_invokable_applications {
        return Err(ApiError::forbidden("Missing permission execution:view"));
    }
    let workflow_ids = parse_uuid_filter(query.workflow_ids.as_deref(), "workflowIds")?;
    let tool_ids = parse_uuid_filter(query.tool_ids.as_deref(), "toolIds")?;
    let initiator_user_ids =
        parse_uuid_filter(query.initiator_user_ids.as_deref(), "initiatorUserIds")?;
    let initiator_department_ids = parse_uuid_filter(
        query.initiator_department_ids.as_deref(),
        "initiatorDepartmentIds",
    )?;
    let trigger_types = parse_enum_filter(
        query.trigger_types.as_deref(),
        "triggerTypes",
        &[
            "user",
            "api_key",
            "webhook",
            "schedule",
            "debug",
            "evaluation",
            "fork",
            "composite",
        ],
    )?;
    let statuses = parse_enum_filter(
        query.statuses.as_deref(),
        "statuses",
        &[
            "created",
            "queued",
            "running",
            "waiting_approval",
            "suspended",
            "succeeded",
            "failed",
            "cancelled",
            "timed_out",
        ],
    )?;
    let session_mode = match query.session_mode.as_deref().unwrap_or("all") {
        "all" => ExecutionSessionModeV1::All,
        "stateless" => ExecutionSessionModeV1::Stateless,
        "session" => ExecutionSessionModeV1::Session,
        _ => {
            return Err(ApiError::bad_request(
                "INVALID_SESSION_MODE",
                "sessionMode must be all, stateless, or session",
            ));
        }
    };
    let trigger_name = validate_filter_text(query.trigger_name, "triggerName")?;
    let search = validate_filter_text(query.search, "search")?;
    let created_after = parse_filter_time(query.created_after.as_deref(), "createdAfter")?;
    let created_before = parse_filter_time(query.created_before.as_deref(), "createdBefore")?;
    if created_after
        .zip(created_before)
        .is_some_and(|(after, before)| after > before)
    {
        return Err(ApiError::bad_request(
            "INVALID_TIME_RANGE",
            "createdAfter must not be later than createdBefore",
        ));
    }
    let request = ExecutionSearchRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        application_ids,
        workflow_ids,
        tool_ids,
        initiator_user_ids,
        initiator_department_ids,
        trigger_types,
        trigger_name,
        statuses,
        session_mode,
        created_after,
        created_before,
        search,
        cursor: query.cursor,
        limit,
    };
    let request_hash = content_hash(&json!({"operation":"execution-search","request":request}))
        .map_err(ApiError::internal)?;
    let (tenant_wide, authorized_application_ids, authorized_workflow_ids) =
        execution_query_scope(&state, &actor).await?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.query.executions",
        authorized_application_ids,
        authorized_workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/query/executions:search",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    let page: ExecutionSearchPageV1 = runtime_json(response).await?;
    let items = summaries_json(&state, actor.tenant_id, page.items).await?;
    Ok(Json(json!({
        "items": items,
        "limit": limit,
        "total": page.total,
        "nextCursor": page.next,
        "snapshotId": page.snapshot_id,
    })))
}

async fn execution_query_scope(
    state: &ControlApiState,
    actor: &Actor,
) -> ApiResult<(bool, BTreeSet<Uuid>, BTreeSet<Uuid>)> {
    let tenant_wide: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_roles ur JOIN roles r ON r.tenant_id=ur.tenant_id AND r.id=ur.role_id AND r.status='active' JOIN role_permissions rp ON rp.tenant_id=r.tenant_id AND rp.role_id=r.id JOIN permissions p ON p.id=rp.permission_id AND p.permission_key='execution:view' WHERE ur.tenant_id=? AND ur.user_id=? AND r.data_scope='company')")
        .bind(actor.tenant_id)
        .bind(actor.user_id)
        .fetch_one(&state.pool)
        .await?;
    if tenant_wide {
        return Ok((true, BTreeSet::new(), BTreeSet::new()));
    }

    let application_ids = sqlx::query_scalar::<_, Uuid>("SELECT a.id FROM applications a WHERE a.tenant_id=? AND (a.owner_user_id=? OR a.visibility='company' OR (a.visibility='department' AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=a.tenant_id AND dc.ancestor_id=a.owner_department_id AND dc.descendant_id=?))) ORDER BY a.id")
        .bind(actor.tenant_id)
        .bind(actor.user_id)
        .bind(actor.department_id)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .collect();
    let workflow_ids = sqlx::query_scalar::<_, Uuid>("SELECT w.id FROM workflows w WHERE w.tenant_id=? AND (w.owner_user_id=? OR EXISTS(SELECT 1 FROM workflow_members wm WHERE wm.tenant_id=w.tenant_id AND wm.workflow_id=w.id AND wm.user_id=?)) ORDER BY w.id")
        .bind(actor.tenant_id)
        .bind(actor.user_id)
        .bind(actor.user_id)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .collect();
    Ok((false, application_ids, workflow_ids))
}

async fn get_execution(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    if !actor
        .permissions
        .iter()
        .any(|permission| matches!(permission.as_str(), "execution:view" | "application:invoke"))
    {
        return Err(ApiError::forbidden("Missing permission execution:view"));
    }
    let detail = runtime_execution_detail(&state, &actor, id).await?;
    let mut response = summary_json(&state, actor.tenant_id, detail.summary).await?;
    let object = response
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("Execution response is not an object"))?;
    object.insert(
        "parentExecutionId".into(),
        json!(detail.parent_execution_id),
    );
    object.insert("input".into(), detail.input.unwrap_or(Value::Null));
    object.insert("output".into(), detail.output.unwrap_or(Value::Null));
    object.insert("error".into(), detail.error.unwrap_or(Value::Null));
    object.insert("stateVersion".into(), json!(detail.state_version));
    object.insert("admissionEpoch".into(), json!(detail.admission_epoch));
    object.insert("traceWatermark".into(), json!(detail.trace_watermark));
    Ok(Json(response))
}

async fn get_nodes(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    if !actor
        .permissions
        .iter()
        .any(|permission| matches!(permission.as_str(), "execution:view" | "application:invoke"))
    {
        return Err(ApiError::forbidden("Missing permission execution:view"));
    }
    let page: ExecutionCollectionPageV1<ExecutionNodeV1> = runtime_execution_get(
        &state,
        &actor,
        id,
        "execution_nodes",
        &format!("/internal/runtime/v1/query/executions/{id}/nodes"),
    )
    .await?;
    Ok(Json(json!({
        "items": page.items.into_iter().map(|node| node_json(id, node)).collect::<Vec<_>>()
    })))
}

async fn get_node(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, node_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:view")?;
    let node: ExecutionNodeV1 = runtime_execution_get(
        &state,
        &actor,
        id,
        "execution_node",
        &format!("/internal/runtime/v1/query/executions/{id}/nodes/{node_id}"),
    )
    .await?;
    Ok(Json(node_json(id, node)))
}

async fn get_events(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(query): Query<ExecutionEventQuery>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:view")?;
    let after = query.after.unwrap_or_default();
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let page: ExecutionEventPageV1 = runtime_execution_get(
        &state,
        &actor,
        id,
        "execution_events",
        &format!("/internal/runtime/v1/query/executions/{id}/events?after={after}&limit={limit}"),
    )
    .await?;
    Ok(Json(
        json!({"items":page.items,"nextCursor":page.next_cursor}),
    ))
}

#[derive(Default, Deserialize)]
struct ExecutionEventQuery {
    after: Option<u64>,
    limit: Option<u32>,
}

async fn get_checkpoints(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:view")?;
    let page: ExecutionCollectionPageV1<ExecutionCheckpointV1> = runtime_execution_get(
        &state,
        &actor,
        id,
        "execution_checkpoints",
        &format!("/internal/runtime/v1/query/executions/{id}/checkpoints"),
    )
    .await?;
    let items = page
        .items
        .into_iter()
        .map(|checkpoint| json!({
            "id":checkpoint.checkpoint_id,"executionId":id,
            "nodeExecutionId":checkpoint.node_execution_id,
            "sequenceNumber":checkpoint.sequence_number,"checkpointType":checkpoint.checkpoint_type,
            "stateHash":checkpoint.state_hash,"activationCount":0,"deliveryCount":0,
            "createdAt":rfc3339(checkpoint.created_at)
        }))
        .collect::<Vec<_>>();
    Ok(Json(json!({"items":items})))
}

async fn get_runtime_details(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    actor.require("trace:view")?;
    let details: ExecutionRuntimeDetailsV1 = runtime_execution_get(
        &state,
        &actor,
        id,
        "execution_runtime_details",
        &format!("/internal/runtime/v1/query/executions/{id}/runtime-details"),
    )
    .await?;
    let input_tokens = details
        .calls
        .iter()
        .map(|call| call.input_tokens)
        .sum::<u64>();
    let output_tokens = details
        .calls
        .iter()
        .map(|call| call.output_tokens)
        .sum::<u64>();
    let cost_micros = details
        .calls
        .iter()
        .map(|call| call.cost_micros)
        .sum::<u64>();
    let cost_currencies = details
        .calls
        .iter()
        .filter_map(|call| call.cost_currency.as_deref())
        .collect::<BTreeSet<_>>();
    let cost_currency = (cost_currencies.len() == 1)
        .then(|| cost_currencies.first().copied())
        .flatten();
    Ok(Json(json!({
        "executionId":id,"inputTokens":input_tokens,"outputTokens":output_tokens,
        "costMicros":cost_micros,"costCurrency":cost_currency,"agentRuns":details.agent_runs,"iterations":[],
        "calls":details.calls,"sandboxes":details.sandboxes,"attempts":details.attempts
    })))
}

async fn get_artifact(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, artifact_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Response> {
    if !actor
        .permissions
        .iter()
        .any(|permission| matches!(permission.as_str(), "trace:view" | "application:invoke"))
    {
        return Err(ApiError::forbidden("Missing permission trace:view"));
    }
    let path = format!("/internal/runtime/v1/query/executions/{id}/artifacts/{artifact_id}");
    let request_hash = content_hash(&json!({
        "operation":"execution_artifact","executionId":id
    }))
    .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.query.execution",
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::from([id]),
        false,
        request_hash,
    )?;
    let upstream = state
        .http
        .get(format!("{}{}", state.runtime_query_url, path))
        .bearer_auth(token)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    if !upstream.status().is_success() {
        return Err(runtime_response_error(upstream).await);
    }
    let status = StatusCode::from_u16(upstream.status().as_u16()).map_err(ApiError::internal)?;
    let mut builder = Response::builder().status(status);
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_DISPOSITION,
        header::ETAG,
    ] {
        if let Some(value) = upstream.headers().get(&name) {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(ApiError::internal)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForkRequest {
    checkpoint_id: Uuid,
    mode: String,
    node_id: Option<String>,
    side_effect_decisions: Value,
    idempotency_key: Option<String>,
}

async fn cancel_execution(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:cancel")?;
    let detail = runtime_execution_detail(&state, &actor, id).await?;
    let request = RuntimeCommandApplyRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        command_id: Uuid::now_v7(),
        object_version: detail.state_version,
        idempotency_key: format!("bff:cancel:{id}:{}", detail.state_version),
        command: ExecutionCommandV1::Cancel {
            execution_id: id,
            expected_state_version: detail.state_version,
        },
    };
    apply_runtime_command(&state, &actor, id, &request).await?;
    Ok(Json(
        json!({"executionId":id,"status":"accepted","replayed":false}),
    ))
}

async fn fork_execution(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<ForkRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("execution:fork")?;
    let detail = runtime_execution_detail(&state, &actor, id).await?;
    crate::canvas_plugin_api::require_bundle_plugins_enabled(
        &state,
        actor.tenant_id,
        detail.summary.bundle_id,
    )
    .await?;
    let mode = match input.mode.as_str() {
        "whole" => PartialExecutionModeV1::Whole,
        "node" => PartialExecutionModeV1::Node,
        "to_node" => PartialExecutionModeV1::ToNode,
        "from_node" => PartialExecutionModeV1::FromNode,
        _ => {
            return Err(ApiError::bad_request(
                "INVALID_FORK_MODE",
                "Fork mode is invalid",
            ));
        }
    };
    let resolution = side_effect_resolution(&input.side_effect_decisions)?;
    let command_id = Uuid::now_v7();
    let request = RuntimeCommandApplyRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        command_id,
        object_version: detail.state_version,
        idempotency_key: input
            .idempotency_key
            .unwrap_or_else(|| format!("bff:fork:{id}:{command_id}")),
        command: ExecutionCommandV1::Fork {
            source_execution_id: id,
            checkpoint_id: input.checkpoint_id,
            origin: execution_origin(&state, &actor).await?,
            mode,
            node_id: input.node_id,
            side_effect_resolution: resolution,
        },
    };
    let receipt = apply_runtime_command(&state, &actor, id, &request).await?;
    let fork_execution_id =
        agentx_runtime_contracts::deterministic_uuid(receipt.event_id, b"fork-execution");
    Ok((
        StatusCode::ACCEPTED,
        Json(
            json!({"executionId":fork_execution_id,"status":"accepted","replayed":receipt.replayed}),
        ),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SideEffectRequest {
    node_execution_id: Uuid,
    decision: String,
    idempotency_key: String,
}

async fn confirm_side_effect(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<SideEffectRequest>,
) -> ApiResult<Json<Value>> {
    actor.require("execution:fork")?;
    let detail = runtime_execution_detail(&state, &actor, id).await?;
    let resolution = parse_resolution(&input.decision)?;
    let request = RuntimeCommandApplyRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        command_id: Uuid::now_v7(),
        object_version: detail.state_version,
        idempotency_key: input.idempotency_key,
        command: ExecutionCommandV1::SideEffectConfirmation {
            execution_id: id,
            node_execution_id: input.node_execution_id,
            resolution,
            expected_state_version: detail.state_version,
        },
    };
    apply_runtime_command(&state, &actor, id, &request).await?;
    Ok(Json(json!({"accepted":true,"replayed":false})))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TraceQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    node_execution_id: Option<Uuid>,
}

async fn get_trace(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(query): Query<TraceQuery>,
) -> ApiResult<Json<Value>> {
    if !actor
        .permissions
        .iter()
        .any(|permission| matches!(permission.as_str(), "trace:view" | "application:invoke"))
    {
        return Err(ApiError::forbidden("Missing permission trace:view"));
    }
    let detail = runtime_execution_detail(&state, &actor, id).await?;
    let request_hash = content_hash(&json!({
        "operation":"execution-trace","executionId":id,
        "expectedWatermark":detail.trace_watermark,"limit":query.limit.unwrap_or(200),
        "cursor":query.cursor,"nodeExecutionId":query.node_execution_id
    }))
    .map_err(ApiError::internal)?;
    let token = delegation_token_with_audience(
        &state,
        &actor,
        "agentx-observability-query",
        "observability.trace.read",
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::from([id]),
        false,
        request_hash.clone(),
    )?;
    let cursor_query = query
        .cursor
        .as_deref()
        .map(|cursor| format!("&cursor={cursor}"))
        .unwrap_or_default();
    let node_execution_query = query
        .node_execution_id
        .map(|node_execution_id| format!("&nodeExecutionId={node_execution_id}"))
        .unwrap_or_default();
    let response = state
        .http
        .get(format!(
            "{}/internal/observability/v1/executions/{id}/trace?expectedWatermark={}&limit={}{}{}",
            state.observability_query_url,
            detail.trace_watermark,
            query.limit.unwrap_or(200).clamp(1, 1000),
            cursor_query,
            node_execution_query
        ))
        .header("x-agentx-request-hash", request_hash.as_str())
        .bearer_auth(token)
        .send()
        .await
        .map_err(observability_unavailable)?;
    if response.status() == reqwest::StatusCode::ACCEPTED {
        return Err(ApiError::accepted(
            "TRACE_DELAYED",
            "Execution is complete but its trace has not reached the expected watermark",
            2,
        ));
    }
    let trace: agentx_runtime_contracts::ExecutionTraceV1 = observability_json(response).await?;
    Ok(Json(json!({
        "executionId":id,"traceId":detail.summary.trace_id,"spans":trace.spans,
        "nextCursor":trace.next,"complete":trace.complete,"degraded":trace.degraded,
        "warningCode":trace.warning_code,"totalSpans":trace.total_spans,
        "expectedWatermark":trace.expected_watermark,"ingestedWatermark":trace.ingested_watermark
    })))
}

async fn get_trace_span(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, span_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    if !actor
        .permissions
        .iter()
        .any(|permission| matches!(permission.as_str(), "trace:view" | "application:invoke"))
    {
        return Err(ApiError::forbidden("Missing permission trace:view"));
    }
    let detail = runtime_execution_detail(&state, &actor, id).await?;
    let request_hash = content_hash(&json!({
        "operation":"trace-span-detail","executionId":id,"spanId":span_id
    }))
    .map_err(ApiError::internal)?;
    let token = delegation_token_with_audience(
        &state,
        &actor,
        "agentx-observability-query",
        "observability.trace.read",
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::from([id]),
        false,
        request_hash.clone(),
    )?;
    let response = state
        .http
        .get(format!(
            "{}/internal/observability/v1/executions/{id}/trace/spans/{span_id}",
            state.observability_query_url
        ))
        .header("x-agentx-request-hash", request_hash.as_str())
        .bearer_auth(token)
        .send()
        .await
        .map_err(observability_unavailable)?;
    let span: agentx_runtime_contracts::TraceSpanDetailV1 = observability_json(response).await?;
    Ok(Json(json!({
        "executionId":id,"traceId":detail.summary.trace_id,"span":span.span,
        "contents":span.contents,"events":span.events
    })))
}

async fn runtime_execution_detail(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<ExecutionDetailV1> {
    let request_hash = content_hash(&json!({"operation":"get_execution","executionId":id}))
        .map_err(ApiError::internal)?;
    let token = delegation_token(
        state,
        actor,
        "runtime.query.execution",
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::from([id]),
        false,
        request_hash,
    )?;
    let response = state
        .http
        .get(format!(
            "{}/internal/runtime/v1/query/executions/{id}",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json(response).await
}

async fn runtime_execution_get<T: serde::de::DeserializeOwned>(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
    operation: &str,
    path: &str,
) -> ApiResult<T> {
    let request_hash = content_hash(&json!({"operation":operation,"executionId":id}))
        .map_err(ApiError::internal)?;
    let token = delegation_token(
        state,
        actor,
        "runtime.query.execution",
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::from([id]),
        false,
        request_hash,
    )?;
    let response = state
        .http
        .get(format!("{}{}", state.runtime_query_url, path))
        .bearer_auth(token)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json(response).await
}

async fn apply_runtime_command(
    state: &ControlApiState,
    actor: &Actor,
    execution_id: Uuid,
    request: &RuntimeCommandApplyRequestV1,
) -> ApiResult<ApplyReceiptV1> {
    let request_hash = content_hash(&json!({"operation":"runtime-command","request":request}))
        .map_err(ApiError::internal)?;
    let token = delegation_token(
        state,
        actor,
        "runtime.command.execution",
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::from([execution_id]),
        false,
        request_hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/runtime-commands:apply",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(request)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json(response).await
}

#[allow(clippy::too_many_arguments)]
fn delegation_token(
    state: &ControlApiState,
    actor: &Actor,
    scope: &str,
    application_ids: BTreeSet<Uuid>,
    workflow_ids: BTreeSet<Uuid>,
    execution_ids: BTreeSet<Uuid>,
    tenant_wide: bool,
    request_hash: ContentHash,
) -> ApiResult<String> {
    delegation_token_with_audience(
        state,
        actor,
        "agentx-runtime-internal",
        scope,
        application_ids,
        workflow_ids,
        execution_ids,
        tenant_wide,
        request_hash,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn mint_observability_token(
    state: &ControlApiState,
    actor: &Actor,
    scope: &str,
    request_hash: agentx_runtime_contracts::ContentHash,
) -> ApiResult<String> {
    let (tenant_wide, application_ids, workflow_ids) = execution_query_scope(state, actor).await?;
    delegation_token_with_audience(
        state,
        actor,
        "agentx-observability-query",
        scope,
        application_ids,
        workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )
}

#[allow(clippy::too_many_arguments)]
fn delegation_token_with_audience(
    state: &ControlApiState,
    actor: &Actor,
    audience: &str,
    scope: &str,
    application_ids: BTreeSet<Uuid>,
    workflow_ids: BTreeSet<Uuid>,
    execution_ids: BTreeSet<Uuid>,
    tenant_wide: bool,
    request_hash: ContentHash,
) -> ApiResult<String> {
    let now = now_unix();
    issue_delegation_token(
        &state.delegation_kid,
        state.delegation_key.expose_secret().as_bytes(),
        &agentx_runtime_contracts::DelegationClaimsV1 {
            iss: "agentx-control".into(),
            aud: audience.into(),
            sub: actor.user_id,
            tenant_id: actor.tenant_id,
            token_version: actor.token_version,
            tenant_wide,
            scope: BTreeSet::from([scope.into()]),
            application_ids,
            workflow_ids,
            execution_ids,
            session_ids: BTreeSet::new(),
            request_hash,
            iat: now,
            exp: now + DELEGATION_TOKEN_TTL_SECONDS,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)
}

async fn runtime_json<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> ApiResult<T> {
    if !response.status().is_success() {
        return Err(runtime_response_error(response).await);
    }
    response.json().await.map_err(ApiError::internal)
}

async fn runtime_response_error(response: reqwest::Response) -> ApiError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    tracing::warn!(%status, response_body=%body, "Runtime Query rejected BFF request");
    if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        ApiError::unavailable("RUNTIME_QUERY_UNAVAILABLE", "Runtime Query is unavailable")
    } else if status == reqwest::StatusCode::GONE {
        ApiError::bad_request(
            "QUERY_CURSOR_EXPIRED",
            "Execution results expired and must be refreshed",
        )
    } else if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
        && body.contains("QUERY_BUDGET_EXCEEDED")
    {
        ApiError::unprocessable(
            "QUERY_BUDGET_EXCEEDED",
            "Narrow the execution filters to at most 10000 matching rows",
        )
    } else if status == reqwest::StatusCode::BAD_REQUEST {
        ApiError::bad_request(
            "INVALID_EXECUTION_FILTER",
            "Runtime rejected the execution filter",
        )
    } else if status == reqwest::StatusCode::NOT_FOUND {
        ApiError::not_found("Runtime object")
    } else {
        ApiError::forbidden("Runtime rejected the delegated request")
    }
}

async fn observability_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> ApiResult<T> {
    if !response.status().is_success() {
        tracing::warn!(status=%response.status(), "Observability rejected BFF request");
        return Err(ApiError::unavailable(
            "OBSERVABILITY_UNAVAILABLE",
            "Observability query is unavailable",
        ));
    }
    response.json().await.map_err(ApiError::internal)
}

fn runtime_unavailable(error: reqwest::Error) -> ApiError {
    tracing::warn!(%error, "Runtime Query is unavailable");
    ApiError::unavailable("RUNTIME_QUERY_UNAVAILABLE", "Runtime Query is unavailable")
}

fn observability_unavailable(error: reqwest::Error) -> ApiError {
    tracing::warn!(%error, "Observability Query is unavailable");
    ApiError::unavailable(
        "OBSERVABILITY_UNAVAILABLE",
        "Observability query is unavailable",
    )
}

async fn summary_json(
    state: &ControlApiState,
    tenant_id: Uuid,
    summary: agentx_runtime_contracts::ExecutionSummaryV1,
) -> ApiResult<Value> {
    summaries_json(state, tenant_id, vec![summary])
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("Execution response is missing"))
}

async fn summaries_json(
    state: &ControlApiState,
    tenant_id: Uuid,
    summaries: Vec<agentx_runtime_contracts::ExecutionSummaryV1>,
) -> ApiResult<Vec<Value>> {
    if summaries.is_empty() {
        return Ok(Vec::new());
    }
    let mut workflow_versions = summaries
        .iter()
        .map(|summary| summary.workflow_version_id)
        .collect::<Vec<_>>();
    workflow_versions.sort_unstable();
    workflow_versions.dedup();
    let mut workflow_query = QueryBuilder::<MySql>::new(
        "SELECT w.id workflow_id,w.name,wv.id workflow_version_id,wv.version_number FROM workflow_versions wv JOIN workflows w ON w.tenant_id=wv.tenant_id AND w.id=wv.workflow_id WHERE w.tenant_id=",
    );
    workflow_query.push_bind(tenant_id).push(" AND wv.id IN (");
    {
        let mut separated = workflow_query.separated(",");
        for id in workflow_versions {
            separated.push_bind(id);
        }
    }
    workflow_query.push(")");
    let workflow_rows = workflow_query.build().fetch_all(&state.pool).await?;
    let workflow_metadata = workflow_rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<Uuid, _>("workflow_version_id")?,
                (
                    row.try_get::<String, _>("name")?,
                    row.try_get::<Option<u64>, _>("version_number")?,
                ),
            ))
        })
        .collect::<Result<HashMap<_, _>, sqlx::Error>>()?;

    let mut application_ids = summaries
        .iter()
        .filter_map(|summary| summary.application_id)
        .collect::<Vec<_>>();
    application_ids.sort_unstable();
    application_ids.dedup();
    let mut application_metadata: HashMap<Uuid, String> = HashMap::new();
    if !application_ids.is_empty() {
        let mut application_query =
            QueryBuilder::<MySql>::new("SELECT id,name FROM applications WHERE tenant_id=");
        application_query.push_bind(tenant_id).push(" AND id IN (");
        {
            let mut separated = application_query.separated(",");
            for id in application_ids {
                separated.push_bind(id);
            }
        }
        application_query.push(")");
        for row in application_query.build().fetch_all(&state.pool).await? {
            application_metadata.insert(
                row.try_get::<Uuid, _>("id")?,
                row.try_get::<String, _>("name")?,
            );
        }
    }

    Ok(summaries
        .into_iter()
        .map(|summary| {
            let (workflow_name, version) = workflow_metadata
                .get(&summary.workflow_version_id)
                .cloned()
                .unwrap_or_else(|| (summary.workflow_id.to_string(), None));
            let application_name = summary
                .application_id
                .and_then(|id| application_metadata.get(&id).cloned());
            execution_summary_json(summary, workflow_name, version, application_name)
        })
        .collect())
}

fn execution_summary_json(
    summary: agentx_runtime_contracts::ExecutionSummaryV1,
    workflow_name: String,
    version: Option<u64>,
    application_name: Option<String>,
) -> Value {
    json!({
        "id":summary.execution_id,"workflowId":summary.workflow_id,"workflowName":workflow_name,
        "applicationId":summary.application_id,"applicationName":application_name,
        "workflowVersionId":summary.workflow_version_id,"workflowVersionNumber":version,
        "invocationId":summary.invocation_id,"sessionId":summary.session_id,"traceId":summary.trace_id,
        "triggerType":summary.trigger_type,"executionType":"production","parentExecutionId":summary.parent_execution_id,
        "initiatorUserId":summary.initiator_user_id,"initiatorUserName":summary.initiator_user_name,
        "initiatorDepartmentId":summary.initiator_department_id,"initiatorDepartmentName":summary.initiator_department_name,
        "triggerSourceId":summary.trigger_source_id,"triggerName":summary.trigger_name,
        "callerExecutionId":null,"forkCheckpointId":null,"status":summary.status,
        "startedAt":rfc3339(summary.created_at),"endedAt":summary.completed_at.and_then(rfc3339),"durationMs":summary.duration_ms,
        "costMicros":summary.cost_micros,"costCurrency":summary.cost_currency,"inputTokens":summary.input_tokens,"outputTokens":summary.output_tokens,
        "errorCode":summary.error_code,"errorMessage":null,"bundleId":summary.bundle_id
    })
}

fn parse_uuid_filter(value: Option<&str>, field: &str) -> ApiResult<Vec<Uuid>> {
    parse_filter_values(value, field)?
        .into_iter()
        .map(|value| {
            Uuid::parse_str(&value).map_err(|_| {
                ApiError::bad_request(
                    "INVALID_EXECUTION_FILTER",
                    format!("{field} contains an invalid UUID"),
                )
            })
        })
        .collect()
}

fn parse_enum_filter(value: Option<&str>, field: &str, allowed: &[&str]) -> ApiResult<Vec<String>> {
    let values = parse_filter_values(value, field)?;
    if let Some(value) = values
        .iter()
        .find(|value| !allowed.contains(&value.as_str()))
    {
        return Err(ApiError::bad_request(
            "INVALID_EXECUTION_FILTER",
            format!("{field} contains unsupported value {value}"),
        ));
    }
    Ok(values)
}

fn parse_filter_values(value: Option<&str>, field: &str) -> ApiResult<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut values = value
        .split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if values.iter().any(String::is_empty) {
        return Err(ApiError::bad_request(
            "INVALID_EXECUTION_FILTER",
            format!("{field} contains an empty value"),
        ));
    }
    values.sort();
    values.dedup();
    if values.len() > 50 {
        return Err(ApiError::bad_request(
            "TOO_MANY_FILTER_VALUES",
            format!("{field} supports at most 50 values"),
        ));
    }
    Ok(values)
}

fn validate_filter_text(value: Option<String>, field: &str) -> ApiResult<Option<String>> {
    let value = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if value
        .as_ref()
        .is_some_and(|value| value.chars().count() > 200)
    {
        return Err(ApiError::bad_request(
            "FILTER_TEXT_TOO_LONG",
            format!("{field} supports at most 200 characters"),
        ));
    }
    Ok(value)
}

fn parse_filter_time(value: Option<&str>, field: &str) -> ApiResult<Option<OffsetDateTime>> {
    value
        .map(|value| {
            OffsetDateTime::parse(value, &Rfc3339).map_err(|_| {
                ApiError::bad_request(
                    "INVALID_EXECUTION_FILTER",
                    format!("{field} must be an RFC 3339 timestamp"),
                )
            })
        })
        .transpose()
}

fn rfc3339(value: OffsetDateTime) -> Option<String> {
    value.format(&Rfc3339).ok()
}

fn node_json(execution_id: Uuid, node: ExecutionNodeV1) -> Value {
    json!({
        "id":node.node_execution_id,"executionId":execution_id,"nodeId":node.node_id,
        "nodeName":node.node_name,"nodeType":node.node_type,"nodeVersion":node.node_version,
        "generation":0,"activationSlot":0,"runIndex":node.run_index,
        "iterationIndex":node.iteration_index,"status":node.status,"capability":node.capability,
        "sideEffectLevel":"none","input":node.input,"output":node.output,
        "errorCode":node.error_code,"errorMessage":node.error_message,"startedAt":node.started_at.and_then(rfc3339),
        "endedAt":node.ended_at.and_then(rfc3339),"costMicros":node.cost_micros,
        "costCurrency":node.cost_currency,"attempts":[],"lineage":[]
    })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use agentx_runtime_contracts::{ExecutionNodeV1, ExecutionSummaryV1};
    use serde_json::json;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};
    use uuid::Uuid;

    use super::{
        execution_summary_json, node_json, parse_enum_filter, parse_filter_time,
        parse_filter_values, parse_uuid_filter, rfc3339, validate_filter_text,
    };

    #[test]
    fn execution_filter_parameters_are_normalized_and_strictly_validated() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        assert_eq!(
            parse_uuid_filter(
                Some(&format!("{second}, {first},{second}")),
                "applicationIds"
            )
            .unwrap(),
            vec![first, second]
        );
        assert_eq!(
            parse_enum_filter(
                Some("schedule, user,schedule"),
                "triggerTypes",
                &["user", "schedule"]
            )
            .unwrap(),
            vec!["schedule", "user"]
        );
        assert!(parse_uuid_filter(Some("not-a-uuid"), "toolIds").is_err());
        assert!(parse_enum_filter(Some("unknown"), "statuses", &["running", "succeeded"]).is_err());
        assert!(parse_filter_values(Some("user,,schedule"), "triggerTypes").is_err());
        assert!(
            parse_filter_values(
                Some(
                    &(0..51)
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                "statuses"
            )
            .is_err()
        );
        assert!(validate_filter_text(Some("x".repeat(201)), "search").is_err());
        assert!(parse_filter_time(Some("2026-99-99"), "createdAfter").is_err());
    }

    #[test]
    fn execution_timestamps_are_explicit_rfc3339_strings() {
        let timestamp = OffsetDateTime::parse("2026-08-20T10:11:12.123456Z", &Rfc3339).unwrap();
        let rendered = rfc3339(timestamp).unwrap();
        assert_eq!(
            OffsetDateTime::parse(&rendered, &Rfc3339).unwrap(),
            timestamp
        );
    }

    #[test]
    fn execution_and_node_json_keep_authoritative_metrics_and_timestamps() {
        let started = OffsetDateTime::parse("2026-08-20T10:11:12Z", &Rfc3339).unwrap();
        let ended = started + time::Duration::seconds(2);
        let execution_id = Uuid::now_v7();
        let execution = execution_summary_json(
            ExecutionSummaryV1 {
                execution_id,
                invocation_id: None,
                application_id: None,
                workflow_id: Uuid::now_v7(),
                workflow_version_id: Uuid::now_v7(),
                session_id: None,
                parent_execution_id: None,
                bundle_id: Uuid::now_v7(),
                trace_id: Uuid::now_v7(),
                trigger_type: "manual".into(),
                initiator_user_id: None,
                initiator_user_name: None,
                initiator_department_id: None,
                initiator_department_name: None,
                trigger_source_id: None,
                trigger_name: None,
                status: "succeeded".into(),
                duration_ms: Some(2_000),
                cost_micros: 99,
                cost_currency: Some("USD".into()),
                input_tokens: 11,
                output_tokens: 17,
                error_code: None,
                created_at: started,
                completed_at: Some(ended),
            },
            "Trace fixture".into(),
            Some(3),
            None,
        );
        assert_eq!(execution["inputTokens"], 11);
        assert_eq!(execution["outputTokens"], 17);
        assert_eq!(execution["startedAt"], "2026-08-20T10:11:12Z");
        assert_eq!(execution["endedAt"], "2026-08-20T10:11:14Z");

        let node = node_json(
            execution_id,
            ExecutionNodeV1 {
                node_execution_id: Uuid::now_v7(),
                node_id: "model".into(),
                node_name: "Model".into(),
                node_type: "model".into(),
                node_version: 1,
                run_index: 2,
                iteration_index: 4,
                status: "succeeded".into(),
                capability: "model".into(),
                input: Some(json!({"main":[]})),
                output: Some(json!({"main":[{"json":{"text":"hello"}}]})),
                error_code: None,
                error_message: None,
                cost_micros: 99,
                cost_currency: Some("USD".into()),
                started_at: Some(started),
                ended_at: Some(ended),
            },
        );
        assert_eq!(node["startedAt"], "2026-08-20T10:11:12Z");
        assert_eq!(node["endedAt"], "2026-08-20T10:11:14Z");
        assert_eq!(node["runIndex"], 2);
        assert_eq!(node["iterationIndex"], 4);
        assert_eq!(node["costMicros"], 99);
        assert_eq!(node["costCurrency"], "USD");
        assert_eq!(node["output"]["main"][0]["json"]["text"], "hello");
    }
}

fn parse_resolution(value: &str) -> ApiResult<SideEffectResolutionV1> {
    match value {
        "execute" => Ok(SideEffectResolutionV1::Execute),
        "reuse_output" => Ok(SideEffectResolutionV1::ReuseOutput),
        "dry_run" => Ok(SideEffectResolutionV1::DryRun),
        _ => Err(ApiError::bad_request(
            "INVALID_SIDE_EFFECT_RESOLUTION",
            "Side-effect resolution is invalid",
        )),
    }
}

fn side_effect_resolution(value: &Value) -> ApiResult<SideEffectResolutionV1> {
    value
        .as_str()
        .or_else(|| value.get("default").and_then(Value::as_str))
        .map(parse_resolution)
        .transpose()
        .map(|value| value.unwrap_or(SideEffectResolutionV1::Execute))
}

#[derive(Deserialize)]
struct DeliveryListQuery {
    application_id: Option<Uuid>,
    invocation_id: Option<Uuid>,
    execution_id: Option<Uuid>,
    status: Option<String>,
    limit: Option<u32>,
}

/// Delivery records (plan7 P7-A): proxies the runtime delivery outbox and
/// dead letters for the application detail "投递记录" view.
async fn search_deliveries(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<DeliveryListQuery>,
) -> ApiResult<Json<Value>> {
    actor.require("application:view")?;
    if let Some(status) = query.status.as_deref() {
        if !matches!(
            status,
            "pending" | "delivering" | "delivered" | "failed" | "dead"
        ) {
            return Err(ApiError::bad_request(
                "INVALID_DELIVERY_STATUS",
                "status must be pending, delivering, delivered, failed, or dead",
            ));
        }
    }
    let (tenant_wide, application_ids, workflow_ids) =
        execution_query_scope(&state, &actor).await?;
    let request = json!({
        "apiVersion": 1,
        "tenantId": actor.tenant_id,
        "applicationId": query.application_id,
        "invocationId": query.invocation_id,
        "executionId": query.execution_id,
        "status": query.status,
        "limit": query.limit.unwrap_or(50).clamp(1, 100),
    });
    let request_hash = content_hash(&json!({"operation":"delivery-search","request":request}))
        .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.query.deliveries",
        application_ids,
        workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/query/deliveries:search",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json::<Value>(response).await.map(Json)
}

async fn get_delivery(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(delivery_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    actor.require("application:view")?;
    let (tenant_wide, application_ids, workflow_ids) =
        execution_query_scope(&state, &actor).await?;
    let request_hash = content_hash(&json!({
        "operation":"delivery-detail",
        "deliveryId":delivery_id,
    }))
    .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.query.deliveries",
        application_ids,
        workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )?;
    let response = state
        .http
        .get(format!(
            "{}/internal/runtime/v1/query/deliveries/{}",
            state.runtime_query_url, delivery_id
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json::<Value>(response).await.map(Json)
}

async fn retry_delivery(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(delivery_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    actor.require("application:manage")?;
    let (tenant_wide, application_ids, workflow_ids) =
        execution_query_scope(&state, &actor).await?;
    let request_hash = content_hash(&json!({
        "operation":"delivery-retry",
        "deliveryId":delivery_id,
    }))
    .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.delivery.retry",
        application_ids,
        workflow_ids,
        BTreeSet::new(),
        tenant_wide,
        request_hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/query/deliveries/{}/retry",
            state.runtime_query_url, delivery_id
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({"apiVersion":1,"tenantId":actor.tenant_id}))
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json::<Value>(response).await.map(Json)
}
