use serde_json::{Value, json};

use crate::{
    AgentMessageV1, AgentRunInputV1, AgentSessionStateV1, BudgetDecision, BudgetPort, ClockPort,
    CompactionKindV1, CompactionSnapshotV1, CompactionStrategy, ContextProjector, CoreEventV1,
    EffectContextV1, EventPort, EventPortError, MessageRole, ModelPort, ModelPortError,
    ModelPurposeV1, ModelRequestV1, ModelResponseV1, OperationKindV1, OperationPhaseV1,
    OperationRecordV1, ReplayPolicyV1, StatePort, StatePortError, ToolDefinitionV1, ToolPort,
    ToolPortError, ToolResultV1, core_tool_registry, validate_core_tool_call,
};

#[derive(
    Clone, Debug, serde::Deserialize, Eq, schemars::JsonSchema, PartialEq, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TerminalReasonV1 {
    Completed,
    Cancelled,
    BudgetExhausted,
    ModelError,
    ToolError,
    OutcomeUnknown,
    ContextOverflow,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRunResultV1 {
    pub terminal_reason: TerminalReasonV1,
    pub state: AgentSessionStateV1,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentCoreError {
    #[error("AGENT_CORE_CONTRACT_UNSUPPORTED")]
    ContractUnsupported,
    #[error("AGENT_RUN_INPUT_INVALID: {0}")]
    InvalidInput(String),
    #[error("AGENT_MODEL_REQUIRED")]
    ModelRequired,
    #[error("AGENT_CORE_TOOLS_REQUIRE_WORKSPACE_SANDBOX")]
    CoreToolsRequireWorkspaceSandbox,
    #[error("AGENT_TOOL_NOT_AUTHORIZED: {0}")]
    ToolNotAuthorized(String),
    #[error("AGENT_TOOL_ARGUMENT_INVALID: {0}")]
    ToolArgumentsInvalid(String),
    #[error("AGENT_SESSION_BUSY")]
    SessionBusy,
    #[error("AGENT_SESSION_PROJECTION_INVALID: {0}")]
    SessionProjectionInvalid(String),
    #[error(transparent)]
    State(#[from] StatePortError),
    #[error(transparent)]
    Event(#[from] EventPortError),
}

pub struct AgentCore;

impl AgentCore {
    #[allow(clippy::too_many_arguments)]
    pub fn run<M, T, S, E, C, B>(
        input: &AgentRunInputV1,
        model: &mut M,
        tools: &mut T,
        state_port: &mut S,
        events: &mut E,
        clock: &C,
        budget: &mut B,
    ) -> Result<AgentRunResultV1, AgentCoreError>
    where
        M: ModelPort,
        T: ToolPort,
        S: StatePort,
        E: EventPort,
        C: ClockPort,
        B: BudgetPort,
    {
        if input.api_version != 1 {
            return Err(AgentCoreError::ContractUnsupported);
        }
        if input.run_id.trim().is_empty()
            || input.session_id.trim().is_empty()
            || input.prompt_message_id.trim().is_empty()
            || input.deadline_at_millis == 0
        {
            return Err(AgentCoreError::InvalidInput(
                "runId, sessionId, promptMessageId and deadlineAtMillis are required".into(),
            ));
        }
        if input
            .system_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.len() > 65_536)
        {
            return Err(AgentCoreError::InvalidInput(
                "systemPrompt exceeds the 64 KiB context limit".into(),
            ));
        }
        if input.model_reference.trim().is_empty() {
            return Err(AgentCoreError::ModelRequired);
        }
        let core_tools = core_tool_registry(input.workspace_sandbox_binding.is_some());
        if input.workspace_sandbox_binding.is_none()
            && input
                .attachment_tools
                .iter()
                .any(|tool| matches!(tool.origin, crate::ToolOriginV1::Core))
        {
            return Err(AgentCoreError::CoreToolsRequireWorkspaceSandbox);
        }
        let mut registry = core_tools;
        registry.extend(input.attachment_tools.clone());
        ensure_unique_tool_names(&registry)?;

        if let Some(projection) = &input.session {
            if projection.session_key.trim().is_empty()
                || projection.session_key != input.session_id
            {
                return Err(AgentCoreError::SessionProjectionInvalid(
                    "session key does not match run input".into(),
                ));
            }
            if let Some(subject) = &projection.trusted_subject {
                subject
                    .validate()
                    .map_err(|error| AgentCoreError::SessionProjectionInvalid(error.into()))?;
            }
        }
        let mut state =
            state_port
                .load(&input.session_id)?
                .unwrap_or_else(|| AgentSessionStateV1 {
                    session_id: input.session_id.clone(),
                    ..AgentSessionStateV1::default()
                });
        if state.session_id != input.session_id {
            return Err(AgentCoreError::SessionProjectionInvalid(
                "loaded state session id does not match run input".into(),
            ));
        }
        // An overflow retry marker belongs to one Agent Run.  A later
        // application-session execution gets a fresh bounded retry budget;
        // the marker must not be inferred from the last Compaction snapshot.
        if state
            .overflow_retry_operation_id
            .as_deref()
            .is_some_and(|operation_id| !operation_id.starts_with(&input.run_id))
        {
            state.overflow_retry_operation_id = None;
        }
        if let Some(projection) = &input.session {
            if state.version != projection.state_version {
                return Err(AgentCoreError::SessionProjectionInvalid(format!(
                    "state version {} does not match projection {}",
                    state.version, projection.state_version
                )));
            }
            if projection.leaf_entry_id.is_some() && state.messages.is_empty() {
                return Err(AgentCoreError::SessionProjectionInvalid(
                    "projection leaf has no recoverable entries".into(),
                ));
            }
        }
        // A retry of the same invocation is a reconciliation pass. Once the
        // prompt has already produced a terminal state, return that state
        // without replaying the model effect. A subsequent invocation uses a
        // new prompt message id and intentionally clears the prior terminal.
        if state
            .messages
            .iter()
            .any(|message| message.message_id == input.prompt_message_id)
        {
            if let Some(reason) = state.terminal.clone() {
                events.publish(CoreEventV1::AgentStarted {
                    run_id: input.run_id.clone(),
                })?;
                events.publish(CoreEventV1::AgentEnded {
                    reason: reason.clone(),
                })?;
                return Ok(AgentRunResultV1 {
                    terminal_reason: reason,
                    state,
                });
            }
        }
        state.terminal = None;
        if state
            .steering_queue
            .len()
            .saturating_add(input.steering_inputs.len())
            > 32
            || state
                .follow_up_queue
                .len()
                .saturating_add(input.follow_up_inputs.len())
                > 32
        {
            return Err(AgentCoreError::SessionBusy);
        }
        for context in &input.external_contexts {
            append_once(
                &mut state.messages,
                AgentMessageV1 {
                    message_id: format!("external-context:{}", context.context_id),
                    role: MessageRole::ExternalContext,
                    content: crate::ports::ModelContentV1::Text(context.content.clone()),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    is_error: false,
                },
            );
        }
        // A queued Workflow Execution is resumed with its original prompt.
        // StatePort also projects that durable row into the follow-up queue;
        // consume the queue copy so the prompt is appended exactly once.
        state
            .steering_queue
            .retain(|message| message.message_id != input.prompt_message_id);
        state
            .follow_up_queue
            .retain(|message| message.message_id != input.prompt_message_id);
        append_once(
            &mut state.messages,
            AgentMessageV1::user(&input.prompt_message_id, &input.prompt),
        );
        for message in &input.steering_inputs {
            append_once(&mut state.steering_queue, message.clone());
        }
        for message in &input.follow_up_inputs {
            append_once(&mut state.follow_up_queue, message.clone());
        }
        commit(state_port, &mut state, input.fencing_token)?;
        events.publish(CoreEventV1::AgentStarted {
            run_id: input.run_id.clone(),
        })?;
        events.publish(CoreEventV1::MessageAdded {
            message_id: input.prompt_message_id.clone(),
        })?;

        let mut turn = 0_u32;
        loop {
            turn += 1;
            let projected = project_for_input(input, &state);
            let projected_tokens = ContextProjector::estimated_tokens(&projected);
            match budget.admit_turn(turn, projected_tokens) {
                BudgetDecision::Cancel => {
                    return terminal(
                        state_port,
                        events,
                        state,
                        input.fencing_token,
                        TerminalReasonV1::Cancelled,
                    );
                }
                BudgetDecision::Exhausted => {
                    return terminal(
                        state_port,
                        events,
                        state,
                        input.fencing_token,
                        TerminalReasonV1::BudgetExhausted,
                    );
                }
                BudgetDecision::Continue => {}
            }
            let strategy = CompactionStrategy::new(input.model_context_window);
            let compactable_tokens: u64 = projected
                .iter()
                .filter(|message| {
                    message.message_id != "system-prompt"
                        && !message.message_id.starts_with("external-context:")
                })
                .map(estimate_message_tokens)
                .sum();
            if projected_tokens >= strategy.threshold_tokens()
                && compactable_tokens > strategy.keep_recent_tokens()
                && compaction_needed(&state)
            {
                match compact(
                    input,
                    &registry,
                    &mut state,
                    model,
                    state_port,
                    events,
                    clock,
                    CompactionKindV1::Threshold,
                ) {
                    Ok(()) => {}
                    Err(ModelPortError::OutcomeUnknown(_)) => {
                        return terminal(
                            state_port,
                            events,
                            state,
                            input.fencing_token,
                            TerminalReasonV1::OutcomeUnknown,
                        );
                    }
                    Err(_) => {
                        return terminal(
                            state_port,
                            events,
                            state,
                            input.fencing_token,
                            TerminalReasonV1::ContextOverflow,
                        );
                    }
                }
            }

            events.publish(CoreEventV1::TurnStarted { turn })?;
            let response = match invoke_model(
                input, &registry, &mut state, model, state_port, events, clock,
            ) {
                Ok(response) => response,
                Err(ModelPortError::ContextOverflow) => {
                    // An overflow retry is deliberately bounded to one
                    // compaction.  A second provider overflow is a stable
                    // terminal result; repeatedly summarizing the same
                    // history would make cost and recovery unbounded.
                    let current_operation_id = state
                        .operation
                        .as_ref()
                        .map(|operation| operation.operation_id.as_str());
                    if state
                        .overflow_retry_operation_id
                        .as_deref()
                        .zip(current_operation_id)
                        .is_some_and(|(retry, current)| retry == current)
                    {
                        return terminal_turn_failure(
                            state_port,
                            events,
                            state,
                            input.fencing_token,
                            turn,
                            TerminalReasonV1::ContextOverflow,
                        );
                    }
                    if compact(
                        input,
                        &registry,
                        &mut state,
                        model,
                        state_port,
                        events,
                        clock,
                        CompactionKindV1::Overflow,
                    )
                    .is_err()
                    {
                        return terminal_turn_failure(
                            state_port,
                            events,
                            state,
                            input.fencing_token,
                            turn,
                            TerminalReasonV1::ContextOverflow,
                        );
                    }
                    state.overflow_retry_operation_id = Some(model_operation_id(input, &state));
                    commit(state_port, &mut state, input.fencing_token)?;
                    match invoke_model(
                        input, &registry, &mut state, model, state_port, events, clock,
                    ) {
                        Ok(response) => {
                            state.overflow_retry_operation_id = None;
                            commit(state_port, &mut state, input.fencing_token)?;
                            response
                        }
                        Err(error) => {
                            let reason = model_terminal_reason(&error);
                            return terminal_turn_failure(
                                state_port,
                                events,
                                state,
                                input.fencing_token,
                                turn,
                                reason,
                            );
                        }
                    }
                }
                Err(error) => {
                    let reason = model_terminal_reason(&error);
                    return terminal_turn_failure(
                        state_port,
                        events,
                        state,
                        input.fencing_token,
                        turn,
                        reason,
                    );
                }
            };
            budget.charge_model(projected_tokens, estimated_text_tokens(&response.content));
            append_once(
                &mut state.messages,
                AgentMessageV1::assistant(
                    &response.message_id,
                    &response.content,
                    response.tool_calls.clone(),
                ),
            );
            commit(state_port, &mut state, input.fencing_token)?;
            events.publish(CoreEventV1::MessageAdded {
                message_id: response.message_id.clone(),
            })?;

            if !response.tool_calls.is_empty() {
                for (index, call) in response.tool_calls.iter().enumerate() {
                    let Some(definition) = registry.iter().find(|tool| tool.name == call.name)
                    else {
                        return Err(AgentCoreError::ToolNotAuthorized(call.name.clone()));
                    };
                    if matches!(definition.origin, crate::ToolOriginV1::Core) {
                        validate_core_tool_call(call)
                            .map_err(AgentCoreError::ToolArgumentsInvalid)?;
                    }
                    if state.messages.iter().any(|message| {
                        message.role == MessageRole::ToolResult
                            && message.tool_call_id.as_deref() == Some(&call.call_id)
                    }) {
                        continue;
                    }
                    let result = invoke_tool(
                        input, definition, call, turn, index, &mut state, tools, state_port,
                        events, clock,
                    );
                    let result = match result {
                        Ok(result) => result,
                        Err(ToolPortError::OutcomeUnknown(_)) => {
                            return terminal_turn_failure(
                                state_port,
                                events,
                                state,
                                input.fencing_token,
                                turn,
                                TerminalReasonV1::OutcomeUnknown,
                            );
                        }
                        Err(error) => ToolResultV1 {
                            content: error.to_string(),
                            structured_result: None,
                            artifact_refs: Vec::new(),
                            truncated: false,
                            is_error: true,
                            terminate: false,
                        },
                    };
                    budget.charge_tool(&call.name);
                    let message_id = format!("tool-result:{}", call.call_id);
                    append_once(
                        &mut state.messages,
                        AgentMessageV1 {
                            message_id: message_id.clone(),
                            role: MessageRole::ToolResult,
                            content: crate::ports::ModelContentV1::Text(result.content),
                            tool_calls: Vec::new(),
                            tool_call_id: Some(call.call_id.clone()),
                            is_error: result.is_error,
                        },
                    );
                    commit(state_port, &mut state, input.fencing_token)?;
                    events.publish(CoreEventV1::MessageAdded { message_id })?;
                    if result.terminate {
                        return terminal_turn_failure(
                            state_port,
                            events,
                            state,
                            input.fencing_token,
                            turn,
                            TerminalReasonV1::ToolError,
                        );
                    }
                }
                drain_queue(&mut state.steering_queue, &mut state.messages);
                commit(state_port, &mut state, input.fencing_token)?;
                events.publish(CoreEventV1::TurnEnded {
                    turn,
                    is_error: false,
                })?;
                continue;
            }

            if !state.steering_queue.is_empty() {
                drain_queue(&mut state.steering_queue, &mut state.messages);
                commit(state_port, &mut state, input.fencing_token)?;
                events.publish(CoreEventV1::TurnEnded {
                    turn,
                    is_error: false,
                })?;
                continue;
            }
            if !state.follow_up_queue.is_empty() {
                drain_queue(&mut state.follow_up_queue, &mut state.messages);
                commit(state_port, &mut state, input.fencing_token)?;
                events.publish(CoreEventV1::TurnEnded {
                    turn,
                    is_error: false,
                })?;
                continue;
            }
            events.publish(CoreEventV1::TurnEnded {
                turn,
                is_error: false,
            })?;
            return terminal(
                state_port,
                events,
                state,
                input.fencing_token,
                TerminalReasonV1::Completed,
            );
        }
    }
}

fn ensure_unique_tool_names(registry: &[ToolDefinitionV1]) -> Result<(), AgentCoreError> {
    let mut names = std::collections::BTreeSet::new();
    for tool in registry {
        if !names.insert(&tool.name) {
            return Err(AgentCoreError::ToolNotAuthorized(format!(
                "duplicate tool name {}",
                tool.name
            )));
        }
    }
    Ok(())
}

fn project_for_input(input: &AgentRunInputV1, state: &AgentSessionStateV1) -> Vec<AgentMessageV1> {
    ContextProjector::project_with_system_prompt(state, input.system_prompt.as_deref())
}

fn append_once(messages: &mut Vec<AgentMessageV1>, message: AgentMessageV1) {
    if !messages
        .iter()
        .any(|current| current.message_id == message.message_id)
    {
        messages.push(message);
    }
}

fn drain_queue(queue: &mut Vec<AgentMessageV1>, messages: &mut Vec<AgentMessageV1>) {
    for message in std::mem::take(queue) {
        append_once(messages, message);
    }
}

fn invoke_model<M, S, E, C>(
    input: &AgentRunInputV1,
    registry: &[ToolDefinitionV1],
    state: &mut AgentSessionStateV1,
    model: &mut M,
    state_port: &mut S,
    events: &mut E,
    clock: &C,
) -> Result<ModelResponseV1, ModelPortError>
where
    M: ModelPort,
    S: StatePort,
    E: EventPort,
    C: ClockPort,
{
    let pending_tool_response = state
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, MessageRole::Assistant))
        .filter(|message| !message.tool_calls.is_empty())
        .filter(|message| {
            message.tool_calls.iter().any(|call| {
                !state.messages.iter().any(|candidate| {
                    candidate.role == MessageRole::ToolResult
                        && candidate.tool_call_id.as_deref() == Some(&call.call_id)
                })
            })
        })
        .cloned();
    if let Some(message) = pending_tool_response {
        return Ok(ModelResponseV1 {
            message_id: message.message_id,
            content: message.content.as_text().unwrap_or_default().to_owned(),
            tool_calls: message.tool_calls,
        });
    }
    let index = state.messages.len();
    let operation_id = model_operation_id(input, state);
    let context = effect_context(input, &operation_id, clock);
    // Reconcile a model effect that was durably settled before the worker
    // crashed while appending the assistant message. The response is stored
    // in Operation State, so retrying the same turn must not invoke the
    // provider again. An in-flight non-idempotent effect is explicitly
    // surfaced as unknown instead of being blindly replayed.
    if let Some(operation) = state
        .operation
        .as_ref()
        .filter(|operation| matches!(operation.operation_kind, OperationKindV1::Model))
        .filter(|operation| match &operation.phase {
            OperationPhaseV1::SettledSuccess { result } => result
                .get("messageId")
                .and_then(Value::as_str)
                .is_some_and(|message_id| {
                    !state
                        .messages
                        .iter()
                        .any(|message| message.message_id == message_id)
                }),
            _ => true,
        })
    {
        match &operation.phase {
            OperationPhaseV1::SettledSuccess { result } => {
                if let Ok(response) = serde_json::from_value::<ModelResponseV1>(result.clone()) {
                    return Ok(response);
                }
                return Err(ModelPortError::OutcomeUnknown(
                    "settled model effect has an invalid stored response".into(),
                ));
            }
            OperationPhaseV1::EffectStarted | OperationPhaseV1::EffectCompleted { .. } => {
                return Err(ModelPortError::OutcomeUnknown(
                    "model effect was sent but settlement is not confirmed".into(),
                ));
            }
            OperationPhaseV1::UnknownOutcome { reason } => {
                return Err(ModelPortError::OutcomeUnknown(reason.clone()));
            }
            OperationPhaseV1::IntentPersisted | OperationPhaseV1::SettledFailure { .. } => {}
        }
    }
    state.operation = Some(operation(
        input,
        &context,
        OperationKindV1::Model,
        ReplayPolicyV1::LedgerDependent,
        json!({"messageCount": index}),
        clock,
    ));
    commit_model_state(state_port, state, input.fencing_token)?;
    events
        .publish(CoreEventV1::ModelIntent {
            operation_id: operation_id.clone(),
        })
        .map_err(|error| ModelPortError::Effect(error.to_string()))?;
    if let Some(operation) = &mut state.operation {
        operation.phase = OperationPhaseV1::EffectStarted;
        operation.updated_at_millis = clock.now_millis();
    }
    commit_model_state(state_port, state, input.fencing_token)?;
    let request = ModelRequestV1 {
        purpose: ModelPurposeV1::AgentTurn,
        model_reference: input.model_reference.clone(),
        messages: project_for_input(input, state),
        tools: registry.to_vec(),
    };
    // Streaming turn (plan7 P7-B): the runtime adapter routes the deltas to
    // the live invocation event stream; the settlement contract is identical
    // to the buffered invoke.
    let response = match model.invoke_stream(
        &request,
        &context,
        &|chunk: &crate::ports::ModelDeltaChunk| {
            let _ = chunk;
        },
    ) {
        Ok(response) => response,
        Err(error) => {
            fail_operation(
                state,
                &error.to_string(),
                matches!(&error, ModelPortError::OutcomeUnknown(_)),
                clock,
            );
            commit_model_state(state_port, state, input.fencing_token)?;
            events
                .publish(CoreEventV1::ModelSettled {
                    operation_id,
                    is_error: true,
                })
                .map_err(|error| ModelPortError::Effect(error.to_string()))?;
            return Err(error);
        }
    };
    settle_operation(
        state,
        serde_json::to_value(&response).unwrap_or(Value::Null),
        clock,
    );
    commit_model_state(state_port, state, input.fencing_token)?;
    events
        .publish(CoreEventV1::ModelSettled {
            operation_id,
            is_error: false,
        })
        .map_err(|error| ModelPortError::Effect(error.to_string()))?;
    Ok(response)
}

fn model_operation_id(input: &AgentRunInputV1, state: &AgentSessionStateV1) -> String {
    let index = state.messages.len();
    let operation_suffix = state
        .compaction
        .as_ref()
        .map(|snapshot| format!(":{}", snapshot.snapshot_id))
        .unwrap_or_default();
    format!("{}:model:{index}{operation_suffix}", input.run_id)
}

#[allow(clippy::too_many_arguments)]
fn invoke_tool<T, S, E, C>(
    input: &AgentRunInputV1,
    definition: &ToolDefinitionV1,
    call: &crate::ToolCallV1,
    turn: u32,
    index: usize,
    state: &mut AgentSessionStateV1,
    tools: &mut T,
    state_port: &mut S,
    events: &mut E,
    clock: &C,
) -> Result<ToolResultV1, ToolPortError>
where
    T: ToolPort,
    S: StatePort,
    E: EventPort,
    C: ClockPort,
{
    let operation_id = format!("{}:tool:{turn}:{index}", input.run_id);
    let context = effect_context(input, &operation_id, clock);
    if let Some(operation) = state
        .operation
        .as_ref()
        .filter(|operation| operation.operation_id == operation_id)
    {
        match &operation.phase {
            OperationPhaseV1::SettledSuccess { result } => {
                return serde_json::from_value(result.clone()).map_err(|error| {
                    ToolPortError::OutcomeUnknown(format!(
                        "settled tool result is invalid: {error}"
                    ))
                });
            }
            OperationPhaseV1::EffectStarted | OperationPhaseV1::EffectCompleted { .. } => {
                return Err(ToolPortError::OutcomeUnknown(
                    "tool effect was sent but settlement is not confirmed".into(),
                ));
            }
            OperationPhaseV1::UnknownOutcome { reason } => {
                return Err(ToolPortError::OutcomeUnknown(reason.clone()));
            }
            OperationPhaseV1::IntentPersisted | OperationPhaseV1::SettledFailure { .. } => {}
        }
    }
    state.operation = Some(operation(
        input,
        &context,
        OperationKindV1::Tool,
        definition.replay_policy,
        serde_json::to_value(call).unwrap_or(Value::Null),
        clock,
    ));
    commit_tool_state(state_port, state, input.fencing_token)?;
    events
        .publish(CoreEventV1::ToolIntent {
            operation_id: operation_id.clone(),
            tool_name: call.name.clone(),
        })
        .map_err(|error| ToolPortError::Effect(error.to_string()))?;
    if let Some(operation) = &mut state.operation {
        operation.phase = OperationPhaseV1::EffectStarted;
        operation.updated_at_millis = clock.now_millis();
    }
    commit_tool_state(state_port, state, input.fencing_token)?;
    let result = match tools.execute(call, &context) {
        Ok(result) => result,
        Err(error) => {
            fail_operation(
                state,
                &error.to_string(),
                matches!(&error, ToolPortError::OutcomeUnknown(_)),
                clock,
            );
            commit_tool_state(state_port, state, input.fencing_token)?;
            events
                .publish(CoreEventV1::ToolSettled {
                    operation_id,
                    tool_name: call.name.clone(),
                    is_error: true,
                })
                .map_err(|error| ToolPortError::Effect(error.to_string()))?;
            return Err(error);
        }
    };
    settle_operation(
        state,
        serde_json::to_value(&result).unwrap_or(Value::Null),
        clock,
    );
    commit_tool_state(state_port, state, input.fencing_token)?;
    events
        .publish(CoreEventV1::ToolSettled {
            operation_id,
            tool_name: call.name.clone(),
            is_error: result.is_error,
        })
        .map_err(|error| ToolPortError::Effect(error.to_string()))?;
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn compact<M, S, E, C>(
    input: &AgentRunInputV1,
    _registry: &[ToolDefinitionV1],
    state: &mut AgentSessionStateV1,
    model: &mut M,
    state_port: &mut S,
    events: &mut E,
    clock: &C,
    kind: CompactionKindV1,
) -> Result<(), ModelPortError>
where
    M: ModelPort,
    S: StatePort,
    E: EventPort,
    C: ClockPort,
{
    let operation_id = format!("{}:compaction:{}", input.run_id, state.messages.len());
    let context = effect_context(input, &operation_id, clock);
    if let Some(operation) = state
        .operation
        .as_ref()
        .filter(|operation| operation.operation_id == operation_id)
    {
        match &operation.phase {
            OperationPhaseV1::SettledSuccess { result } => {
                if let (Some(summary), Some(snapshot_id)) = (
                    result.get("summary").and_then(Value::as_str),
                    result.get("snapshotId").and_then(Value::as_str),
                ) {
                    let summary = summary.to_owned();
                    let snapshot_id = snapshot_id.to_owned();
                    let tokens_before = result
                        .get("tokensBefore")
                        .and_then(Value::as_u64)
                        .or_else(|| result.get("tokens_before").and_then(Value::as_u64))
                        .unwrap_or_else(|| {
                            ContextProjector::estimated_tokens(&project_for_input(input, state))
                        });
                    apply_compaction_snapshot(
                        state,
                        kind,
                        &snapshot_id,
                        &summary,
                        input.model_context_window,
                        tokens_before,
                    );
                    commit_model_state(state_port, state, input.fencing_token)?;
                    return Ok(());
                }
                return Err(ModelPortError::OutcomeUnknown(
                    "settled compaction has no recoverable summary".into(),
                ));
            }
            OperationPhaseV1::EffectStarted | OperationPhaseV1::EffectCompleted { .. } => {
                return Err(ModelPortError::OutcomeUnknown(
                    "compaction effect was sent but settlement is not confirmed".into(),
                ));
            }
            OperationPhaseV1::UnknownOutcome { reason } => {
                return Err(ModelPortError::OutcomeUnknown(reason.clone()));
            }
            OperationPhaseV1::IntentPersisted | OperationPhaseV1::SettledFailure { .. } => {}
        }
    }
    state.operation = Some(operation(
        input,
        &context,
        OperationKindV1::Compaction,
        ReplayPolicyV1::LedgerDependent,
        json!({"kind": kind}),
        clock,
    ));
    commit_model_state(state_port, state, input.fencing_token)?;
    if let Some(operation) = &mut state.operation {
        operation.phase = OperationPhaseV1::EffectStarted;
        operation.updated_at_millis = clock.now_millis();
    }
    commit_model_state(state_port, state, input.fencing_token)?;
    let messages = project_for_input(input, state);
    let tokens_before = ContextProjector::estimated_tokens(&messages);
    let response = model.invoke(
        &ModelRequestV1 {
            purpose: ModelPurposeV1::Compaction,
            model_reference: input.model_reference.clone(),
            messages,
            // Compaction is a summarization effect, never a tool-bearing
            // Model turn.  This prevents a provider from executing an
            // attachment while the durable summary is being produced.
            tools: Vec::new(),
        },
        &context,
    )?;
    let snapshot_id = response.message_id.clone();
    let summary = response.content.clone();
    apply_compaction_snapshot(
        state,
        kind,
        &snapshot_id,
        &summary,
        input.model_context_window,
        tokens_before,
    );
    settle_operation(
        state,
        json!({
            "compacted": true,
            "snapshotId": snapshot_id,
            "summary": summary,
            "tokensBefore": tokens_before,
            "tokensAfter": ContextProjector::estimated_tokens(&project_for_input(input, state)),
            "kind": kind,
        }),
        clock,
    );
    commit_model_state(state_port, state, input.fencing_token)?;
    events
        .publish(CoreEventV1::CompactionCompleted { kind })
        .map_err(|error| ModelPortError::Effect(error.to_string()))?;
    Ok(())
}

/// 估算单条消息的 token 数
fn estimate_message_tokens(msg: &AgentMessageV1) -> u64 {
    let content_tokens = (msg.content.as_text().map(str::len).unwrap_or(0) as u64).div_ceil(4);
    let tool_tokens: u64 = msg
        .tool_calls
        .iter()
        .map(|tc| (tc.arguments.to_string().chars().count() as u64).div_ceil(4))
        .sum();
    content_tokens + tool_tokens + 4 // +4 for message overhead
}

/// Retains a contiguous suffix of complete user turns. A tool response cannot
/// be retained without the assistant call that precedes it. Oversized turns
/// are represented by the summary instead of being reinserted over budget.
fn select_retained_tail(
    messages: &[AgentMessageV1],
    keep_recent_tokens: u64,
) -> Vec<AgentMessageV1> {
    if messages.is_empty() {
        return Vec::new();
    }
    let mut boundaries = vec![0];
    boundaries.extend(
        messages
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, message)| message.role == MessageRole::User)
            .map(|(index, _)| index),
    );
    boundaries.push(messages.len());
    let mut retained_start = messages.len();
    let mut token_budget = keep_recent_tokens;
    for turn in boundaries.windows(2).rev() {
        let tokens: u64 = messages[turn[0]..turn[1]]
            .iter()
            .map(estimate_message_tokens)
            .sum();
        if tokens > token_budget {
            break;
        }
        token_budget -= tokens;
        retained_start = turn[0];
    }
    messages[retained_start..].to_vec()
}

