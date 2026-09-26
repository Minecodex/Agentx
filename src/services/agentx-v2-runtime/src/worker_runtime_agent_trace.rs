use super::*;

impl RuntimeWorker {
    pub(super) async fn emit_agent_core_events(
        &self,
        run_id: Uuid,
        claim: &ClaimedWorkerAttempt,
        events: &[agentx_agent_core::CoreEventV1],
    ) {
        let mut turn = 0_u32;
        for (index, event) in events.iter().enumerate() {
            let (
                entity_id,
                parent,
                span_kind,
                span_name,
                event_type,
                event_kind,
                status,
                error_code,
                attributes,
            ) = match event {
                // Deltas are forwarded live by the runtime adapter and never
                // reach the batching collector; ignore them defensively here.
                agentx_agent_core::CoreEventV1::ModelDelta { .. } => {
                    continue;
                }
                agentx_agent_core::CoreEventV1::AgentStarted { .. } => (
                    run_id,
                    Some((
                        claim.task.attempt_id,
                        agentx_runtime_contracts::TraceSpanKindV1::Attempt,
                    )),
                    agentx_runtime_contracts::TraceSpanKindV1::AgentRun,
                    "Agent run",
                    "agent_run.started",
                    agentx_runtime_contracts::TraceEventKindV1::Started,
                    "running",
                    None,
                    json!({"source":"agent_core"}),
                ),
                agentx_agent_core::CoreEventV1::TurnStarted { turn: current } => {
                    turn = *current;
                    let id = crate::worker_support::stable_id(
                        run_id,
                        format!("turn-{current}").as_bytes(),
                    );
                    (
                        id,
                        Some((run_id, agentx_runtime_contracts::TraceSpanKindV1::AgentRun)),
                        agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                        "Agent turn",
                        "agent_turn.started",
                        agentx_runtime_contracts::TraceEventKindV1::Started,
                        "running",
                        None,
                        json!({"turn":current}),
                    )
                }
                agentx_agent_core::CoreEventV1::TurnEnded {
                    turn: current,
                    is_error,
                } => {
                    let id = crate::worker_support::stable_id(
                        run_id,
                        format!("turn-{current}").as_bytes(),
                    );
                    (
                        id,
                        Some((run_id, agentx_runtime_contracts::TraceSpanKindV1::AgentRun)),
                        agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                        "Agent turn",
                        "agent_turn.finished",
                        agentx_runtime_contracts::TraceEventKindV1::Finished,
                        if *is_error { "failed" } else { "succeeded" },
                        None,
                        json!({"turn":current,"isError":is_error}),
                    )
                }
                agentx_agent_core::CoreEventV1::ModelIntent { operation_id } => (
                    crate::worker_support::stable_id(run_id, operation_id.as_bytes()),
                    Some((
                        crate::worker_support::stable_id(run_id, format!("turn-{turn}").as_bytes()),
                        agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                    )),
                    agentx_runtime_contracts::TraceSpanKindV1::RuntimeCall,
                    "Agent model operation",
                    "agent_model.intent",
                    agentx_runtime_contracts::TraceEventKindV1::Started,
                    "running",
                    None,
                    json!({"operationId":operation_id,"turn":turn}),
                ),
                agentx_agent_core::CoreEventV1::ModelSettled {
                    operation_id,
                    is_error,
                } => (
                    crate::worker_support::stable_id(run_id, operation_id.as_bytes()),
                    Some((
                        crate::worker_support::stable_id(run_id, format!("turn-{turn}").as_bytes()),
                        agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                    )),
                    agentx_runtime_contracts::TraceSpanKindV1::RuntimeCall,
                    "Agent model operation",
                    "agent_model.settled",
                    agentx_runtime_contracts::TraceEventKindV1::Finished,
                    if *is_error { "failed" } else { "succeeded" },
                    if *is_error {
                        Some("AGENT_MODEL_ERROR")
                    } else {
                        None
                    },
                    json!({"operationId":operation_id,"turn":turn,"isError":is_error}),
                ),
                agentx_agent_core::CoreEventV1::MessageAdded { message_id } => (
                    crate::worker_support::stable_id(
                        run_id,
                        format!("message-{message_id}").as_bytes(),
                    ),
                    Some((
                        crate::worker_support::stable_id(run_id, format!("turn-{turn}").as_bytes()),
                        agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                    )),
                    agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                    "Agent message",
                    "agent_message.added",
                    agentx_runtime_contracts::TraceEventKindV1::Finished,
                    "succeeded",
                    None,
                    json!({"messageId":message_id,"turn":turn}),
                ),
                agentx_agent_core::CoreEventV1::RecoveryRequired { operation_id } => (
                    crate::worker_support::stable_id(run_id, operation_id.as_bytes()),
                    Some((run_id, agentx_runtime_contracts::TraceSpanKindV1::AgentRun)),
                    agentx_runtime_contracts::TraceSpanKindV1::RuntimeCall,
                    "Agent recovery required",
                    "agent_recovery.required",
                    agentx_runtime_contracts::TraceEventKindV1::Updated,
                    "paused",
                    Some("AGENT_RECOVERY_REQUIRED"),
                    json!({"operationId":operation_id}),
                ),
                agentx_agent_core::CoreEventV1::AgentEnded { reason } => (
                    run_id,
                    Some((
                        claim.task.attempt_id,
                        agentx_runtime_contracts::TraceSpanKindV1::Attempt,
                    )),
                    agentx_runtime_contracts::TraceSpanKindV1::AgentRun,
                    "Agent run",
                    "agent_run.finished",
                    agentx_runtime_contracts::TraceEventKindV1::Finished,
                    match reason {
                        agentx_agent_core::TerminalReasonV1::Completed => "succeeded",
                        agentx_agent_core::TerminalReasonV1::Cancelled => "cancelled",
                        agentx_agent_core::TerminalReasonV1::OutcomeUnknown => "outcome_unknown",
                        _ => "failed",
                    },
                    None,
                    json!({"stopReason":format!("{reason:?}")}),
                ),
                agentx_agent_core::CoreEventV1::ToolIntent {
                    operation_id,
                    tool_name,
                } => (
                    crate::worker_support::stable_id(run_id, operation_id.as_bytes()),
                    Some((
                        crate::worker_support::stable_id(run_id, format!("turn-{turn}").as_bytes()),
                        agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                    )),
                    agentx_runtime_contracts::TraceSpanKindV1::RuntimeCall,
                    "Agent tool operation",
                    "agent_tool.intent",
                    agentx_runtime_contracts::TraceEventKindV1::Started,
                    "running",
                    None,
                    json!({"operationId":operation_id,"toolName":tool_name,"turn":turn}),
                ),
                agentx_agent_core::CoreEventV1::ToolSettled {
                    operation_id,
                    tool_name,
                    is_error,
                } => (
                    crate::worker_support::stable_id(run_id, operation_id.as_bytes()),
                    Some((
                        crate::worker_support::stable_id(run_id, format!("turn-{turn}").as_bytes()),
                        agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                    )),
                    agentx_runtime_contracts::TraceSpanKindV1::RuntimeCall,
                    "Agent tool operation",
                    "agent_tool.settled",
                    agentx_runtime_contracts::TraceEventKindV1::Finished,
                    if *is_error { "failed" } else { "succeeded" },
                    None,
                    json!({"operationId":operation_id,"toolName":tool_name,"isError":is_error,"turn":turn}),
                ),
                agentx_agent_core::CoreEventV1::CompactionCompleted { kind } => (
                    crate::worker_support::stable_id(
                        run_id,
                        format!("compaction-{index}").as_bytes(),
                    ),
                    Some((run_id, agentx_runtime_contracts::TraceSpanKindV1::AgentRun)),
                    agentx_runtime_contracts::TraceSpanKindV1::RuntimeCall,
                    "Agent compaction",
                    "agent_compaction.finished",
                    agentx_runtime_contracts::TraceEventKindV1::Finished,
                    "succeeded",
                    None,
                    json!({"kind":kind}),
                ),
            };
            let mut trace = crate::trace_delivery::TraceDraft::span(
                claim.task.tenant_id,
                claim.task.execution_id,
                entity_id,
                parent,
                span_kind,
                span_name,
                event_kind,
                event_type,
                status,
            );
            trace.node_execution_id = Some(claim.task.node_execution_id);
            trace.attempt_id = Some(claim.task.attempt_id);
            trace.agent_run_id = Some(run_id);
            trace.error_code = error_code.map(str::to_owned);
            trace.attributes = attributes;
            let Ok(mut tx) = self.pool.begin().await else {
                continue;
            };
            if crate::trace_delivery::enqueue(&mut tx, trace).await.is_ok() {
                let _ = tx.commit().await;
            }
        }
    }

