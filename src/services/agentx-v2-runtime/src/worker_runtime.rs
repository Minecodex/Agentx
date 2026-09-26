use std::{collections::BTreeMap, sync::Arc};

use agentx_node_protocol::{Item, NodeCapability};
use agentx_runtime_contracts::{
    ContentHash, RuntimeObjectReferenceV1, RuntimeResourceBindingV1,
    RuntimeResourceConfigurationV1, RuntimeResourceKindV1, StorageDomain, WorkerResultStatusV1,
    WorkerResultV1,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bytes::Bytes;
use object_store::{ObjectStore, path::Path as ObjectPath};
use reqwest::{StatusCode, header::HeaderMap};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, Row};
use uuid::Uuid;

use crate::{
    egress::{EgressRequestContext, ProviderHttpClient},
    engine::ClaimedWorkerAttempt,
    vault::RuntimeVault,
    worker_support::{
        openai_chat_completions_endpoint, provider_secret_header, runtime_call_fingerprint,
        runtime_call_span_name, stable_id,
    },
};

#[path = "worker_runtime_agent_attachments.rs"]
mod agent_attachments;
#[path = "worker_runtime_agent_budget.rs"]
mod agent_budget;
#[path = "worker_runtime_agent_core.rs"]
mod agent_core;
#[path = "worker_runtime_agent_model.rs"]
mod agent_model;
#[path = "worker_runtime_agent_state.rs"]
mod agent_state;
#[path = "worker_runtime_agent_trace.rs"]
mod agent_trace;
#[path = "worker_runtime_builtin.rs"]
mod builtin;
mod mcp;
#[path = "worker_runtime_output.rs"]
mod output;
#[path = "worker_runtime_plugin.rs"]
pub(crate) mod plugin;
#[path = "worker_runtime_provider.rs"]
mod provider;

#[cfg(test)]
use output::system_prompt;
use output::{
    apply_model_price, finalize_rag_response, memory_execution_output,
    openai_chat_request_streaming, openai_execution_output, provider_usage_detail,
    rag_execution_output, rag_query_request, runtime_call_is_replayable, runtime_call_side_effect,
    sandbox_execution_output, tool_execution_output,
};

pub struct WorkerExecution {
    pub status: WorkerResultStatusV1,
    pub outputs: BTreeMap<String, Vec<Item>>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub retryable: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct WorkerProviderResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Clone, Debug)]
pub enum WorkerProviderError {
    Denied(String),
    Request {
        message: String,
        is_connect: bool,
    },
    /// The provider stream violated the SSE/protocol contract; never retried
    /// automatically (plan7 P7-B).
    Protocol(String),
}

#[async_trait::async_trait]
pub trait WorkerProvider: Send + Sync {
    async fn post_json(
        &self,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: std::time::Duration,
        headers: HeaderMap,
        body: &Value,
    ) -> Result<WorkerProviderResponse, WorkerProviderError>;

    async fn post_sandbox_manager_json(
        &self,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: std::time::Duration,
        headers: HeaderMap,
        body: &Value,
    ) -> Result<WorkerProviderResponse, WorkerProviderError> {
        self.post_json(endpoint, context, timeout, headers, body)
            .await
    }

    async fn request_json(
        &self,
        method: &str,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: std::time::Duration,
        headers: HeaderMap,
        body: Option<&Value>,
    ) -> Result<WorkerProviderResponse, WorkerProviderError> {
        if method == "POST" {
            self.post_json(
                endpoint,
                context,
                timeout,
                headers,
                body.unwrap_or(&Value::Null),
            )
            .await
        } else {
            Err(WorkerProviderError::Denied(format!(
                "HTTP method {method} is not supported by this provider"
            )))
        }
    }

    async fn legacy_sse_rpc(
        &self,
        _session_key: &str,
        _endpoint: &str,
        _context: EgressRequestContext,
        _timeout: std::time::Duration,
        _headers: HeaderMap,
        _body: &Value,
    ) -> Result<WorkerProviderResponse, WorkerProviderError> {
        Err(WorkerProviderError::Denied(
            "Legacy SSE is not supported by this provider".into(),
        ))
    }

    async fn close_legacy_sse_session(&self, _session_key: &str) {}

    /// Streams a JSON POST response body chunk by chunk (plan7 P7-B). The
    /// default rejects so test doubles and alternate providers stay valid
    /// until they opt into streaming.
    async fn post_json_stream(
        &self,
        _endpoint: &str,
        _context: EgressRequestContext,
        _timeout: std::time::Duration,
        _headers: HeaderMap,
        _body: &Value,
    ) -> Result<WorkerStreamResponse, WorkerProviderError> {
        Err(WorkerProviderError::Denied(
            "Streaming is not supported by this provider".into(),
        ))
    }
}

/// A streaming provider response: status and headers are buffered, the body
/// is consumed incrementally by the caller.
pub struct WorkerStreamResponse {
    pub status: StatusCode,
    pub response: reqwest::Response,
}

#[derive(Clone)]
enum RuntimeHttpTransport {
    Provider,
    ProviderStream(crate::worker_runtime_delta::ModelDeltaSink),
    SandboxManager,
}

impl WorkerExecution {
    fn succeeded(value: Value) -> Self {
        Self {
            status: WorkerResultStatusV1::Succeeded,
            outputs: BTreeMap::from([(
                "main".into(),
                vec![Item {
                    json: value,
                    ..Item::default()
                }],
            )]),
            error_code: None,
            error_message: None,
            retryable: None,
        }
    }

    fn failed(code: &str, message: impl Into<String>, outcome_unknown: bool) -> Self {
        Self {
            status: if outcome_unknown {
                WorkerResultStatusV1::OutcomeUnknown
            } else {
                WorkerResultStatusV1::Failed
            },
            outputs: BTreeMap::new(),
            error_code: Some(code.into()),
            error_message: Some(message.into()),
            retryable: None,
        }
    }

    fn cancelled(code: &str, message: impl Into<String>) -> Self {
        Self {
            status: WorkerResultStatusV1::Cancelled,
            outputs: BTreeMap::new(),
            error_code: Some(code.into()),
            error_message: Some(message.into()),
            retryable: Some(false),
        }
    }

    fn suspended(resume: Value) -> Self {
        Self {
            status: WorkerResultStatusV1::Suspended,
            outputs: BTreeMap::from([(
                "main".into(),
                vec![Item {
                    json: resume,
                    ..Item::default()
                }],
            )]),
            error_code: None,
            error_message: None,
            retryable: None,
        }
    }
}

#[derive(Clone)]
pub struct RuntimeWorker {
    pub(crate) pool: MySqlPool,
    pub(crate) provider: Arc<dyn WorkerProvider>,
    pub(crate) vault: Option<RuntimeVault>,
    pub(crate) objects: Arc<dyn ObjectStore>,
    pub(crate) plugin_slots: Arc<tokio::sync::Semaphore>,
    plugin_parallelism: usize,
    pub(crate) plugin_trace: plugin::PluginTraceSink,
    pub(crate) plugin_artifacts: Arc<plugin::PluginArtifactCache>,
    pub(crate) deltas: crate::worker_runtime_delta::ModelDeltaSink,
}