fn compaction_needed(state: &AgentSessionStateV1) -> bool {
    let Some(compaction) = &state.compaction else {
        return true;
    };
    let retained_end = compaction
        .through_message_index
        .saturating_add(1)
        .saturating_add(compaction.retained_tail.len());
    state.messages.len() > retained_end
}

fn apply_compaction_snapshot(
    state: &mut AgentSessionStateV1,
    kind: CompactionKindV1,
    snapshot_id: &str,
    summary: &str,
    model_context_window: u64,
    tokens_before: u64,
) {
    const MAX_SUMMARY_CHARS: usize = 16_384;
    let bounded_summary = if summary.chars().count() > MAX_SUMMARY_CHARS {
        let mut value = summary.chars().take(MAX_SUMMARY_CHARS).collect::<String>();
        value.push_str("\n[summary truncated]");
        value
    } else {
        summary.to_owned()
    };

    let strategy = CompactionStrategy::new(model_context_window);
    let mut retained_tail = select_retained_tail(&state.messages, strategy.keep_recent_tokens());
    if !state.messages.is_empty() && retained_tail.len() == state.messages.len() {
        // An overflow-triggered compaction must replace at least one turn;
        // retaining the whole history cannot represent a summarized prefix.
        let next_turn = state
            .messages
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, message)| message.role == MessageRole::User)
            .map_or(state.messages.len(), |(index, _)| index);
        retained_tail = state.messages[next_turn..].to_vec();
    }

    let retained_start = state.messages.len().saturating_sub(retained_tail.len());

    state.compaction = Some(CompactionSnapshotV1 {
        snapshot_id: snapshot_id.to_owned(),
        kind,
        summary: bounded_summary,
        through_message_index: retained_start.saturating_sub(1),
        retained_tail,
        tokens_before,
    });
}

