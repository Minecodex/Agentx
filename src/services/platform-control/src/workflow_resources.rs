use std::collections::{HashMap, VecDeque};

use agentx_domain::{
    ResourceOperation, ResourceReference, ResourceType, ResourceVersionSnapshot, WorkflowDefinition,
};
use agentx_runtime_contracts::{
    ContentHash, RuntimeSkillAssetV2, RuntimeSkillDependencyV2, RuntimeSkillProgramV2,
    deterministic_uuid,
};
use bytes::Bytes;
use object_store::path::Path as ObjectPath;
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::ControlApiState,
};

pub(crate) async fn replace_draft_resources(
    transaction: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    workflow_id: Uuid,
    draft_id: Uuid,
    definition: &WorkflowDefinition,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM workflow_draft_resources WHERE tenant_id=? AND draft_id=?")
        .bind(tenant_id)
        .bind(draft_id)
        .execute(&mut **transaction)
        .await?;
    for node in &definition.nodes {
        for reference in &node.resource_references {
            sqlx::query("INSERT INTO workflow_draft_resources(id,tenant_id,workflow_id,draft_id,node_id,node_name,resource_type,resource_id,resource_version_id,operation_key,relation) VALUES(?,?,?,?,?,?,?,?,?,?, 'resource_reference')")
                .bind(Uuid::now_v7()).bind(tenant_id).bind(workflow_id).bind(draft_id)
                .bind(&node.id).bind(&node.name).bind(reference.resource_type.as_str())
                .bind(reference.resource_id).bind(reference.resource_version_id)
                .bind(reference.operation.as_str()).execute(&mut **transaction).await?;
        }
        if is_composite_node(&node.node_type)
            && let Some(version_id) = node
                .parameters
                .get("workflowVersionId")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
        {
            let target_workflow_id: Uuid = sqlx::query_scalar(
                "SELECT workflow_id FROM workflow_versions WHERE tenant_id=? AND id=?",
            )
            .bind(tenant_id)
            .bind(version_id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or_else(|| ApiError::not_found("Sub-workflow version"))?;
            sqlx::query("INSERT INTO workflow_draft_resources(id,tenant_id,workflow_id,draft_id,node_id,node_name,resource_type,resource_id,resource_version_id,operation_key,relation) VALUES(?,?,?,?,?,?,?,?,?,'use','subworkflow')")
                .bind(Uuid::now_v7()).bind(tenant_id).bind(workflow_id).bind(draft_id)
                .bind(&node.id).bind(&node.name).bind("workflow").bind(target_workflow_id)
                .bind(version_id).execute(&mut **transaction).await?;
        }
    }
    Ok(())
}

pub(crate) async fn build_version_snapshots(
    state: &ControlApiState,
    tenant_id: Uuid,
    workflow_id: Uuid,
    definition: &WorkflowDefinition,
) -> ApiResult<Vec<ResourceVersionSnapshot>> {
    let identity_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow_service_identities WHERE tenant_id=? AND workflow_id=? AND status='active'",
    )
    .bind(tenant_id)
    .bind(workflow_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::unprocessable("WORKFLOW_IDENTITY_INACTIVE", "Workflow Service Identity is not active"))?;
    let mut queue = VecDeque::new();
    for node in &definition.nodes {
        for reference in &node.resource_references {
            queue.push_back((node.id.clone(), reference.clone(), Vec::<Uuid>::new()));
        }
    }
    let mut seen = HashMap::new();
    let mut snapshots = Vec::new();
    while let Some((node_id, mut reference, ancestry)) = queue.pop_front() {
        let snapshot = resource_snapshot(state, tenant_id, &mut reference).await?;
        if reference.resource_type == ResourceType::Skill
            && reference
                .resource_version_id
                .is_some_and(|version| ancestry.contains(&version))
        {
            return Err(ApiError::unprocessable(
                "AGENT_SKILL_DEPENDENCY_CYCLE",
                "Skill dependency graph contains a cycle",
            ));
        }
        let identity = (
            node_id.clone(),
            reference.resource_type,
            reference.resource_id,
            reference.operation,
        );
        if let Some(existing_version) = seen.get(&identity) {
            if existing_version != &reference.resource_version_id {
                return Err(ApiError::unprocessable(
                    "AGENT_SKILL_DEPENDENCY_VERSION_CONFLICT",
                    format!(
                        "Resource {} is required at conflicting exact versions",
                        reference.resource_id
                    ),
                ));
            }
            continue;
        }
        seen.insert(identity, reference.resource_version_id);
        require_grant(state, tenant_id, identity_id, &reference).await?;
        if reference.resource_type == ResourceType::Skill {
            let version_id = reference.resource_version_id.ok_or_else(|| {
                ApiError::unprocessable("RESOURCE_VERSION_MISSING", "Skill version is required")
            })?;
            let mut dependency_ancestry = ancestry.clone();
            dependency_ancestry.push(version_id);
            let rows = sqlx::query("SELECT resource_type,resource_id,resource_version_id,operation_key FROM skill_dependencies WHERE tenant_id=? AND skill_version_id=?")
                .bind(tenant_id).bind(version_id).fetch_all(&state.pool).await?;
            for row in rows {
                queue.push_back((
                    node_id.clone(),
                    ResourceReference {
                        binding_role: None,
                        resource_type: parse_resource_type(row.try_get("resource_type")?)?,
                        resource_id: row.try_get("resource_id")?,
                        resource_version_id: row.try_get("resource_version_id")?,
                        operation: parse_operation(row.try_get("operation_key")?)?,
                    },
                    dependency_ancestry.clone(),
                ));
            }
        }
        if reference.resource_type == ResourceType::McpTool {
            let row = sqlx::query("SELECT t.server_id,tv.server_version_id FROM mcp_tools t JOIN mcp_tool_versions tv ON tv.tenant_id=t.tenant_id AND tv.tool_id=t.id WHERE t.tenant_id=? AND t.id=? AND tv.id=?")
                    .bind(tenant_id)
                    .bind(reference.resource_id)
                    .bind(reference.resource_version_id)
                    .fetch_one(&state.pool)
                    .await?;
            let server_id: Uuid = row.try_get("server_id")?;
            queue.push_back((
                node_id.clone(),
                ResourceReference {
                    resource_version_id: Some(row.try_get("server_version_id")?),
                    ..generated_reference(ResourceType::McpServer, server_id)
                },
                ancestry.clone(),
            ));
        }
        if reference.resource_type == ResourceType::McpServer
            && snapshot.pointer("/transport/kind").and_then(Value::as_str) == Some("stdio")
        {
            let profile_id = snapshot
                .get("runtimeSandboxProfileId")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(|| {
                    ApiError::unprocessable(
                        "MCP_STDIO_SANDBOX_REQUIRED",
                        "stdio MCP Server Version has no Runtime Sandbox",
                    )
                })?;
            let profile_version_id = snapshot
                .get("runtimeSandboxProfileVersionId")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(|| {
                    ApiError::unprocessable(
                        "MCP_STDIO_SANDBOX_REQUIRED",
                        "stdio MCP Server Version has no exact Runtime Sandbox version",
                    )
                })?;
            queue.push_back((
                node_id.clone(),
                ResourceReference {
                    binding_role: None,
                    resource_type: ResourceType::SandboxProfile,
                    resource_id: profile_id,
                    resource_version_id: Some(profile_version_id),
                    operation: ResourceOperation::Use,
                },
                ancestry.clone(),
            ));
            if let Some(credentials) = snapshot
                .pointer("/transport/environmentCredentialRefs")
                .and_then(Value::as_array)
            {
                for credential_id in credentials
                    .iter()
                    .filter_map(|value| value.get("credentialId"))
                    .filter_map(Value::as_str)
                    .filter_map(|value| Uuid::parse_str(value).ok())
                {
                    queue.push_back((
                        node_id.clone(),
                        generated_reference(ResourceType::Credential, credential_id),
                        ancestry.clone(),
                    ));
                }
            }
        }
        for credential_id in credential_dependencies(state, tenant_id, &reference).await? {
            queue.push_back((
                node_id.clone(),
                generated_reference(ResourceType::Credential, credential_id),
                ancestry.clone(),
            ));
        }
        snapshots.push(ResourceVersionSnapshot {
            node_id,
            reference,
            snapshot_hash: agentx_runtime_contracts::content_hash(&snapshot)
                .map_err(ApiError::internal)?
                .to_string(),
            snapshot,
        });
    }
    validate_sandbox_egress(definition, &snapshots)?;
    Ok(snapshots)
}

