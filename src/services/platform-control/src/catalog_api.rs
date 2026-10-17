use std::collections::BTreeMap;

use agentx_api_types::PageResponse;
use agentx_node_protocol::NodeManifestVersion;
use agentx_runtime::NodeRegistry;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/node-definitions", get(list_node_definitions))
        .route(
            "/api/v1/node-definitions/{node_type}/versions/{version}",
            get(get_node_definition),
        )
        .route(
            "/api/v1/node-definitions/{node_type}/versions/{version}/providers/{provider}",
            get(list_provider_options),
        )
        .route(
            "/api/v1/node-definitions/{node_type}/versions/{version}/resolve",
            post(resolve_node_definition),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    search: Option<String>,
    category: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeDefinitionSummary {
    node_type: String,
    version: u32,
    display_name: String,
    description: String,
    category: String,
    keywords: Vec<String>,
    icon_key: String,
    capability: String,
    execution_style: String,
    manifest_hash: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeDefinitionDetail {
    node_type: String,
    version: u32,
    manifest_hash: String,
    manifest: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderQuery {
    search: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
    parameters: Option<String>,
    resource_references: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderOption {
    value: String,
    label: String,
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest: Option<Value>,
}

#[derive(Serialize)]
struct ProviderOptionsResponse {
    items: Vec<ProviderOption>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveDefinitionRequest {
    #[serde(default)]
    configuration: Value,
    #[serde(default)]
    upstream_contracts: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NodeDefinitionQuery {
    bundle_digest: Option<String>,
}

async fn resolve_node_definition(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((node_type, version)): Path<(String, u32)>,
    Json(input): Json<ResolveDefinitionRequest>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:view")?;
    let manifest = manifests_for_tenant(&state, actor.tenant_id)
        .await?
        .into_iter()
        .find(|(manifest, _)| manifest.node_type == node_type && manifest.version == version)
        .map(|(manifest, _)| manifest)
        .ok_or_else(|| ApiError::not_found("Node definition"))?;
    let value = if manifest.capability == agentx_node_protocol::NodeCapability::PluginNodejs
        && let Some(binding) = manifest.plugin.as_ref()
    {
        crate::canvas_plugin_api::invoke_plugin_method(&state,actor.tenant_id,binding,"node.resolveDefinition",serde_json::json!({"nodeType":node_type,"configuration":input.configuration,"upstreamContracts":input.upstream_contracts,"inputPorts":manifest.input_ports,"outputPorts":manifest.output_ports,"outputSchema":manifest.output_schema,"outputPortSchemas":manifest.output_port_schemas})).await?
    } else {
        serde_json::json!({"status":"complete","inputPorts":manifest.input_ports,"outputPorts":manifest.output_ports,"outputSchema":manifest.output_schema,"outputPortSchemas":manifest.output_port_schemas})
    };
    Ok(Json(value))
}

async fn list_node_definitions(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<CatalogQuery>,
) -> ApiResult<Json<PageResponse<NodeDefinitionSummary>>> {
    actor.require("workflow:view")?;
    let mut manifests = manifests_for_tenant(&state, actor.tenant_id).await?;
    let search = query.search.unwrap_or_default().trim().to_ascii_lowercase();
    let category = query.category.unwrap_or_default();
    manifests.retain(|(manifest, _)| {
        (category.is_empty() || manifest.category == category)
            && (search.is_empty()
                || manifest.node_type.to_ascii_lowercase().contains(&search)
                || manifest.display_name.to_ascii_lowercase().contains(&search)
                || manifest
                    .keywords
                    .iter()
                    .any(|keyword| keyword.to_ascii_lowercase().contains(&search)))
    });
    manifests.sort_by(|left, right| {
        left.0
            .category
            .cmp(&right.0.category)
            .then(left.0.display_name.cmp(&right.0.display_name))
            .then(left.0.version.cmp(&right.0.version))
    });
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 100);
    let total = manifests.len() as u64;
    let items = manifests
        .into_iter()
        .skip(((page - 1) * page_size) as usize)
        .take(page_size as usize)
        .map(|(manifest, manifest_hash)| summary(manifest, manifest_hash))
        .collect();
    Ok(Json(PageResponse {
        items,
        page,
        page_size,
        total,
    }))
}

async fn get_node_definition(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((node_type, version)): Path<(String, u32)>,
    Query(query): Query<NodeDefinitionQuery>,
) -> ApiResult<Json<NodeDefinitionDetail>> {
    actor.require("workflow:view")?;
    let (manifest, hash) = if let Some(digest) = query.bundle_digest.as_deref() {
        crate::canvas_plugin_api::plugin_manifest_by_digest(
            &state,
            actor.tenant_id,
            &node_type,
            version,
            digest,
        )
        .await?
    } else {
        let current = manifests_for_tenant(&state, actor.tenant_id)
            .await?
            .into_iter()
            .find(|(manifest, _)| manifest.node_type == node_type && manifest.version == version);
        if let Some(current) = current {
            current
        } else {
            crate::canvas_plugin_api::plugin_manifest_by_identity(
                &state,
                actor.tenant_id,
                &node_type,
                version,
            )
            .await?
            .ok_or_else(|| ApiError::not_found("Node definition"))?
        }
    };
    Ok(Json(NodeDefinitionDetail {
        node_type,
        version,
        manifest_hash: hash,
        manifest: serde_json::to_value(manifest).map_err(ApiError::internal)?,
    }))
}

async fn list_provider_options(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((node_type, version, provider)): Path<(String, u32, String)>,
    Query(query): Query<ProviderQuery>,
) -> ApiResult<Json<ProviderOptionsResponse>> {
    actor.require("workflow:view")?;
    let manifest = manifests_for_tenant(&state, actor.tenant_id)
        .await?
        .into_iter()
        .find(|(manifest, _)| manifest.node_type == node_type && manifest.version == version)
        .map(|(manifest, _)| manifest)
        .ok_or_else(|| ApiError::not_found("Node definition"))?;
    if !manifest.providers.iter().any(|value| value == &provider) {
        return Err(ApiError::not_found("Node option provider"));
    }
    let raw_search = query.search.unwrap_or_default();
    let search = format!("%{}%", raw_search.trim());
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let provider_parameters = query
        .parameters
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| ApiError::bad_request("PROVIDER_PARAMETERS_INVALID", error.to_string()))?
        .unwrap_or_else(|| Value::Object(Default::default()));
    let host_resource_references = query
        .resource_references
        .as_deref()
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|error| ApiError::bad_request("PROVIDER_RESOURCES_INVALID", error.to_string()))?
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let mut next_cursor = None;
    let items = match provider.as_str() {
        "workflow_versions" => {
            let rows = sqlx::query("SELECT v.id,v.workflow_id,v.version_number,v.definition_json,w.name FROM workflow_versions v JOIN workflows w ON w.tenant_id=v.tenant_id AND w.id=v.workflow_id WHERE v.tenant_id=? AND v.publication_status='published' AND w.status='active' AND (?='%%' OR w.name LIKE ?) AND (w.owner_user_id=? OR w.visibility='company' OR EXISTS(SELECT 1 FROM workflow_members wm WHERE wm.tenant_id=w.tenant_id AND wm.workflow_id=w.id AND wm.user_id=?)) ORDER BY w.name,v.version_number DESC LIMIT ?")
                .bind(actor.tenant_id).bind(&search).bind(&search).bind(actor.user_id).bind(actor.user_id).bind(limit).fetch_all(&state.pool).await?;
            let definitions = rows
                .iter()
                .map(|row| {
                    Ok((
                        row.try_get::<Uuid, _>("id")?,
                        serde_json::from_value::<agentx_domain::WorkflowDefinition>(
                            row.try_get("definition_json")?,
                        )
                        .map_err(ApiError::internal)?,
                    ))
                })
                .collect::<ApiResult<BTreeMap<_, _>>>()?;
            let registry = agentx_bundle_builder::node_registry_with_composites(&definitions)
                .map_err(ApiError::internal)?;
            rows.into_iter()
                .map(|row| {
                    let version_id: Uuid = row.try_get("id")?;
                    let workflow_id: Uuid = row.try_get("workflow_id")?;
                    let version_number: u64 = row.try_get("version_number")?;
                    let name: String = row.try_get("name")?;
                    let node_type = format!("workflow.{}", version_id.simple());
                    let mut manifest = registry
                        .get(&node_type, 1)
                        .expect("derived Workflow Version Manifest")
                        .clone();
                    manifest.display_name = format!("{name} · v{version_number}");
                    if let Some(schema) = manifest.parameter_schema.as_object_mut() {
                        schema.insert(
                            "x-agentx-workflowId".into(),
                            Value::String(workflow_id.to_string()),
                        );
                        schema.insert(
                            "x-agentx-workflowVersionId".into(),
                            Value::String(version_id.to_string()),
                        );
                        schema.insert("x-agentx-versionNumber".into(), Value::from(version_number));
                    }
                    Ok(ProviderOption {
                        value: version_id.to_string(),
                        label: format!("{name} · v{version_number}"),
                        description: Some(format!("Published workflow version {version_number}")),
                        manifest: Some(serde_json::to_value(manifest).map_err(ApiError::internal)?),
                    })
                })
                .collect::<ApiResult<_>>()?
        }
        "users" => {
            let rows = sqlx::query("SELECT id,display_name,username FROM users WHERE tenant_id=? AND status='active' AND (?='%%' OR display_name LIKE ? OR username LIKE ?) ORDER BY display_name,id LIMIT ?")
                .bind(actor.tenant_id).bind(&search).bind(&search).bind(&search).bind(limit).fetch_all(&state.pool).await?;
            rows.into_iter()
                .map(|row| {
                    Ok(ProviderOption {
                        value: row.try_get::<Uuid, _>("id")?.to_string(),
                        label: row.try_get("display_name")?,
                        description: Some(format!("@{}", row.try_get::<String, _>("username")?)),
                        manifest: None,
                    })
                })
                .collect::<Result<_, sqlx::Error>>()?
        }
        _ => {
            if let Some(binding) = manifest.plugin.as_ref() {
                let value=crate::canvas_plugin_api::invoke_plugin_method(&state,actor.tenant_id,binding,"node.invokeProvider",serde_json::json!({"provider":provider,"input":{"search":raw_search,"limit":limit,"cursor":query.cursor,"parameters":provider_parameters},"hostResourceReferences":host_resource_references})).await?;
                if value.is_array() {
                    serde_json::from_value::<Vec<ProviderOption>>(value).map_err(|error| {
                        ApiError::unprocessable("PLUGIN_PROVIDER_RESULT_INVALID", error.to_string())
                    })?
                } else {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase", deny_unknown_fields)]
                    struct Page {
                        items: Vec<ProviderOption>,
                        next_cursor: Option<String>,
                    }
                    let page = serde_json::from_value::<Page>(value).map_err(|error| {
                        ApiError::unprocessable("PLUGIN_PROVIDER_RESULT_INVALID", error.to_string())
                    })?;
                    next_cursor = page.next_cursor;
                    page.items
                }
            } else {
                return Err(ApiError::unprocessable(
                    "NODE_PROVIDER_UNSUPPORTED",
                    format!("Provider '{provider}' is not available"),
                ));
            }
        }
    };
    Ok(Json(ProviderOptionsResponse { items, next_cursor }))
}

pub(crate) async fn manifests_for_tenant(
    state: &ControlApiState,
    tenant: Uuid,
) -> ApiResult<Vec<(NodeManifestVersion, String)>> {
    let rows = sqlx::query("SELECT d.node_type,d.source_type,v.version_number,v.manifest_json,v.manifest_hash FROM node_definitions d JOIN node_definition_versions v ON v.node_definition_id=d.id WHERE d.status='active' AND (d.tenant_id IS NULL OR d.tenant_id=?) ORDER BY d.tenant_id IS NULL,d.node_type,v.version_number")
        .bind(tenant).fetch_all(&state.pool).await?;
    let registry = NodeRegistry::m5_defaults();
    let mut result = Vec::new();
    for manifest in registry.studio_manifests() {
        let value = serde_json::to_value(manifest).map_err(ApiError::internal)?;
        let hash = agentx_domain::canonical_content_hash(&value).map_err(ApiError::internal)?;
        result.push((manifest.clone(), hash));
    }
    for row in rows {
        let source: String = row.try_get("source_type")?;
        if source == "registry" {
            validate_registry_snapshot(
                &registry,
                &source,
                row.try_get("node_type")?,
                row.try_get("version_number")?,
                row.try_get("manifest_json")?,
                row.try_get("manifest_hash")?,
            )?;
        }
    }
    for manifest in crate::canvas_plugin_api::plugin_manifests_for_tenant(state, tenant).await? {
        let value = serde_json::to_value(&manifest).map_err(ApiError::internal)?;
        let hash = agentx_domain::canonical_content_hash(&value).map_err(ApiError::internal)?;
        result.push((manifest, hash));
    }
    Ok(result)
}

fn validate_registry_snapshot(
    registry: &NodeRegistry,
    source: &str,
    node_type: &str,
    version: u32,
    value: Value,
    stored_hash: &str,
) -> ApiResult<()> {
    let manifest: NodeManifestVersion =
        serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
    let actual_hash = agentx_domain::canonical_content_hash(&value).map_err(ApiError::internal)?;
    let expected = registry.get(node_type, version).ok_or_else(|| {
        ApiError::unprocessable(
            "NODE_MANIFEST_SNAPSHOT_DRIFT",
            format!("Stored Manifest {node_type}@{version} has no Registry source"),
        )
    })?;
    let expected_hash = agentx_domain::canonical_content_hash(
        &serde_json::to_value(expected).map_err(ApiError::internal)?,
    )
    .map_err(ApiError::internal)?;
    if source != "registry"
        || manifest.node_type != node_type
        || manifest.version != version
        || stored_hash != actual_hash
        || stored_hash != expected_hash
    {
        return Err(ApiError::unprocessable(
            "NODE_MANIFEST_SNAPSHOT_DRIFT",
            format!(
                "Stored Manifest {node_type}@{version} does not match its Registry source and hash"
            ),
        ));
    }
    Ok(())
}

fn summary(manifest: NodeManifestVersion, manifest_hash: String) -> NodeDefinitionSummary {
    NodeDefinitionSummary {
        node_type: manifest.node_type,
        version: manifest.version,
        display_name: manifest.display_name,
        description: manifest.description,
        category: manifest.category,
        keywords: manifest.keywords,
        icon_key: manifest.icon_key,
        capability: manifest.capability.as_str().into(),
        execution_style: serde_json::to_value(manifest.execution_style)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_default(),
        manifest_hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_registry_snapshots_cannot_override_content_or_source() {
        let registry = NodeRegistry::m5_defaults();
        let value = serde_json::to_value(registry.get("set", 1).unwrap()).unwrap();
        let hash = agentx_domain::canonical_content_hash(&value).unwrap();
        assert!(
            validate_registry_snapshot(&registry, "registry", "set", 1, value.clone(), &hash)
                .is_ok()
        );
        let mut changed = value.clone();
        changed["displayName"] = Value::String("tampered".into());
        assert!(
            validate_registry_snapshot(&registry, "registry", "set", 1, changed, &hash).is_err()
        );
        assert!(
            validate_registry_snapshot(&registry, "remote", "set", 1, value.clone(), &hash)
                .is_err()
        );
        assert!(validate_registry_snapshot(&registry, "registry", "set", 2, value, &hash).is_err());
    }
}
