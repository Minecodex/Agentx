use std::{collections::BTreeMap, env, fmt, sync::Arc, time::Duration};

use agentx_runtime_contracts::{
    EGRESS_RUNTIME_TOKEN_TTL_SECONDS, EGRESS_TOKEN_AUDIENCE, EGRESS_TOKEN_ISSUER,
    EgressConnectClaimsV1, EgressDestinationV1, EgressMode, EgressPortRangeV1, EgressRole,
    content_hash, issue_egress_connect_token, now_unix,
};
use anyhow::{Context, Result, bail};
use reqwest::{
    Method, Response, Url,
    header::{
        AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HeaderName, HeaderValue,
        TRANSFER_ENCODING,
    },
};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

const MAX_PROVIDER_REDIRECTS: usize = 5;
const MAX_PROVIDER_CONNECT_ATTEMPTS: usize = 3;

fn exact_destination(host: &str, port: u16) -> Vec<EgressDestinationV1> {
    vec![EgressDestinationV1 {
        target: host.to_owned(),
        ports: vec![EgressPortRangeV1 {
            from: port,
            to: port,
        }],
    }]
}

#[derive(Clone, Copy, Debug)]
pub struct EgressRequestContext {
    pub tenant_id: Uuid,
    pub execution_id: Option<Uuid>,
    pub request_id: Option<Uuid>,
}

impl EgressRequestContext {
    #[must_use]
    pub const fn request(tenant_id: Uuid, request_id: Uuid) -> Self {
        Self {
            tenant_id,
            execution_id: None,
            request_id: Some(request_id),
        }
    }

    #[must_use]
    pub const fn execution(tenant_id: Uuid, execution_id: Uuid) -> Self {
        Self {
            tenant_id,
            execution_id: Some(execution_id),
            request_id: None,
        }
    }
}

#[derive(Clone)]
pub struct ProviderHttpClient {
    role: EgressRole,
    proxy_url: Url,
    key_id: Arc<str>,
    private_key_pem: Arc<Vec<u8>>,
    direct: reqwest::Client,
    pub(crate) legacy_sse_sessions: Arc<Mutex<BTreeMap<String, mpsc::Sender<LegacySseExchange>>>>,
}

pub(crate) struct LegacySseExchange {
    pub headers: reqwest::header::HeaderMap,
    pub body: Value,
    pub timeout: Duration,
    pub response: oneshot::Sender<Result<LegacySseExchangeResponse, LegacySseExchangeError>>,
}

pub(crate) struct LegacySseExchangeResponse {
    pub status: reqwest::StatusCode,
    pub headers: reqwest::header::HeaderMap,
    pub body: bytes::Bytes,
}

#[derive(Clone, Debug)]
pub(crate) struct LegacySseExchangeError {
    pub message: String,
    pub is_connect: bool,
}

pub struct ProviderRequestBuilder {
    provider: ProviderHttpClient,
    context: EgressRequestContext,
    timeout: Duration,
    inner: reqwest::RequestBuilder,
}

#[derive(Debug)]
pub enum ProviderRequestError {
    Request(reqwest::Error),
    Policy(anyhow::Error),
    Timeout,
    RedirectLimit,
    UncloneableRedirect,
}

impl ProviderRequestError {
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout) || matches!(self, Self::Request(error) if error.is_timeout())
    }

    #[must_use]
    pub fn is_connect(&self) -> bool {
        matches!(self, Self::Request(error) if error.is_connect())
    }
}

impl fmt::Display for ProviderRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(error) => error.fmt(formatter),
            Self::Policy(error) => write!(formatter, "provider redirect was denied: {error}"),
            Self::Timeout => formatter.write_str("provider request timed out"),
            Self::RedirectLimit => formatter.write_str("provider redirect limit exceeded"),
            Self::UncloneableRedirect => {
                formatter.write_str("provider redirect cannot replay a streaming request body")
            }
        }
    }
}

impl std::error::Error for ProviderRequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request(error) => Some(error),
            Self::Policy(error) => Some(error.as_ref()),
            Self::Timeout | Self::RedirectLimit | Self::UncloneableRedirect => None,
        }
    }
}

