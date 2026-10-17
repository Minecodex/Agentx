use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

use crate::trace_delivery::{TraceDraft, bounded_preview, enqueue_best_effort};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_node_attempt_spans(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    node_execution_id: Uuid,
    attempt_id: Uuid,
    node_name: &str,
    input: &Value,
    attempt_number: u16,
    completed_by_overlay: bool,
) {
    let status = if completed_by_overlay {
        "succeeded"
    } else {
        "queued"
    };
    let preview = bounded_preview(input);
    let mut node = TraceDraft::span(
        tenant_id,
        execution_id,
        node_execution_id,
        Some((
            execution_id,
            agentx_runtime_contracts::TraceSpanKindV1::Execution,
        )),
        agentx_runtime_contracts::TraceSpanKindV1::Node,
        node_name,
        if attempt_number == 1 {
            agentx_runtime_contracts::TraceEventKindV1::Started
        } else {
            agentx_runtime_contracts::TraceEventKindV1::Updated
        },
        if attempt_number == 1 {
            "node.started"
        } else {
            "node.retry_started"
        },
        status,
    );
    node.node_execution_id = Some(node_execution_id);
    node.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::NodeInput);
    node.content_preview = preview.clone();
    enqueue_best_effort(tx, node).await;
    let mut attempt = TraceDraft::span(
        tenant_id,
        execution_id,
        attempt_id,
        Some((
            node_execution_id,
            agentx_runtime_contracts::TraceSpanKindV1::Node,
        )),
        agentx_runtime_contracts::TraceSpanKindV1::Attempt,
        format!("Attempt {attempt_number}"),
        agentx_runtime_contracts::TraceEventKindV1::Started,
        "attempt.queued",
        status,
    );
    attempt.node_execution_id = Some(node_execution_id);
    attempt.attempt_id = Some(attempt_id);
    attempt.attributes = json!({"attemptNumber":attempt_number});
    attempt.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::AttemptInput);
    attempt.content_preview = preview;
    enqueue_best_effort(tx, attempt).await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn finish_resumed_spans(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    node_execution_id: Uuid,
    status: &str,
    wait_status: &str,
    output: &Value,
    error_code: Option<&str>,
) -> crate::error::RuntimeResult<()> {
    let node = sqlx::query(
        "SELECT COALESCE(NULLIF(node_name,''),node_key) node_name FROM node_executions WHERE tenant_id=? AND execution_id=? AND id=?",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .bind(node_execution_id)
    .fetch_one(&mut **tx)
    .await?;
    let attempt = sqlx::query("SELECT id,attempt_number FROM node_attempts WHERE tenant_id=? AND execution_id=? AND node_execution_id=? ORDER BY attempt_number DESC LIMIT 1")
        .bind(tenant_id).bind(execution_id).bind(node_execution_id).fetch_optional(&mut **tx).await?;
    let content = bounded_preview(output);
    if let Some(attempt) = attempt {
        let attempt_id: Uuid = attempt.try_get("id")?;
        let mut trace = TraceDraft::span(
            tenant_id,
            execution_id,
            attempt_id,
            Some((
                node_execution_id,
                agentx_runtime_contracts::TraceSpanKindV1::Node,
            )),
            agentx_runtime_contracts::TraceSpanKindV1::Attempt,
            format!("Attempt {}", attempt.try_get::<u32, _>("attempt_number")?),
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "attempt.finished",
            status,
        );
        trace.node_execution_id = Some(node_execution_id);
        trace.attempt_id = Some(attempt_id);
        trace.error_code = error_code.map(str::to_owned);
        trace.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::AttemptOutput);
        trace.content_preview = content.clone();
        enqueue_best_effort(tx, trace).await;
    }
    let mut node_trace = TraceDraft::span(
        tenant_id,
        execution_id,
        node_execution_id,
        Some((
            execution_id,
            agentx_runtime_contracts::TraceSpanKindV1::Execution,
        )),
        agentx_runtime_contracts::TraceSpanKindV1::Node,
        node.try_get::<String, _>("node_name")?,
        agentx_runtime_contracts::TraceEventKindV1::Finished,
        "node.finished",
        status,
    );
    node_trace.node_execution_id = Some(node_execution_id);
    node_trace.error_code = error_code.map(str::to_owned);
    node_trace.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::NodeOutput);
    node_trace.content_preview = content.clone();
    enqueue_best_effort(tx, node_trace).await;

    let approval = sqlx::query("SELECT id,status,title FROM approval_tasks WHERE tenant_id=? AND execution_id=? AND node_execution_id=? ORDER BY created_at DESC LIMIT 1")
        .bind(tenant_id).bind(execution_id).bind(node_execution_id).fetch_optional(&mut **tx).await?;
    if let Some(row) = approval {
        let wait_id: Uuid = row.try_get("id")?;
        let wait_kind = row
            .try_get::<String, _>("wait_kind")
            .unwrap_or_else(|_| "approval".into());
        let wait_name = row
            .try_get::<String, _>("title")
            .unwrap_or_else(|_| wait_kind.clone());
        let terminal_status = if wait_status == "timed_out" {
            "timed_out".to_owned()
        } else {
            row.try_get::<String, _>("status")
                .unwrap_or_else(|_| "succeeded".into())
        };
        let mut trace = TraceDraft::span(
            tenant_id,
            execution_id,
            wait_id,
            Some((
                node_execution_id,
                agentx_runtime_contracts::TraceSpanKindV1::Node,
            )),
            agentx_runtime_contracts::TraceSpanKindV1::Wait,
            if wait_kind == "approval" {
                format!("Approval · {wait_name}")
            } else {
                format!("Wait · {wait_name}")
            },
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "wait.finished",
            terminal_status,
        );
        trace.node_execution_id = Some(node_execution_id);
        trace.wait_id = Some(wait_id);
        trace.attributes = json!({"waitKind":wait_kind,"resumeStatus":wait_status});
        trace.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::WaitResponse);
        trace.content_preview = content;
        enqueue_best_effort(tx, trace).await;
    }
    Ok(())
}

