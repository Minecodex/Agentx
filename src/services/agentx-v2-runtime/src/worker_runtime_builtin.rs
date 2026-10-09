use std::collections::{BTreeMap, BTreeSet};

use agentx_node_protocol::Item;
use agentx_runtime_contracts::WorkerResultStatusV1;
use serde_json::Value;

use super::{ClaimedWorkerAttempt, WorkerExecution};

pub(super) fn execute(claim: &ClaimedWorkerAttempt) -> WorkerExecution {
    let parameters = &claim.node_parameters;
    let main = claim.inputs.get("main").cloned().unwrap_or_default();
    match claim.node_type.as_str() {
        "loop_over_items" => loop_items(claim),
        "if" => if_items(claim, main),
        "merge" => merge(claim, parameters),
        "reply_message" => reply_or_send(claim, false),
        "send_message" => reply_or_send(claim, true),
        _ => output("main", main),
    }
}

/// Renders the delivery intent for the reply/send nodes (plan7 P7-A). The
/// node itself stays a pure function: it emits the rendered intent with a
/// deterministic delivery id; the settlement transaction resolves the channel
/// and inserts the delivery row.
fn reply_or_send(claim: &ClaimedWorkerAttempt, send: bool) -> WorkerExecution {
    let parameters = &claim.node_parameters;
    let content = parameters
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if content.trim().is_empty() {
        return WorkerExecution::failed(
            "DELIVERY_PAYLOAD_INVALID",
            "reply/send content resolved to empty text",
            false,
        );
    }
    let delivery_id =
        agentx_runtime_contracts::deterministic_uuid(claim.lease.attempt_id, b"delivery:node");
    let mut intent = serde_json::json!({
        "content": content,
        "deliveryId": delivery_id.to_string(),
        "status": "queued",
    });
    if send {
        for field in ["channelId", "targetConversationId", "senderId"] {
            if let Some(value) = parameters.get(field) {
                intent[field] = value.clone();
            }
        }
    }
    output(
        "main",
        vec![Item {
            json: intent,
            ..Item::default()
        }],
    )
}

fn output(port: &str, items: Vec<Item>) -> WorkerExecution {
    WorkerExecution {
        status: WorkerResultStatusV1::Succeeded,
        outputs: BTreeMap::from([(port.into(), items)]),
        error_code: None,
        error_message: None,
        retryable: None,
    }
}

/// Evaluates the ordered `cases[]` (first match wins) and routes each item to
/// the `case:<id>` output whose id matches the connection handle, falling back
/// to the fixed `else` port. Conditions arrive per item as already-resolved
/// booleans; a case combines its conditions with its `logicalOp`.
fn if_items(claim: &ClaimedWorkerAttempt, items: Vec<Item>) -> WorkerExecution {
    let offset = claim
        .inputs
        .iter()
        .take_while(|(port, _)| port.as_str() != "main")
        .map(|(_, items)| items.len())
        .sum::<usize>();
    route_if_items(
        |index| {
            claim
                .per_item_parameters
                .get(offset + index)
                .unwrap_or(&claim.node_parameters)
        },
        items,
    )
}

fn route_if_items<'a>(
    parameters_at: impl Fn(usize) -> &'a Value,
    items: Vec<Item>,
) -> WorkerExecution {
    let mut outputs: BTreeMap<String, Vec<Item>> = BTreeMap::new();
    for (index, item) in items.into_iter().enumerate() {
        let parameters = parameters_at(index);
        let matched = parameters
            .get("cases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|case| {
                let id = case.get("id").and_then(Value::as_str)?;
                let mut conditions = case
                    .get("conditions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|condition| {
                        condition
                            .get("condition")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    });
                let matched = match case.get("logicalOp").and_then(Value::as_str) {
                    Some("or") => conditions.any(|passed| passed),
                    _ => conditions.all(|passed| passed),
                };
                matched.then(|| format!("case:{id}"))
            });
        let port = matched.unwrap_or_else(|| "else".to_owned());
        outputs.entry(port).or_default().push(item);
    }
    WorkerExecution {
        status: WorkerResultStatusV1::Succeeded,
        outputs,
        error_code: None,
        error_message: None,
        retryable: None,
    }
}

fn loop_items(claim: &ClaimedWorkerAttempt) -> WorkerExecution {
    let Some(values) = claim.node_parameters.get("input").and_then(Value::as_array) else {
        return WorkerExecution::failed(
            "LOOP_INPUT_NOT_ARRAY",
            "Loop input must resolve to an array",
            false,
        );
    };
    output(
        "main",
        vec![Item {
            json: serde_json::json!({"items":values}),
            ..Item::default()
        }],
    )
}

