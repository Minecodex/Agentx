//! Version-owned authoring documents. Published payloads never change.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    workflow_api::{audit, load_draft, require_workflow_access},
};
use agentx_domain::{EditorDocument, WorkflowDefinition, canonical_content_hash};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

pub(crate) const SELECT_VERSION: &str = "SELECT v.*,b.version_number base_version_number,d.revision draft_revision,d.generation document_generation,IF(v.publication_status='published' AND d.id IS NOT NULL AND (d.content_hash<>v.content_hash OR d.editor_hash<>v.editor_hash),TRUE,FALSE) has_draft,u.display_name updated_by_name FROM workflow_versions v LEFT JOIN workflow_versions b ON b.tenant_id=v.tenant_id AND b.workflow_id=v.workflow_id AND b.id=v.base_version_id LEFT JOIN workflow_drafts d ON d.tenant_id=v.tenant_id AND d.workflow_id=v.workflow_id AND d.id=v.working_draft_id JOIN users u ON u.tenant_id=v.tenant_id AND u.id=v.updated_by";

#[derive(Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowVersionResponse {
    pub id: Uuid,
    pub workflow_id: Uuid,
    pub version_number: u64,
    pub publication_status: String,
    pub archived: bool,
    pub working_draft_id: Option<Uuid>,
    pub draft_revision: Option<u64>,
    pub has_draft: bool,
    pub base_version_id: Option<Uuid>,
    pub base_version_number: Option<u64>,
    pub source_draft_id: Uuid,
    pub source_revision_id: Uuid,
    pub source_revision: u64,
    pub schema_version: String,
    pub content_hash: String,
    pub editor_hash: String,
    pub definition: Value,
    pub editor_document: Value,
    pub node_manifests:
        std::collections::BTreeMap<String, agentx_node_protocol::NodeManifestVersion>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    pub updated_by_name: String,
    pub created_at: String,
    pub updated_at: String,
    pub published_at: Option<String>,
}

pub(crate) fn version_from_row(row: sqlx::mysql::MySqlRow) -> ApiResult<WorkflowVersionResponse> {
    let timestamp = |name| -> ApiResult<String> {
        row.try_get::<time::OffsetDateTime, _>(name)?
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(ApiError::internal)
    };
    Ok(WorkflowVersionResponse {
        id: row.try_get("id")?,
        workflow_id: row.try_get("workflow_id")?,
        version_number: row.try_get("version_number")?,
        publication_status: row.try_get("publication_status")?,
        archived: row.try_get("archived")?,
        working_draft_id: row.try_get("working_draft_id")?,
        draft_revision: row.try_get("draft_revision")?,
        has_draft: row.try_get("has_draft")?,
        base_version_id: row.try_get("base_version_id")?,
        base_version_number: row.try_get("base_version_number")?,
        source_draft_id: row.try_get("source_draft_id")?,
        source_revision_id: row.try_get("source_revision_id")?,
        source_revision: row.try_get("source_revision")?,
        schema_version: row.try_get("schema_version")?,
        content_hash: row.try_get("content_hash")?,
        editor_hash: row.try_get("editor_hash")?,
        definition: row.try_get("definition_json")?,
        editor_document: row.try_get("editor_json")?,
        node_manifests: serde_json::from_value(row.try_get("node_manifests_json")?)
            .map_err(ApiError::internal)?,
        created_by: row.try_get("created_by")?,
        updated_by: row.try_get("updated_by")?,
        updated_by_name: row.try_get("updated_by_name")?,
        created_at: timestamp("created_at")?,
        updated_at: timestamp("updated_at")?,
        published_at: row
            .try_get::<Option<time::OffsetDateTime>, _>("published_at")?
            .map(|time| time.format(&time::format_description::well_known::Rfc3339))
            .transpose()
            .map_err(ApiError::internal)?,
    })
}

pub(crate) fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/workflows/{id}/versions", get(list).post(create))
        .route(
            "/api/v1/workflows/{id}/versions/{version_id}",
            get(details).patch(archive),
        )
        .route(
            "/api/v1/workflows/{id}/versions/{version_id}/edit",
            post(edit),
        )
        .route(
            "/api/v1/workflows/{id}/versions/{version_id}/restore",
            post(restore),
        )
}

