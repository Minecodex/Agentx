//! The durable external-input state machine. All adapters use submit; HTTP success from a Hook never does.
use agentx_runtime_contracts::{
    InputWaitV1, RuntimePublishErrorCodeV1, WaitInputReceiptV1, WaitInputsV1,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySql, Row, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    error::{RuntimeError, RuntimeResult},
    workflow_interaction::{self, timestamp},
};

fn invalid(code: &'static str, message: &str) -> RuntimeError {
    RuntimeError::InvalidRequest(code, message.into())
}

pub(crate) fn scope(tenant: Uuid, application: Uuid, context: &Value) -> Option<Vec<u8>> {
    let key = json!([
        tenant,
        application,
        context.get("webhookTriggerId")?,
        context.get("providerConnectionId")?,
        context.pointer("/conversation/id")?,
        context.pointer("/sender/id")?
    ]);
    Some(Sha256::digest(key.to_string().as_bytes()).to_vec())
}

pub(crate) fn hook_delivery_id(wait_id: Uuid) -> Uuid {
    agentx_runtime_contracts::deterministic_uuid(wait_id, b"input-wait-hook")
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Uuid,
    node_execution: Uuid,
    checkpoint: Uuid,
    node_id: &str,
    parameters: &Value,
) -> RuntimeResult<()> {
    let context = workflow_interaction::context(tx, tenant, execution, node_id).await?;
    // Serialize the root's per-path sequence, including parallel child executions.
    let root_status: String = sqlx::query_scalar(
        "SELECT status FROM workflow_executions WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(tenant)
    .bind(context.root_execution_id)
    .fetch_one(&mut **tx)
    .await?;
    if matches!(
        root_status.as_str(),
        "succeeded" | "failed" | "cancelled" | "timed_out"
    ) {
        return Err(invalid(
            "WAIT_EXECUTION_FINISHED",
            "The root execution has already ended",
        ));
    }
    let path_hash = Sha256::digest(context.node_path.as_bytes()).to_vec();
    let occurrence: u32 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(occurrence),0)+1 AS UNSIGNED) FROM input_waits WHERE tenant_id=? AND root_execution_id=? AND path_hash=?")
        .bind(tenant).bind(context.root_execution_id).bind(&path_hash).fetch_one(&mut **tx).await?;
    let preset = context
        .settings
        .get("waitInputMode")
        .and_then(Value::as_str)
        == Some("preset");
    let reply = if preset {
        let plan: WaitInputsV1 = serde_json::from_value(
            context
                .settings
                .get("waitInputs")
                .cloned()
                .unwrap_or_else(|| json!({})),
        )
        .map_err(|error| invalid("WAIT_INPUT_PLAN_INVALID", &error.to_string()))?;
        Some(preset_reply(&plan, &context.node_path, occurrence)?.clone())
    } else {
        None
    };
    let active_scope = if preset {
        None
    } else {
        context.application_id.and_then(|app| {
            context
                .trigger_context
                .as_ref()
                .and_then(|ctx| scope(tenant, app, ctx))
        })
    };
    let wait_id = agentx_runtime_contracts::deterministic_uuid(node_execution, b"input-wait");
    let timeout_ms = match parameters.get("timeoutMs") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().filter(|value| *value > 0).ok_or_else(|| {
            invalid(
                "WAIT_TIMEOUT_INVALID",
                "Wait timeout must be a positive integer",
            )
        })?),
    };
    let deadline = timeout_ms
        .map(|ms| {
            i64::try_from(ms)
                .ok()
                .and_then(|ms| OffsetDateTime::now_utc().checked_add(Duration::milliseconds(ms)))
                .ok_or_else(|| invalid("WAIT_TIMEOUT_INVALID", "Wait timeout is too large"))
        })
        .transpose()?;
    let hook = match parameters.get("hookUrl") {
        None | Some(Value::Null) => None,
        Some(Value::String(url)) => Some(url.trim()).filter(|url| !url.is_empty()),
        Some(_) => {
            return Err(invalid(
                "WAIT_HOOK_URL_INVALID",
                "Hook URL must resolve to text",
            ));
        }
    };
    if let Some(url) = hook {
        super::input_wait_delivery::validate_url(url)?;
    }
    if !preset && hook.is_some() && context.application_id.is_none() {
        return Err(invalid(
            "WAIT_HOOK_APPLICATION_REQUIRED",
            "Hook notifications require an application invocation",
        ));
    }
    let inserted = sqlx::query("INSERT INTO input_waits(id,tenant_id,root_execution_id,execution_id,invocation_id,application_id,node_execution_id,checkpoint_id,node_id,node_path,path_hash,occurrence,active_scope,status,deadline_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,'waiting',?)")
        .bind(wait_id).bind(tenant).bind(context.root_execution_id).bind(execution).bind(context.invocation_id).bind(context.application_id)
        .bind(node_execution).bind(checkpoint).bind(node_id).bind(&context.node_path).bind(path_hash).bind(occurrence).bind(active_scope).bind(deadline).execute(&mut **tx).await;
    if let Err(error) = inserted {
        if error
            .as_database_error()
            .is_some_and(|error| error.is_unique_violation())
        {
            return Err(invalid(
                "CHANNEL_WAIT_CONFLICT",
                "This channel conversation and user already have an active input wait",
            ));
        }
        return Err(error.into());
    }
    let resume_url = resume_url(context.invocation_id, wait_id);
    let event_id = agentx_runtime_contracts::deterministic_uuid(wait_id, b"input-required");
    let payload = json!({"eventId":event_id,"type":"workflow.input_required","invocationId":context.invocation_id,"executionId":execution,"nodeExecutionId":node_execution,"nodeId":node_id,"nodePath":context.node_path,"occurrence":occurrence,"waitId":wait_id,"resumeUrl":resume_url,"expiresAt":deadline.map(timestamp).transpose()?});
    workflow_interaction::event(
        tx,
        tenant,
        execution,
        &context,
        "workflow.input_required",
        payload.clone(),
    )
    .await?;
    if !preset && let Some(url) = hook {
        let application = context.application_id.ok_or_else(|| {
            invalid(
                "WAIT_HOOK_APPLICATION_REQUIRED",
                "Hook notifications require an application invocation",
            )
        })?;
        crate::delivery::enqueue(
            tx,
            &crate::delivery::DeliveryEnqueue {
                id: hook_delivery_id(wait_id),
                tenant_id: tenant,
                application_id: application,
                invocation_id: context.invocation_id,
                execution_id: execution,
                channel_binding_id: None,
                provider: "input_hook".into(),
                origin: format!("wait:{wait_id}"),
                target: json!({"url":url}),
                credential_ref: None,
                payload,
            },
        )
        .await?;
    }
    if let Some(content) = reply {
        submit_tx(
            tx,
            tenant,
            wait_id,
            &format!("preset:{wait_id}"),
            &content,
            "preset",
        )
        .await?;
    }
    Ok(())
}

