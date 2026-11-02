use agentx_domain::WorkflowNode;
use serde_json::Value;

use super::{CompileIssue, compiler_code::validate_code_node};

pub(super) fn validate_node_owned_schemas(
    definition_index: usize,
    node: &WorkflowNode,
    issues: &mut Vec<CompileIssue>,
) {
    if node.node_type == "code" {
        validate_code_node(definition_index, node, issues);
        return;
    }
    if node.node_type == "await_input" {
        if let Some(schema) = node.parameters.get("responseSchema")
            && let Err(message) = crate::input_wait_validator(schema)
        {
            issues.push(CompileIssue {
                code: "WAIT_RESPONSE_SCHEMA_INVALID".into(),
                path: format!("nodes[{definition_index}].parameters.responseSchema"),
                message,
            });
        }
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
