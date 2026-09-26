use std::{
    collections::{BTreeSet, HashMap, HashSet},
    env,
    sync::Arc,
    time::Duration,
};

use agentx_runtime_contracts::{
    ContentHash, DelegationClaimsV1, ExecutionTraceV1, ObservabilityAggregatePageV1,
    ObservabilityAggregateRequestV1, ObservabilityAggregateRowV1, ObservabilityDimensionV1,
    ObservabilityMetricV1, TraceContentV1, TraceEventEnvelopeV1, TraceEventKindV1,
    TraceSearchPageV1, TraceSearchRequestV1, TraceSpanDetailV1, TraceSpanKindV1,
    TraceSpanSummaryV1, content_hash,
};
use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use redis::{
    AsyncCommands, FromRedisValue,
    aio::ConnectionManager,
    streams::{
        StreamAutoClaimOptions, StreamAutoClaimReply, StreamId, StreamPendingReply,
        StreamReadOptions, StreamReadReply,
    },
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

const TRACE_STREAM: &str = "agentx:v2:trace:v1";
const TRACE_GROUP: &str = "agentx:v2:observability:v1";

#[derive(Clone)]
struct AppState {
    clickhouse_query: clickhouse::Client,
    clickhouse_consumer: clickhouse::Client,
    redis: RedisSettings,
    keys: Arc<HashMap<String, Vec<u8>>>,
    issuer: String,
    consumer: String,
    tenant_limits: Arc<Mutex<HashMap<Uuid, Arc<Semaphore>>>>,
}

#[derive(Clone)]
struct RedisSettings {
    url: SecretString,
    username: Option<String>,
    password: Option<SecretString>,
}

#[derive(clickhouse::Row, Deserialize, Serialize)]
struct TraceRow {
    #[serde(with = "clickhouse::serde::uuid")]
    event_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    tenant_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    execution_id: Uuid,
    execution_sequence: u64,
    #[serde(with = "clickhouse::serde::uuid")]
    trace_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    span_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid::option")]
    parent_span_id: Option<Uuid>,
    event_kind: String,
    span_kind: String,
    span_name: String,
    #[serde(with = "clickhouse::serde::uuid::option")]
    node_execution_id: Option<Uuid>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    attempt_id: Option<Uuid>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    agent_run_id: Option<Uuid>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    agent_iteration_id: Option<Uuid>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    runtime_call_id: Option<Uuid>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    sandbox_lease_id: Option<Uuid>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    wait_id: Option<Uuid>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    workflow_id: Option<Uuid>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    application_id: Option<Uuid>,
    resource_type: Option<String>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    resource_id: Option<Uuid>,
    resource_version: Option<String>,
    event_type: String,
    status: String,
    error_code: Option<String>,
    error_message: Option<String>,
    duration_ms: Option<u64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cost_micros: u64,
    attributes_json: String,
    #[serde(with = "clickhouse::serde::uuid::option")]
    content_ref: Option<Uuid>,
    content_kind: Option<String>,
    content_preview_json: Option<String>,
    content_hash: String,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    occurred_at: OffsetDateTime,
}

#[derive(clickhouse::Row, Deserialize)]
struct TraceSpanKeyRow {
    #[serde(with = "clickhouse::serde::uuid")]
    span_id: Uuid,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    started_at: OffsetDateTime,
}

impl From<TraceEventEnvelopeV1> for TraceRow {
    fn from(value: TraceEventEnvelopeV1) -> Self {
        Self {
            event_id: value.event_id,
            tenant_id: value.tenant_id,
            execution_id: value.execution_id,
            execution_sequence: value.execution_sequence,
            trace_id: value.trace_id,
            workflow_id: value.workflow_id,
            application_id: value.application_id,
            span_id: value.span_id,
            parent_span_id: value.parent_span_id,
            event_kind: trace_event_kind_name(value.event_kind).into(),
            span_kind: trace_span_kind_name(value.span_kind).into(),
            span_name: value.span_name,
            node_execution_id: value.node_execution_id,
            attempt_id: value.attempt_id,
            agent_run_id: value.agent_run_id,
            agent_iteration_id: value.agent_iteration_id,
            runtime_call_id: value.runtime_call_id,
            sandbox_lease_id: value.sandbox_lease_id,
            wait_id: value.wait_id,
            resource_type: value.resource_type,
            resource_id: value.resource_id,
            resource_version: value.resource_version,
            event_type: value.event_type,
            status: value.status,
            error_code: value.error_code,
            error_message: value.error_message,
            duration_ms: value.duration_ms,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cost_micros: value.cost_micros,
            attributes_json: value.attributes.to_string(),
            content_ref: value.content_ref,
            content_kind: value.content_kind.and_then(|kind| {
                serde_json::to_value(kind)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
            }),
            content_preview_json: value.content_preview.map(|preview| preview.to_string()),
            content_hash: value.content_hash.to_string(),
            occurred_at: value.occurred_at,
        }
    }
}

impl TryFrom<TraceRow> for TraceEventEnvelopeV1 {
    type Error = anyhow::Error;

    fn try_from(value: TraceRow) -> Result<Self> {
        Ok(Self {
            schema_version: 1,
            event_id: value.event_id,
            tenant_id: value.tenant_id,
            execution_id: value.execution_id,
            execution_sequence: value.execution_sequence,
            trace_id: value.trace_id,
            workflow_id: value.workflow_id,
            application_id: value.application_id,
            span_id: value.span_id,
            parent_span_id: value.parent_span_id,
            event_kind: parse_trace_event_kind(&value.event_kind)?,
            span_kind: parse_trace_span_kind(&value.span_kind)?,
            span_name: value.span_name,
            node_execution_id: value.node_execution_id,
            attempt_id: value.attempt_id,
            agent_run_id: value.agent_run_id,
            agent_iteration_id: value.agent_iteration_id,
            runtime_call_id: value.runtime_call_id,
            sandbox_lease_id: value.sandbox_lease_id,
            wait_id: value.wait_id,
            resource_type: value.resource_type,
            resource_id: value.resource_id,
            resource_version: value.resource_version,
            event_type: value.event_type,
            status: value.status,
            duration_ms: value.duration_ms,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cost_micros: value.cost_micros,
            error_code: value.error_code,
            error_message: value.error_message,
            attributes: serde_json::from_str(&value.attributes_json)?,
            content_ref: value.content_ref,
            content_kind: value
                .content_kind
                .map(|kind| serde_json::from_value(Value::String(kind)))
                .transpose()?,
            content_preview: value
                .content_preview_json
                .map(|preview| serde_json::from_str(&preview))
                .transpose()?,
            occurred_at: value.occurred_at,
            content_hash: ContentHash::parse(value.content_hash)?,
        })
    }
}

fn trace_event_kind_name(kind: TraceEventKindV1) -> &'static str {
    match kind {
        TraceEventKindV1::Started => "started",
        TraceEventKindV1::Updated => "updated",
        TraceEventKindV1::Finished => "finished",
    }
}

