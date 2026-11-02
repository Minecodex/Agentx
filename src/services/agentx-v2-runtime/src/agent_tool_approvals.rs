//! Durable permission for one concrete Core Effect. No second approval or task state machine.
use crate::{
    engine::ClaimedWorkerAttempt,
    error::{RuntimeError, RuntimeResult},
};
use agentx_agent_core::{EffectContextV1, ToolCallV1, ToolResultV1};
use agentx_runtime_contracts::{
    ApprovalSubjectV1, RuntimeMcpToolPolicyV1, content_hash, deterministic_uuid,
};
use serde_json::{Value, json};
use sqlx::{MySql, MySqlPool, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

pub(crate) enum Gate {
    Execute { effect: Uuid, call_index: u32 },
    Settled(ToolResultV1),
    Pending(Uuid),
}
pub(crate) struct Proposal<'a> {
    pub claim: &'a ClaimedWorkerAttempt,
    pub run_id: Uuid,
    pub context: &'a EffectContextV1,
    pub call: &'a ToolCallV1,
    pub tool_id: Uuid,
    pub tool_version: &'a str,
    pub tool_name: &'a str,
    pub policy: &'a RuntimeMcpToolPolicyV1,
    pub call_index: u32,
}
fn invalid(code: &'static str, message: &str) -> RuntimeError {
    RuntimeError::InvalidRequest(code, message.into())
}

pub(crate) async fn authorize_policy(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    tool: Uuid,
    policy: &RuntimeMcpToolPolicyV1,
) -> RuntimeResult<()> {
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mcp_tool_policy_projection WHERE tenant_id=? AND tool_id=? AND revision=? AND content_hash=? AND enabled=TRUE)")
        .bind(tenant).bind(tool).bind(policy.revision).bind(policy.content_hash.as_str()).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(invalid(
            "MCP_TOOL_POLICY_CHANGED",
            "The current tool policy was disabled or changed; publish the updated workflow before executing",
        ));
    }
    Ok(())
}