fn validate_sandbox_egress(
    definition: &WorkflowDefinition,
    snapshots: &[ResourceVersionSnapshot],
) -> ApiResult<()> {
    for node in definition
        .nodes
        .iter()
        .filter(|node| node.node_type == "code")
    {
        let requested = node
            .parameters
            .pointer("/networkPolicy/mode")
            .and_then(Value::as_str)
            .unwrap_or("deny");
        if !matches!(requested, "deny" | "allowlist") {
            return Err(ApiError::unprocessable(
                "INVALID_SANDBOX_EGRESS_MODE",
                format!(
                    "Code node {} network mode must be deny or allowlist",
                    node.id
                ),
            ));
        }
        if requested != "allowlist" {
            continue;
        }
        let allowed = snapshots.iter().any(|snapshot| {
            snapshot.node_id == node.id
                && snapshot.reference.resource_type == ResourceType::SandboxProfile
                && snapshot.snapshot.pointer("/networkPolicy/egressMode")
                    == Some(&Value::String("tcp_proxy".to_owned()))
        });
        if !allowed {
            return Err(ApiError::unprocessable(
                "SANDBOX_EGRESS_EXCEEDS_PROFILE",
                format!(
                    "Code node {} requests TCP proxy access but its Sandbox Profile does not allow it",
                    node.id
                ),
            ));
        }
    }
    Ok(())
}

