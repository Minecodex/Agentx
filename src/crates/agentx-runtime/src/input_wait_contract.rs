//! Frozen input-wait contracts. Validation never coerces, supplies defaults or performs I/O.
use agentx_runtime_contracts::GatewayFieldErrorV1;
use serde_json::Value;

pub fn input_wait_validator(schema: &Value) -> Result<jsonschema::Validator, String> {
    jsonschema::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| error.to_string())
}

pub fn validate_input_wait_content(
    schema: &Value,
    content: &Value,
) -> Result<Vec<GatewayFieldErrorV1>, String> {
    let validator = input_wait_validator(schema)?;
    Ok(validator
        .iter_errors(content)
        .take(100)
        .map(|error| {
            let mut field = error.instance_path.to_string();
            let required = match &error.kind {
                jsonschema::error::ValidationErrorKind::Required { property } => {
                    if let Some(name) = property.as_str() {
                        field.push('/');
                        field.push_str(&name.replace('~', "~0").replace('/', "~1"));
                    }
                    true
                }
                _ => false,
            };
            GatewayFieldErrorV1 {
                field,
                code: if required {
                    "REQUIRED"
                } else {
                    "INVALID_VALUE"
                }
                .into(),
                message: error.to_string(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_nested_wait_fields_and_dates_without_modifying_content() {
        let schema = json!({"type":"object","properties":{"date":{"type":"string","format":"date"},"contact":{"type":"object","properties":{"a/b":{"type":"string","minLength":1}},"required":["a/b"]}},"required":["date","contact"],"additionalProperties":false});
        let invalid = json!({"date":"2026-02-30","contact":{}});
        let errors = validate_input_wait_content(&schema, &invalid).unwrap();
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().any(|error| error.field == "/date"));
        assert!(errors.iter().any(|error| error.field == "/contact/a~1b"));
        assert!(
            validate_input_wait_content(
                &schema,
                &json!({"date":"2026-10-31","contact":{"a/b":"Li"}})
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(invalid, json!({"date":"2026-02-30","contact":{}}));
    }

    #[test]
    fn schemas_are_literal_and_remote_references_never_resolve() {
        assert!(input_wait_validator(&json!({"type":"not-a-type"})).is_err());
        assert!(input_wait_validator(&json!({"$ref":"https://example.com/private.json"})).is_err());
        assert!(
            !validate_input_wait_content(&json!(false), &Value::Null)
                .unwrap()
                .is_empty()
        );
        assert!(
            validate_input_wait_content(&json!(true), &Value::Null)
                .unwrap()
                .is_empty()
        );
        let schema = json!({"type":"object","required":["count"],"properties":{"count":{"type":"integer","default":1}}});
        assert!(
            !validate_input_wait_content(&schema, &json!({}))
                .unwrap()
                .is_empty()
        );
        assert!(
            !validate_input_wait_content(&schema, &json!({"count":"1"}))
                .unwrap()
                .is_empty()
        );
    }
}
