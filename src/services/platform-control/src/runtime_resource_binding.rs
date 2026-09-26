use std::collections::BTreeSet;

use agentx_bundle_builder::composite_ir_object_id;
use agentx_domain::ResourceVersionSnapshot;
use agentx_runtime_contracts::{
    ContentHash, ExecutionDepartmentSnapshotV1, ExecutionWorkflowSnapshotV1,
    RuntimeEnvironmentCredentialReferenceV1, RuntimeGrantBindingV1, RuntimeMcpSandboxReferenceV1,
    RuntimeMcpTransportV2, RuntimeModelPriceV1, RuntimeResourceBindingV1,
    RuntimeResourceConfigurationV1, RuntimeResourceKindV1, SandboxEgressModeV1,
    VaultSecretReferenceV1,
};
use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

#[cfg(test)]
pub(crate) fn runtime_resource_kind_from_control(
    value: &str,
) -> anyhow::Result<agentx_runtime_contracts::RuntimeResourceKindV1> {
    use agentx_runtime_contracts::RuntimeResourceKindV1;
    Ok(match value {
        "model" => RuntimeResourceKindV1::Model,
        "mcp" | "mcp_server" | "mcp_tool" => RuntimeResourceKindV1::Mcp,
        "rag" => RuntimeResourceKindV1::Rag,
        "memory" => RuntimeResourceKindV1::Memory,
        "skill" => RuntimeResourceKindV1::Skill,
        "credential" => RuntimeResourceKindV1::Credential,
        "sandbox" | "sandbox_profile" => RuntimeResourceKindV1::SandboxProfile,
        "composite" => RuntimeResourceKindV1::Composite,
        unsupported => anyhow::bail!("unsupported Control Resource Grant type {unsupported}"),
    })
}

pub(crate) async fn authorization_grants(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    service_identity_id: Uuid,
) -> std::result::Result<(Vec<Uuid>, Vec<RuntimeGrantBindingV1>), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id,resource_type,resource_id,resource_version_id,operation_key FROM resource_grants WHERE tenant_id=? AND subject_type='workflow_service_identity' AND subject_id=? ORDER BY id",
    )
    .bind(tenant_id)
    .bind(service_identity_id)
    .fetch_all(pool)
    .await?;
    let mut ids = Vec::with_capacity(rows.len());
    let mut bindings = Vec::with_capacity(rows.len());
    for row in rows {
        let grant_id: Uuid = row.try_get("id")?;
        ids.push(grant_id);
        bindings.push(RuntimeGrantBindingV1 {
            grant_id,
            resource_type: row.try_get("resource_type")?,
            resource_id: row.try_get("resource_id")?,
            resource_version_id: row.try_get("resource_version_id")?,
            operation: row.try_get("operation_key")?,
        });
    }
    Ok((ids, bindings))
}

pub(crate) fn from_row(row: &sqlx::mysql::MySqlRow) -> Result<RuntimeResourceBindingV1> {
    let resource_type: String = row.try_get("resource_type")?;
    let resource_id: Uuid = row.try_get("resource_id")?;
    let version_id: Option<Uuid> = row.try_get("resource_version_id")?;
    let snapshot: Value = row.try_get("snapshot_json")?;
    let content_hash = ContentHash::parse(row.try_get::<String, _>("snapshot_hash")?)?;
    from_parts(
        &resource_type,
        resource_id,
        version_id,
        snapshot,
        content_hash,
    )
}

pub(crate) fn from_snapshot(value: &ResourceVersionSnapshot) -> Result<RuntimeResourceBindingV1> {
    from_parts(
        value.reference.resource_type.as_str(),
        value.reference.resource_id,
        value.reference.resource_version_id,
        value.snapshot.clone(),
        ContentHash::parse(&value.snapshot_hash)?,
    )
}