fn operation<C: ClockPort>(
    input: &AgentRunInputV1,
    context: &EffectContextV1,
    operation_kind: OperationKindV1,
    replay_policy: ReplayPolicyV1,
    intent: Value,
    clock: &C,
) -> OperationRecordV1 {
    OperationRecordV1 {
        operation_id: context.operation_id.clone(),
        effect_id: context.effect_id.clone(),
        operation_kind,
        attempt: 1,
        intent,
        effect_identity: context.idempotency_key.clone(),
        replay_policy,
        phase: OperationPhaseV1::IntentPersisted,
        fencing_token: input.fencing_token,
        created_at_millis: clock.now_millis(),
        updated_at_millis: clock.now_millis(),
    }
}

fn effect_context<C: ClockPort>(
    input: &AgentRunInputV1,
    operation_id: &str,
    _clock: &C,
) -> EffectContextV1 {
    let effect_id = format!("{operation_id}:effect");
    EffectContextV1 {
        run_id: input.run_id.clone(),
        operation_id: operation_id.into(),
        effect_id: effect_id.clone(),
        // Fencing proves which Worker may settle state; it is deliberately not
        // part of effect identity so a replacement Worker reconciles the same
        // provider operation instead of creating a second side effect.
        idempotency_key: format!("{}:{effect_id}", input.run_id),
        fencing_token: input.fencing_token,
        // The deadline belongs to the accepted Operation and must remain
        // stable across Worker replacement.  Extending it to `now` would
        // silently turn an expired operation into an unbounded retry.
        deadline_at_millis: input.deadline_at_millis,
    }
}

