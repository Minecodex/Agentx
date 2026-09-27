use std::collections::{BTreeMap, BTreeSet};

use agentx_domain::WorkflowDefinition;
use agentx_node_protocol::{NodeCapability, NodeManifestVersion};
use agentx_runtime::{COMPILER_VERSION, CompileContext, NodeRegistry, WorkflowCompiler};
use agentx_runtime_contracts::{
    AgentBundleContractV2, AgentResourceClosureEntryV2, BUNDLE_SCHEMA_VERSION,
    CompiledAgentBundleEntryV2, DependencyClosureEntryV1, DependencyClosureV1, DependencyKindV1,
    ExecutionOriginV1, ExecutionSpecBundleV2, ExecutionSpecPayloadV2,
    RuntimeAuthorizationSnapshotV1, RuntimeCallPurposeV1, RuntimeModelEvaluatorExecutionV1,
    RuntimeObjectReferenceV1, RuntimePolicyV1, RuntimeResourceBindingV1, RuntimeTriggerSpecV1,
    RuntimeWorkPackageOverlayV1, RuntimeWorkPackagePayloadV1, RuntimeWorkPackageV1,
    WORK_PACKAGE_SCHEMA_VERSION, WorkPackagePurpose, WorkerCompatibilityV1,
};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

pub type ResolvedPluginManifestDependencies = BTreeMap<Uuid, BTreeMap<String, NodeManifestVersion>>;

#[derive(Clone)]
pub struct BundleBuildSource {
    pub bundle_id: Uuid,
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub deployment_id: Uuid,
    pub workflow_id: Uuid,
    pub workflow_version_id: Uuid,
    pub workflow_name: String,
    pub workflow_version_number: u64,
    pub workflow_owner_department: Option<agentx_runtime_contracts::ExecutionDepartmentSnapshotV1>,
    pub sequence: u64,
    pub definition: WorkflowDefinition,
    pub dependency_versions: BTreeMap<Uuid, WorkflowDefinition>,
    pub plugin_manifests: Vec<NodeManifestVersion>,
    pub supported_capabilities: BTreeSet<String>,
    pub input_contract: Value,
    pub output_contract: Value,
    pub resources: Vec<RuntimeResourceBindingV1>,
    pub authorization: RuntimeAuthorizationSnapshotV1,
    pub triggers: Vec<RuntimeTriggerSpecV1>,
    pub runtime_policy: RuntimePolicyV1,
    pub objects: Vec<RuntimeObjectReferenceV1>,
    pub created_at: OffsetDateTime,
}

#[derive(Clone)]
pub struct WorkPackageBuildSource {
    pub package_id: Uuid,
    pub tenant_id: Uuid,
    pub workflow: agentx_runtime_contracts::ExecutionWorkflowSnapshotV1,
    pub origin: ExecutionOriginV1,
    pub purpose: WorkPackagePurpose,
    pub call_purpose: RuntimeCallPurposeV1,
    pub spec: agentx_runtime_contracts::RuntimeWorkPackageSpecV1,
    pub source_revision: String,
    pub definition: WorkflowDefinition,
    pub dependency_versions: BTreeMap<Uuid, WorkflowDefinition>,
    pub plugin_manifests: Vec<NodeManifestVersion>,
    pub supported_capabilities: BTreeSet<String>,
    pub overlay: RuntimeWorkPackageOverlayV1,
    pub resources: Vec<RuntimeResourceBindingV1>,
    pub authorization: RuntimeAuthorizationSnapshotV1,
    pub objects: Vec<RuntimeObjectReferenceV1>,
    pub runtime_policy: RuntimePolicyV1,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("Workflow compilation failed: {0}")]
    Compilation(String),
    #[error("fixed sub-workflow version {0} is missing")]
    MissingDependency(Uuid),
    #[error("sub-workflow dependency cycle contains {0}")]
    DependencyCycle(Uuid),
    #[error("fixed sub-workflow version {0} has no immutable Runtime object")]
    MissingDependencyObject(Uuid),
    #[error("Runtime capability {0} is not supported by this publishing slice")]
    UnsupportedCapability(String),
    #[error("Runtime object uses a non-canonical key")]
    NonCanonicalObject,
    #[error("Bundle signing failed: {0}")]
    Signing(#[from] agentx_runtime_contracts::ContractError),
}

