use super::*;
use crate::{CompileContext, NodeRegistry, WorkflowCompiler};
use agentx_domain::WorkflowDefinition;

fn compile(mut value: serde_json::Value) -> CompiledWorkflow {
    let definition = value.as_object_mut().expect("workflow fixture object");
    definition.insert(
        "schemaVersion".into(),
        json!(agentx_domain::WORKFLOW_SCHEMA_VERSION),
    );
    definition.insert(
        "start".into(),
        json!({"inputs":{"type":"object","properties":{"test_items":{"type":"array","items":{}}},"additionalProperties":false},"contexts":{}}),
    );
    definition
        .entry("end")
        .or_insert_with(|| json!({"outputs":{}}));
    for node in definition
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .expect("workflow fixture nodes")
    {
        let node = node.as_object_mut().expect("workflow fixture node");
        let key = node.get("id").cloned().expect("workflow fixture node id");
        node.insert("key".into(), key);
        node.insert("contextWrites".into(), json!([]));
        match node.get("type").and_then(Value::as_str) {
            Some("if") => {
                for condition in node
                    .get_mut("parameters")
                    .and_then(Value::as_object_mut)
                    .and_then(|parameters| parameters.get_mut("cases"))
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                    .filter_map(|case| case.get_mut("conditions").and_then(Value::as_array_mut))
                    .flatten()
                {
                    if let Some(value) = condition.get("condition").cloned()
                        && !value.is_object()
                    {
                        condition["condition"] = json!({"left":{"kind":"literal","value":value},"operator":"eq","right":{"kind":"literal","value":true}});
                    }
                }
            }
            Some("loop_over_items") => {
                if node
                    .get("parameters")
                    .and_then(|parameters| parameters.get("input"))
                    .and_then(|input| input.get("kind"))
                    .and_then(Value::as_str)
                    == Some("literal")
                {
                    node["parameters"]["input"] = json!({"kind":"reference","selector":{"namespace":"inputs","run":{"kind":"current"},"item":{"kind":"current"},"path":["test_items"]},"missingPolicy":{"kind":"error"}});
                }
            }
            Some("declarative_http") => wrap_text_parameter(node, "url"),
            Some("approval") => {
                wrap_text_parameter(node, "title");
                wrap_text_parameter(node, "description");
            }
            Some("model" | "agent") => {
                wrap_text_parameter(node, "prompt");
                wrap_text_parameter(node, "systemPrompt");
                wrap_text_parameter(node, "userQuestion");
            }
            _ => {}
        }
    }
    let node_ids = definition["nodes"]
        .as_array()
        .expect("workflow fixture nodes")
        .iter()
        .filter(|node| node.get("parentId").is_none())
        .filter_map(|node| node.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let node_types = definition["nodes"]
        .as_array()
        .expect("workflow fixture nodes")
        .iter()
        .filter_map(|node| {
            Some((
                node.get("id")?.as_str()?.to_owned(),
                node.get("type")?.as_str()?.to_owned(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let connections = definition
        .get_mut("connections")
        .and_then(Value::as_array_mut)
        .expect("workflow fixture connections");
    let mut has_incoming = BTreeSet::new();
    let mut has_outgoing = BTreeSet::new();
    for connection in connections.iter() {
        if let Some(source) = connection.get("sourceNodeId").and_then(Value::as_str) {
            has_outgoing.insert(source.to_owned());
        }
        if let Some(target) = connection.get("targetNodeId").and_then(Value::as_str) {
            has_incoming.insert(target.to_owned());
        }
    }
    if let Some(root) = node_ids.iter().find(|id| !has_incoming.contains(*id)) {
        connections.push(json!({"id":"__test_start__","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":root,"targetHandle":"main","order":0}));
    }
    let has_exit_connection = connections.iter().any(|connection| {
        connection.get("targetNodeId").and_then(Value::as_str) == Some("__test_exit__")
            && connection.get("targetHandle").and_then(Value::as_str) == Some("main")
    });
    if !has_exit_connection {
        let end_source = node_ids
            .iter()
            .find(|id| !has_outgoing.contains(*id))
            .or_else(|| node_ids.last())
            .expect("workflow fixture has node");
        let end_handle = node_types
            .get(end_source)
            .map(|node_type| match node_type.as_str() {
                "if" => "else",
                "approval" => "decision:approved",
                "loop_over_items" => "main",
                _ => "main",
            })
            .unwrap_or("main");
        connections.push(json!({"id":"__test_end__","sourceNodeId":end_source,"sourceHandle":end_handle,"targetNodeId":"__test_exit__","targetHandle":"main","order":99}));
    }
    definition
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .expect("workflow fixture nodes")
        .push(json!({
            "id":"__test_exit__","key":"__test_exit__","type":"exit","typeVersion":1,"name":"End",
            "parameters":{"outputs":{},"errorOutputs":{}}
        }));
    let registry = NodeRegistry::m4_defaults();
    WorkflowCompiler::new(&registry)
        .compile(
            &serde_json::from_value::<WorkflowDefinition>(value).unwrap(),
            &CompileContext::default(),
        )
        .unwrap()
}

fn wrap_text_parameter(node: &mut serde_json::Map<String, Value>, name: &str) {
    let Some(value) = node
        .get_mut("parameters")
        .and_then(Value::as_object_mut)
        .and_then(|parameters| parameters.get_mut(name))
    else {
        return;
    };
    if let Some(text) = value.as_str() {
        *value = json!({"kind":"template","segments":[{"kind":"text","text":text}]});
    }
}

fn item(value: i64) -> Item {
    Item {
        json: json!({"value":value}),
        ..Item::default()
    }
}

#[test]
fn closes_unselected_branch_without_blocking_merge() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "nodes":[
            {"id":"trigger","type":"set","typeVersion":1,"name":"Root",},
            {"id":"if","type":"if","typeVersion":1,"name":"IF","parameters":{"cases":[{"id":"c1","conditions":[{"condition":true}]}]}},
            {"id":"merge","type":"merge","typeVersion":1,"name":"Merge",}
        ],
        "connections":[
            {"id":"a","sourceNodeId":"trigger","sourceHandle":"main","targetNodeId":"if","targetHandle":"main","order":0},
            {"id":"b","sourceNodeId":"if","sourceHandle":"case:c1","targetNodeId":"merge","targetHandle":"main:0","order":0},
            {"id":"c","sourceNodeId":"if","sourceHandle":"else","targetNodeId":"merge","targetHandle":"main:1","order":1}
        ]
    }));
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let trigger = machine.next_ready().unwrap();
    machine.start_attempt(trigger).unwrap();
    machine
        .complete(trigger, BTreeMap::from([("main".into(), vec![item(1)])]))
        .unwrap();
    let condition = machine.next_ready().unwrap();
    machine.start_attempt(condition).unwrap();
    machine
        .complete(
            condition,
            BTreeMap::from([("case:c1".into(), vec![item(1)])]),
        )
        .unwrap();
    let merge = machine.next_ready().expect("merge becomes ready");
    assert_eq!(machine.activations[&merge].inputs["main:0"].len(), 1);
}

#[test]
fn start_to_exit_workflow_completes_immediately() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "nodes":[],
        "connections":[
            {"id":"a","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"__test_exit__","targetHandle":"main","order":0}
        ]
    }));
    assert!(
        workflow.start_to_exit.is_some(),
        "fixture must compile as start→exit"
    );
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    assert_eq!(machine.status(), RuntimeExecutionStatus::Succeeded);
    assert!(machine.next_ready().is_none());
    assert!(machine.end_deliveries().is_empty());
}

