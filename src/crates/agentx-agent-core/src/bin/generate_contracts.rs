use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use agentx_agent_core::{
    AgentBundleV2, AgentDefinitionV6, AgentManifestV2, AgentRunInputV1, AgentSessionEntryV1,
    AgentSessionIdentityV1, AgentSessionProjectionV1, AgentSessionRegisterV1, AgentSessionStateV1,
    AgentSessionUsageV1, ApplicationUserEvidenceV1, CompactionRegisterV1, DurableOperationStateV1,
    McpServerVersionV1, OperationRecordV1, RetentionPolicyV1, SandboxProcessSessionContractV1,
};
use schemars::{JsonSchema, schema_for};
use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    let schema_directory = PathBuf::from(
        arguments
            .next()
            .unwrap_or_else(|| "contracts/schemas/agent-core-v1".into()),
    );
    let openapi_file = PathBuf::from(
        arguments
            .next()
            .unwrap_or_else(|| "contracts/openapi/agent-core-contracts-v1.json".into()),
    );
    fs::create_dir_all(&schema_directory)?;
    if let Some(parent) = openapi_file.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut schemas = BTreeMap::new();
    add::<AgentDefinitionV6>(&schema_directory, &mut schemas, "AgentDefinitionV6")?;
    add::<AgentManifestV2>(&schema_directory, &mut schemas, "AgentManifestV2")?;
    add::<AgentBundleV2>(&schema_directory, &mut schemas, "AgentBundleV2")?;
    add::<AgentRunInputV1>(&schema_directory, &mut schemas, "AgentRunInputV1")?;
    add::<AgentSessionStateV1>(&schema_directory, &mut schemas, "AgentSessionStateV1")?;
    add::<AgentSessionEntryV1>(&schema_directory, &mut schemas, "AgentSessionEntryV1")?;
    add::<AgentSessionIdentityV1>(&schema_directory, &mut schemas, "AgentSessionIdentityV1")?;
    add::<AgentSessionProjectionV1>(&schema_directory, &mut schemas, "AgentSessionProjectionV1")?;
    add::<AgentSessionRegisterV1>(&schema_directory, &mut schemas, "AgentSessionRegisterV1")?;
    add::<AgentSessionUsageV1>(&schema_directory, &mut schemas, "AgentSessionUsageV1")?;
    add::<CompactionRegisterV1>(&schema_directory, &mut schemas, "CompactionRegisterV1")?;
    add::<DurableOperationStateV1>(&schema_directory, &mut schemas, "DurableOperationStateV1")?;
    add::<ApplicationUserEvidenceV1>(&schema_directory, &mut schemas, "ApplicationUserEvidenceV1")?;
    add::<RetentionPolicyV1>(&schema_directory, &mut schemas, "RetentionPolicyV1")?;
    add::<OperationRecordV1>(&schema_directory, &mut schemas, "OperationRecordV1")?;
    add::<SandboxProcessSessionContractV1>(
        &schema_directory,
        &mut schemas,
        "SandboxProcessSessionContractV1",
    )?;
    add::<McpServerVersionV1>(&schema_directory, &mut schemas, "McpServerVersionV1")?;
    write_json(
        &openapi_file,
        &json!({
            "openapi": "3.1.0",
            "info": {
                "title": "Agentx Agent Core frozen contract draft",
                "version": "1.1.0-p3-04"
            },
            "paths": {},
            "components": {"schemas": schemas}
        }),
    )?;
    Ok(())
}

fn add<T: JsonSchema>(
    directory: &Path,
    schemas: &mut BTreeMap<String, Value>,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let schema = serde_json::to_value(schema_for!(T))?;
    write_json(&directory.join(format!("{name}.schema.json")), &schema)?;
    schemas.insert(name.into(), schema);
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes)?;
    Ok(())
}
