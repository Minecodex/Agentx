use agentx_runtime_infrastructure::{RuntimeRedisSettings, connect_runtime_redis};
use anyhow::Result;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use std::{env, future::Future, time::Duration};
use uuid::Uuid;

#[path = "support/runtime_task_queue.rs"]
mod runtime_task_queue;

#[tokio::main]
async fn main() -> Result<()> {
    agentx_service_kit::install_tls_provider();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .try_init();
    let roles = runtime_roles()?;
    let state = agentx_v2_runtime::RuntimeState::maintenance_from_env().await?;
    let pool = state.pool.clone();
    let redis_settings = RuntimeRedisSettings::from_env()?;
    let mut bootstrap_redis = connect_runtime_redis(&redis_settings).await?;
    runtime_task_queue::ensure_groups(&mut bootstrap_redis).await?;
    drop(bootstrap_redis);
    let owner = agentx_mysql_lease::LeaseOwner::for_process()?.0;
    let lifecycle = agentx_service_kit::ServiceLifecycle::default();
    let metrics = agentx_service_kit::MetricsRegistry::default();
    let health = agentx_service_kit::HealthRegistry::default();
    health.register("runtime_mysql", true).await;
    health.register("runtime_redis", true).await;
    health.set_status("runtime_mysql", "ready").await;
    health.set_status("runtime_redis", "ready").await;
    let mut tasks = tokio::task::JoinSet::new();
    if roles.contains("command") || roles.contains("coordinator") {
        let command_state = state.clone();
        let role_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "command",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let state = command_state.clone();
                let lifecycle = role_lifecycle.clone();
                async move { command_loop(state, owner, lifecycle, progress).await }
            },
        ));
    }
    if roles.contains("outbox") {
        let sequencer_pool = pool.clone();
        let sequencer_settings = redis_settings.clone();
        let sequencer_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "event-sequencer",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let pool = sequencer_pool.clone();
                let settings = sequencer_settings.clone();
                let lifecycle = sequencer_lifecycle.clone();
                async move {
                    let redis = connect_runtime_redis(&settings).await?;
                    event_sequencer_loop(pool, redis, owner, lifecycle, progress).await
                }
            },
        ));
        let dispatch_pool = pool.clone();
        let dispatch_settings = redis_settings.clone();
        let dispatch_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "execution-outbox",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let pool = dispatch_pool.clone();
                let settings = dispatch_settings.clone();
                let lifecycle = dispatch_lifecycle.clone();
                async move {
                    // Sequencing and dispatch use independent connections and
                    // supervisors so an invalid Integration Event cannot
                    // starve authoritative execution work.
                    let redis = connect_runtime_redis(&settings).await?;
                    dispatch_loop(pool, redis, owner, lifecycle, progress).await
                }
            },
        ));
    }
    if roles.contains("recovery") {
        let pool = pool.clone();
        let settings = redis_settings.clone();
        let maintenance_state = state.clone();
        let recovery_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "recovery",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let pool = pool.clone();
                let settings = settings.clone();
                let lifecycle = recovery_lifecycle.clone();
                async move {
                    let redis = connect_runtime_redis(&settings).await?;
                    recovery_loop(pool, redis, owner, lifecycle, progress).await
                }
            },
        ));
        let maintenance_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "maintenance",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let state = maintenance_state.clone();
                let lifecycle = maintenance_lifecycle.clone();
                async move { maintenance_loop(state, owner, lifecycle, progress).await }
            },
        ));
    }
    if roles.contains("artifact") || roles.contains("quota") {
        let retention_state = state.clone();
        let artifact_state = state.clone();
        let retention_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "retention",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let state = retention_state.clone();
                let lifecycle = retention_lifecycle.clone();
                async move { retention_loop(state, owner, lifecycle, progress).await }
            },
        ));
        let artifact_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "artifact",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let state = artifact_state.clone();
                let lifecycle = artifact_lifecycle.clone();
                async move { artifact_loop(state, lifecycle, progress).await }
            },
        ));
    }
    if roles.contains("quota") {
        let quota_pool = pool.clone();
        let settings = redis_settings.clone();
        let role_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "quota",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let pool = quota_pool.clone();
                let settings = settings.clone();
                let lifecycle = role_lifecycle.clone();
                async move {
                    let redis = connect_runtime_redis(&settings).await?;
                    quota_projection_loop(pool, redis, owner, lifecycle, progress).await
                }
            },
        ));
    }
    if roles.contains("trigger") {
        let pool = pool.clone();
        let role_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "trigger",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let pool = pool.clone();
                let lifecycle = role_lifecycle.clone();
                async move { trigger_loop(pool, owner, lifecycle, progress).await }
            },
        ));
    }
    if roles.contains("stream") {
        let pool = pool.clone();
        let role_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "stream",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let pool = pool.clone();
                let lifecycle = role_lifecycle.clone();
                async move {
                    agentx_v2_runtime::stream::stream_loop(pool, owner, lifecycle, progress).await
                }
            },
        ));
    }
    if roles.contains("trace-relay") {
        let trace_pool = pool.clone();
        let settings = redis_settings.clone();
        let role_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "trace-relay",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let pool = trace_pool.clone();
                let settings = settings.clone();
                let lifecycle = role_lifecycle.clone();
                async move {
                    let redis = connect_runtime_redis(&settings).await?;
                    trace_relay_loop(pool, redis, owner, lifecycle, progress).await
                }
            },
        ));
    }
    if roles.contains("delivery") {
        let delivery_pool = pool.clone();
        let delivery_state = state.clone();
        let role_lifecycle = lifecycle.clone();
        tasks.spawn(supervise_role(
            "delivery",
            lifecycle.clone(),
            health.clone(),
            metrics.clone(),
            move |progress| {
                let pool = delivery_pool.clone();
                let state = delivery_state.clone();
                let lifecycle = role_lifecycle.clone();
                async move { delivery_loop(pool, state, owner, lifecycle, progress).await }
            },
        ));
    }
    anyhow::ensure!(!tasks.is_empty(), "AGENTX_RUNTIME_ROLES selected no role");
    let service_lifecycle = lifecycle.clone();
    let service_metrics = metrics.clone();
    let metrics_health = health.clone();
    let metrics_redis = redis_settings.clone();
    tasks.spawn(async move {
        agentx_service_kit::serve_with_lifecycle(
            "workflow-runtime",
            axum::Router::new(),
            health,
            service_lifecycle,
            service_metrics,
        )
        .await
    });
    let metrics_lifecycle = lifecycle.clone();
    tasks.spawn(async move {
        collect_runtime_metrics(
            pool,
            metrics_redis,
            metrics,
            metrics_health,
            metrics_lifecycle,
        )
        .await
    });
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

