use agentx_runtime_contracts::{
    TraceContentKindV1, TraceEventEnvelopeV1, TraceEventKindV1, TraceSpanKindV1, content_hash,
    deterministic_uuid,
};
use redis::{AsyncCommands, aio::ConnectionManager};
use serde_json::{Value, json};
use sqlx::{Executor, MySql, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

pub const TRACE_STREAM: &str = "agentx:v2:trace:v1";

pub struct TraceDraft {
    pub tenant_id: Uuid,
    pub execution_id: Uuid,
    pub event_type: String,
    pub event_kind: TraceEventKindV1,
    pub span_kind: TraceSpanKindV1,
    pub span_name: String,
    pub span_id: Uuid,
    pub parent_span_id: Option<Uuid>,
    pub status: String,
    pub node_execution_id: Option<Uuid>,
    pub attempt_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub agent_iteration_id: Option<Uuid>,
    pub runtime_call_id: Option<Uuid>,
    pub sandbox_lease_id: Option<Uuid>,
    pub wait_id: Option<Uuid>,
    pub resource_type: Option<String>,
    pub resource_id: Option<Uuid>,
    pub resource_version: Option<String>,
    pub duration_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_micros: u64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attributes: Value,
    pub content_ref: Option<Uuid>,
    pub content_kind: Option<TraceContentKindV1>,
    pub content_preview: Option<Value>,
    pub occurred_at: OffsetDateTime,
}

impl TraceDraft {
    pub fn execution(
        tenant_id: Uuid,
        execution_id: Uuid,
        event_type: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        let event_type = event_type.into();
        let status = status.into();
        let event_kind = if event_type == "execution.accepted" {
            TraceEventKindV1::Started
        } else if matches!(
            status.as_str(),
            "succeeded" | "failed" | "cancelled" | "timed_out"
        ) {
            TraceEventKindV1::Finished
        } else {
            TraceEventKindV1::Updated
        };
        Self {
            tenant_id,
            execution_id,
            event_type,
            event_kind,
            span_kind: TraceSpanKindV1::Execution,
            span_name: "Workflow execution".into(),
            span_id: trace_span_id(execution_id, TraceSpanKindV1::Execution),
            parent_span_id: None,
            status,
            node_execution_id: None,
            attempt_id: None,
            agent_run_id: None,
            agent_iteration_id: None,
            runtime_call_id: None,
            sandbox_lease_id: None,
            wait_id: None,
            resource_type: None,
            resource_id: None,
            resource_version: None,
            duration_ms: None,
            input_tokens: None,
            output_tokens: None,
            cost_micros: 0,
            error_code: None,
            error_message: None,
            attributes: json!({}),
            content_ref: None,
            content_kind: None,
            content_preview: None,
            occurred_at: OffsetDateTime::now_utc(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn span(
        tenant_id: Uuid,
        execution_id: Uuid,
        entity_id: Uuid,
        parent_entity: Option<(Uuid, TraceSpanKindV1)>,
        span_kind: TraceSpanKindV1,
        span_name: impl Into<String>,
        event_kind: TraceEventKindV1,
        event_type: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        let mut draft = Self::execution(tenant_id, execution_id, event_type, status);
        draft.event_kind = event_kind;
        draft.span_kind = span_kind;
        draft.span_name = span_name.into();
        draft.span_id = trace_span_id(entity_id, span_kind);
        draft.parent_span_id = parent_entity.map(|(id, kind)| trace_span_id(id, kind));
        draft
    }
}

pub fn trace_span_id(entity_id: Uuid, kind: TraceSpanKindV1) -> Uuid {
    let kind = match kind {
        TraceSpanKindV1::Execution => "execution",
        TraceSpanKindV1::Boundary => "boundary",
        TraceSpanKindV1::Node => "node",
        TraceSpanKindV1::Attempt => "attempt",
        TraceSpanKindV1::AgentRun => "agent_run",
        TraceSpanKindV1::AgentIteration => "agent_iteration",
        TraceSpanKindV1::RuntimeCall => "runtime_call",
        TraceSpanKindV1::Sandbox => "sandbox",
        TraceSpanKindV1::Wait => "wait",
        TraceSpanKindV1::PluginOperation => "plugin_operation",
    };
    deterministic_uuid(entity_id, format!("agentx-trace-span-v1:{kind}").as_bytes())
}

pub fn boundary_entity_id(execution_id: Uuid, boundary: &str) -> Uuid {
    deterministic_uuid(
        execution_id,
        format!("agentx-trace-boundary-v1:{boundary}").as_bytes(),
    )
}

pub fn bounded_preview(value: &Value) -> Option<Value> {
    let mut preview = value.clone();
    redact_preview(&mut preview);
    serde_json::to_vec(&preview)
        .ok()
        .filter(|encoded| encoded.len() <= 16 * 1024)
        .map(|_| preview)
}

fn redact_preview(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
                let usage_counter = matches!(
                    normalized.as_str(),
                    "inputtokens" | "outputtokens" | "totaltokens" | "maxtokens"
                );
                if !usage_counter
                    && (normalized.contains("password")
                        || normalized.contains("secret")
                        || normalized.contains("token")
                        || normalized.contains("authorization")
                        || normalized.contains("apikey")
                        || normalized.contains("credential"))
                {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_preview(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(redact_preview),
        _ => {}
    }
}

/// Adds a Trace event without allowing observability failure to roll back Runtime state.
pub async fn enqueue_best_effort(tx: &mut Transaction<'_, MySql>, draft: TraceDraft) {
    // MySQL rejects SAVEPOINT when it is sent through the prepared-statement
    // protocol. Executing a raw statement keeps this best-effort boundary on
    // the text protocol while all data-bearing statements remain prepared.
    if let Err(error) = (&mut **tx).execute("SAVEPOINT agentx_trace_event").await {
        tracing::warn!(%error, "Trace savepoint creation failed");
        return;
    }
    match enqueue(tx, draft).await {
        Ok(_) => {
            if let Err(error) = (&mut **tx)
                .execute("RELEASE SAVEPOINT agentx_trace_event")
                .await
            {
                tracing::warn!(%error, "Trace savepoint release failed");
            }
        }
        Err(error) => {
            let rollback = (&mut **tx)
                .execute("ROLLBACK TO SAVEPOINT agentx_trace_event")
                .await;
            let _ = (&mut **tx)
                .execute("RELEASE SAVEPOINT agentx_trace_event")
                .await;
            tracing::warn!(%error, rollback_error = ?rollback.err(), "Trace enqueue failed; Runtime state will continue");
        }
    }
}

pub async fn enqueue(tx: &mut Transaction<'_, MySql>, draft: TraceDraft) -> RuntimeResult<u64> {
    if !draft.attributes.is_object() || draft.attributes.to_string().len() > 16 * 1024 {
        return Err(RuntimeError::BadRequest(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::UnsupportedCapability,
            "Trace attributes exceed the reviewed object budget".into(),
        ));
    }
    // Read the immutable Trace identity without locking the Runtime authority
    // row. The sequence itself is allocated atomically below, immediately
    // before the outbox writes, so Envelope construction no longer holds an
    // execution-row lock across the initial read/modify/write cycle.
    let identity = sqlx::query(
        "SELECT e.trace_id,e.workflow_id,i.application_id FROM workflow_executions e LEFT JOIN application_invocations i ON i.tenant_id=e.tenant_id AND i.id=e.invocation_id WHERE e.tenant_id=? AND e.id=?",
    )
    .bind(draft.tenant_id)
    .bind(draft.execution_id)
    .fetch_one(&mut **tx)
    .await?;
    let trace_id: Uuid = identity.try_get("trace_id")?;
    let workflow_id: Option<Uuid> = identity.try_get("workflow_id")?;
    let application_id: Option<Uuid> = identity.try_get("application_id")?;
    let changed = sqlx::query(
        "UPDATE workflow_executions SET trace_watermark=LAST_INSERT_ID(trace_watermark+1) WHERE tenant_id=? AND id=?",
    )
    .bind(draft.tenant_id)
    .bind(draft.execution_id)
    .execute(&mut **tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::NotFound);
    }
    // LAST_INSERT_ID(expr) is scoped to the transaction's pinned MySQL
    // connection and therefore returns exactly the value allocated above.
    let sequence: u64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()")
        .fetch_one(&mut **tx)
        .await?;
    let event_id = Uuid::now_v7();
    let occurred_at = draft.occurred_at;
    let unsigned = json!({
        "schemaVersion":1,"eventId":event_id,"tenantId":draft.tenant_id,
        "executionId":draft.execution_id,"executionSequence":sequence,
        "traceId":trace_id,"workflowId":workflow_id,"applicationId":application_id,"spanId":draft.span_id,
        "parentSpanId":draft.parent_span_id,"eventKind":draft.event_kind,
        "spanKind":draft.span_kind,"spanName":draft.span_name,
        "nodeExecutionId":draft.node_execution_id,
        "attemptId":draft.attempt_id,"agentRunId":draft.agent_run_id,
        "agentIterationId":draft.agent_iteration_id,"runtimeCallId":draft.runtime_call_id,
        "sandboxLeaseId":draft.sandbox_lease_id,"waitId":draft.wait_id,
        "resourceType":draft.resource_type,"resourceId":draft.resource_id,
        "resourceVersion":draft.resource_version,"eventType":draft.event_type,
        "status":draft.status,"durationMs":draft.duration_ms,"inputTokens":draft.input_tokens,
        "outputTokens":draft.output_tokens,"costMicros":draft.cost_micros,
        "errorCode":draft.error_code,"errorMessage":draft.error_message,
        "attributes":draft.attributes,
        "contentRef":draft.content_ref,"contentKind":draft.content_kind,
        "contentPreview":draft.content_preview,"occurredAt":occurred_at
    });
    let hash = content_hash(&unsigned).map_err(|error| RuntimeError::Internal(error.into()))?;
    let event_summary = json!({
        "nodeExecutionId":draft.node_execution_id,"attemptId":draft.attempt_id,
        "runtimeCallId":draft.runtime_call_id,"errorCode":draft.error_code,
        "errorMessage":draft.error_message,
        "attributes":draft.attributes
    });
    let envelope = TraceEventEnvelopeV1 {
        schema_version: 1,
        event_id,
        tenant_id: draft.tenant_id,
        execution_id: draft.execution_id,
        execution_sequence: sequence,
        trace_id,
        workflow_id,
        application_id,
        span_id: draft.span_id,
        parent_span_id: draft.parent_span_id,
        event_kind: draft.event_kind,
        span_kind: draft.span_kind,
        span_name: draft.span_name,
        node_execution_id: draft.node_execution_id,
        attempt_id: draft.attempt_id,
        agent_run_id: draft.agent_run_id,
        agent_iteration_id: draft.agent_iteration_id,
        runtime_call_id: draft.runtime_call_id,
        sandbox_lease_id: draft.sandbox_lease_id,
        wait_id: draft.wait_id,
        resource_type: draft.resource_type,
        resource_id: draft.resource_id,
        resource_version: draft.resource_version,
        event_type: draft.event_type,
        status: draft.status,
        duration_ms: draft.duration_ms,
        input_tokens: draft.input_tokens,
        output_tokens: draft.output_tokens,
        cost_micros: draft.cost_micros,
        error_code: draft.error_code,
        error_message: draft.error_message,
        attributes: draft.attributes,
        content_ref: draft.content_ref,
        content_kind: draft.content_kind,
        content_preview: draft.content_preview,
        occurred_at,
        content_hash: hash.clone(),
    };
    let bytes =
        serde_json::to_vec(&envelope).map_err(|error| RuntimeError::Internal(error.into()))?;
    if bytes.len() > 64 * 1024 {
        return Err(RuntimeError::BadRequest(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::UnsupportedCapability,
            "Trace Envelope exceeds 64 KiB".into(),
        ));
    }
    sqlx::query("INSERT INTO trace_outbox(event_id,tenant_id,execution_id,execution_sequence,payload_json,content_hash,status) VALUES(?,?,?,?,?,?,'pending')")
        .bind(event_id).bind(draft.tenant_id).bind(draft.execution_id).bind(sequence)
        .bind(serde_json::to_value(&envelope).map_err(|error|RuntimeError::Internal(error.into()))?)
        .bind(hash.as_str()).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO execution_events(tenant_id,execution_id,sequence_number,event_type,status,summary_json,occurred_at) VALUES(?,?,?,?,?,?,?)")
        .bind(draft.tenant_id).bind(draft.execution_id).bind(sequence).bind(&envelope.event_type)
        .bind(&envelope.status).bind(event_summary)
        .bind(occurred_at).execute(&mut **tx).await?;
    Ok(sequence)
}

#[derive(Clone, Debug)]
pub struct TraceClaim {
    pub event_id: Uuid,
    pub owner_id: Uuid,
    pub fencing_token: u64,
    pub envelope: TraceEventEnvelopeV1,
}

pub async fn claim(pool: &sqlx::MySqlPool, owner_id: Uuid) -> RuntimeResult<Option<TraceClaim>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT event_id,payload_json FROM trace_outbox WHERE status IN ('pending','failed') AND available_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY created_at,event_id LIMIT 1 FOR UPDATE SKIP LOCKED")
        .fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let event_id: Uuid = row.try_get("event_id")?;
    sqlx::query("UPDATE trace_outbox SET status='pending',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),heartbeat_at=UTC_TIMESTAMP(6),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE event_id=? AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6))")
        .bind(owner_id).bind(event_id).execute(&mut *tx).await?;
    let fencing_token: u64 =
        sqlx::query_scalar("SELECT fencing_token FROM trace_outbox WHERE event_id=?")
            .bind(event_id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    let envelope = serde_json::from_value(row.try_get("payload_json")?)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    Ok(Some(TraceClaim {
        event_id,
        owner_id,
        fencing_token,
        envelope,
    }))
}

pub async fn publish(redis: &mut ConnectionManager, claim: &TraceClaim) -> RuntimeResult<String> {
    let payload = serde_json::to_string(&claim.envelope)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let stream_id = redis::cmd("XADD")
        .arg(TRACE_STREAM)
        .arg("*")
        .arg("event_id")
        .arg(claim.event_id.to_string())
        .arg("content_hash")
        .arg(claim.envelope.content_hash.as_str())
        .arg("payload")
        .arg(payload)
        .query_async::<String>(redis)
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    Ok(stream_id)
}

pub async fn complete(
    pool: &sqlx::MySqlPool,
    claim: &TraceClaim,
    stream_id: &str,
) -> RuntimeResult<()> {
    let changed = sqlx::query("UPDATE trace_outbox SET status='streamed',stream_id=?,streamed_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL,heartbeat_at=NULL,last_error=NULL WHERE event_id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6) AND status='pending'")
        .bind(stream_id).bind(claim.event_id).bind(claim.owner_id).bind(claim.fencing_token).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Trace Outbox Lease was lost".into(),
        ));
    }
    Ok(())
}

pub async fn fail(pool: &sqlx::MySqlPool, claim: &TraceClaim, error: &str) -> RuntimeResult<()> {
    let message = error.chars().take(1000).collect::<String>();
    let delay = (1_u64 << claim.fencing_token.min(6)).min(60);
    let changed = sqlx::query("UPDATE trace_outbox SET status='failed',available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),locked_by=NULL,locked_until=NULL,heartbeat_at=NULL,last_error=? WHERE event_id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(delay).bind(message).bind(claim.event_id).bind(claim.owner_id).bind(claim.fencing_token).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Trace Outbox Lease was lost".into(),
        ));
    }
    Ok(())
}

