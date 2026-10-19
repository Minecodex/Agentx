//! Fixed application test contracts. Data adaptation happens only while authoring.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TestContract {
    pub wait_input_nodes: Vec<agentx_runtime_contracts::WaitInputNodeV1>,
    pub input_schema: Value,
    pub output_schema: Value,
    pub fingerprint: String,
}

impl TestContract {
    pub fn from_definition(definition: &agentx_domain::WorkflowDefinition) -> Self {
        let (input_schema, output_schema) = workflow_schemas(definition);
        let fingerprint = fingerprint(&input_schema, &output_schema);
        Self {
            wait_input_nodes: definition
                .nodes
                .iter()
                .filter(|node| node.node_type == "await_input" && !node.disabled)
                .map(|node| agentx_runtime_contracts::WaitInputNodeV1 {
                    path: agentx_runtime_contracts::node_path("", &node.id),
                    node_id: node.id.clone(),
                    name: node.name.clone(),
                })
                .collect(),
            input_schema,
            output_schema,
            fingerprint,
        }
    }

    pub fn with_wait_nodes(
        mut self,
        nodes: Vec<agentx_runtime_contracts::WaitInputNodeV1>,
    ) -> Self {
        let paths = nodes.iter().map(|node| &node.path).collect::<Vec<_>>();
        self.fingerprint = format!(
            "{:x}",
            Sha256::digest(
                json!({"io":fingerprint(&self.input_schema,&self.output_schema),"waitNodes":paths})
                    .to_string()
                    .as_bytes()
            )
        );
        self.wait_input_nodes = nodes;
        self
    }

    pub fn errors(&self, input: &Value, expected: &Value) -> Vec<CaseFieldError> {
        let mut errors = validate(&self.input_schema, input, "input");
        errors.extend(validate(&self.output_schema, expected, "expectedOutput"));
        errors
    }
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaseFieldError {
    pub field: String,
    pub code: CaseErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum CaseErrorCode {
    Required,
    InvalidValue,
    InvalidContract,
    PendingField,
    ReferenceUnconfirmed,
    ArtifactUnavailable,
}

pub(crate) fn workflow_schemas(definition: &agentx_domain::WorkflowDefinition) -> (Value, Value) {
    let all = definition.end.completion == agentx_domain::WorkflowCompletion::AllComplete;
    let properties = definition.end.outputs.iter().map(|(name, output)| {
        let mut schema = output.schema.clone();
        if let Some(object) = schema.as_object_mut() {
            object.insert("x-agentx-sensitive".into(), json!(output.sensitive));
        }
        if all {
            schema = json!({"type":"array", "items": if output.required { schema } else { json!({"anyOf":[schema,{"type":"null"}]}) }});
        }
        (name.clone(), schema)
    }).collect::<Map<_, _>>();
    let required = definition
        .end
        .outputs
        .iter()
        .filter(|(_, output)| all || output.required)
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    (
        definition.start.inputs.clone(),
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    )
}

pub(crate) fn fingerprint(input: &Value, output: &Value) -> String {
    let canonical = json!({"input":normalize_schema(input),"output":normalize_schema(output)});
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical).expect("JSON serializes"))
    )
}

// Recurse only through Schema-valued keywords. A property or a default object
// containing `title` or `description` is business data, never an annotation.
fn normalize_schema(value: &Value) -> Value {
    let Some(schema) = value.as_object() else {
        return value.clone();
    };
    let mut result = Map::new();
    for (key, value) in schema {
        let normalized = match key.as_str() {
            "title" | "description" | "examples" | "$comment" | "x-agentx-ui-order" => continue,
            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
                value
                    .as_object()
                    .map(|items| {
                        Value::Object(
                            items
                                .iter()
                                .map(|(name, schema)| (name.clone(), normalize_schema(schema)))
                                .collect(),
                        )
                    })
                    .unwrap_or_else(|| value.clone())
            }
            "items"
            | "additionalProperties"
            | "unevaluatedProperties"
            | "propertyNames"
            | "contains"
            | "not"
            | "if"
            | "then"
            | "else" => normalize_schema(value),
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => Value::Array(
                value
                    .as_array()
                    .map(|items| items.iter().map(normalize_schema).collect())
                    .unwrap_or_default(),
            ),
            "required" | "enum" | "type" if value.is_array() => {
                let mut items = value.as_array().expect("array").clone();
                items.sort_by_cached_key(Value::to_string);
                Value::Array(items)
            }
            _ => value.clone(),
        };
        result.insert(key.clone(), normalized);
    }
    Value::Object(result)
}

