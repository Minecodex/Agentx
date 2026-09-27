use agentx_v2_runtime::RuntimeState;
use anyhow::Result;
use axum::{
    Router,
    http::{HeaderName, HeaderValue, Method, header},
    routing::{get, post},
};
use futures::StreamExt;
use std::time::Duration;
use tower_http::cors::{AllowOrigin, CorsLayer};

#[tokio::main]
async fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() == Some("openapi") {
        let path = arguments
            .next()
            .unwrap_or_else(|| "contracts/openapi/trigger-gateway.json".into());
        anyhow::ensure!(arguments.next().is_none(), "unexpected argument");
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&agentx_v2_runtime::gateway::public_openapi())?,
        )?;
        return Ok(());
    }
    let state = RuntimeState::gateway_from_env().await?;
    let lifecycle = agentx_service_kit::ServiceLifecycle::default();
    let metrics = agentx_service_kit::MetricsRegistry::default();
    let health = agentx_service_kit::HealthRegistry::default();
    health.register("runtime_mysql", true).await;
    health.register("runtime_object_storage", true).await;
    health.set_status("runtime_mysql", "ready").await;
    health.set_status("runtime_object_storage", "ready").await;
    let probe_state = state.clone();
    let probe_health = health.clone();
    let probe_lifecycle = lifecycle.clone();
    let probe_progress = agentx_service_kit::RoleProgressWatchdog::start(
        "dependency-probe",
        Duration::from_secs(agentx_service_kit::ROLE_WATCHDOG_TIMEOUT_SECONDS),
        health.clone(),
        lifecycle.clone(),
        metrics.clone(),
    )
    .await;
    tokio::spawn(async move {
        while !probe_lifecycle.is_draining() {
            let started = std::time::Instant::now();
            match sqlx::query("SELECT 1").execute(&probe_state.pool).await {
                Ok(_) => probe_health.set_status("runtime_mysql", "ready").await,
                Err(error) => {
                    probe_health
                        .set_status("runtime_mysql", "unavailable")
                        .await;
                    tracing::warn!(%error, "Runtime Gateway MySQL readiness probe failed");
                }
            }
            match probe_state.objects.list(None).next().await.transpose() {
                Ok(_) => {
                    probe_health
                        .set_status("runtime_object_storage", "ready")
                        .await;
                }
                Err(error) => {
                    probe_health
                        .set_status("runtime_object_storage", "unavailable")
                        .await;
                    tracing::warn!(%error, "Runtime Gateway object storage readiness probe failed");
                }
            }
            probe_progress.processed_since(started).await;
            tokio::select! {
                () = probe_lifecycle.cancelled() => break,
                () = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    });
    let public_gateway = agentx_v2_runtime::gateway::router().layer(runtime_cors()?);
    let router = Router::new()
        .route(
            "/internal/runtime/v1/events:export",
            get(agentx_v2_runtime::event_export::export_events),
        )
        .route(
            "/internal/runtime/v1/governance-snapshots:export",
            post(agentx_v2_runtime::event_export::export_governance_snapshot),
        )
        .route(
            "/internal/runtime/v1/objects:upload",
            post(agentx_v2_runtime::object_upload::upload_object),
        )
        .route(
            "/internal/runtime/v1/bundles:prepare",
            post(agentx_v2_runtime::publish::prepare_bundle),
        )
        .route(
            "/internal/runtime/v1/admission-commands:apply",
            post(agentx_v2_runtime::publish::apply_admission),
        )
        .route(
            "/internal/runtime/v1/chat-mappings:apply",
            post(agentx_v2_runtime::publish::apply_chat_mapping),
        )
        .route(
            "/internal/runtime/v1/deployments:activate",
            post(agentx_v2_runtime::publish::activate_deployment),
        )
        .route(
            "/internal/runtime/v1/deployments:rollback",
            post(agentx_v2_runtime::publish::rollback_deployment),
        )
        .route(
            "/internal/runtime/v1/deployments:disable",
            post(agentx_v2_runtime::publish::disable_deployment),
        )
        .route(
            "/internal/runtime/v1/channel-status",
            get(agentx_v2_runtime::stream::channel_status),
        )
        .route(
            "/internal/runtime/v1/query/invocations:search",
            post(agentx_v2_runtime::query::search_invocations),
        )
        .route(
            "/internal/runtime/v1/query/deliveries:search",
            post(agentx_v2_runtime::delivery_query::search_deliveries),
        )
        .route(
            "/internal/runtime/v1/query/deliveries/{id}",
            get(agentx_v2_runtime::delivery_query::get_delivery),
        )
        .route(
            "/internal/runtime/v1/query/deliveries/{id}/retry",
            post(agentx_v2_runtime::delivery_query::retry_delivery),
        )
        .route(
            "/internal/runtime/v1/query/invocations/{id}",
            get(agentx_v2_runtime::query::get_invocation),
        )
        .route(
            "/internal/runtime/v1/query/executions:search",
            post(agentx_v2_runtime::query::search_executions),
        )
        .route(
            "/internal/runtime/v1/query/executions/{id}",
            get(agentx_v2_runtime::query::get_execution),
        )
        .route(
            "/internal/runtime/v1/query/executions/{id}/nodes",
            get(agentx_v2_runtime::query::get_execution_nodes),
        )
        .route(
            "/internal/runtime/v1/query/executions/{id}/nodes/{node_execution_id}",
            get(agentx_v2_runtime::query::get_execution_node),
        )
        .route(
            "/internal/runtime/v1/query/executions/{id}/events",
            get(agentx_v2_runtime::query::get_execution_events),
        )
        .route(
            "/internal/runtime/v1/query/executions/{id}/checkpoints",
            get(agentx_v2_runtime::query::get_execution_checkpoints),
        )
        .route(
            "/internal/runtime/v1/query/executions/{id}/runtime-details",
            get(agentx_v2_runtime::query::get_execution_runtime_details),
        )
        .route(
            "/internal/runtime/v1/query/executions/{id}/artifacts/{artifact_id}",
            get(agentx_v2_runtime::query::get_execution_artifact),
        )
        .route(
            "/internal/runtime/v1/query/sessions:search",
            post(agentx_v2_runtime::query::search_sessions),
        )
        .route(
            "/internal/runtime/v1/query/sessions/{id}",
            get(agentx_v2_runtime::query::get_session),
        )
        .route(
            "/internal/runtime/v1/query/agent-sessions:search",
            post(agentx_v2_runtime::query::search_agent_sessions),
        )
        .route(
            "/internal/runtime/v1/query/agent-sessions/{session_key}/{node_key}",
            get(agentx_v2_runtime::query::get_agent_session),
        )
        .route(
            "/internal/runtime/v1/agent-sessions:clear",
            post(agentx_v2_runtime::query::clear_agent_session),
        )
        .route(
            "/internal/runtime/v1/agent-subject-memory:clear",
            post(agentx_v2_runtime::query::clear_agent_subject_memory),
        )
        .route(
            "/internal/runtime/v1/query/agent-subject-memory:search",
            post(agentx_v2_runtime::query::search_agent_subject_memory),
        )
        .route(
            "/internal/runtime/v1/session-commands:apply",
            post(agentx_v2_runtime::query::apply_session_command),
        )
        .route(
            "/internal/runtime/v1/work-packages:prepare",
            post(agentx_v2_runtime::internal_engine::prepare_work_package),
        )
        .route(
            "/internal/runtime/v1/work-packages/{package_action}",
            post(agentx_v2_runtime::internal_engine::apply_work_package_action),
        )
        .route(
            "/internal/runtime/v1/runtime-commands:apply",
            post(agentx_v2_runtime::internal_engine::apply_runtime_command),
        )
        .route(
            "/internal/runtime/v1/references:check",
            post(agentx_v2_runtime::internal_engine::check_references),
        )
        .route(
            "/internal/runtime/v1/retention-commands:apply",
            post(agentx_v2_runtime::internal_engine::apply_retention_command),
        )
        .route(
            "/internal/runtime/v1/resource-checks:execute",
            post(agentx_v2_runtime::resource_check::execute_resource_check),
        )
        .route(
            "/internal/runtime/v1/resource-operations:execute",
            post(agentx_v2_runtime::resource_check::execute_resource_operation),
        )
        .route(
            "/internal/runtime/v1/plugin-design-operations:execute",
            post(agentx_v2_runtime::plugin_design::execute),
        )
        .nest("/gateway/v1", public_gateway)
        .with_state(state);
    let admission_metrics = metrics.clone();
    let router = router.layer(axum::middleware::from_fn(
        move |request: axum::http::Request<axum::body::Body>, next: axum::middleware::Next| {
            let metrics = admission_metrics.clone();
            async move {
                let count = agentx_v2_runtime::admission::rejection_count();
                metrics
                    .set("agentx_admission_rejections_total", count as f64)
                    .await;
                next.run(request).await
            }
        },
    ));
    agentx_service_kit::serve_with_lifecycle("runtime-gateway", router, health, lifecycle, metrics)
        .await
}

fn runtime_cors() -> Result<CorsLayer> {
    let dynamic = std::env::var("AGENTX_RUNTIME_CORS_ALLOW_DYNAMIC_ORIGIN")
        .is_ok_and(|value| value.eq_ignore_ascii_case("true"));
    let origin = std::env::var("AGENTX_RUNTIME_CORS_ALLOWED_ORIGIN").ok();
    runtime_cors_layer(dynamic, origin.as_deref())
}

fn runtime_cors_layer(dynamic: bool, origin: Option<&str>) -> Result<CorsLayer> {
    let allow_origin = if dynamic {
        AllowOrigin::mirror_request()
    } else {
        AllowOrigin::exact(
            origin
                .ok_or_else(|| anyhow::anyhow!("AGENTX_RUNTIME_CORS_ALLOWED_ORIGIN is required"))?
                .parse::<HeaderValue>()?,
        )
    };
    Ok(CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("idempotency-key"),
            HeaderName::from_static("last-event-id"),
            HeaderName::from_static("x-agentx-signature"),
            HeaderName::from_static("x-agentx-timestamp"),
        ])
        .max_age(Duration::from_secs(600)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    async fn preflight(cors: CorsLayer, origin: &'static str) -> axum::response::Response {
        Router::new()
            .route(
                "/gateway/v1/sessions",
                post(|| async { StatusCode::CREATED }),
            )
            .layer(cors)
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/gateway/v1/sessions")
                    .header(header::ORIGIN, origin)
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "authorization,idempotency-key",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn local_cors_reflects_port_forward_origin_with_credentials() {
        let response = preflight(
            runtime_cors_layer(true, None).unwrap(),
            "http://127.0.0.1:54321",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("http://127.0.0.1:54321"))
        );
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
            Some(&HeaderValue::from_static("true"))
        );
    }

    #[tokio::test]
    async fn production_cors_rejects_unconfigured_origin() {
        let cors = runtime_cors_layer(false, Some("https://control.agentx.example")).unwrap();
        let response = preflight(cors, "https://other.example").await;
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://control.agentx.example"))
        );
    }
}
