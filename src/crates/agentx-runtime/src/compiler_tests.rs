use super::*;
use agentx_node_protocol::{
    Item, NodeCapability, OutputCardinality, PluginNodeBinding, PluginRuntimeArtifact,
    plugin_runtime_object_id,
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn reference(
    namespace: ValueNamespace,
    source_node_id: Option<&str>,
    port: Option<&str>,
    item: ValueSelection,
    path: &[&str],
) -> InputBinding {
    InputBinding::Reference {
        selector: ValueSelector {
            namespace,
            source_node_id: source_node_id.map(str::to_owned),
            port: port.map(str::to_owned),
            run: ValueSelection::Current,
            item,
            path: path
                .iter()
                .map(|value| ValuePathSegment::Key((*value).into()))
                .collect(),
        },
        missing_policy: MissingValuePolicy::Error,
    }
}

fn reference_json(
    namespace: ValueNamespace,
    source_node_id: Option<&str>,
    port: Option<&str>,
    item: ValueSelection,
    path: &[&str],
) -> serde_json::Value {
    serde_json::to_value(reference(namespace, source_node_id, port, item, path)).unwrap()
}

fn text_template(value: &str) -> serde_json::Value {
    json!({"kind":"template","segments":[{"kind":"text","text":value}]})
}

fn reference_template_json(
    namespace: ValueNamespace,
    source_node_id: Option<&str>,
    port: Option<&str>,
    item: ValueSelection,
    path: &[&str],
) -> serde_json::Value {
    let InputBinding::Reference {
        selector,
        missing_policy,
    } = reference(namespace, source_node_id, port, item, path)
    else {
        unreachable!()
    };
    json!({"kind":"template","segments":[{"kind":"reference","selector":selector,"missingPolicy":missing_policy}]})
}

#[test]
fn schema_compatibility_checks_array_items_and_required_object_fields() {
    assert!(json_schemas_compatible(
        &json!({"type":"number"}),
        &json!({"type":"integer"})
    ));
    assert!(!json_schemas_compatible(
        &json!({"type":"array","items":{"type":"number"}}),
        &json!({"type":"array","items":{"type":"string"}})
    ));
    assert!(!json_schemas_compatible(
        &json!({"type":"object","required":["id"],"properties":{"id":{"type":"integer"}}}),
        &json!({"type":"object","properties":{"name":{"type":"string"}}})
    ));
}

#[test]
fn condition_operator_is_checked_against_the_left_schema() {
    let mut definition = fixture();
    definition.start.inputs["properties"]["ok"] = json!({"type":"string"});
    definition.nodes[1].parameters["cases"][0]["conditions"][0]["condition"]["operator"] =
        json!("gt");
    let error = WorkflowCompiler::new(&NodeRegistry::m5_defaults())
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "CONDITION_OPERATOR_TYPE_MISMATCH")
    );
}

#[test]
fn removed_expression_binding_is_rejected() {
    assert!(
        serde_json::from_value::<InputBinding>(json!({
            "kind":"expression",
            "root":{"kind":"literal","value":1}
        }))
        .is_err()
    );
}

#[test]
fn code_output_example_derives_dynamic_nested_schema_and_requires_an_object() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}} ,"contexts":{}},
        "nodes":[
            {"id":"code","key":"code","type":"code","typeVersion":1,"name":"Code","parameters":{"runner":"python","inputs":{"kind":"object","fields":{}},"source":"def main(**inputs): return {}","outputExample":{"answer":null,"items":[],"nested":{"name":""},"homogeneous":[{"id":1},{"id":2}],"heterogeneous":[1,"two"]},"networkPolicy":{"mode":"deny","destinations":[]}},"contextWrites":[]},
            exit_node(exit_parameters(json!({}), json!({})))
        ],
        "connections":[
            {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"code","targetHandle":"main","order":0},
            {"id":"end","sourceNodeId":"code","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}}
    })).unwrap();
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let compiled = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();
    let schema = &compiled.nodes[0].effective_output_contract.port_schemas["main"]["properties"]["structuredOutput"];
    assert_eq!(schema["properties"]["answer"], json!({}));
    assert_eq!(schema["properties"]["items"]["items"], json!({}));
    assert_eq!(schema["properties"]["nested"]["required"], json!(["name"]));
    assert_eq!(
        schema["properties"]["nested"]["additionalProperties"],
        json!(false)
    );
    assert_eq!(
        schema["properties"]["homogeneous"]["items"]["properties"]["id"]["type"],
        json!("integer")
    );
    assert_eq!(schema["properties"]["heterogeneous"]["items"], json!({}));

    let mut invalid = definition;
    invalid.nodes[0].parameters["outputExample"] = json!([]);
    let error = compiler
        .compile(&invalid, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "CODE_OUTPUT_EXAMPLE_OBJECT_REQUIRED")
    );
}

#[test]
fn code_network_policy_allows_explicit_private_targets_but_rejects_protected_hosts() {
    let mut definition: WorkflowDefinition = serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"code","key":"code","type":"code","typeVersion":1,"name":"Code","parameters":{"runner":"python","inputs":{"kind":"object","fields":{}},"source":"def main(**inputs): return {}","outputExample":{},"networkPolicy":{"mode":"allowlist","destinations":[{"target":"10.0.0.0/8","ports":[{"from":5432,"to":5432}]}]}},"contextWrites":[]},
            exit_node(exit_parameters(json!({}), json!({})))
        ],
        "connections":[
            {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"code","targetHandle":"main","order":0},
            {"id":"end","sourceNodeId":"code","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}}
    })).unwrap();
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();

    definition.nodes[0].parameters["networkPolicy"]["destinations"][0]["target"] =
        json!("metadata.default.svc");
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "CODE_NETWORK_TARGET_FORBIDDEN")
    );
    assert!(
        compiler
            .validate_draft(&definition, &CompileContext::default())
            .iter()
            .any(|issue| issue.code == "CODE_NETWORK_TARGET_FORBIDDEN")
    );
}