pub async fn ensure_stream(redis: &mut ConnectionManager) -> RuntimeResult<bool> {
    let exists: bool = redis
        .exists(TRACE_STREAM)
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    if !exists {
        let _: String = redis::cmd("XADD")
            .arg(TRACE_STREAM)
            .arg("MAXLEN")
            .arg("~")
            .arg(1_000_000)
            .arg("*")
            .arg("bootstrap")
            .arg("1")
            .query_async(redis)
            .await
            .map_err(|error| RuntimeError::Internal(error.into()))?;
    }
    Ok(!exists)
}

pub async fn requeue_after_stream_loss(pool: &sqlx::MySqlPool) -> RuntimeResult<u64> {
    let changed = sqlx::query(
        "UPDATE trace_outbox SET status='pending',available_at=UTC_TIMESTAMP(6),stream_id=NULL,streamed_at=NULL,locked_by=NULL,locked_until=NULL,heartbeat_at=NULL,last_error='runtime_redis_stream_rebuilt' WHERE status='streamed'",
    )
    .execute(pool)
    .await?;
    Ok(changed.rows_affected())
}

#[cfg(test)]
mod tests {
    use agentx_runtime_contracts::{TraceSpanKindV1, deterministic_uuid};
    use serde_json::json;
    use uuid::Uuid;

