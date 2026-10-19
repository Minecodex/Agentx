use agentx_domain::NodeExecutionId;
use agentx_runtime::{ExecutionMachine, ExpressionContext, ExpressionEngine, NodeActivation};
use serde_json::{Value, json};
use sqlx::{MySql, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    engine_persistence::{load_output_namespace, upsert_failed_activation},
    engine_protocol::machine_error,
    error::{RuntimeError, RuntimeResult},
};

pub(crate) struct SuspendRequest<'a> {
    pub tenant_id: Uuid,
    pub execution_id: Uuid,
    pub bundle_id: Uuid,
    pub work_package_id: Option<Uuid>,
    pub node_execution_id: NodeExecutionId,
    pub attempt_id: Uuid,
    pub state_version: u64,
    pub node: &'a agentx_runtime::CompiledNode,
    pub activation: &'a NodeActivation,
    pub machine: &'a mut ExecutionMachine,
    pub context: &'a Value,
}

pub(crate) async fn resolve_and_create(
    tx: &mut Transaction<'_, MySql>,
    mut request: SuspendRequest<'_>,
) -> RuntimeResult<()> {
    let current = request
        .activation
        .inputs
        .values()
        .flat_map(|items| items.iter())
        .next()
        .map(|item| item.json.clone())
        .unwrap_or(Value::Null);
    let execution_input: Value =
        sqlx::query_scalar("SELECT input_json FROM workflow_executions WHERE tenant_id=? AND id=?")
            .bind(request.tenant_id)
            .bind(request.execution_id)
            .fetch_one(&mut **tx)
            .await?;
    let resolved_parameters = ExpressionEngine.resolve_parameters(
        &request.node.parameters,
        &ExpressionContext {
            json: current.clone(),
            input: current.clone(),
            inputs: execution_input,
            outputs: load_output_namespace(tx, request.tenant_id, request.execution_id).await?,
            loop_context: request.activation.loop_frame.clone().unwrap_or(Value::Null),
            contexts: request.context.clone(),
            execution: crate::execution_context::with_node(
                crate::execution_context::load(tx, request.tenant_id, request.execution_id).await?,
                &request.node.id,
                request.node_execution_id.as_uuid(),
                request.activation.run_index,
                None,
                None,
            ),
            run_index: request.activation.run_index,
            output_node_keys: request
                .machine
                .workflow()
                .nodes
                .iter()
                .map(|node| (node.id.clone(), node.key.clone()))
                .collect(),
            ..ExpressionContext::default()
        },
    );
    let resolved_parameters = match resolved_parameters {
        Ok(parameters) => parameters,
        Err(error) => {
            fail_suspended_node(
                tx,
                &mut request,
                "DYNAMIC_VALUE_EVALUATION_FAILED",
                error.to_string(),
            )
            .await?;
            return Ok(());
        }
    };
    let result = create(
        tx,
        request.tenant_id,
        request.execution_id,
        request.bundle_id,
        request.work_package_id,
        request.node_execution_id,
        request.state_version,
        request.node,
        &resolved_parameters,
        &current,
        request.machine,
        request.context,
    )
    .await;
    match result {
        Err(RuntimeError::InvalidRequest(code, message)) => {
            fail_suspended_node(tx, &mut request, code, message).await
        }
        result => result,
    }
}

async fn fail_suspended_node(
    tx: &mut Transaction<'_, MySql>,
    request: &mut SuspendRequest<'_>,
    code: &str,
    message: String,
) -> RuntimeResult<()> {
    request
        .machine
        .fail(request.node_execution_id, code, &message, false)
        .map_err(machine_error)?;
    let failed = request
        .machine
        .activation(request.node_execution_id)
        .cloned()
        .ok_or_else(|| RuntimeError::Internal(anyhow::anyhow!("activation disappeared")))?;
    upsert_failed_activation(
        tx,
        request.tenant_id,
        request.execution_id,
        &failed,
        request.node,
        code,
        &message,
    )
    .await?;
    sqlx::query("UPDATE node_attempts SET status='failed',error_code=?,error_message=?,ended_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND status='suspended'")
        .bind(code)
        .bind(&message)
        .bind(request.tenant_id)
        .bind(request.attempt_id)
        .execute(&mut **tx)
        .await?;
    let mut trace = crate::trace_delivery::TraceDraft::execution(
        request.tenant_id,
        request.execution_id,
        "node.failed",
        "failed",
    );
    trace.node_execution_id = Some(request.node_execution_id.as_uuid());
    trace.attempt_id = Some(request.attempt_id);
    trace.error_code = Some(code.into());
    trace.error_message = Some(message);
    crate::trace_delivery::enqueue_best_effort(tx, trace).await;
    Ok(())
}

