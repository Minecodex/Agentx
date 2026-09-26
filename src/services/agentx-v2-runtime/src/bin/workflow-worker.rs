use agentx_runtime_infrastructure::{
    RuntimeMySqlSettings, RuntimeObjectStorageSettings, RuntimeRedisSettings,
    connect_runtime_mysql, connect_runtime_redis, runtime_object_store,
};
use anyhow::{Context, Result};
use std::{env, future::Future, sync::Arc, time::Duration};
use uuid::Uuid;

#[path = "support/runtime_task_queue.rs"]
mod runtime_task_queue;

#[tokio::main]
async fn main() -> Result<()> {
    let capabilities = worker_capabilities()?;
    validate_plugin_runtime(&capabilities).await?;
    let pool = connect_runtime_mysql(&RuntimeMySqlSettings::from_env()?).await?;
    let redis_settings = RuntimeRedisSettings::from_env()?;
    let mut bootstrap_redis = connect_worker_redis(&redis_settings, "bootstrap").await?;
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
    for capability in &capabilities {
        agentx_v2_runtime::engine::register_worker(
            &pool,
            owner,
            capability,
            env!("CARGO_PKG_VERSION"),
        )
        .await?;
    }
    let heartbeat_pool = pool.clone();
    let heartbeat_capabilities = capabilities.clone();
    let heartbeat_lifecycle = lifecycle.clone();
    let mut tasks = tokio::task::JoinSet::<Result<()>>::new();
    tasks.spawn(async move {
        while !heartbeat_lifecycle.is_draining() {
            tokio::select! {
                () = heartbeat_lifecycle.cancelled() => break,
                () = tokio::time::sleep(Duration::from_secs(10)) => {}
            }
            for capability in &heartbeat_capabilities {
                if agentx_v2_runtime::engine::heartbeat_worker(&heartbeat_pool, owner, capability)
                    .await
                    .is_err()
                {
                    let _ = agentx_v2_runtime::engine::register_worker(
                        &heartbeat_pool,
                        owner,
                        capability,
                        env!("CARGO_PKG_VERSION"),
                    )
                    .await;
                }
            }
        }
        agentx_v2_runtime::engine::mark_worker_draining(&heartbeat_pool, owner)
            .await
            .map_err(Into::into)
    });
    let worker = Arc::new(
        agentx_v2_runtime::worker_runtime::RuntimeWorker::new(
            pool.clone(),
            runtime_object_store(&RuntimeObjectStorageSettings::from_env()?)?,
        )?
        // Model deltas publish SSE wakeups on the runtime redis; missing
        // redis degrades to the gateway's 1s poll fallback.
        .with_delta_sink(pool.clone(), redis_client_for_wakeups(&redis_settings)),
    );
    for capability in capabilities {
        let parallelism = if capability == "plugin_nodejs" {
            worker.plugin_parallelism()
        } else {
            1
        };
        for lane in 0..parallelism {
            let capability = capability.clone();
            let pool = pool.clone();
            // XREAD BLOCK must not share a multiplexed Redis connection with the
            // other capability loops. A cloned ConnectionManager shares the same
            // physical connection and serializes blocking reads, which can starve
            // an otherwise ready capability for longer than an Invocation timeout.
            let capability_redis_settings = redis_settings.clone();
            let redis = WorkerRedis {
                connection: connect_worker_redis(
                    &capability_redis_settings,
                    &format!("capability:{capability}:{lane}"),
                )
                .await?,
                settings: capability_redis_settings,
            };
            let worker = worker.clone();
            let worker_lifecycle = lifecycle.clone();
            let progress = agentx_service_kit::RoleProgressWatchdog::start(
                format!("worker:{capability}:{lane}"),
                Duration::from_secs(agentx_service_kit::ROLE_WATCHDOG_TIMEOUT_SECONDS),
                health.clone(),
                lifecycle.clone(),
                metrics.clone(),
            )
            .await;
            tasks.spawn(async move {
                worker_loop(
                    pool,
                    redis,
                    worker,
                    owner,
                    capability,
                    lane,
                    worker_lifecycle,
                    progress,
                )
                .await
            });
        }
    }
    let service_lifecycle = lifecycle.clone();
    let service_metrics = metrics.clone();
    let metrics_health = health.clone();
    let metrics_redis = redis_settings.clone();
    tasks.spawn(async move {
        agentx_service_kit::serve_with_lifecycle(
            "workflow-worker",
            axum::Router::new(),
            health,
            service_lifecycle,
            service_metrics,
        )
        .await
    });
    let metrics_lifecycle = lifecycle.clone();
    tasks.spawn(async move {
        collect_worker_metrics(
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

async fn validate_plugin_runtime(capabilities: &[String]) -> Result<()> {
    if !capabilities.iter().any(|value| value == "plugin_nodejs") {
        return Ok(());
    }
    let node = env::var("AGENTX_NODE_EXECUTABLE").unwrap_or_else(|_| "node".into());
    let output = tokio::process::Command::new(&node)
        .arg("--version")
        .output()
        .await
        .with_context(|| format!("start Node.js executable '{node}'"))?;
    let version = String::from_utf8_lossy(&output.stdout);
    anyhow::ensure!(
        output.status.success() && version.trim() == "v24.20.0",
        "plugin_nodejs requires Node.js 24.20.0; found {}",
        version.trim()
    );
    let runner = env::var("AGENTX_PLUGIN_RUNNER_PATH")
        .unwrap_or_else(|_| "/opt/agentx/plugin-runner/runner.mjs".into());
    anyhow::ensure!(
        std::path::Path::new(&runner).is_file(),
        "plugin_nodejs Runner is missing at {runner}"
    );
    let memory_mb = env::var("AGENTX_PLUGIN_MEMORY_MB")
        .unwrap_or_else(|_| "96".into())
        .parse::<u32>()
        .context("AGENTX_PLUGIN_MEMORY_MB must be an integer")?;
    anyhow::ensure!(
        (32..=1024).contains(&memory_mb),
        "AGENTX_PLUGIN_MEMORY_MB must be between 32 and 1024"
    );
    Ok(())
}

async fn connect_worker_redis(
    settings: &RuntimeRedisSettings,
    purpose: &str,
) -> Result<redis::aio::ConnectionManager> {
    const MAX_ATTEMPTS: u32 = 6;
    for attempt in 1..=MAX_ATTEMPTS {
        match connect_runtime_redis(settings).await {
            Ok(connection) => return Ok(connection),
            Err(error) if attempt < MAX_ATTEMPTS => {
                tracing::warn!(%error, %purpose, attempt, "Worker Redis startup connection failed; retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Worker Redis startup connection failed for {purpose} after {MAX_ATTEMPTS} attempts"
                    )
                });
            }
        }
    }
    unreachable!("Worker Redis retry loop always returns")
}

fn worker_capabilities() -> Result<Vec<String>> {
    let configured = env::var("AGENTX_WORKER_CAPABILITIES")
        .or_else(|_| env::var("AGENTX_WORKER_CAPABILITY"))
        .unwrap_or_else(|_| "all".into());
    let capabilities = if configured.trim() == "all" {
        agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    } else {
        configured
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    anyhow::ensure!(!capabilities.is_empty(), "Worker capability set is empty");
    for capability in &capabilities {
        anyhow::ensure!(
            agentx_node_protocol::ALL_RUNTIME_CAPABILITIES.contains(&capability.as_str()),
            "unsupported Worker capability {capability}"
        );
    }
    Ok(capabilities)
}

#[allow(clippy::too_many_arguments)]
async fn worker_loop(
    pool: sqlx::MySqlPool,
    mut redis: WorkerRedis,
    worker: Arc<agentx_v2_runtime::worker_runtime::RuntimeWorker>,
    owner: Uuid,
    capability: String,
    lane: usize,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    let consumer = format!("{owner}:{capability}:{lane}");
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        match process_worker_batch(
            &pool,
            &mut redis.connection,
            &worker,
            owner,
            &capability,
            &consumer,
            &lifecycle,
        )
        .await
        {
            Ok(()) => progress.processed_since(started).await,
            Err(error) => {
                // A dependency failure is not a stalled scheduler: the loop is
                // still making progress by reclaiming the batch and retrying.
                // Keep liveness tied to the local role loop so a Redis rebuild
                // cannot trigger a false Kubernetes restart after the watchdog
                // timeout.
                progress.processed_since(started).await;
                tracing::warn!(%error, %capability, "Worker batch failed and will be reclaimed");
                match connect_worker_redis(
                    &redis.settings,
                    &format!("capability:{capability}:reconnect"),
                )
                .await
                {
                    Ok(connection) => redis.connection = connection,
                    Err(reconnect_error) => tracing::warn!(
                        error = %reconnect_error,
                        %capability,
                        "Worker Redis reconnect failed; retrying the capability loop"
                    ),
                }
                tokio::select! {
                    () = lifecycle.cancelled() => return Ok(()),
                    () = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        }
    }
}

struct WorkerRedis {
    connection: redis::aio::ConnectionManager,
    settings: RuntimeRedisSettings,
}

/// A plain client handle for best-effort pubsub from the delta sink; None
/// keeps the sink write-only when redis is not reachable at startup.
fn redis_client_for_wakeups(settings: &RuntimeRedisSettings) -> Option<redis::Client> {
    agentx_runtime_infrastructure::runtime_redis_client(settings).ok()
}

#[allow(clippy::too_many_arguments)]
async fn process_worker_batch(
    pool: &sqlx::MySqlPool,
    redis: &mut redis::aio::ConnectionManager,
    worker: &agentx_v2_runtime::worker_runtime::RuntimeWorker,
    owner: Uuid,
    capability: &str,
    consumer: &str,
    lifecycle: &agentx_service_kit::ServiceLifecycle,
) -> Result<()> {
    // XREAD itself blocks for one second. Bound the full queue read so a
    // multiplexed connection left half-open by a Redis Pod replacement cannot
    // freeze this capability forever. The outer loop replaces the connection
    // after either an I/O error or this deadline.
    let items = tokio::time::timeout(
        Duration::from_secs(10),
        runtime_task_queue::read(redis, capability, consumer, 1_000),
    )
    .await
    .context("Runtime Worker queue read timed out")??;
    for item in items {
        let Some(claim) =
            agentx_v2_runtime::engine::claim_worker_attempt(pool, owner, capability, &item.task)
                .await?
        else {
            runtime_task_queue::ack(redis, &item).await?;
            continue;
        };
        let mut lease = claim.lease.clone();
        let execution = agentx_v2_runtime::worker_support::with_operation_deadline(
            claim.task.deadline_at,
            worker.execute(&claim),
        );
        tokio::pin!(execution);
        let drain_deadline = lifecycle.drain_deadline();
        tokio::pin!(drain_deadline);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        heartbeat.tick().await;
        let outcome = loop {
            tokio::select! {
                result = &mut execution => break result.unwrap_or_else(|_| agentx_v2_runtime::worker_runtime::WorkerExecution {
                    status: agentx_runtime_contracts::WorkerResultStatusV1::Failed,
                    outputs: std::collections::BTreeMap::new(),
                    error_code: Some("NODE_EXECUTION_TIMED_OUT".into()),
                    error_message: Some("Node execution exceeded its operation deadline".into()),
                    retryable: Some(false),
                }),
                () = &mut drain_deadline => return Ok(()),
                _ = heartbeat.tick() => {
                    lease = heartbeat_attempt_with_retry(
                        pool,
                        &lease,
                        capability,
                        "execution",
                    ).await?;
                }
            }
        };
        tracing::info!(
            attempt_id = %claim.task.attempt_id,
            execution_id = %claim.task.execution_id,
            %capability,
            "Worker execution finished; finalizing result"
        );
        // Provider work may consume almost the entire 30-second Attempt Lease.
        // Refresh it before Artifact/result finalization so a valid result is
        // not reclaimed while it is being made durable.
        lease = heartbeat_attempt_with_retry(pool, &lease, capability, "finalization").await?;
        let mut result = worker.build_result(&claim, outcome).await?;
        result.fencing_token = lease.fencing_token;
        tracing::info!(
            attempt_id = %claim.task.attempt_id,
            execution_id = %claim.task.execution_id,
            %capability,
            "Worker result built; submitting authoritative transition"
        );
        submit_result_with_retry(pool, worker, &mut lease, &result, capability).await?;
        runtime_task_queue::ack(redis, &item).await?;
    }
    Ok(())
}

async fn submit_result_with_retry(
    pool: &sqlx::MySqlPool,
    worker: &agentx_v2_runtime::worker_runtime::RuntimeWorker,
    lease: &mut agentx_runtime_contracts::WorkerAttemptLeaseV1,
    result: &agentx_runtime_contracts::WorkerResultV1,
    capability: &str,
) -> Result<()> {
    const MAX_ATTEMPTS: u32 = 3;
    const SUBMIT_TIMEOUT: Duration = Duration::from_secs(20);
    let mut repeated_error: Option<(String, u32)> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let submitted = tokio::time::timeout(
            SUBMIT_TIMEOUT,
            agentx_v2_runtime::engine::submit_worker_result_with_objects(
                pool,
                worker.object_store(),
                result,
            ),
        )
        .await;
        match submitted {
            Ok(Ok(_)) => {
                tracing::info!(
                    attempt_id = %result.attempt_id,
                    %capability,
                    submit_attempt = attempt,
                    "Worker result transition committed"
                );
                return Ok(());
            }
            Ok(Err(agentx_v2_runtime::error::RuntimeError::DatabaseUnavailable))
                if attempt < MAX_ATTEMPTS =>
            {
                tracing::warn!(
                    attempt_id = %result.attempt_id,
                    %capability,
                    submit_attempt = attempt,
                    "Worker result submission hit a transient database failure; retrying"
                );
            }
            Ok(Err(agentx_v2_runtime::error::RuntimeError::Conflict(_, message))) => {
                tracing::info!(
                    attempt_id = %result.attempt_id,
                    %capability,
                    %message,
                    "Worker result lost its Lease race; acknowledging the stale task"
                );
                return Ok(());
            }
            Ok(Err(error)) if non_infrastructure_submission_error(&error) => {
                let signature = format!("{error:?}");
                let count = repeated_error
                    .as_ref()
                    .filter(|(previous, _)| previous == &signature)
                    .map_or(1, |(_, count)| count + 1);
                repeated_error = Some((signature, count));
                if count >= MAX_ATTEMPTS {
                    return submit_poisoned_result(pool, lease, result, &error.to_string()).await;
                }
                tracing::warn!(
                    attempt_id = %result.attempt_id,
                    %capability,
                    submit_attempt = attempt,
                    %error,
                    "Worker result submission was deterministically rejected; retrying before poisoning"
                );
            }
            Ok(Err(error)) => return Err(error.into()),
            Err(_) if attempt < MAX_ATTEMPTS => {
                tracing::warn!(
                    attempt_id = %result.attempt_id,
                    %capability,
                    submit_attempt = attempt,
                    timeout_seconds = SUBMIT_TIMEOUT.as_secs(),
                    "Worker result submission timed out; retrying idempotently"
                );
            }
            Err(_) => {
                anyhow::bail!("Worker result submission timed out after {MAX_ATTEMPTS} attempts");
            }
        }
        *lease = heartbeat_attempt_with_retry(pool, lease, capability, "result_submission").await?;
        tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
    }
    unreachable!("bounded Worker result retry loop always returns")
}

async fn heartbeat_attempt_with_retry(
    pool: &sqlx::MySqlPool,
    lease: &agentx_runtime_contracts::WorkerAttemptLeaseV1,
    capability: &str,
    phase: &str,
) -> agentx_v2_runtime::error::RuntimeResult<agentx_runtime_contracts::WorkerAttemptLeaseV1> {
    heartbeat_attempt_with_retry_using(lease, capability, phase, || {
        agentx_v2_runtime::engine::heartbeat_attempt(pool, lease)
    })
    .await
}

async fn heartbeat_attempt_with_retry_using<F, Fut>(
    lease: &agentx_runtime_contracts::WorkerAttemptLeaseV1,
    capability: &str,
    phase: &str,
    mut heartbeat: F,
) -> agentx_v2_runtime::error::RuntimeResult<agentx_runtime_contracts::WorkerAttemptLeaseV1>
where
    F: FnMut() -> Fut,
    Fut: Future<
        Output = agentx_v2_runtime::error::RuntimeResult<
            agentx_runtime_contracts::WorkerAttemptLeaseV1,
        >,
    >,
{
    const MAX_RETRIES: u32 = 3;
    for retry in 0..=MAX_RETRIES {
        match heartbeat().await {
            Ok(refreshed) => return Ok(refreshed),
            Err(agentx_v2_runtime::error::RuntimeError::DatabaseUnavailable)
                if retry < MAX_RETRIES =>
            {
                let backoff = Duration::from_millis(50 * u64::from(retry + 1));
                tracing::warn!(
                    attempt_id = %lease.attempt_id,
                    %capability,
                    %phase,
                    heartbeat_retry = retry + 1,
                    backoff_milliseconds = backoff.as_millis(),
                    "Worker Attempt heartbeat hit a transient database failure; retrying"
                );
                tokio::time::sleep(backoff).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded Worker heartbeat retry loop always returns")
}

fn non_infrastructure_submission_error(error: &agentx_v2_runtime::error::RuntimeError) -> bool {
    use agentx_v2_runtime::error::RuntimeError;
    matches!(
        error,
        RuntimeError::Deterministic { .. }
            | RuntimeError::InvalidRequest(..)
            | RuntimeError::BadRequest(..)
            | RuntimeError::Unauthorized
            | RuntimeError::NotFound
            | RuntimeError::ProviderRejected
            | RuntimeError::Internal(_)
    )
}

async fn submit_poisoned_result(
    pool: &sqlx::MySqlPool,
    lease: &agentx_runtime_contracts::WorkerAttemptLeaseV1,
    result: &agentx_runtime_contracts::WorkerResultV1,
    rejection: &str,
) -> Result<()> {
    let outputs = std::collections::BTreeMap::new();
    let message = format!("Worker Result was rejected three times: {rejection}");
    let result_hash = agentx_v2_runtime::engine::worker_result_hash(
        agentx_runtime_contracts::WorkerResultStatusV1::Failed,
        &outputs,
        None,
        Some("WORKER_RESULT_POISONED"),
        Some(&message),
        None,
        None,
    )?;
    let poisoned = agentx_runtime_contracts::WorkerResultV1 {
        protocol_version: result.protocol_version,
        attempt_id: result.attempt_id,
        worker_id: result.worker_id,
        fencing_token: lease.fencing_token,
        status: agentx_runtime_contracts::WorkerResultStatusV1::Failed,
        result_hash,
        outputs,
        output_object: None,
        error_code: Some("WORKER_RESULT_POISONED".into()),
        error_message: Some(message),
        retryable: None,
        partial_output_object: None,
    };
    agentx_v2_runtime::engine::submit_worker_result(pool, &poisoned).await?;
    tracing::error!(
        attempt_id = %result.attempt_id,
        "Worker result was poisoned after three deterministic submission failures"
    );
    Ok(())
}

async fn collect_worker_metrics(
    pool: sqlx::MySqlPool,
    redis_settings: RuntimeRedisSettings,
    metrics: agentx_service_kit::MetricsRegistry,
    health: agentx_service_kit::HealthRegistry,
    lifecycle: agentx_service_kit::ServiceLifecycle,
) -> Result<()> {
    while !lifecycle.is_draining() {
        let sample = async {
            let ready: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_attempts WHERE status='queued'").fetch_one(&pool).await?;
            let oldest: f64 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(TIMESTAMPDIFF(MICROSECOND,created_at,UTC_TIMESTAMP(6))),0)/1000000.0 AS DOUBLE) FROM node_attempts WHERE status='queued'").fetch_one(&pool).await?;
            let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_attempts WHERE status='running' AND locked_until>UTC_TIMESTAMP(6)").fetch_one(&pool).await?;
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
                tracing::warn!(%error, "Worker metrics refresh failed");
            }
        }
        match connect_runtime_redis(&redis_settings).await {
            Ok(mut redis) => match redis::cmd("PING").query_async::<String>(&mut redis).await {
                Ok(_) => health.set_status("runtime_redis", "ready").await,
                Err(error) => {
                    health.set_status("runtime_redis", "unavailable").await;
                    tracing::warn!(%error, "Worker Redis readiness probe failed");
                }
            },
            Err(error) => {
                health.set_status("runtime_redis", "unavailable").await;
                tracing::warn!(%error, "Worker Redis readiness connection failed");
            }
        }
        metrics
            .set(
                "agentx_mysql_pool_waiters",
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

#[cfg(test)]
mod tests {
    use super::heartbeat_attempt_with_retry_using;
    use agentx_runtime_contracts::{RuntimePublishErrorCodeV1, WorkerAttemptLeaseV1};
    use agentx_v2_runtime::error::RuntimeError;
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn lease() -> WorkerAttemptLeaseV1 {
        WorkerAttemptLeaseV1 {
            protocol_version: 1,
            attempt_id: Uuid::now_v7(),
            worker_id: Uuid::now_v7(),
            fencing_token: 7,
            locked_until: OffsetDateTime::now_utc(),
        }
    }

    #[tokio::test]
    async fn completed_result_finalization_retries_a_transient_heartbeat_failure() {
        let original = lease();
        let expected = original.clone();
        let attempts = Arc::new(AtomicU32::new(0));
        let observed_attempts = attempts.clone();

        let refreshed =
            heartbeat_attempt_with_retry_using(&original, "code", "finalization", move || {
                let attempt = observed_attempts.fetch_add(1, Ordering::SeqCst);
                let expected = expected.clone();
                async move {
                    if attempt == 0 {
                        Err(RuntimeError::DatabaseUnavailable)
                    } else {
                        Ok(expected)
                    }
                }
            })
            .await
            .expect("a transient heartbeat failure must not discard a completed result");

        assert_eq!(refreshed.attempt_id, original.attempt_id);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn heartbeat_lease_errors_are_not_retried() {
        let original = lease();
        let attempts = Arc::new(AtomicU32::new(0));
        let observed_attempts = attempts.clone();

        let error =
            heartbeat_attempt_with_retry_using(&original, "code", "finalization", move || {
                observed_attempts.fetch_add(1, Ordering::SeqCst);
                async {
                    Err(RuntimeError::Conflict(
                        RuntimePublishErrorCodeV1::IdempotencyConflict,
                        "Attempt Lease was lost".into(),
                    ))
                }
            })
            .await
            .expect_err("non-database heartbeat failures must remain fail-fast");

        assert!(matches!(error, RuntimeError::Conflict(_, _)));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