#[test]
fn compiler_derives_loop_body_entries_sinks_and_parallelism_from_parent_ids() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{
            "inputs":{
                "type":"object",
                "properties":{"items":{"type":"array","items":{"type":"object","properties":{"value":{"type":"number"}},"required":["value"],"additionalProperties":false}}},
                "required":["items"],
                "additionalProperties":false
            },
            "contexts":{}
        },
        "nodes":[
            {"id":"loop","key":"loop","type":"loop_over_items","typeVersion":1,"name":"Loop","parameters":{"input":reference_json(ValueNamespace::Inputs,None,None,ValueSelection::Current,&["items"]),"outputSelector":reference_json(ValueNamespace::Outputs,Some("collect"),Some("main"),ValueSelection::Current,&[]),"parallelism":2,"errorMode":"terminate"},"contextWrites":[],"resourceReferences":[],"settings":{}},
            {"id":"slow","key":"slow","type":"declarative_http","typeVersion":1,"name":"Slow","parentId":"loop","parameters":{"method":"GET","url":text_template("http://echo-mcp/v1/plan5/delay"),"query":[],"headers":[]},"contextWrites":[],"resourceReferences":[],"settings":{"timeoutMs":30000}},
            {"id":"collect","key":"collect","type":"set","typeVersion":1,"name":"Collect","parentId":"loop","parameters":{"values":{"kind":"object","fields":{"value":reference_json(ValueNamespace::Loop,None,None,ValueSelection::Current,&["item","value"]),"all":reference_json(ValueNamespace::Loop,None,None,ValueSelection::Current,&["items"])}},"keepOnlySet":true},"contextWrites":[],"resourceReferences":[],"settings":{}},
            {"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","protected":true,"parameters":{"outputs":{},"errorOutputs":{}},"contextWrites":[],"resourceReferences":[],"settings":{}}
        ],
        "connections":[
            {"id":"start-loop","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"loop","targetHandle":"main","order":0},
            {"id":"slow-collect","sourceNodeId":"slow","sourceHandle":"main","targetNodeId":"collect","targetHandle":"main","order":0},
            {"id":"loop-exit","sourceNodeId":"loop","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"completion":"first_return","outputs":{},"error":{"outputs":{}}},
        "settings":{"executionOrder":"deterministic","activationBudget":100}
    })).unwrap();
    let compiled = WorkflowCompiler::new(&NodeRegistry::m5_defaults())
        .compile(&definition, &CompileContext::default())
        .unwrap();
    let loop_node = compiled
        .nodes
        .iter()
        .find(|node| node.id == "loop")
        .unwrap();
    let body = loop_node.loop_body.as_ref().expect("compiled loop body");
    assert_eq!(body.parallelism, 2);
    assert_eq!(
        loop_node.effective_output_contract.port_schemas["main"]["properties"]["items"]["items"]["properties"]
            ["value"]["type"],
        "number"
    );
    let slow = compiled
        .nodes
        .iter()
        .position(|node| node.id == "slow")
        .unwrap();
    let collect = compiled
        .nodes
        .iter()
        .position(|node| node.id == "collect")
        .unwrap();
    assert_eq!(body.entries, vec![slow]);
    assert_eq!(body.sinks, vec![collect]);

    let mut machine = crate::ExecutionMachine::new(
        compiled,
        vec![Item {
            json: json!({"items":[{"value":1},{"value":2},{"value":3}]}),
            ..Item::default()
        }],
    )
    .unwrap();
    let loop_activation = machine.next_ready().expect("loop ready");
    machine.start_attempt(loop_activation).unwrap();
    machine
        .complete(
            loop_activation,
            BTreeMap::from([(
                "main".into(),
                vec![Item {
                    json: json!({"items":[{"value":1},{"value":2},{"value":3}]}),
                    ..Item::default()
                }],
            )]),
        )
        .unwrap();
    let checkpoint = serde_json::to_value(&machine).unwrap();
    assert_eq!(checkpoint["pending_loops"].as_array().unwrap().len(), 1);
    assert_eq!(
        machine
            .activations()
            .filter(|activation| {
                activation.status == crate::ActivationStatus::Ready && activation.node_index == slow
            })
            .count(),
        2
    );
}

fn condition_literal(value: bool) -> serde_json::Value {
    json!({"left":{"kind":"literal","value":value},"operator":"eq","right":{"kind":"literal","value":true}})
}

fn exit_node(parameters: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End",
        "protected":true,"parameters":parameters
    })
}

fn exit_parameters(
    outputs: serde_json::Value,
    error_outputs: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({"outputs":outputs,"errorOutputs":error_outputs})
}

fn fixture() -> WorkflowDefinition {
    serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{"ok":{"type":"boolean"}},"additionalProperties":false},"contexts":{}},
            "settings":{"activationBudget":20,"executionOrder":"deterministic"},
            "nodes":[
                {"id":"root","key":"root","type":"set","typeVersion":1,"name":"Root","contextWrites":[]},
                {"id":"if","key":"condition","type":"if","typeVersion":1,"name":"IF","parameters":{"cases":[{"id":"c1","conditions":[{"condition":{"left":reference_json(ValueNamespace::Item,None,None,ValueSelection::Current,&["ok"]),"operator":"eq","right":{"kind":"literal","value":true}}}]}]},"contextWrites":[]},
                {"id":"merge","key":"merge","type":"merge","typeVersion":1,"name":"Merge","contextWrites":[]},
                {"id":"loop","key":"loop","type":"set","typeVersion":1,"name":"Cycle step","parameters":{"values":{"kind":"object","fields":{}}},"contextWrites":[]},
                exit_node(exit_parameters(json!({}), json!({})))
            ],
            "connections":[
                {"id":"__start__-root","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"root","targetHandle":"main","order":0},
                {"id":"a","sourceNodeId":"root","sourceHandle":"main","targetNodeId":"if","targetHandle":"main","order":0},
                {"id":"b","sourceNodeId":"if","sourceHandle":"case:c1","targetNodeId":"merge","targetHandle":"main:0","order":0},
                {"id":"c","sourceNodeId":"if","sourceHandle":"else","targetNodeId":"merge","targetHandle":"main:1","order":1},
                {"id":"d","sourceNodeId":"merge","sourceHandle":"main","targetNodeId":"loop","targetHandle":"main","order":0},
                {"id":"e","sourceNodeId":"loop","sourceHandle":"main","targetNodeId":"merge","targetHandle":"main:2","order":0},
                {"id":"loop-end","sourceNodeId":"loop","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":1}
            ],
            "end":{"outputs":{}}
        })).unwrap()
}

#[test]
fn workflow_uses_one_immutable_version_of_each_plugin_package() {
    let mut registry = NodeRegistry::m5_defaults();
    for (node_type, package_version, digest) in [
        (
            "acme.mapper_v1",
            "1.0.0",
            format!("sha256:{}", "a".repeat(64)),
        ),
        (
            "acme.mapper_v2",
            "2.0.0",
            format!("sha256:{}", "b".repeat(64)),
        ),
    ] {
        let mut manifest = registry.get("set", 1).unwrap().clone();
        manifest.node_type = node_type.into();
        manifest.capability = NodeCapability::PluginNodejs;
        let runtime_source = "export async function execute(){}";
        manifest.plugin = Some(PluginNodeBinding {
            package_id: "acme/mapper".into(),
            package_version: package_version.into(),
            bundle_digest: digest.clone(),
            runtime_entry: "runtime/entry.js".into(),
            runtime_source: runtime_source.into(),
            runtime_artifact: Some(PluginRuntimeArtifact {
                object_id: plugin_runtime_object_id(&digest),
                content_hash: format!("sha256:{:x}", Sha256::digest(runtime_source.as_bytes())),
                size_bytes: runtime_source.len() as u64,
                media_type: "text/javascript".into(),
            }),
            ui_entry: None,
            ui_source: None,
            ui_styles: None,
            ui_assets: BTreeMap::new(),
            trace_renderers: vec![],
        });
        registry.register(manifest).unwrap();
    }
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"v1","key":"v1","type":"acme.mapper_v1","typeVersion":1,"name":"Mapper v1","parameters":{},"contextWrites":[]},
            {"id":"v2","key":"v2","type":"acme.mapper_v2","typeVersion":1,"name":"Mapper v2","parameters":{},"contextWrites":[]},
            exit_node(exit_parameters(json!({}), json!({})))
        ],
        "connections":[
            {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"v1","targetHandle":"main","order":0},
            {"id":"next","sourceNodeId":"v1","sourceHandle":"main","targetNodeId":"v2","targetHandle":"main","order":0},
            {"id":"end","sourceNodeId":"v2","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}}
    }))
    .unwrap();
    let error = WorkflowCompiler::new(&registry)
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(error.issues.iter().any(|issue| {
        issue.code == "PLUGIN_PACKAGE_VERSION_CONFLICT" && issue.path == "nodes[1].typeVersion"
    }));
}

