use agentx_runtime_contracts::{
    ContentHash, DelegationClaimsV1, ExecutionCheckpointV1, ExecutionCollectionPageV1,
    ExecutionDetailV1, ExecutionEventPageV1, ExecutionEventV1, ExecutionNodeV1,
    ExecutionRuntimeDetailsV1, ExecutionSearchPageV1, ExecutionSearchRequestV1, ExecutionSummaryV1,
    InvocationDetailV1, InvocationSearchPageV1, InvocationSearchRequestV1, InvocationSummaryV1,
    NodeAttemptV1, RuntimeCallDetailV1, content_hash,
};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use object_store::path::Path as ObjectPath;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

const QUERY_SNAPSHOT_LIMIT: usize = 10_000;
const QUERY_SNAPSHOT_TTL_SECONDS: i64 = 15 * 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotCursor {
    snapshot_id: Uuid,
    filter_hash: ContentHash,
    snapshot_upper_bound: String,
    offset: u64,
    issued_at: i64,
}

pub async fn search_executions(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<ExecutionSearchRequestV1>,
) -> RuntimeResult<Json<ExecutionSearchPageV1>> {
    validate_page_limit(request.limit)?;
    validate_execution_search(&request)?;
    let request_hash = query_request_hash("execution-search", &request)?;
    let claims = authorize_delegation(
        &state,
        &headers,
        request.tenant_id,
        "runtime.query.executions",
        &request_hash,
    )
    .await?;
    authorize_list_scope(
        &state,
        &claims,
        &request.application_ids,
        &request.workflow_ids,
    )
    .await?;
    let filter_hash = execution_filter_hash(&request)?;
    let (snapshot_id, upper_bound, total, offset) = if let Some(cursor) = &request.cursor {
        resume_snapshot(
            &state,
            cursor,
            request.tenant_id,
            claims.sub,
            "execution",
            &filter_hash,
        )
        .await?
    } else {
        create_execution_snapshot(&state, &claims, &request, &filter_hash).await?
    };
    let (items, next) = execution_snapshot_page(
        &state,
        snapshot_id,
        &filter_hash,
        &upper_bound,
        offset,
        request.limit,
        total,
    )
    .await?;
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(ExecutionSearchPageV1 {
        api_version: 1,
        snapshot_id,
        snapshot_upper_bound: upper_bound,
        total,
        items,
        next,
    }))
}

pub async fn search_invocations(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<InvocationSearchRequestV1>,
) -> RuntimeResult<Json<InvocationSearchPageV1>> {
    validate_page_limit(request.limit)?;
    let request_hash = query_request_hash("invocation-search", &request)?;
    let claims = authorize_delegation(
        &state,
        &headers,
        request.tenant_id,
        "runtime.query.invocations",
        &request_hash,
    )
    .await?;
    authorize_list_scope(&state, &claims, &request.application_ids, &[]).await?;
    let filter_hash = invocation_filter_hash(&request)?;
    let (snapshot_id, upper_bound, total, offset) = if let Some(cursor) = &request.cursor {
        resume_snapshot(
            &state,
            cursor,
            request.tenant_id,
            claims.sub,
            "invocation",
            &filter_hash,
        )
        .await?
    } else {
        create_invocation_snapshot(&state, &claims, &request, &filter_hash).await?
    };
    let rows = sqlx::query(
        "SELECT summary_json FROM runtime_query_snapshot_items WHERE snapshot_id=? AND ordinal>=? ORDER BY ordinal LIMIT ?",
    )
    .bind(snapshot_id)
    .bind(offset)
    .bind(request.limit)
    .fetch_all(&state.pool)
    .await?;
    let items = rows
        .into_iter()
        .map(|row| {
            serde_json::from_value(row.try_get("summary_json")?)
                .map_err(|error| RuntimeError::Internal(error.into()))
        })
        .collect::<RuntimeResult<Vec<InvocationSummaryV1>>>()?;
    let next_offset = offset + items.len() as u64;
    let next = (next_offset < total)
        .then(|| encode_cursor(snapshot_id, filter_hash, upper_bound.clone(), next_offset))
        .transpose()?;
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(InvocationSearchPageV1 {
        api_version: 1,
        snapshot_id,
        snapshot_upper_bound: upper_bound,
        total,
        items,
        next,
    }))
}

pub async fn get_execution(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> RuntimeResult<Json<ExecutionDetailV1>> {
    let row = execution_row(&state, id).await?;
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    let request_hash = content_hash(&json!({"operation":"get_execution","executionId":id}))
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let claims = authorize_delegation(
        &state,
        &headers,
        tenant_id,
        "runtime.query.execution",
        &request_hash,
    )
    .await?;
    authorize_execution_row(&state, &claims, &row, id).await?;
    let output = load_execution_output(&state, &row, tenant_id).await?;
    let response = ExecutionDetailV1 {
        api_version: 1,
        summary: execution_summary(&row)?,
        state_version: row.try_get("state_version")?,
        admission_epoch: row.try_get("admission_epoch")?,
        trace_watermark: row.try_get("trace_watermark")?,
        trace_event_count: row.try_get("trace_event_count")?,
        parent_execution_id: row.try_get("parent_execution_id")?,
        work_package_id: row.try_get("work_package_id")?,
        input: row.try_get("input_json")?,
        output,
        error: row.try_get("error_json")?,
    };
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(response))
}

pub async fn get_invocation(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> RuntimeResult<Json<InvocationDetailV1>> {
    let row = sqlx::query("SELECT id,tenant_id,application_id,session_id,execution_id,bundle_id,admission_epoch,caller_type,status,request_hash,state_version,result_json,error_json,created_at,completed_at FROM application_invocations WHERE id=?")
        .bind(id).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    let request_hash = content_hash(&json!({"operation":"get_invocation","invocationId":id}))
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let claims = authorize_delegation(
        &state,
        &headers,
        tenant_id,
        "runtime.query.invocation",
        &request_hash,
    )
    .await?;
    let application_id: Uuid = row.try_get("application_id")?;
    if !claims.tenant_wide && !claims.application_ids.contains(&application_id) {
        return Err(RuntimeError::Unauthorized);
    }
    verify_application_grant(&state, &claims, application_id).await?;
    let response = InvocationDetailV1 {
        api_version: 1,
        summary: invocation_summary(&row)?,
        request_hash: row.try_get("request_hash")?,
        status_version: row.try_get("state_version")?,
        response: row
            .try_get::<Option<Value>, _>("result_json")?
            .or(row.try_get("error_json")?),
    };
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(response))
}

