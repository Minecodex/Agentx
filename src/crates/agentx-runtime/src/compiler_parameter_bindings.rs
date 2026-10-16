use agentx_domain::{InputBinding, ValueNamespace};
use serde_json::Value;

pub(super) fn validate_disabled_parameter_shapes(
    definition: &agentx_domain::WorkflowDefinition,
    registry: &crate::NodeRegistry,
    resolved: Option<
        &std::collections::BTreeMap<String, agentx_node_protocol::NodeManifestVersion>,
    >,
    issues: &mut Vec<super::CompileIssue>,
) {
    for (index, node) in definition
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.disabled)
    {
        if let Some(manifest) = resolved
            .and_then(|values| values.get(&node.id))
            .or_else(|| registry.get(&node.node_type, node.type_version))
        {
            super::validate_parameters(index, node, manifest, issues);
            super::validate_parameter_binding_contracts(index, node, manifest, issues);
        }
    }
}

use super::{
    CompileIssue, ParameterBindingContract, collect_binding_selectors, parameter_items_schema,
    parameter_property_schema,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_parameter_binding_value_contract(
    parameter_name: &str,
    path: &str,
    value: &Value,
    schema: Option<&Value>,
    inherited: &ParameterBindingContract,
    issues: &mut Vec<CompileIssue>,
) {
    let contract = parameter_binding_contract(schema, inherited);
    if let Ok(binding) = serde_json::from_value::<InputBinding>(value.clone()) {
        let kind = match &binding {
            InputBinding::Literal { .. } => "literal",
            InputBinding::Reference { .. } => "reference",
            InputBinding::Template { .. } => "template",
            InputBinding::Array { .. } => "array",
            InputBinding::Object { .. } => "object",
        };
        let allowed = contract.accepted_kinds.contains(kind);
        if !allowed {
            issues.push(CompileIssue {
                code: "PARAMETER_BINDING_NOT_ALLOWED".into(),
                path: path.into(),
                message: format!("Parameter '{parameter_name}' does not allow this value source"),
            });
        }
        if matches!(&binding, InputBinding::Literal { value } if value.is_array() || value.is_object())
        {
            issues.push(CompileIssue {
                code: "INPUT_LITERAL_SCALAR_REQUIRED".into(),
                path: path.into(),
                message: "Array and object inputs must use recursive InputBinding nodes".into(),
            });
        }
        let mut selectors = Vec::new();
        collect_binding_selectors(&binding, &mut selectors);
        for selector in selectors {
            let namespace = namespace_name(selector.namespace);
            if !contract.allowed_namespaces.is_empty()
                && !contract.allowed_namespaces.contains(namespace)
            {
                issues.push(CompileIssue {
                    code: "BINDING_NAMESPACE_NOT_ALLOWED".into(),
                    path: path.into(),
                    message: format!(
                        "Parameter '{parameter_name}' cannot reference namespace '{namespace}'"
                    ),
                });
            }
        }
        return;
    }
    if !contract.accepted_kinds.is_empty() {
        issues.push(CompileIssue {
            code: "INPUT_BINDING_REQUIRED".into(),
            path: path.into(),
            message: format!("Parameter '{parameter_name}' requires an InputBinding value"),
        });
        return;
    }
    if value.is_string() {
        return;
    }

    match value {
        Value::Array(values) => {
            let item_schema = schema.and_then(parameter_items_schema);
            for (index, item) in values.iter().enumerate() {
                validate_parameter_binding_value_contract(
                    parameter_name,
                    &format!("{path}[{index}]"),
                    item,
                    item_schema,
                    &contract,
                    issues,
                );
            }
        }
        Value::Object(values) => {
            for (name, child) in values {
                let child_schema =
                    schema.and_then(|schema| parameter_property_schema(schema, name));
                validate_parameter_binding_value_contract(
                    parameter_name,
                    &format!("{path}.{name}"),
                    child,
                    child_schema,
                    &contract,
                    issues,
                );
            }
        }
        _ => {}
    }
}

fn namespace_name(namespace: ValueNamespace) -> &'static str {
    match namespace {
        ValueNamespace::Inputs => "inputs",
        ValueNamespace::Outputs => "outputs",
        ValueNamespace::Contexts => "contexts",
        ValueNamespace::Execution => "execution",
        ValueNamespace::Item => "item",
        ValueNamespace::Loop => "loop",
    }
}

fn parameter_binding_contract(
    schema: Option<&Value>,
    inherited: &ParameterBindingContract,
) -> ParameterBindingContract {
    let binding = schema.and_then(|schema| schema.get("x-agentx-binding"));
    let accepted_kinds = binding
        .and_then(|binding| binding.get("acceptedKinds"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| inherited.accepted_kinds.clone());
    let allowed_namespaces = schema
        .and(binding)
        .and_then(|binding| binding.get("allowedNamespaces"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| inherited.allowed_namespaces.clone());
    ParameterBindingContract {
        accepted_kinds,
        allowed_namespaces,
    }
}