pub async fn enqueue_due(pool: &sqlx::MySqlPool, _owner: Uuid, limit: u32) -> RuntimeResult<u64> {
    let mut tx = pool.begin().await?;
    let approvals = sqlx::query(
        "SELECT id,tenant_id,execution_id,node_execution_id,request_payload_json FROM approval_tasks WHERE status IN ('pending','claimed') AND deadline_at<=UTC_TIMESTAMP(6) ORDER BY deadline_at,id LIMIT ? FOR UPDATE SKIP LOCKED",
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(&mut *tx)
    .await?;
    for row in &approvals {
        use sqlx::Row;
        let task_id: Uuid = row.try_get("id")?;
        let tenant_id: Uuid = row.try_get("tenant_id")?;
        let execution_id: Uuid = row.try_get("execution_id")?;
        let node_execution_id: Uuid = row.try_get("node_execution_id")?;
        let input: Option<Value> = row.try_get("request_payload_json")?;
        sqlx::query("UPDATE approval_tasks SET status='timed_out',resume_status='pending',version=version+1,locked_by=NULL,locked_until=NULL WHERE tenant_id=? AND id=? AND status IN ('pending','claimed')")
            .bind(tenant_id).bind(task_id).execute(&mut *tx).await?;
        sqlx::query("INSERT IGNORE INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'resume_execution','execution',?,?,?,'pending')")
            .bind(crate::engine_names::deterministic_uuid(task_id, b"approval-timeout-command"))
            .bind(tenant_id).bind(execution_id.to_string()).bind(format!("approval-timeout:{task_id}"))
            .bind(json!({"nodeExecutionId":node_execution_id,"outputPort":"timed_out","waitStatus":"timed_out","payload":approval_timeout_output(task_id,input.unwrap_or(Value::Null))}))
            .execute(&mut *tx).await?;
        crate::event_export::enqueue_approval_event_from_task(&mut tx, tenant_id, task_id).await?;
    }
    tx.commit().await?;
    Ok(approvals.len() as u64 + crate::input_waits::enqueue_due(pool, limit).await?)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    bundle_id: Uuid,
    work_package_id: Option<Uuid>,
    node_execution_id: NodeExecutionId,
    state_version: u64,
    node: &agentx_runtime::CompiledNode,
    parameters: &Value,
    input: &Value,
    machine: &ExecutionMachine,
    context: &Value,
) -> RuntimeResult<()> {
    let checkpoint_id = crate::engine::persist_checkpoint(
        tx,
        tenant_id,
        execution_id,
        bundle_id,
        work_package_id,
        state_version,
        Some(node_execution_id),
        machine,
        context,
        "node_suspended",
    )
    .await?;
    if node.node_type == "await_input" {
        crate::input_waits::create(
            tx,
            tenant_id,
            execution_id,
            node_execution_id.as_uuid(),
            checkpoint_id,
            &node.id,
            parameters,
        )
        .await?;
    } else if node.node_type == "approval" {
        let task_id = Uuid::now_v7();
        let workflow_id: Uuid = sqlx::query_scalar(
            "SELECT workflow_id FROM workflow_executions WHERE tenant_id=? AND id=?",
        )
        .bind(tenant_id)
        .bind(execution_id)
        .fetch_one(&mut **tx)
        .await?;
        let title = parameters
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(&node.name)
            .to_owned();
        let description = parameters
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let request = Some(input.clone());
        let (timeout_microseconds, timeout_at) = approval_timeout(parameters)?;
        // The button set is snapshotted with the task so later workflow edits
        // can never desync a pending task from the branches it was created
        // with; absent buttons default to the frozen approve/reject pair.
        let buttons = parameters.get("buttons").cloned().unwrap_or_else(|| {
            json!([
                {"id":"approved","label":"Approve"},
                {"id":"rejected","label":"Reject"}
            ])
        });
        sqlx::query(
            "INSERT INTO approval_tasks(id,tenant_id,execution_id,node_execution_id,bundle_id,checkpoint_id,workflow_id,node_id,title,description,request_payload_json,buttons_json,status,resume_status,deadline_at,version) VALUES(?,?,?,?,?,?,?,?,?,?,?,?, 'pending','not_requested',COALESCE(?,TIMESTAMPADD(MICROSECOND,?,UTC_TIMESTAMP(6))),1)",
        )
        .bind(task_id)
        .bind(tenant_id)
        .bind(execution_id)
        .bind(node_execution_id.as_uuid())
        .bind(bundle_id)
        .bind(checkpoint_id)
        .bind(workflow_id)
        .bind(&node.id)
        .bind(&title)
        .bind(&description)
        .bind(&request)
        .bind(&buttons)
        .bind(timeout_at)
        .bind(timeout_microseconds)
        .execute(&mut **tx)
        .await?;
        if let Some(candidate_id) = parameters
            .get("candidateUserId")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
        {
            sqlx::query("INSERT INTO approval_candidates(tenant_id,approval_task_id,candidate_type,candidate_id) VALUES(?,?,'user',?)")
                .bind(tenant_id)
                .bind(task_id)
                .bind(candidate_id)
                .execute(&mut **tx)
                .await?;
        }
        crate::event_export::enqueue_approval_event_from_task(tx, tenant_id, task_id).await?;
        emit_wait_started(
            tx,
            tenant_id,
            execution_id,
            node_execution_id,
            task_id,
            "approval",
            &title,
            request.as_ref(),
        )
        .await;
    } else {
        // Suspend nodes must be explicitly backed by a durable domain record.
        return Err(RuntimeError::Internal(anyhow::anyhow!(
            "node type {} cannot suspend",
            node.node_type
        )));
    }
    Ok(())
}

fn approval_timeout(parameters: &Value) -> RuntimeResult<(Option<i64>, Option<OffsetDateTime>)> {
    let timeout_microseconds = parameters
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .map(|value| i64::try_from(value.saturating_mul(1_000)))
        .transpose()
        .map_err(|_| {
            RuntimeError::BadRequest(
                agentx_runtime_contracts::RuntimePublishErrorCodeV1::BundleReferenceConflict,
                "Approval timeoutMs is too large".into(),
            )
        })?;
    Ok((timeout_microseconds, None))
}

fn approval_timeout_output(task_id: Uuid, input: Value) -> Value {
    json!({
        "taskId": task_id,
        "decision": "timed_out",
        "reason": null,
        "decidedBy": null,
        "input": input,
    })
}

#[allow(clippy::too_many_arguments)]
async fn emit_wait_started(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    node_execution_id: NodeExecutionId,
    wait_id: Uuid,
    wait_kind: &str,
    span_name: &str,
    content: Option<&Value>,
) {
    let mut trace = crate::trace_delivery::TraceDraft::span(
        tenant_id,
        execution_id,
        wait_id,
        Some((
            node_execution_id.as_uuid(),
            agentx_runtime_contracts::TraceSpanKindV1::Node,
        )),
        agentx_runtime_contracts::TraceSpanKindV1::Wait,
        format!("Approval · {span_name}"),
        agentx_runtime_contracts::TraceEventKindV1::Started,
        "wait.started",
        "waiting",
    );
    trace.node_execution_id = Some(node_execution_id.as_uuid());
    trace.wait_id = Some(wait_id);
    trace.attributes = json!({"waitKind":wait_kind});
    trace.content_kind = content.map(|_| agentx_runtime_contracts::TraceContentKindV1::WaitRequest);
    trace.content_preview = content.and_then(crate::trace_delivery::bounded_preview);
    crate::trace_delivery::enqueue_best_effort(tx, trace).await;
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{approval_timeout, approval_timeout_output};

    #[test]
    fn approval_timeout_consumes_only_milliseconds() {
        let (seconds, deadline) = approval_timeout(&json!({"timeoutMs":1501})).unwrap();
        assert_eq!(seconds, Some(1_501_000));
        assert_eq!(deadline, None);
        assert_eq!(approval_timeout(&json!({})).unwrap(), (None, None));
    }

    #[test]
    fn approval_timeout_output_matches_the_frozen_port_contract() {
        let task_id = Uuid::now_v7();
        assert_eq!(
            approval_timeout_output(task_id, json!({"request":"deploy"})),
            json!({"taskId":task_id,"decision":"timed_out","reason":null,"decidedBy":null,"input":{"request":"deploy"}})
        );
    }
}
