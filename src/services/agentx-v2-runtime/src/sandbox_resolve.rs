//! Network-denied design resolution. Each invocation runs in a fresh process;
//! one workflow resolution may reuse its own short-lived sandbox container.
use super::*;

const IMAGE: &str = "opensandbox/code-interpreter@sha256:133a3c1720dd52291a019740c2987e7164ea6de79e23d8198798e58950ae2e6e";
const RUNNER: &str = include_str!("../../../plugins/packages/plugin-runner/runner.mjs");

pub(super) async fn execute(
    State(state): State<SandboxManagerState>,
    Json(request): Json<agentx_runtime_contracts::PluginDesignOperationRequestV1>,
) -> RuntimeResult<Json<agentx_runtime_contracts::PluginDesignOperationResponseV1>> {
    if request.method != "node.resolveDefinition"
        || !request.resources.is_empty()
        || request.plugin.runtime_source.len() > 2 * 1024 * 1024
        || request.resolve_session_id == Some(request.operation_id)
    {
        return Err(bad_request(
            "Resolve-only sandbox rejects resources and business methods",
        ));
    }
    let fingerprint = content_hash(&serde_json::to_value(&request).map_err(RuntimeError::from)?)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    if let Some(row)=sqlx::query("SELECT fingerprint,status,result_json FROM sandbox_design_leases WHERE tenant_id=? AND operation_id=?").bind(request.tenant_id).bind(request.operation_id).fetch_optional(&state.pool).await? {
        if row.try_get::<String,_>("fingerprint")?!=fingerprint.as_str() {return Err(conflict("Design operation identity conflict"));}
        if row.try_get::<String,_>("status")?=="completed" {return Ok(Json(agentx_runtime_contracts::PluginDesignOperationResponseV1 {protocol_version:1,operation_id:request.operation_id,result:row.try_get("result_json")?}));}
        return Err(RuntimeError::Unavailable);
    }
    let mut tx = state.pool.begin().await?;
    if let Some(session) = request.resolve_session_id {
        let session_hash = session_fingerprint(request.tenant_id, session)?;
        sqlx::query("INSERT INTO sandbox_design_leases(operation_id,session_id,tenant_id,fingerprint,owner,status,lease_until) VALUES(?,?,?,?,?,'idle',UTC_TIMESTAMP(6)+INTERVAL 60 SECOND) ON DUPLICATE KEY UPDATE operation_id=operation_id")
            .bind(session).bind(session).bind(request.tenant_id).bind(session_hash.as_str()).bind(request.operation_id).execute(&mut *tx).await?;
        let acquired = sqlx::query("UPDATE sandbox_design_leases SET owner=?,status='executing',lease_until=UTC_TIMESTAMP(6)+INTERVAL 60 SECOND WHERE operation_id=? AND session_id=? AND tenant_id=? AND fingerprint=? AND status='idle' AND lease_until>UTC_TIMESTAMP(6)")
            .bind(request.operation_id).bind(session).bind(session).bind(request.tenant_id).bind(session_hash.as_str()).execute(&mut *tx).await?;
        if acquired.rows_affected() != 1 {
            return Err(conflict("Resolve session is closed or already in use"));
        }
    }
    sqlx::query("INSERT INTO sandbox_design_leases(operation_id,session_id,tenant_id,fingerprint,owner,status,lease_until) VALUES(?,?,?,?,?,'creating',UTC_TIMESTAMP(6)+INTERVAL 60 SECOND)")
        .bind(request.operation_id).bind(request.resolve_session_id).bind(request.tenant_id).bind(fingerprint.as_str()).bind(state.owner).execute(&mut *tx).await?;
    tx.commit().await?;
    let result = run(&state, &request).await;
    let cleanup = if let Some(session) = request.resolve_session_id {
        if result.is_err() {
            close_session_inner(&state, request.tenant_id, session, false).await
        } else {
            let released=sqlx::query("UPDATE sandbox_design_leases SET status='idle',lease_until=UTC_TIMESTAMP(6)+INTERVAL 60 SECOND WHERE tenant_id=? AND operation_id=? AND session_id=? AND owner=? AND status='executing'")
                .bind(request.tenant_id).bind(session).bind(session).bind(request.operation_id).execute(&state.pool).await?;
            if released.rows_affected() == 1 {
                Ok(())
            } else {
                Err(RuntimeError::Unavailable)
            }
        }
    } else {
        cleanup_sandbox(&state, request.tenant_id, request.operation_id).await
    };
    if cleanup.is_err() {
        sqlx::query("UPDATE sandbox_design_leases SET status='cleanup_pending',lease_until=UTC_TIMESTAMP(6) WHERE tenant_id=? AND operation_id=? AND owner=? AND status IN ('creating','executing')").bind(request.tenant_id).bind(request.operation_id).bind(state.owner).execute(&state.pool).await?;
        return Err(RuntimeError::Unavailable);
    }
    match result {
        Ok(result) => {
            let completed=sqlx::query("UPDATE sandbox_design_leases SET status='completed',result_json=?,lease_until=UTC_TIMESTAMP(6) WHERE tenant_id=? AND operation_id=? AND owner=? AND status IN ('creating','executing')").bind(&result).bind(request.tenant_id).bind(request.operation_id).bind(state.owner).execute(&state.pool).await?;
            if completed.rows_affected() != 1 {
                return Err(RuntimeError::Unavailable);
            }
            Ok(Json(
                agentx_runtime_contracts::PluginDesignOperationResponseV1 {
                    protocol_version: 1,
                    operation_id: request.operation_id,
                    result,
                },
            ))
        }
        Err(error) => {
            tracing::warn!(operation_id=%request.operation_id, %error, "Resolve-only operation failed");
            sqlx::query("UPDATE sandbox_design_leases SET status='failed',lease_until=UTC_TIMESTAMP(6) WHERE tenant_id=? AND operation_id=? AND owner=?").bind(request.tenant_id).bind(request.operation_id).bind(state.owner).execute(&state.pool).await?;
            Err(error)
        }
    }
}