async fn supervise_role<F, Fut>(
    role: &'static str,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    health: agentx_service_kit::HealthRegistry,
    metrics: agentx_service_kit::MetricsRegistry,
    mut run: F,
) -> Result<()>
where
    F: FnMut(agentx_service_kit::RoleProgressWatchdog) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let progress = agentx_service_kit::RoleProgressWatchdog::start(
        role,
        Duration::from_secs(agentx_service_kit::ROLE_WATCHDOG_TIMEOUT_SECONDS),
        health,
        lifecycle.clone(),
        metrics,
    )
    .await;
    let mut consecutive_failures = 0u32;
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = tokio::time::Instant::now();
        progress.progress();
        match run(progress.clone()).await {
            Ok(()) => tracing::warn!(role, "Runtime background role exited unexpectedly"),
            Err(error) => tracing::error!(%error, role, "Runtime background role failed"),
        }
        if started.elapsed() >= Duration::from_secs(30) {
            consecutive_failures = 0;
        } else {
            consecutive_failures = consecutive_failures.saturating_add(1).min(6);
        }
        let backoff_ms = 100u64
            .saturating_mul(1u64 << consecutive_failures)
            .min(5_000);
        tracing::warn!(role, backoff_ms, "Runtime background role will restart");
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
    }
}

