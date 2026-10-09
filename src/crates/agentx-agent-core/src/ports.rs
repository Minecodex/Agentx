use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentMessageV1, AgentSessionStateV1, ToolCallV1, ToolDefinitionV1};

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPurposeV1 {
    AgentTurn,
    Compaction,
}

/// One multimodal content part of a model message (plan7 P7-B). `Text` is
/// the untagged default so pure-text state and requests stay plain strings.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ModelContentV1 {
    Text(String),
    Parts(Vec<ModelContentPartV1>),
}

impl From<String> for ModelContentV1 {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for ModelContentV1 {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl ModelContentV1 {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Parts(parts) => parts.iter().find_map(|part| match part {
                ModelContentPartV1::Text { text } => Some(text.as_str()),
                _ => None,
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelContentPartV1 {
    Text { text: String },
    ImageUrl { image_url: ImageUrlV1 },
    InputAudio { input_audio: InputAudioV1 },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ImageUrlV1 {
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InputAudioV1 {
    pub data: String,
    pub format: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModelRequestV1 {
    pub purpose: ModelPurposeV1,
    pub model_reference: String,
    pub messages: Vec<AgentMessageV1>,
    pub tools: Vec<ToolDefinitionV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ModelResponseV1 {
    pub message_id: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallV1>,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ModelPortError {
    #[error("model context overflow")]
    ContextOverflow,
    #[error("model request was cancelled")]
    Cancelled,
    #[error("model effect outcome is unknown: {0}")]
    OutcomeUnknown(String),
    #[error("model effect failed: {0}")]
    Effect(String),
}

/// One streamed token increment of a model response (plan7 P7-B). The
/// callback is synchronous: implementations must not block and should hand
/// the chunk to a non-blocking sink.
#[derive(Clone, Debug)]
pub struct ModelDeltaChunk {
    pub text: String,
    pub reasoning_text: Option<String>,
}

pub trait ModelPort {
    fn invoke(
        &mut self,
        request: &ModelRequestV1,
        context: &EffectContextV1,
    ) -> Result<ModelResponseV1, ModelPortError>;

    /// Streaming variant: same settlement contract as `invoke` (the final
    /// full response is still returned); the default delegates to `invoke`
    /// without emitting deltas so existing ports and tests stay valid.
    fn invoke_stream(
        &mut self,
        request: &ModelRequestV1,
        context: &EffectContextV1,
        on_delta: &(dyn Fn(&ModelDeltaChunk) + Send + Sync),
    ) -> Result<ModelResponseV1, ModelPortError> {
        let _ = on_delta;
        self.invoke(request, context)
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectContextV1 {
    pub run_id: String,
    pub operation_id: String,
    pub effect_id: String,
    pub idempotency_key: String,
    pub fencing_token: u64,
    pub deadline_at_millis: u64,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum StatePortError {
    #[error("state version conflict")]
    Conflict,
    #[error("state lease was lost")]
    LeaseLost,
    #[error("state persistence failed: {0}")]
    Persistence(String),
}

pub trait StatePort {
    fn load(&mut self, session_id: &str) -> Result<Option<AgentSessionStateV1>, StatePortError>;
    fn commit(
        &mut self,
        expected_version: u64,
        state: &AgentSessionStateV1,
        fencing_token: u64,
    ) -> Result<u64, StatePortError>;
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoreEventV1 {
    AgentStarted {
        run_id: String,
    },
    MessageAdded {
        message_id: String,
    },
    TurnStarted {
        turn: u32,
    },
    TurnEnded {
        turn: u32,
        is_error: bool,
    },
    ModelIntent {
        operation_id: String,
    },
    ModelSettled {
        operation_id: String,
        is_error: bool,
    },
    /// Live token increment of the running model operation. Bypasses the
    /// EventCollector batching by design: the runtime adapter forwards it
    /// straight to the invocation event stream (plan7 P7-B).
    ModelDelta {
        operation_id: String,
        text: String,
    },
    ToolIntent {
        operation_id: String,
        tool_name: String,
    },
    ToolSettled {
        operation_id: String,
        tool_name: String,
        is_error: bool,
    },
    CompactionCompleted {
        kind: crate::CompactionKindV1,
    },
    RecoveryRequired {
        operation_id: String,
    },
    AgentEnded {
        reason: crate::TerminalReasonV1,
    },
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("event publication failed: {0}")]
pub struct EventPortError(pub String);

pub trait EventPort {
    fn publish(&mut self, event: CoreEventV1) -> Result<(), EventPortError>;
}

pub trait ClockPort {
    fn now_millis(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetDecision {
    Continue,
    Cancel,
    Exhausted,
}

pub trait BudgetPort {
    fn admit_turn(&mut self, turn: u32, projected_tokens: u64) -> BudgetDecision;
    fn charge_model(&mut self, _input_tokens: u64, _output_tokens: u64) {}
    fn charge_tool(&mut self, _tool_name: &str) {}
}
