use std::collections::{BTreeMap, BTreeSet, VecDeque};

use agentx_domain::{
    ConditionOperator, ConditionSpec, ContextDefinition, ContextWriteOperation, ExitParameters,
    InputBinding, InputTemplateSegment, MissingValuePolicy, ReferenceBinding, ValueNamespace,
    ValuePathSegment, ValueSelection, ValueSelector, WORKFLOW_EXIT_NODE_TYPE,
    WORKFLOW_START_NODE_ID, WorkflowDefinition, WorkflowNode, WorkflowOutput,
    canonical_content_hash, validate_definition,
};
use agentx_node_protocol::{NodeManifestVersion, OutputCardinality, PortKind};
use agentx_runtime_contracts::{
    CompiledConnection, CompiledExit, CompiledNode, CompiledTerminalConnection, CompiledWorkflow,
    IR_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::NodeRegistry;

#[path = "compiler_agent.rs"]
mod compiler_agent;
#[path = "compiler_code.rs"]
mod compiler_code;
#[path = "compiler_graph.rs"]
mod compiler_graph;
#[path = "compiler_normalization.rs"]
mod normalization;
#[path = "compiler_parameter_bindings.rs"]
mod parameter_bindings;
#[path = "compiler_references.rs"]
mod references;
#[path = "compiler_schema.rs"]
mod schema;
use compiler_agent::{compile_agent_node, validate_binding_slots};
use compiler_code::validate_code_node;
use compiler_graph::{adjacency, strongly_connected_components};
use normalization::{
    normalized_context_writes, normalized_node_parameters, validate_parameter_reference_types,
};
use references::*;
use schema::*;

pub const COMPILER_VERSION: &str = "agentx-workflow-8.0.0";

#[derive(Clone, Debug, Default)]
pub struct CompileContext {
    pub current_workflow_version_id: Option<String>,
    pub ancestor_workflow_version_ids: BTreeSet<String>,
    /// Immutable display/tool names resolved by the publisher for conflict checks.
    pub resource_tool_names: BTreeMap<uuid::Uuid, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileIssue {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Error)]
#[error("workflow compilation failed with {issues_len} issue(s)")]
pub struct CompileError {
    pub issues: Vec<CompileIssue>,
    issues_len: usize,
}

impl CompileError {
    fn new(issues: Vec<CompileIssue>) -> Self {
        let issues_len = issues.len();
        Self { issues, issues_len }
    }
}

pub struct WorkflowCompiler<'a> {
    registry: &'a NodeRegistry,
    resolved_manifests: Option<&'a BTreeMap<String, NodeManifestVersion>>,
}

impl<'a> WorkflowCompiler<'a> {
    #[must_use]
    pub fn new(registry: &'a NodeRegistry) -> Self {
        Self {
            registry,
            resolved_manifests: None,
        }
    }

    /// Uses server-resolved, per-node manifests for dynamic plugin contracts.
    /// The map is keyed by Workflow node id so two instances of the same
    /// plugin version may expose different ports without changing package
    /// identity or mutating the shared registry.
    #[must_use]
    pub fn with_resolved_manifests(
        mut self,
        manifests: &'a BTreeMap<String, NodeManifestVersion>,
    ) -> Self {
        self.resolved_manifests = Some(manifests);
        self
    }

