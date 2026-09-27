//! Outbound delivery outbox (plan7 P7-A): the authoritative state machine for
//! IM channel replies. Rows are enqueued in the same transaction as the
//! triggering state change (execution terminal for L1 auto replies, worker
//! attempt settlement for the reply/send nodes) and claimed by the delivery
//! loop role with MySQL lease + fencing, mirroring execution_outbox.

use sqlx::{MySql, MySqlPool, Row};
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

/// Retryable deliveries attempt at most five times before moving to the dead
/// letter archive (plan7 01 §3.3).
pub const DELIVERY_MAX_ATTEMPTS: u32 = 5;

/// A delivery enqueue request; `id` is derived deterministically by callers so
/// replays of the same settlement stay idempotent.
pub struct DeliveryEnqueue {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub invocation_id: Option<Uuid>,
    pub execution_id: Uuid,
    pub channel_binding_id: Uuid,
    pub provider: String,
    /// Distinguishes L1 terminal replies from L2/L3 node replies so both can
    /// coexist for one execution without colliding on the idempotency key.
    pub origin: String,
    pub target: serde_json::Value,
    pub credential_ref: Option<serde_json::Value>,
    pub payload: serde_json::Value,
}

pub async fn enqueue(
    tx: &mut sqlx::Transaction<'_, MySql>,
    request: &DeliveryEnqueue,
) -> RuntimeResult<()> {
    let idempotency_key = format!("{}:{}", request.execution_id, request.origin);
    let changed = sqlx::query(
        "INSERT INTO delivery_outbox(id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,target_json,credential_ref_json,payload_json,status,idempotency_key) VALUES(?,?,?,?,?,?,?,?,?,?,?,'pending',?) ON DUPLICATE KEY UPDATE id=id",
    )
    .bind(request.id)
    .bind(request.tenant_id)
    .bind(request.application_id)
    .bind(request.invocation_id)
    .bind(request.execution_id)
    .bind(request.channel_binding_id)
    .bind(&request.provider)
    .bind(request.origin.chars().take(400).collect::<String>())
    .bind(&request.target)
    .bind(&request.credential_ref)
    .bind(&request.payload)
    .bind(idempotency_key)
    .execute(&mut **tx)
    .await?;
    if changed.rows_affected() != 1 {
        // The ON DUPLICATE branch reports 0 affected rows for idempotent
        // replays with identical values and 1 for fresh inserts; both are
        // success. MySQL reports 2 for conflicting updates, which cannot
        // happen here because the update is a no-op.
        if changed.rows_affected() > 2 {
            return Err(RuntimeError::Internal(anyhow::anyhow!(
                "unexpected delivery enqueue result"
            )));
        }
    }
    Ok(())
}

pub struct DeliveryClaim {
    pub id: Uuid,
    pub owner: Uuid,
    pub fencing_token: u64,
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub invocation_id: Option<Uuid>,
    pub execution_id: Uuid,
    pub channel_binding_id: Uuid,
    pub provider: String,
    pub origin: String,
    pub target: serde_json::Value,
    pub credential_ref: Option<serde_json::Value>,
    pub payload: serde_json::Value,
    pub attempt_count: u32,
}

pub async fn claim(pool: &MySqlPool, owner: Uuid) -> RuntimeResult<Option<DeliveryClaim>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,target_json,credential_ref_json,payload_json,attempt_count FROM delivery_outbox WHERE status='pending' AND next_attempt_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let id: Uuid = row.try_get("id")?;
    sqlx::query(
        "UPDATE delivery_outbox SET status='delivering',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=? AND status='pending'",
    )
    .bind(owner)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let fencing_token: u64 =
        sqlx::query_scalar("SELECT fencing_token FROM delivery_outbox WHERE id=?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    let attempt_count: u32 = row.try_get::<u32, _>("attempt_count")? + 1;
    tx.commit().await?;
    Ok(Some(DeliveryClaim {
        id,
        owner,
        fencing_token,
        tenant_id: row.try_get("tenant_id")?,
        application_id: row.try_get("application_id")?,
        invocation_id: row.try_get("invocation_id")?,
        execution_id: row.try_get("execution_id")?,
        channel_binding_id: row.try_get("channel_binding_id")?,
        provider: row.try_get("provider")?,
        origin: row.try_get("origin")?,
        target: row.try_get("target")?,
        credential_ref: row.try_get("credential_ref")?,
        payload: row.try_get("payload")?,
        attempt_count,
    }))
}

fn lease_guard() -> &'static str {
    "locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)"
}