#[test]
fn retry_adds_attempt_to_same_activation_and_late_transitions_fail() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "nodes":[{"id":"trigger","type":"set","typeVersion":1,"name":"Root","settings":{"retryOnFail":true,"maxTries":2}}],
        "connections":[]
    }));
    let mut machine = ExecutionMachine::new(workflow, vec![]).unwrap();
    let activation = machine.next_ready().unwrap();
    machine.start_attempt(activation).unwrap();
    machine.fail(activation, "TEMP", "temporary", true).unwrap();
    assert_eq!(machine.next_ready(), Some(activation));
    machine.start_attempt(activation).unwrap();
    machine
        .complete(activation, BTreeMap::from([("main".into(), vec![item(1)])]))
        .unwrap();
    assert_eq!(machine.activations[&activation].attempts.len(), 2);
    assert_eq!(machine.status(), RuntimeExecutionStatus::Succeeded);
    assert_eq!(
        machine.start_attempt(activation),
        Err(MachineError::ExecutionTerminal)
    );
}

#[test]
fn timeout_is_terminal_and_marks_the_active_attempt_failed() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "nodes":[{"id":"model","type":"model","typeVersion":1,"name":"Model"}],
        "connections":[]
    }));
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let activation = machine.next_ready().unwrap();
    machine.start_attempt(activation).unwrap();

    machine.timeout();

    assert_eq!(machine.status(), RuntimeExecutionStatus::TimedOut);
    assert_eq!(
        machine.activation(activation).unwrap().status,
        ActivationStatus::Failed
    );
    let attempt = machine
        .activation(activation)
        .unwrap()
        .attempts
        .last()
        .unwrap();
    assert_eq!(attempt.status, AttemptStatus::Failed);
    assert_eq!(
        attempt.error_code.as_deref(),
        Some("NODE_EXECUTION_TIMED_OUT")
    );
    assert_eq!(machine.next_ready(), None);
    machine.timeout();
    assert_eq!(machine.status(), RuntimeExecutionStatus::TimedOut);
}

