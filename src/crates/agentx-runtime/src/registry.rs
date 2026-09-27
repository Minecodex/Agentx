use std::collections::BTreeMap;

use agentx_domain::ResourceType;
use agentx_node_protocol::{
    BindingSlotPlacement, NODE_PROTOCOL_VERSION, NodeManifestVersion, PluginNodeBinding,
    PluginRuntimeArtifact, plugin_runtime_object_id,
};
#[cfg(test)]
use agentx_node_protocol::{NodeCapability, PortKind};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Clone, Debug, Default)]
pub struct NodeRegistry {
    manifests: BTreeMap<(String, u32), NodeManifestVersion>,
}

#[derive(Debug, Error, PartialEq)]
pub enum RegistryError {
    #[error("node manifest {node_type}@{version} already exists")]
    Duplicate { node_type: String, version: u32 },
    #[error("node manifest {node_type}@{version} uses unsupported protocol {protocol_version}")]
    UnsupportedProtocol {
        node_type: String,
        version: u32,
        protocol_version: String,
    },
    #[error("node manifest {node_type}@{version} is invalid: {message}")]
    InvalidManifest {
        node_type: String,
        version: u32,
        message: String,
    },
}

impl NodeRegistry {
    #[must_use]
    pub fn is_definition_node_type(node_type: &str) -> bool {
        !node_type.is_empty()
            && !matches!(
                node_type,
                "mcp_tool" | "skill" | "rag" | "memory" | "sandbox"
            )
    }