fn parse_trace_event_kind(value: &str) -> Result<TraceEventKindV1> {
    match value {
        "started" => Ok(TraceEventKindV1::Started),
        "updated" => Ok(TraceEventKindV1::Updated),
        "finished" => Ok(TraceEventKindV1::Finished),
        value => anyhow::bail!("unsupported Trace event kind {value}"),
    }
}

fn trace_span_kind_name(kind: TraceSpanKindV1) -> &'static str {
    match kind {
        TraceSpanKindV1::Execution => "execution",
        TraceSpanKindV1::Boundary => "boundary",
        TraceSpanKindV1::Node => "node",
        TraceSpanKindV1::Attempt => "attempt",
        TraceSpanKindV1::AgentRun => "agent_run",
        TraceSpanKindV1::AgentIteration => "agent_iteration",
        TraceSpanKindV1::RuntimeCall => "runtime_call",
        TraceSpanKindV1::Sandbox => "sandbox",
        TraceSpanKindV1::Wait => "wait",
        TraceSpanKindV1::PluginOperation => "plugin_operation",
    }
}

fn parse_trace_span_kind(value: &str) -> Result<TraceSpanKindV1> {
    match value {
        "execution" => Ok(TraceSpanKindV1::Execution),
        "boundary" => Ok(TraceSpanKindV1::Boundary),
        "node" => Ok(TraceSpanKindV1::Node),
        "attempt" => Ok(TraceSpanKindV1::Attempt),
        "agent_run" => Ok(TraceSpanKindV1::AgentRun),
        "agent_iteration" => Ok(TraceSpanKindV1::AgentIteration),
        "runtime_call" => Ok(TraceSpanKindV1::RuntimeCall),
        "sandbox" => Ok(TraceSpanKindV1::Sandbox),
        "wait" => Ok(TraceSpanKindV1::Wait),
        "plugin_operation" => Ok(TraceSpanKindV1::PluginOperation),
        value => anyhow::bail!("unsupported Trace span kind {value}"),
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED",
            "Authentication failed",
        )
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "OBSERVABILITY_UNAVAILABLE",
            message,
        )
    }

    fn budget(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "OBSERVABILITY_QUERY_BUDGET_EXCEEDED",
            message,
        )
    }

    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"code":self.code,"message":self.message,"requestId":Uuid::now_v7()})),
        )
            .into_response()
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