fn merge(claim: &ClaimedWorkerAttempt, parameters: &Value) -> WorkerExecution {
    let mode = parameters
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("append");
    if mode == "append" {
        return output("main", claim.inputs.values().flatten().cloned().collect());
    }
    let left = claim
        .inputs
        .get("left")
        .cloned()
        .or_else(|| claim.inputs.get("main:0").cloned())
        .unwrap_or_default();
    let right = claim
        .inputs
        .get("right")
        .cloned()
        .or_else(|| claim.inputs.get("main:1").cloned())
        .unwrap_or_default();
    if mode == "combine_by_position" {
        let length = left.len().max(right.len());
        let items = (0..length)
            .filter_map(|index| combine_items(left.get(index), right.get(index), parameters))
            .collect();
        return output("main", items);
    }
    let left_field = parameters
        .get("leftField")
        .and_then(Value::as_str)
        .unwrap_or("id");
    let right_field = parameters
        .get("rightField")
        .and_then(Value::as_str)
        .unwrap_or("id");
    let left_map = left
        .iter()
        .filter_map(|item| get_path(&item.json, left_field).map(|value| (canonical(value), item)))
        .collect::<BTreeMap<_, _>>();
    let right_map = right
        .iter()
        .filter_map(|item| get_path(&item.json, right_field).map(|value| (canonical(value), item)))
        .collect::<BTreeMap<_, _>>();
    let join = parameters
        .get("joinType")
        .and_then(Value::as_str)
        .unwrap_or("inner");
    let keys = left_map
        .keys()
        .chain(right_map.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let items = keys
        .into_iter()
        .filter_map(|key| {
            let left = left_map.get(&key).copied();
            let right = right_map.get(&key).copied();
            let included = match join {
                "left" => left.is_some(),
                "right" => right.is_some(),
                "full" => true,
                _ => left.is_some() && right.is_some(),
            };
            included
                .then(|| combine_items(left, right, parameters))
                .flatten()
        })
        .collect();
    output("main", items)
}

fn combine_items(left: Option<&Item>, right: Option<&Item>, parameters: &Value) -> Option<Item> {
    let mut result = left.cloned().or_else(|| right.cloned())?;
    let (Some(target), Some(source)) = (
        result.json.as_object_mut(),
        right.and_then(|item| item.json.as_object()),
    ) else {
        return Some(result);
    };
    match parameters
        .get("conflictStrategy")
        .and_then(Value::as_str)
        .unwrap_or("prefer_right")
    {
        "prefer_left" => {
            for (name, value) in source {
                target.entry(name.clone()).or_insert_with(|| value.clone());
            }
        }
        "suffix" => {
            for (name, value) in source {
                if let Some(left_value) = target.remove(name) {
                    if left_value == *value {
                        target.insert(name.clone(), left_value);
                    } else {
                        target.insert(format!("{name}_left"), left_value);
                        target.insert(format!("{name}_right"), value.clone());
                    }
                } else {
                    target.insert(name.clone(), value.clone());
                }
            }
        }
        _ => target.extend(source.clone()),
    }
    Some(result)
}

fn canonical(value: &Value) -> String {
    agentx_runtime_contracts::canonical_bytes(value)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .filter(|segment| !segment.is_empty())
        .try_fold(value, |current, segment| current.get(segment))
}

#[cfg(test)]
mod tests {
    use agentx_node_protocol::Item;
    use serde_json::{Value, json};

    use super::combine_items;

    fn items(values: Vec<Value>) -> Vec<Item> {
        values
            .into_iter()
            .map(|json| Item {
                json,
                ..Item::default()
            })
            .collect()
    }

    #[test]
    fn if_items_route_to_first_matching_case_then_else() {
        let per_item = [
            json!({
                "cases":[
                    {"id":"big","conditions":[{"condition":false},{"condition":true}],"logicalOp":"or"},
                    {"id":"small","conditions":[{"condition":false}]}
                ]
            }),
            json!({
                "cases":[
                    {"id":"big","conditions":[{"condition":false}]},
                    {"id":"small","conditions":[{"condition":false}]}
                ]
            }),
        ];
        let result = super::route_if_items(
            |index| &per_item[index],
            items(vec![json!({"v":1}), json!({"v":2})]),
        );
        assert_eq!(
            result.status,
            agentx_runtime_contracts::WorkerResultStatusV1::Succeeded
        );
        // item 1: case "big" matches via `or`; item 2: no case matches -> else.
        assert_eq!(result.outputs["case:big"].len(), 1);
        assert_eq!(result.outputs["case:big"][0].json, json!({"v":1}));
        assert_eq!(result.outputs["else"].len(), 1);
        assert!(!result.outputs.contains_key("case:small"));
    }

    #[test]
    fn combine_items_supports_suffix_conflict_strategy() {
        let merged = combine_items(
            Some(&items(vec![json!({"id":2,"value":"left"})])[0]),
            Some(&items(vec![json!({"id":2,"value":"right"})])[0]),
            &json!({"conflictStrategy":"suffix"}),
        )
        .unwrap();
        assert_eq!(
            merged.json,
            json!({"id":2,"value_left":"left","value_right":"right"})
        );
    }
}
