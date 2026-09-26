use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::{SigningKey, VerifyingKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    BUNDLE_SCHEMA_VERSION, CompiledWorkflowV1, ContentHash, Ed25519Signature,
    RuntimeAuthorizationSnapshotV1, RuntimeCallPurposeV1, RuntimeMcpTransportV2, RuntimePolicyV1,
    RuntimeResourceBindingV1, RuntimeResourceConfigurationV1, RuntimeResourceKindV1,
    RuntimeWorkPackageOverlayV1, WorkerCompatibilityV1,
};

pub const RUNTIME_OBJECT_KEY_PREFIX: &str = "runtime";

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeObjectReferenceV1 {
    pub tenant_id: Uuid,
    pub storage_domain: StorageDomain,
    pub object_id: Uuid,
    pub object_key: String,
    pub content_hash: ContentHash,
    pub size_bytes: u64,
    pub media_type: String,
}

impl RuntimeObjectReferenceV1 {
    #[must_use]
    pub fn canonical_key(tenant_id: Uuid, object_id: Uuid, content_hash: &ContentHash) -> String {
        format!(
            "{RUNTIME_OBJECT_KEY_PREFIX}/{tenant_id}/{object_id}/{}",
            content_hash
                .as_str()
                .strip_prefix("sha256:")
                .expect("ContentHash always carries the sha256 prefix")
        )
    }

