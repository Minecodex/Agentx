//! Stream channel supervisor: lease-claimed reverse WebSocket connections
//! (DingTalk Stream, Feishu long connection) that feed provider events into
//! the same normalize/map/dispatch pipeline as HTTP callbacks.

pub(crate) mod dingtalk_stream;
pub(crate) mod feishu_ws;

use std::collections::HashMap;
use std::time::Duration;

use agentx_runtime_contracts::{VaultSecretReferenceV1, WebhookInputMappingV1, WebhookProviderV1};
use anyhow::{Context, Result};
use sqlx::{MySqlPool, Row};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use url::Url;
use uuid::Uuid;

pub type ProviderWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn is_public_host(url: &Url) -> bool {
    if !matches!(url.scheme(), "wss" | "https") {
        return false;
    }
    url.host_str().is_some_and(|host| {
        let host = host
            .trim_matches(['[', ']'])
            .trim_end_matches('.')
            .to_ascii_lowercase();
        host.contains('.')
            && !host.ends_with(".svc")
            && !host.ends_with(".svc.cluster.local")
            && host != "localhost"
            && !host.starts_with("127.")
    })
}

fn is_public_wss(url: &Url) -> bool {
    is_public_host(url)
}

/// POSTs a provider bootstrap request. Public HTTPS endpoints go through the
/// managed egress proxy when configured; cluster fixtures and test URL
/// overrides post directly.
pub(crate) async fn provider_post_json(
    url: &str,
    body: &serde_json::Value,
    tenant_id: Uuid,
    request_id: Uuid,
) -> Result<serde_json::Value> {
    let parsed = Url::parse(url)?;
    let client = if is_public_host(&parsed) {
        crate::egress::ProviderHttpClient::from_env(
            agentx_runtime_contracts::EgressRole::WorkflowRuntime,
        )
        .map_err(|error| anyhow::anyhow!("managed provider egress is unavailable: {error}"))?
    } else {
        crate::egress::ProviderHttpClient::for_internal_provider(
            agentx_runtime_contracts::EgressRole::WorkflowRuntime,
        )?
    };
    let response = client
        .post(
            url,
            crate::egress::EgressRequestContext::request(tenant_id, request_id),
            std::time::Duration::from_secs(10),
        )
        .map_err(|error| anyhow::anyhow!("provider bootstrap request was rejected: {error}"))?
        .json(body)
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("provider bootstrap request failed: {error}"))?;
    let response = response
        .error_for_status()
        .map_err(|error| anyhow::anyhow!("provider bootstrap request failed: {error}"))?;
    Ok(response.json().await?)
}

/// Dials a provider WebSocket. Public WSS endpoints go through the managed
/// egress CONNECT tunnel when the egress proxy is configured; cluster fixtures
/// and test URL overrides dial directly.
/// rustls 0.23 needs a process-level CryptoProvider; nothing else in the
/// workspace installs one because this is the first tokio-tungstenite TLS
/// path. Installing ring explicitly avoids the builder panic.
fn ensure_crypto_provider() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}

pub(crate) async fn dial_websocket(
    url: &str,
    tenant_id: Uuid,
    request_id: Uuid,
) -> Result<ProviderWebSocket> {
    // rustls needs a process-level CryptoProvider for every TLS path,
    // including tokio-tungstenite's internal connect_async.
    ensure_crypto_provider();
    let parsed = Url::parse(url)?;
    if is_public_wss(&parsed) {
        let client = crate::egress::ProviderHttpClient::from_env(
            agentx_runtime_contracts::EgressRole::WorkflowRuntime,
        )
        .map_err(|error| anyhow::anyhow!("managed provider egress is unavailable: {error}"))?;
        let tunnel = client
            .open_public_websocket_tunnel(
                &parsed,
                crate::egress::EgressRequestContext::request(tenant_id, request_id),
            )
            .await?;
        let roots = tokio_rustls::rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let request = parsed.as_str().into_client_request()?;
        let (socket, _) = tokio_tungstenite::client_async_tls_with_config(
            request,
            tunnel,
            None,
            Some(Connector::Rustls(std::sync::Arc::new(config))),
        )
        .await?;
        return Ok(socket);
    }
    let (socket, _) = tokio_tungstenite::connect_async(url).await?;
    Ok(socket)
}