    pub fn compile(
        &self,
        definition: &WorkflowDefinition,
        context: &CompileContext,
    ) -> Result<CompiledWorkflow, CompileError> {
        let mut issues = validate_definition(definition)
            .into_iter()
            .map(|issue| CompileIssue {
                code: issue.code,
                path: issue.path,
                message: issue.message,
            })
            .collect::<Vec<_>>();
        crate::schema_contract::validate_workflow_contract_schemas(definition, &mut issues);
        parameter_bindings::validate_disabled_parameter_shapes(
            definition,
            self.registry,
            self.resolved_manifests,
            &mut issues,
        );
        let enabled = definition
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| !node.disabled && node.node_type != WORKFLOW_EXIT_NODE_TYPE)
            .collect::<Vec<_>>();
        let indexes = enabled
            .iter()
            .enumerate()
            .map(|(compiled, (_, node))| (node.id.as_str(), compiled))
            .collect::<BTreeMap<_, _>>();
        let exits = definition
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.node_type == WORKFLOW_EXIT_NODE_TYPE && !node.disabled)
            .map(|(definition_index, node)| (node.id.as_str(), (definition_index, node)))
            .collect::<BTreeMap<_, _>>();
        let exit_order = definition
            .nodes
            .iter()
            .filter(|node| node.node_type == WORKFLOW_EXIT_NODE_TYPE && !node.disabled)
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();

        let mut manifests: Vec<Option<NodeManifestVersion>> = Vec::with_capacity(enabled.len());
        let mut subworkflows = BTreeSet::new();
        let mut plugin_packages = BTreeMap::<String, (String, String, usize)>::new();
        for (definition_index, node) in &enabled {
            let path = format!("nodes[{definition_index}]");
            let removed_resource_node = matches!(
                node.node_type.as_str(),
                "mcp_tool" | "skill" | "rag" | "memory"
            );
            let public_definition_node = NodeRegistry::is_definition_node_type(&node.node_type);
            let manifest = self
                .resolved_manifests
                .and_then(|manifests| manifests.get(&node.id))
                .cloned()
                .or_else(|| {
                    (public_definition_node && !removed_resource_node)
                        .then(|| {
                            self.registry.resolve_definition_manifest(
                                &node.node_type,
                                node.type_version,
                                &node.parameters,
                            )
                        })
                        .flatten()
                        .cloned()
                });
            if removed_resource_node {
                issues.push(CompileIssue {
                    code: "RESOURCE_CAPABILITY_NODE_REMOVED".into(),
                    path: format!("{path}.type"),
                    message: format!("{} is available only as an Agent resource", node.node_type),
                });
            } else if !public_definition_node {
                issues.push(CompileIssue {
                    code: "NODE_TYPE_NOT_PUBLIC".into(),
                    path: format!("{path}.type"),
                    message: format!("{} is not a public Workflow node type", node.node_type),
                });
            } else if manifest.is_none() {
                issues.push(CompileIssue {
                    code: "UNKNOWN_NODE_VERSION".into(),
                    path: format!("{path}.typeVersion"),
                    message: format!(
                        "Node {}@{} is not registered",
                        node.node_type, node.type_version
                    ),
                });
            }
            if let Some(manifest) = &manifest {
                if let Some(plugin) = &manifest.plugin {
                    match plugin_packages.get(&plugin.package_id) {
                        Some((version, digest, first_index))
                            if version != &plugin.package_version
                                || digest != &plugin.bundle_digest =>
                        {
                            issues.push(CompileIssue {
                                code: "PLUGIN_PACKAGE_VERSION_CONFLICT".into(),
                                path: format!("{path}.typeVersion"),
                                message: format!(
                                    "Plugin package '{}' is already locked by nodes[{first_index}] to {} ({})",
                                    plugin.package_id, version, digest
                                ),
                            });
                        }
                        Some(_) => {}
                        None => {
                            plugin_packages.insert(
                                plugin.package_id.clone(),
                                (
                                    plugin.package_version.clone(),
                                    plugin.bundle_digest.clone(),
                                    *definition_index,
                                ),
                            );
                        }
                    }
                }
                validate_binding_slots(*definition_index, node, manifest, context, &mut issues);
                validate_parameters(*definition_index, node, manifest, &mut issues);
                validate_node_owned_schemas(*definition_index, node, &mut issues);
                validate_dynamic_branch_ids(*definition_index, node, &mut issues);
                validate_context_write_capability(
                    *definition_index,
                    node,
                    manifest,
                    definition,
                    &mut issues,
                );
                validate_composite_context_contract(
                    *definition_index,
                    node,
                    manifest,
                    definition,
                    &mut issues,
                );
            }
            validate_legacy_expressions(
                &node.parameters,
                &format!("{path}.parameters"),
                &mut issues,
            );
            for (write_index, write) in node.context_writes.iter().enumerate() {
                let value =
                    serde_json::to_value(&write.value).expect("dynamic context value serializes");
                validate_legacy_expressions(
                    &value,
                    &format!("{path}.contextWrites[{write_index}].value"),
                    &mut issues,
                );
            }
            if is_subworkflow_type(&node.node_type) {
                match node
                    .parameters
                    .get("workflowVersionId")
                    .and_then(Value::as_str)
                {
                    Some(version) => {
                        if context.current_workflow_version_id.as_deref() == Some(version)
                            || context.ancestor_workflow_version_ids.contains(version)
                        {
                            issues.push(CompileIssue {
                                code: "RECURSIVE_SUBWORKFLOW".into(),
                                path: format!("{path}.parameters.workflowVersionId"),
                                message: "Sub-workflow recursion is not allowed".into(),
                            });
                        }
                        subworkflows.insert(version.to_owned());
                    }
                    None => issues.push(CompileIssue {
                        code: "SUBWORKFLOW_VERSION_REQUIRED".into(),
                        path: format!("{path}.parameters.workflowVersionId"),
                        message: "Sub-workflow must reference an immutable Workflow Version".into(),
                    }),
                }
            }
            manifests.push(manifest);
        }

        validate_containers(&enabled, definition, &mut issues);

        let mut raw_connections = Vec::new();
        let mut raw_start_connections = Vec::new();
        let mut raw_terminal_connections = Vec::new();
        let mut start_to_exit: Option<String> = None;
        let mut terminal_fanouts: BTreeSet<(&str, &str)> = BTreeSet::new();
        for (definition_index, connection) in definition.connections.iter().enumerate() {
            if let (Some(&source_index), Some(&target_index)) = (
                indexes.get(connection.source_node_id.as_str()),
                indexes.get(connection.target_node_id.as_str()),
            ) {
                let source_parent = enabled[source_index].1.parent_id.as_deref();
                let target_parent = enabled[target_index].1.parent_id.as_deref();
                let same_level = source_parent == target_parent;
                if !same_level {
                    issues.push(CompileIssue {
                        code: "CONTAINER_EDGE_CROSSES_BOUNDARY".into(),
                        path: format!("connections[{definition_index}]"),
                        message: format!(
                            "Edge '{}' crosses a loop container boundary: '{}' and '{}' must live at the same level; Loop body entry is derived from parentId and body in-degree",
                            connection.id, connection.source_node_id, connection.target_node_id
                        ),
                    });
                }
            }
            if exits.contains_key(connection.target_node_id.as_str()) {
                let exit_id = connection.target_node_id.as_str();
                if connection.source_node_id == WORKFLOW_START_NODE_ID {
                    if connection.target_handle == "main" {
                        start_to_exit = Some(exit_id.to_owned());
                    }
                    continue;
                }
                let Some(&source) = indexes.get(connection.source_node_id.as_str()) else {
                    continue;
                };
                if !terminal_fanouts.insert((
                    connection.source_node_id.as_str(),
                    connection.source_handle.as_str(),
                )) {
                    issues.push(CompileIssue {
                        code: "DUPLICATE_TERMINAL_FANOUT".into(),
                        path: format!("connections[{definition_index}]"),
                        message: format!(
                            "Node '{}' main/error port may only fan out to a single exit node",
                            connection.source_node_id
                        ),
                    });
                }
                if let Some(manifest) = &manifests[source]
                    && !port_matches(&manifest.output_ports, &connection.source_handle)
                {
                    issues.push(CompileIssue {
                        code: "UNKNOWN_SOURCE_PORT".into(),
                        path: format!("connections[{definition_index}].sourceHandle"),
                        message: format!(
                            "Port '{}' is not declared by {}",
                            connection.source_handle, manifest.node_type
                        ),
                    });
                }
                if !dynamic_output_handle_valid(enabled[source].1, &connection.source_handle) {
                    issues.push(CompileIssue {
                        code: "UNKNOWN_DYNAMIC_SOURCE_PORT".into(),
                        path: format!("connections[{definition_index}].sourceHandle"),
                        message: format!(
                            "Dynamic port '{}' is not declared by this node instance",
                            connection.source_handle
                        ),
                    });
                }
                if let Some(manifest) = &manifests[source]
                    && let Some(kind) = port_kind(&manifest.output_ports, &connection.source_handle)
                    && ((connection.target_handle == "main" && kind != PortKind::Main)
                        || (connection.target_handle == "error" && kind != PortKind::Error))
                {
                    issues.push(CompileIssue {
                        code: "BOUNDARY_PORT_KIND_MISMATCH".into(),
                        path: format!("connections[{definition_index}]"),
                        message:
                            "Exit.main accepts Main output and Exit.error accepts Error output"
                                .into(),
                    });
                }
                raw_terminal_connections.push((connection, source, exit_id.to_owned()));
                continue;
            }
            if connection.source_node_id == WORKFLOW_START_NODE_ID {
                let Some(&target) = indexes.get(connection.target_node_id.as_str()) else {
                    continue;
                };
                if let Some(manifest) = &manifests[target]
                    && !port_matches(&manifest.input_ports, &connection.target_handle)
                {
                    issues.push(CompileIssue {
                        code: "UNKNOWN_TARGET_PORT".into(),
                        path: format!("connections[{definition_index}].targetHandle"),
                        message: format!(
                            "Port '{}' is not declared by {}",
                            connection.target_handle, manifest.node_type
                        ),
                    });
                }
                if !dynamic_input_handle_valid(enabled[target].1, &connection.target_handle) {
                    issues.push(CompileIssue {
                        code: "UNKNOWN_DYNAMIC_TARGET_PORT".into(),
                        path: format!("connections[{definition_index}].targetHandle"),
                        message: "Target port is not available in the selected node mode".into(),
                    });
                }
                raw_start_connections.push((connection, target));
                continue;
            }
            let (Some(&source), Some(&target)) = (
                indexes.get(connection.source_node_id.as_str()),
                indexes.get(connection.target_node_id.as_str()),
            ) else {
                continue;
            };
            if let Some(manifest) = &manifests[source]
                && !port_matches(&manifest.output_ports, &connection.source_handle)
            {
                issues.push(CompileIssue {
                    code: "UNKNOWN_SOURCE_PORT".into(),
                    path: format!("connections[{definition_index}].sourceHandle"),
                    message: format!(
                        "Port '{}' is not declared by {}",
                        connection.source_handle, manifest.node_type
                    ),
                });
            }
            if !dynamic_output_handle_valid(enabled[source].1, &connection.source_handle) {
                issues.push(CompileIssue {
                    code: "UNKNOWN_DYNAMIC_SOURCE_PORT".into(),
                    path: format!("connections[{definition_index}].sourceHandle"),
                    message: format!(
                        "Dynamic port '{}' is not declared by this node instance",
                        connection.source_handle
                    ),
                });
            }
            if let Some(manifest) = &manifests[target]
                && !port_matches(&manifest.input_ports, &connection.target_handle)
            {
                issues.push(CompileIssue {
                    code: "UNKNOWN_TARGET_PORT".into(),
                    path: format!("connections[{definition_index}].targetHandle"),
                    message: format!(
                        "Port '{}' is not declared by {}",
                        connection.target_handle, manifest.node_type
                    ),
                });
            }
            if !dynamic_input_handle_valid(enabled[target].1, &connection.target_handle) {
                issues.push(CompileIssue {
                    code: "UNKNOWN_DYNAMIC_TARGET_PORT".into(),
                    path: format!("connections[{definition_index}].targetHandle"),
                    message: "Target port is not available in the selected node mode".into(),
                });
            }
            if let (Some(source_manifest), Some(target_manifest)) =
                (&manifests[source], &manifests[target])
                && let (Some(source_kind), Some(target_kind)) = (
                    port_kind(&source_manifest.output_ports, &connection.source_handle),
                    port_kind(&target_manifest.input_ports, &connection.target_handle),
                )
                && source_kind != target_kind
            {
                issues.push(CompileIssue {
                    code: "PORT_KIND_MISMATCH".into(),
                    path: format!("connections[{definition_index}]"),
                    message: format!(
                        "Connection {} links incompatible {:?} and {:?} ports",
                        connection.id, source_kind, target_kind
                    ),
                });
            }
            raw_connections.push((connection, source, target, connection.order));
        }

        let graph = adjacency(enabled.len(), &raw_connections);
        let empty_error_sources: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (target, (definition_index, node)) in enabled.iter().enumerate() {
            if let Some(manifest) = manifests[target].as_ref() {
                validate_parameter_binding_contracts(
                    *definition_index,
                    node,
                    manifest,
                    &mut issues,
                );
                validate_condition_contracts(
                    *definition_index,
                    node,
                    definition,
                    &enabled,
                    &manifests,
                    &mut issues,
                );
                validate_parameter_reference_types(
                    &node.parameters,
                    &manifest.parameter_schema,
                    &format!("nodes[{definition_index}].parameters"),
                    definition,
                    &enabled,
                    &manifests,
                    &mut issues,
                );
            }
            let mut reference_parameters = node.parameters.clone();
            let loop_output_selector = if node.node_type == "loop_over_items" {
                reference_parameters
                    .as_object_mut()
                    .and_then(|parameters| parameters.remove("outputSelector"))
            } else {
                None
            };
            validate_reference_paths(
                &reference_parameters,
                &format!("nodes[{definition_index}].parameters"),
                Some(target),
                ReferenceUsage::Parameter,
                definition,
                &enabled,
                &manifests,
                &graph,
                &empty_error_sources,
                &mut issues,
            );
            if let Some(output_selector) = loop_output_selector {
                let mut loop_graph = graph.clone();
                for (child, (_, candidate)) in enabled.iter().enumerate() {
                    if candidate.parent_id.as_deref() == Some(node.id.as_str()) {
                        loop_graph[child].push(target);
                    }
                }
                validate_reference_paths(
                    &output_selector,
                    &format!("nodes[{definition_index}].parameters.outputSelector"),
                    Some(target),
                    ReferenceUsage::LoopOutput,
                    definition,
                    &enabled,
                    &manifests,
                    &loop_graph,
                    &empty_error_sources,
                    &mut issues,
                );
            }
            for (write_index, write) in node.context_writes.iter().enumerate() {
                let value =
                    serde_json::to_value(&write.value).expect("dynamic context value serializes");
                validate_reference_paths(
                    &value,
                    &format!("nodes[{definition_index}].contextWrites[{write_index}].value"),
                    Some(target),
                    ReferenceUsage::ContextWrite,
                    definition,
                    &enabled,
                    &manifests,
                    &graph,
                    &empty_error_sources,
                    &mut issues,
                );
            }
        }
        // Extended graph: enabled nodes plus one virtual node per exit so
        // exit mapping references can be validated against their own
        // predecessor set.
        let exit_indexes: BTreeMap<&str, usize> = exits
            .keys()
            .enumerate()
            .map(|(offset, id)| (*id, enabled.len() + offset))
            .collect();
        let mut reference_graph = graph.clone();
        reference_graph.resize(enabled.len() + exits.len(), Vec::new());
        for (_, source, exit_id) in &raw_terminal_connections {
            if let Some(&exit_index) = exit_indexes.get(exit_id.as_str()) {
                reference_graph[*source].push(exit_index);
            }
        }
        let exit_error_sources: BTreeMap<usize, Vec<usize>> = exit_indexes
            .iter()
            .map(|(exit_id, exit_index)| {
                let sources = definition
                    .connections
                    .iter()
                    .filter(|connection| {
                        connection.target_handle == "error"
                            && connection.target_node_id.as_str() == *exit_id
                    })
                    .filter_map(|connection| {
                        enabled
                            .iter()
                            .position(|(_, node)| node.id == connection.source_node_id)
                    })
                    .collect::<Vec<_>>();
                (*exit_index, sources)
            })
            .collect();
        for (exit_id, (definition_index, node)) in &exits {
            let Some(&exit_index) = exit_indexes.get(*exit_id) else {
                continue;
            };
            let parameters = ExitParameters::parse(&node.parameters).unwrap_or_default();
            for (name, dynamic) in &parameters.outputs {
                let value = serde_json::to_value(dynamic).expect("dynamic exit value serializes");
                let path = format!("nodes[{definition_index}].parameters.outputs.{name}");
                validate_legacy_expressions(&value, &path, &mut issues);
                let required = definition
                    .end
                    .outputs
                    .get(name)
                    .is_some_and(|output| output.required);
                validate_reference_paths(
                    &value,
                    &path,
                    Some(exit_index),
                    ReferenceUsage::EndOutput { required },
                    definition,
                    &enabled,
                    &manifests,
                    &reference_graph,
                    &exit_error_sources,
                    &mut issues,
                );
                if let Some(contract) = definition.end.outputs.get(name) {
                    validate_exit_mapping_contract(
                        &path,
                        dynamic,
                        contract,
                        definition,
                        &enabled,
                        &manifests,
                        &mut issues,
                    );
                }
            }
            for (name, dynamic) in &parameters.error_outputs {
                let value = serde_json::to_value(dynamic).expect("dynamic exit value serializes");
                let path = format!("nodes[{definition_index}].parameters.errorOutputs.{name}");
                validate_legacy_expressions(&value, &path, &mut issues);
                let required = definition
                    .end
                    .error
                    .outputs
                    .get(name)
                    .is_some_and(|output| output.required);
                validate_reference_paths(
                    &value,
                    &path,
                    Some(exit_index),
                    ReferenceUsage::EndErrorOutput { required },
                    definition,
                    &enabled,
                    &manifests,
                    &reference_graph,
                    &exit_error_sources,
                    &mut issues,
                );
                if let Some(contract) = definition.end.error.outputs.get(name) {
                    validate_exit_mapping_contract(
                        &path,
                        dynamic,
                        contract,
                        definition,
                        &enabled,
                        &manifests,
                        &mut issues,
                    );
                }
            }
        }

        let start_nodes = raw_start_connections
            .iter()
            .map(|(_, target)| *target)
            .collect::<BTreeSet<_>>();
        let end_main_nodes = raw_terminal_connections
            .iter()
            .filter(|(connection, _, _)| connection.target_handle == "main")
            .map(|(_, source, _)| *source)
            .collect::<BTreeSet<_>>();
        validate_reachability(
            &enabled,
            &raw_connections,
            &manifests,
            &start_nodes,
            &end_main_nodes,
            start_to_exit.as_deref(),
            &mut issues,
        );
        if !issues.is_empty() {
            return Err(CompileError::new(issues));
        }

        let adjacency = graph;
        let components = strongly_connected_components(&adjacency);
        let mut component_by_node = vec![0; enabled.len()];
        for (component_index, component) in components.iter().enumerate() {
            for node in component {
                component_by_node[*node] = component_index;
            }
        }
        let mut connections = raw_connections
            .iter()
            .enumerate()
            .map(
                |(index, (connection, source, target, branch_order))| CompiledConnection {
                    index,
                    id: connection.id.clone(),
                    source_node: *source,
                    source_port: connection.source_handle.clone(),
                    source_port_kind: port_kind(
                        &manifests[*source]
                            .as_ref()
                            .expect("manifest validated")
                            .output_ports,
                        &connection.source_handle,
                    )
                    .expect("source port validated"),
                    target_node: *target,
                    target_port: connection.target_handle.clone(),
                    target_port_kind: port_kind(
                        &manifests[*target]
                            .as_ref()
                            .expect("manifest validated")
                            .input_ports,
                        &connection.target_handle,
                    )
                    .expect("target port validated"),
                    branch_order: *branch_order,
                    back_edge: component_by_node[*source] == component_by_node[*target]
                        && (components[component_by_node[*source]].len() > 1 || source == target),
                },
            )
            .collect::<Vec<_>>();
        connections.sort_by_key(|edge| (edge.source_node, edge.branch_order, edge.index));
        for (index, connection) in connections.iter_mut().enumerate() {
            connection.index = index;
        }

        let mut nodes = enabled
            .iter()
            .enumerate()
            .map(|(index, (_, node))| {
                let manifest = manifests[index].as_ref().expect("manifests validated");
                CompiledNode {
                    index,
                    id: node.id.clone(),
                    key: node.key.clone(),
                    name: node.name.clone(),
                    node_type: node.node_type.clone(),
                    type_version: node.type_version,
                    parameters: normalized_node_parameters(node, manifest),
                    parameter_schema: manifest.parameter_schema.clone(),
                    context_writes: normalized_context_writes(node, definition),
                    settings: {
                        let mut settings = node.settings.clone();
                        if settings.timeout_ms.is_none() {
                            settings.timeout_ms = manifest.default_timeout_ms;
                        }
                        settings
                    },
                    capability: manifest.capability.clone(),
                    execution_style: manifest.execution_style.clone(),
                    readiness: manifest.readiness.clone(),
                    required_input_ports: manifest
                        .input_ports
                        .iter()
                        .filter(|port| port.required)
                        .map(|port| port.name.clone())
                        .collect(),
                    output_ports: manifest
                        .output_ports
                        .iter()
                        .map(|port| port.name.clone())
                        .collect(),
                    variadic_output_ports: manifest
                        .output_ports
                        .iter()
                        .filter(|port| port.variadic)
                        .map(|port| port.name.clone())
                        .collect(),
                    routes_error: definition.connections.iter().any(|connection| {
                        connection.source_node_id == node.id
                            && connection.source_handle == "error"
                            && !node.disabled
                    }),
                    container: node.parent_id.clone(),
                    loop_body: compiled_loop_body(definition, node, &indexes),
                    effective_output_contract: effective_output_contract(
                        node, manifest, definition, &enabled, &manifests,
                    ),
                    side_effect_level: manifest.side_effect_level.clone(),
                    plugin: manifest.plugin.clone(),
                    agent: compile_agent_node(node, manifest),
                    incoming_connections: Vec::new(),
                    outgoing_connections: Vec::new(),
                    component_index: component_by_node[index],
                }
            })
            .collect::<Vec<_>>();
        for connection in &connections {
            nodes[connection.source_node]
                .outgoing_connections
                .push(connection.index);
            nodes[connection.target_node]
                .incoming_connections
                .push(connection.index);
        }
        let start_nodes = start_nodes.into_iter().collect::<Vec<_>>();
        let terminal_connections = raw_terminal_connections
            .into_iter()
            .map(
                |(connection, source_node, target_exit)| CompiledTerminalConnection {
                    id: connection.id.clone(),
                    source_node,
                    source_port: connection.source_handle.clone(),
                    target_port: connection.target_handle.clone(),
                    target_exit,
                    branch_order: connection.order,
                },
            )
            .collect::<Vec<_>>();
        let exits = exits
            .iter()
            .map(|(exit_id, (_, node))| {
                let parameters = ExitParameters::parse(&node.parameters).unwrap_or_default();
                (
                    (*exit_id).to_owned(),
                    CompiledExit {
                        outputs: parameters.outputs,
                        error_outputs: parameters.error_outputs,
                        protected: node.protected,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let definition_hash = canonical_content_hash(
            &serde_json::to_value(definition).expect("definition serializes"),
        )
        .expect("definition hash serializes");
        let hash_source = serde_json::json!({
            "compilerVersion": COMPILER_VERSION,
            "definitionHash": definition_hash,
            "nodes": &nodes,
            "connections": &connections,
            "terminalConnections": &terminal_connections,
            "exits": &exits,
            "exitOrder": &exit_order,
            "startNodes": &start_nodes,
            "startToExit": &start_to_exit,
            "components": &components,
        });
        let bytes = serde_json::to_vec(&hash_source).expect("compiled workflow serializes");
        let canonical_hash = format!("sha256:ir-v1:{:x}", Sha256::digest(bytes));
        Ok(CompiledWorkflow {
            contract_version: IR_SCHEMA_VERSION,
            schema_version: definition.schema_version.clone(),
            compiler_version: COMPILER_VERSION.into(),
            canonical_hash,
            definition_hash,
            execution_order: definition.settings.execution_order,
            activation_budget: definition.settings.activation_budget,
            start: definition.start.clone(),
            contexts: definition.start.contexts.clone(),
            end: definition.end.clone(),
            exits,
            exit_order,
            nodes,
            connections,
            terminal_connections,
            start_to_exit,
            start_nodes,
            strongly_connected_components: components,
            subworkflow_version_ids: subworkflows.into_iter().collect(),
        })
    }

    /// Draft saving rejects references and security-sensitive configuration
    /// that can never be valid without requiring every publish-time gate.
    pub fn validate_draft(
        &self,
        definition: &WorkflowDefinition,
        context: &CompileContext,
    ) -> Vec<CompileIssue> {
        self.compile(definition, context)
            .err()
            .map(|error| {
                error
                    .issues
                    .into_iter()
                    .filter(|issue| draft_save_issue(&issue.code))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolve the same output contracts used by compilation without executing
    /// nodes or requiring a complete, publishable graph.
    pub fn design_output_contracts(
        &self,
        definition: &WorkflowDefinition,
    ) -> BTreeMap<String, agentx_runtime_contracts::EffectiveOutputContractV1> {
        let nodes = definition
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| !node.disabled)
            .collect::<Vec<_>>();
        let manifests = nodes
            .iter()
            .map(|(_, node)| {
                self.resolved_manifests
                    .and_then(|resolved| resolved.get(&node.id))
                    .cloned()
                    .or_else(|| {
                        self.registry
                            .resolve_definition_manifest(
                                &node.node_type,
                                node.type_version,
                                &node.parameters,
                            )
                            .cloned()
                    })
            })
            .collect::<Vec<_>>();
        nodes
            .iter()
            .zip(&manifests)
            .filter_map(|((_, node), manifest)| {
                manifest.as_ref().map(|manifest| {
                    (
                        node.id.clone(),
                        effective_output_contract(node, manifest, definition, &nodes, &manifests),
                    )
                })
            })
            .collect()
    }
}

fn effective_output_contract(
    node: &WorkflowNode,
    manifest: &NodeManifestVersion,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
) -> agentx_runtime_contracts::EffectiveOutputContractV1 {
    let mut port_schemas = BTreeMap::new();
    let mut cardinalities = BTreeMap::new();
    for port in &manifest.output_ports {
        let instance_ports = match port.name.as_str() {
            "case" if port.variadic => node
                .parameters
                .get("cases")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|branch| branch.get("id").and_then(Value::as_str))
                .map(|id| format!("case:{id}"))
                .collect::<Vec<_>>(),
            "decision" if port.variadic => {
                let configured = node.parameters.get("buttons").and_then(Value::as_array);
                if let Some(buttons) = configured.filter(|buttons| !buttons.is_empty()) {
                    buttons
                        .iter()
                        .filter_map(|button| button.get("id").and_then(Value::as_str))
                        .map(|id| format!("decision:{id}"))
                        .collect()
                } else {
                    vec!["decision:approved".into(), "decision:rejected".into()]
                }
            }
            _ => vec![port.name.clone()],
        };
        for instance_port in instance_ports {
            port_schemas.insert(
                instance_port.clone(),
                effective_output_schema(
                    node,
                    manifest,
                    &instance_port,
                    definition,
                    nodes,
                    manifests,
                ),
            );
            cardinalities.insert(
                instance_port,
                manifest
                    .output_cardinality
                    .get(&port.name)
                    .copied()
                    .unwrap_or_default(),
            );
        }
    }
    agentx_runtime_contracts::EffectiveOutputContractV1 {
        port_schemas,
        cardinalities,
    }
}

fn draft_save_issue(code: &str) -> bool {
    matches!(
        code,
        "UNKNOWN_NODE_VERSION"
            | "BINDING_MULTIPLE_NOT_ALLOWED"
            | "INVALID_NODE_PARAMETERS"
            | "INVALID_PARAMETER_SCHEMA"
            | "RESOURCE_CAPABILITY_NODE_REMOVED"
            | "NODE_TYPE_NOT_PUBLIC"
            | "UNKNOWN_SOURCE_PORT"
            | "UNKNOWN_DYNAMIC_SOURCE_PORT"
            | "UNKNOWN_DYNAMIC_TARGET_PORT"
            | "UNKNOWN_TARGET_PORT"
            | "PARAMETER_BINDING_NOT_ALLOWED"
            | "INPUT_LITERAL_SCALAR_REQUIRED"
            | "INPUT_BINDING_REQUIRED"
            | "BINDING_NAMESPACE_NOT_ALLOWED"
            | "DYNAMIC_BRANCH_ID_INVALID"
            | "CONTAINER_EDGE_CROSSES_BOUNDARY"
            | "CONTAINER_PARENT_UNKNOWN"
            | "CONTAINER_PARENT_NOT_LOOP"
            | "CONTAINER_NESTING_FORBIDDEN"
            | "CONTAINER_BODY_CYCLE"
            | "OUTPUT_REFERENCE_NOT_FOUND"
            | "UNKNOWN_OUTPUT_REFERENCE"
            | "OUTPUT_NOT_PREDECESSOR"
            | "EXPRESSION_DEPENDENCY_CYCLE"
            | "OUTPUT_PORT_REQUIRED"
            | "UNKNOWN_OUTPUT_PORT"
            | "UNKNOWN_OUTPUT_FIELD"
            | "LOOP_REFERENCE_OUTSIDE_ITERATION"
            | "UNKNOWN_LOOP_REFERENCE"
            | "SENSITIVE_INPUT_EXPOSURE"
            | "SENSITIVE_CONTEXT_EXPOSURE"
            | "SENSITIVE_OUTPUT_EXPOSURE"
            | "ERROR_OUTPUT_NOT_COMMON_PREDECESSOR"
            | "CODE_NETWORK_POLICY_INVALID"
            | "CODE_NETWORK_TARGET_FORBIDDEN"
            | "CODE_NETWORK_PORT_RANGE_INVALID"
            | "CODE_OUTPUT_EXAMPLE_REQUIRED"
            | "CODE_OUTPUT_EXAMPLE_OBJECT_REQUIRED"
    )
}

fn validate_parameters(
    definition_index: usize,
    node: &WorkflowNode,
    manifest: &NodeManifestVersion,
    issues: &mut Vec<CompileIssue>,
) {
    let schema = binding_aware_schema(&manifest.parameter_schema, true);
    let validator = match jsonschema::validator_for(&schema) {
        Ok(validator) => validator,
        Err(error) => {
            issues.push(CompileIssue {
                code: "INVALID_PARAMETER_SCHEMA".into(),
                path: format!("nodes[{definition_index}].typeVersion"),
                message: format!("Node Manifest parameter schema is invalid: {error}"),
            });
            return;
        }
    };
    let empty = serde_json::json!({});
    let parameters = if node.parameters.is_null() {
        &empty
    } else {
        &node.parameters
    };
    for error in validator.iter_errors(parameters) {
        let suffix = error.instance_path.to_string();
        // A draft can omit a top-level configuration field. A supplied value must still
        // satisfy its shape, including nested bindings/conditions that inspectors read.
        let missing_configuration = suffix.is_empty()
            && matches!(
                error.kind,
                jsonschema::error::ValidationErrorKind::Required { .. }
            );
        issues.push(CompileIssue {
            code: if missing_configuration {
                "NODE_PARAMETER_REQUIRED"
            } else {
                "INVALID_NODE_PARAMETERS"
            }
            .into(),
            path: format!(
                "nodes[{definition_index}].parameters{}",
                suffix.replace('/', ".")
            ),
            message: error.to_string(),
        });
    }
}

fn validate_node_owned_schemas(
    definition_index: usize,
    node: &WorkflowNode,
    issues: &mut Vec<CompileIssue>,
) {
    if node.node_type == "code" {
        validate_code_node(definition_index, node, issues);
        return;
    }
    let field = match node.node_type.as_str() {
        "model"
            if node.parameters.get("responseMode").and_then(Value::as_str)
                == Some("json_schema") =>
        {
            Some("structuredSchema")
        }
        _ => None,
    };
    let Some(field) = field else { return };
    let Some(schema) = node.parameters.get(field) else {
        issues.push(CompileIssue {
            code: "NODE_OUTPUT_SCHEMA_REQUIRED".into(),
            path: format!("nodes[{definition_index}].parameters.{field}"),
            message: format!("{field} is required for this node configuration"),
        });
        return;
    };
    if let Err(error) = jsonschema::validator_for(schema) {
        issues.push(CompileIssue {
            code: "NODE_OUTPUT_SCHEMA_INVALID".into(),
            path: format!("nodes[{definition_index}].parameters.{field}"),
            message: error.to_string(),
        });
    }
}

fn validate_dynamic_branch_ids(
    definition_index: usize,
    node: &WorkflowNode,
    issues: &mut Vec<CompileIssue>,
) {
    let (field, reserved) = match node.node_type.as_str() {
        "if" => ("cases", &["else", "error"][..]),
        "approval" => ("buttons", &["timed_out", "error"][..]),
        _ => return,
    };
    let mut seen = BTreeSet::new();
    for (index, entry) in node
        .parameters
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let id = entry.get("id").and_then(Value::as_str).unwrap_or_default();
        let message = if id.is_empty() {
            Some(format!(
                "Missing non-empty string at parameters.{field}[{index}].id. Put id inside each {field} array item, not directly under parameters."
            ))
        } else if id.len() > 128
            || !id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            Some(
                "Branch id must contain 1–128 ASCII letters, digits, underscores or hyphens".into(),
            )
        } else if reserved.contains(&id) {
            Some(format!(
                "Branch id '{id}' is reserved; reserved ids: {}",
                reserved.join(", ")
            ))
        } else if !seen.insert(id) {
            Some(format!(
                "Duplicate branch id '{id}'; each {field} item needs a unique id"
            ))
        } else {
            None
        };
        if let Some(message) = message {
            issues.push(CompileIssue {
                code: "DYNAMIC_BRANCH_ID_INVALID".into(),
                path: format!("nodes[{definition_index}].parameters.{field}[{index}].id"),
                message,
            });
        }
    }
}

fn dynamic_output_handle_valid(node: &WorkflowNode, handle: &str) -> bool {
    let (prefix, field) = match node.node_type.as_str() {
        "if" if handle.starts_with("case:") => ("case:", "cases"),
        "approval" if handle.starts_with("decision:") => ("decision:", "buttons"),
        _ => return true,
    };
    let id = handle.trim_start_matches(prefix);
    if node.node_type == "approval" && node.parameters.get("buttons").is_none() {
        return matches!(id, "approved" | "rejected");
    }
    node.parameters
        .get(field)
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
        })
}

