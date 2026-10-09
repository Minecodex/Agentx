//! OpenAI-compatible ModelPort adapter for the production Agent Core.

use std::collections::BTreeMap;
use std::sync::{Arc, atomic::Ordering};

use agentx_agent_core::{
    EffectContextV1, MessageRole, ModelPort, ModelPortError, ModelRequestV1, ModelResponseV1,
    ToolCallV1, ToolPortError,
};
use agentx_runtime_contracts::{
    AgentAttachmentToolV1, AgentCapabilityAuthorizationEvidenceV1, RuntimeMcpTransportV2,
    RuntimeResourceBindingV1, RuntimeResourceConfigurationV1, VaultSecretReferenceV1,
};
use serde_json::{Value, json};

use super::agent_attachments::authorize_attachment_projection;
use super::agent_budget::BudgetCounters;
use super::{ClaimedWorkerAttempt, RuntimeWorker};
use crate::worker_support::openai_chat_completions_endpoint;

pub(super) struct ProviderModelPort<'a> {
    worker: &'a RuntimeWorker,
    claim: &'a ClaimedWorkerAttempt,
    binding: RuntimeResourceBindingV1,
    session_key: String,
    node_key: String,
    pub(super) call_index: u32,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cost_micros: u64,
    counters: Arc<BudgetCounters>,
    attachment_tools: BTreeMap<String, AgentAttachmentToolV1>,
    authorization_evidence: Vec<AgentCapabilityAuthorizationEvidenceV1>,
}