pub(crate) async fn insert_version_snapshots(
    transaction: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    workflow_version_id: Uuid,
    snapshots: &[ResourceVersionSnapshot],
) -> ApiResult<()> {
    for snapshot in snapshots {
        sqlx::query("INSERT INTO workflow_version_resources(id,tenant_id,workflow_version_id,node_id,resource_type,resource_id,resource_version_id,operation_key,snapshot_json,snapshot_hash) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(Uuid::now_v7()).bind(tenant_id).bind(workflow_version_id)
            .bind(&snapshot.node_id).bind(snapshot.reference.resource_type.as_str())
            .bind(snapshot.reference.resource_id).bind(snapshot.reference.resource_version_id)
            .bind(snapshot.reference.operation.as_str()).bind(&snapshot.snapshot)
            .bind(&snapshot.snapshot_hash).execute(&mut **transaction).await?;
    }
    Ok(())
}

fn generated_reference(resource_type: ResourceType, resource_id: Uuid) -> ResourceReference {
    ResourceReference {
        binding_role: None,
        resource_type,
        resource_id,
        resource_version_id: None,
        operation: ResourceOperation::Use,
    }
}

async fn require_grant(
    state: &ControlApiState,
    tenant_id: Uuid,
    identity_id: Uuid,
    reference: &ResourceReference,
) -> ApiResult<()> {
    let granted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grants WHERE tenant_id=? AND subject_type='workflow_service_identity' AND subject_id=? AND resource_type=? AND resource_id=? AND (operation_key=? OR operation_key='manage') AND (resource_version_id IS NULL OR resource_version_id=?))")
        .bind(tenant_id).bind(identity_id).bind(reference.resource_type.as_str())
        .bind(reference.resource_id).bind(reference.operation.as_str())
        .bind(reference.resource_version_id).fetch_one(&state.pool).await?;
    if granted {
        Ok(())
    } else {
        Err(ApiError::unprocessable(
            "RESOURCE_GRANT_MISSING",
            format!(
                "{} {} requires {} grant",
                reference.resource_type.as_str(),
                reference.resource_id,
                reference.operation.as_str()
            ),
        ))
    }
}

pub(crate) async fn resource_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    reference: &mut ResourceReference,
) -> ApiResult<Value> {
    match reference.resource_type {
        ResourceType::Credential => {
            credential_snapshot(state, tenant_id, reference.resource_id).await
        }
        ResourceType::Model => model_snapshot(state, tenant_id, reference).await,
        ResourceType::McpServer => mcp_server_snapshot(state, tenant_id, reference).await,
        ResourceType::McpTool => mcp_tool_snapshot(state, tenant_id, reference).await,
        ResourceType::Skill => skill_snapshot(state, tenant_id, reference).await,
        ResourceType::Rag => external_snapshot(state, tenant_id, reference, true).await,
        ResourceType::Memory => external_snapshot(state, tenant_id, reference, false).await,
        ResourceType::SandboxProfile => sandbox_snapshot(state, tenant_id, reference).await,
    }
}

async fn credential_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    credential_id: Uuid,
) -> ApiResult<Value> {
    let row = sqlx::query("SELECT c.id,c.credential_type,c.current_secret_version,c.version,sv.secret_ref,sv.provider_version FROM credentials c JOIN credential_secret_versions sv ON sv.tenant_id=c.tenant_id AND sv.credential_id=c.id AND sv.version_number=c.current_secret_version WHERE c.tenant_id=? AND c.id=? AND c.status='active' AND sv.provider='vault_kv_v2'")
        .bind(tenant_id).bind(credential_id).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::unprocessable("RESOURCE_UNAVAILABLE", "Credential is missing, disabled, or has no Vault version"))?;
    let provider_version = row
        .try_get::<String, _>("provider_version")?
        .parse::<u64>()
        .map_err(|_| ApiError::internal("Vault provider version is invalid"))?;
    Ok(json!({
        "id":row.try_get::<Uuid,_>("id")?,
        "credentialType":row.try_get::<String,_>("credential_type")?,
        "secretVersion":row.try_get::<u64,_>("current_secret_version")?,
        "resourceVersion":row.try_get::<u64,_>("version")?,
        "vaultSecretRef":{
            "mount":state.vault_mount,
            "path":row.try_get::<String,_>("secret_ref")?,
            "key":"value",
            "version":provider_version
        }
    }))
}

