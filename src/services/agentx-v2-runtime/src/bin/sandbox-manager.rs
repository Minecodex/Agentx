use std::{env, time::Duration};

use agentx_runtime_infrastructure::{
    RuntimeMySqlSettings, RuntimeObjectStorageSettings, connect_runtime_mysql, runtime_object_store,
};
use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    agentx_service_kit::install_tls_provider();
    let pool = connect_runtime_mysql(&RuntimeMySqlSettings::from_env()?).await?;
    let provider_endpoint = env::var("AGENTX_OPENSANDBOX_ENDPOINT")?;
    let provider_secure_access = env::var("AGENTX_OPENSANDBOX_SECURE_ACCESS")
        .context("AGENTX_OPENSANDBOX_SECURE_ACCESS is required")?
        .parse::<bool>()
        .context("AGENTX_OPENSANDBOX_SECURE_ACCESS must be true or false")?;
    let owner = agentx_mysql_lease::LeaseOwner::for_process()?.0;
    let lifecycle = agentx_service_kit::ServiceLifecycle::default();
    let metrics = agentx_service_kit::MetricsRegistry::default();
    let health = agentx_service_kit::HealthRegistry::default();
    health.register("runtime_mysql", true).await;
    health.register("opensandbox", false).await;
    health.set_status("runtime_mysql", "ready").await;
    health.set_status("opensandbox", "ready").await;
    let state = agentx_v2_runtime::sandbox::SandboxManagerState {
        pool,
        client: agentx_service_kit::reqwest_client_builder_with_ca(
            "AGENTX_OPENSANDBOX_TLS_CA_PATH",
        )?
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()?,
        provider_endpoint,
        provider_api_key: env::var("AGENTX_OPENSANDBOX_API_KEY").ok(),
        provider_secure_access,
        owner,
        vault: Some(agentx_v2_runtime::vault::RuntimeVault::from_env()?),
        objects: runtime_object_store(&RuntimeObjectStorageSettings::from_env()?)?,
        process_sockets: Default::default(),
    };
    let reaper_state = state.clone();
    let reaper_lifecycle = lifecycle.clone();
    let reaper_progress = agentx_service_kit::RoleProgressWatchdog::start(
        "sandbox-reaper",
        Duration::from_secs(agentx_service_kit::ROLE_WATCHDOG_TIMEOUT_SECONDS),
        health.clone(),
        lifecycle.clone(),
        metrics.clone(),
    )
    .await;
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        while !reaper_lifecycle.is_draining() {
            let started = std::time::Instant::now();
            match agentx_v2_runtime::sandbox::reconcile_one(&reaper_state).await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(Duration::from_secs(1)).await,
                Err(error) => {
                    tracing::warn!(%error, "Sandbox Reaper reconciliation failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            reaper_progress.processed_since(started).await;
        }
        Ok::<(), anyhow::Error>(())
    });
    let service_lifecycle = lifecycle.clone();
    let service_metrics = metrics.clone();
    let metrics_health = health.clone();
    let router_state = state.clone();
    tasks.spawn(async move {
        agentx_service_kit::serve_with_lifecycle(
            "sandbox-manager",
            agentx_v2_runtime::sandbox::router(router_state),
            health,
            service_lifecycle,
            service_metrics,
        )
        .await
    });
    let metrics_lifecycle = lifecycle.clone();
    tasks.spawn(
        async move { collect_metrics(state, metrics, metrics_health, metrics_lifecycle).await },
    );
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

async fn collect_metrics(
    state: agentx_v2_runtime::sandbox::SandboxManagerState,
    metrics: agentx_service_kit::MetricsRegistry,
    health: agentx_service_kit::HealthRegistry,
    lifecycle: agentx_service_kit::ServiceLifecycle,
) -> Result<()> {
    while !lifecycle.is_draining() {
        let sample = async {
            let acquire_started = std::time::Instant::now();
            let mut connection = state.pool.acquire().await?;
            metrics.observe_mysql_pool_wait(acquire_started.elapsed()).await;
            let ready: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sandbox_leases WHERE status IN ('creating','interrupting','terminating','orphaned')").fetch_one(&mut *connection).await?;
            let oldest: f64 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(TIMESTAMPDIFF(MICROSECOND,created_at,UTC_TIMESTAMP(6))),0)/1000000.0 AS DOUBLE) FROM sandbox_leases WHERE status IN ('creating','interrupting','terminating','orphaned')").fetch_one(&mut *connection).await?;
            let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sandbox_leases WHERE locked_until>UTC_TIMESTAMP(6)").fetch_one(&mut *connection).await?;
            Ok::<_, sqlx::Error>((ready, oldest, active))
        }.await;
        match sample {
            Ok((ready, oldest, active)) => {
                health.set_status("runtime_mysql", "ready").await;
                metrics.set("agentx_queue_ready_items", ready as f64).await;
                metrics
                    .set("agentx_queue_oldest_ready_seconds", oldest)
                    .await;
                metrics.set("agentx_active_leases", active as f64).await;
            }
            Err(error) => {
                health.set_status("runtime_mysql", "unavailable").await;
                tracing::warn!(%error, "Sandbox metrics refresh failed");
            }
        }
        tokio::select! {
            () = lifecycle.cancelled() => break,
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
    Ok(())
}