    pub fn register(&mut self, manifest: NodeManifestVersion) -> Result<(), RegistryError> {
        if manifest.protocol_version != NODE_PROTOCOL_VERSION {
            return Err(RegistryError::UnsupportedProtocol {
                node_type: manifest.node_type,
                version: manifest.version,
                protocol_version: manifest.protocol_version,
            });
        }
        if let Err(message) = manifest
            .validate_binding_slots()
            .and_then(|()| manifest.validate_localizations())
            .and_then(|()| manifest.validate_plugin())
            .and_then(|()| validate_agent_manifest(&manifest))
        {
            return Err(RegistryError::InvalidManifest {
                node_type: manifest.node_type,
                version: manifest.version,
                message,
            });
        }
        let key = manifest.key();
        if self.manifests.insert(key.clone(), manifest).is_some() {
            return Err(RegistryError::Duplicate {
                node_type: key.0,
                version: key.1,
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn get(&self, node_type: &str, version: u32) -> Option<&NodeManifestVersion> {
        self.manifests.get(&(node_type.to_owned(), version))
    }

    /// Resolves the generic Studio Sub-workflow node to the immutable
    /// version-derived Manifest when that dependency snapshot is available.
    #[must_use]
    pub fn resolve_definition_manifest(
        &self,
        node_type: &str,
        version: u32,
        parameters: &Value,
    ) -> Option<&NodeManifestVersion> {
        if node_type == "sub_workflow"
            && let Some(version_id) = parameters
                .get("workflowVersionId")
                .and_then(Value::as_str)
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
        {
            let derived = format!("workflow.{}", version_id.simple());
            if let Some(manifest) = self.get(&derived, version) {
                return Some(manifest);
            }
        }
        self.get(node_type, version)
    }

    pub fn manifests(&self) -> impl Iterator<Item = &NodeManifestVersion> {
        self.manifests.values()
    }

    /// The only runtime node types users may create in Workflow Studio.
    /// Resource executors remain registered for Agent attachments, but never
    /// become a second public node catalog.
    pub fn studio_manifests(&self) -> impl Iterator<Item = &NodeManifestVersion> {
        self.manifests.values().filter(|manifest| {
            Self::is_definition_node_type(&manifest.node_type)
                && !manifest.node_type.starts_with("workflow.")
        })
    }

    #[must_use]
    pub fn m4_defaults() -> Self {
        let mut registry = Self::default();
        for manifest in default_manifests() {
            registry
                .register(manifest)
                .expect("default manifests are valid");
        }
        registry
    }

    #[must_use]
    pub fn m5_defaults() -> Self {
        Self::m4_defaults()
    }
}

fn validate_agent_manifest(manifest: &NodeManifestVersion) -> Result<(), String> {
    if manifest.node_type != "agent" {
        return Ok(());
    }
    if manifest.version != 2 {
        return Err("Manifest 3.0 requires Agent node version 2".into());
    }
    let expected = [
        (
            "model",
            ResourceType::Model,
            BindingSlotPlacement::Inspector,
            true,
            false,
        ),
        (
            "workspace_sandbox",
            ResourceType::SandboxProfile,
            BindingSlotPlacement::Inspector,
            false,
            false,
        ),
        (
            "mcp_tools",
            ResourceType::McpTool,
            BindingSlotPlacement::Inspector,
            false,
            true,
        ),
        (
            "skills",
            ResourceType::Skill,
            BindingSlotPlacement::Inspector,
            false,
            true,
        ),
        (
            "knowledge",
            ResourceType::Rag,
            BindingSlotPlacement::Inspector,
            false,
            true,
        ),
        (
            "long_term_memory",
            ResourceType::Memory,
            BindingSlotPlacement::Inspector,
            false,
            false,
        ),
    ];
    if manifest.binding_slots.len() != expected.len() {
        return Err("Agent manifest must declare exactly the six frozen resource slots".into());
    }
    for (name, resource_type, placement, required, multiple) in expected {
        let Some(slot) = manifest.binding_slots.iter().find(|slot| slot.name == name) else {
            return Err(format!(
                "Agent manifest is missing frozen resource slot '{name}'"
            ));
        };
        if slot.resource_type != resource_type
            || slot.placement != placement
            || slot.required != required
            || slot.multiple != multiple
        {
            return Err(format!(
                "Agent resource slot '{name}' does not match its frozen Manifest 3.0 contract"
            ));
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuiltinPackageManifest {
    protocol_version: u32,
    sdk_api_version: u32,
    package_id: String,
    package_version: String,
    display_name: String,
    description: String,
    runtime_entry: String,
    nodes: Vec<String>,
    #[serde(default)]
    trace_renderers: Vec<agentx_node_protocol::PluginTraceRenderer>,
}

fn default_manifests() -> Vec<NodeManifestVersion> {
    let core_nodes = [
        (
            "nodes/agent.json",
            include_str!("../../../plugins/builtin/core/nodes/agent.json"),
        ),
        (
            "nodes/approval.json",
            include_str!("../../../plugins/builtin/core/nodes/approval.json"),
        ),
        (
            "nodes/code.json",
            include_str!("../../../plugins/builtin/core/nodes/code.json"),
        ),
        (
            "nodes/if.json",
            include_str!("../../../plugins/builtin/core/nodes/if.json"),
        ),
        (
            "nodes/loop_over_items.json",
            include_str!("../../../plugins/builtin/core/nodes/loop_over_items.json"),
        ),
        (
            "nodes/merge.json",
            include_str!("../../../plugins/builtin/core/nodes/merge.json"),
        ),
        (
            "nodes/model.json",
            include_str!("../../../plugins/builtin/core/nodes/model.json"),
        ),
        (
            "nodes/sub_workflow.json",
            include_str!("../../../plugins/builtin/core/nodes/sub_workflow.json"),
        ),
        (
            "nodes/mcp_tool.json",
            include_str!("../../../plugins/builtin/core/nodes/mcp_tool.json"),
        ),
        (
            "nodes/skill.json",
            include_str!("../../../plugins/builtin/core/nodes/skill.json"),
        ),
        (
            "nodes/rag.json",
            include_str!("../../../plugins/builtin/core/nodes/rag.json"),
        ),
        (
            "nodes/memory.json",
            include_str!("../../../plugins/builtin/core/nodes/memory.json"),
        ),
        (
            "nodes/reply_message.json",
            include_str!("../../../plugins/builtin/core/nodes/reply_message.json"),
        ),
        (
            "nodes/send_message.json",
            include_str!("../../../plugins/builtin/core/nodes/send_message.json"),
        ),
    ];
    let data_nodes = [
        (
            "nodes/list.json",
            include_str!("../../../plugins/builtin/data/nodes/list.json"),
        ),
        (
            "nodes/set.json",
            include_str!("../../../plugins/builtin/data/nodes/set.json"),
        ),
    ];
    let http_nodes = [(
        "nodes/declarative_http.json",
        include_str!("../../../plugins/builtin/http/nodes/declarative_http.json"),
    )];
    [
        load_builtin_package(
            include_str!("../../../plugins/builtin/core/manifest.json"),
            &core_nodes,
            None,
        ),
        load_builtin_package(
            include_str!("../../../plugins/builtin/data/manifest.json"),
            &data_nodes,
            Some(include_str!(
                "../../../plugins/builtin/data/dist/runtime.js"
            )),
        ),
        load_builtin_package(
            include_str!("../../../plugins/builtin/http/manifest.json"),
            &http_nodes,
            Some(include_str!(
                "../../../plugins/builtin/http/dist/runtime.js"
            )),
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn load_builtin_package(
    package_source: &str,
    nodes: &[(&str, &str)],
    runtime_source: Option<&str>,
) -> Vec<NodeManifestVersion> {
    let package: BuiltinPackageManifest =
        serde_json::from_str(package_source).expect("built-in package Manifest is valid");
    assert_eq!(package.protocol_version, 1, "built-in package protocol");
    assert_eq!(package.sdk_api_version, 2, "built-in package SDK API");
    assert!(!package.display_name.trim().is_empty());
    assert!(!package.description.trim().is_empty());
    assert_eq!(
        package.nodes,
        nodes
            .iter()
            .map(|(path, _)| (*path).to_owned())
            .collect::<Vec<_>>(),
        "built-in package node files must match its Manifest"
    );
    assert_eq!(runtime_source.is_none(), package.runtime_entry == "native");

    let mut package_hasher = Sha256::new();
    package_hasher.update(package_source.as_bytes());
    for (_, source) in nodes {
        package_hasher.update(source.as_bytes());
    }
    if let Some(source) = runtime_source {
        package_hasher.update(source.as_bytes());
    }
    let bundle_digest = format!("sha256:{:x}", package_hasher.finalize());
    let runtime_artifact = runtime_source.map(|source| PluginRuntimeArtifact {
        object_id: plugin_runtime_object_id(&bundle_digest),
        content_hash: format!("sha256:{:x}", Sha256::digest(source.as_bytes())),
        size_bytes: source.len() as u64,
        media_type: "text/javascript".into(),
    });

    nodes
        .iter()
        .map(|(_, source)| {
            let mut manifest: NodeManifestVersion =
                serde_json::from_str(source).expect("built-in node Manifest is valid");
            assert!(
                manifest.plugin.is_none(),
                "package node files cannot embed bindings"
            );
            manifest.plugin = Some(PluginNodeBinding {
                package_id: package.package_id.clone(),
                package_version: package.package_version.clone(),
                bundle_digest: bundle_digest.clone(),
                runtime_entry: package.runtime_entry.clone(),
                runtime_source: runtime_source.unwrap_or_default().into(),
                runtime_artifact: runtime_artifact.clone(),
                ui_entry: None,
                ui_source: None,
                ui_styles: None,
                ui_assets: BTreeMap::new(),
                trace_renderers: package.trace_renderers.clone(),
            });
            manifest
                .validate_plugin()
                .expect("built-in package binding is valid");
            manifest
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn unknown_versions_are_not_coerced() {
        let registry = NodeRegistry::m4_defaults();
        assert!(registry.get("set", 1).is_some());
        assert!(registry.get("set", 2).is_none());

        let mut manifest_v1 = registry.get("set", 1).expect("set manifest").clone();
        manifest_v1.protocol_version = "1.0".into();
        assert!(matches!(
            NodeRegistry::default().register(manifest_v1),
            Err(RegistryError::UnsupportedProtocol { .. })
        ));
    }

    #[test]
    fn every_studio_node_has_an_immutable_builtin_package_identity() {
        let registry = NodeRegistry::m5_defaults();
        for manifest in registry.studio_manifests() {
            let plugin = manifest.plugin.as_ref().expect("studio package identity");
            assert!(matches!(
                plugin.package_id.as_str(),
                "agentx/core" | "agentx/data" | "agentx/http"
            ));
            assert_eq!(plugin.package_version, "1.0.0");
            assert!(plugin.bundle_digest.starts_with("sha256:"));
        }
    }

    #[test]
    fn approval_manifest_declares_decision_output_contracts() {
        let registry = NodeRegistry::m5_defaults();
        let manifest = registry.get("approval", 1).expect("approval manifest");

        let decision = manifest
            .output_port_schemas
            .get("decision")
            .expect("decision port schema");
        assert_eq!(
            decision["required"],
            json!(["taskId", "decision", "decidedBy", "reason", "input"])
        );
        assert!(decision["properties"]["decision"].get("enum").is_none());
        assert_eq!(
            manifest.output_port_schemas["timed_out"]["properties"]["decision"]["enum"],
            json!(["timed_out"])
        );
        assert!(
            manifest
                .output_ports
                .iter()
                .any(|port| port.name == "decision" && port.variadic)
        );
        assert_eq!(
            manifest.output_port_schemas["error"]["properties"]["code"]["type"],
            "string"
        );
    }

    #[test]
    fn specialized_controls_own_bindings_only_at_editable_value_leaves() {
        let registry = NodeRegistry::m5_defaults();
        let if_schema = &registry.get("if", 1).unwrap().parameter_schema;
        assert!(
            if_schema["properties"]["cases"]
                .get("x-agentx-binding")
                .is_none()
        );
        assert_eq!(
            if_schema["properties"]["cases"]["items"]["properties"]["conditions"]["items"]["properties"]
                ["condition"]["properties"]["left"]["x-agentx-binding"]["acceptedKinds"],
            json!(["literal", "reference", "template", "array", "object"])
        );

        let list_schema = &registry.get("list", 1).unwrap().parameter_schema;
        assert!(
            list_schema["properties"]["filter"]
                .get("x-agentx-binding")
                .is_none()
        );
        assert!(
            list_schema["properties"]["sort"]
                .get("x-agentx-binding")
                .is_none()
        );
        assert_eq!(
            list_schema["properties"]["sort"]["items"]["properties"]["selector"]["x-agentx-binding"]
                ["acceptedKinds"],
            json!(["reference"])
        );

        let http_schema = &registry
            .get("declarative_http", 1)
            .unwrap()
            .parameter_schema;
        assert!(
            http_schema["properties"]["query"]
                .get("x-agentx-binding")
                .is_none()
        );
        assert_eq!(
            http_schema["properties"]["query"]["items"]["properties"]["value"]["x-agentx-binding"]
                ["acceptedKinds"],
            json!(["literal", "reference", "template"])
        );
    }

    #[test]
    fn every_error_port_uses_the_standard_error_contract() {
        let registry = NodeRegistry::m5_defaults();
        for manifest in registry.manifests() {
            for port in manifest
                .output_ports
                .iter()
                .filter(|port| port.kind == PortKind::Error)
            {
                let schema = manifest
                    .output_port_schemas
                    .get(&port.name)
                    .expect("error port schema");
                assert_eq!(
                    schema["required"],
                    json!([
                        "code",
                        "message",
                        "retryable",
                        "details",
                        "sourceNodeId",
                        "nodeExecutionId"
                    ]),
                    "{}",
                    manifest.node_type
                );
            }
        }
    }

    #[test]
    fn model_and_agent_publish_only_the_ai_response_contract() {
        let registry = NodeRegistry::m5_defaults();
        for node_type in ["model", "agent"] {
            let version = if node_type == "agent" { 2 } else { 1 };
            let properties = registry.get(node_type, version).unwrap().output_schema["properties"]
                .as_object()
                .unwrap();
            assert_eq!(
                properties.keys().cloned().collect::<BTreeSet<_>>(),
                BTreeSet::from(
                    [
                        "text",
                        "reasoningContent",
                        "structuredOutput",
                        "files",
                        "citations",
                        "usage",
                        "finishReason",
                        "partial"
                    ]
                    .map(str::to_owned)
                )
            );
            for removed in [
                "message",
                "messages",
                "toolCalls",
                "iterations",
                "providerRawResponse",
            ] {
                assert!(!properties.contains_key(removed));
            }
        }
    }

    #[test]
    fn semantic_file_and_citation_outputs_use_strong_contracts() {
        let registry = NodeRegistry::m5_defaults();
        let ai = &registry.get("model", 1).unwrap().output_schema;
        assert_eq!(
            ai["properties"]["files"]["items"]["required"],
            json!([
                "artifactId",
                "fileName",
                "contentType",
                "sizeBytes",
                "sha256"
            ])
        );
        assert_eq!(
            ai["properties"]["citations"]["items"]["required"],
            json!(["sourceId", "text", "metadata"])
        );
        let tool = &registry.get("mcp_tool", 1).unwrap().output_schema;
        assert_eq!(
            tool["properties"]["files"]["items"]["properties"]["artifactId"]["format"],
            "uuid"
        );
    }

    #[test]
    fn ui_fields_are_declared_and_agent_parameters_are_editable() {
        let registry = NodeRegistry::m5_defaults();
        for manifest in registry.manifests() {
            let parameters = manifest.parameter_schema["properties"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            for field in manifest.ui_schema.fields.keys() {
                assert!(
                    parameters.contains_key(field),
                    "{} exposes undeclared UI parameter {field}",
                    manifest.node_type
                );
            }
        }
        let agent = registry.get("agent", 2).expect("Agent v2 manifest");
        for parameter in agent.parameter_schema["properties"]
            .as_object()
            .expect("Agent parameters")
            .keys()
        {
            assert!(
                agent.ui_schema.fields.contains_key(parameter),
                "Agent parameter {parameter} has no UI field"
            );
        }
    }

    #[test]
    fn built_in_localizations_reference_declared_protocol_names() {
        let registry = NodeRegistry::m5_defaults();
        for manifest in registry.manifests() {
            manifest
                .validate_localizations()
                .expect("valid localization");
            assert!(manifest.localizations.contains_key("zh-CN"));
            assert!(manifest.localizations.contains_key("en-US"));
        }

        let branch = registry.get("if", 1).expect("if manifest");
        let english = branch.localizations.get("en-US").expect("English locale");
        assert!(
            english
                .parameter_labels
                .contains_key("cases[].conditions[].condition")
        );
        assert!(english.parameter_descriptions.is_empty());

        let mut invalid = registry.get("if", 1).expect("if manifest").clone();
        invalid
            .localizations
            .get_mut("zh-CN")
            .expect("Chinese localization")
            .output_port_labels
            .insert("missing".into(), "不存在".into());
        assert!(invalid.validate_localizations().is_err());
        let mut target = NodeRegistry::default();
        assert!(matches!(
            target.register(invalid),
            Err(RegistryError::InvalidManifest { .. })
        ));

        let mut invalid_nested = branch.clone();
        invalid_nested
            .localizations
            .get_mut("en-US")
            .expect("English localization")
            .parameter_labels
            .insert("cases[].missing".into(), "Missing".into());
        assert!(invalid_nested.validate_localizations().is_err());
    }

    #[test]
    fn agent_resources_remain_internal_and_studio_catalog_is_exact() {
        let registry = NodeRegistry::m5_defaults();
        assert_eq!(
            registry
                .get("mcp_tool", 1)
                .expect("MCP manifest")
                .capability,
            NodeCapability::McpTool
        );
        assert_eq!(
            registry
                .studio_manifests()
                .map(|manifest| manifest.node_type.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "agent",
                "approval",
                "code",
                "declarative_http",
                "if",
                "list",
                "loop_over_items",
                "reply_message",
                "send_message",
                "merge",
                "model",
                "set",
                "sub_workflow",
            ])
        );
        for resource_only in ["mcp_tool", "skill", "rag", "memory"] {
            assert!(
                registry
                    .studio_manifests()
                    .all(|manifest| manifest.node_type != resource_only),
                "{resource_only} must stay out of the Studio Catalog"
            );
        }
        assert_eq!(
            registry.get("skill", 1).expect("Skill manifest").capability,
            NodeCapability::Skill
        );

        let agent = registry.get("agent", 2).expect("Agent v2 manifest");
        assert_eq!(
            agent.parameter_schema["properties"]["userQuestion"]["type"],
            "string"
        );
        assert!(
            agent.parameter_schema["properties"]
                .get("messages")
                .is_none()
        );
        assert!(
            agent
                .binding_slots
                .iter()
                .any(|slot| slot.name == "mcp_tools"
                    && slot.placement == BindingSlotPlacement::Inspector)
        );
        assert!(
            agent
                .binding_slots
                .iter()
                .any(|slot| slot.name == "skills"
                    && slot.placement == BindingSlotPlacement::Inspector)
        );
        assert!(agent.binding_slots.iter().any(|slot| {
            slot.name == "model"
                && slot.required
                && slot.placement == BindingSlotPlacement::Inspector
        }));
        assert!(agent.binding_slots.iter().any(|slot| {
            slot.name == "workspace_sandbox"
                && !slot.required
                && slot.placement == BindingSlotPlacement::Inspector
        }));
        assert!(registry.get("agent", 1).is_none());

        let mut old_agent = agent.clone();
        old_agent.version = 1;
        assert!(matches!(
            NodeRegistry::default().register(old_agent),
            Err(RegistryError::InvalidManifest { .. })
        ));
        let mut invalid_agent = agent.clone();
        invalid_agent
            .binding_slots
            .iter_mut()
            .find(|slot| slot.name == "model")
            .expect("model slot")
            .placement = BindingSlotPlacement::Canvas;
        assert!(matches!(
            NodeRegistry::default().register(invalid_agent),
            Err(RegistryError::InvalidManifest { .. })
        ));
    }

    #[test]
    fn generated_studio_catalog_fixture_does_not_drift() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/src/features/workflow-designer/testing/studio-catalog.fixture.json");
        let committed: Value = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
        )
        .expect("Studio Catalog fixture is valid JSON");
        let registry = NodeRegistry::m5_defaults();
        let generated = serde_json::to_value(registry.studio_manifests().collect::<Vec<_>>())
            .expect("Studio Catalog serializes");
        assert_eq!(
            committed, generated,
            "Studio Catalog drift: run `cargo run -p agentx-runtime --bin generate-studio-catalog -- src/web/src/features/workflow-designer/testing/studio-catalog.fixture.json`"
        );
    }

    #[test]
    fn merge_manifest_ports_and_modes_are_declared() {
        let registry = NodeRegistry::m5_defaults();
        let merge = registry.get("merge", 1).expect("merge manifest");
        assert_eq!(
            merge
                .input_ports
                .iter()
                .map(|port| port.name.as_str())
                .collect::<Vec<_>>(),
            ["main", "left", "right"]
        );
        assert!(
            merge.parameter_schema["properties"]["mode"]["enum"]
                .as_array()
                .expect("merge modes")
                .iter()
                .any(|value| value == "combine_by_key")
        );
    }
}