fn settle_operation<C: ClockPort>(state: &mut AgentSessionStateV1, result: Value, clock: &C) {
    if let Some(operation) = &mut state.operation {
        operation.phase = OperationPhaseV1::SettledSuccess { result };
        operation.updated_at_millis = clock.now_millis();
    }
}

fn fail_operation<C: ClockPort>(
    state: &mut AgentSessionStateV1,
    error: &str,
    outcome_unknown: bool,
    clock: &C,
) {
    if let Some(operation) = &mut state.operation {
        operation.phase = if outcome_unknown {
            OperationPhaseV1::UnknownOutcome {
                reason: error.into(),
            }
        } else {
            OperationPhaseV1::SettledFailure {
                error: error.into(),
            }
        };
        operation.updated_at_millis = clock.now_millis();
    }
}

fn commit<S: StatePort>(
    state_port: &mut S,
    state: &mut AgentSessionStateV1,
    fencing_token: u64,
) -> Result<(), StatePortError> {
    let version = state_port.commit(state.version, state, fencing_token)?;
    state.version = version;
    Ok(())
}

fn commit_model_state<S: StatePort>(
    state_port: &mut S,
    state: &mut AgentSessionStateV1,
    fencing_token: u64,
) -> Result<(), ModelPortError> {
    commit(state_port, state, fencing_token)
        .map_err(|error| ModelPortError::Effect(error.to_string()))
}