pub async fn get_execution_nodes(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> RuntimeResult<Json<ExecutionCollectionPageV1<ExecutionNodeV1>>> {
    let claims = authorize_execution(&state, &headers, id, "execution_nodes").await?;
    let rows = sqlx::query("SELECT id,node_id,node_name,node_type,node_version,run_index,iteration_index,status,capability,input_json,output_json,error_code,error_message,(SELECT CAST(COALESCE(SUM(rc.cost_micros),0) AS UNSIGNED) FROM runtime_calls rc WHERE rc.tenant_id=node_executions.tenant_id AND rc.node_execution_id=node_executions.id) cost_micros,(SELECT CASE WHEN COUNT(DISTINCT rc.cost_currency)=1 THEN MAX(rc.cost_currency) ELSE NULL END FROM runtime_calls rc WHERE rc.tenant_id=node_executions.tenant_id AND rc.node_execution_id=node_executions.id) cost_currency,started_at,ended_at FROM node_executions WHERE tenant_id=? AND execution_id=? ORDER BY run_index,iteration_index,created_at,id")
        .bind(claims.tenant_id).bind(id).fetch_all(&state.pool).await?;
    let items = rows
        .into_iter()
        .map(node_from_row)
        .collect::<RuntimeResult<Vec<_>>>()?;
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(ExecutionCollectionPageV1 {
        api_version: 1,
        items,
    }))
}

pub async fn get_execution_node(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path((id, node_execution_id)): Path<(Uuid, Uuid)>,
) -> RuntimeResult<Json<ExecutionNodeV1>> {
    let claims = authorize_execution(&state, &headers, id, "execution_node").await?;
    let row = sqlx::query("SELECT id,node_id,node_name,node_type,node_version,run_index,iteration_index,status,capability,input_json,output_json,error_code,error_message,(SELECT CAST(COALESCE(SUM(rc.cost_micros),0) AS UNSIGNED) FROM runtime_calls rc WHERE rc.tenant_id=node_executions.tenant_id AND rc.node_execution_id=node_executions.id) cost_micros,(SELECT CASE WHEN COUNT(DISTINCT rc.cost_currency)=1 THEN MAX(rc.cost_currency) ELSE NULL END FROM runtime_calls rc WHERE rc.tenant_id=node_executions.tenant_id AND rc.node_execution_id=node_executions.id) cost_currency,started_at,ended_at FROM node_executions WHERE tenant_id=? AND execution_id=? AND id=?")
        .bind(claims.tenant_id)
        .bind(id)
        .bind(node_execution_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(RuntimeError::NotFound)?;
    let item = node_from_row(row)?;
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(item))
}

pub async fn get_execution_model_deltas(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<ExecutionEventsQuery>,
) -> RuntimeResult<Json<Value>> {
    let claims = authorize_execution(&state, &headers, id, "execution_model_deltas").await?;
    let after = query.after.unwrap_or_default();
    let limit = query.limit.unwrap_or(1000).clamp(1, 1000);
    let rows = sqlx::query("SELECT sequence_number,payload_json FROM execution_model_deltas WHERE tenant_id=? AND execution_id=? AND sequence_number>? ORDER BY sequence_number LIMIT ?")
        .bind(claims.tenant_id).bind(id).bind(after).bind(limit).fetch_all(&state.pool).await?;
    let items = rows.iter().map(|row| -> RuntimeResult<Value> { Ok(json!({"sequence":row.try_get::<u64,_>("sequence_number")?,"payload":row.try_get::<Value,_>("payload_json")?})) }).collect::<RuntimeResult<Vec<_>>>()?;
    let cursor = items
        .last()
        .and_then(|item| item["sequence"].as_u64())
        .unwrap_or(after);
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(json!({"items":items,"nextCursor":cursor})))
}

pub async fn get_execution_events(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<ExecutionEventsQuery>,
) -> RuntimeResult<Json<ExecutionEventPageV1>> {
    let claims = authorize_execution(&state, &headers, id, "execution_events").await?;
    let after = query.after.unwrap_or_default();
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let rows = sqlx::query("SELECT sequence_number,event_type,status,summary_json,occurred_at FROM execution_events WHERE tenant_id=? AND execution_id=? AND sequence_number>? ORDER BY sequence_number LIMIT ?")
        .bind(claims.tenant_id).bind(id).bind(after).bind(limit).fetch_all(&state.pool).await?;
    let items = rows
        .into_iter()
        .map(|row| -> RuntimeResult<ExecutionEventV1> {
            Ok(ExecutionEventV1 {
                sequence: row.try_get("sequence_number")?,
                event_type: row.try_get("event_type")?,
                status: row.try_get("status")?,
                summary: row.try_get("summary_json")?,
                occurred_at: row.try_get("occurred_at")?,
            })
        })
        .collect::<RuntimeResult<Vec<_>>>()?;
    let next_cursor = items.last().map(|event| event.sequence);
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(ExecutionEventPageV1 {
        api_version: 1,
        items,
        next_cursor,
    }))
}

#[derive(Default, Deserialize)]
pub struct ExecutionEventsQuery {
    after: Option<u64>,
    limit: Option<u32>,
}

