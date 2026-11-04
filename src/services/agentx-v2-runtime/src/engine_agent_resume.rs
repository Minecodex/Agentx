//! Resume a suspended Core Effect without changing its Attempt or budget.
use super::*;

pub(super) async fn resume(pool: &MySqlPool, claim: &RuntimeCommandClaim) -> RuntimeResult<()> {
    let effect = claim
        .payload
        .get("agentToolEffectId")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| {
            runtime_bad_request(
                "TOOL_RESUME_INVALID",
                "Tool resume requires agentToolEffectId",
            )
        })?;
    let mut tx = pool.begin().await?;
    let command=sqlx::query("SELECT status,locked_by,fencing_token,COALESCE(locked_until>UTC_TIMESTAMP(6),FALSE) lease_active FROM runtime_commands WHERE id=? FOR UPDATE").bind(claim.command_id).fetch_one(&mut *tx).await?;
    if command.try_get::<String, _>("status")? == "completed" {
        tx.commit().await?;
        return Ok(());
    }
    ensure_command_lease(&command, claim)?;
    let execution=sqlx::query("SELECT e.status,e.bundle_id,e.work_package_id,e.state_version,s.worker_compatibility_json,r.context_json,r.context_version,r.machine_state_json,r.state_version runtime_state_version FROM workflow_executions e JOIN execution_snapshots s ON s.execution_id=e.id JOIN execution_runtime_state r ON r.execution_id=e.id WHERE e.tenant_id=? AND e.id=? FOR UPDATE")
        .bind(claim.tenant_id).bind(claim.execution_id).fetch_one(&mut *tx).await?;
    if matches!(
        execution.try_get::<String, _>("status")?.as_str(),
        "succeeded" | "failed" | "cancelled" | "timed_out"
    ) {
        complete_command(
            &mut tx,
            claim,
            json!({"applied":false,"reason":"execution_closed"}),
        )
        .await?;
        tx.commit().await?;
        return Ok(());
    }
    crate::application_batches::capacity::acquire(&mut tx, claim.tenant_id, claim.execution_id)
        .await?;
    let row=sqlx::query("SELECT f.attempt_id,f.node_execution_id,f.active_remaining_ms,f.resume_command_id,a.status,p.status approval_status FROM agent_tool_effects f JOIN node_attempts a ON a.tenant_id=f.tenant_id AND a.id=f.attempt_id JOIN approval_tasks p ON p.tenant_id=f.tenant_id AND p.id=f.approval_task_id WHERE f.tenant_id=? AND f.id=? AND f.execution_id=? FOR UPDATE")
        .bind(claim.tenant_id).bind(effect).bind(claim.execution_id).fetch_one(&mut *tx).await?;
    if row.try_get::<Option<Uuid>, _>("resume_command_id")? != Some(claim.command_id)
        || row.try_get::<String, _>("status")? != "suspended"
        || !matches!(
            row.try_get::<String, _>("approval_status")?.as_str(),
            "decided" | "timed_out" | "cancelled"
        )
    {
        return Err(lease_conflict(
            "Original tool Effect is not ready to continue",
        ));
    }
    let version: u64 = execution.try_get("state_version")?;
    if version != execution.try_get::<u64, _>("runtime_state_version")? {
        return Err(lease_conflict(
            "Tool resume observed inconsistent machine state",
        ));
    }
    let mut machine: ExecutionMachine =
        serde_json::from_value(execution.try_get("machine_state_json")?)
            .map_err(|e| RuntimeError::Internal(e.into()))?;
    let node_id: Uuid = row.try_get("node_execution_id")?;
    let node_execution_id = NodeExecutionId::from_uuid(node_id);
    let attempt: Uuid = row.try_get("attempt_id")?;
    machine
        .continue_waiting_operation(node_execution_id)
        .map_err(machine_error)?;
    let activation = machine
        .activation(node_execution_id)
        .ok_or_else(|| runtime_bad_request("TOOL_RESUME_INVALID", "Tool activation missing"))?;
    if activation
        .attempts
        .last()
        .map(|attempt| attempt.id.as_uuid())
        != Some(attempt)
    {
        return Err(lease_conflict("Tool resume changed the original Attempt"));
    }
    let node = &machine.workflow().nodes[activation.node_index];
    let remaining: u64 = row.try_get("active_remaining_ms")?;
    let deadline = OffsetDateTime::now_utc()
        + Duration::milliseconds(i64::try_from(remaining).map_err(|_| {
            runtime_bad_request("TOOL_RESUME_INVALID", "Invalid remaining active deadline")
        })?);
    let bundle: Uuid = execution.try_get("bundle_id")?;
    let work_package: Option<Uuid> = execution.try_get("work_package_id")?;
    let compatibility: agentx_runtime_contracts::WorkerCompatibilityV1 =
        serde_json::from_value(execution.try_get("worker_compatibility_json")?)
            .map_err(|e| RuntimeError::Internal(e.into()))?;
    let task = WorkerTaskV1 {
        protocol_version: 1,
        task_id: agentx_runtime_contracts::deterministic_uuid(claim.command_id, b"dispatch"),
        tenant_id: claim.tenant_id,
        execution_id: claim.execution_id,
        node_execution_id: node_id,
        attempt_id: attempt,
        capability: node.capability.clone(),
        bundle_id: bundle,
        work_package_id: work_package,
        state_version: version + 1,
        compatibility_hash: agentx_runtime_contracts::content_hash(&compatibility)
            .map_err(|e| RuntimeError::Internal(e.into()))?,
        deadline_at: deadline,
    };
    let ttl = u32::try_from(remaining / 1000 + 60).map_err(|_| {
        runtime_bad_request("TOOL_RESUME_INVALID", "Invalid tool reservation deadline")
    })?;
    crate::quota::reserve(
        &mut tx,
        claim.tenant_id,
        "node_concurrency",
        "attempt",
        &attempt.to_string(),
        &format!("tool-resume:{}:node", claim.command_id),
        1,
        ttl,
    )
    .await?;
    sqlx::query("UPDATE node_attempts SET status='queued',dispatch_task_id=?,deadline_at=?,lease_token=NULL,locked_until=NULL,heartbeat_at=NULL,ended_at=NULL,output_json=NULL,result_hash=NULL,result_object_id=NULL,error_code=NULL,error_message=NULL WHERE tenant_id=? AND id=? AND status='suspended'")
        .bind(task.task_id).bind(deadline).bind(claim.tenant_id).bind(attempt).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO execution_outbox(id,tenant_id,execution_id,node_execution_id,attempt_id,message_type,capability,payload_json,task_hash,status,available_at) VALUES(?,?,?,?,?,'dispatch_node',?,?,?,'pending',UTC_TIMESTAMP(6))")
        .bind(task.task_id).bind(claim.tenant_id).bind(claim.execution_id).bind(node_id).bind(attempt).bind(node.capability.as_str()).bind(serde_json::to_value(&task).map_err(|e|RuntimeError::Internal(e.into()))?).bind(agentx_runtime_contracts::content_hash(&task).map_err(|e|RuntimeError::Internal(e.into()))?.as_str()).execute(&mut *tx).await?;
    let context: Value = execution.try_get("context_json")?;
    persist_machine(
        &mut tx,
        PersistMachineRequest {
            tenant_id: claim.tenant_id,
            execution_id: claim.execution_id,
            bundle_id: bundle,
            work_package_id: work_package,
            state_version: version + 1,
            context_version: execution.try_get("context_version")?,
            context: &context,
            machine: &machine,
            checkpoint_type: "agent_effect_resumed",
        },
    )
    .await?;
    sqlx::query("UPDATE workflow_executions SET status=?,state_version=? WHERE tenant_id=? AND id=? AND state_version=?").bind(machine_status(machine.status())).bind(version+1).bind(claim.tenant_id).bind(claim.execution_id).bind(version).execute(&mut *tx).await?;
    complete_command(
        &mut tx,
        claim,
        json!({"applied":true,"attemptId":attempt,"effectId":effect,"stateVersion":version+1}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