#[tokio::main]
async fn main() -> Result<()> {
    let state = AppState::from_env()?;
    let lifecycle = agentx_service_kit::ServiceLifecycle::default();
    let metrics = agentx_service_kit::MetricsRegistry::default();
    ensure_group(&state).await?;
    let roles = roles()?;
    let health = agentx_service_kit::HealthRegistry::default();
    health.register("runtime_redis", true).await;
    health.register("clickhouse", true).await;
    // `ensure_group` has already authenticated against the restricted Redis ACL.
    // Mark both required dependencies ready only after an explicit ClickHouse query
    // succeeds as well; otherwise the shared health registry correctly keeps this
    // pod out of service.
    state
        .clickhouse_query
        .query("SELECT 1")
        .fetch_one::<u8>()
        .await
        .context("Observability ClickHouse readiness query failed")?;
    health.set_status("runtime_redis", "ready").await;
    health.set_status("clickhouse", "ready").await;
    let mut tasks = tokio::task::JoinSet::new();
    if roles.contains("trace-consumer") {
        let consumer = state.clone();
        let consumer_lifecycle = lifecycle.clone();
        let consumer_progress = agentx_service_kit::RoleProgressWatchdog::start(
            "trace-consumer",
            Duration::from_secs(agentx_service_kit::ROLE_WATCHDOG_TIMEOUT_SECONDS),
            health.clone(),
            lifecycle.clone(),
            metrics.clone(),
        )
        .await;
        tasks.spawn(
            async move { consume_loop(consumer, consumer_lifecycle, consumer_progress).await },
        );
    }
    let router = if roles.contains("query") {
        Router::new()
            .route(
                "/internal/observability/v1/executions/{id}/trace",
                get(execution_trace),
            )
            .route(
                "/internal/observability/v1/executions/{id}/trace/spans/{span_id}",
                get(trace_span_detail),
            )
            .route(
                "/internal/observability/v1/traces:search",
                post(search_traces),
            )
            .route(
                "/internal/observability/v1/aggregates:query",
                post(query_aggregates),
            )
            .with_state(state.clone())
    } else {
        Router::new()
    };
    let service_lifecycle = lifecycle.clone();
    let service_metrics = metrics.clone();
    let metrics_health = health.clone();
    tasks.spawn(async move {
        agentx_service_kit::serve_with_lifecycle(
            "agentx-observability",
            router,
            health,
            service_lifecycle,
            service_metrics,
        )
        .await
    });
    let metrics_state = state.clone();
    let metrics_lifecycle = lifecycle.clone();
    tasks.spawn(async move {
        collect_metrics(metrics_state, metrics, metrics_health, metrics_lifecycle).await
    });
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

impl AppState {
    fn from_env() -> Result<Self> {
        let clickhouse_base = clickhouse::Client::default()
            .with_url(required("AGENTX_CLICKHOUSE_URL")?)
            .with_database(
                env::var("AGENTX_CLICKHOUSE_DATABASE")
                    .unwrap_or_else(|_| "agentx_observability".into()),
            );
        let clickhouse_query = clickhouse_base
            .clone()
            .with_user(required("AGENTX_CLICKHOUSE_QUERY_USER")?)
            .with_password(required("AGENTX_CLICKHOUSE_QUERY_PASSWORD")?);
        let clickhouse_consumer = clickhouse_base
            .with_user(required("AGENTX_CLICKHOUSE_CONSUMER_USER")?)
            .with_password(required("AGENTX_CLICKHOUSE_CONSUMER_PASSWORD")?);
        let keys = serde_json::from_str::<HashMap<String, String>>(&required(
            "AGENTX_OBSERVABILITY_BFF_JWT_PUBLIC_KEYS_JSON",
        )?)?
        .into_iter()
        .map(|(kid, key)| (kid, key.into_bytes()))
        .collect();
        Ok(Self {
            clickhouse_query,
            clickhouse_consumer,
            redis: RedisSettings {
                url: SecretString::from(required("AGENTX_OBSERVABILITY_REDIS_URL")?),
                username: env::var("AGENTX_OBSERVABILITY_REDIS_USERNAME")
                    .ok()
                    .filter(|value| !value.is_empty()),
                password: env::var("AGENTX_OBSERVABILITY_REDIS_PASSWORD")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .map(SecretString::from),
            },
            keys: Arc::new(keys),
            issuer: env::var("AGENTX_OBSERVABILITY_JWT_ISSUER")
                .unwrap_or_else(|_| "agentx-control".into()),
            consumer: agentx_mysql_lease::LeaseOwner::for_process()?.0.to_string(),
            tenant_limits: Default::default(),
        })
    }
}

async fn ensure_group(state: &AppState) -> Result<()> {
    let mut redis = connect_redis(&state.redis).await?;
    let result: redis::RedisResult<redis::Value> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(TRACE_STREAM)
        .arg(TRACE_GROUP)
        .arg("0")
        .arg("MKSTREAM")
        .query_async(&mut redis)
        .await;
    if let Err(error) = result {
        if !error.to_string().contains("BUSYGROUP") {
            return Err(error.into());
        }
    }
    Ok(())
}

async fn consume_loop(
    state: AppState,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    while !lifecycle.is_draining() {
        let started = std::time::Instant::now();
        if let Err(error) = consume_batch(&state).await {
            tracing::warn!(%error, "Observability Trace Consumer batch failed");
            if let Err(group_error) = ensure_group(&state).await {
                tracing::warn!(%group_error, "Observability Consumer Group recovery failed");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        progress.processed_since(started).await;
    }
    Ok(())
}

async fn collect_metrics(
    state: AppState,
    metrics: agentx_service_kit::MetricsRegistry,
    health: agentx_service_kit::HealthRegistry,
    lifecycle: agentx_service_kit::ServiceLifecycle,
) -> Result<()> {
    while !lifecycle.is_draining() {
        match async {
            let mut redis = connect_redis(&state.redis).await?;
            let pending = redis
                .xpending::<_, _, StreamPendingReply>(TRACE_STREAM, TRACE_GROUP)
                .await?;
            Ok::<_, anyhow::Error>(pending)
        }
        .await
        {
            Ok(pending) => {
                health.set_status("runtime_redis", "ready").await;
                metrics
                    .set("agentx_queue_ready_items", pending.count() as f64)
                    .await;
                let oldest = match pending {
                    StreamPendingReply::Data(data) => data
                        .start_id
                        .split_once('-')
                        .and_then(|(milliseconds, _)| milliseconds.parse::<i128>().ok())
                        .map(|milliseconds| {
                            let now = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
                            now.saturating_sub(milliseconds) as f64 / 1_000.0
                        })
                        .unwrap_or_default(),
                    StreamPendingReply::Empty => 0.0,
                };
                metrics
                    .set("agentx_queue_oldest_ready_seconds", oldest)
                    .await;
            }
            Err(error) => {
                health.set_status("runtime_redis", "unavailable").await;
                tracing::warn!(%error, "Observability metrics refresh failed");
            }
        }
        match state
            .clickhouse_query
            .query("SELECT 1")
            .fetch_one::<u8>()
            .await
        {
            Ok(_) => health.set_status("clickhouse", "ready").await,
            Err(error) => {
                health.set_status("clickhouse", "unavailable").await;
                tracing::warn!(%error, "Observability ClickHouse readiness probe failed");
            }
        }
        tokio::select! {
            () = lifecycle.cancelled() => break,
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
    Ok(())
}

async fn consume_batch(state: &AppState) -> Result<()> {
    let mut redis = connect_redis(&state.redis).await?;
    let claimed: StreamAutoClaimReply = redis
        .xautoclaim_options(
            TRACE_STREAM,
            TRACE_GROUP,
            &state.consumer,
            30_000,
            "0-0",
            StreamAutoClaimOptions::default().count(100),
        )
        .await?;
    if !claimed.claimed.is_empty() {
        process_items(state, &mut redis, claimed.claimed).await?;
        return Ok(());
    }
    let options = StreamReadOptions::default()
        .group(TRACE_GROUP, &state.consumer)
        .count(100)
        .block(5_000);
    let reply: StreamReadReply = redis
        .xread_options(&[TRACE_STREAM], &[">"], &options)
        .await?;
    for key in reply.keys {
        process_items(state, &mut redis, key.ids).await?;
    }
    Ok(())
}

async fn process_items(
    state: &AppState,
    redis: &mut ConnectionManager,
    items: Vec<StreamId>,
) -> Result<()> {
    for item in items {
        let Some(value) = item.map.get("payload") else {
            let _: u64 = redis.xack(TRACE_STREAM, TRACE_GROUP, &[&item.id]).await?;
            continue;
        };
        let payload_text = String::from_redis_value(value)?;
        anyhow::ensure!(
            payload_text.len() <= 64 * 1024,
            "Trace payload exceeds 64 KiB"
        );
        let envelope: TraceEventEnvelopeV1 =
            serde_json::from_str(&payload_text).context("Trace Envelope is invalid")?;
        anyhow::ensure!(
            envelope.attributes.is_object() && envelope.attributes.to_string().len() <= 16 * 1024,
            "Trace attributes exceed the reviewed object budget"
        );
        let existing = existing_trace_hash(&state.clickhouse_consumer, envelope.event_id).await?;
        match existing {
            Some(hash) if hash != envelope.content_hash.as_str() => {
                let mut insert = state.clickhouse_consumer.insert("trace_ingest_conflicts")?;
                insert
                    .write(&TraceConflictRow {
                        event_id: envelope.event_id,
                        tenant_id: envelope.tenant_id,
                        execution_id: envelope.execution_id,
                        existing_hash: hash,
                        conflicting_hash: envelope.content_hash.to_string(),
                        stream_id: item.id.clone(),
                    })
                    .await?;
                insert.end().await?;
                write_health(state, "degraded", &item.id, "TRACE_EVENT_HASH_CONFLICT").await?;
            }
            Some(_) => {}
            None => {
                let mut insert = state.clickhouse_consumer.insert("workflow_trace_events")?;
                insert.write(&TraceRow::from(envelope)).await?;
                insert.end().await?;
                write_health(state, "ready", &item.id, "").await?;
            }
        }
        let _: u64 = redis.xack(TRACE_STREAM, TRACE_GROUP, &[&item.id]).await?;
    }
    Ok(())
}

async fn existing_trace_hash(
    clickhouse: &clickhouse::Client,
    event_id: Uuid,
) -> Result<Option<String>> {
    clickhouse
        .query("SELECT content_hash FROM workflow_trace_events WHERE event_id=? LIMIT 1")
        .bind(event_id)
        .fetch_optional::<String>()
        .await
        .map_err(Into::into)
}

#[derive(clickhouse::Row, Serialize)]
struct TraceConflictRow {
    #[serde(with = "clickhouse::serde::uuid")]
    event_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    tenant_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    execution_id: Uuid,
    existing_hash: String,
    conflicting_hash: String,
    stream_id: String,
}

#[derive(clickhouse::Row, Serialize)]
struct ConsumerHealthRow {
    consumer_id: String,
    state: String,
    last_stream_id: String,
    pending_count: u64,
    conflict_count: u64,
    last_error: String,
}

async fn write_health(state: &AppState, status: &str, stream_id: &str, error: &str) -> Result<()> {
    let mut insert = state
        .clickhouse_consumer
        .insert("observability_consumer_health")?;
    insert
        .write(&ConsumerHealthRow {
            consumer_id: state.consumer.clone(),
            state: status.into(),
            last_stream_id: stream_id.into(),
            pending_count: 0,
            conflict_count: u64::from(!error.is_empty()),
            last_error: error.into(),
        })
        .await?;
    insert.end().await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionTraceQuery {
    expected_watermark: u64,
    limit: Option<u32>,
    cursor: Option<String>,
    node_execution_id: Option<Uuid>,
}

async fn execution_trace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(execution_id): Path<Uuid>,
    Query(query): Query<ExecutionTraceQuery>,
) -> ApiResult<Response> {
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let request_hash = content_hash(&json!({
        "operation":"execution-trace","executionId":execution_id,
        "expectedWatermark":query.expected_watermark,"limit":limit,"cursor":query.cursor,
        "nodeExecutionId":query.node_execution_id
    }))
    .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let claims = authorize(&state, &headers, "observability.trace.read", &request_hash).await?;
    if !claims.execution_ids.contains(&execution_id) {
        return Err(ApiError::unauthorized());
    }
    let _permit = tenant_permit(&state, claims.tenant_id).await?;
    let query_id = Uuid::now_v7();
    let query_id_text = query_id.to_string();
    let cursor = query.cursor.as_deref().map(parse_span_cursor).transpose()?;
    let span_query = span_page_sql(cursor.is_some(), query.node_execution_id.is_some());
    let mut page_query = state
        .clickhouse_query
        .query(span_query)
        .with_option("query_id", &query_id_text)
        .with_option("max_execution_time", "5")
        .bind(claims.tenant_id)
        .bind(execution_id)
        .bind(claims.tenant_id);
    if let Some(node_execution_id) = query.node_execution_id {
        page_query = page_query.bind(node_execution_id);
    }
    if let Some((started_at, span_id)) = cursor {
        page_query = page_query.bind(timestamp_micros(started_at)?).bind(span_id);
    }
    let query_result = tokio::time::timeout(
        Duration::from_secs(5),
        page_query
            .bind(u64::from(limit) + 1)
            .fetch_all::<TraceSpanKeyRow>(),
    )
    .await;
    let mut span_keys = match query_result {
        Ok(result) => result.map_err(|error| ApiError::unavailable(error.to_string()))?,
        Err(_) => {
            cancel_query(&state.clickhouse_query, &query_id_text).await;
            return Err(ApiError::unavailable(format!(
                "ClickHouse query {query_id} timed out"
            )));
        }
    };
    let has_next = span_keys.len() > limit as usize;
    if has_next {
        span_keys.truncate(limit as usize);
    }
    let next = (has_next && !span_keys.is_empty()).then(|| {
        let key = &span_keys[span_keys.len() - 1];
        format!(
            "{}:{}",
            key.started_at.unix_timestamp_nanos() / 1_000,
            key.span_id
        )
    });
    let rows = if span_keys.is_empty() {
        Vec::new()
    } else {
        let span_ids = serde_json::to_string(
            &span_keys
                .iter()
                .map(|row| row.span_id.to_string())
                .collect::<Vec<_>>(),
        )
        .map_err(|error| ApiError::budget(error.to_string()))?;
        state
            .clickhouse_query
            .query("SELECT event_id,tenant_id,execution_id,execution_sequence,trace_id,span_id,parent_span_id,event_kind,span_kind,span_name,node_execution_id,attempt_id,agent_run_id,agent_iteration_id,runtime_call_id,sandbox_lease_id,wait_id,workflow_id,application_id,resource_type,resource_id,resource_version,event_type,status,error_code,error_message,duration_ms,input_tokens,output_tokens,cost_micros,attributes_json,content_ref,content_kind,content_preview_json,content_hash,occurred_at FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND has(JSONExtract(?, 'Array(String)'),toString(span_id)) AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?) ORDER BY execution_sequence,event_id")
            .with_option("query_id", format!("{query_id_text}-events"))
            .with_option("max_execution_time", "5")
            .bind(claims.tenant_id)
            .bind(execution_id)
            .bind(span_ids)
            .bind(claims.tenant_id)
            .fetch_all::<TraceRow>()
            .await
            .map_err(|error| ApiError::unavailable(error.to_string()))?
    };
    let watermark = state
        .clickhouse_query
        .query("SELECT max(execution_sequence) FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?)")
        .with_option("query_id", format!("{query_id_text}-watermark"))
        .with_option("max_execution_time", "5")
        .bind(claims.tenant_id)
        .bind(execution_id)
        .bind(claims.tenant_id)
        .fetch_one::<u64>()
        .await
        .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let degraded = has_conflicts(&state, claims.tenant_id, Some(execution_id)).await?;
    let total_spans_sql = if query.node_execution_id.is_some() {
        "SELECT uniqExact(span_id) FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?) AND node_execution_id=?"
    } else {
        "SELECT uniqExact(span_id) FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?)"
    };
    let mut total_spans_query = state
        .clickhouse_query
        .query(total_spans_sql)
        .with_option("query_id", format!("{query_id_text}-count"))
        .with_option("max_execution_time", "5")
        .bind(claims.tenant_id)
        .bind(execution_id)
        .bind(claims.tenant_id);
    if let Some(node_execution_id) = query.node_execution_id {
        total_spans_query = total_spans_query.bind(node_execution_id);
    }
    let total_spans = total_spans_query
        .fetch_one::<u64>()
        .await
        .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let events = rows
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>>>()
        .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let mut spans = aggregate_spans(&events);
    spans.sort_by_key(|span| (span.started_at, span.span_id));
    let complete = watermark >= query.expected_watermark;
    let trace = ExecutionTraceV1 {
        api_version: 1,
        execution_id,
        ingested_watermark: watermark,
        expected_watermark: query.expected_watermark,
        complete,
        degraded,
        warning_code: (!complete).then(|| "TRACE_DELAYED".into()),
        total_spans,
        next,
        spans,
    };
    Ok((StatusCode::OK, Json(trace)).into_response())
}

fn span_page_sql(has_cursor: bool, has_node_filter: bool) -> &'static str {
    match (has_cursor, has_node_filter) {
        (true, true) => {
            "SELECT span_id,if(countIf(event_kind='started')>0,minIf(occurred_at,event_kind='started'),min(occurred_at)) started_at FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?) AND node_execution_id=? GROUP BY span_id HAVING (started_at,span_id)>(fromUnixTimestamp64Micro(?),?) ORDER BY started_at,span_id LIMIT ?"
        }
        (true, false) => {
            "SELECT span_id,if(countIf(event_kind='started')>0,minIf(occurred_at,event_kind='started'),min(occurred_at)) started_at FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?) GROUP BY span_id HAVING (started_at,span_id)>(fromUnixTimestamp64Micro(?),?) ORDER BY started_at,span_id LIMIT ?"
        }
        (false, true) => {
            "SELECT span_id,if(countIf(event_kind='started')>0,minIf(occurred_at,event_kind='started'),min(occurred_at)) started_at FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?) AND node_execution_id=? GROUP BY span_id ORDER BY started_at,span_id LIMIT ?"
        }
        (false, false) => {
            "SELECT span_id,if(countIf(event_kind='started')>0,minIf(occurred_at,event_kind='started'),min(occurred_at)) started_at FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?) GROUP BY span_id ORDER BY started_at,span_id LIMIT ?"
        }
    }
}

fn aggregate_spans(events: &[TraceEventEnvelopeV1]) -> Vec<TraceSpanSummaryV1> {
    let mut grouped: HashMap<Uuid, Vec<&TraceEventEnvelopeV1>> = HashMap::new();
    for event in events {
        grouped.entry(event.span_id).or_default().push(event);
    }
    grouped
        .into_values()
        .filter_map(|mut events| {
            events.sort_by_key(|event| (event.occurred_at, event.execution_sequence));
            let first = events
                .iter()
                .find(|event| event.event_kind == TraceEventKindV1::Started)
                .copied()
                .unwrap_or(*events.first()?);
            let latest = *events.last()?;
            let finished = events
                .iter()
                .rev()
                .find(|event| event.event_kind == TraceEventKindV1::Finished)
                .copied();
            let ended_at = finished.map(|event| event.occurred_at.max(first.occurred_at));
            let duration_ms = finished.and_then(|event| {
                event.duration_ms.or_else(|| {
                    u64::try_from(
                        (event.occurred_at - first.occurred_at)
                            .whole_milliseconds()
                            .max(0),
                    )
                    .ok()
                })
            });
            let terminal = finished.unwrap_or(latest);
            let metric_event = events
                .iter()
                .rev()
                .find(|event| event.input_tokens.is_some() || event.output_tokens.is_some())
                .copied();
            let cost_event = events
                .iter()
                .rev()
                .find(|event| event.cost_micros > 0)
                .copied();
            Some(TraceSpanSummaryV1 {
                span_id: first.span_id,
                parent_span_id: first.parent_span_id,
                span_kind: first.span_kind,
                span_name: first.span_name.clone(),
                status: terminal.status.clone(),
                started_at: first.occurred_at,
                ended_at,
                duration_ms,
                node_execution_id: latest.node_execution_id,
                attempt_id: latest.attempt_id,
                agent_run_id: latest.agent_run_id,
                agent_iteration_id: latest.agent_iteration_id,
                runtime_call_id: latest.runtime_call_id,
                sandbox_lease_id: latest.sandbox_lease_id,
                wait_id: latest.wait_id,
                resource_type: latest.resource_type.clone(),
                resource_id: latest.resource_id,
                resource_version: latest.resource_version.clone(),
                input_tokens: metric_event.and_then(|event| event.input_tokens),
                output_tokens: metric_event.and_then(|event| event.output_tokens),
                cost_micros: cost_event
                    .map(|event| event.cost_micros)
                    .unwrap_or(latest.cost_micros),
                error_code: terminal
                    .error_code
                    .clone()
                    .or_else(|| latest.error_code.clone()),
                error_message: terminal
                    .error_message
                    .clone()
                    .or_else(|| latest.error_message.clone()),
                has_details: events.iter().any(|event| {
                    event.content_ref.is_some()
                        || event.content_preview.is_some()
                        || event
                            .attributes
                            .as_object()
                            .is_some_and(|value| !value.is_empty())
                }),
            })
        })
        .collect()
}

#[cfg(test)]
fn span_cursor(span: &TraceSpanSummaryV1) -> String {
    format!(
        "{}:{}",
        span.started_at.unix_timestamp_nanos() / 1_000,
        span.span_id
    )
}

fn parse_span_cursor(value: &str) -> ApiResult<(OffsetDateTime, Uuid)> {
    let (micros, span_id) = value.split_once(':').ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_TRACE_CURSOR",
            "Trace cursor is invalid",
        )
    })?;
    let micros = micros.parse::<i128>().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_TRACE_CURSOR",
            "Trace cursor is invalid",
        )
    })?;
    let started_at = OffsetDateTime::from_unix_timestamp_nanos(micros.saturating_mul(1_000))
        .map_err(|_| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "INVALID_TRACE_CURSOR",
                "Trace cursor is invalid",
            )
        })?;
    let span_id = Uuid::parse_str(span_id).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_TRACE_CURSOR",
            "Trace cursor is invalid",
        )
    })?;
    Ok((started_at, span_id))
}