#[test]
fn per_node_resolved_plugin_manifest_freezes_dynamic_ports_and_schema() {
    let registry = NodeRegistry::m5_defaults();
    let mut definition = fixture();
    definition
        .nodes
        .retain(|node| matches!(node.id.as_str(), "root" | "exit"));
    definition.connections = vec![
        serde_json::from_value(json!({"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"root","targetHandle":"main","order":0})).unwrap(),
        serde_json::from_value(json!({"id":"end","sourceNodeId":"root","sourceHandle":"metadata","targetNodeId":"exit","targetHandle":"main","order":0})).unwrap(),
    ];
    let mut resolved = registry.get("set", 1).unwrap().clone();
    resolved.output_ports.push(agentx_node_protocol::NodePort {
        name: "metadata".into(),
        kind: agentx_node_protocol::PortKind::Main,
        required: false,
        variadic: false,
    });
    resolved.output_port_schemas.insert(
        "metadata".into(),
        json!({"type":"object","properties":{"mapped":{"type":"integer"}},"required":["mapped"],"additionalProperties":false}),
    );
    let overrides = std::collections::BTreeMap::from([("root".into(), resolved)]);
    let compiled = WorkflowCompiler::new(&registry)
        .with_resolved_manifests(&overrides)
        .compile(&definition, &CompileContext::default())
        .unwrap();
    assert_eq!(
        compiled.nodes[0].effective_output_contract.port_schemas["metadata"]["properties"]["mapped"]
            ["type"],
        "integer"
    );
}

fn agent_fixture(with_sandbox: bool, with_mcp_attachment: bool) -> WorkflowDefinition {
    let mut references = vec![serde_json::json!({
        "resourceType":"model",
        "bindingRole":"model",
        "resourceId":"11111111-1111-4111-8111-111111111111",
        "resourceVersionId":"22222222-2222-4222-8222-222222222222",
        "operation":"use"
    })];
    if with_sandbox {
        references.push(serde_json::json!({
            "resourceType":"sandbox_profile",
            "bindingRole":"workspace_sandbox",
            "resourceId":"33333333-3333-4333-8333-333333333333",
            "resourceVersionId":"44444444-4444-4444-8444-444444444444",
            "operation":"use"
        }));
    }
    if with_mcp_attachment {
        references.push(serde_json::json!({
            "bindingRole":"mcp_tools",
            "resourceType":"mcp_tool",
            "resourceId":"55555555-5555-4555-8555-555555555555",
            "resourceVersionId":"66666666-6666-4666-8666-666666666666",
            "operation":"use"
        }));
    }
    serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[{
            "id":"agent","key":"agent","type":"agent","typeVersion":2,"name":"Agent",
            "parameters":{"sessionPolicy":{"mode":"invocation"}},
            "resourceReferences":references,"contextWrites":[]
        },exit_node(exit_parameters(json!({}), json!({})))],
        "connections":[
            {"id":"start-agent","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"agent","targetHandle":"main","order":0},
            {"id":"agent-end","sourceNodeId":"agent","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}}
    }))
    .expect("Agent Definition 8.0 fixture")
}

#[test]
fn agent_ir_derives_core_tools_only_from_the_workspace_sandbox() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);

    let without_sandbox = compiler
        .compile(&agent_fixture(false, false), &CompileContext::default())
        .expect("Agent without Workspace Sandbox compiles");
    let agent = without_sandbox.nodes[0].agent.as_ref().expect("Agent IR");
    assert!(agent.workspace_sandbox.is_none());
    assert!(agent.core_tools.is_empty());

    let with_sandbox = compiler
        .compile(&agent_fixture(true, false), &CompileContext::default())
        .expect("Agent with Workspace Sandbox compiles");
    let agent = with_sandbox.nodes[0].agent.as_ref().expect("Agent IR");
    let names = agent
        .core_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["read", "write", "edit", "bash"]);
    assert!(agent.core_tools.iter().all(|tool| {
        tool.workspace_sandbox_resource_id
            == uuid::Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap()
            && tool.workspace_sandbox_version_id
                == uuid::Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap()
    }));
}

#[test]
fn agent_core_tool_name_conflict_exists_only_when_core_tools_are_derived() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let context = CompileContext {
        resource_tool_names: BTreeMap::from([(
            uuid::Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap(),
            "read".into(),
        )]),
        ..CompileContext::default()
    };

    compiler
        .compile(&agent_fixture(false, true), &context)
        .expect("MCP read tool is valid when no core tools are registered");
    let error = compiler
        .compile(&agent_fixture(true, true), &context)
        .expect_err("MCP read tool conflicts with the derived core read tool");
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "AGENT_CORE_TOOL_NAME_CONFLICT")
    );
}

#[test]
fn compilation_is_deterministic_and_marks_cycles() {
    let registry = NodeRegistry::m4_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let first = compiler
        .compile(&fixture(), &CompileContext::default())
        .unwrap();
    let second = compiler
        .compile(&fixture(), &CompileContext::default())
        .unwrap();
    assert_eq!(first.canonical_hash, second.canonical_hash);
    assert!(
        first
            .connections
            .iter()
            .any(|connection| connection.back_edge)
    );
    assert_eq!(first.connections[1].branch_order, 0);
    assert_eq!(first.connections[2].branch_order, 1);
}

#[test]
fn rejects_unknown_ports_and_recursive_subworkflows() {
    let registry = NodeRegistry::m4_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition = fixture();
    definition
        .connections
        .iter_mut()
        .find(|connection| connection.id == "a")
        .expect("fixture edge")
        .source_handle = "missing".into();
    definition.nodes.insert(4, serde_json::from_value(serde_json::json!({
            "id":"sub","key":"sub","type":"sub_workflow","typeVersion":1,"name":"Sub","parameters":{"workflowVersionId":"version-a"},"contextWrites":[]
        })).unwrap());
    definition.connections.push(serde_json::from_value(serde_json::json!({
            "id":"sub-edge","sourceNodeId":"loop","sourceHandle":"main","targetNodeId":"sub","targetHandle":"main","order":1
        })).unwrap());
    let error = compiler
        .compile(
            &definition,
            &CompileContext {
                current_workflow_version_id: Some("version-a".into()),
                ancestor_workflow_version_ids: BTreeSet::new(),
                resource_tool_names: BTreeMap::new(),
            },
        )
        .unwrap_err();
    let codes = error
        .issues
        .iter()
        .map(|issue| issue.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"UNKNOWN_SOURCE_PORT"));
    assert!(codes.contains(&"RECURSIVE_SUBWORKFLOW"));
}