    pub(super) async fn emit_resolved_parameters(&self, claim: &ClaimedWorkerAttempt) {
        let mut trace = crate::trace_delivery::TraceDraft::span(
            claim.task.tenant_id,
            claim.task.execution_id,
            claim.task.attempt_id,
            Some((
                claim.task.node_execution_id,
                agentx_runtime_contracts::TraceSpanKindV1::Node,
            )),
            agentx_runtime_contracts::TraceSpanKindV1::Attempt,
            "Resolved node parameters",
            agentx_runtime_contracts::TraceEventKindV1::Updated,
            "node.parameters.resolved",
            "running",
        );
        trace.node_execution_id = Some(claim.task.node_execution_id);
        trace.attempt_id = Some(claim.task.attempt_id);
        trace.attributes = json!({"diagnostic":"resolved_parameters","nodeType":claim.node_type,"stringConversions":claim.string_conversions});
        trace.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::ResolvedParameters);
        trace.content_preview = crate::trace_delivery::bounded_preview(&json!({
            "common": claim.node_parameters,
            "perItem": claim.per_item_parameters,
        }));
        let Ok(mut tx) = self.pool.begin().await else {
            return;
        };
        if crate::trace_delivery::enqueue(&mut tx, trace).await.is_ok() {
            let _ = tx.commit().await;
        }
    }

    pub(super) async fn emit_agent_span(
        &self,
        run_id: Uuid,
        iteration_id: Option<Uuid>,
        event_kind: agentx_runtime_contracts::TraceEventKindV1,
        status: &str,
        error_code: Option<&str>,
        content: Option<&Value>,
    ) {
        let row = match sqlx::query("SELECT tenant_id,execution_id,node_execution_id,attempt_id,input_tokens,output_tokens,cost_micros,stop_reason FROM agent_runs WHERE id=?")
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await
        {
            Ok(Some(row)) => row,
            _ => return,
        };
        let Ok(tenant_id) = row.try_get::<Uuid, _>("tenant_id") else {
            return;
        };
        let Ok(execution_id) = row.try_get::<Uuid, _>("execution_id") else {
            return;
        };
        let Ok(attempt_id) = row.try_get::<Uuid, _>("attempt_id") else {
            return;
        };
        let (entity_id, parent, kind, name, event_type) = if let Some(iteration_id) = iteration_id {
            let index = sqlx::query_scalar::<_, u32>(
                "SELECT iteration_index FROM agent_iterations WHERE id=?",
            )
            .bind(iteration_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
            (
                iteration_id,
                (run_id, agentx_runtime_contracts::TraceSpanKindV1::AgentRun),
                agentx_runtime_contracts::TraceSpanKindV1::AgentIteration,
                format!("Iteration {}", index + 1),
                "agent_iteration",
            )
        } else {
            (
                run_id,
                (
                    attempt_id,
                    agentx_runtime_contracts::TraceSpanKindV1::Attempt,
                ),
                agentx_runtime_contracts::TraceSpanKindV1::AgentRun,
                "Agent run".into(),
                "agent_run",
            )
        };
        let mut trace = crate::trace_delivery::TraceDraft::span(
            tenant_id,
            execution_id,
            entity_id,
            Some(parent),
            kind,
            name,
            event_kind,
            format!(
                "{event_type}.{}",
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
        trace.agent_run_id = Some(run_id);
        trace.agent_iteration_id = iteration_id;
        trace.input_tokens = row.try_get("input_tokens").ok();
        trace.output_tokens = row.try_get("output_tokens").ok();
        trace.cost_micros = row.try_get("cost_micros").unwrap_or_default();
        trace.error_code = error_code.map(str::to_owned);
        trace.error_message = (status == "failed")
            .then(|| {
                row.try_get::<Option<String>, _>("stop_reason")
                    .ok()
                    .flatten()
            })
            .flatten();
        trace.attributes =
            json!({"stopReason":row.try_get::<Option<String>, _>("stop_reason").ok().flatten()});
        trace.content_kind = Some(match (iteration_id.is_some(), event_kind) {
            (true, agentx_runtime_contracts::TraceEventKindV1::Started) => {
                agentx_runtime_contracts::TraceContentKindV1::IterationInput
            }
            (true, _) => agentx_runtime_contracts::TraceContentKindV1::IterationOutput,
            (false, agentx_runtime_contracts::TraceEventKindV1::Started) => {
                agentx_runtime_contracts::TraceContentKindV1::AgentInput
            }
            (false, _) => agentx_runtime_contracts::TraceContentKindV1::AgentOutput,
        });
        trace.content_preview = content.and_then(crate::trace_delivery::bounded_preview);
        let Ok(mut tx) = self.pool.begin().await else {
            return;
        };
        if crate::trace_delivery::enqueue(&mut tx, trace).await.is_ok() {
            let _ = tx.commit().await;
        }
    }
}