pub(crate) fn validate(schema: &Value, value: &Value, prefix: &str) -> Vec<CaseFieldError> {
    match jsonschema::validator_for(schema) {
        Ok(validator) => validator
            .iter_errors(value)
            .map(|error| {
                let mut pointer = error.instance_path.to_string();
                if let jsonschema::error::ValidationErrorKind::Required { property } = &error.kind {
                    if let Some(name) = property.as_str() {
                        pointer.push('/');
                        pointer.push_str(&name.replace('~', "~0").replace('/', "~1"));
                    }
                }
                CaseFieldError {
                    field: format!("{prefix}{pointer}"),
                    code: if matches!(
                        error.kind,
                        jsonschema::error::ValidationErrorKind::Required { .. }
                    ) {
                        CaseErrorCode::Required
                    } else {
                        CaseErrorCode::InvalidValue
                    },
                    message: error.to_string(),
                }
            })
            .collect(),
        Err(error) => vec![CaseFieldError {
            field: prefix.into(),
            code: CaseErrorCode::InvalidContract,
            message: format!("Invalid parameter contract: {error}"),
        }],
    }
}

/// Copy explicitly supplied values; never invent defaults, rename or coerce.
pub(crate) fn project(source: &Value, old: &Value, target: &Value) -> Value {
    if normalize_schema(old) == normalize_schema(target) {
        return source.clone();
    }
    let mut output = Map::new();
    for (name, schema) in target
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let Some(value) = source.get(name) else {
            continue;
        };
        let Some(previous) = old
            .get("properties")
            .and_then(|properties| properties.get(name))
        else {
            continue;
        };
        if previous.get("type") != schema.get("type") {
            continue;
        }
        if schema.get("type").and_then(Value::as_str) == Some("object") {
            output.insert(name.clone(), project(value, previous, schema));
        } else if validate(schema, value, "").is_empty() {
            output.insert(name.clone(), value.clone());
        }
    }
    Value::Object(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_ignore_only_schema_annotations_and_unordered_sets() {
        let a = json!({"type":"object","title":"A","required":["title","description"],"properties":{"title":{"type":"string","description":"help"},"description":{"enum":["b","a"],"type":"string"}}});
        let b = json!({"properties":{"description":{"type":"string","enum":["a","b"]},"title":{"type":"string"}},"required":["description","title"],"title":"B","type":"object"});
        assert_eq!(fingerprint(&a, &json!({})), fingerprint(&b, &json!({})));
        let mut changed = b.clone();
        changed["properties"]["description"]["default"] = json!({"title":"business"});
        assert_ne!(
            fingerprint(&a, &json!({})),
            fingerprint(&changed, &json!({}))
        );
        changed["properties"]
            .as_object_mut()
            .unwrap()
            .remove("title");
        assert_ne!(
            fingerprint(&b, &json!({})),
            fingerprint(&changed, &json!({}))
        );
    }

    #[test]
    fn every_validation_change_requires_a_new_dataset() {
        let schema = json!({"type":"object","required":["question"],"properties":{"question":{"type":"string","enum":["a","b"],"default":"a","minLength":1,"x-agentx-max-size-bytes":100}}});
        let mut variants = Vec::new();
        let mut added = schema.clone();
        added["properties"]["optional"] = json!({"type":"string"});
        variants.push(added);
        let mut deleted = schema.clone();
        deleted["properties"]
            .as_object_mut()
            .unwrap()
            .remove("question");
        variants.push(deleted);
        let mut renamed = schema.clone();
        let field = renamed["properties"]
            .as_object_mut()
            .unwrap()
            .remove("question")
            .unwrap();
        renamed["properties"]["query"] = field;
        renamed["required"] = json!(["query"]);
        variants.push(renamed);
        let mut typed = schema.clone();
        typed["properties"]["question"] = json!({"type":"integer"});
        variants.push(typed);
        let mut optional = schema.clone();
        optional["required"] = json!([]);
        variants.push(optional);
        for (key, value) in [
            ("default", json!("b")),
            ("enum", json!(["a", "b", "c"])),
            ("minLength", json!(2)),
            ("x-agentx-max-size-bytes", json!(200)),
            ("x-agentx-content-types", json!(["text/plain"])),
        ] {
            let mut changed = schema.clone();
            changed["properties"]["question"][key] = value;
            variants.push(changed);
        }
        for changed in variants {
            assert_ne!(
                fingerprint(&schema, &json!({})),
                fingerprint(&changed, &json!({})),
                "input {changed}"
            );
            assert_ne!(
                fingerprint(&json!({}), &schema),
                fingerprint(&json!({}), &changed),
                "output {changed}"
            );
        }
    }

    #[test]
    fn copies_only_matching_paths_and_valid_typed_values() {
        let old = json!({"type":"object","properties":{"question":{"type":"string"},"old":{"type":"string"},"number":{"type":"string"},"nested":{"type":"object","properties":{"name":{"type":"string"},"age":{"type":"integer"}}}}});
        let new = json!({"type":"object","properties":{"query":{"type":"string"},"number":{"type":"integer"},"nested":{"type":"object","required":["region"],"properties":{"name":{"type":"string"},"age":{"type":"integer","minimum":18},"region":{"type":"string"}}}}});
        let copied = project(
            &json!({"question":"hello","old":"x","number":"12","nested":{"name":"A","age":5}}),
            &old,
            &new,
        );
        assert_eq!(copied, json!({"nested":{"name":"A"}}));
        assert!(!validate(&new, &copied, "input").is_empty());
    }
}
