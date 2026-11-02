//! Application page authoring. Published business execution remains exclusively in Runtime.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};
use agentx_agent_core::AgentSessionStateV1;
use agentx_runtime_contracts::{
    ApplicationPageContractV1, ApplicationPageDraftV1, ApplicationUiBuilderTargetV1,
    ExecutionSpecPayloadV2, PageEditorSnapshotV1, content_hash,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

#[path = "application_ui_editor.rs"]
mod editor;
pub(crate) use editor::{Acknowledgement, CreateRun};

pub(crate) fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/applications/{application}/ui/drafts",
            get(list).post(create),
        )
        .route(
            "/api/v1/applications/{application}/ui/drafts/{draft}",
            get(get_draft).put(save),
        )
        .route(
            "/api/v1/applications/{application}/ui/drafts/{draft}/builder-sessions",
            post(create_session),
        )
        .route(
            "/api/v1/application-ui/sessions/{session}",
            get(editor::get_session),
        )
        .route(
            "/api/v1/application-ui/sessions/{session}/snapshot",
            axum::routing::put(editor::snapshot),
        )
        .route(
            "/api/v1/application-ui/sessions/{session}/runs",
            post(editor::create_run),
        )
        .route(
            "/api/v1/application-ui/sessions/{session}/acknowledgements",
            post(editor::acknowledge),
        )
}

pub(crate) fn apply_openapi(document: &mut Value) {
    let operations = [
        (
            "/api/v1/applications/{application}/ui/drafts",
            "get",
            None,
            Some("ApplicationPageDraftV1"),
            true,
        ),
        (
            "/api/v1/applications/{application}/ui/drafts",
            "post",
            Some("CreatePageDraft"),
            Some("ApplicationPageDraftV1"),
            false,
        ),
        (
            "/api/v1/applications/{application}/ui/drafts/{draft}",
            "get",
            None,
            Some("ApplicationPageDraftV1"),
            false,
        ),
        (
            "/api/v1/applications/{application}/ui/drafts/{draft}",
            "put",
            Some("SavePageDraft"),
            Some("SavePageReceipt"),
            false,
        ),
        (
            "/api/v1/applications/{application}/ui/drafts/{draft}/builder-sessions",
            "post",
            Some("CreatePageBuilderSession"),
            Some("PageBuilderSessionV1"),
            false,
        ),
        (
            "/api/v1/application-ui/sessions/{session}",
            "get",
            None,
            Some("PageBuilderSessionV1"),
            false,
        ),
        (
            "/api/v1/application-ui/sessions/{session}/snapshot",
            "put",
            Some("PageEditorSnapshotV1"),
            None,
            false,
        ),
        (
            "/api/v1/application-ui/sessions/{session}/runs",
            "post",
            Some("CreatePageBuilderRun"),
            None,
            false,
        ),
        (
            "/api/v1/application-ui/sessions/{session}/acknowledgements",
            "post",
            Some("PageBuilderAcknowledgement"),
            None,
            false,
        ),
    ];
    for (path, method, input, output, array) in operations {
        let parameters=path.split('/').filter_map(|segment|segment.strip_prefix('{').and_then(|segment|segment.strip_suffix('}'))).map(|name|json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}})).collect::<Vec<_>>();
        let schema = output.map_or(
            json!({}),
            |name| json!({"$ref":format!("#/components/schemas/{name}")}),
        );
        let schema = if array {
            json!({"type":"array","items":schema})
        } else {
            schema
        };
        let mut operation = json!({"parameters":parameters,"responses":{"200":{"description":"Application page authoring result","content":{"application/json":{"schema":schema}}},"403":{"description":"Permission denied"},"409":{"description":"Stale binding, revision or request identity"}},"security":[{"bearerAuth":[]}]});
        if let Some(name) = input {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{name}")}}}});
        }
        document["paths"][path][method] = operation;
    }
}

