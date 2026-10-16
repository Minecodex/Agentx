use agentx_domain::{EditorDocument, WorkflowDefinition, canonical_content_hash};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    workflow_api::{
        DraftResponse, WorkflowVersionResponse, load_draft, require_workflow_access,
        version_from_row,
    },
};

pub(crate) fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/workflows/{id}/drafts", get(list).post(create))
        .route(
            "/api/v1/workflows/{id}/drafts/{draft_id}/metadata",
            axum::routing::patch(update),
        )
        .route("/api/v1/workflows/{id}/versions/{version_id}", get(version))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateDraft {
    name: String,
    base_version_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateDraft {
    name: String,
    status: String,
    expected_revision: u64,
}

async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow): Path<Uuid>,
) -> ApiResult<Json<Vec<DraftResponse>>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, workflow, false).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow_drafts WHERE tenant_id=? AND workflow_id=? ORDER BY created_at,id",
    )
    .bind(actor.tenant_id)
    .bind(workflow)
    .fetch_all(&state.pool)
    .await?;
    let mut drafts = Vec::with_capacity(ids.len());
    for id in ids {
        drafts.push(load_draft(&state, actor.tenant_id, workflow, id).await?);
    }
    Ok(Json(drafts))
}

async fn create(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow): Path<Uuid>,
    Json(input): Json<CreateDraft>,
) -> ApiResult<(StatusCode, Json<DraftResponse>)> {
    actor.require("workflow:edit")?;
    require_workflow_access(&state, &actor, workflow, true).await?;
    let name = crate::control_helpers::required_name(&input.name)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "SELECT id FROM workflows WHERE tenant_id=? AND id=? AND status='active' FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(workflow)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Active workflow"))?;
    let (definition, editor) = if let Some(base) = input.base_version_id {
        let row = sqlx::query("SELECT definition_json,editor_json FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND id=?")
            .bind(actor.tenant_id).bind(workflow).bind(base).fetch_optional(&mut *tx).await?.ok_or_else(|| ApiError::not_found("Workflow version"))?;
        (
            row.try_get::<Value, _>("definition_json")?,
            row.try_get::<Value, _>("editor_json")?,
        )
    } else {
        (
            serde_json::to_value(WorkflowDefinition::empty()).map_err(ApiError::internal)?,
            serde_json::to_value(EditorDocument::default()).map_err(ApiError::internal)?,
        )
    };
    let draft = Uuid::now_v7();
    sqlx::query("INSERT INTO workflow_drafts(id,tenant_id,workflow_id,name,base_version_id,created_by,schema_version,revision,definition_json,editor_json,content_hash,editor_hash,updated_by) VALUES(?,?,?,?,?,?,'8.0',0,?,?,?,?,?)")
        .bind(draft).bind(actor.tenant_id).bind(workflow).bind(name).bind(input.base_version_id).bind(actor.user_id).bind(&definition).bind(&editor)
        .bind(canonical_content_hash(&definition).map_err(ApiError::internal)?).bind(canonical_content_hash(&editor).map_err(ApiError::internal)?).bind(actor.user_id).execute(&mut *tx).await?;
    record_initial_revision(&mut tx, &actor, workflow, draft).await?;
    if let Some(base) = input.base_version_id {
        let resources = sqlx::query("SELECT node_id,resource_type,resource_id,resource_version_id,operation_key,snapshot_json,snapshot_hash FROM workflow_version_resources WHERE tenant_id=? AND workflow_version_id=? ORDER BY node_id,resource_type,resource_id")
            .bind(actor.tenant_id).bind(base).fetch_all(&mut *tx).await?;
        let snapshots = resources
            .into_iter()
            .map(crate::workflow_revision_resources::snapshot_from_row)
            .collect::<ApiResult<Vec<_>>>()?;
        sqlx::query("UPDATE workflow_draft_revisions SET resource_snapshot_json=? WHERE tenant_id=? AND draft_id=? AND revision=0").bind(serde_json::to_value(snapshots).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(draft).execute(&mut *tx).await?;
    }
    let parsed = serde_json::from_value(definition).map_err(ApiError::internal)?;
    crate::workflow_resources::replace_draft_resources(
        &mut tx,
        actor.tenant_id,
        workflow,
        draft,
        &parsed,
    )
    .await?;
    crate::workflow_api::audit(
        &mut tx,
        &actor,
        "workflow.draft_created",
        "workflow",
        workflow,
        json!({"draftId":draft,"baseVersionId":input.base_version_id}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_draft(&state, actor.tenant_id, workflow, draft).await?),
    ))
}

async fn update(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((workflow, draft)): Path<(Uuid, Uuid)>,
    Json(input): Json<UpdateDraft>,
) -> ApiResult<Json<DraftResponse>> {
    actor.require("workflow:edit")?;
    require_workflow_access(&state, &actor, workflow, true).await?;
    if !matches!(input.status.as_str(), "active" | "archived") {
        return Err(ApiError::bad_request(
            "INVALID_DRAFT_STATUS",
            "Invalid draft status",
        ));
    }
    let name = crate::control_helpers::required_name(&input.name)?;
    let changed = sqlx::query("UPDATE workflow_drafts SET name=?,status=? WHERE tenant_id=? AND workflow_id=? AND id=? AND revision=?")
        .bind(name).bind(input.status).bind(actor.tenant_id).bind(workflow).bind(draft).bind(input.expected_revision).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "DRAFT_REVISION_CONFLICT",
            "Draft changed; reload before editing its metadata",
        ));
    }
    Ok(Json(
        load_draft(&state, actor.tenant_id, workflow, draft).await?,
    ))
}

async fn version(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((workflow, version)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<WorkflowVersionResponse>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, workflow, false).await?;
    let row = sqlx::query("SELECT id,workflow_id,version_number,source_draft_id,source_revision_id,base_version_id,source_revision,schema_version,content_hash,definition_json,node_manifests_json,editor_json,created_by,created_at FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND id=?")
        .bind(actor.tenant_id).bind(workflow).bind(version).fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::not_found("Workflow version"))?;
    Ok(Json(version_from_row(row)?))
}

pub(crate) async fn record_initial_revision(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    workflow: Uuid,
    draft: Uuid,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO workflow_draft_revisions(id,tenant_id,workflow_id,draft_id,revision,schema_version,definition_json,editor_json,content_hash,editor_hash,created_by) SELECT ?,tenant_id,workflow_id,id,0,schema_version,definition_json,editor_json,content_hash,editor_hash,? FROM workflow_drafts WHERE tenant_id=? AND workflow_id=? AND id=?")
        .bind(Uuid::now_v7()).bind(actor.user_id).bind(actor.tenant_id).bind(workflow).bind(draft).execute(&mut **tx).await?;
    Ok(())
}