async fn run(
    state: &SandboxManagerState,
    request: &agentx_runtime_contracts::PluginDesignOperationRequestV1,
) -> RuntimeResult<Value> {
    let scope = request.resolve_session_id.unwrap_or(request.operation_id);
    let key = format!("resolve:{scope}");
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT sandbox_id FROM sandbox_design_leases WHERE operation_id=? AND tenant_id=?",
    )
    .bind(scope)
    .bind(request.tenant_id)
    .fetch_one(&state.pool)
    .await?;
    let fresh = existing.is_none();
    let sandbox = if let Some(existing) = existing {
        existing
    } else {
        let metadata = BTreeMap::from([
            ("agentxLeaseId".into(), scope.to_string()),
            ("agentxDesignOperation".into(), "resolve-only".into()),
            ("agentxTenantId".into(), request.tenant_id.to_string()),
        ]);
        let created = create_provider_sandbox(
            state,
            IMAGE,
            if request.resolve_session_id.is_some() {
                300
            } else {
                60
            },
            500,
            512 * 1024 * 1024,
            128 * 1024 * 1024,
            SandboxEgressModeV1::None,
            metadata,
            &key,
        )
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
        let sandbox = created
            .0
            .get("id")
            .and_then(Value::as_str)
            .ok_or(RuntimeError::Unavailable)?
            .to_owned();
        let owner = if request.resolve_session_id.is_some() {
            request.operation_id
        } else {
            state.owner
        };
        let attached=sqlx::query("UPDATE sandbox_design_leases SET sandbox_id=?,status='executing' WHERE tenant_id=? AND operation_id=? AND owner=? AND status IN ('creating','executing')")
        .bind(&sandbox).bind(request.tenant_id).bind(scope).bind(owner).execute(&state.pool).await?;
        if attached.rows_affected() != 1 {
            terminate_provider(state, &sandbox, &key)
                .await
                .map_err(|_| RuntimeError::Unavailable)?;
            return Err(RuntimeError::Unavailable);
        }
        sandbox
    };
    let (endpoint, headers) = process::provider_execd_endpoint(state, &sandbox)
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    if fresh {
        wait_execd_ready(state, &endpoint, &headers)
            .await
            .map_err(|_| RuntimeError::Unavailable)?;
    }
    let mut parameters = request.parameters.clone();
    parameters["runtimeSource"] = json!(request.plugin.runtime_source);
    parameters["runtimeEntry"] = json!(request.plugin.runtime_entry);
    parameters["deadlineMs"] = json!(10000);
    parameters["hostResourceReferences"] = json!([]);
    let messages = format!(
        "{}\n{}\n",
        json!({"jsonrpc":"2.0","id":"initialize","method":"runner.initialize","params":{"protocolVersion":2,"sdkApiVersion":2}}),
        json!({"jsonrpc":"2.0","id":"resolve","method":"node.resolveDefinition","params":parameters})
    );
    if fresh {
        upload_sandbox_file(
            state,
            &endpoint,
            headers.clone(),
            "/tmp/agentx-resolve-runner.mjs",
            RUNNER.as_bytes().to_vec(),
        )
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    }
    upload_sandbox_file(
        state,
        &endpoint,
        headers.clone(),
        "/tmp/agentx-resolve-input.jsonl",
        messages.into_bytes(),
    )
    .await
    .map_err(|_| RuntimeError::Unavailable)?;
    // Neither the runner nor untrusted JSON enters the shell argument vector.
    // Only the trusted runner file is readable by the permission-bound process.
    let command = "env -i PATH=/opt/node/v22.2.0/bin:/usr/local/bin:/usr/bin:/bin node --experimental-permission --allow-fs-read=/tmp/agentx-resolve-runner.mjs --max-old-space-size=96 /tmp/agentx-resolve-runner.mjs < /tmp/agentx-resolve-input.jsonl";
    let response=state.client.post(endpoint.join("command").map_err(|_|RuntimeError::Unavailable)?).headers(headers).json(&json!({"command":command,"cwd":"/workspace","background":false,"timeout":10000,"envs":{}})).send().await.map_err(|_|RuntimeError::Unavailable)?;
    if !response.status().is_success() {
        return Err(RuntimeError::ProviderRejected);
    }
    let (stdout, _stderr, exit) = read_resolve_command_result(response).await?;
    if exit != 0 {
        return Err(RuntimeError::InvalidRequest(
            "RESOLVE_ONLY_FAILED",
            "Isolated definition resolution failed or attempted a prohibited effect".into(),
        ));
    }
    let replies = parse_replies(&stdout)?;
    if replies.iter().any(|reply| reply.get("method").is_some()) {
        return Err(RuntimeError::InvalidRequest(
            "RESOLVE_ONLY_EFFECT_DENIED",
            "Host bridge calls are forbidden during definition resolution".into(),
        ));
    }
    let reply = replies
        .iter()
        .find(|reply| reply.get("id").and_then(Value::as_str) == Some("resolve"))
        .ok_or(RuntimeError::ProviderRejected)?;
    reply.get("result").cloned().ok_or_else(|| {
        RuntimeError::InvalidRequest(
            "RESOLVE_ONLY_FAILED",
            "The plugin could not resolve its definition in the isolated environment".into(),
        )
    })
}