pub async fn get_execution_checkpoints(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> RuntimeResult<Json<ExecutionCollectionPageV1<ExecutionCheckpointV1>>> {
    let claims = authorize_execution(&state, &headers, id, "execution_checkpoints").await?;
    let rows = sqlx::query("SELECT id,node_execution_id,sequence_number,checkpoint_type,state_hash,payload_hash,state_version,created_at FROM checkpoints WHERE tenant_id=? AND execution_id=? ORDER BY sequence_number,id")
        .bind(claims.tenant_id).bind(id).fetch_all(&state.pool).await?;
    let items = rows
        .into_iter()
        .map(|row| -> RuntimeResult<_> {
            Ok(ExecutionCheckpointV1 {
                checkpoint_id: row.try_get("id")?,
                node_execution_id: row.try_get("node_execution_id")?,
                sequence_number: row.try_get("sequence_number")?,
                checkpoint_type: row.try_get("checkpoint_type")?,
                state_hash: row.try_get("state_hash")?,
                payload_hash: parse_hash(row.try_get("payload_hash")?)?,
                state_version: row.try_get("state_version")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<RuntimeResult<Vec<_>>>()?;
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(ExecutionCollectionPageV1 {
        api_version: 1,
        items,
    }))
}

pub async fn get_execution_runtime_details(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> RuntimeResult<Json<ExecutionRuntimeDetailsV1>> {
    let claims = authorize_execution(&state, &headers, id, "execution_runtime_details").await?;
    let attempts = sqlx::query("SELECT a.id,a.node_execution_id,a.attempt_number,a.status,a.fencing_token,a.result_hash,a.error_code,a.error_message,COALESCE(a.worker_instance_id,(SELECT BIN_TO_UUID(l.worker_id) FROM worker_leases l WHERE l.tenant_id=a.tenant_id AND l.node_attempt_id=a.id ORDER BY l.fencing_token DESC LIMIT 1)) worker_id FROM node_attempts a WHERE a.tenant_id=? AND a.execution_id=? ORDER BY a.node_execution_id,a.attempt_number")
        .bind(claims.tenant_id).bind(id).fetch_all(&state.pool).await?.into_iter().map(|row| -> RuntimeResult<_> { Ok(NodeAttemptV1 {
            attempt_id: row.try_get("id")?, node_execution_id: row.try_get("node_execution_id")?,
            attempt_number: row.try_get("attempt_number")?, status: row.try_get("status")?,
            worker_id: row.try_get::<Option<String>,_>("worker_id")?.map(parse_worker_id).transpose()?,
            fencing_token: row.try_get("fencing_token")?, result_hash: row.try_get::<Option<String>,_>("result_hash")?.map(parse_hash).transpose()?,
            error_code: row.try_get("error_code")?, error_message: row.try_get("error_message")?,
        })}).collect::<RuntimeResult<Vec<_>>>()?;
    let calls = sqlx::query("SELECT id,node_execution_id,attempt_id,call_kind,status,resource_type,resource_id,resource_version_id,input_tokens,output_tokens,cost_micros,cost_currency,error_code FROM runtime_calls WHERE tenant_id=? AND execution_id=? ORDER BY started_at,id")
        .bind(claims.tenant_id).bind(id).fetch_all(&state.pool).await?.into_iter().map(|row| -> RuntimeResult<_> { Ok(RuntimeCallDetailV1 {
            call_id: row.try_get("id")?, node_execution_id: row.try_get("node_execution_id")?, attempt_id: row.try_get("attempt_id")?,
            call_kind: row.try_get("call_kind")?, status: row.try_get("status")?, resource_type: row.try_get("resource_type")?,
            resource_id: row.try_get("resource_id")?, resource_version: row.try_get("resource_version_id")?,
            input_tokens: row.try_get("input_tokens")?, output_tokens: row.try_get("output_tokens")?, cost_micros: row.try_get("cost_micros")?, cost_currency: row.try_get("cost_currency")?, error_code: row.try_get("error_code")?,
        })}).collect::<RuntimeResult<Vec<_>>>()?;
    let agent_runs = json_rows(&state, "SELECT JSON_OBJECT('id',BIN_TO_UUID(id),'status',status,'iterationCount',iteration_count,'modelCallCount',model_call_count,'toolCallCount',tool_call_count,'stopReason',stop_reason) value FROM agent_runs WHERE tenant_id=? AND execution_id=? ORDER BY started_at,id", claims.tenant_id, id).await?;
    let sandboxes = json_rows(&state, "SELECT JSON_OBJECT('id',BIN_TO_UUID(id),'status',status,'sandboxId',sandbox_id,'expiresAt',DATE_FORMAT(expires_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'terminationAttempts',termination_attempts,'outcomeUnknown',outcome_unknown) value FROM sandbox_leases WHERE tenant_id=? AND execution_id=? ORDER BY created_at,id", claims.tenant_id, id).await?;
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(ExecutionRuntimeDetailsV1 {
        api_version: 1,
        attempts,
        calls,
        agent_runs,
        sandboxes,
    }))
}

pub async fn get_execution_artifact(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path((id, artifact_id)): Path<(Uuid, Uuid)>,
) -> RuntimeResult<Response> {
    let claims = authorize_execution(&state, &headers, id, "execution_artifact").await?;
    let row = sqlx::query("SELECT a.content_type,a.size_bytes,a.sha256,a.storage_key FROM artifacts a JOIN artifact_references r ON r.tenant_id=a.tenant_id AND r.artifact_id=a.id WHERE a.tenant_id=? AND a.id=? AND a.deleted_at IS NULL AND (r.owner_id=? OR EXISTS(SELECT 1 FROM node_executions n WHERE n.tenant_id=a.tenant_id AND n.execution_id=? AND r.owner_id=BIN_TO_UUID(n.id))) LIMIT 1")
        .bind(claims.tenant_id).bind(artifact_id).bind(id.to_string()).bind(id).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    let content_type: String = row.try_get("content_type")?;
    let size_bytes: u64 = row.try_get("size_bytes")?;
    let sha256: String = row.try_get("sha256")?;
    let bytes = state
        .objects
        .get(&ObjectPath::from(row.try_get::<String, _>("storage_key")?))
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?
        .bytes()
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    if bytes.len() as u64 != size_bytes || format!("{:x}", Sha256::digest(&bytes)) != sha256 {
        return Err(RuntimeError::Unavailable);
    }
    complete_query_receipt(&state, claims.jti, "OK").await?;
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&content_type)
                .map_err(|error| RuntimeError::Internal(error.into()))?,
        )
        .header(header::CONTENT_LENGTH, size_bytes)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{artifact_id}\""),
        )
        .header(header::ETAG, format!("\"sha256:{sha256}\""))
        .body(Body::from(bytes))
        .map_err(|error| RuntimeError::Internal(error.into()))
}

