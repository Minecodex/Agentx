use std::{
    collections::{BTreeMap, VecDeque},
    env,
    net::SocketAddr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use agentx_api_types::{DependencyHealth, HealthResponse};
use anyhow::{Context, Result};
use axum::{
    Extension, Json, Router,
    extract::Request,
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use tokio::sync::{RwLock, watch};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
pub struct RequestId(pub Uuid);

/// Installs the explicit rustls CryptoProvider for the process. Feature
/// unification across workspace and test dependencies can leave both ring
/// and aws-lc-rs enabled, which disables rustls's automatic selection and
/// panics on the first TLS connection in production (TLS-everywhere) mode.
/// Services call this at the very top of `main`.
pub fn install_tls_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

pub fn reqwest_client_builder_with_ca(ca_path_env: &str) -> Result<reqwest::ClientBuilder> {
    let mut builder = reqwest::Client::builder();
    if let Some(path) = env::var_os(ca_path_env).filter(|value| !value.is_empty()) {
        let pem = std::fs::read(&path)
            .with_context(|| format!("failed reading CA bundle configured by {ca_path_env}"))?;
        let certificate = reqwest::Certificate::from_pem(&pem)
            .with_context(|| format!("invalid PEM CA bundle in {ca_path_env}"))?;
        builder = builder.add_root_certificate(certificate);
    }
    Ok(builder)
}

#[derive(Clone, Default)]
pub struct HealthRegistry {
    dependencies: Arc<RwLock<BTreeMap<String, DependencyHealth>>>,
}

pub const METRIC_NAMES: [&str; 20] = [
    "agentx_queue_ready_items",
    "agentx_queue_oldest_ready_seconds",
    "agentx_active_leases",
    "agentx_role_processing_seconds",
    "agentx_http_inflight_requests",
    "agentx_sse_connections",
    "agentx_drain_inflight",
    "agentx_mysql_pool_busy_connections",
    "agentx_mysql_pool_wait_p95_ms",
    "agentx_mysql_pool_wait_samples",
    "agentx_provider_pool_utilization",
    "agentx_egress_active_tunnels",
    "agentx_egress_allowed_total",
    "agentx_egress_denied_total",
    "agentx_egress_bytes_total",
    // plan7 P7-D3: admission and provider health surfaces for capacity runs.
    "agentx_admission_rejections_total",
    "agentx_provider_circuit_open",
    "agentx_redis_task_unread_items",
    "agentx_redis_task_pending_items",
    "agentx_redis_admission_available",
];
pub const DRAIN_TIMEOUT_SECONDS: u64 = 45;
pub const ROLE_WATCHDOG_TIMEOUT_SECONDS: u64 = 90;

#[derive(Clone, Default)]
pub struct MetricsRegistry {
    values: Arc<RwLock<BTreeMap<&'static str, f64>>>,
    mysql_pool_wait_samples: Arc<Mutex<VecDeque<f64>>>,
}

impl MetricsRegistry {
    pub async fn initialize(&self) {
        let mut values = self.values.write().await;
        for name in METRIC_NAMES {
            values.entry(name).or_insert(0.0);
        }
    }

    pub async fn set(&self, name: &'static str, value: f64) {
        debug_assert!(METRIC_NAMES.contains(&name));
        self.values.write().await.insert(name, value.max(0.0));
    }

    pub async fn add(&self, name: &'static str, delta: f64) {
        debug_assert!(METRIC_NAMES.contains(&name));
        let mut values = self.values.write().await;
        let value = values.entry(name).or_insert(0.0);
        *value = (*value + delta).max(0.0);
    }

    /// Five-second pool probes retain a bounded distribution of actual
    /// acquire waits. The sample count is exposed so missing data is visible.
    pub async fn observe_mysql_pool_wait(&self, duration: Duration) {
        let (p95, count) = {
            let mut samples = self
                .mysql_pool_wait_samples
                .lock()
                .expect("pool wait samples lock");
            samples.push_back(duration.as_secs_f64() * 1000.0);
            if samples.len() > 2048 {
                samples.pop_front();
            }
            let mut sorted = samples.iter().copied().collect::<Vec<_>>();
            sorted.sort_by(f64::total_cmp);
            (
                sorted[(sorted.len() * 95).div_ceil(100).saturating_sub(1)],
                sorted.len(),
            )
        };
        self.set("agentx_mysql_pool_wait_p95_ms", p95).await;
        self.set("agentx_mysql_pool_wait_samples", count as f64)
            .await;
    }

    pub async fn track(&self, name: &'static str) -> MetricGuard {
        self.add(name, 1.0).await;
        MetricGuard {
            metrics: self.clone(),
            name,
        }
    }

    async fn render(&self) -> String {
        let values = self.values.read().await;
        let mut output = String::new();
        for name in METRIC_NAMES {
            let value = values.get(name).copied().unwrap_or_default();
            output.push_str("# TYPE ");
            output.push_str(name);
            output.push_str(" gauge\n");
            output.push_str(name);
            output.push(' ');
            output.push_str(&value.to_string());
            output.push('\n');
        }
        output
    }
}

pub struct MetricGuard {
    metrics: MetricsRegistry,
    name: &'static str,
}

impl Drop for MetricGuard {
    fn drop(&mut self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let metrics = self.metrics.clone();
            let name = self.name;
            runtime.spawn(async move {
                metrics.add(name, -1.0).await;
            });
        }
    }
}

#[derive(Clone)]
pub struct RoleProgressWatchdog {
    role_health_name: Arc<str>,
    last_progress: Arc<Mutex<Instant>>,
    metrics: MetricsRegistry,
}

impl RoleProgressWatchdog {
    pub async fn start(
        role: impl Into<String>,
        max_stall: Duration,
        registry: HealthRegistry,
        lifecycle: ServiceLifecycle,
        metrics: MetricsRegistry,
    ) -> Self {
        let role_health_name: Arc<str> = format!("role:{}", role.into()).into();
        registry.register(role_health_name.to_string(), true).await;
        registry.set_status(&role_health_name, "ready").await;
        let watchdog = Self {
            role_health_name,
            last_progress: Arc::new(Mutex::new(Instant::now())),
            metrics,
        };
        let monitor = watchdog.clone();
        tokio::spawn(async move {
            let interval = (max_stall / 4)
                .min(Duration::from_secs(5))
                .max(Duration::from_millis(10));
            while !lifecycle.is_draining() {
                let stalled = monitor.elapsed() > max_stall;
                registry
                    .set_status(
                        &monitor.role_health_name,
                        if stalled { "stalled" } else { "ready" },
                    )
                    .await;
                tokio::select! {
                    () = lifecycle.cancelled() => break,
                    () = tokio::time::sleep(interval) => {}
                }
            }
        });
        watchdog
    }

    pub fn progress(&self) {
        *self
            .last_progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }

    pub async fn processed_since(&self, started: Instant) {
        self.progress();
        self.metrics
            .set(
                "agentx_role_processing_seconds",
                started.elapsed().as_secs_f64(),
            )
            .await;
    }

    fn elapsed(&self) -> Duration {
        self.last_progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed()
    }
}

struct LifecycleInner {
    draining: AtomicBool,
    in_flight: AtomicU64,
    drain_tx: watch::Sender<bool>,
    drain_started: OnceLock<Instant>,
}

#[derive(Clone)]
pub struct ServiceLifecycle {
    inner: Arc<LifecycleInner>,
}

impl Default for ServiceLifecycle {
    fn default() -> Self {
        let (drain_tx, _) = watch::channel(false);
        Self {
            inner: Arc::new(LifecycleInner {
                draining: AtomicBool::new(false),
                in_flight: AtomicU64::new(0),
                drain_tx,
                drain_started: OnceLock::new(),
            }),
        }
    }
}

impl ServiceLifecycle {
    pub fn begin_drain(&self) {
        if !self.inner.draining.swap(true, Ordering::SeqCst) {
            let _ = self.inner.drain_started.set(Instant::now());
            let _ = self.inner.drain_tx.send(true);
        }
    }

    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.inner.draining.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn in_flight(&self) -> u64 {
        self.inner.in_flight.load(Ordering::Relaxed)
    }

    pub async fn cancelled(&self) {
        if self.is_draining() {
            return;
        }
        let mut receiver = self.inner.drain_tx.subscribe();
        while !*receiver.borrow() {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }

    pub async fn drain_deadline(&self) {
        self.cancelled().await;
        let remaining = self
            .inner
            .drain_started
            .get()
            .map_or(Duration::ZERO, |started| {
                Duration::from_secs(DRAIN_TIMEOUT_SECONDS).saturating_sub(started.elapsed())
            });
        tokio::time::sleep(remaining).await;
    }

    fn track(&self) -> InFlightGuard {
        self.inner.in_flight.fetch_add(1, Ordering::Relaxed);
        InFlightGuard(self.clone())
    }
}

struct InFlightGuard(ServiceLifecycle);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.inner.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

impl HealthRegistry {
    pub async fn register(&self, name: impl Into<String>, required: bool) {
        let name = name.into();
        self.dependencies.write().await.insert(
            name.clone(),
            DependencyHealth {
                name,
                status: "unknown".to_owned(),
                required,
            },
        );
    }

    pub async fn set_status(&self, name: &str, status: impl Into<String>) {
        if let Some(dependency) = self.dependencies.write().await.get_mut(name) {
            dependency.status = status.into();
        }
    }

    async fn snapshot(&self) -> Vec<DependencyHealth> {
        self.dependencies.read().await.values().cloned().collect()
    }

    pub async fn overall_status(&self) -> &'static str {
        let dependencies = self.snapshot().await;
        if dependencies
            .iter()
            .any(|dependency| dependency.required && dependency.status != "ready")
        {
            "unavailable"
        } else if dependencies
            .iter()
            .any(|dependency| !dependency.required && dependency.status != "ready")
        {
            "degraded"
        } else {
            "ready"
        }
    }

    async fn role_schedulers_live(&self) -> bool {
        self.snapshot()
            .await
            .iter()
            .all(|dependency| !dependency.name.starts_with("role:") || dependency.status == "ready")
    }
}

#[derive(Clone)]
struct HealthState {
    service_name: &'static str,
    registry: HealthRegistry,
    lifecycle: ServiceLifecycle,
}

pub async fn run_service(service_name: &'static str) -> Result<()> {
    serve(service_name, Router::new(), HealthRegistry::default()).await
}

pub async fn serve(
    service_name: &'static str,
    router: Router,
    registry: HealthRegistry,
) -> Result<()> {
    serve_with_lifecycle(
        service_name,
        router,
        registry,
        ServiceLifecycle::default(),
        MetricsRegistry::default(),
    )
    .await
}

pub async fn serve_with_lifecycle(
    service_name: &'static str,
    router: Router,
    registry: HealthRegistry,
    lifecycle: ServiceLifecycle,
    metrics: MetricsRegistry,
) -> Result<()> {
    init_tracing();
    metrics.initialize().await;

    let bind_address = env::var("AGENTX_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
        .parse::<SocketAddr>()
        .context("AGENTX_BIND_ADDR must be a valid socket address")?;

    let health_state = HealthState {
        service_name,
        registry,
        lifecycle: lifecycle.clone(),
    };
    let router = router
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .layer(middleware::from_fn(track_in_flight))
        .layer(middleware::from_fn(reject_writes_while_draining))
        .layer(middleware::from_fn(request_id))
        .layer(TraceLayer::new_for_http())
        // Extension layers must wrap the middleware above so Drain and
        // in-flight accounting can observe the shared process state.
        .layer(Extension(metrics.clone()))
        .layer(Extension(lifecycle.clone()))
        .layer(Extension(health_state.clone()));

    let listener = tokio::net::TcpListener::bind(bind_address)
        .await
        .with_context(|| format!("failed to bind {bind_address}"))?;

    info!(service = service_name, %bind_address, "service started");

    let metrics_address = env::var("AGENTX_METRICS_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:9092".to_owned())
        .parse::<SocketAddr>()
        .context("AGENTX_METRICS_BIND_ADDR must be a valid socket address")?;
    let metrics_listener = tokio::net::TcpListener::bind(metrics_address)
        .await
        .with_context(|| format!("failed to bind metrics endpoint {metrics_address}"))?;
    let metrics_router = Router::new()
        .route("/metrics", get(metrics_endpoint))
        .layer(Extension(metrics.clone()));

    let admin_address = env::var("AGENTX_ADMIN_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:9091".to_owned())
        .parse::<SocketAddr>()
        .context("AGENTX_ADMIN_BIND_ADDR must be a valid socket address")?;
    let admin_listener = tokio::net::TcpListener::bind(admin_address)
        .await
        .with_context(|| format!("failed to bind admin endpoint {admin_address}"))?;
    let admin_router = Router::new()
        .route("/health/drain", post(drain))
        .layer(Extension(metrics.clone()))
        .layer(Extension(lifecycle.clone()))
        .layer(Extension(health_state.clone()));

    let signal_lifecycle = lifecycle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        signal_lifecycle.begin_drain();
    });

    let http_shutdown = lifecycle.clone();
    let metrics_shutdown = lifecycle.clone();
    let admin_shutdown = lifecycle.clone();
    let mut http = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { http_shutdown.drain_deadline().await })
            .await
    });
    let mut metrics_server = tokio::spawn(async move {
        axum::serve(metrics_listener, metrics_router)
            .with_graceful_shutdown(async move { metrics_shutdown.drain_deadline().await })
            .await
    });
    let mut admin_server = tokio::spawn(async move {
        axum::serve(admin_listener, admin_router)
            .with_graceful_shutdown(async move { admin_shutdown.drain_deadline().await })
            .await
    });

    tokio::select! {
        result = &mut http => {
            result.context("HTTP service task failed")??;
            lifecycle.begin_drain();
        }
        result = &mut metrics_server => {
            result.context("metrics service task failed")??;
            lifecycle.begin_drain();
        }
        result = &mut admin_server => {
            result.context("admin service task failed")??;
            lifecycle.begin_drain();
        }
        () = lifecycle.cancelled() => {}
    }

    let finish = async {
        let (http_result, metrics_result, admin_result) =
            tokio::join!(&mut http, &mut metrics_server, &mut admin_server);
        http_result.context("HTTP service task failed")??;
        metrics_result.context("metrics service task failed")??;
        admin_result.context("admin service task failed")??;
        Ok::<(), anyhow::Error>(())
    };
    match tokio::time::timeout(Duration::from_secs(DRAIN_TIMEOUT_SECONDS + 5), finish).await {
        Ok(result) => result,
        Err(_) => {
            http.abort();
            metrics_server.abort();
            admin_server.abort();
            Ok(())
        }
    }
}