fn from_parts(
    resource_type: &str,
    resource_id: Uuid,
    version_id: Option<Uuid>,
    snapshot: Value,
    content_hash: ContentHash,
) -> Result<RuntimeResourceBindingV1> {
    let resource_version = version_id
        .map(|id| id.to_string())
        .or_else(|| json_string(&snapshot, "resourceVersion"))
        .unwrap_or_else(|| "fixed".into());
    let state_epoch = snapshot
        .get("resourceVersion")
        .or_else(|| snapshot.get("versionNumber"))
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let (resource_kind, configuration, object_ids) = match resource_type {
        "model" => (
            RuntimeResourceKindV1::Model,
            RuntimeResourceConfigurationV1::Model {
                provider: required_json_string(&snapshot, "providerType")?,
                endpoint: required_json_string(&snapshot, "endpoint")?,
                model: required_json_string(&snapshot, "modelName")?,
                context_window: snapshot
                    .get("contextWindow")
                    .and_then(Value::as_u64)
                    .unwrap_or(128_000),
                price: RuntimeModelPriceV1 {
                    version_id: required_json_scalar_string(
                        snapshot.pointer("/price/versionId"),
                        "Model Runtime binding requires price.versionId",
                    )?,
                    currency: required_json_pointer_string(
                        &snapshot,
                        "/price/currency",
                        "Model Runtime binding requires price.currency",
                    )?,
                    input_per_million: required_json_pointer_string(
                        &snapshot,
                        "/price/inputPerMillion",
                        "Model Runtime binding requires price.inputPerMillion",
                    )?,
                    output_per_million: required_json_pointer_string(
                        &snapshot,
                        "/price/outputPerMillion",
                        "Model Runtime binding requires price.outputPerMillion",
                    )?,
                },
                credential: optional_vault_reference(&snapshot)?,
                capabilities: snapshot
                    .get("capabilities")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_owned))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            },
            vec![],
        ),
        "mcp_tool" | "mcp_server" => {
            let schema_hash = snapshot
                .get("schemaHash")
                .or_else(|| snapshot.get("configurationHash"))
                .and_then(Value::as_str)
                .context("MCP Runtime binding requires schemaHash")?;
            (
                RuntimeResourceKindV1::Mcp,
                RuntimeResourceConfigurationV1::Mcp {
                    server_id: json_uuid(&snapshot, "serverId")
                        .context("MCP Runtime binding requires serverId")?,
                    server_version_id: json_uuid(&snapshot, "serverVersionId")
                        .context("MCP Runtime binding requires serverVersionId")?,
                    transport: runtime_mcp_transport(&snapshot)?,
                    tool_name: json_string(&snapshot, "toolName")
                        .unwrap_or_else(|| "__server__".into()),
                    tool_version: version_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| resource_version.clone()),
                    input_schema_hash: ContentHash::parse(schema_hash)?,
                    input_schema: snapshot
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type":"object"})),
                    output_schema: snapshot
                        .get("outputSchema")
                        .cloned()
                        .filter(|value| !value.is_null()),
                    side_effect: json_string(&snapshot, "sideEffect")
                        .unwrap_or_else(|| "unknown".into()),
                    timeout_seconds: snapshot
                        .get("timeoutSeconds")
                        .and_then(Value::as_u64)
                        .unwrap_or(30)
                        .try_into()
                        .unwrap_or(30),
                    credential: optional_vault_reference(&snapshot)?,
                },
                vec![],
            )
        }
        "rag" => (
            RuntimeResourceKindV1::Rag,
            RuntimeResourceConfigurationV1::Rag {
                provider: json_string(&snapshot, "provider").unwrap_or_else(|| "lightrag".into()),
                endpoint: required_json_string(&snapshot, "endpoint")?,
                namespace: required_json_string(&snapshot, "externalResourceId")?,
                index_version: resource_version.clone(),
                credential: optional_vault_reference(&snapshot)?,
            },
            vec![],
        ),
        "memory" => (
            RuntimeResourceKindV1::Memory,
            RuntimeResourceConfigurationV1::Memory {
                endpoint: required_json_string(&snapshot, "endpoint")?,
                namespace: required_json_string(&snapshot, "externalNamespace")?,
                memory_version: resource_version.clone(),
                access_mode: json_string(&snapshot, "accessMode").unwrap_or_else(|| "read".into()),
                credential: optional_vault_reference(&snapshot)?,
            },
            vec![],
        ),
        "skill" => {
            version_id.context("Skill Runtime binding requires a fixed version")?;
            let entrypoint = json_uuid(&snapshot, "entrypointObjectId")
                .context("Skill Runtime binding requires a signed V2 program object")?;
            let entrypoint_content_hash =
                ContentHash::parse(required_json_string(&snapshot, "entrypointContentHash")?)?;
            let dependency_object_ids = uuid_array(&snapshot, "dependencyObjectIds")?;
            let dependencies = serde_json::from_value(
                snapshot
                    .get("dependencies")
                    .cloned()
                    .context("Skill Runtime binding requires frozen dependencies")?,
            )?;
            let mut object_ids = vec![entrypoint];
            object_ids.extend(dependency_object_ids.iter().copied());
            (
                RuntimeResourceKindV1::Skill,
                RuntimeResourceConfigurationV1::Skill {
                    entrypoint_object_id: entrypoint,
                    entrypoint_content_hash,
                    dependency_object_ids,
                    dependencies,
                },
                object_ids,
            )
        }
        "credential" => (
            RuntimeResourceKindV1::Credential,
            RuntimeResourceConfigurationV1::Credential {
                credential_type: required_json_string(&snapshot, "credentialType")?,
                secret: required_vault_reference(&snapshot)?,
                allowed_operations: BTreeSet::from(["use".into()]),
            },
            vec![],
        ),
        "sandbox_profile" => (
            RuntimeResourceKindV1::SandboxProfile,
            RuntimeResourceConfigurationV1::SandboxProfile {
                provider: json_string(&snapshot, "provider")
                    .unwrap_or_else(|| "opensandbox".into()),
                image: required_json_string(&snapshot, "imageDigest")?,
                cpu_millis: required_json_u32(&snapshot, "cpuMillis")?,
                memory_bytes: required_json_u64(&snapshot, "memoryBytes")?,
                disk_bytes: required_json_u64(&snapshot, "diskBytes")?,
                pid_limit: required_json_u32(&snapshot, "pidsLimit")?,
                egress_mode: match snapshot
                    .pointer("/networkPolicy/egressMode")
                    .and_then(Value::as_str)
                    .unwrap_or("none")
                {
                    "none" => SandboxEgressModeV1::None,
                    "tcp_proxy" => SandboxEgressModeV1::TcpProxy,
                    other => anyhow::bail!("unsupported Sandbox egress mode {other}"),
                },
                maximum_ttl_seconds: required_json_u32(&snapshot, "timeoutSeconds")?,
            },
            vec![],
        ),
        "workflow" => {
            let workflow_version_id = version_id
                .or_else(|| json_uuid(&snapshot, "workflowVersionId"))
                .context("Composite Runtime binding requires a fixed Workflow Version")?;
            let workflow = serde_json::from_value::<ExecutionWorkflowSnapshotV1>(
                snapshot
                    .get("workflow")
                    .cloned()
                    .context("Composite Runtime binding requires workflow snapshot metadata")?,
            )?;
            anyhow::ensure!(
                workflow.version_id == workflow_version_id,
                "Composite Runtime binding Workflow snapshot has a mismatched version"
            );
            (
                RuntimeResourceKindV1::Composite,
                RuntimeResourceConfigurationV1::Composite {
                    workflow,
                    definition_object_id: workflow_version_id,
                    ir_object_id: composite_ir_object_id(workflow_version_id),
                },
                vec![
                    workflow_version_id,
                    composite_ir_object_id(workflow_version_id),
                ],
            )
        }
        other => anyhow::bail!("unsupported Runtime resource type {other}"),
    };
    Ok(RuntimeResourceBindingV1 {
        resource_kind,
        resource_id,
        resource_version,
        state_epoch,
        content_hash,
        configuration,
        object_ids,
    })
}

