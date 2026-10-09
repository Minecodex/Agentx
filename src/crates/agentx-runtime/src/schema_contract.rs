use agentx_domain::WorkflowDefinition;
use serde_json::Value;

use crate::compiler::CompileIssue;

pub(crate) fn validate_workflow_contract_schemas(
    definition: &WorkflowDefinition,
    issues: &mut Vec<CompileIssue>,
) {
    validate_schema("start.inputs", &definition.start.inputs, issues);
    for (name, context) in &definition.start.contexts {
        validate_schema(
            &format!("start.contexts.{name}.schema"),
            &context.schema,
            issues,
        );
    }
}

fn validate_schema(path: &str, schema: &Value, issues: &mut Vec<CompileIssue>) {
    let validator = match jsonschema::validator_for(schema) {
        Ok(validator) => validator,
        Err(error) => {
            issue(issues, path, "INVALID_JSON_SCHEMA", error.to_string());
            return;
        }
    };
    if let Some(default) = schema.get("default") {
        if let Err(error) = validator.validate(default) {
            issue(
                issues,
                &format!("{path}.default"),
                "INVALID_SCHEMA_DEFAULT",
                error.to_string(),
            );
        }
    }
    validate_range(path, schema, "minimum", "maximum", issues);
    validate_range(path, schema, "minLength", "maxLength", issues);
    validate_range(path, schema, "minItems", "maxItems", issues);
    if schema
        .get("multipleOf")
        .and_then(Value::as_f64)
        .is_some_and(|value| value <= 0.0)
    {
        issue(
            issues,
            &format!("{path}.multipleOf"),
            "INVALID_SCHEMA_CONSTRAINT",
            "multipleOf must be greater than zero",
        );
    }
    for keyword in ["x-agentx-max-size-bytes", "x-agentx-max-total-size-bytes"] {
        if schema
            .get(keyword)
            .and_then(Value::as_u64)
            .is_some_and(|value| value == 0)
        {
            issue(
                issues,
                &format!("{path}.{keyword}"),
                "INVALID_ARTIFACT_CONSTRAINT",
                "Artifact size limits must be greater than zero",
            );
        }
    }
    if schema
        .get("x-agentx-artifact-array")
        .and_then(Value::as_bool)
        == Some(true)
        && schema.get("type").and_then(Value::as_str) != Some("array")
    {
        issue(
            issues,
            path,
            "INVALID_ARTIFACT_SCHEMA",
            "Artifact array fields must use an array schema",
        );
    }
    // plan7 P7-B: modality-marked inputs receive artifact reference arrays from
    // the chat input projection, so the schema must declare them accordingly.
    if let Some(modality) = schema.get("x-agentx-modality").and_then(Value::as_str) {
        if !matches!(modality, "image" | "audio") {
            issue(
                issues,
                path,
                "INVALID_MODALITY_SCHEMA",
                "x-agentx-modality only supports image and audio",
            );
        }
        if schema.get("type").and_then(Value::as_str) != Some("array") {
            issue(
                issues,
                path,
                "INVALID_MODALITY_SCHEMA",
                "Modality-marked fields must use an array schema",
            );
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            validate_schema(&format!("{path}.properties.{name}"), property, issues);
        }
    }
    if let Some(items) = schema.get("items") {
        validate_schema(&format!("{path}.items"), items, issues);
    }
}

fn validate_range(
    path: &str,
    schema: &Value,
    minimum: &str,
    maximum: &str,
    issues: &mut Vec<CompileIssue>,
) {
    let lower = schema.get(minimum).and_then(Value::as_f64);
    let upper = schema.get(maximum).and_then(Value::as_f64);
    if matches!((lower, upper), (Some(lower), Some(upper)) if lower > upper) {
        issue(
            issues,
            path,
            "INVALID_SCHEMA_CONSTRAINT",
            format!("{minimum} cannot exceed {maximum}"),
        );
    }
}

fn issue(issues: &mut Vec<CompileIssue>, path: &str, code: &str, message: impl Into<String>) {
    issues.push(CompileIssue {
        code: code.to_owned(),
        path: path.to_owned(),
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use agentx_domain::WorkflowDefinition;
    use serde_json::json;

    use super::validate_workflow_contract_schemas;

    #[test]
    fn rejects_invalid_ranges_and_defaults_before_publish() {
        let mut definition = WorkflowDefinition::empty();
        definition.start.inputs = json!({
            "type":"object",
            "properties":{
                "name":{"type":"string","minLength":5,"maxLength":2},
                "count":{"type":"number","minimum":1,"default":0}
            }
        });
        let mut issues = Vec::new();
        validate_workflow_contract_schemas(&definition, &mut issues);
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "INVALID_SCHEMA_CONSTRAINT")
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "INVALID_SCHEMA_DEFAULT")
        );
    }

    #[test]
    fn rejects_unsupported_modality_markers_and_non_array_fields() {
        let mut definition = WorkflowDefinition::empty();
        definition.start.inputs = json!({
            "type":"object",
            "properties":{
                "poster":{"type":"string","x-agentx-modality":"image"},
                "clip":{"type":"array","x-agentx-modality":"video"}
            }
        });
        let mut issues = Vec::new();
        validate_workflow_contract_schemas(&definition, &mut issues);
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "INVALID_MODALITY_SCHEMA"
                    && issue.message.contains("array schema"))
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "INVALID_MODALITY_SCHEMA"
                    && issue.message.contains("image and audio"))
        );
    }
}