async fn create_execution_snapshot(
    state: &RuntimeState,
    claims: &DelegationClaimsV1,
    request: &ExecutionSearchRequestV1,
    filter_hash: &ContentHash,
) -> RuntimeResult<(Uuid, String, u64, u64)> {
    let apps = serde_json::to_string(&request.application_ids)?;
    let workflows = serde_json::to_string(&request.workflow_ids)?;
    let authorized_apps = serde_json::to_string(&claims.application_ids)?;
    let authorized_workflows = serde_json::to_string(&claims.workflow_ids)?;
    let tools = serde_json::to_string(&request.tool_ids)?;
    let users = serde_json::to_string(&request.initiator_user_ids)?;
    let departments = serde_json::to_string(&request.initiator_department_ids)?;
    let trigger_types = serde_json::to_string(&request.trigger_types)?;
    let statuses = serde_json::to_string(&request.statuses)?;
    let error_codes = serde_json::to_string(&request.error_codes)?;
    let session_mode = match request.session_mode {
        agentx_runtime_contracts::ExecutionSessionModeV1::All => "all",
        agentx_runtime_contracts::ExecutionSessionModeV1::Stateless => "stateless",
        agentx_runtime_contracts::ExecutionSessionModeV1::Session => "session",
    };
    let trigger_name = request
        .trigger_name
        .as_ref()
        .map(|value| format!("%{}%", escape_like(&value.trim().to_lowercase())));
    let search = request
        .search
        .as_ref()
        .map(|value| format!("%{}%", escape_like(value.trim())));
    let mut tx = state.pool.begin().await?;
    let rows = sqlx::query("SELECT /*+ MAX_EXECUTION_TIME(5000) */ id,invocation_id,application_id,workflow_id,workflow_version_id,session_id,parent_execution_id,bundle_id,trace_id,trigger_type,initiator_user_id,initiator_user_name,initiator_department_id,initiator_department_name,trigger_source_id,trigger_name,status,duration_ms,(SELECT CAST(COALESCE(SUM(rc.cost_micros),0) AS UNSIGNED) FROM runtime_calls rc WHERE rc.tenant_id=workflow_executions.tenant_id AND rc.execution_id=workflow_executions.id) cost_micros,(SELECT CASE WHEN COUNT(DISTINCT rc.cost_currency)=1 THEN MAX(rc.cost_currency) ELSE NULL END FROM runtime_calls rc WHERE rc.tenant_id=workflow_executions.tenant_id AND rc.execution_id=workflow_executions.id) cost_currency,(SELECT CAST(COALESCE(SUM(rc.input_tokens),0) AS UNSIGNED) FROM runtime_calls rc WHERE rc.tenant_id=workflow_executions.tenant_id AND rc.execution_id=workflow_executions.id) input_tokens,(SELECT CAST(COALESCE(SUM(rc.output_tokens),0) AS UNSIGNED) FROM runtime_calls rc WHERE rc.tenant_id=workflow_executions.tenant_id AND rc.execution_id=workflow_executions.id) output_tokens,error_code,created_at,ended_at FROM workflow_executions WHERE tenant_id=? AND (?=TRUE OR (JSON_CONTAINS(?,JSON_QUOTE(BIN_TO_UUID(application_id))) AND EXISTS(SELECT 1 FROM runtime_user_application_grants scope_app WHERE scope_app.tenant_id=workflow_executions.tenant_id AND scope_app.user_id=? AND scope_app.application_id=workflow_executions.application_id AND scope_app.status='active' AND scope_app.can_query=TRUE)) OR (JSON_CONTAINS(?,JSON_QUOTE(BIN_TO_UUID(workflow_id))) AND EXISTS(SELECT 1 FROM runtime_user_workflow_grants scope_workflow WHERE scope_workflow.tenant_id=workflow_executions.tenant_id AND scope_workflow.user_id=? AND scope_workflow.workflow_id=workflow_executions.workflow_id AND scope_workflow.status='active'))) AND retention_deleted_at IS NULL AND (?='all' OR (?='stateless' AND session_id IS NULL) OR (?='session' AND session_id IS NOT NULL)) AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(BIN_TO_UUID(application_id)))) AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(BIN_TO_UUID(workflow_id)))) AND (JSON_LENGTH(?)=0 OR EXISTS(SELECT 1 FROM runtime_calls tool_call WHERE tool_call.tenant_id=workflow_executions.tenant_id AND tool_call.execution_id=workflow_executions.id AND tool_call.call_kind='mcp_tool' AND JSON_CONTAINS(?,JSON_QUOTE(BIN_TO_UUID(tool_call.resource_id))))) AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(BIN_TO_UUID(initiator_user_id)))) AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(BIN_TO_UUID(initiator_department_id)))) AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(trigger_type))) AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(status))) AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(error_code))) AND (? IS NULL OR LOWER(trigger_name) LIKE ?) AND (? IS NULL OR created_at>=?) AND (? IS NULL OR created_at<=?) AND (? IS NULL OR BIN_TO_UUID(id) LIKE ? OR BIN_TO_UUID(trace_id) LIKE ? OR error_code LIKE ?) ORDER BY created_at DESC,id DESC LIMIT 10001")
        .bind(request.tenant_id).bind(claims.tenant_wide).bind(&authorized_apps).bind(claims.sub).bind(&authorized_workflows).bind(claims.sub)
        .bind(session_mode).bind(session_mode).bind(session_mode).bind(&apps).bind(&apps).bind(&workflows).bind(&workflows)
        .bind(&tools).bind(&tools).bind(&users).bind(&users).bind(&departments).bind(&departments)
        .bind(&trigger_types).bind(&trigger_types).bind(&statuses).bind(&statuses).bind(&error_codes).bind(&error_codes)
        .bind(&trigger_name).bind(&trigger_name)
        .bind(request.created_after).bind(request.created_after).bind(request.created_before).bind(request.created_before)
        .bind(&search).bind(&search).bind(&search).bind(&search).fetch_all(&mut *tx).await?;
    enforce_snapshot_budget(rows.len())?;
    let snapshot_id = Uuid::now_v7();
    let upper_bound = upper_bound();
    sqlx::query("INSERT INTO runtime_query_snapshots(id,tenant_id,subject_id,query_kind,filter_hash,upper_bound,total_count,expires_at) VALUES(?,?,?,'execution',?,?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 15 MINUTE))")
        .bind(snapshot_id).bind(request.tenant_id).bind(claims.sub).bind(filter_hash.as_str()).bind(&upper_bound).bind(rows.len() as u64).execute(&mut *tx).await?;
    for (ordinal, row) in rows.iter().enumerate() {
        let summary = execution_summary(row)?;
        sqlx::query("INSERT INTO runtime_query_snapshot_items(snapshot_id,ordinal,object_id,summary_json) VALUES(?,?,?,?)")
            .bind(snapshot_id).bind(ordinal as u64).bind(summary.execution_id).bind(serde_json::to_value(summary)?).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok((snapshot_id, upper_bound, rows.len() as u64, 0))
}

async fn create_invocation_snapshot(
    state: &RuntimeState,
    claims: &DelegationClaimsV1,
    request: &InvocationSearchRequestV1,
    filter_hash: &ContentHash,
) -> RuntimeResult<(Uuid, String, u64, u64)> {
    let apps = serde_json::to_string(&request.application_ids)?;
    let statuses = serde_json::to_string(&request.statuses)?;
    let mut tx = state.pool.begin().await?;
    let rows = sqlx::query("SELECT /*+ MAX_EXECUTION_TIME(5000) */ id,application_id,session_id,execution_id,bundle_id,admission_epoch,caller_type,status,created_at,completed_at FROM application_invocations WHERE tenant_id=? AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(BIN_TO_UUID(application_id)))) AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(status))) AND (? IS NULL OR created_at>=?) AND (? IS NULL OR created_at<=?) ORDER BY created_at DESC,id DESC LIMIT 10001")
        .bind(request.tenant_id).bind(&apps).bind(&apps).bind(&statuses).bind(&statuses)
        .bind(request.created_after).bind(request.created_after).bind(request.created_before).bind(request.created_before).fetch_all(&mut *tx).await?;
    enforce_snapshot_budget(rows.len())?;
    let snapshot_id = Uuid::now_v7();
    let upper_bound = upper_bound();
    sqlx::query("INSERT INTO runtime_query_snapshots(id,tenant_id,subject_id,query_kind,filter_hash,upper_bound,total_count,expires_at) VALUES(?,?,?,'invocation',?,?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 15 MINUTE))")
        .bind(snapshot_id).bind(request.tenant_id).bind(claims.sub).bind(filter_hash.as_str()).bind(&upper_bound).bind(rows.len() as u64).execute(&mut *tx).await?;
    for (ordinal, row) in rows.iter().enumerate() {
        let summary = invocation_summary(row)?;
        sqlx::query("INSERT INTO runtime_query_snapshot_items(snapshot_id,ordinal,object_id,summary_json) VALUES(?,?,?,?)")
            .bind(snapshot_id).bind(ordinal as u64).bind(summary.invocation_id).bind(serde_json::to_value(summary)?).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok((snapshot_id, upper_bound, rows.len() as u64, 0))
}

async fn resume_snapshot(
    state: &RuntimeState,
    cursor: &str,
    tenant_id: Uuid,
    subject_id: Uuid,
    query_kind: &str,
    expected_filter_hash: &ContentHash,
) -> RuntimeResult<(Uuid, String, u64, u64)> {
    let cursor = decode_cursor(cursor)?;
    if &cursor.filter_hash != expected_filter_hash
        || OffsetDateTime::now_utc().unix_timestamp() - cursor.issued_at
            > QUERY_SNAPSHOT_TTL_SECONDS
    {
        return Err(RuntimeError::QueryCursorExpired);
    }
    let row = sqlx::query("SELECT upper_bound,total_count,filter_hash FROM runtime_query_snapshots WHERE id=? AND tenant_id=? AND subject_id=? AND query_kind=? AND expires_at>UTC_TIMESTAMP(6)")
        .bind(cursor.snapshot_id).bind(tenant_id).bind(subject_id).bind(query_kind).fetch_optional(&state.pool).await?.ok_or(RuntimeError::QueryCursorExpired)?;
    let stored_hash: String = row.try_get("filter_hash")?;
    let upper_bound: String = row.try_get("upper_bound")?;
    if stored_hash != expected_filter_hash.as_str() || upper_bound != cursor.snapshot_upper_bound {
        return Err(RuntimeError::QueryCursorExpired);
    }
    Ok((
        cursor.snapshot_id,
        upper_bound,
        row.try_get("total_count")?,
        cursor.offset,
    ))
}

async fn execution_snapshot_page(
    state: &RuntimeState,
    snapshot_id: Uuid,
    filter_hash: &ContentHash,
    upper_bound: &str,
    offset: u64,
    limit: u32,
    total: u64,
) -> RuntimeResult<(Vec<ExecutionSummaryV1>, Option<String>)> {
    let rows = sqlx::query("SELECT summary_json FROM runtime_query_snapshot_items WHERE snapshot_id=? AND ordinal>=? ORDER BY ordinal LIMIT ?")
        .bind(snapshot_id).bind(offset).bind(limit).fetch_all(&state.pool).await?;
    let items = rows
        .into_iter()
        .map(|row| {
            serde_json::from_value(row.try_get("summary_json")?)
                .map_err(|error| RuntimeError::Internal(error.into()))
        })
        .collect::<RuntimeResult<Vec<_>>>()?;
    let next_offset = offset + items.len() as u64;
    let next = (next_offset < total)
        .then(|| {
            encode_cursor(
                snapshot_id,
                filter_hash.clone(),
                upper_bound.to_owned(),
                next_offset,
            )
        })
        .transpose()?;
    Ok((items, next))
}

async fn authorize_delegation(
    state: &RuntimeState,
    headers: &HeaderMap,
    tenant_id: Uuid,
    scope: &str,
    request_hash: &ContentHash,
) -> RuntimeResult<DelegationClaimsV1> {
    let claims = state.trust.delegation(headers, tenant_id, scope)?;
    if &claims.request_hash != request_hash {
        return Err(RuntimeError::Unauthorized);
    }
    let user = sqlx::query("SELECT token_version,status,tenant_query_enabled FROM runtime_user_admission WHERE tenant_id=? AND user_id=?")
        .bind(tenant_id).bind(claims.sub).fetch_optional(&state.pool).await?.ok_or(RuntimeError::Unauthorized)?;
    if user.try_get::<String, _>("status")? != "active"
        || user.try_get::<u64, _>("token_version")? != claims.token_version
        || (claims.tenant_wide && !user.try_get::<bool, _>("tenant_query_enabled")?)
    {
        return Err(RuntimeError::Unauthorized);
    }
    let inserted = sqlx::query("INSERT IGNORE INTO runtime_query_receipts(jti,tenant_id,subject_id,scope,request_hash,status,expires_at) VALUES(?,?,?,?,?,'accepted',DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 14 DAY))")
        .bind(claims.jti).bind(tenant_id).bind(claims.sub).bind(scope).bind(request_hash.as_str()).execute(&state.pool).await?;
    if inserted.rows_affected() != 1 {
        return Err(RuntimeError::Unauthorized);
    }
    Ok(claims)
}

async fn authorize_list_scope(
    state: &RuntimeState,
    claims: &DelegationClaimsV1,
    applications: &[Uuid],
    workflows: &[Uuid],
) -> RuntimeResult<()> {
    for application in applications {
        if !claims.tenant_wide && !claims.application_ids.contains(application) {
            return Err(RuntimeError::Unauthorized);
        }
        verify_application_grant(state, claims, *application).await?;
    }
    for workflow in workflows {
        if !claims.tenant_wide && !claims.workflow_ids.contains(workflow) {
            return Err(RuntimeError::Unauthorized);
        }
        let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_user_workflow_grants WHERE tenant_id=? AND user_id=? AND workflow_id=? AND status='active')")
            .bind(claims.tenant_id).bind(claims.sub).bind(workflow).fetch_one(&state.pool).await?;
        if !allowed && !claims.tenant_wide {
            return Err(RuntimeError::Unauthorized);
        }
    }
    Ok(())
}

async fn verify_application_grant(
    state: &RuntimeState,
    claims: &DelegationClaimsV1,
    application_id: Uuid,
) -> RuntimeResult<()> {
    if claims.tenant_wide {
        return Ok(());
    }
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_user_application_grants WHERE tenant_id=? AND user_id=? AND application_id=? AND status='active' AND can_query=TRUE)")
        .bind(claims.tenant_id).bind(claims.sub).bind(application_id).fetch_one(&state.pool).await?;
    if !allowed {
        return Err(RuntimeError::Unauthorized);
    }
    Ok(())
}

