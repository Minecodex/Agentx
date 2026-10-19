//! Raw workflow messages and durable external input. These carry no intent or approval semantics.
use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// JSON-pointer node paths (including sub-workflow call sites), each with its own reply sequence.
pub type WaitInputsV1 = BTreeMap<String, Vec<Value>>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitInputNodeV1 {
    pub path: String,
    pub node_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowMessageV1 {
    pub id: Uuid,
    pub execution_id: Uuid,
    pub node_execution_id: Uuid,
    pub content: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputWaitV1 {
    pub wait_id: Uuid,
    pub execution_id: Uuid,
    pub node_execution_id: Uuid,
    pub node_id: String,
    pub node_path: String,
    pub occurrence: u32,
    pub status: String,
    pub resume_url: Option<String>,
    pub expires_at: Option<String>,
    pub content: Option<Value>,
    pub source: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitWaitInputV1 {
    pub content: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitInputReceiptV1 {
    pub wait_id: Uuid,
    pub execution_id: Uuid,
    pub receipt_id: Uuid,
    pub status: String,
}

pub fn node_path(parent: &str, node_id: &str) -> String {
    format!("{parent}/{}", node_id.replace('~', "~0").replace('/', "~1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_keep_call_sites_and_escape_ids() {
        assert_eq!(
            node_path(&node_path("", "child"), "ask/a~b"),
            "/child/ask~1a~0b"
        );
    }

    #[test]
    fn raw_input_accepts_text_and_objects_but_requires_content() {
        for value in [
            serde_json::json!({"content":"我再考虑一下"}),
            serde_json::json!({"content":{"ok":false}}),
        ] {
            assert!(serde_json::from_value::<SubmitWaitInputV1>(value).is_ok());
        }
        assert!(serde_json::from_value::<SubmitWaitInputV1>(serde_json::json!({})).is_err());
    }
}
