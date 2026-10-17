use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

const CONTRACT: &str = include_str!("../../../../contracts/openapi/platform-api.json");

pub(crate) fn requested_output() -> Result<Option<PathBuf>> {
    let mut arguments = std::env::args().skip(1);
    let Some(command) = arguments.next() else {
        return Ok(None);
    };
    if command != "openapi" {
        anyhow::bail!("unknown platform-control command {command}");
    }
    Ok(Some(PathBuf::from(arguments.next().unwrap_or_else(|| {
        "contracts/openapi/platform-api.json".into()
    }))))
}

pub(crate) fn write(path: &Path) -> Result<()> {
    let mut contract: Value = serde_json::from_str(CONTRACT).context("invalid Platform OpenAPI")?;
    contract["x-agentx-generator"] = Value::String("platform-control".into());
    contract["x-agentx-contract-version"] = Value::String("plan8".into());
    contract["components"]["schemas"]["BuilderModelSetting"] = serde_json::to_value(
        schemars::schema_for!(crate::studio_builder_setting::BuilderModelSetting),
    )?;
    contract["components"]["schemas"]["SetBuilderModel"] = serde_json::to_value(
        schemars::schema_for!(crate::studio_builder_setting::SetBuilderModel),
    )?;
    add_schema::<agentx_runtime_contracts::StudioEditorSnapshotV1>(
        &mut contract,
        "StudioEditorSnapshotV1",
    )?;
    add_schema::<agentx_runtime_contracts::StudioEditorCommandV1>(
        &mut contract,
        "StudioEditorCommandV1",
    )?;
    add_schema::<agentx_runtime_contracts::StudioEditorAcknowledgementV1>(
        &mut contract,
        "StudioEditorAcknowledgementV1",
    )?;
    add_schema::<crate::studio_builder_api::CreateBuilderSession>(
        &mut contract,
        "CreateBuilderSession",
    )?;
    add_schema::<crate::studio_builder_api::CreateBuilderRun>(&mut contract, "CreateBuilderRun")?;
    add_schema::<crate::studio_builder_api::CommandReceipt>(
        &mut contract,
        "BuilderCommandReceipt",
    )?;
    contract["paths"]["/api/v1/studio-builder/sessions/{session}/commands/{command}/receipt"] = serde_json::json!({
        "get": {
            "parameters": [
                {"in":"path","name":"session","required":true,"schema":{"type":"string","format":"uuid"}},
                {"in":"path","name":"command","required":true,"schema":{"type":"string","format":"uuid"}}
            ],
            "security":[{"bearerAuth":[]}],
            "responses":{"200":{"description":"Durable editor command status","content":{"application/json":{"schema":{"$ref":"#/components/schemas/BuilderCommandReceipt"}}}}}
        }
    });
    add_schema::<agentx_runtime_contracts::WorkflowDesignSourceV1>(
        &mut contract,
        "WorkflowDesignSourceV1",
    )?;
    add_schema::<agentx_runtime_contracts::StudioBuilderSessionV1>(
        &mut contract,
        "StudioBuilderSessionV1",
    )?;
    add_schema::<crate::workflow_versions::WorkflowVersionResponse>(
        &mut contract,
        "WorkflowVersionResponse",
    )?;
    add_schema::<crate::workflow_api::DraftResponse>(&mut contract, "DraftResponse")?;
    add_schema::<crate::workflow_publishing::PublishWorkflowRequest>(
        &mut contract,
        "PublishWorkflowRequest",
    )?;
    add_schema::<crate::workflow_versions::RestoreRequest>(&mut contract, "RestoreVersionRequest")?;
    add_schema::<crate::workflow_versions::ArchiveVersionRequest>(
        &mut contract,
        "ArchiveVersionRequest",
    )?;
    add_schema::<crate::external_resource_api::ConnectionResponse>(
        &mut contract,
        "ConnectionResponse",
    )?;
    add_schema::<crate::external_resource_api::UpdateMemoryConnectionRequest>(
        &mut contract,
        "UpdateMemoryConnectionRequest",
    )?;
    add_schema::<agentx_runtime_contracts::ApplicationUserIdentityV1>(
        &mut contract,
        "ApplicationUserIdentity",
    )?;
    add_schema::<crate::runtime_bff::AgentSubjectMemoryInput>(
        &mut contract,
        "AgentSubjectMemoryInput",
    )?;
    for path in [
        "/api/v1/agent-subject-memory/audit",
        "/api/v1/agent-subject-memory/clear",
    ] {
        contract["paths"][path] = serde_json::json!({"post":{
            "security":[{"bearerAuth":[]}],
            "requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/AgentSubjectMemoryInput"}}}},
            "responses":{"200":{"description":"Memory scope operation result","content":{"application/json":{"schema":{"type":"object"}}}}}
        }});
    }
    add_schema::<agentx_runtime_contracts::InvocationSearchPageV1>(
        &mut contract,
        "ApplicationInvocationPageResponse",
    )?;
    contract["paths"]["/api/v1/applications/{id}/invocations"] = serde_json::json!({"get": {
        "security":[{"bearerAuth":[]}],
        "parameters":[
            {"in":"path","name":"id","required":true,"schema":{"type":"string","format":"uuid"}},
            {"in":"query","name":"externalUserId","schema":{"type":"string"}},
            {"in":"query","name":"userSource","schema":{"type":"string","enum":["platform","channel","api_parameter"]}},
            {"in":"query","name":"cursor","schema":{"type":"string"}}
        ],
        "responses":{"200":{"description":"Application invocation records","content":{"application/json":{"schema":{"$ref":"#/components/schemas/ApplicationInvocationPageResponse"}}}}}
    }});
    let query = serde_json::to_value(schemars::schema_for!(crate::resource_api::OptionQuery))?;
    let mut parameters = vec![
        serde_json::json!({"in":"path","name":"id","required":true,"schema":{"type":"string","format":"uuid"}}),
    ];
    for (name, schema) in query["properties"]
        .as_object()
        .context("resource query properties")?
    {
        let required = query["required"]
            .as_array()
            .is_some_and(|fields| fields.iter().any(|field| field == name));
        parameters.push(
            serde_json::json!({"in":"query","name":name,"required":required,"schema":schema}),
        );
    }
    contract["paths"]["/api/v1/workflows/{id}/resource-options"]["get"]["parameters"] =
        serde_json::json!(parameters);
    let value = serde_json::to_string_pretty(&contract)? + "\n";
    std::fs::write(path, value)
        .with_context(|| format!("write Platform OpenAPI to {}", path.display()))?;
    Ok(())
}