fn dynamic_input_handle_valid(node: &WorkflowNode, handle: &str) -> bool {
    if node.node_type != "merge" {
        return true;
    }
    match node
        .parameters
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("append")
    {
        "append" => handle == "main" || handle.starts_with("main:"),
        "combine_by_position" | "combine_by_key" => matches!(handle, "left" | "right"),
        _ => false,
    }
}

fn validate_context_write_capability(
    definition_index: usize,
    node: &WorkflowNode,
    manifest: &NodeManifestVersion,
    definition: &WorkflowDefinition,
    issues: &mut Vec<CompileIssue>,
) {
    if !node.context_writes.is_empty() && !manifest.context_write_capability {
        issues.push(CompileIssue {
            code: "CONTEXT_WRITE_NOT_SUPPORTED".into(),
            path: format!("nodes[{definition_index}].contextWrites"),
            message: format!(
                "Node type '{}' cannot write Workflow Context",
                node.node_type
            ),
        });
    }
    for (write_index, write) in node.context_writes.iter().enumerate() {
        let segments = write.path.split('.').collect::<Vec<_>>();
        let root = segments.first().copied().unwrap_or_default();
        match definition.start.contexts.get(root) {
            None => issues.push(CompileIssue {
                code: "UNKNOWN_CONTEXT_WRITE".into(),
                path: format!("nodes[{definition_index}].contextWrites[{write_index}].path"),
                message: format!("Context '{}' is not declared by Start", root),
            }),
            Some(context) if !context.mutable => issues.push(CompileIssue {
                code: "CONTEXT_READ_ONLY".into(),
                path: format!("nodes[{definition_index}].contextWrites[{write_index}].path"),
                message: format!("Context '{}' is immutable", root),
            }),
            Some(context) => {
                let nested = segments[1..]
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect::<Vec<_>>();
                let Some(target_schema) = json_schema_at_path(&context.schema, &nested) else {
                    issues.push(CompileIssue {
                        code: "UNKNOWN_CONTEXT_WRITE_PATH".into(),
                        path: format!(
                            "nodes[{definition_index}].contextWrites[{write_index}].path"
                        ),
                        message: format!("Context write path '{}' is not declared", write.path),
                    });
                    continue;
                };
                let target_type = target_schema.get("type").and_then(Value::as_str);
                let compatible = match write.operation {
                    ContextWriteOperation::Append => target_type == Some("array"),
                    ContextWriteOperation::MergeObject => target_type == Some("object"),
                    ContextWriteOperation::Increment
                    | ContextWriteOperation::Min
                    | ContextWriteOperation::Max => {
                        matches!(target_type, Some("number" | "integer"))
                    }
                    _ => true,
                };
                if !compatible {
                    issues.push(CompileIssue {
                        code: "CONTEXT_WRITE_OPERATION_TYPE_MISMATCH".into(),
                        path: format!(
                            "nodes[{definition_index}].contextWrites[{write_index}].operation"
                        ),
                        message: format!(
                            "Context write operation is not valid for '{}' ({})",
                            write.path,
                            target_type.unwrap_or("unknown")
                        ),
                    });
                }
            }
        }
    }
}

