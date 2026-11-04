//! Formal Invocation cancellation shared by employee and batch services.
use crate::{
    RuntimeState,
    application_caller::ApplicationCaller as Caller,
    error::{RuntimeError, RuntimeResult},
};
use agentx_runtime_contracts::{CommandAcceptedV1, ContentHash};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;
fn request_hash(value: &Value) -> RuntimeResult<ContentHash> {
    agentx_runtime_contracts::content_hash(value).map_err(|e| RuntimeError::Internal(e.into()))
}
fn terminal(value: &str) -> bool {
    matches!(value, "completed" | "failed" | "cancelled")
}
pub(crate) async fn cancel_authorized_invocation(
    state: &RuntimeState,
    caller: &Caller,
    id: Uuid,
    key: &str,
    actor_type: &str,
    actor_id: Uuid,
) -> RuntimeResult<CommandAcceptedV1> {
    let command_key = format!(
        "task-cancel:{}",
        request_hash(&json!([caller.application_id, actor_type, actor_id, key]))?.as_str()
    );
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT execution_id,status FROM application_invocations WHERE id=? AND tenant_id=? AND application_id=? AND caller_type=? AND caller_id=? FOR UPDATE")
        .bind(id).bind(caller.tenant_id).bind(caller.application_id).bind(caller.caller_type).bind(caller.subject_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    if let Some(existing) = sqlx::query_scalar::<_,String>("SELECT aggregate_id FROM runtime_commands WHERE tenant_id=? AND command_type='cancel_execution' AND idempotency_key=?")
        .bind(caller.tenant_id).bind(&command_key).fetch_optional(&mut *tx).await?
    {
        if existing != row.try_get::<Uuid,_>("execution_id")?.to_string() {
            return Err(RuntimeError::Conflict(agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,"Cancel request ID was reused for another task".into()));
        }
        tx.commit().await?;
        return Ok(CommandAcceptedV1{accepted:true,replayed:true});
    }
    if terminal(&row.try_get::<String, _>("status")?) {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Invocation is already terminal".into(),
        ));
    }
    let execution_id: Uuid = row.try_get("execution_id")?;
    let result = sqlx::query("INSERT IGNORE INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'cancel_execution','execution',?,?,?,'pending')")
        .bind(Uuid::now_v7()).bind(caller.tenant_id).bind(execution_id.to_string()).bind(&command_key).bind(json!({"invocationId":id,"executionId":execution_id,"actorType":actor_type,"actorId":actor_id})).execute(&mut *tx).await?;
    let replayed = result.rows_affected() == 0;
    if replayed {
        let existing: String = sqlx::query_scalar("SELECT aggregate_id FROM runtime_commands WHERE tenant_id=? AND command_type='cancel_execution' AND idempotency_key=?")
            .bind(caller.tenant_id).bind(&command_key).fetch_one(&mut *tx).await?;
        if existing != execution_id.to_string() {
            return Err(RuntimeError::Conflict(
                agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Cancel idempotency key was reused for another Invocation".into(),
            ));
        }
    } else {
        sqlx::query("INSERT INTO invocation_events(tenant_id,invocation_id,event_id,sequence_number,event_type,payload_json) SELECT ?,?,?,COALESCE(MAX(sequence_number),0)+1,'cancel.requested',? FROM invocation_events WHERE tenant_id=? AND invocation_id=?")
            .bind(caller.tenant_id).bind(id).bind(Uuid::now_v7()).bind(json!({"executionId":execution_id,"actorType":actor_type,"actorId":actor_id})).bind(caller.tenant_id).bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(CommandAcceptedV1 {
        accepted: true,
        replayed,
    })
}