pub(crate) async fn enter(pool: &MySqlPool, proposal: Proposal<'_>) -> RuntimeResult<Gate> {
    let claim = proposal.claim;
    let tenant = claim.task.tenant_id;
    let rule = proposal.policy.approval.as_ref().ok_or_else(|| {
        invalid(
            "TOOL_APPROVAL_POLICY_REQUIRED",
            "The concrete tool does not have an approval policy",
        )
    })?;
    rule.validate()
        .map_err(|message| RuntimeError::InvalidRequest("TOOL_APPROVAL_POLICY_INVALID", message))?;
    if proposal.context.operation_id.len() > 191
        || proposal.context.effect_id.len() > 191
        || proposal.call.call_id.len() > 191
    {
        return Err(invalid(
            "TOOL_APPROVAL_ID_INVALID",
            "The tool operation identity exceeds the supported limit",
        ));
    }
    let hash =
        content_hash(&proposal.call.arguments).map_err(|e| RuntimeError::Internal(e.into()))?;
    let effect = deterministic_uuid(proposal.run_id, proposal.context.effect_id.as_bytes());
    let mut tx = pool.begin().await?;
    authorize_policy(&mut tx, tenant, proposal.tool_id, proposal.policy).await?;
    let execution=sqlx::query("SELECT e.workflow_id,e.status,a.deadline_at,n.node_id FROM workflow_executions e JOIN node_executions n ON n.tenant_id=e.tenant_id AND n.execution_id=e.id JOIN node_attempts a ON a.tenant_id=n.tenant_id AND a.node_execution_id=n.id WHERE e.tenant_id=? AND e.id=? AND n.id=? AND a.id=? AND a.status='running' AND a.lease_token=? AND a.fencing_token=? AND a.locked_until>UTC_TIMESTAMP(6) FOR UPDATE")
        .bind(tenant).bind(claim.task.execution_id).bind(claim.task.node_execution_id).bind(claim.task.attempt_id).bind(claim.lease.worker_id).bind(claim.lease.fencing_token).fetch_optional(&mut *tx).await?.ok_or_else(||invalid("TOOL_APPROVAL_LEASE_LOST","The original tool Attempt is no longer owned by this Worker"))?;
    if matches!(
        execution.try_get::<String, _>("status")?.as_str(),
        "succeeded" | "failed" | "cancelled" | "timed_out"
    ) {
        return Err(invalid(
            "TOOL_APPROVAL_EXECUTION_CLOSED",
            "The original workflow has ended",
        ));
    }
    let intent:Value=sqlx::query_scalar("SELECT operation_json FROM agent_session_operations WHERE tenant_id=? AND operation_id=? AND attempt_id=?")
        .bind(tenant).bind(&proposal.context.operation_id).bind(claim.task.attempt_id).fetch_optional(&mut *tx).await?.ok_or_else(||invalid("TOOL_APPROVAL_INTENT_REQUIRED","The original Core Intent has not been committed"))?;
    if intent["intent"]
        != serde_json::to_value(proposal.call).map_err(|e| RuntimeError::Internal(e.into()))?
    {
        return Err(invalid(
            "TOOL_APPROVAL_INTENT_CONFLICT",
            "The proposal does not match the durable Core Intent",
        ));
    }
    let existing =
        sqlx::query("SELECT * FROM agent_tool_effects WHERE tenant_id=? AND id=? FOR UPDATE")
            .bind(tenant)
            .bind(effect)
            .fetch_optional(&mut *tx)
            .await?;
    let approval = deterministic_uuid(effect, b"approval");
    if let Some(row) = existing {
        if row.try_get::<Uuid, _>("attempt_id")? != claim.task.attempt_id
            || row.try_get::<Uuid, _>("tool_id")? != proposal.tool_id
            || row.try_get::<String, _>("tool_version")? != proposal.tool_version
            || row.try_get::<String, _>("arguments_hash")? != hash.as_str()
            || row.try_get::<Value, _>("policy_json")?
                != serde_json::to_value(proposal.policy)
                    .map_err(|e| RuntimeError::Internal(e.into()))?
        {
            return Err(invalid(
                "TOOL_APPROVAL_EFFECT_CONFLICT",
                "The original Effect cannot be reused with new parameters or a new policy",
            ));
        }
        if row.try_get::<bool, _>("outcome_unknown")? {
            return Err(RuntimeError::StateConflict(
                "TOOL_EFFECT_OUTCOME_UNKNOWN",
                "The original write result must be verified before any retry".into(),
            ));
        }
        if let Some(result) = row.try_get::<Option<Value>, _>("result_json")? {
            return Ok(Gate::Settled(
                serde_json::from_value(result).map_err(|e| RuntimeError::Internal(e.into()))?,
            ));
        }
        let task=sqlx::query("SELECT status,decision_id,decision_reason,decided_by FROM approval_tasks WHERE tenant_id=? AND id=? FOR UPDATE").bind(tenant).bind(approval).fetch_one(&mut *tx).await?;
        let status: String = task.try_get("status")?;
        if matches!(status.as_str(), "pending" | "claimed") {
            return Ok(Gate::Pending(approval));
        }
        if status == "decided"
            && task.try_get::<Option<String>, _>("decision_id")?.as_deref() == Some("approved")
        {
            let actor: Uuid = task.try_get("decided_by")?;
            if !crate::approval_candidates::current(&mut tx, tenant, approval, actor).await? {
                return Err(invalid(
                    "TOOL_APPROVER_REVOKED",
                    "The deciding member no longer has approval eligibility",
                ));
            }
            let index = row.try_get("runtime_call_index")?;
            tx.commit().await?;
            return Ok(Gate::Execute {
                effect,
                call_index: index,
            });
        }
        let result = rejection(
            &status,
            task.try_get::<Option<String>, _>("decision_reason")?
                .as_deref(),
        );
        sqlx::query("UPDATE agent_tool_effects SET result_json=? WHERE tenant_id=? AND id=? AND result_json IS NULL").bind(serde_json::to_value(&result).map_err(|e|RuntimeError::Internal(e.into()))?).bind(tenant).bind(effect).execute(&mut *tx).await?;
        sqlx::query("UPDATE approval_tasks SET resume_status='succeeded',version=version+1 WHERE tenant_id=? AND id=?").bind(tenant).bind(approval).execute(&mut *tx).await?;
        crate::event_export::enqueue_approval_event_from_task(&mut tx, tenant, approval).await?;
        tx.commit().await?;
        return Ok(Gate::Settled(result));
    }
    let repetitions:i64=sqlx::query_scalar("SELECT COUNT(*) FROM agent_tool_effects WHERE tenant_id=? AND agent_run_id=? AND tool_id=? AND arguments_hash=?").bind(tenant).bind(proposal.run_id).bind(proposal.tool_id).bind(hash.as_str()).fetch_one(&mut *tx).await?;
    if repetitions >= 3 {
        return Ok(Gate::Settled(ToolResultV1 {
            content: "Repeated unchanged tool approval proposals exceeded the run budget".into(),
            structured_result: Some(json!({"code":"TOOL_APPROVAL_REPEATED_PROPOSAL"})),
            artifact_refs: vec![],
            truncated: false,
            is_error: true,
            terminate: true,
        }));
    }
    let mut arguments = proposal.call.arguments.clone();
    redact(&mut arguments);
    let request = json!({"toolName":proposal.tool_name,"toolId":proposal.tool_id,"arguments":arguments,"argumentsHash":hash,"effectId":effect});
    if request.to_string().len() > 65536 {
        return Err(invalid(
            "TOOL_APPROVAL_PARAMETERS_TOO_LARGE",
            "Use authorized Artifact references to keep the complete approval parameters under 64 KiB",
        ));
    }
    let deadline: OffsetDateTime = execution.try_get("deadline_at")?;
    let remaining = (deadline - OffsetDateTime::now_utc()).whole_milliseconds();
    if remaining <= 0 {
        return Err(invalid(
            "TOOL_APPROVAL_ACTIVE_BUDGET_EXPIRED",
            "The Agent active execution deadline has expired",
        ));
    }
    let subject = ApprovalSubjectV1::AgentToolEffect {
        effect_id: effect,
        agent_run_id: proposal.run_id,
        operation_id: proposal.context.operation_id.clone(),
        tool_call_id: proposal.call.call_id.clone(),
        tool_id: proposal.tool_id,
        tool_version: proposal.tool_version.into(),
        tool_name: proposal.tool_name.into(),
        arguments_hash: hash.clone(),
        policy_revision: proposal.policy.revision,
        policy_hash: proposal.policy.content_hash.clone(),
    };
    sqlx::query("INSERT INTO agent_tool_effects(id,tenant_id,execution_id,node_execution_id,attempt_id,agent_run_id,operation_id,effect_id,tool_call_id,tool_id,tool_version,tool_name,arguments_json,arguments_hash,policy_json,runtime_call_index,active_remaining_ms,approval_task_id) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(effect).bind(tenant).bind(claim.task.execution_id).bind(claim.task.node_execution_id).bind(claim.task.attempt_id).bind(proposal.run_id).bind(&proposal.context.operation_id).bind(&proposal.context.effect_id).bind(&proposal.call.call_id).bind(proposal.tool_id).bind(proposal.tool_version).bind(proposal.tool_name).bind(&proposal.call.arguments).bind(hash.as_str()).bind(serde_json::to_value(proposal.policy).map_err(|e|RuntimeError::Internal(e.into()))?).bind(proposal.call_index).bind(remaining as u64).bind(approval).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO approval_tasks(id,tenant_id,execution_id,node_execution_id,bundle_id,checkpoint_id,workflow_id,node_id,title,description,request_payload_json,buttons_json,subject_json,tool_effect_id,status,resume_status,deadline_at) VALUES(?,?,?,?,?,NULL,?,?,?,?,?,?,?,?,'pending','not_requested',TIMESTAMPADD(SECOND,?,UTC_TIMESTAMP(6)))")
        .bind(approval).bind(tenant).bind(claim.task.execution_id).bind(claim.task.node_execution_id).bind(claim.task.bundle_id).bind(execution.try_get::<Uuid,_>("workflow_id")?).bind(execution.try_get::<String,_>("node_id")?).bind(format!("Approve {}",proposal.tool_name).chars().take(255).collect::<String>()).bind("Approve only this concrete tool operation and its frozen parameters").bind(request).bind(json!([{"id":"approved","label":"Approve"},{"id":"rejected","label":"Reject"}])).bind(serde_json::to_value(subject).map_err(|e|RuntimeError::Internal(e.into()))?).bind(effect).bind(rule.timeout_seconds).execute(&mut *tx).await?;
    for (kind, ids) in [("user", &rule.user_ids), ("role", &rule.role_ids)] {
        for id in ids {
            sqlx::query("INSERT IGNORE INTO approval_candidates(tenant_id,approval_task_id,candidate_type,candidate_id) VALUES(?,?,?,?)").bind(tenant).bind(approval).bind(kind).bind(id).execute(&mut *tx).await?;
        }
    }
    crate::event_export::enqueue_approval_event_from_task(&mut tx, tenant, approval).await?;
    let context = crate::workflow_interaction::context(
        &mut tx,
        tenant,
        claim.task.execution_id,
        &execution.try_get::<String, _>("node_id")?,
    )
    .await?;
    crate::workflow_interaction::event(
        &mut tx,
        tenant,
        claim.task.execution_id,
        &context,
        "workflow.approval_required",
        json!({"approvalId":approval,"effectId":effect,"kind":"agent_tool_effect"}),
    )
    .await?;
    tx.commit().await?;
    Ok(Gate::Pending(approval))
}