async fn artifact_loop(
    state: agentx_v2_runtime::RuntimeState,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        if !agentx_v2_runtime::artifact::externalize_one(&state).await? {
            progress.processed_since(started).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        } else {
            progress.processed_since(started).await;
        }
    }
}

async fn quota_projection_loop(
    pool: sqlx::MySqlPool,
    mut redis: ConnectionManager,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        progress.progress();
        let Some(lease) = agentx_v2_runtime::quota::claim_projection(&pool, owner).await? else {
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        };
        loop {
            let started = std::time::Instant::now();
            for counter in agentx_v2_runtime::quota::counter_projection(&pool).await? {
                let prefix = format!(
                    "agentx:v2:quota:v1:{}:{}",
                    counter.tenant_id, counter.dimension
                );
                let _: () = redis
                    .set(format!("{prefix}:limit"), counter.hard_limit)
                    .await?;
                let _: () = redis
                    .set(
                        format!("{prefix}:used"),
                        counter.active.saturating_add(counter.committed),
                    )
                    .await?;
            }
            let _: () = redis
                .set(
                    "agentx:v2:quota:v1:projection-ready",
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .await?;
            if agentx_v2_runtime::quota::heartbeat_projection(&pool, lease)
                .await
                .is_err()
            {
                break;
            }
            progress.processed_since(started).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

fn runtime_roles() -> Result<std::collections::BTreeSet<String>> {
    let roles = env::var("AGENTX_RUNTIME_ROLES")
        .unwrap_or_else(|_| "coordinator,command,outbox,recovery,artifact,quota".into())
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    anyhow::ensure!(
        roles.iter().all(|role| matches!(
            role.as_str(),
            "coordinator"
                | "command"
                | "outbox"
                | "recovery"
                | "trigger"
                | "stream"
                | "artifact"
                | "quota"
                | "trace-relay"
                | "delivery"
        )),
        "AGENTX_RUNTIME_ROLES contains an unsupported role"
    );
    Ok(roles)
}

async fn trigger_loop(
    pool: sqlx::MySqlPool,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        let claims = agentx_v2_runtime::trigger::claim(&pool, owner, 100).await?;
        if claims.is_empty() {
            tokio::time::sleep(Duration::from_secs(1)).await;
            progress.processed_since(started).await;
            continue;
        }
        let mut executions = tokio::task::JoinSet::new();
        for claim in claims {
            let pool = pool.clone();
            executions.spawn(async move {
                let binding_id = claim.binding_id;
                (
                    binding_id,
                    agentx_v2_runtime::trigger::execute_with_heartbeat(&pool, &claim).await,
                )
            });
        }
        while let Some(result) = executions.join_next().await {
            let (binding_id, result) = result?;
            if let Err(error) = result {
                tracing::warn!(%error,%binding_id,"Runtime Trigger failed");
            }
        }
        progress.processed_since(started).await;
    }
}

async fn maintenance_loop(
    state: agentx_v2_runtime::RuntimeState,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        agentx_v2_runtime::gc::cleanup_expired_temporary_objects(&state, 100).await?;
        let run_id = Uuid::now_v7();
        agentx_v2_runtime::gc::mark_collectable(&state, run_id).await?;
        while agentx_v2_runtime::gc::sweep_one(&state, run_id, owner).await? {}
        progress.processed_since(started).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

async fn retention_loop(
    state: agentx_v2_runtime::RuntimeState,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        if !agentx_v2_runtime::retention::run_once(&state, owner).await? {
            progress.processed_since(started).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        } else {
            progress.processed_since(started).await;
        }
    }
}

async fn command_loop(
    state: agentx_v2_runtime::RuntimeState,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        let claims = agentx_v2_runtime::execution::claim_commands(&state.pool, owner, 100).await?;
        if claims.is_empty() {
            tokio::time::sleep(Duration::from_millis(200)).await;
            progress.processed_since(started).await;
            continue;
        }
        for claim in claims {
            if let Err(error) =
                agentx_v2_runtime::execution::process_command_with_state(&state, &claim).await
            {
                tracing::warn!(%error,command_id=%claim.command_id,"Runtime command failed")
            }
        }
        progress.processed_since(started).await;
    }
}

async fn event_sequencer_loop(
    pool: sqlx::MySqlPool,
    mut redis: ConnectionManager,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        if let Some(event) = agentx_v2_runtime::event_export::sequence_one(&pool, owner).await? {
            if let Some(invocation_id) = event.invocation_id {
                agentx_v2_runtime::sse_wakeup::publish_connection(&mut redis, invocation_id)
                    .await?;
            }
            progress.processed_since(started).await;
            continue;
        }
        progress.processed_since(started).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn dispatch_loop(
    pool: sqlx::MySqlPool,
    mut redis: ConnectionManager,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        let Some(claim) = agentx_v2_runtime::execution::claim_dispatch(&pool, owner).await? else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            progress.processed_since(started).await;
            continue;
        };
        if let Err(error) = runtime_task_queue::publish(&mut redis, &claim.task()?).await {
            if let Err(release_error) =
                agentx_v2_runtime::execution::release_dispatch(&pool, &claim, &error.to_string())
                    .await
            {
                tracing::warn!(
                    %release_error,
                    outbox_id = %claim.id,
                    "Failed to release Runtime dispatch after Redis publish failure"
                );
            }
            return Err(error);
        }
        agentx_v2_runtime::execution::complete_dispatch(&pool, &claim).await?;
        progress.processed_since(started).await;
    }
}

