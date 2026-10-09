//! Distributed gateway admission (plan7 P7-D3): tenant in-flight invocation
//! and queue watermark checks that run before a request is accepted. This is
//! layered on top of the existing primitives by design:
//!
//! - the in-process caller token bucket keeps its per-caller burst semantics
//!   (rate_limit.rs);
//! - quota prerequisites keep their 400 `RUNTIME_QUOTA_EXCEEDED` business
//!   rejection (quota.rs) — admission overload is a different failure and
//!   answers 429 + Retry-After.

use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

/// Concurrent non-terminal invocations one tenant may keep in flight.
fn max_running_per_tenant() -> i64 {
    std::env::var("AGENTX_RUNTIME_GATEWAY_ADMISSION_MAX_RUNNING")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500)
}

/// Queued node attempts across the tenant's executions; beyond this the
/// intake is overflowing and further requests must wait instead of piling on.
fn queue_watermark() -> i64 {
    std::env::var("AGENTX_RUNTIME_GATEWAY_ADMISSION_QUEUE_WATERMARK")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2000)
}

/// Retry-After hint (seconds) for rejected invocations.
fn retry_after_seconds() -> u32 {
    std::env::var("AGENTX_RUNTIME_GATEWAY_ADMISSION_RETRY_AFTER_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2)
}

/// Process-wide rejection counter; the gateway middleware mirrors it onto the
/// metrics registry so /metrics reflects it without a MySQL scan.
pub static REJECTIONS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn record_rejection() {
    REJECTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn rejection_count() -> i64 {
    REJECTIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Queue and running work both consume the admission budget. Storage
/// failures propagate; an unavailable watermark cannot authorize intake.
pub async fn check(
    pool: &sqlx::MySqlPool,
    redis: Option<&redis::aio::ConnectionManager>,
    tenant_id: Uuid,
) -> RuntimeResult<()> {
    let (running, queued): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM application_invocations WHERE tenant_id=? AND status IN ('queued','running')) running,(SELECT COUNT(*) FROM node_attempts WHERE tenant_id=? AND status='queued') queued",
    )
    .bind(tenant_id)
    .bind(tenant_id)
    .fetch_one(pool)
    .await?;
    if overloaded(running, queued, max_running_per_tenant(), queue_watermark()) {
        record_rejection();
        return Err(RuntimeError::AdmissionRejected {
            retry_after_seconds: retry_after_seconds(),
        });
    }
    let (unread, pending) = crate::redis_admission::snapshot(
        redis.ok_or(RuntimeError::Unavailable)?,
        queue_watermark(),
    )
    .await?;
    if unread.saturating_add(pending) >= queue_watermark() {
        record_rejection();
        return Err(RuntimeError::AdmissionRejected {
            retry_after_seconds: retry_after_seconds(),
        });
    }
    Ok(())
}

fn overloaded(running: i64, queued: i64, max_running: i64, max_queued: i64) -> bool {
    running >= max_running || queued >= max_queued
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_budget_rejects_the_next_invocation() {
        assert!(!overloaded(499, 1999, 500, 2000));
        assert!(overloaded(500, 0, 500, 2000));
        assert!(overloaded(0, 2000, 500, 2000));
    }

    #[test]
    fn thresholds_come_from_environment_with_safe_defaults() {
        assert_eq!(max_running_per_tenant(), 500);
        assert_eq!(queue_watermark(), 2000);
        assert_eq!(retry_after_seconds(), 2);
    }
}