fn validate_composite_context_contract(
    definition_index: usize,
    node: &WorkflowNode,
    manifest: &NodeManifestVersion,
    definition: &WorkflowDefinition,
    issues: &mut Vec<CompileIssue>,
) {
    if !is_subworkflow_type(&node.node_type) {
        return;
    }
    let Some(contract) = manifest.parameter_schema.get("x-agentx-contextContract") else {
        if node.node_type == "sub_workflow" {
            // The generic creation Manifest is used while the immutable
            // dependency is unavailable; dependency closure validation emits
            // the authoritative missing-version error.
            return;
        }
        issues.push(CompileIssue {
            code: "COMPOSITE_CONTEXT_CONTRACT_MISSING".into(),
            path: format!("nodes[{definition_index}].typeVersion"),
            message: "Composite Manifest does not contain its Context contract".into(),
        });
        return;
    };
    let Ok(child_contexts) =
        serde_json::from_value::<BTreeMap<String, ContextDefinition>>(contract.clone())
    else {
        issues.push(CompileIssue {
            code: "COMPOSITE_CONTEXT_CONTRACT_INVALID".into(),
            path: format!("nodes[{definition_index}].typeVersion"),
            message: "Composite Manifest Context contract is invalid".into(),
        });
        return;
    };
    for (name, child) in child_contexts {
        if !child.mutable {
            continue;
        }
        let Some(parent) = definition.start.contexts.get(&name) else {
            issues.push(CompileIssue {
                code: "COMPOSITE_MUTABLE_CONTEXT_UNDECLARED".into(),
                path: format!("nodes[{definition_index}].typeVersion"),
                message: format!(
                    "Mutable child Context '{name}' must be declared by the parent Workflow"
                ),
            });
            continue;
        };
        if !parent.mutable
            || parent.scope != child.scope
            || parent.merge_policy != child.merge_policy
            || parent.schema != child.schema
        {
            issues.push(CompileIssue {
                code: "COMPOSITE_CONTEXT_CONTRACT_MISMATCH".into(),
                path: format!("nodes[{definition_index}].typeVersion"),
                message: format!(
                    "Parent Context '{name}' must match the mutable child Context schema, scope and merge policy"
                ),
            });
        }
    }
}