pub async fn complete(
    pool: &MySqlPool,
    claim: &DeliveryClaim,
    provider_message_id: Option<&str>,
) -> RuntimeResult<()> {
    let changed = sqlx::query(&format!(
        "UPDATE delivery_outbox SET status='delivered',provider_message_id=?,locked_by=NULL,locked_until=NULL,last_error_code=NULL,last_error_message=NULL WHERE id=? AND status='delivering' AND {}",
        lease_guard()
    ))
    .bind(provider_message_id)
    .bind(claim.owner)
    .bind(claim.fencing_token)
    .bind(claim.id)
    .execute(pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Delivery Lease was lost".into(),
        ));
    }
    Ok(())
}

/// Retryable failure (network / 5xx / rate limit): exponential backoff with
/// jitter-free doubling capped at 60s; after five attempts the row moves to
/// the dead letter archive.
pub async fn fail_retryable(
    pool: &MySqlPool,
    claim: &DeliveryClaim,
    code: &str,
    message: &str,
) -> RuntimeResult<()> {
    if claim.attempt_count >= DELIVERY_MAX_ATTEMPTS {
        return dead(pool, claim, code, message).await;
    }
    let delay = (1_u64 << claim.attempt_count.min(6)).min(60);
    let changed = sqlx::query(&format!(
        "UPDATE delivery_outbox SET status='pending',next_attempt_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),locked_by=NULL,locked_until=NULL,last_error_code=?,last_error_message=? WHERE id=? AND status='delivering' AND {}",
        lease_guard()
    ))
    .bind(delay)
    .bind(code)
    .bind(message.chars().take(1000).collect::<String>())
    .bind(claim.owner)
    .bind(claim.fencing_token)
    .bind(claim.id)
    .execute(pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Delivery Lease was lost".into(),
        ));
    }
    Ok(())
}

/// Non-retryable failure (4xx credential invalid / target missing / content
/// rejected) or exhaustion of retries: archive to the dead letter table.
pub async fn dead(
    pool: &MySqlPool,
    claim: &DeliveryClaim,
    code: &str,
    message: &str,
) -> RuntimeResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO delivery_dead_letters(id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,target_json,credential_ref_json,payload_json,attempt_count,last_error_code,last_error_message,provider_message_id,idempotency_key,created_at) SELECT id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,target_json,credential_ref_json,payload_json,attempt_count,?,?,provider_message_id,idempotency_key,created_at FROM delivery_outbox WHERE id=? ON DUPLICATE KEY UPDATE id=id",
    )
    .bind(code)
    .bind(message.chars().take(1000).collect::<String>())
    .bind(claim.id)
    .execute(&mut *tx)
    .await?;
    let changed = sqlx::query(&format!(
        "DELETE FROM delivery_outbox WHERE id=? AND {}",
        lease_guard()
    ))
    .bind(claim.owner)
    .bind(claim.fencing_token)
    .bind(claim.id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        tx.rollback().await?;
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Delivery Lease was lost".into(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

/// Dead letter replay (POST /deliveries/{id}/retry): re-enqueues the archived
/// record as a fresh pending delivery.
pub async fn replay_dead_letter(
    pool: &MySqlPool,
    tenant_id: Uuid,
    dead_letter_id: Uuid,
) -> RuntimeResult<Uuid> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,target_json,credential_ref_json,payload_json,idempotency_key FROM delivery_dead_letters WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(dead_letter_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Err(RuntimeError::NotFound);
    };
    let id: Uuid = row.try_get("id")?;
    let changed = sqlx::query(
        "INSERT INTO delivery_outbox(id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,target_json,credential_ref_json,payload_json,status,idempotency_key) SELECT id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,target_json,credential_ref_json,payload_json,'pending',idempotency_key FROM delivery_dead_letters WHERE id=? ON DUPLICATE KEY UPDATE status='pending',next_attempt_at=UTC_TIMESTAMP(6),attempt_count=0,locked_by=NULL,locked_until=NULL,last_error_code=NULL,last_error_message=NULL",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() == 0 {
        // The outbox row already exists and nothing changed; make it pending
        // through the conflict branch above by touching it explicitly.
        sqlx::query("UPDATE delivery_outbox SET status='pending',next_attempt_at=UTC_TIMESTAMP(6),attempt_count=0,locked_by=NULL,locked_until=NULL WHERE id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM delivery_dead_letters WHERE tenant_id=? AND id=?")
        .bind(tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_attempts_matches_plan() {
        assert_eq!(DELIVERY_MAX_ATTEMPTS, 5);
    }
}