    #[must_use]
    pub fn has_canonical_key(&self) -> bool {
        self.storage_domain == StorageDomain::Runtime
            && self.object_key
                == Self::canonical_key(self.tenant_id, self.object_id, &self.content_hash)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageDomain {
    Runtime,
    Observability,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKindV1 {
    CompositeWorkflow,
    Skill,
    Model,
    Mcp,
    Rag,
    Memory,
    Credential,
    SandboxProfile,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DependencyClosureEntryV1 {
    pub kind: DependencyKindV1,
    pub object_id: Uuid,
    pub version: String,
    pub content_hash: ContentHash,
    pub object_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DependencyClosureV1 {
    pub entries: Vec<DependencyClosureEntryV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionSpecPayloadV2 {
    #[serde(deserialize_with = "crate::deserialize_v2")]
    pub schema_version: u32,
    pub bundle_id: Uuid,
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub deployment_id: Uuid,
    pub workflow_id: Uuid,
    pub workflow_version_id: Uuid,
    pub workflow_name: String,
    pub workflow_version_number: u64,
    pub workflow_owner_department: Option<crate::ExecutionDepartmentSnapshotV1>,
    pub bundle_sequence: u64,
    pub definition: Value,
    pub compiled_ir: CompiledWorkflowV1,
    pub node_manifests: Vec<Value>,
    pub agent_bundle: AgentBundleContractV2,
    pub input_contract: Value,
    pub output_contract: Value,
    pub context_contract: Value,
    pub resources: Vec<RuntimeResourceBindingV1>,
    pub authorization: RuntimeAuthorizationSnapshotV1,
    pub dependency_closure: DependencyClosureV1,
    pub triggers: Vec<crate::RuntimeTriggerSpecV1>,
    pub runtime_policy: RuntimePolicyV1,
    pub objects: Vec<RuntimeObjectReferenceV1>,
    pub worker_compatibility: WorkerCompatibilityV1,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl ExecutionSpecPayloadV2 {
    #[must_use]
    pub fn current_schema_version() -> u32 {
        BUNDLE_SCHEMA_VERSION
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionSpecBundleV2 {
    pub payload: ExecutionSpecPayloadV2,
    pub content_hash: ContentHash,
    pub signature: Ed25519Signature,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPackagePurpose {
    Debug,
    Evaluation,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeWorkPackagePayloadV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub package_id: Uuid,
    pub tenant_id: Uuid,
    pub workflow: crate::ExecutionWorkflowSnapshotV1,
    pub origin: crate::ExecutionOriginV1,
    pub purpose: WorkPackagePurpose,
    pub call_purpose: RuntimeCallPurposeV1,
    pub spec: crate::RuntimeWorkPackageSpecV1,
    pub model_evaluator_executions: Vec<crate::RuntimeModelEvaluatorExecutionV1>,
    pub source_revision: String,
    pub definition: Value,
    pub compiled_ir: CompiledWorkflowV1,
    pub node_manifests: Vec<Value>,
    pub agent_bundle: AgentBundleContractV2,
    pub overlay: RuntimeWorkPackageOverlayV1,
    pub resource_closure: DependencyClosureV1,
    pub resources: Vec<RuntimeResourceBindingV1>,
    pub authorization: RuntimeAuthorizationSnapshotV1,
    pub objects: Vec<RuntimeObjectReferenceV1>,
    pub runtime_policy: RuntimePolicyV1,
    pub worker_compatibility: WorkerCompatibilityV1,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

pub const AGENT_BUNDLE_VERSION: &str = "2.0";
pub const AGENT_CORE_CONTRACT_VERSION: &str = "1.1";
pub const PI_REFERENCE_REPOSITORY: &str = "https://github.com/earendil-works/pi";
pub const PI_REFERENCE_COMMIT: &str = "a69bef789bc95abf0acee16f7b4660b70b650bb9";
pub const PI_REFERENCE_PACKAGE_VERSION: &str = "0.84.2";

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBundleContractV2 {
    pub bundle_version: String,
    pub core_contract_version: String,
    pub reference_repository: String,
    pub reference_commit: String,
    pub reference_package_version: String,
    pub definition_hash: String,
    pub compiler_version: String,
    pub manifest_hashes: BTreeMap<String, ContentHash>,
    pub policy_epoch: u64,
    pub grant_ids: Vec<Uuid>,
    pub agents: Vec<crate::CompiledAgentBundleEntryV2>,
}

impl AgentBundleContractV2 {
    #[must_use]
    pub fn empty(definition_hash: impl Into<String>, compiler_version: impl Into<String>) -> Self {
        Self {
            bundle_version: AGENT_BUNDLE_VERSION.into(),
            core_contract_version: AGENT_CORE_CONTRACT_VERSION.into(),
            reference_repository: PI_REFERENCE_REPOSITORY.into(),
            reference_commit: PI_REFERENCE_COMMIT.into(),
            reference_package_version: PI_REFERENCE_PACKAGE_VERSION.into(),
            definition_hash: definition_hash.into(),
            compiler_version: compiler_version.into(),
            manifest_hashes: BTreeMap::new(),
            policy_epoch: 0,
            grant_ids: Vec::new(),
            agents: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompiledAgentBundleEntryV2 {
    pub node_id: String,
    pub configuration: crate::CompiledAgentNodeV2,
    pub resource_closure: Vec<AgentResourceClosureEntryV2>,
    pub attachment_registry: AgentAttachmentRegistryV1,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentAttachmentRegistryV1 {
    pub contexts: Vec<AgentExternalContextBindingV1>,
    pub tools: Vec<AgentAttachmentToolV1>,
    pub authorization_evidence: Vec<AgentCapabilityAuthorizationEvidenceV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentExternalContextBindingV1 {
    pub context_id: String,
    pub origin: String,
    pub resource_id: Uuid,
    pub resource_version_id: Uuid,
    pub object_id: Uuid,
    pub content_hash: ContentHash,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentAttachmentToolV1 {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub origin: String,
    pub replay_policy: String,
    pub resource_id: Uuid,
    pub resource_version_id: Uuid,
    pub operation: String,
    /// `run` is the P3-04 namespace; `subject` is reserved for the
    /// long-term-memory slot and requires trusted subject evidence.
    pub scope: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCapabilityAuthorizationEvidenceV1 {
    pub resource_type: String,
    pub resource_id: Uuid,
    /// Versioned resources carry their exact UUID. Fixed dependencies such as
    /// Vault-backed credentials intentionally carry no resource version.
    pub resource_version_id: Option<Uuid>,
    pub operation: String,
    pub policy_epoch: u64,
    pub grant_ids: Vec<Uuid>,
}

impl AgentCapabilityAuthorizationEvidenceV1 {
    /// Returns the wire-level resource version used by runtime bindings.
    /// Fixed dependencies (currently Vault credentials) use the literal
    /// `fixed` marker instead of pretending to have a UUID version.
    #[must_use]
    pub fn binding_version(&self) -> String {
        self.resource_version_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "fixed".into())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentKnowledgeCitationV1 {
    pub knowledge_resource_id: Uuid,
    pub document_id: String,
    pub chunk_id: String,
    pub title: Option<String>,
    pub uri: Option<String>,
    pub score: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentKnowledgeDocumentSummaryV1 {
    pub content: String,
    pub truncated: bool,
    pub metadata: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentKnowledgeSearchResultV1 {
    pub documents: Vec<AgentKnowledgeDocumentSummaryV1>,
    pub citations: Vec<AgentKnowledgeCitationV1>,
    pub trust: String,
    pub prompt_boundary: String,
    pub artifact_refs: Vec<String>,
    pub truncated: bool,
}

pub fn derive_attachment_registry(
    configuration: &crate::CompiledAgentNodeV2,
    resources: &[RuntimeResourceBindingV1],
    policy_epoch: u64,
    grant_bindings: &[crate::RuntimeGrantBindingV1],
) -> Result<AgentAttachmentRegistryV1, String> {
    let mut registry = AgentAttachmentRegistryV1::default();
    let mut names = BTreeSet::from_iter(
        configuration
            .core_tools
            .iter()
            .map(|tool| tool.name.clone()),
    );
    let mut direct_attachments = BTreeSet::new();
    for attachment in &configuration.attachments {
        if !direct_attachments.insert((
            attachment.resource_type,
            attachment.resource_id,
            attachment.resource_version_id,
            attachment.operation,
        )) {
            return Err(format!(
                "AGENT_ATTACHMENT_BINDING_DUPLICATE: {}",
                attachment.resource_id
            ));
        }
    }
    let attachments = expand_skill_attachments(configuration, resources)?;
    let mut skill_resource_added = false;
    for attachment in &attachments {
        let binding = resources
            .iter()
            .find(|resource| {
                resource.resource_id == attachment.resource_id
                    && resource.resource_version == attachment.resource_version_id.to_string()
            })
            .ok_or_else(|| {
                format!(
                    "AGENT_ATTACHMENT_BINDING_MISSING: {}",
                    attachment.resource_id
                )
            })?;
        push_authorization_evidence(
            &mut registry.authorization_evidence,
            attachment.resource_type.as_str(),
            binding,
            attachment.operation.as_str(),
            policy_epoch,
            grant_bindings,
        )?;
        let dependency_bindings = attachment_dependency_bindings(binding, resources);
        let mut add_tool = |tool: AgentAttachmentToolV1| -> Result<(), String> {
            if !valid_agent_tool_name(&tool.name) || !names.insert(tool.name.clone()) {
                return Err(format!("AGENT_TOOL_NAME_CONFLICT: {}", tool.name));
            }
            registry.tools.push(tool);
            Ok(())
        };
        match &binding.configuration {
            RuntimeResourceConfigurationV1::Mcp {
                tool_name,
                input_schema,
                side_effect,
                ..
            } if tool_name != "__server__" => add_tool(AgentAttachmentToolV1 {
                name: tool_name.clone(),
                description: "Runtime-pinned MCP tool".into(),
                input_schema: input_schema.clone(),
                origin: "mcp".into(),
                replay_policy: match side_effect.as_str() {
                    "none" | "read_only" => "safe",
                    "idempotent" => "idempotency_required",
                    _ => "never",
                }
                .into(),
                resource_id: attachment.resource_id,
                resource_version_id: attachment.resource_version_id,
                operation: attachment.operation.as_str().into(),
                scope: "run".into(),
            })?,
            RuntimeResourceConfigurationV1::Skill {
                entrypoint_object_id,
                entrypoint_content_hash,
                dependency_object_ids,
                ..
            } => {
                registry.contexts.push(AgentExternalContextBindingV1 {
                    context_id: format!("skill:{}", attachment.resource_version_id),
                    origin: "skill".into(),
                    resource_id: attachment.resource_id,
                    resource_version_id: attachment.resource_version_id,
                    object_id: *entrypoint_object_id,
                    content_hash: entrypoint_content_hash.clone(),
                });
                if !dependency_object_ids.is_empty() && !skill_resource_added {
                    add_tool(AgentAttachmentToolV1 {
                        name: "skill_resource".into(),
                        description: "Read an immutable signed Skill asset.".into(),
                        input_schema: serde_json::json!({
                            "type":"object","additionalProperties":false,
                            "required":["skillVersionId","path"],
                            "properties":{
                                "skillVersionId":{"type":"string","format":"uuid"},
                                "path":{"type":"string","minLength":1},
                                "startByte":{"type":"integer","minimum":0},
                                "maxBytes":{"type":"integer","minimum":1,"maximum":8388608},
                                "encoding":{"enum":["utf8","base64"]}
                            }
                        }),
                        origin: "skill".into(),
                        replay_policy: "safe".into(),
                        resource_id: attachment.resource_id,
                        resource_version_id: attachment.resource_version_id,
                        operation: attachment.operation.as_str().into(),
                        scope: "run".into(),
                    })?;
                    skill_resource_added = true;
                }
            }
            RuntimeResourceConfigurationV1::Rag { .. } => add_tool(AgentAttachmentToolV1 {
                name: format!("knowledge_search_{}", attachment.resource_id.simple()),
                description: "Search an authorized Knowledge resource and return citations.".into(),
                input_schema: serde_json::json!({
                    "type":"object","additionalProperties":false,"required":["query"],
                    "properties":{"query":{"type":"string","minLength":1},"topK":{"type":"integer","minimum":1,"maximum":20},"filters":{"type":"object"}}
                }),
                origin: "knowledge".into(),
                replay_policy: "safe".into(),
                resource_id: attachment.resource_id,
                resource_version_id: attachment.resource_version_id,
                operation: attachment.operation.as_str().into(),
                scope: "run".into(),
            })?,
            RuntimeResourceConfigurationV1::Memory { access_mode, .. } => {
                if matches!(
                    attachment.operation,
                    agentx_domain::ResourceOperation::Use
                        | agentx_domain::ResourceOperation::Read
                        | agentx_domain::ResourceOperation::Write
                        | agentx_domain::ResourceOperation::Manage
                ) {
                    add_tool(AgentAttachmentToolV1 {
                        name: "memory_recall".into(),
                        description: "Recall data from the run-scoped authorized Memory namespace."
                            .into(),
                        input_schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["query"],"properties":{"query":{"type":"string","minLength":1},"topK":{"type":"integer","minimum":1,"maximum":20}}}),
                        origin: "memory".into(),
                        replay_policy: "safe".into(),
                        resource_id: attachment.resource_id,
                        resource_version_id: attachment.resource_version_id,
                        operation: "read".into(),
                        scope: if attachment.binding_role == "long_term_memory" {
                            "subject"
                        } else {
                            "run"
                        }
                        .into(),
                    })?;
                }
                if access_mode == "read_write"
                    && matches!(
                        attachment.operation,
                        agentx_domain::ResourceOperation::Write
                            | agentx_domain::ResourceOperation::Manage
                    )
                {
                    add_tool(AgentAttachmentToolV1 {
                        name: "memory_write".into(),
                        description: "Write data to the run-scoped authorized Memory namespace."
                            .into(),
                        input_schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["text"],"properties":{"text":{"type":"string","minLength":1},"metadata":{"type":"object"}}}),
                        origin: "memory".into(),
                        replay_policy: "never".into(),
                        resource_id: attachment.resource_id,
                        resource_version_id: attachment.resource_version_id,
                        operation: "write".into(),
                        scope: if attachment.binding_role == "long_term_memory" {
                            "subject"
                        } else {
                            "run"
                        }
                        .into(),
                    })?;
                }
            }
            _ => {
                return Err(format!(
                    "AGENT_ATTACHMENT_BINDING_INVALID: {}",
                    attachment.resource_id
                ));
            }
        }
        for dependency in dependency_bindings {
            push_authorization_evidence(
                &mut registry.authorization_evidence,
                runtime_resource_type(dependency),
                dependency,
                "use",
                policy_epoch,
                grant_bindings,
            )?;
        }
    }
    if registry.tools.len() > 128 {
        return Err("AGENT_TOOL_REGISTRY_LIMIT_EXCEEDED: maximum 128 tools".into());
    }
    let schema_bytes = registry.tools.iter().try_fold(0_usize, |total, tool| {
        serde_json::to_vec(&tool.input_schema)
            .map(|encoded| total.saturating_add(encoded.len()))
            .map_err(|error| format!("AGENT_TOOL_SCHEMA_INVALID: {error}"))
    })?;
    if schema_bytes > 1024 * 1024 {
        return Err("AGENT_TOOL_SCHEMA_LIMIT_EXCEEDED: maximum 1 MiB".into());
    }
    if registry.contexts.len() > 64 {
        return Err("AGENT_EXTERNAL_CONTEXT_LIMIT_EXCEEDED: maximum 64 contexts".into());
    }
    registry
        .contexts
        .sort_by(|left, right| left.context_id.cmp(&right.context_id));
    registry
        .tools
        .sort_by(|left, right| left.name.cmp(&right.name));
    registry.authorization_evidence.sort_by_key(|item| {
        (
            item.resource_type.clone(),
            item.resource_id,
            item.resource_version_id,
        )
    });
    registry.authorization_evidence.dedup_by(|left, right| {
        left.resource_type == right.resource_type
            && left.resource_id == right.resource_id
            && left.resource_version_id == right.resource_version_id
            && left.operation == right.operation
    });
    Ok(registry)
}

fn expand_skill_attachments(
    configuration: &crate::CompiledAgentNodeV2,
    resources: &[RuntimeResourceBindingV1],
) -> Result<Vec<crate::CompiledAgentAttachmentV2>, String> {
    validate_skill_dependency_cycles(configuration, resources)?;
    let mut expanded = configuration.attachments.clone();
    let mut seen = expanded
        .iter()
        .map(|attachment| {
            (
                attachment.resource_type,
                attachment.resource_id,
                attachment.resource_version_id,
                attachment.operation,
            )
        })
        .collect::<BTreeSet<_>>();
    let mut versions = expanded
        .iter()
        .map(|attachment| {
            (
                (attachment.resource_type, attachment.resource_id),
                attachment.resource_version_id,
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut index = 0;
    while index < expanded.len() {
        let attachment = expanded[index].clone();
        index += 1;
        if attachment.resource_type != agentx_domain::ResourceType::Skill {
            continue;
        }
        let binding = resources
            .iter()
            .find(|resource| {
                resource.resource_id == attachment.resource_id
                    && resource.resource_version == attachment.resource_version_id.to_string()
            })
            .ok_or_else(|| {
                format!(
                    "AGENT_ATTACHMENT_BINDING_MISSING: {}",
                    attachment.resource_id
                )
            })?;
        let RuntimeResourceConfigurationV1::Skill { dependencies, .. } = &binding.configuration
        else {
            return Err(format!(
                "AGENT_ATTACHMENT_BINDING_INVALID: {}",
                attachment.resource_id
            ));
        };
        for dependency in dependencies {
            let resource_type = parse_skill_resource_type(&dependency.resource_type)?;
            if !matches!(
                resource_type,
                agentx_domain::ResourceType::McpTool
                    | agentx_domain::ResourceType::Skill
                    | agentx_domain::ResourceType::Rag
                    | agentx_domain::ResourceType::Memory
            ) {
                continue;
            }
            let resource_version_id = dependency.resource_version_id.ok_or_else(|| {
                format!(
                    "AGENT_ATTACHMENT_VERSION_NOT_EXACT: {}",
                    dependency.resource_id
                )
            })?;
            let operation = parse_skill_operation(&dependency.operation)?;
            if let Some(existing) =
                versions.insert((resource_type, dependency.resource_id), resource_version_id)
                && existing != resource_version_id
            {
                return Err(format!(
                    "AGENT_SKILL_DEPENDENCY_VERSION_CONFLICT: {}",
                    dependency.resource_id
                ));
            }
            if seen.insert((
                resource_type,
                dependency.resource_id,
                resource_version_id,
                operation,
            )) {
                expanded.push(crate::CompiledAgentAttachmentV2 {
                    binding_role: "skill_dependency".into(),
                    resource_type,
                    resource_id: dependency.resource_id,
                    resource_version_id,
                    operation,
                });
            }
        }
    }
    Ok(expanded)
}

fn validate_skill_dependency_cycles(
    configuration: &crate::CompiledAgentNodeV2,
    resources: &[RuntimeResourceBindingV1],
) -> Result<(), String> {
    fn visit(
        resource_id: Uuid,
        resource_version_id: Uuid,
        resources: &[RuntimeResourceBindingV1],
        visiting: &mut BTreeSet<(Uuid, Uuid)>,
        visited: &mut BTreeSet<(Uuid, Uuid)>,
    ) -> Result<(), String> {
        let key = (resource_id, resource_version_id);
        if visited.contains(&key) {
            return Ok(());
        }
        if !visiting.insert(key) {
            return Err(format!(
                "AGENT_SKILL_DEPENDENCY_CYCLE: {resource_id}@{resource_version_id}"
            ));
        }
        let binding = resources
            .iter()
            .find(|binding| {
                binding.resource_id == resource_id
                    && binding.resource_version == resource_version_id.to_string()
            })
            .ok_or_else(|| format!("AGENT_ATTACHMENT_BINDING_MISSING: {resource_id}"))?;
        let RuntimeResourceConfigurationV1::Skill { dependencies, .. } = &binding.configuration
        else {
            return Err(format!("AGENT_ATTACHMENT_BINDING_INVALID: {resource_id}"));
        };
        for dependency in dependencies {
            if parse_skill_resource_type(&dependency.resource_type)?
                != agentx_domain::ResourceType::Skill
            {
                continue;
            }
            let dependency_version = dependency.resource_version_id.ok_or_else(|| {
                format!(
                    "AGENT_ATTACHMENT_VERSION_NOT_EXACT: {}",
                    dependency.resource_id
                )
            })?;
            visit(
                dependency.resource_id,
                dependency_version,
                resources,
                visiting,
                visited,
            )?;
        }
        visiting.remove(&key);
        visited.insert(key);
        Ok(())
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for attachment in &configuration.attachments {
        if attachment.resource_type == agentx_domain::ResourceType::Skill {
            visit(
                attachment.resource_id,
                attachment.resource_version_id,
                resources,
                &mut visiting,
                &mut visited,
            )?;
        }
    }
    Ok(())
}

fn parse_skill_resource_type(value: &str) -> Result<agentx_domain::ResourceType, String> {
    match value {
        "credential" => Ok(agentx_domain::ResourceType::Credential),
        "model" => Ok(agentx_domain::ResourceType::Model),
        "mcp_server" => Ok(agentx_domain::ResourceType::McpServer),
        "mcp_tool" => Ok(agentx_domain::ResourceType::McpTool),
        "skill" => Ok(agentx_domain::ResourceType::Skill),
        "rag" | "knowledge" => Ok(agentx_domain::ResourceType::Rag),
        "memory" => Ok(agentx_domain::ResourceType::Memory),
        "sandbox_profile" => Ok(agentx_domain::ResourceType::SandboxProfile),
        other => Err(format!("AGENT_SKILL_DEPENDENCY_TYPE_INVALID: {other}")),
    }
}

fn parse_skill_operation(value: &str) -> Result<agentx_domain::ResourceOperation, String> {
    match value {
        "view" => Ok(agentx_domain::ResourceOperation::View),
        "use" => Ok(agentx_domain::ResourceOperation::Use),
        "read" => Ok(agentx_domain::ResourceOperation::Read),
        "write" => Ok(agentx_domain::ResourceOperation::Write),
        "manage" => Ok(agentx_domain::ResourceOperation::Manage),
        other => Err(format!("AGENT_SKILL_DEPENDENCY_OPERATION_INVALID: {other}")),
    }
}

fn push_authorization_evidence(
    evidence: &mut Vec<AgentCapabilityAuthorizationEvidenceV1>,
    resource_type: &str,
    binding: &RuntimeResourceBindingV1,
    operation: &str,
    policy_epoch: u64,
    grant_bindings: &[crate::RuntimeGrantBindingV1],
) -> Result<(), String> {
    let resource_version_id = if binding.resource_version == "fixed" {
        None
    } else {
        Some(Uuid::parse_str(&binding.resource_version).map_err(|_| {
            format!(
                "AGENT_ATTACHMENT_VERSION_NOT_EXACT: {}",
                binding.resource_id
            )
        })?)
    };
    let mut exact_grants = grant_bindings
        .iter()
        .filter(|grant| {
            grant.resource_type == resource_type
                && grant.resource_id == binding.resource_id
                && grant
                    .resource_version_id
                    .is_none_or(|version| Some(version) == resource_version_id)
                && grant_operation_allows(&grant.operation, operation)
        })
        .map(|grant| grant.grant_id)
        .collect::<Vec<_>>();
    exact_grants.sort_unstable();
    exact_grants.dedup();
    if exact_grants.is_empty() {
        return Err(format!(
            "AGENT_ATTACHMENT_GRANT_MISSING: {resource_type}:{}:{operation}",
            binding.resource_id
        ));
    }
    evidence.push(AgentCapabilityAuthorizationEvidenceV1 {
        resource_type: resource_type.into(),
        resource_id: binding.resource_id,
        resource_version_id,
        operation: operation.into(),
        policy_epoch,
        grant_ids: exact_grants,
    });
    Ok(())
}

fn grant_operation_allows(granted: &str, requested: &str) -> bool {
    granted == "manage" || granted == requested || (requested == "read" && granted == "use")
}

fn attachment_dependency_bindings<'a>(
    binding: &RuntimeResourceBindingV1,
    resources: &'a [RuntimeResourceBindingV1],
) -> Vec<&'a RuntimeResourceBindingV1> {
    let mut dependencies = Vec::new();
    match &binding.configuration {
        RuntimeResourceConfigurationV1::Mcp {
            server_id,
            server_version_id,
            transport,
            credential,
            ..
        } => {
            if let Some(server) = resources.iter().find(|candidate| {
                candidate.resource_id == *server_id
                    && candidate.resource_version == server_version_id.to_string()
                    && matches!(
                        &candidate.configuration,
                        RuntimeResourceConfigurationV1::Mcp { tool_name, .. }
                            if tool_name == "__server__"
                    )
            }) {
                dependencies.push(server);
            }
            if let Some(secret) = credential {
                if let Some(resource) = credential_binding(resources, secret) {
                    dependencies.push(resource);
                }
            }
            if let RuntimeMcpTransportV2::Stdio {
                environment_credential_refs,
                runtime_sandbox,
                ..
            } = transport
            {
                if let Some(sandbox) = resources.iter().find(|candidate| {
                    candidate.resource_id == runtime_sandbox.resource_id
                        && candidate.resource_version
                            == runtime_sandbox.resource_version_id.to_string()
                        && matches!(
                            candidate.configuration,
                            RuntimeResourceConfigurationV1::SandboxProfile { .. }
                        )
                }) {
                    dependencies.push(sandbox);
                }
                for reference in environment_credential_refs {
                    if let Some(resource) = credential_binding(resources, &reference.credential) {
                        dependencies.push(resource);
                    }
                }
            }
        }
        RuntimeResourceConfigurationV1::Rag { credential, .. }
        | RuntimeResourceConfigurationV1::Memory { credential, .. } => {
            if let Some(secret) = credential {
                if let Some(resource) = credential_binding(resources, secret) {
                    dependencies.push(resource);
                }
            }
        }
        _ => {}
    }
    dependencies.sort_by_key(|resource| (runtime_resource_type(resource), resource.resource_id));
    dependencies.dedup_by_key(|resource| resource.resource_id);
    dependencies
}

fn credential_binding<'a>(
    resources: &'a [RuntimeResourceBindingV1],
    secret: &crate::VaultSecretReferenceV1,
) -> Option<&'a RuntimeResourceBindingV1> {
    resources.iter().find(|candidate| {
        matches!(
            &candidate.configuration,
            RuntimeResourceConfigurationV1::Credential { secret: candidate_secret, .. }
                if serde_json::to_value(candidate_secret).ok() == serde_json::to_value(secret).ok()
        )
    })
}

fn runtime_resource_type(binding: &RuntimeResourceBindingV1) -> &'static str {
    match &binding.configuration {
        RuntimeResourceConfigurationV1::Model { .. } => "model",
        RuntimeResourceConfigurationV1::Mcp { tool_name, .. } if tool_name == "__server__" => {
            "mcp_server"
        }
        RuntimeResourceConfigurationV1::Mcp { .. } => "mcp_tool",
        RuntimeResourceConfigurationV1::Rag { .. } => "rag",
        RuntimeResourceConfigurationV1::Memory { .. } => "memory",
        RuntimeResourceConfigurationV1::Skill { .. } => "skill",
        RuntimeResourceConfigurationV1::Credential { .. } => "credential",
        RuntimeResourceConfigurationV1::SandboxProfile { .. } => "sandbox_profile",
        RuntimeResourceConfigurationV1::Composite { .. } => "workflow",
    }
}

fn valid_agent_tool_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
        && name.len() <= 64
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentResourceClosureEntryV2 {
    pub resource_type: agentx_domain::ResourceType,
    pub resource_id: Uuid,
    /// Exact UUID for immutable versions; `None` represents a fixed
    /// dependency such as a Vault-backed credential.
    pub resource_version_id: Option<Uuid>,
    pub operations: BTreeSet<agentx_domain::ResourceOperation>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeWorkPackageV1 {
    pub payload: RuntimeWorkPackagePayloadV1,
    pub content_hash: ContentHash,
    pub signature: Ed25519Signature,
}

impl ExecutionSpecBundleV2 {
    pub fn signed(
        payload: ExecutionSpecPayloadV2,
        key_id: impl Into<String>,
        key: &SigningKey,
    ) -> Result<Self, crate::ContractError> {
        validate_bundle_payload(&payload)?;
        let content_hash = crate::content_hash(&payload)?;
        let signature = crate::sign_canonical(key_id, key, &payload)?;
        Ok(Self {
            payload,
            content_hash,
            signature,
        })
    }

    pub fn verify(&self, key: &VerifyingKey) -> Result<(), crate::ContractError> {
        validate_bundle_payload(&self.payload)?;
        if crate::content_hash(&self.payload)? != self.content_hash {
            return Err(crate::ContractError::ContentHashMismatch);
        }
        crate::verify_canonical(key, &self.payload, &self.signature)
    }
}

impl RuntimeWorkPackageV1 {
    pub fn signed(
        payload: RuntimeWorkPackagePayloadV1,
        key_id: impl Into<String>,
        key: &SigningKey,
    ) -> Result<Self, crate::ContractError> {
        validate_work_package_payload(&payload)?;
        let content_hash = crate::content_hash(&payload)?;
        let signature = crate::sign_canonical(key_id, key, &payload)?;
        Ok(Self {
            payload,
            content_hash,
            signature,
        })
    }

    pub fn verify(&self, key: &VerifyingKey) -> Result<(), crate::ContractError> {
        validate_work_package_payload(&self.payload)?;
        if crate::content_hash(&self.payload)? != self.content_hash {
            return Err(crate::ContractError::ContentHashMismatch);
        }
        crate::verify_canonical(key, &self.payload, &self.signature)
    }
}

fn validate_bundle_payload(payload: &ExecutionSpecPayloadV2) -> Result<(), crate::ContractError> {
    payload
        .runtime_policy
        .validate()
        .map_err(crate::ContractError::InvalidRuntimePolicy)?;
    if payload.authorization.tenant_id != payload.tenant_id
        || payload.authorization.workflow_id != payload.workflow_id
        || payload.workflow_name.trim().is_empty()
        || payload.workflow_version_number == 0
        || payload
            .objects
            .iter()
            .any(|object| object.tenant_id != payload.tenant_id || !object.has_canonical_key())
        || !valid_resource_bindings(&payload.resources, &payload.objects)
        || !valid_plugin_artifacts(&payload.node_manifests, &payload.objects)
    {
        return Err(crate::ContractError::InvalidImmutableReference);
    }
    Ok(())
}

fn validate_work_package_payload(
    payload: &RuntimeWorkPackagePayloadV1,
) -> Result<(), crate::ContractError> {
    let maximum_ttl = match payload.purpose {
        WorkPackagePurpose::Debug => time::Duration::hours(1),
        WorkPackagePurpose::Evaluation => time::Duration::hours(24),
    };
    if payload.expires_at <= payload.created_at
        || payload.expires_at - payload.created_at > maximum_ttl
    {
        return Err(crate::ContractError::InvalidWorkPackageTtl);
    }
    let spec_valid = match (&payload.purpose, &payload.call_purpose, &payload.spec) {
        (
            WorkPackagePurpose::Debug,
            RuntimeCallPurposeV1::Debug,
            crate::RuntimeWorkPackageSpecV1::Debug {
                draft_revision,
                debug_plan,
            },
        ) => *draft_revision > 0 && valid_debug_plan(payload, debug_plan),
        (
            WorkPackagePurpose::Evaluation,
            RuntimeCallPurposeV1::Evaluation,
            crate::RuntimeWorkPackageSpecV1::Evaluation {
                cases, evaluators, ..
            },
        ) => {
            let case_ids = cases
                .iter()
                .map(|case| case.case_id)
                .collect::<std::collections::BTreeSet<_>>();
            let evaluator_ids = evaluators
                .iter()
                .map(|evaluator| match evaluator {
                    crate::RuntimeEvaluatorV1::DeterministicRule { evaluator_id, .. }
                    | crate::RuntimeEvaluatorV1::Model { evaluator_id, .. } => *evaluator_id,
                })
                .collect::<std::collections::BTreeSet<_>>();
            let resource_ids = payload
                .resources
                .iter()
                .map(|resource| (resource.resource_kind, resource.resource_id))
                .collect::<std::collections::BTreeSet<_>>();
            let object_ids = payload
                .objects
                .iter()
                .map(|object| object.object_id)
                .collect::<std::collections::BTreeSet<_>>();
            let model_evaluators = evaluators
                .iter()
                .filter_map(|evaluator| match evaluator {
                    crate::RuntimeEvaluatorV1::Model {
                        evaluator_id,
                        resource_id,
                        prompt_object_id,
                    } => Some((*evaluator_id, *resource_id, *prompt_object_id)),
                    crate::RuntimeEvaluatorV1::DeterministicRule { .. } => None,
                })
                .collect::<std::collections::BTreeSet<_>>();
            let evaluator_executions = payload
                .model_evaluator_executions
                .iter()
                .map(|execution| {
                    (
                        execution.evaluator_id,
                        execution.resource_id,
                        execution.prompt_object_id,
                    )
                })
                .collect::<std::collections::BTreeSet<_>>();
            let evaluators_resolve = evaluators.iter().all(|evaluator| match evaluator {
                crate::RuntimeEvaluatorV1::DeterministicRule { expression, .. } => {
                    !expression.trim().is_empty()
                }
                crate::RuntimeEvaluatorV1::Model {
                    resource_id,
                    prompt_object_id,
                    ..
                } => {
                    resource_ids.contains(&(RuntimeResourceKindV1::Model, *resource_id))
                        && object_ids.contains(prompt_object_id)
                }
            });
            !cases.is_empty()
                && case_ids.len() == cases.len()
                && !evaluators.is_empty()
                && evaluator_ids.len() == evaluators.len()
                && evaluators_resolve
                && model_evaluators == evaluator_executions
                && payload.model_evaluator_executions.len() == evaluator_executions.len()
        }
        _ => false,
    };
    if !spec_valid {
        return Err(crate::ContractError::InvalidImmutableReference);
    }
    if !matches!(
        payload.spec,
        crate::RuntimeWorkPackageSpecV1::Evaluation { .. }
    ) && !payload.model_evaluator_executions.is_empty()
    {
        return Err(crate::ContractError::InvalidImmutableReference);
    }
    payload
        .runtime_policy
        .validate()
        .map_err(crate::ContractError::InvalidRuntimePolicy)?;
    if payload.authorization.tenant_id != payload.tenant_id
        || payload.authorization.workflow_id != payload.workflow.id
        || payload.workflow.name.trim().is_empty()
        || payload.workflow.version_number == 0
        || payload
            .objects
            .iter()
            .any(|object| object.tenant_id != payload.tenant_id || !object.has_canonical_key())
        || !valid_resource_bindings(&payload.resources, &payload.objects)
        || !valid_plugin_artifacts(&payload.node_manifests, &payload.objects)
    {
        return Err(crate::ContractError::InvalidImmutableReference);
    }
    Ok(())
}

fn valid_debug_plan(
    payload: &RuntimeWorkPackagePayloadV1,
    plan: &crate::RuntimeDebugPlanV1,
) -> bool {
    use crate::{PartialExecutionModeV1, RuntimeDebugInputSourceV1};

    let all = payload
        .compiled_ir
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let included = plan
        .included_node_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let skipped = plan
        .skipped_node_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    if included.len() != plan.included_node_ids.len()
        || skipped.len() != plan.skipped_node_ids.len()
        || !included.is_disjoint(&skipped)
        || included
            .union(&skipped)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            != all
        || plan
            .side_effect_decisions
            .keys()
            .any(|node_id| !included.contains(node_id))
    {
        return false;
    }

    let expected = match (plan.mode, plan.target_node_id.as_deref()) {
        (PartialExecutionModeV1::Whole, None) => all.clone(),
        (PartialExecutionModeV1::Whole, Some(_)) | (_, None) => return false,
        (mode, Some(target)) => {
            let Some(selected) = payload
                .compiled_ir
                .nodes
                .iter()
                .position(|node| node.id == target)
            else {
                return false;
            };
            let mut indexes = std::collections::BTreeSet::from([selected]);
            let mut frontier = std::collections::VecDeque::from([selected]);
            while let Some(node) = frontier.pop_front() {
                let connections = match mode {
                    PartialExecutionModeV1::Node => continue,
                    PartialExecutionModeV1::ToNode => {
                        &payload.compiled_ir.nodes[node].incoming_connections
                    }
                    PartialExecutionModeV1::FromNode => {
                        &payload.compiled_ir.nodes[node].outgoing_connections
                    }
                    PartialExecutionModeV1::Whole => unreachable!(),
                };
                for connection in connections {
                    let connection = &payload.compiled_ir.connections[*connection];
                    let next = if mode == PartialExecutionModeV1::ToNode {
                        connection.source_node
                    } else {
                        connection.target_node
                    };
                    if indexes.insert(next) {
                        frontier.push_back(next);
                    }
                }
            }
            indexes
                .into_iter()
                .map(|index| payload.compiled_ir.nodes[index].id.clone())
                .collect()
        }
    };
    if included != expected {
        return false;
    }

    let input_source_valid = match (&plan.mode, &plan.input_source) {
        (PartialExecutionModeV1::Node | PartialExecutionModeV1::FromNode, Some(source)) => {
            match source {
                RuntimeDebugInputSourceV1::Manual { value } => value == &payload.overlay.input,
                RuntimeDebugInputSourceV1::HistoryOutput { output_port, .. } => {
                    !output_port.trim().is_empty()
                }
                RuntimeDebugInputSourceV1::Artifact { .. } => true,
            }
        }
        (PartialExecutionModeV1::Whole | PartialExecutionModeV1::ToNode, None) => true,
        _ => false,
    };
    input_source_valid
        && payload.compiled_ir.nodes.iter().all(|node| {
            node.side_effect_level != agentx_node_protocol::SideEffectLevel::Irreversible
                || !included.contains(&node.id)
                || plan.side_effect_decisions.contains_key(&node.id)
        })
}

fn valid_resource_bindings(
    resources: &[RuntimeResourceBindingV1],
    objects: &[RuntimeObjectReferenceV1],
) -> bool {
    let available_objects = objects
        .iter()
        .map(|object| object.object_id)
        .collect::<std::collections::BTreeSet<_>>();
    let mut identities = std::collections::BTreeSet::new();
    resources.iter().all(|resource| {
        identities.insert((
            resource.resource_kind,
            resource.resource_id,
            resource.resource_version.clone(),
        )) && resource
            .object_ids
            .iter()
            .all(|object_id| available_objects.contains(object_id))
            && configuration_kind(&resource.configuration) == resource.resource_kind
            && valid_configuration(resource)
    })
}

fn valid_plugin_artifacts(manifests: &[Value], objects: &[RuntimeObjectReferenceV1]) -> bool {
    manifests.iter().all(|value| {
        let Ok(manifest) =
            serde_json::from_value::<agentx_node_protocol::NodeManifestVersion>(value.clone())
        else {
            return false;
        };
        let Some(plugin) = manifest.plugin else {
            return true;
        };
        if plugin.package_id.starts_with("agentx/") {
            return true;
        }
        let Some(artifact) = plugin.runtime_artifact else {
            return plugin.runtime_entry == "native";
        };
        objects.iter().any(|object| {
            object.object_id == artifact.object_id
                && object.content_hash.as_str() == artifact.content_hash
                && object.size_bytes == artifact.size_bytes
                && object.media_type == artifact.media_type
        })
    })
}

fn valid_configuration(resource: &RuntimeResourceBindingV1) -> bool {
    match &resource.configuration {
        RuntimeResourceConfigurationV1::Composite {
            workflow,
            definition_object_id,
            ir_object_id,
        } => {
            resource.resource_id == workflow.version_id
                && resource.resource_version == workflow.version_id.to_string()
                && workflow.version_number > 0
                && !workflow.name.trim().is_empty()
                && definition_object_id == &workflow.version_id
                && definition_object_id != ir_object_id
        }
        _ => true,
    }
}

const fn configuration_kind(
    configuration: &RuntimeResourceConfigurationV1,
) -> RuntimeResourceKindV1 {
    match configuration {
        RuntimeResourceConfigurationV1::Model { .. } => RuntimeResourceKindV1::Model,
        RuntimeResourceConfigurationV1::Mcp { .. } => RuntimeResourceKindV1::Mcp,
        RuntimeResourceConfigurationV1::Rag { .. } => RuntimeResourceKindV1::Rag,
        RuntimeResourceConfigurationV1::Memory { .. } => RuntimeResourceKindV1::Memory,
        RuntimeResourceConfigurationV1::Skill { .. } => RuntimeResourceKindV1::Skill,
        RuntimeResourceConfigurationV1::Credential { .. } => RuntimeResourceKindV1::Credential,
        RuntimeResourceConfigurationV1::SandboxProfile { .. } => {
            RuntimeResourceKindV1::SandboxProfile
        }
        RuntimeResourceConfigurationV1::Composite { .. } => RuntimeResourceKindV1::Composite,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleLifecycleState {
    Building,
    Prepared,
    Active,
    Superseded,
    Disabled,
    Retained,
    GarbageCollectable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleReferenceKind {
    ActiveExecution,
    CheckpointForkSource,
    PinnedSession,
    PendingWait,
    RetentionHold,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleReferenceV1 {
    pub bundle_id: Uuid,
    pub kind: BundleReferenceKind,
    pub owner_id: Uuid,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub retained_until: Option<OffsetDateTime>,
}

#[cfg(test)]
mod attachment_registry_tests {
    use super::*;
    use agentx_domain::{ResourceOperation, ResourceType};

    fn hash() -> ContentHash {
        ContentHash::parse(format!("sha256:{}", "a".repeat(64))).expect("hash")
    }

    fn configuration(
        attachments: Vec<crate::CompiledAgentAttachmentV2>,
    ) -> crate::CompiledAgentNodeV2 {
        crate::CompiledAgentNodeV2 {
            contract_version: AGENT_CORE_CONTRACT_VERSION.into(),
            session_policy: crate::AgentSessionPolicyModeV2::Invocation,
            model: crate::CompiledAgentResourceReferenceV2 {
                binding_role: "model".into(),
                resource_type: ResourceType::Model,
                resource_id: Uuid::from_u128(10),
                resource_version_id: Uuid::from_u128(11),
                operation: ResourceOperation::Use,
            },
            workspace_sandbox: None,
            attachments,
            core_tools: vec![],
        }
    }

    fn attachment(
        resource_type: ResourceType,
        resource_id: Uuid,
        resource_version_id: Uuid,
    ) -> crate::CompiledAgentAttachmentV2 {
        crate::CompiledAgentAttachmentV2 {
            binding_role: resource_type.as_str().into(),
            resource_type,
            resource_id,
            resource_version_id,
            operation: ResourceOperation::Use,
        }
    }

    fn mcp_binding(
        resource_id: Uuid,
        resource_version_id: Uuid,
        tool_name: String,
        input_schema: Value,
    ) -> RuntimeResourceBindingV1 {
        RuntimeResourceBindingV1 {
            resource_kind: RuntimeResourceKindV1::Mcp,
            resource_id,
            resource_version: resource_version_id.to_string(),
            state_epoch: 1,
            content_hash: hash(),
            configuration: RuntimeResourceConfigurationV1::Mcp {
                server_id: Uuid::from_u128(1000 + resource_id.as_u128()),
                server_version_id: Uuid::from_u128(2000 + resource_id.as_u128()),
                transport: RuntimeMcpTransportV2::StreamableHttp {
                    endpoint: "https://mcp.example/rpc".into(),
                },
                tool_name,
                tool_version: resource_version_id.to_string(),
                input_schema_hash: hash(),
                input_schema,
                output_schema: None,
                side_effect: "none".into(),
                timeout_seconds: 30,
                credential: None,
            },
            object_ids: vec![],
        }
    }

    fn skill_binding(
        resource_id: Uuid,
        resource_version_id: Uuid,
        dependencies: Vec<crate::RuntimeSkillDependencyV2>,
    ) -> RuntimeResourceBindingV1 {
        let object_id = Uuid::from_u128(5000 + resource_id.as_u128());
        RuntimeResourceBindingV1 {
            resource_kind: RuntimeResourceKindV1::Skill,
            resource_id,
            resource_version: resource_version_id.to_string(),
            state_epoch: 1,
            content_hash: hash(),
            configuration: RuntimeResourceConfigurationV1::Skill {
                entrypoint_object_id: object_id,
                entrypoint_content_hash: hash(),
                dependency_object_ids: vec![],
                dependencies,
            },
            object_ids: vec![object_id],
        }
    }

    fn grants(resources: &[RuntimeResourceBindingV1]) -> Vec<crate::RuntimeGrantBindingV1> {
        resources
            .iter()
            .enumerate()
            .map(|(index, resource)| crate::RuntimeGrantBindingV1 {
                grant_id: Uuid::from_u128(90_000 + index as u128),
                resource_type: runtime_resource_type(resource).into(),
                resource_id: resource.resource_id,
                resource_version_id: Uuid::parse_str(&resource.resource_version).ok(),
                operation: "use".into(),
            })
            .collect()
    }

    #[test]
    fn recursively_expands_skill_dependencies_into_contexts_tools_and_evidence() {
        let skill_id = Uuid::from_u128(1);
        let skill_version = Uuid::from_u128(2);
        let rag_id = Uuid::from_u128(3);
        let rag_version = Uuid::from_u128(4);
        let program_object = Uuid::from_u128(5);
        let configuration = crate::CompiledAgentNodeV2 {
            contract_version: AGENT_CORE_CONTRACT_VERSION.into(),
            session_policy: crate::AgentSessionPolicyModeV2::Invocation,
            model: crate::CompiledAgentResourceReferenceV2 {
                binding_role: "model".into(),
                resource_type: ResourceType::Model,
                resource_id: Uuid::from_u128(10),
                resource_version_id: Uuid::from_u128(11),
                operation: ResourceOperation::Use,
            },
            workspace_sandbox: None,
            attachments: vec![crate::CompiledAgentAttachmentV2 {
                binding_role: "skill".into(),
                resource_type: ResourceType::Skill,
                resource_id: skill_id,
                resource_version_id: skill_version,
                operation: ResourceOperation::Use,
            }],
            core_tools: vec![],
        };
        let resources = vec![
            RuntimeResourceBindingV1 {
                resource_kind: RuntimeResourceKindV1::Skill,
                resource_id: skill_id,
                resource_version: skill_version.to_string(),
                state_epoch: 1,
                content_hash: hash(),
                configuration: RuntimeResourceConfigurationV1::Skill {
                    entrypoint_object_id: program_object,
                    entrypoint_content_hash: hash(),
                    dependency_object_ids: vec![],
                    dependencies: vec![crate::RuntimeSkillDependencyV2 {
                        resource_type: "rag".into(),
                        resource_id: rag_id,
                        resource_version_id: Some(rag_version),
                        operation: "use".into(),
                    }],
                },
                object_ids: vec![program_object],
            },
            RuntimeResourceBindingV1 {
                resource_kind: RuntimeResourceKindV1::Rag,
                resource_id: rag_id,
                resource_version: rag_version.to_string(),
                state_epoch: 1,
                content_hash: hash(),
                configuration: RuntimeResourceConfigurationV1::Rag {
                    provider: "lightrag".into(),
                    endpoint: "https://rag.example/search".into(),
                    namespace: "knowledge".into(),
                    index_version: rag_version.to_string(),
                    credential: None,
                },
                object_ids: vec![],
            },
        ];
        let grant_bindings = grants(&resources);
        let registry = derive_attachment_registry(&configuration, &resources, 7, &grant_bindings)
            .expect("registry");
        assert_eq!(registry.contexts.len(), 1);
        assert_eq!(registry.contexts[0].content_hash, hash());
        assert!(registry.tools.iter().any(|tool| {
            tool.name == format!("knowledge_search_{}", rag_id.simple())
                && tool.resource_version_id == rag_version
        }));
        assert!(registry.authorization_evidence.iter().any(|evidence| {
            evidence.resource_id == rag_id
                && evidence.resource_version_id == Some(rag_version)
                && evidence.policy_epoch == 7
                && evidence.grant_ids == vec![grant_bindings[1].grant_id]
        }));
    }

    #[test]
    fn authorization_evidence_uses_only_the_exact_resource_grant() {
        let first_id = Uuid::from_u128(21);
        let first_version = Uuid::from_u128(22);
        let second_id = Uuid::from_u128(23);
        let second_version = Uuid::from_u128(24);
        let resources = vec![
            mcp_binding(
                first_id,
                first_version,
                "first_tool".into(),
                serde_json::json!({"type":"object"}),
            ),
            mcp_binding(
                second_id,
                second_version,
                "second_tool".into(),
                serde_json::json!({"type":"object"}),
            ),
        ];
        let grant_bindings = grants(&resources);
        let registry = derive_attachment_registry(
            &configuration(vec![
                attachment(ResourceType::McpTool, first_id, first_version),
                attachment(ResourceType::McpTool, second_id, second_version),
            ]),
            &resources,
            9,
            &grant_bindings,
        )
        .expect("registry");
        for (resource_id, grant_id) in [
            (first_id, grant_bindings[0].grant_id),
            (second_id, grant_bindings[1].grant_id),
        ] {
            let evidence = registry
                .authorization_evidence
                .iter()
                .find(|evidence| evidence.resource_id == resource_id)
                .expect("resource evidence");
            assert_eq!(evidence.grant_ids, vec![grant_id]);
        }

        let error = derive_attachment_registry(
            &configuration(vec![
                attachment(ResourceType::McpTool, first_id, first_version),
                attachment(ResourceType::McpTool, second_id, second_version),
            ]),
            &resources,
            9,
            &grant_bindings[..1],
        )
        .expect_err("an unrelated resource grant cannot authorize the second tool");
        assert!(error.contains("AGENT_ATTACHMENT_GRANT_MISSING"));
        assert!(error.contains(&second_id.to_string()));
    }

    #[test]
    fn fixed_credential_dependencies_keep_unversioned_authorization_evidence() {
        let tool_id = Uuid::from_u128(41);
        let tool_version = Uuid::from_u128(42);
        let credential_id = Uuid::from_u128(43);
        let secret = crate::VaultSecretReferenceV1 {
            mount: "runtime".into(),
            path: "mcp/echo".into(),
            key: "token".into(),
            version: 7,
        };
        let mut tool = mcp_binding(
            tool_id,
            tool_version,
            "credential_tool".into(),
            serde_json::json!({"type":"object"}),
        );
        if let RuntimeResourceConfigurationV1::Mcp { credential, .. } = &mut tool.configuration {
            *credential = Some(secret.clone());
        }
        let credential = RuntimeResourceBindingV1 {
            resource_kind: RuntimeResourceKindV1::Credential,
            resource_id: credential_id,
            resource_version: "fixed".into(),
            state_epoch: 1,
            content_hash: hash(),
            configuration: RuntimeResourceConfigurationV1::Credential {
                credential_type: "bearer".into(),
                secret,
                allowed_operations: BTreeSet::from(["use".into()]),
            },
            object_ids: vec![],
        };
        let resources = vec![tool, credential];
        let registry = derive_attachment_registry(
            &configuration(vec![attachment(
                ResourceType::McpTool,
                tool_id,
                tool_version,
            )]),
            &resources,
            11,
            &grants(&resources),
        )
        .expect("fixed credential dependency is valid");
        let evidence = registry
            .authorization_evidence
            .iter()
            .find(|item| item.resource_id == credential_id)
            .expect("credential evidence");
        assert_eq!(evidence.resource_version_id, None);
        assert_eq!(evidence.binding_version(), "fixed");
    }

    #[test]
    fn rejects_duplicate_bindings_and_core_tool_name_conflicts() {
        let resource_id = Uuid::from_u128(30);
        let version_id = Uuid::from_u128(31);
        let item = attachment(ResourceType::McpTool, resource_id, version_id);
        let duplicate =
            derive_attachment_registry(&configuration(vec![item.clone(), item]), &[], 1, &[])
                .expect_err("duplicate binding must fail");
        assert!(duplicate.starts_with("AGENT_ATTACHMENT_BINDING_DUPLICATE"));

        let mut conflict_configuration = configuration(vec![attachment(
            ResourceType::McpTool,
            resource_id,
            version_id,
        )]);
        conflict_configuration
            .core_tools
            .push(crate::DerivedCoreToolV2 {
                name: "read".into(),
                replay_policy: crate::CoreToolReplayPolicyV2::Safe,
                workspace_sandbox_resource_id: Uuid::from_u128(33),
                workspace_sandbox_version_id: Uuid::from_u128(34),
            });
        let conflict_resources = vec![mcp_binding(
            resource_id,
            version_id,
            "read".into(),
            serde_json::json!({"type":"object"}),
        )];
        let conflict = derive_attachment_registry(
            &conflict_configuration,
            &conflict_resources,
            1,
            &grants(&conflict_resources),
        )
        .expect_err("core and attachment names must not collide");
        assert_eq!(conflict, "AGENT_TOOL_NAME_CONFLICT: read");
    }

    #[test]
    fn enforces_tool_schema_and_external_context_limits() {
        let mut attachments = Vec::new();
        let mut resources = Vec::new();
        for index in 0..129_u128 {
            let resource_id = Uuid::from_u128(10_000 + index);
            let version_id = Uuid::from_u128(20_000 + index);
            attachments.push(attachment(ResourceType::McpTool, resource_id, version_id));
            resources.push(mcp_binding(
                resource_id,
                version_id,
                format!("tool_{index}"),
                serde_json::json!({"type":"object"}),
            ));
        }
        let tool_limit = derive_attachment_registry(
            &configuration(attachments),
            &resources,
            1,
            &grants(&resources),
        )
        .expect_err("more than 128 tools must fail");
        assert!(tool_limit.starts_with("AGENT_TOOL_REGISTRY_LIMIT_EXCEEDED"));

        let resource_id = Uuid::from_u128(41);
        let version_id = Uuid::from_u128(42);
        let schema_resources = vec![mcp_binding(
            resource_id,
            version_id,
            "huge_schema".into(),
            serde_json::json!({"type":"object","description":"x".repeat(1024 * 1024)}),
        )];
        let schema_limit = derive_attachment_registry(
            &configuration(vec![attachment(
                ResourceType::McpTool,
                resource_id,
                version_id,
            )]),
            &schema_resources,
            1,
            &grants(&schema_resources),
        )
        .expect_err("schemas larger than one MiB must fail");
        assert!(schema_limit.starts_with("AGENT_TOOL_SCHEMA_LIMIT_EXCEEDED"));

        let mut skill_attachments = Vec::new();
        let mut skills = Vec::new();
        for index in 0..65_u128 {
            let skill_id = Uuid::from_u128(30_000 + index);
            let skill_version = Uuid::from_u128(40_000 + index);
            skill_attachments.push(attachment(ResourceType::Skill, skill_id, skill_version));
            skills.push(skill_binding(skill_id, skill_version, vec![]));
        }
        let context_limit = derive_attachment_registry(
            &configuration(skill_attachments),
            &skills,
            1,
            &grants(&skills),
        )
        .expect_err("more than 64 contexts must fail");
        assert!(context_limit.starts_with("AGENT_EXTERNAL_CONTEXT_LIMIT_EXCEEDED"));
    }

    #[test]
    fn rejects_recursive_skill_dependency_cycles() {
        let first_id = Uuid::from_u128(50);
        let first_version = Uuid::from_u128(51);
        let second_id = Uuid::from_u128(52);
        let second_version = Uuid::from_u128(53);
        let dependency = |resource_id, resource_version_id| crate::RuntimeSkillDependencyV2 {
            resource_type: "skill".into(),
            resource_id,
            resource_version_id: Some(resource_version_id),
            operation: "use".into(),
        };
        let error = derive_attachment_registry(
            &configuration(vec![attachment(
                ResourceType::Skill,
                first_id,
                first_version,
            )]),
            &[
                skill_binding(
                    first_id,
                    first_version,
                    vec![dependency(second_id, second_version)],
                ),
                skill_binding(
                    second_id,
                    second_version,
                    vec![dependency(first_id, first_version)],
                ),
            ],
            1,
            &[],
        )
        .expect_err("skill dependency cycle must fail");
        assert!(error.starts_with("AGENT_SKILL_DEPENDENCY_CYCLE"));
    }
}
