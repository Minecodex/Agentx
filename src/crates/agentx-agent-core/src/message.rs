use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ports::ModelContentV1;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    ToolResult,
    ExternalContext,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentMessageV1 {
    pub message_id: String,
    pub role: MessageRole,
    /// Multimodal content (plan7 P7-B): plain text stays a bare JSON string
    /// (untagged, Text first) so existing session state deserializes without
    /// migration; image/audio parts use the array form.
    pub content: ModelContentV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolCallV1 {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

impl AgentMessageV1 {
    pub fn user(message_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            role: MessageRole::User,
            content: ModelContentV1::Text(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            is_error: false,
        }
    }

    pub fn assistant(
        message_id: impl Into<String>,
        content: impl Into<String>,
        tool_calls: Vec<ToolCallV1>,
    ) -> Self {
        Self {
            message_id: message_id.into(),
            role: MessageRole::Assistant,
            content: ModelContentV1::Text(content.into()),
            tool_calls,
            tool_call_id: None,
            is_error: false,
        }
    }
}
