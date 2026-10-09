use agentx_runtime_contracts::{
    AgentSessionClearReceiptV1, AgentSessionClearRequestV1, AgentSessionDiagnosticV1,
    AgentSessionEntryViewV1, AgentSessionSearchPageV1, AgentSessionSearchRequestV1,
    AgentSessionSummaryV1, AgentSubjectMemoryAuditViewV1, AgentSubjectMemoryClearReceiptV1,
    AgentSubjectMemoryClearRequestV1, AgentSubjectMemorySearchPageV1,
    AgentSubjectMemorySearchRequestV1, SessionDetailV1, SessionSearchPageV1,
    SessionSearchRequestV1, SessionSummaryV1, SessionUpgradeCommandV1, SessionUpgradeReceiptV1,
    SessionVersionPolicyV1,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

pub use crate::query_authority::{
    ExecutionEventsQuery, get_execution, get_execution_artifact, get_execution_checkpoints,
    get_execution_events, get_execution_model_deltas, get_execution_node, get_execution_nodes,
    get_execution_runtime_details, get_invocation, search_executions, search_invocations,
};

pub async fn search_agent_sessions(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<AgentSessionSearchRequestV1>,
) -> RuntimeResult<Json<AgentSessionSearchPageV1>> {
    let claims =
        state
            .trust
            .delegation(&headers, request.tenant_id, "runtime.query.agent_sessions")?;
    if let Some(application_id) = request.application_id {
        if !claims.tenant_wide && !claims.application_ids.contains(&application_id) {
            return Err(RuntimeError::Unauthorized);
        }
    } else if !claims.tenant_wide {
        return Err(RuntimeError::Unauthorized);
    }
    let limit = request.limit.clamp(1, 100);
    let rows = sqlx::query("SELECT session_key,session_id,stable_agent_node_key,application_id,state_version,fencing_token,leaf_entry_id,open_operation_id,terminal_state,bundle_hash,model_version,updated_at FROM agent_session_registers WHERE tenant_id=? AND (? IS NULL OR application_id=?) AND (? IS NULL OR session_key=?) AND (? IS NULL OR stable_agent_node_key=?) AND (? IS NULL OR CONCAT(DATE_FORMAT(updated_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'|',session_key,'|',stable_agent_node_key)>?) ORDER BY updated_at,session_key,stable_agent_node_key LIMIT ?")
        .bind(request.tenant_id)
        .bind(request.application_id).bind(request.application_id)
        .bind(&request.session_key).bind(&request.session_key)
        .bind(&request.stable_agent_node_key).bind(&request.stable_agent_node_key)
        .bind(&request.after).bind(&request.after)
        .bind(limit + 1)
        .fetch_all(&state.pool)
        .await?;
    let mut items = rows
        .iter()
        .map(agent_session_summary)
        .collect::<RuntimeResult<Vec<_>>>()?;
    let next = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|item| {
            format!(
                "{}|{}|{}",
                item.updated_at
                    .format(&time::format_description::well_known::Iso8601::DEFAULT)
                    .unwrap_or_default(),
                item.session_key,
                item.stable_agent_node_key
            )
        })
    } else {
        None
    };
    Ok(Json(AgentSessionSearchPageV1 {
        api_version: 1,
        items,
        next,
    }))
}