pub(crate) fn rejection(status: &str, reason: Option<&str>) -> ToolResultV1 {
    let code = match status {
        "timed_out" => "TOOL_APPROVAL_TIMED_OUT",
        "cancelled" => "TOOL_APPROVAL_CANCELLED",
        _ => "TOOL_APPROVAL_REJECTED",
    };
    let result = json!({"code":code,"reason":reason,"executed":false,"newProposalRequired":true});
    ToolResultV1 {
        content: result.to_string(),
        structured_result: Some(result),
        artifact_refs: vec![],
        truncated: false,
        is_error: true,
        terminate: status == "cancelled",
    }
}
fn redact(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let key = key.to_ascii_lowercase();
                if [
                    "password",
                    "secret",
                    "token",
                    "authorization",
                    "api_key",
                    "apikey",
                    "credential",
                ]
                .iter()
                .any(|name| key.contains(name))
                {
                    *value = Value::String("[redacted]".into())
                } else {
                    redact(value)
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value)
            }
        }
        _ => {}
    }
}

/// Record a definitive authorization denial only while this Worker still owns
/// the original unsent Effect. An existing receipt always keeps precedence.
pub(crate) async fn deny_unsent(
    pool: &MySqlPool,
    claim: &ClaimedWorkerAttempt,
    run: Uuid,
    context: &EffectContextV1,
    failure: &ToolResultV1,
) -> RuntimeResult<ToolResultV1> {
    let tenant = claim.task.tenant_id;
    let effect = deterministic_uuid(run, context.effect_id.as_bytes());
    let mut tx = pool.begin().await?;
    let owned: Option<Uuid> = sqlx::query_scalar("SELECT a.id FROM node_attempts a JOIN workflow_executions e ON e.tenant_id=a.tenant_id AND e.id=a.execution_id WHERE a.tenant_id=? AND a.id=? AND a.status='running' AND a.lease_token=? AND a.fencing_token=? AND a.locked_until>UTC_TIMESTAMP(6) AND a.deadline_at>UTC_TIMESTAMP(6) AND e.status NOT IN ('succeeded','failed','cancelled','timed_out') FOR UPDATE")
        .bind(tenant).bind(claim.task.attempt_id).bind(claim.lease.worker_id).bind(claim.lease.fencing_token).fetch_optional(&mut *tx).await?;
    if owned.is_none() {
        return Err(RuntimeError::StateConflict(
            "TOOL_EFFECT_OUTCOME_UNKNOWN",
            "The original Worker no longer owns this operation".into(),
        ));
    }
    let row = sqlx::query("SELECT approval_task_id,runtime_call_index,result_json,outcome_unknown FROM agent_tool_effects WHERE tenant_id=? AND id=? AND attempt_id=? AND agent_run_id=? AND operation_id=? AND effect_id=? FOR UPDATE")
        .bind(tenant).bind(effect).bind(claim.task.attempt_id).bind(run).bind(&context.operation_id).bind(&context.effect_id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        return Ok(failure.clone());
    };
    if let Some(saved) = row.try_get::<Option<Value>, _>("result_json")? {
        return serde_json::from_value(saved).map_err(|error| RuntimeError::Internal(error.into()));
    }
    let index: u32 = row.try_get("runtime_call_index")?;
    let dispatched: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_calls WHERE tenant_id=? AND attempt_id=? AND agent_run_id=? AND status<>'reserved' AND ((call_kind='mcp_tool' AND call_index=?) OR (call_kind='sandbox' AND JSON_UNQUOTE(JSON_EXTRACT(request_json,'$.effectId'))=? AND JSON_UNQUOTE(JSON_EXTRACT(request_json,'$.frame.method'))='tools/call')))")
        .bind(tenant).bind(claim.task.attempt_id).bind(run).bind(index).bind(&context.effect_id).fetch_one(&mut *tx).await?;
    if row.try_get::<bool, _>("outcome_unknown")? || dispatched {
        return Err(RuntimeError::StateConflict(
            "TOOL_EFFECT_OUTCOME_UNKNOWN",
            "The dispatched operation must retain its original result for verification".into(),
        ));
    }
    sqlx::query("UPDATE agent_tool_effects SET result_json=? WHERE tenant_id=? AND id=? AND result_json IS NULL AND outcome_unknown=FALSE")
        .bind(serde_json::to_value(failure).map_err(|error| RuntimeError::Internal(error.into()))?).bind(tenant).bind(effect).execute(&mut *tx).await?;
    let approval: Uuid = row.try_get("approval_task_id")?;
    sqlx::query("UPDATE approval_tasks SET resume_status='succeeded',version=version+1 WHERE tenant_id=? AND id=?")
        .bind(tenant).bind(approval).execute(&mut *tx).await?;
    crate::event_export::enqueue_approval_event_from_task(&mut tx, tenant, approval).await?;
    tx.commit().await?;
    Ok(failure.clone())
}