pub(crate) async fn workflow_snapshot(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    version_id: Uuid,
) -> Result<ExecutionWorkflowSnapshotV1> {
    let row = sqlx::query("SELECT w.id workflow_id,w.name,v.version_number,w.owner_department_id,d.name owner_department_name FROM workflow_versions v JOIN workflows w ON w.tenant_id=v.tenant_id AND w.id=v.workflow_id LEFT JOIN departments d ON d.tenant_id=w.tenant_id AND d.id=w.owner_department_id WHERE v.tenant_id=? AND v.id=?")
        .bind(tenant_id)
        .bind(version_id)
        .fetch_optional(pool)
        .await?
        .with_context(|| format!("fixed Workflow Version {version_id} is missing"))?;
    let owner_department_id: Option<Uuid> = row.try_get("owner_department_id")?;
    Ok(ExecutionWorkflowSnapshotV1 {
        id: row.try_get("workflow_id")?,
        name: row.try_get("name")?,
        version_id,
        version_number: row.try_get("version_number")?,
        owner_department: owner_department_id.map(|id| ExecutionDepartmentSnapshotV1 {
            id,
            name: row.try_get("owner_department_name").unwrap_or_default(),
        }),
    })
}

fn runtime_mcp_transport(snapshot: &Value) -> Result<RuntimeMcpTransportV2> {
    let transport = snapshot
        .get("transport")
        .context("MCP Runtime binding requires tagged transport")?;
    match transport.get("kind").and_then(Value::as_str) {
        Some("streamable_http") => Ok(RuntimeMcpTransportV2::StreamableHttp {
            endpoint: required_json_string(transport, "endpoint")?,
        }),
        Some("sse") => Ok(RuntimeMcpTransportV2::Sse {
            endpoint: required_json_string(transport, "endpoint")?,
        }),
        Some("stdio") => {
            let runtime_sandbox = transport
                .get("runtimeSandbox")
                .context("stdio MCP Runtime binding requires runtimeSandbox")?;
            let environment_credential_refs = transport
                .get("environmentCredentialRefs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|reference| {
                    Ok(RuntimeEnvironmentCredentialReferenceV1 {
                        name: required_json_string(reference, "name")?,
                        credential: serde_json::from_value(
                            reference.get("vaultSecretRef").cloned().context(
                                "stdio MCP environment Credential requires Vault reference",
                            )?,
                        )?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(RuntimeMcpTransportV2::Stdio {
                command: required_json_string(transport, "command")?,
                args: transport
                    .get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .context("stdio MCP args must be strings")
                    })
                    .collect::<Result<Vec<_>>>()?,
                environment_credential_refs,
                runtime_sandbox: RuntimeMcpSandboxReferenceV1 {
                    resource_id: json_uuid(runtime_sandbox, "resourceId")
                        .context("stdio MCP runtimeSandbox.resourceId is required")?,
                    resource_version_id: json_uuid(runtime_sandbox, "resourceVersionId")
                        .context("stdio MCP runtimeSandbox.resourceVersionId is required")?,
                },
            })
        }
        _ => anyhow::bail!("MCP Runtime binding has an unsupported tagged transport"),
    }
}

fn required_json_pointer_string(
    value: &Value,
    pointer: &str,
    message: &'static str,
) -> Result<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context(message)
}