pub(crate) async fn authorize(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
) -> ApiResult<()> {
    crate::application_testing::authorize(state, actor, application, "application:manage").await?;
    Ok(())
}

pub(crate) async fn authorize_session(
    state: &ControlApiState,
    actor: &Actor,
    row: &sqlx::mysql::MySqlRow,
    write: bool,
) -> ApiResult<()> {
    authorize(state, actor, row.try_get("application_id")?).await?;
    if row.try_get::<String, _>("target_kind")? != "application_ui_builder" {
        return Err(ApiError::forbidden(
            "This session is not an application page",
        ));
    }
    if write {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM application_ui_drafts d JOIN applications a ON a.tenant_id=d.tenant_id AND a.id=d.application_id WHERE d.tenant_id=? AND d.id=? AND d.application_id=? AND d.bundle_id=? AND d.contract_hash=? AND a.status='active')")
            .bind(actor.tenant_id).bind(row.try_get::<Uuid,_>("ui_draft_id")?).bind(row.try_get::<Uuid,_>("application_id")?).bind(row.try_get::<Uuid,_>("bundle_id")?).bind(row.try_get::<String,_>("contract_hash")?).fetch_one(&state.pool).await?;
        if !valid {
            return Err(ApiError::conflict(
                "PAGE_BINDING_STALE",
                "The page draft or its frozen contract is no longer active",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn authorize_target(
    state: &ControlApiState,
    actor: &Actor,
    target: &ApplicationUiBuilderTargetV1,
) -> ApiResult<()> {
    authorize(state, actor, target.application_id).await?;
    let row=sqlx::query("SELECT * FROM design_agent_sessions WHERE tenant_id=? AND id=? AND user_id=? AND target_kind='application_ui_builder' AND application_id=? AND ui_draft_id=? AND bundle_id=? AND contract_hash=?")
        .bind(actor.tenant_id).bind(target.session_id).bind(actor.user_id).bind(target.application_id).bind(target.ui_draft_id).bind(target.bundle_id).bind(target.contract_hash.as_str()).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::forbidden("Page target does not match the current editor binding"))?;
    authorize_session(state, actor, &row, true).await
}

pub(crate) async fn lock_binding(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    row: &sqlx::mysql::MySqlRow,
) -> ApiResult<()> {
    sqlx::query("SELECT id FROM application_ui_drafts WHERE tenant_id=? AND application_id=? AND id=? AND bundle_id=? AND contract_hash=? FOR UPDATE")
        .bind(actor.tenant_id).bind(row.try_get::<Uuid,_>("application_id")?).bind(row.try_get::<Uuid,_>("ui_draft_id")?).bind(row.try_get::<Uuid,_>("bundle_id")?).bind(row.try_get::<String,_>("contract_hash")?).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::conflict("PAGE_BINDING_STALE","Page contract changed"))?;
    Ok(())
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateDraft {
    pub deployment_id: Uuid,
}

async fn create(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
    Json(input): Json<CreateDraft>,
) -> ApiResult<Json<ApplicationPageDraftV1>> {
    authorize(&state, &actor, application).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "SELECT id FROM applications WHERE tenant_id=? AND id=? AND status='active' FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(application)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Active application"))?;
    if let Some(id) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM application_ui_drafts WHERE tenant_id=? AND application_id=? ORDER BY created_at,id LIMIT 1")
        .bind(actor.tenant_id).bind(application).fetch_optional(&mut *tx).await? {
        tx.commit().await?;
        return Ok(Json(load(&state, actor.tenant_id, application, id).await?));
    }
    let row=sqlx::query("SELECT a.name,a.description,b.payload_json FROM application_deployments d JOIN applications a ON a.tenant_id=d.tenant_id AND a.id=d.application_id JOIN execution_spec_bundles b ON b.tenant_id=d.tenant_id AND b.deployment_id=d.id WHERE d.tenant_id=? AND d.application_id=? AND d.id=? AND d.status IN ('active','superseded') AND b.status='published'")
        .bind(actor.tenant_id).bind(application).bind(input.deployment_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::conflict("PAGE_DEPLOYMENT_REQUIRED","Select a published application deployment"))?;
    let bundle: ExecutionSpecPayloadV2 =
        serde_json::from_value(row.try_get("payload_json")?).map_err(ApiError::internal)?;
    let definition =
        serde_json::from_value(bundle.definition.clone()).map_err(ApiError::internal)?;
    let waits =
        crate::workflow_wait_inputs::discover(&state.pool, actor.tenant_id, &definition).await?;
    let mapping:Option<Value>=sqlx::query_scalar("SELECT mapping_json FROM application_playground_configs WHERE tenant_id=? AND deployment_id=? AND publish_status='active' AND published_version=version")
        .bind(actor.tenant_id).bind(input.deployment_id).fetch_optional(&state.pool).await?.flatten();
    let approval_stages=bundle.compiled_ir.nodes.iter().filter_map(|node|{
        let kind=if node.node_type=="approval" {agentx_runtime_contracts::ApplicationApprovalKindV1::WorkflowNode}
        else if node.node_type=="agent" && bundle.resources.iter().any(|binding|matches!(&binding.configuration,agentx_runtime_contracts::RuntimeResourceConfigurationV1::Mcp{tool_policy:Some(policy),..} if policy.approval.is_some())) {agentx_runtime_contracts::ApplicationApprovalKindV1::AgentToolEffect}
        else {return None};
        Some(agentx_runtime_contracts::ApplicationApprovalStageV1{node_id:node.id.clone(),title:node.name.clone(),kind})
    }).collect();
    let contract = ApplicationPageContractV1 {
        application_id: application,
        application_name: row.try_get("name")?,
        description: row.try_get("description")?,
        deployment_id: bundle.deployment_id,
        workflow_version_id: bundle.workflow_version_id,
        bundle_id: bundle.bundle_id,
        input_schema: bundle.input_contract,
        output_schema: bundle.output_contract,
        wait_input_nodes: waits,
        approval_stages,
        chat_mapping: mapping
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?,
        sdk_version: agentx_runtime_contracts::APPLICATION_PAGE_SDK_VERSION,
        max_upload_bytes: 50 * 1024 * 1024,
    };
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO application_ui_drafts(id,tenant_id,application_id,bundle_id,contract_json,contract_hash,html,html_hash,created_by) VALUES(?,?,?,?,?,?,?, ?,?)")
        .bind(id).bind(actor.tenant_id).bind(application).bind(contract.bundle_id).bind(serde_json::to_value(&contract).map_err(ApiError::internal)?).bind(content_hash(&contract).map_err(ApiError::internal)?.as_str()).bind("").bind(content_hash(&"").map_err(ApiError::internal)?.as_str()).bind(actor.user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(load(&state, actor.tenant_id, application, id).await?))
}

async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
) -> ApiResult<Json<Vec<ApplicationPageDraftV1>>> {
    authorize(&state, &actor, application).await?;
    let rows=sqlx::query("SELECT * FROM application_ui_drafts WHERE tenant_id=? AND application_id=? ORDER BY created_at,id LIMIT 1").bind(actor.tenant_id).bind(application).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter().map(from_row).collect::<ApiResult<_>>()?,
    ))
}