async fn trace_relay_loop(
    pool: sqlx::MySqlPool,
    mut redis: ConnectionManager,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    let mut last_stream_check = tokio::time::Instant::now() - Duration::from_secs(1);
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        if last_stream_check.elapsed() >= Duration::from_secs(1) {
            if agentx_v2_runtime::trace_delivery::ensure_stream(&mut redis).await? {
                let requeued =
                    agentx_v2_runtime::trace_delivery::requeue_after_stream_loss(&pool).await?;
                tracing::warn!(
                    requeued,
                    "Runtime Trace Stream was rebuilt from MySQL Outbox"
                );
            }
            last_stream_check = tokio::time::Instant::now();
        }
        let claims = agentx_v2_runtime::trace_delivery::claim(&pool, owner).await?;
        if claims.is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            progress.processed_since(started).await;
            continue;
        }
        match agentx_v2_runtime::trace_delivery::publish(&mut redis, &claims).await {
            Ok(stream_ids) => {
                agentx_v2_runtime::trace_delivery::complete(&pool, &claims, &stream_ids).await?;
            }
            Err(error) => {
                tracing::warn!(%error,events=claims.len(),"Trace Relay publish failed");
                agentx_v2_runtime::trace_delivery::fail(&pool, &claims, &error.to_string()).await?;
            }
        }
        progress.processed_since(started).await;
    }
}