#[test]
fn rejects_undeclared_parameters_and_requires_the_new_loop_contract() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition = fixture();
    definition.nodes[3].parameters = serde_json::json!({"batchSize":0});
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "INVALID_NODE_PARAMETERS"
                && issue.path.starts_with("nodes[3].parameters"))
    );

    definition.nodes[3].node_type = "loop_over_items".into();
    definition.nodes[3].parameters = serde_json::json!({});
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(error.issues.iter().any(|issue| {
        issue.code == "INVALID_NODE_PARAMETERS"
            && (issue.message.contains("input") || issue.message.contains("outputSelector"))
    }));
}

#[test]
fn resource_capabilities_cannot_be_compiled_as_standalone_nodes() {
    let mut definition = fixture();
    definition.nodes[0].node_type = "mcp_tool".into();
    let error = WorkflowCompiler::new(&NodeRegistry::m5_defaults())
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(error.issues.iter().any(|issue| {
        issue.code == "RESOURCE_CAPABILITY_NODE_REMOVED" && issue.path == "nodes[0].type"
    }));
}

#[test]
fn validates_nested_parameter_bindings_against_the_leaf_schema() {
    let registry = NodeRegistry::m5_defaults();
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","required":["question"],"properties":{"question":{"type":"string"}},"additionalProperties":false},"contexts":{}},
        "nodes":[{
            "id":"model","key":"model","type":"model","typeVersion":1,"name":"Model",
            "parameters":{"prompt":text_template("Answer the question"),"userQuestion":{"kind":"template","segments":[{"kind":"reference","selector":{"namespace":"inputs","run":{"kind":"current"},"item":{"kind":"current"},"path":["question"]},"missingPolicy":{"kind":"error"}}]}},
            "contextWrites":[]
        },exit_node(exit_parameters(json!({}), json!({})))],
        "connections":[
            {"id":"start-model","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"model","targetHandle":"main","order":0},
            {"id":"model-end","sourceNodeId":"model","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}}
    }))
    .unwrap();

    WorkflowCompiler::new(&registry)
        .compile(&definition, &CompileContext::default())
        .unwrap();

    let mut restricted = registry.get("model", 1).unwrap().clone();
    restricted.parameter_schema["properties"]["userQuestion"]
        .as_object_mut()
        .unwrap()
        .remove("x-agentx-binding");
    let mut restricted_registry = NodeRegistry::default();
    restricted_registry.register(restricted).unwrap();
    let error = WorkflowCompiler::new(&restricted_registry)
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(error.issues.iter().any(|issue| {
        issue.code == "PARAMETER_BINDING_NOT_ALLOWED"
            && issue.path == "nodes[0].parameters.userQuestion"
    }));
}

#[test]
fn standalone_model_reference_does_not_use_agent_binding_slots() {
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[{
            "id":"model","key":"model","type":"model","typeVersion":1,"name":"Model",
            "parameters":{"prompt":text_template("Answer with the configured model"),"userQuestion":text_template("question")},
            "resourceReferences":[{
                "resourceType":"model",
                "resourceId":"11111111-1111-4111-8111-111111111111",
                "resourceVersionId":null,
                "operation":"use"
            }],
            "contextWrites":[]
        },exit_node(exit_parameters(json!({}), json!({})))],
        "connections":[
            {"id":"start-model","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"model","targetHandle":"main","order":0},
            {"id":"model-end","sourceNodeId":"model","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}}
    }))
    .expect("standalone Model Definition 8.0 fixture");

    let compiled = WorkflowCompiler::new(&NodeRegistry::m5_defaults())
        .compile(&definition, &CompileContext::default())
        .expect("standalone Model resource reference compiles");
    assert!(compiled.nodes[0].agent.is_none());
}

#[test]
fn rejects_structured_parameter_references_from_disallowed_namespaces() {
    let registry = NodeRegistry::m5_defaults();
    let mut restricted = registry.get("model", 1).unwrap().clone();
    restricted.parameter_schema["properties"]["userQuestion"]["x-agentx-binding"]["allowedNamespaces"] =
        serde_json::json!(["outputs"]);
    let mut restricted_registry = NodeRegistry::default();
    restricted_registry.register(restricted).unwrap();

    let mut definition = fixture();
    definition.nodes[0].node_type = "model".into();
    definition.nodes[0].parameters["userQuestion"] = json!({"kind":"template","segments":[{"kind":"reference","selector":{"namespace":"inputs","run":{"kind":"current"},"item":{"kind":"current"},"path":["question"]},"missingPolicy":{"kind":"error"}}]});

    let error = WorkflowCompiler::new(&restricted_registry)
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(error.issues.iter().any(|issue| {
        issue.code == "BINDING_NAMESPACE_NOT_ALLOWED"
            && issue.path == "nodes[0].parameters.userQuestion"
    }));
}

#[test]
fn roots_are_declared_by_start_connections() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{},"additionalProperties":false},"contexts":{}},
            "nodes":[
                {"id":"root","key":"root","type":"set","typeVersion":1,"name":"Root","contextWrites":[]},
                {"id":"set","key":"set","type":"set","typeVersion":1,"name":"Set","parameters":{"values":{"kind":"object","fields":{"ok":{"kind":"literal","value":true}}}},"contextWrites":[]},
                exit_node(exit_parameters(json!({}), json!({})))
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"root","targetHandle":"main","order":0},
                {"id":"root-set","sourceNodeId":"root","sourceHandle":"main","targetNodeId":"set","targetHandle":"main","order":0},
                {"id":"end","sourceNodeId":"set","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{}}
        }))
        .unwrap();

    let compiled = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();
    assert_eq!(compiled.start_nodes, vec![0]);
}

#[test]
fn rejects_a_node_without_an_explicit_start_or_end() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{},"additionalProperties":false},"contexts":{}},
            "nodes":[{"id":"set","key":"set","type":"set","typeVersion":1,"name":"Set","parameters":{"values":{"kind":"object","fields":{}}},"contextWrites":[]}],
            "connections":[],
            "end":{"outputs":{}}
        }))
        .unwrap();

    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "START_REQUIRED")
    );
}

#[test]
fn container_body_sinks_reach_the_end_through_their_container() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{"items":{"type":"array","items":{"type":"object"}}}},"contexts":{}},
        "nodes":[
            {"id":"loop","key":"loop","type":"loop_over_items","typeVersion":1,"name":"Loop","parameters":{"input":reference_json(ValueNamespace::Inputs,None,None,ValueSelection::Current,&["items"]),"outputSelector":reference_json(ValueNamespace::Outputs,Some("body"),Some("main"),ValueSelection::Current,&[]),"errorMode":"remove","parallelism":5},"contextWrites":[]},
            {"id":"body","key":"body","type":"set","typeVersion":1,"name":"Body","parentId":"loop","parameters":{"values":{"kind":"object","fields":{}},"keepOnlySet":false},"contextWrites":[]},
            exit_node(exit_parameters(json!({}), json!({})))
        ],
        "connections":[
            {"id":"start-loop","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"loop","targetHandle":"main","order":0},
            {"id":"loop-end","sourceNodeId":"loop","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":1}
        ],
        "end":{"outputs":{}}
    }))
    .unwrap();
    let compiled = compiler
        .compile(&definition, &CompileContext::default())
        .expect("a body sink converges through its container, so the main path reaches the exit");
    assert!(
        compiled
            .nodes
            .iter()
            .any(|node| node.id == "body" && node.container.as_deref() == Some("loop"))
    );
}