fn commit_tool_state<S: StatePort>(
    state_port: &mut S,
    state: &mut AgentSessionStateV1,
    fencing_token: u64,
) -> Result<(), ToolPortError> {
    commit(state_port, state, fencing_token)
        .map_err(|error| ToolPortError::Effect(error.to_string()))
}

fn model_terminal_reason(error: &ModelPortError) -> TerminalReasonV1 {
    match error {
        ModelPortError::Cancelled => TerminalReasonV1::Cancelled,
        ModelPortError::OutcomeUnknown(_) => TerminalReasonV1::OutcomeUnknown,
        ModelPortError::ContextOverflow => TerminalReasonV1::ContextOverflow,
        ModelPortError::Effect(_) => TerminalReasonV1::ModelError,
    }
}

fn terminal<S: StatePort, E: EventPort>(
    state_port: &mut S,
    events: &mut E,
    mut state: AgentSessionStateV1,
    fencing_token: u64,
    reason: TerminalReasonV1,
) -> Result<AgentRunResultV1, AgentCoreError> {
    state.terminal = Some(reason.clone());
    commit(state_port, &mut state, fencing_token)?;
    events.publish(CoreEventV1::AgentEnded {
        reason: reason.clone(),
    })?;
    Ok(AgentRunResultV1 {
        terminal_reason: reason,
        state,
    })
}