pub(crate) async fn lock_workflow(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    workflow: Uuid,
) -> ApiResult<()> {
    sqlx::query("SELECT id FROM workflows WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id)
        .bind(workflow)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::not_found("Workflow"))?;
    Ok(())
}

pub(crate) async fn load(
    state: &ControlApiState,
    actor: &Actor,
    workflow: Uuid,
    version: Uuid,
) -> ApiResult<WorkflowVersionResponse> {
    let row = sqlx::query(&format!(
        "{SELECT_VERSION} WHERE v.tenant_id=? AND v.workflow_id=? AND v.id=?"
    ))
    .bind(actor.tenant_id)
    .bind(workflow)
    .bind(version)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Workflow version"))?;
    version_from_row(row)
}

async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, workflow, false).await?;
    let summary=SELECT_VERSION.replace("v.*", "v.id,v.workflow_id,v.version_number,v.publication_status,v.archived,v.working_draft_id,v.base_version_id,v.source_draft_id,v.source_revision_id,v.source_revision,v.schema_version,v.content_hash,v.editor_hash,v.created_by,v.updated_by,v.created_at,v.updated_at,v.published_at,JSON_OBJECT() definition_json,JSON_OBJECT() editor_json,JSON_OBJECT() node_manifests_json");
    let rows = sqlx::query(&format!(
        "{summary} WHERE v.tenant_id=? AND v.workflow_id=? ORDER BY v.version_number DESC"
    ))
    .bind(actor.tenant_id)
    .bind(workflow)
    .fetch_all(&state.pool)
    .await?;
    let mut items = Vec::new();
    for row in rows {
        let mut item = serde_json::to_value(version_from_row(row)?).map_err(ApiError::internal)?;
        for key in ["definition", "editorDocument", "nodeManifests"] {
            item.as_object_mut().unwrap().remove(key);
        }
        items.push(item);
    }
    Ok(Json(json!(items)))
}

async fn details(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((workflow, version)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<WorkflowVersionResponse>> {
    actor.require("workflow:view")?;
    require_workflow_access(&state, &actor, workflow, false).await?;
    Ok(Json(load(&state, &actor, workflow, version).await?))
}

/// Persist a successful result in the same transaction as its business writes.
pub(crate) async fn replay(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    operation: &str,
    headers: &HeaderMap,
    request: &Value,
) -> ApiResult<Option<Value>> {
    let key = request_key(headers)?;
    let hash = canonical_content_hash(request).map_err(ApiError::internal)?;
    let operation = format!("{}:{operation}", actor.user_id);
    if let Some(row)=sqlx::query("SELECT request_hash,response_json FROM idempotency_records WHERE tenant_id=? AND operation_key=? AND idempotency_key=? FOR UPDATE").bind(actor.tenant_id).bind(operation).bind(key).fetch_optional(&mut **tx).await? {
        if row.try_get::<String,_>("request_hash")? != hash { return Err(ApiError::conflict("IDEMPOTENCY_CONFLICT","This request key was used with different content")); }
        return Ok(row.try_get("response_json")?);
    }
    Ok(None)
}

fn request_key(headers: &HeaderMap) -> ApiResult<&str> {
    headers
        .get("Idempotency-Key")
        .and_then(|key| key.to_str().ok())
        .filter(|key| !key.is_empty() && key.len() <= 128)
        .ok_or_else(|| {
            ApiError::bad_request(
                "IDEMPOTENCY_KEY_REQUIRED",
                "A nonempty Idempotency-Key of at most 128 bytes is required",
            )
        })
}

pub(crate) async fn receipt(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    operation: &str,
    headers: &HeaderMap,
    request: &Value,
    response: &Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO idempotency_records(tenant_id,operation_key,idempotency_key,request_hash,response_json,expires_at) VALUES(?,?,?,?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 7 DAY))")
        .bind(actor.tenant_id).bind(format!("{}:{operation}",actor.user_id)).bind(request_key(headers)?).bind(canonical_content_hash(request).map_err(ApiError::internal)?).bind(response).execute(&mut **tx).await?;
    Ok(())
}

pub(crate) async fn insert_unpublished(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    workflow: Uuid,
    draft: Uuid,
) -> ApiResult<Uuid> {
    let version = Uuid::now_v7();
    let number: u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(version_number),0)+1 AS UNSIGNED) FROM workflow_versions WHERE tenant_id=? AND workflow_id=?").bind(actor.tenant_id).bind(workflow).fetch_one(&mut **tx).await?;
    sqlx::query("INSERT INTO workflow_versions(id,tenant_id,workflow_id,version_number,working_draft_id,source_draft_id,source_revision_id,source_revision,schema_version,definition_json,editor_json,content_hash,editor_hash,created_by,updated_by) SELECT ?,tenant_id,workflow_id,?,draft_id,draft_id,id,revision,schema_version,definition_json,editor_json,content_hash,editor_hash,?,? FROM workflow_draft_revisions WHERE tenant_id=? AND workflow_id=? AND draft_id=? AND revision=0")
        .bind(version).bind(number).bind(actor.user_id).bind(actor.user_id).bind(actor.tenant_id).bind(workflow).bind(draft).execute(&mut **tx).await?;
    Ok(version)
}