async fn trace_span_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((execution_id, span_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<TraceSpanDetailV1>> {
    let request_hash = content_hash(&json!({
        "operation":"trace-span-detail","executionId":execution_id,"spanId":span_id
    }))
    .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let claims = authorize(&state, &headers, "observability.trace.read", &request_hash).await?;
    if !claims.execution_ids.contains(&execution_id) {
        return Err(ApiError::unauthorized());
    }
    let rows = state.clickhouse_query.query("SELECT event_id,tenant_id,execution_id,execution_sequence,trace_id,span_id,parent_span_id,event_kind,span_kind,span_name,node_execution_id,attempt_id,agent_run_id,agent_iteration_id,runtime_call_id,sandbox_lease_id,wait_id,workflow_id,application_id,resource_type,resource_id,resource_version,event_type,status,error_code,error_message,duration_ms,input_tokens,output_tokens,cost_micros,attributes_json,content_ref,content_kind,content_preview_json,content_hash,occurred_at FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND span_id=? AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?) ORDER BY execution_sequence,event_id")
        .with_option("max_execution_time", "5")
        .bind(claims.tenant_id)
        .bind(execution_id)
        .bind(span_id)
        .bind(claims.tenant_id)
        .fetch_all::<TraceRow>()
        .await
        .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let events = rows
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<TraceEventEnvelopeV1>>>()
        .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let span = aggregate_spans(&events).into_iter().next().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "TRACE_SPAN_NOT_FOUND",
            "Trace Span was not found",
        )
    })?;
    let contents = trace_contents(&events);
    Ok(Json(TraceSpanDetailV1 {
        api_version: 1,
        execution_id,
        span,
        contents,
        events,
    }))
}

