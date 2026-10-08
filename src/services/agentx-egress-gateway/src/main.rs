use std::{env, net::SocketAddr, path::Path};

use agentx_egress_gateway::{GatewayState, ListenerKind, load_tls_config};
use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    agentx_service_kit::install_tls_provider();
    let lifecycle = agentx_service_kit::ServiceLifecycle::default();
    let metrics = agentx_service_kit::MetricsRegistry::default();
    let health = agentx_service_kit::HealthRegistry::default();
    let state = GatewayState::from_env(metrics.clone())?;
    let runtime_address = address("AGENTX_EGRESS_RUNTIME_BIND_ADDR", "0.0.0.0:3128")?;
    let sandbox_address = address("AGENTX_EGRESS_SANDBOX_BIND_ADDR", "0.0.0.0:3129")?;
    let tls = load_tls_config(
        Path::new(&required("AGENTX_EGRESS_TLS_CERT_PATH")?),
        Path::new(&required("AGENTX_EGRESS_TLS_KEY_PATH")?),
    )?;

    let mut tasks = tokio::task::JoinSet::<Result<()>>::new();
    let runtime_state = state.clone();
    let runtime_lifecycle = lifecycle.clone();
    tasks.spawn(async move {
        agentx_egress_gateway::serve_plain(
            runtime_address,
            runtime_state,
            ListenerKind::Runtime,
            runtime_lifecycle,
        )
        .await
    });
    let sandbox_lifecycle = lifecycle.clone();
    tasks.spawn(async move {
        agentx_egress_gateway::serve_tls(
            sandbox_address,
            state,
            ListenerKind::Sandbox,
            sandbox_lifecycle,
            tls,
        )
        .await
    });
    let service_lifecycle = lifecycle.clone();
    tasks.spawn(async move {
        agentx_service_kit::serve_with_lifecycle(
            "agentx-egress-gateway",
            axum::Router::new(),
            health,
            service_lifecycle,
            metrics,
        )
        .await
    });
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => lifecycle.begin_drain(),
            Ok(Err(error)) => {
                lifecycle.begin_drain();
                return Err(error);
            }
            Err(error) => {
                lifecycle.begin_drain();
                return Err(error.into());
            }
        }
    }
    Ok(())
}

fn address(name: &str, fallback: &str) -> Result<SocketAddr> {
    env::var(name)
        .unwrap_or_else(|_| fallback.into())
        .parse()
        .with_context(|| format!("{name} must be a valid socket address"))
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} is required"))
}