fn error_terminal_fixture() -> Value {
    json!({
        "schemaVersion":"8.0",
        "end":{"outputs":{},"error":{"outputs":{}}},
        "nodes":[
            {"id":"first","type":"set","typeVersion":1,"name":"First"},
            {"id":"second","type":"set","typeVersion":1,"name":"Second"}
        ],
        "connections":[
            {"id":"start-first","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"first","targetHandle":"main","order":0},
            {"id":"start-second","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"second","targetHandle":"main","order":1},
            {"id":"first-main","sourceNodeId":"first","sourceHandle":"main","targetNodeId":"__test_exit__","targetHandle":"main","order":0},
            {"id":"second-main","sourceNodeId":"second","sourceHandle":"main","targetNodeId":"__test_exit__","targetHandle":"main","order":1},
            {"id":"first-error","sourceNodeId":"first","sourceHandle":"error","targetNodeId":"__test_exit__","targetHandle":"error","order":0},
            {"id":"second-error","sourceNodeId":"second","sourceHandle":"error","targetNodeId":"__test_exit__","targetHandle":"error","order":1}
        ]
    })
}

#[test]
fn end_error_fail_fast_cancels_other_running_activations() {
    let workflow = compile(error_terminal_fixture());
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let first = machine.next_ready().unwrap();
    let second = machine.next_ready().unwrap();
    machine.start_attempt(first).unwrap();
    machine.start_attempt(second).unwrap();
    machine.suspend(second).unwrap();

    machine.fail(first, "FAILED", "failed", false).unwrap();

    assert_eq!(machine.status(), RuntimeExecutionStatus::Failed);
    assert_eq!(
        machine.activation(second).unwrap().status,
        ActivationStatus::Cancelled
    );
    assert_eq!(
        machine.activation(second).unwrap().attempts[0].status,
        AttemptStatus::Cancelled
    );
    assert_eq!(machine.end_deliveries().len(), 1);
    assert_eq!(machine.end_deliveries()[0].target_port, "error");
}

fn parallel_fixture(completion: &str) -> Value {
    json!({
        "schemaVersion":"8.0",
        "end":{"completion":completion,"outputs":{},"error":{"outputs":{}}},
        "nodes":[
            {"id":"first","type":"set","typeVersion":1,"name":"First"},
            {"id":"second","type":"set","typeVersion":1,"name":"Second"}
        ],
        "connections":[
            {"id":"start-first","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"first","targetHandle":"main","order":0},
            {"id":"start-second","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"second","targetHandle":"main","order":1},
            {"id":"first-end","sourceNodeId":"first","sourceHandle":"main","targetNodeId":"__test_exit__","targetHandle":"main","order":0},
            {"id":"second-end","sourceNodeId":"second","sourceHandle":"main","targetNodeId":"__test_exit__","targetHandle":"main","order":1}
        ]
    })
}

fn loop_container_fixture(error_mode: &str) -> Value {
    json!({
        "schemaVersion":"8.0",
        "end":{"outputs":{},"error":{"outputs":{}}},
        "nodes":[
            {"id":"trigger","type":"set","typeVersion":1,"name":"Trigger"},
            {"id":"loop","type":"loop_over_items","typeVersion":1,"name":"Loop","parameters":{"input":{"kind":"literal","value":[]},"outputSelector":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"body","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":[]},"missingPolicy":{"kind":"error"}},"parallelism":1,"errorMode":error_mode}},
            {"id":"body","type":"set","typeVersion":1,"name":"Body","parentId":"loop"}
        ],
        "connections":[
            {"id":"start-trigger","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"trigger","targetHandle":"main","order":0},
            {"id":"trigger-loop","sourceNodeId":"trigger","sourceHandle":"main","targetNodeId":"loop","targetHandle":"main","order":0}
        ],
        "settings":{"activationBudget":100}
    })
}

#[test]
fn loop_container_aggregates_body_rounds_in_iteration_order() {
    let workflow = compile(loop_container_fixture("terminate"));
    let batch = vec![item(1), item(2), item(3)];
    let mut machine = ExecutionMachine::new(workflow, batch.clone()).unwrap();
    let trigger = machine.next_ready().unwrap();
    machine.start_attempt(trigger).unwrap();
    machine
        .complete(trigger, BTreeMap::from([("main".into(), batch.clone())]))
        .unwrap();
    let loop_activation = machine.next_ready().unwrap();
    machine.start_attempt(loop_activation).unwrap();
    machine
        .complete(
            loop_activation,
            BTreeMap::from([(
                "main".into(),
                vec![Item {
                    json: json!({"items":batch.clone().into_iter().map(|item| item.json).collect::<Vec<_>>() }),
                    ..Item::default()
                }],
            )]),
        )
        .unwrap();
    for expected in [1, 2, 3] {
        let body = machine.next_ready().unwrap();
        assert_eq!(
            machine
                .activation(body)
                .unwrap()
                .loop_frame
                .as_ref()
                .unwrap()["items"],
            json!([{"value":1},{"value":2},{"value":3}])
        );
        machine.start_attempt(body).unwrap();
        machine
            .complete(
                body,
                BTreeMap::from([("main".into(), vec![item(expected)])]),
            )
            .unwrap();
    }
    assert_eq!(machine.status(), RuntimeExecutionStatus::Succeeded);
    let aggregate = machine
        .end_deliveries()
        .iter()
        .find(|delivery| delivery.target_port == "main")
        .unwrap();
    let values: Vec<i64> = aggregate.items[0].json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["value"].as_i64().unwrap())
        .collect();
    assert_eq!(values, vec![1, 2, 3]);
}

#[test]
fn loop_container_error_mode_remove_drops_failed_rounds() {
    let workflow = compile(loop_container_fixture("remove"));
    let batch = vec![item(1), item(2)];
    let mut machine = ExecutionMachine::new(workflow, batch.clone()).unwrap();
    let trigger = machine.next_ready().unwrap();
    machine.start_attempt(trigger).unwrap();
    machine
        .complete(trigger, BTreeMap::from([("main".into(), batch.clone())]))
        .unwrap();
    let loop_activation = machine.next_ready().unwrap();
    machine.start_attempt(loop_activation).unwrap();
    machine
        .complete(
            loop_activation,
            BTreeMap::from([(
                "main".into(),
                vec![Item {
                    json: json!({"items":batch.into_iter().map(|item| item.json).collect::<Vec<_>>() }),
                    ..Item::default()
                }],
            )]),
        )
        .unwrap();
    // First round fails inside the body, the second succeeds.
    let first = machine.next_ready().unwrap();
    machine.start_attempt(first).unwrap();
    machine
        .fail(first, "BODY_FAILED", "body round failed", false)
        .unwrap();
    let second = machine.next_ready().unwrap();
    machine.start_attempt(second).unwrap();
    machine
        .complete(second, BTreeMap::from([("main".into(), vec![item(2)])]))
        .unwrap();
    assert_eq!(machine.status(), RuntimeExecutionStatus::Succeeded);
    let aggregate = machine
        .end_deliveries()
        .iter()
        .find(|delivery| delivery.target_port == "main")
        .unwrap();
    let values: Vec<i64> = aggregate.items[0].json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["value"].as_i64().unwrap())
        .collect();
    assert_eq!(values, vec![2]);
}

#[test]
fn loop_parallelism_survives_checkpoint_and_keeps_input_order() {
    let mut fixture = loop_container_fixture("terminate");
    fixture["nodes"][1]["parameters"]["parallelism"] = json!(2);
    let workflow = compile(fixture);
    let batch = vec![item(1), item(2), item(3)];
    let mut machine = ExecutionMachine::new(workflow, batch.clone()).unwrap();
    let trigger = machine.next_ready().unwrap();
    machine.start_attempt(trigger).unwrap();
    machine
        .complete(trigger, BTreeMap::from([("main".into(), batch.clone())]))
        .unwrap();
    let loop_activation = machine.next_ready().unwrap();
    machine.start_attempt(loop_activation).unwrap();
    machine
        .complete(
            loop_activation,
            BTreeMap::from([(
                "main".into(),
                vec![Item {
                    json: json!({"items":batch.into_iter().map(|item| item.json).collect::<Vec<_>>() }),
                    ..Item::default()
                }],
            )]),
        )
        .unwrap();
    let first = machine.next_ready().unwrap();
    machine.start_attempt(first).unwrap();
    let second = machine.next_ready().unwrap();
    machine.start_attempt(second).unwrap();
    assert!(
        machine.next_ready().is_none(),
        "parallelism=2 must keep the third round queued"
    );

    let mut machine: ExecutionMachine =
        serde_json::from_value(serde_json::to_value(machine).unwrap()).unwrap();
    machine
        .complete(second, BTreeMap::from([("main".into(), vec![item(2)])]))
        .unwrap();
    let third = machine
        .next_ready()
        .expect("a completed slot activates the queued round");
    machine.start_attempt(third).unwrap();
    machine
        .complete(third, BTreeMap::from([("main".into(), vec![item(3)])]))
        .unwrap();
    machine
        .complete(first, BTreeMap::from([("main".into(), vec![item(1)])]))
        .unwrap();

    let values = machine.end_deliveries()[0].items[0].json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["value"].as_i64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(values, vec![1, 2, 3]);
}

#[test]
fn loop_error_mode_continue_preserves_failed_positions_as_null() {
    let workflow = compile(loop_container_fixture("continue"));
    let batch = vec![item(1), item(2)];
    let mut machine = ExecutionMachine::new(workflow, batch.clone()).unwrap();
    let trigger = machine.next_ready().unwrap();
    machine.start_attempt(trigger).unwrap();
    machine
        .complete(trigger, BTreeMap::from([("main".into(), batch.clone())]))
        .unwrap();
    let loop_activation = machine.next_ready().unwrap();
    machine.start_attempt(loop_activation).unwrap();
    machine
        .complete(
            loop_activation,
            BTreeMap::from([(
                "main".into(),
                vec![Item {
                    json: json!({"items":batch.into_iter().map(|item| item.json).collect::<Vec<_>>() }),
                    ..Item::default()
                }],
            )]),
        )
        .unwrap();
    let failed = machine.next_ready().unwrap();
    machine.start_attempt(failed).unwrap();
    machine
        .fail(failed, "BROKEN", "round failed", false)
        .unwrap();
    let succeeded = machine.next_ready().unwrap();
    machine.start_attempt(succeeded).unwrap();
    machine
        .complete(succeeded, BTreeMap::from([("main".into(), vec![item(2)])]))
        .unwrap();
    assert_eq!(
        machine.end_deliveries()[0].items[0].json["items"],
        json!([null, {"value":2}])
    );
}

#[test]
fn first_return_finalizes_on_the_first_exit_delivery_and_cancels_the_rest() {
    let workflow = compile(parallel_fixture("first_return"));
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let first = machine.next_ready().unwrap();
    let second = machine.next_ready().unwrap();
    machine.start_attempt(first).unwrap();
    machine.start_attempt(second).unwrap();
    machine
        .complete(first, BTreeMap::from([("main".into(), vec![item(1)])]))
        .unwrap();
    assert_eq!(machine.status(), RuntimeExecutionStatus::Succeeded);
    assert_eq!(
        machine.activation(second).unwrap().status,
        ActivationStatus::Cancelled
    );
    assert_eq!(machine.end_deliveries().len(), 1);
}

#[test]
fn all_complete_waits_for_every_branch_and_collects_all_deliveries() {
    let workflow = compile(parallel_fixture("all_complete"));
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let first = machine.next_ready().unwrap();
    let second = machine.next_ready().unwrap();
    machine.start_attempt(first).unwrap();
    machine
        .complete(first, BTreeMap::from([("main".into(), vec![item(1)])]))
        .unwrap();
    assert_eq!(
        machine.status(),
        RuntimeExecutionStatus::Running,
        "all_complete must not finalize on the first return"
    );
    machine.start_attempt(second).unwrap();
    machine
        .complete(second, BTreeMap::from([("main".into(), vec![item(2)])]))
        .unwrap();
    assert_eq!(machine.status(), RuntimeExecutionStatus::Succeeded);
    assert_eq!(machine.end_deliveries().len(), 2);
}

#[test]
fn approval_releases_execution_and_resumes_once() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "nodes":[
            {"id":"trigger","type":"set","typeVersion":1,"name":"Root",},
            {"id":"approval","type":"approval","typeVersion":1,"name":"Approval","parameters":{"candidateUserId":"018f0000-0000-7000-8000-000000000001"}}
        ],
        "connections":[{"id":"a","sourceNodeId":"trigger","sourceHandle":"main","targetNodeId":"approval","targetHandle":"main","order":0}]
    }));
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let trigger = machine.next_ready().unwrap();
    machine.start_attempt(trigger).unwrap();
    machine
        .complete(trigger, BTreeMap::from([("main".into(), vec![item(1)])]))
        .unwrap();
    let approval = machine.next_ready().unwrap();
    machine.start_attempt(approval).unwrap();
    machine.suspend(approval).unwrap();
    assert_eq!(machine.status(), RuntimeExecutionStatus::Waiting);
    machine
        .resume(approval, "decision:approved", vec![item(2)])
        .unwrap();
    assert_eq!(machine.status(), RuntimeExecutionStatus::Succeeded);
    assert_eq!(
        machine.resume(approval, "decision:approved", vec![]),
        Err(MachineError::ExecutionTerminal)
    );
}