fn validate_parameter_binding_contracts(
    definition_index: usize,
    node: &WorkflowNode,
    manifest: &NodeManifestVersion,
    issues: &mut Vec<CompileIssue>,
) {
    let Some(properties) = manifest
        .parameter_schema
        .get("properties")
        .and_then(Value::as_object)
    else {
        return;
    };
    let Some(parameters) = node.parameters.as_object() else {
        return;
    };
    for (name, value) in parameters {
        let Some(schema) = properties.get(name) else {
            continue;
        };
        let path = format!("nodes[{definition_index}].parameters.{name}");
        parameter_bindings::validate_parameter_binding_value_contract(
            name,
            &path,
            value,
            Some(schema),
            &ParameterBindingContract::default(),
            issues,
        );
    }
}

fn validate_condition_contracts(
    definition_index: usize,
    node: &WorkflowNode,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    issues: &mut Vec<CompileIssue>,
) {
    if node.node_type != "if" {
        return;
    }
    let Some(groups) = node.parameters.get("cases").and_then(Value::as_array) else {
        return;
    };
    let mut conditions: Vec<(String, Option<&Value>)> = Vec::new();
    for (group_index, group) in groups.iter().enumerate() {
        for (condition_index, row) in group
            .get("conditions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            conditions.push((
                format!("nodes[{definition_index}].parameters.cases[{group_index}].conditions[{condition_index}].condition"),
                row.get("condition"),
            ));
        }
    }
    for (path, value) in conditions {
        let Some(value) = value else { continue };
        let Ok(condition) = serde_json::from_value::<ConditionSpec>(value.clone()) else {
            continue;
        };
        validate_condition_spec(
            &condition, &path, node, definition, nodes, manifests, issues,
        );
    }
}