    use super::{bounded_preview, trace_span_id};

    #[test]
    fn span_ids_use_the_versioned_kind_namespace() {
        let entity_id = Uuid::parse_str("018f0000-0000-7000-8000-000000000001").unwrap();
        assert_eq!(
            trace_span_id(entity_id, TraceSpanKindV1::AgentIteration),
            deterministic_uuid(entity_id, b"agentx-trace-span-v1:agent_iteration")
        );
        assert_ne!(
            trace_span_id(entity_id, TraceSpanKindV1::AgentIteration),
            trace_span_id(entity_id, TraceSpanKindV1::AgentRun)
        );
    }

    #[test]
    fn inline_previews_are_recursively_redacted_and_bounded() {
        let preview = bounded_preview(&json!({
            "authorization":"Bearer value",
            "nested":{"api_key":"value","safe":"visible"},
            "items":[{"password":"value"}],
            "usage":{"input_tokens":12,"outputTokens":8,"total_tokens":20,"maxTokens":100}
        }))
        .unwrap();
        assert_eq!(preview["authorization"], "[REDACTED]");
        assert_eq!(preview["nested"]["api_key"], "[REDACTED]");
        assert_eq!(preview["nested"]["safe"], "visible");
        assert_eq!(preview["items"][0]["password"], "[REDACTED]");
        assert_eq!(preview["usage"]["input_tokens"], 12);
        assert_eq!(preview["usage"]["outputTokens"], 8);
        assert_eq!(preview["usage"]["total_tokens"], 20);
        assert_eq!(preview["usage"]["maxTokens"], 100);
        assert!(bounded_preview(&json!({"value":"x".repeat(17 * 1024)})).is_none());
    }
}