pub(crate) async fn settle(
    pool: &MySqlPool,
    tenant: Uuid,
    effect: Uuid,
    result: Option<&ToolResultV1>,
    unknown: bool,
) -> RuntimeResult<()> {
    let mut tx = pool.begin().await?;
    let row=sqlx::query("SELECT approval_task_id,result_json,outcome_unknown FROM agent_tool_effects WHERE tenant_id=? AND id=? FOR UPDATE").bind(tenant).bind(effect).fetch_one(&mut *tx).await?;
    let result = result
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| RuntimeError::Internal(e.into()))?;
    if let Some(saved) = row.try_get::<Option<Value>, _>("result_json")? {
        if Some(saved) != result {
            return Err(invalid(
                "TOOL_EFFECT_RECEIPT_CONFLICT",
                "The concrete Effect already has a different result receipt",
            ));
        }
        return Ok(());
    }
    sqlx::query(
        "UPDATE agent_tool_effects SET result_json=?,outcome_unknown=? WHERE tenant_id=? AND id=?",
    )
    .bind(result)
    .bind(unknown)
    .bind(tenant)
    .bind(effect)
    .execute(&mut *tx)
    .await?;
    let approval: Uuid = row.try_get("approval_task_id")?;
    sqlx::query(
        "UPDATE approval_tasks SET resume_status=?,version=version+1 WHERE tenant_id=? AND id=?",
    )
    .bind(if unknown {
        "blocked_runtime"
    } else {
        "succeeded"
    })
    .bind(tenant)
    .bind(approval)
    .execute(&mut *tx)
    .await?;
    crate::event_export::enqueue_approval_event_from_task(&mut tx, tenant, approval).await?;
    tx.commit().await?;
    Ok(())
}