#[test]
fn container_edges_may_not_cross_the_loop_boundary() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"trigger","key":"trigger","type":"set","typeVersion":1,"name":"Trigger","contextWrites":[]},
            {"id":"loop","key":"loop","type":"loop_over_items","typeVersion":1,"name":"Loop","contextWrites":[]},
            {"id":"body","key":"body","type":"set","typeVersion":1,"name":"Body","parentId":"loop","contextWrites":[]},
            {"id":"outside","key":"outside","type":"set","typeVersion":1,"name":"Outside","contextWrites":[]},
            exit_node(exit_parameters(json!({}), json!({})))
        ],
        "connections":[
            {"id":"start-trigger","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"trigger","targetHandle":"main","order":0},
            {"id":"trigger-loop","sourceNodeId":"trigger","sourceHandle":"main","targetNodeId":"loop","targetHandle":"main","order":0},
            {"id":"loop-body","sourceNodeId":"loop","sourceHandle":"main","targetNodeId":"body","targetHandle":"main","order":0},
            {"id":"body-outside","sourceNodeId":"body","sourceHandle":"main","targetNodeId":"outside","targetHandle":"main","order":0},
            {"id":"outside-end","sourceNodeId":"outside","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}}
    }))
    .unwrap();
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error.issues.iter().any(|issue| {
            issue.code == "CONTAINER_EDGE_CROSSES_BOUNDARY"
                && issue.message.contains("body")
                && issue.message.contains("outside")
        }),
        "the cross-boundary error must name both endpoints"
    );
}

#[test]
fn loop_references_are_only_valid_inside_the_container_body() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"trigger","key":"trigger","type":"set","typeVersion":1,"name":"Trigger","contextWrites":[]},
            {"id":"loop","key":"loop","type":"loop_over_items","typeVersion":1,"name":"Loop","contextWrites":[]},
            {"id":"body","key":"body","type":"declarative_http","typeVersion":1,"name":"Body","parentId":"loop","parameters":{"method":"GET","url":{"kind":"template","segments":[{"kind":"reference","selector":{"namespace":"loop","run":{"kind":"current"},"item":{"kind":"current"},"path":["index"]},"missingPolicy":{"kind":"error"}}]}},"contextWrites":[]},
            {"id":"outside","key":"outside","type":"declarative_http","typeVersion":1,"name":"Outside","parameters":{"method":"GET","url":{"kind":"template","segments":[{"kind":"reference","selector":{"namespace":"loop","run":{"kind":"current"},"item":{"kind":"current"},"path":["index"]},"missingPolicy":{"kind":"error"}}]}},"contextWrites":[]},
            exit_node(exit_parameters(json!({}), json!({})))
        ],
        "connections":[
            {"id":"start-trigger","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"trigger","targetHandle":"main","order":0},
            {"id":"trigger-loop","sourceNodeId":"trigger","sourceHandle":"main","targetNodeId":"loop","targetHandle":"main","order":0},
            {"id":"loop-body","sourceNodeId":"loop","sourceHandle":"main","targetNodeId":"body","targetHandle":"main","order":0},
            {"id":"loop-outside","sourceNodeId":"trigger","sourceHandle":"main","targetNodeId":"outside","targetHandle":"main","order":1},
            {"id":"outside-end","sourceNodeId":"outside","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}}
    }))
    .unwrap();
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "LOOP_REFERENCE_OUTSIDE_ITERATION"),
        "issues were: {:?}",
        error
            .issues
            .iter()
            .map(|issue| issue.code.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn error_connections_do_not_change_the_explicit_end_contract() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{},"additionalProperties":false},"contexts":{}},
            "nodes":[
                {"id":"if","key":"condition","type":"if","typeVersion":1,"name":"Condition","parameters":{"cases":[{"id":"c1","conditions":[{"condition":condition_literal(true)}]}]},"contextWrites":[],"settings":{}},
                {"id":"handler","key":"handler","type":"set","typeVersion":1,"name":"Recovery","parameters":{},"contextWrites":[]},
                exit_node(exit_parameters(
                    json!({"answer":reference_json(ValueNamespace::Outputs,Some("if"),Some("case:c1"),ValueSelection::First,&[])}),
                    json!({})
                ))
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"if","targetHandle":"main","order":0},
                {"id":"normal","sourceNodeId":"if","sourceHandle":"case:c1","targetNodeId":"handler","targetHandle":"main","order":0},
                {"id":"end","sourceNodeId":"handler","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0},
                {"id":"error","sourceNodeId":"if","sourceHandle":"error","targetNodeId":"exit","targetHandle":"error","order":0}
            ],
            "end":{"outputs":{"answer":{"schema":{"type":"object"},"required":false}}}
        }))
        .unwrap();

    let compiled = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();
    assert_eq!(compiled.end.outputs.len(), 1);
    let error_terminal = compiled
        .terminal_connections
        .iter()
        .find(|edge| edge.id == "error")
        .unwrap();
    assert_eq!(error_terminal.source_port, "error");
    assert_eq!(error_terminal.target_port, "error");
}

#[test]
fn error_only_nodes_do_not_require_a_main_path_to_end() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"success","key":"success","type":"set","typeVersion":1,"name":"Success","parameters":{},"contextWrites":[],"settings":{}},
            exit_node(exit_parameters(json!({}), json!({})))
        ],
        "connections":[
            {"id":"start-success","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"success","targetHandle":"main","order":0},
            {"id":"success-end","sourceNodeId":"success","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0},
            {"id":"success-error","sourceNodeId":"success","sourceHandle":"error","targetNodeId":"exit","targetHandle":"error","order":0}
        ],
        "end":{"outputs":{},"error":{"outputs":{}}}
    })).unwrap();

    let compiled = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();
    assert_eq!(compiled.start_nodes.len(), 1);
    assert!(
        compiled
            .terminal_connections
            .iter()
            .any(|connection| connection.target_port == "error")
    );
}

#[test]
fn end_error_outputs_accept_fixed_error_item_fields() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
            "nodes":[
                {"id":"branch","key":"branch","type":"if","typeVersion":1,"name":"Branch","parameters":{"cases":[{"id":"c1","conditions":[{"condition":condition_literal(true)}]}]},"contextWrites":[]},
                exit_node(exit_parameters(
                    json!({}),
                    json!({"message":reference_json(ValueNamespace::Item,None,None,ValueSelection::Current,&["message"])})
                ))
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"branch","targetHandle":"main","order":0},
                {"id":"main","sourceNodeId":"branch","sourceHandle":"case:c1","targetNodeId":"exit","targetHandle":"main","order":0},
                {"id":"error","sourceNodeId":"branch","sourceHandle":"error","targetNodeId":"exit","targetHandle":"error","order":0}
            ],
            "end":{"outputs":{},"error":{"outputs":{"message":{"schema":{"type":"string"},"required":true}}}}
        })).unwrap();
    compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();
    definition
        .end
        .error
        .outputs
        .get_mut("message")
        .unwrap()
        .schema = serde_json::json!({"type":"number"});
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(error.issues.iter().any(|issue| {
        issue.code == "END_OUTPUT_TYPE_MISMATCH"
            && issue.path == "nodes[1].parameters.errorOutputs.message"
    }));
}

