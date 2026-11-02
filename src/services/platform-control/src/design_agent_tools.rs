use agentx_agent_core::{ReplayPolicyV1, ToolDefinitionV1, ToolOriginV1};
use schemars::JsonSchema;
use serde_json::{Value, json};

pub(crate) fn tool<T: JsonSchema>(name: &str, description: &str) -> ToolDefinitionV1 {
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    expose_discriminated_properties(&mut schema);
    ToolDefinitionV1 {
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        origin: ToolOriginV1::Platform,
        replay_policy: ReplayPolicyV1::IdempotencyRequired,
    }
}

// Providers inspect top-level properties to guide tool generation. Retain the
// closed union for validation while exposing every discriminator and field.
fn expose_discriminated_properties(schema: &mut Value) {
    match schema {
        Value::Array(items) => items.iter_mut().for_each(expose_discriminated_properties),
        Value::Object(object) => {
            object
                .values_mut()
                .for_each(expose_discriminated_properties);
            let Some(branches) = object.get("oneOf").and_then(Value::as_array) else {
                return;
            };
            let Some(discriminator) = ["action", "kind"].into_iter().find(|key| {
                branches.iter().all(|branch| {
                    branch["type"] == "object" && branch["properties"][key]["const"].is_string()
                })
            }) else {
                return;
            };
            let mut properties = serde_json::Map::new();
            for branch in branches {
                for (key, value) in branch["properties"].as_object().unwrap() {
                    properties
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
            }
            properties.insert(discriminator.into(),json!({"type":"string","enum":branches.iter().map(|branch|branch["properties"][discriminator]["const"].clone()).collect::<Vec<_>>()}));
            object.insert("type".into(), json!("object"));
            object.insert("properties".into(), Value::Object(properties));
            object.insert("required".into(), json!([discriminator]));
            object.insert("additionalProperties".into(), json!(false));
        }
        _ => {}
    }
}