pub async fn get_agent_session(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path((session_key, node_key)): Path<(String, String)>,
) -> RuntimeResult<Json<AgentSessionDiagnosticV1>> {
    let candidates = sqlx::query("SELECT session_key,session_id,stable_agent_node_key,application_id,state_version,fencing_token,leaf_entry_id,open_operation_id,terminal_state,bundle_hash,model_version,updated_at,tenant_id,register_json FROM agent_session_registers WHERE session_key=? AND stable_agent_node_key=?")
        .bind(&session_key).bind(&node_key).fetch_all(&state.pool).await?;
    let mut selected = None;
    for candidate in candidates {
        let tenant_id: Uuid = candidate.try_get("tenant_id")?;
        let Ok(claims) =
            state
                .trust
                .delegation(&headers, tenant_id, "runtime.query.agent_sessions")
        else {
            continue;
        };
        let application_id: Option<Uuid> = candidate.try_get("application_id")?;
        if claims.tenant_wide
            || application_id.is_some_and(|id| claims.application_ids.contains(&id))
        {
            selected = Some((candidate, tenant_id));
            break;
        }
    }
    let (row, tenant_id) = selected.ok_or(RuntimeError::NotFound)?;
    let summary = agent_session_summary(&row)?;
    let entries = sqlx::query("SELECT entry_id,session_id,lane,parent_entry_id,entry_kind,payload_json,operation_id,created_at FROM agent_session_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? ORDER BY sequence_number,entry_id LIMIT 1000")
        .bind(tenant_id).bind(&session_key).bind(&node_key).fetch_all(&state.pool).await?
        .into_iter().map(|row| {
            let entry_kind: String = row.try_get("entry_kind")?;
            let payload: Option<Value> = row.try_get("payload_json")?;
            Ok(AgentSessionEntryViewV1 {
                entry_id: row.try_get("entry_id")?,
                session_id: row.try_get("session_id")?,
                lane: row.try_get("lane")?,
                parent_entry_id: row.try_get("parent_entry_id")?,
                entry_kind: entry_kind.clone(),
                payload: diagnostic_entry_payload(&entry_kind, payload),
                operation_id: row.try_get("operation_id")?,
                created_at: row.try_get("created_at")?,
            })
        }).collect::<RuntimeResult<Vec<_>>>()?;
    let usages = sqlx::query("SELECT usage_id,operation_id,effect_id,usage_kind,input_tokens,output_tokens,cost_micros,cost_currency,created_at FROM agent_session_usages WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? ORDER BY created_at,usage_id LIMIT 1000")
        .bind(tenant_id).bind(&session_key).bind(&node_key).fetch_all(&state.pool).await?
        .into_iter().map(|row| Ok(json!({"usageId":row.try_get::<String,_>("usage_id")?,"operationId":row.try_get::<String,_>("operation_id")?,"effectId":row.try_get::<String,_>("effect_id")?,"usageKind":row.try_get::<String,_>("usage_kind")?,"inputTokens":row.try_get::<u64,_>("input_tokens")?,"outputTokens":row.try_get::<u64,_>("output_tokens")?,"costMicros":row.try_get::<u64,_>("cost_micros")?,"costCurrency":row.try_get::<Option<String>,_>("cost_currency")?,"createdAt":row.try_get::<time::OffsetDateTime,_>("created_at")?}))).collect::<RuntimeResult<Vec<_>>>()?;
    let recovery = sqlx::query("SELECT operation_json,phase,recovery_action,updated_at FROM agent_session_operations WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? ORDER BY updated_at DESC LIMIT 1")
        .bind(tenant_id).bind(&session_key).bind(&node_key).fetch_optional(&state.pool).await?.map(|row| {
            let mut operation = row.try_get::<Value, _>("operation_json").unwrap_or(Value::Null);
            if let Some(object) = operation.as_object_mut() {
                object.remove("intent");
                object.remove("effectIdentity");
            }
            json!({
                "phase":row.try_get::<String,_>("phase").unwrap_or_default(),
                "recoveryAction":row.try_get::<Option<String>,_>("recovery_action").unwrap_or_default(),
                "operation":operation,
                "updatedAt":row.try_get::<time::OffsetDateTime,_>("updated_at").ok()
            })
        });
    let register_json: Value = row.try_get("register_json")?;
    let compaction = register_json
        .get("compaction")
        .filter(|value| !value.is_null())
        .cloned();
    Ok(Json(AgentSessionDiagnosticV1 {
        api_version: 1,
        summary,
        entries,
        usages,
        compaction,
        recovery,
    }))
}

