use super::*;

#[derive(Clone, Copy)]
pub(super) enum ReferenceUsage {
    Parameter,
    LoopOutput,
    ContextWrite,
    EndOutput { required: bool },
    EndErrorOutput { required: bool },
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_reference_paths(
    value: &Value,
    path: &str,
    target: Option<usize>,
    usage: ReferenceUsage,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    graph: &[Vec<usize>],
    exit_error_sources: &BTreeMap<usize, Vec<usize>>,
    issues: &mut Vec<CompileIssue>,
) {
    if value.is_object()
        && let Ok(binding) = serde_json::from_value::<InputBinding>(value.clone())
    {
        let mut selectors = Vec::new();
        collect_binding_selectors(&binding, &mut selectors);
        for selector in selectors {
            if let Some(reference) = structured_selector_reference(selector, nodes) {
                validate_reference_path(
                    &reference,
                    path,
                    target,
                    usage,
                    definition,
                    nodes,
                    manifests,
                    graph,
                    exit_error_sources,
                    issues,
                );
            } else {
                issues.push(CompileIssue {
                    code: "OUTPUT_REFERENCE_NOT_FOUND".into(),
                    path: path.into(),
                    message: "Structured selector references an unknown source node".into(),
                });
            }
        }
        return;
    }
    match value {
        Value::String(_) => {}
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_reference_paths(
                    value,
                    &format!("{path}[{index}]"),
                    target,
                    usage,
                    definition,
                    nodes,
                    manifests,
                    graph,
                    exit_error_sources,
                    issues,
                );
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                validate_reference_paths(
                    value,
                    &format!("{path}.{key}"),
                    target,
                    usage,
                    definition,
                    nodes,
                    manifests,
                    graph,
                    exit_error_sources,
                    issues,
                );
            }
        }
        _ => {}
    }
}

pub(super) fn collect_binding_selectors<'a>(
    value: &'a InputBinding,
    selectors: &mut Vec<&'a ValueSelector>,
) {
    match value {
        InputBinding::Literal { .. } => {}
        InputBinding::Reference { selector, .. } => {
            selectors.push(selector);
        }
        InputBinding::Template { segments } => {
            for segment in segments {
                if let InputTemplateSegment::Reference { selector, .. } = segment {
                    selectors.push(selector);
                }
            }
        }
        InputBinding::Array { items } => {
            for item in items {
                collect_binding_selectors(item, selectors);
            }
        }
        InputBinding::Object { fields } => {
            for field in fields.values() {
                collect_binding_selectors(field, selectors);
            }
        }
    }
}