pub(crate) fn preset_reply<'a>(
    plan: &'a WaitInputsV1,
    path: &str,
    occurrence: u32,
) -> RuntimeResult<&'a Value> {
    plan.get(path)
        .and_then(|replies| {
            occurrence
                .checked_sub(1)
                .and_then(|index| replies.get(index as usize))
        })
        .ok_or_else(|| {
            invalid(
                "WAIT_INPUT_EXHAUSTED",
                &format!("No preset reply for {path}, occurrence {occurrence}"),
            )
        })
}

pub fn resume_url(invocation: Option<Uuid>, wait_id: Uuid) -> Option<String> {
    invocation.map(|id| format!("/gateway/v1/invocations/{id}/waits/{wait_id}/input"))
}

pub async fn submit(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    wait_id: Uuid,
    key: &str,
    content: &Value,
    source: &str,
) -> RuntimeResult<WaitInputReceiptV1> {
    let mut tx = pool.begin().await?;
    let result = submit_tx(&mut tx, tenant, wait_id, key, content, source).await?;
    tx.commit().await?;
    Ok(result)
}

pub(crate) async fn submit_tx(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    wait_id: Uuid,
    key: &str,
    content: &Value,
    source: &str,
) -> RuntimeResult<WaitInputReceiptV1> {
    if key.is_empty() || key.len() > 192 {
        return Err(invalid(
            "IDEMPOTENCY_KEY_REQUIRED",
            "A nonempty Idempotency-Key of at most 192 bytes is required",
        ));
    }
    let execution: Uuid =
        sqlx::query_scalar("SELECT execution_id FROM input_waits WHERE tenant_id=? AND id=?")
            .bind(tenant)
            .bind(wait_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(RuntimeError::NotFound)?;
    // Same lock order as engine settlement/cancellation: execution before wait.
    let execution_status: String = sqlx::query_scalar(
        "SELECT status FROM workflow_executions WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(tenant)
    .bind(execution)
    .fetch_one(&mut **tx)
    .await?;
    let row = sqlx::query("SELECT *,COALESCE(deadline_at<=UTC_TIMESTAMP(6),FALSE) expired FROM input_waits WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(tenant).bind(wait_id).fetch_one(&mut **tx).await?;
    let hash = agentx_runtime_contracts::content_hash(content)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    if row.try_get::<Option<String>, _>("input_key")?.as_deref() == Some(key) {
        if row.try_get::<Option<String>, _>("input_hash")?.as_deref() != Some(hash.as_str()) {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Input idempotency key was reused with different content".into(),
            ));
        }
        return Ok(WaitInputReceiptV1 {
            wait_id,
            execution_id: execution,
            receipt_id: row.try_get("receipt_id")?,
            status: "accepted".into(),
        });
    }
    if row.try_get::<String, _>("status")? != "waiting"
        || matches!(
            execution_status.as_str(),
            "succeeded" | "failed" | "cancelled" | "timed_out"
        )
    {
        return Err(RuntimeError::StateConflict(
            "WAIT_ALREADY_FINISHED",
            "This input wait has already ended".into(),
        ));
    }
    if row.try_get::<bool, _>("expired")? {
        return Err(RuntimeError::StateConflict(
            "WAIT_EXPIRED",
            "This input wait has expired".into(),
        ));
    }
    let receipt_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query("UPDATE input_waits SET status='received',resume_status='pending',active_scope=NULL,content_json=?,input_source=?,receipt_id=?,input_key=?,input_hash=?,received_at=? WHERE tenant_id=? AND id=? AND status='waiting'")
        .bind(content).bind(source).bind(receipt_id).bind(key).bind(hash.as_str()).bind(now).bind(tenant).bind(wait_id).execute(&mut **tx).await?;
    let output =
        json!({"waitId":wait_id,"content":content,"source":source,"receivedAt":timestamp(now)?});
    enqueue_resume(
        tx,
        tenant,
        execution,
        wait_id,
        row.try_get("node_execution_id")?,
        "main",
        &output,
    )
    .await?;
    stop_hook(tx, tenant, wait_id).await?;
    let context =
        workflow_interaction::context(tx, tenant, execution, &row.try_get::<String, _>("node_id")?)
            .await?;
    workflow_interaction::event(tx, tenant, execution, &context, "workflow.input_received", json!({"waitId":wait_id,"receiptId":receipt_id,"nodeExecutionId":row.try_get::<Uuid,_>("node_execution_id")?,"nodePath":context.node_path,"occurrence":row.try_get::<u32,_>("occurrence")?,"output":output})).await?;
    Ok(WaitInputReceiptV1 {
        wait_id,
        execution_id: execution,
        receipt_id,
        status: "accepted".into(),
    })
}

async fn enqueue_resume(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Uuid,
    wait: Uuid,
    node_execution: Uuid,
    port: &str,
    output: &Value,
) -> RuntimeResult<()> {
    let command = agentx_runtime_contracts::deterministic_uuid(wait, b"input-wait-resume");
    sqlx::query("INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'resume_execution','execution',?,?,?,'pending')")
        .bind(command).bind(tenant).bind(execution.to_string()).bind(format!("input-wait:{wait}"))
        .bind(json!({"waitId":wait,"nodeExecutionId":node_execution,"outputPort":port,"waitStatus":if port=="timed_out" {"timed_out"} else {"resumed"},"payload":output})).execute(&mut **tx).await?;
    Ok(())
}

async fn stop_hook(tx: &mut Transaction<'_, MySql>, tenant: Uuid, wait: Uuid) -> RuntimeResult<()> {
    sqlx::query("UPDATE delivery_outbox SET status='cancelled',locked_by=NULL,locked_until=NULL,fencing_token=fencing_token+1 WHERE tenant_id=? AND id=? AND status IN ('pending','delivering','failed')")
        .bind(tenant).bind(hook_delivery_id(wait)).execute(&mut **tx).await?;
    Ok(())
}

pub async fn enqueue_due(pool: &sqlx::MySqlPool, limit: u32) -> RuntimeResult<u64> {
    let candidates = sqlx::query("SELECT id,tenant_id,execution_id FROM input_waits WHERE status='waiting' AND deadline_at<=UTC_TIMESTAMP(6) ORDER BY deadline_at,id LIMIT ?")
        .bind(limit.clamp(1,100)).fetch_all(pool).await?;
    let mut count = 0;
    for candidate in candidates {
        let mut tx = pool.begin().await?;
        let tenant: Uuid = candidate.try_get("tenant_id")?;
        let execution: Uuid = candidate.try_get("execution_id")?;
        sqlx::query("SELECT id FROM workflow_executions WHERE tenant_id=? AND id=? FOR UPDATE")
            .bind(tenant)
            .bind(execution)
            .fetch_one(&mut *tx)
            .await?;
        let wait: Uuid = candidate.try_get("id")?;
        let row=sqlx::query("SELECT node_execution_id,node_id FROM input_waits WHERE tenant_id=? AND id=? AND status='waiting' AND deadline_at<=UTC_TIMESTAMP(6) FOR UPDATE")
            .bind(tenant).bind(wait).fetch_optional(&mut *tx).await?;
        if let Some(row) = row {
            sqlx::query("UPDATE input_waits SET status='timed_out',resume_status='pending',active_scope=NULL WHERE tenant_id=? AND id=?").bind(tenant).bind(wait).execute(&mut *tx).await?;
            let output = json!({"waitId":wait,"content":null,"receivedAt":null,"source":"timeout"});
            enqueue_resume(
                &mut tx,
                tenant,
                execution,
                wait,
                row.try_get("node_execution_id")?,
                "timed_out",
                &output,
            )
            .await?;
            stop_hook(&mut tx, tenant, wait).await?;
            let context = workflow_interaction::context(
                &mut tx,
                tenant,
                execution,
                &row.try_get::<String, _>("node_id")?,
            )
            .await?;
            workflow_interaction::event(&mut tx,tenant,execution,&context,"workflow.input_timed_out",json!({"waitId":wait,"nodeExecutionId":row.try_get::<Uuid,_>("node_execution_id")?})).await?;
            count += 1;
        }
        tx.commit().await?;
    }
    Ok(count)
}

pub(crate) async fn resumed(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Uuid,
    wait: Uuid,
) -> RuntimeResult<()> {
    sqlx::query("UPDATE input_waits SET resume_status='succeeded' WHERE tenant_id=? AND execution_id=? AND id=? AND resume_status='pending'")
        .bind(tenant).bind(execution).bind(wait).execute(&mut **tx).await?;
    let row=sqlx::query("SELECT node_id,node_execution_id,status FROM input_waits WHERE tenant_id=? AND execution_id=? AND id=?").bind(tenant).bind(execution).bind(wait).fetch_one(&mut **tx).await?;
    let context =
        workflow_interaction::context(tx, tenant, execution, &row.try_get::<String, _>("node_id")?)
            .await?;
    workflow_interaction::event(tx,tenant,execution,&context,"workflow.input_finished",json!({"waitId":wait,"nodeExecutionId":row.try_get::<Uuid,_>("node_execution_id")?,"status":row.try_get::<String,_>("status")?})).await
}

pub(crate) async fn cancel(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Uuid,
) -> RuntimeResult<()> {
    let rows=sqlx::query("SELECT id,node_id,node_execution_id,execution_id FROM input_waits WHERE tenant_id=? AND (execution_id=? OR root_execution_id=?) AND (status='waiting' OR resume_status='pending') FOR UPDATE")
        .bind(tenant).bind(execution).bind(execution).fetch_all(&mut **tx).await?;
    for row in rows {
        let execution: Uuid = row.try_get("execution_id")?;
        let wait: Uuid = row.try_get("id")?;
        sqlx::query("UPDATE input_waits SET status='cancelled',resume_status='cancelled',active_scope=NULL WHERE tenant_id=? AND id=?").bind(tenant).bind(wait).execute(&mut **tx).await?;
        stop_hook(tx, tenant, wait).await?;
        let context = workflow_interaction::context(
            tx,
            tenant,
            execution,
            &row.try_get::<String, _>("node_id")?,
        )
        .await?;
        workflow_interaction::event(tx,tenant,execution,&context,"workflow.input_finished",json!({"waitId":wait,"nodeExecutionId":row.try_get::<Uuid,_>("node_execution_id")?,"status":"cancelled"})).await?;
    }
    Ok(())
}

pub async fn list(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    invocation: Option<Uuid>,
    root: Option<Uuid>,
    active_only: bool,
) -> RuntimeResult<Vec<InputWaitV1>> {
    let rows=sqlx::query("SELECT * FROM input_waits WHERE tenant_id=? AND ((? IS NOT NULL AND invocation_id=?) OR (? IS NOT NULL AND root_execution_id=?)) AND (?=FALSE OR status='waiting') ORDER BY created_at,id")
        .bind(tenant).bind(invocation).bind(invocation).bind(root).bind(root).bind(active_only).fetch_all(pool).await?;
    rows.into_iter().map(from_row).collect()
}

pub(crate) async fn report(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    root: Uuid,
) -> RuntimeResult<Vec<InputWaitV1>> {
    sqlx::query("SELECT * FROM input_waits WHERE tenant_id=? AND root_execution_id=? ORDER BY created_at,id")
        .bind(tenant).bind(root).fetch_all(&mut **tx).await?.into_iter().map(from_row).collect()
}

fn from_row(row: sqlx::mysql::MySqlRow) -> RuntimeResult<InputWaitV1> {
    let wait_id = row.try_get("id")?;
    Ok(InputWaitV1 {
        wait_id,
        execution_id: row.try_get("execution_id")?,
        node_execution_id: row.try_get("node_execution_id")?,
        node_id: row.try_get("node_id")?,
        node_path: row.try_get("node_path")?,
        occurrence: row.try_get("occurrence")?,
        status: row.try_get("status")?,
        resume_url: resume_url(row.try_get("invocation_id")?, wait_id),
        expires_at: row
            .try_get::<Option<OffsetDateTime>, _>("deadline_at")?
            .map(timestamp)
            .transpose()?,
        content: row.try_get("content_json")?,
        source: row.try_get("input_source")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sequences_are_per_node_and_exhaustion_never_guesses() {
        let plan =
            serde_json::from_value(json!({"/a":["考虑一下","确认"],"/child/b":[{"ok":false}]}))
                .unwrap();
        assert_eq!(preset_reply(&plan, "/a", 2).unwrap(), &json!("确认"));
        assert_eq!(
            preset_reply(&plan, "/child/b", 1).unwrap(),
            &json!({"ok":false})
        );
        assert!(preset_reply(&plan, "/a", 3).is_err());
        assert!(preset_reply(&plan, "/missing", 1).is_err());
    }
    #[test]
    fn channel_scope_isolates_sender_conversation_and_connection() {
        let tenant = Uuid::now_v7();
        let app = Uuid::now_v7();
        let first = json!({"webhookTriggerId":"binding","providerConnectionId":"connection","conversation":{"id":"group"},"sender":{"id":"alice"}});
        let mut second = first.clone();
        second["sender"]["id"] = json!("bob");
        assert_ne!(scope(tenant, app, &first), scope(tenant, app, &second));
        assert_ne!(
            scope(tenant, app, &first),
            scope(tenant, Uuid::now_v7(), &first)
        );
    }
}