#[test]
fn suspended_composite_can_converge_to_a_failed_terminal() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "nodes":[
            {"id":"child","type":"approval","typeVersion":1,"name":"Child","parameters":{"candidateUserId":"018f0000-0000-7000-8000-000000000001"},"settings":{}}
        ],
        "connections":[
            {"id":"child-error","sourceNodeId":"child","sourceHandle":"error","targetNodeId":"__test_exit__","targetHandle":"error","order":0}
        ],
        "end":{"outputs":{},"error":{"outputs":{}}}
    }));
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let child = machine.next_ready().unwrap();
    machine.start_attempt(child).unwrap();
    machine.suspend(child).unwrap();

    machine
        .fail(child, "COMPOSITE_CHILD_FAILED", "child failed", false)
        .unwrap();

    assert_eq!(machine.status(), RuntimeExecutionStatus::Failed);
    assert_eq!(
        machine.activation(child).unwrap().status,
        ActivationStatus::Failed
    );
    assert_eq!(machine.end_deliveries()[0].target_port, "error");
}

#[test]
fn partial_forks_select_the_expected_subgraph_and_inputs() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "nodes":[
            {"id":"trigger","type":"set","typeVersion":1,"name":"Root",},
            {"id":"first","type":"set","typeVersion":1,"name":"First",},
            {"id":"last","type":"set","typeVersion":1,"name":"Last",}
        ],
        "connections":[
            {"id":"a","sourceNodeId":"trigger","sourceHandle":"main","targetNodeId":"first","targetHandle":"main","order":0},
            {"id":"b","sourceNodeId":"first","sourceHandle":"main","targetNodeId":"last","targetHandle":"main","order":0}
        ]
    }));
    let mut source = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let trigger = source.next_ready().unwrap();
    source.start_attempt(trigger).unwrap();
    source
        .complete(trigger, BTreeMap::from([("main".into(), vec![item(1)])]))
        .unwrap();

    let from = source
        .fork_from_checkpoint(
            PartialExecutionMode::FromNode,
            Some("first"),
            vec![],
            &json!({"override":true}),
        )
        .unwrap();
    assert_eq!(
        from.workflow
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "last"]
    );
    let first = from.activations().next().unwrap();
    assert_eq!(first.inputs["main"][0].json["override"], true);

    let to = source
        .fork_from_checkpoint(
            PartialExecutionMode::ToNode,
            Some("first"),
            vec![item(9)],
            &Value::Null,
        )
        .unwrap();
    assert_eq!(
        to.workflow
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["trigger", "first"]
    );

    let node = source
        .fork_from_checkpoint(
            PartialExecutionMode::Node,
            Some("first"),
            vec![],
            &Value::Null,
        )
        .unwrap();
    assert_eq!(node.workflow.nodes.len(), 1);
    assert!(node.workflow.connections.is_empty());

    let mut direct = ExecutionMachine::new_partial(
        source.workflow.clone(),
        PartialExecutionMode::Node,
        "last",
        vec![item(7)],
    )
    .unwrap();
    assert_eq!(direct.workflow.nodes.len(), 1);
    assert_eq!(direct.workflow.nodes[0].id, "last");
    assert_eq!(
        direct.activations().next().unwrap().inputs["main"][0].json["value"],
        7
    );
    let activation = direct.next_ready().unwrap();
    direct.start_attempt(activation).unwrap();
    direct
        .complete(activation, BTreeMap::from([("main".into(), vec![item(7)])]))
        .unwrap();
    assert_eq!(direct.status(), RuntimeExecutionStatus::Succeeded);
    assert_eq!(direct.end_deliveries().len(), 1);
    assert_eq!(direct.end_deliveries()[0].target_port, "main");
    assert_eq!(direct.end_deliveries()[0].items[0].json["value"], 7);
}

