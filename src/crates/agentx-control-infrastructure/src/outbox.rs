use std::time::Duration;

use agentx_application::{Outbox, OutboxDelivery, OutboxDispatcher, OutboxMessage};
use agentx_domain::TenantId;
use agentx_mysql_lease::{
    DEFAULT_BATCH_SIZE, DEFAULT_LEASE_SECONDS, LeaseError, LeaseOwner, require_single_lease_write,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::{MySqlPool, Row};
use uuid::Uuid;

pub struct MySqlControlOutbox {
    pool: MySqlPool,
}

impl MySqlControlOutbox {
    #[must_use]
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Outbox for MySqlControlOutbox {
    async fn append(&self, message: OutboxMessage) -> Result<()> {
        sqlx::query(
            "INSERT INTO outbox (id, tenant_id, event_type, aggregate_type, aggregate_id, payload_json, status) VALUES (?, ?, ?, ?, ?, ?, 'pending')",
        )
        .bind(message.event_id)
        .bind(message.tenant_id.as_uuid())
        .bind(message.event_type)
        .bind(message.aggregate_type)
        .bind(message.aggregate_id)
        .bind(message.payload)
        .execute(&self.pool)
        .await
        .context("failed to append Control outbox event")?;
        Ok(())
    }
}

pub struct MySqlControlOutboxDispatcher {
    pool: MySqlPool,
}

impl MySqlControlOutboxDispatcher {
    #[must_use]
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OutboxDispatcher for MySqlControlOutboxDispatcher {
    async fn claim(&self, batch_size: u32, lease: Duration) -> Result<Vec<OutboxDelivery>> {
        let owner = LeaseOwner::new();
        let lease_seconds = lease.as_secs().clamp(1, DEFAULT_LEASE_SECONDS);
        let mut transaction = self
            .pool
            .begin()
            .await
            .context("begin Control outbox claim")?;
        let rows = sqlx::query(
            "SELECT id FROM outbox WHERE aggregate_type NOT IN ('evaluation_dispatch','evaluation_model_admission') AND status IN ('pending','failed') AND published_at IS NULL AND available_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY occurred_at,id LIMIT ? FOR UPDATE SKIP LOCKED",
        )
        .bind(batch_size.clamp(1, DEFAULT_BATCH_SIZE))
        .fetch_all(&mut *transaction)
        .await
        .context("select Control outbox events")?;
        for row in rows {
            let event_id: Uuid = row.try_get("id")?;
            sqlx::query("UPDATE outbox SET status='processing',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=? AND status IN ('pending','failed') AND published_at IS NULL AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6))")
                .bind(owner.0)
                .bind(lease_seconds)
                .bind(event_id)
                .execute(&mut *transaction)
                .await
                .context("lease Control outbox event")?;
        }
        transaction
            .commit()
            .await
            .context("commit Control outbox claim")?;

        let rows = sqlx::query("SELECT id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,fencing_token FROM outbox WHERE locked_by=? AND status='processing' AND published_at IS NULL AND locked_until>UTC_TIMESTAMP(6) ORDER BY occurred_at,id")
            .bind(owner.0)
            .fetch_all(&self.pool)
            .await
            .context("load leased Control outbox events")?;
        rows.into_iter()
            .map(|row| {
                Ok(OutboxDelivery {
                    event_id: row.try_get("id")?,
                    tenant_id: TenantId::from_uuid(row.try_get("tenant_id")?),
                    event_type: row.try_get("event_type")?,
                    aggregate_type: row.try_get("aggregate_type")?,
                    aggregate_id: row.try_get("aggregate_id")?,
                    payload: row.try_get("payload_json")?,
                    attempt_count: row.try_get("fencing_token")?,
                    lease_id: owner.0,
                })
            })
            .collect()
    }

    async fn mark_published(&self, delivery: &OutboxDelivery) -> Result<bool> {
        let result = sqlx::query("UPDATE outbox SET status='published',published_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL,last_error=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND status='processing' AND published_at IS NULL AND locked_until>UTC_TIMESTAMP(6)")
            .bind(delivery.event_id)
            .bind(delivery.lease_id)
            .bind(delivery.attempt_count)
            .execute(&self.pool)
            .await
            .context("mark Control outbox event published")?;
        lease_result(result.rows_affected())
    }

    async fn mark_failed(
        &self,
        delivery: &OutboxDelivery,
        error: &str,
        retry_after: Duration,
    ) -> Result<bool> {
        let retry_seconds = retry_after.as_secs().clamp(1, 86_400);
        let message = error.chars().take(1024).collect::<String>();
        let result = sqlx::query("UPDATE outbox SET status='failed',available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),locked_by=NULL,locked_until=NULL,last_error=? WHERE id=? AND locked_by=? AND fencing_token=? AND status='processing' AND published_at IS NULL AND locked_until>UTC_TIMESTAMP(6)")
            .bind(retry_seconds)
            .bind(message)
            .bind(delivery.event_id)
            .bind(delivery.lease_id)
            .bind(delivery.attempt_count)
            .execute(&self.pool)
            .await
            .context("reschedule failed Control outbox event")?;
        lease_result(result.rows_affected())
    }
}

fn lease_result(rows_affected: u64) -> Result<bool> {
    match require_single_lease_write(rows_affected) {
        Ok(()) => Ok(true),
        Err(LeaseError::LeaseLost) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
