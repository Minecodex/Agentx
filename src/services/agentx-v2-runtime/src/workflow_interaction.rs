use agentx_runtime_contracts::{WorkflowMessageV1, node_path};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

pub(crate) struct InteractionContext {
    pub root_execution_id: Uuid,
    pub invocation_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub node_path: String,
    pub settings: Value,
    pub trigger_context: Option<Value>,
}

/// Follow immutable parent/call-site identities, never display names or the current deployment.
pub(crate) async fn context(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Uuid,
    node_id: &str,
) -> RuntimeResult<InteractionContext> {
    let mut current = execution;
    let mut path = vec![node_id.to_owned()];
    loop {
        let row = sqlx::query("SELECT e.parent_execution_id,e.parent_node_execution_id,e.invocation_id,e.application_id,s.runtime_settings_json FROM workflow_executions e JOIN execution_snapshots s ON s.tenant_id=e.tenant_id AND s.execution_id=e.id WHERE e.tenant_id=? AND e.id=?")
            .bind(tenant).bind(current).fetch_one(&mut **tx).await?;
        if let (Some(parent), Some(call_site_id)) = (
            row.try_get::<Option<Uuid>, _>("parent_execution_id")?,
            row.try_get::<Option<Uuid>, _>("parent_node_execution_id")?,
        ) {
            let call_site: Option<String> = sqlx::query_scalar(
                "SELECT node_id FROM node_executions WHERE tenant_id=? AND execution_id=? AND id=?",
            )
            .bind(tenant)
            .bind(parent)
            .bind(call_site_id)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some(call_site) = call_site {
                path.push(call_site);
                current = parent;
                continue;
            }
        }
        let invocation_id: Option<Uuid> = row.try_get("invocation_id")?;
        let trigger_context = if let Some(id) = invocation_id {
            sqlx::query_scalar("SELECT trigger_context_json FROM application_invocations WHERE tenant_id=? AND id=?")
                .bind(tenant).bind(id).fetch_one(&mut **tx).await?
        } else {
            None
        };
        let path = path
            .iter()
            .rev()
            .fold(String::new(), |path, id| node_path(&path, id));
        return Ok(InteractionContext {
            root_execution_id: current,
            invocation_id,
            application_id: row.try_get("application_id")?,
            node_path: path,
            settings: row.try_get("runtime_settings_json")?,
            trigger_context,
        });
    }
}

pub(crate) async fn event(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Uuid,
    context: &InteractionContext,
    kind: &str,
    payload: Value,
) -> RuntimeResult<()> {
    let wait_id = payload
        .get("waitId")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    let node_execution = payload
        .get("nodeExecutionId")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    let mut trace = if let Some(wait_id) = wait_id {
        use agentx_runtime_contracts::{TraceEventKindV1, TraceSpanKindV1};
        let event_kind = match kind {
            "workflow.input_required" => TraceEventKindV1::Started,
            "workflow.input_finished" => TraceEventKindV1::Finished,
            _ => TraceEventKindV1::Updated,
        };
        let mut trace = crate::trace_delivery::TraceDraft::span(
            tenant,
            execution,
            wait_id,
            node_execution.map(|node| (node, TraceSpanKindV1::Node)),
            TraceSpanKindV1::Wait,
            "Wait for input",
            event_kind,
            kind,
            payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("waiting"),
        );
        trace.wait_id = Some(wait_id);
        trace.content_kind = Some(if kind == "workflow.input_required" {
            agentx_runtime_contracts::TraceContentKindV1::WaitRequest
        } else {
            agentx_runtime_contracts::TraceContentKindV1::WaitResponse
        });
        trace.content_preview = payload
            .get("output")
            .and_then(crate::trace_delivery::bounded_preview);
        trace
    } else {
        crate::trace_delivery::TraceDraft::execution(tenant, execution, kind, "recorded")
    };
    trace.node_execution_id = payload
        .get("nodeExecutionId")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    trace.attributes = payload.clone();
    crate::trace_delivery::enqueue(tx, trace).await?;
    if let Some(invocation) = context.invocation_id {
        crate::engine_persistence::insert_invocation_event(
            tx,
            tenant,
            invocation,
            kind,
            payload.clone(),
        )
        .await?;
    }
    sqlx::query("INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,payload_json,status) VALUES(?,?,?,'runtime_event',?,'pending')")
        .bind(Uuid::now_v7()).bind(tenant).bind(context.root_execution_id)
        .bind(json!({"type":kind,"payload":payload})).execute(&mut **tx).await?;
    Ok(())
}

pub(crate) async fn message(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Uuid,
    node_execution: Uuid,
    content: &str,
) -> RuntimeResult<InteractionContext> {
    let node_id: String =
        sqlx::query_scalar("SELECT node_id FROM node_executions WHERE tenant_id=? AND id=?")
            .bind(tenant)
            .bind(node_execution)
            .fetch_one(&mut **tx)
            .await?;
    let context = context(tx, tenant, execution, &node_id).await?;
    let id = agentx_runtime_contracts::deterministic_uuid(
        execution,
        format!("node:{node_execution}:0").as_bytes(),
    );
    let inserted = sqlx::query("INSERT IGNORE INTO workflow_messages(id,tenant_id,root_execution_id,execution_id,invocation_id,node_execution_id,content) VALUES(?,?,?,?,?,?,?)")
        .bind(id).bind(tenant).bind(context.root_execution_id).bind(execution).bind(context.invocation_id)
        .bind(node_execution).bind(content).execute(&mut **tx).await?;
    if inserted.rows_affected() == 1 {
        event(tx, tenant, execution, &context, "workflow.message", json!({
            "id":id,"executionId":execution,"nodeExecutionId":node_execution,"content":content,"createdAt":timestamp(OffsetDateTime::now_utc())?
        })).await?;
    }
    Ok(context)
}

pub async fn messages(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    invocation: Option<Uuid>,
    root: Option<Uuid>,
) -> RuntimeResult<Vec<WorkflowMessageV1>> {
    let rows = sqlx::query("SELECT id,execution_id,node_execution_id,content,created_at FROM workflow_messages WHERE tenant_id=? AND ((? IS NOT NULL AND invocation_id=?) OR (? IS NOT NULL AND root_execution_id=?)) ORDER BY created_at,id")
        .bind(tenant).bind(invocation).bind(invocation).bind(root).bind(root).fetch_all(pool).await?;
    rows.into_iter().map(message_row).collect()
}

pub(crate) async fn report(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    root: Uuid,
) -> RuntimeResult<Vec<WorkflowMessageV1>> {
    sqlx::query("SELECT id,execution_id,node_execution_id,content,created_at FROM workflow_messages WHERE tenant_id=? AND root_execution_id=? ORDER BY created_at,id")
        .bind(tenant).bind(root).fetch_all(&mut **tx).await?.into_iter().map(message_row).collect()
}

fn message_row(row: sqlx::mysql::MySqlRow) -> RuntimeResult<WorkflowMessageV1> {
    Ok(WorkflowMessageV1 {
        id: row.try_get("id")?,
        execution_id: row.try_get("execution_id")?,
        node_execution_id: row.try_get("node_execution_id")?,
        content: row.try_get("content")?,
        created_at: timestamp(row.try_get("created_at")?)?,
    })
}

pub(crate) fn timestamp(time: OffsetDateTime) -> RuntimeResult<String> {
    time.format(&Rfc3339)
        .map_err(|error| RuntimeError::Internal(error.into()))
}