/// Delivery loop (plan7 P7-A): claims delivery_outbox rows, sends through the
/// provider clients and records the delivery outcome on the invocation event
/// stream so the SSE consumers see reply state transitions. Wakeup rides the
/// existing sequencer: the runtime_event row enqueued with the outcome is
/// processed by the event-sequencer role which publishes sse_wakeup.
async fn delivery_loop(
    pool: sqlx::MySqlPool,
    state: agentx_v2_runtime::RuntimeState,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    let http = agentx_v2_runtime::egress::ProviderHttpClient::from_env(
        agentx_runtime_contracts::EgressRole::WorkflowRuntime,
    )?;
    let Some(vault) = state.vault.clone() else {
        anyhow::bail!("delivery role requires the runtime vault");
    };
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        // A crashed delivery loop leaves rows in 'delivering' with a dead
        // lease; requeue expired ones so in-flight deliveries are not lost.
        agentx_v2_runtime::delivery::requeue_expired(&pool).await?;
        let Some(claim) = agentx_v2_runtime::delivery::claim(&pool, owner).await? else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            progress.processed_since(started).await;
            continue;
        };
        let outcome = agentx_v2_runtime::delivery_send::send(&vault, &http, &claim).await;
        let event_payload = match &outcome {
            Ok(provider_message_id) => {
                if let Err(error) = agentx_v2_runtime::delivery::complete(
                    &pool,
                    &claim,
                    provider_message_id.as_deref(),
                )
                .await
                {
                    tracing::warn!(%error, delivery_id = %claim.id, "Delivery complete lost lease");
                }
                serde_json::json!({
                    "deliveryId": claim.id,
                    "status": "delivered",
                    "provider": claim.provider,
                    "providerMessageId": provider_message_id,
                })
            }
            Err(error) => {
                let result = if error.retryable {
                    agentx_v2_runtime::delivery::fail_retryable(
                        &pool,
                        &claim,
                        error.code,
                        &error.message,
                    )
                    .await
                } else {
                    agentx_v2_runtime::delivery::dead(&pool, &claim, error.code, &error.message)
                        .await
                };
                if let Err(failure) = result {
                    tracing::warn!(%failure, delivery_id = %claim.id, "Delivery failure update lost lease");
                }
                serde_json::json!({
                    "deliveryId": claim.id,
                    "status": if error.retryable { "retry_scheduled" } else { "dead" },
                    "provider": claim.provider,
                    "errorCode": error.code,
                    "errorMessage": error.message,
                })
            }
        };
        record_delivery_event(&pool, &claim, event_payload).await;
        progress.processed_since(started).await;
    }
}