#[must_use]
pub fn composite_ir_object_id(workflow_version_id: Uuid) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(b"agentx-composite-ir-v1\0");
    digest.update(workflow_version_id.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

pub fn compile_workflow_version(
    definition: &WorkflowDefinition,
    workflow_version_id: Uuid,
) -> Result<agentx_runtime_contracts::CompiledWorkflowV1, BuildError> {
    compile_workflow_version_with_dependencies(definition, workflow_version_id, &BTreeMap::new())
}

pub fn compile_workflow_version_with_dependencies(
    definition: &WorkflowDefinition,
    workflow_version_id: Uuid,
    dependency_versions: &BTreeMap<Uuid, WorkflowDefinition>,
) -> Result<agentx_runtime_contracts::CompiledWorkflowV1, BuildError> {
    let registry = node_registry_with_composites(dependency_versions)?;
    WorkflowCompiler::new(&registry)
        .compile(
            definition,
            &CompileContext {
                current_workflow_version_id: Some(workflow_version_id.to_string()),
                ancestor_workflow_version_ids: BTreeSet::new(),
                resource_tool_names: BTreeMap::new(),
            },
        )
        .map(strip_runtime_sources)
        .map_err(|error| {
            BuildError::Compilation(
                error
                    .issues
                    .into_iter()
                    .map(|issue| issue.code)
                    .collect::<Vec<_>>()
                    .join(","),
            )
        })
}

fn resource_tool_names(resources: &[RuntimeResourceBindingV1]) -> BTreeMap<Uuid, String> {
    resources
        .iter()
        .filter_map(|resource| match &resource.configuration {
            agentx_runtime_contracts::RuntimeResourceConfigurationV1::Mcp { tool_name, .. }
                if tool_name != "__server__" =>
            {
                Some((resource.resource_id, tool_name.clone()))
            }
            _ => None,
        })
        .collect()
}

fn build_agent_bundle(
    compiled: &agentx_runtime_contracts::CompiledWorkflowV1,
    node_manifests: &[Value],
    authorization: &RuntimeAuthorizationSnapshotV1,
    resources: &[RuntimeResourceBindingV1],
) -> Result<AgentBundleContractV2, BuildError> {
    let mut contract =
        AgentBundleContractV2::empty(compiled.definition_hash.clone(), COMPILER_VERSION);
    for manifest in node_manifests {
        let node_type = manifest
            .get("nodeType")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let version = manifest.get("version").and_then(Value::as_u64).unwrap_or(0);
        contract.manifest_hashes.insert(
            format!("{node_type}:{version}"),
            agentx_runtime_contracts::content_hash(manifest)?,
        );
    }
    contract.policy_epoch = authorization.policy_epoch;
    contract.grant_ids = authorization.grant_ids.clone();
    contract.grant_ids.sort_unstable();
    contract.grant_ids.dedup();

    for node in &compiled.nodes {
        let Some(configuration) = node.agent.clone() else {
            continue;
        };
        let attachment_registry = agentx_runtime_contracts::derive_attachment_registry(
            &configuration,
            resources,
            authorization.policy_epoch,
            &authorization.grant_bindings,
        )
        .map_err(BuildError::Compilation)?;
        let mut resource_closure = Vec::<AgentResourceClosureEntryV2>::new();
        let mut add_reference = |resource_type, resource_id, resource_version_id, operation| {
            if let Some(existing) = resource_closure.iter_mut().find(|entry| {
                entry.resource_type == resource_type
                    && entry.resource_id == resource_id
                    && entry.resource_version_id == resource_version_id
            }) {
                existing.operations.insert(operation);
            } else {
                resource_closure.push(AgentResourceClosureEntryV2 {
                    resource_type,
                    resource_id,
                    resource_version_id,
                    operations: BTreeSet::from([operation]),
                });
            }
        };
        add_reference(
            configuration.model.resource_type,
            configuration.model.resource_id,
            Some(configuration.model.resource_version_id),
            configuration.model.operation,
        );
        if let Some(sandbox) = &configuration.workspace_sandbox {
            add_reference(
                sandbox.resource_type,
                sandbox.resource_id,
                Some(sandbox.resource_version_id),
                sandbox.operation,
            );
        }
        for attachment in &configuration.attachments {
            add_reference(
                attachment.resource_type,
                attachment.resource_id,
                Some(attachment.resource_version_id),
                attachment.operation,
            );
        }
        for evidence in &attachment_registry.authorization_evidence {
            add_reference(
                agent_resource_type(&evidence.resource_type)?,
                evidence.resource_id,
                evidence.resource_version_id,
                agent_resource_operation(&evidence.operation)?,
            );
        }
        resource_closure.sort_by_key(|entry| {
            (
                entry.resource_type,
                entry.resource_id,
                entry.resource_version_id,
            )
        });
        contract.agents.push(CompiledAgentBundleEntryV2 {
            node_id: node.id.clone(),
            configuration,
            resource_closure,
            attachment_registry,
        });
    }
    contract
        .agents
        .sort_by(|left, right| left.node_id.cmp(&right.node_id));
    Ok(contract)
}

fn agent_resource_type(value: &str) -> Result<agentx_domain::ResourceType, BuildError> {
    match value {
        "credential" => Ok(agentx_domain::ResourceType::Credential),
        "model" => Ok(agentx_domain::ResourceType::Model),
        "mcp_server" => Ok(agentx_domain::ResourceType::McpServer),
        "mcp_tool" => Ok(agentx_domain::ResourceType::McpTool),
        "skill" => Ok(agentx_domain::ResourceType::Skill),
        "rag" | "knowledge" => Ok(agentx_domain::ResourceType::Rag),
        "memory" => Ok(agentx_domain::ResourceType::Memory),
        "sandbox_profile" => Ok(agentx_domain::ResourceType::SandboxProfile),
        other => Err(BuildError::Compilation(format!(
            "AGENT_ATTACHMENT_RESOURCE_TYPE_INVALID: {other}"
        ))),
    }
}

fn agent_resource_operation(value: &str) -> Result<agentx_domain::ResourceOperation, BuildError> {
    match value {
        "view" => Ok(agentx_domain::ResourceOperation::View),
        "use" => Ok(agentx_domain::ResourceOperation::Use),
        "read" => Ok(agentx_domain::ResourceOperation::Read),
        "write" => Ok(agentx_domain::ResourceOperation::Write),
        "manage" => Ok(agentx_domain::ResourceOperation::Manage),
        other => Err(BuildError::Compilation(format!(
            "AGENT_ATTACHMENT_OPERATION_INVALID: {other}"
        ))),
    }
}

/// Builds the immutable node registry used by Control validation, Bundle/Work Package
/// compilation and Runtime composite snapshots. Each fixed Workflow Version gets a
/// version-addressed Manifest whose input, output and mutable Context contract comes
/// from that version's immutable Definition.
pub fn node_registry_with_composites(
    dependency_versions: &BTreeMap<Uuid, WorkflowDefinition>,
) -> Result<NodeRegistry, BuildError> {
    let mut registry = NodeRegistry::m5_defaults();
    let base = registry
        .get("sub_workflow", 1)
        .expect("the built-in composite manifest exists")
        .clone();
    for (version_id, definition) in dependency_versions {
        let mut manifest = base.clone();
        manifest.node_type = format!("workflow.{}", version_id.simple());
        manifest.display_name = format!("Workflow Version {version_id}");
        manifest.description = "Immutable composite Workflow Version".into();
        manifest.parameter_schema = json!({
            "type": "object",
            "x-agentx-contextContract": definition.start.contexts,
            "required": ["workflowVersionId", "inputs"],
            "properties": {
                "workflowVersionId": {
                    "type": "string",
                    "format": "uuid",
                    "const": version_id.to_string()
                },
                "inputs": {
                    "allOf": [definition.start.inputs],
                    "x-agentx-binding": {
                        "acceptedKinds": ["literal", "reference", "template", "array", "object"],
                        "allowedNamespaces": ["inputs", "outputs", "contexts", "execution"],
                        "acceptedCardinality": ["single"],
                        "missingPolicies": ["error", "null", "omit"],
                        "recursive": true
                    }
                }
            },
            "additionalProperties": false
        });
        let all_complete =
            definition.end.completion == agentx_domain::WorkflowCompletion::AllComplete;
        let required = definition
            .end
            .outputs
            .iter()
            .filter(|(_, output)| all_complete || output.required)
            .map(|(name, _)| Value::String(name.clone()))
            .collect::<Vec<_>>();
        let properties = definition
            .end
            .outputs
            .iter()
            .map(|(name, output)| {
                let schema = if all_complete {
                    let item_schema = if output.required {
                        output.schema.clone()
                    } else {
                        json!({"anyOf":[output.schema.clone(),{"type":"null"}]})
                    };
                    json!({"type":"array","items":item_schema})
                } else {
                    output.schema.clone()
                };
                (name.clone(), schema)
            })
            .collect::<serde_json::Map<_, _>>();
        manifest.output_schema = json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        });
        registry.register(manifest).map_err(|error| {
            BuildError::Compilation(format!("INVALID_COMPOSITE_MANIFEST:{error}"))
        })?;
    }
    Ok(registry)
}

pub fn compile_workflow_version_with_plugins(
    definition: &WorkflowDefinition,
    workflow_version_id: Uuid,
    dependency_versions: &BTreeMap<Uuid, WorkflowDefinition>,
    plugin_manifests: &[NodeManifestVersion],
) -> Result<agentx_runtime_contracts::CompiledWorkflowV1, BuildError> {
    compile_workflow_version_with_resolved_plugins(
        definition,
        workflow_version_id,
        dependency_versions,
        plugin_manifests,
        &BTreeMap::new(),
    )
}

pub fn compile_workflow_version_with_resolved_plugins(
    definition: &WorkflowDefinition,
    workflow_version_id: Uuid,
    dependency_versions: &BTreeMap<Uuid, WorkflowDefinition>,
    plugin_manifests: &[NodeManifestVersion],
    resolved_manifests: &BTreeMap<String, NodeManifestVersion>,
) -> Result<agentx_runtime_contracts::CompiledWorkflowV1, BuildError> {
    let registry = node_registry_with_plugins(dependency_versions, plugin_manifests)?;
    WorkflowCompiler::new(&registry)
        .with_resolved_manifests(resolved_manifests)
        .compile(
            definition,
            &CompileContext {
                current_workflow_version_id: Some(workflow_version_id.to_string()),
                ancestor_workflow_version_ids: BTreeSet::new(),
                resource_tool_names: BTreeMap::new(),
            },
        )
        .map(strip_runtime_sources)
        .map_err(|error| {
            BuildError::Compilation(
                error
                    .issues
                    .into_iter()
                    .map(|issue| issue.code)
                    .collect::<Vec<_>>()
                    .join(","),
            )
        })
}

pub fn node_registry_with_plugins(
    dependency_versions: &BTreeMap<Uuid, WorkflowDefinition>,
    plugin_manifests: &[NodeManifestVersion],
) -> Result<NodeRegistry, BuildError> {
    let mut registry = node_registry_with_composites(dependency_versions)?;
    for manifest in plugin_manifests {
        registry
            .register(manifest.clone())
            .map_err(|error| BuildError::Compilation(format!("INVALID_PLUGIN_MANIFEST:{error}")))?;
    }
    Ok(registry)
}

pub fn build_bundle(
    source: BundleBuildSource,
    key_id: &str,
    signing_key: &SigningKey,
) -> Result<ExecutionSpecBundleV2, BuildError> {
    build_bundle_with_resolved(
        source,
        &BTreeMap::new(),
        &BTreeMap::new(),
        key_id,
        signing_key,
    )
}