fn trace_contents(events: &[TraceEventEnvelopeV1]) -> Vec<TraceContentV1> {
    events
        .iter()
        .filter_map(|event| {
            event.content_kind.map(|kind| TraceContentV1 {
                event_id: event.event_id,
                kind,
                preview: event.content_preview.clone(),
                content_ref: event.content_ref,
                occurred_at: event.occurred_at,
            })
        })
        .collect()
}

async fn search_traces(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TraceSearchRequestV1>,
) -> ApiResult<Json<TraceSearchPageV1>> {
    validate_range(request.from, request.to, request.limit)?;
    let request_hash = content_hash(&json!({"operation":"trace-search","request":request}))
        .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let claims = authorize(&state, &headers, "observability.trace.read", &request_hash).await?;
    if claims.tenant_id != request.tenant_id
        || request
            .execution_id
            .is_some_and(|id| !claims.execution_ids.contains(&id))
    {
        return Err(ApiError::unauthorized());
    }
    let _permit = tenant_permit(&state, claims.tenant_id).await?;
    let query_id = Uuid::now_v7();
    let event_types = serde_json::to_string(&request.event_types)
        .map_err(|error| ApiError::budget(error.to_string()))?;
    let statuses = serde_json::to_string(&request.statuses)
        .map_err(|error| ApiError::budget(error.to_string()))?;
    let query_id_text = query_id.to_string();
    let query_result = tokio::time::timeout(Duration::from_secs(5), state.clickhouse_query.query("SELECT event_id,tenant_id,execution_id,execution_sequence,trace_id,span_id,parent_span_id,event_kind,span_kind,span_name,node_execution_id,attempt_id,agent_run_id,agent_iteration_id,runtime_call_id,sandbox_lease_id,wait_id,workflow_id,application_id,resource_type,resource_id,resource_version,event_type,status,error_code,error_message,duration_ms,input_tokens,output_tokens,cost_micros,attributes_json,content_ref,content_kind,content_preview_json,content_hash,occurred_at FROM workflow_trace_events FINAL WHERE tenant_id=? AND occurred_at>=fromUnixTimestamp64Micro(?) AND occurred_at<=fromUnixTimestamp64Micro(?) AND (? IS NULL OR execution_id=?) AND (? IS NULL OR workflow_id=?) AND (JSONLength(?)=0 OR has(JSONExtract(?, 'Array(String)'),event_type)) AND (JSONLength(?)=0 OR has(JSONExtract(?, 'Array(String)'),status)) AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?) ORDER BY occurred_at,event_id LIMIT ?")
        .with_option("query_id", &query_id_text)
        .with_option("max_execution_time", "5")
        .bind(request.tenant_id).bind(timestamp_micros(request.from)?).bind(timestamp_micros(request.to)?).bind(request.execution_id).bind(request.execution_id).bind(&event_types).bind(&event_types).bind(&statuses).bind(&statuses).bind(request.tenant_id).bind(request.limit).fetch_all::<TraceRow>()).await;
    let rows = match query_result {
        Ok(result) => result.map_err(|error| ApiError::unavailable(error.to_string()))?,
        Err(_) => {
            cancel_query(&state.clickhouse_query, &query_id_text).await;
            return Err(ApiError::unavailable(format!(
                "ClickHouse query {query_id} timed out"
            )));
        }
    };
    let degraded = has_conflicts(&state, claims.tenant_id, request.execution_id).await?;
    let events = rows
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>>>()
        .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let next = if (events.len() as u32) >= request.limit {
        events.last().map(|event: &TraceEventEnvelopeV1| {
            format!("{}:{}", event.event_id, event.occurred_at.unix_timestamp())
        })
    } else {
        None
    };
    Ok(Json(TraceSearchPageV1 {
        api_version: 1,
        query_id,
        events,
        next,
        degraded,
    }))
}