/// Called in the Worker-result transaction, so a decision cannot wake a still-running Attempt.
pub(crate) async fn pause_budget(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    attempt: Uuid,
) -> RuntimeResult<bool> {
    let row=sqlx::query("SELECT p.deadline_at,f.active_remaining_ms FROM agent_tool_effects f JOIN approval_tasks p ON p.tenant_id=f.tenant_id AND p.id=f.approval_task_id WHERE f.tenant_id=? AND f.attempt_id=? AND f.result_json IS NULL AND f.outcome_unknown=FALSE ORDER BY f.created_at DESC LIMIT 1 FOR UPDATE")
        .bind(tenant).bind(attempt).fetch_optional(&mut **tx).await?;
    let Some(row) = row else { return Ok(false) };
    let deadline: OffsetDateTime = row.try_get("deadline_at")?;
    let remaining: u64 = row.try_get("active_remaining_ms")?;
    let until = deadline
        + time::Duration::milliseconds(remaining.min(i64::MAX as u64) as i64)
        + time::Duration::minutes(1);
    crate::quota::pause_attempt(tx, tenant, attempt, until).await?;
    // Keep application-session ownership while a human owns the pending decision.
    sqlx::query("UPDATE agent_session_registers r JOIN agent_session_operations o ON o.tenant_id=r.tenant_id AND o.operation_id=r.open_operation_id SET r.lease_expires_at=? WHERE o.tenant_id=? AND o.attempt_id=?").bind(until).bind(tenant).bind(attempt).execute(&mut **tx).await?;
    Ok(true)
}

