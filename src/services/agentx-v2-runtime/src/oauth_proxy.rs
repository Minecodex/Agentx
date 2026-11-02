//! Fixed-route reverse proxy to the mature OAuth server's public endpoints.
//! Hydra's admin API is never routed to public clients.
use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{StatusCode, header},
    response::Response,
    routing::any,
};
use std::time::Duration;
pub fn routes() -> Router<RuntimeState> {
    Router::new()
        .route("/oauth2/{*path}", any(proxy))
        .route("/.well-known/openid-configuration", any(proxy))
        .route("/.well-known/oauth-authorization-server", any(proxy))
        .route("/.well-known/jwks.json", any(proxy))
}
async fn proxy(request: Request) -> RuntimeResult<Response> {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let allowed = match parts.method.as_str() {
        "GET" => matches!(
            path,
            "/oauth2/auth"
                | "/oauth2/sessions/logout"
                | "/.well-known/openid-configuration"
                | "/.well-known/oauth-authorization-server"
                | "/.well-known/jwks.json"
        ),
        "POST" => matches!(path, "/oauth2/token" | "/oauth2/revoke"),
        _ => false,
    };
    if !allowed {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("response"));
    }
    let upstream =
        std::env::var("AGENTX_HYDRA_PUBLIC_URL").map_err(|_| RuntimeError::Unavailable)?;
    let origin =
        std::env::var("AGENTX_RUNTIME_PUBLIC_ORIGIN").map_err(|_| RuntimeError::Unavailable)?;
    let public = url::Url::parse(&origin).map_err(|_| RuntimeError::Unavailable)?;
    let path = if path == "/.well-known/oauth-authorization-server" {
        "/.well-known/openid-configuration"
    } else {
        path
    };
    let mut target = url::Url::parse(&format!("{}{path}", upstream.trim_end_matches('/')))
        .map_err(|_| RuntimeError::Unavailable)?;
    target.set_query(parts.uri.query());
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| RuntimeError::Unavailable)?;
    let mut outbound = client
        .request(parts.method, target)
        .header("x-forwarded-proto", public.scheme())
        .header(
            "x-forwarded-host",
            &public[url::Position::BeforeHost..url::Position::AfterPort],
        );
    for name in [
        header::CONTENT_TYPE,
        header::AUTHORIZATION,
        header::COOKIE,
        header::ACCEPT,
    ] {
        if let Some(value) = parts.headers.get(&name) {
            outbound = outbound.header(name, value)
        }
    }
    let bytes = to_bytes(body, 1024 * 1024).await.map_err(|_| {
        RuntimeError::InvalidRequest(
            "OAUTH_REQUEST_TOO_LARGE",
            "OAuth request exceeded its limit".into(),
        )
    })?;
    let response = outbound
        .body(bytes)
        .send()
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    let mut result = Response::builder()
        .status(response.status())
        .header(header::CACHE_CONTROL, "no-store");
    for name in [
        header::CONTENT_TYPE,
        header::LOCATION,
        header::SET_COOKIE,
        header::WWW_AUTHENTICATE,
    ] {
        for value in response.headers().get_all(&name) {
            result = result.header(&name, value)
        }
    }
    // Token bodies stay on the direct TLS client connection; they never enter task or evidence logs.
    result
        .body(Body::from_stream(response.bytes_stream()))
        .map_err(|_| RuntimeError::Unavailable)
}