pub fn build_bundle_with_resolved(
    mut source: BundleBuildSource,
    resolved_manifests: &BTreeMap<String, NodeManifestVersion>,
    resolved_dependencies: &ResolvedPluginManifestDependencies,
    key_id: &str,
    signing_key: &SigningKey,
) -> Result<ExecutionSpecBundleV2, BuildError> {
    if source
        .objects
        .iter()
        .any(|object| !object.has_canonical_key())
    {
        return Err(BuildError::NonCanonicalObject);
    }
    let registry =
        node_registry_with_plugins(&source.dependency_versions, &source.plugin_manifests)?;
    source.triggers.sort_by_key(|trigger| trigger.trigger_id);
    if source
        .triggers
        .windows(2)
        .any(|pair| pair[0].trigger_id == pair[1].trigger_id)
    {
        return Err(BuildError::Compilation(
            "DUPLICATE_RUNTIME_TRIGGER_ID".into(),
        ));
    }
    let compiled = WorkflowCompiler::new(&registry)
        .with_resolved_manifests(resolved_manifests)
        .compile(
            &source.definition,
            &CompileContext {
                current_workflow_version_id: Some(source.workflow_version_id.to_string()),
                ancestor_workflow_version_ids: BTreeSet::new(),
                resource_tool_names: resource_tool_names(&source.resources),
            },
        )
        .map_err(|error| {
            BuildError::Compilation(
                error
                    .issues
                    .into_iter()
                    .map(|issue| issue.code)
                    .collect::<Vec<_>>()
                    .join(","),
            )
        })?;
    let compiled = strip_runtime_sources(compiled);
    validate_model_structured_support(&source.definition, &source.resources)?;
    let mut closure = Vec::new();
    let mut visiting = BTreeSet::from([source.workflow_version_id]);
    collect_dependencies(
        &registry,
        &source.dependency_versions,
        resolved_dependencies,
        &compiled.subworkflow_version_ids,
        &mut visiting,
        &mut closure,
    )?;
    let available_objects = source
        .objects
        .iter()
        .map(|object| object.object_id)
        .collect::<BTreeSet<_>>();
    for entry in &closure {
        for object_id in &entry.object_ids {
            if !available_objects.contains(object_id) {
                return Err(BuildError::MissingDependencyObject(*object_id));
            }
        }
    }
    if !available_objects.contains(&source.workflow_version_id) {
        return Err(BuildError::MissingDependencyObject(
            source.workflow_version_id,
        ));
    }
    let node_manifests = frozen_closure_node_manifests(
        &source.definition,
        &source.dependency_versions,
        &registry,
        resolved_manifests,
        resolved_dependencies,
    );
    let capabilities = compiled
        .nodes
        .iter()
        .map(|node| capability_name(&node.capability).to_owned())
        .collect::<BTreeSet<_>>();
    if let Some(capability) = capabilities
        .iter()
        .find(|capability| !source.supported_capabilities.contains(*capability))
    {
        return Err(BuildError::UnsupportedCapability(capability.clone()));
    }
    let context_contract = serde_json::to_value(&source.definition.start.contexts)
        .expect("Workflow contexts serialize");
    let agent_bundle = build_agent_bundle(
        &compiled,
        &node_manifests,
        &source.authorization,
        &source.resources,
    )?;
    ExecutionSpecBundleV2::signed(
        ExecutionSpecPayloadV2 {
            schema_version: BUNDLE_SCHEMA_VERSION,
            bundle_id: source.bundle_id,
            tenant_id: source.tenant_id,
            application_id: source.application_id,
            deployment_id: source.deployment_id,
            workflow_id: source.workflow_id,
            workflow_version_id: source.workflow_version_id,
            workflow_name: source.workflow_name,
            workflow_version_number: source.workflow_version_number,
            workflow_owner_department: source.workflow_owner_department,
            bundle_sequence: source.sequence,
            definition: serde_json::to_value(source.definition)
                .expect("Workflow Definition serializes"),
            compiled_ir: compiled,
            node_manifests,
            agent_bundle,
            input_contract: source.input_contract,
            output_contract: source.output_contract,
            context_contract,
            resources: source.resources,
            authorization: source.authorization,
            dependency_closure: DependencyClosureV1 { entries: closure },
            triggers: source.triggers,
            runtime_policy: source.runtime_policy,
            objects: source.objects,
            worker_compatibility: WorkerCompatibilityV1 {
                protocol_version: 1,
                ir_versions: BTreeSet::from([1]),
                compiler_versions: BTreeSet::from([COMPILER_VERSION.into()]),
                capabilities,
                manifest_versions: BTreeSet::from([
                    agentx_node_protocol::NODE_PROTOCOL_VERSION.into()
                ]),
            },
            created_at: source.created_at,
        },
        key_id,
        signing_key,
    )
    .map_err(BuildError::from)
}

pub fn build_work_package(
    source: WorkPackageBuildSource,
    key_id: &str,
    signing_key: &SigningKey,
) -> Result<RuntimeWorkPackageV1, BuildError> {
    build_work_package_with_resolved(
        source,
        &BTreeMap::new(),
        &BTreeMap::new(),
        key_id,
        signing_key,
    )
}

pub fn build_work_package_with_resolved(
    source: WorkPackageBuildSource,
    resolved_manifests: &BTreeMap<String, NodeManifestVersion>,
    resolved_dependencies: &ResolvedPluginManifestDependencies,
    key_id: &str,
    signing_key: &SigningKey,
) -> Result<RuntimeWorkPackageV1, BuildError> {
    if source
        .objects
        .iter()
        .any(|object| !object.has_canonical_key())
    {
        return Err(BuildError::NonCanonicalObject);
    }
    let registry =
        node_registry_with_plugins(&source.dependency_versions, &source.plugin_manifests)?;
    let compiled = WorkflowCompiler::new(&registry)
        .with_resolved_manifests(resolved_manifests)
        .compile(
            &source.definition,
            &CompileContext {
                current_workflow_version_id: Some(source.package_id.to_string()),
                ancestor_workflow_version_ids: BTreeSet::new(),
                resource_tool_names: resource_tool_names(&source.resources),
            },
        )
        .map_err(|error| {
            BuildError::Compilation(
                error
                    .issues
                    .into_iter()
                    .map(|issue| issue.code)
                    .collect::<Vec<_>>()
                    .join(","),
            )
        })?;
    let compiled = strip_runtime_sources(compiled);
    validate_model_structured_support(&source.definition, &source.resources)?;
    let mut closure = Vec::new();
    let mut visiting = BTreeSet::from([source.package_id]);
    collect_dependencies(
        &registry,
        &source.dependency_versions,
        resolved_dependencies,
        &compiled.subworkflow_version_ids,
        &mut visiting,
        &mut closure,
    )?;
    let available_objects = source
        .objects
        .iter()
        .map(|object| object.object_id)
        .collect::<BTreeSet<_>>();
    for entry in &closure {
        for object_id in &entry.object_ids {
            if !available_objects.contains(object_id) {
                return Err(BuildError::MissingDependencyObject(*object_id));
            }
        }
    }
    let node_manifests = frozen_closure_node_manifests(
        &source.definition,
        &source.dependency_versions,
        &registry,
        resolved_manifests,
        resolved_dependencies,
    );
    let mut capabilities = compiled
        .nodes
        .iter()
        .map(|node| capability_name(&node.capability).to_owned())
        .collect::<BTreeSet<_>>();
    if matches!(
        &source.spec,
        agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Evaluation { evaluators, .. }
            if evaluators.iter().any(|evaluator| matches!(evaluator, agentx_runtime_contracts::RuntimeEvaluatorV1::Model { .. }))
    ) {
        capabilities.insert("model".into());
    }
    if let Some(capability) = capabilities
        .iter()
        .find(|capability| !source.supported_capabilities.contains(*capability))
    {
        return Err(BuildError::UnsupportedCapability(capability.clone()));
    }
    let model_evaluator_executions = build_model_evaluators(&source.spec, &registry)?;
    let agent_bundle = build_agent_bundle(
        &compiled,
        &node_manifests,
        &source.authorization,
        &source.resources,
    )?;
    RuntimeWorkPackageV1::signed(
        RuntimeWorkPackagePayloadV1 {
            schema_version: WORK_PACKAGE_SCHEMA_VERSION,
            package_id: source.package_id,
            tenant_id: source.tenant_id,
            workflow: source.workflow,
            origin: source.origin,
            purpose: source.purpose,
            call_purpose: source.call_purpose,
            spec: source.spec,
            model_evaluator_executions,
            source_revision: source.source_revision,
            definition: serde_json::to_value(source.definition)
                .expect("Workflow Definition serializes"),
            compiled_ir: compiled,
            node_manifests,
            agent_bundle,
            overlay: source.overlay,
            resource_closure: DependencyClosureV1 { entries: closure },
            resources: source.resources,
            authorization: source.authorization,
            objects: source.objects,
            runtime_policy: source.runtime_policy,
            worker_compatibility: WorkerCompatibilityV1 {
                protocol_version: 1,
                ir_versions: BTreeSet::from([1]),
                compiler_versions: BTreeSet::from([COMPILER_VERSION.into()]),
                capabilities,
                manifest_versions: BTreeSet::from([
                    agentx_node_protocol::NODE_PROTOCOL_VERSION.into()
                ]),
            },
            created_at: source.created_at,
            expires_at: source.expires_at,
        },
        key_id,
        signing_key,
    )
    .map_err(BuildError::from)
}