pub struct StreamClaim {
    pub binding_id: Uuid,
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub provider: WebhookProviderV1,
    pub trigger_name: String,
    pub configuration_revision: u64,
    pub secret_ref: VaultSecretReferenceV1,
    pub input_mappings: Vec<WebhookInputMappingV1>,
    pub fixed_inputs: serde_json::Value,
    pub owner: Uuid,
    pub fencing_token: u64,
}

pub async fn claim(pool: &MySqlPool, owner: Uuid, limit: i64) -> Result<Vec<StreamClaim>> {
    let mut tx = pool.begin().await?;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM webhook_bindings WHERE status='active' AND channel_mode='stream' AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY id LIMIT ? FOR UPDATE SKIP LOCKED")
        .bind(limit.clamp(1, 50)).fetch_all(&mut *tx).await?;
    let mut claimed = Vec::new();
    for id in ids {
        let updated = sqlx::query("UPDATE webhook_bindings SET locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),heartbeat_at=UTC_TIMESTAMP(6),fencing_token=fencing_token+1,connection_status='pending',connection_error=NULL WHERE id=? AND status='active' AND channel_mode='stream' AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6))")
            .bind(owner).bind(id).execute(&mut *tx).await?;
        if updated.rows_affected() == 1 {
            claimed.push(id);
        }
    }
    let mut claims = Vec::new();
    for id in claimed {
        let row = match sqlx::query("SELECT w.id,w.tenant_id,w.application_id,w.provider_type,w.secret_ref_json,w.input_mapping_json,w.fixed_inputs_json,w.configuration_revision,w.fencing_token,CAST(JSON_UNQUOTE(JSON_EXTRACT(t.configuration_json,'$.triggerName')) AS CHAR(255)) trigger_name FROM webhook_bindings w JOIN trigger_bindings t ON t.tenant_id=w.tenant_id AND t.id=w.id WHERE w.id=? AND w.locked_by=? AND w.locked_until>UTC_TIMESTAMP(6)")
            .bind(id).bind(owner).fetch_optional(&mut *tx).await? { Some(row) => row, None => continue };
        let provider = match row
            .try_get::<Option<String>, _>("provider_type")?
            .as_deref()
            .unwrap_or("agentx")
        {
            "dingtalk" => WebhookProviderV1::Dingtalk,
            "feishu" => WebhookProviderV1::Feishu,
            other => {
                tracing::warn!(
                    provider = other,
                    "Stream binding has an unsupported provider"
                );
                continue;
            }
        };
        claims.push(StreamClaim {
            binding_id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            application_id: row.try_get("application_id")?,
            provider,
            trigger_name: row
                .try_get::<Option<String>, _>("trigger_name")?
                .unwrap_or_default(),
            configuration_revision: row.try_get("configuration_revision")?,
            secret_ref: serde_json::from_value(row.try_get("secret_ref_json")?)?,
            input_mappings: row
                .try_get::<Option<serde_json::Value>, _>("input_mapping_json")?
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default(),
            fixed_inputs: row
                .try_get::<Option<serde_json::Value>, _>("fixed_inputs_json")?
                .unwrap_or(serde_json::json!({})),
            owner,
            fencing_token: row.try_get("fencing_token")?,
        });
    }
    tx.commit().await?;
    Ok(claims)
}

pub async fn heartbeat(pool: &MySqlPool, claim: &StreamClaim) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query("UPDATE webhook_bindings SET locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),heartbeat_at=UTC_TIMESTAMP(6) WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(claim.binding_id).bind(claim.owner).bind(claim.fencing_token).execute(pool).await?;
    Ok(changed.rows_affected() == 1)
}

