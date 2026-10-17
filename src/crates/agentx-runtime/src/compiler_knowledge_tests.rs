use super::*;
use serde_json::json;

fn workflow() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "schemaVersion":"8.0","start":{"inputs":{"type":"object"},"contexts":{}},
        "end":{"outputs":{"answer":{"schema":{"type":"string"},"required":true}},"error":{"outputs":{"message":{"schema":{"type":"string"},"required":true}}}},
        "nodes":[
            {"id":"knowledge","key":"knowledge","type":"knowledge_retrieval","typeVersion":1,"name":"Knowledge","parameters":{"query":{"kind":"literal","value":"question"},"topK":5},"resourceReferences":[{"resourceType":"rag","resourceId":"018f47a0-7e9c-7000-8000-000000000001","operation":"read"}]},
            {"id":"success","key":"success","type":"exit","typeVersion":1,"name":"Success","protected":true,"parameters":{"outputs":{"answer":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"knowledge","path":["text"]}}}}},
            {"id":"failure","key":"failure","type":"exit","typeVersion":1,"name":"Failure","parameters":{"errorOutputs":{"message":{"kind":"reference","selector":{"namespace":"item","path":["message"]}}}}}
        ],
        "connections":[
            {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"knowledge","targetHandle":"main","order":0},
            {"id":"success","sourceNodeId":"knowledge","sourceHandle":"main","targetNodeId":"success","targetHandle":"main","order":0},
            {"id":"failure","sourceNodeId":"knowledge","sourceHandle":"error","targetNodeId":"failure","targetHandle":"error","order":0}
        ]
    })).unwrap()
}

#[test]
fn knowledge_and_independent_error_exit_compile_with_port_specific_mappings() {
    let registry = NodeRegistry::m5_defaults();
    let compiler = WorkflowCompiler::new(&registry);
    let definition = workflow();
    compiler
        .compile(&definition, &CompileContext::default())
        .unwrap();
    let mut missing = definition.clone();
    missing.nodes[0].resource_references.clear();
    assert!(
        compiler
            .validate_draft(&missing, &CompileContext::default())
            .is_empty()
    );
    assert!(
        compiler
            .compile(&missing, &CompileContext::default())
            .unwrap_err()
            .issues
            .iter()
            .any(|issue| issue.code == "BINDING_REQUIRED")
    );
    let mut multiple = definition.clone();
    let mut reference = multiple.nodes[0].resource_references[0].clone();
    reference.resource_id = uuid::Uuid::from_u128(42);
    multiple.nodes[0].resource_references.push(reference);
    compiler
        .compile(&multiple, &CompileContext::default())
        .unwrap();
    let mut wrong = definition;
    wrong.nodes[0].resource_references[0].operation = agentx_domain::ResourceOperation::Write;
    assert!(
        agentx_domain::validate_definition(&wrong)
            .iter()
            .any(|issue| issue.code == "INVALID_RESOURCE_REFERENCE")
    );
}

#[test]
fn missing_error_mapping_is_rejected_but_success_is_not_required_on_error_only_exit() {
    let mut definition = workflow();
    definition.nodes[2].parameters = json!({});
    let issues = agentx_domain::validate_definition(&definition);
    let mappings = issues
        .iter()
        .filter(|issue| issue.code == "EXIT_REQUIRED_MAPPING_MISSING")
        .collect::<Vec<_>>();
    assert_eq!(mappings.len(), 1);
    assert_eq!(mappings[0].path, "nodes[2].parameters.errorOutputs.message");
}