fn validate_model_structured_support(
    definition: &WorkflowDefinition,
    resources: &[RuntimeResourceBindingV1],
) -> Result<(), BuildError> {
    for node in definition.nodes.iter().filter(|node| {
        node.node_type == "model"
            && node.parameters.get("responseMode").and_then(Value::as_str) == Some("json_schema")
    }) {
        let reference = node
            .resource_references
            .iter()
            .find(|reference| reference.resource_type == agentx_domain::ResourceType::Model)
            .ok_or_else(|| BuildError::Compilation("MODEL_RESOURCE_REQUIRED".into()))?;
        let version = reference
            .resource_version_id
            .ok_or_else(|| BuildError::Compilation("MODEL_RESOURCE_VERSION_REQUIRED".into()))?;
        let binding = resources
            .iter()
            .find(|binding| {
                binding.resource_kind == agentx_runtime_contracts::RuntimeResourceKindV1::Model
                    && binding.resource_id == reference.resource_id
                    && binding.resource_version == version.to_string()
            })
            .ok_or_else(|| BuildError::Compilation("MODEL_RESOURCE_SNAPSHOT_MISSING".into()))?;
        let supported = matches!(
            &binding.configuration,
            agentx_runtime_contracts::RuntimeResourceConfigurationV1::Model { provider, .. }
                if provider == "openai_compatible"
        );
        if !supported {
            return Err(BuildError::Compilation(
                "MODEL_STRUCTURED_OUTPUT_UNSUPPORTED".into(),
            ));
        }
    }
    Ok(())
}

fn frozen_node_manifests(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
    resolved_manifests: &BTreeMap<String, NodeManifestVersion>,
) -> Vec<Value> {
    definition
        .nodes
        .iter()
        .filter(|node| !node.disabled && node.node_type != agentx_domain::WORKFLOW_EXIT_NODE_TYPE)
        .filter_map(|node| {
            resolved_manifests.get(&node.id).or_else(|| {
                registry.resolve_definition_manifest(
                    &node.node_type,
                    node.type_version,
                    &node.parameters,
                )
            })
        })
        .map(|manifest| {
            let mut manifest = manifest.clone();
            if let Some(plugin) = manifest.plugin.as_mut() {
                plugin.runtime_source.clear();
            }
            serde_json::to_value(manifest).expect("Node Manifest serializes")
        })
        .collect()
}

fn strip_runtime_sources(
    mut compiled: agentx_runtime_contracts::CompiledWorkflowV1,
) -> agentx_runtime_contracts::CompiledWorkflowV1 {
    for node in &mut compiled.nodes {
        if let Some(plugin) = node.plugin.as_mut() {
            plugin.runtime_source.clear();
        }
    }
    compiled
}

fn frozen_closure_node_manifests(
    definition: &WorkflowDefinition,
    dependencies: &BTreeMap<Uuid, WorkflowDefinition>,
    registry: &NodeRegistry,
    resolved_manifests: &BTreeMap<String, NodeManifestVersion>,
    resolved_dependencies: &ResolvedPluginManifestDependencies,
) -> Vec<Value> {
    let mut manifests = frozen_node_manifests(definition, registry, resolved_manifests);
    let empty = BTreeMap::new();
    for (version_id, definition) in dependencies {
        manifests.extend(frozen_node_manifests(
            definition,
            registry,
            resolved_dependencies.get(version_id).unwrap_or(&empty),
        ));
    }
    manifests.sort_by_key(Value::to_string);
    manifests.dedup();
    manifests
}

fn build_model_evaluators(
    spec: &agentx_runtime_contracts::RuntimeWorkPackageSpecV1,
    registry: &NodeRegistry,
) -> Result<Vec<RuntimeModelEvaluatorExecutionV1>, BuildError> {
    let agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Evaluation { evaluators, .. } = spec
    else {
        return Ok(Vec::new());
    };
    evaluators
        .iter()
        .filter_map(|evaluator| match evaluator {
            agentx_runtime_contracts::RuntimeEvaluatorV1::Model {
                evaluator_id,
                resource_id,
                prompt_object_id,
            } => Some((*evaluator_id, *resource_id, *prompt_object_id)),
            agentx_runtime_contracts::RuntimeEvaluatorV1::DeterministicRule { .. } => None,
        })
        .map(|(evaluator_id, resource_id, prompt_object_id)| {
            let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
                "schemaVersion":"8.0",
                "start":{"inputs":{},"contexts":{}},
                "nodes":[{
                    "id":"evaluate",
                    "key":"evaluate",
                    "type":"model",
                    "typeVersion":1,
                    "name":"Model Evaluator",
                    "parameters":{"prompt":{"kind":"template","segments":[{"kind":"text","text":"runtime_object"}]},"responseMode":"json_schema","structuredSchema":{"type":"object","additionalProperties":true}},
                    "contextWrites":[],
                    "resourceReferences":[{
                        "resourceType":"model",
                        "resourceId":resource_id,
                        "operation":"use"
                    }]
                },{
                    "id":"__exit__","key":"__exit__","type":"exit","typeVersion":1,"name":"End",
                    "parameters":{"outputs":{"evaluation":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"evaluate","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":["structuredOutput"]},"missingPolicy":{"kind":"error"}}},"errorOutputs":{}}}
                ],
                "connections":[
                    {"id":"start-evaluate","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"evaluate","targetHandle":"main","order":0},
                    {"id":"evaluate-end","sourceNodeId":"evaluate","sourceHandle":"main","targetNodeId":"__exit__","targetHandle":"main","order":0}
                ],
                "end":{"outputs":{"evaluation":{"schema":{"type":"object"},"required":true}}},
                "settings":{"activationBudget":4,"executionOrder":"deterministic"}
            }))
            .map_err(|error| BuildError::Compilation(error.to_string()))?;
            let compiled_ir = WorkflowCompiler::new(registry)
                .compile(
                    &definition,
                    &CompileContext {
                        current_workflow_version_id: Some(evaluator_id.to_string()),
                        ancestor_workflow_version_ids: BTreeSet::new(),
                        resource_tool_names: BTreeMap::new(),
                    },
                )
                .map_err(|error| {
                    BuildError::Compilation(
                        error
                            .issues
                            .into_iter()
                            .map(|issue| issue.code)
                            .collect::<Vec<_>>()
                            .join(","),
                    )
                })?;
            let node_manifests = compiled_ir
                .nodes
                .iter()
                .filter_map(|node| registry.get(&node.node_type, node.type_version))
                .map(|manifest| serde_json::to_value(manifest).expect("Node Manifest serializes"))
                .collect();
            Ok(RuntimeModelEvaluatorExecutionV1 {
                evaluator_id,
                resource_id,
                prompt_object_id,
                definition: serde_json::to_value(definition)
                    .expect("model evaluator Definition serializes"),
                compiled_ir,
                node_manifests,
            })
        })
        .collect()
}