/// Terminal exit from inside an open turn. The turn span must close with an
/// error status before the run settles, otherwise the trace keeps a dangling
/// "Agent turn" span in the running state forever.
fn terminal_turn_failure<S: StatePort, E: EventPort>(
    state_port: &mut S,
    events: &mut E,
    state: AgentSessionStateV1,
    fencing_token: u64,
    turn: u32,
    reason: TerminalReasonV1,
) -> Result<AgentRunResultV1, AgentCoreError> {
    events.publish(CoreEventV1::TurnEnded {
        turn,
        is_error: true,
    })?;
    terminal(state_port, events, state, fencing_token, reason)
}

fn estimated_text_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedClock;
    impl ClockPort for FixedClock {
        fn now_millis(&self) -> u64 {
            100
        }
    }

    fn input(fencing_token: u64) -> AgentRunInputV1 {
        AgentRunInputV1 {
            api_version: 1,
            run_id: "run-1".into(),
            session_id: "session-1".into(),
            session: None,
            fencing_token,
            deadline_at_millis: 1_000,
            model_reference: "model@1".into(),
            system_prompt: None,
            workspace_sandbox_binding: None,
            attachment_tools: Vec::new(),
            external_contexts: Vec::new(),
            prompt_message_id: "prompt-1".into(),
            prompt: "hello".into(),
            steering_inputs: Vec::new(),
            follow_up_inputs: Vec::new(),
            model_context_window: 200_000,
        }
    }

    #[test]
    fn replacement_worker_keeps_effect_identity_but_changes_fencing_proof() {
        let first = effect_context(&input(3), "run-1:model:0", &FixedClock);
        let replacement = effect_context(&input(4), "run-1:model:0", &FixedClock);
        assert_eq!(first.operation_id, replacement.operation_id);
        assert_eq!(first.effect_id, replacement.effect_id);
        assert_eq!(first.idempotency_key, replacement.idempotency_key);
        assert_ne!(first.fencing_token, replacement.fencing_token);
    }
}