pub async fn release(
    pool: &MySqlPool,
    claim: &StreamClaim,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE webhook_bindings SET locked_by=NULL,locked_until=NULL,heartbeat_at=NULL,connection_status=? WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(status).bind(claim.binding_id).bind(claim.owner).bind(claim.fencing_token).execute(pool).await?;
    Ok(())
}

pub async fn update_status(
    pool: &MySqlPool,
    claim: &StreamClaim,
    status: &str,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE webhook_bindings SET connection_status=?,connection_error=?,last_connected_at=IF(?='connected',UTC_TIMESTAMP(6),last_connected_at) WHERE id=? AND locked_by=? AND fencing_token=?")
        .bind(status).bind(error).bind(status).bind(claim.binding_id).bind(claim.owner).bind(claim.fencing_token).execute(pool).await?;
    Ok(())
}

async fn binding_current(pool: &MySqlPool, claim: &StreamClaim) -> Result<bool> {
    let row = sqlx::query("SELECT status,channel_mode,configuration_revision FROM webhook_bindings WHERE tenant_id=? AND id=?")
        .bind(claim.tenant_id).bind(claim.binding_id).fetch_optional(pool).await?;
    Ok(
        matches!(row, Some(row) if row.try_get::<String,_>("status")? == "active"
        && row.try_get::<String,_>("channel_mode")? == "stream"
        && row.try_get::<u64,_>("configuration_revision")? == claim.configuration_revision),
    )
}