fn validate_condition_spec(
    condition: &ConditionSpec,
    path: &str,
    node: &WorkflowNode,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    issues: &mut Vec<CompileIssue>,
) {
    let left_schema = condition_operand_schema(&condition.left, node, definition, nodes, manifests);
    let Some(left_schema) = left_schema else {
        issues.push(CompileIssue {
            code: "CONDITION_OPERAND_TYPE_UNKNOWN".into(),
            path: format!("{path}.left"),
            message: "Condition left value must have a declared schema".into(),
        });
        return;
    };
    let left_type = schema_types(&left_schema)
        .into_iter()
        .find(|value| *value != "null")
        .unwrap_or("unknown");
    let allowed = match left_type {
        "string" => matches!(
            condition.operator,
            ConditionOperator::Eq
                | ConditionOperator::Ne
                | ConditionOperator::Contains
                | ConditionOperator::NotContains
                | ConditionOperator::StartsWith
                | ConditionOperator::EndsWith
                | ConditionOperator::Matches
                | ConditionOperator::IsEmpty
                | ConditionOperator::IsNotEmpty
        ),
        "number" | "integer" => matches!(
            condition.operator,
            ConditionOperator::Eq
                | ConditionOperator::Ne
                | ConditionOperator::Gt
                | ConditionOperator::Gte
                | ConditionOperator::Lt
                | ConditionOperator::Lte
                | ConditionOperator::IsEmpty
                | ConditionOperator::IsNotEmpty
        ),
        "boolean" => matches!(
            condition.operator,
            ConditionOperator::Eq | ConditionOperator::Ne
        ),
        "array" => matches!(
            condition.operator,
            ConditionOperator::Contains
                | ConditionOperator::NotContains
                | ConditionOperator::IsEmpty
                | ConditionOperator::IsNotEmpty
        ),
        "object" => matches!(
            condition.operator,
            ConditionOperator::IsEmpty | ConditionOperator::IsNotEmpty
        ),
        _ => matches!(
            condition.operator,
            ConditionOperator::Eq
                | ConditionOperator::Ne
                | ConditionOperator::IsEmpty
                | ConditionOperator::IsNotEmpty
        ),
    };
    if !allowed {
        issues.push(CompileIssue {
            code: "CONDITION_OPERATOR_TYPE_MISMATCH".into(),
            path: format!("{path}.operator"),
            message: format!(
                "Condition operator {:?} is not valid for {left_type}",
                condition.operator
            ),
        });
        return;
    }
    if matches!(
        condition.operator,
        ConditionOperator::IsEmpty | ConditionOperator::IsNotEmpty
    ) {
        return;
    }
    let Some(right) = condition.right.as_ref() else {
        issues.push(CompileIssue {
            code: "CONDITION_RIGHT_OPERAND_REQUIRED".into(),
            path: format!("{path}.right"),
            message: "Condition operator requires a right value".into(),
        });
        return;
    };
    let Some(right_schema) = condition_operand_schema(right, node, definition, nodes, manifests)
    else {
        issues.push(CompileIssue {
            code: "CONDITION_OPERAND_TYPE_UNKNOWN".into(),
            path: format!("{path}.right"),
            message: "Condition right value must have a declared schema".into(),
        });
        return;
    };
    let compatible = match condition.operator {
        ConditionOperator::Contains | ConditionOperator::NotContains if left_type == "array" => {
            left_schema
                .get("items")
                .is_none_or(|items| json_schemas_compatible(items, &right_schema))
        }
        _ => {
            json_schemas_compatible(&left_schema, &right_schema)
                || schema_types(&left_schema).is_empty()
                || schema_types(&right_schema).is_empty()
                || schema_types(&left_schema).contains(&"string")
                || schema_types(&right_schema).contains(&"string")
        }
    };
    if !compatible {
        issues.push(CompileIssue {
            code: "CONDITION_OPERAND_TYPE_MISMATCH".into(),
            path: format!("{path}.right"),
            message: "Condition operands have incompatible schemas".into(),
        });
    }
}

