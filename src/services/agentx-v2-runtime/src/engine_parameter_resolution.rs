use std::collections::BTreeMap;

use agentx_domain::{ConditionOperator, InputBinding, InputTemplateSegment, ValueNamespace};
use agentx_node_protocol::Item;
use agentx_runtime::{ExpressionContext, ExpressionEngine, ExpressionError};
use serde_json::{Value, json};

pub(super) fn reads_outputs(value: &Value) -> bool {
    if value.is_object()
        && let Ok(binding) = serde_json::from_value::<InputBinding>(value.clone())
    {
        return binding_reads_outputs(&binding);
    }
    match value {
        Value::Array(values) => values.iter().any(reads_outputs),
        Value::Object(fields) => fields.values().any(reads_outputs),
        _ => false,
    }
}

fn binding_reads_outputs(binding: &InputBinding) -> bool {
    match binding {
        InputBinding::Literal { .. } => false,
        InputBinding::Reference { selector, .. } => selector.namespace == ValueNamespace::Outputs,
        InputBinding::Array { items } => items.iter().any(binding_reads_outputs),
        InputBinding::Object { fields } => fields.values().any(binding_reads_outputs),
        InputBinding::Template { segments } => segments.iter().any(|segment| {
            matches!(segment, InputTemplateSegment::Reference { selector, .. } if selector.namespace == ValueNamespace::Outputs)
        }),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve(
    node_type: &str,
    raw_parameters: &Value,
    parameter_schema: &Value,
    inputs_by_port: &BTreeMap<String, Vec<Item>>,
    loop_context: Value,
    inputs: Value,
    outputs: Value,
    contexts: Value,
    execution: Value,
    output_node_keys: BTreeMap<String, String>,
) -> Result<(Value, Vec<Value>, Value), ExpressionError> {
    let items = inputs_by_port
        .values()
        .flat_map(|items| items.iter())
        .collect::<Vec<_>>();
    let base = ExpressionContext {
        inputs,
        outputs,
        contexts,
        execution,
        loop_context,
        output_node_keys,
        ..ExpressionContext::default()
    };
    let common_item = items.first().map_or(Value::Null, |item| item.json.clone());
    let mut common_parameters = raw_parameters.clone();
    if node_type == "list"
        && let Some(parameters) = common_parameters.as_object_mut()
    {
        parameters.remove("filter");
        parameters.remove("sort");
    }
    if node_type == "loop_over_items"
        && let Some(parameters) = common_parameters.as_object_mut()
    {
        parameters.remove("outputSelector");
    }
    let (mut common, common_conversions) = ExpressionEngine.resolve_parameters_with_schema(
        &common_parameters,
        parameter_schema,
        &ExpressionContext {
            json: common_item.clone(),
            input: common_item,
            ..base.clone()
        },
    )?;
    evaluate_condition_parameters(node_type, &mut common)?;
    let parameter_items = if matches!(node_type, "list" | "loop_over_items") {
        common
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        items.into_iter().map(|item| item.json.clone()).collect()
    };
    let per_item = parameter_items
        .into_iter()
        .enumerate()
        .map(|(item_index, item)| {
            let mut item_execution = base.execution.clone();
            if let Some(node) = item_execution
                .get_mut("node")
                .and_then(serde_json::Value::as_object_mut)
            {
                node.insert("itemIndex".into(), serde_json::json!(item_index));
            }
            let (mut parameters, conversions) = ExpressionEngine.resolve_parameters_with_schema(
                if node_type == "loop_over_items" {
                    &common_parameters
                } else {
                    raw_parameters
                },
                parameter_schema,
                &ExpressionContext {
                    json: item.clone(),
                    input: item,
                    item_index,
                    execution: item_execution,
                    ..base.clone()
                },
            )?;
            evaluate_condition_parameters(node_type, &mut parameters)?;
            Ok((parameters, conversions))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (per_item_values, per_item_conversions): (Vec<_>, Vec<_>) = per_item.into_iter().unzip();
    if node_type == "list" {
        if let Some(first) = per_item_values.first() {
            if let (Some(target), Some(source)) = (common.as_object_mut(), first.as_object()) {
                for name in ["filter", "sort"] {
                    if let Some(value) = source.get(name) {
                        target.insert(name.into(), value.clone());
                    }
                }
            }
        } else if let Some(target) = common.as_object_mut() {
            target.insert("filter".into(), json!({"conditions":[],"logicalOp":"and"}));
            target.insert("sort".into(), json!([]));
        }
    }
    Ok((
        common,
        per_item_values,
        json!({"common":common_conversions,"perItem":per_item_conversions}),
    ))
}

fn evaluate_condition_parameters(
    node_type: &str,
    parameters: &mut Value,
) -> Result<(), ExpressionError> {
    let groups = match node_type {
        "if" => parameters.get_mut("cases").and_then(Value::as_array_mut),
        "list" => parameters
            .get_mut("filter")
            .and_then(|filter| filter.get_mut("conditions"))
            .and_then(Value::as_array_mut),
        _ => return Ok(()),
    };
    if node_type == "list" {
        if let Some(rows) = groups {
            evaluate_condition_rows(rows)?;
        }
        return Ok(());
    }
    for group in groups.into_iter().flatten() {
        if let Some(rows) = group.get_mut("conditions").and_then(Value::as_array_mut) {
            evaluate_condition_rows(rows)?;
        }
    }
    Ok(())
}

fn evaluate_condition_rows(rows: &mut [Value]) -> Result<(), ExpressionError> {
    for row in rows {
        let Some(condition) = row.get_mut("condition") else {
            continue;
        };
        let operator = condition
            .get("operator")
            .cloned()
            .and_then(|value| serde_json::from_value::<ConditionOperator>(value).ok())
            .ok_or_else(|| {
                ExpressionError::InvalidOperation("condition operator is invalid".into())
            })?;
        let left = condition.get("left").cloned().unwrap_or(Value::Null);
        let right = condition
            .get("right")
            .cloned()
            .map(|value| coerce_condition_right(&left, value))
            .transpose()?;
        *condition =
            Value::Bool(ExpressionEngine.evaluate_condition(operator, &left, right.as_ref())?);
    }
    Ok(())
}

fn coerce_condition_right(left: &Value, right: Value) -> Result<Value, ExpressionError> {
    if left.is_string() {
        return Ok(match right {
            Value::String(_) => right,
            Value::Array(_) | Value::Object(_) => Value::String(
                serde_json::to_string(&right)
                    .map_err(|error| ExpressionError::Result(error.to_string()))?,
            ),
            value => Value::String(value.to_string().trim_matches('"').to_owned()),
        });
    }
    let Some(text) = right.as_str() else {
        return Ok(right);
    };
    if left.is_i64() || left.is_u64() {
        return text.parse::<i64>().map(|value| json!(value)).map_err(|_| {
            ExpressionError::InvalidOperation("condition right value is not an integer".into())
        });
    }
    if left.is_f64() {
        return text.parse::<f64>().map(|value| json!(value)).map_err(|_| {
            ExpressionError::InvalidOperation("condition right value is not a number".into())
        });
    }
    if left.is_boolean() {
        return match text.to_ascii_lowercase().as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(ExpressionError::InvalidOperation(
                "condition right value is not a boolean".into(),
            )),
        };
    }
    if left.is_array() || left.is_object() {
        let parsed = serde_json::from_str::<Value>(text).map_err(|_| {
            ExpressionError::InvalidOperation("condition right value is not valid JSON".into())
        })?;
        if left.is_array() != parsed.is_array() || left.is_object() != parsed.is_object() {
            return Err(ExpressionError::InvalidOperation(
                "condition operands have different types".into(),
            ));
        }
        return Ok(parsed);
    }
    Ok(right)
}

#[cfg(test)]
mod tests {
    use agentx_domain::{
        InputBinding, MissingValuePolicy, ValueNamespace, ValuePathSegment, ValueSelection,
        ValueSelector,
    };

    use super::*;
    use serde_json::json;

    #[test]
    fn nested_output_bindings_still_resolve_but_literal_reference_data_needs_no_history() {
        let reference = json!({"kind":"reference","selector":{
            "namespace":"outputs","sourceNodeId":"earlier-id","port":"main",
            "run":{"kind":"current"},"item":{"kind":"first"},"path":["answer"]
        },"missingPolicy":{"kind":"error"}});
        let parameters = json!({"value":{"kind":"array","items":[{"kind":"template","segments":[
            {"kind":"text","text":"Answer: "}, reference.clone()
        ]}]}});
        assert!(reads_outputs(&parameters));
        let mut outputs = serde_json::Map::new();
        crate::engine_persistence::merge_output_namespace(
            &mut outputs,
            "earlier-key",
            0,
            &json!({
                "main":[{"json":{"answer":"ready"},"lineage":[]}]
            }),
        );
        let (resolved, _, _) = resolve(
            "set",
            &parameters,
            &json!({}),
            &BTreeMap::new(),
            Value::Null,
            Value::Null,
            Value::Object(outputs),
            json!({}),
            json!({}),
            BTreeMap::from([("earlier-id".into(), "earlier-key".into())]),
        )
        .unwrap();
        assert_eq!(resolved["value"], json!(["Answer: ready"]));
        let literal = json!({"value":{"kind":"literal","value":reference}});
        assert!(!reads_outputs(&literal));
        let (resolved, _, _) = resolve(
            "set",
            &literal,
            &json!({}),
            &BTreeMap::new(),
            Value::Null,
            Value::Null,
            json!({}),
            json!({}),
            json!({}),
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(resolved["value"], reference);
    }

    #[test]
    fn resolves_item_parameters_for_each_input_item() {
        let binding = serde_json::to_value(InputBinding::Reference {
            selector: ValueSelector {
                namespace: ValueNamespace::Item,
                source_node_id: None,
                port: None,
                run: ValueSelection::Current,
                item: ValueSelection::Current,
                path: vec![ValuePathSegment::Key("keep".into())],
            },
            missing_policy: MissingValuePolicy::Error,
        })
        .unwrap();
        let items = BTreeMap::from([(
            "main".into(),
            vec![
                Item {
                    json: json!({"keep":true}),
                    ..Item::default()
                },
                Item {
                    json: json!({"keep":false}),
                    ..Item::default()
                },
            ],
        )]);
        let (common, per_item, conversions) = resolve(
            "if",
            &json!({"cases":[{"id":"keep","conditions":[{"condition":{"left":binding,"operator":"eq","right":{"kind":"literal","value":true}}}],"logicalOp":"and"}]}),
            &json!({}),
            &items,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            common.pointer("/cases/0/conditions/0/condition"),
            Some(&json!(true))
        );
        assert_eq!(
            per_item[0].pointer("/cases/0/conditions/0/condition"),
            Some(&json!(true))
        );
        assert_eq!(
            per_item[1].pointer("/cases/0/conditions/0/condition"),
            Some(&json!(false))
        );
        assert_eq!(conversions, json!({"common":[],"perItem":[[],[]]}));
    }

    #[test]
    fn records_canonical_string_conversions_without_exposing_values() {
        let parameters = json!({
            "bodyText":{"kind":"template","segments":[
                {"kind":"reference","selector":{"namespace":"item","run":{"kind":"current"},"item":{"kind":"current"},"path":["body"]},"missingPolicy":{"kind":"error"}}
            ]}
        });
        let items = BTreeMap::from([(
            "main".into(),
            vec![Item {
                json: json!({"body":{"b":2,"a":1}}),
                ..Item::default()
            }],
        )]);
        let (common, _, conversions) = resolve(
            "set",
            &parameters,
            &json!({}),
            &items,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(common["bodyText"], "{\"a\":1,\"b\":2}");
        assert_eq!(conversions["common"][0]["sourceType"], "object");
        assert_eq!(
            conversions["common"][0]["targetPath"],
            "parameters.bodyText.segments[0]"
        );
        assert!(conversions.to_string().find("\"a\":1").is_none());
    }

    #[test]
    fn resolves_nested_set_literals_for_loop_body_items() {
        let items = BTreeMap::from([(
            "main".into(),
            vec![Item {
                json: json!({"score":9}),
                ..Item::default()
            }],
        )]);
        let (common, per_item, _) = resolve(
            "set",
            &json!({"values":{"kind":"object","fields":{"processed":{"kind":"literal","value":true}}},"keepOnlySet":false}),
            &json!({}),
            &items,
            json!({"item":{"score":9},"index":0}),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(common["values"]["processed"], true);
        assert_eq!(per_item[0]["values"]["processed"], true);
    }
}