async fn get_draft(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, draft)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApplicationPageDraftV1>> {
    authorize(&state, &actor, application).await?;
    Ok(Json(
        load(&state, actor.tenant_id, application, draft).await?,
    ))
}

pub(crate) async fn load(
    state: &ControlApiState,
    tenant: Uuid,
    application: Uuid,
    draft: Uuid,
) -> ApiResult<ApplicationPageDraftV1> {
    let row = sqlx::query(
        "SELECT * FROM application_ui_drafts WHERE tenant_id=? AND application_id=? AND id=?",
    )
    .bind(tenant)
    .bind(application)
    .bind(draft)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Application page draft"))?;
    from_row(row)
}

fn from_row(row: sqlx::mysql::MySqlRow) -> ApiResult<ApplicationPageDraftV1> {
    Ok(ApplicationPageDraftV1 {
        id: row.try_get("id")?,
        application_id: row.try_get("application_id")?,
        contract: serde_json::from_value(row.try_get("contract_json")?)
            .map_err(ApiError::internal)?,
        contract_hash: serde_json::from_value(json!(row.try_get::<String, _>("contract_hash")?))
            .map_err(ApiError::internal)?,
        html: row.try_get("html")?,
        html_hash: serde_json::from_value(json!(row.try_get::<String, _>("html_hash")?))
            .map_err(ApiError::internal)?,
        revision: row.try_get("revision")?,
    })
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveDraft {
    pub save_id: Uuid,
    pub base_revision: u64,
    pub html: String,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveReceipt {
    pub revision: u64,
    pub html_hash: agentx_runtime_contracts::ContentHash,
}

async fn save(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, draft)): Path<(Uuid, Uuid)>,
    Json(input): Json<SaveDraft>,
) -> ApiResult<Json<SaveReceipt>> {
    authorize(&state, &actor, application).await?;
    validate_html_size(&input.html)?;
    let hash = content_hash(&input.html).map_err(ApiError::internal)?;
    let mut tx = state.pool.begin().await?;
    let row=sqlx::query("SELECT revision FROM application_ui_drafts WHERE tenant_id=? AND application_id=? AND id=? FOR UPDATE").bind(actor.tenant_id).bind(application).bind(draft).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Application page draft"))?;
    if let Some(saved)=sqlx::query("SELECT ui_draft_id,base_revision,html_hash,result_revision FROM application_ui_saves WHERE tenant_id=? AND id=?").bind(actor.tenant_id).bind(input.save_id).fetch_optional(&mut *tx).await? {
        if saved.try_get::<Uuid,_>("ui_draft_id")?!=draft||saved.try_get::<u64,_>("base_revision")?!=input.base_revision||saved.try_get::<String,_>("html_hash")?!=hash.as_str() {return Err(ApiError::conflict("PAGE_SAVE_CONFLICT","Save ID was reused for different content"));}
        return Ok(Json(SaveReceipt{revision:saved.try_get("result_revision")?,html_hash:hash}));
    }
    if row.try_get::<u64, _>("revision")? != input.base_revision {
        return Err(ApiError::conflict(
            "PAGE_REVISION_CONFLICT",
            "The page changed; reload before saving",
        ));
    }
    let revision = input.base_revision + 1;
    sqlx::query(
        "UPDATE application_ui_drafts SET html=?,html_hash=?,revision=? WHERE tenant_id=? AND id=?",
    )
    .bind(&input.html)
    .bind(hash.as_str())
    .bind(revision)
    .bind(actor.tenant_id)
    .bind(draft)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO application_ui_saves(id,tenant_id,ui_draft_id,base_revision,html_hash,result_revision) VALUES(?,?,?,?,?,?)").bind(input.save_id).bind(actor.tenant_id).bind(draft).bind(input.base_revision).bind(hash.as_str()).bind(revision).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(SaveReceipt {
        revision,
        html_hash: hash,
    }))
}