async fn model_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    reference: &mut ResourceReference,
) -> ApiResult<Value> {
    let row = sqlx::query("SELECT a.id alias_id,a.alias,a.version alias_version,d.id deployment_id,d.connection_name,d.model_name,d.max_input_tokens,d.max_output_tokens,d.version deployment_version,d.default_parameters,d.capabilities_json,d.endpoint,d.provider_type,d.credential_id,pv.id price_version_id,pv.version_number price_version_number,pv.currency,CAST(pv.input_per_million AS CHAR) input_per_million,CAST(pv.output_per_million AS CHAR) output_per_million FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id JOIN model_price_versions pv ON pv.tenant_id=d.tenant_id AND pv.deployment_id=d.id AND pv.id=(SELECT latest.id FROM model_price_versions latest WHERE latest.tenant_id=d.tenant_id AND latest.deployment_id=d.id ORDER BY latest.version_number DESC LIMIT 1) WHERE a.tenant_id=? AND a.id=? AND a.status='active' AND d.status='active'")
        .bind(tenant_id).bind(reference.resource_id).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::unprocessable("RESOURCE_UNAVAILABLE", "Model is missing or disabled"))?;
    let deployment_id: Uuid = row.try_get("deployment_id")?;
    if let Some(requested) = reference.resource_version_id
        && requested != deployment_id
    {
        return Err(ApiError::unprocessable(
            "RESOURCE_VERSION_INVALID",
            "Model alias no longer points to the requested deployment",
        ));
    }
    reference.resource_version_id = Some(deployment_id);
    let credential_id: Option<Uuid> = row.try_get("credential_id")?;
    let secret = optional_credential_snapshot(state, tenant_id, credential_id).await?;
    Ok(json!({
        "aliasId":row.try_get::<Uuid,_>("alias_id")?,"alias":row.try_get::<String,_>("alias")?,
        "aliasVersion":row.try_get::<u64,_>("alias_version")?,"deploymentId":deployment_id,
        "deploymentVersion":row.try_get::<u64,_>("deployment_version")?,
        "connectionName":row.try_get::<String,_>("connection_name")?,"modelName":row.try_get::<String,_>("model_name")?,
        "maxInputTokens":row.try_get::<u64,_>("max_input_tokens")?,"maxOutputTokens":row.try_get::<u64,_>("max_output_tokens")?,
        "defaultParameters":row.try_get::<Value,_>("default_parameters")?,"providerType":row.try_get::<String,_>("provider_type")?,
        "endpoint":row.try_get::<String,_>("endpoint")?,"credentialId":credential_id,"vaultSecretRef":secret,
        "capabilities":row.try_get::<Option<Value>,_>("capabilities_json")?.unwrap_or_else(|| json!([])),
        "price":{"versionId":row.try_get::<Uuid,_>("price_version_id")?,"versionNumber":row.try_get::<u64,_>("price_version_number")?,"currency":row.try_get::<String,_>("currency")?,"inputPerMillion":row.try_get::<String,_>("input_per_million")?,"outputPerMillion":row.try_get::<String,_>("output_per_million")?}
    }))
}

async fn mcp_server_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    reference: &mut ResourceReference,
) -> ApiResult<Value> {
    let row = if let Some(version_id) = reference.resource_version_id {
        sqlx::query("SELECT s.id,s.name,s.version,sv.id server_version_id,sv.version_number,sv.transport,sv.endpoint,sv.credential_id,sv.runtime_sandbox_profile_id,sv.runtime_sandbox_profile_version_id,sv.configuration_json,sv.configuration_hash FROM mcp_servers s JOIN mcp_server_versions sv ON sv.server_id=s.id AND sv.id=? WHERE s.tenant_id=? AND s.id=? AND s.status='active'")
            .bind(version_id).bind(tenant_id).bind(reference.resource_id).fetch_optional(&state.pool).await?
    } else {
        sqlx::query("SELECT s.id,s.name,s.version,sv.id server_version_id,sv.version_number,sv.transport,sv.endpoint,sv.credential_id,sv.runtime_sandbox_profile_id,sv.runtime_sandbox_profile_version_id,sv.configuration_json,sv.configuration_hash FROM mcp_servers s JOIN mcp_server_versions sv ON sv.server_id=s.id AND sv.version_number=s.current_version_number WHERE s.tenant_id=? AND s.id=? AND s.status='active'")
            .bind(tenant_id).bind(reference.resource_id).fetch_optional(&state.pool).await?
    }
        .ok_or_else(|| ApiError::unprocessable("RESOURCE_UNAVAILABLE", "MCP server is missing or disabled"))?;
    let version_id: Uuid = row.try_get("server_version_id")?;
    reference.resource_version_id = Some(version_id);
    let credential_id: Option<Uuid> = row.try_get("credential_id")?;
    let transport = mcp_transport_snapshot(
        state,
        tenant_id,
        row.try_get::<Value, _>("configuration_json")?
            .get("transport")
            .cloned()
            .unwrap_or(Value::Null),
    )
    .await?;
    Ok(
        json!({"serverId":row.try_get::<Uuid,_>("id")?,"name":row.try_get::<String,_>("name")?,
        "serverVersionId":version_id,"versionNumber":row.try_get::<u64,_>("version_number")?,
        "transport":transport,
        "endpoint":row.try_get::<Option<String>,_>("endpoint")?,
        "credentialId":credential_id,"configurationHash":prefixed_hash(row.try_get("configuration_hash")?),
        "runtimeSandboxProfileId":row.try_get::<Option<Uuid>,_>("runtime_sandbox_profile_id")?,
        "runtimeSandboxProfileVersionId":row.try_get::<Option<Uuid>,_>("runtime_sandbox_profile_version_id")?,
        "vaultSecretRef":optional_credential_snapshot(state,tenant_id,credential_id).await?}),
    )
}