fn condition_operand_schema(
    binding: &InputBinding,
    node: &WorkflowNode,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
) -> Option<Value> {
    let InputBinding::Reference { selector, .. } = binding else {
        return references::value_binding_schema(binding, definition, nodes, manifests);
    };
    if selector.namespace != ValueNamespace::Item {
        return references::value_binding_schema(binding, definition, nodes, manifests);
    }
    let path = selector
        .path
        .iter()
        .map(|segment| match segment {
            ValuePathSegment::Key(value) => value.clone(),
            ValuePathSegment::Index(value) => value.to_string(),
        })
        .collect::<Vec<_>>();
    let mut selected = None;
    for connection in definition
        .connections
        .iter()
        .filter(|connection| connection.target_node_id == node.id)
    {
        let base = source_item_schema(
            &connection.source_node_id,
            &connection.source_handle,
            definition,
            nodes,
            manifests,
            &mut BTreeSet::new(),
        )?;
        let candidate = references::json_schema_at_path(&base, &path).cloned()?;
        if selected.as_ref().is_some_and(|current| {
            !json_schemas_compatible(current, &candidate)
                || !json_schemas_compatible(&candidate, current)
        }) {
            return None;
        }
        selected = Some(candidate);
    }
    selected
}

fn source_item_schema(
    source_id: &str,
    source_port: &str,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    visiting: &mut BTreeSet<String>,
) -> Option<Value> {
    if source_id == WORKFLOW_START_NODE_ID {
        return Some(definition.start.inputs.clone());
    }
    if !visiting.insert(source_id.to_owned()) {
        return None;
    }
    let source = nodes
        .iter()
        .position(|(_, candidate)| candidate.id == source_id)?;
    let node = nodes[source].1;
    let schema = references::effective_output_schema(
        node,
        manifests.get(source)?.as_ref()?,
        source_port,
        definition,
        nodes,
        manifests,
    );
    visiting.remove(source_id);
    Some(schema)
}

fn compiled_loop_body(
    definition: &WorkflowDefinition,
    node: &WorkflowNode,
    indexes: &BTreeMap<&str, usize>,
) -> Option<agentx_runtime_contracts::CompiledLoopBodyV1> {
    if node.node_type != "loop_over_items" {
        return None;
    }
    let children = definition
        .nodes
        .iter()
        .filter(|candidate| candidate.parent_id.as_deref() == Some(node.id.as_str()))
        .collect::<Vec<_>>();
    if children.is_empty() {
        return None;
    }
    let child_ids = children
        .iter()
        .map(|child| child.id.as_str())
        .collect::<BTreeSet<_>>();
    let entries = definition
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, candidate)| child_ids.contains(candidate.id.as_str()))
        .filter(|(_, candidate)| {
            !definition.connections.iter().any(|connection| {
                child_ids.contains(connection.source_node_id.as_str())
                    && connection.target_node_id == candidate.id
            })
        })
        .filter_map(|(definition_index, _)| enabled_index(indexes, definition, definition_index))
        .collect::<Vec<_>>();
    let sinks = definition
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.parent_id.as_deref() == Some(node.id.as_str()))
        .filter(|(_, candidate)| {
            !definition.connections.iter().any(|connection| {
                connection.source_node_id == candidate.id
                    && child_ids.contains(connection.target_node_id.as_str())
            })
        })
        .filter_map(|(definition_index, _)| enabled_index(indexes, definition, definition_index))
        .collect::<Vec<_>>();
    let error_mode = node
        .parameters
        .get("errorMode")
        .and_then(Value::as_str)
        .unwrap_or("terminate")
        .to_owned();
    let parallelism = node
        .parameters
        .get("parallelism")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(1);
    let output_selector = node
        .parameters
        .get("outputSelector")
        .cloned()
        .and_then(|value| serde_json::from_value::<ReferenceBinding>(value).ok())?;
    Some(agentx_runtime_contracts::CompiledLoopBodyV1 {
        entries,
        sinks,
        output_selector,
        parallelism,
        error_mode,
    })
}

fn enabled_index(
    indexes: &BTreeMap<&str, usize>,
    definition: &WorkflowDefinition,
    definition_index: usize,
) -> Option<usize> {
    let id = definition
        .nodes
        .get(definition_index)?
        .id
        .as_str()
        .to_owned();
    indexes.get(id.as_str()).copied()
}