#[test]
fn end_error_outputs_reject_unknown_error_item_fields() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
            "nodes":[
                {"id":"branch","key":"branch","type":"if","typeVersion":1,"name":"Branch","parameters":{"cases":[{"id":"c1","conditions":[{"condition":condition_literal(true)}]}]},"contextWrites":[]},
                exit_node(exit_parameters(
                    json!({}),
                    json!({"bad":reference_json(ValueNamespace::Item,None,None,ValueSelection::Current,&["notAField"])})
                ))
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"branch","targetHandle":"main","order":0},
                {"id":"main","sourceNodeId":"branch","sourceHandle":"case:c1","targetNodeId":"exit","targetHandle":"main","order":0},
                {"id":"error","sourceNodeId":"branch","sourceHandle":"error","targetNodeId":"exit","targetHandle":"error","order":0}
            ],
            "end":{"outputs":{},"error":{"outputs":{"bad":{"schema":{"type":"string"}}}}}
        })).unwrap();
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "UNKNOWN_ERROR_ITEM_REFERENCE")
    );

    definition.nodes[1].parameters["errorOutputs"]["bad"] = reference_json(
        ValueNamespace::Item,
        None,
        None,
        ValueSelection::Current,
        &["code"],
    );
    compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();

    for removed in ["sourceNodeKey", "runIndex", "iterationIndex"] {
        definition.nodes[1].parameters["errorOutputs"]["bad"] = reference_json(
            ValueNamespace::Item,
            None,
            None,
            ValueSelection::Current,
            &[removed],
        );
        let error = compiler
            .compile(&definition, &CompileContext::default())
            .unwrap_err();
        assert!(
            error
                .issues
                .iter()
                .any(|issue| issue.code == "UNKNOWN_ERROR_ITEM_REFERENCE"),
            "removed error field {removed} must not compile"
        );
    }
}

#[test]
fn end_error_output_can_use_only_common_error_predecessor_outputs() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let base = serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"root","key":"root","type":"set","typeVersion":1,"name":"Root","contextWrites":[]},
            {"id":"a","key":"a","type":"set","typeVersion":1,"name":"A","contextWrites":[]},
            {"id":"b","key":"b","type":"set","typeVersion":1,"name":"B","contextWrites":[]},
            {"id":"merge","key":"merge","type":"merge","typeVersion":1,"name":"Merge","contextWrites":[]},
            exit_node(exit_parameters(
                json!({}),
                json!({"value":reference_json(ValueNamespace::Outputs,Some("root"),Some("main"),ValueSelection::First,&[])})
            ))
        ],
        "connections":[
            {"id":"start-root","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"root","targetHandle":"main","order":0},
            {"id":"root-a","sourceNodeId":"root","sourceHandle":"main","targetNodeId":"a","targetHandle":"main","order":0},
            {"id":"root-b","sourceNodeId":"root","sourceHandle":"main","targetNodeId":"b","targetHandle":"main","order":1},
            {"id":"a-merge","sourceNodeId":"a","sourceHandle":"main","targetNodeId":"merge","targetHandle":"main:0","order":0},
            {"id":"b-merge","sourceNodeId":"b","sourceHandle":"main","targetNodeId":"merge","targetHandle":"main:1","order":1},
            {"id":"merge-main","sourceNodeId":"merge","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0},
            {"id":"a-error","sourceNodeId":"a","sourceHandle":"error","targetNodeId":"exit","targetHandle":"error","order":0},
            {"id":"b-error","sourceNodeId":"b","sourceHandle":"error","targetNodeId":"exit","targetHandle":"error","order":1}
        ],
        "end":{"outputs":{},"error":{"outputs":{"value":{"schema":{"type":"object"}}}}}
    });
    let mut definition: WorkflowDefinition = serde_json::from_value(base).unwrap();
    compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();

    definition.nodes[4].parameters["errorOutputs"]["value"] = reference_json(
        ValueNamespace::Outputs,
        Some("a"),
        Some("main"),
        ValueSelection::First,
        &[],
    );
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "ERROR_OUTPUT_NOT_COMMON_PREDECESSOR")
    );
}

#[test]
fn context_writes_can_read_native_output_and_validate_operation_type() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{}},"contexts":{"answer":{"schema":{"type":"string"},"default":"","mutable":true,"scope":"execution_tree"}}},
            "nodes":[
                {"id":"http","key":"http","type":"declarative_http","typeVersion":1,"name":"HTTP","parameters":{"url":text_template("https://example.invalid")},"contextWrites":[{"operation":"set","path":"answer","value":reference_json(ValueNamespace::Outputs,Some("http"),Some("main"),ValueSelection::Current,&["body"])}]},
                exit_node(exit_parameters(json!({}), json!({})))
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"http","targetHandle":"main","order":0},
                {"id":"end","sourceNodeId":"http","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{}}
        })).unwrap();
    compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();

    definition.nodes[0].context_writes[0].operation = ContextWriteOperation::Append;
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| { issue.code == "CONTEXT_WRITE_OPERATION_TYPE_MISMATCH" })
    );
}

#[test]
fn rejects_end_outputs_that_reference_unknown_nodes() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition = fixture();
    definition.nodes[4].parameters = exit_parameters(
        json!({"answer":reference_json(ValueNamespace::Outputs,Some("missing"),Some("main"),ValueSelection::First,&["answer"])}),
        json!({}),
    );
    definition.end = serde_json::from_value(serde_json::json!({
        "outputs":{"answer":{"schema":{"type":"string"},"required":true}}
    }))
    .unwrap();
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "OUTPUT_REFERENCE_NOT_FOUND")
    );
}

#[test]
fn validates_run_item_and_output_schema_paths() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
            "nodes":[
                {"id":"http","key":"http","type":"declarative_http","typeVersion":1,"name":"HTTP","parameters":{"url":text_template("https://example.invalid")},"contextWrites":[]},
                exit_node(exit_parameters(
                    json!({"answer":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"http","port":"main","run":{"kind":"index","index":0},"item":{"kind":"index","index":0},"path":["missing"]},"missingPolicy":{"kind":"error"}}}),
                    json!({})
                ))
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"http","targetHandle":"main","order":0},
                {"id":"end","sourceNodeId":"http","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{"answer":{"schema":{"type":"string"},"required":false}}}
        }))
        .unwrap();
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "UNKNOWN_OUTPUT_FIELD")
    );
}

#[test]
fn removed_ai_output_fields_require_reselecting_text() {
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{},"additionalProperties":false},"contexts":{}},
        "nodes":[
            {"id":"model","key":"model","type":"model","typeVersion":1,"name":"Model","parameters":{"prompt":text_template("system"),"userQuestion":text_template("question")},"contextWrites":[]},
            exit_node(exit_parameters(
                json!({"answer":reference_json(ValueNamespace::Outputs,Some("model"),Some("main"),ValueSelection::First,&["message"])}),
                json!({})
            ))
        ],
        "connections":[
            {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"model","targetHandle":"main","order":0},
            {"id":"end","sourceNodeId":"model","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{"answer":{"schema":{"type":"string"},"required":true}}}
    }))
    .unwrap();
    let error = WorkflowCompiler::new(&NodeRegistry::m5_defaults())
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    let issue = error
        .issues
        .iter()
        .find(|issue| issue.code == "UNKNOWN_OUTPUT_FIELD")
        .expect("removed AI field issue");
    assert!(issue.message.contains("no longer exists"));
    assert!(issue.message.contains("'text'"));
}

