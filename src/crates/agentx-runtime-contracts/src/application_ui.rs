use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const APPLICATION_PAGE_SDK_VERSION: u32 = 1;
pub const APPLICATION_PAGE_MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationApprovalStageV1 {
    pub node_id: String,
    pub title: String,
    pub kind: ApplicationApprovalKindV1,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationApprovalKindV1 {
    WorkflowNode,
    AgentToolEffect,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationPageContractV1 {
    pub application_id: Uuid,
    pub application_name: String,
    pub description: Option<String>,
    pub deployment_id: Uuid,
    pub workflow_version_id: Uuid,
    pub bundle_id: Uuid,
    pub input_schema: Value,
    pub output_schema: Value,
    pub wait_input_nodes: Vec<crate::WaitInputNodeV1>,
    pub approval_stages: Vec<ApplicationApprovalStageV1>,
    pub chat_mapping: Option<crate::ChatMappingV1>,
    pub sdk_version: u32,
    pub max_upload_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationPageDraftV1 {
    pub id: Uuid,
    pub application_id: Uuid,
    pub contract: ApplicationPageContractV1,
    pub contract_hash: crate::ContentHash,
    pub html: String,
    pub html_hash: crate::ContentHash,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageEditorSnapshotV1 {
    pub editor_instance_id: Uuid,
    pub local_revision: u64,
    pub server_revision: u64,
    pub html: String,
    pub html_hash: crate::ContentHash,
    pub save_status: crate::StudioSaveStatusV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageEditCommandV1 {
    pub command_id: Uuid,
    pub run_id: Uuid,
    pub ui_draft_id: Uuid,
    pub editor_instance_id: Uuid,
    pub base_local_revision: u64,
    pub base_server_revision: u64,
    pub base_html_hash: crate::ContentHash,
    pub html: String,
    pub html_hash: crate::ContentHash,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageBuilderSessionV1 {
    pub id: Uuid,
    pub ui_draft_id: Uuid,
    pub snapshot: PageEditorSnapshotV1,
    pub runs: Vec<crate::StudioBuilderRunSummaryV1>,
}