impl RuntimeWorker {
    pub fn new(pool: MySqlPool, objects: Arc<dyn ObjectStore>) -> anyhow::Result<Self> {
        let plugin_concurrency = std::env::var("AGENTX_PLUGIN_MAX_PROCESSES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(8_usize)
            .clamp(1, 64);
        let plugin_artifacts = Arc::new(plugin::PluginArtifactCache::from_env());
        let plugin_trace = plugin::PluginTraceSink::new(pool.clone(), objects.clone());
        let deltas = crate::worker_runtime_delta::ModelDeltaSink::start(pool.clone(), None);
        Ok(Self {
            plugin_trace,
            deltas,
            pool,
            provider: Arc::new(ProviderHttpClient::from_env(
                agentx_runtime_contracts::EgressRole::WorkflowWorker,
            )?),
            vault: RuntimeVault::from_env().ok(),
            objects,
            plugin_slots: Arc::new(tokio::sync::Semaphore::new(plugin_concurrency)),
            plugin_parallelism: plugin_concurrency,
            plugin_artifacts,
        })
    }

    #[doc(hidden)]
    #[must_use]
    pub fn new_with_provider(
        pool: MySqlPool,
        objects: Arc<dyn ObjectStore>,
        provider: Arc<dyn WorkerProvider>,
    ) -> Self {
        let plugin_artifacts = Arc::new(plugin::PluginArtifactCache::for_tests());
        let plugin_trace = plugin::PluginTraceSink::new(pool.clone(), objects.clone());
        Self {
            plugin_trace,
            deltas: crate::worker_runtime_delta::ModelDeltaSink::disabled(),
            pool,
            provider,
            vault: RuntimeVault::from_env().ok(),
            objects,
            plugin_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            plugin_parallelism: 8,
            plugin_artifacts,
        }
    }

    /// Attaches a live delta sink plus the redis client used for SSE wakeup
    /// publication (plan7 P7-B); production workers call this after
    /// construction.
    pub fn with_delta_sink(mut self, pool: MySqlPool, redis: Option<redis::Client>) -> Self {
        self.deltas = crate::worker_runtime_delta::ModelDeltaSink::start(pool, redis);
        self
    }

    pub fn plugin_parallelism(&self) -> usize {
        self.plugin_parallelism
    }

    pub async fn execute(&self, claim: &ClaimedWorkerAttempt) -> WorkerExecution {
        self.emit_resolved_parameters(claim).await;
        let execution = self.execute_claim(claim).await;
        // Deltas must land before the attempt settles; the terminal event
        // would otherwise cut the SSE stream ahead of pending frames.
        self.deltas.flush().await;
        execution
    }

    async fn execute_claim(&self, claim: &ClaimedWorkerAttempt) -> WorkerExecution {
        match claim.task.capability {
            NodeCapability::Builtin => self.execute_builtin(claim),
            NodeCapability::PluginNodejs => plugin::execute(self, claim).await,
            NodeCapability::Agent => self.execute_agent_core(claim).await,
            NodeCapability::Model | NodeCapability::Sandbox => self.execute_resource(claim).await,
            NodeCapability::McpTool
            | NodeCapability::Rag
            | NodeCapability::Memory
            | NodeCapability::Skill => WorkerExecution::failed(
                "RESOURCE_CAPABILITY_NODE_REMOVED",
                "This capability is executable only through an Agent resource slot",
                false,
            ),
        }
    }

    #[must_use]
    pub fn object_store(&self) -> Arc<dyn ObjectStore> {
        self.objects.clone()
    }

    pub async fn build_result(
        &self,
        claim: &ClaimedWorkerAttempt,
        execution: WorkerExecution,
    ) -> anyhow::Result<WorkerResultV1> {
        crate::trace_artifact::externalize_attempt_input(&self.pool, &self.objects, claim).await;
        let encoded = agentx_runtime_contracts::canonical_bytes(&execution.outputs)?;
        let (outputs, output_object) =
            if encoded.len() > agentx_runtime_contracts::INLINE_RESULT_LIMIT_BYTES as usize {
                (
                    BTreeMap::new(),
                    Some(self.persist_attempt_output(claim, encoded).await?),
                )
            } else {
                (execution.outputs, None)
            };
        let result_hash = crate::engine::worker_result_hash(
            execution.status,
            &outputs,
            output_object.as_ref(),
            execution.error_code.as_deref(),
            execution.error_message.as_deref(),
            execution.retryable,
            None,
        )?;
        Ok(WorkerResultV1 {
            protocol_version: 1,
            attempt_id: claim.task.attempt_id,
            worker_id: claim.lease.worker_id,
            fencing_token: claim.lease.fencing_token,
            status: execution.status,
            result_hash,
            outputs,
            output_object,
            error_code: execution.error_code,
            error_message: execution.error_message,
            retryable: execution.retryable,
            partial_output_object: None,
        })
    }

    async fn persist_attempt_output(
        &self,
        claim: &ClaimedWorkerAttempt,
        encoded: Vec<u8>,
    ) -> anyhow::Result<RuntimeObjectReferenceV1> {
        let object_id = stable_id(claim.task.attempt_id, b"worker-result-object");
        let content_hash =
            ContentHash::parse(format!("sha256:{:x}", Sha256::digest(encoded.as_slice())))?;
        let object_key =
            RuntimeObjectReferenceV1::canonical_key(claim.task.tenant_id, object_id, &content_hash);
        if let Some(row) = sqlx::query(
            "SELECT content_hash,size_bytes,media_type,status FROM runtime_objects WHERE tenant_id=? AND object_id=?",
        )
        .bind(claim.task.tenant_id)
        .bind(object_id)
        .fetch_optional(&self.pool)
        .await?
        {
            anyhow::ensure!(
                row.try_get::<String, _>("content_hash")? == content_hash.as_str()
                    && row.try_get::<u64, _>("size_bytes")? == encoded.len() as u64
                    && row.try_get::<String, _>("media_type")? == "application/json"
                    && row.try_get::<String, _>("status")? == "ready",
                "attempt output object identity conflicts with existing content"
            );
            let artifact = RuntimeObjectReferenceV1 {
                tenant_id: claim.task.tenant_id,
                storage_domain: StorageDomain::Runtime,
                object_id,
                object_key,
                content_hash,
                size_bytes: encoded.len() as u64,
                media_type: "application/json".into(),
            };
            crate::trace_artifact::register_artifact(
                &self.pool,
                &artifact,
                claim.task.execution_id,
                claim.task.node_execution_id,
                agentx_runtime_contracts::TraceContentKindV1::AttemptOutput,
            )
            .await?;
            return Ok(artifact);
        }
        let path = ObjectPath::from(object_key.clone());
        self.objects
            .put(&path, bytes::Bytes::from(encoded.clone()).into())
            .await?;
        let request_hash = agentx_runtime_contracts::content_hash(&json!({
            "attemptId":claim.task.attempt_id,
            "contentHash":content_hash,
            "sizeBytes":encoded.len(),
        }))?;
        let inserted = sqlx::query(
            "INSERT INTO runtime_objects(object_id,tenant_id,object_key,content_hash,size_bytes,media_type,status,idempotency_key,request_hash,temporary_expires_at,ready_at) VALUES(?,?,?,?,?,'application/json','ready',?,?,UTC_TIMESTAMP(6),UTC_TIMESTAMP(6))",
        )
        .bind(object_id)
        .bind(claim.task.tenant_id)
        .bind(&object_key)
        .bind(content_hash.as_str())
        .bind(encoded.len() as u64)
        .bind(format!("worker-result:{}", claim.task.attempt_id))
        .bind(request_hash.as_str())
        .execute(&self.pool)
        .await;
        if let Err(error) = inserted {
            let _ = self.objects.delete(&path).await;
            return Err(error.into());
        }
        let artifact = RuntimeObjectReferenceV1 {
            tenant_id: claim.task.tenant_id,
            storage_domain: StorageDomain::Runtime,
            object_id,
            object_key,
            content_hash,
            size_bytes: encoded.len() as u64,
            media_type: "application/json".into(),
        };
        crate::trace_artifact::register_artifact(
            &self.pool,
            &artifact,
            claim.task.execution_id,
            claim.task.node_execution_id,
            agentx_runtime_contracts::TraceContentKindV1::AttemptOutput,
        )
        .await?;
        Ok(artifact)
    }

    fn execute_builtin(&self, claim: &ClaimedWorkerAttempt) -> WorkerExecution {
        builtin::execute(claim)
    }

    async fn execute_agent_core(&self, claim: &ClaimedWorkerAttempt) -> WorkerExecution {
        agent_core::execute(self, claim).await
    }

    async fn execute_resource(&self, claim: &ClaimedWorkerAttempt) -> WorkerExecution {
        self.execute_binding(claim, capability_resource_kind(&claim.task.capability))
            .await
    }

    async fn execute_binding(
        &self,
        claim: &ClaimedWorkerAttempt,
        expected_kind: RuntimeResourceKindV1,
    ) -> WorkerExecution {
        let Some(binding) = select_binding(claim, expected_kind) else {
            return WorkerExecution::failed(
                "RUNTIME_RESOURCE_BINDING_MISSING",
                "Node has no exact immutable Runtime Resource Binding",
                false,
            );
        };
        let input = if expected_kind == RuntimeResourceKindV1::Model {
            match self.model_input(claim).await {
                Ok(input) => input,
                Err(error) => {
                    return WorkerExecution::failed(
                        "MODEL_PROMPT_OBJECT_INVALID",
                        error.to_string(),
                        false,
                    );
                }
            }
        } else {
            first_input(claim).unwrap_or(Value::Null)
        };
        self.execute_provider_call(claim, binding, input, 0).await
    }

    async fn model_input(&self, claim: &ClaimedWorkerAttempt) -> anyhow::Result<Value> {
        let target = first_input(claim).unwrap_or(Value::Null);
        let Some(prompt_object_id) = claim
            .node_parameters
            .get("promptObjectId")
            .or_else(|| target.get("promptObjectId"))
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
        else {
            return Ok(target);
        };
        let bytes = self
            .load_runtime_object(claim.task.tenant_id, prompt_object_id)
            .await?;
        let prompt = serde_json::from_slice::<Value>(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        Ok(json!({
            "prompt": prompt,
            "target": target,
            "promptObjectId": prompt_object_id,
        }))
    }

    async fn load_runtime_object(
        &self,
        tenant_id: Uuid,
        object_id: Uuid,
    ) -> anyhow::Result<bytes::Bytes> {
        let row = sqlx::query(
            "SELECT CAST(object_key AS CHAR CHARACTER SET utf8mb4) AS object_key,content_hash,size_bytes FROM runtime_objects WHERE tenant_id=? AND object_id=? AND status='ready'",
        )
        .bind(tenant_id)
        .bind(object_id)
        .fetch_one(&self.pool)
        .await?;
        let key: String = row.try_get("object_key")?;
        let expected_hash: String = row.try_get("content_hash")?;
        let expected_size: u64 = row.try_get("size_bytes")?;
        let bytes = self
            .objects
            .get(&ObjectPath::from(key))
            .await?
            .bytes()
            .await?;
        anyhow::ensure!(
            bytes.len() as u64 == expected_size,
            "Runtime object size does not match metadata"
        );
        let actual_hash = format!("sha256:{:x}", Sha256::digest(&bytes));
        anyhow::ensure!(
            actual_hash == expected_hash,
            "Runtime object hash does not match metadata"
        );
        Ok(bytes)
    }

    async fn execute_provider_call(
        &self,
        claim: &ClaimedWorkerAttempt,
        binding: &RuntimeResourceBindingV1,
        input: Value,
        call_index: u32,
    ) -> WorkerExecution {
        let structured_model = claim.node_type == "model"
            && claim
                .node_parameters
                .get("responseMode")
                .and_then(Value::as_str)
                == Some("json_schema");
        if let RuntimeResourceConfigurationV1::Model {
            provider,
            endpoint,
            model,
            price,
            credential,
            ..
        } = &binding.configuration
            && provider == "openai_compatible"
        {
            let endpoint = openai_chat_completions_endpoint(endpoint);
            let streaming = claim
                .node_parameters
                .get("stream")
                .and_then(Value::as_str)
                .map(|value| value != "false")
                .unwrap_or(true);
            let request = openai_chat_request_streaming(claim, model, price, &input, streaming);
            let execution = if streaming {
                self.call_http_stream(
                    claim,
                    "model",
                    &endpoint,
                    request,
                    call_index,
                    credential.as_ref(),
                    "authorization",
                    Some(binding),
                )
                .await
            } else {
                self.call_http(
                    claim,
                    "model",
                    &endpoint,
                    request,
                    call_index,
                    credential.as_ref(),
                    "authorization",
                    Some(binding),
                )
                .await
            };
            return openai_execution_output(execution, &claim.node_parameters);
        }
        if structured_model {
            return WorkerExecution::failed(
                "MODEL_STRUCTURED_OUTPUT_UNSUPPORTED",
                "The selected model provider does not support native JSON Schema responses",
                false,
            );
        }
        let (kind, endpoint, request, secret, secret_header) = match &binding.configuration {
            RuntimeResourceConfigurationV1::Model {
                endpoint,
                model,
                price,
                credential,
                ..
            } => (
                "model",
                endpoint.clone(),
                json!({"model":model,"input":input,"priceVersion":price.version_id,"parameters":claim.node_parameters}),
                credential.as_ref(),
                "authorization",
            ),
            RuntimeResourceConfigurationV1::Mcp {
                transport,
                tool_name,
                credential,
                ..
            } => {
                let endpoint = match transport {
                    agentx_runtime_contracts::RuntimeMcpTransportV2::StreamableHttp {
                        endpoint,
                    }
                    | agentx_runtime_contracts::RuntimeMcpTransportV2::Sse { endpoint } => endpoint,
                    agentx_runtime_contracts::RuntimeMcpTransportV2::Stdio { .. } => {
                        return WorkerExecution::failed(
                            "MCP_STDIO_PROCESS_UNAVAILABLE",
                            "stdio MCP requires a Sandbox Process Session",
                            false,
                        );
                    }
                };
                return self
                    .call_mcp_tool(
                        claim,
                        endpoint,
                        tool_name,
                        input,
                        call_index,
                        credential.as_ref(),
                        binding,
                    )
                    .await;
            }
            RuntimeResourceConfigurationV1::Rag {
                provider,
                endpoint,
                namespace,
                index_version,
                credential,
            } => {
                let operation = claim
                    .node_parameters
                    .get("operation")
                    .and_then(Value::as_str)
                    .unwrap_or("query");
                let payload = claim.node_parameters.get("input").cloned().unwrap_or(input);
                let (path, body, secret_header) = match rag_query_request(
                    provider,
                    operation,
                    namespace,
                    index_version,
                    &payload,
                ) {
                    Ok(built) => built,
                    Err(failed) => return failed,
                };
                (
                    "rag",
                    format!("{}/{}", endpoint.trim_end_matches('/'), path),
                    body,
                    credential.as_ref(),
                    secret_header,
                )
            }
            RuntimeResourceConfigurationV1::Memory {
                endpoint,
                namespace,
                memory_version,
                credential,
                ..
            } => {
                let operation = claim
                    .node_parameters
                    .get("operation")
                    .and_then(Value::as_str)
                    .unwrap_or("search");
                let input = claim.node_parameters.get("input").cloned().unwrap_or(input);
                let (path, request) = if operation == "add" || operation == "write" {
                    let content = input.get("text").cloned().unwrap_or_else(|| input.clone());
                    (
                        "memories",
                        json!({
                            "messages":[{"role":"user","content":content}],
                            "user_id":namespace,
                            "version":memory_version
                        }),
                    )
                } else {
                    let query = input.get("query").cloned().unwrap_or_else(|| input.clone());
                    let top_k = input.get("top_k").and_then(Value::as_u64).unwrap_or(5);
                    (
                        "search",
                        json!({"query":query,"filters":{"user_id":namespace},"top_k":top_k}),
                    )
                };
                (
                    "memory",
                    format!("{}/{}", endpoint.trim_end_matches('/'), path),
                    request,
                    credential.as_ref(),
                    "authorization",
                )
            }
            RuntimeResourceConfigurationV1::SandboxProfile { .. } => {
                let Ok(manager) = std::env::var("AGENTX_SANDBOX_MANAGER_ENDPOINT") else {
                    return WorkerExecution::failed(
                        "SANDBOX_MANAGER_UNAVAILABLE",
                        "AGENTX_SANDBOX_MANAGER_ENDPOINT is not configured",
                        false,
                    );
                };
                let endpoint = format!(
                    "{}/internal/runtime/v1/sandboxes:execute",
                    manager.trim_end_matches('/')
                );
                let execution = self
                    .call_sandbox_manager_http(
                        claim,
                        &endpoint,
                        serde_json::to_value(crate::sandbox::SandboxExecuteRequestV1 {
                            api_version: 1,
                            tenant_id: claim.task.tenant_id,
                            execution_id: claim.task.execution_id,
                            node_execution_id: claim.task.node_execution_id,
                            attempt_id: claim.task.attempt_id,
                            worker_id: claim.lease.worker_id,
                            fencing_token: claim.lease.fencing_token,
                            idempotency_key: format!("sandbox:execute:{}", claim.task.attempt_id),
                            profile: binding.clone(),
                            input,
                            parameters: claim.node_parameters.clone(),
                        })
                        .unwrap_or(Value::Null),
                        call_index,
                        Some(binding),
                    )
                    .await;
                return sandbox_execution_output(execution, &claim.node_parameters);
            }
            _ => {
                return WorkerExecution::failed(
                    "RUNTIME_RESOURCE_NOT_EXECUTABLE",
                    "Runtime Resource configuration is not executable",
                    false,
                );
            }
        };
        let execution = self
            .call_http(
                claim,
                kind,
                &endpoint,
                request,
                call_index,
                secret,
                secret_header,
                Some(binding),
            )
            .await;
        match binding.resource_kind {
            RuntimeResourceKindV1::Mcp => tool_execution_output(execution),
            RuntimeResourceKindV1::Rag => {
                let provider = match &binding.configuration {
                    RuntimeResourceConfigurationV1::Rag { provider, .. } => provider.clone(),
                    _ => "lightrag".into(),
                };
                rag_execution_output(finalize_rag_response(&provider, execution))
            }
            RuntimeResourceKindV1::Memory => memory_execution_output(execution),
            _ => execution,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call_http(
        &self,
        claim: &ClaimedWorkerAttempt,
        kind: &str,
        endpoint: &str,
        request: Value,
        call_index: u32,
        secret: Option<&agentx_runtime_contracts::VaultSecretReferenceV1>,
        secret_header: &'static str,
        binding: Option<&RuntimeResourceBindingV1>,
    ) -> WorkerExecution {
        self.call_http_with_transport(
            claim,
            kind,
            endpoint,
            request,
            call_index,
            secret,
            secret_header,
            binding,
            None,
            RuntimeHttpTransport::Provider,
        )
        .await
    }

    /// Streaming variant (plan7 P7-B): the provider body is consumed as an
    /// SSE token stream while deltas are emitted live; the aggregated
    /// response keeps the buffered settlement/replay contract identical.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call_http_stream(
        &self,
        claim: &ClaimedWorkerAttempt,
        kind: &str,
        endpoint: &str,
        request: Value,
        call_index: u32,
        secret: Option<&agentx_runtime_contracts::VaultSecretReferenceV1>,
        secret_header: &'static str,
        binding: Option<&RuntimeResourceBindingV1>,
    ) -> WorkerExecution {
        self.call_http_with_transport(
            claim,
            kind,
            endpoint,
            request,
            call_index,
            secret,
            secret_header,
            binding,
            None,
            RuntimeHttpTransport::ProviderStream(self.deltas.clone()),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call_http_effect(
        &self,
        claim: &ClaimedWorkerAttempt,
        kind: &str,
        endpoint: &str,
        request: Value,
        call_index: u32,
        effect_idempotency_key: &str,
        secret: Option<&agentx_runtime_contracts::VaultSecretReferenceV1>,
        secret_header: &'static str,
        binding: Option<&RuntimeResourceBindingV1>,
    ) -> WorkerExecution {
        self.call_http_with_transport(
            claim,
            kind,
            endpoint,
            request,
            call_index,
            secret,
            secret_header,
            binding,
            Some(effect_idempotency_key),
            RuntimeHttpTransport::Provider,
        )
        .await
    }

    /// Effect-keyed streaming call for agent model turns (plan7 P7-B).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call_http_effect_stream(
        &self,
        claim: &ClaimedWorkerAttempt,
        kind: &str,
        endpoint: &str,
        request: Value,
        call_index: u32,
        effect_idempotency_key: &str,
        secret: Option<&agentx_runtime_contracts::VaultSecretReferenceV1>,
        secret_header: &'static str,
        binding: Option<&RuntimeResourceBindingV1>,
    ) -> WorkerExecution {
        self.call_http_with_transport(
            claim,
            kind,
            endpoint,
            request,
            call_index,
            secret,
            secret_header,
            binding,
            Some(effect_idempotency_key),
            RuntimeHttpTransport::ProviderStream(self.deltas.clone()),
        )
        .await
    }

    async fn call_sandbox_manager_http(
        &self,
        claim: &ClaimedWorkerAttempt,
        endpoint: &str,
        request: Value,
        call_index: u32,
        binding: Option<&RuntimeResourceBindingV1>,
    ) -> WorkerExecution {
        self.call_http_with_transport(
            claim,
            "sandbox",
            endpoint,
            request,
            call_index,
            None,
            "authorization",
            binding,
            None,
            RuntimeHttpTransport::SandboxManager,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn call_http_with_transport(
        &self,
        claim: &ClaimedWorkerAttempt,
        kind: &str,
        endpoint: &str,
        request: Value,
        call_index: u32,
        secret: Option<&agentx_runtime_contracts::VaultSecretReferenceV1>,
        secret_header: &'static str,
        binding: Option<&RuntimeResourceBindingV1>,
        effect_idempotency_key: Option<&str>,
        transport: RuntimeHttpTransport,
    ) -> WorkerExecution {
        let fingerprint = runtime_call_fingerprint(kind, &request);
        let idempotency_key = effect_idempotency_key.map_or_else(
            || format!("{}:{kind}:{call_index}", claim.task.attempt_id),
            |identity| format!("agent-effect:{:x}", Sha256::digest(identity.as_bytes())),
        );
        let call_id = effect_idempotency_key.map_or_else(
            || {
                stable_id(
                    claim.task.attempt_id,
                    format!("{kind}:{call_index}").as_bytes(),
                )
            },
            |_| stable_id(claim.task.tenant_id, idempotency_key.as_bytes()),
        );
        match self
            .reserve_call(
                claim,
                call_id,
                kind,
                &idempotency_key,
                &fingerprint,
                &request,
                call_index,
                binding,
            )
            .await
        {
            Ok(Some(value)) => return WorkerExecution::succeeded(value),
            Ok(None) => {}
            Err(result) => return result,
        }
        let mut endpoint = endpoint.to_owned();
        let mut headers = HeaderMap::new();
        let mut secret_redactions = Vec::new();
        headers.insert(
            "Idempotency-Key",
            reqwest::header::HeaderValue::from_str(&idempotency_key)
                .expect("UUID-based idempotency key is a valid header"),
        );
        if kind == "http" {
            if let Some(values) = request.get("headers").and_then(Value::as_object) {
                for (name, value) in values {
                    let Ok(name) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) else {
                        return self
                            .fail_call(
                                call_id,
                                "HTTP_HEADER_INVALID",
                                format!("Invalid HTTP header name: {name}"),
                                false,
                            )
                            .await;
                    };
                    let Some(value) = value.as_str() else {
                        return self
                            .fail_call(
                                call_id,
                                "HTTP_HEADER_INVALID",
                                format!("HTTP header {name} must be a string"),
                                false,
                            )
                            .await;
                    };
                    let Ok(value) = reqwest::header::HeaderValue::from_str(value) else {
                        return self
                            .fail_call(
                                call_id,
                                "HTTP_HEADER_INVALID",
                                format!("Invalid value for HTTP header {name}"),
                                false,
                            )
                            .await;
                    };
                    headers.insert(name, value);
                }
            }
        }
        if let Some(reference) = secret {
            let Some(vault) = &self.vault else {
                return self
                    .fail_call(
                        call_id,
                        "VAULT_UNAVAILABLE",
                        "Runtime Vault is not configured",
                        false,
                    )
                    .await;
            };
            match vault.read(reference).await {
                Ok(value) => {
                    secret_redactions.extend(secret_fragments(&value));
                    let header_secret = provider_secret_header(&value, secret_header);
                    secret_redactions.push(header_secret.clone());
                    match reqwest::header::HeaderValue::from_bytes(&header_secret) {
                        Ok(value) => {
                            headers.insert(
                                reqwest::header::HeaderName::from_static(secret_header),
                                value,
                            );
                        }
                        Err(_) => {
                            return self
                                .fail_call(
                                    call_id,
                                    "VAULT_SECRET_INVALID",
                                    "Vault value is not a valid authorization header",
                                    false,
                                )
                                .await;
                        }
                    }
                }
                Err(error) => {
                    return self
                        .fail_call(call_id, "VAULT_UNAVAILABLE", error.to_string(), false)
                        .await;
                }
            }
        }
        if kind == "http"
            && let Some(RuntimeResourceBindingV1 {
                configuration:
                    RuntimeResourceConfigurationV1::Credential {
                        credential_type,
                        secret,
                        ..
                    },
                ..
            }) = binding
        {
            let Some(vault) = &self.vault else {
                return self
                    .fail_call(
                        call_id,
                        "VAULT_UNAVAILABLE",
                        "Runtime Vault is not configured",
                        false,
                    )
                    .await;
            };
            let value = match vault.read(secret).await {
                Ok(value) => value,
                Err(error) => {
                    return self
                        .fail_call(call_id, "VAULT_UNAVAILABLE", error.to_string(), false)
                        .await;
                }
            };
            secret_redactions.extend(secret_fragments(&value));
            let endpoint_before_credential = endpoint.clone();
            let headers_before_credential = headers.clone();
            if let Err(message) = apply_http_credential(
                &mut endpoint,
                &mut headers,
                credential_type,
                &value,
                request.get("apiKeyPlacement"),
            ) {
                return self
                    .fail_call(call_id, "HTTP_CREDENTIAL_INVALID", message, false)
                    .await;
            }
            if endpoint != endpoint_before_credential {
                secret_redactions.push(endpoint.as_bytes().to_vec());
            }
            for (name, value) in &headers {
                if headers_before_credential.get(name) != Some(value) {
                    secret_redactions.push(value.as_bytes().to_vec());
                }
            }
        }
        if let Err(error) =
            sqlx::query("UPDATE runtime_calls SET status='sent' WHERE id=? AND status='reserved'")
                .bind(call_id)
                .execute(&self.pool)
                .await
        {
            return WorkerExecution::failed(
                "RUNTIME_CALL_STATE_UNAVAILABLE",
                error.to_string(),
                false,
            );
        }
        let context =
            EgressRequestContext::execution(claim.task.tenant_id, claim.task.execution_id);
        let response = match match transport {
            RuntimeHttpTransport::ProviderStream(delta_sink) => {
                let provider = self.provider.clone();
                let endpoint = endpoint.clone();
                let request = request.clone();
                let timeout_ms = claim.timeout_ms;
                let claim_tenant = claim.task.tenant_id;
                let invocation_id = claim.invocation_id;
                let attempt_id = claim.task.attempt_id;
                Box::pin(async move {
                    let stream = provider
                        .post_json_stream(
                            &endpoint,
                            context,
                            std::time::Duration::from_millis(timeout_ms),
                            headers,
                            &request,
                        )
                        .await?;
                    let status = stream.status;
                    let mut sink = delta_sink.clone();
                    let aggregated = output::aggregate_openai_sse_stream(
                        stream.response,
                        &mut move |text: &str, reasoning: Option<&str>| {
                            sink.emit(claim_tenant, invocation_id, attempt_id, "", text, reasoning);
                        },
                    )
                    .await;
                    match aggregated {
                        Ok(value) => Ok(WorkerProviderResponse {
                            status,
                            headers: HeaderMap::new(),
                            body: bytes::Bytes::from(
                                serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()),
                            ),
                        }),
                        Err(message) => Err(WorkerProviderError::Protocol(message)),
                    }
                })
            }
            RuntimeHttpTransport::Provider if kind == "http" => self.provider.request_json(
                request
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET"),
                &endpoint,
                context,
                std::time::Duration::from_millis(claim.timeout_ms),
                headers,
                (!matches!(
                    request.get("method").and_then(Value::as_str),
                    Some("GET" | "DELETE")
                ))
                .then(|| request.get("body").unwrap_or(&Value::Null)),
            ),
            RuntimeHttpTransport::Provider => self.provider.post_json(
                &endpoint,
                context,
                std::time::Duration::from_millis(claim.timeout_ms),
                headers,
                &request,
            ),
            RuntimeHttpTransport::SandboxManager => self.provider.post_sandbox_manager_json(
                &endpoint,
                context,
                std::time::Duration::from_millis(claim.timeout_ms),
                headers,
                &request,
            ),
        }
        .await
        {
            Ok(response) => response,
            Err(WorkerProviderError::Denied(message)) => {
                return self
                    .fail_call(
                        call_id,
                        "PROVIDER_ENDPOINT_DENIED",
                        redact_secret_text(&message, &secret_redactions),
                        false,
                    )
                    .await;
            }
            Err(WorkerProviderError::Protocol(message)) => {
                return self
                    .fail_call(
                        call_id,
                        "MODEL_STREAM_PROTOCOL_ERROR",
                        redact_secret_text(&message, &secret_redactions),
                        false,
                    )
                    .await;
            }
            Err(WorkerProviderError::Request {
                message,
                is_connect,
            }) => {
                let unknown = !is_connect;
                return self
                    .fail_call(
                        call_id,
                        if unknown {
                            "PROVIDER_OUTCOME_UNKNOWN"
                        } else {
                            "PROVIDER_UNAVAILABLE"
                        },
                        redact_secret_text(&message, &secret_redactions),
                        unknown,
                    )
                    .await;
            }
        };
        let status = response.status;
        let provider_request_id = response
            .headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or("application/octet-stream")
            .trim()
            .to_owned();
        let response_body = redact_secret_bytes(&response.body, &secret_redactions);
        let binary_response = kind == "http"
            && content_type != "application/json"
            && !content_type.ends_with("+json")
            && !content_type.starts_with("text/");
        let binary_artifact = if binary_response {
            crate::trace_artifact::externalize_http_binary(
                &self.pool,
                &self.objects,
                claim,
                call_id,
                response_body.clone(),
                &content_type,
            )
            .await
        } else {
            None
        };
        let mut payload = match serde_json::from_slice::<Value>(&response_body) {
            Ok(value) if !binary_response => value,
            Ok(_) if binary_response => Value::Null,
            Ok(value) => value,
            Err(_error) if kind == "http" && !binary_response => {
                Value::String(String::from_utf8_lossy(&response_body).into_owned())
            }
            _ if binary_response => Value::Null,
            Err(error) => {
                return self
                    .fail_call(
                        call_id,
                        "PROVIDER_RESPONSE_INVALID",
                        error.to_string(),
                        false,
                    )
                    .await;
            }
        };
        if !status.is_success() {
            return self
                .fail_call(
                    call_id,
                    "PROVIDER_REJECTED",
                    format!("HTTP {status}: {payload}"),
                    false,
                )
                .await;
        }
        let raw_provider_response = if binary_response {
            json!({"binary":true,"contentType":content_type,"sizeBytes":response_body.len()})
        } else {
            payload.clone()
        };
        let usage_estimated = payload
            .get("usage_estimated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (input_tokens, output_tokens, cost_micros, cost_currency) =
            if let Some(RuntimeResourceBindingV1 {
                configuration: RuntimeResourceConfigurationV1::Model { price, .. },
                ..
            }) = binding
            {
                match apply_model_price(&mut payload, price) {
                    Ok((input, output, cost)) => {
                        (input, output, cost, Some(price.currency.as_str()))
                    }
                    Err(message) => {
                        return self
                            .fail_call(call_id, "MODEL_PRICE_SNAPSHOT_INVALID", message, false)
                            .await;
                    }
                }
            } else {
                let (input, output, cost) = provider_usage_detail(&payload);
                (input, output, cost, None)
            };
        let response_artifact_id = if let Some(artifact) = &binary_artifact {
            Some(artifact.object_id)
        } else {
            crate::trace_artifact::externalize_runtime_call_response(
                &self.pool,
                &self.objects,
                claim,
                call_id,
                &raw_provider_response,
            )
            .await
        };
        if matches!(kind, "sandbox" | "rag" | "memory")
            && let Some(artifact_id) = response_artifact_id
        {
            payload["artifactRefs"] = json!([artifact_id.to_string()]);
            payload["truncated"] = json!(true);
        }
        if kind == "http" {
            let response_headers = response
                .headers
                .iter()
                .filter(|(name, _)| {
                    !matches!(
                        name.as_str(),
                        "set-cookie" | "authorization" | "proxy-authorization"
                    )
                })
                .filter_map(|(name, value)| {
                    value.to_str().ok().map(|value| {
                        (
                            name.as_str().to_owned(),
                            Value::String(redact_secret_text(value, &secret_redactions)),
                        )
                    })
                })
                .collect::<serde_json::Map<_, _>>();
            let files = binary_artifact.as_ref().map(|artifact| {
                let sha256 = artifact.content_hash.as_str().trim_start_matches("sha256:");
                json!([{"artifactId":artifact.object_id,"fileName":"response.bin","contentType":artifact.media_type,"sizeBytes":artifact.size_bytes,"sha256":sha256}])
            }).unwrap_or_else(|| json!([]));
            let body = if binary_response {
                String::new()
            } else {
                String::from_utf8_lossy(&response_body).into_owned()
            };
            payload = json!({"statusCode":status.as_u16(),"headers":response_headers,"body":body,"files":files});
        }
        if let Err(error) = sqlx::query(
            "UPDATE runtime_calls SET status='succeeded',provider_request_id=?,response_json=?,response_artifact_id=?,input_tokens=?,output_tokens=?,cost_micros=?,cost_currency=?,usage_estimated=?,ended_at=UTC_TIMESTAMP(6) WHERE id=? AND status='sent'",
        )
        .bind(provider_request_id)
        .bind(&payload)
        .bind(response_artifact_id)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(cost_micros)
        .bind(cost_currency)
        .bind(usage_estimated)
        .bind(call_id)
        .execute(&self.pool)
        .await
        {
            return WorkerExecution::failed("RUNTIME_CALL_COMMIT_FAILED", error.to_string(), true);
        }
        self.emit_runtime_call_trace(
            call_id,
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            "succeeded",
            None,
            Some(&raw_provider_response),
        )
        .await;
        WorkerExecution::succeeded(payload)
    }

    #[allow(clippy::too_many_arguments)]
    async fn reserve_call(
        &self,
        claim: &ClaimedWorkerAttempt,
        call_id: Uuid,
        kind: &str,
        idempotency_key: &str,
        fingerprint: &str,
        request: &Value,
        call_index: u32,
        binding: Option<&RuntimeResourceBindingV1>,
    ) -> Result<Option<Value>, WorkerExecution> {
        let mut tx = self.pool.begin().await.map_err(|error| {
            WorkerExecution::failed("RUNTIME_CALL_STATE_UNAVAILABLE", error.to_string(), false)
        })?;
        if let Some(row) = sqlx::query(
            "SELECT id,status,side_effect,request_fingerprint,response_json,error_code,error_message FROM runtime_calls WHERE tenant_id=? AND idempotency_key=? FOR UPDATE",
        )
        .bind(claim.task.tenant_id)
        .bind(idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_UNAVAILABLE", error.to_string(), false))?
        {
            if row.try_get::<String, _>("request_fingerprint").map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_INVALID", error.to_string(), false))? != fingerprint {
                return Err(WorkerExecution::failed("RUNTIME_CALL_IDEMPOTENCY_CONFLICT", "Runtime Call key was reused with different input", false));
            }
            let status: String = row.try_get("status").map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_INVALID", error.to_string(), false))?;
            if status == "succeeded" {
                let response = row.try_get::<Option<Value>, _>("response_json").map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_INVALID", error.to_string(), false))?.unwrap_or(Value::Null);
                tx.commit().await.map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_UNAVAILABLE", error.to_string(), false))?;
                return Ok(Some(response));
            }
            let side_effect: String = row.try_get("side_effect").map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_INVALID", error.to_string(), false))?;
            let stdio_frame_requires_reconciliation =
                kind == "sandbox" && request.get("frame").is_some() && status == "sent";
            if runtime_call_is_replayable(&status, &side_effect)
                && !stdio_frame_requires_reconciliation
            {
                sqlx::query(
                    "UPDATE runtime_calls SET status='reserved',error_code=NULL,error_message=NULL,ended_at=NULL WHERE id=? AND status=?",
                )
                .bind(row.try_get::<Uuid, _>("id").map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_INVALID", error.to_string(), false))?)
                .bind(&status)
                .execute(&mut *tx)
                .await
                .map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_UNAVAILABLE", error.to_string(), false))?;
                tx.commit().await.map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_UNAVAILABLE", error.to_string(), false))?;
                return Ok(None);
            }
            return Err(WorkerExecution::failed(
                row.try_get::<Option<String>, _>("error_code").ok().flatten().as_deref().unwrap_or("PROVIDER_OUTCOME_UNKNOWN"),
                row.try_get::<Option<String>, _>("error_message").ok().flatten().unwrap_or_else(|| format!("Runtime Call remains {status}")),
                status == "sent" || status == "outcome_unknown",
            ));
        }
        let tool_name_snapshot = binding.and_then(|value| match &value.configuration {
            RuntimeResourceConfigurationV1::Mcp { tool_name, .. } if kind == "mcp_tool" => {
                Some(tool_name.as_str())
            }
            _ => None,
        });
        let side_effect = binding
            .and_then(|binding| match &binding.configuration {
                RuntimeResourceConfigurationV1::Mcp { side_effect, .. } if kind == "mcp_tool" => {
                    Some(match side_effect.as_str() {
                        "none" | "read_only" => "none",
                        "idempotent" => "idempotent",
                        _ => "irreversible",
                    })
                }
                _ => None,
            })
            .unwrap_or_else(|| runtime_call_side_effect(kind, request));
        sqlx::query(
            "INSERT INTO runtime_calls(id,tenant_id,execution_id,node_execution_id,attempt_id,agent_run_id,plugin_parent_span_entity_id,iteration_index,call_index,call_kind,idempotency_key,request_fingerprint,resource_type,resource_id,resource_version_id,tool_name_snapshot,side_effect,status,request_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,'reserved',?)",
        )
        .bind(call_id)
        .bind(claim.task.tenant_id)
        .bind(claim.task.execution_id)
        .bind(claim.task.node_execution_id)
        .bind(claim.task.attempt_id)
        .bind((claim.node_type == "agent").then(|| stable_id(claim.task.attempt_id, b"agent-run")))
        .bind(claim.trace_parent_span_entity_id)
        .bind(if claim.node_type == "agent" { call_index / 2 } else { 0 })
        .bind(call_index)
        .bind(kind)
        .bind(idempotency_key)
        .bind(fingerprint)
        .bind(binding.map(|value| resource_kind_name(value.resource_kind)))
        .bind(binding.map(|value| value.resource_id))
        .bind(binding.map(|value| value.resource_version.as_str()))
        .bind(tool_name_snapshot)
        .bind(side_effect)
        .bind(request)
        .execute(&mut *tx)
        .await
        .map_err(|error| WorkerExecution::failed("RUNTIME_CALL_STATE_UNAVAILABLE", error.to_string(), false))?;
        tx.commit().await.map_err(|error| {
            WorkerExecution::failed("RUNTIME_CALL_STATE_UNAVAILABLE", error.to_string(), false)
        })?;
        self.emit_runtime_call_trace(
            call_id,
            agentx_runtime_contracts::TraceEventKindV1::Started,
            "reserved",
            None,
            Some(request),
        )
        .await;
        Ok(None)
    }

    async fn fail_call(
        &self,
        call_id: Uuid,
        code: &str,
        message: impl Into<String>,
        outcome_unknown: bool,
    ) -> WorkerExecution {
        let message = message.into();
        let _ = sqlx::query(
            "UPDATE runtime_calls SET status=?,error_code=?,error_message=?,ended_at=UTC_TIMESTAMP(6) WHERE id=? AND status IN ('reserved','sent')",
        )
        .bind(if outcome_unknown { "outcome_unknown" } else { "failed" })
        .bind(code)
        .bind(&message)
        .bind(call_id)
        .execute(&self.pool)
        .await;
        self.emit_runtime_call_trace(
            call_id,
            agentx_runtime_contracts::TraceEventKindV1::Finished,
            if outcome_unknown {
                "outcome_unknown"
            } else {
                "failed"
            },
            Some(code),
            None,
        )
        .await;
        WorkerExecution::failed(code, message, outcome_unknown)
    }

    async fn emit_runtime_call_trace(
        &self,
        call_id: Uuid,
        event_kind: agentx_runtime_contracts::TraceEventKindV1,
        status: &str,
        error_code: Option<&str>,
        content: Option<&Value>,
    ) {
        let row = match sqlx::query("SELECT tenant_id,execution_id,node_execution_id,attempt_id,agent_run_id,plugin_parent_span_entity_id,iteration_index,call_kind,resource_type,resource_id,resource_version_id,response_artifact_id,input_tokens,output_tokens,cost_micros,error_message FROM runtime_calls WHERE id=?")
            .bind(call_id)
            .fetch_optional(&self.pool)
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, %call_id, "Runtime Call Trace lookup failed");
                return;
            }
        };
        let tenant_id = match row.try_get::<Uuid, _>("tenant_id") {
            Ok(value) => value,
            Err(_) => return,
        };
        let execution_id = match row.try_get::<Uuid, _>("execution_id") {
            Ok(value) => value,
            Err(_) => return,
        };
        let attempt_id = match row.try_get::<Uuid, _>("attempt_id") {
            Ok(value) => value,
            Err(_) => return,
        };
        let kind = row
            .try_get::<String, _>("call_kind")
            .unwrap_or_else(|_| "runtime".into());
        let agent_run_id = row
            .try_get::<Option<Uuid>, _>("agent_run_id")
            .ok()
            .flatten();
        let agent_iteration_id = agent_run_id.map(|run_id| {
            let index = row.try_get::<u32, _>("iteration_index").unwrap_or_default();
            stable_id(run_id, format!("iteration-{index}").as_bytes())
        });
        let plugin_parent = row
            .try_get::<Option<Uuid>, _>("plugin_parent_span_entity_id")
            .ok()
            .flatten();
        let parent = plugin_parent
            .map(|id| {
                (
                    id,
                    agentx_runtime_contracts::TraceSpanKindV1::PluginOperation,
                )
            })
            .or_else(|| {
                agent_iteration_id.map(|id| {
                    (
                        id,
                        agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                    )
                })
            })
            .unwrap_or((
                attempt_id,
                agentx_runtime_contracts::TraceSpanKindV1::Attempt,
            ));
        let mut trace = crate::trace_delivery::TraceDraft::span(
            tenant_id,
            execution_id,
            call_id,
            Some(parent),
            agentx_runtime_contracts::TraceSpanKindV1::RuntimeCall,
            runtime_call_span_name(&kind),
            event_kind,
            format!(
                "runtime_call.{}",
                if event_kind == agentx_runtime_contracts::TraceEventKindV1::Finished {
                    "finished"
                } else {
                    "started"
                }
            ),
            status,
        );
        trace.node_execution_id = row.try_get("node_execution_id").ok();
        trace.attempt_id = Some(attempt_id);
        trace.agent_run_id = agent_run_id;
        trace.agent_iteration_id = agent_iteration_id;
        trace.runtime_call_id = Some(call_id);
        trace.resource_type = row.try_get("resource_type").ok();
        trace.resource_id = row.try_get("resource_id").ok();
        trace.resource_version = row
            .try_get::<Option<Uuid>, _>("resource_version_id")
            .ok()
            .flatten()
            .map(|value| value.to_string());
        trace.input_tokens = row.try_get("input_tokens").ok();
        trace.output_tokens = row.try_get("output_tokens").ok();
        trace.cost_micros = row.try_get("cost_micros").unwrap_or_default();
        trace.attributes = json!({
            "meteringSource": if plugin_parent.is_some() { "plugin_host_call" } else { "platform_runtime_call" },
            "costIncludedInParent": false,
        });
        trace.error_code = error_code.map(str::to_owned);
        trace.error_message = row.try_get("error_message").ok();
        trace.content_kind = Some(
            if event_kind == agentx_runtime_contracts::TraceEventKindV1::Started {
                agentx_runtime_contracts::TraceContentKindV1::RuntimeRequest
            } else {
                agentx_runtime_contracts::TraceContentKindV1::RuntimeResponse
            },
        );
        trace.content_ref = row
            .try_get::<Option<Uuid>, _>("response_artifact_id")
            .ok()
            .flatten();
        trace.content_preview = if trace.content_ref.is_some() {
            None
        } else {
            content.and_then(crate::trace_delivery::bounded_preview)
        };
        let Ok(mut tx) = self.pool.begin().await else {
            return;
        };
        if let Err(error) = crate::trace_delivery::enqueue(&mut tx, trace).await {
            tracing::warn!(%error, %call_id, "Runtime Call Trace enqueue failed");
            return;
        }
        if let Err(error) = tx.commit().await {
            tracing::warn!(%error, %call_id, "Runtime Call Trace commit failed");
        }
    }
}

fn select_binding(
    claim: &ClaimedWorkerAttempt,
    kind: RuntimeResourceKindV1,
) -> Option<&RuntimeResourceBindingV1> {
    let requested = claim
        .node_parameters
        .get("resourceId")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    claim.resources.iter().find(|binding| {
        binding.resource_kind == kind
            && requested.is_none_or(|resource_id| resource_id == binding.resource_id)
            && (kind != RuntimeResourceKindV1::Mcp || is_mcp_tool_binding(binding))
    })
}

fn mcp_tool_binding(resources: &[RuntimeResourceBindingV1]) -> Option<&RuntimeResourceBindingV1> {
    resources
        .iter()
        .find(|binding| is_mcp_tool_binding(binding))
}

fn is_mcp_tool_binding(binding: &RuntimeResourceBindingV1) -> bool {
    binding.resource_kind == RuntimeResourceKindV1::Mcp
        && matches!(
            &binding.configuration,
            RuntimeResourceConfigurationV1::Mcp { tool_name, .. }
                if !tool_name.trim().is_empty() && tool_name != "__server__"
        )
}

fn capability_resource_kind(capability: &NodeCapability) -> RuntimeResourceKindV1 {
    match capability {
        NodeCapability::Model | NodeCapability::Agent => RuntimeResourceKindV1::Model,
        NodeCapability::McpTool => RuntimeResourceKindV1::Mcp,
        NodeCapability::Skill => RuntimeResourceKindV1::Skill,
        NodeCapability::Rag => RuntimeResourceKindV1::Rag,
        NodeCapability::Memory => RuntimeResourceKindV1::Memory,
        NodeCapability::Sandbox => RuntimeResourceKindV1::SandboxProfile,
        NodeCapability::Builtin | NodeCapability::PluginNodejs => RuntimeResourceKindV1::Credential,
    }
}

fn resource_kind_name(kind: RuntimeResourceKindV1) -> &'static str {
    match kind {
        RuntimeResourceKindV1::Model => "model",
        RuntimeResourceKindV1::Mcp => "mcp",
        RuntimeResourceKindV1::Rag => "rag",
        RuntimeResourceKindV1::Memory => "memory",
        RuntimeResourceKindV1::Skill => "skill",
        RuntimeResourceKindV1::Credential => "credential",
        RuntimeResourceKindV1::SandboxProfile => "sandbox_profile",
        RuntimeResourceKindV1::Composite => "composite",
    }
}

fn first_input(claim: &ClaimedWorkerAttempt) -> Option<Value> {
    claim
        .inputs
        .get("main")
        .or_else(|| claim.inputs.values().next())
        .and_then(|items| items.first())
        .map(|item| item.json.clone())
}

fn declarative_http_request(
    parameters: &Value,
    input: Option<Value>,
) -> Result<(String, Value), String> {
    let endpoint = parameters
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "HTTP url must resolve to a string".to_owned())?;
    let mut endpoint =
        reqwest::Url::parse(endpoint).map_err(|error| format!("HTTP url is invalid: {error}"))?;
    if let Some(query) = parameters.get("query").and_then(Value::as_array) {
        let mut pairs = endpoint.query_pairs_mut();
        for entry in query {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "HTTP query name is required".to_owned())?;
            let value = entry.get("value").map(http_value_text).unwrap_or_default();
            pairs.append_pair(name, &value);
        }
    }
    let mut headers = serde_json::Map::new();
    if let Some(entries) = parameters.get("headers").and_then(Value::as_array) {
        for entry in entries {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "HTTP header name is required".to_owned())?;
            headers.insert(
                name.into(),
                Value::String(entry.get("value").map(http_value_text).unwrap_or_default()),
            );
        }
    }
    Ok((
        endpoint.to_string(),
        json!({
            "method":parameters.get("method").and_then(Value::as_str).unwrap_or("GET"),
            "headers":headers,
            "body":parameters.get("body").cloned().unwrap_or_else(|| input.unwrap_or(Value::Null)),
            "apiKeyPlacement":parameters.get("apiKeyPlacement").cloned().unwrap_or(Value::Null),
        }),
    ))
}