#[test]
fn end_string_output_preserves_reference_binding_in_ir() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"http","key":"http","type":"declarative_http","typeVersion":1,"name":"HTTP","parameters":{"url":text_template("https://example.invalid")},"contextWrites":[]},
            exit_node(exit_parameters(
                json!({"answer":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"http","port":"main","run":{"kind":"current"},"item":{"kind":"first"},"path":["body"]},"missingPolicy":{"kind":"error"}}}),
                json!({})
            ))
        ],
        "connections":[{"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"http","targetHandle":"main","order":0},{"id":"end","sourceNodeId":"http","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}],
        "end":{"outputs":{"answer":{"schema":{"type":"string"},"required":true}}}
    })).unwrap();
    let compiled = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();
    assert!(matches!(
        compiled.exits["exit"].outputs["answer"],
        InputBinding::Reference { .. }
    ));
}

#[test]
fn http_body_string_is_rejected_for_object_end_output() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"http","key":"http","type":"declarative_http","typeVersion":1,"name":"HTTP","parameters":{"url":text_template("https://example.invalid")},"contextWrites":[]},
            exit_node(exit_parameters(
                json!({"answer":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"http","port":"main","run":{"kind":"current"},"item":{"kind":"first"},"path":["body"]},"missingPolicy":{"kind":"error"}}}),
                json!({})
            ))
        ],
        "connections":[{"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"http","targetHandle":"main","order":0},{"id":"end","sourceNodeId":"http","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}],
        "end":{"outputs":{"answer":{"schema":{"type":"object"},"required":true}}}
    })).unwrap();
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "END_OUTPUT_TYPE_MISMATCH")
    );
}

#[test]
fn freezes_approval_decision_port_schema_and_accepts_decision_reference() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[{
            "id":"approval","key":"approval","type":"approval","typeVersion":1,
            "name":"Approval","parameters":{"title":text_template("Review"),"candidateUserId":"018f0000-0000-7000-8000-000000000001"},
            "contextWrites":[]
        },exit_node(exit_parameters(
            json!({"decision":reference_json(ValueNamespace::Outputs,Some("approval"),Some("decision:approved"),ValueSelection::Current,&["decision"])}),
            json!({})
        ))],
        "connections":[
            {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"approval","targetHandle":"main","order":0},
            {"id":"end","sourceNodeId":"approval","sourceHandle":"decision:approved","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{"decision":{
            "schema":{"type":"string","enum":["approved"]},
            "required":false
        }}}
    }))
    .unwrap();

    let compiled = compiler
        .compile(&definition, &CompileContext::default())
        .expect("approval decision reference compiles");
    let contract = &compiled.nodes[0].effective_output_contract;
    assert!(!contract.port_schemas.contains_key("decision"));
    let schema = &contract.port_schemas["decision:approved"];
    assert_eq!(
        schema["properties"]["decision"]["enum"],
        serde_json::json!(["approved"])
    );
    assert_eq!(
        contract.port_schemas["decision:rejected"]["properties"]["decision"]["enum"],
        serde_json::json!(["rejected"])
    );
    assert_eq!(
        schema["required"],
        serde_json::json!(["taskId", "decision", "decidedBy", "reason", "input"])
    );
    assert_eq!(
        contract.cardinalities["decision:approved"],
        OutputCardinality::ZeroOrOne
    );
}

#[test]
fn rejects_nullable_branch_selection_for_required_end_output() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
            "nodes":[
                {"id":"branch","key":"branch","type":"if","typeVersion":1,"name":"Branch","parameters":{"cases":[{"id":"c1","conditions":[{"condition":condition_literal(true)}]}]},"contextWrites":[]},
                exit_node(exit_parameters(
                    json!({"answer":reference_json(ValueNamespace::Outputs,Some("branch"),Some("case:c1"),ValueSelection::First,&[])}),
                    json!({})
                ))
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"branch","targetHandle":"main","order":0},
                {"id":"end","sourceNodeId":"branch","sourceHandle":"case:c1","targetNodeId":"exit","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{"answer":{"schema":{"type":"object"},"required":true}}}
        }))
        .unwrap();
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "REQUIRED_OUTPUT_MAY_BE_EMPTY")
    );

    definition.end.outputs.get_mut("answer").unwrap().required = false;
    assert!(
        compiler
            .compile(&definition, &CompileContext::default())
            .is_ok()
    );
}

#[test]
fn sensitive_context_is_readable_but_cannot_be_projected_or_returned() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","properties":{}},"contexts":{"secret":{"schema":{"type":"string"},"default":"","mutable":false,"sensitive":true,"scope":"execution_tree","mergePolicy":"replace","clientWritable":false}}},
            "nodes":[
                {"id":"http","key":"http","type":"declarative_http","typeVersion":1,"name":"HTTP","parameters":{"url":reference_template_json(ValueNamespace::Contexts,None,None,ValueSelection::Current,&["secret"])},"contextWrites":[]},
                exit_node(exit_parameters(json!({}), json!({})))
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"http","targetHandle":"main","order":0},
                {"id":"end","sourceNodeId":"http","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
            ],
            "end":{"outputs":{}}
        }))
        .unwrap();
    assert!(
        compiler
            .compile(&definition, &CompileContext::default())
            .is_ok()
    );
    definition.nodes[1].parameters = exit_parameters(
        json!({"secret":reference_json(ValueNamespace::Contexts,None,None,ValueSelection::Current,&["secret"])}),
        json!({}),
    );
    definition.end = serde_json::from_value(serde_json::json!({
        "outputs":{"secret":{"schema":{"type":"string"},"required":true,"sensitive":true}}
    }))
    .unwrap();
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "SENSITIVE_CONTEXT_EXPOSURE")
    );
}

