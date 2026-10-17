use std::{fs, path::PathBuf};

use agentx_agent_core::{
    AgentBundleV2, AgentDefinitionV6, AgentManifestV2, AgentRunInputV1, AgentSessionEntryV1,
    AgentSessionRegisterV1, AgentSessionStateV1, AgentSessionUsageV1, ApplicationUserEvidenceV1,
    DurableOperationStateV1, McpServerVersionV1, OperationRecordV1,
    SandboxProcessSessionContractV1,
};
use schemars::{JsonSchema, schema_for};
use serde_json::Value;

fn assert_schema<T: JsonSchema>(name: &str) {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/schemas/agent-core-v1");
    let committed: Value = serde_json::from_slice(
        &fs::read(root.join(format!("{name}.schema.json"))).expect("committed schema"),
    )
    .expect("schema json");
    let generated = serde_json::to_value(schema_for!(T)).expect("generated schema");
    assert_eq!(committed, generated, "schema drift for {name}");
}

#[test]
fn generated_agent_core_contracts_have_no_drift() {
    assert_schema::<AgentDefinitionV6>("AgentDefinitionV6");
    assert_schema::<AgentManifestV2>("AgentManifestV2");
    assert_schema::<AgentBundleV2>("AgentBundleV2");
    assert_schema::<AgentRunInputV1>("AgentRunInputV1");
    assert_schema::<AgentSessionStateV1>("AgentSessionStateV1");
    assert_schema::<AgentSessionEntryV1>("AgentSessionEntryV1");
    assert_schema::<AgentSessionRegisterV1>("AgentSessionRegisterV1");
    assert_schema::<AgentSessionUsageV1>("AgentSessionUsageV1");
    assert_schema::<DurableOperationStateV1>("DurableOperationStateV1");
    assert_schema::<ApplicationUserEvidenceV1>("ApplicationUserEvidenceV1");
    assert_schema::<OperationRecordV1>("OperationRecordV1");
    assert_schema::<SandboxProcessSessionContractV1>("SandboxProcessSessionContractV1");
    assert_schema::<McpServerVersionV1>("McpServerVersionV1");
}

#[test]
fn session_entry_schema_requires_exactly_one_payload_location() {
    let schema = serde_json::to_value(schema_for!(AgentSessionEntryV1)).unwrap();
    let choices = schema
        .get("oneOf")
        .and_then(Value::as_array)
        .expect("payload storage must be a schema choice");
    assert_eq!(choices.len(), 2);
    assert!(choices.iter().all(|choice| {
        choice
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| required.len() == 1)
    }));
}