/// Role entry point, hosted by the workflow-runtime `stream` role.
pub async fn stream_loop(
    pool: MySqlPool,
    owner: Uuid,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    let vault =
        crate::vault::RuntimeVault::from_env().context("stream role requires Vault access")?;
    let mut running: HashMap<Uuid, tokio::task::JoinHandle<()>> = HashMap::new();
    loop {
        if lifecycle.is_draining() {
            break;
        }
        progress.progress();
        for claim in claim(&pool, owner, 50).await? {
            tracing::info!(binding = %claim.binding_id, provider = ?claim.provider, "Stream channel claimed");
            let pool = pool.clone();
            let vault = vault.clone();
            let lifecycle = lifecycle.clone();
            let binding_id = claim.binding_id;
            running.insert(
                binding_id,
                tokio::spawn(async move {
                    run_binding(pool, vault, lifecycle, claim).await;
                }),
            );
        }
        running.retain(|_, handle| !handle.is_finished());
        tokio::select! {
            _ = lifecycle.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
    for (_, handle) in running.drain() {
        handle.abort();
    }
    Ok(())
}

async fn run_binding(
    pool: MySqlPool,
    vault: crate::vault::RuntimeVault,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    claim: StreamClaim,
) {
    let secret = match vault.read(&claim.secret_ref).await {
        Ok(secret) => secret,
        Err(error) => {
            tracing::warn!(%error, binding = %claim.binding_id, "Stream channel secret unavailable");
            let _ = update_status(&pool, &claim, "error", Some("secret unavailable")).await;
            let _ = release(&pool, &claim, "error").await;
            return;
        }
    };
    // Reconnect backoff stays below the 30s lease so heartbeats never lapse
    // long enough for another replica to claim the binding.
    let mut backoff = Duration::from_secs(5);
    loop {
        if lifecycle.is_draining() || !binding_current(&pool, &claim).await.unwrap_or(false) {
            let _ = release(&pool, &claim, "disconnected").await;
            return;
        }
        let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(10));
        heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        heartbeat_interval.tick().await;
        let run = run_provider(&pool, &claim, &secret);
        tokio::pin!(run);
        loop {
            tokio::select! {
                result = &mut run => {
                    if let Err(error) = result {
                        tracing::warn!(%error, binding = %claim.binding_id, "Stream connection ended");
                        let _ = update_status(&pool, &claim, "reconnecting", Some(&error.to_string())).await;
                    }
                    break;
                }
                _ = heartbeat_interval.tick() => {
                    if !heartbeat(&pool, &claim).await.unwrap_or(false) {
                        tracing::warn!(binding = %claim.binding_id, "Stream lease lost");
                        let _ = release(&pool, &claim, "disconnected").await;
                        return;
                    }
                }
                _ = lifecycle.cancelled() => {
                    let _ = release(&pool, &claim, "disconnected").await;
                    return;
                }
            }
        }
        tokio::select! {
            _ = lifecycle.cancelled() => { let _ = release(&pool, &claim, "disconnected").await; return; }
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(Duration::from_secs(15));
    }
}

/// Internal endpoint consumed by Control to surface stream channel connection
/// state in the Application channel UI.
pub async fn channel_status(
    axum::extract::State(state): axum::extract::State<crate::RuntimeState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<axum::Json<serde_json::Value>, crate::error::RuntimeError> {
    let tenant_id: Uuid = query
        .get("tenantId")
        .and_then(|value| value.parse().ok())
        .ok_or(crate::error::RuntimeError::InvalidRequest(
            "TENANT_REQUIRED",
            "tenantId is required".into(),
        ))?;
    let application_id: Option<Uuid> = query
        .get("applicationId")
        .and_then(|value| value.parse().ok());
    let claims = state
        .trust
        .delegation(&headers, tenant_id, "runtime.channels.status")?;
    if let Some(application_id) = application_id
        && !claims.tenant_wide
        && !claims.application_ids.contains(&application_id)
    {
        return Err(crate::error::RuntimeError::Unauthorized);
    }
    let rows = sqlx::query("SELECT id,application_id,channel_mode,connection_status,connection_error,last_connected_at,heartbeat_at FROM webhook_bindings WHERE tenant_id=? AND channel_mode='stream' AND (? IS NULL OR application_id=?)")
        .bind(tenant_id).bind(application_id).bind(application_id)
        .fetch_all(&state.pool).await?;
    let channels: Vec<serde_json::Value> = rows.iter().map(|row| serde_json::json!({
        "id": row.try_get::<Uuid, _>("id").ok(),
        "applicationId": row.try_get::<Uuid, _>("application_id").ok(),
        "mode": row.try_get::<String, _>("channel_mode").ok(),
        "connectionStatus": row.try_get::<Option<String>, _>("connection_status").ok().flatten(),
        "connectionError": row.try_get::<Option<String>, _>("connection_error").ok().flatten(),
        "lastConnectedAt": row.try_get::<Option<time::OffsetDateTime>, _>("last_connected_at").ok().flatten().map(|value| value.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()),
        "heartbeatAt": row.try_get::<Option<time::OffsetDateTime>, _>("heartbeat_at").ok().flatten().map(|value| value.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()),
    })).collect();
    Ok(axum::Json(serde_json::json!({ "channels": channels })))
}

async fn run_provider(pool: &MySqlPool, claim: &StreamClaim, secret: &[u8]) -> Result<()> {
    match claim.provider {
        WebhookProviderV1::Dingtalk => dingtalk_stream::run(pool, claim, secret).await,
        WebhookProviderV1::Feishu => feishu_ws::run(pool, claim, secret).await,
        other => Err(anyhow::anyhow!(
            "provider {other:?} does not support stream mode"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_tls_hosts_take_the_egress_path() {
        assert!(is_public_host(
            &Url::parse("https://api.dingtalk.com/v1.0/gateway/connections/open").unwrap()
        ));
        assert!(is_public_host(
            &Url::parse("wss://wss-open-connection.dingtalk.com:443/connect").unwrap()
        ));
        assert!(
            !is_public_host(&Url::parse("ws://127.0.0.1:9000/connect").unwrap()),
            "test overrides dial directly"
        );
        assert!(
            !is_public_host(&Url::parse("ws://mock-gateway.agentx-deps.svc/connect").unwrap()),
            "cluster fixtures dial directly"
        );
        assert!(
            !is_public_host(&Url::parse("http://api.dingtalk.com/v1").unwrap()),
            "plain HTTP is never treated as a public provider endpoint"
        );
    }
}