fn required_json_scalar_string(value: Option<&Value>, message: &'static str) -> Result<String> {
    optional_json_scalar_string(value)?.context(message)
}

fn optional_vault_reference(snapshot: &Value) -> Result<Option<VaultSecretReferenceV1>> {
    let value = snapshot
        .get("vaultSecretRef")
        .or_else(|| snapshot.get("secretRef"))
        .filter(|value| !value.is_null());
    value
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

fn required_vault_reference(snapshot: &Value) -> Result<VaultSecretReferenceV1> {
    optional_vault_reference(snapshot)?
        .context("Credential Runtime binding requires a versioned Vault secret reference")
}

pub(crate) fn required_json_string(value: &Value, key: &str) -> Result<String> {
    json_string(value, key).with_context(|| format!("Runtime binding requires {key}"))
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn json_scalar_string(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        _ => anyhow::bail!("Runtime binding version must be a string or number"),
    }
}

fn optional_json_scalar_string(value: Option<&Value>) -> Result<Option<String>> {
    value
        .filter(|value| !value.is_null())
        .map(json_scalar_string)
        .transpose()
}

pub(crate) fn json_uuid(value: &Value, key: &str) -> Option<Uuid> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

pub(crate) fn required_json_u64(value: &Value, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("Runtime binding requires numeric {key}"))
}

