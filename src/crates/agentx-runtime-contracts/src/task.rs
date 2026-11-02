//! A public projection of an Invocation. These states are derived, never independently persisted.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStateV1 {
    Accepted,
    Running,
    WaitingInput,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskStageV1 {
    pub node_execution_id: Uuid,
    pub title: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskApprovalV1 {
    pub approval_id: Uuid,
    pub title: String,
    pub status: String,
    pub version: u64,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskActionsV1 {
    pub can_cancel: bool,
    pub input_wait_ids: Vec<Uuid>,
    pub downloadable_artifact_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskViewV1 {
    pub task_id: Uuid,
    pub task_url: Option<String>,
    pub application_id: Uuid,
    pub deployment_id: Uuid,
    pub workflow_version_id: Uuid,
    pub bundle_id: Uuid,
    pub session_id: Option<Uuid>,
    pub caller_type: String,
    pub access_binding: Option<crate::ApplicationAccessBindingV1>,
    pub state: TaskStateV1,
    pub invocation_status: String,
    pub stages: Vec<TaskStageV1>,
    pub pending_inputs: Vec<crate::InputWaitV1>,
    pub pending_approvals: Vec<TaskApprovalV1>,
    pub messages: Vec<crate::WorkflowMessageV1>,
    pub outputs: Option<Value>,
    pub files: Vec<crate::ArtifactUploadResponseV1>,
    pub error: Option<Value>,
    pub allowed_actions: TaskActionsV1,
    pub revision: u64,
    pub event_cursor: u64,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MyTasksPageV1 {
    pub items: Vec<TaskViewV1>,
    /// Opaque page boundary scoped to the same authenticated application and caller.
    pub next: Option<String>,
}