fn collect_dependencies(
    registry: &NodeRegistry,
    definitions: &BTreeMap<Uuid, WorkflowDefinition>,
    resolved_dependencies: &ResolvedPluginManifestDependencies,
    version_ids: &[String],
    visiting: &mut BTreeSet<Uuid>,
    closure: &mut Vec<DependencyClosureEntryV1>,
) -> Result<(), BuildError> {
    let mut ids = version_ids
        .iter()
        .map(|value| {
            Uuid::parse_str(value)
                .map_err(|_| BuildError::Compilation("INVALID_SUBWORKFLOW_VERSION".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        if !visiting.insert(id) {
            return Err(BuildError::DependencyCycle(id));
        }
        let definition = definitions
            .get(&id)
            .ok_or(BuildError::MissingDependency(id))?;
        for referenced_id in direct_dependency_ids(definition)? {
            if visiting.contains(&referenced_id) {
                return Err(BuildError::DependencyCycle(referenced_id));
            }
        }
        let empty = BTreeMap::new();
        let resolved = resolved_dependencies.get(&id).unwrap_or(&empty);
        let compiled = WorkflowCompiler::new(registry)
            .with_resolved_manifests(resolved)
            .compile(
                definition,
                &CompileContext {
                    current_workflow_version_id: Some(id.to_string()),
                    ancestor_workflow_version_ids: visiting.iter().map(Uuid::to_string).collect(),
                    resource_tool_names: BTreeMap::new(),
                },
            )
            .map_err(|error| {
                BuildError::Compilation(
                    error
                        .issues
                        .into_iter()
                        .map(|issue| issue.code)
                        .collect::<Vec<_>>()
                        .join(","),
                )
            })?;
        let compiled = strip_runtime_sources(compiled);
        collect_dependencies(
            registry,
            definitions,
            resolved_dependencies,
            &compiled.subworkflow_version_ids,
            visiting,
            closure,
        )?;
        let hash = agentx_runtime_contracts::content_hash(definition)?;
        closure.push(DependencyClosureEntryV1 {
            kind: DependencyKindV1::CompositeWorkflow,
            object_id: id,
            version: id.to_string(),
            content_hash: hash,
            object_ids: vec![id, composite_ir_object_id(id)],
        });
        visiting.remove(&id);
    }
    closure.sort_by_key(|entry| (entry.kind, entry.object_id));
    closure.dedup_by_key(|entry| (entry.kind, entry.object_id));
    Ok(())
}

fn direct_dependency_ids(definition: &WorkflowDefinition) -> Result<Vec<Uuid>, BuildError> {
    definition
        .nodes
        .iter()
        .filter(|node| node.node_type == "sub_workflow")
        .map(|node| {
            node.parameters
                .get("workflowVersionId")
                .and_then(Value::as_str)
                .ok_or_else(|| BuildError::Compilation("SUBWORKFLOW_VERSION_REQUIRED".to_owned()))
                .and_then(|value| {
                    Uuid::parse_str(value).map_err(|_| {
                        BuildError::Compilation("INVALID_SUBWORKFLOW_VERSION".to_owned())
                    })
                })
        })
        .collect()
}

fn capability_name(capability: &NodeCapability) -> &'static str {
    capability.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentx_runtime_contracts::{ContentHash, RuntimeAuthorizationSnapshotV1, StorageDomain};
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use serde_json::json;

    fn definition() -> WorkflowDefinition {
        serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"],"additionalProperties":false},"contexts":{}},
            "nodes":[{"id":"pass","key":"pass","type":"set","typeVersion":1,"name":"Pass","parameters":{},"contextWrites":[]},{"id":"__exit__","key":"__exit__","type":"exit","typeVersion":1,"name":"End","parameters":{"outputs":{"message":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"pass","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":["message"]},"missingPolicy":{"kind":"error"}}},"errorOutputs":{}}}],
            "connections":[
                {"id":"start-pass","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"pass","targetHandle":"main","order":0},
                {"id":"pass-end","sourceNodeId":"pass","sourceHandle":"main","targetNodeId":"__exit__","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{"message":{"schema":{"type":"string"},"required":true}}},
            "settings":{}
        })).unwrap()
    }

    fn source() -> BundleBuildSource {
        let tenant_id = Uuid::from_u128(1);
        let object_id = Uuid::from_u128(6);
        let hash =
            agentx_runtime_contracts::ContentHash::parse(format!("sha256:{}", "a".repeat(64)))
                .unwrap();
        BundleBuildSource {
            bundle_id: Uuid::from_u128(2),
            tenant_id,
            application_id: Uuid::from_u128(3),
            deployment_id: Uuid::from_u128(4),
            workflow_id: Uuid::from_u128(5),
            workflow_version_id: Uuid::from_u128(6),
            workflow_name: "Workflow".into(),
            workflow_version_number: 1,
            workflow_owner_department: None,
            sequence: 1,
            definition: definition(),
            dependency_versions: BTreeMap::new(),
            plugin_manifests: vec![],
            supported_capabilities: BTreeSet::from(["builtin".into(), "plugin_nodejs".into()]),
            input_contract: json!({"type":"object"}),
            output_contract: json!({"type":"object"}),
            resources: vec![],
            authorization: RuntimeAuthorizationSnapshotV1 {
                schema_version: 1,
                tenant_id,
                service_identity_id: Uuid::from_u128(7),
                workflow_id: Uuid::from_u128(5),
                policy_epoch: 1,
                capabilities: BTreeSet::from(["builtin".into()]),
                grant_ids: vec![],
                grant_bindings: vec![],
                maximum_policy_staleness_seconds: 72 * 60 * 60,
                captured_at: OffsetDateTime::UNIX_EPOCH,
            },
            triggers: vec![],
            runtime_policy: RuntimePolicyV1 {
                timeout_seconds: 30,
                operation_deadline_seconds: 30,
                ..RuntimePolicyV1::default()
            },
            objects: vec![
                RuntimeObjectReferenceV1 {
                    tenant_id,
                    storage_domain: StorageDomain::Runtime,
                    object_id,
                    object_key: RuntimeObjectReferenceV1::canonical_key(
                        tenant_id, object_id, &hash,
                    ),
                    content_hash: hash,
                    size_bytes: 2,
                    media_type: "application/json".into(),
                },
                builtin_data_runtime_object(tenant_id),
            ],
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn agent_definition(with_sandbox: bool, session_mode: &str) -> WorkflowDefinition {
        let mut references = vec![json!({
            "bindingRole":"model",
            "resourceType":"model",
            "resourceId":"11111111-1111-4111-8111-111111111111",
            "resourceVersionId":"22222222-2222-4222-8222-222222222222",
            "operation":"use"
        })];
        if with_sandbox {
            references.push(json!({
                "bindingRole":"workspace_sandbox",
                "resourceType":"sandbox_profile",
                "resourceId":"33333333-3333-4333-8333-333333333333",
                "resourceVersionId":"44444444-4444-4444-8444-444444444444",
                "operation":"use"
            }));
        }
        serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
            "nodes":[{
                "id":"agent","key":"agent","type":"agent","typeVersion":2,"name":"Agent",
                "parameters":{"sessionPolicy":{"mode":session_mode}},
                "resourceReferences":references,"contextWrites":[]
            },{
                "id":"__exit__","key":"__exit__","type":"exit","typeVersion":1,"name":"End","parameters":{"outputs":{},"errorOutputs":{}}
            }],
            "connections":[
                {"id":"start-agent","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"agent","targetHandle":"main","order":0},
                {"id":"agent-end","sourceNodeId":"agent","sourceHandle":"main","targetNodeId":"__exit__","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{}}
        }))
        .expect("Agent Definition 8.0 fixture")
    }

    fn agent_source(with_sandbox: bool, session_mode: &str) -> BundleBuildSource {
        let mut source = source();
        source.definition = agent_definition(with_sandbox, session_mode);
        source.supported_capabilities = BTreeSet::from(["agent".into()]);
        source.authorization.capabilities = BTreeSet::from(["agent".into()]);
        source
    }

    fn structured_model_source(provider: &str) -> BundleBuildSource {
        let resource_id = Uuid::from_u128(101);
        let version_id = Uuid::from_u128(102);
        let mut source = source();
        source.definition.nodes[0].node_type = "model".into();
        source.definition.nodes[0].parameters = json!({
            "responseMode":"json_schema",
            "structuredSchema":{"type":"object","required":["answer"],"properties":{"answer":{"type":"string"}},"additionalProperties":false}
        });
        source.definition.nodes[0].resource_references = vec![agentx_domain::ResourceReference {
            binding_role: None,
            resource_type: agentx_domain::ResourceType::Model,
            resource_id,
            resource_version_id: Some(version_id),
            operation: agentx_domain::ResourceOperation::Use,
        }];
        source.definition.nodes[1].parameters = json!({"outputs":{},"errorOutputs":{}});
        source.definition.end = Default::default();
        source.supported_capabilities = BTreeSet::from(["model".into()]);
        source.authorization.capabilities = BTreeSet::from(["model".into()]);
        source.resources = vec![RuntimeResourceBindingV1 {
            resource_kind: agentx_runtime_contracts::RuntimeResourceKindV1::Model,
            resource_id,
            resource_version: version_id.to_string(),
            state_epoch: 1,
            content_hash: agentx_runtime_contracts::ContentHash::parse(format!(
                "sha256:{}",
                "c".repeat(64)
            ))
            .unwrap(),
            configuration: agentx_runtime_contracts::RuntimeResourceConfigurationV1::Model {
                provider: provider.into(),
                endpoint: "https://models.example/v1".into(),
                model: "test-model".into(),
                context_window: 128_000,
                price: agentx_runtime_contracts::RuntimeModelPriceV1 {
                    version_id: "price-1".into(),
                    currency: "USD".into(),
                    input_per_million: "1".into(),
                    output_per_million: "2".into(),
                },
                credential: None,
                capabilities: Vec::new(),
            },
            object_ids: vec![],
        }];
        source
    }

    fn work_package_source() -> WorkPackageBuildSource {
        let tenant_id = Uuid::from_u128(1);
        let package_id = Uuid::from_u128(30);
        let definition = definition();
        let compiled = compile_workflow_version(&definition, package_id).unwrap();
        WorkPackageBuildSource {
            package_id,
            tenant_id,
            workflow: agentx_runtime_contracts::ExecutionWorkflowSnapshotV1 {
                id: Uuid::from_u128(5),
                name: "Workflow".into(),
                version_id: package_id,
                version_number: 7,
                owner_department: None,
            },
            origin: agentx_runtime_contracts::ExecutionOriginV1::system(None),
            purpose: WorkPackagePurpose::Debug,
            call_purpose: RuntimeCallPurposeV1::Debug,
            spec: agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Debug {
                draft_revision: 7,
                debug_plan: agentx_runtime_contracts::RuntimeDebugPlanV1::whole(&compiled),
            },
            source_revision: "draft:7".into(),
            definition,
            dependency_versions: BTreeMap::new(),
            plugin_manifests: vec![],
            supported_capabilities: BTreeSet::from(["builtin".into(), "plugin_nodejs".into()]),
            overlay: RuntimeWorkPackageOverlayV1 {
                input: json!({"message":"debug"}),
                ..RuntimeWorkPackageOverlayV1::default()
            },
            resources: vec![],
            authorization: RuntimeAuthorizationSnapshotV1 {
                schema_version: 1,
                tenant_id,
                service_identity_id: Uuid::from_u128(7),
                workflow_id: Uuid::from_u128(5),
                policy_epoch: 1,
                capabilities: BTreeSet::from(["builtin".into()]),
                grant_ids: vec![],
                grant_bindings: vec![],
                maximum_policy_staleness_seconds: 72 * 60 * 60,
                captured_at: OffsetDateTime::UNIX_EPOCH,
            },
            objects: vec![builtin_data_runtime_object(tenant_id)],
            runtime_policy: RuntimePolicyV1::default(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1),
        }
    }

    fn builtin_data_runtime_object(tenant_id: Uuid) -> RuntimeObjectReferenceV1 {
        let registry = NodeRegistry::m5_defaults();
        let artifact = registry
            .get("set", 1)
            .and_then(|manifest| manifest.plugin.as_ref())
            .and_then(|plugin| plugin.runtime_artifact.as_ref())
            .expect("agentx/data Runtime artifact");
        let content_hash = ContentHash::parse(artifact.content_hash.clone()).unwrap();
        RuntimeObjectReferenceV1 {
            tenant_id,
            storage_domain: StorageDomain::Runtime,
            object_id: artifact.object_id,
            object_key: RuntimeObjectReferenceV1::canonical_key(
                tenant_id,
                artifact.object_id,
                &content_hash,
            ),
            content_hash,
            size_bytes: artifact.size_bytes,
            media_type: artifact.media_type.clone(),
        }
    }

    #[test]
    fn rebuild_is_deterministic_and_bundle_is_signed() {
        let key = SigningKey::generate(&mut OsRng);
        let first = build_bundle(source(), "bundle-current", &key).unwrap();
        let second = build_bundle(source(), "bundle-current", &key).unwrap();
        assert_eq!(first.content_hash, second.content_hash);
        let plugin = first.payload.compiled_ir.nodes[0].plugin.as_ref().unwrap();
        assert!(plugin.runtime_source.is_empty());
        assert!(first.payload.objects.iter().any(|object| {
            Some(object.object_id)
                == plugin
                    .runtime_artifact
                    .as_ref()
                    .map(|artifact| artifact.object_id)
        }));
        assert!(first.payload.node_manifests.iter().all(|manifest| {
            manifest
                .pointer("/plugin/runtimeSource")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        }));
        assert_eq!(
            first.signature.signature_base64,
            second.signature.signature_base64
        );
        first.verify(&key.verifying_key()).unwrap();
    }

    #[test]
    fn bundle_v2_freezes_agent_configuration_and_sandbox_derived_tools() {
        let key = SigningKey::generate(&mut OsRng);
        let without_sandbox =
            build_bundle(agent_source(false, "invocation"), "bundle-current", &key).unwrap();
        assert_eq!(without_sandbox.payload.schema_version, 2);
        assert_eq!(without_sandbox.payload.agent_bundle.bundle_version, "2.0");
        assert_eq!(
            without_sandbox.payload.agent_bundle.definition_hash,
            without_sandbox.payload.compiled_ir.definition_hash
        );
        let agent = &without_sandbox.payload.agent_bundle.agents[0];
        assert!(agent.configuration.workspace_sandbox.is_none());
        assert!(agent.configuration.core_tools.is_empty());

        let with_sandbox =
            build_bundle(agent_source(true, "invocation"), "bundle-current", &key).unwrap();
        let agent = &with_sandbox.payload.agent_bundle.agents[0];
        assert_eq!(
            agent
                .configuration
                .core_tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["read", "write", "edit", "bash"]
        );
        assert_eq!(agent.resource_closure.len(), 2);
        assert_ne!(without_sandbox.content_hash, with_sandbox.content_hash);
    }

    #[test]
    fn bundle_v2_hash_changes_with_agent_contract_inputs() {
        let key = SigningKey::generate(&mut OsRng);
        let baseline =
            build_bundle(agent_source(false, "invocation"), "bundle-current", &key).unwrap();

        let mut changed_model = agent_source(false, "invocation");
        changed_model.definition.nodes[0].resource_references[0].resource_version_id =
            Some(Uuid::from_u128(99));
        let changed_model = build_bundle(changed_model, "bundle-current", &key).unwrap();

        let changed_session = build_bundle(
            agent_source(false, "application_session"),
            "bundle-current",
            &key,
        )
        .unwrap();

        let mut changed_grant = agent_source(false, "invocation");
        changed_grant.authorization.grant_ids = vec![Uuid::from_u128(77)];
        let changed_grant = build_bundle(changed_grant, "bundle-current", &key).unwrap();

        let mut changed_epoch = agent_source(false, "invocation");
        changed_epoch.authorization.policy_epoch += 1;
        let changed_epoch = build_bundle(changed_epoch, "bundle-current", &key).unwrap();

        for changed in [changed_model, changed_session, changed_grant, changed_epoch] {
            assert_ne!(baseline.content_hash, changed.content_hash);
        }
    }

    #[test]
    fn structured_model_requires_a_provider_with_native_json_schema_support() {
        let key = SigningKey::generate(&mut OsRng);
        let unsupported = build_bundle(
            structured_model_source("custom_http"),
            "bundle-current",
            &key,
        );
        assert!(matches!(
            unsupported,
            Err(BuildError::Compilation(ref code)) if code == "MODEL_STRUCTURED_OUTPUT_UNSUPPORTED"
        ));
        build_bundle(
            structured_model_source("openai_compatible"),
            "bundle-current",
            &key,
        )
        .expect("openai-compatible model freezes native JSON Schema mode");
    }

    #[test]
    fn work_package_build_is_deterministic_and_uses_an_independent_key() {
        let bundle_key = SigningKey::generate(&mut OsRng);
        let work_package_key = SigningKey::generate(&mut OsRng);
        let first = build_work_package(
            work_package_source(),
            "work-package-current",
            &work_package_key,
        )
        .unwrap();
        let second = build_work_package(
            work_package_source(),
            "work-package-current",
            &work_package_key,
        )
        .unwrap();
        assert_eq!(first.content_hash, second.content_hash);
        assert_eq!(
            first.signature.signature_base64,
            second.signature.signature_base64
        );
        first.verify(&work_package_key.verifying_key()).unwrap();
        assert!(first.verify(&bundle_key.verifying_key()).is_err());
    }

    #[test]
    fn non_canonical_object_key_is_rejected() {
        let mut source = source();
        source.objects[0].object_key = "wrong".into();
        assert!(matches!(
            build_bundle(source, "bundle-current", &SigningKey::generate(&mut OsRng)),
            Err(BuildError::NonCanonicalObject)
        ));
    }

    #[test]
    fn missing_fixed_dependency_is_rejected() {
        let child = Uuid::from_u128(20);
        let mut source = source();
        source.definition = composite_definition(&[child]);
        let result = build_bundle(source, "bundle-current", &SigningKey::generate(&mut OsRng));
        assert!(
            matches!(
                result,
                Err(BuildError::MissingDependency(id)) if id == child
            ),
            "{result:?}"
        );
    }

    #[test]
    fn mutable_subworkflow_head_and_unknown_node_version_are_rejected() {
        let mut mutable_head_source = source();
        mutable_head_source.definition.nodes[0].node_type = "sub_workflow".into();
        mutable_head_source.definition.nodes[0].parameters =
            json!({"workflowId": Uuid::from_u128(20)});
        assert!(matches!(
            build_bundle(
                mutable_head_source,
                "bundle-current",
                &SigningKey::generate(&mut OsRng)
            ),
            Err(BuildError::Compilation(code)) if code.contains("SUBWORKFLOW_VERSION_REQUIRED")
        ));

        let mut source = source();
        source.definition.nodes[0].type_version = 999;
        assert!(matches!(
            build_bundle(source, "bundle-current", &SigningKey::generate(&mut OsRng)),
            Err(BuildError::Compilation(code)) if code.contains("UNKNOWN_NODE_VERSION")
        ));
    }

    #[test]
    fn dependency_cycles_are_rejected_before_signing() {
        let root = Uuid::from_u128(6);
        let child = Uuid::from_u128(20);
        let mut source = source();
        source.definition = composite_definition(&[child]);
        source
            .dependency_versions
            .insert(child, composite_definition(&[root]));
        source.objects.push(object(source.tenant_id, child));
        let result = build_bundle(source, "bundle-current", &SigningKey::generate(&mut OsRng));
        assert!(
            matches!(
                result,
                Err(BuildError::DependencyCycle(id)) if id == root
            ),
            "{result:?}"
        );
    }

    #[test]
    fn unsupported_runtime_capability_is_rejected_by_the_builder() {
        let mut source = source();
        source.supported_capabilities.remove("plugin_nodejs");
        source.definition.nodes[0].node_type = "declarative_http".into();
        source.definition.nodes[0].parameters = json!({"url":{"kind":"template","segments":[{"kind":"text","text":"https://example.invalid"}]}});
        source.definition.nodes[1].parameters = json!({"outputs":{},"errorOutputs":{}});
        source.definition.end = Default::default();
        let result = build_bundle(source, "bundle-current", &SigningKey::generate(&mut OsRng));
        assert!(
            matches!(
                result,
                Err(BuildError::UnsupportedCapability(ref capability)) if capability == "plugin_nodejs"
            ),
            "{result:?}"
        );
    }

    #[test]
    fn transitive_dependency_closure_is_sorted_complete_and_deterministic() {
        let first_child = Uuid::from_u128(20);
        let grandchild = Uuid::from_u128(10);
        let mut source = source();
        source.definition = composite_definition(&[first_child]);
        source
            .dependency_versions
            .insert(first_child, composite_definition(&[grandchild]));
        source.dependency_versions.insert(grandchild, definition());
        source.objects.extend([
            object(source.tenant_id, first_child),
            object(source.tenant_id, composite_ir_object_id(first_child)),
            object(source.tenant_id, grandchild),
            object(source.tenant_id, composite_ir_object_id(grandchild)),
        ]);
        let key = SigningKey::generate(&mut OsRng);
        let first = build_bundle(source.clone(), "bundle-current", &key).unwrap();
        let second = build_bundle(source, "bundle-current", &key).unwrap();
        let entries = &first.payload.dependency_closure.entries;
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.object_id)
                .collect::<Vec<_>>(),
            vec![grandchild, first_child]
        );
        assert!(
            entries.iter().all(|entry| entry.object_ids
                == [entry.object_id, composite_ir_object_id(entry.object_id)])
        );
        assert_eq!(first.content_hash, second.content_hash);
    }

    #[test]
    fn immutable_composite_registry_pins_version_io_and_context_contracts() {
        let child_id = Uuid::from_u128(42);
        let child: WorkflowDefinition = serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{
                "inputs":{"type":"object","required":["question"],"properties":{"question":{"type":"string"}},"additionalProperties":false},
                "contexts":{"counter":{"schema":{"type":"number"},"default":0,"mutable":true,"sensitive":false,"clientWritable":false,"scope":"execution_tree","mergePolicy":"increment"}}
            },
            "nodes":[{"id":"__exit__","key":"__exit__","type":"exit","typeVersion":1,"name":"End","parameters":{"outputs":{"answer":{"kind":"reference","selector":{"namespace":"inputs","run":{"kind":"current"},"item":{"kind":"current"},"path":["question"]},"missingPolicy":{"kind":"error"}}},"errorOutputs":{}}}],
            "connections":[{"id":"direct","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"__exit__","targetHandle":"main","order":0}],
            "end":{"outputs":{"answer":{"schema":{"type":"string"},"required":true,"sensitive":false}}},
            "settings":{}
        }))
        .unwrap();
        let definitions = BTreeMap::from([(child_id, child)]);
        let registry = node_registry_with_composites(&definitions).unwrap();
        let node_type = format!("workflow.{}", child_id.simple());
        let manifest = registry.get(&node_type, 1).unwrap();
        assert_eq!(
            manifest.parameter_schema["properties"]["workflowVersionId"]["const"],
            child_id.to_string()
        );
        assert_eq!(
            manifest.parameter_schema["properties"]["inputs"]["allOf"][0]["required"],
            json!(["question"])
        );
        assert_eq!(
            manifest.parameter_schema["properties"]["inputs"]["x-agentx-binding"]["allowedNamespaces"],
            json!(["inputs", "outputs", "contexts", "execution"])
        );
        assert_eq!(manifest.output_schema["required"], json!(["answer"]));
        assert_eq!(
            manifest.parameter_schema["x-agentx-contextContract"]["counter"]["mergePolicy"],
            "increment"
        );

        let parent: WorkflowDefinition = serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{
                "inputs":{"type":"object","required":["question"],"properties":{"question":{"type":"string"}},"additionalProperties":false},
                "contexts":{"counter":{"schema":{"type":"number"},"default":0,"mutable":true,"sensitive":false,"clientWritable":false,"scope":"execution_tree","mergePolicy":"increment"}}
            },
            "nodes":[
                {"id":"child","key":"child","type":"sub_workflow","typeVersion":1,"name":"Child","parameters":{"workflowVersionId":child_id,"inputs":{"kind":"object","fields":{"question":{"kind":"reference","selector":{"namespace":"inputs","run":{"kind":"current"},"item":{"kind":"current"},"path":["question"]},"missingPolicy":{"kind":"error"}}}}},"contextWrites":[],"resourceReferences":[]},
                {"id":"summary","key":"summary","type":"set","typeVersion":1,"name":"Summary","parameters":{"values":{"kind":"object","fields":{"answer":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"child","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":["answer"]},"missingPolicy":{"kind":"error"}},"counter":{"kind":"reference","selector":{"namespace":"contexts","run":{"kind":"current"},"item":{"kind":"current"},"path":["counter"]},"missingPolicy":{"kind":"error"}}}},"keepOnlySet":true},"contextWrites":[],"resourceReferences":[]},
                {"id":"__exit__","key":"__exit__","type":"exit","typeVersion":1,"name":"End","parameters":{"outputs":{"answer":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"summary","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":["answer"]},"missingPolicy":{"kind":"error"}},"counter":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"summary","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":["counter"]},"missingPolicy":{"kind":"error"}}},"errorOutputs":{}}}
            ],
            "connections":[
                {"id":"start-child","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"child","targetHandle":"main","order":0},
                {"id":"child-summary","sourceNodeId":"child","sourceHandle":"main","targetNodeId":"summary","targetHandle":"main","order":0},
                {"id":"summary-end","sourceNodeId":"summary","sourceHandle":"main","targetNodeId":"__exit__","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{"answer":{"schema":{"type":"string"},"required":true,"sensitive":false},"counter":{"schema":{"type":"number"},"required":true,"sensitive":false}}},
            "settings":{}
        }))
        .unwrap();
        let compiled =
            compile_workflow_version_with_dependencies(&parent, Uuid::from_u128(43), &definitions)
                .unwrap();
        assert_eq!(
            compiled.nodes[0].effective_output_contract.port_schemas["main"]["required"],
            json!(["answer"])
        );
    }

    #[test]
    fn dependency_closure_compiles_each_child_with_its_resolved_plugin_manifest() {
        let child_id = Uuid::from_u128(420);
        let child: WorkflowDefinition = serde_json::from_value(json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{"message":{"type":"string"}}},"contexts":{}},
            "nodes":[
                {"id":"dynamic","key":"dynamic","type":"set","typeVersion":1,"name":"Dynamic","parameters":{},"contextWrites":[],"resourceReferences":[]},
                {"id":"__exit__","key":"__exit__","type":"exit","typeVersion":1,"name":"End","parameters":{"outputs":{"message":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"dynamic","port":"alternate","run":{"kind":"current"},"item":{"kind":"current"},"path":["message"]},"missingPolicy":{"kind":"error"}}},"errorOutputs":{}}}
            ],
            "connections":[
                {"id":"start-dynamic","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"dynamic","targetHandle":"main","order":0},
                {"id":"dynamic-end","sourceNodeId":"dynamic","sourceHandle":"alternate","targetNodeId":"__exit__","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{"message":{"schema":{"type":"string"},"required":true}}},
            "settings":{}
        }))
        .unwrap();
        let dependencies = BTreeMap::from([(child_id, child)]);
        let parent = composite_definition(&[child_id]);
        let registry = node_registry_with_composites(&dependencies).unwrap();
        let compiled_parent = WorkflowCompiler::new(&registry)
            .compile(&parent, &CompileContext::default())
            .unwrap();
        let mut resolved = registry.get("set", 1).unwrap().clone();
        resolved.output_ports.push(agentx_node_protocol::NodePort {
            name: "alternate".into(),
            kind: agentx_node_protocol::PortKind::Main,
            required: false,
            variadic: false,
        });
        resolved.output_port_schemas.insert(
            "alternate".into(),
            json!({"type":"object","required":["message"],"properties":{"message":{"type":"string"}},"additionalProperties":false}),
        );
        let mut closure = Vec::new();
        let mut visiting = BTreeSet::from([Uuid::from_u128(421)]);
        assert!(
            collect_dependencies(
                &registry,
                &dependencies,
                &BTreeMap::new(),
                &compiled_parent.subworkflow_version_ids,
                &mut visiting,
                &mut closure,
            )
            .is_err(),
            "the dependency is invalid without its per-node dynamic port"
        );

        let resolved_dependencies =
            BTreeMap::from([(child_id, BTreeMap::from([("dynamic".into(), resolved)]))]);
        let mut closure = Vec::new();
        let mut visiting = BTreeSet::from([Uuid::from_u128(421)]);
        collect_dependencies(
            &registry,
            &dependencies,
            &resolved_dependencies,
            &compiled_parent.subworkflow_version_ids,
            &mut visiting,
            &mut closure,
        )
        .unwrap();
        assert_eq!(closure.len(), 1);
        assert_eq!(closure[0].object_id, child_id);
        let frozen = frozen_closure_node_manifests(
            &parent,
            &dependencies,
            &registry,
            &BTreeMap::new(),
            &resolved_dependencies,
        );
        assert!(frozen.iter().any(|manifest| {
            manifest["nodeType"] == "set"
                && manifest["outputPorts"]
                    .as_array()
                    .is_some_and(|ports| ports.iter().any(|port| port["name"] == "alternate"))
        }));
    }

    fn composite_definition(children: &[Uuid]) -> WorkflowDefinition {
        let mut value = serde_json::to_value(definition()).unwrap();
        let mut nodes = children
            .iter()
            .enumerate()
            .map(|(index, child)| {
                json!({
                    "id": format!("child_{index}"),
                    "key": format!("child_{index}"),
                    "type": "sub_workflow",
                    "typeVersion": 1,
                    "name": format!("Child {index}"),
                    "parameters": {"workflowVersionId": child,"inputs":{"kind":"object","fields":{"message":{"kind":"literal","value":"hello"}}}},
                    "contextWrites": [],
                    "resourceReferences": []
                })
            })
            .collect::<Vec<_>>();
        let mut connections = Vec::new();
        for index in 0..children.len() {
            connections.push(json!({
                "id": format!("edge-{index}"),
                "sourceNodeId": if index == 0 { "__start__".to_owned() } else { format!("child_{}", index - 1) },
                "sourceHandle": "main",
                "targetNodeId": format!("child_{index}"),
                "targetHandle": "main",
                "order": 0
            }));
        }
        if !children.is_empty() {
            nodes.push(json!({
                "id":"__exit__","key":"__exit__","type":"exit","typeVersion":1,"name":"End",
                "parameters":{"outputs":{},"errorOutputs":{}}
            }));
            connections.push(json!({
                "id": "edge-end",
                "sourceNodeId": format!("child_{}", children.len() - 1),
                "sourceHandle": "main",
                "targetNodeId": "__exit__",
                "targetHandle": "main",
                "order": 0
            }));
        }
        value["nodes"] = Value::Array(nodes);
        value["connections"] = Value::Array(connections);
        value["end"] = json!({"outputs": {}});
        serde_json::from_value(value).unwrap()
    }

    fn object(tenant_id: Uuid, object_id: Uuid) -> RuntimeObjectReferenceV1 {
        let hash =
            agentx_runtime_contracts::ContentHash::parse(format!("sha256:{}", "b".repeat(64)))
                .unwrap();
        RuntimeObjectReferenceV1 {
            tenant_id,
            storage_domain: StorageDomain::Runtime,
            object_id,
            object_key: RuntimeObjectReferenceV1::canonical_key(tenant_id, object_id, &hash),
            content_hash: hash,
            size_bytes: 2,
            media_type: "application/json".into(),
        }
    }
}