async fn read_resolve_command_result(
    mut response: reqwest::Response,
) -> RuntimeResult<(String, String, i64)> {
    let mut body = Vec::new();
    let mut scanned = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RuntimeError::Unavailable)?
    {
        if body.len() + chunk.len() > 8 * 1024 * 1024 {
            return Err(RuntimeError::ProviderRejected);
        }
        body.extend_from_slice(&chunk);
        while let Some(offset) = body[scanned..].iter().position(|byte| *byte == b'\n') {
            let end = scanned + offset + 1;
            let line = std::str::from_utf8(&body[scanned..end])
                .map_err(|_| RuntimeError::ProviderRejected)?
                .trim();
            scanned = end;
            if line.is_empty() || line.starts_with(':') || line.starts_with("event:") {
                continue;
            }
            let data = line.strip_prefix("data:").unwrap_or(line).trim();
            let event: Value =
                serde_json::from_str(data).map_err(|_| RuntimeError::ProviderRejected)?;
            let terminal = match event.get("type").and_then(Value::as_str) {
                Some("execution_complete" | "error") => true,
                Some("result") => event
                    .get("exit_code")
                    .or_else(|| event.get("exitCode"))
                    .is_some(),
                _ => false,
            };
            if terminal {
                // Execd flushes the terminal event before its one-second tail
                // drain. Resolve-only has no external effects to synchronize.
                let text = std::str::from_utf8(&body[..end])
                    .map_err(|_| RuntimeError::ProviderRejected)?;
                return parse_command_stream(text).map_err(|_| RuntimeError::ProviderRejected);
            }
        }
    }
    let text = std::str::from_utf8(&body).map_err(|_| RuntimeError::ProviderRejected)?;
    parse_command_stream(text).map_err(|_| RuntimeError::ProviderRejected)
}