async fn query_aggregates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ObservabilityAggregateRequestV1>,
) -> ApiResult<Json<ObservabilityAggregatePageV1>> {
    validate_range(request.from, request.to, request.limit)?;
    if request.metrics.is_empty()
        || request.dimensions.len() > 3
        || !request.filters.as_object().is_some_and(|filters| {
            filters.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "workflowId"
                        | "applicationId"
                        | "status"
                        | "errorCode"
                        | "provider"
                        | "resourceType"
                )
            })
        })
    {
        return Err(ApiError::budget(
            "Aggregate dimensions, metrics or filters are invalid",
        ));
    }
    let request_hash = content_hash(&json!({"operation":"aggregate-query","request":request}))
        .map_err(|error| ApiError::unavailable(error.to_string()))?;
    let claims = authorize(
        &state,
        &headers,
        "observability.aggregate.read",
        &request_hash,
    )
    .await?;
    if claims.tenant_id != request.tenant_id {
        return Err(ApiError::unauthorized());
    }
    let _permit = tenant_permit(&state, claims.tenant_id).await?;
    let query_id = Uuid::now_v7();
    let sql = aggregate_query_sql(&request)?;
    let query_id_text = query_id.to_string();
    let query_result = tokio::time::timeout(
        Duration::from_secs(5),
        state
            .clickhouse_query
            .query(&sql)
            .with_option("query_id", &query_id_text)
            .with_option("max_execution_time", "5")
            .bind(request.tenant_id)
            .bind(timestamp_micros(request.from)?)
            .bind(timestamp_micros(request.to)?)
            .bind(request.tenant_id)
            .bind(request.limit)
            .fetch_all::<AggregateRow>(),
    )
    .await;
    let rows = match query_result {
        Ok(result) => result.map_err(|error| ApiError::unavailable(error.to_string()))?,
        Err(_) => {
            cancel_query(&state.clickhouse_query, &query_id_text).await;
            return Err(ApiError::unavailable(format!(
                "ClickHouse query {query_id} timed out"
            )));
        }
    };
    let degraded = has_conflicts(&state, claims.tenant_id, None).await?;
    Ok(Json(ObservabilityAggregatePageV1 {
        api_version: 1,
        query_id,
        rows: rows
            .into_iter()
            .map(|row| ObservabilityAggregateRowV1 {
                dimensions: serde_json::from_str(&row.dimensions).unwrap_or_else(|_| json!({})),
                metrics: serde_json::from_str(&row.metrics).unwrap_or_else(|_| json!({})),
            })
            .collect(),
        degraded,
    }))
}