async fn mcp_tool_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    reference: &mut ResourceReference,
) -> ApiResult<Value> {
    let version_id =
        resolve_version(state, tenant_id, reference, "mcp_tool_versions", "tool_id").await?;
    reference.resource_version_id = Some(version_id);
    let row = sqlx::query("SELECT tv.id,tv.version_number,tv.input_schema,tv.output_schema,tv.annotations_json,tv.schema_hash,t.name,t.title,s.id server_id,sv.id server_version_id,sv.transport,sv.endpoint,sv.credential_id,sv.runtime_sandbox_profile_id,sv.runtime_sandbox_profile_version_id,sv.configuration_json,sv.configuration_hash,p.timeout_seconds,p.side_effect FROM mcp_tool_versions tv JOIN mcp_tools t ON t.id=tv.tool_id JOIN mcp_servers s ON s.id=t.server_id JOIN mcp_server_versions sv ON sv.id=tv.server_version_id AND sv.server_id=s.id JOIN mcp_tool_policies p ON p.tool_id=t.id AND p.tenant_id=t.tenant_id WHERE tv.tenant_id=? AND tv.id=? AND tv.tool_id=? AND t.availability='available' AND s.status='active' AND p.enabled=TRUE")
        .bind(tenant_id).bind(version_id).bind(reference.resource_id).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::unprocessable("RESOURCE_UNAVAILABLE", "MCP tool is missing, disabled, or unavailable"))?;
    let credential_id: Option<Uuid> = row.try_get("credential_id")?;
    let transport = mcp_transport_snapshot(
        state,
        tenant_id,
        row.try_get::<Value, _>("configuration_json")?
            .get("transport")
            .cloned()
            .unwrap_or(Value::Null),
    )
    .await?;
    Ok(
        json!({"toolVersionId":version_id,"toolVersionNumber":row.try_get::<u64,_>("version_number")?,
        "toolName":row.try_get::<String,_>("name")?,"title":row.try_get::<Option<String>,_>("title")?,
        "serverId":row.try_get::<Uuid,_>("server_id")?,"serverVersionId":row.try_get::<Uuid,_>("server_version_id")?,
        "transport":transport,
        "endpoint":row.try_get::<Option<String>,_>("endpoint")?,
        "credentialId":credential_id,"configurationHash":prefixed_hash(row.try_get("configuration_hash")?),
        "runtimeSandboxProfileId":row.try_get::<Option<Uuid>,_>("runtime_sandbox_profile_id")?,
        "runtimeSandboxProfileVersionId":row.try_get::<Option<Uuid>,_>("runtime_sandbox_profile_version_id")?,
        "inputSchema":row.try_get::<Value,_>("input_schema")?,"outputSchema":row.try_get::<Option<Value>,_>("output_schema")?,
        "annotations":row.try_get::<Value,_>("annotations_json")?,"schemaHash":prefixed_hash(row.try_get("schema_hash")?),
        "timeoutSeconds":row.try_get::<u32,_>("timeout_seconds")?,"sideEffect":row.try_get::<String,_>("side_effect")?,
        "vaultSecretRef":optional_credential_snapshot(state,tenant_id,credential_id).await?}),
    )
}

