use std::collections::{BTreeMap, BTreeSet};

use agentx_node_protocol::NodeCapability;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{CompiledWorkflowV1, ContentHash, INTERNAL_API_VERSION, RuntimeObjectReferenceV1};

pub const MAX_POLICY_STALENESS_SECONDS: u32 = 72 * 60 * 60;
pub const DEFAULT_HANDLE_TTL_SECONDS: u32 = 300;
pub const INLINE_RESULT_LIMIT_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeRetryPolicyV1 {
    pub maximum_attempts: u16,
    pub initial_backoff_millis: u64,
    pub maximum_backoff_millis: u64,
    pub backoff_multiplier_milli: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeCheckpointPolicyV1 {
    pub on_execution_start: bool,
    pub on_node_completed: bool,
    pub on_suspended: bool,
    pub periodic_node_interval: Option<u32>,
    pub inline_limit_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeContextPolicyV1 {
    pub maximum_bytes: u64,
    pub compare_and_swap_required: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeOutputPolicyV1 {
    pub inline_limit_bytes: u64,
    pub external_media_type: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimePolicyV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub timeout_seconds: u32,
    pub operation_deadline_seconds: u32,
    pub activation_budget: u32,
    pub maximum_policy_staleness_seconds: u32,
    pub credential_handle_ttl_seconds: u32,
    pub retry: RuntimeRetryPolicyV1,
    pub checkpoint: RuntimeCheckpointPolicyV1,
    pub context: RuntimeContextPolicyV1,
    pub output: RuntimeOutputPolicyV1,
}

impl RuntimePolicyV1 {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.maximum_policy_staleness_seconds > MAX_POLICY_STALENESS_SECONDS {
            return Err("maximum policy staleness exceeds 72 hours");
        }
        if self.credential_handle_ttl_seconds > DEFAULT_HANDLE_TTL_SECONDS {
            return Err("credential handle TTL exceeds 300 seconds");
        }
        if self.checkpoint.inline_limit_bytes > INLINE_RESULT_LIMIT_BYTES
            || self.output.inline_limit_bytes > INLINE_RESULT_LIMIT_BYTES
        {
            return Err("inline payload limit exceeds 64 KiB");
        }
        if self.retry.maximum_attempts == 0 || self.activation_budget == 0 {
            return Err("retry attempts and activation budget must be positive");
        }
        Ok(())
    }
}

impl Default for RuntimePolicyV1 {
    fn default() -> Self {
        Self {
            schema_version: 1,
            timeout_seconds: 300,
            operation_deadline_seconds: 300,
            activation_budget: 10_000,
            maximum_policy_staleness_seconds: MAX_POLICY_STALENESS_SECONDS,
            credential_handle_ttl_seconds: DEFAULT_HANDLE_TTL_SECONDS,
            retry: RuntimeRetryPolicyV1 {
                maximum_attempts: 3,
                initial_backoff_millis: 500,
                maximum_backoff_millis: 30_000,
                backoff_multiplier_milli: 2_000,
            },
            checkpoint: RuntimeCheckpointPolicyV1 {
                on_execution_start: true,
                on_node_completed: true,
                on_suspended: true,
                periodic_node_interval: None,
                inline_limit_bytes: INLINE_RESULT_LIMIT_BYTES,
            },
            context: RuntimeContextPolicyV1 {
                maximum_bytes: 1024 * 1024,
                compare_and_swap_required: true,
            },
            output: RuntimeOutputPolicyV1 {
                inline_limit_bytes: INLINE_RESULT_LIMIT_BYTES,
                external_media_type: "application/json".into(),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeAuthorizationSnapshotV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub tenant_id: Uuid,
    pub service_identity_id: Uuid,
    pub workflow_id: Uuid,
    pub policy_epoch: u64,
    pub grant_ids: Vec<Uuid>,
    pub grant_bindings: Vec<RuntimeGrantBindingV1>,
    pub capabilities: BTreeSet<String>,
    pub maximum_policy_staleness_seconds: u32,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub captured_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeGrantBindingV1 {
    pub grant_id: Uuid,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub resource_version_id: Option<Uuid>,
    pub operation: String,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResourceKindV1 {
    Model,
    Mcp,
    Rag,
    Memory,
    Skill,
    Credential,
    SandboxProfile,
    Composite,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeMcpTransportV2 {
    StreamableHttp {
        endpoint: String,
    },
    Sse {
        endpoint: String,
    },
    Stdio {
        command: String,
        args: Vec<String>,
        environment_credential_refs: Vec<RuntimeEnvironmentCredentialReferenceV1>,
        runtime_sandbox: RuntimeMcpSandboxReferenceV1,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeEnvironmentCredentialReferenceV1 {
    pub name: String,
    pub credential: crate::VaultSecretReferenceV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeMcpSandboxReferenceV1 {
    pub resource_id: Uuid,
    pub resource_version_id: Uuid,
}

fn default_rag_provider() -> String {
    "lightrag".into()
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeResourceConfigurationV1 {
    Model {
        provider: String,
        endpoint: String,
        model: String,
        context_window: u64,
        price: RuntimeModelPriceV1,
        credential: Option<crate::VaultSecretReferenceV1>,
        /// Modalities the deployment accepts as native model input
        /// (plan7 P7-B); absent means text-only so old bundles stay valid.
        #[serde(default)]
        capabilities: Vec<String>,
    },
    Mcp {
        server_id: Uuid,
        server_version_id: Uuid,
        transport: RuntimeMcpTransportV2,
        tool_name: String,
        tool_version: String,
        input_schema_hash: ContentHash,
        input_schema: Value,
        output_schema: Option<Value>,
        side_effect: String,
        timeout_seconds: u32,
        credential: Option<crate::VaultSecretReferenceV1>,
    },
    Rag {
        #[serde(default = "default_rag_provider")]
        provider: String,
        endpoint: String,
        namespace: String,
        index_version: String,
        credential: Option<crate::VaultSecretReferenceV1>,
    },
    Memory {
        endpoint: String,
        namespace: String,
        memory_version: String,
        access_mode: String,
        credential: Option<crate::VaultSecretReferenceV1>,
    },
    Skill {
        entrypoint_object_id: Uuid,
        entrypoint_content_hash: ContentHash,
        dependency_object_ids: Vec<Uuid>,
        dependencies: Vec<RuntimeSkillDependencyV2>,
    },
    Credential {
        credential_type: String,
        secret: crate::VaultSecretReferenceV1,
        allowed_operations: BTreeSet<String>,
    },
    SandboxProfile {
        provider: String,
        image: String,
        cpu_millis: u32,
        memory_bytes: u64,
        disk_bytes: u64,
        pid_limit: u32,
        egress_mode: SandboxEgressModeV1,
        maximum_ttl_seconds: u32,
    },
    Composite {
        workflow: crate::ExecutionWorkflowSnapshotV1,
        definition_object_id: Uuid,
        ir_object_id: Uuid,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeModelPriceV1 {
    pub version_id: String,
    pub currency: String,
    pub input_per_million: String,
    pub output_per_million: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxEgressModeV1 {
    #[default]
    None,
    TcpProxy,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeResourceBindingV1 {
    pub resource_kind: RuntimeResourceKindV1,
    pub resource_id: Uuid,
    pub resource_version: String,
    pub state_epoch: u64,
    pub content_hash: ContentHash,
    pub configuration: RuntimeResourceConfigurationV1,
    pub object_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeResourceProbeV1 {
    ModelChat { model: String },
    McpInitialize,
    HttpGet { path: String },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeResourceCheckRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub tenant_id: Uuid,
    pub endpoint: String,
    pub credential: Option<crate::VaultSecretReferenceV1>,
    pub probe: RuntimeResourceProbeV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeResourceCheckResponseV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub status: String,
    pub latency_ms: u64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub checked_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeResourceOperationV1 {
    McpInitialize,
    McpDiscover,
    McpCall { tool_name: String, arguments: Value },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeResourceOperationRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub operation_id: Uuid,
    pub tenant_id: Uuid,
    pub server_version_id: Uuid,
    pub transport: RuntimeMcpTransportV2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<crate::VaultSecretReferenceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_sandbox_profile: Option<RuntimeResourceBindingV1>,
    pub timeout_seconds: u32,
    pub operation: RuntimeResourceOperationV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeResourceOperationResponseV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub operation_id: Uuid,
    pub result: Value,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSkillAssetV2 {
    pub path: String,
    pub object_id: Uuid,
    pub content_hash: ContentHash,
    pub media_type: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSkillDependencyV2 {
    pub resource_type: String,
    pub resource_id: Uuid,
    pub resource_version_id: Option<Uuid>,
    pub operation: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSkillProgramV2 {
    #[serde(deserialize_with = "crate::deserialize_v2")]
    pub schema_version: u32,
    pub skill_version_id: Uuid,
    pub instructions: String,
    pub assets: Vec<RuntimeSkillAssetV2>,
    pub dependencies: Vec<RuntimeSkillDependencyV2>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCallPurposeV1 {
    Production,
    Debug,
    Evaluation,
    Composite,
    Recovery,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeWorkPackageOverlayV1 {
    pub input: Value,
    pub context: BTreeMap<String, Value>,
    pub node_parameters: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeEvaluationCaseV1 {
    pub case_id: Uuid,
    pub input: Value,
    pub expected_output: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeEvaluatorV1 {
    DeterministicRule {
        evaluator_id: Uuid,
        expression: String,
    },
    Model {
        evaluator_id: Uuid,
        resource_id: Uuid,
        prompt_object_id: Uuid,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeModelEvaluatorExecutionV1 {
    pub evaluator_id: Uuid,
    pub resource_id: Uuid,
    pub prompt_object_id: Uuid,
    pub definition: Value,
    pub compiled_ir: CompiledWorkflowV1,
    pub node_manifests: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeDebugInputSourceV1 {
    Manual {
        value: Value,
    },
    HistoryOutput {
        execution_id: Uuid,
        node_execution_id: Uuid,
        output_port: String,
    },
    Artifact {
        execution_id: Uuid,
        artifact_id: Uuid,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDebugPlanV1 {
    pub mode: PartialExecutionModeV1,
    pub target_node_id: Option<String>,
    pub included_node_ids: Vec<String>,
    pub skipped_node_ids: Vec<String>,
    pub input_source: Option<RuntimeDebugInputSourceV1>,
    pub side_effect_decisions: BTreeMap<String, SideEffectResolutionV1>,
}

impl RuntimeDebugPlanV1 {
    #[must_use]
    pub fn whole(compiled: &CompiledWorkflowV1) -> Self {
        Self {
            mode: PartialExecutionModeV1::Whole,
            target_node_id: None,
            included_node_ids: compiled.nodes.iter().map(|node| node.id.clone()).collect(),
            skipped_node_ids: Vec::new(),
            input_source: None,
            side_effect_decisions: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeWorkPackageSpecV1 {
    Debug {
        draft_revision: u64,
        debug_plan: RuntimeDebugPlanV1,
    },
    Evaluation {
        dataset_version_id: Uuid,
        profile_version_id: Uuid,
        cases: Vec<RuntimeEvaluationCaseV1>,
        evaluators: Vec<RuntimeEvaluatorV1>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerResultStatusV1 {
    Succeeded,
    Failed,
    Suspended,
    Cancelled,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerTaskV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub protocol_version: u32,
    pub task_id: Uuid,
    pub tenant_id: Uuid,
    pub execution_id: Uuid,
    pub node_execution_id: Uuid,
    pub attempt_id: Uuid,
    pub capability: NodeCapability,
    pub bundle_id: Uuid,
    pub work_package_id: Option<Uuid>,
    pub state_version: u64,
    pub compatibility_hash: ContentHash,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub deadline_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerAttemptLeaseV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub protocol_version: u32,
    pub attempt_id: Uuid,
    pub worker_id: Uuid,
    pub fencing_token: u64,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub locked_until: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerResultV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub protocol_version: u32,
    pub attempt_id: Uuid,
    pub worker_id: Uuid,
    pub fencing_token: u64,
    pub status: WorkerResultStatusV1,
    pub result_hash: ContentHash,
    pub outputs: BTreeMap<String, Vec<agentx_node_protocol::Item>>,
    pub output_object: Option<RuntimeObjectReferenceV1>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    /// Explicit execution-level retry decision. `None` delegates to the
    /// frozen node retry policy; deterministic plugin failures use `false`.
    pub retryable: Option<bool>,
    pub partial_output_object: Option<RuntimeObjectReferenceV1>,
}

impl WorkerResultV1 {
    pub fn computed_result_hash(&self) -> Result<ContentHash, crate::ContractError> {
        crate::content_hash(&serde_json::json!({
            "status": self.status,
            "outputs": self.outputs,
            "outputObject": self.output_object,
            "errorCode": self.error_code,
            "errorMessage": self.error_message,
            "retryable": self.retryable,
            "partialOutputObject": self.partial_output_object,
        }))
    }

    pub fn validate_integrity(&self) -> Result<(), crate::ContractError> {
        if (self.output_object.is_some() && !self.outputs.is_empty())
            || self
                .output_object
                .iter()
                .chain(self.partial_output_object.iter())
                .any(|object| !object.has_canonical_key())
        {
            return Err(crate::ContractError::InvalidImmutableReference);
        }
        if self.computed_result_hash()? != self.result_hash {
            return Err(crate::ContractError::ContentHashMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialExecutionModeV1 {
    Whole,
    Node,
    ToNode,
    FromNode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectResolutionV1 {
    Execute,
    ReuseOutput,
    DryRun,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ExecutionCommandV1 {
    Cancel {
        execution_id: Uuid,
        expected_state_version: u64,
    },
    Fork {
        source_execution_id: Uuid,
        checkpoint_id: Uuid,
        origin: crate::ExecutionOriginV1,
        mode: PartialExecutionModeV1,
        node_id: Option<String>,
        side_effect_resolution: SideEffectResolutionV1,
    },
    SideEffectConfirmation {
        execution_id: Uuid,
        node_execution_id: Uuid,
        resolution: SideEffectResolutionV1,
        expected_state_version: u64,
    },
    WorkPackageCancel {
        package_id: Uuid,
        expected_version: u64,
    },
    RetentionRun {
        run_id: Uuid,
        dry_run: bool,
        policy_version: u64,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeCommandApplyRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub command_id: Uuid,
    pub object_version: u64,
    pub idempotency_key: String,
    pub command: ExecutionCommandV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelWorkPackageRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub package_id: Uuid,
    pub expected_version: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReferenceCheckRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub object_ids: Vec<Uuid>,
    pub bundle_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReferenceCheckReceiptV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub safe_to_delete: bool,
    pub blocking_references: Vec<RuntimeReferenceBlockV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeReferenceBlockV1 {
    pub reference_kind: String,
    pub owner_id: Uuid,
    pub object_id: Option<Uuid>,
    pub bundle_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetentionCommandRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub run_id: Uuid,
    pub policy_version: u64,
    pub dry_run: bool,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResourceStateStatusV1 {
    Active,
    Disabled,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeResourceStateV1 {
    pub tenant_id: Uuid,
    pub resource_kind: RuntimeResourceKindV1,
    pub resource_id: Uuid,
    pub resource_version: String,
    pub state_epoch: u64,
    pub status: RuntimeResourceStateStatusV1,
    pub content_hash: ContentHash,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeGrantStateV1 {
    pub tenant_id: Uuid,
    pub identity_id: Uuid,
    pub grant_id: Uuid,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub operations: BTreeSet<String>,
    pub policy_epoch: u64,
    pub enabled: bool,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum QuotaDimensionV1 {
    ExecutionConcurrency,
    NodeConcurrency,
    SandboxConcurrency,
    AgentIterations,
    Tokens,
    CostMicros,
    ArtifactBytes,
    CpuMillis,
    MemoryBytes,
    Pids,
    DiskBytes,
    TtlSeconds,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeQuotaPolicyV1 {
    pub tenant_id: Uuid,
    pub policy_version: u64,
    pub limits: BTreeMap<QuotaDimensionV1, u64>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalActionValueV1 {
    Claim,
    Release,
    Reassign,
    Cancel,
    Timeout,
    /// A canvas-defined approval button: resumes the workflow on the
    /// `decision:<id>` output port and persists the button id as the decision.
    Decide {
        decision_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeApprovalActionV1 {
    pub task_id: Uuid,
    pub task_version: u64,
    pub action: ApprovalActionValueV1,
    pub actor_id: Uuid,
    pub target_user_id: Option<Uuid>,
    pub input: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeRetentionHoldV1 {
    pub hold_id: Uuid,
    pub aggregate_type: String,
    pub aggregate_id: Uuid,
    pub held: bool,
    pub reason: String,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeRetentionDataTypeV1 {
    Artifact,
    Execution,
    ApplicationMessage,
    EvaluationReport,
    Trace,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeRetentionPolicyV1 {
    pub tenant_id: Uuid,
    pub policy_version: u64,
    pub retention_days: BTreeMap<RuntimeRetentionDataTypeV1, u32>,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeApprovalCandidateKindV1 {
    User,
    Role,
    Department,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeApprovalCandidateV1 {
    pub candidate_type: RuntimeApprovalCandidateKindV1,
    pub candidate_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeApprovalButtonV1 {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeEvaluationRuleResultV1 {
    pub id: Uuid,
    pub profile_rule_id: Uuid,
    pub status: String,
    pub passed: Option<bool>,
    pub score: Option<f64>,
    pub detail: Value,
    pub duration_ms: Option<u64>,
    pub cost_micros: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeEvaluationCaseResultV1 {
    pub id: Uuid,
    pub source_case_id: Uuid,
    pub target_command_id: Uuid,
    pub target_execution_id: Option<Uuid>,
    pub status: String,
    pub actual_output: Option<Value>,
    pub duration_ms: Option<u64>,
    pub cost_micros: u64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub rules: Vec<RuntimeEvaluationRuleResultV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeEvaluationMetricsV1 {
    pub total_cases: u64,
    pub completed_cases: u64,
    pub passed_rules: u64,
    pub failed_rules: u64,
    pub error_rules: u64,
    pub total_cost_micros: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeEvaluationReportV1 {
    pub cases: Vec<RuntimeEvaluationCaseResultV1>,
    pub metrics: RuntimeEvaluationMetricsV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeRetentionItemV1 {
    pub id: Uuid,
    pub data_type: String,
    pub target_id: String,
    pub status: String,
    pub reason: Option<String>,
    pub attempt_count: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeEventPayloadV1 {
    ExecutionChanged {
        execution_id: Uuid,
        invocation_id: Option<Uuid>,
        application_id: Option<Uuid>,
        workflow_id: Uuid,
        workflow_version_id: Uuid,
        bundle_id: Uuid,
        status: String,
        state_version: u64,
        admission_epoch: u64,
        trace_watermark: u64,
        result_hash: Option<ContentHash>,
        output_object: Option<RuntimeObjectReferenceV1>,
        error: Option<Value>,
    },
    ExecutionTerminal {
        execution_id: Uuid,
        status: String,
        result_hash: Option<ContentHash>,
        output_object: Option<RuntimeObjectReferenceV1>,
    },
    ApprovalChanged {
        task_id: Uuid,
        task_version: u64,
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
    EvaluationChanged {
        run_id: Uuid,
        run_version: u64,
        work_package_id: Uuid,
        status: String,
        completed_cases: u64,
        total_cases: u64,
        report: Option<RuntimeEvaluationReportV1>,
    },
    DebugChanged {
        package_id: Uuid,
        package_version: u64,
        work_package_id: Uuid,
        status: String,
        result: Option<Value>,
        #[schemars(with = "String")]
        #[serde(with = "time::serde::rfc3339")]
        expires_at: OffsetDateTime,
    },
    RetentionChanged {
        run_id: Uuid,
        run_version: u64,
        status: String,
        marked_count: u64,
        deleted_count: u64,
        failed_count: u64,
        dry_run: bool,
        items: Vec<RuntimeRetentionItemV1>,
    },
    NotificationChanged {
        notification_id: Uuid,
        notification_version: u64,
        notification_type: String,
        title_key: String,
        body_key: String,
        arguments: Value,
        target_type: String,
        target_id: Uuid,
        target_path: String,
        tone: String,
    },
    RuntimeCallChanged {
        call_id: Uuid,
        status: String,
        provider_request_id: Option<String>,
    },
}

#[must_use]
pub const fn current_engine_api_version() -> u32 {
    INTERNAL_API_VERSION
}