pub(crate) async fn finish_cancelled_spans(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
) -> crate::error::RuntimeResult<()> {
    for row in sqlx::query("SELECT i.id,i.agent_run_id,r.node_execution_id,r.attempt_id,i.iteration_index FROM agent_iterations i JOIN agent_runs r ON r.id=i.agent_run_id WHERE r.tenant_id=? AND r.execution_id=? AND i.status='cancelled'")
        .bind(tenant_id).bind(execution_id).fetch_all(&mut **tx).await?
    {
        let iteration_id: Uuid = row.try_get("id")?;
        let run_id: Uuid = row.try_get("agent_run_id")?;
        let mut trace = TraceDraft::span(
            tenant_id, execution_id, iteration_id,
            Some((run_id, agentx_runtime_contracts::TraceSpanKindV1::AgentRun)),
            agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
            format!("Iteration {}", row.try_get::<u32, _>("iteration_index")? + 1),
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "agent_iteration.cancelled", "cancelled",
        );
        trace.node_execution_id = row.try_get("node_execution_id").ok();
        trace.attempt_id = row.try_get("attempt_id").ok();
        trace.agent_run_id = Some(run_id);
        trace.agent_iteration_id = Some(iteration_id);
        enqueue_best_effort(tx, trace).await;
    }
    for row in sqlx::query("SELECT id,node_execution_id,attempt_id,input_tokens,output_tokens,cost_micros FROM agent_runs WHERE tenant_id=? AND execution_id=? AND status='cancelled'")
        .bind(tenant_id).bind(execution_id).fetch_all(&mut **tx).await?
    {
        let run_id: Uuid = row.try_get("id")?;
        let attempt_id: Uuid = row.try_get("attempt_id")?;
        let mut trace = TraceDraft::span(
            tenant_id, execution_id, run_id,
            Some((attempt_id, agentx_runtime_contracts::TraceSpanKindV1::Attempt)),
            agentx_runtime_contracts::TraceSpanKindV1::AgentRun,
            "Agent run",
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "agent_run.cancelled", "cancelled",
        );
        trace.node_execution_id = row.try_get("node_execution_id").ok();
        trace.attempt_id = Some(attempt_id);
        trace.agent_run_id = Some(run_id);
        trace.input_tokens = row.try_get("input_tokens").ok();
        trace.output_tokens = row.try_get("output_tokens").ok();
        trace.cost_micros = row.try_get("cost_micros").unwrap_or_default();
        enqueue_best_effort(tx, trace).await;
    }
    for row in sqlx::query("SELECT id,node_execution_id,attempt_number FROM node_attempts WHERE tenant_id=? AND execution_id=? AND status='cancelled'")
        .bind(tenant_id).bind(execution_id).fetch_all(&mut **tx).await?
    {
        let attempt_id: Uuid = row.try_get("id")?;
        let node_id: Uuid = row.try_get("node_execution_id")?;
        let mut trace = TraceDraft::span(
            tenant_id, execution_id, attempt_id,
            Some((node_id, agentx_runtime_contracts::TraceSpanKindV1::Node)),
            agentx_runtime_contracts::TraceSpanKindV1::Attempt,
            format!("Attempt {}", row.try_get::<u32, _>("attempt_number")?),
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "attempt.cancelled", "cancelled",
        );
        trace.node_execution_id = Some(node_id);
        trace.attempt_id = Some(attempt_id);
        enqueue_best_effort(tx, trace).await;
    }
    for row in sqlx::query("SELECT id,COALESCE(NULLIF(node_name,''),node_key) node_name FROM node_executions WHERE tenant_id=? AND execution_id=? AND status='cancelled'")
        .bind(tenant_id).bind(execution_id).fetch_all(&mut **tx).await?
    {
        let node_id: Uuid = row.try_get("id")?;
        let mut trace = TraceDraft::span(
            tenant_id, execution_id, node_id,
            Some((execution_id, agentx_runtime_contracts::TraceSpanKindV1::Execution)),
            agentx_runtime_contracts::TraceSpanKindV1::Node,
            row.try_get::<String, _>("node_name")?,
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "node.cancelled", "cancelled",
        );
        trace.node_execution_id = Some(node_id);
        enqueue_best_effort(tx, trace).await;
    }
    for row in sqlx::query("SELECT id,node_execution_id,attempt_id,agent_run_id,iteration_index,plugin_parent_span_entity_id,status,input_tokens,output_tokens,cost_micros FROM runtime_calls WHERE tenant_id=? AND execution_id=? AND status IN ('cancelled','outcome_unknown')")
        .bind(tenant_id).bind(execution_id).fetch_all(&mut **tx).await?
    {
        let call_id: Uuid = row.try_get("id")?;
        let attempt_id: Uuid = row.try_get("attempt_id")?;
        let run_id = row.try_get::<Option<Uuid>, _>("agent_run_id")?;
        let turn = row.try_get::<u32, _>("iteration_index")?;
        let iteration_id = run_id.filter(|_| turn > 0).map(|id| crate::worker_support::agent_turn_entity_id(id, turn));
        let parent = row.try_get::<Option<Uuid>, _>("plugin_parent_span_entity_id")?
            .map(|id| (id, agentx_runtime_contracts::TraceSpanKindV1::PluginOperation))
            .or_else(|| iteration_id.map(|id| (id, agentx_runtime_contracts::TraceSpanKindV1::AgentIteration)))
            .or_else(|| run_id.map(|id| (id, agentx_runtime_contracts::TraceSpanKindV1::AgentRun)))
            .unwrap_or((attempt_id, agentx_runtime_contracts::TraceSpanKindV1::Attempt));
        let status: String = row.try_get("status")?;
        let mut trace = TraceDraft::span(
            tenant_id, execution_id, call_id,
            Some(parent),
            agentx_runtime_contracts::TraceSpanKindV1::RuntimeCall,
            "Runtime call",
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "runtime_call.cancelled", &status,
        );
        trace.node_execution_id = row.try_get("node_execution_id").ok();
        trace.attempt_id = Some(attempt_id);
        trace.agent_run_id = run_id;
        trace.agent_iteration_id = iteration_id;
        trace.runtime_call_id = Some(call_id);
        trace.input_tokens = row.try_get("input_tokens").ok();
        trace.output_tokens = row.try_get("output_tokens").ok();
        trace.cost_micros = row.try_get("cost_micros").unwrap_or_default();
        enqueue_best_effort(tx, trace).await;
    }
    for row in sqlx::query("SELECT id,node_execution_id,attempt_id,profile_version_id,last_error FROM sandbox_leases WHERE tenant_id=? AND execution_id=? AND status='interrupting' AND last_error='execution_cancelled'")
        .bind(tenant_id).bind(execution_id).fetch_all(&mut **tx).await?
    {
        let lease_id: Uuid = row.try_get("id")?;
        let attempt_id: Uuid = row.try_get("attempt_id")?;
        let mut trace = TraceDraft::span(
            tenant_id, execution_id, lease_id,
            Some((attempt_id, agentx_runtime_contracts::TraceSpanKindV1::Attempt)),
            agentx_runtime_contracts::TraceSpanKindV1::Sandbox,
            "OpenSandbox execution",
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "sandbox.cancelled", "cancelled",
        );
        trace.node_execution_id = row.try_get("node_execution_id").ok();
        trace.attempt_id = Some(attempt_id);
        trace.sandbox_lease_id = Some(lease_id);
        trace.resource_type = Some("sandbox_profile".into());
        trace.resource_id = row.try_get("profile_version_id").ok();
        trace.error_code = Some("EXECUTION_CANCELLED".into());
        trace.error_message = Some("Execution was cancelled while the Sandbox was active".into());
        enqueue_best_effort(tx, trace).await;
    }
    for row in sqlx::query("SELECT id,node_execution_id,title wait_kind FROM approval_tasks WHERE tenant_id=? AND execution_id=? AND status='cancelled'")
        .bind(tenant_id).bind(execution_id).fetch_all(&mut **tx).await?
    {
        enqueue_cancelled_wait(tx, tenant_id, execution_id, &row).await?;
    }
    Ok(())
}

async fn enqueue_cancelled_wait(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    row: &sqlx::mysql::MySqlRow,
) -> crate::error::RuntimeResult<()> {
    let wait_id: Uuid = row.try_get("id")?;
    let node_id: Uuid = row.try_get("node_execution_id")?;
    let label: String = row.try_get("wait_kind")?;
    let mut trace = TraceDraft::span(
        tenant_id,
        execution_id,
        wait_id,
        Some((node_id, agentx_runtime_contracts::TraceSpanKindV1::Node)),
        agentx_runtime_contracts::TraceSpanKindV1::Wait,
        format!("Approval · {label}"),
        agentx_runtime_contracts::TraceEventKindV1::Finished,
        "wait.cancelled",
        "cancelled",
    );
    trace.node_execution_id = Some(node_id);
    trace.wait_id = Some(wait_id);
    trace.attributes = json!({"waitKind":"approval"});
    enqueue_best_effort(tx, trace).await;
    Ok(())
}