async fn authorize_execution(
    state: &RuntimeState,
    headers: &HeaderMap,
    id: Uuid,
    operation: &str,
) -> RuntimeResult<DelegationClaimsV1> {
    let row = execution_row(state, id).await?;
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    let request_hash = content_hash(&json!({"operation":operation,"executionId":id}))
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let claims = authorize_delegation(
        state,
        headers,
        tenant_id,
        "runtime.query.execution",
        &request_hash,
    )
    .await?;
    authorize_execution_row(state, &claims, &row, id).await?;
    Ok(claims)
}

async fn authorize_execution_row(
    state: &RuntimeState,
    claims: &DelegationClaimsV1,
    row: &sqlx::mysql::MySqlRow,
    id: Uuid,
) -> RuntimeResult<()> {
    if claims.tenant_wide {
        return Ok(());
    }
    if !claims.execution_ids.is_empty() && !claims.execution_ids.contains(&id) {
        return Err(RuntimeError::Unauthorized);
    }
    if let Some(application_id) = row.try_get::<Option<Uuid>, _>("application_id")? {
        if claims.execution_ids.contains(&id) || claims.application_ids.contains(&application_id) {
            return verify_application_grant(state, claims, application_id).await;
        }
    }
    let workflow_id: Uuid = row.try_get("workflow_id")?;
    if claims.execution_ids.contains(&id) || claims.workflow_ids.contains(&workflow_id) {
        let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_user_workflow_grants WHERE tenant_id=? AND user_id=? AND workflow_id=? AND status='active')")
            .bind(claims.tenant_id).bind(claims.sub).bind(workflow_id).fetch_one(&state.pool).await?;
        if allowed {
            return Ok(());
        }
    }
    Err(RuntimeError::Unauthorized)
}

