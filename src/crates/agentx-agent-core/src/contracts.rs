use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ReplayPolicyV1, ToolDefinitionV1};

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
pub enum ContractValidationError {
    #[error("AGENT_MODEL_REQUIRED")]
    AgentModelRequired,
    #[error("AGENT_RESOURCE_SLOT_INVALID: {0}")]
    AgentResourceSlotInvalid(String),
    #[error("AGENT_CORE_TOOLS_REQUIRE_WORKSPACE_SANDBOX")]
    AgentCoreToolsRequireWorkspaceSandbox,
    #[error("MCP_STDIO_SANDBOX_REQUIRED")]
    McpStdioSandboxRequired,
    #[error("MCP_RUNTIME_SANDBOX_FORBIDDEN")]
    McpRuntimeSandboxForbidden,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResourceReferenceV1 {
    pub resource_type: String,
    pub resource_id: String,
    pub resource_version_id: String,
    pub operation: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPolicyModeV1 {
    ApplicationSession,
    Invocation,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionPolicyV1 {
    pub mode: SessionPolicyModeV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentDefinitionV6 {
    pub api_version: String,
    pub model: ResourceReferenceV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_sandbox: Option<ResourceReferenceV1>,
    pub session_policy: SessionPolicyV1,
    #[serde(default)]
    pub mcp_tools: Vec<ResourceReferenceV1>,
    #[serde(default)]
    pub skills: Vec<ResourceReferenceV1>,
    #[serde(default)]
    pub knowledge: Vec<ResourceReferenceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_term_memory: Option<ResourceReferenceV1>,
}

impl AgentDefinitionV6 {
    pub fn validate(&self) -> Result<(), ContractValidationError> {
        if self.api_version != "6.0"
            || self.model.resource_type != "model"
            || self.model.operation != "use"
            || self.model.resource_version_id.trim().is_empty()
        {
            return Err(ContractValidationError::AgentModelRequired);
        }
        if self.workspace_sandbox.as_ref().is_some_and(|sandbox| {
            sandbox.resource_type != "sandbox_profile"
                || sandbox.operation != "use"
                || sandbox.resource_version_id.trim().is_empty()
        }) {
            return Err(ContractValidationError::AgentResourceSlotInvalid(
                "workspace_sandbox".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotPlacementV2 {
    Inspector,
    Canvas,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResourceSlotV2 {
    pub name: String,
    pub resource_type: String,
    pub placement: SlotPlacementV2,
    pub minimum: u32,
    pub maximum: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentManifestV2 {
    pub manifest_version: String,
    pub node_type: String,
    pub resource_slots: Vec<ResourceSlotV2>,
}

pub fn frozen_agent_manifest_v2() -> AgentManifestV2 {
    AgentManifestV2 {
        manifest_version: "2.0".into(),
        node_type: "agent".into(),
        resource_slots: vec![
            ResourceSlotV2 {
                name: "model".into(),
                resource_type: "model".into(),
                placement: SlotPlacementV2::Inspector,
                minimum: 1,
                maximum: Some(1),
            },
            ResourceSlotV2 {
                name: "workspace_sandbox".into(),
                resource_type: "sandbox_profile".into(),
                placement: SlotPlacementV2::Inspector,
                minimum: 0,
                maximum: Some(1),
            },
            ResourceSlotV2 {
                name: "mcp_tools".into(),
                resource_type: "mcp_tool".into(),
                placement: SlotPlacementV2::Canvas,
                minimum: 0,
                maximum: None,
            },
            ResourceSlotV2 {
                name: "skills".into(),
                resource_type: "skill".into(),
                placement: SlotPlacementV2::Canvas,
                minimum: 0,
                maximum: None,
            },
            ResourceSlotV2 {
                name: "knowledge".into(),
                resource_type: "rag".into(),
                placement: SlotPlacementV2::Canvas,
                minimum: 0,
                maximum: None,
            },
            ResourceSlotV2 {
                name: "long_term_memory".into(),
                resource_type: "memory".into(),
                placement: SlotPlacementV2::Canvas,
                minimum: 0,
                maximum: Some(1),
            },
        ],
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentBundleV2 {
    pub bundle_version: String,
    pub core_contract_version: String,
    pub reference_commit: String,
    pub model: ResourceReferenceV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_sandbox: Option<ResourceReferenceV1>,
    pub core_tools: Vec<ToolDefinitionV1>,
    pub attachments: Vec<ResourceReferenceV1>,
}

pub fn derive_agent_bundle_v2(
    definition: &AgentDefinitionV6,
) -> Result<AgentBundleV2, ContractValidationError> {
    definition.validate()?;
    let mut attachments = Vec::new();
    attachments.extend(definition.mcp_tools.clone());
    attachments.extend(definition.skills.clone());
    attachments.extend(definition.knowledge.clone());
    attachments.extend(definition.long_term_memory.clone());
    attachments.sort_by(|left, right| {
        (
            left.resource_type.as_str(),
            left.resource_id.as_str(),
            left.resource_version_id.as_str(),
        )
            .cmp(&(
                right.resource_type.as_str(),
                right.resource_id.as_str(),
                right.resource_version_id.as_str(),
            ))
    });
    Ok(AgentBundleV2 {
        bundle_version: "2.0".into(),
        core_contract_version: crate::CORE_CONTRACT_VERSION.into(),
        reference_commit: crate::PI_REFERENCE_COMMIT.into(),
        model: definition.model.clone(),
        workspace_sandbox: definition.workspace_sandbox.clone(),
        core_tools: crate::core_tool_registry(definition.workspace_sandbox.is_some()),
        attachments,
    })
}

/// Pi Agent 风格的自动压缩策略
///
/// 根据模型的 context window 自动计算压缩阈值和保留策略。
/// 不暴露给外部配置，完全由系统内部管理。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompactionStrategy {
    context_window: u64,
}

impl CompactionStrategy {
    /// 从模型的 context window 创建压缩策略
    pub fn new(context_window: u64) -> Self {
        Self { context_window }
    }

    /// 触发压缩的阈值（72% 的 context window）
    ///
    /// 参考 Pi Agent 的策略，在 70-75% 时触发压缩
    pub fn threshold_tokens(&self) -> u64 {
        (self.context_window as f64 * 0.72) as u64
    }

    /// 保留最近对话的 token 预算（15% 的 context window）
    ///
    /// 用于智能选择保留哪些消息
    pub fn keep_recent_tokens(&self) -> u64 {
        (self.context_window as f64 * 0.15) as u64
    }

    /// 预留给系统提示、外部上下文、模型输出的空间（5%）
    #[allow(dead_code)]
    pub fn reserve_tokens(&self) -> u64 {
        (self.context_window as f64 * 0.05) as u64
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalContextOriginV1 {
    Skill,
    StudioDocument,
}

/// An immutable, provenance-carrying context fragment resolved by the Worker
/// from the signed Bundle. Core orders it before the invocation's user
/// message, but never derives tool authorization from its contents.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExternalContextV1 {
    pub context_id: String,
    pub origin: ExternalContextOriginV1,
    pub source_resource_id: String,
    pub source_resource_version_id: String,
    pub content_hash: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentRunInputV1 {
    pub api_version: u32,
    pub run_id: String,
    pub session_id: String,
    /// Durable Entry/Register projection used for recovery.  It is optional
    /// for the offline Core spike; production Worker always supplies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<crate::AgentSessionProjectionV1>,
    pub fencing_token: u64,
    pub deadline_at_millis: u64,
    pub model_reference: String,
    /// The Agent Inspector system prompt is projected for every Model Effect
    /// but is intentionally not appended to durable Session history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_sandbox_binding: Option<String>,
    #[serde(default)]
    pub attachment_tools: Vec<ToolDefinitionV1>,
    #[serde(default)]
    pub external_contexts: Vec<ExternalContextV1>,
    pub prompt_message_id: String,
    pub prompt: String,
    #[serde(default)]
    pub steering_inputs: Vec<crate::AgentMessageV1>,
    #[serde(default)]
    pub follow_up_inputs: Vec<crate::AgentMessageV1>,
    /// 模型的 context window 大小（tokens），用于自动计算压缩策略
    pub model_context_window: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProcessLeaseV1 {
    pub lease_id: String,
    pub fencing_token: u64,
    pub expires_at_millis: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SandboxProcessSessionContractV1 {
    pub process_session_id: String,
    pub server_version_id: String,
    pub runtime_sandbox: ResourceReferenceV1,
    pub command: String,
    pub args: Vec<String>,
    pub max_frame_bytes: u64,
    pub idle_timeout_millis: u64,
    pub heartbeat_interval_millis: u64,
    pub lease: ProcessLeaseV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransportV1 {
    StreamableHttp {
        endpoint: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential: Option<ResourceReferenceV1>,
    },
    Sse {
        endpoint: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential: Option<ResourceReferenceV1>,
    },
    Stdio {
        command: String,
        args: Vec<String>,
        #[serde(default)]
        environment_credential_refs: Vec<ResourceReferenceV1>,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct McpServerVersionV1 {
    pub server_version_id: String,
    pub transport: McpTransportV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_sandbox: Option<ResourceReferenceV1>,
}

impl McpServerVersionV1 {
    pub fn validate(&self) -> Result<(), ContractValidationError> {
        match (&self.transport, &self.runtime_sandbox) {
            (McpTransportV1::Stdio { .. }, None) => {
                Err(ContractValidationError::McpStdioSandboxRequired)
            }
            (McpTransportV1::Stdio { .. }, Some(sandbox))
                if sandbox.resource_type != "sandbox_profile"
                    || sandbox.operation != "use"
                    || sandbox.resource_version_id.trim().is_empty() =>
            {
                Err(ContractValidationError::McpStdioSandboxRequired)
            }
            (McpTransportV1::Stdio { .. }, Some(_)) => Ok(()),
            (_, Some(_)) => Err(ContractValidationError::McpRuntimeSandboxForbidden),
            (_, None) => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectPolicyV1 {
    pub effect_kind: String,
    pub replay_policy: ReplayPolicyV1,
}