#[derive(clickhouse::Row, Deserialize)]
struct AggregateRow {
    dimensions: String,
    metrics: String,
}

async fn has_conflicts(
    state: &AppState,
    tenant_id: Uuid,
    execution_id: Option<Uuid>,
) -> ApiResult<bool> {
    let count = if let Some(execution_id) = execution_id {
        state
            .clickhouse_query
            .query(
                "SELECT count() FROM trace_ingest_conflicts WHERE tenant_id=? AND execution_id=?",
            )
            .with_option("max_execution_time", "5")
            .bind(tenant_id)
            .bind(execution_id)
            .fetch_one::<u64>()
            .await
    } else {
        state
            .clickhouse_query
            .query("SELECT count() FROM trace_ingest_conflicts WHERE tenant_id=?")
            .with_option("max_execution_time", "5")
            .bind(tenant_id)
            .fetch_one::<u64>()
            .await
    }
    .map_err(|error| ApiError::unavailable(error.to_string()))?;
    Ok(count > 0)
}

async fn cancel_query(client: &clickhouse::Client, query_id: &str) {
    if let Err(error) = client
        .query("KILL QUERY WHERE query_id=? SYNC")
        .bind(query_id)
        .execute()
        .await
    {
        tracing::warn!(%error, %query_id, "ClickHouse query cancellation failed");
    }
}

fn dimension_sql(value: &ObservabilityDimensionV1) -> (&'static str, &'static str) {
    match value {
        ObservabilityDimensionV1::Hour => ("hour", "toStartOfHour(occurred_at)"),
        ObservabilityDimensionV1::Day => ("day", "toDate(occurred_at)"),
        ObservabilityDimensionV1::Workflow => ("workflow", "workflow_id"),
        ObservabilityDimensionV1::Application => ("application", "application_id"),
        ObservabilityDimensionV1::Status => ("status", "status"),
        ObservabilityDimensionV1::ErrorCode => ("errorCode", "ifNull(error_code,'')"),
        ObservabilityDimensionV1::Provider => {
            ("provider", "JSONExtractString(attributes_json,'provider')")
        }
        ObservabilityDimensionV1::ResourceType => ("resourceType", "ifNull(resource_type,'')"),
        ObservabilityDimensionV1::SpanName => ("spanName", "span_name"),
    }
}
fn metric_sql(value: &ObservabilityMetricV1) -> (&'static str, &'static str) {
    match value {
        ObservabilityMetricV1::Count => ("count", "count()"),
        ObservabilityMetricV1::DurationMillis => ("durationMillis", "sum(ifNull(duration_ms,0))"),
        ObservabilityMetricV1::CostMicros => ("costMicros", "sum(cost_micros)"),
        ObservabilityMetricV1::InputTokens => ("inputTokens", "sum(ifNull(input_tokens,0))"),
        ObservabilityMetricV1::OutputTokens => ("outputTokens", "sum(ifNull(output_tokens,0))"),
        ObservabilityMetricV1::ErrorRate => ("errorRate", "avg(status='failed')"),
        ObservabilityMetricV1::DurationP50 => ("durationP50", "quantile(0.5)(duration_ms)"),
        ObservabilityMetricV1::DurationP95 => ("durationP95", "quantile(0.95)(duration_ms)"),
    }
}