impl ProviderRequestBuilder {
    #[must_use]
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<axum::http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<axum::http::Error>,
    {
        self.inner = self.inner.header(key, value);
        self
    }

    #[must_use]
    pub fn bearer_auth<T>(mut self, token: T) -> Self
    where
        T: fmt::Display,
    {
        self.inner = self.inner.bearer_auth(token);
        self
    }

    #[must_use]
    pub fn json<T: Serialize + ?Sized>(mut self, json: &T) -> Self {
        self.inner = self.inner.json(json);
        self
    }

    pub async fn send(self) -> Result<Response, ProviderRequestError> {
        let started = tokio::time::Instant::now();
        let mut request = self.inner.build().map_err(ProviderRequestError::Request)?;
        for redirect_count in 0..=MAX_PROVIDER_REDIRECTS {
            let redirect_replay = request
                .try_clone()
                .ok_or(ProviderRequestError::UncloneableRedirect)?;
            let mut connect_attempt = 0;
            let response = loop {
                connect_attempt += 1;
                let elapsed = started.elapsed();
                let remaining = self
                    .timeout
                    .checked_sub(elapsed)
                    .ok_or(ProviderRequestError::Timeout)?;
                *request.timeout_mut() = Some(remaining);
                let connect_replay = request
                    .try_clone()
                    .ok_or(ProviderRequestError::UncloneableRedirect)?;
                let client = self
                    .provider
                    .client_for_url(request.url(), self.context)
                    .map_err(ProviderRequestError::Policy)?;
                match tokio::time::timeout(remaining, client.execute(request)).await {
                    Ok(Ok(response)) => break response,
                    Ok(Err(error))
                        if error.is_connect()
                            && connect_attempt < MAX_PROVIDER_CONNECT_ATTEMPTS =>
                    {
                        request = connect_replay;
                    }
                    Ok(Err(error)) => return Err(ProviderRequestError::Request(error)),
                    Err(_) => return Err(ProviderRequestError::Timeout),
                }
            };
            if !response.status().is_redirection() {
                return Ok(response);
            }
            let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
                return Ok(response);
            };
            if redirect_count == MAX_PROVIDER_REDIRECTS {
                return Err(ProviderRequestError::RedirectLimit);
            }
            let location = location.to_str().map_err(|error| {
                ProviderRequestError::Policy(anyhow::anyhow!("invalid redirect Location: {error}"))
            })?;
            request = prepare_redirect_request(
                redirect_replay,
                response.url(),
                response.status(),
                location,
            )?;
        }
        Err(ProviderRequestError::RedirectLimit)
    }
}

fn prepare_redirect_request(
    mut request: reqwest::Request,
    response_url: &Url,
    status: reqwest::StatusCode,
    location: &str,
) -> Result<reqwest::Request, ProviderRequestError> {
    let next_url = response_url
        .join(location)
        .context("invalid provider redirect URL")
        .and_then(|url| validate_provider_url(url.as_str()))
        .map_err(ProviderRequestError::Policy)?;
    let cross_origin = origin(&next_url) != origin(request.url());
    *request.url_mut() = next_url;
    if cross_origin {
        request.headers_mut().remove(AUTHORIZATION);
        request.headers_mut().remove(COOKIE);
    }
    if status == reqwest::StatusCode::SEE_OTHER
        || ((status == reqwest::StatusCode::MOVED_PERMANENTLY
            || status == reqwest::StatusCode::FOUND)
            && request.method() == Method::POST)
    {
        *request.method_mut() = Method::GET;
        *request.body_mut() = None;
        request.headers_mut().remove(CONTENT_LENGTH);
        request.headers_mut().remove(CONTENT_TYPE);
        request.headers_mut().remove(TRANSFER_ENCODING);
    }
    Ok(request)
}

