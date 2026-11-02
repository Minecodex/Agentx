//! Approval adapter around the existing MCP executor and its call ledger.
use super::*;
use crate::agent_tool_approvals::{self, Gate, Proposal};
use crate::worker_runtime::tool_execution_output;
use agentx_agent_core::{EffectObservationV1, ToolResultV1};
use sqlx::Row;

fn port_error(error: crate::error::RuntimeError) -> ToolPortError {
    match error {
        crate::error::RuntimeError::StateConflict("TOOL_EFFECT_OUTCOME_UNKNOWN", message) => {
            ToolPortError::OutcomeUnknown(message)
        }
        crate::error::RuntimeError::InvalidRequest(code, message) => {
            ToolPortError::Effect(format!("{code}: {message}"))
        }
        other => ToolPortError::Effect(other.to_string()),
    }
}
fn failure(error: &ToolPortError) -> ToolResultV1 {
    ToolResultV1 {
        content: error.to_string(),
        structured_result: Some(json!({"error":error.to_string()})),
        artifact_refs: vec![],
        truncated: false,
        is_error: true,
        terminate: true,
    }
}

pub(super) async fn previous_usage(
    worker: &RuntimeWorker,
    claim: &ClaimedWorkerAttempt,
    run: Uuid,
) -> crate::error::RuntimeResult<(u64, u64, u64)> {
    let row=sqlx::query("SELECT CAST(COALESCE(SUM(input_tokens),0) AS UNSIGNED) input_tokens,CAST(COALESCE(SUM(output_tokens),0) AS UNSIGNED) output_tokens,CAST(COALESCE(SUM(cost_micros),0) AS UNSIGNED) cost_micros FROM runtime_calls WHERE tenant_id=? AND attempt_id=? AND agent_run_id=? AND call_kind IN ('model','compaction')")
        .bind(claim.task.tenant_id).bind(claim.task.attempt_id).bind(run).fetch_one(&worker.pool).await?;
    Ok((
        row.try_get("input_tokens")?,
        row.try_get("output_tokens")?,
        row.try_get("cost_micros")?,
    ))
}
impl AgentToolRouter<'_> {
    pub(super) fn stored_approval_observation(
        &self,
        binding: &RuntimeResourceBindingV1,
        call: &ToolCallV1,
        context: &EffectContextV1,
    ) -> Option<EffectObservationV1> {
        let effect = agentx_runtime_contracts::deterministic_uuid(
            self.agent_run_id,
            context.effect_id.as_bytes(),
        );
        let stored = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let row = sqlx::query("SELECT tool_id,tool_version,tool_call_id,arguments_hash,outcome_unknown,result_json,runtime_call_index FROM agent_tool_effects WHERE tenant_id=? AND id=? AND attempt_id=? AND operation_id=? AND effect_id=?")
                    .bind(self.claim.task.tenant_id).bind(effect).bind(self.claim.task.attempt_id)
                    .bind(&context.operation_id).bind(&context.effect_id).fetch_optional(&self.worker.pool).await?;
                let Some(row) = row else { return Ok::<_,sqlx::Error>(None); };
                let index: u32 = row.try_get("runtime_call_index")?;
                let stdio = matches!(&binding.configuration,RuntimeResourceConfigurationV1::Mcp{transport:RuntimeMcpTransportV2::Stdio{..},..});
                let receipt = if stdio {
                    sqlx::query("SELECT status,response_json,error_code,error_message FROM runtime_calls WHERE tenant_id=? AND attempt_id=? AND agent_run_id=? AND call_kind='sandbox' AND JSON_UNQUOTE(JSON_EXTRACT(request_json,'$.effectId'))=? AND JSON_UNQUOTE(JSON_EXTRACT(request_json,'$.frame.method'))='tools/call' ORDER BY started_at DESC LIMIT 1")
                        .bind(self.claim.task.tenant_id).bind(self.claim.task.attempt_id).bind(self.agent_run_id)
                        .bind(&context.effect_id).fetch_optional(&self.worker.pool).await?
                } else {
                    sqlx::query("SELECT status,response_json,error_code,error_message FROM runtime_calls WHERE tenant_id=? AND attempt_id=? AND agent_run_id=? AND call_kind='mcp_tool' AND call_index=?")
                        .bind(self.claim.task.tenant_id).bind(self.claim.task.attempt_id).bind(self.agent_run_id)
                        .bind(index).fetch_optional(&self.worker.pool).await?
                };
                Ok(Some((row,receipt,stdio)))
            })
        });
        let Ok(stored) = stored else {
            return Some(EffectObservationV1::Unknown);
        };
        let (row, receipt, stdio) = stored?;
        let Ok(hash) = agentx_runtime_contracts::content_hash(&call.arguments) else {
            return Some(EffectObservationV1::Unknown);
        };
        let RuntimeResourceConfigurationV1::Mcp { tool_version, .. } = &binding.configuration
        else {
            return Some(EffectObservationV1::Unknown);
        };
        if row.try_get::<Uuid, _>("tool_id").ok() != Some(binding.resource_id)
            || row.try_get::<String, _>("arguments_hash").ok().as_deref() != Some(hash.as_str())
            || row.try_get::<String, _>("tool_call_id").ok().as_deref()
                != Some(call.call_id.as_str())
            || row.try_get::<String, _>("tool_version").ok().as_deref()
                != Some(tool_version.as_str())
        {
            return Some(EffectObservationV1::Unknown);
        }
        let Ok(unknown) = row.try_get::<bool, _>("outcome_unknown") else {
            return Some(EffectObservationV1::Unknown);
        };
        if unknown {
            return Some(EffectObservationV1::Unknown);
        }
        let Ok(result) = row.try_get::<Option<Value>, _>("result_json") else {
            return Some(EffectObservationV1::Unknown);
        };
        if let Some(result) = result {
            return Some(EffectObservationV1::Confirmed(result));
        }
        let receipt = receipt?;
        let Ok(status) = receipt.try_get::<String, _>("status") else {
            return Some(EffectObservationV1::Unknown);
        };
        if status == "reserved" {
            return None;
        }
        let result = if status == "succeeded" && !stdio {
            let Ok(Some(payload)) = receipt.try_get::<Option<Value>, _>("response_json") else {
                return Some(EffectObservationV1::Unknown);
            };
            tool_result_from_execution(tool_execution_output(WorkerExecution::succeeded(payload)))
        } else if matches!(status.as_str(), "failed" | "cancelled") {
            Err(ToolPortError::Effect(
                receipt
                    .try_get::<Option<String>, _>("error_message")
                    .ok()
                    .flatten()
                    .or_else(|| {
                        receipt
                            .try_get::<Option<String>, _>("error_code")
                            .ok()
                            .flatten()
                    })
                    .unwrap_or_else(|| "The original tool operation failed".into()),
            ))
        } else {
            let error = ToolPortError::OutcomeUnknown(
                "The dispatched tool operation needs result verification".into(),
            );
            let _ = self.settle_approval(effect, &Err(error));
            return Some(EffectObservationV1::Unknown);
        };
        if self.settle_approval(effect, &result).is_err() {
            return Some(EffectObservationV1::Unknown);
        }
        let result = result.unwrap_or_else(|error| failure(&error));
        Some(EffectObservationV1::Confirmed(
            serde_json::to_value(result).expect("ToolResult serialization"),
        ))
    }
    fn enter_approval(
        &self,
        binding: &RuntimeResourceBindingV1,
        call: &ToolCallV1,
        context: &EffectContextV1,
    ) -> Result<Option<Gate>, ToolPortError> {
        let RuntimeResourceConfigurationV1::Mcp {
            tool_policy: Some(policy),
            tool_name,
            tool_version,
            ..
        } = &binding.configuration
        else {
            return Ok(None);
        };
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut tx = self
                    .worker
                    .pool
                    .begin()
                    .await
                    .map_err(crate::error::RuntimeError::from)?;
                agent_tool_approvals::authorize_policy(
                    &mut tx,
                    self.claim.task.tenant_id,
                    binding.resource_id,
                    policy,
                )
                .await?;
                tx.commit()
                    .await
                    .map_err(crate::error::RuntimeError::from)?;
                if policy.approval.is_none() {
                    return Ok(None);
                }
                agent_tool_approvals::enter(
                    &self.worker.pool,
                    Proposal {
                        claim: self.claim,
                        run_id: self.agent_run_id,
                        context,
                        call,
                        tool_id: binding.resource_id,
                        tool_version,
                        tool_name,
                        policy,
                        call_index: self.call_index,
                    },
                )
                .await
                .map(Some)
            })
        })
        .map_err(port_error)
    }
    fn settle_approval(
        &self,
        effect: Uuid,
        result: &Result<ToolResultV1, ToolPortError>,
    ) -> Result<(), ToolPortError> {
        let unknown = matches!(result, Err(ToolPortError::OutcomeUnknown(_)));
        let stored = match result {
            Ok(value) => value.clone(),
            Err(error) => failure(error),
        };
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(agent_tool_approvals::settle(
                &self.worker.pool,
                self.claim.task.tenant_id,
                effect,
                (!unknown).then_some(&stored),
                unknown,
            ))
        })
        .map_err(port_error)
    }
    fn deny_unsent_approval(
        &self,
        context: &EffectContextV1,
        error: &ToolPortError,
    ) -> Result<ToolResultV1, ToolPortError> {
        if matches!(error, ToolPortError::OutcomeUnknown(_)) {
            return Err(ToolPortError::OutcomeUnknown(error.to_string()));
        }
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(agent_tool_approvals::deny_unsent(
                &self.worker.pool,
                self.claim,
                self.agent_run_id,
                context,
                &failure(error),
            ))
        })
        .map_err(port_error)
    }
    pub(super) fn execute_mcp_attachment(
        &mut self,
        binding: &RuntimeResourceBindingV1,
        descriptor: &AgentAttachmentToolV1,
        call: &ToolCallV1,
        context: &EffectContextV1,
    ) -> Result<ToolResultV1, ToolPortError> {
        let gate = match self.enter_approval(binding, call, context) {
            Ok(gate) => gate,
            Err(error) => return self.deny_unsent_approval(context, &error),
        };
        let (effect, index) = match gate {
            Some(Gate::Pending(id)) => {
                return Err(ToolPortError::Pending(format!("tool-approval:{id}")));
            }
            Some(Gate::Settled(result)) => return Ok(result),
            Some(Gate::Execute { effect, call_index }) => (Some(effect), call_index),
            None => (None, self.call_index),
        };
        self.call_index = self.call_index.max(index.saturating_add(1));
        let RuntimeResourceConfigurationV1::Mcp {
            server_version_id,
            transport,
            tool_name,
            credential,
            ..
        } = &binding.configuration
        else {
            unreachable!()
        };
        let result = match transport {
            RuntimeMcpTransportV2::Stdio { .. } => self.stdio_mcp_call(
                binding,
                &descriptor.replay_policy,
                effect.map(|_| index),
                call,
                context,
            ),
            RuntimeMcpTransportV2::StreamableHttp { endpoint }
            | RuntimeMcpTransportV2::Sse { endpoint } => {
                let existing_session = self.mcp_sessions.get(server_version_id).cloned();
                let legacy_sse = matches!(transport, RuntimeMcpTransportV2::Sse { .. });
                let key = legacy_sse.then(|| {
                    self.legacy_sse_session_keys
                        .entry(*server_version_id)
                        .or_insert_with(|| {
                            format!(
                                "agent-run:{}:mcp-server:{}",
                                self.agent_run_id, server_version_id
                            )
                        })
                        .clone()
                });
                let (execution, next_session) = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(self.worker.call_agent_mcp_tool(
                        self.claim,
                        endpoint,
                        tool_name,
                        call.arguments.clone(),
                        index,
                        credential.as_ref(),
                        binding,
                        existing_session.as_deref(),
                        legacy_sse,
                        key.as_deref(),
                    ))
                });
                if let Some(session) = next_session {
                    self.mcp_sessions.insert(*server_version_id, session);
                }
                tool_result_from_execution(execution)
            }
        };
        if let Some(effect) = effect {
            self.settle_approval(effect, &result)?;
        }
        result
    }
    pub(super) fn observe_approved_tool(
        &mut self,
        context: &EffectContextV1,
    ) -> EffectObservationV1 {
        let intent = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(sqlx::query_scalar::<_,Value>("SELECT operation_json FROM agent_session_operations WHERE tenant_id=? AND operation_id=? AND attempt_id=?").bind(self.claim.task.tenant_id).bind(&context.operation_id).bind(self.claim.task.attempt_id).fetch_optional(&self.worker.pool))
        });
        let Ok(Some(intent)) = intent else {
            return EffectObservationV1::Unknown;
        };
        let Ok(call) = serde_json::from_value::<ToolCallV1>(intent["intent"].clone()) else {
            return EffectObservationV1::Unknown;
        };
        let Some(descriptor) = self
            .attachments
            .iter()
            .find(|tool| tool.name == call.name)
            .cloned()
        else {
            return EffectObservationV1::Unknown;
        };
        let Some(binding) = self
            .claim
            .resources
            .iter()
            .find(|binding| {
                binding.resource_id == descriptor.resource_id
                    && binding.resource_version == descriptor.resource_version_id.to_string()
            })
            .cloned()
        else {
            return EffectObservationV1::Unknown;
        };
        if !matches!(&binding.configuration,RuntimeResourceConfigurationV1::Mcp{tool_policy:Some(policy),..} if policy.approval.is_some())
        {
            return EffectObservationV1::Unknown;
        }
        // Reading a durable receipt is not a new tool invocation. Preserve its
        // factual result before checking whether a new dispatch is still authorized.
        if let Some(observation) = self.stored_approval_observation(&binding, &call, context) {
            return observation;
        }
        let result = self
            .authorize_attachment(&binding, &descriptor.operation)
            .and_then(|_| self.enter_approval(&binding, &call, context));
        match result {
            Ok(Some(Gate::Pending(id))) => {
                EffectObservationV1::Pending(format!("tool-approval:{id}"))
            }
            Ok(Some(Gate::Settled(result))) => EffectObservationV1::Confirmed(
                serde_json::to_value(result).expect("ToolResult serialization"),
            ),
            Ok(Some(Gate::Execute { effect, call_index })) => {
                if matches!(
                    &binding.configuration,
                    RuntimeResourceConfigurationV1::Mcp {
                        transport: RuntimeMcpTransportV2::Stdio { .. },
                        ..
                    }
                ) {
                    let status = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(sqlx::query_scalar::<_,String>("SELECT status FROM runtime_calls WHERE tenant_id=? AND attempt_id=? AND agent_run_id=? AND call_kind='sandbox' AND JSON_UNQUOTE(JSON_EXTRACT(request_json,'$.effectId'))=? AND JSON_UNQUOTE(JSON_EXTRACT(request_json,'$.frame.method'))='tools/call' ORDER BY started_at DESC LIMIT 1").bind(self.claim.task.tenant_id).bind(self.claim.task.attempt_id).bind(self.agent_run_id).bind(&context.effect_id).fetch_optional(&self.worker.pool))
                    });
                    // A dispatched stdio frame without its Core receipt needs reconciliation, never another write.
                    return match status {
                        Ok(None) => EffectObservationV1::NotStarted,
                        Ok(Some(status)) if status == "reserved" => EffectObservationV1::NotStarted,
                        _ => {
                            let _ = self.settle_approval(
                                effect,
                                &Err(ToolPortError::OutcomeUnknown(
                                    "The original stdio operation needs result verification".into(),
                                )),
                            );
                            EffectObservationV1::Unknown
                        }
                    };
                }
                let status = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(sqlx::query_scalar::<_,String>("SELECT status FROM runtime_calls WHERE tenant_id=? AND attempt_id=? AND agent_run_id=? AND call_kind='mcp_tool' AND call_index=?").bind(self.claim.task.tenant_id).bind(self.claim.task.attempt_id).bind(self.agent_run_id).bind(call_index).fetch_optional(&self.worker.pool))
                });
                // Re-enter the same ledger key only when it is demonstrably unsent or settled.
                match status {
                    Ok(None) => EffectObservationV1::NotStarted,
                    Ok(Some(status))
                        if matches!(
                            status.as_str(),
                            "reserved" | "succeeded" | "failed" | "cancelled"
                        ) =>
                    {
                        EffectObservationV1::NotStarted
                    }
                    _ => {
                        let _ = self.settle_approval(
                            effect,
                            &Err(ToolPortError::OutcomeUnknown(
                                "The original tool operation needs result verification".into(),
                            )),
                        );
                        EffectObservationV1::Unknown
                    }
                }
            }
            Err(error) => match self.deny_unsent_approval(context, &error) {
                Ok(result) => EffectObservationV1::Confirmed(
                    serde_json::to_value(result).expect("ToolResult serialization"),
                ),
                Err(_) => EffectObservationV1::Unknown,
            },
            _ => EffectObservationV1::Unknown,
        }
    }
}
