use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{LazyLock, Mutex},
    time::Instant,
};

use agentx_domain::WorkflowDefinition;
use agentx_node_protocol::{NodeCapability, NodeManifestVersion, NodePort, PluginNodeBinding};
use agentx_runtime::{CompileIssue, NodeRegistry};
use agentx_runtime_contracts::{
    ControlRole, PluginDesignOperationRequestV1, PluginDesignOperationResponseV1,
    PluginResolveSessionV1, ServiceClaimsV1, issue_service_token, now_unix,
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::MySqlPool;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::ControlApiState,
};

static DESIGN_CACHE: LazyLock<Mutex<HashMap<String, (Instant, Value)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct ResolveSession {
    id: Uuid,
    started: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ResolvedWorkflowPlugins {
    pub definitions: BTreeMap<String, Value>,
    pub manifests: BTreeMap<String, NodeManifestVersion>,
    pub dependency_manifests: agentx_bundle_builder::ResolvedPluginManifestDependencies,
    pub incomplete: Vec<CompileIssue>,
    pub invalid: Vec<CompileIssue>,
}

pub(crate) async fn frozen_for_version(
    pool: &MySqlPool,
    tenant: Uuid,
    version: Uuid,
    dependencies: &BTreeMap<Uuid, WorkflowDefinition>,
) -> ApiResult<ResolvedWorkflowPlugins> {
    let root: Value = sqlx::query_scalar(
        "SELECT node_manifests_json FROM workflow_versions WHERE tenant_id=? AND id=?",
    )
    .bind(tenant)
    .bind(version)
    .fetch_one(pool)
    .await?;
    let mut frozen = ResolvedWorkflowPlugins {
        manifests: serde_json::from_value(root).map_err(ApiError::internal)?,
        ..Default::default()
    };
    for id in dependencies.keys() {
        let value: Value = sqlx::query_scalar(
            "SELECT node_manifests_json FROM workflow_versions WHERE tenant_id=? AND id=?",
        )
        .bind(tenant)
        .bind(id)
        .fetch_one(pool)
        .await?;
        frozen.dependency_manifests.insert(
            *id,
            serde_json::from_value(value).map_err(ApiError::internal)?,
        );
    }
    Ok(frozen)
}

pub(crate) fn resolved_runtime_plugin_bindings(
    resolved: &ResolvedWorkflowPlugins,
) -> Vec<PluginNodeBinding> {
    let mut bindings = BTreeMap::new();
    for manifest in resolved.manifests.values().chain(
        resolved
            .dependency_manifests
            .values()
            .flat_map(|manifests| manifests.values()),
    ) {
        if let Some(binding) = manifest.plugin.as_ref()
            && let Some(artifact) = binding.runtime_artifact.as_ref()
        {
            bindings
                .entry((artifact.object_id, artifact.content_hash.clone()))
                .or_insert_with(|| binding.clone());
        }
    }
    bindings.into_values().collect()
}

pub(crate) async fn resolve_workflow_plugin_closure(
    state: &ControlApiState,
    tenant: Uuid,
    definition: &WorkflowDefinition,
    dependencies: &BTreeMap<Uuid, WorkflowDefinition>,
    registry: &NodeRegistry,
) -> ApiResult<ResolvedWorkflowPlugins> {
    let mut result = resolve_workflow_plugins(state, tenant, definition, registry).await?;
    for (version_id, dependency) in dependencies {
        let mut resolved = resolve_workflow_plugins(state, tenant, dependency, registry).await?;
        for issue in resolved
            .incomplete
            .iter_mut()
            .chain(resolved.invalid.iter_mut())
        {
            issue.path = format!("dependencies.{version_id}.{}", issue.path);
        }
        result.incomplete.append(&mut resolved.incomplete);
        result.invalid.append(&mut resolved.invalid);
        result
            .dependency_manifests
            .insert(*version_id, resolved.manifests);
    }
    Ok(result)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedDefinitionValue {
    status: String,
    #[serde(default)]
    input_ports: Option<Vec<NodePort>>,
    #[serde(default)]
    output_ports: Option<Vec<NodePort>>,
    #[serde(default)]
    output_schema: Option<Value>,
    #[serde(default)]
    output_port_schemas: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    issues: Vec<ResolvedDefinitionIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedDefinitionIssue {
    path: String,
    code: String,
    message: String,
}

pub(crate) async fn resolve_workflow_plugins(
    state: &ControlApiState,
    tenant: Uuid,
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> ApiResult<ResolvedWorkflowPlugins> {
    let mut session = ResolveSession {
        id: Uuid::now_v7(),
        started: false,
    };
    let result =
        resolve_workflow_plugins_in_session(state, tenant, definition, registry, &mut session)
            .await;
    let closed = if session.started {
        close_resolve_session(state, tenant, session.id).await
    } else {
        Ok(())
    };
    match result {
        Err(error) => Err(error),
        Ok(result) => closed.map(|()| result),
    }
}

async fn resolve_workflow_plugins_in_session(
    state: &ControlApiState,
    tenant: Uuid,
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
    session: &mut ResolveSession,
) -> ApiResult<ResolvedWorkflowPlugins> {
    let mut result = ResolvedWorkflowPlugins::default();
    for index in workflow_resolution_order(definition) {
        let node = &definition.nodes[index];
        if node.disabled || node.node_type == agentx_domain::WORKFLOW_EXIT_NODE_TYPE {
            continue;
        }
        let Some(base) = registry.resolve_definition_manifest(
            &node.node_type,
            node.type_version,
            &node.parameters,
        ) else {
            continue;
        };
        let Some(binding) = base.plugin.as_ref() else {
            continue;
        };
        if base.capability != NodeCapability::PluginNodejs || binding.runtime_source.is_empty() {
            continue;
        }
        let upstream_contracts =
            workflow_upstream_contracts(definition, node, registry, &result.manifests);
        let raw = invoke_plugin_method_in_session(
            state,
            tenant,
            binding,
            "node.resolveDefinition",
            json!({
                "configuration":node.parameters,
                "nodeType":node.node_type,
                "upstreamContracts":upstream_contracts,
                "inputPorts":base.input_ports,
                "outputPorts":base.output_ports,
                "outputSchema":base.output_schema,
                "outputPortSchemas":base.output_port_schemas,
                "hostResourceReferences":node.resource_references,
            }),
            Some(session),
        )
        .await?;
        result.definitions.insert(node.id.clone(), raw.clone());
        let resolved: ResolvedDefinitionValue = serde_json::from_value(raw).map_err(|error| {
            ApiError::unprocessable("PLUGIN_RESOLVED_DEFINITION_INVALID", error.to_string())
        })?;
        let mut candidate = base.clone();
        if let Some(value) = resolved.input_ports {
            candidate.input_ports = value;
        }
        if let Some(value) = resolved.output_ports {
            candidate.output_ports = value;
        }
        if let Some(value) = resolved.output_schema {
            candidate.output_schema = value;
        }
        if let Some(value) = resolved.output_port_schemas {
            candidate.output_port_schemas = value;
        }
        let mut verifier = NodeRegistry::default();
        if let Err(error) = verifier.register(candidate.clone()) {
            result.definitions.insert(node.id.clone(), json!({"status":"invalid","issues":[{
                "path":"", "code":"PLUGIN_RESOLVED_DEFINITION_INVALID", "message":error.to_string()
            }]}));
            result.invalid.push(CompileIssue {
                code: "PLUGIN_RESOLVED_DEFINITION_INVALID".into(),
                path: format!("nodes[{index}].typeVersion"),
                message: error.to_string(),
            });
            continue;
        }
        let issues = resolved
            .issues
            .into_iter()
            .map(|issue| CompileIssue {
                code: issue.code,
                path: if issue.path.is_empty() {
                    format!("nodes[{index}].parameters")
                } else {
                    format!("nodes[{index}].parameters.{}", issue.path)
                },
                message: issue.message,
            })
            .collect::<Vec<_>>();
        match resolved.status.as_str() {
            "complete" => {}
            "incomplete" => {
                if issues.is_empty() {
                    result.incomplete.push(CompileIssue {
                        code: "PLUGIN_DEFINITION_INCOMPLETE".into(),
                        path: format!("nodes[{index}].parameters"),
                        message: "Plugin configuration is incomplete".into(),
                    });
                } else {
                    result.incomplete.extend(issues);
                }
            }
            "invalid" => {
                if issues.is_empty() {
                    result.invalid.push(CompileIssue {
                        code: "PLUGIN_RESOLVED_DEFINITION_INVALID".into(),
                        path: format!("nodes[{index}].parameters"),
                        message: "Plugin rejected its configuration".into(),
                    });
                } else {
                    result.invalid.extend(issues);
                }
            }
            status => result.invalid.push(CompileIssue {
                code: "PLUGIN_RESOLVED_DEFINITION_INVALID".into(),
                path: format!("nodes[{index}].typeVersion"),
                message: format!("Plugin returned unsupported definition status '{status}'"),
            }),
        }
        result.manifests.insert(node.id.clone(), candidate);
    }
    Ok(result)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResolveDraftPluginsRequest {
    definition: WorkflowDefinition,
}

pub(crate) async fn resolve_draft_plugins(
    axum::extract::State(state): axum::extract::State<ControlApiState>,
    actor: crate::control_api::Actor,
    axum::extract::Path((id, draft_id)): axum::extract::Path<(Uuid, Uuid)>,
    axum::Json(input): axum::Json<ResolveDraftPluginsRequest>,
) -> ApiResult<axum::Json<Value>> {
    actor.require("workflow:view")?;
    crate::workflow_api::require_workflow_access(&state, &actor, id, false).await?;
    crate::workflow_api::load_draft(&state, actor.tenant_id, id, draft_id).await?;
    let dependencies = crate::workflow_api::load_composite_definitions_for_actor(
        &state,
        &actor,
        id,
        &input.definition,
    )
    .await?;
    let registry =
        crate::canvas_plugin_api::registry_for_tenant(&state, actor.tenant_id, &dependencies, true)
            .await?;
    let resolved =
        resolve_workflow_plugins(&state, actor.tenant_id, &input.definition, &registry).await?;
    Ok(axum::Json(json!({"nodes":resolved.definitions})))
}

fn workflow_resolution_order(definition: &WorkflowDefinition) -> Vec<usize> {
    let positions = definition
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut indegree = vec![0_usize; definition.nodes.len()];
    let mut outgoing = vec![Vec::new(); definition.nodes.len()];
    for connection in &definition.connections {
        let (Some(&source), Some(&target)) = (
            positions.get(connection.source_node_id.as_str()),
            positions.get(connection.target_node_id.as_str()),
        ) else {
            continue;
        };
        if source != target {
            indegree[target] += 1;
            outgoing[source].push(target);
        }
    }
    let mut ready = indegree
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (*value == 0).then_some(index))
        .collect::<std::collections::VecDeque<_>>();
    let mut order = Vec::with_capacity(definition.nodes.len());
    while let Some(index) = ready.pop_front() {
        order.push(index);
        for target in &outgoing[index] {
            indegree[*target] -= 1;
            if indegree[*target] == 0 {
                ready.push_back(*target);
            }
        }
    }
    for index in 0..definition.nodes.len() {
        if !order.contains(&index) {
            order.push(index);
        }
    }
    order
}

pub(crate) async fn lock_resolved_plugin_versions(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    manifests: &BTreeMap<String, NodeManifestVersion>,
) -> ApiResult<()> {
    let identities = manifests
        .values()
        .filter_map(|manifest| {
            manifest.plugin.as_ref().and_then(|plugin| {
                (!plugin.package_id.starts_with("agentx/")).then(|| {
                    (
                        manifest.node_type.clone(),
                        manifest.version,
                        plugin.bundle_digest.clone(),
                    )
                })
            })
        })
        .collect::<BTreeSet<_>>();
    for (node_type, version, digest) in identities {
        let locked = sqlx::query_scalar::<_, Uuid>(
            "SELECT pv.id FROM canvas_plugin_versions pv JOIN node_definition_versions nv ON nv.plugin_version_id=pv.id JOIN node_definitions d ON d.id=nv.node_definition_id WHERE pv.tenant_id=? AND d.tenant_id=? AND d.node_type=? AND nv.version_number=? AND pv.bundle_digest=? FOR SHARE",
        )
        .bind(tenant)
        .bind(tenant)
        .bind(node_type)
        .bind(version)
        .bind(digest)
        .fetch_optional(&mut **tx)
        .await?;
        if locked.is_none() {
            return Err(ApiError::conflict(
                "PLUGIN_VERSION_CHANGED",
                "A Canvas Plugin version changed while the Workflow was being saved",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn lock_resolved_plugin_closure(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    resolved: &ResolvedWorkflowPlugins,
) -> ApiResult<()> {
    lock_resolved_plugin_versions(tx, tenant, &resolved.manifests).await?;
    for manifests in resolved.dependency_manifests.values() {
        lock_resolved_plugin_versions(tx, tenant, manifests).await?;
    }
    Ok(())
}

fn workflow_upstream_contracts(
    definition: &WorkflowDefinition,
    target: &agentx_domain::WorkflowNode,
    registry: &NodeRegistry,
    resolved: &BTreeMap<String, NodeManifestVersion>,
) -> Value {
    let mut contracts = serde_json::Map::new();
    contracts.insert("$inputs".into(), definition.start.inputs.clone());
    contracts.insert(
        "$contexts".into(),
        serde_json::to_value(&definition.start.contexts).unwrap_or_else(|_| json!({})),
    );
    let mut node_contracts = serde_json::Map::new();
    for node in definition.nodes.iter().filter(|node| !node.disabled) {
        let Some(manifest) = resolved.get(&node.id).or_else(|| {
            registry.resolve_definition_manifest(
                &node.node_type,
                node.type_version,
                &node.parameters,
            )
        }) else {
            continue;
        };
        let ports = manifest
            .output_ports
            .iter()
            .map(|port| {
                (
                    port.name.clone(),
                    manifest
                        .output_port_schemas
                        .get(&port.name)
                        .cloned()
                        .unwrap_or_else(|| manifest.output_schema.clone()),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        node_contracts.insert(node.id.clone(), json!({"ports":ports}));
    }
    contracts.insert("$nodes".into(), Value::Object(node_contracts));
    for connection in definition
        .connections
        .iter()
        .filter(|connection| connection.target_node_id == target.id)
    {
        let schema = if connection.source_node_id == agentx_domain::WORKFLOW_START_NODE_ID {
            definition.start.inputs.clone()
        } else if let Some(source) = definition
            .nodes
            .iter()
            .find(|node| node.id == connection.source_node_id)
        {
            resolved
                .get(&source.id)
                .or_else(|| {
                    registry.resolve_definition_manifest(
                        &source.node_type,
                        source.type_version,
                        &source.parameters,
                    )
                })
                .map(|manifest| {
                    manifest
                        .output_port_schemas
                        .get(&connection.source_handle)
                        .cloned()
                        .unwrap_or_else(|| manifest.output_schema.clone())
                })
                .unwrap_or_else(|| json!({}))
        } else {
            json!({})
        };
        contracts.insert(
            connection.target_handle.clone(),
            json!({
                "sourceNodeId":connection.source_node_id,
                "sourcePort":connection.source_handle,
                "schema":schema,
            }),
        );
    }
    Value::Object(contracts)
}

pub(crate) async fn invoke_plugin_method(
    state: &ControlApiState,
    tenant_id: Uuid,
    binding: &PluginNodeBinding,
    method: &str,
    params: Value,
) -> ApiResult<Value> {
    invoke_plugin_method_in_session(state, tenant_id, binding, method, params, None).await
}

async fn invoke_plugin_method_in_session(
    state: &ControlApiState,
    tenant_id: Uuid,
    binding: &PluginNodeBinding,
    method: &str,
    mut params: Value,
    session: Option<&mut ResolveSession>,
) -> ApiResult<Value> {
    let cache_key = if method == "node.resolveDefinition" {
        let hash = agentx_domain::canonical_content_hash(&params).map_err(ApiError::internal)?;
        Some(format!(
            "{tenant_id}:{}:{method}:{hash}",
            binding.bundle_digest
        ))
    } else {
        None
    };
    if let Some(key) = cache_key.as_ref() {
        let mut cache = DESIGN_CACHE.lock().expect("plugin design cache lock");
        cache.retain(|_, (created, _)| created.elapsed() < std::time::Duration::from_secs(300));
        if let Some((_, value)) = cache.get(key) {
            return Ok(value.clone());
        }
    }
    let resolve_session_id = session.map(|session| {
        session.started = true;
        session.id
    });
    let host_resource_references = params
        .as_object_mut()
        .and_then(|params| params.remove("hostResourceReferences"))
        .unwrap_or_else(|| json!([]));
    let references: Vec<agentx_domain::ResourceReference> =
        serde_json::from_value(host_resource_references).map_err(|error| {
            ApiError::unprocessable("PLUGIN_DESIGN_RESOURCE_INVALID", error.to_string())
        })?;
    let mut resources = Vec::with_capacity(references.len());
    for mut reference in if method == "node.resolveDefinition" {
        Vec::new()
    } else {
        references
    } {
        let snapshot =
            crate::workflow_resources::resource_snapshot(state, tenant_id, &mut reference).await?;
        let snapshot_hash = runtime_snapshot_hash(&snapshot)?;
        let snapshot = agentx_domain::ResourceVersionSnapshot {
            node_id: "plugin-design".into(),
            reference,
            snapshot,
            snapshot_hash,
        };
        resources.push(
            crate::runtime_resource_binding::from_snapshot(&snapshot).map_err(|error| {
                ApiError::unprocessable("PLUGIN_DESIGN_RESOURCE_INVALID", error.to_string())
            })?,
        );
    }
    let now = now_unix();
    let token = issue_service_token(
        &state.runtime_command_kid,
        state.runtime_command_key.expose_secret().as_bytes(),
        &ServiceClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: "platform-control-plugin-design".into(),
            role: ControlRole::Publisher,
            scope: BTreeSet::from(["runtime.plugins.design".into()]),
            iat: now,
            exp: now + 30,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)?;
    let request = PluginDesignOperationRequestV1 {
        protocol_version: 1,
        operation_id: Uuid::now_v7(),
        tenant_id,
        resolve_session_id,
        method: method.into(),
        plugin: binding.clone(),
        resources,
        parameters: params,
    };
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/plugin-design-operations:execute",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(
            if method == "node.resolveDefinition" {
                55
            } else {
                15
            },
        ))
        .json(&request)
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Runtime plugin design operation is unavailable");
            ApiError::unavailable(
                "RUNTIME_UNAVAILABLE",
                "Runtime plugin design operation is unavailable",
            )
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(%status, response_body=%body, "Runtime rejected plugin design operation");
        return Err(ApiError::unprocessable(
            "PLUGIN_DESIGN_OPERATION_FAILED",
            "Runtime rejected plugin design operation",
        ));
    }
    let response: PluginDesignOperationResponseV1 =
        response.json().await.map_err(ApiError::internal)?;
    if response.operation_id != request.operation_id {
        return Err(ApiError::unprocessable(
            "PLUGIN_PROTOCOL_ERROR",
            "Runtime plugin design operation ID mismatch",
        ));
    }
    if let Some(key) = cache_key {
        let mut cache = DESIGN_CACHE.lock().expect("plugin design cache lock");
        if cache.len() >= 512
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (created, _))| *created)
                .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
        cache.insert(key, (Instant::now(), response.result.clone()));
    }
    Ok(response.result)
}

async fn close_resolve_session(
    state: &ControlApiState,
    tenant_id: Uuid,
    session_id: Uuid,
) -> ApiResult<()> {
    let now = now_unix();
    let token = issue_service_token(
        &state.runtime_command_kid,
        state.runtime_command_key.expose_secret().as_bytes(),
        &ServiceClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: "platform-control-plugin-design".into(),
            role: ControlRole::Publisher,
            scope: BTreeSet::from(["runtime.plugins.design".into()]),
            iat: now,
            exp: now + 30,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/plugin-resolve-sessions:close",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(15))
        .json(&PluginResolveSessionV1 {
            protocol_version: 1,
            tenant_id,
            session_id,
        })
        .send()
        .await
        .map_err(|_| {
            ApiError::unavailable(
                "RUNTIME_UNAVAILABLE",
                "Resolve-only session cleanup is unavailable",
            )
        })?;
    if !response.status().is_success() {
        return Err(ApiError::unavailable(
            "RUNTIME_UNAVAILABLE",
            "Resolve-only session cleanup is unavailable",
        ));
    }
    let closed: PluginResolveSessionV1 = response.json().await.map_err(ApiError::internal)?;
    if closed.tenant_id != tenant_id || closed.session_id != session_id {
        return Err(ApiError::unavailable(
            "PLUGIN_PROTOCOL_ERROR",
            "Resolve-only session cleanup identity mismatch",
        ));
    }
    Ok(())
}

fn runtime_snapshot_hash(snapshot: &Value) -> ApiResult<String> {
    agentx_runtime_contracts::content_hash(snapshot)
        .map(|hash| hash.to_string())
        .map_err(ApiError::internal)
}

#[cfg(test)]
mod tests {
    use super::runtime_snapshot_hash;
    use serde_json::json;

    #[test]
    fn design_resources_use_runtime_content_hash_contract() {
        let hash = runtime_snapshot_hash(&json!({"kind":"credential","version":1})).unwrap();
        assert!(agentx_runtime_contracts::ContentHash::parse(hash).is_ok());
    }
}