fn http_value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn secret_fragments(secret: &[u8]) -> Vec<Vec<u8>> {
    fn collect(value: &Value, fragments: &mut Vec<Vec<u8>>) {
        match value {
            Value::String(value) if !value.is_empty() => fragments.push(value.as_bytes().to_vec()),
            Value::Array(values) => values.iter().for_each(|value| collect(value, fragments)),
            Value::Object(values) => values.values().for_each(|value| collect(value, fragments)),
            _ => {}
        }
    }
    let mut fragments = (!secret.is_empty())
        .then(|| secret.to_vec())
        .into_iter()
        .collect::<Vec<_>>();
    if let Ok(value) = serde_json::from_slice::<Value>(secret) {
        collect(&value, &mut fragments);
    }
    fragments.sort();
    fragments.dedup();
    fragments
}

fn redact_secret_text(value: &str, secrets: &[Vec<u8>]) -> String {
    String::from_utf8_lossy(&redact_secret_bytes(value.as_bytes(), secrets)).into_owned()
}

fn redact_secret_bytes(value: &[u8], secrets: &[Vec<u8>]) -> Vec<u8> {
    const REDACTED: &[u8] = b"[REDACTED]";
    let mut output = value.to_vec();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        let mut next = Vec::with_capacity(output.len());
        let mut offset = 0;
        while offset < output.len() {
            if output[offset..].starts_with(secret) {
                next.extend_from_slice(REDACTED);
                offset += secret.len();
            } else {
                next.push(output[offset]);
                offset += 1;
            }
        }
        output = next;
    }
    output
}

