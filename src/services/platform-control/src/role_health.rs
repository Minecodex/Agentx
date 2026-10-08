use std::time::Duration;

use agentx_service_kit::{
    HealthRegistry, MetricsRegistry, ROLE_WATCHDOG_TIMEOUT_SECONDS, RoleProgressWatchdog,
    ServiceLifecycle,
};
use anyhow::Result;
use sqlx::{MySqlPool, Row};

pub async fn watchdog(
    role: &'static str,
    health: &HealthRegistry,
    lifecycle: &ServiceLifecycle,
    metrics: &MetricsRegistry,
) -> RoleProgressWatchdog {
    RoleProgressWatchdog::start(
        role,
        Duration::from_secs(ROLE_WATCHDOG_TIMEOUT_SECONDS),
        health.clone(),
        lifecycle.clone(),
        metrics.clone(),
    )
    .await
}

pub async fn collect_control_metrics(
    pool: MySqlPool,
    metrics: MetricsRegistry,
    health: HealthRegistry,
    lifecycle: ServiceLifecycle,
) -> Result<()> {
    while !lifecycle.is_draining() {
        let sample = async {
            let acquire_started = std::time::Instant::now();
            let mut connection = pool.acquire().await?;
            metrics.observe_mysql_pool_wait(acquire_started.elapsed()).await;
            let row = sqlx::query("SELECT COUNT(*) ready_items,CAST(COALESCE(MAX(TIMESTAMPDIFF(MICROSECOND,available_at,UTC_TIMESTAMP(6))),0)/1000000.0 AS DOUBLE) oldest_seconds FROM outbox WHERE status IN ('pending','failed') AND available_at<=UTC_TIMESTAMP(6)").fetch_one(&mut *connection).await?;
            let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM publish_attempts WHERE locked_until>UTC_TIMESTAMP(6)").fetch_one(&mut *connection).await?;
            Ok::<_, sqlx::Error>((
                row.try_get::<i64, _>("ready_items")?,
                row.try_get::<f64, _>("oldest_seconds")?,
                active,
            ))
        }
        .await;
        match sample {
            Ok((ready, oldest, active)) => {
                health.set_status("control_mysql", "ready").await;
                metrics.set("agentx_queue_ready_items", ready as f64).await;
                metrics
                    .set("agentx_queue_oldest_ready_seconds", oldest)
                    .await;
                metrics.set("agentx_active_leases", active as f64).await;
            }
            Err(error) => {
                health.set_status("control_mysql", "unavailable").await;
                tracing::warn!(%error, "Control metrics refresh failed");
            }
        }
        tokio::select! {
            () = lifecycle.cancelled() => break,
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
    Ok(())
}
