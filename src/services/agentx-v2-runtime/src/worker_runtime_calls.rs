//! Runtime call ledger helpers (reserve/fail/trace), split from
//! worker_runtime.rs to respect the 2000-line file limit.
use agentx_runtime_contracts::{
    RuntimeResourceBindingV1, RuntimeResourceConfigurationV1, RuntimeResourceKindV1,
};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use super::engine::ClaimedWorkerAttempt;
use super::worker_runtime::output::runtime_call_is_replayable;
pub(crate) use super::worker_runtime::output::runtime_call_side_effect;
use super::worker_runtime::{RuntimeWorker, WorkerExecution};
use super::worker_support::{runtime_call_span_name, stable_id};

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

impl RuntimeWorker {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn reserve_call(
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

    pub(crate) async fn fail_call(
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

    pub(crate) async fn emit_runtime_call_trace(
        &self,
        call_id: Uuid,
        event_kind: agentx_runtime_contracts::TraceEventKindV1,
        status: &str,
        error_code: Option<&str>,
        content: Option<&Value>,
    ) {
        let row = match sqlx::query("SELECT tenant_id,execution_id,node_execution_id,attempt_id,agent_run_id,plugin_parent_span_entity_id,iteration_index,call_kind,tool_name_snapshot,resource_type,resource_id,resource_version_id,response_artifact_id,input_tokens,output_tokens,cost_micros,first_token_ms,error_message FROM runtime_calls WHERE id=?")
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
        let span_name = if kind == "mcp_tool" {
            match row.try_get::<Option<String>, _>("tool_name_snapshot") {
                Ok(Some(name)) if !name.is_empty() => name,
                _ => {
                    tracing::warn!(%call_id, "MCP trace is missing its frozen tool name");
                    return;
                }
            }
        } else {
            runtime_call_span_name(&kind)
        };
        let mut trace = crate::trace_delivery::TraceDraft::span(
            tenant_id,
            execution_id,
            call_id,
            Some(parent),
            agentx_runtime_contracts::TraceSpanKindV1::RuntimeCall,
            span_name,
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
            "firstTokenMs": row.try_get::<Option<u64>, _>("first_token_ms").ok().flatten(),
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
