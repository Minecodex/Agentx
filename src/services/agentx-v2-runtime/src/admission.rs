//! Distributed gateway admission (plan7 P7-D3): tenant in-flight invocation
//! and queue watermark checks that run before a request is accepted. This is
//! layered on top of the existing primitives by design:
//!
//! - the in-process caller token bucket keeps its per-caller burst semantics
//!   (rate_limit.rs);
//! - quota prerequisites keep their 400 `RUNTIME_QUOTA_EXCEEDED` business
//!   rejection (quota.rs) — admission overload is a different failure and
//!   answers 429 + Retry-After.

use std::time::Duration;

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

pub struct AdmissionReject {
    pub running: i64,
    pub queued: i64,
}

impl AdmissionReject {
    pub fn retry_after(&self) -> Duration {
        Duration::from_secs(retry_after_seconds() as u64)
    }
}

/// Overload admission check: counts the tenant's in-flight invocations and
/// queued node attempts. Storage failures fail open — availability of the
/// intake beats a spurious rejection when MySQL blips.
pub async fn check(pool: &sqlx::MySqlPool, tenant_id: Uuid) -> Result<(), AdmissionReject> {
    let running: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM application_invocations WHERE tenant_id=? AND status='running'",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    if running > max_running_per_tenant() {
        return Err(AdmissionReject { running, queued: 0 });
    }
    let queued: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM node_attempts a JOIN workflow_executions e ON e.tenant_id=a.tenant_id AND e.id=a.execution_id WHERE e.tenant_id=? AND a.status='queued'",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    if queued > queue_watermark() {
        return Err(AdmissionReject { running, queued });
    }
    Ok(())
}

/// Maps an admission rejection to the 429 transport error. The Retry-After
/// hint travels in the message channel; the response layer emits the header.
pub fn to_error(reject: &AdmissionReject) -> RuntimeError {
    RuntimeError::AdmissionRejected {
        retry_after_seconds: reject.retry_after().as_secs() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_come_from_environment_with_safe_defaults() {
        assert_eq!(max_running_per_tenant(), 500);
        assert_eq!(queue_watermark(), 2000);
        assert_eq!(retry_after_seconds(), 2);
    }
}
