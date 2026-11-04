//! Durable application batches coordinate independent formal Invocations.
use crate::{ApplicationAccessBindingV1, ArtifactUploadResponseV1, TaskViewV1};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const APPLICATION_BATCH_MAX_ITEMS: usize = 100;
pub const APPLICATION_BATCH_MAX_BYTES: usize = 1024 * 1024;
pub const APPLICATION_BATCH_CONCURRENCY: usize = 4;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchInputV1 {
    pub client_item_id: String,
    pub input: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateBatchV1 {
    pub name: String,
    pub items: Vec<BatchInputV1>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchQueryV1 {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub state: Option<String>,
    pub source: Option<String>,
    pub request_key: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchCountsV1 {
    pub total: u32,
    pub queued: u32,
    pub running: u32,
    pub waiting_input: u32,
    pub waiting_approval: u32,
    pub completed: u32,
    pub failed: u32,
    pub cancelled: u32,
    pub outcome_unknown: u32,
    pub ended: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchViewV1 {
    pub batch_id: Uuid,
    pub application_id: Uuid,
    pub name: String,
    pub source: String,
    pub caller_type: String,
    pub caller_id: Uuid,
    pub caller_name: Option<String>,
    pub bundle_id: Uuid,
    pub workflow_version_id: Uuid,
    pub workflow_version_number: u64,
    pub access_binding: Option<ApplicationAccessBindingV1>,
    pub page_publication_id: Option<Uuid>,
    pub source_batch_id: Option<Uuid>,
    pub dispatch_state: String,
    pub state: String,
    pub revision: u64,
    pub counts: BatchCountsV1,
    pub created_at: String,
    pub can_stop: bool,
    pub can_cancel: bool,
    pub can_rerun: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchItemViewV1 {
    pub item_id: Uuid,
    pub client_item_id: String,
    pub ordinal: u32,
    pub input: Value,
    pub dispatch_state: String,
    pub state: String,
    pub task: Option<TaskViewV1>,
    pub error: Option<Value>,
    pub can_rerun: bool,
    pub source_item_id: Option<Uuid>,
    pub source_invocation_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchesPageV1 {
    pub items: Vec<BatchViewV1>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchItemsPageV1 {
    pub items: Vec<BatchItemViewV1>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchRerunV1 {
    pub item_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchExportV1 {
    pub export_id: Uuid,
    pub batch_id: Uuid,
    pub status: String,
    pub file: Option<ArtifactUploadResponseV1>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadTableV1 {
    pub artifact_id: Option<Uuid>,
    pub text: Option<String>,
    pub delimiter: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableDataV1 {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub lines: Vec<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManageBatchRequestV1 {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub batch_id: Option<Uuid>,
    pub operation: String,
    pub query: Option<BatchQueryV1>,
    pub item_ids: Option<Vec<Uuid>>,
    pub export_id: Option<Uuid>,
    pub request_key: Option<String>,
}