pub(crate) async fn authorize_execution_command(
    state: &RuntimeState,
    headers: &HeaderMap,
    request: &agentx_runtime_contracts::RuntimeCommandApplyRequestV1,
    execution_id: Uuid,
) -> RuntimeResult<()> {
    let row = execution_row(state, execution_id).await?;
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    if tenant_id != request.tenant_id {
        return Err(RuntimeError::Unauthorized);
    }
    if row.try_get::<u64, _>("state_version")? != request.object_version {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::HeadVersionConflict,
            "Runtime command observed a stale Execution version".into(),
        ));
    }
    let request_hash = content_hash(&json!({"operation":"runtime-command","request":request}))
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let claims = authorize_delegation(
        state,
        headers,
        tenant_id,
        "runtime.command.execution",
        &request_hash,
    )
    .await?;
    authorize_execution_row(state, &claims, &row, execution_id).await
}

async fn execution_row(state: &RuntimeState, id: Uuid) -> RuntimeResult<sqlx::mysql::MySqlRow> {
    sqlx::query("SELECT id,tenant_id,invocation_id,application_id,workflow_id,workflow_version_id,session_id,parent_execution_id,bundle_id,trace_id,trigger_type,initiator_user_id,initiator_user_name,initiator_department_id,initiator_department_name,trigger_source_id,trigger_name,status,duration_ms,(SELECT CAST(COALESCE(SUM(rc.cost_micros),0) AS UNSIGNED) FROM runtime_calls rc WHERE rc.tenant_id=workflow_executions.tenant_id AND rc.execution_id=workflow_executions.id) cost_micros,(SELECT CASE WHEN COUNT(DISTINCT rc.cost_currency)=1 THEN MAX(rc.cost_currency) ELSE NULL END FROM runtime_calls rc WHERE rc.tenant_id=workflow_executions.tenant_id AND rc.execution_id=workflow_executions.id) cost_currency,(SELECT CAST(COALESCE(SUM(rc.input_tokens),0) AS UNSIGNED) FROM runtime_calls rc WHERE rc.tenant_id=workflow_executions.tenant_id AND rc.execution_id=workflow_executions.id) input_tokens,(SELECT CAST(COALESCE(SUM(rc.output_tokens),0) AS UNSIGNED) FROM runtime_calls rc WHERE rc.tenant_id=workflow_executions.tenant_id AND rc.execution_id=workflow_executions.id) output_tokens,error_code,created_at,ended_at,state_version,admission_epoch,CAST(COALESCE((SELECT MAX(t.sequence_number) FROM execution_events t WHERE t.tenant_id=workflow_executions.tenant_id AND t.execution_id=workflow_executions.id),0) AS UNSIGNED) trace_watermark,CAST((SELECT COUNT(*) FROM execution_events t WHERE t.tenant_id=workflow_executions.tenant_id AND t.execution_id=workflow_executions.id) AS UNSIGNED) trace_event_count,work_package_id,input_json,output_json,error_json,terminal_result_object_id,terminal_result_hash FROM workflow_executions WHERE id=? AND retention_deleted_at IS NULL")
        .bind(id).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)
}