async fn create(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:edit")?;
    require_workflow_access(&state, &actor, workflow, true).await?;
    let mut tx = state.pool.begin().await?;
    lock_workflow(&mut tx, &actor, workflow).await?;
    let operation = format!("canvas:{workflow}");
    let request = json!({});
    if let Some(value) = replay(&mut tx, &actor, &operation, &headers, &request).await? {
        tx.commit().await?;
        return Ok(Json(value));
    }
    let draft = Uuid::now_v7();
    let definition =
        serde_json::to_value(WorkflowDefinition::empty()).map_err(ApiError::internal)?;
    let editor = serde_json::to_value(EditorDocument::default()).map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO workflow_drafts(id,tenant_id,workflow_id,created_by,schema_version,definition_json,editor_json,content_hash,editor_hash,updated_by) VALUES(?,?,?,?,'8.0',?,?,?,?,?)").bind(draft).bind(actor.tenant_id).bind(workflow).bind(actor.user_id).bind(&definition).bind(&editor).bind(canonical_content_hash(&definition).map_err(ApiError::internal)?).bind(canonical_content_hash(&editor).map_err(ApiError::internal)?).bind(actor.user_id).execute(&mut *tx).await?;
    crate::workflow_drafts::record_initial_revision(&mut tx, &actor, workflow, draft).await?;
    let version = insert_unpublished(&mut tx, &actor, workflow, draft).await?;
    let result = json!({"id":version,"workflowId":workflow,"versionNumber":sqlx::query_scalar::<_,u64>("SELECT version_number FROM workflow_versions WHERE id=?").bind(version).fetch_one(&mut *tx).await?});
    receipt(&mut tx, &actor, &operation, &headers, &request, &result).await?;
    audit(
        &mut tx,
        &actor,
        "workflow.canvas_created",
        "workflow",
        workflow,
        result.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(result))
}