impl ProviderHttpClient {
    pub fn from_env(role: EgressRole) -> Result<Self> {
        let proxy_url = env::var("AGENTX_EGRESS_PROXY_URL")
            .context("AGENTX_EGRESS_PROXY_URL is required")?
            .parse::<Url>()
            .context("AGENTX_EGRESS_PROXY_URL is invalid")?;
        if proxy_url.scheme() != "http" || proxy_url.host_str().is_none() {
            bail!("AGENTX_EGRESS_PROXY_URL must be an internal HTTP proxy URL");
        }
        let key_id =
            env::var("AGENTX_EGRESS_JWT_KEY_ID").context("AGENTX_EGRESS_JWT_KEY_ID is required")?;
        if key_id.trim().is_empty() {
            bail!("AGENTX_EGRESS_JWT_KEY_ID cannot be empty");
        }
        let private_key_pem = env::var("AGENTX_EGRESS_JWT_PRIVATE_KEY_PEM")
            .context("AGENTX_EGRESS_JWT_PRIVATE_KEY_PEM is required")?
            .into_bytes();
        let direct = provider_builder()?
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            role,
            proxy_url,
            key_id: key_id.into(),
            private_key_pem: Arc::new(private_key_pem),
            direct,
            legacy_sse_sessions: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    #[cfg(test)]
    pub(crate) fn direct_for_test(address: std::net::SocketAddr) -> Self {
        let direct = provider_builder()
            .expect("test provider client")
            .resolve("echo-mcp", address)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("test provider client build");
        Self {
            role: EgressRole::WorkflowWorker,
            proxy_url: "http://127.0.0.1:1".parse().expect("test proxy URL"),
            key_id: Arc::from("test-key"),
            private_key_pem: Arc::new(Vec::new()),
            direct,
            legacy_sse_sessions: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(crate) fn for_internal_provider(role: EgressRole) -> Result<Self> {
        let direct = provider_builder()?
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            role,
            proxy_url: "http://127.0.0.1:1"
                .parse()
                .expect("fixed internal placeholder"),
            key_id: Arc::from("internal-only"),
            private_key_pem: Arc::new(Vec::new()),
            direct,
            legacy_sse_sessions: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    pub fn get(
        &self,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: Duration,
    ) -> Result<ProviderRequestBuilder> {
        self.request(Method::GET, endpoint, context, timeout)
    }

    pub fn post(
        &self,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: Duration,
    ) -> Result<ProviderRequestBuilder> {
        self.request(Method::POST, endpoint, context, timeout)
    }

    /// Builds the one trusted worker-to-Sandbox-Manager request. This deliberately
    /// bypasses the public provider proxy, but only after validating the fixed
    /// internal service name and contract path.
    pub fn post_sandbox_manager(
        &self,
        endpoint: &str,
        timeout: Duration,
    ) -> Result<reqwest::RequestBuilder> {
        let url = validate_sandbox_manager_execute_url(endpoint)?;
        Ok(self.direct.post(url).timeout(timeout))
    }

    pub fn request(
        &self,
        method: Method,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: Duration,
    ) -> Result<ProviderRequestBuilder> {
        let url = validate_provider_url(endpoint)?;
        let inner = self.direct.request(method, url).timeout(timeout);
        Ok(ProviderRequestBuilder {
            provider: self.clone(),
            context,
            timeout,
            inner,
        })
    }

    fn client_for_url(&self, url: &Url, context: EgressRequestContext) -> Result<reqwest::Client> {
        if is_managed_cluster_fixture(url) {
            return Ok(self.direct.clone());
        }
        let host = url
            .host_str()
            .context("provider endpoint host is required")?
            .trim_matches(['[', ']'])
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .context("provider port is required")?;
        let destinations = exact_destination(&host, port);
        let now = now_unix();
        let claims = EgressConnectClaimsV1 {
            iss: EGRESS_TOKEN_ISSUER.into(),
            aud: EGRESS_TOKEN_AUDIENCE.into(),
            role: self.role,
            tenant_id: context.tenant_id,
            execution_id: context.execution_id,
            request_id: context.request_id,
            egress_mode: EgressMode::TcpProxy,
            policy_hash: content_hash(&destinations)?,
            destinations,
            iat: now,
            exp: now + EGRESS_RUNTIME_TOKEN_TTL_SECONDS,
            jti: Uuid::now_v7(),
        };
        let token = issue_egress_connect_token(&self.key_id, &self.private_key_pem, &claims)
            .context("failed signing egress CONNECT token")?;
        let proxy_auth = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .context("egress CONNECT token is not a valid header")?;
        let proxy = reqwest::Proxy::all(self.proxy_url.clone())?.custom_http_auth(proxy_auth);
        let client = provider_builder()?
            .proxy(proxy)
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(client)
    }

    /// Opens a CONNECT tunnel through the managed egress proxy for a public
    /// WSS endpoint and returns the raw tunnelled TCP stream. Callers complete
    /// the TLS and WebSocket handshake over it, so long-lived provider stream
    /// connections follow the same controlled public egress path as HTTP.
    pub async fn open_public_websocket_tunnel(
        &self,
        url: &Url,
        context: EgressRequestContext,
    ) -> Result<tokio::net::TcpStream> {
        let host = url
            .host_str()
            .context("websocket endpoint host is required")?
            .trim_matches(['[', ']'])
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .context("websocket endpoint port is required")?;
        let destinations = exact_destination(&host, port);
        let now = now_unix();
        let claims = EgressConnectClaimsV1 {
            iss: EGRESS_TOKEN_ISSUER.into(),
            aud: EGRESS_TOKEN_AUDIENCE.into(),
            role: self.role,
            tenant_id: context.tenant_id,
            execution_id: context.execution_id,
            request_id: context.request_id,
            egress_mode: EgressMode::TcpProxy,
            policy_hash: content_hash(&destinations)?,
            destinations,
            iat: now,
            exp: now + EGRESS_RUNTIME_TOKEN_TTL_SECONDS,
            jti: Uuid::now_v7(),
        };
        let token = issue_egress_connect_token(&self.key_id, &self.private_key_pem, &claims)
            .context("failed signing egress CONNECT token")?;
        let proxy_host = self
            .proxy_url
            .host_str()
            .context("egress proxy host is required")?
            .to_owned();
        let proxy_port = self.proxy_url.port_or_known_default().unwrap_or(3128);
        let mut stream = tokio::net::TcpStream::connect((proxy_host.as_str(), proxy_port))
            .await
            .context("egress proxy connection failed")?;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let request = format!(
            "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\nProxy-Authorization: Bearer {token}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await?;
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 256];
        loop {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                bail!("egress proxy closed the tunnel during CONNECT")
            }
            buffer.extend_from_slice(&chunk[..read]);
            let Some(header_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                if buffer.len() > 8 * 1024 {
                    bail!("egress proxy CONNECT response is too large")
                }
                continue;
            };
            let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
            let status = head.lines().next().unwrap_or_default();
            if !status.contains(" 200 ") {
                bail!("egress proxy refused CONNECT: {status}")
            }
            return Ok(stream);
        }
    }
}

fn origin(url: &Url) -> (&str, Option<&str>, Option<u16>) {
    (url.scheme(), url.host_str(), url.port_or_known_default())
}

pub fn validate_provider_url(endpoint: &str) -> Result<Url> {
    let url = Url::parse(endpoint).context("provider endpoint is invalid")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("provider endpoint must be an HTTP(S) URL with a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("provider endpoint userinfo is forbidden");
    }
    if url.fragment().is_some() {
        bail!("provider endpoint fragment is forbidden");
    }
    if url.scheme() == "http" && !is_managed_cluster_fixture(&url) {
        bail!("public provider endpoints must use HTTPS");
    }
    Ok(url)
}

pub fn validate_sandbox_manager_execute_url(endpoint: &str) -> Result<Url> {
    let url = Url::parse(endpoint).context("Sandbox Manager endpoint is invalid")?;
    if url.scheme() != "http" || url.host_str().is_none() {
        bail!("Sandbox Manager endpoint must be an internal HTTP URL with a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("Sandbox Manager endpoint userinfo is forbidden");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("Sandbox Manager endpoint query and fragment are forbidden");
    }
    let path = url.path();
    let process_action = path
        .strip_prefix("/internal/runtime/v1/sandbox-process-sessions/")
        .and_then(|tail| tail.rsplit_once(':'))
        .is_some_and(|(id, action)| {
            uuid::Uuid::parse_str(id).is_ok()
                && matches!(
                    action,
                    "write" | "read" | "wait" | "interrupt" | "terminate" | "reconcile"
                )
        });
    if !matches!(
        path,
        "/internal/runtime/v1/sandboxes:execute"
            | "/internal/runtime/v1/sandboxes:acquire"
            | "/internal/runtime/v1/sandboxes:tool"
            | "/internal/runtime/v1/sandboxes:release"
            | "/internal/runtime/v1/sandbox-process-sessions:start"
    ) && !process_action
    {
        bail!("Sandbox Manager endpoint path is not an approved runtime contract");
    }
    let host = url
        .host_str()
        .expect("host presence was checked")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let trusted_service = host == "sandbox-manager"
        || (host.starts_with("sandbox-manager.")
            && (host.ends_with(".svc") || host.ends_with(".svc.cluster.local")));
    if !trusted_service {
        bail!("Sandbox Manager endpoint must target the sandbox-manager Kubernetes service");
    }
    Ok(url)
}

/// Plain-HTTP provider endpoints are restricted to in-cluster fixture
/// services. The allowlist is the first DNS label of the service and defaults
/// to the managed E2E fixtures; deployments can extend it (for example with a
/// self-hosted `ragflow` service) through AGENTX_EGRESS_HTTP_PROVIDER_SERVICES.
fn http_provider_services() -> Vec<String> {
    let configured = env::var("AGENTX_EGRESS_HTTP_PROVIDER_SERVICES")
        .unwrap_or_else(|_| "echo-mcp,echo-node,lightrag,mem0".into());
    configured
        .split(',')
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn is_managed_cluster_fixture(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let service = host.split('.').next().unwrap_or_default();
    let fixture_service = http_provider_services().iter().any(|item| item == service);
    let fixture_dns =
        !host.contains('.') || host.ends_with(".svc") || host.ends_with(".svc.cluster.local");
    url.scheme() == "http" && fixture_service && fixture_dns
}

fn provider_builder() -> Result<reqwest::ClientBuilder> {
    agentx_service_kit::reqwest_client_builder_with_ca("AGENTX_RUNTIME_PROVIDER_TLS_CA_PATH")
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::sync::Mutex;

    use reqwest::{Method, StatusCode, header};

    use super::{
        prepare_redirect_request, validate_provider_url, validate_sandbox_manager_execute_url,
    };

    // validate_provider_url reads AGENTX_EGRESS_HTTP_PROVIDER_SERVICES; the
    // test that mutates it must not race the tests asserting default behavior.
    static PROVIDER_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn public_endpoints_require_https_and_forbid_userinfo() {
        let _guard = PROVIDER_ENV_LOCK.lock().unwrap();
        assert!(validate_provider_url("https://api.example.com/v1").is_ok());
        assert!(validate_provider_url("http://api.example.com/v1").is_err());
        assert!(validate_provider_url("https://user:password@api.example.com/v1").is_err());
        assert!(validate_provider_url("https://api.example.com/v1#secret").is_err());
    }

    #[test]
    fn managed_cluster_fixtures_keep_their_http_path() {
        let _guard = PROVIDER_ENV_LOCK.lock().unwrap();
        assert!(
            validate_provider_url("http://echo-mcp.agentx-deps.svc.cluster.local:8090/mcp").is_ok()
        );
        assert!(validate_provider_url("http://10.0.0.1:8090/mcp").is_err());
        assert!(
            validate_provider_url("http://arbitrary-service.test.svc.cluster.local:8090/mcp")
                .is_err()
        );
        assert!(validate_provider_url("http://ragflow.agentx-deps.svc:9380/api/v1").is_err());
    }

    #[test]
    fn http_provider_services_allowlist_is_configurable() {
        let _guard = PROVIDER_ENV_LOCK.lock().unwrap();
        // SAFETY: the shared lock keeps concurrent tests off the ambient env
        // while the override is held; it is restored below.
        unsafe {
            env::set_var("AGENTX_EGRESS_HTTP_PROVIDER_SERVICES", "echo-mcp,ragflow");
        }
        assert!(validate_provider_url("http://ragflow.agentx-deps.svc:9380/api/v1").is_ok());
        assert!(validate_provider_url("http://lightrag.agentx-deps.svc:9621/query").is_err());
        unsafe {
            env::remove_var("AGENTX_EGRESS_HTTP_PROVIDER_SERVICES");
        }
        assert!(validate_provider_url("http://ragflow.agentx-deps.svc:9380/api/v1").is_err());
    }

    #[test]
    fn only_the_fixed_sandbox_manager_contract_endpoints_are_trusted() {
        assert!(
            validate_sandbox_manager_execute_url(
                "http://sandbox-manager:8080/internal/runtime/v1/sandboxes:execute"
            )
            .is_ok()
        );
        for operation in ["acquire", "tool", "release"] {
            assert!(
                validate_sandbox_manager_execute_url(&format!(
                    "http://sandbox-manager:8080/internal/runtime/v1/sandboxes:{operation}"
                ))
                .is_ok()
            );
        }
        assert!(
            validate_sandbox_manager_execute_url(
                "http://sandbox-manager:8080/internal/runtime/v1/sandbox-process-sessions:start"
            )
            .is_ok()
        );
        assert!(
            validate_sandbox_manager_execute_url(
                "http://sandbox-manager:8080/internal/runtime/v1/sandbox-process-sessions/00000000-0000-0000-0000-000000000001:write"
            )
            .is_ok()
        );
        assert!(
            validate_sandbox_manager_execute_url(
                "http://sandbox-manager:8080/internal/runtime/v1/sandbox-process-sessions/not-a-uuid:terminate"
            )
            .is_err()
        );
        assert!(
            validate_sandbox_manager_execute_url(
                "http://sandbox-manager.agentx-runtime.svc.cluster.local:8080/internal/runtime/v1/sandboxes:execute"
            )
            .is_ok()
        );
        assert!(
            validate_sandbox_manager_execute_url(
                "http://10.0.0.8:8080/internal/runtime/v1/sandboxes:execute"
            )
            .is_err()
        );
        assert!(
            validate_sandbox_manager_execute_url(
                "http://arbitrary-service.agentx-runtime.svc:8080/internal/runtime/v1/sandboxes:execute"
            )
            .is_err()
        );
        assert!(
            validate_sandbox_manager_execute_url(
                "http://sandbox-manager:8080/internal/runtime/v1/admin"
            )
            .is_err()
        );
    }

    #[test]
    fn cross_origin_redirect_revalidates_and_drops_credentials() {
        let request = reqwest::Client::new()
            .post("https://provider.example/v1/chat")
            .bearer_auth("provider-secret")
            .header(header::COOKIE, "session=secret")
            .json(&serde_json::json!({"prompt":"hello"}))
            .build()
            .unwrap();
        let redirected = prepare_redirect_request(
            request,
            &"https://provider.example/v1/chat".parse().unwrap(),
            StatusCode::FOUND,
            "https://other.example/v2/chat",
        )
        .unwrap();
        assert_eq!(redirected.url().host_str(), Some("other.example"));
        assert_eq!(redirected.method(), Method::GET);
        assert!(!redirected.headers().contains_key(header::AUTHORIZATION));
        assert!(!redirected.headers().contains_key(header::COOKIE));
        assert!(redirected.body().is_none());
    }

    #[test]
    fn same_origin_temporary_redirect_preserves_request_semantics() {
        let request = reqwest::Client::new()
            .post("https://provider.example/v1/chat")
            .bearer_auth("provider-secret")
            .body("payload")
            .build()
            .unwrap();
        let redirected = prepare_redirect_request(
            request,
            &"https://provider.example/v1/chat".parse().unwrap(),
            StatusCode::TEMPORARY_REDIRECT,
            "/v2/chat",
        )
        .unwrap();
        assert_eq!(redirected.url().path(), "/v2/chat");
        assert_eq!(redirected.method(), Method::POST);
        assert!(redirected.headers().contains_key(header::AUTHORIZATION));
        assert!(redirected.body().is_some());
    }
}