fn execution_summary(row: &sqlx::mysql::MySqlRow) -> RuntimeResult<ExecutionSummaryV1> {
    Ok(ExecutionSummaryV1 {
        execution_id: row.try_get("id")?,
        invocation_id: row.try_get("invocation_id")?,
        application_id: row.try_get("application_id")?,
        workflow_id: row.try_get("workflow_id")?,
        workflow_version_id: row.try_get("workflow_version_id")?,
        session_id: row.try_get("session_id")?,
        parent_execution_id: row.try_get("parent_execution_id")?,
        bundle_id: row
            .try_get::<Option<Uuid>, _>("bundle_id")?
            .ok_or_else(|| RuntimeError::Internal(anyhow::anyhow!("Execution has no Bundle")))?,
        trace_id: row.try_get("trace_id")?,
        trigger_type: row.try_get("trigger_type")?,
        initiator_user_id: row.try_get("initiator_user_id")?,
        initiator_user_name: row.try_get("initiator_user_name")?,
        initiator_department_id: row.try_get("initiator_department_id")?,
        initiator_department_name: row.try_get("initiator_department_name")?,
        trigger_source_id: row.try_get("trigger_source_id")?,
        trigger_name: row.try_get("trigger_name")?,
        status: row.try_get("status")?,
        duration_ms: row.try_get("duration_ms")?,
        cost_micros: row.try_get("cost_micros")?,
        cost_currency: row.try_get("cost_currency")?,
        input_tokens: row.try_get("input_tokens")?,
        output_tokens: row.try_get("output_tokens")?,
        error_code: row.try_get("error_code")?,
        created_at: row.try_get("created_at")?,
        completed_at: row.try_get("ended_at")?,
    })
}

