use std::collections::VecDeque;

use agentx_agent_core::*;
use serde_json::json;

struct FakeModel {
    responses: VecDeque<Result<ModelResponseV1, ModelPortError>>,
    requests: Vec<ModelRequestV1>,
}

impl ModelPort for FakeModel {
    fn invoke(
        &mut self,
        request: &ModelRequestV1,
        _context: &EffectContextV1,
    ) -> Result<ModelResponseV1, ModelPortError> {
        self.requests.push(request.clone());
        self.responses.pop_front().expect("scripted model response")
    }
}

#[derive(Default)]
struct FakeTools {
    calls: Vec<ToolCallV1>,
    result: Option<Result<ToolResultV1, ToolPortError>>,
}

impl ToolPort for FakeTools {
    fn execute(
        &mut self,
        call: &ToolCallV1,
        _context: &EffectContextV1,
    ) -> Result<ToolResultV1, ToolPortError> {
        self.calls.push(call.clone());
        self.result.take().unwrap_or_else(|| {
            Ok(ToolResultV1 {
                content: "tool-ok".into(),
                structured_result: None,
                artifact_refs: Vec::new(),
                truncated: false,
                is_error: false,
                terminate: false,
            })
        })
    }
}

#[derive(Default)]
struct MemoryState {
    state: Option<AgentSessionStateV1>,
    fencing_token: u64,
    commits: usize,
}

impl StatePort for MemoryState {
    fn load(&mut self, _session_id: &str) -> Result<Option<AgentSessionStateV1>, StatePortError> {
        Ok(self.state.clone())
    }

    fn commit(
        &mut self,
        expected_version: u64,
        state: &AgentSessionStateV1,
        fencing_token: u64,
    ) -> Result<u64, StatePortError> {
        if self.fencing_token != 0 && fencing_token < self.fencing_token {
            return Err(StatePortError::LeaseLost);
        }
        if self
            .state
            .as_ref()
            .is_some_and(|current| current.version != expected_version)
        {
            return Err(StatePortError::Conflict);
        }
        self.fencing_token = fencing_token;
        let mut persisted = state.clone();
        persisted.version = expected_version + 1;
        self.state = Some(persisted);
        self.commits += 1;
        Ok(expected_version + 1)
    }
}

#[derive(Default)]
struct Events(Vec<CoreEventV1>);

impl EventPort for Events {
    fn publish(&mut self, event: CoreEventV1) -> Result<(), EventPortError> {
        self.0.push(event);
        Ok(())
    }
}

struct FixedClock;

impl ClockPort for FixedClock {
    fn now_millis(&self) -> u64 {
        1_777_777
    }
}

struct FixedBudget {
    decision: BudgetDecision,
    admitted: usize,
    tools: usize,
}

impl BudgetPort for FixedBudget {
    fn admit_turn(&mut self, _turn: u32, _projected_tokens: u64) -> BudgetDecision {
        self.admitted += 1;
        self.decision
    }

    fn charge_tool(&mut self, _tool_name: &str) {
        self.tools += 1;
    }
}

fn input(sandbox: bool) -> AgentRunInputV1 {
    AgentRunInputV1 {
        api_version: 1,
        run_id: "run-1".into(),
        session_id: "session-1".into(),
        session: None,
        fencing_token: 3,
        deadline_at_millis: 9_999_999,
        model_reference: "model-version-1".into(),
        system_prompt: None,
        workspace_sandbox_binding: sandbox.then(|| "sandbox-binding-1".into()),
        attachment_tools: vec![],
        external_contexts: vec![],
        prompt_message_id: "user-1".into(),
        prompt: "hello".into(),
        steering_inputs: vec![],
        follow_up_inputs: vec![],
        model_context_window: 200_000,
    }
}

fn response(
    id: &str,
    text: &str,
    calls: Vec<ToolCallV1>,
) -> Result<ModelResponseV1, ModelPortError> {
    Ok(ModelResponseV1 {
        message_id: id.into(),
        content: text.into(),
        tool_calls: calls,
    })
}

