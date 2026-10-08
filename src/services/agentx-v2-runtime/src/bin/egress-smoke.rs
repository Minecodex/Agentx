use std::{env, net::SocketAddr, time::Duration};

use agentx_runtime_contracts::EgressRole;
use agentx_v2_runtime::egress::{EgressRequestContext, ProviderHttpClient};
use anyhow::{Context, Result, bail};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use tokio::task::JoinSet;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    agentx_service_kit::install_tls_provider();
    if env::var_os("AGENTX_EGRESS_SMOKE_ASSERT_DIRECT_BLOCKED").is_some() {
        assert_direct_public_egress_blocked().await?;
        println!(
            "{}",
            json!({"status":"passed","negative":"direct-public-egress"})
        );
        return Ok(());
    }
    let endpoint = env::var("AGENTX_EGRESS_SMOKE_ENDPOINT")
        .context("AGENTX_EGRESS_SMOKE_ENDPOINT is required")?;
    let endpoint = endpoint.trim_end_matches('/').to_owned();
    let client = ProviderHttpClient::from_env(EgressRole::WorkflowWorker)?;
    let context = EgressRequestContext::execution(Uuid::now_v7(), Uuid::now_v7());
    let stability_only = env::var("AGENTX_EGRESS_SMOKE_STABILITY_ONLY")
        .is_ok_and(|value| value.eq_ignore_ascii_case("true"));
    if !stability_only {
        run_matrix(&client, context, &endpoint).await?;
        assert_denied(&client, context, "https://10.0.0.1/").await?;
        assert_denied(
            &client,
            context,
            "https://169.254.169.254/latest/meta-data/",
        )
        .await?;
    }

    let concurrency = env_u32("AGENTX_EGRESS_SMOKE_CONCURRENCY", 8, 1, 64)?;
    let stability_seconds = env_u32("AGENTX_EGRESS_SMOKE_STABILITY_SECONDS", 0, 0, 7_200)?;
    let stability_interval_seconds =
        env_u32("AGENTX_EGRESS_SMOKE_STABILITY_INTERVAL_SECONDS", 10, 1, 60)?;
    let stability_endpoint = env::var("AGENTX_EGRESS_SMOKE_STABILITY_ENDPOINT")
        .unwrap_or_else(|_| format!("{endpoint}/health"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(u64::from(stability_seconds));
    let mut rounds = 0_u64;
    let mut transient_fixture_retries = 0_u64;
    loop {
        transient_fixture_retries +=
            run_concurrent_health(&client, context, &stability_endpoint, concurrency).await?;
        rounds += 1;
        if stability_seconds == 0 || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(u64::from(stability_interval_seconds))).await;
    }
    println!(
        "{}",
        json!({
            "status":"passed",
            "mode":if stability_only { "stability-only" } else { "functional-matrix" },
            "matrix":if stability_only { Vec::<&str>::new() } else { vec!["model","mcp-streamable-http","mcp-sse","memory","rag","http-request"] },
            "negative":if stability_only { Vec::<&str>::new() } else { vec!["private-ip","metadata"] },
            "concurrency":concurrency,
            "rounds":rounds,
            "stabilityIntervalSeconds":stability_interval_seconds,
            "transientFixtureRetries":transient_fixture_retries
        })
    );
    Ok(())
}

async fn assert_direct_public_egress_blocked() -> Result<()> {
    let resolved = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host(("example.com", 443)),
    )
    .await
    .context("direct-egress DNS resolution timed out")?
    .context("direct-egress DNS resolution failed")?
    .collect::<Vec<_>>();
    if resolved.is_empty() {
        bail!("direct-egress DNS returned no addresses");
    }

    let address = "1.1.1.1:443"
        .parse::<SocketAddr>()
        .context("invalid public test address")?;
    let connected = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(address),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .is_some();
    if connected {
        bail!("Runtime Pod bypassed agentx-egress-gateway and connected to {address}");
    }
    Ok(())
}

async fn run_matrix(
    client: &ProviderHttpClient,
    context: EgressRequestContext,
    endpoint: &str,
) -> Result<()> {
    let cases = [
        (
            "model",
            "/v1/chat/completions",
            json!({"model":"fixture","messages":[]}),
        ),
        ("rag-query", "/rag/query", json!({"query":"health"})),
        (
            "rag-insert",
            "/rag/documents/text",
            json!({"text":"health"}),
        ),
        ("memory-search", "/memory/search", json!({"query":"health"})),
        ("memory-write", "/memory/memories", json!({"messages":[]})),
        ("http-request", "/http", json!({"input":"health"})),
    ];
    for (name, path, payload) in cases {
        let response = client
            .post(
                &format!("{endpoint}{path}"),
                context,
                Duration::from_secs(15),
            )?
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("{name} request failed"))?;
        if !response.status().is_success() {
            bail!("{name} returned {}", response.status());
        }
        let value = response
            .json::<Value>()
            .await
            .with_context(|| format!("{name} response was not JSON"))?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            bail!("{name} fixture response was not successful");
        }
    }

    for accept in ["application/json", "application/json, text/event-stream"] {
        let response = client
            .post(&format!("{endpoint}/mcp"), context, Duration::from_secs(15))?
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, accept)
            .json(&json!({
                "jsonrpc":"2.0",
                "id":1,
                "method":"initialize",
                "params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"agentx-smoke","version":"1"}}
            }))
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("MCP fixture returned {}", response.status());
        }
        let body = response.text().await?;
        if !body.contains("protocolVersion") {
            bail!("MCP fixture response is invalid");
        }
    }
    Ok(())
}

async fn assert_denied(
    client: &ProviderHttpClient,
    context: EgressRequestContext,
    endpoint: &str,
) -> Result<()> {
    match client
        .get(endpoint, context, Duration::from_secs(5))?
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            bail!("forbidden endpoint {endpoint} was reachable")
        }
        _ => Ok(()),
    }
}

async fn run_concurrent_health(
    client: &ProviderHttpClient,
    context: EgressRequestContext,
    endpoint: &str,
    concurrency: u32,
) -> Result<u64> {
    let mut tasks = JoinSet::new();
    for task_index in 0..concurrency {
        let client = client.clone();
        let url = endpoint.to_owned();
        tasks.spawn(async move {
            for attempt in 0..6_u64 {
                let result = client
                    .get(&url, context, Duration::from_secs(15))?
                    .send()
                    .await;
                match result {
                    Ok(response) if response.status().is_success() => {
                        return Ok::<u64, anyhow::Error>(attempt);
                    }
                    Ok(response) if response.status().is_server_error() && attempt < 5 => {}
                    Ok(response) => bail!("health fixture returned {}", response.status()),
                    Err(error) if (error.is_connect() || error.is_timeout()) && attempt < 5 => {}
                    Err(error) => return Err(error.into()),
                }
                let backoff_ms = (1_u64 << attempt.min(4)) * 500 + u64::from(task_index) * 25;
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
            unreachable!("the bounded fixture retry loop always returns")
        });
    }
    let mut retries = 0_u64;
    while let Some(result) = tasks.join_next().await {
        retries += result??;
    }
    Ok(retries)
}

fn env_u32(name: &str, default: u32, minimum: u32, maximum: u32) -> Result<u32> {
    let value = env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .parse::<u32>()
        .with_context(|| format!("{name} must be an integer"))?;
    if !(minimum..=maximum).contains(&value) {
        bail!("{name} must be in {minimum}..={maximum}");
    }
    Ok(value)
}
