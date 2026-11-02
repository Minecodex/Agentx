use jsonschema::ValidationError;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StartInputError {
    #[error("Start input schema is invalid: {0}")]
    InvalidSchema(String),
    #[error("{0}")]
    InvalidInput(String),
}

pub fn materialize_and_validate_start_input(
    input: &mut Value,
    schema: &Value,
) -> Result<(), StartInputError> {
    apply_schema_defaults(input, schema);
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| StartInputError::InvalidSchema(error.to_string()))?;
    validator.validate(input).map_err(validation_error)
}

fn validation_error(error: ValidationError<'_>) -> StartInputError {
    StartInputError::InvalidInput(error.to_string())
}

fn apply_schema_defaults(value: &mut Value, schema: &Value) {
    match (value, schema.get("type").and_then(Value::as_str)) {
        (Value::Object(object), Some("object")) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            for (name, property_schema) in properties {
                if !object.contains_key(name)
                    && let Some(default) = property_schema.get("default")
                {
                    object.insert(name.clone(), default.clone());
                }
                if let Some(child) = object.get_mut(name) {
                    apply_schema_defaults(child, property_schema);
                }
            }
        }
        (Value::Array(items), Some("array")) => {
            if let Some(item_schema) = schema.get("items") {
                for item in items {
                    apply_schema_defaults(item, item_schema);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{StartInputError, materialize_and_validate_start_input};
    use serde_json::json;

    #[test]
    fn applies_nested_defaults_before_validation() {
        let schema = json!({
            "type":"object",
            "properties":{
                "options":{
                    "type":"object",
                    "default":{},
                    "properties":{"limit":{"type":"number","minimum":1,"default":5}}
                }
            },
            "additionalProperties":false
        });
        let mut input = json!({});
        materialize_and_validate_start_input(&mut input, &schema).unwrap();
        assert_eq!(input, json!({"options":{"limit":5}}));
    }

    #[test]
    fn rejects_values_outside_the_declared_contract() {
        let schema = json!({"type":"object","properties":{"name":{"type":"string","minLength":2}},"required":["name"]});
        let mut input = json!({"name":"x"});
        assert!(matches!(
            materialize_and_validate_start_input(&mut input, &schema),
            Err(StartInputError::InvalidInput(_))
        ));
    }
}