fn aggregate_query_sql(request: &ObservabilityAggregateRequestV1) -> ApiResult<String> {
    let dimensions = request
        .dimensions
        .iter()
        .map(dimension_sql)
        .collect::<Vec<_>>();
    let metrics = request.metrics.iter().map(metric_sql).collect::<Vec<_>>();
    let dimension_json = if dimensions.is_empty() {
        "'{}' dimensions".into()
    } else {
        format!(
            "toJSONString(map({})) dimensions",
            dimensions
                .iter()
                .map(|(key, value)| format!("'{key}',toString({value})"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let metric_json = format!(
        "toJSONString(map({})) metrics",
        metrics
            .iter()
            .map(|(key, value)| format!("'{key}',toString({value})"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let group = if dimensions.is_empty() {
        String::new()
    } else {
        format!(
            " GROUP BY {}",
            dimensions
                .iter()
                .map(|(_, value)| *value)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let filters = aggregate_filter_sql(&request.filters)?;
    let mut sql = String::from("SELECT ");
    sql.push_str(&dimension_json);
    sql.push(',');
    sql.push_str(&metric_json);
    sql.push_str(" FROM workflow_trace_events FINAL WHERE tenant_id=? AND occurred_at>=fromUnixTimestamp64Micro(?) AND occurred_at<=fromUnixTimestamp64Micro(?) AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?)");
    sql.push_str(&filters);
    sql.push_str(&group);
    sql.push_str(" LIMIT ?");
    Ok(sql)
}

fn timestamp_micros(value: OffsetDateTime) -> ApiResult<i64> {
    i64::try_from(value.unix_timestamp_nanos() / 1_000)
        .map_err(|_| ApiError::budget("Timestamp is outside the ClickHouse DateTime64 range"))
}

fn aggregate_filter_sql(filters: &serde_json::Value) -> ApiResult<String> {
    let Some(filters) = filters.as_object() else {
        return Err(ApiError::budget("Aggregate filters must be an object"));
    };
    let mut sql = String::new();
    for (key, value) in filters {
        let raw = value
            .as_str()
            .ok_or_else(|| ApiError::budget("Aggregate filter values must be strings"))?;
        match key.as_str() {
            "workflowId" | "applicationId" => {
                let id = Uuid::parse_str(raw)
                    .map_err(|_| ApiError::budget("Aggregate UUID filter is invalid"))?;
                let column = if key == "workflowId" {
                    "workflow_id"
                } else {
                    "application_id"
                };
                sql.push_str(&format!(" AND {column}=toUUID('{id}')"));
            }
            "status" | "errorCode" | "provider" | "resourceType" => {
                if raw.is_empty()
                    || raw.len() > 128
                    || !raw.bytes().all(|value| {
                        value.is_ascii_alphanumeric()
                            || matches!(value, b'_' | b'-' | b'.' | b':' | b'/')
                    })
                {
                    return Err(ApiError::budget("Aggregate string filter is invalid"));
                }
                let expression = match key.as_str() {
                    "status" => "status",
                    "errorCode" => "ifNull(error_code,'')",
                    "provider" => "JSONExtractString(attributes_json,'provider')",
                    _ => "ifNull(resource_type,'')",
                };
                sql.push_str(&format!(" AND {expression}='{raw}'"));
            }
            _ => return Err(ApiError::budget("Aggregate filter is not supported")),
        }
    }
    Ok(sql)
}

async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    scope: &str,
    expected_hash: &ContentHash,
) -> ApiResult<DelegationClaimsV1> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let claims = verify_observability_token(token, &state.keys, &state.issuer, scope)
        .map_err(|_| ApiError::unauthorized())?;
    if &claims.request_hash != expected_hash {
        return Err(ApiError::unauthorized());
    }
    let header_hash = headers
        .get("x-agentx-request-hash")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(ApiError::unauthorized)?;
    if header_hash != expected_hash.as_str() {
        return Err(ApiError::unauthorized());
    }
    let mut redis = connect_redis(&state.redis)
        .await
        .map_err(|_| ApiError::unavailable("Delegation replay store is unavailable"))?;
    let accepted: Option<String> = redis::cmd("SET")
        .arg(format!("agentx:v2:observability:jti:{}", claims.jti))
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(60)
        .query_async(&mut redis)
        .await
        .map_err(|_| ApiError::unavailable("Delegation replay store is unavailable"))?;
    if accepted.is_none() {
        return Err(ApiError::unauthorized());
    }
    Ok(claims)
}

fn verify_observability_token(
    token: &str,
    keys: &HashMap<String, Vec<u8>>,
    issuer: &str,
    scope: &str,
) -> std::result::Result<DelegationClaimsV1, agentx_runtime_contracts::ServiceJwtError> {
    let header = jsonwebtoken::decode_header(token)
        .map_err(|_| agentx_runtime_contracts::ServiceJwtError::Invalid)?;
    let kid = header
        .kid
        .ok_or(agentx_runtime_contracts::ServiceJwtError::MissingKeyId)?;
    let key = keys
        .get(&kid)
        .ok_or(agentx_runtime_contracts::ServiceJwtError::UnknownKeyId)?;
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&["agentx-observability-query"]);
    validation.required_spec_claims = HashSet::from([
        "exp".into(),
        "iat".into(),
        "iss".into(),
        "aud".into(),
        "sub".into(),
    ]);
    let claims = jsonwebtoken::decode::<DelegationClaimsV1>(
        token,
        &jsonwebtoken::DecodingKey::from_rsa_pem(key)
            .map_err(|_| agentx_runtime_contracts::ServiceJwtError::Invalid)?,
        &validation,
    )
    .map_err(|_| agentx_runtime_contracts::ServiceJwtError::Invalid)?
    .claims;
    if claims.exp - claims.iat > 60 {
        return Err(agentx_runtime_contracts::ServiceJwtError::LifetimeExceeded);
    }
    if !claims.scope.contains(scope) {
        return Err(agentx_runtime_contracts::ServiceJwtError::ScopeNotAllowed);
    }
    Ok(claims)
}

async fn tenant_permit(state: &AppState, tenant: Uuid) -> ApiResult<OwnedSemaphorePermit> {
    let semaphore = {
        let mut limits = state.tenant_limits.lock().await;
        limits
            .entry(tenant)
            .or_insert_with(|| Arc::new(Semaphore::new(4)))
            .clone()
    };
    semaphore
        .try_acquire_owned()
        .map_err(|_| ApiError::budget("Tenant already has four active Observability queries"))
}

fn validate_range(from: OffsetDateTime, to: OffsetDateTime, limit: u32) -> ApiResult<()> {
    if limit == 0 || limit > 1000 || to <= from || to - from > time::Duration::days(30) {
        return Err(ApiError::budget(
            "Query range or row limit exceeds the Observability budget",
        ));
    }
    Ok(())
}

async fn connect_redis(settings: &RedisSettings) -> Result<ConnectionManager> {
    let mut url = url::Url::parse(settings.url.expose_secret())?;
    if let Some(username) = &settings.username {
        url.set_username(username).map_err(|()| {
            anyhow::anyhow!("Observability Redis URL cannot carry the configured username")
        })?;
    }
    if let Some(password) = &settings.password {
        url.set_password(Some(password.expose_secret()))
            .map_err(|()| {
                anyhow::anyhow!("Observability Redis URL cannot carry the configured password")
            })?;
    }
    let client = redis::Client::open(url.as_str())?;
    Ok(tokio::time::timeout(Duration::from_secs(5), client.get_connection_manager()).await??)
}

fn roles() -> Result<BTreeSet<String>> {
    let values = env::var("AGENTX_OBSERVABILITY_ROLES")
        .unwrap_or_else(|_| "trace-consumer,query".into())
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    anyhow::ensure!(!values.is_empty(), "AGENTX_OBSERVABILITY_ROLES is empty");
    anyhow::ensure!(
        values
            .iter()
            .all(|value| matches!(value.as_str(), "trace-consumer" | "query")),
        "AGENTX_OBSERVABILITY_ROLES contains an unsupported role"
    );
    Ok(values)
}
fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("required environment variable {name} is missing"))
}

#[cfg(test)]
mod observability_tests;
