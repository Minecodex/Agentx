use crate::ContentHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolApprovalRuleV1 {
    pub user_ids: Vec<Uuid>,
    pub role_ids: Vec<Uuid>,
    pub timeout_seconds: u32,
}
impl ToolApprovalRuleV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.user_ids.len() + self.role_ids.len() == 0
            || self.user_ids.len() + self.role_ids.len() > 32
            || !(60..=86400).contains(&self.timeout_seconds)
        {
            return Err(
                "Select 1–32 approvers or roles and an approval timeout of 60–86400 seconds".into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeMcpToolPolicyV1 {
    pub revision: u64,
    pub content_hash: ContentHash,
    pub approval: Option<ToolApprovalRuleV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpToolPolicyAdmissionV1 {
    pub tenant_id: Uuid,
    pub tool_id: Uuid,
    pub revision: u64,
    pub content_hash: ContentHash,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ApprovalSubjectV1 {
    WorkflowNode,
    AgentToolEffect {
        effect_id: Uuid,
        agent_run_id: Uuid,
        operation_id: String,
        tool_call_id: String,
        tool_id: Uuid,
        tool_version: String,
        tool_name: String,
        arguments_hash: ContentHash,
        policy_revision: u64,
        policy_hash: ContentHash,
    },
}