async fn skill_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    reference: &mut ResourceReference,
) -> ApiResult<Value> {
    let version_id =
        resolve_version(state, tenant_id, reference, "skill_versions", "skill_id").await?;
    reference.resource_version_id = Some(version_id);
    let row = sqlx::query("SELECT sv.version_number,sv.source_revision,sv.content_hash FROM skill_versions sv JOIN skills s ON s.id=sv.skill_id WHERE sv.tenant_id=? AND sv.id=? AND sv.skill_id=? AND s.status='active'")
        .bind(tenant_id).bind(version_id).bind(reference.resource_id).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::unprocessable("RESOURCE_UNAVAILABLE", "Skill version is unavailable"))?;
    let files = sqlx::query("SELECT f.path,f.mime_type,f.content_hash,f.size_bytes,a.storage_key FROM skill_version_files f JOIN artifacts a ON a.tenant_id=f.tenant_id AND a.id=f.artifact_id AND a.deleted_at IS NULL WHERE f.tenant_id=? AND f.skill_version_id=? ORDER BY f.path")
        .bind(tenant_id)
        .bind(version_id)
        .fetch_all(&state.pool)
        .await?;
    if files.is_empty() {
        return Err(ApiError::unprocessable(
            "SKILL_INVALID",
            "Skill version has no immutable files",
        ));
    }
    let mut instructions = None;
    let mut assets = Vec::with_capacity(files.len());
    let mut runtime_objects = Vec::with_capacity(files.len() + 1);
    for file in files {
        let path: String = file.try_get("path")?;
        let source_key: String = file.try_get("storage_key")?;
        let content_hash = ContentHash::parse(prefixed_hash(file.try_get("content_hash")?))
            .map_err(ApiError::internal)?;
        let size_bytes: u64 = file.try_get("size_bytes")?;
        let media_type: String = file.try_get("mime_type")?;
        let object_id = deterministic_uuid(version_id, format!("skill-asset:{path}").as_bytes());
        if path == "SKILL.md" {
            let bytes = state
                .control_objects
                .get(&ObjectPath::from(source_key.clone()))
                .await
                .map_err(ApiError::internal)?
                .bytes()
                .await
                .map_err(ApiError::internal)?;
            instructions =
                Some(String::from_utf8(bytes.to_vec()).map_err(|_| {
                    ApiError::unprocessable("SKILL_INVALID", "SKILL.md must be UTF-8")
                })?);
        }
        assets.push(RuntimeSkillAssetV2 {
            path,
            object_id,
            content_hash: content_hash.clone(),
            media_type: media_type.clone(),
            size_bytes,
        });
        runtime_objects.push(json!({
            "objectId": object_id,
            "sourceKey": source_key,
            "contentHash": content_hash.as_str(),
            "sizeBytes": size_bytes,
            "mediaType": media_type,
        }));
    }
    let instructions = instructions
        .ok_or_else(|| ApiError::unprocessable("SKILL_INVALID", "Skill version has no SKILL.md"))?;
    let dependencies = sqlx::query("SELECT resource_type,resource_id,resource_version_id,operation_key FROM skill_dependencies WHERE tenant_id=? AND skill_version_id=? ORDER BY resource_type,resource_id,resource_version_id,operation_key")
        .bind(tenant_id)
        .bind(version_id)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(|dependency| {
            Ok(RuntimeSkillDependencyV2 {
                resource_type: dependency.try_get("resource_type")?,
                resource_id: dependency.try_get("resource_id")?,
                resource_version_id: dependency.try_get("resource_version_id")?,
                operation: dependency.try_get("operation_key")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let program = RuntimeSkillProgramV2 {
        schema_version: 2,
        skill_version_id: version_id,
        instructions,
        assets,
        dependencies,
    };
    let bytes = agentx_runtime_contracts::canonical_bytes(&program).map_err(ApiError::internal)?;
    let hash = agentx_runtime_contracts::content_hash(&program).map_err(ApiError::internal)?;
    let runtime_source_key = format!(
        "runtime-skills/{tenant_id}/{version_id}/{}",
        hash.as_str().trim_start_matches("sha256:")
    );
    state
        .control_objects
        .put(
            &ObjectPath::from(runtime_source_key.clone()),
            Bytes::from(bytes.clone()).into(),
        )
        .await
        .map_err(ApiError::internal)?;
    let entrypoint_object_id = deterministic_uuid(version_id, b"runtime-skill-program-v2");
    let dependency_object_ids = program
        .assets
        .iter()
        .map(|asset| asset.object_id)
        .collect::<Vec<_>>();
    runtime_objects.push(json!({
        "objectId": entrypoint_object_id,
        "sourceKey": runtime_source_key,
        "contentHash": hash.as_str(),
        "sizeBytes": bytes.len(),
        "mediaType": "application/vnd.agentx.runtime-skill.v2+json",
    }));
    Ok(
        json!({"versionId":version_id,"versionNumber":row.try_get::<u64,_>("version_number")?,
        "sourceRevision":row.try_get::<u64,_>("source_revision")?,"contentHash":row.try_get::<String,_>("content_hash")?,
        "resourceVersion":row.try_get::<u64,_>("version_number")?,
        "entrypointObjectId":entrypoint_object_id,
        "entrypointContentHash":hash.as_str(),
        "dependencyObjectIds":dependency_object_ids,
        "dependencies":program.dependencies,
        "runtimeObjects":runtime_objects}),
    )
}

async fn external_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    reference: &mut ResourceReference,
    rag: bool,
) -> ApiResult<Value> {
    let resource_id = reference.resource_id;
    // Rag and Memory are unpinned "follow current" resources in drafts; pin a
    // deterministic synthetic version so runtime bindings keep matching the
    // compiled attachment descriptors (same derivation in compiler_agent).
    if reference.resource_version_id.is_none() {
        reference.resource_version_id = Some(agentx_runtime_contracts::deterministic_uuid(
            resource_id,
            b"external-current",
        ));
    }
    let row = if rag {
        sqlx::query("SELECT r.external_resource_id external_name,r.version resource_version,c.id connection_id,c.provider,c.endpoint,c.version connection_version,c.credential_id,c.configuration_json FROM rag_resources r JOIN rag_connections c ON c.id=r.connection_id WHERE r.tenant_id=? AND r.id=? AND r.status='active' AND c.status='active'")
            .bind(tenant_id).bind(resource_id).fetch_optional(&state.pool).await?
    } else {
        sqlx::query("SELECT n.external_namespace external_name,n.access_mode,n.version resource_version,c.id connection_id,NULL provider,c.endpoint,c.version connection_version,c.credential_id,c.configuration_json FROM memory_namespaces n JOIN memory_connections c ON c.id=n.connection_id WHERE n.tenant_id=? AND n.id=? AND n.status='active' AND c.status='active'")
            .bind(tenant_id).bind(resource_id).fetch_optional(&state.pool).await?
    }.ok_or_else(|| ApiError::unprocessable("RESOURCE_UNAVAILABLE", "External resource or connection is unavailable"))?;
    let credential_id: Option<Uuid> = row.try_get("credential_id")?;
    let mut value = json!({"resourceVersion":row.try_get::<u64,_>("resource_version")?,
        "connectionId":row.try_get::<Uuid,_>("connection_id")?,"provider":row.try_get::<Option<String>,_>("provider")?.unwrap_or_else(||"lightrag".into()),"endpoint":row.try_get::<String,_>("endpoint")?,
        "connectionVersion":row.try_get::<u64,_>("connection_version")?,"credentialId":credential_id,
        "configuration":row.try_get::<Value,_>("configuration_json")?,
        "vaultSecretRef":optional_credential_snapshot(state,tenant_id,credential_id).await?});
    if rag {
        value["resourceId"] = json!(resource_id);
        value["externalResourceId"] = json!(row.try_get::<String, _>("external_name")?);
    } else {
        value["namespaceId"] = json!(resource_id);
        value["externalNamespace"] = json!(row.try_get::<String, _>("external_name")?);
        value["accessMode"] = json!(row.try_get::<String, _>("access_mode")?);
    }
    Ok(value)
}

async fn sandbox_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    reference: &mut ResourceReference,
) -> ApiResult<Value> {
    let version_id = resolve_version(
        state,
        tenant_id,
        reference,
        "sandbox_profile_versions",
        "profile_id",
    )
    .await?;
    reference.resource_version_id = Some(version_id);
    let row = sqlx::query("SELECT v.version_number,v.runner,v.image_digest,v.cpu_millis,v.memory_bytes,v.pids_limit,v.disk_bytes,v.timeout_seconds,v.output_limit_bytes,v.network_policy_json,v.configuration_hash FROM sandbox_profile_versions v JOIN sandbox_profiles p ON p.id=v.profile_id WHERE v.tenant_id=? AND v.id=? AND v.profile_id=? AND p.status='active'")
        .bind(tenant_id).bind(version_id).bind(reference.resource_id).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::unprocessable("RESOURCE_UNAVAILABLE", "Sandbox Profile version is unavailable"))?;
    Ok(
        json!({"profileVersionId":version_id,"versionNumber":row.try_get::<u64,_>("version_number")?,
        "runner":row.try_get::<String,_>("runner")?,"provider":"opensandbox","imageDigest":row.try_get::<String,_>("image_digest")?,
        "cpuMillis":row.try_get::<u32,_>("cpu_millis")?,"memoryBytes":row.try_get::<u64,_>("memory_bytes")?,
        "pidsLimit":row.try_get::<u32,_>("pids_limit")?,"diskBytes":row.try_get::<u64,_>("disk_bytes")?,
        "timeoutSeconds":row.try_get::<u32,_>("timeout_seconds")?,"outputLimitBytes":row.try_get::<u64,_>("output_limit_bytes")?,
        "networkPolicy":row.try_get::<Value,_>("network_policy_json")?,"configurationHash":prefixed_hash(row.try_get("configuration_hash")?),
        "resourceVersion":row.try_get::<u64,_>("version_number")?}),
    )
}

