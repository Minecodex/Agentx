use agentx_domain::{
    EditorDocument, NodeLayout, WorkflowConnection, WorkflowDefinition, WorkflowEnd, WorkflowNode,
    WorkflowSettings, WorkflowStart,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StudioDocumentV1 {
    pub definition: WorkflowDefinition,
    pub editor_document: EditorDocument,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum StudioEditScopeV1 {
    Canvas,
    Nodes { node_ids: Vec<String> },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StudioEditorSnapshotV1 {
    pub editor_instance_id: Uuid,
    pub local_document_revision: u64,
    pub document_hash: String,
    pub server_revision: u64,
    pub save_status: StudioSaveStatusV1,
    pub document: StudioDocumentV1,
    pub selected_node_ids: Vec<String>,
    pub selected_execution_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum StudioEditIntentV1 {
    AutoLayout,
    ReplaceDocument {
        document: StudioDocumentV1,
    },
    UpsertNode {
        node: WorkflowNode,
        layout: Option<NodeLayout>,
    },
    RemoveNode {
        node_id: String,
    },
    ReplaceConnections {
        connections: Vec<WorkflowConnection>,
    },
    ConfigureWorkflow {
        start: Option<WorkflowStart>,
        end: Option<WorkflowEnd>,
        settings: Option<WorkflowSettings>,
    },
    SetEditorDocument {
        editor_document: EditorDocument,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StudioEditorCommandV1 {
    pub command_id: Uuid,
    pub run_id: Uuid,
    pub draft_id: Uuid,
    pub editor_instance_id: Uuid,
    pub expected_local_document_revision: u64,
    pub expected_document_hash: String,
    pub result_document_hash: String,
    /// The editor may change only node geometry and the Start position before its atomic commit.
    pub auto_layout: bool,
    pub document: StudioDocumentV1,
    pub changed_node_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StudioAcknowledgementStatusV1 {
    Applied,
    Stale,
    ReadOnly,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StudioEditorAcknowledgementV1 {
    pub command_id: Uuid,
    pub status: StudioAcknowledgementStatusV1,
    pub snapshot: StudioEditorSnapshotV1,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StudioSaveStatusV1 {
    Saved,
    Unsaved,
}