async fn edit(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((workflow, version)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:edit")?;
    require_workflow_access(&state, &actor, workflow, true).await?;
    let mut tx = state.pool.begin().await?;
    lock_workflow(&mut tx, &actor, workflow).await?;
    let row = sqlx::query(
        "SELECT * FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(workflow)
    .bind(version)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Workflow version"))?;
    if row.try_get::<bool, _>("archived")? {
        return Err(ApiError::conflict(
            "VERSION_ARCHIVED",
            "This version is archived",
        ));
    }
    let draft = if let Some(draft) = row.try_get::<Option<Uuid>, _>("working_draft_id")? {
        draft
    } else {
        let draft = Uuid::now_v7();
        sqlx::query("INSERT INTO workflow_drafts(id,tenant_id,workflow_id,base_version_id,created_by,schema_version,definition_json,editor_json,content_hash,editor_hash,updated_by) SELECT ?,tenant_id,workflow_id,id,?,schema_version,definition_json,editor_json,content_hash,editor_hash,? FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND id=?").bind(draft).bind(actor.user_id).bind(actor.user_id).bind(actor.tenant_id).bind(workflow).bind(version).execute(&mut *tx).await?;
        crate::workflow_drafts::record_initial_revision(&mut tx, &actor, workflow, draft).await?;
        let resources = frozen_resources(&mut tx, actor.tenant_id, version).await?;
        sqlx::query("UPDATE workflow_draft_revisions SET resource_snapshot_json=? WHERE tenant_id=? AND draft_id=? AND revision=0").bind(resources).bind(actor.tenant_id).bind(draft).execute(&mut *tx).await?;
        sqlx::query("UPDATE workflow_versions SET working_draft_id=? WHERE tenant_id=? AND id=?")
            .bind(draft)
            .bind(actor.tenant_id)
            .bind(version)
            .execute(&mut *tx)
            .await?;
        let definition =
            serde_json::from_value(row.try_get("definition_json")?).map_err(ApiError::internal)?;
        crate::workflow_resources::replace_draft_resources(
            &mut tx,
            actor.tenant_id,
            workflow,
            draft,
            &definition,
        )
        .await?;
        draft
    };
    tx.commit().await?;
    Ok(Json(
        json!({"version":load(&state,&actor,workflow,version).await?,"document":load_draft(&state,actor.tenant_id,workflow,draft).await?,"restoreTarget":if row.try_get::<String,_>("publication_status")?=="published" {json!({"kind":"version","versionNumber":row.try_get::<u64,_>("version_number")?})}else{json!({"kind":"initial"})}}),
    ))
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoreRequest {
    pub expected_revision: u64,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ArchiveVersionRequest {
    pub archived: bool,
}

/// Lifecycle changes invalidate old editor sessions without deleting their history.
pub(crate) async fn invalidate_editors(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    draft: Uuid,
) -> ApiResult<()> {
    sqlx::query("UPDATE studio_builder_runs r JOIN studio_builder_sessions s ON s.tenant_id=r.tenant_id AND s.id=r.session_id SET r.status='cancelled',r.completed_at=UTC_TIMESTAMP(6),r.lease_owner=NULL,r.lease_until=NULL,r.fencing_token=r.fencing_token+1 WHERE s.tenant_id=? AND s.draft_id=? AND r.status IN ('queued','running','waiting','outcome_unknown')").bind(tenant).bind(draft).execute(&mut **tx).await?;
    sqlx::query("UPDATE studio_builder_effects e JOIN studio_builder_sessions s ON s.tenant_id=e.tenant_id AND s.id=e.session_id SET e.status='cancelled' WHERE s.tenant_id=? AND s.draft_id=? AND e.status='pending'").bind(tenant).bind(draft).execute(&mut **tx).await?;
    sqlx::query("UPDATE workflow_drafts SET generation=generation+1 WHERE tenant_id=? AND id=?")
        .bind(tenant)
        .bind(draft)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM workflow_debug_overlays WHERE tenant_id=? AND draft_id=?")
        .bind(tenant)
        .bind(draft)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn append_snapshot(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    workflow: Uuid,
    draft: Uuid,
    source: &sqlx::mysql::MySqlRow,
    resources: Option<&Value>,
) -> ApiResult<()> {
    let definition: Value = source.try_get("definition_json")?;
    let editor: Value = source.try_get("editor_json")?;
    let resources = resources
        .cloned()
        .unwrap_or(source.try_get::<Value, _>("resource_snapshot_json")?);
    sqlx::query("UPDATE workflow_drafts SET revision=revision+1,definition_json=?,editor_json=?,content_hash=?,editor_hash=?,updated_by=? WHERE tenant_id=? AND workflow_id=? AND id=?").bind(&definition).bind(&editor).bind(source.try_get::<String,_>("content_hash")?).bind(source.try_get::<String,_>("editor_hash")?).bind(actor.user_id).bind(actor.tenant_id).bind(workflow).bind(draft).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO workflow_draft_revisions(id,tenant_id,workflow_id,draft_id,revision,schema_version,definition_json,editor_json,content_hash,editor_hash,resource_snapshot_json,created_by) SELECT ?,tenant_id,workflow_id,id,revision,schema_version,definition_json,editor_json,content_hash,editor_hash,?,? FROM workflow_drafts WHERE tenant_id=? AND id=?").bind(Uuid::now_v7()).bind(resources).bind(actor.user_id).bind(actor.tenant_id).bind(draft).execute(&mut **tx).await?;
    let parsed = serde_json::from_value(definition).map_err(ApiError::internal)?;
    crate::workflow_resources::replace_draft_resources(
        tx,
        actor.tenant_id,
        workflow,
        draft,
        &parsed,
    )
    .await?;
    sync_unpublished(tx, actor, workflow, draft).await?;
    invalidate_editors(tx, actor.tenant_id, draft).await
}

pub(crate) async fn sync_unpublished(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    workflow: Uuid,
    draft: Uuid,
) -> ApiResult<()> {
    sqlx::query("UPDATE workflow_versions v JOIN workflow_drafts d ON d.tenant_id=v.tenant_id AND d.id=v.working_draft_id JOIN workflow_draft_revisions r ON r.tenant_id=d.tenant_id AND r.draft_id=d.id AND r.revision=d.revision SET v.source_revision_id=r.id,v.source_revision=r.revision,v.definition_json=r.definition_json,v.editor_json=r.editor_json,v.content_hash=r.content_hash,v.editor_hash=r.editor_hash,v.updated_by=?,v.updated_at=UTC_TIMESTAMP(6) WHERE v.tenant_id=? AND v.workflow_id=? AND v.working_draft_id=? AND v.publication_status='unpublished'").bind(actor.user_id).bind(actor.tenant_id).bind(workflow).bind(draft).execute(&mut **tx).await?;
    sqlx::query("UPDATE workflow_versions SET updated_by=?,updated_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND workflow_id=? AND working_draft_id=?").bind(actor.user_id).bind(actor.tenant_id).bind(workflow).bind(draft).execute(&mut **tx).await?;
    sqlx::query("UPDATE workflows SET updated_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(workflow)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn restore(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((workflow, version)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<RestoreRequest>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:edit")?;
    require_workflow_access(&state, &actor, workflow, true).await?;
    let mut tx = state.pool.begin().await?;
    lock_workflow(&mut tx, &actor, workflow).await?;
    let operation = format!("restore:{version}");
    let request = json!(input);
    if let Some(value) = replay(&mut tx, &actor, &operation, &headers, &request).await? {
        tx.commit().await?;
        return Ok(Json(value));
    }
    let row=sqlx::query("SELECT v.publication_status,v.archived,v.working_draft_id,d.revision FROM workflow_versions v LEFT JOIN workflow_drafts d ON d.tenant_id=v.tenant_id AND d.id=v.working_draft_id WHERE v.tenant_id=? AND v.workflow_id=? AND v.id=? FOR UPDATE").bind(actor.tenant_id).bind(workflow).bind(version).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Workflow version"))?;
    if row.try_get::<bool, _>("archived")? {
        return Err(ApiError::conflict(
            "VERSION_ARCHIVED",
            "This version is archived",
        ));
    }
    let draft = row
        .try_get::<Option<Uuid>, _>("working_draft_id")?
        .ok_or_else(|| ApiError::conflict("EDITOR_REQUIRED", "Open the editor before restoring"))?;
    if row.try_get::<u64, _>("revision")? != input.expected_revision {
        return Err(ApiError::conflict(
            "DRAFT_REVISION_CONFLICT",
            "The document changed; reload before restoring",
        ));
    }
    let source = if row.try_get::<String, _>("publication_status")? == "published" {
        sqlx::query("SELECT r.* FROM workflow_versions v JOIN workflow_draft_revisions r ON r.tenant_id=v.tenant_id AND r.id=v.source_revision_id WHERE v.tenant_id=? AND v.id=?").bind(actor.tenant_id).bind(version).fetch_one(&mut *tx).await?
    } else {
        sqlx::query("SELECT * FROM workflow_draft_revisions WHERE tenant_id=? AND draft_id=? AND revision=0").bind(actor.tenant_id).bind(draft).fetch_one(&mut *tx).await?
    };
    let resources = if row.try_get::<String, _>("publication_status")? == "published" {
        Some(frozen_resources(&mut tx, actor.tenant_id, version).await?)
    } else {
        None
    };
    append_snapshot(
        &mut tx,
        &actor,
        workflow,
        draft,
        &source,
        resources.as_ref(),
    )
    .await?;
    let result = json!({"revision":input.expected_revision+1,"versionId":version});
    receipt(&mut tx, &actor, &operation, &headers, &request, &result).await?;
    audit(
        &mut tx,
        &actor,
        "workflow.canvas_restored",
        "workflow",
        workflow,
        result.clone(),
    )
    .await?;
    tx.commit().await?;
    cancel_expired_models(&state, actor.tenant_id, workflow).await;
    Ok(Json(result))
}

async fn archive(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((workflow, version)): Path<(Uuid, Uuid)>,
    Json(input): Json<ArchiveVersionRequest>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:archive")?;
    require_workflow_access(&state, &actor, workflow, true).await?;
    let mut tx = state.pool.begin().await?;
    lock_workflow(&mut tx, &actor, workflow).await?;
    let row=sqlx::query("SELECT working_draft_id,archived FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND id=? FOR UPDATE").bind(actor.tenant_id).bind(workflow).bind(version).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Workflow version"))?;
    if input.archived != row.try_get::<bool, _>("archived")? {
        if let Some(draft) = row.try_get::<Option<Uuid>, _>("working_draft_id")? {
            let source=sqlx::query("SELECT r.* FROM workflow_draft_revisions r JOIN workflow_drafts d ON d.tenant_id=r.tenant_id AND d.id=r.draft_id AND d.revision=r.revision WHERE d.tenant_id=? AND d.id=?").bind(actor.tenant_id).bind(draft).fetch_one(&mut *tx).await?;
            append_snapshot(&mut tx, &actor, workflow, draft, &source, None).await?;
        }
        sqlx::query("UPDATE workflow_versions SET archived=?,updated_by=?,updated_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=?").bind(input.archived).bind(actor.user_id).bind(actor.tenant_id).bind(version).execute(&mut *tx).await?;
        audit(
            &mut tx,
            &actor,
            "workflow.version_archived",
            "workflow",
            workflow,
            json!({"versionId":version,"archived":input.archived}),
        )
        .await?;
    }
    tx.commit().await?;
    cancel_expired_models(&state, actor.tenant_id, workflow).await;
    Ok(Json(json!({"archived":input.archived})))
}

/// Read the publication facts, which can be more complete than a saved draft revision.
pub(crate) async fn frozen_resources(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    version: Uuid,
) -> ApiResult<Value> {
    let rows=sqlx::query("SELECT * FROM workflow_version_resources WHERE tenant_id=? AND workflow_version_id=? ORDER BY node_id,resource_type,resource_id,operation_key").bind(tenant).bind(version).fetch_all(&mut **tx).await?;
    let snapshots = rows
        .into_iter()
        .map(crate::workflow_revision_resources::snapshot_from_row)
        .collect::<ApiResult<Vec<_>>>()?;
    serde_json::to_value(snapshots).map_err(ApiError::internal)
}

/// The transactional fence is authoritative; stop in-flight inference after commit as well.
pub(crate) async fn cancel_expired_models(state: &ControlApiState, tenant: Uuid, workflow: Uuid) {
    let result:ApiResult<()>=async {
        let rows=sqlx::query("SELECT s.id session_id,s.draft_id,s.user_id,s.state_json,r.id run_id FROM studio_builder_sessions s JOIN workflow_drafts d ON d.tenant_id=s.tenant_id AND d.id=s.draft_id JOIN studio_builder_runs r ON r.tenant_id=s.tenant_id AND r.session_id=s.id WHERE s.tenant_id=? AND s.workflow_id=? AND s.document_generation<>d.generation AND r.status='cancelled'").bind(tenant).bind(workflow).fetch_all(&state.pool).await?;
        for row in rows {
            let run:Uuid=row.try_get("run_id")?;
            let core:agentx_agent_core::AgentSessionStateV1=serde_json::from_value(row.try_get("state_json")?).map_err(ApiError::internal)?;
            let target=agentx_runtime_contracts::StudioBuilderTargetV1{tenant_id:tenant,workflow_id:workflow,draft_id:row.try_get("draft_id")?,session_id:row.try_get("session_id")?,run_id:run,user_id:row.try_get("user_id")?};
            if let Some(operation)=core.operation.filter(|operation|operation.operation_id.starts_with(&run.to_string())&&matches!(operation.operation_kind,agentx_agent_core::OperationKindV1::Model|agentx_agent_core::OperationKindV1::Compaction)) {
                let effect=agentx_runtime_contracts::deterministic_uuid(run,operation.effect_id.as_bytes());
                let _=crate::studio_builder_runtime::cancel(state,&target,effect).await;
            }
            crate::studio_builder_runtime::append_entry(state,&target,&format!("terminal:{run}"),json!({"type":"terminal","status":"cancelled"})).await?;
        }
        Ok(())
    }.await;
    if let Err(error) = result {
        tracing::warn!(%workflow,?error,"Fenced editor inference cancellation could not be delivered");
    }
}