impl<'a> ProviderModelPort<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        worker: &'a RuntimeWorker,
        claim: &'a ClaimedWorkerAttempt,
        binding: RuntimeResourceBindingV1,
        session_key: String,
        node_key: String,
        counters: Arc<BudgetCounters>,
        attachment_tools: Vec<AgentAttachmentToolV1>,
        authorization_evidence: Vec<AgentCapabilityAuthorizationEvidenceV1>,
    ) -> Self {
        Self {
            worker,
            claim,
            binding,
            session_key,
            node_key,
            call_index: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost_micros: 0,
            counters,
            attachment_tools: attachment_tools
                .into_iter()
                .map(|tool| (tool.name.clone(), tool))
                .collect(),
            authorization_evidence,
        }
    }

    fn binding_authorized(
        &self,
        binding: &RuntimeResourceBindingV1,
        operation: &str,
    ) -> Result<bool, ToolPortError> {
        match authorize_attachment_projection(
            self.worker,
            self.claim,
            &self.authorization_evidence,
            binding,
            operation,
        ) {
            Ok(()) => Ok(true),
            Err(ToolPortError::Unauthorized(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn exact_binding(
        &self,
        resource_id: uuid::Uuid,
        resource_version_id: uuid::Uuid,
    ) -> Option<&RuntimeResourceBindingV1> {
        self.claim.resources.iter().find(|binding| {
            binding.resource_id == resource_id
                && binding.resource_version == resource_version_id.to_string()
        })
    }

    fn credential_binding(
        &self,
        secret: &VaultSecretReferenceV1,
    ) -> Option<&RuntimeResourceBindingV1> {
        self.claim.resources.iter().find(|binding| {
            matches!(
                &binding.configuration,
                RuntimeResourceConfigurationV1::Credential { secret: candidate, .. }
                    if serde_json::to_value(candidate).ok() == serde_json::to_value(secret).ok()
            )
        })
    }

    fn dependency_authorized(
        &self,
        binding: &RuntimeResourceBindingV1,
    ) -> Result<bool, ToolPortError> {
        match &binding.configuration {
            RuntimeResourceConfigurationV1::Mcp {
                server_id,
                server_version_id,
                transport,
                credential,
                ..
            } => {
                let Some(server) = self.exact_binding(*server_id, *server_version_id) else {
                    return Ok(false);
                };
                if !self.binding_authorized(server, "use")? {
                    return Ok(false);
                }
                if let Some(secret) = credential {
                    let Some(credential) = self.credential_binding(secret) else {
                        return Ok(false);
                    };
                    if !self.binding_authorized(credential, "use")? {
                        return Ok(false);
                    }
                }
                if let RuntimeMcpTransportV2::Stdio {
                    environment_credential_refs,
                    runtime_sandbox,
                    ..
                } = transport
                {
                    let Some(sandbox) = self.exact_binding(
                        runtime_sandbox.resource_id,
                        runtime_sandbox.resource_version_id,
                    ) else {
                        return Ok(false);
                    };
                    if !self.binding_authorized(sandbox, "use")? {
                        return Ok(false);
                    }
                    for reference in environment_credential_refs {
                        let Some(credential) = self.credential_binding(&reference.credential)
                        else {
                            return Ok(false);
                        };
                        if !self.binding_authorized(credential, "use")? {
                            return Ok(false);
                        }
                    }
                }
            }
            RuntimeResourceConfigurationV1::Rag { credential, .. }
            | RuntimeResourceConfigurationV1::Memory { credential, .. } => {
                if let Some(secret) = credential {
                    let Some(credential) = self.credential_binding(secret) else {
                        return Ok(false);
                    };
                    if !self.binding_authorized(credential, "use")? {
                        return Ok(false);
                    }
                }
            }
            _ => {}
        }
        Ok(true)
    }

    fn attachment_authorized(
        &self,
        descriptor: &AgentAttachmentToolV1,
    ) -> Result<bool, ToolPortError> {
        let Some(binding) =
            self.exact_binding(descriptor.resource_id, descriptor.resource_version_id)
        else {
            return Ok(false);
        };
        if !self.binding_authorized(binding, &descriptor.operation)? {
            return Ok(false);
        }
        self.dependency_authorized(binding)
    }
}

impl ModelPort for ProviderModelPort<'_> {
    fn invoke(
        &mut self,
        request: &ModelRequestV1,
        context: &EffectContextV1,
    ) -> Result<ModelResponseV1, ModelPortError> {
        self.invoke_with_mode(request, context, false)
    }

    fn invoke_stream(
        &mut self,
        request: &ModelRequestV1,
        context: &EffectContextV1,
        _on_delta: &(dyn Fn(&agentx_agent_core::ModelDeltaChunk) + Send + Sync),
    ) -> Result<ModelResponseV1, ModelPortError> {
        // Deltas are routed straight to the invocation event sink by the
        // streaming transport; the driver-level callback stays unused here.
        self.invoke_with_mode(request, context, true)
    }
}

impl<'a> ProviderModelPort<'a> {
    fn invoke_with_mode(
        &mut self,
        request: &ModelRequestV1,
        context: &EffectContextV1,
        streaming: bool,
    ) -> Result<ModelResponseV1, ModelPortError> {
        let RuntimeResourceConfigurationV1::Model {
            provider,
            endpoint,
            model,
            price,
            credential,
            ..
        } = &self.binding.configuration
        else {
            return Err(ModelPortError::Effect(
                "Model binding configuration is invalid".into(),
            ));
        };
        if provider != "openai_compatible" {
            return Err(ModelPortError::Effect(
                "Agent Model must use the OpenAI-compatible provider contract".into(),
            ));
        }
        let mut messages = Vec::new();
        for message in &request.messages {
            let role = match message.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::ToolResult => "tool",
                MessageRole::ExternalContext => "system",
            };
            let mut value = json!({"role":role,"content":message.content});
            if let Some(id) = &message.tool_call_id {
                value["tool_call_id"] = json!(id);
            }
            if !message.tool_calls.is_empty() {
                value["tool_calls"] = json!(message.tool_calls.iter().map(|call| json!({"id":call.call_id,"type":"function","function":{"name":call.name,"arguments":call.arguments.to_string()}})).collect::<Vec<_>>());
            }
            messages.push(value);
        }
        let mut tools = Vec::new();
        for tool in &request.tools {
            let authorized = if let Some(descriptor) = self.attachment_tools.get(&tool.name) {
                self.attachment_authorized(descriptor).map_err(|error| {
                    ModelPortError::Effect(format!(
                        "attachment authorization refresh failed: {error}"
                    ))
                })?
            } else {
                true
            };
            if authorized {
                tools.push(json!({"type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.input_schema}}));
            }
        }
        let (purpose, call_kind) = match request.purpose {
            agentx_agent_core::ModelPurposeV1::AgentTurn => ("agent_turn", "model"),
            agentx_agent_core::ModelPurposeV1::Compaction => ("compaction", "compaction"),
        };
        let mut body = json!({"model":model,"messages":messages,"stream":streaming,"metadata":{"priceVersion":price.version_id,"agentPurpose":purpose}});
        if streaming {
            body["stream_options"] = json!({"include_usage":true});
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        let call_index = self.call_index;
        self.call_index = self.call_index.saturating_add(1);
        let endpoint = openai_chat_completions_endpoint(endpoint);
        let execution = if streaming {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(self.worker.call_http_effect_stream(
                    self.claim,
                    call_kind,
                    &endpoint,
                    body,
                    call_index,
                    &context.idempotency_key,
                    credential.as_ref(),
                    "authorization",
                    Some(&self.binding),
                ))
            })
        } else {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(self.worker.call_http_effect(
                    self.claim,
                    call_kind,
                    &endpoint,
                    body,
                    call_index,
                    &context.idempotency_key,
                    credential.as_ref(),
                    "authorization",
                    Some(&self.binding),
                ))
            })
        };
        if execution.status == agentx_runtime_contracts::WorkerResultStatusV1::OutcomeUnknown {
            return Err(ModelPortError::OutcomeUnknown(
                execution.error_message.unwrap_or_default(),
            ));
        }
        if execution.status != agentx_runtime_contracts::WorkerResultStatusV1::Succeeded {
            let message = execution
                .error_message
                .clone()
                .unwrap_or_else(|| "Provider Model call failed".into());
            if is_context_overflow_error(execution.error_code.as_deref(), &message) {
                return Err(ModelPortError::ContextOverflow);
            }
            return Err(ModelPortError::Effect(message));
        }
        let payload = execution
            .outputs
            .get("main")
            .and_then(|value| value.first())
            .map(|item| item.json.clone())
            .unwrap_or(Value::Null);
        let usage = payload.get("usage").unwrap_or(&Value::Null);
        let input_tokens = usage
            .get("prompt_tokens")
            .or_else(|| usage.get("inputTokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output_tokens = usage
            .get("completion_tokens")
            .or_else(|| usage.get("outputTokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_read_tokens = usage
            .get("cache_read_tokens")
            .or_else(|| usage.get("cacheReadTokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_write_tokens = usage
            .get("cache_write_tokens")
            .or_else(|| usage.get("cacheWriteTokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cost_micros = usage.get("costMicros").and_then(Value::as_u64).unwrap_or(0);
        let cost_currency = usage
            .get("costCurrency")
            .or_else(|| usage.get("currency"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.input_tokens = self.input_tokens.saturating_add(input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
        self.cost_micros = self.cost_micros.saturating_add(cost_micros);
        self.counters
            .input_tokens
            .fetch_add(input_tokens, Ordering::Relaxed);
        self.counters
            .output_tokens
            .fetch_add(output_tokens, Ordering::Relaxed);
        self.counters
            .cost_micros
            .fetch_add(cost_micros, Ordering::Relaxed);
        // Session Usage is an idempotent projection of the Runtime Call
        // Ledger.  The unique (operation,effect,kind) key makes a settlement
        // retry observable exactly once without making Core aware of SQL.
        let usage_kind = match request.purpose {
            agentx_agent_core::ModelPurposeV1::AgentTurn => "model",
            agentx_agent_core::ModelPurposeV1::Compaction => "compaction",
        };
        let usage_id = crate::worker_support::stable_id(
            self.claim.task.attempt_id,
            format!(
                "session-usage:{}:{}:{}",
                context.operation_id, context.effect_id, usage_kind
            )
            .as_bytes(),
        );
        let insert = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(sqlx::query(
                "INSERT INTO agent_session_usages(usage_id,tenant_id,session_key,stable_agent_node_key,session_id,operation_id,effect_id,usage_kind,resource_reference,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_micros,cost_currency) VALUES(?,?,?,?,?,?,?, ?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE usage_id=usage_id",
            )
            .bind(usage_id.to_string())
            .bind(self.claim.task.tenant_id)
            .bind(&self.session_key)
            .bind(&self.node_key)
            .bind(&self.session_key)
            .bind(&context.operation_id)
            .bind(&context.effect_id)
            .bind(usage_kind)
            .bind(&request.model_reference)
            .bind(input_tokens)
            .bind(output_tokens)
            .bind(cache_read_tokens)
            .bind(cache_write_tokens)
            .bind(cost_micros)
            .bind(cost_currency)
            .execute(&self.worker.pool))
        });
        if let Err(error) = insert {
            return Err(ModelPortError::Effect(format!(
                "AGENT_SESSION_USAGE_UNAVAILABLE: {error}"
            )));
        }
        let message = payload.pointer("/choices/0/message").ok_or_else(|| {
            ModelPortError::Effect("Provider response has no assistant message".into())
        })?;
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|call| {
                        let id = call.get("id")?.as_str()?.to_owned();
                        let function = call.get("function")?;
                        let name = function.get("name")?.as_str()?.to_owned();
                        let raw = function
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}");
                        let arguments =
                            serde_json::from_str(raw).unwrap_or_else(|_| json!({"value":raw}));
                        Some(ToolCallV1 {
                            call_id: id,
                            name,
                            arguments,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ModelResponseV1 {
            message_id: message
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    format!("assistant:{}:{}", self.claim.task.attempt_id, call_index)
                }),
            content,
            tool_calls,
        })
    }
}

fn is_context_overflow_error(code: Option<&str>, message: &str) -> bool {
    let haystack = format!(
        "{} {}",
        code.unwrap_or_default().to_ascii_lowercase(),
        message.to_ascii_lowercase()
    );
    [
        "context_length_exceeded",
        "context length exceeded",
        "maximum context length",
        "prompt is too long",
        "too many tokens",
        "context_window_exceeded",
    ]
    .iter()
    .any(|marker| haystack.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::is_context_overflow_error;

    #[test]
    fn provider_context_overflow_errors_are_classified() {
        assert!(is_context_overflow_error(
            Some("context_length_exceeded"),
            "request rejected"
        ));
        assert!(is_context_overflow_error(
            Some("PROVIDER_REJECTED"),
            "maximum context length is 128k"
        ));
        assert!(!is_context_overflow_error(
            Some("PROVIDER_REJECTED"),
            "invalid API key"
        ));
    }
}