fn validate_containers(
    nodes: &[(usize, &agentx_domain::WorkflowNode)],
    definition: &WorkflowDefinition,
    issues: &mut Vec<CompileIssue>,
) {
    let by_id = nodes
        .iter()
        .map(|(definition_index, node)| (node.id.as_str(), (definition_index, node)))
        .collect::<BTreeMap<_, _>>();
    for (definition_index, node) in nodes {
        let Some(parent_id) = node.parent_id.as_deref() else {
            continue;
        };
        let Some(&(parent_index, parent)) = by_id.get(parent_id) else {
            issues.push(CompileIssue {
                code: "CONTAINER_PARENT_UNKNOWN".into(),
                path: format!("nodes[{definition_index}].parentId"),
                message: format!("Container parent '{parent_id}' does not exist"),
            });
            continue;
        };
        if parent.node_type != "loop_over_items" {
            issues.push(CompileIssue {
                code: "CONTAINER_PARENT_NOT_LOOP".into(),
                path: format!("nodes[{definition_index}].parentId"),
                message: format!(
                    "Only loop_over_items nodes can contain children, got '{}'",
                    parent.node_type
                ),
            });
        }
        if parent.parent_id.is_some() {
            issues.push(CompileIssue {
                code: "CONTAINER_NESTING_FORBIDDEN".into(),
                path: format!("nodes[{parent_index}].parentId"),
                message: "Loop containers cannot be nested".into(),
            });
        }
    }
    // Body sub-DAGs must be acyclic: iterations converge through the
    // container node, never through a cycle inside the body.
    let all: BTreeMap<&str, &agentx_domain::WorkflowNode> = nodes
        .iter()
        .map(|(_, node)| (node.id.as_str(), *node))
        .collect();
    for (container_id, container) in &all {
        if container.node_type != "loop_over_items" {
            continue;
        }
        let body: BTreeMap<&str, &agentx_domain::WorkflowNode> = all
            .iter()
            .filter(|(_, node)| node.parent_id.as_deref() == Some(*container_id))
            .map(|(id, node)| (*id, *node))
            .collect();
        let container_definition_index = nodes
            .iter()
            .find(|(_, candidate)| candidate.id.as_str() == *container_id)
            .map(|(index, _)| *index)
            .unwrap_or_default();
        if body.is_empty() {
            issues.push(CompileIssue {
                code: "LOOP_BODY_EMPTY".into(),
                path: format!("nodes[{container_definition_index}].parentId"),
                message: format!("Loop container '{container_id}' must contain at least one node"),
            });
        }
        let entry_count = body
            .keys()
            .filter(|body_id| {
                !definition.connections.iter().any(|connection| {
                    body.contains_key(connection.source_node_id.as_str())
                        && connection.target_node_id.as_str() == **body_id
                })
            })
            .count();
        if !body.is_empty() && entry_count == 0 {
            issues.push(CompileIssue {
                code: "LOOP_BODY_ENTRY_REQUIRED".into(),
                path: format!("nodes[{container_definition_index}].parentId"),
                message: format!("Loop container '{container_id}' has no acyclic body entry"),
            });
        }
        if node_uses_omit(
            container
                .parameters
                .get("outputSelector")
                .and_then(|value| serde_json::from_value::<ReferenceBinding>(value.clone()).ok())
                .as_ref(),
        ) {
            issues.push(CompileIssue {
                code: "LOOP_OUTPUT_SELECTOR_OMIT_FORBIDDEN".into(),
                path: format!("nodes[{container_definition_index}].parameters.outputSelector"),
                message: "Loop outputSelector must produce one value per successful iteration; omit is not allowed".into(),
            });
        }
        let mut visiting = BTreeSet::new();
        let mut done = BTreeSet::new();
        fn walk<'a>(
            current: &'a str,
            body: &BTreeMap<&'a str, &agentx_domain::WorkflowNode>,
            definition: &'a agentx_domain::WorkflowDefinition,
            visiting: &mut BTreeSet<&'a str>,
            done: &mut BTreeSet<&'a str>,
        ) -> bool {
            if done.contains(current) {
                return false;
            }
            if !visiting.insert(current) {
                return true;
            }
            let cycle = definition.connections.iter().any(|connection| {
                connection.source_node_id == current
                    && body.contains_key(connection.target_node_id.as_str())
                    && walk(
                        connection.target_node_id.as_str(),
                        body,
                        definition,
                        visiting,
                        done,
                    )
            });
            visiting.remove(current);
            done.insert(current);
            cycle
        }
        for body_id in body.keys() {
            if walk(body_id, &body, definition, &mut visiting, &mut done) {
                issues.push(CompileIssue {
                    code: "CONTAINER_BODY_CYCLE".into(),
                    path: "connections".into(),
                    message: format!(
                        "The body of loop container '{container_id}' must stay acyclic"
                    ),
                });
                break;
            }
        }
    }
}

fn node_uses_omit(dynamic: Option<&ReferenceBinding>) -> bool {
    matches!(
        dynamic,
        Some(ReferenceBinding::Reference {
            missing_policy: MissingValuePolicy::Omit,
            ..
        })
    )
}

fn validate_reachability(
    nodes: &[(usize, &agentx_domain::WorkflowNode)],
    connections: &[(&agentx_domain::WorkflowConnection, usize, usize, u32)],
    manifests: &[Option<NodeManifestVersion>],
    start_nodes: &BTreeSet<usize>,
    end_main_nodes: &BTreeSet<usize>,
    start_to_exit: Option<&str>,
    issues: &mut Vec<CompileIssue>,
) {
    let mut adjacency = adjacency(nodes.len(), connections);
    for (container_index, (_, container)) in nodes.iter().enumerate() {
        if container.node_type != "loop_over_items" {
            continue;
        }
        let child_indexes = nodes
            .iter()
            .enumerate()
            .filter(|(_, (_, candidate))| {
                candidate.parent_id.as_deref() == Some(container.id.as_str())
            })
            .map(|(index, _)| index)
            .collect::<BTreeSet<_>>();
        for &candidate in &child_indexes {
            let has_body_predecessor = connections.iter().any(|(_, source, target, _)| {
                *target == candidate && child_indexes.contains(source)
            });
            if !has_body_predecessor {
                adjacency[container_index].push(candidate);
            }
        }
    }
    if start_nodes.is_empty() && start_to_exit.is_none() {
        issues.push(CompileIssue {
            code: "START_REQUIRED".into(),
            path: "connections".into(),
            message: "Start.main must have an explicit connection".into(),
        });
    }
    if end_main_nodes.is_empty() && start_to_exit.is_none() {
        issues.push(CompileIssue {
            code: "END_MAIN_REQUIRED".into(),
            path: "connections".into(),
            message: "At least one exit node must receive a main connection".into(),
        });
    }
    let mut reachable = BTreeSet::new();
    let mut queue = start_nodes.iter().copied().collect::<VecDeque<_>>();
    while let Some(node) = queue.pop_front() {
        if reachable.insert(node) {
            queue.extend(adjacency[node].iter().copied());
        }
    }
    for (compiled_index, (definition_index, node)) in nodes.iter().enumerate() {
        if !reachable.contains(&compiled_index) {
            issues.push(CompileIssue {
                code: "UNREACHABLE_NODE".into(),
                path: format!("nodes[{definition_index}]"),
                message: format!("Enabled node '{}' is unreachable", node.id),
            });
        }
    }
    let mut reverse = vec![Vec::new(); nodes.len()];
    for (connection, source, target, _) in connections {
        if connection.source_handle == "error" {
            continue;
        }
        reverse[*target].push(*source);
    }
    // Container body sinks converge through their container: a loop child
    // implicitly flows into the loop node, so it can reach an exit whenever
    // the container itself does (the machine aggregates sinks per iteration).
    for (compiled_index, (_, node)) in nodes.iter().enumerate() {
        let Some(container_id) = node.parent_id.as_deref() else {
            continue;
        };
        if let Some((container_index, _)) = nodes
            .iter()
            .enumerate()
            .find(|(_, (_, candidate))| candidate.id == container_id)
        {
            reverse[container_index].push(compiled_index);
        }
    }
    let mut reaches_end = BTreeSet::new();
    let mut queue = end_main_nodes.iter().copied().collect::<VecDeque<_>>();
    while let Some(node) = queue.pop_front() {
        if reaches_end.insert(node) {
            queue.extend(reverse[node].iter().copied());
        }
    }
    for (compiled_index, (definition_index, node)) in nodes.iter().enumerate() {
        let has_main_output = manifests
            .get(compiled_index)
            .and_then(Option::as_ref)
            .is_some_and(|manifest| {
                manifest
                    .output_ports
                    .iter()
                    .any(|port| port.kind == PortKind::Main)
            });
        if reachable.contains(&compiled_index)
            && has_main_output
            && !reaches_end.contains(&compiled_index)
        {
            issues.push(CompileIssue {
                code: "MAIN_PATH_DOES_NOT_REACH_END".into(),
                path: format!("nodes[{definition_index}]"),
                message: format!("Enabled node '{}' cannot reach an exit node", node.id),
            });
        }
    }
}

#[cfg(test)]
#[path = "compiler_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "compiler_knowledge_tests.rs"]
mod knowledge_tests;