fn apply_http_credential(
    endpoint: &mut String,
    headers: &mut HeaderMap,
    credential_type: &str,
    secret: &[u8],
    api_key_placement: Option<&Value>,
) -> Result<(), String> {
    match credential_type {
        "bearer" => insert_http_header(
            headers,
            "authorization",
            &format!(
                "Bearer {}",
                std::str::from_utf8(secret).map_err(|_| "Bearer credential is not UTF-8")?
            ),
        ),
        "basic" => {
            let value: Value = serde_json::from_slice(secret)
                .map_err(|_| "Basic credential must be a JSON object")?;
            let username = value
                .get("username")
                .and_then(Value::as_str)
                .ok_or("Basic credential username is missing")?;
            let password = value
                .get("password")
                .and_then(Value::as_str)
                .ok_or("Basic credential password is missing")?;
            insert_http_header(
                headers,
                "authorization",
                &format!(
                    "Basic {}",
                    BASE64_STANDARD.encode(format!("{username}:{password}"))
                ),
            )
        }
        "api_key" => {
            let placement = api_key_placement
                .and_then(Value::as_object)
                .ok_or("apiKeyPlacement is required for an API Key credential")?;
            let location = placement
                .get("in")
                .and_then(Value::as_str)
                .ok_or("apiKeyPlacement.in is required")?;
            let name = placement
                .get("name")
                .and_then(Value::as_str)
                .ok_or("apiKeyPlacement.name is required")?;
            let value =
                std::str::from_utf8(secret).map_err(|_| "API Key credential is not UTF-8")?;
            if location == "header" {
                insert_http_header(headers, name, value)
            } else if location == "query" {
                append_http_query(endpoint, name, value)
            } else {
                Err("apiKeyPlacement.in must be header or query".into())
            }
        }
        "custom_json" => {
            let value: Value = serde_json::from_slice(secret)
                .map_err(|_| "Custom HTTP credential must be JSON")?;
            let object = value
                .as_object()
                .ok_or("Custom HTTP credential must be an object")?;
            if let Some(headers_value) = object.get("headers") {
                let values = headers_value
                    .as_object()
                    .ok_or("Custom HTTP credential headers must be an object")?;
                for (name, value) in values {
                    insert_http_header(
                        headers,
                        name,
                        value
                            .as_str()
                            .ok_or("Custom credential header values must be strings")?,
                    )?;
                }
            }
            if let Some(query_value) = object.get("query") {
                let values = query_value
                    .as_object()
                    .ok_or("Custom HTTP credential query must be an object")?;
                for (name, value) in values {
                    append_http_query(
                        endpoint,
                        name,
                        value
                            .as_str()
                            .ok_or("Custom credential query values must be strings")?,
                    )?;
                }
            }
            if object
                .keys()
                .any(|key| !matches!(key.as_str(), "headers" | "query"))
            {
                return Err("Custom HTTP credential only accepts headers and query".into());
            }
            Ok(())
        }
        _ => Err("Credential type is not supported by the HTTP node".into()),
    }
}

fn insert_http_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<(), String> {
    let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| "Credential header name is invalid")?;
    let value = reqwest::header::HeaderValue::from_str(value)
        .map_err(|_| "Credential header value is invalid")?;
    headers.insert(name, value);
    Ok(())
}

fn append_http_query(endpoint: &mut String, name: &str, value: &str) -> Result<(), String> {
    let mut url = reqwest::Url::parse(endpoint).map_err(|_| "HTTP endpoint is invalid")?;
    url.query_pairs_mut().append_pair(name, value);
    *endpoint = url.to_string();
    Ok(())
}

fn successful_value(execution: &WorkerExecution) -> Option<Value> {
    if execution.status != WorkerResultStatusV1::Succeeded {
        return None;
    }
    execution
        .outputs
        .get("main")
        .and_then(|items| items.first())
        .map(|item| item.json.clone())
}

#[cfg(test)]
#[path = "worker_runtime_tests.rs"]
mod tests;