#[test]
fn checkpoint_state_round_trips_through_json() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "nodes":[{"id":"trigger","type":"set","typeVersion":1,"name":"Root"}],
        "connections":[]
    }));
    let machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let value = serde_json::to_value(&machine).unwrap();
    let restored: ExecutionMachine = serde_json::from_value(value).unwrap();
    assert_eq!(restored.status(), RuntimeExecutionStatus::Running);
    assert_eq!(restored.activations().count(), 1);
}

#[test]
fn confirmation_wait_is_visible_and_resumes_the_same_activation() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "nodes":[
            {"id":"trigger","type":"set","typeVersion":1,"name":"Root",},
            {"id":"remote","type":"declarative_http","typeVersion":1,"name":"Remote","parameters":{"method":"GET","url":"http://node"}}
        ],
        "connections":[{"id":"start","sourceNodeId":"trigger","sourceHandle":"main","targetNodeId":"remote","targetHandle":"main","order":0}]
    }));
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let trigger = machine.next_ready().unwrap();
    machine.start_attempt(trigger).unwrap();
    machine
        .complete(trigger, BTreeMap::from([("main".into(), vec![item(1)])]))
        .unwrap();
    let activation = machine.next_ready().unwrap();
    machine.defer_for_confirmation(activation).unwrap();
    assert_eq!(machine.status(), RuntimeExecutionStatus::Waiting);
    assert_eq!(
        machine.activation(activation).unwrap().status,
        ActivationStatus::Waiting
    );
    machine.resume_confirmation();
    assert_eq!(machine.status(), RuntimeExecutionStatus::Running);
    assert_eq!(machine.next_ready(), Some(activation));
    assert_eq!(
        machine.activation(activation).unwrap().status,
        ActivationStatus::Ready
    );
}