fn invocation_summary(row: &sqlx::mysql::MySqlRow) -> RuntimeResult<InvocationSummaryV1> {
    Ok(InvocationSummaryV1 {
        invocation_id: row.try_get("id")?,
        execution_id: row.try_get("execution_id")?,
        application_id: row.try_get("application_id")?,
        session_id: row.try_get("session_id")?,
        bundle_id: row
            .try_get::<Option<Uuid>, _>("bundle_id")?
            .ok_or_else(|| RuntimeError::Internal(anyhow::anyhow!("Invocation has no Bundle")))?,
        admission_epoch: row.try_get("admission_epoch")?,
        caller_type: row.try_get("caller_type")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn node_from_row(row: sqlx::mysql::MySqlRow) -> RuntimeResult<ExecutionNodeV1> {
    Ok(ExecutionNodeV1 {
        node_execution_id: row.try_get("id")?,
        node_id: row.try_get("node_id")?,
        node_name: row.try_get("node_name")?,
        node_type: row.try_get("node_type")?,
        node_version: row.try_get("node_version")?,
        run_index: row.try_get("run_index")?,
        iteration_index: row.try_get("iteration_index")?,
        status: row.try_get("status")?,
        capability: row.try_get("capability")?,
        input: row.try_get("input_json")?,
        output: row.try_get("output_json")?,
        error_code: row.try_get("error_code")?,
        error_message: row.try_get("error_message")?,
        cost_micros: row.try_get("cost_micros")?,
        cost_currency: row.try_get("cost_currency")?,
        started_at: row.try_get("started_at")?,
        ended_at: row.try_get("ended_at")?,
    })
}

async fn load_execution_output(
    state: &RuntimeState,
    row: &sqlx::mysql::MySqlRow,
    tenant_id: Uuid,
) -> RuntimeResult<Option<Value>> {
    if let Some(output) = row.try_get("output_json")? {
        return Ok(Some(output));
    }
    if let (Some(artifact_id), Some(expected_hash)) = (
        row.try_get::<Option<Uuid>, _>("terminal_result_object_id")?,
        row.try_get::<Option<String>, _>("terminal_result_hash")?,
    ) {
        let bytes =
            crate::artifact::load_artifact(state, tenant_id, artifact_id, &expected_hash).await?;
        return serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| RuntimeError::Internal(error.into()));
    }
    Ok(None)
}

async fn json_rows(
    state: &RuntimeState,
    query: &str,
    tenant_id: Uuid,
    execution_id: Uuid,
) -> RuntimeResult<Vec<Value>> {
    sqlx::query(query)
        .bind(tenant_id)
        .bind(execution_id)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(|row| row.try_get("value").map_err(RuntimeError::from))
        .collect()
}

fn execution_filter_hash(request: &ExecutionSearchRequestV1) -> RuntimeResult<ContentHash> {
    content_hash(&json!({"tenantId":request.tenant_id,"applicationIds":request.application_ids,"workflowIds":request.workflow_ids,"toolIds":request.tool_ids,"initiatorUserIds":request.initiator_user_ids,"initiatorDepartmentIds":request.initiator_department_ids,"triggerTypes":request.trigger_types,"triggerName":request.trigger_name,"statuses":request.statuses,"errorCodes":request.error_codes,"sessionMode":request.session_mode,"createdAfter":request.created_after,"createdBefore":request.created_before,"search":request.search}))
        .map_err(|error| RuntimeError::Internal(error.into()))
}

fn invocation_filter_hash(request: &InvocationSearchRequestV1) -> RuntimeResult<ContentHash> {
    content_hash(&json!({"tenantId":request.tenant_id,"applicationIds":request.application_ids,"statuses":request.statuses,"createdAfter":request.created_after,"createdBefore":request.created_before}))
        .map_err(|error| RuntimeError::Internal(error.into()))
}

fn query_request_hash<T: Serialize>(operation: &str, request: &T) -> RuntimeResult<ContentHash> {
    content_hash(&json!({"operation":operation,"request":request}))
        .map_err(|error| RuntimeError::Internal(error.into()))
}

fn encode_cursor(
    snapshot_id: Uuid,
    filter_hash: ContentHash,
    snapshot_upper_bound: String,
    offset: u64,
) -> RuntimeResult<String> {
    serde_json::to_vec(&SnapshotCursor {
        snapshot_id,
        filter_hash,
        snapshot_upper_bound,
        offset,
        issued_at: OffsetDateTime::now_utc().unix_timestamp(),
    })
    .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
    .map_err(|error| RuntimeError::Internal(error.into()))
}

fn decode_cursor(value: &str) -> RuntimeResult<SnapshotCursor> {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(RuntimeError::QueryCursorExpired)
}

fn upper_bound() -> String {
    format!(
        "{}:{}",
        OffsetDateTime::now_utc().unix_timestamp_nanos(),
        Uuid::now_v7()
    )
}

fn validate_page_limit(limit: u32) -> RuntimeResult<()> {
    if !(1..=100).contains(&limit) {
        return Err(RuntimeError::QueryBudgetExceeded);
    }
    Ok(())
}

fn enforce_snapshot_budget(row_count: usize) -> RuntimeResult<()> {
    if row_count > QUERY_SNAPSHOT_LIMIT {
        return Err(RuntimeError::QueryBudgetExceeded);
    }
    Ok(())
}

fn validate_execution_search(request: &ExecutionSearchRequestV1) -> RuntimeResult<()> {
    for values in [
        request.application_ids.len(),
        request.workflow_ids.len(),
        request.tool_ids.len(),
        request.initiator_user_ids.len(),
        request.initiator_department_ids.len(),
        request.trigger_types.len(),
        request.statuses.len(),
        request.error_codes.len(),
    ] {
        if values > 50 {
            return Err(RuntimeError::InvalidRequest(
                "TOO_MANY_FILTER_VALUES",
                "Each execution filter supports at most 50 values".into(),
            ));
        }
    }
    for value in [request.search.as_deref(), request.trigger_name.as_deref()]
        .into_iter()
        .flatten()
    {
        if value.chars().count() > 200 {
            return Err(RuntimeError::InvalidRequest(
                "FILTER_TEXT_TOO_LONG",
                "Execution filter text supports at most 200 characters".into(),
            ));
        }
    }
    Ok(())
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn parse_hash(value: String) -> RuntimeResult<ContentHash> {
    ContentHash::parse(value).map_err(|error| RuntimeError::Internal(error.into()))
}

fn parse_worker_id(value: String) -> RuntimeResult<Uuid> {
    Uuid::parse_str(&value).map_err(|error| RuntimeError::Internal(error.into()))
}

async fn complete_query_receipt(state: &RuntimeState, jti: Uuid, code: &str) -> RuntimeResult<()> {
    sqlx::query("UPDATE runtime_query_receipts SET status='completed',result_code=?,completed_at=UTC_TIMESTAMP(6) WHERE jti=? AND status='accepted'")
        .bind(code).bind(jti).execute(&state.pool).await?;
    Ok(())
}

impl From<serde_json::Error> for RuntimeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(error.into())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use agentx_runtime_contracts::ExecutionSearchRequestV1;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::{enforce_snapshot_budget, execution_filter_hash, validate_execution_search};

    fn request() -> ExecutionSearchRequestV1 {
        ExecutionSearchRequestV1 {
            api_version: 1,
            tenant_id: Uuid::from_u128(1),
            application_ids: vec![],
            workflow_ids: vec![],
            tool_ids: vec![],
            initiator_user_ids: vec![],
            initiator_department_ids: vec![],
            trigger_types: vec![],
            trigger_name: None,
            statuses: vec![],
            error_codes: vec![],
            session_mode: agentx_runtime_contracts::ExecutionSessionModeV1::All,
            created_after: None,
            created_before: None,
            search: None,
            cursor: None,
            limit: 8,
        }
    }

    #[test]
    fn execution_snapshot_hash_covers_every_filter_but_not_pagination() {
        let base = request();
        let mut variants = Vec::new();
        let id = Uuid::from_u128(2);
        macro_rules! variant {
            ($field:ident, $value:expr) => {{
                let mut value = base.clone();
                value.$field = $value;
                variants.push(value);
            }};
        }
        variant!(application_ids, vec![id]);
        variant!(workflow_ids, vec![id]);
        variant!(tool_ids, vec![id]);
        variant!(initiator_user_ids, vec![id]);
        variant!(initiator_department_ids, vec![id]);
        variant!(trigger_types, vec!["schedule".into()]);
        variant!(trigger_name, Some("nightly".into()));
        variant!(statuses, vec!["succeeded".into()]);
        variant!(error_codes, vec!["MODEL_INPUT_UNSUPPORTED".into()]);
        variant!(
            session_mode,
            agentx_runtime_contracts::ExecutionSessionModeV1::Session
        );
        variant!(created_after, Some(OffsetDateTime::UNIX_EPOCH));
        variant!(created_before, Some(OffsetDateTime::UNIX_EPOCH));
        variant!(search, Some("TRACE_FAILURE".into()));

        let base_hash = execution_filter_hash(&base).unwrap();
        let hashes = variants
            .iter()
            .map(|request| execution_filter_hash(request).unwrap().to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(hashes.len(), variants.len());
        assert!(!hashes.contains(&base_hash.to_string()));

        let mut paged = base.clone();
        paged.cursor = Some("opaque".into());
        paged.limit = 100;
        assert_eq!(execution_filter_hash(&paged).unwrap(), base_hash);
    }

    #[test]
    fn execution_filter_budget_rejects_oversized_dimensions_and_text() {
        let mut oversized = request();
        oversized.tool_ids = (1..=51).map(Uuid::from_u128).collect();
        assert!(validate_execution_search(&oversized).is_err());

        let mut text = request();
        text.trigger_name = Some("触".repeat(201));
        assert!(validate_execution_search(&text).is_err());

        assert!(enforce_snapshot_budget(10_000).is_ok());
        assert!(enforce_snapshot_budget(10_001).is_err());
    }
}