fn add_schema<T: schemars::JsonSchema>(contract: &mut Value, name: &str) -> Result<()> {
    let mut schema = serde_json::to_value(schemars::schema_for!(T))?;
    let definitions = schema
        .as_object_mut()
        .unwrap()
        .remove("$defs")
        .unwrap_or_else(|| serde_json::json!({}));
    schema.as_object_mut().unwrap().remove("$schema");
    fn refs(value: &mut Value, prefix: &str) {
        match value {
            Value::Object(fields) => {
                if let Some(Value::String(reference)) = fields.get_mut("$ref") {
                    if let Some(definition) = reference.strip_prefix("#/$defs/") {
                        *reference = format!("#/components/schemas/{prefix}_{definition}");
                    }
                }
                for value in fields.values_mut() {
                    refs(value, prefix);
                }
            }
            Value::Array(values) => {
                for value in values {
                    refs(value, prefix);
                }
            }
            _ => {}
        }
    }
    refs(&mut schema, name);
    contract["components"]["schemas"][name] = schema;
    for (definition, mut value) in definitions.as_object().unwrap().clone() {
        refs(&mut value, name);
        contract["components"]["schemas"][format!("{name}_{definition}")] = value;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_contract_has_all_cutover_paths() {
        let value: Value = serde_json::from_str(CONTRACT).unwrap();
        assert!(value["paths"]["/api/v1/workflows/{id}/run"]["post"].is_object());
        assert!(value["paths"]["/api/v1/runtime/quotas"]["put"].is_object());
        assert!(value["paths"]["/api/v1/departments/{id}/resource-options"]["get"].is_object());
        assert!(value["paths"]["/api/v1/approvals/{id}/decide"]["post"].is_object());
        for removed in ["approve", "reject", "timeout"] {
            assert!(
                value["paths"]
                    .get(format!("/api/v1/approvals/{{id}}/{removed}"))
                    .is_none()
            );
        }
        for schema in [
            "CanvasPlugin",
            "CanvasPluginVersion",
            "CanvasPluginImport",
            "CanvasPluginReferences",
        ] {
            assert!(value["components"]["schemas"][schema].is_object());
        }
        assert_eq!(
            value["paths"]["/api/v1/canvas-plugins"]["get"]["responses"]["200"]["content"]["application/json"]
                ["schema"]["$ref"],
            "#/components/schemas/CanvasPluginPage"
        );
    }
}