fn budget(decision: BudgetDecision) -> FixedBudget {
    FixedBudget {
        decision,
        admitted: 0,
        tools: 0,
    }
}

#[test]
fn sandbox_controls_registration_and_tool_calls_stay_behind_port() {
    let call = ToolCallV1 {
        call_id: "call-1".into(),
        name: "read".into(),
        arguments: json!({"path":"notes.txt"}),
    };
    let mut model = FakeModel {
        responses: VecDeque::from([
            response("assistant-1", "", vec![call]),
            response("assistant-2", "done", vec![]),
        ]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let mut budget = budget(BudgetDecision::Continue);
    let result = AgentCore::run(
        &input(true),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget,
    )
    .expect("agent completes");

    assert_eq!(result.terminal_reason, TerminalReasonV1::Completed);
    assert_eq!(tools.calls.len(), 1);
    assert_eq!(budget.tools, 1);
    assert_eq!(
        model.requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["read", "write", "edit", "bash"]
    );
    assert!(result.state.messages.iter().any(|message| {
        message.role == MessageRole::ToolResult && message.tool_call_id.as_deref() == Some("call-1")
    }));
}

#[test]
fn no_sandbox_means_model_cannot_see_core_tools() {
    let mut model = FakeModel {
        responses: VecDeque::from([response("assistant-1", "done", vec![])]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    AgentCore::run(
        &input(false),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("agent completes");
    assert!(model.requests[0].tools.is_empty());
}

#[test]
fn retry_of_settled_prompt_reconciles_without_replaying_model() {
    let mut model = FakeModel {
        responses: VecDeque::from([response("assistant-1", "done", vec![])]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let mut first_budget = budget(BudgetDecision::Continue);
    let first = AgentCore::run(
        &input(false),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut first_budget,
    )
    .expect("first run completes");
    assert_eq!(first.terminal_reason, TerminalReasonV1::Completed);
    let mut retry_budget = budget(BudgetDecision::Continue);
    let retry = AgentCore::run(
        &input(false),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut retry_budget,
    )
    .expect("retry reconciles settled state");
    assert_eq!(retry.terminal_reason, TerminalReasonV1::Completed);
    assert_eq!(model.requests.len(), 1);
}

#[test]
fn settled_model_effect_is_reconciled_before_assistant_message_append() {
    let mut model = FakeModel {
        responses: VecDeque::from([response("assistant-1", "done", vec![])]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    AgentCore::run(
        &input(false),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("first run completes");
    let persisted = state.state.as_mut().expect("state persisted");
    persisted
        .messages
        .retain(|message| message.message_id != "assistant-1");
    persisted.terminal = None;
    let mut retry_budget = budget(BudgetDecision::Continue);
    let recovered = AgentCore::run(
        &input(false),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut retry_budget,
    )
    .expect("settled model effect is recovered");
    assert_eq!(recovered.terminal_reason, TerminalReasonV1::Completed);
    assert_eq!(model.requests.len(), 1);
    assert!(
        recovered
            .state
            .messages
            .iter()
            .any(|message| message.message_id == "assistant-1")
    );
}

#[test]
fn steering_precedes_follow_up_and_both_create_new_turns() {
    let mut run_input = input(false);
    run_input.steering_inputs = vec![AgentMessageV1::user("steer-1", "steer")];
    run_input.follow_up_inputs = vec![AgentMessageV1::user("follow-1", "follow")];
    let mut model = FakeModel {
        responses: VecDeque::from([
            response("assistant-1", "first", vec![]),
            response("assistant-2", "second", vec![]),
            response("assistant-3", "third", vec![]),
        ]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let result = AgentCore::run(
        &run_input,
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("agent completes");
    let ids = result
        .state
        .messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect::<Vec<_>>();
    assert!(
        ids.iter().position(|id| *id == "steer-1") < ids.iter().position(|id| *id == "follow-1")
    );
    assert_eq!(model.requests.len(), 3);
}

#[test]
fn overflow_compacts_then_retries_the_turn() {
    let mut model = FakeModel {
        responses: VecDeque::from([
            Err(ModelPortError::ContextOverflow),
            response("compaction-1", "summary", vec![]),
            response("assistant-1", "done", vec![]),
        ]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let result = AgentCore::run(
        &input(false),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("agent recovers from overflow");
    assert_eq!(
        result.state.compaction.as_ref().map(|item| item.kind),
        Some(CompactionKindV1::Overflow)
    );
    assert_eq!(model.requests[1].purpose, ModelPurposeV1::Compaction);
}

#[test]
fn threshold_compaction_projects_summary_tail_and_recent_messages() {
    let mut run_input = input(false);
    run_input.model_context_window = 100;
    let existing = AgentSessionStateV1 {
        session_id: run_input.session_id.clone(),
        version: 7,
        messages: vec![
            AgentMessageV1::user(
                "old-1",
                "a long old message that must be summarized because it contains many many many many many many many many many many many many many many many many many many many many many many many many words",
            ),
            AgentMessageV1::assistant(
                "old-2",
                "another old message with lots of content that should be compressed because it is very very very very very very very very very very very very very very very very very very very very very very long",
                vec![],
            ),
        ],
        ..AgentSessionStateV1::default()
    };
    let mut model = FakeModel {
        responses: VecDeque::from([
            response("compaction-1", "stable summary", vec![]),
            response("assistant-1", "done", vec![]),
        ]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState {
        state: Some(existing),
        fencing_token: 3,
        commits: 0,
    };
    let mut events = Events::default();
    let result = AgentCore::run(
        &run_input,
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("threshold compaction completes");
    assert_eq!(
        result.state.compaction.as_ref().map(|item| item.kind),
        Some(CompactionKindV1::Threshold)
    );
    assert_eq!(model.requests[0].purpose, ModelPurposeV1::Compaction);
    assert!(
        model.requests[1]
            .messages
            .iter()
            .any(|message| message.content.as_text() == Some("stable summary"))
    );
}

#[test]
fn threshold_compaction_does_not_reinsert_an_oversized_turn_or_drop_new_tool_results() {
    let mut run_input = input(true);
    run_input.model_context_window = 712;
    run_input.prompt = "context ".repeat(400);
    let call = ToolCallV1 {
        call_id: "call-1".into(),
        name: "read".into(),
        arguments: json!({"path":"notes.txt"}),
    };
    let mut model = FakeModel {
        responses: VecDeque::from([
            response("compaction-1", "short summary", vec![]),
            response("assistant-1", "", vec![call]),
            response("assistant-2", "done", vec![]),
        ]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let result = AgentCore::run(
        &run_input,
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("compacted agent completes");

    assert_eq!(result.terminal_reason, TerminalReasonV1::Completed);
    assert_eq!(model.requests.len(), 3);
    assert_eq!(model.requests[0].purpose, ModelPurposeV1::Compaction);
    assert!(model.requests[2].messages.iter().any(|message| {
        message.role == MessageRole::ToolResult && message.tool_call_id.as_deref() == Some("call-1")
    }));
    assert!(
        !model.requests[2]
            .messages
            .iter()
            .any(|message| message.content.as_text() == Some(run_input.prompt.as_str()))
    );
}

#[test]
fn settled_tool_effect_is_recovered_without_second_tool_call() {
    let call = ToolCallV1 {
        call_id: "call-1".into(),
        name: "read".into(),
        arguments: json!({"path":"notes.txt"}),
    };
    let mut model = FakeModel {
        responses: VecDeque::from([
            response("assistant-1", "", vec![call.clone()]),
            response("assistant-2", "done", vec![]),
            response("assistant-3", "done", vec![]),
        ]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    AgentCore::run(
        &input(true),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .unwrap();
    let tool_calls_before = tools.calls.len();
    let persisted = state.state.as_mut().unwrap();
    persisted.terminal = None;
    persisted.messages.retain(|message| {
        !(message.role == MessageRole::ToolResult
            && message.tool_call_id.as_deref() == Some("call-1"))
    });
    persisted.operation = Some(OperationRecordV1 {
        operation_id: "run-1:tool:1:0".into(),
        effect_id: "run-1:tool:1:0:effect".into(),
        operation_kind: OperationKindV1::Tool,
        attempt: 1,
        intent: json!(call),
        effect_identity: "run-1:tool:1:0:effect".into(),
        replay_policy: ReplayPolicyV1::Never,
        phase: OperationPhaseV1::SettledSuccess {
            result: json!(ToolResultV1 {
                content: "tool-ok".into(),
                structured_result: None,
                artifact_refs: Vec::<String>::new(),
                truncated: false,
                is_error: false,
                terminate: false,
            }),
        },
        fencing_token: 3,
        created_at_millis: 1,
        updated_at_millis: 1,
    });
    let recovered = AgentCore::run(
        &input(true),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .unwrap();
    assert_eq!(recovered.terminal_reason, TerminalReasonV1::Completed);
    assert_eq!(
        tools.calls.len(),
        tool_calls_before,
        "sent-but-unsettled tool must not be replayed"
    );
}

#[test]
fn second_provider_overflow_has_stable_terminal_without_third_retry() {
    let mut model = FakeModel {
        responses: VecDeque::from([
            Err(ModelPortError::ContextOverflow),
            response("summary", "summary", vec![]),
            Err(ModelPortError::ContextOverflow),
        ]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let result = AgentCore::run(
        &input(false),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .unwrap();
    assert_eq!(result.terminal_reason, TerminalReasonV1::ContextOverflow);
    assert_eq!(model.requests.len(), 3);
}

#[test]
fn model_failure_and_unknown_tool_outcome_have_distinct_terminals() {
    let mut failing_model = FakeModel {
        responses: VecDeque::from([Err(ModelPortError::Effect("provider failed".into()))]),
        requests: vec![],
    };
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let model_result = AgentCore::run(
        &input(false),
        &mut failing_model,
        &mut FakeTools::default(),
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("model failure is a controlled terminal");
    assert_eq!(model_result.terminal_reason, TerminalReasonV1::ModelError);
    assert!(matches!(
        model_result
            .state
            .operation
            .map(|operation| operation.phase),
        Some(OperationPhaseV1::SettledFailure { .. })
    ));

    let call = ToolCallV1 {
        call_id: "call-unknown".into(),
        name: "bash".into(),
        arguments: json!({"argv":["effect"]}),
    };
    let mut model = FakeModel {
        responses: VecDeque::from([response("assistant-1", "", vec![call])]),
        requests: vec![],
    };
    let mut tools = FakeTools {
        calls: vec![],
        result: Some(Err(ToolPortError::OutcomeUnknown("response lost".into()))),
    };
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let tool_result = AgentCore::run(
        &input(true),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("unknown tool outcome pauses safely");
    assert_eq!(
        tool_result.terminal_reason,
        TerminalReasonV1::OutcomeUnknown
    );
    assert!(matches!(
        tool_result.state.operation.map(|operation| operation.phase),
        Some(OperationPhaseV1::UnknownOutcome { .. })
    ));
}

#[test]
fn cancellation_and_budget_exhaustion_stop_before_model_effect() {
    for (decision, expected) in [
        (BudgetDecision::Cancel, TerminalReasonV1::Cancelled),
        (BudgetDecision::Exhausted, TerminalReasonV1::BudgetExhausted),
    ] {
        let mut model = FakeModel {
            responses: VecDeque::new(),
            requests: vec![],
        };
        let mut tools = FakeTools::default();
        let mut state = MemoryState::default();
        let mut events = Events::default();
        let result = AgentCore::run(
            &input(false),
            &mut model,
            &mut tools,
            &mut state,
            &mut events,
            &FixedClock,
            &mut budget(decision),
        )
        .expect("controlled terminal");
        assert_eq!(result.terminal_reason, expected);
        assert!(model.requests.is_empty());
    }
}

#[test]
fn newer_fencing_token_prevents_an_old_worker_from_committing() {
    let mut store = MemoryState::default();
    let state = AgentSessionStateV1 {
        session_id: "session-1".into(),
        ..AgentSessionStateV1::default()
    };
    store.commit(0, &state, 9).expect("new lease commits");
    assert!(matches!(
        store.commit(1, &state, 8),
        Err(StatePortError::LeaseLost)
    ));
}

#[test]
fn core_rejects_unsupported_input_contract_before_loading_state() {
    let mut run = input(false);
    run.api_version = 2;
    let mut model = FakeModel {
        responses: VecDeque::new(),
        requests: vec![],
    };
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let error = AgentCore::run(
        &run,
        &mut model,
        &mut FakeTools::default(),
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect_err("unsupported Core input must fail before any effect");
    assert!(matches!(error, AgentCoreError::ContractUnsupported));
    assert!(model.requests.is_empty());
    assert!(state.state.is_none());
}

#[test]
fn core_rejects_loaded_state_for_a_different_session() {
    let mut state = MemoryState {
        state: Some(AgentSessionStateV1 {
            session_id: "another-session".into(),
            ..AgentSessionStateV1::default()
        }),
        ..MemoryState::default()
    };
    let mut model = FakeModel {
        responses: VecDeque::new(),
        requests: vec![],
    };
    let mut events = Events::default();
    let error = AgentCore::run(
        &input(false),
        &mut model,
        &mut FakeTools::default(),
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect_err("state from another session must not be mixed");
    assert!(matches!(error, AgentCoreError::SessionProjectionInvalid(_)));
    assert!(model.requests.is_empty());
}

#[test]
fn turn_started_and_ended_events_are_emitted() {
    let mut model = FakeModel {
        responses: VecDeque::from([response("assistant-1", "done", vec![])]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let result = AgentCore::run(
        &input(false),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("agent completes");
    assert_eq!(result.terminal_reason, TerminalReasonV1::Completed);
    let turn_started = events
        .0
        .iter()
        .filter(|e| matches!(e, CoreEventV1::TurnStarted { .. }))
        .count();
    let turn_ended = events
        .0
        .iter()
        .filter(|e| matches!(e, CoreEventV1::TurnEnded { .. }))
        .count();
    assert_eq!(turn_started, 1, "should emit one TurnStarted event");
    assert_eq!(turn_ended, 1, "should emit one TurnEnded event");
}

#[test]
fn model_failure_closes_open_turn_and_model_operation_spans() {
    let mut model = FakeModel {
        responses: VecDeque::from([Err(ModelPortError::Effect(
            "HTTP 429 Too Many Requests: rate_limit_error".into(),
        ))]),
        requests: vec![],
    };
    let mut tools = FakeTools::default();
    let mut state = MemoryState::default();
    let mut events = Events::default();
    let result = AgentCore::run(
        &input(false),
        &mut model,
        &mut tools,
        &mut state,
        &mut events,
        &FixedClock,
        &mut budget(BudgetDecision::Continue),
    )
    .expect("model failure settles the run");
    assert_eq!(result.terminal_reason, TerminalReasonV1::ModelError);
    assert!(
        events
            .0
            .iter()
            .any(|event| matches!(event, CoreEventV1::ModelSettled { is_error: true, .. })),
        "model failure must settle the model operation span with an error"
    );
    assert!(
        events
            .0
            .iter()
            .any(|event| matches!(event, CoreEventV1::TurnEnded { is_error: true, .. })),
        "model failure must close the open turn span with an error"
    );
    let detail = result
        .state
        .operation
        .as_ref()
        .and_then(|operation| match &operation.phase {
            OperationPhaseV1::SettledFailure { error } => Some(error.as_str()),
            _ => None,
        });
    assert!(
        detail.is_some_and(|detail| detail.contains("429")),
        "terminal operation retains the provider error for settlement"
    );
}