pub async fn wake_decided(pool: &MySqlPool, limit: u32) -> RuntimeResult<u32> {
    let mut tx = pool.begin().await?;
    let rows=sqlx::query("SELECT f.id,f.tenant_id,f.execution_id FROM agent_tool_effects f JOIN approval_tasks p ON p.tenant_id=f.tenant_id AND p.id=f.approval_task_id JOIN node_attempts a ON a.tenant_id=f.tenant_id AND a.id=f.attempt_id JOIN workflow_executions e ON e.tenant_id=f.tenant_id AND e.id=f.execution_id WHERE f.resume_command_id IS NULL AND a.status='suspended' AND p.status IN ('decided','timed_out','cancelled') AND e.status NOT IN ('succeeded','failed','cancelled','timed_out') ORDER BY p.decided_at,p.id LIMIT ? FOR UPDATE SKIP LOCKED").bind(limit.clamp(1,100)).fetch_all(&mut *tx).await?;
    for row in &rows {
        let effect: Uuid = row.try_get("id")?;
        let tenant: Uuid = row.try_get("tenant_id")?;
        let execution: Uuid = row.try_get("execution_id")?;
        let command = deterministic_uuid(effect, b"resume");
        sqlx::query("INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'resume_execution','execution',?,?,?,'pending')").bind(command).bind(tenant).bind(execution.to_string()).bind(format!("tool-effect:{effect}:resume")).bind(json!({"agentToolEffectId":effect})).execute(&mut *tx).await?;
        sqlx::query("UPDATE agent_tool_effects SET resume_command_id=? WHERE tenant_id=? AND id=? AND resume_command_id IS NULL").bind(command).bind(tenant).bind(effect).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(rows.len() as u32)
}

/// Authorization and the durable Sent boundary share a transaction. No business call is sent first.
async fn authorize_dispatch(
    tx: &mut Transaction<'_, MySql>,
    claim: &ClaimedWorkerAttempt,
    binding: &agentx_runtime_contracts::RuntimeResourceBindingV1,
    call_index: u32,
) -> RuntimeResult<()> {
    use agentx_runtime_contracts::{
        RuntimeAuthorizationSnapshotV1, RuntimeResourceConfigurationV1,
    };
    let RuntimeResourceConfigurationV1::Mcp {
        tool_policy: Some(policy),
        server_id,
        server_version_id,
        credential,
        ..
    } = &binding.configuration
    else {
        return Ok(());
    };
    authorize_policy(tx, claim.task.tenant_id, binding.resource_id, policy).await?;
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM node_attempts a JOIN workflow_executions e ON e.tenant_id=a.tenant_id AND e.id=a.execution_id WHERE a.tenant_id=? AND a.id=? AND a.status='running' AND a.lease_token=? AND a.fencing_token=? AND a.locked_until>UTC_TIMESTAMP(6) AND a.deadline_at>UTC_TIMESTAMP(6) AND e.status NOT IN ('succeeded','failed','cancelled','timed_out'))")
        .bind(claim.task.tenant_id).bind(claim.task.attempt_id).bind(claim.lease.worker_id).bind(claim.lease.fencing_token).fetch_one(&mut **tx).await?;
    if !active {
        return Err(invalid(
            "TOOL_EFFECT_LEASE_LOST",
            "The original operation was cancelled or its Worker ownership expired",
        ));
    }
    let mut resources = vec![binding.clone()];
    for dependency in &claim.resources {
        let server = dependency.resource_id == *server_id
            && dependency.resource_version == server_version_id.to_string();
        let secret = matches!(&dependency.configuration,RuntimeResourceConfigurationV1::Credential{secret,..} if credential.as_ref().is_some_and(|reference|serde_json::to_value(reference).ok()==serde_json::to_value(secret).ok()));
        if server || secret {
            resources.push(dependency.clone());
        }
    }
    crate::engine_persistence::authorize_resources(tx, claim.task.tenant_id, &resources).await?;
    let snapshot:Value=sqlx::query_scalar("SELECT authorization_snapshot_json FROM execution_snapshots WHERE tenant_id=? AND execution_id=?").bind(claim.task.tenant_id).bind(claim.task.execution_id).fetch_one(&mut **tx).await?;
    let mut authorization: RuntimeAuthorizationSnapshotV1 =
        serde_json::from_value(snapshot).map_err(|e| RuntimeError::Internal(e.into()))?;
    authorization.grant_ids.retain(|id| {
        authorization.grant_bindings.iter().any(|grant| {
            grant.grant_id == *id
                && resources
                    .iter()
                    .any(|resource| resource.resource_id == grant.resource_id)
        })
    });
    crate::engine_persistence::authorize_snapshot(tx, &authorization).await?;
    if policy.approval.is_some() {
        let approval=sqlx::query("SELECT p.id,p.decided_by FROM agent_tool_effects f JOIN approval_tasks p ON p.tenant_id=f.tenant_id AND p.id=f.approval_task_id WHERE f.tenant_id=? AND f.attempt_id=? AND f.tool_id=? AND f.runtime_call_index=? AND f.outcome_unknown=FALSE AND f.result_json IS NULL AND p.status='decided' AND p.decision_id='approved' FOR UPDATE")
            .bind(claim.task.tenant_id).bind(claim.task.attempt_id).bind(binding.resource_id).bind(call_index).fetch_optional(&mut **tx).await?.ok_or_else(||invalid("TOOL_APPROVAL_REQUIRED","The concrete operation has no valid approval"))?;
        if !crate::approval_candidates::current(
            tx,
            claim.task.tenant_id,
            approval.try_get("id")?,
            approval.try_get("decided_by")?,
        )
        .await?
        {
            return Err(invalid(
                "TOOL_APPROVER_REVOKED",
                "The approver no longer has current eligibility",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn recheck_dispatch(
    pool: &MySqlPool,
    claim: &ClaimedWorkerAttempt,
    binding: &agentx_runtime_contracts::RuntimeResourceBindingV1,
    call_index: u32,
) -> RuntimeResult<()> {
    let mut tx = pool.begin().await?;
    authorize_dispatch(&mut tx, claim, binding, call_index).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn mark_sent(
    pool: &MySqlPool,
    claim: &ClaimedWorkerAttempt,
    binding: &agentx_runtime_contracts::RuntimeResourceBindingV1,
    call_index: u32,
    call_id: Uuid,
) -> RuntimeResult<()> {
    let mut tx = pool.begin().await?;
    authorize_dispatch(&mut tx, claim, binding, call_index).await?;
    let changed = sqlx::query(
        "UPDATE runtime_calls SET status='sent' WHERE id=? AND tenant_id=? AND status='reserved'",
    )
    .bind(call_id)
    .bind(claim.task.tenant_id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(invalid(
            "TOOL_EFFECT_DISPATCH_CONFLICT",
            "The ledger is no longer reserved for this operation",
        ));
    }
    tx.commit().await?;
    Ok(())
}
