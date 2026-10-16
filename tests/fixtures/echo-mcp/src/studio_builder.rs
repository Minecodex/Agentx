//! Deterministic provider responses for browser fault and design-capability tests.
use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};

static OVERFLOW_PROMPTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn call(name: &str, arguments: Value) -> Value {
    json!({"id":"fixture-call","type":"function","function":{"name":name,"arguments":arguments.to_string()}})
}

pub async fn respond(request: &Value) -> Response {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let index = messages
        .iter()
        .rposition(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("P8FIX:"))
        })
        .unwrap_or(0);
    let prompt = messages
        .get(index)
        .and_then(|message| message["content"].as_str())
        .unwrap_or("");
    if request.get("tools").is_some()
        && prompt.contains("P8FIX: overflow")
        && OVERFLOW_PROMPTS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap()
            .insert(prompt.to_owned())
    {
        return Response::builder().status(StatusCode::BAD_REQUEST)
            .header(header::CONTENT_TYPE,"application/json")
            .body(Body::from(json!({"error":{"code":"context_length_exceeded","message":"maximum context length exceeded by the design fixture"}}).to_string())).unwrap();
    }
    let mut results = Vec::new();
    for message in messages
        .iter()
        .skip(index + 1)
        .filter(|message| message["role"] == "tool")
    {
        let name = messages
            .iter()
            .flat_map(|m| m["tool_calls"].as_array().into_iter().flatten())
            .find(|call| call["id"] == message["tool_call_id"])
            .and_then(|call| call.pointer("/function/name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let value = message["content"]
            .as_str()
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
            .unwrap_or(Value::Null);
        results.push((name.to_owned(), value));
    }
    if request.get("tools").is_some() && prompt.contains("slow") && results.is_empty() {
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
    }
    let mut tool = None;
    let mut text =
        "P8 fixture: edits acknowledged and statically checked. No workflow was executed."
            .to_owned();
    if request.get("tools").is_none() {
        text="Design-only summary: preserve current Draft target, user constraints and confirmed ACKs; use workflow_read to refresh facts. Never execute, save, publish, grant or deploy. Past effects are already settled; do not replay them.".into();
    } else if let Some(document) = prompt.strip_prefix("P8FIX: inspector=") {
        text = "## 编辑完成\n\n- **节点**已创建，可点击编辑。\n- 使用 `InputBinding`，只做静态校验。\n\n```json\n{\"executed\":false}\n```".into();
        if results.is_empty() {
            tool = Some(call("workflow_read", json!({"action":"summary"})));
        } else if !results.iter().any(|(name, _)| name == "workflow_edit") {
            let metadata = &results.last().unwrap().1["metadata"];
            tool = Some(call(
                "workflow_edit",
                json!({"action":"batch","expectedLocalDocumentRevision":metadata["localDocumentRevision"],"expectedDocumentHash":metadata["documentHash"],"intents":[{"action":"replace_document","document":serde_json::from_str::<Value>(document).unwrap()}]}),
            ));
        } else if !results.iter().any(|(name, _)| name == "workflow_validate") {
            let edit = &results
                .iter()
                .find(|(name, _)| name == "workflow_edit")
                .unwrap()
                .1;
            if edit["status"] == "applied" {
                tool = Some(call(
                    "workflow_validate",
                    json!({"action":"draft","expectedDocumentHash":edit["documentHash"]}),
                ));
            }
        }
    } else if prompt.contains("artifact_proof") {
        if let Some((_, read)) = results.last() {
            if read.get("totalCharacters").is_none() && read.get("artifactId").is_some() {
                tool = Some(call(
                    "workflow_read",
                    json!({"action":"artifact","artifactId":read["artifactId"],"offset":0,"limit":8000}),
                ));
            }
        } else {
            tool = Some(call(
                "workflow_read",
                json!({"action":"document","cursor":0,"limit":100}),
            ));
        }
    } else if prompt.contains("forbidden") {
        if results.is_empty() {
            tool = Some(call("execute_workflow", json!({"action":"run"})));
        }
    } else if prompt.contains("resource_queries")
        && results
            .iter()
            .filter(|(name, _)| name == "resource_query")
            .count()
            < 3
    {
        let step = results
            .iter()
            .filter(|(name, _)| name == "resource_query")
            .count();
        let resource = prompt
            .split("resource_ref=")
            .nth(1)
            .and_then(|tail| tail.split_whitespace().next())
            .unwrap();
        let action = ["search", "inspect", "options"][step];
        let mut arguments = json!({"action":action,"resourceType":"model","operation":"use"});
        if step == 0 {
            arguments["search"] = json!("");
        } else {
            arguments["resourceId"] = json!(resource);
        }
        tool = Some(call("resource_query", arguments));
    } else if prompt.contains("inspect")
        && !results.iter().any(|(name, _)| name == "execution_inspect")
    {
        tool = Some(call("execution_inspect", json!({"action":"summary"})));
    } else if prompt.contains("invalid") && results.iter().any(|(name, _)| name == "workflow_edit")
    {
        text = "Invalid edit was rejected; no document or execution was changed.".into();
    } else if results
        .last()
        .is_some_and(|(name, value)| name == "workflow_validate" && value.get("error").is_some())
    {
        tool = Some(call("workflow_read", json!({"action":"summary"})));
    } else if prompt.contains("question") && !results.iter().any(|(name, _)| name == "ask_user") {
        tool = Some(call(
            "ask_user",
            json!({"question":"Which label should the node use?","options":["Approved","Review"]}),
        ));
    } else if results
        .last()
        .is_none_or(|(name, _)| name != "workflow_read")
        && !results.iter().any(|(name, value)| {
            name == "workflow_edit"
                && matches!(value["status"].as_str(), Some("applied" | "no_change"))
        })
    {
        tool = Some(call(
            "workflow_read",
            json!({"action":"document","cursor":0,"limit":if prompt.contains("large=") {2} else {100}}),
        ));
    } else if let Some((_, read)) = results
        .iter()
        .rev()
        .find(|(name, _)| name == "workflow_read")
    {
        if !results.iter().any(|(name, value)| {
            name == "workflow_edit"
                && matches!(value["status"].as_str(), Some("applied" | "no_change"))
        }) {
            let label = prompt
                .split("label=")
                .nth(1)
                .and_then(|value| value.split_whitespace().next())
                .unwrap_or("P8 Decision");
            let mut node=read["nodes"].as_array().and_then(|nodes|nodes.iter().find(|node|node["id"]=="p8-decision")).cloned().unwrap_or_else(||json!({"id":"p8-decision","key":"p8_decision","type":"if","typeVersion":1,"name":label,"parameters":{"cases":[{"id":"yes","name":"Yes","logicalOp":"and","conditions":[{"condition":{"left":{"kind":"literal","value":true},"operator":"eq","right":{"kind":"literal","value":true}}}]}]},"contextWrites":[],"resourceReferences":[],"settings":{},"disabled":false,"protected":false}));
            node["name"] = json!(label);
            if let Some(model) = prompt
                .split("resource_ref=")
                .nth(1)
                .and_then(|tail| tail.split_whitespace().next())
            {
                node["resourceReferences"] =
                    json!([{"resourceType":"model","resourceId":model,"operation":"use"}]);
            }
            if prompt.contains("clear_resource") {
                node["resourceReferences"] = json!([]);
            }
            if prompt.contains("invalid_scope") {
                node["parentId"] = json!("missing-loop-container");
            }
            if prompt.contains("invalid_downstream") {
                node["parameters"]["cases"][0]["conditions"][0]["condition"]["left"] = json!({"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"exit","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":[]},"missingPolicy":{"kind":"error"}});
            }
            let exit = read["nodes"]
                .as_array()
                .and_then(|nodes| nodes.iter().find(|node| node["protected"] == true))
                .and_then(|node| node["id"].as_str())
                .unwrap_or("exit");
            let mut intents = if read["nodes"]
                .as_array()
                .is_some_and(|nodes| nodes.iter().any(|node| node["id"] == "p8-decision"))
            {
                json!([{"action":"upsert_node","node":node,"layout":null}])
            } else {
                json!([{"action":"upsert_node","node":node,"layout":{"nodeId":"p8-decision","x":250,"y":220,"collapsed":false}},{"action":"replace_connections","connections":[{"id":"p8-start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"p8-decision","targetHandle":"main","order":0},{"id":"p8-yes","sourceNodeId":"p8-decision","sourceHandle":"case:yes","targetNodeId":exit,"targetHandle":"main","order":0},{"id":"p8-default","sourceNodeId":"p8-decision","sourceHandle":"else","targetNodeId":exit,"targetHandle":"main","order":0}]}])
            };
            if prompt.contains("P8FIX: invalid label=") {
                intents
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"action":"remove_node","nodeId":exit}));
            }
            if prompt.contains("invalid_port") {
                intents = json!([{"action":"replace_connections","connections":[{"id":"bad","sourceNodeId":"p8-decision","sourceHandle":"case:missing","targetNodeId":exit,"targetHandle":"main","order":0}]}]);
            }
            if prompt.contains("whole") {
                intents.as_array_mut().unwrap().push(json!({"action":"upsert_node","node":{"id":"p8-code","key":"p8_code","type":"code","typeVersion":1,"name":"Static Code","parameters":{"inputs":{"kind":"object","fields":{}},"runner":"javascript","networkPolicy":{"mode":"deny","destinations":[]},"source":"throw new Error('AI_MUST_NOT_EXECUTE');","outputExample":{"marker":true}},"contextWrites":[],"resourceReferences":[],"settings":{},"disabled":false,"protected":false},"layout":{"nodeId":"p8-code","x":520,"y":400,"collapsed":false}}));
                intents.as_array_mut().unwrap().push(json!({"action":"replace_connections","connections":[{"id":"whole-start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"p8-decision","targetHandle":"main","order":0},{"id":"whole-code","sourceNodeId":"p8-decision","sourceHandle":"case:yes","targetNodeId":"p8-code","targetHandle":"main","order":0},{"id":"whole-code-exit","sourceNodeId":"p8-code","sourceHandle":"main","targetNodeId":exit,"targetHandle":"main","order":0},{"id":"whole-else","sourceNodeId":"p8-decision","sourceHandle":"else","targetNodeId":exit,"targetHandle":"main","order":0}]}));
            }
            if prompt.contains("with_loop") {
                intents.as_array_mut().unwrap().extend([
                    json!({"action":"upsert_node","node":{"id":"p8-loop","key":"p8_loop","type":"loop_over_items","typeVersion":1,"name":"Design Loop","parameters":{"input":{"kind":"array","items":[{"kind":"object","fields":{"marker":{"kind":"literal","value":true}}}]},"outputSelector":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"p8-loop-body","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":[]},"missingPolicy":{"kind":"error"}},"parallelism":1,"errorMode":"terminate"},"contextWrites":[],"resourceReferences":[],"settings":{},"disabled":false,"protected":false},"layout":{"nodeId":"p8-loop","x":520,"y":200,"collapsed":false}}),
                    json!({"action":"upsert_node","node":{"id":"p8-loop-body","key":"p8_loop_body","type":"set","typeVersion":1,"name":"Loop item","parentId":"p8-loop","parameters":{"values":{"kind":"object","fields":{"marker":{"kind":"literal","value":true}}},"keepOnlySet":true},"contextWrites":[],"resourceReferences":[],"settings":{},"disabled":false,"protected":false},"layout":{"nodeId":"p8-loop-body","x":60,"y":120,"collapsed":false}}),
                    json!({"action":"replace_connections","connections":[{"id":"loop-start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"p8-decision","targetHandle":"main","order":0},{"id":"loop-branch","sourceNodeId":"p8-decision","sourceHandle":"case:yes","targetNodeId":"p8-loop","targetHandle":"main","order":0},{"id":"loop-code","sourceNodeId":"p8-loop","sourceHandle":"main","targetNodeId":"p8-code","targetHandle":"main","order":0},{"id":"loop-code-exit","sourceNodeId":"p8-code","sourceHandle":"main","targetNodeId":exit,"targetHandle":"main","order":0},{"id":"loop-else","sourceNodeId":"p8-decision","sourceHandle":"else","targetNodeId":exit,"targetHandle":"main","order":0}]})
                ]);
            }
            if let Some(count) = prompt
                .split("large=")
                .nth(1)
                .and_then(|tail| tail.split_whitespace().next())
                .and_then(|count| count.parse::<usize>().ok())
                .filter(|count| matches!(count, 150 | 300))
            {
                let mut nodes = read["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|node| node["id"] == "p8-decision" || node["id"] == exit)
                    .cloned()
                    .collect::<Vec<_>>();
                let base = nodes
                    .iter()
                    .find(|node| node["id"] == "p8-decision")
                    .unwrap()
                    .clone();
                let mut layouts = read["editorDocument"]["nodeLayouts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|layout| layout["nodeId"] == "p8-decision" || layout["nodeId"] == exit)
                    .cloned()
                    .collect::<Vec<_>>();
                let base_layout = layouts
                    .iter()
                    .find(|layout| layout["nodeId"] == "p8-decision")
                    .unwrap()
                    .clone();
                for index in 0..count - 2 {
                    let id = format!("p8-branch-{index}");
                    let mut next = base.clone();
                    next["id"] = json!(id);
                    next["key"] = json!(format!("branch_{index}"));
                    next["name"] = json!(format!("Branch {index}"));
                    nodes.push(next);
                    let mut layout = base_layout.clone();
                    layout["nodeId"] = json!(id);
                    layout["x"] = json!(250 + (index % 10) * 300);
                    layout["y"] = json!(220 + (index / 10) * 220);
                    layouts.push(layout);
                }
                let mut connections = vec![
                    json!({"id":"p8-start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"p8-decision","targetHandle":"main","order":0}),
                ];
                for index in 0..count - 1 {
                    let id = if index == 0 {
                        "p8-decision".to_owned()
                    } else {
                        format!("p8-branch-{}", index - 1)
                    };
                    let next = if index == count - 2 {
                        exit.to_owned()
                    } else {
                        format!("p8-branch-{index}")
                    };
                    for (port, target) in [("case:yes", exit.to_owned()), ("else", next)] {
                        connections.push(json!({"id":format!("edge-{index}-{port}"),"sourceNodeId":id,"sourceHandle":port,"targetNodeId":target,"targetHandle":"main","order":0}));
                    }
                }
                let mut editor = read["editorDocument"].clone();
                editor["nodeLayouts"] = json!(layouts);
                editor["edges"] = json!(
                    connections
                        .iter()
                        .map(|edge| json!({"edgeId":edge["id"]}))
                        .collect::<Vec<_>>()
                );
                intents = json!([{"action":"replace_document","document":{"definition":{"schemaVersion":"8.0","start":read["start"],"end":read["end"],"settings":read["settings"],"nodes":nodes,"connections":connections},"editorDocument":editor}}]);
            }
            tool = Some(call(
                "workflow_edit",
                json!({"action":"batch","expectedLocalDocumentRevision":read["metadata"]["localDocumentRevision"],"expectedDocumentHash":read["metadata"]["documentHash"],"intents":intents}),
            ));
        } else if !results.iter().any(|(name, value)| {
            name == "workflow_validate" && value["executionPerformed"] == false
        }) {
            let hash = results
                .iter()
                .rev()
                .find_map(|(name, value)| {
                    if name == "workflow_read" && value["metadata"]["documentHash"].is_string() {
                        Some(value["metadata"]["documentHash"].clone())
                    } else if name == "workflow_edit" && value["documentHash"].is_string() {
                        Some(value["documentHash"].clone())
                    } else {
                        None
                    }
                })
                .unwrap_or(Value::Null);
            tool = Some(call(
                "workflow_validate",
                json!({"action":"complete","expectedDocumentHash":hash}),
            ));
        }
    }
    let reason = if tool.is_some() { "tool_calls" } else { "stop" };
    let delta = if let Some(tool) = tool {
        json!({"tool_calls":[{"index":0,"id":tool["id"],"type":"function","function":tool["function"]}]})
    } else {
        json!({"content":text})
    };
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"id":"p8-response","choices":[{"index":0,"delta":delta,"finish_reason":reason}]}),
        json!({"id":"p8-response","choices":[],"usage":{"prompt_tokens":32,"completion_tokens":48,"total_tokens":80}})
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}