async fn request_id(mut request: Request, next: Next) -> Response {
    let request_id = Uuid::now_v7();
    request.extensions_mut().insert(RequestId(request_id));
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .entry(HeaderName::from_static("x-request-id"))
        .or_insert_with(|| {
            HeaderValue::from_str(&request_id.to_string())
                .expect("UUID is always a valid header value")
        });
    response
}

async fn reject_writes_while_draining(request: Request, next: Next) -> Response {
    let draining = request
        .extensions()
        .get::<ServiceLifecycle>()
        .is_some_and(ServiceLifecycle::is_draining);
    let health = request.uri().path().starts_with("/health/");
    if draining && !health && !matches!(*request.method(), Method::GET | Method::HEAD) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "5")],
            Json(json!({
                "code": "SERVICE_DRAINING",
                "message": "The service is draining and is not accepting new writes"
            })),
        )
            .into_response();
    }
    next.run(request).await
}

async fn track_in_flight(request: Request, next: Next) -> Response {
    if request.uri().path().starts_with("/health/") {
        return next.run(request).await;
    }
    let lifecycle = request.extensions().get::<ServiceLifecycle>().cloned();
    let metrics = request.extensions().get::<MetricsRegistry>().cloned();
    let _guard = lifecycle.as_ref().map(ServiceLifecycle::track);
    if let Some(metrics) = &metrics {
        metrics.add("agentx_http_inflight_requests", 1.0).await;
        if lifecycle
            .as_ref()
            .is_some_and(ServiceLifecycle::is_draining)
        {
            metrics
                .set(
                    "agentx_drain_inflight",
                    lifecycle.as_ref().map_or(0, ServiceLifecycle::in_flight) as f64,
                )
                .await;
        }
    }
    let response = next.run(request).await;
    if let Some(metrics) = &metrics {
        metrics.add("agentx_http_inflight_requests", -1.0).await;
        if lifecycle
            .as_ref()
            .is_some_and(ServiceLifecycle::is_draining)
        {
            metrics
                .set(
                    "agentx_drain_inflight",
                    lifecycle.as_ref().map_or(0, ServiceLifecycle::in_flight) as f64,
                )
                .await;
        }
    }
    response
}