async fn record_delivery_event(
    pool: &sqlx::MySqlPool,
    claim: &agentx_v2_runtime::delivery::DeliveryClaim,
    payload: serde_json::Value,
) {
    // invocation_events is keyed by invocation; send_message deliveries that
    // run outside an invocation only publish the runtime_event row.
    if claim.invocation_id.is_none() {
        let inserted = sqlx::query(
            "INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,payload_json,status) VALUES(?,?,?,'runtime_event',?,'pending')",
        )
        .bind(Uuid::now_v7())
        .bind(claim.tenant_id)
        .bind(claim.execution_id)
        .bind(serde_json::json!({"type": "delivery.completed", "deliveryId": claim.id}))
        .execute(pool)
        .await;
        if let Err(error) = inserted {
            tracing::warn!(%error, delivery_id = %claim.id, "Delivery outbox event insert failed");
        }
        return;
    }
    let Ok(mut tx) = pool.begin().await else {
        tracing::warn!(delivery_id = %claim.id, "Delivery event transaction failed to start");
        return;
    };
    let event_type =
        if payload.get("status").and_then(serde_json::Value::as_str) == Some("delivered") {
            "delivery.completed"
        } else {
            "delivery.failed"
        };
    if sqlx::query("SELECT id FROM application_invocations WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(claim.tenant_id)
        .bind(claim.invocation_id)
        .fetch_one(&mut *tx)
        .await
        .is_err()
    {
        return;
    }
    let next: Option<u64> = sqlx::query_scalar(
        "SELECT CAST(COALESCE(MAX(sequence_number),0)+1 AS UNSIGNED) FROM invocation_events WHERE tenant_id=? AND invocation_id=? FOR UPDATE",
    )
    .bind(claim.tenant_id)
    .bind(claim.invocation_id)
    .fetch_one(&mut *tx)
    .await
    .ok();
    let Some(next) = next else {
        return;
    };
    let inserted = sqlx::query(
        "INSERT INTO invocation_events(tenant_id,invocation_id,event_id,sequence_number,event_type,payload_json) VALUES(?,?,?,?,?,?)",
    )
    .bind(claim.tenant_id)
    .bind(claim.invocation_id)
    .bind(Uuid::now_v7())
    .bind(next)
    .bind(event_type)
    .bind(&payload)
    .execute(&mut *tx)
    .await;
    let outboxed = sqlx::query(
        "INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,payload_json,status) VALUES(?,?,?,'runtime_event',?,'pending')",
    )
    .bind(Uuid::now_v7())
    .bind(claim.tenant_id)
    .bind(claim.execution_id)
    .bind(serde_json::json!({"type": event_type, "deliveryId": claim.id}))
    .execute(&mut *tx)
    .await;
    match (inserted, outboxed) {
        (Ok(_), Ok(_)) => {
            if let Err(error) = tx.commit().await {
                tracing::warn!(%error, delivery_id = %claim.id, "Delivery event commit failed");
            }
        }
        (error_insert, error_outbox) => {
            let _ = tx.rollback().await;
            tracing::warn!(
                ?error_insert,
                ?error_outbox,
                delivery_id = %claim.id,
                "Delivery event insert failed"
            );
        }
    }
}

async fn recovery_loop(
    pool: sqlx::MySqlPool,
    mut redis: ConnectionManager,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        agentx_v2_runtime::enqueue_due_approval_timeouts(&pool, owner, 100).await?;
        agentx_v2_runtime::composite_execution::enqueue_overdue(&pool, 100).await?;
        let woken =
            agentx_v2_runtime::agent_session_queue::wake_pending_sessions(&pool, 100).await?;
        if woken > 0 {
            tracing::debug!(woken, "Agent Session pending inputs scheduled for resume");
        }
        runtime_task_queue::ensure_groups(&mut redis).await?;
        for message in agentx_v2_runtime::execution::recover_dispatches(&pool, 100).await? {
            runtime_task_queue::publish(&mut redis, &message).await?;
        }
        progress.processed_since(started).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn collect_runtime_metrics(
    pool: sqlx::MySqlPool,
    redis_settings: RuntimeRedisSettings,
    metrics: agentx_service_kit::MetricsRegistry,
    health: agentx_service_kit::HealthRegistry,
    lifecycle: agentx_service_kit::ServiceLifecycle,
) -> Result<()> {
    while !lifecycle.is_draining() {
        let sample = async {
            let acquire_started = std::time::Instant::now();
            let mut connection = pool.acquire().await?;
            metrics.observe_mysql_pool_wait(acquire_started.elapsed()).await;
            let row = sqlx::query("SELECT COUNT(*) ready_items,CAST(COALESCE(MAX(TIMESTAMPDIFF(MICROSECOND,available_at,UTC_TIMESTAMP(6))),0)/1000000.0 AS DOUBLE) oldest_seconds FROM execution_outbox WHERE status='pending' AND available_at<=UTC_TIMESTAMP(6)").fetch_one(&mut *connection).await?;
            let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_attempts WHERE status='running' AND locked_until>UTC_TIMESTAMP(6)").fetch_one(&mut *connection).await?;
            Ok::<_, sqlx::Error>((sqlx::Row::try_get::<i64, _>(&row, "ready_items")?, sqlx::Row::try_get::<f64, _>(&row, "oldest_seconds")?, active))
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
                tracing::warn!(%error, "Runtime metrics refresh failed");
            }
        }
        match connect_runtime_redis(&redis_settings).await {
            Ok(mut redis) => match redis::cmd("PING").query_async::<String>(&mut redis).await {
                Ok(_) => health.set_status("runtime_redis", "ready").await,
                Err(error) => {
                    health.set_status("runtime_redis", "unavailable").await;
                    tracing::warn!(%error, "Runtime Redis readiness probe failed");
                }
            },
            Err(error) => {
                health.set_status("runtime_redis", "unavailable").await;
                tracing::warn!(%error, "Runtime Redis readiness connection failed");
            }
        }
        metrics
            .set(
                "agentx_mysql_pool_busy_connections",
                pool.size().saturating_sub(pool.num_idle() as u32) as f64,
            )
            .await;
        tokio::select! {
            () = lifecycle.cancelled() => break,
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
    Ok(())
}