#[test]
fn ordinary_cycle_stops_at_the_activation_budget() {
    let workflow = compile(json!({
        "schemaVersion":"8.0",
        "settings":{"activationBudget":7},
        "nodes":[
            {"id":"trigger","type":"set","typeVersion":1,"name":"Root",},
            {"id":"step","type":"set","typeVersion":1,"name":"Step",},
            {"id":"branch","type":"if","typeVersion":1,"name":"Branch","parameters":{"cases":[{"id":"c1","conditions":[{"condition":true}]}]}}
        ],
        "connections":[
            {"id":"start","sourceNodeId":"trigger","sourceHandle":"main","targetNodeId":"step","targetHandle":"main","order":0},
            {"id":"forward","sourceNodeId":"step","sourceHandle":"main","targetNodeId":"branch","targetHandle":"main","order":0},
            {"id":"back","sourceNodeId":"branch","sourceHandle":"case:c1","targetNodeId":"step","targetHandle":"main","order":0}
        ]
    }));
    let mut machine = ExecutionMachine::new(workflow, vec![item(1)]).unwrap();
    let error = loop {
        let activation = machine.next_ready().unwrap();
        let node = machine.workflow.nodes[machine.activation(activation).unwrap().node_index]
            .node_type
            .clone();
        machine.start_attempt(activation).unwrap();
        let port = if node == "if" { "case:c1" } else { "main" };
        if let Err(error) =
            machine.complete(activation, BTreeMap::from([(port.into(), vec![item(1)])]))
        {
            break error;
        }
    };
    assert_eq!(error, MachineError::ActivationBudgetExceeded(7));
    assert_eq!(machine.status(), RuntimeExecutionStatus::Failed);
    assert_eq!(machine.activations().count(), 7);
}