async fn drain(
    Extension(state): Extension<HealthState>,
    Extension(metrics): Extension<MetricsRegistry>,
) -> StatusCode {
    state.lifecycle.begin_drain();
    metrics
        .set("agentx_drain_inflight", state.lifecycle.in_flight() as f64)
        .await;
    StatusCode::NO_CONTENT
}

async fn metrics_endpoint(Extension(metrics): Extension<MetricsRegistry>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        metrics.render().await,
    )
}

async fn live(Extension(state): Extension<HealthState>) -> (StatusCode, Json<HealthResponse>) {
    let live = state.registry.role_schedulers_live().await;
    (
        if live {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(HealthResponse {
            service: state.service_name.to_owned(),
            status: if live { "live" } else { "role_stalled" }.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            dependencies: Vec::new(),
        }),
    )
}

async fn ready(Extension(state): Extension<HealthState>) -> (StatusCode, Json<HealthResponse>) {
    let dependencies = state.registry.snapshot().await;
    let ready = !state.lifecycle.is_draining()
        && dependencies
            .iter()
            .all(|dependency| !dependency.required || dependency.status == "ready");
    let degraded = dependencies
        .iter()
        .any(|dependency| !dependency.required && dependency.status != "ready");
    let status = if ready {
        if degraded { "degraded" } else { "ready" }
    } else {
        "not_ready"
    };

    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(HealthResponse {
            service: state.service_name.to_owned(),
            status: status.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            dependencies,
        }),
    )
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .json()
        .try_init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HealthRegistry, HealthState, MetricsRegistry, RoleProgressWatchdog, ServiceLifecycle, live,
        ready, reject_writes_while_draining,
    };
    use axum::{
        Extension, Router,
        body::Body,
        http::{Method, Request, StatusCode},
        middleware,
        routing::post,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn readiness_recovers_and_optional_failures_only_degrade() {
        let registry = HealthRegistry::default();
        registry.register("mysql", true).await;
        registry.register("redis", false).await;
        let state = HealthState {
            service_name: "test-service",
            registry: registry.clone(),
            lifecycle: ServiceLifecycle::default(),
        };

        registry.set_status("mysql", "unavailable").await;
        registry.set_status("redis", "degraded").await;
        let (status, body) = ready(Extension(state.clone())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body.status, "not_ready");

        registry.set_status("mysql", "ready").await;
        let (status, body) = ready(Extension(state.clone())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.status, "degraded");

        registry.set_status("redis", "ready").await;
        let (status, body) = ready(Extension(state)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.status, "ready");
    }

    #[tokio::test]
    async fn drain_changes_readiness_without_changing_liveness() {
        let lifecycle = ServiceLifecycle::default();
        let state = HealthState {
            service_name: "test-service",
            registry: HealthRegistry::default(),
            lifecycle: lifecycle.clone(),
        };
        lifecycle.begin_drain();
        let (status, body) = ready(Extension(state)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body.status, "not_ready");
    }

    #[tokio::test]
    async fn required_metric_names_are_always_rendered() {
        let metrics = super::MetricsRegistry::default();
        metrics.initialize().await;
        let rendered = metrics.render().await;
        for name in super::METRIC_NAMES {
            assert!(rendered.contains(name));
        }
    }

    #[tokio::test]
    async fn drain_middleware_rejects_writes_with_stable_retry_contract() {
        let lifecycle = ServiceLifecycle::default();
        lifecycle.begin_drain();
        let app = Router::new()
            .route("/write", post(|| async { StatusCode::NO_CONTENT }))
            .layer(middleware::from_fn(reject_writes_while_draining))
            .layer(Extension(lifecycle));
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/write")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers().get("retry-after").unwrap(), "5");
    }

    #[tokio::test]
    async fn drain_endpoint_sets_readiness_and_drain_metric() {
        let lifecycle = ServiceLifecycle::default();
        let metrics = MetricsRegistry::default();
        super::drain(
            Extension(HealthState {
                service_name: "test-service",
                registry: HealthRegistry::default(),
                lifecycle: lifecycle.clone(),
            }),
            Extension(metrics.clone()),
        )
        .await;
        assert!(lifecycle.is_draining());
        assert!(metrics.render().await.contains("agentx_drain_inflight 0"));
    }

    #[tokio::test]
    async fn pool_wait_samples_and_dropped_connections_remain_observable() {
        let metrics = MetricsRegistry::default();
        metrics.initialize().await;
        for milliseconds in 1..=100 {
            metrics
                .observe_mysql_pool_wait(std::time::Duration::from_millis(milliseconds))
                .await;
        }
        assert!(
            metrics
                .render()
                .await
                .contains("agentx_mysql_pool_wait_p95_ms 95\n")
        );
        assert!(
            metrics
                .render()
                .await
                .contains("agentx_mysql_pool_wait_samples 100\n")
        );
        let connection = metrics.track("agentx_sse_connections").await;
        assert!(
            metrics
                .render()
                .await
                .contains("agentx_sse_connections 1\n")
        );
        drop(connection);
        tokio::task::yield_now().await;
        assert!(
            metrics
                .render()
                .await
                .contains("agentx_sse_connections 0\n")
        );
    }

    #[tokio::test]
    async fn role_watchdog_changes_liveness_without_treating_dependencies_as_process_health() {
        let registry = HealthRegistry::default();
        registry.register("mysql", true).await;
        registry.set_status("mysql", "unavailable").await;
        let lifecycle = ServiceLifecycle::default();
        let watchdog = RoleProgressWatchdog::start(
            "fixture",
            std::time::Duration::from_millis(100),
            registry.clone(),
            lifecycle.clone(),
            MetricsRegistry::default(),
        )
        .await;
        let state = HealthState {
            service_name: "fixture",
            registry: registry.clone(),
            lifecycle,
        };
        let (initial, _) = live(Extension(state.clone())).await;
        assert_eq!(initial, StatusCode::OK);
        tokio::time::sleep(std::time::Duration::from_millis(140)).await;
        let (stalled, _) = live(Extension(state.clone())).await;
        assert_eq!(stalled, StatusCode::SERVICE_UNAVAILABLE);
        watchdog.progress();
        tokio::time::sleep(std::time::Duration::from_millis(35)).await;
        let (recovered, _) = live(Extension(state)).await;
        assert_eq!(recovered, StatusCode::OK);
    }
}
