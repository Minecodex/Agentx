use std::{collections::BTreeMap, env, sync::Arc};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, Method, Response, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Serialize;
use tokio::sync::RwLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum FaultMode {
    Off,
    Armed,
    Forwarding,
    Held,
}

#[derive(Clone)]
struct ProxyState {
    upstream: String,
    client: reqwest::Client,
    faults: Arc<RwLock<BTreeMap<String, FaultMode>>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    agentx_service_kit::install_tls_provider();
    let upstream = env::var("AGENTX_FAULT_PROXY_UPSTREAM")
        .context("AGENTX_FAULT_PROXY_UPSTREAM is required")?
        .trim_end_matches('/')
        .to_owned();
    let state = ProxyState {
        upstream,
        client: reqwest::Client::new(),
        faults: Arc::new(RwLock::new(BTreeMap::from([
            ("prepare".into(), FaultMode::Off),
            ("activate".into(), FaultMode::Off),
            ("message".into(), FaultMode::Off),
        ]))),
    };
    let router = Router::new()
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(|| async { StatusCode::OK }))
        .route("/e2e/faults", get(faults))
        .route("/e2e/faults/{action}", post(control_fault))
        .fallback(proxy)
        .with_state(state);
    let address = env::var("AGENTX_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn faults(State(state): State<ProxyState>) -> Json<BTreeMap<String, FaultMode>> {
    Json(state.faults.read().await.clone())
}

async fn control_fault(
    State(state): State<ProxyState>,
    Path(action): Path<String>,
) -> Result<Json<BTreeMap<String, FaultMode>>, StatusCode> {
    let (operation, mode) = parse_fault_action(&action).ok_or(StatusCode::NOT_FOUND)?;
    set_fault(&state, operation.to_owned(), mode).await
}

async fn set_fault(
    state: &ProxyState,
    operation: String,
    mode: FaultMode,
) -> Result<Json<BTreeMap<String, FaultMode>>, StatusCode> {
    let mut faults = state.faults.write().await;
    let value = faults.get_mut(&operation).ok_or(StatusCode::NOT_FOUND)?;
    *value = mode;
    Ok(Json(faults.clone()))
}

async fn proxy(
    State(state): State<ProxyState>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let operation = operation(uri.path());
    let discard_response = if let Some(operation) = operation {
        let mut faults = state.faults.write().await;
        match faults.get(operation).copied().unwrap_or(FaultMode::Off) {
            FaultMode::Held | FaultMode::Forwarding => {
                return fault_response(StatusCode::SERVICE_UNAVAILABLE, "E2E_RESPONSE_HELD");
            }
            FaultMode::Armed => {
                faults.insert(operation.into(), FaultMode::Forwarding);
                true
            }
            FaultMode::Off => false,
        }
    } else {
        false
    };

    let mut request = state
        .client
        .request(method, format!("{}{}", state.upstream, uri))
        .body(body);
    for (name, value) in &headers {
        if !is_hop_header(name.as_str()) && name != axum::http::header::HOST {
            request = request.header(name, value);
        }
    }
    let upstream = match request.send().await {
        Ok(response) => response,
        Err(_) => {
            if discard_response {
                state
                    .faults
                    .write()
                    .await
                    .insert(operation.expect("armed operation").into(), FaultMode::Armed);
            }
            return fault_response(StatusCode::BAD_GATEWAY, "E2E_UPSTREAM_UNAVAILABLE");
        }
    };
    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => return fault_response(StatusCode::BAD_GATEWAY, "E2E_UPSTREAM_BODY_LOST"),
    };
    if discard_response {
        state
            .faults
            .write()
            .await
            .insert(operation.expect("armed operation").into(), FaultMode::Held);
        loop {
            let released = state
                .faults
                .read()
                .await
                .get(operation.expect("armed operation"))
                .copied()
                == Some(FaultMode::Off);
            if released {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        return fault_response(StatusCode::BAD_GATEWAY, "E2E_RESPONSE_DROPPED");
    }

    let mut response = Response::builder().status(status);
    for (name, value) in &response_headers {
        if !is_hop_header(name.as_str()) && name != axum::http::header::CONTENT_LENGTH {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn operation(path: &str) -> Option<&'static str> {
    match path {
        "/internal/runtime/v1/bundles:prepare" => Some("prepare"),
        "/internal/runtime/v1/deployments:activate" => Some("activate"),
        _ if path.starts_with("/gateway/v1/sessions/") && path.ends_with("/messages") => {
            Some("message")
        }
        _ => None,
    }
}

fn parse_fault_action(value: &str) -> Option<(&str, FaultMode)> {
    value
        .strip_suffix(":arm")
        .map(|operation| (operation, FaultMode::Armed))
        .or_else(|| {
            value
                .strip_suffix(":release")
                .map(|operation| (operation, FaultMode::Off))
        })
}

fn is_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn fault_response(status: StatusCode, code: &str) -> Response<Body> {
    (
        status,
        Json(serde_json::json!({
            "code": code,
            "message": "the V2 fault proxy discarded an upstream response"
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{FaultMode, operation, parse_fault_action};

    #[test]
    fn fault_actions_preserve_the_e2e_api_path() {
        assert_eq!(
            parse_fault_action("prepare:arm"),
            Some(("prepare", FaultMode::Armed))
        );
        assert_eq!(
            parse_fault_action("activate:release"),
            Some(("activate", FaultMode::Off))
        );
        assert_eq!(
            operation("/gateway/v1/sessions/session-id/messages"),
            Some("message")
        );
        assert_eq!(parse_fault_action("prepare:unknown"), None);
    }
}
