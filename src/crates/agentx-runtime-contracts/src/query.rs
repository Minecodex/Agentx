use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ContentHash, RuntimeApprovalButtonV1, RuntimeApprovalCandidateV1, RuntimeEvaluationReportV1,
    RuntimeEventPayloadV1, RuntimeRetentionItemV1,
};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventExportRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub after_cursor: u64,
    pub limit: u32,
    pub wait_seconds: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeIntegrationEventEnvelopeV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub cursor: u64,
    pub event_id: Uuid,
    pub source_outbox_id: Uuid,
    pub tenant_id: Uuid,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub aggregate_version: u64,
    pub event_type: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub payload: RuntimeEventPayloadV1,
    pub content_hash: ContentHash,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventExportPageV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub from_cursor: u64,
    pub next_cursor: u64,
    pub upper_cursor: u64,
    pub retention_floor_cursor: u64,
    pub has_more: bool,
    pub events: Vec<RuntimeIntegrationEventEnvelopeV1>,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeGovernanceObjectKindV1 {
    Approval,
    Evaluation,
    Notification,
    Debug,
    Retention,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeGovernanceSnapshotPayloadV1 {
    Approval {
        execution_id: Uuid,
        workflow_id: Uuid,
        node_id: String,
        title: String,
        description: Option<String>,
        request: Option<Value>,
        buttons: Vec<RuntimeApprovalButtonV1>,
        status: String,
        resume_status: String,
        claimed_by: Option<Uuid>,
        #[schemars(with = "Option<String>")]
        #[serde(with = "time::serde::rfc3339::option")]
        deadline_at: Option<OffsetDateTime>,
        decision: Option<Value>,
        candidates: Vec<RuntimeApprovalCandidateV1>,
    },
    Evaluation {
        work_package_id: Uuid,
        status: String,
        completed_cases: u64,
        total_cases: u64,
        report: Option<RuntimeEvaluationReportV1>,
    },
    Notification {
        notification_type: String,
        title_key: String,
        body_key: String,
        arguments: Value,
        target_type: String,
        target_id: Uuid,
        target_path: String,
        tone: String,
    },
    Debug {
        work_package_id: Uuid,
        status: String,
        result: Option<Value>,
        #[schemars(with = "String")]
        #[serde(with = "time::serde::rfc3339")]
        expires_at: OffsetDateTime,
    },
    Retention {
        status: String,
        marked_count: u64,
        deleted_count: u64,
        failed_count: u64,
        dry_run: bool,
        items: Vec<RuntimeRetentionItemV1>,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeGovernanceSnapshotItemV1 {
    pub object_type: RuntimeGovernanceObjectKindV1,
    pub object_id: Uuid,
    pub object_version: u64,
    pub last_event_cursor: u64,
    pub deleted: bool,
    pub payload: RuntimeGovernanceSnapshotPayloadV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GovernanceSnapshotRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub object_types: Vec<RuntimeGovernanceObjectKindV1>,
    pub snapshot_upper_cursor: Option<u64>,
    pub page_cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GovernanceSnapshotPageV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub snapshot_upper_cursor: u64,
    pub retention_floor_cursor: u64,
    pub objects: Vec<RuntimeGovernanceSnapshotItemV1>,
    pub next_page_cursor: Option<String>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegationScopeV1 {
    pub tenant_id: Uuid,
    pub subject_id: Uuid,
    pub token_version: u64,
    pub tenant_wide: bool,
    pub operations: Vec<String>,
    pub application_ids: Vec<Uuid>,
    pub workflow_ids: Vec<Uuid>,
    pub execution_ids: Vec<Uuid>,
    pub session_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionSearchRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub application_ids: Vec<Uuid>,
    pub workflow_ids: Vec<Uuid>,
    pub tool_ids: Vec<Uuid>,
    pub initiator_user_ids: Vec<Uuid>,
    pub initiator_department_ids: Vec<Uuid>,
    pub trigger_types: Vec<String>,
    pub trigger_name: Option<String>,
    pub statuses: Vec<String>,
    pub session_mode: ExecutionSessionModeV1,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub created_after: Option<OffsetDateTime>,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub created_before: Option<OffsetDateTime>,
    pub search: Option<String>,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSessionModeV1 {
    #[default]
    All,
    Stateless,
    Session,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationSearchRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub application_ids: Vec<Uuid>,
    pub statuses: Vec<String>,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub created_after: Option<OffsetDateTime>,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub created_before: Option<OffsetDateTime>,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionSummaryV1 {
    pub execution_id: Uuid,
    pub invocation_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub workflow_id: Uuid,
    pub workflow_version_id: Uuid,
    pub session_id: Option<Uuid>,
    pub parent_execution_id: Option<Uuid>,
    pub bundle_id: Uuid,
    pub trace_id: Uuid,
    pub trigger_type: String,
    pub initiator_user_id: Option<Uuid>,
    pub initiator_user_name: Option<String>,
    pub initiator_department_id: Option<Uuid>,
    pub initiator_department_name: Option<String>,
    pub trigger_source_id: Option<Uuid>,
    pub trigger_name: Option<String>,
    pub status: String,
    pub duration_ms: Option<u64>,
    pub cost_micros: u64,
    pub cost_currency: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub error_code: Option<String>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationSummaryV1 {
    pub invocation_id: Uuid,
    pub execution_id: Option<Uuid>,
    pub application_id: Uuid,
    pub session_id: Option<Uuid>,
    pub bundle_id: Uuid,
    pub admission_epoch: u64,
    pub caller_type: String,
    pub status: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionSearchPageV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub snapshot_id: Uuid,
    pub snapshot_upper_bound: String,
    pub total: u64,
    pub items: Vec<ExecutionSummaryV1>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationSearchPageV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub snapshot_id: Uuid,
    pub snapshot_upper_bound: String,
    pub total: u64,
    pub items: Vec<InvocationSummaryV1>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionDetailV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub summary: ExecutionSummaryV1,
    pub state_version: u64,
    pub admission_epoch: u64,
    pub trace_watermark: u64,
    pub parent_execution_id: Option<Uuid>,
    pub work_package_id: Option<Uuid>,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationDetailV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub summary: InvocationSummaryV1,
    pub request_hash: String,
    pub status_version: u64,
    pub response: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionNodeV1 {
    pub node_execution_id: Uuid,
    pub node_id: String,
    pub node_name: String,
    pub node_type: String,
    pub node_version: u32,
    pub run_index: u32,
    pub iteration_index: u32,
    pub status: String,
    pub capability: String,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub cost_micros: u64,
    pub cost_currency: Option<String>,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeAttemptV1 {
    pub attempt_id: Uuid,
    pub node_execution_id: Uuid,
    pub attempt_number: u32,
    pub status: String,
    pub worker_id: Option<Uuid>,
    pub fencing_token: u64,
    pub result_hash: Option<ContentHash>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionCheckpointV1 {
    pub checkpoint_id: Uuid,
    pub node_execution_id: Option<Uuid>,
    pub sequence_number: u64,
    pub checkpoint_type: String,
    pub state_hash: String,
    pub payload_hash: ContentHash,
    pub state_version: u64,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeCallDetailV1 {
    pub call_id: Uuid,
    pub node_execution_id: Uuid,
    pub attempt_id: Uuid,
    pub call_kind: String,
    pub status: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<Uuid>,
    pub resource_version: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micros: u64,
    pub cost_currency: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionRuntimeDetailsV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub attempts: Vec<NodeAttemptV1>,
    pub calls: Vec<RuntimeCallDetailV1>,
    pub agent_runs: Vec<Value>,
    pub sandboxes: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionArtifactV1 {
    pub artifact_id: Uuid,
    pub content_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub owner_type: String,
    pub owner_id: String,
    pub reference_role: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionCollectionPageV1<T> {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub items: Vec<T>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionEventV1 {
    pub sequence: u64,
    pub event_type: String,
    pub status: String,
    pub summary: Value,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionEventPageV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub items: Vec<ExecutionEventV1>,
    pub next_cursor: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionStateV1 {
    Ready,
    Rebuilding,
    Degraded,
    Error,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionStatusV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub projection_name: String,
    pub state: ProjectionStateV1,
    pub cursor: u64,
    pub retention_floor_cursor: u64,
    pub active_generation: u64,
    pub building_generation: Option<u64>,
    pub last_error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceEventEnvelopeV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub event_id: Uuid,
    pub tenant_id: Uuid,
    pub execution_id: Uuid,
    pub execution_sequence: u64,
    pub trace_id: Uuid,
    /// Aggregation dimensions (plan7 P7-C): filled at the single outbox
    /// enqueue point from the execution row; absent on events produced by
    /// older runtimes (serde default keeps the consumer green during the
    /// rollout window).
    #[serde(default)]
    pub workflow_id: Option<Uuid>,
    #[serde(default)]
    pub application_id: Option<Uuid>,
    pub span_id: Uuid,
    pub parent_span_id: Option<Uuid>,
    pub event_kind: TraceEventKindV1,
    pub span_kind: TraceSpanKindV1,
    pub span_name: String,
    pub node_execution_id: Option<Uuid>,
    pub attempt_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub agent_iteration_id: Option<Uuid>,
    pub runtime_call_id: Option<Uuid>,
    pub sandbox_lease_id: Option<Uuid>,
    pub wait_id: Option<Uuid>,
    pub resource_type: Option<String>,
    pub resource_id: Option<Uuid>,
    pub resource_version: Option<String>,
    pub event_type: String,
    pub status: String,
    pub duration_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_micros: u64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attributes: Value,
    pub content_ref: Option<Uuid>,
    pub content_kind: Option<TraceContentKindV1>,
    pub content_preview: Option<Value>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub content_hash: ContentHash,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEventKindV1 {
    Started,
    Updated,
    Finished,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceSpanKindV1 {
    Execution,
    Boundary,
    Node,
    Attempt,
    AgentRun,
    AgentIteration,
    RuntimeCall,
    Sandbox,
    Wait,
    PluginOperation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceContentKindV1 {
    WorkflowInput,
    WorkflowOutput,
    NodeInput,
    NodeOutput,
    AttemptInput,
    AttemptOutput,
    ResolvedParameters,
    RuntimeRequest,
    RuntimeResponse,
    AgentInput,
    AgentOutput,
    IterationInput,
    IterationOutput,
    SandboxRequest,
    SandboxResponse,
    WaitRequest,
    WaitResponse,
    ConversionRecord,
    PluginContent,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceSpanSummaryV1 {
    pub span_id: Uuid,
    pub parent_span_id: Option<Uuid>,
    pub span_kind: TraceSpanKindV1,
    pub span_name: String,
    pub status: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    pub duration_ms: Option<u64>,
    pub node_execution_id: Option<Uuid>,
    pub attempt_id: Option<Uuid>,
    pub agent_run_id: Option<Uuid>,
    pub agent_iteration_id: Option<Uuid>,
    pub runtime_call_id: Option<Uuid>,
    pub sandbox_lease_id: Option<Uuid>,
    pub wait_id: Option<Uuid>,
    pub resource_type: Option<String>,
    pub resource_id: Option<Uuid>,
    pub resource_version: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_micros: u64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub has_details: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceContentV1 {
    pub event_id: Uuid,
    pub kind: TraceContentKindV1,
    pub preview: Option<Value>,
    pub content_ref: Option<Uuid>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceSpanDetailV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub execution_id: Uuid,
    pub span: TraceSpanSummaryV1,
    pub contents: Vec<TraceContentV1>,
    pub events: Vec<TraceEventEnvelopeV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceSearchRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub execution_id: Option<Uuid>,
    /// plan7 P7-C: aggregate-page drill-down needs workflow-scoped search.
    #[serde(default)]
    pub workflow_id: Option<Uuid>,
    pub event_types: Vec<String>,
    pub statuses: Vec<String>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub from: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub to: OffsetDateTime,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceSearchPageV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub query_id: Uuid,
    pub events: Vec<TraceEventEnvelopeV1>,
    pub next: Option<String>,
    pub degraded: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionTraceV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub execution_id: Uuid,
    pub ingested_watermark: u64,
    pub expected_watermark: u64,
    pub complete: bool,
    pub degraded: bool,
    pub warning_code: Option<String>,
    pub total_spans: u64,
    pub next: Option<String>,
    pub spans: Vec<TraceSpanSummaryV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservabilityMetricV1 {
    Count,
    DurationMillis,
    CostMicros,
    InputTokens,
    OutputTokens,
    ErrorRate,
    /// plan7 P7-C: node-duration distribution charts.
    DurationP50,
    DurationP95,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservabilityDimensionV1 {
    Hour,
    Day,
    Workflow,
    Application,
    Status,
    ErrorCode,
    Provider,
    ResourceType,
    /// plan7 P7-C: node-duration distribution charts.
    SpanName,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservabilityAggregateRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub from: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub to: OffsetDateTime,
    pub metrics: Vec<ObservabilityMetricV1>,
    pub dimensions: Vec<ObservabilityDimensionV1>,
    pub filters: Value,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservabilityAggregateRowV1 {
    pub dimensions: Value,
    pub metrics: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservabilityAggregatePageV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub query_id: Uuid,
    pub rows: Vec<ObservabilityAggregateRowV1>,
    pub degraded: bool,
}