async fn resolve_version(
    state: &ControlApiState,
    tenant_id: Uuid,
    reference: &ResourceReference,
    table: &str,
    parent_column: &str,
) -> ApiResult<Uuid> {
    if let Some(id) = reference.resource_version_id {
        return Ok(id);
    }
    let id = match (table, parent_column) {
        ("mcp_tool_versions", "tool_id") => sqlx::query_scalar("SELECT id FROM mcp_tool_versions WHERE tenant_id=? AND tool_id=? ORDER BY version_number DESC LIMIT 1").bind(tenant_id).bind(reference.resource_id).fetch_optional(&state.pool).await?,
        ("skill_versions", "skill_id") => sqlx::query_scalar("SELECT id FROM skill_versions WHERE tenant_id=? AND skill_id=? ORDER BY version_number DESC LIMIT 1").bind(tenant_id).bind(reference.resource_id).fetch_optional(&state.pool).await?,
        ("sandbox_profile_versions", "profile_id") => sqlx::query_scalar("SELECT id FROM sandbox_profile_versions WHERE tenant_id=? AND profile_id=? ORDER BY version_number DESC LIMIT 1").bind(tenant_id).bind(reference.resource_id).fetch_optional(&state.pool).await?,
        _ => None,
    };
    id.ok_or_else(|| {
        ApiError::unprocessable(
            "RESOURCE_VERSION_MISSING",
            "Resource has no available version",
        )
    })
}