fn parse_replies(stdout: &str) -> RuntimeResult<Vec<Value>> {
    // Execd emits a stdout event per line and removes its trailing newline.
    // Its collected output is therefore an adjacent JSON value stream.
    serde_json::Deserializer::from_str(stdout)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RuntimeError::ProviderRejected)
}

fn session_fingerprint(
    tenant: Uuid,
    session: Uuid,
) -> RuntimeResult<agentx_runtime_contracts::ContentHash> {
    content_hash(&json!({"kind":"resolve-session","tenantId":tenant,"sessionId":session}))
        .map_err(|error| RuntimeError::Internal(error.into()))
}

async fn cleanup_sandbox(
    state: &SandboxManagerState,
    tenant: Uuid,
    scope: Uuid,
) -> RuntimeResult<()> {
    let sandbox: Option<String> = sqlx::query_scalar(
        "SELECT sandbox_id FROM sandbox_design_leases WHERE tenant_id=? AND operation_id=?",
    )
    .bind(tenant)
    .bind(scope)
    .fetch_one(&state.pool)
    .await?;
    let sandbox = match sandbox {
        Some(id) => Some(id),
        None => find_provider_sandbox(state, scope).await,
    };
    if let Some(sandbox) = sandbox {
        terminate_provider(state, &sandbox, &format!("resolve:{scope}"))
            .await
            .map_err(|_| RuntimeError::Unavailable)?;
    }
    Ok(())
}

pub(super) async fn close_session(
    State(state): State<SandboxManagerState>,
    Json(request): Json<agentx_runtime_contracts::PluginResolveSessionV1>,
) -> RuntimeResult<Json<agentx_runtime_contracts::PluginResolveSessionV1>> {
    close_session_inner(&state, request.tenant_id, request.session_id, false).await?;
    Ok(Json(request))
}