#[test]
fn loop_namespace_is_only_available_inside_a_loop_component() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition = fixture();
    definition.nodes[1].parameters = serde_json::json!({"cases":[{"id":"c1","conditions":[{"condition":{"left":{"kind":"reference","selector":{"namespace":"loop","run":{"kind":"current"},"item":{"kind":"current"},"path":["index"]},"missingPolicy":{"kind":"error"}},"operator":"gt","right":{"kind":"literal","value":0}}}]}]});
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "LOOP_REFERENCE_OUTSIDE_ITERATION"),
        "issues were: {:?}",
        error
            .issues
            .iter()
            .map(|issue| issue.code.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn composite_manifest_pins_version_and_mutable_context_contract() {
    let version = uuid::Uuid::now_v7();
    let node_type = format!("workflow.{}", version.simple());
    let child_context = serde_json::json!({
        "history":{
            "schema":{"type":"array","items":{"type":"string"}},
            "default":[],"mutable":true,"sensitive":false,"scope":"session",
            "mergePolicy":"append","clientWritable":false
        }
    });
    let mut registry = NodeRegistry::m5_defaults();
    let mut manifest = registry.get("sub_workflow", 1).unwrap().clone();
    manifest.node_type = node_type.clone();
    manifest.parameter_schema = serde_json::json!({
        "type":"object",
        "x-agentx-contextContract":child_context.clone(),
        "required":["workflowVersionId","inputs"],
        "properties":{
            "workflowVersionId":{"type":"string","const":version.to_string()},
            "inputs":{
                "allOf":[{"type":"object","required":["question"],"properties":{"question":{"type":"string"}}}],
                "x-agentx-binding":{"acceptedKinds":["literal","reference","template","array","object"],"allowedNamespaces":["inputs","outputs","contexts"],"acceptedCardinality":["single"],"missingPolicies":["error","null","omit"],"recursive":true}
            }
        },
        "additionalProperties":false
    });
    registry.register(manifest).unwrap();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
            "schemaVersion":"8.0",
            "start":{"inputs":{"type":"object","required":["question"],"properties":{"question":{"type":"string"}}},"contexts":child_context},
            "nodes":[
                {"id":"child","key":"child","type":"sub_workflow","typeVersion":1,"name":"Child","parameters":{"workflowVersionId":version.to_string(),"inputs":{"kind":"object","fields":{"question":reference_json(ValueNamespace::Inputs,None,None,ValueSelection::Current,&["question"])}}},"contextWrites":[]},
                exit_node(exit_parameters(json!({}), json!({})))
            ],
            "connections":[
                {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"child","targetHandle":"main","order":0},
                {"id":"end","sourceNodeId":"child","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
            ],"end":{"outputs":{}}
        }))
        .unwrap();
    compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();

    definition
        .start
        .contexts
        .get_mut("history")
        .unwrap()
        .merge_policy = agentx_domain::ContextMergePolicy::Replace;
    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "COMPOSITE_CONTEXT_CONTRACT_MISMATCH")
    );
}

#[test]
fn terminal_connections_carry_the_exit_and_reject_fanout() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"http","key":"http","type":"declarative_http","typeVersion":1,"name":"HTTP","parameters":{"url":text_template("https://example.invalid")},"contextWrites":[]},
            {"id":"exit1","key":"exit1","type":"exit","typeVersion":1,"name":"End One","parameters":{"outputs":{},"errorOutputs":{}}},
            {"id":"exit2","key":"exit2","type":"exit","typeVersion":1,"name":"End Two","parameters":{"outputs":{},"errorOutputs":{}}}
        ],
        "connections":[
            {"id":"start-http","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"http","targetHandle":"main","order":0},
            {"id":"http-exit1","sourceNodeId":"http","sourceHandle":"main","targetNodeId":"exit1","targetHandle":"main","order":0},
            {"id":"http-exit2","sourceNodeId":"http","sourceHandle":"main","targetNodeId":"exit2","targetHandle":"main","order":1}
        ],
        "end":{"outputs":{}}
    }))
    .unwrap();

    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "DUPLICATE_TERMINAL_FANOUT")
    );

    definition.connections.pop();
    let compiled = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();
    assert_eq!(compiled.terminal_connections.len(), 1);
    assert_eq!(compiled.terminal_connections[0].target_exit, "exit1");
    assert!(compiled.exits.contains_key("exit1"));
    assert!(compiled.exits.contains_key("exit2"));
}

#[test]
fn exit_mappings_must_reference_their_own_predecessors() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let mut definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{}},"contexts":{}},
        "nodes":[
            {"id":"root","key":"root","type":"set","typeVersion":1,"name":"Root","contextWrites":[]},
            {"id":"left","key":"left","type":"set","typeVersion":1,"name":"Left","contextWrites":[]},
            {"id":"right","key":"right","type":"set","typeVersion":1,"name":"Right","contextWrites":[]},
            {"id":"exit1","key":"exit1","type":"exit","typeVersion":1,"name":"End One","parameters":{
                "outputs":{"answer":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"right","port":"main","run":{"kind":"current"},"item":{"kind":"first"},"path":[]},"missingPolicy":{"kind":"error"}}},
                "errorOutputs":{}
            }},
            {"id":"exit2","key":"exit2","type":"exit","typeVersion":1,"name":"End Two","parameters":{"outputs":{},"errorOutputs":{}}}
        ],
        "connections":[
            {"id":"start-root","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"root","targetHandle":"main","order":0},
            {"id":"root-left","sourceNodeId":"root","sourceHandle":"main","targetNodeId":"left","targetHandle":"main","order":0},
            {"id":"root-right","sourceNodeId":"root","sourceHandle":"main","targetNodeId":"right","targetHandle":"main","order":1},
            {"id":"left-exit1","sourceNodeId":"left","sourceHandle":"main","targetNodeId":"exit1","targetHandle":"main","order":0},
            {"id":"right-exit2","sourceNodeId":"right","sourceHandle":"main","targetNodeId":"exit2","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{"answer":{"schema":{"type":"object"},"required":false}}}
    }))
    .unwrap();

    let error = compiler
        .compile(&definition, &CompileContext::default())
        .unwrap_err();
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.code == "OUTPUT_NOT_PREDECESSOR"
                && issue.path == "nodes[3].parameters.outputs.answer")
    );

    definition.nodes[3].parameters["outputs"]["answer"] = reference_json(
        ValueNamespace::Outputs,
        Some("left"),
        Some("main"),
        ValueSelection::First,
        &[],
    );
    compiler
        .compile(&definition, &CompileContext::default())
        .expect("exit mapping referencing its own predecessor compiles");
}

#[test]
fn empty_definition_compiles_start_to_exit() {
    let compiled = WorkflowCompiler::new(&NodeRegistry::m5_defaults())
        .compile(&WorkflowDefinition::empty(), &CompileContext::default())
        .expect("empty definition compiles");
    assert!(compiled.nodes.is_empty());
    assert_eq!(compiled.start_to_exit.as_deref(), Some("exit"));
    assert!(compiled.exits["exit"].protected);
}

#[test]
fn reply_message_intent_satisfies_effective_output_contract() {
    // plan7 P7-A: the delivery builtin emits content + deliveryId + status on
    // main; the frozen per-port contract must accept exactly that shape.
    let workflow = WorkflowCompiler::new(&NodeRegistry::m5_defaults())
        .compile(
            &serde_json::from_value(serde_json::json!({
                "schemaVersion":"8.0",
                "start":{"inputs":{"type":"object","properties":{"message":{"type":"string"}}},"contexts":{}},
                "nodes":[
                    {"id":"relay","key":"relay","type":"reply_message","typeVersion":1,"name":"Relay",
                     "parameters":{"content":"hello"}},
                    {"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End",
                     "parameters":{"outputs":{},"errorOutputs":{}}}
                ],
                "connections":[
                    {"id":"a","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"relay","targetHandle":"main","order":0},
                    {"id":"b","sourceNodeId":"relay","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
                ],
                "end":{"outputs":{}}
            }))
            .unwrap(),
            &CompileContext::default(),
        )
        .unwrap();
    let node = workflow
        .nodes
        .iter()
        .find(|node| node.id == "relay")
        .unwrap();
    let schema = &node.effective_output_contract.port_schemas["main"];
    let validator = jsonschema::validator_for(schema).unwrap();
    validator
        .validate(&serde_json::json!({
            "content":"hello",
            "deliveryId":"00000000-0000-0000-0000-000000000001",
            "status":"queued"
        }))
        .unwrap();
}
