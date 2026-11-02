use agentx_runtime_contracts::{RuntimeResourceBindingV1, VaultSecretReferenceV1};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};

use super::output::tool_execution_output;
use super::{
    RuntimeWorker, WorkerExecution, provider_secret_header, runtime_call_fingerprint, stable_id,
};
use crate::egress::EgressRequestContext;
use crate::engine::ClaimedWorkerAttempt;

impl RuntimeWorker {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn call_mcp_tool(
        &self,
        claim: &ClaimedWorkerAttempt,
        endpoint: &str,
        tool_name: &str,
        input: Value,
        call_index: u32,
        secret: Option<&VaultSecretReferenceV1>,
        binding: &RuntimeResourceBindingV1,
    ) -> WorkerExecution {
        if matches!(&binding.configuration,agentx_runtime_contracts::RuntimeResourceConfigurationV1::Mcp{tool_policy:Some(policy),..} if policy.approval.is_some())
        {
            return WorkerExecution::failed(
                "TOOL_APPROVAL_AGENT_REQUIRED",
                "This protected tool must run as a concrete Agent Effect; direct MCP node calls are not permitted",
                false,
            );
        }

        let (legacy_sse, legacy_sse_session_key) = match &binding.configuration {
            agentx_runtime_contracts::RuntimeResourceConfigurationV1::Mcp {
                server_version_id,
                transport: agentx_runtime_contracts::RuntimeMcpTransportV2::Sse { .. },
                ..
            } => (
                true,
                Some(format!(
                    "standalone-attempt:{}:mcp-server:{server_version_id}",
                    claim.task.attempt_id
                )),
            ),
            _ => (false, None),
        };
        let result = self
            .call_agent_mcp_tool(
                claim,
                endpoint,
                tool_name,
                input,
                call_index,
                secret,
                binding,
                None,
                legacy_sse,
                legacy_sse_session_key.as_deref(),
            )
            .await
            .0;
        if let Some(session_key) = legacy_sse_session_key {
            self.provider.close_legacy_sse_session(&session_key).await;
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn call_agent_mcp_tool(
        &self,
        claim: &ClaimedWorkerAttempt,
        endpoint: &str,
        tool_name: &str,
        input: Value,
        call_index: u32,
        secret: Option<&VaultSecretReferenceV1>,
        binding: &RuntimeResourceBindingV1,
        existing_session: Option<&str>,
        legacy_sse: bool,
        legacy_sse_session_key: Option<&str>,
    ) -> (WorkerExecution, Option<String>) {
        let side_effect = match &binding.configuration {
            agentx_runtime_contracts::RuntimeResourceConfigurationV1::Mcp {
                side_effect, ..
            } => side_effect.as_str(),
            _ => "unknown",
        };
        let request = json!({
            "jsonrpc":"2.0",
            "id":claim.task.attempt_id,
            "method":"tools/call",
            "params":{"name":tool_name,"arguments":input},
            "sideEffect":side_effect,
        });
        let fingerprint = runtime_call_fingerprint("mcp_tool", &request);
        let call_id = stable_id(
            claim.task.attempt_id,
            format!("mcp_tool:{call_index}").as_bytes(),
        );
        let idempotency_key = format!("{}:mcp_tool:{call_index}", claim.task.attempt_id);
        match self
            .reserve_call(
                claim,
                call_id,
                "mcp_tool",
                &idempotency_key,
                &fingerprint,
                &request,
                call_index,
                Some(binding),
            )
            .await
        {
            Ok(Some(value)) => {
                return (
                    tool_execution_output(WorkerExecution::succeeded(value)),
                    existing_session.map(str::to_owned),
                );
            }
            Ok(None) => {}
            Err(result) => return (result, existing_session.map(str::to_owned)),
        }
        let credential = if let Some(reference) = secret {
            let Some(vault) = &self.vault else {
                return (
                    self.fail_call(
                        call_id,
                        "VAULT_UNAVAILABLE",
                        "Runtime Vault is not configured",
                        false,
                    )
                    .await,
                    existing_session.map(str::to_owned),
                );
            };
            match vault.read(reference).await {
                Ok(value) => Some(provider_secret_header(&value, "authorization")),
                Err(error) => {
                    return (
                        self.fail_call(call_id, "VAULT_UNAVAILABLE", error.to_string(), false)
                            .await,
                        existing_session.map(str::to_owned),
                    );
                }
            }
        } else {
            None
        };
        let mut session = existing_session.map(str::to_owned);
        if session.is_none() {
            let initialized = self
                .mcp_rpc(
                    claim,
                    endpoint,
                    credential.as_deref(),
                    None,
                    Some(json!(1)),
                    "initialize",
                    json!({
                        "protocolVersion":"2025-03-26",
                        "capabilities":{},
                        "clientInfo":{"name":"agentx-runtime-worker","version":"1.1"},
                    }),
                    legacy_sse,
                    legacy_sse_session_key,
                )
                .await;
            let (_, initialized_session) = match initialized {
                Ok(value) => value,
                Err(error) => {
                    return (
                        self.fail_call(call_id, "MCP_INITIALIZE_FAILED", error, false)
                            .await,
                        None,
                    );
                }
            };
            session = initialized_session;
            if let Err(error) = self
                .mcp_rpc(
                    claim,
                    endpoint,
                    credential.as_deref(),
                    session.as_deref(),
                    None,
                    "notifications/initialized",
                    json!({}),
                    legacy_sse,
                    legacy_sse_session_key,
                )
                .await
            {
                return (
                    self.fail_call(call_id, "MCP_INITIALIZE_FAILED", error, false)
                        .await,
                    session,
                );
            }
        }
        if let Err(error) =
            crate::agent_tool_approvals::mark_sent(&self.pool, claim, binding, call_index, call_id)
                .await
        {
            return (
                self.fail_call(
                    call_id,
                    "TOOL_EFFECT_AUTHORIZATION_DENIED",
                    error.to_string(),
                    false,
                )
                .await,
                session,
            );
        }
        let (mut payload, next_session) = match self
            .mcp_rpc(
                claim,
                endpoint,
                credential.as_deref(),
                session.as_deref(),
                Some(json!(claim.task.attempt_id)),
                "tools/call",
                json!({
                    "name":tool_name,
                    "arguments":request["params"]["arguments"].clone(),
                    "_meta":{"agentx/idempotencyKey":idempotency_key}
                }),
                legacy_sse,
                legacy_sse_session_key,
            )
            .await
        {
            Ok(value) => value,
            Err(error) => {
                return (
                    self.fail_call(call_id, "PROVIDER_OUTCOME_UNKNOWN", error, true)
                        .await,
                    session,
                );
            }
        };
        let response_artifact_id = crate::trace_artifact::externalize_runtime_call_response(
            &self.pool,
            &self.objects,
            claim,
            call_id,
            &payload,
        )
        .await;
        if let Some(artifact_id) = response_artifact_id {
            payload["artifactRefs"] = json!([artifact_id.to_string()]);
            payload["truncated"] = json!(true);
        }
        if let Err(error) = sqlx::query(
            "UPDATE runtime_calls SET status='succeeded',response_json=?,response_artifact_id=?,ended_at=UTC_TIMESTAMP(6) WHERE id=? AND status='sent'",
        )
        .bind(&payload)
        .bind(response_artifact_id)
        .bind(call_id)
        .execute(&self.pool)
        .await
        {
            return (WorkerExecution::failed("RUNTIME_CALL_COMMIT_FAILED", error.to_string(), true), next_session.or(session));
        }
        self.emit_runtime_call_trace(
            call_id,
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "succeeded",
            None,
            Some(&payload),
        )
        .await;
        (
            tool_execution_output(WorkerExecution::succeeded(payload)),
            next_session.or(session),
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn mcp_rpc(
        &self,
        claim: &ClaimedWorkerAttempt,
        endpoint: &str,
        credential: Option<&[u8]>,
        session: Option<&str>,
        id: Option<Value>,
        method: &str,
        params: Value,
        legacy_sse: bool,
        legacy_sse_session_key: Option<&str>,
    ) -> Result<(Value, Option<String>), String> {
        let mut envelope = json!({"jsonrpc":"2.0","method":method,"params":params});
        if let Some(id) = id {
            envelope["id"] = id;
        }
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert(
            "accept",
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        if let Some(session) = session.filter(|_| !legacy_sse) {
            headers.insert(
                "mcp-session-id",
                HeaderValue::from_str(session).map_err(|error| error.to_string())?,
            );
        }
        if let Some(credential) = credential {
            let value = HeaderValue::from_bytes(credential)
                .map_err(|_| "MCP credential is not a valid authorization header".to_owned())?;
            headers.insert("authorization", value);
        }
        let context =
            EgressRequestContext::execution(claim.task.tenant_id, claim.task.execution_id);
        let response = if legacy_sse {
            self.provider
                .legacy_sse_rpc(
                    legacy_sse_session_key.ok_or_else(|| {
                        "Legacy SSE requires an Agent Run scoped session key".to_owned()
                    })?,
                    endpoint,
                    context,
                    std::time::Duration::from_secs(300),
                    headers,
                    &envelope,
                )
                .await
        } else {
            self.provider
                .post_json(
                    endpoint,
                    context,
                    std::time::Duration::from_secs(300),
                    headers,
                    &envelope,
                )
                .await
        }
        .map_err(|error| format!("{error:?}"))?;
        let status = response.status;
        let next_session = response
            .headers
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .or_else(|| session.map(str::to_owned))
            .or_else(|| {
                legacy_sse
                    .then(|| legacy_sse_session_key.map(str::to_owned))
                    .flatten()
            });
        let bytes = response.body;
        if !status.is_success() {
            return Err(format!(
                "MCP HTTP {status}: {}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        if envelope.get("id").is_none() {
            return Ok((Value::Null, next_session));
        }
        let envelope = parse_mcp_response(&bytes)?;
        if let Some(error) = envelope.get("error") {
            return Err(format!("MCP RPC error: {error}"));
        }
        Ok((
            envelope.get("result").cloned().unwrap_or(Value::Null),
            next_session,
        ))
    }
}

fn parse_mcp_response(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > 1024 * 1024 {
        return Err("MCP response exceeds 1 MiB".into());
    }
    if let Ok(value) = serde_json::from_slice(bytes) {
        return Ok(value);
    }
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    text.lines()
        .rev()
        .filter_map(|line| line.strip_prefix("data:"))
        .find_map(|line| serde_json::from_str(line.trim()).ok())
        .ok_or_else(|| "MCP response is neither JSON nor an SSE JSON event".into())
}

#[cfg(test)]
mod tests {
    use super::parse_mcp_response;
    use serde_json::json;

    #[test]
    fn parses_json_and_streamable_http_sse_envelopes() {
        assert_eq!(
            parse_mcp_response(br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#).unwrap(),
            json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}})
        );
        assert_eq!(
            parse_mcp_response(
                b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":true}}\n\n"
            )
            .unwrap(),
            json!({"jsonrpc":"2.0","id":2,"result":{"ok":true}})
        );
    }
}
