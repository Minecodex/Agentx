//! Shared discovery for authoring, immutable test contracts and scheduled/testing runs.
use agentx_domain::WorkflowDefinition;
use agentx_runtime_contracts::{WaitInputNodeV1, WaitInputsV1, node_path};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/workflows/{id}/wait-input-nodes",
            post(resolve_draft),
        )
        .route(
            "/api/v1/applications/{id}/wait-input-nodes",
            get(resolve_application),
        )
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResolveRequest {
    definition: WorkflowDefinition,
    included_node_ids: Option<Vec<String>>,
}

async fn resolve_draft(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(mut input): Json<ResolveRequest>,
) -> ApiResult<Json<Vec<WaitInputNodeV1>>> {
    actor.require("workflow:view")?;
    crate::workflow_api::require_workflow_access(&state, &actor, id, false).await?;
    if let Some(ids) = input.included_node_ids {
        input.definition.nodes.retain(|node| ids.contains(&node.id));
    }
    Ok(Json(
        discover(&state.pool, actor.tenant_id, &input.definition).await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplicationVersion {
    workflow_version_id: Option<Uuid>,
}

async fn resolve_application(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(version): Query<ApplicationVersion>,
) -> ApiResult<Json<Vec<WaitInputNodeV1>>> {
    let workflow =
        crate::application_testing::authorize(&state, &actor, id, "application:view").await?;
    let selected = match version.workflow_version_id {
        Some(version) => Some(version),
        None => sqlx::query_scalar("SELECT d.workflow_version_id FROM application_deployment_heads h JOIN application_deployments d ON d.tenant_id=h.tenant_id AND d.id=h.deployment_id WHERE h.tenant_id=? AND h.application_id=?").bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?,
    };
    let definition: Option<serde_json::Value>=sqlx::query_scalar("SELECT definition_json FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND publication_status='published' AND (? IS NULL OR id=?) ORDER BY version_number DESC LIMIT 1")
        .bind(actor.tenant_id).bind(workflow).bind(selected).bind(selected).fetch_optional(&state.pool).await?;
    match definition {
        Some(definition) => Ok(Json(
            discover(
                &state.pool,
                actor.tenant_id,
                &serde_json::from_value(definition).map_err(ApiError::internal)?,
            )
            .await?,
        )),
        None => Ok(Json(Vec::new())),
    }
}

pub(crate) async fn discover(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    definition: &WorkflowDefinition,
) -> ApiResult<Vec<WaitInputNodeV1>> {
    let mut pending = vec![(
        definition.clone(),
        String::new(),
        Vec::<Uuid>::new(),
        String::new(),
    )];
    let mut result = Vec::new();
    while let Some((definition, parent, ancestors, label)) = pending.pop() {
        for node in definition.nodes.iter().filter(|node| !node.disabled) {
            let path = node_path(&parent, &node.id);
            let name = if label.is_empty() {
                node.name.clone()
            } else {
                format!("{label} / {}", node.name)
            };
            if node.node_type == "await_input" {
                result.push(WaitInputNodeV1 {
                    path,
                    node_id: node.id.clone(),
                    name,
                });
            } else if node.node_type == "sub_workflow" {
                let Some(id) = node
                    .parameters
                    .get("workflowVersionId")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| Uuid::parse_str(id).ok())
                else {
                    continue;
                };
                if ancestors.contains(&id) {
                    return Err(ApiError::unprocessable(
                        "COMPOSITE_CYCLE",
                        "Sub-workflow dependency cycle",
                    ));
                }
                let row=sqlx::query("SELECT definition_json FROM workflow_versions WHERE tenant_id=? AND id=? AND publication_status='published'")
                    .bind(tenant).bind(id).fetch_optional(pool).await?.ok_or_else(||ApiError::unprocessable("COMPOSITE_VERSION_UNAVAILABLE","A fixed sub-workflow version is unavailable"))?;
                let child = serde_json::from_value(row.try_get("definition_json")?)
                    .map_err(ApiError::internal)?;
                let mut chain = ancestors.clone();
                chain.push(id);
                pending.push((child, path, chain, name));
            }
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

pub(crate) fn validate_plan(nodes: &[WaitInputNodeV1], plan: &WaitInputsV1) -> ApiResult<()> {
    if plan
        .keys()
        .any(|path| !nodes.iter().any(|node| &node.path == path))
    {
        return Err(ApiError::unprocessable(
            "WAIT_INPUT_NODE_UNKNOWN",
            "A preset reply targets a node outside this workflow version or debug scope",
        ));
    }
    Ok(())
}