async fn credential_dependencies(
    state: &ControlApiState,
    tenant_id: Uuid,
    reference: &ResourceReference,
) -> ApiResult<Vec<Uuid>> {
    let ids = match reference.resource_type {
        ResourceType::Model => sqlx::query_scalar("SELECT d.credential_id FROM model_aliases a JOIN model_deployments d ON d.id=a.deployment_id WHERE a.tenant_id=? AND a.id=? AND d.credential_id IS NOT NULL").bind(tenant_id).bind(reference.resource_id).fetch_all(&state.pool).await?,
        ResourceType::McpServer => sqlx::query_scalar("SELECT credential_id FROM mcp_server_versions WHERE tenant_id=? AND server_id=? AND id=? AND credential_id IS NOT NULL").bind(tenant_id).bind(reference.resource_id).bind(reference.resource_version_id).fetch_all(&state.pool).await?,
        ResourceType::Rag => sqlx::query_scalar("SELECT c.credential_id FROM rag_resources r JOIN rag_connections c ON c.id=r.connection_id WHERE r.tenant_id=? AND r.id=? AND c.credential_id IS NOT NULL").bind(tenant_id).bind(reference.resource_id).fetch_all(&state.pool).await?,
        ResourceType::Memory => sqlx::query_scalar("SELECT c.credential_id FROM memory_namespaces n JOIN memory_connections c ON c.id=n.connection_id WHERE n.tenant_id=? AND n.id=? AND c.credential_id IS NOT NULL").bind(tenant_id).bind(reference.resource_id).fetch_all(&state.pool).await?,
        _ => Vec::new(),
    };
    Ok(ids)
}

async fn optional_credential_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    credential_id: Option<Uuid>,
) -> ApiResult<Option<Value>> {
    match credential_id {
        Some(id) => Ok(Some(
            credential_snapshot(state, tenant_id, id).await?["vaultSecretRef"].clone(),
        )),
        None => Ok(None),
    }
}

async fn mcp_transport_snapshot(
    state: &ControlApiState,
    tenant_id: Uuid,
    mut transport: Value,
) -> ApiResult<Value> {
    if transport.get("kind").and_then(Value::as_str) != Some("stdio") {
        return Ok(transport);
    }
    let Some(references) = transport
        .get_mut("environmentCredentialRefs")
        .and_then(Value::as_array_mut)
    else {
        return Ok(transport);
    };
    for reference in references {
        let credential_id = reference
            .get("credentialId")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| {
                ApiError::unprocessable(
                    "MCP_STDIO_CREDENTIAL_INVALID",
                    "stdio MCP environment Credential is invalid",
                )
            })?;
        reference["vaultSecretRef"] =
            optional_credential_snapshot(state, tenant_id, Some(credential_id))
                .await?
                .unwrap_or(Value::Null);
    }
    Ok(transport)
}

fn parse_resource_type(value: String) -> ApiResult<ResourceType> {
    match value.as_str() {
        "credential" => Ok(ResourceType::Credential),
        "model" => Ok(ResourceType::Model),
        "mcp_server" => Ok(ResourceType::McpServer),
        "mcp_tool" => Ok(ResourceType::McpTool),
        "skill" => Ok(ResourceType::Skill),
        "rag" => Ok(ResourceType::Rag),
        "memory" => Ok(ResourceType::Memory),
        "sandbox_profile" => Ok(ResourceType::SandboxProfile),
        _ => Err(ApiError::unprocessable(
            "INVALID_RESOURCE_TYPE",
            "Skill dependency resource type is invalid",
        )),
    }
}

fn parse_operation(value: String) -> ApiResult<ResourceOperation> {
    match value.as_str() {
        "view" => Ok(ResourceOperation::View),
        "use" => Ok(ResourceOperation::Use),
        "read" => Ok(ResourceOperation::Read),
        "write" => Ok(ResourceOperation::Write),
        "manage" => Ok(ResourceOperation::Manage),
        _ => Err(ApiError::unprocessable(
            "INVALID_RESOURCE_OPERATION",
            "Skill dependency operation is invalid",
        )),
    }
}

fn prefixed_hash(value: String) -> String {
    if value.starts_with("sha256:") {
        value
    } else {
        format!("sha256:{value}")
    }
}

fn is_composite_node(node_type: &str) -> bool {
    node_type == "sub_workflow"
}