fn required_json_u32(value: &Value, key: &str) -> Result<u32> {
    required_json_u64(value, key)?
        .try_into()
        .with_context(|| format!("Runtime binding {key} exceeds u32"))
}

fn uuid_array(value: &Value, key: &str) -> Result<Vec<Uuid>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|value| {
            let value = value
                .as_str()
                .with_context(|| format!("Runtime binding {key} must contain UUID strings"))?;
            Uuid::parse_str(value).map_err(Into::into)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use agentx_runtime_contracts::{
        ContentHash, RuntimeResourceConfigurationV1, RuntimeResourceKindV1,
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::{from_parts, optional_json_scalar_string, optional_vault_reference};

    #[test]
    fn null_optional_versions_are_absent() {
        let snapshot = json!({"price":{"versionId":null}});
        assert_eq!(
            optional_json_scalar_string(snapshot.pointer("/price/versionId")).unwrap(),
            None
        );
        let snapshot = json!({"price":{"versionId":7}});
        assert_eq!(
            optional_json_scalar_string(snapshot.pointer("/price/versionId")).unwrap(),
            Some("7".into())
        );
    }

    #[test]
    fn null_optional_vault_reference_is_absent() {
        assert!(
            optional_vault_reference(&json!({"vaultSecretRef": null}))
                .unwrap()
                .is_none()
        );
        assert!(optional_vault_reference(&json!({})).unwrap().is_none());
    }

    #[test]
    fn model_binding_freezes_the_complete_price_snapshot() {
        let snapshot = json!({
            "providerType":"openai_compatible",
            "endpoint":"https://provider.test/v1",
            "modelName":"fixture",
            "contextWindow":4096,
            "price":{
                "versionId":Uuid::nil(),
                "currency":"USD",
                "inputPerMillion":"5.00000000",
                "outputPerMillion":"30.00000000"
            }
        });
        let binding = from_parts(
            "model",
            Uuid::now_v7(),
            Some(Uuid::now_v7()),
            snapshot.clone(),
            ContentHash::parse(format!("sha256:{}", "a".repeat(64))).unwrap(),
        )
        .unwrap();
        assert_eq!(binding.resource_kind, RuntimeResourceKindV1::Model);
        assert!(matches!(
            binding.configuration,
            RuntimeResourceConfigurationV1::Model { context_window, price, .. }
                if context_window == 4096
                    && price.currency == "USD"
                    && price.input_per_million == "5.00000000"
                    && price.output_per_million == "30.00000000"
        ));

        let mut missing = snapshot;
        missing["price"]["outputPerMillion"] = serde_json::Value::Null;
        assert!(
            from_parts(
                "model",
                Uuid::now_v7(),
                Some(Uuid::now_v7()),
                missing,
                ContentHash::parse(format!("sha256:{}", "b".repeat(64))).unwrap(),
            )
            .is_err()
        );
    }
}