fn diagnostic_entry_payload(entry_kind: &str, payload: Option<Value>) -> Option<Value> {
    let payload = payload?;
    let hash = crate::worker_support::raw_hash(&payload);
    match entry_kind {
        // Tool results and external contexts may contain provider secrets or
        // prompt-injected data. Diagnostics expose provenance and bounded
        // metadata, never the full content.
        "message.tool_result" | "custom.external_context" => Some(json!({
            "redacted": true,
            "contentHash": hash,
            "entryKind": entry_kind,
        })),
        "message.tool_call" => {
            let call_id = payload.get("toolCallId").cloned().unwrap_or(Value::Null);
            let tool_name = payload
                .get("toolCalls")
                .and_then(Value::as_array)
                .and_then(|calls| calls.first())
                .and_then(|call| call.get("name"))
                .cloned()
                .unwrap_or(Value::Null);
            Some(json!({
                "redacted": true,
                "contentHash": hash,
                "toolCallId": call_id,
                "toolName": tool_name,
            }))
        }
        _ => Some(payload),
    }
}

pub async fn clear_agent_session(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<AgentSessionClearRequestV1>,
) -> RuntimeResult<Json<AgentSessionClearReceiptV1>> {
    let claims = state
        .trust
        .delegation(&headers, request.tenant_id, "runtime.sessions.clear")?;
    // The request idempotency key is part of the clear identity.  A stable
    // audit id lets a retry return the original receipt even though the
    // Session rows themselves have already been removed.
    let clear_target = format!(
        "agent-session-clear:{}:{}:{}",
        request.session_key, request.stable_agent_node_key, request.idempotency_key
    );
    let audit_id = crate::worker_support::stable_id(request.tenant_id, clear_target.as_bytes());
    let mut tx = state.pool.begin().await?;
    if let Some(existing) = sqlx::query(
        "SELECT detail_json FROM audit_events WHERE tenant_id=? AND id=? AND action='agent_session.clear'",
    )
    .bind(request.tenant_id)
    .bind(audit_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        let detail: Value = existing.try_get("detail_json")?;
        let cleared_entries = detail
            .get("entries")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let cleared_pending = detail
            .get("pending")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let application_id = detail
            .get("applicationId")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok());
        if !claims.tenant_wide
            && !application_id.is_some_and(|id| claims.application_ids.contains(&id))
        {
            return Err(RuntimeError::Unauthorized);
        }
        tx.rollback().await?;
        return Ok(Json(AgentSessionClearReceiptV1 {
            api_version: 1,
            session_key: request.session_key,
            stable_agent_node_key: request.stable_agent_node_key,
            cleared_entries,
            cleared_pending,
            audit_id,
        }));
    }
    let row = sqlx::query("SELECT application_id FROM agent_session_registers WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? FOR UPDATE")
        .bind(request.tenant_id)
        .bind(&request.session_key)
        .bind(&request.stable_agent_node_key)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RuntimeError::NotFound)?;
    let application_id: Option<Uuid> = row.try_get("application_id")?;
    if !claims.tenant_wide && !application_id.is_some_and(|id| claims.application_ids.contains(&id))
    {
        return Err(RuntimeError::Unauthorized);
    }
    // MySQL exposes COUNT as a signed BIGINT through the text protocol even
    // when the expression is cast to UNSIGNED. Decode using the signed type
    // and convert at the API boundary to avoid sqlx runtime type mismatch.
    let entries_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_session_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?").bind(request.tenant_id).bind(&request.session_key).bind(&request.stable_agent_node_key).fetch_one(&mut *tx).await?;
    let artifact_ids = sqlx::query_scalar::<_, Uuid>("SELECT payload_artifact_id FROM agent_session_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? AND payload_artifact_id IS NOT NULL")
        .bind(request.tenant_id)
        .bind(&request.session_key)
        .bind(&request.stable_agent_node_key)
        .fetch_all(&mut *tx)
        .await?;
    let pending_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_session_pending_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?").bind(request.tenant_id).bind(&request.session_key).bind(&request.stable_agent_node_key).fetch_one(&mut *tx).await?;
    let entries = u64::try_from(entries_count).unwrap_or_default();
    let pending = u64::try_from(pending_count).unwrap_or_default();
    sqlx::query("DELETE FROM agent_session_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?")
        .bind(request.tenant_id)
        .bind(&request.session_key)
        .bind(&request.stable_agent_node_key)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM agent_session_pending_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?")
        .bind(request.tenant_id)
        .bind(&request.session_key)
        .bind(&request.stable_agent_node_key)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM agent_session_operations WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?")
        .bind(request.tenant_id)
        .bind(&request.session_key)
        .bind(&request.stable_agent_node_key)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM agent_session_usages WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?")
        .bind(request.tenant_id)
        .bind(&request.session_key)
        .bind(&request.stable_agent_node_key)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM agent_session_registers WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?").bind(request.tenant_id).bind(&request.session_key).bind(&request.stable_agent_node_key).execute(&mut *tx).await?;
    for artifact_id in artifact_ids {
        sqlx::query("UPDATE runtime_objects SET status='deleted',deleted_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND object_id=? AND status='ready' AND NOT EXISTS (SELECT 1 FROM bundle_objects WHERE tenant_id=? AND object_id=?) AND NOT EXISTS (SELECT 1 FROM checkpoint_artifacts WHERE tenant_id=? AND artifact_id=?)")
            .bind(request.tenant_id)
            .bind(artifact_id)
            .bind(request.tenant_id)
            .bind(artifact_id)
            .bind(request.tenant_id)
            .bind(artifact_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("INSERT INTO audit_events(id,tenant_id,actor_user_id,action,target_type,target_id,request_id,detail_json) VALUES(?,?,?,'agent_session.clear','agent_session',?,?,?)")
        .bind(audit_id)
        .bind(request.tenant_id)
        .bind(claims.sub)
        .bind(format!("{}:{}", request.session_key, request.stable_agent_node_key))
        .bind(audit_id)
        .bind(json!({"idempotencyKey":request.idempotency_key,"applicationId":application_id,"entries":entries,"pending":pending}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(AgentSessionClearReceiptV1 {
        api_version: 1,
        session_key: request.session_key,
        stable_agent_node_key: request.stable_agent_node_key,
        cleared_entries: entries,
        cleared_pending: pending,
        audit_id,
    }))
}

pub async fn clear_agent_subject_memory(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<AgentSubjectMemoryClearRequestV1>,
) -> RuntimeResult<Json<AgentSubjectMemoryClearReceiptV1>> {
    let claims = state
        .trust
        .delegation(&headers, request.tenant_id, "runtime.memory.clear")?;
    if !claims.tenant_wide && !claims.application_ids.contains(&request.application_id) {
        return Err(RuntimeError::Unauthorized);
    }
    let scope_hash = crate::worker_support::raw_hash(&json!({
        "tenantId": request.tenant_id,
        "applicationId": request.application_id,
        "subjectId": claims.sub,
        "memoryVersion": request.memory_resource_version_id,
    }));
    let clear_id = crate::worker_support::stable_id(
        request.tenant_id,
        format!(
            "agent-subject-memory-clear:{}:{}",
            scope_hash, request.idempotency_key
        )
        .as_bytes(),
    );
    let audit_id = crate::worker_support::stable_id(
        request.tenant_id,
        format!("agent-subject-memory-audit:{}", clear_id).as_bytes(),
    );
    let mut tx = state.pool.begin().await?;
    if let Some(existing) = sqlx::query(
        "SELECT clear_id,audit_id,scope_hash,status FROM agent_subject_memory_clears WHERE tenant_id=? AND idempotency_key=?",
    )
    .bind(request.tenant_id)
    .bind(&request.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        let existing_scope: String = existing.try_get("scope_hash")?;
        if existing_scope != scope_hash {
            return Err(RuntimeError::Conflict(
                agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Memory clear idempotency key is bound to another scope".into(),
            ));
        }
        let existing_id: Uuid = existing.try_get("clear_id")?;
        let existing_audit_id: Uuid = existing.try_get("audit_id")?;
        let status: String = existing.try_get("status")?;
        return Ok(Json(AgentSubjectMemoryClearReceiptV1 {
            api_version: 1,
            clear_id: existing_id,
            audit_id: existing_audit_id,
            scope_hash,
            status,
        }));
    }
    // Runtime tombstones are the authoritative P3-05 clear boundary.  The
    // provider adapter checks them before every Effect, so the clear is
    // applied atomically with its audit record and cannot be resurrected by a
    // retry or a stale Worker.
    sqlx::query("INSERT INTO agent_subject_memory_clears(clear_id,audit_id,tenant_id,application_id,authenticated_subject_id,memory_resource_version_id,scope_hash,idempotency_key,status,applied_at) VALUES(?,?,?,?,?,?,?,?,'applied',UTC_TIMESTAMP(6))")
        .bind(clear_id)
        .bind(audit_id)
        .bind(request.tenant_id)
        .bind(request.application_id)
        .bind(claims.sub)
        .bind(request.memory_resource_version_id)
        .bind(&scope_hash)
        .bind(&request.idempotency_key)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO agent_subject_memory_audit(audit_id,tenant_id,application_id,authenticated_subject_id,memory_resource_version_id,operation,scope_hash,result_hash) VALUES(?,?,?,?,?,'clear',?,NULL)")
        .bind(audit_id)
        .bind(request.tenant_id)
        .bind(request.application_id)
        .bind(claims.sub)
        .bind(request.memory_resource_version_id)
        .bind(&scope_hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(AgentSubjectMemoryClearReceiptV1 {
        api_version: 1,
        clear_id,
        audit_id,
        scope_hash,
        status: "applied".into(),
    }))
}

pub async fn search_agent_subject_memory(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<AgentSubjectMemorySearchRequestV1>,
) -> RuntimeResult<Json<AgentSubjectMemorySearchPageV1>> {
    let claims = state.trust.delegation(
        &headers,
        request.tenant_id,
        "runtime.query.agent_subject_memory",
    )?;
    if !claims.tenant_wide && !claims.application_ids.contains(&request.application_id) {
        return Err(RuntimeError::Unauthorized);
    }
    let limit = request.limit.clamp(1, 100);
    let rows = sqlx::query(
        "SELECT audit_id,operation,scope_hash,operation_id,created_at FROM agent_subject_memory_audit WHERE tenant_id=? AND application_id=? AND authenticated_subject_id=? AND memory_resource_version_id=? AND (? IS NULL OR CONCAT(DATE_FORMAT(created_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'|',HEX(audit_id))>?) ORDER BY created_at,audit_id LIMIT ?",
    )
    .bind(request.tenant_id)
    .bind(request.application_id)
    .bind(claims.sub)
    .bind(request.memory_resource_version_id)
    .bind(&request.after)
    .bind(&request.after)
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let mut items = rows
        .iter()
        .map(|row| {
            Ok(AgentSubjectMemoryAuditViewV1 {
                audit_id: row.try_get("audit_id")?,
                operation: row.try_get("operation")?,
                scope_hash: row.try_get("scope_hash")?,
                operation_id: row.try_get("operation_id")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<RuntimeResult<Vec<_>>>()?;
    let next = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|item| {
            format!(
                "{}|{}",
                item.created_at
                    .format(&time::format_description::well_known::Iso8601::DEFAULT)
                    .unwrap_or_default(),
                item.audit_id.simple()
            )
        })
    } else {
        None
    };
    Ok(Json(AgentSubjectMemorySearchPageV1 {
        api_version: 1,
        items,
        next,
    }))
}

fn agent_session_summary(row: &sqlx::mysql::MySqlRow) -> RuntimeResult<AgentSessionSummaryV1> {
    Ok(AgentSessionSummaryV1 {
        session_key: row.try_get("session_key")?,
        session_id: row.try_get("session_id")?,
        stable_agent_node_key: row.try_get("stable_agent_node_key")?,
        application_id: row.try_get("application_id")?,
        state_version: row.try_get("state_version")?,
        fencing_token: row.try_get("fencing_token")?,
        leaf_entry_id: row.try_get("leaf_entry_id")?,
        open_operation_id: row.try_get("open_operation_id")?,
        terminal_state: row.try_get("terminal_state")?,
        bundle_hash: row.try_get("bundle_hash")?,
        model_version: row.try_get("model_version")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub async fn search_sessions(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<SessionSearchRequestV1>,
) -> RuntimeResult<Json<SessionSearchPageV1>> {
    let claims = state
        .trust
        .delegation(&headers, request.tenant_id, "runtime.query.sessions")?;
    let application_id = request.application_id.ok_or(RuntimeError::Unauthorized)?;
    if !claims.application_ids.contains(&application_id) {
        return Err(RuntimeError::Unauthorized);
    }
    let limit = request.limit.clamp(1, 100);
    let statuses =
        serde_json::to_string(&request.statuses).map_err(|e| RuntimeError::Internal(e.into()))?;
    let rows = sqlx::query("SELECT id,application_id,application_deployment_id,workflow_version_id,bundle_id,version_policy,external_user_id,title,status,version,updated_at FROM application_sessions WHERE tenant_id=? AND application_id=? AND (JSON_LENGTH(?)=0 OR JSON_CONTAINS(?,JSON_QUOTE(status))) AND (? IS NULL OR CONCAT(DATE_FORMAT(updated_at,'%Y-%m-%dT%H:%i:%s.%fZ'),'|',LOWER(HEX(id)))>?) ORDER BY updated_at,id LIMIT ?")
        .bind(request.tenant_id).bind(application_id).bind(&statuses).bind(&statuses).bind(&request.after).bind(&request.after).bind(limit+1).fetch_all(&state.pool).await?;
    let mut items = rows
        .into_iter()
        .map(session_summary)
        .collect::<RuntimeResult<Vec<_>>>()?;
    let next = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items.last().map(|item| {
            format!(
                "{}|{}",
                item.updated_at
                    .format(&time::format_description::well_known::Iso8601::DEFAULT)
                    .unwrap_or_default(),
                item.session_id.simple()
            )
        })
    } else {
        None
    };
    Ok(Json(SessionSearchPageV1 {
        api_version: 1,
        items,
        next,
    }))
}

pub async fn get_session(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> RuntimeResult<Json<SessionDetailV1>> {
    let row = sqlx::query("SELECT tenant_id,id,application_id,application_deployment_id,workflow_version_id,bundle_id,version_policy,status,version,updated_at,external_user_id,title FROM application_sessions WHERE id=?")
        .bind(id).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    let claims = state
        .trust
        .delegation(&headers, tenant_id, "runtime.query.sessions")?;
    let application_id: Uuid = row.try_get("application_id")?;
    if !claims.session_ids.contains(&id) && !claims.application_ids.contains(&application_id) {
        return Err(RuntimeError::Unauthorized);
    }
    let summary = session_summary(&row)?;
    Ok(Json(SessionDetailV1 {
        api_version: 1,
        summary,
        external_user_id: row.try_get("external_user_id")?,
        title: row.try_get("title")?,
    }))
}

pub async fn apply_session_command(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<SessionUpgradeCommandV1>,
) -> RuntimeResult<Json<SessionUpgradeReceiptV1>> {
    let claims = state
        .trust
        .delegation(&headers, request.tenant_id, "runtime.sessions.upgrade")?;
    if !claims.session_ids.contains(&request.session_id)
        && !claims.application_ids.contains(&request.application_id)
    {
        return Err(RuntimeError::Unauthorized);
    }
    if let Some(row) = sqlx::query("SELECT to_bundle_id,to_session_version FROM application_session_version_history WHERE tenant_id=? AND session_id=? AND command_idempotency_key=?")
        .bind(request.tenant_id).bind(request.session_id).bind(&request.idempotency_key).fetch_optional(&state.pool).await? {
        if row.try_get::<Option<Uuid>,_>("to_bundle_id")? != Some(request.target_bundle_id) { return Err(RuntimeError::Conflict(agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict, "Session command key conflicts".into())); }
        return Ok(Json(SessionUpgradeReceiptV1 { api_version:1, session_id:request.session_id, bundle_id:request.target_bundle_id, session_version:row.try_get("to_session_version")?, replayed:true }));
    }
    let mut tx = state.pool.begin().await?;
    let session = sqlx::query("SELECT bundle_id,workflow_version_id,version,version_policy FROM application_sessions WHERE tenant_id=? AND application_id=? AND id=? AND status='active' FOR UPDATE")
        .bind(request.tenant_id).bind(request.application_id).bind(request.session_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    if session.try_get::<String, _>("version_policy")? != "manual_upgrade"
        || session.try_get::<u64, _>("version")? != request.expected_session_version
    {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::HeadVersionConflict,
            "Session Version does not match".into(),
        ));
    }
    let target = sqlx::query("SELECT workflow_version_id FROM deployment_bundles WHERE tenant_id=? AND application_id=? AND id=? AND status IN ('active','superseded','retained')")
        .bind(request.tenant_id).bind(request.application_id).bind(request.target_bundle_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    let old_bundle: Option<Uuid> = session.try_get("bundle_id")?;
    let new_version = request.expected_session_version + 1;
    sqlx::query("UPDATE application_sessions SET bundle_id=?,workflow_version_id=?,version=? WHERE tenant_id=? AND id=? AND version=?")
        .bind(request.target_bundle_id).bind(target.try_get::<Uuid,_>("workflow_version_id")?).bind(new_version).bind(request.tenant_id).bind(request.session_id).bind(request.expected_session_version).execute(&mut *tx).await?;
    if let Some(old) = old_bundle {
        sqlx::query("UPDATE bundle_references SET released_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND bundle_id=? AND reference_kind='pinned_session' AND owner_id=? AND released_at IS NULL").bind(request.tenant_id).bind(old).bind(request.session_id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'pinned_session',?)").bind(Uuid::now_v7()).bind(request.tenant_id).bind(request.target_bundle_id).bind(request.session_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO application_session_version_history(id,tenant_id,session_id,from_workflow_version_id,to_workflow_version_id,from_bundle_id,to_bundle_id,from_session_version,to_session_version,changed_by,command_idempotency_key) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(request.tenant_id).bind(request.session_id).bind(session.try_get::<Option<Uuid>,_>("workflow_version_id")?).bind(target.try_get::<Uuid,_>("workflow_version_id")?).bind(old_bundle).bind(request.target_bundle_id).bind(request.expected_session_version).bind(new_version).bind(request.actor_user_id).bind(&request.idempotency_key).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(SessionUpgradeReceiptV1 {
        api_version: 1,
        session_id: request.session_id,
        bundle_id: request.target_bundle_id,
        session_version: new_version,
        replayed: false,
    }))
}

fn session_summary(
    row: impl std::borrow::Borrow<sqlx::mysql::MySqlRow>,
) -> RuntimeResult<SessionSummaryV1> {
    let row = row.borrow();
    let policy = match row.try_get::<String, _>("version_policy")?.as_str() {
        "pinned" => SessionVersionPolicyV1::Pinned,
        "follow_deployment" => SessionVersionPolicyV1::FollowDeployment,
        "manual_upgrade" => SessionVersionPolicyV1::ManualUpgrade,
        _ => {
            return Err(RuntimeError::Internal(anyhow::anyhow!(
                "invalid Session policy"
            )));
        }
    };
    Ok(SessionSummaryV1 {
        session_id: row.try_get("id")?,
        application_id: row.try_get("application_id")?,
        application_deployment_id: row.try_get("application_deployment_id")?,
        workflow_version_id: row.try_get("workflow_version_id")?,
        bundle_id: row.try_get("bundle_id")?,
        version_policy: policy,
        external_user_id: row.try_get("external_user_id")?,
        title: row.try_get("title")?,
        status: row.try_get("status")?,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}
