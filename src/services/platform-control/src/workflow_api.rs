use std::collections::{BTreeMap, VecDeque};

use agentx_api_types::PageResponse;
use agentx_domain::{
    EditorDocument, WorkflowDefinition, canonical_content_hash, validate_definition,
    validate_editor_document,
};
use agentx_runtime::{CompileContext, WorkflowCompiler};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    control_helpers::required_name,
    workflow_resources::replace_draft_resources,
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/workflows",
            get(list_workflows).post(create_workflow),
        )
        .route(
            "/api/v1/workflows/{id}",
            get(get_workflow)
                .patch(update_workflow)
                .delete(delete_workflow),
        )
        .route(
            "/api/v1/workflows/{id}/drafts/{draft_id}",
            get(get_draft).put(save_draft),
        )
        .route(
            "/api/v1/workflows/{id}/drafts/{draft_id}/validate",
            post(validate_draft),
        )
        .route(
            "/api/v1/workflows/{id}/drafts/{draft_id}/resolve-plugins",
            post(crate::canvas_plugin_resolution::resolve_draft_plugins),
        )
        .route(
            "/api/v1/workflows/{id}/drafts/{draft_id}/debug-overlays/{node_id}",
            get(get_debug_overlay)
                .put(save_debug_overlay)
                .delete(delete_debug_overlay),
        )
        .route(
            "/api/v1/workflows/{id}/drafts/{draft_id}/revisions",
            get(list_revisions),
        )
        .route(
            "/api/v1/workflows/{id}/members",
            get(list_members).post(upsert_member),
        )
        .route(
            "/api/v1/workflows/{id}/member-candidates",
            get(member_candidates),
        )
        .route(
            "/api/v1/workflows/{id}/members/{user_id}",
            axum::routing::delete(delete_member),
        )
        .route(
            "/api/v1/workflows/{id}/deployments",
            get(list_deployments).post(crate::workflow_publishing::publish),
        )
        .route(
            "/api/v1/workflows/{id}/deployments/{environment_id}/rollback",
            post(crate::workflow_publishing::rollback),
        )
        .route(
            "/api/v1/environments",
            get(list_environments).post(create_environment),
        )
        .route(
            "/api/v1/environments/{id}",
            axum::routing::patch(update_environment).delete(delete_environment),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkflowListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    search: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowResponse {
    id: Uuid,
    name: String,
    description: Option<String>,
    status: String,
    visibility: String,
    owner_user_id: Uuid,
    owner_department_id: Uuid,
    service_identity_id: Uuid,
    owner_name: String,
    latest_version: Option<u64>,
    version: u64,
    can_edit: bool,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateWorkflowRequest {
    name: String,
    description: Option<String>,
    visibility: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateWorkflowRequest {
    name: String,
    description: Option<String>,
    visibility: String,
    version: u64,
}

#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftResponse {
    pub version_id: Uuid,
    pub version_number: u64,
    pub publication_status: String,
    pub archived: bool,
    pub generation: u64,
    pub base_version_id: Option<Uuid>,
    pub id: Uuid,
    pub workflow_id: Uuid,
    pub schema_version: String,
    pub revision: u64,
    pub definition: Value,
    pub editor_document: Value,
    pub definition_hash: String,
    pub editor_hash: String,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub updated_at: OffsetDateTime,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveDraftRequest {
    expected_revision: u64,
    definition: Value,
    editor_document: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RevisionResponse {
    id: Uuid,
    revision: u64,
    schema_version: String,
    definition_hash: String,
    editor_hash: String,
    created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowMemberResponse {
    user_id: Uuid,
    username: String,
    display_name: String,
    member_role: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpsertWorkflowMemberRequest {
    user_id: Uuid,
    member_role: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentResponse {
    id: Uuid,
    code: String,
    name: String,
    is_builtin: bool,
    status: String,
    version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateEnvironmentRequest {
    code: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateEnvironmentRequest {
    name: String,
    status: String,
    version: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeploymentResponse {
    id: Uuid,
    workflow_id: Uuid,
    environment_id: Uuid,
    environment_name: String,
    workflow_version_id: Uuid,
    version_number: u64,
    sequence_number: u64,
    status: String,
    source: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

async fn list_workflows(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<WorkflowListQuery>,
) -> ApiResult<Json<PageResponse<WorkflowResponse>>> {
    actor.require("workflow:view")?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let offset = u64::from((page - 1) * page_size);
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let status = query.status.unwrap_or_default();
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let (rows, total) = if administrator {
        let rows = sqlx::query("SELECT w.id,w.name,w.description,w.status,w.visibility,w.owner_user_id,w.owner_department_id,si.id service_identity_id,u.display_name owner_name,w.version,w.updated_at,(SELECT MAX(version_number) FROM workflow_versions v WHERE v.tenant_id=w.tenant_id AND v.workflow_id=w.id) latest_version FROM workflows w JOIN users u ON u.tenant_id=w.tenant_id AND u.id=w.owner_user_id JOIN workflow_service_identities si ON si.tenant_id=w.tenant_id AND si.workflow_id=w.id WHERE w.tenant_id=? AND (?='' OR w.status=?) AND (?='%%' OR w.name LIKE ?) ORDER BY w.updated_at DESC,w.id DESC LIMIT ? OFFSET ?")
            .bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(offset).fetch_all(&state.pool).await?;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows w WHERE w.tenant_id=? AND (?='' OR w.status=?) AND (?='%%' OR w.name LIKE ?)")
            .bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    } else {
        let rows = sqlx::query("SELECT w.id,w.name,w.description,w.status,w.visibility,w.owner_user_id,w.owner_department_id,si.id service_identity_id,u.display_name owner_name,w.version,w.updated_at,(SELECT MAX(version_number) FROM workflow_versions v WHERE v.tenant_id=w.tenant_id AND v.workflow_id=w.id) latest_version FROM workflows w JOIN users u ON u.tenant_id=w.tenant_id AND u.id=w.owner_user_id JOIN workflow_service_identities si ON si.tenant_id=w.tenant_id AND si.workflow_id=w.id WHERE w.tenant_id=? AND (w.owner_user_id=? OR w.visibility='company' OR EXISTS(SELECT 1 FROM workflow_members wm WHERE wm.tenant_id=w.tenant_id AND wm.workflow_id=w.id AND wm.user_id=?) OR (w.visibility='department' AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=w.tenant_id AND dc.ancestor_id=w.owner_department_id AND dc.descendant_id=?))) AND (?='' OR w.status=?) AND (?='%%' OR w.name LIKE ?) ORDER BY w.updated_at DESC,w.id DESC LIMIT ? OFFSET ?")
            .bind(actor.tenant_id).bind(actor.user_id).bind(actor.user_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(offset).fetch_all(&state.pool).await?;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows w WHERE w.tenant_id=? AND (w.owner_user_id=? OR w.visibility='company' OR EXISTS(SELECT 1 FROM workflow_members wm WHERE wm.tenant_id=w.tenant_id AND wm.workflow_id=w.id AND wm.user_id=?) OR (w.visibility='department' AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=w.tenant_id AND dc.ancestor_id=w.owner_department_id AND dc.descendant_id=?))) AND (?='' OR w.status=?) AND (?='%%' OR w.name LIKE ?)")
            .bind(actor.tenant_id).bind(actor.user_id).bind(actor.user_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    };
    Ok(Json(PageResponse {
        items: rows
            .into_iter()
            .map(|row| {
                let can_edit = actor
                    .permissions
                    .iter()
                    .any(|value| value == "workflow:edit")
                    && (administrator || row.try_get::<Uuid, _>("owner_user_id")? == actor.user_id);
                workflow_from_row(row, can_edit)
            })
            .collect::<Result<_, _>>()?,
        page,
        page_size,
        total: total.try_into().unwrap_or_default(),
    }))
}

async fn create_workflow(
    State(state): State<ControlApiState>,
    actor: Actor,
    headers: axum::http::HeaderMap,
    Json(input): Json<CreateWorkflowRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("workflow:create")?;
    validate_visibility(&input.visibility)?;
    let name = required_name(&input.name)?;
    let workflow_id = Uuid::now_v7();
    let identity_id = Uuid::now_v7();
    let draft_id = Uuid::now_v7();
    let definition =
        serde_json::to_value(WorkflowDefinition::empty()).map_err(ApiError::internal)?;
    let editor = serde_json::to_value(EditorDocument::default()).map_err(ApiError::internal)?;
    let definition_hash = canonical_content_hash(&definition).map_err(ApiError::internal)?;
    let editor_hash = canonical_content_hash(&editor).map_err(ApiError::internal)?;
    let request =
        json!({"name":input.name,"description":input.description,"visibility":input.visibility});
    let keyed = headers.contains_key("Idempotency-Key");
    let mut tx = state.pool.begin().await?;
    if keyed {
        sqlx::query("SELECT id FROM users WHERE tenant_id=? AND id=? FOR UPDATE")
            .bind(actor.tenant_id)
            .bind(actor.user_id)
            .fetch_one(&mut *tx)
            .await?;
        if let Some(result) =
            crate::workflow_versions::replay(&mut tx, &actor, "create-workflow", &headers, &request)
                .await?
        {
            tx.commit().await?;
            return Ok((StatusCode::CREATED, Json(result)));
        }
    }
    sqlx::query("INSERT INTO workflows(id,tenant_id,name,description,visibility,owner_user_id,owner_department_id) VALUES(?,?,?,?,?,?,?)")
        .bind(workflow_id).bind(actor.tenant_id).bind(&name).bind(&input.description).bind(&input.visibility).bind(actor.user_id).bind(actor.department_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO workflow_service_identities(id,tenant_id,workflow_id) VALUES(?,?,?)")
        .bind(identity_id)
        .bind(actor.tenant_id)
        .bind(workflow_id)
        .execute(&mut *tx)
        .await?;
    let admission_commands =
        crate::runtime_admission::emit_new_service_identity(&mut tx, actor.tenant_id, identity_id)
            .await?;
    sqlx::query("INSERT INTO workflow_members(tenant_id,workflow_id,user_id,member_role,created_by) VALUES(?,?,?,'manager',?)")
        .bind(actor.tenant_id).bind(workflow_id).bind(actor.user_id).bind(actor.user_id).execute(&mut *tx).await?;
    emit_workflow_query_grant(
        &mut tx,
        actor.tenant_id,
        workflow_id,
        actor.user_id,
        1,
        true,
    )
    .await?;
    sqlx::query("INSERT INTO workflow_drafts(id,tenant_id,workflow_id,schema_version,revision,definition_json,editor_json,content_hash,editor_hash,updated_by,created_by) VALUES(?,?,?,'8.0',0,?,?,?,?,?,?)")
        .bind(draft_id).bind(actor.tenant_id).bind(workflow_id).bind(&definition).bind(&editor).bind(&definition_hash).bind(&editor_hash).bind(actor.user_id).bind(actor.user_id).execute(&mut *tx).await?;
    crate::workflow_drafts::record_initial_revision(&mut tx, &actor, workflow_id, draft_id).await?;
    audit(
        &mut tx,
        &actor,
        "workflow.created",
        "workflow",
        workflow_id,
        json!({"name":name}),
    )
    .await?;
    let version_id =
        crate::workflow_versions::insert_unpublished(&mut tx, &actor, workflow_id, draft_id)
            .await?;
    let row=sqlx::query("SELECT w.id,w.name,w.description,w.status,w.visibility,w.owner_user_id,w.owner_department_id,si.id service_identity_id,u.display_name owner_name,w.version,w.updated_at,(SELECT MAX(version_number) FROM workflow_versions v WHERE v.tenant_id=w.tenant_id AND v.workflow_id=w.id) latest_version FROM workflows w JOIN users u ON u.tenant_id=w.tenant_id AND u.id=w.owner_user_id JOIN workflow_service_identities si ON si.tenant_id=w.tenant_id AND si.workflow_id=w.id WHERE w.tenant_id=? AND w.id=?").bind(actor.tenant_id).bind(workflow_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Workflow"))?;
    let mut result = serde_json::to_value(workflow_from_row(
        row,
        actor.permissions.iter().any(|p| p == "workflow:edit"),
    )?)
    .map_err(ApiError::internal)?;
    result["createdVersionId"] = json!(version_id);
    if keyed {
        crate::workflow_versions::receipt(
            &mut tx,
            &actor,
            "create-workflow",
            &headers,
            &request,
            &result,
        )
        .await?;
    }
    tx.commit().await?;
    crate::runtime_admission::publish_barrier(&state, admission_commands).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn get_workflow(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<WorkflowResponse>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, id, false).await?;
    Ok(Json(load_workflow(&state, &actor, id).await?))
}

async fn update_workflow(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateWorkflowRequest>,
) -> ApiResult<Json<WorkflowResponse>> {
    actor.require("workflow:edit")?;
    require_workflow_access(&state, &actor, id, true).await?;
    validate_visibility(&input.visibility)?;
    let name = required_name(&input.name)?;
    let changed = sqlx::query("UPDATE workflows SET name=?,description=?,visibility=?,version=version+1 WHERE tenant_id=? AND id=? AND version=? AND status='active'")
        .bind(name).bind(input.description).bind(input.visibility).bind(actor.tenant_id).bind(id).bind(input.version).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "WORKFLOW_VERSION_CONFLICT",
            "Workflow changed or is archived",
        ));
    }
    Ok(Json(load_workflow(&state, &actor, id).await?))
}

async fn delete_workflow(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("workflow:archive")?;
    require_workflow_access(&state, &actor, id, true).await?;
    crate::workflow_revision_resources::require_unreferenced(
        &state.pool,
        actor.tenant_id,
        "workflow",
        id,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    crate::workflow_versions::lock_workflow(&mut tx, &actor, id).await?;
    let references: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM applications WHERE tenant_id=? AND workflow_id=?)+(SELECT COUNT(*) FROM workflow_deployments WHERE tenant_id=? AND workflow_id=?)+(SELECT COUNT(*) FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND publication_status='published')+(SELECT COUNT(*) FROM studio_builder_runs r JOIN studio_builder_sessions s ON s.tenant_id=r.tenant_id AND s.id=r.session_id WHERE s.tenant_id=? AND s.workflow_id=? AND r.status IN ('queued','running','waiting','outcome_unknown'))")
        .bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).fetch_one(&mut *tx).await?;
    if references != 0 {
        return Err(ApiError::conflict(
            "WORKFLOW_REFERENCED",
            "Workflow is referenced by an application or deployment",
        ));
    }
    for statement in [
        "DELETE e FROM studio_builder_entries e JOIN studio_builder_sessions s ON s.tenant_id=e.tenant_id AND s.id=e.session_id WHERE s.tenant_id=? AND s.workflow_id=?",
        "DELETE e FROM studio_builder_effects e JOIN studio_builder_sessions s ON s.tenant_id=e.tenant_id AND s.id=e.session_id WHERE s.tenant_id=? AND s.workflow_id=?",
        "DELETE r FROM studio_builder_runs r JOIN studio_builder_sessions s ON s.tenant_id=r.tenant_id AND s.id=r.session_id WHERE s.tenant_id=? AND s.workflow_id=?",
        "DELETE FROM studio_builder_sessions WHERE tenant_id=? AND workflow_id=?",
        "DELETE FROM workflow_debug_overlays WHERE tenant_id=? AND workflow_id=?",
        "DELETE FROM workflow_draft_resources WHERE tenant_id=? AND workflow_id=?",
        "DELETE FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND publication_status='unpublished'",
        "DELETE FROM workflow_draft_revisions WHERE tenant_id=? AND workflow_id=?",
        "DELETE FROM workflow_members WHERE tenant_id=? AND workflow_id=?",
        "DELETE FROM workflow_service_identities WHERE tenant_id=? AND workflow_id=?",
        "DELETE FROM workflow_drafts WHERE tenant_id=? AND workflow_id=?",
        "DELETE FROM workflows WHERE tenant_id=? AND id=?",
    ] {
        sqlx::query(statement)
            .bind(actor.tenant_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    audit(
        &mut tx,
        &actor,
        "workflow.deleted",
        "workflow",
        id,
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_draft(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<DraftResponse>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, id, false).await?;
    Ok(Json(
        load_draft(&state, actor.tenant_id, id, draft_id).await?,
    ))
}

async fn save_draft(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<SaveDraftRequest>,
) -> ApiResult<Json<DraftResponse>> {
    actor.require("workflow:edit")?;
    require_workflow_access(&state, &actor, id, true).await?;
    let (definition, editor, definition_hash, editor_hash) =
        validate_documents(input.definition, input.editor_document)?;
    let parsed_definition: WorkflowDefinition = serde_json::from_value(definition.clone())
        .map_err(|error| {
            ApiError::unprocessable("INVALID_WORKFLOW_DEFINITION", error.to_string())
        })?;
    let dependencies =
        load_composite_definitions_for_actor(&state, &actor, id, &parsed_definition).await?;
    let registry =
        crate::canvas_plugin_api::registry_for_tenant(&state, actor.tenant_id, &dependencies, true)
            .await?;
    let resolved = crate::canvas_plugin_api::resolve_workflow_plugin_closure(
        &state,
        actor.tenant_id,
        &parsed_definition,
        &dependencies,
        &registry,
    )
    .await?;
    if !resolved.invalid.is_empty() {
        let mut error = ApiError::unprocessable(
            "PLUGIN_RESOLVED_DEFINITION_INVALID",
            "A plugin rejected its resolved node contract",
        );
        for issue in resolved.invalid {
            error = error.with_field_error(issue.path, issue.code, issue.message);
        }
        return Err(error);
    }
    let draft_issues = WorkflowCompiler::new(&registry)
        .with_resolved_manifests(&resolved.manifests)
        .validate_draft(&parsed_definition, &CompileContext::default());
    if !draft_issues.is_empty() {
        let mut error = ApiError::unprocessable(
            "INVALID_WORKFLOW_DRAFT",
            "Draft contains an invalid reference or security-sensitive node configuration",
        );
        for issue in draft_issues {
            error = error.with_field_error(issue.path, issue.code, issue.message);
        }
        return Err(error);
    }
    let frozen = crate::workflow_revision_resources::for_save(
        &state,
        actor.tenant_id,
        id,
        draft_id,
        input.expected_revision,
        &parsed_definition,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    crate::workflow_versions::lock_workflow(&mut tx, &actor, id).await?;
    crate::canvas_plugin_api::lock_resolved_plugin_closure(&mut tx, actor.tenant_id, &resolved)
        .await?;
    crate::workflow_revision_resources::lock_for_save(&mut tx, actor.tenant_id, &frozen).await?;
    let row = sqlx::query(
        "SELECT d.*,v.id version_id,v.version_number,v.publication_status,v.archived FROM workflow_drafts d JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.working_draft_id=d.id AND v.workflow_id=d.workflow_id WHERE d.tenant_id=? AND d.workflow_id=? AND d.id=? AND v.archived=FALSE FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .bind(draft_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Workflow draft"))?;
    let current: u64 = row.try_get("revision")?;
    if current != input.expected_revision {
        return Err(ApiError::conflict(
            "DRAFT_REVISION_CONFLICT",
            format!("Draft is now at revision {current}"),
        ));
    }
    let next = current
        .checked_add(1)
        .ok_or_else(|| ApiError::internal("draft revision exhausted"))?;
    sqlx::query("UPDATE workflow_drafts SET revision=?,schema_version='8.0',definition_json=?,editor_json=?,content_hash=?,editor_hash=?,updated_by=? WHERE tenant_id=? AND workflow_id=? AND id=? AND revision=?")
        .bind(next).bind(&definition).bind(&editor).bind(&definition_hash).bind(&editor_hash).bind(actor.user_id).bind(actor.tenant_id).bind(id).bind(draft_id).bind(current).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO workflow_draft_revisions(id,tenant_id,workflow_id,draft_id,revision,schema_version,definition_json,editor_json,content_hash,editor_hash,created_by) VALUES(?,?,?,?,?,'8.0',?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(draft_id).bind(next).bind(&definition).bind(&editor).bind(&definition_hash).bind(&editor_hash).bind(actor.user_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE workflow_draft_revisions SET resource_snapshot_json=? WHERE tenant_id=? AND draft_id=? AND revision=?").bind(serde_json::to_value(&frozen).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(draft_id).bind(next).execute(&mut *tx).await?;
    replace_draft_resources(&mut tx, actor.tenant_id, id, draft_id, &parsed_definition).await?;
    audit(
        &mut tx,
        &actor,
        "workflow.draft_saved",
        "workflow",
        id,
        json!({"revision":next}),
    )
    .await?;
    let updated_at =
        sqlx::query_scalar("SELECT updated_at FROM workflow_drafts WHERE tenant_id=? AND id=?")
            .bind(actor.tenant_id)
            .bind(draft_id)
            .fetch_one(&mut *tx)
            .await?;
    let saved = DraftResponse {
        version_id: row.try_get("version_id")?,
        version_number: row.try_get("version_number")?,
        publication_status: row.try_get("publication_status")?,
        archived: row.try_get("archived")?,
        generation: row.try_get("generation")?,
        base_version_id: row.try_get("base_version_id")?,
        id: draft_id,
        workflow_id: id,
        schema_version: "8.0".into(),
        revision: next,
        definition,
        editor_document: editor,
        definition_hash,
        editor_hash,
        updated_at,
    };
    crate::workflow_versions::sync_unpublished(&mut tx, &actor, id, draft_id).await?;
    tx.commit().await?;
    Ok(Json(saved))
}

async fn list_revisions(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<RevisionResponse>>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, id, false).await?;
    let rows = sqlx::query("SELECT id,revision,schema_version,content_hash,editor_hash,created_by,created_at FROM workflow_draft_revisions WHERE tenant_id=? AND workflow_id=? AND draft_id=? ORDER BY revision DESC")
        .bind(actor.tenant_id).bind(id).bind(draft_id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                Ok(RevisionResponse {
                    id: row.try_get("id")?,
                    revision: row.try_get("revision")?,
                    schema_version: row.try_get("schema_version")?,
                    definition_hash: row.try_get("content_hash")?,
                    editor_hash: row.try_get::<String, _>("editor_hash")?,
                    created_by: row.try_get("created_by")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?,
    ))
}

async fn list_members(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<WorkflowMemberResponse>>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, id, false).await?;
    let rows = sqlx::query("SELECT wm.user_id,u.username,u.display_name,wm.member_role FROM workflow_members wm JOIN users u ON u.tenant_id=wm.tenant_id AND u.id=wm.user_id WHERE wm.tenant_id=? AND wm.workflow_id=? ORDER BY u.display_name,u.id")
        .bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                Ok(WorkflowMemberResponse {
                    user_id: row.try_get("user_id")?,
                    username: row.try_get("username")?,
                    display_name: row.try_get("display_name")?,
                    member_role: row.try_get("member_role")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?,
    ))
}

#[derive(Deserialize)]
struct MemberSearch {
    search: Option<String>,
}

async fn member_candidates(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(query): Query<MemberSearch>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:manage_member")?;
    require_workflow_access(&state, &actor, id, true).await?;
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let rows=sqlx::query("SELECT u.id,u.username,u.display_name FROM users u WHERE u.tenant_id=? AND u.status='active' AND (u.username LIKE ? OR u.display_name LIKE ?) AND NOT EXISTS(SELECT 1 FROM workflow_members m WHERE m.tenant_id=u.tenant_id AND m.workflow_id=? AND m.user_id=u.id) ORDER BY u.display_name,u.id LIMIT 100").bind(actor.tenant_id).bind(&search).bind(&search).bind(id).fetch_all(&state.pool).await?;
    let items=rows.into_iter().map(|row|->ApiResult<Value>{Ok(json!({"id":row.try_get::<Uuid,_>("id")?,"username":row.try_get::<String,_>("username")?,"displayName":row.try_get::<String,_>("display_name")?}))}).collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(json!(items)))
}

async fn upsert_member(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpsertWorkflowMemberRequest>,
) -> ApiResult<StatusCode> {
    actor.require("workflow:manage_member")?;
    require_workflow_access(&state, &actor, id, true).await?;
    if !matches!(input.member_role.as_str(), "viewer" | "editor" | "manager") {
        return Err(ApiError::bad_request(
            "INVALID_MEMBER_ROLE",
            "Member role is invalid",
        ));
    }
    let user_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE tenant_id=? AND id=? AND status='active')",
    )
    .bind(actor.tenant_id)
    .bind(input.user_id)
    .fetch_one(&state.pool)
    .await?;
    if !user_exists {
        return Err(ApiError::not_found("User"));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO workflow_members(tenant_id,workflow_id,user_id,member_role,created_by) VALUES(?,?,?,?,?) ON DUPLICATE KEY UPDATE member_role=VALUES(member_role)")
        .bind(actor.tenant_id).bind(id).bind(input.user_id).bind(input.member_role).bind(actor.user_id).execute(&mut *tx).await?;
    let grant_version = bump_workflow_version(&mut tx, actor.tenant_id, id).await?;
    emit_workflow_query_grant(
        &mut tx,
        actor.tenant_id,
        id,
        input.user_id,
        grant_version,
        true,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_member(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    actor.require("workflow:manage_member")?;
    require_workflow_access(&state, &actor, id, true).await?;
    let owner: Uuid =
        sqlx::query_scalar("SELECT owner_user_id FROM workflows WHERE tenant_id=? AND id=?")
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
    if owner == user_id {
        return Err(ApiError::conflict(
            "OWNER_MEMBER_REQUIRED",
            "Workflow owner cannot be removed",
        ));
    }
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query(
        "DELETE FROM workflow_members WHERE tenant_id=? AND workflow_id=? AND user_id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::not_found("Workflow member"));
    }
    let grant_version = bump_workflow_version(&mut tx, actor.tenant_id, id).await?;
    emit_workflow_query_grant(&mut tx, actor.tenant_id, id, user_id, grant_version, false).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_environments(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<Json<Vec<EnvironmentResponse>>> {
    actor.require("workflow:view")?;
    let rows=sqlx::query("SELECT id,code,name,is_builtin,status,version FROM workflow_environments WHERE tenant_id=? ORDER BY is_builtin DESC,name,id").bind(actor.tenant_id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(environment_from_row)
            .collect::<Result<_, _>>()?,
    ))
}

async fn create_environment(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateEnvironmentRequest>,
) -> ApiResult<(StatusCode, Json<EnvironmentResponse>)> {
    actor.require("workflow:publish")?;
    let name = required_name(&input.name)?;
    let code = input.code.trim().to_ascii_lowercase().replace(' ', "-");
    if code.is_empty()
        || code.len() > 64
        || !code
            .bytes()
            .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'-')
    {
        return Err(ApiError::bad_request(
            "INVALID_ENVIRONMENT_CODE",
            "Environment code is invalid",
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO workflow_environments(id,tenant_id,code,name) VALUES(?,?,?,?)")
        .bind(id)
        .bind(actor.tenant_id)
        .bind(code)
        .bind(name)
        .execute(&state.pool)
        .await
        .map_err(|error| match &error {
            sqlx::Error::Database(database) if database.is_unique_violation() => {
                ApiError::conflict("ENVIRONMENT_CODE_EXISTS", "Environment code already exists")
                    .with_field_error(
                        "code",
                        "ENVIRONMENT_CODE_EXISTS",
                        "Environment code already exists",
                    )
            }
            _ => ApiError::from(error),
        })?;
    let row=sqlx::query("SELECT id,code,name,is_builtin,status,version FROM workflow_environments WHERE tenant_id=? AND id=?").bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    Ok((StatusCode::CREATED, Json(environment_from_row(row)?)))
}

async fn update_environment(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateEnvironmentRequest>,
) -> ApiResult<Json<EnvironmentResponse>> {
    actor.require("workflow:publish")?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_ENVIRONMENT_STATUS",
            "Environment status is invalid",
        ));
    }
    let name = required_name(&input.name)?;
    let changed=sqlx::query("UPDATE workflow_environments SET name=?,status=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?").bind(name).bind(input.status).bind(actor.tenant_id).bind(id).bind(input.version).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "ENVIRONMENT_VERSION_CONFLICT",
            "Environment changed",
        ));
    }
    let row=sqlx::query("SELECT id,code,name,is_builtin,status,version FROM workflow_environments WHERE tenant_id=? AND id=?").bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    Ok(Json(environment_from_row(row)?))
}

async fn delete_environment(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("workflow:publish")?;
    let row=sqlx::query("SELECT is_builtin,(SELECT COUNT(*) FROM workflow_deployments d WHERE d.tenant_id=e.tenant_id AND d.environment_id=e.id) references_count FROM workflow_environments e WHERE tenant_id=? AND id=?").bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Environment"))?;
    if row.try_get::<bool, _>("is_builtin")? {
        return Err(ApiError::conflict(
            "BUILTIN_ENVIRONMENT",
            "Built-in environments cannot be deleted",
        ));
    }
    if row.try_get::<i64, _>("references_count")? != 0 {
        return Err(ApiError::conflict(
            "ENVIRONMENT_REFERENCED",
            "Environment is referenced by a deployment",
        ));
    }
    sqlx::query("DELETE FROM workflow_environments WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_deployments(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<DeploymentResponse>>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, id, false).await?;
    let rows=sqlx::query("SELECT d.id,d.workflow_id,d.environment_id,e.name environment_name,d.workflow_version_id,v.version_number,d.sequence_number,d.status,d.source,d.created_at FROM workflow_deployments d JOIN workflow_environments e ON e.tenant_id=d.tenant_id AND e.id=d.environment_id JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.id=d.workflow_version_id WHERE d.tenant_id=? AND d.workflow_id=? ORDER BY d.created_at DESC,d.id DESC").bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(deployment_from_row)
            .collect::<Result<_, _>>()?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValidateDraftRequest {
    definition: Value,
    editor_document: Value,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidationIssue {
    code: String,
    severity: String,
    node_id: Option<String>,
    field_path: Option<String>,
    message: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidateDraftResponse {
    issues: Vec<ValidationIssue>,
    definition_hash: Option<String>,
    editor_hash: Option<String>,
    compiler_version: Option<String>,
}

pub(crate) async fn builder_validate_document(
    state: &ControlApiState,
    actor: &Actor,
    workflow: Uuid,
    draft: Uuid,
    document: &agentx_runtime_contracts::StudioDocumentV1,
    complete: bool,
) -> ApiResult<Value> {
    let Json(result) = validate_document(
        State(state.clone()),
        actor.clone(),
        Path((workflow, draft)),
        Json(ValidateDraftRequest {
            definition: serde_json::to_value(&document.definition).map_err(ApiError::internal)?,
            editor_document: serde_json::to_value(&document.editor_document)
                .map_err(ApiError::internal)?,
        }),
        complete,
    )
    .await?;
    serde_json::to_value(result).map_err(ApiError::internal)
}

async fn validate_draft(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<ValidateDraftRequest>,
) -> ApiResult<Json<ValidateDraftResponse>> {
    validate_document(State(state), actor, Path((id, draft_id)), Json(input), true).await
}

async fn validate_document(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<ValidateDraftRequest>,
    complete: bool,
) -> ApiResult<Json<ValidateDraftResponse>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, id, false).await?;
    load_draft(&state, actor.tenant_id, id, draft_id).await?;
    let editor_value = input.editor_document;
    let mut issues = Vec::new();
    let mut definition_hash = None;
    let mut editor_hash = None;
    let mut compiler_version = None;
    match serde_json::from_value::<WorkflowDefinition>(input.definition.clone()) {
        Err(error) => issues.push(validation_issue(
            "INVALID_WORKFLOW_DEFINITION",
            "",
            error.to_string(),
        )),
        Ok(definition) => {
            issues.extend(
                validate_definition(&definition)
                    .into_iter()
                    .map(|issue| validation_issue(&issue.code, &issue.path, issue.message)),
            );
            match serde_json::from_value::<EditorDocument>(editor_value.clone()) {
                Err(error) => issues.push(validation_issue(
                    "INVALID_EDITOR_DOCUMENT",
                    "",
                    error.to_string(),
                )),
                Ok(editor) => issues.extend(
                    validate_editor_document(&definition, &editor)
                        .into_iter()
                        .map(|issue| validation_issue(&issue.code, &issue.path, issue.message)),
                ),
            }
            if issues.is_empty() {
                let dependencies =
                    load_composite_definitions_for_actor(&state, &actor, id, &definition).await?;
                let registry = crate::canvas_plugin_api::registry_for_tenant(
                    &state,
                    actor.tenant_id,
                    &dependencies,
                    true,
                )
                .await?;
                let resolved = crate::canvas_plugin_api::resolve_workflow_plugin_closure(
                    &state,
                    actor.tenant_id,
                    &definition,
                    &dependencies,
                    &registry,
                )
                .await?;
                if complete {
                    issues.extend(resolved.incomplete.iter().map(|issue| {
                        validation_issue(&issue.code, &issue.path, issue.message.clone())
                    }));
                }
                issues.extend(resolved.invalid.iter().map(|issue| {
                    validation_issue(&issue.code, &issue.path, issue.message.clone())
                }));
                if !complete {
                    issues.extend(
                        WorkflowCompiler::new(&registry)
                            .with_resolved_manifests(&resolved.manifests)
                            .validate_draft(&definition, &CompileContext::default())
                            .into_iter()
                            .map(|issue| validation_issue(&issue.code, &issue.path, issue.message)),
                    );
                } else {
                    match WorkflowCompiler::new(&registry)
                        .with_resolved_manifests(&resolved.manifests)
                        .compile(&definition, &CompileContext::default())
                    {
                        Ok(compiled) => compiler_version = Some(compiled.compiler_version),
                        Err(error) => issues.extend(error.issues.into_iter().map(|issue| {
                            validation_issue(&issue.code, &issue.path, issue.message)
                        })),
                    }
                }
            }
            definition_hash = canonical_content_hash(&input.definition).ok();
            editor_hash = canonical_content_hash(&editor_value).ok();
        }
    }
    for issue in &mut issues {
        if let Some(index) = issue
            .node_id
            .as_deref()
            .and_then(|id| id.parse::<usize>().ok())
        {
            issue.node_id = input
                .definition
                .get("nodes")
                .and_then(Value::as_array)
                .and_then(|nodes| nodes.get(index))
                .and_then(|node| node.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if let Some(path) = &mut issue.field_path {
                if let Some(field) = path.strip_prefix(&format!("nodes[{index}].")) {
                    *path = field.to_owned();
                }
            }
        }
    }
    Ok(Json(ValidateDraftResponse {
        issues,
        definition_hash,
        editor_hash,
        compiler_version,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugOverlayResponse {
    id: Uuid,
    workflow_id: Uuid,
    node_id: String,
    kind: String,
    payload: Value,
    artifact_id: Option<Uuid>,
    schema_hash: Option<String>,
    stale: bool,
    updated_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveDebugOverlayRequest {
    kind: String,
    payload: Value,
    artifact_id: Option<Uuid>,
    schema_hash: Option<String>,
}

async fn get_debug_overlay(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id, node_id)): Path<(Uuid, Uuid, String)>,
) -> ApiResult<Json<DebugOverlayResponse>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, id, false).await?;
    let row=sqlx::query("SELECT id,workflow_id,node_id,kind,payload_json,artifact_id,schema_hash,stale,updated_by,updated_at FROM workflow_debug_overlays WHERE tenant_id=? AND workflow_id=? AND draft_id=? AND node_id=?").bind(actor.tenant_id).bind(id).bind(draft_id).bind(node_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Debug overlay"))?;
    Ok(Json(overlay_from_row(row)?))
}

async fn save_debug_overlay(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id, node_id)): Path<(Uuid, Uuid, String)>,
    Json(input): Json<SaveDebugOverlayRequest>,
) -> ApiResult<Json<DebugOverlayResponse>> {
    actor.require("workflow:edit")?;
    require_workflow_access(&state, &actor, id, true).await?;
    if !matches!(
        input.kind.as_str(),
        "pin_data" | "mock_output" | "temporary_input" | "history_output" | "artifact"
    ) {
        return Err(ApiError::bad_request(
            "INVALID_DEBUG_OVERLAY_KIND",
            "Unsupported debug overlay kind",
        ));
    }
    if (input.kind == "artifact") != input.artifact_id.is_some() {
        return Err(ApiError::unprocessable(
            "DEBUG_OVERLAY_ARTIFACT_INVALID",
            "artifactId is required only for artifact overlays",
        ));
    }
    let definition = load_definition(&state, actor.tenant_id, id, draft_id).await?;
    let node = definition
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .ok_or_else(|| ApiError::not_found("Workflow node"))?;
    let schema_hash=canonical_content_hash(&json!({"nodeType":node.node_type,"typeVersion":node.type_version,"parameters":node.parameters})).map_err(ApiError::internal)?;
    if input
        .schema_hash
        .as_deref()
        .is_some_and(|hash| hash != schema_hash)
    {
        return Err(ApiError::unprocessable(
            "DEBUG_OVERLAY_SCHEMA_STALE",
            "The overlay schema no longer matches the selected node",
        ));
    }
    sqlx::query("INSERT INTO workflow_debug_overlays(id,tenant_id,workflow_id,draft_id,node_id,kind,payload_json,artifact_id,schema_hash,stale,updated_by) VALUES(?,?,?,?,?,?,?,?,?,FALSE,?) ON DUPLICATE KEY UPDATE kind=VALUES(kind),payload_json=VALUES(payload_json),artifact_id=VALUES(artifact_id),schema_hash=VALUES(schema_hash),stale=FALSE,updated_by=VALUES(updated_by)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(draft_id).bind(&node_id).bind(input.kind).bind(input.payload).bind(input.artifact_id).bind(schema_hash).bind(actor.user_id).execute(&state.pool).await?;
    get_debug_overlay(State(state), actor, Path((id, draft_id, node_id))).await
}

async fn delete_debug_overlay(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id, node_id)): Path<(Uuid, Uuid, String)>,
) -> ApiResult<StatusCode> {
    actor.require("workflow:edit")?;
    require_workflow_access(&state, &actor, id, true).await?;
    sqlx::query(
        "DELETE FROM workflow_debug_overlays WHERE tenant_id=? AND workflow_id=? AND draft_id=? AND node_id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
        .bind(draft_id)
    .bind(node_id)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn require_workflow_access(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
    write: bool,
) -> ApiResult<()> {
    if actor.roles.iter().any(|role| role == "company_admin") {
        return workflow_exists(state, actor.tenant_id, id).await;
    }
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflows w WHERE w.tenant_id=? AND w.id=? AND (w.owner_user_id=? OR EXISTS(SELECT 1 FROM workflow_members wm WHERE wm.tenant_id=w.tenant_id AND wm.workflow_id=w.id AND wm.user_id=? AND (?=FALSE OR wm.member_role IN ('editor','manager'))) OR (?=FALSE AND (w.visibility='company' OR (w.visibility='department' AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=w.tenant_id AND dc.ancestor_id=w.owner_department_id AND dc.descendant_id=?))))))")
        .bind(actor.tenant_id).bind(id).bind(actor.user_id).bind(actor.user_id).bind(write).bind(write).bind(actor.department_id).fetch_one(&state.pool).await?;
    if allowed {
        Ok(())
    } else {
        Err(ApiError::not_found("Workflow"))
    }
}

async fn workflow_exists(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<()> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflows WHERE tenant_id=? AND id=?)")
            .bind(tenant)
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("Workflow"))
    }
}

async fn bump_workflow_version(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    workflow_id: Uuid,
) -> ApiResult<u64> {
    let changed = sqlx::query(
        "UPDATE workflows SET version=version+1 WHERE tenant_id=? AND id=? AND status='active'",
    )
    .bind(tenant_id)
    .bind(workflow_id)
    .execute(&mut **tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::not_found("Workflow"));
    }
    Ok(
        sqlx::query_scalar("SELECT version FROM workflows WHERE tenant_id=? AND id=?")
            .bind(tenant_id)
            .bind(workflow_id)
            .fetch_one(&mut **tx)
            .await?,
    )
}

pub(crate) async fn emit_workflow_query_grant(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    workflow_id: Uuid,
    user_id: Uuid,
    grant_version: u64,
    enabled: bool,
) -> ApiResult<()> {
    let can_query: bool = if enabled {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u JOIN user_roles ur ON ur.tenant_id=u.tenant_id AND ur.user_id=u.id JOIN roles r ON r.tenant_id=ur.tenant_id AND r.id=ur.role_id AND r.status='active' JOIN role_permissions rp ON rp.tenant_id=r.tenant_id AND rp.role_id=r.id JOIN permissions p ON p.id=rp.permission_id AND p.permission_key='execution:view' WHERE u.tenant_id=? AND u.id=? AND u.status='active')")
            .bind(tenant_id)
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await?
    } else {
        false
    };
    let payload = json!({
        "userId": user_id,
        "workflowId": workflow_id,
        "grantVersion": grant_version,
        "canQuery": can_query,
        "admissionEpoch": grant_version,
    });
    let request_hash = agentx_runtime_contracts::content_hash(&payload)
        .map_err(ApiError::internal)?
        .to_string();
    sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,?,'workflow_admission',?,?,'pending',?,?)")
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind("RuntimeUserWorkflowGrantChanged")
        .bind(workflow_id.to_string())
        .bind(payload)
        .bind(request_hash)
        .bind(format!("workflow-query-grant:{workflow_id}:{user_id}:{grant_version}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn load_workflow(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<WorkflowResponse> {
    let row=sqlx::query("SELECT w.id,w.name,w.description,w.status,w.visibility,w.owner_user_id,w.owner_department_id,si.id service_identity_id,u.display_name owner_name,w.version,w.updated_at,(SELECT MAX(version_number) FROM workflow_versions v WHERE v.tenant_id=w.tenant_id AND v.workflow_id=w.id) latest_version FROM workflows w JOIN users u ON u.tenant_id=w.tenant_id AND u.id=w.owner_user_id JOIN workflow_service_identities si ON si.tenant_id=w.tenant_id AND si.workflow_id=w.id WHERE w.tenant_id=? AND w.id=?").bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Workflow"))?;
    let can_edit = actor
        .permissions
        .iter()
        .any(|value| value == "workflow:edit")
        && require_workflow_access(state, actor, id, true)
            .await
            .is_ok();
    Ok(workflow_from_row(row, can_edit)?)
}

pub(crate) async fn load_draft(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
    draft_id: Uuid,
) -> ApiResult<DraftResponse> {
    let row=sqlx::query("SELECT d.*,v.id version_id,v.version_number,v.publication_status,v.archived FROM workflow_drafts d JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.workflow_id=d.workflow_id AND v.working_draft_id=d.id WHERE d.tenant_id=? AND d.workflow_id=? AND d.id=?").bind(tenant).bind(id).bind(draft_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Workflow document"))?;
    Ok(DraftResponse {
        version_id: row.try_get("version_id")?,
        version_number: row.try_get("version_number")?,
        publication_status: row.try_get("publication_status")?,
        archived: row.try_get("archived")?,
        generation: row.try_get("generation")?,
        base_version_id: row.try_get("base_version_id")?,
        id: row.try_get("id")?,
        workflow_id: row.try_get("workflow_id")?,
        schema_version: row.try_get("schema_version")?,
        revision: row.try_get("revision")?,
        definition: row.try_get("definition_json")?,
        editor_document: row.try_get("editor_json")?,
        definition_hash: row.try_get("content_hash")?,
        editor_hash: row.try_get("editor_hash")?,
        updated_at: row.try_get("updated_at")?,
    })
}
async fn load_definition(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
    draft_id: Uuid,
) -> ApiResult<WorkflowDefinition> {
    let value: Value = sqlx::query_scalar(
        "SELECT definition_json FROM workflow_drafts WHERE tenant_id=? AND workflow_id=? AND id=?",
    )
    .bind(tenant)
    .bind(id)
    .bind(draft_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Workflow draft"))?;
    serde_json::from_value(value).map_err(ApiError::internal)
}

pub(crate) fn validate_documents(
    definition: Value,
    editor: Value,
) -> ApiResult<(Value, Value, String, String)> {
    let parsed: WorkflowDefinition =
        serde_json::from_value(definition.clone()).map_err(|error| {
            ApiError::unprocessable("INVALID_WORKFLOW_DEFINITION", error.to_string())
        })?;
    let issues = validate_definition(&parsed);
    if !issues.is_empty() {
        let error = ApiError::unprocessable(
            "INVALID_WORKFLOW_DEFINITION",
            issues
                .iter()
                .map(|issue| issue.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        );
        return Err(issues.into_iter().fold(error, |error, issue| {
            error.with_field_error(issue.path, issue.code, issue.message)
        }));
    }
    let parsed_editor: EditorDocument = serde_json::from_value(editor.clone())
        .map_err(|error| ApiError::unprocessable("INVALID_EDITOR_DOCUMENT", error.to_string()))?;
    let issues = validate_editor_document(&parsed, &parsed_editor);
    if !issues.is_empty() {
        let error = ApiError::unprocessable(
            "INVALID_EDITOR_DOCUMENT",
            issues
                .iter()
                .map(|issue| issue.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        );
        return Err(issues.into_iter().fold(error, |error, issue| {
            error.with_field_error(issue.path, issue.code, issue.message)
        }));
    }
    let definition_hash = canonical_content_hash(&definition).map_err(ApiError::internal)?;
    let editor_hash = canonical_content_hash(&editor).map_err(ApiError::internal)?;
    Ok((definition, editor, definition_hash, editor_hash))
}

fn workflow_from_row(
    row: sqlx::mysql::MySqlRow,
    can_edit: bool,
) -> Result<WorkflowResponse, sqlx::Error> {
    Ok(WorkflowResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
        visibility: row.try_get("visibility")?,
        owner_user_id: row.try_get("owner_user_id")?,
        owner_department_id: row.try_get("owner_department_id")?,
        service_identity_id: row.try_get("service_identity_id")?,
        owner_name: row.try_get("owner_name")?,
        latest_version: row.try_get("latest_version")?,
        version: row.try_get("version")?,
        can_edit,
        updated_at: row.try_get("updated_at")?,
    })
}
fn environment_from_row(row: sqlx::mysql::MySqlRow) -> Result<EnvironmentResponse, sqlx::Error> {
    Ok(EnvironmentResponse {
        id: row.try_get("id")?,
        code: row.try_get("code")?,
        name: row.try_get("name")?,
        is_builtin: row.try_get("is_builtin")?,
        status: row.try_get("status")?,
        version: row.try_get("version")?,
    })
}
pub(crate) fn deployment_from_row(
    row: sqlx::mysql::MySqlRow,
) -> Result<DeploymentResponse, sqlx::Error> {
    Ok(DeploymentResponse {
        id: row.try_get("id")?,
        workflow_id: row.try_get("workflow_id")?,
        environment_id: row.try_get("environment_id")?,
        environment_name: row.try_get("environment_name")?,
        workflow_version_id: row.try_get("workflow_version_id")?,
        version_number: row.try_get("version_number")?,
        sequence_number: row.try_get("sequence_number")?,
        status: row.try_get("status")?,
        source: row.try_get("source")?,
        created_at: row.try_get("created_at")?,
    })
}
fn overlay_from_row(row: sqlx::mysql::MySqlRow) -> Result<DebugOverlayResponse, sqlx::Error> {
    Ok(DebugOverlayResponse {
        id: row.try_get("id")?,
        workflow_id: row.try_get("workflow_id")?,
        node_id: row.try_get("node_id")?,
        kind: row.try_get("kind")?,
        payload: row.try_get("payload_json")?,
        artifact_id: row.try_get("artifact_id")?,
        schema_hash: row.try_get("schema_hash")?,
        stale: row.try_get("stale")?,
        updated_by: row.try_get("updated_by")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn validate_visibility(value: &str) -> ApiResult<()> {
    if matches!(value, "private" | "department" | "company") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "INVALID_VISIBILITY",
            "Visibility must be private, department, or company",
        ))
    }
}
fn validation_issue(code: &str, path: &str, message: String) -> ValidationIssue {
    ValidationIssue {
        code: code.to_owned(),
        severity: "error".into(),
        node_id: path
            .strip_prefix("nodes[")
            .and_then(|value| value.split_once(']'))
            .map(|(index, _)| index.to_owned()),
        field_path: (!path.is_empty()).then(|| path.to_owned()),
        message,
    }
}

pub(crate) async fn load_composite_definitions_for_actor(
    state: &ControlApiState,
    actor: &Actor,
    root_workflow_id: Uuid,
    definition: &WorkflowDefinition,
) -> ApiResult<BTreeMap<Uuid, WorkflowDefinition>> {
    for version in composite_dependency_ids(definition)? {
        let workflow: Uuid = sqlx::query_scalar(
            "SELECT workflow_id FROM workflow_versions WHERE tenant_id=? AND id=? AND publication_status='published'",
        )
        .bind(actor.tenant_id)
        .bind(version)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Workflow version"))?;
        require_workflow_access(state, actor, workflow, false).await?;
    }
    load_composite_definitions(state, actor.tenant_id, root_workflow_id, definition).await
}

pub(crate) async fn load_composite_definitions(
    state: &ControlApiState,
    tenant_id: Uuid,
    root_workflow_id: Uuid,
    definition: &WorkflowDefinition,
) -> ApiResult<BTreeMap<Uuid, WorkflowDefinition>> {
    let mut definitions = BTreeMap::new();
    let mut pending = VecDeque::from(composite_dependency_ids(definition)?);
    while let Some(version_id) = pending.pop_front() {
        if definitions.contains_key(&version_id) {
            continue;
        }
        let row = sqlx::query(
            "SELECT workflow_id,definition_json FROM workflow_versions WHERE tenant_id=? AND id=? AND publication_status='published'",
        )
        .bind(tenant_id)
        .bind(version_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| {
            ApiError::unprocessable(
                "SUBWORKFLOW_VERSION_NOT_FOUND",
                format!("Fixed Workflow Version {version_id} does not exist"),
            )
        })?;
        if row.try_get::<Uuid, _>("workflow_id")? == root_workflow_id {
            return Err(ApiError::unprocessable(
                "RECURSIVE_SUBWORKFLOW",
                "A Workflow cannot reference its own version lineage through a Composite",
            ));
        }
        let child: WorkflowDefinition = serde_json::from_value(
            row.try_get::<Value, _>("definition_json")?,
        )
        .map_err(|error| {
            ApiError::unprocessable("INVALID_SUBWORKFLOW_DEFINITION", error.to_string())
        })?;
        pending.extend(composite_dependency_ids(&child)?);
        definitions.insert(version_id, child);
    }
    Ok(definitions)
}

fn composite_dependency_ids(definition: &WorkflowDefinition) -> ApiResult<Vec<Uuid>> {
    definition
        .nodes
        .iter()
        .filter(|node| node.node_type == "sub_workflow")
        .map(|node| {
            node.parameters
                .get("workflowVersionId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ApiError::unprocessable(
                        "SUBWORKFLOW_VERSION_REQUIRED",
                        "Composite nodes must reference an immutable Workflow Version",
                    )
                })
                .and_then(|value| {
                    Uuid::parse_str(value).map_err(|_| {
                        ApiError::unprocessable(
                            "INVALID_SUBWORKFLOW_VERSION",
                            "Composite Workflow Version must be a UUID",
                        )
                    })
                })
        })
        .collect()
}

pub(crate) async fn audit(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    action: &str,
    target_type: &str,
    target: Uuid,
    detail: Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO audit_events(id,tenant_id,actor_user_id,action,target_type,target_id,request_id,detail_json) VALUES(?,?,?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(actor.user_id).bind(action).bind(target_type).bind(target.to_string()).bind(Uuid::now_v7()).bind(detail).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visibility_is_stable() {
        assert!(validate_visibility("company").is_ok());
        assert!(validate_visibility("public").is_err());
    }

    #[test]
    fn editor_rejection_preserves_actionable_exit_field_feedback() {
        let mut definition = WorkflowDefinition::empty();
        definition.nodes[0].parameters = json!({"outputs":{},"errorOutputs":{
            "message":{"kind":"reference","namespace":"item","path":["message"]}
        }});
        let feedback = validate_documents(
            serde_json::to_value(&definition).unwrap(),
            serde_json::to_value(EditorDocument::default()).unwrap(),
        )
        .unwrap_err()
        .design_feedback();
        assert_eq!(
            feedback["fieldErrors"][0]["code"],
            "EXIT_PARAMETERS_INVALID"
        );
        assert_eq!(feedback["fieldErrors"][0]["field"], "nodes[0].parameters");
        assert!(
            feedback["fieldErrors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("selector")
        );

        definition.nodes[0].parameters = json!({"outputs":{},"errorOutputs":{}});
        definition.end.outputs.insert(
            "answer".into(),
            serde_json::from_value(json!({"schema":{"type":"string"},"required":true})).unwrap(),
        );
        let feedback = validate_documents(
            serde_json::to_value(definition).unwrap(),
            serde_json::to_value(EditorDocument::default()).unwrap(),
        )
        .unwrap_err()
        .design_feedback();
        assert_eq!(
            feedback["fieldErrors"][0]["code"],
            "EXIT_REQUIRED_MAPPING_MISSING"
        );
        assert_eq!(
            feedback["fieldErrors"][0]["field"],
            "nodes[0].parameters.outputs.answer"
        );
    }
}