pub(crate) fn validate_html_size(html: &str) -> ApiResult<()> {
    if html.len() > agentx_runtime_contracts::APPLICATION_PAGE_MAX_BYTES {
        return Err(ApiError::bad_request(
            "PAGE_TOO_LARGE",
            "HTML exceeds the 1 MiB page limit",
        ));
    }
    Ok(())
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateSession {
    pub snapshot: PageEditorSnapshotV1,
}

pub(crate) fn verify_snapshot(snapshot: &PageEditorSnapshotV1) -> ApiResult<()> {
    validate_html_size(&snapshot.html)?;
    if content_hash(&snapshot.html).map_err(ApiError::internal)? != snapshot.html_hash {
        return Err(ApiError::conflict(
            "PAGE_HASH_MISMATCH",
            "The page snapshot hash does not match its content",
        ));
    }
    Ok(())
}

async fn create_session(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, draft)): Path<(Uuid, Uuid)>,
    Json(input): Json<CreateSession>,
) -> ApiResult<Json<agentx_runtime_contracts::PageBuilderSessionV1>> {
    authorize(&state, &actor, application).await?;
    verify_snapshot(&input.snapshot)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM application_ui_drafts WHERE tenant_id=? AND application_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id).bind(application).bind(draft).fetch_optional(&mut *tx).await?
        .ok_or_else(||ApiError::not_found("Application page"))?;
    let page = load(&state, actor.tenant_id, application, draft).await?;
    if page.revision != input.snapshot.server_revision {
        return Err(ApiError::conflict(
            "PAGE_REVISION_CONFLICT",
            "Reload the page before binding an editor",
        ));
    }
    if let Some(row)=sqlx::query("SELECT id,editor_instance_id,snapshot_json FROM design_agent_sessions WHERE tenant_id=? AND ui_draft_id=? AND user_id=? ORDER BY created_at,id LIMIT 1 FOR UPDATE")
        .bind(actor.tenant_id).bind(draft).bind(actor.user_id).fetch_optional(&mut *tx).await? {
        let session:Uuid=row.try_get("id")?;
        if row.try_get::<Uuid,_>("editor_instance_id")? != input.snapshot.editor_instance_id {
            let previous:PageEditorSnapshotV1=serde_json::from_value(row.try_get("snapshot_json")?).map_err(ApiError::internal)?;
            let mut snapshot=input.snapshot;
            snapshot.html=page.html;
            snapshot.html_hash=page.html_hash;
            snapshot.local_revision=previous.local_revision.max(snapshot.local_revision)+1;
            snapshot.server_revision=page.revision+1;
            snapshot.save_status=agentx_runtime_contracts::StudioSaveStatusV1::Saved;
            // Fence writes from the former window without changing the HTML.
            sqlx::query("UPDATE application_ui_drafts SET revision=revision+1 WHERE tenant_id=? AND id=?")
                .bind(actor.tenant_id).bind(draft).execute(&mut *tx).await?;
            sqlx::query("UPDATE design_agent_sessions SET editor_instance_id=?,snapshot_json=? WHERE tenant_id=? AND id=?")
                .bind(snapshot.editor_instance_id).bind(serde_json::to_value(snapshot).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(session).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        return editor::get_session(State(state), actor, Path(session)).await;
    }
    let id = Uuid::now_v7();
    let core = AgentSessionStateV1 {
        session_id: id.to_string(),
        ..Default::default()
    };
    sqlx::query("INSERT INTO design_agent_sessions(id,tenant_id,target_kind,application_id,ui_draft_id,bundle_id,contract_hash,user_id,editor_instance_id,snapshot_json,state_json) VALUES(?,?,'application_ui_builder',?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id")
        .bind(id).bind(actor.tenant_id).bind(application).bind(draft).bind(page.contract.bundle_id).bind(page.contract_hash.as_str()).bind(actor.user_id).bind(input.snapshot.editor_instance_id).bind(serde_json::to_value(&input.snapshot).map_err(ApiError::internal)?).bind(serde_json::to_value(core).map_err(ApiError::internal)?).execute(&mut *tx).await?;
    tx.commit().await?;
    let session:Uuid=sqlx::query_scalar("SELECT id FROM design_agent_sessions WHERE tenant_id=? AND ui_draft_id=? AND user_id=? AND editor_instance_id=?").bind(actor.tenant_id).bind(draft).bind(actor.user_id).bind(input.snapshot.editor_instance_id).fetch_one(&state.pool).await?;
    editor::get_session(State(state), actor, Path(session)).await
}