async fn close_session_inner(
    state: &SandboxManagerState,
    tenant: Uuid,
    session: Uuid,
    expired_only: bool,
) -> RuntimeResult<()> {
    let fingerprint = session_fingerprint(tenant, session)?;
    let mut tx = state.pool.begin().await?;
    // A tombstone also fences an invocation that arrives after its close.
    sqlx::query("INSERT INTO sandbox_design_leases(operation_id,session_id,tenant_id,fingerprint,owner,status,lease_until) VALUES(?,?,?,?,?,'completed',UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE operation_id=operation_id")
        .bind(session).bind(session).bind(tenant).bind(fingerprint.as_str()).bind(state.owner).execute(&mut *tx).await?;
    let row=sqlx::query("SELECT session_id,tenant_id,fingerprint,status,lease_until<=UTC_TIMESTAMP(6) AS expired FROM sandbox_design_leases WHERE operation_id=? FOR UPDATE").bind(session).fetch_one(&mut *tx).await?;
    if row.try_get::<Uuid, _>("tenant_id")? != tenant
        || row.try_get::<Option<Uuid>, _>("session_id")? != Some(session)
        || row.try_get::<String, _>("fingerprint")? != fingerprint.as_str()
    {
        return Err(conflict("Resolve session identity conflict"));
    }
    if matches!(
        row.try_get::<String, _>("status")?.as_str(),
        "completed" | "failed"
    ) || expired_only && !row.try_get::<bool, _>("expired")?
    {
        tx.commit().await?;
        return Ok(());
    }
    if row.try_get::<String, _>("status")? == "cleanup_pending"
        && !row.try_get::<bool, _>("expired")?
    {
        return Err(RuntimeError::Unavailable);
    }
    sqlx::query("UPDATE sandbox_design_leases SET status='cleanup_pending',owner=?,lease_until=UTC_TIMESTAMP(6)+INTERVAL 20 SECOND WHERE operation_id=?")
        .bind(state.owner).bind(session).execute(&mut *tx).await?;
    tx.commit().await?;
    cleanup_sandbox(state, tenant, session).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE sandbox_design_leases SET status='completed',lease_until=UTC_TIMESTAMP(6) WHERE tenant_id=? AND operation_id=? AND owner=? AND status='cleanup_pending'")
        .bind(tenant).bind(session).bind(state.owner).execute(&mut *tx).await?;
    sqlx::query("UPDATE sandbox_design_leases SET status='failed',lease_until=UTC_TIMESTAMP(6) WHERE tenant_id=? AND session_id=? AND operation_id<>? AND status IN ('creating','executing','cleanup_pending')")
        .bind(tenant).bind(session).bind(session).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn reconcile_one(state: &SandboxManagerState) -> RuntimeResult<bool> {
    let row=sqlx::query("SELECT tenant_id,operation_id,session_id FROM sandbox_design_leases WHERE status IN ('creating','executing','idle','cleanup_pending') AND lease_until<=UTC_TIMESTAMP(6) ORDER BY lease_until LIMIT 1").fetch_optional(&state.pool).await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let operation: Uuid = row.try_get("operation_id")?;
    let tenant: Uuid = row.try_get("tenant_id")?;
    if let Some(session) = row.try_get::<Option<Uuid>, _>("session_id")? {
        close_session_inner(state, tenant, session, true).await?;
        if operation != session {
            sqlx::query("UPDATE sandbox_design_leases SET status='failed' WHERE operation_id=? AND lease_until<=UTC_TIMESTAMP(6) AND status IN ('creating','executing','cleanup_pending')")
                .bind(operation).execute(&state.pool).await?;
        }
    } else {
        cleanup_sandbox(state, tenant, operation).await?;
        sqlx::query("UPDATE sandbox_design_leases SET status='failed' WHERE operation_id=? AND lease_until<=UTC_TIMESTAMP(6)").bind(operation).execute(&state.pool).await?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{parse_replies, read_resolve_command_result};

    #[tokio::test]
    async fn resolve_finishes_at_terminal_frame_without_waiting_for_tail_drain() {
        use axum::{
            Router,
            body::{Body, Bytes},
            routing::get,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app=Router::new().route("/command",get(||async {
            Body::from_stream(async_stream::stream! {
                let frames="data: {\"type\":\"stdout\",\"text\":\"完整输出\"}\n\ndata: {\"type\":\"execution_complete\"}\n\n";
                // Split inside a multibyte codepoint as well as JSON lines.
                for chunk in frames.as_bytes().chunks(2) {
                    yield Ok::<_,std::convert::Infallible>(Bytes::copy_from_slice(chunk));
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                yield Ok(Bytes::from_static(b"\n"));
            })
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = reqwest::get(format!("{endpoint}/command")).await.unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            read_resolve_command_result(response),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, ("完整输出".into(), String::new(), 0));
        server.abort();
    }

    #[tokio::test]
    async fn resolve_rejects_a_stream_without_a_terminal_event() {
        use axum::{Router, routing::get};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/command",
            get(|| async { "data: {\"type\":\"stdout\",\"text\":\"partial\"}\n\n" }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = reqwest::get(format!("{endpoint}/command")).await.unwrap();
        assert!(read_resolve_command_result(response).await.is_err());
        server.abort();
    }

    #[test]
    fn execd_line_events_preserve_both_rpc_replies_without_newlines() {
        let replies = parse_replies(r#"{"jsonrpc":"2.0","id":"initialize","result":{}}{"jsonrpc":"2.0","id":"resolve","result":{"status":"complete"}}"#).unwrap();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[1]["id"], "resolve");
        assert_eq!(replies[1]["result"]["status"], "complete");
    }

    #[test]
    fn malformed_stdout_cannot_hide_a_host_bridge_frame() {
        assert!(parse_replies(r#"{"method":"host.resourceOperation"}not-json"#).is_err());
    }
}