pub(super) fn structured_selector_reference(
    selector: &ValueSelector,
    nodes: &[(usize, &WorkflowNode)],
) -> Option<Vec<String>> {
    let root = match selector.namespace {
        ValueNamespace::Inputs => "inputs",
        ValueNamespace::Outputs => "outputs",
        ValueNamespace::Contexts => "contexts",
        ValueNamespace::Execution => "execution",
        ValueNamespace::Item => "item",
        ValueNamespace::Loop => "loop",
    };
    let mut reference = vec![root.to_owned()];
    if selector.namespace == ValueNamespace::Outputs {
        let id = selector.source_node_id.as_deref()?;
        let node = nodes.iter().find(|(_, node)| node.id == id)?.1;
        reference.push(node.key.clone());
        if let ValueSelection::Index { index } = selector.run {
            reference.extend([
                "runs".into(),
                index.to_string(),
                selector.port.clone().unwrap_or_else(|| "main".into()),
            ]);
            reference.push(match selector.item {
                ValueSelection::Index { index } => index.to_string(),
                _ => "0".into(),
            });
            reference.push("json".into());
        } else {
            reference.push(selector.port.clone().unwrap_or_else(|| "main".into()));
            match selector.item {
                ValueSelection::Current => reference.extend(["current".into(), "json".into()]),
                ValueSelection::First => reference.extend(["first".into(), "json".into()]),
                ValueSelection::Last => reference.extend(["last".into(), "json".into()]),
                ValueSelection::All => reference.push("all".into()),
                ValueSelection::Index { index } => {
                    reference.extend(["all".into(), index.to_string(), "json".into()])
                }
            }
        }
    } else if selector.namespace == ValueNamespace::Item {
        reference.push("json".into());
    }
    reference.extend(selector.path.iter().map(|segment| match segment {
        ValuePathSegment::Key(key) => key.clone(),
        ValuePathSegment::Index(index) => index.to_string(),
    }));
    Some(reference)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_reference_path(
    reference: &[String],
    path: &str,
    target: Option<usize>,
    usage: ReferenceUsage,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    graph: &[Vec<usize>],
    exit_error_sources: &BTreeMap<usize, Vec<usize>>,
    issues: &mut Vec<CompileIssue>,
) {
    let Some(root) = reference.first().map(String::as_str) else {
        return;
    };
    match root {
        "inputs" => {
            if reference.get(1).is_none()
                || !json_schema_has_path(&definition.start.inputs, &reference[1..])
            {
                reference_issue(issues, "UNKNOWN_INPUT_REFERENCE", path, reference);
            } else if matches!(
                usage,
                ReferenceUsage::EndOutput { .. } | ReferenceUsage::EndErrorOutput { .. }
            ) && json_schema_path_is_sensitive(&definition.start.inputs, &reference[1..])
            {
                reference_issue(issues, "SENSITIVE_INPUT_EXPOSURE", path, reference);
            }
        }
        "contexts" => {
            let Some(name) = reference.get(1) else {
                reference_issue(issues, "UNKNOWN_CONTEXT_REFERENCE", path, reference);
                return;
            };
            let Some(context) = definition.start.contexts.get(name) else {
                reference_issue(issues, "UNKNOWN_CONTEXT_REFERENCE", path, reference);
                return;
            };
            if let Some(target) = target
                && manifests.get(target).is_some_and(|manifest| {
                    manifest
                        .as_ref()
                        .is_some_and(|manifest| !manifest.context_read_capability)
                })
            {
                reference_issue(issues, "CONTEXT_READ_NOT_SUPPORTED", path, reference);
            }
            if context.sensitive
                && matches!(
                    usage,
                    ReferenceUsage::EndOutput { .. } | ReferenceUsage::EndErrorOutput { .. }
                )
            {
                reference_issue(issues, "SENSITIVE_CONTEXT_EXPOSURE", path, reference);
            }
            if reference.len() > 2 && !json_schema_has_path(&context.schema, &reference[2..]) {
                reference_issue(issues, "UNKNOWN_CONTEXT_REFERENCE", path, reference);
            }
        }
        "outputs" => {
            let Some(key) = reference.get(1) else {
                reference_issue(issues, "UNKNOWN_OUTPUT_REFERENCE", path, reference);
                return;
            };
            let Some((source, (_, source_node))) = nodes
                .iter()
                .enumerate()
                .find(|(_, (_, node))| &node.key == key)
            else {
                reference_issue(issues, "UNKNOWN_OUTPUT_REFERENCE", path, reference);
                return;
            };
            if (target == Some(source) && !matches!(usage, ReferenceUsage::ContextWrite))
                || target.is_some_and(|target| !is_reachable(source, target, graph))
            {
                reference_issue(issues, "OUTPUT_NOT_PREDECESSOR", path, reference);
                return;
            }
            if matches!(usage, ReferenceUsage::EndErrorOutput { .. })
                && let Some(exit_index) = target
                && let Some(error_sources) = exit_error_sources.get(&exit_index)
                && !error_sources.is_empty()
                && !error_sources.iter().all(|&error_source| {
                    source == error_source || is_reachable(source, error_source, graph)
                })
            {
                reference_issue(
                    issues,
                    "ERROR_OUTPUT_NOT_COMMON_PREDECESSOR",
                    path,
                    reference,
                );
                return;
            }
            let Some(port) = reference.get(2) else {
                reference_issue(issues, "OUTPUT_PORT_REQUIRED", path, reference);
                return;
            };
            let Some(manifest) = manifests[source].as_ref() else {
                return;
            };
            let output_schema = merged_output_schema(source_node, manifest, port);
            if port == "runs" {
                if !manifest.selector_capabilities.supports_run_selection {
                    reference_issue(issues, "OUTPUT_RUN_SELECTOR_NOT_SUPPORTED", path, reference);
                    return;
                }
                let Some(run_index) = reference.get(3) else {
                    reference_issue(issues, "OUTPUT_RUN_INDEX_REQUIRED", path, reference);
                    return;
                };
                if run_index.parse::<u32>().is_err() {
                    reference_issue(issues, "OUTPUT_RUN_INDEX_INVALID", path, reference);
                    return;
                }
                let Some(run_port) = reference.get(4) else {
                    reference_issue(issues, "OUTPUT_PORT_REQUIRED", path, reference);
                    return;
                };
                if !port_matches(&manifest.output_ports, run_port) {
                    reference_issue(issues, "UNKNOWN_OUTPUT_PORT", path, reference);
                    return;
                }
                let Some(item_index) = reference.get(5) else {
                    reference_issue(issues, "OUTPUT_ITEM_INDEX_REQUIRED", path, reference);
                    return;
                };
                if item_index.parse::<u32>().is_err() {
                    reference_issue(issues, "OUTPUT_ITEM_INDEX_INVALID", path, reference);
                    return;
                }
                if reference.get(6).map(String::as_str) != Some("json") {
                    reference_issue(issues, "OUTPUT_ITEM_JSON_REQUIRED", path, reference);
                    return;
                }
                if reference.len() > 7
                    && output_schema.get("properties").is_some()
                    && !json_schema_has_path(&output_schema, &reference[7..])
                {
                    unknown_output_field_issue(issues, path, reference, manifest, &reference[7..]);
                }
                if output_reference_is_sensitive(&output_schema, reference)
                    && usage_exposes_value(usage)
                {
                    reference_issue(issues, "SENSITIVE_OUTPUT_EXPOSURE", path, reference);
                }
                if matches!(
                    usage,
                    ReferenceUsage::EndOutput { required: true }
                        | ReferenceUsage::EndErrorOutput { required: true }
                ) {
                    reference_issue(issues, "REQUIRED_OUTPUT_MAY_BE_EMPTY", path, reference);
                }
                return;
            }
            if !port_matches(&manifest.output_ports, port) {
                reference_issue(issues, "UNKNOWN_OUTPUT_PORT", path, reference);
                return;
            }
            // Variadic handles (e.g. `case:c1`) inherit the cardinality of the
            // declared variadic port they resolve to.
            let cardinality = manifest
                .output_cardinality
                .get(port)
                .or_else(|| {
                    port.split_once(':').and_then(|(base, _)| {
                        let base = base.to_owned();
                        manifest.output_cardinality.get(&base)
                    })
                })
                .copied()
                .unwrap_or_default();
            let selector = reference.get(3).map(String::as_str);
            if !matches!(selector, Some("current" | "first" | "last" | "all")) {
                reference_issue(issues, "OUTPUT_SELECTOR_REQUIRED", path, reference);
                return;
            }
            if let Some(selector) = selector {
                let supported = match selector {
                    "current" => manifest.selector_capabilities.supports_current,
                    "first" | "last" => manifest.selector_capabilities.supports_first_last,
                    "all" => manifest.selector_capabilities.supports_all,
                    _ => false,
                };
                if !supported {
                    reference_issue(issues, "OUTPUT_SELECTOR_NOT_SUPPORTED", path, reference);
                    return;
                }
                let field_offset = if selector == "all" {
                    if reference.len() == 4 {
                        4
                    } else {
                        let Some(item_index) = reference.get(4) else {
                            return;
                        };
                        if item_index.parse::<u32>().is_err() {
                            reference_issue(issues, "OUTPUT_ITEM_INDEX_INVALID", path, reference);
                            return;
                        }
                        if reference.get(5).map(String::as_str) != Some("json") {
                            reference_issue(issues, "OUTPUT_ITEM_JSON_REQUIRED", path, reference);
                            return;
                        }
                        6
                    }
                } else {
                    if reference.get(4).map(String::as_str) != Some("json") {
                        reference_issue(issues, "OUTPUT_ITEM_JSON_REQUIRED", path, reference);
                        return;
                    }
                    5
                };
                if reference.len() > field_offset
                    && output_schema.get("properties").is_some()
                    && !json_schema_allows_path(&output_schema, &reference[field_offset..])
                {
                    unknown_output_field_issue(
                        issues,
                        path,
                        reference,
                        manifest,
                        &reference[field_offset..],
                    );
                }
                if output_reference_is_sensitive(&output_schema, reference)
                    && usage_exposes_value(usage)
                {
                    reference_issue(issues, "SENSITIVE_OUTPUT_EXPOSURE", path, reference);
                }
                if matches!(
                    usage,
                    ReferenceUsage::EndOutput { required: true }
                        | ReferenceUsage::EndErrorOutput { required: true }
                ) && selector != "all"
                    && matches!(
                        cardinality,
                        OutputCardinality::ZeroOrOne | OutputCardinality::ZeroOrMany
                    )
                {
                    reference_issue(issues, "REQUIRED_OUTPUT_MAY_BE_EMPTY", path, reference);
                }
            }
            if let Some(target) = target
                && !(target == source && matches!(usage, ReferenceUsage::ContextWrite))
                && is_reachable(target, source, graph)
            {
                reference_issue(issues, "EXPRESSION_DEPENDENCY_CYCLE", path, reference);
            }
            let _ = source_node;
        }
        "loop" => {
            if !matches!(
                reference.get(1).map(String::as_str),
                Some("item" | "items" | "index")
            ) {
                reference_issue(issues, "UNKNOWN_LOOP_REFERENCE", path, reference);
                return;
            }
            if matches!(usage, ReferenceUsage::LoopOutput) {
                return;
            }
            let Some(target) = target else {
                reference_issue(issues, "LOOP_REFERENCE_OUTSIDE_ITERATION", path, reference);
                return;
            };
            // `loop.*` is only valid inside the body of the loop container
            // it refers to (parent-descendant, not graph reachability).
            let inside_container = nodes.iter().any(|(_, node)| {
                node.node_type == "loop_over_items"
                    && nodes.iter().any(|(_, candidate)| {
                        candidate.parent_id.as_deref() == Some(node.id.as_str())
                    })
                    && nodes.iter().any(|(_, candidate)| {
                        candidate.parent_id.as_deref() == Some(node.id.as_str())
                            && candidate.id == nodes[target].1.id
                    })
            });
            if !inside_container {
                reference_issue(issues, "LOOP_REFERENCE_OUTSIDE_ITERATION", path, reference);
            }
        }
        "item" if matches!(usage, ReferenceUsage::EndErrorOutput { .. }) => {
            const ERROR_FIELDS: &[&str] = &[
                "code",
                "message",
                "details",
                "sourceNodeId",
                "nodeExecutionId",
                "retryable",
            ];
            if reference.get(1).map(String::as_str) != Some("json")
                || reference
                    .get(2)
                    .is_some_and(|field| !ERROR_FIELDS.contains(&field.as_str()))
            {
                reference_issue(issues, "UNKNOWN_ERROR_ITEM_REFERENCE", path, reference);
            }
        }
        "item" if matches!(usage, ReferenceUsage::EndOutput { .. }) => {
            reference_issue(issues, "ITEM_NOT_AVAILABLE_AT_SUCCESS_END", path, reference);
        }
        _ => {}
    }
}

pub(super) fn usage_exposes_value(usage: ReferenceUsage) -> bool {
    matches!(
        usage,
        ReferenceUsage::EndOutput { .. } | ReferenceUsage::EndErrorOutput { .. }
    )
}

pub(super) fn json_schema_has_path(schema: &Value, path: &[String]) -> bool {
    json_schema_at_path(schema, path).is_some()
}

pub(super) fn json_schema_allows_path(schema: &Value, path: &[String]) -> bool {
    let mut current = schema;
    for segment in path {
        if segment.parse::<usize>().is_ok() {
            let Some(items) = current.get("items") else {
                return current.get("type").and_then(Value::as_str) != Some("array");
            };
            current = items;
            continue;
        }
        if let Some(child) = current
            .get("properties")
            .and_then(Value::as_object)
            .and_then(|properties| properties.get(segment))
        {
            current = child;
            continue;
        }
        match current.get("additionalProperties") {
            Some(Value::Bool(false)) => return false,
            Some(child) if child.is_object() => current = child,
            _ => return true,
        }
    }
    true
}

pub(super) fn json_schema_path_is_sensitive(schema: &Value, path: &[String]) -> bool {
    let mut current = schema;
    if schema_is_sensitive(current) {
        return true;
    }
    for segment in path {
        let Some(next) = json_schema_child(current, segment) else {
            return false;
        };
        current = next;
        if schema_is_sensitive(current) {
            return true;
        }
    }
    false
}

pub(super) fn schema_is_sensitive(schema: &Value) -> bool {
    schema
        .get("sensitive")
        .or_else(|| schema.get("x-sensitive"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub(super) fn output_reference_is_sensitive(output_schema: &Value, reference: &[String]) -> bool {
    let fields = if reference.get(2).map(String::as_str) == Some("runs") {
        reference
            .get(6)
            .is_some_and(|value| value == "json")
            .then_some(&reference[7..])
    } else {
        match reference.get(3).map(String::as_str) {
            Some("all") if reference.get(5).is_some_and(|value| value == "json") => {
                Some(&reference[6..])
            }
            Some("current" | "first" | "last")
                if reference.get(4).is_some_and(|value| value == "json") =>
            {
                Some(&reference[5..])
            }
            _ => None,
        }
    };
    fields.is_some_and(|fields| json_schema_path_is_sensitive(output_schema, fields))
}

pub(super) fn merged_output_schema(
    node: &WorkflowNode,
    manifest: &NodeManifestVersion,
    port: &str,
) -> Value {
    let manifest_port = if port.starts_with("case:") {
        "case"
    } else if port.starts_with("decision:") {
        "decision"
    } else {
        port
    };
    let mut schema = manifest
        .output_port_schemas
        .get(manifest_port)
        .cloned()
        .unwrap_or_else(|| manifest.output_schema.clone());
    let Some(schema_object) = schema.as_object_mut() else {
        return schema;
    };
    if !schema_object.contains_key("properties") {
        schema_object.insert("properties".into(), Value::Object(Default::default()));
    }
    let properties = schema_object
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("object output schema properties");
    if manifest_port == "main"
        && node.node_type == "code"
        && let Some(output_example) = node.parameters.get("outputExample")
    {
        properties.insert("structuredOutput".into(), infer_json_schema(output_example));
    }
    if manifest_port == "main"
        && node.node_type == "model"
        && node.parameters.get("responseMode").and_then(Value::as_str) == Some("json_schema")
        && let Some(output_schema) = node.parameters.get("structuredSchema")
    {
        properties.insert("structuredOutput".into(), output_schema.clone());
    }
    if manifest_port == "main"
        && node.node_type == "await_input"
        && let Some(response_schema) = node.parameters.get("responseSchema")
    {
        properties.insert("content".into(), response_schema.clone());
    }
    if let Some(decision_id) = port.strip_prefix("decision:")
        && let Some(decision) = properties
            .get_mut("decision")
            .and_then(Value::as_object_mut)
    {
        decision.insert("enum".into(), json!([decision_id]));
    }
    schema
}

pub(super) fn effective_output_schema(
    node: &WorkflowNode,
    manifest: &NodeManifestVersion,
    port: &str,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
) -> Value {
    effective_output_schema_inner(
        node,
        manifest,
        port,
        definition,
        nodes,
        manifests,
        &mut BTreeSet::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn effective_output_schema_inner(
    node: &WorkflowNode,
    manifest: &NodeManifestVersion,
    port: &str,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    visiting: &mut BTreeSet<String>,
) -> Value {
    let mut schema = merged_output_schema(node, manifest, port);
    if port != "main" || !visiting.insert(node.id.clone()) {
        return schema;
    }
    if node.node_type == "set" {
        let mut base = if node
            .parameters
            .get("keepOnlySet")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            json!({"type":"object","properties":{},"required":[],"additionalProperties":false})
        } else if let Some(connection) = definition
            .connections
            .iter()
            .find(|connection| connection.target_node_id == node.id)
        {
            if connection.source_node_id == WORKFLOW_START_NODE_ID {
                definition.start.inputs.clone()
            } else if let Some(source) = nodes
                .iter()
                .position(|(_, candidate)| candidate.id == connection.source_node_id)
            {
                effective_output_schema_inner(
                    nodes[source].1,
                    manifests[source]
                        .as_ref()
                        .expect("source manifest validated"),
                    &connection.source_handle,
                    definition,
                    nodes,
                    manifests,
                    visiting,
                )
            } else {
                json!({"type":"object","properties":{}})
            }
        } else {
            json!({"type":"object","properties":{}})
        };
        let properties = base.as_object_mut().and_then(|base| {
            base.entry("properties")
                .or_insert_with(|| json!({}))
                .as_object_mut()
        });
        if let Some(properties) = properties {
            if let Some(InputBinding::Object { fields: values }) = node
                .parameters
                .get("values")
                .cloned()
                .and_then(|value| serde_json::from_value::<InputBinding>(value).ok())
            {
                for (name, binding) in values {
                    if let Some(field_schema) = loop_binding_schema_inner(
                        node, &binding, definition, nodes, manifests, visiting,
                    )
                    .or_else(|| {
                        value_binding_schema_inner(&binding, definition, nodes, manifests, visiting)
                    }) {
                        properties.insert(name, field_schema);
                    }
                }
            }
            if let Some(projected) = schema.get("properties").and_then(Value::as_object) {
                for (name, field_schema) in projected {
                    properties.insert(name.clone(), field_schema.clone());
                }
            }
        }
        visiting.remove(&node.id);
        return base;
    }
    if !matches!(node.node_type.as_str(), "list" | "loop_over_items") {
        visiting.remove(&node.id);
        return schema;
    }
    let binding = node
        .parameters
        .get(if node.node_type == "list" {
            "input"
        } else {
            "outputSelector"
        })
        .cloned()
        .and_then(|value| serde_json::from_value::<InputBinding>(value).ok());
    let inferred = binding.as_ref().and_then(|binding| {
        value_binding_schema_inner(binding, definition, nodes, manifests, visiting)
    });
    let item_schema = if node.node_type == "list" {
        inferred.and_then(|schema| schema.get("items").cloned())
    } else {
        inferred
    };
    if let Some(item_schema) = item_schema
        && let Some(items) = schema.pointer_mut("/properties/items/items")
    {
        *items = item_schema;
    }
    visiting.remove(&node.id);
    schema
}

fn loop_binding_schema_inner(
    target_node: &WorkflowNode,
    binding: &InputBinding,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    visiting: &mut BTreeSet<String>,
) -> Option<Value> {
    let InputBinding::Reference { selector, .. } = binding else {
        return None;
    };
    if selector.namespace != ValueNamespace::Loop {
        return None;
    }
    let ValuePathSegment::Key(root) = selector.path.first()? else {
        return None;
    };
    if root == "index" {
        return (selector.path.len() == 1).then(|| json!({"type":"integer"}));
    }
    if root != "item" && root != "items" {
        return None;
    }
    let parent_id = target_node.parent_id.as_deref()?;
    let loop_index = nodes.iter().position(|(_, candidate)| {
        candidate.id == parent_id && candidate.node_type == "loop_over_items"
    })?;
    let input = nodes[loop_index].1.parameters.get("input")?.clone();
    let input_binding = serde_json::from_value::<InputBinding>(input).ok()?;
    let input_schema =
        value_binding_schema_inner(&input_binding, definition, nodes, manifests, visiting)?;
    let selected_schema = if root == "items" {
        input_schema
    } else {
        input_schema.get("items")?.clone()
    };
    let path = selector.path[1..]
        .iter()
        .map(|segment| match segment {
            ValuePathSegment::Key(key) => key.clone(),
            ValuePathSegment::Index(index) => index.to_string(),
        })
        .collect::<Vec<_>>();
    json_schema_at_path(&selected_schema, &path).cloned()
}

pub(super) fn value_binding_schema(
    binding: &InputBinding,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
) -> Option<Value> {
    value_binding_schema_inner(binding, definition, nodes, manifests, &mut BTreeSet::new())
}

fn value_binding_schema_inner(
    binding: &InputBinding,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    visiting: &mut BTreeSet<String>,
) -> Option<Value> {
    match binding {
        InputBinding::Literal { value } => Some(infer_json_schema(value)),
        InputBinding::Reference { selector, .. } => {
            let path = selector
                .path
                .iter()
                .map(|segment| match segment {
                    ValuePathSegment::Key(key) => key.clone(),
                    ValuePathSegment::Index(index) => index.to_string(),
                })
                .collect::<Vec<_>>();
            let base = match selector.namespace {
                ValueNamespace::Inputs => definition.start.inputs.clone(),
                ValueNamespace::Contexts => {
                    let ValuePathSegment::Key(name) = selector.path.first()? else {
                        return None;
                    };
                    return json_schema_at_path(
                        &definition.start.contexts.get(name)?.schema,
                        &path[1..],
                    )
                    .cloned();
                }
                ValueNamespace::Outputs => {
                    let source_id = selector.source_node_id.as_ref()?;
                    let source = nodes
                        .iter()
                        .position(|(_, candidate)| candidate.id == *source_id)?;
                    effective_output_schema_inner(
                        nodes[source].1,
                        manifests.get(source)?.as_ref()?,
                        selector.port.as_deref().unwrap_or("main"),
                        definition,
                        nodes,
                        manifests,
                        visiting,
                    )
                }
                ValueNamespace::Execution => return execution_selector_schema(&selector.path),
                ValueNamespace::Item => return item_selector_schema(&selector.path),
                ValueNamespace::Loop => {
                    return match selector.path.as_slice() {
                        [ValuePathSegment::Key(name)] if name == "index" => {
                            Some(json!({"type":"integer"}))
                        }
                        _ => None,
                    };
                }
            };
            let selected = json_schema_at_path(&base, &path).cloned()?;
            if matches!(selector.item, ValueSelection::All) {
                Some(json!({"type":"array","items":selected}))
            } else {
                Some(selected)
            }
        }
        InputBinding::Template { .. } => Some(json!({"type":"string"})),
        InputBinding::Array { items } => Some(json!({
            "type":"array",
            "items": infer_binding_items_schema(items, definition, nodes, manifests, visiting)
        })),
        InputBinding::Object { fields } => Some(json!({
            "type":"object",
            "properties": fields.iter().filter_map(|(name, value)| {
                value_binding_schema_inner(value, definition, nodes, manifests, visiting)
                    .map(|schema| (name.clone(), schema))
            }).collect::<serde_json::Map<_,_>>(),
            "required": fields.keys().cloned().collect::<Vec<_>>(),
            "additionalProperties":false
        })),
    }
}

fn infer_binding_items_schema(
    items: &[InputBinding],
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    visiting: &mut BTreeSet<String>,
) -> Value {
    let schemas = items
        .iter()
        .filter_map(|item| value_binding_schema_inner(item, definition, nodes, manifests, visiting))
        .collect::<Vec<_>>();
    let Some(first) = schemas.first() else {
        return json!({});
    };
    if schemas.iter().all(|schema| {
        json_schemas_compatible(first, schema) && json_schemas_compatible(schema, first)
    }) {
        first.clone()
    } else {
        json!({})
    }
}

fn item_selector_schema(path: &[ValuePathSegment]) -> Option<Value> {
    let [ValuePathSegment::Key(name)] = path else {
        return None;
    };
    match name.as_str() {
        "code" | "message" | "sourceNodeId" | "nodeExecutionId" => Some(json!({"type":"string"})),
        "retryable" => Some(json!({"type":"boolean"})),
        "details" => Some(json!({"type":"object"})),
        _ => None,
    }
}

fn execution_selector_schema(path: &[ValuePathSegment]) -> Option<Value> {
    let keys = path
        .iter()
        .map(|segment| match segment {
            ValuePathSegment::Key(value) => Some(value.as_str()),
            ValuePathSegment::Index(_) => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let value_type = match keys.as_slice() {
        ["node", "runIndex" | "itemIndex" | "loopIterationIndex"]
        | ["workflow", "versionNumber"] => "integer",
        [
            "initiator",
            "roles",
            "ids" | "codes" | "names" | "assignments",
        ] => "array",
        ["id" | "startedAt"]
        | ["node", "id" | "executionId"]
        | ["workflow", "id" | "name" | "versionId"]
        | ["workflow", "ownerDepartment", "id" | "name"]
        | ["trigger", "type" | "sourceId" | "name"]
        | ["initiator", "type"]
        | ["initiator", "user", "id" | "name"]
        | ["initiator", "department", "id" | "name"]
        | ["application", "id"]
        | ["invocation", "id"]
        | ["session", "id" | "externalUserId"]
        | ["parentExecutionId"] => "string",
        _ => return None,
    };
    Some(json!({"type":value_type}))
}

pub(super) fn infer_json_schema(value: &Value) -> Value {
    match value {
        Value::Null => json!({}),
        Value::Bool(_) => json!({"type":"boolean"}),
        Value::Number(number)
            if number.is_i64()
                || number.is_u64()
                || number.as_f64().is_some_and(|value| value.fract() == 0.0) =>
        {
            json!({"type":"integer"})
        }
        Value::Number(_) => json!({"type":"number"}),
        Value::String(_) => json!({"type":"string"}),
        Value::Array(values) => json!({"type":"array","items":infer_json_array_items(values)}),
        Value::Object(values) => {
            json!({
                "type":"object",
                "properties":values.iter().map(|(name,value)| (name.clone(),infer_json_schema(value))).collect::<serde_json::Map<_,_>>(),
                "required":values.keys().cloned().collect::<Vec<_>>(),
                "additionalProperties":false
            })
        }
    }
}

fn infer_json_array_items(values: &[Value]) -> Value {
    let schemas = values.iter().map(infer_json_schema).collect::<Vec<_>>();
    let Some(first) = schemas.first() else {
        return json!({});
    };
    if schemas.iter().all(|schema| {
        schema
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "integer" || kind == "number")
    }) {
        if schemas.iter().any(|schema| schema["type"] == "number") {
            json!({"type":"number"})
        } else {
            json!({"type":"integer"})
        }
    } else if schemas.iter().all(|schema| {
        json_schemas_compatible(first, schema) && json_schemas_compatible(schema, first)
    }) {
        first.clone()
    } else {
        json!({})
    }
}

pub(super) fn validate_exit_mapping_contract(
    path: &str,
    binding: &InputBinding,
    contract: &WorkflowOutput,
    definition: &WorkflowDefinition,
    nodes: &[(usize, &WorkflowNode)],
    manifests: &[Option<NodeManifestVersion>],
    issues: &mut Vec<CompileIssue>,
) {
    if contract.schema.get("type").is_none() {
        return;
    }
    let actual = value_binding_schema(binding, definition, nodes, manifests);
    if actual.is_none() {
        issues.push(CompileIssue {
            code: "END_OUTPUT_TYPE_UNKNOWN".into(),
            path: path.into(),
            message: format!(
                "Exit mapping at {path} declares {}, but the bound value has no concrete type",
                contract.schema.get("type").unwrap_or(&Value::Null)
            ),
        });
    } else if actual
        .as_ref()
        .is_some_and(|actual| !json_schemas_compatible(&contract.schema, actual))
    {
        issues.push(CompileIssue {
            code: "END_OUTPUT_TYPE_MISMATCH".into(),
            path: path.into(),
            message: format!(
                "Exit mapping at {path} declares {}, but the bound value is {}",
                contract.schema.get("type").unwrap_or(&Value::Null),
                actual
                    .as_ref()
                    .and_then(|schema| schema.get("type"))
                    .unwrap_or(&Value::Null)
            ),
        });
    }
}

pub(super) fn json_schema_at_path<'a>(schema: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = schema;
    for segment in path {
        current = json_schema_child(current, segment)?;
    }
    Some(current)
}

pub(super) fn json_schema_child<'a>(schema: &'a Value, segment: &str) -> Option<&'a Value> {
    if segment.parse::<usize>().is_ok() {
        return schema.get("items");
    }
    schema
        .get("properties")
        .and_then(|properties| properties.get(segment))
        .or_else(|| {
            schema
                .get("additionalProperties")
                .filter(|additional| additional.is_object())
        })
}

pub(super) fn is_reachable(source: usize, target: usize, graph: &[Vec<usize>]) -> bool {
    let mut seen = vec![false; graph.len()];
    let mut pending = vec![source];
    while let Some(node) = pending.pop() {
        if node == target {
            return true;
        }
        if seen[node] {
            continue;
        }
        seen[node] = true;
        pending.extend(graph[node].iter().copied());
    }
    false
}

pub(super) fn reference_issue(
    issues: &mut Vec<CompileIssue>,
    code: &str,
    path: &str,
    reference: &[String],
) {
    issues.push(CompileIssue { code: code.into(), path: path.into(), message: format!("Invalid structured reference '{}'; references must resolve through the Workflow 5.0 contract", reference.join(".")) });
}

pub(super) fn unknown_output_field_issue(
    issues: &mut Vec<CompileIssue>,
    path: &str,
    reference: &[String],
    manifest: &NodeManifestVersion,
    fields: &[String],
) {
    let missing = fields.first().map(String::as_str).unwrap_or("unknown");
    let message = if matches!(manifest.node_type.as_str(), "model" | "agent")
        && matches!(
            missing,
            "message"
                | "messages"
                | "toolCalls"
                | "iterations"
                | "artifacts"
                | "providerRawResponse"
        ) {
        format!("AI output field '{missing}' no longer exists; reselect the stable 'text' field")
    } else {
        format!("Output field '{missing}' does not exist in the frozen Manifest contract")
    };
    issues.push(CompileIssue {
        code: "UNKNOWN_OUTPUT_FIELD".into(),
        path: path.into(),
        message: format!("{message} (reference '{}')", reference.join(".")),
    });
}
