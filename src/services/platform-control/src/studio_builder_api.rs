use agentx_agent_core::{AgentSessionStateV1, ToolResultV1};
use agentx_domain::canonical_content_hash;
use agentx_runtime_contracts::{
    StudioAcknowledgementStatusV1, StudioBuilderTargetV1, StudioEditorAcknowledgementV1,
    StudioEditorCommandV1, StudioEditorSnapshotV1,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    studio_builder_runtime::append_entry,
};

pub(crate) fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/workflows/{workflow}/drafts/{draft}/builder-sessions",
            axum::routing::post(create_session),
        )
        .route(
            "/api/v1/studio-builder/sessions/{session}",
            get(get_session),
        )
        .route(
            "/api/v1/studio-builder/sessions/{session}/snapshot",
            axum::routing::put(sync_snapshot),
        )
        .route(
            "/api/v1/studio-builder/sessions/{session}/runs",
            axum::routing::post(create_run),
        )
        .route(
            "/api/v1/studio-builder/sessions/{session}/entries",
            get(entries),
        )
        .route(
            "/api/v1/studio-builder/sessions/{session}/events",
            get(events),
        )
        .route(
            "/api/v1/studio-builder/sessions/{session}/acknowledgements",
            axum::routing::post(acknowledge),
        )
        .route(
            "/api/v1/studio-builder/sessions/{session}/commands/{command}/receipt",
            get(command_receipt),
        )
        .route(
            "/api/v1/studio-builder/sessions/{session}/answers",
            axum::routing::post(answer),
        )
        .route(
            "/api/v1/studio-builder/sessions/{session}/runs/{run}/cancel",
            axum::routing::post(cancel_run),
        )
}

#[derive(serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandReceipt {
    status: CommandReceiptStatus,
}

#[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum CommandReceiptStatus {
    Pending,
    Settled,
    Cancelled,
}

async fn command_receipt(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((session, command)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<CommandReceipt>> {
    session_row(&state, &actor, session, false).await?;
    let row=sqlx::query("SELECT status FROM studio_builder_effects WHERE tenant_id=? AND session_id=? AND id=? AND tool_name='workflow_edit'").bind(actor.tenant_id).bind(session).bind(command).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Editor command"))?;
    let status = serde_json::from_value(Value::String(row.try_get("status")?))
        .map_err(ApiError::internal)?;
    Ok(Json(CommandReceipt { status }))
}

#[derive(Clone, Copy)]
pub(crate) enum DesignRevocation {
    Setting(u64),
    Model(Uuid, u64),
    Credential(Uuid, u64),
}

pub(crate) async fn revoke_runs(
    state: &ControlApiState,
    tenant: Uuid,
    revocation: DesignRevocation,
) -> ApiResult<()> {
    let (setting, model, model_epoch, credential, credential_epoch) = match revocation {
        DesignRevocation::Setting(version) => (Some(version), None, 0, None, 0),
        DesignRevocation::Model(id, version) => (None, Some(id), version, None, 0),
        DesignRevocation::Credential(id, version) => (None, None, 0, Some(id), version),
    };
    let mut tx = state.pool.begin().await?;
    let rows=sqlx::query("SELECT r.id,s.id session_id,s.workflow_id,s.draft_id,s.user_id,s.state_json FROM studio_builder_runs r JOIN studio_builder_sessions s ON s.tenant_id=r.tenant_id AND s.id=r.session_id WHERE r.tenant_id=? AND r.status IN ('queued','running','waiting') AND (? IS NULL OR r.setting_version<=?) AND (? IS NULL OR (r.model_alias_id=? AND r.model_alias_version<=?)) AND (? IS NULL OR (r.credential_id=? AND r.credential_version<=?)) FOR UPDATE").bind(tenant).bind(setting).bind(setting).bind(model).bind(model).bind(model_epoch).bind(credential).bind(credential).bind(credential_epoch).fetch_all(&mut *tx).await?;
    for row in &rows {
        let run: Uuid = row.try_get("id")?;
        sqlx::query("UPDATE studio_builder_runs SET status='cancelled',completed_at=UTC_TIMESTAMP(6),lease_owner=NULL,lease_until=NULL WHERE tenant_id=? AND id=?").bind(tenant).bind(run).execute(&mut *tx).await?;
        sqlx::query("UPDATE studio_builder_effects SET status='cancelled' WHERE tenant_id=? AND run_id=? AND status='pending'").bind(tenant).bind(run).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    for row in rows {
        let target = StudioBuilderTargetV1 {
            tenant_id: tenant,
            run_id: row.try_get("id")?,
            session_id: row.try_get("session_id")?,
            workflow_id: row.try_get("workflow_id")?,
            draft_id: row.try_get("draft_id")?,
            user_id: row.try_get("user_id")?,
        };
        let core: AgentSessionStateV1 =
            serde_json::from_value(row.try_get("state_json")?).map_err(ApiError::internal)?;
        if let Some(operation) = core.operation.filter(|operation| {
            operation
                .operation_id
                .starts_with(&target.run_id.to_string())
                && matches!(
                    operation.operation_kind,
                    agentx_agent_core::OperationKindV1::Model
                        | agentx_agent_core::OperationKindV1::Compaction
                )
        }) {
            let effect = agentx_runtime_contracts::deterministic_uuid(
                target.run_id,
                operation.effect_id.as_bytes(),
            );
            let _ = crate::studio_builder_runtime::cancel(state, &target, effect).await;
        }
        append_entry(
            state,
            &target,
            &format!("terminal:{}", target.run_id),
            json!({"type":"terminal","status":"cancelled","reason":"model_authorization_revoked"}),
        )
        .await?;
    }
    Ok(())
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateBuilderSession {
    pub snapshot: StudioEditorSnapshotV1,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateBuilderRun {
    pub run_id: Uuid,
    pub prompt: String,
    pub snapshot: StudioEditorSnapshotV1,
    pub edit_scope: agentx_runtime_contracts::StudioEditScopeV1,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Answer {
    question_id: Uuid,
    editor_instance_id: Uuid,
    answer: String,
}
#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    after: Option<u64>,
}

pub(crate) fn document_hash(snapshot: &StudioEditorSnapshotV1) -> ApiResult<String> {
    canonical_content_hash(&serde_json::to_value(&snapshot.document).map_err(ApiError::internal)?)
        .map_err(ApiError::internal)
}
pub(crate) fn verify_snapshot(snapshot: &StudioEditorSnapshotV1) -> ApiResult<()> {
    if document_hash(snapshot)? != snapshot.document_hash {
        return Err(ApiError::conflict(
            "EDITOR_DOCUMENT_HASH_MISMATCH",
            "Document hash does not match its content",
        ));
    }
    if snapshot.selected_node_ids.iter().any(|id| {
        !snapshot
            .document
            .definition
            .nodes
            .iter()
            .any(|node| &node.id == id)
    }) {
        return Err(ApiError::bad_request(
            "EDITOR_SELECTION_INVALID",
            "A selected node is missing",
        ));
    }
    // Typed, bounded snapshots may contain incomplete or invalid design fields. The agent can read and repair them;
    // only a validated edit command can be committed to the local document.
    if serde_json::to_vec(&snapshot.document)
        .map_err(ApiError::internal)?
        .len()
        > 4 * 1024 * 1024
    {
        return Err(ApiError::bad_request(
            "EDITOR_DOCUMENT_TOO_LARGE",
            "Editor document exceeds 4 MiB",
        ));
    }
    Ok(())
}

pub(crate) async fn session_row(
    state: &ControlApiState,
    actor: &Actor,
    session: Uuid,
    write: bool,
) -> ApiResult<sqlx::mysql::MySqlRow> {
    actor.require("workflow:edit")?;
    let row = sqlx::query(
        "SELECT * FROM studio_builder_sessions WHERE tenant_id=? AND id=? AND user_id=?",
    )
    .bind(actor.tenant_id)
    .bind(session)
    .bind(actor.user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Builder session"))?;
    let workflow = row.try_get("workflow_id")?;
    crate::workflow_api::require_workflow_access(state, actor, workflow, write).await?;
    let draft =
        crate::workflow_api::load_draft(state, actor.tenant_id, workflow, row.try_get("draft_id")?)
            .await?;
    if write
        && (draft.archived || row.try_get::<u64, _>("document_generation")? != draft.generation)
    {
        return Err(ApiError::conflict(
            "DRAFT_READ_ONLY",
            "The version is archived or this editor binding has expired",
        ));
    }
    Ok(row)
}

async fn lock_binding(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    actor: &Actor,
    row: &sqlx::mysql::MySqlRow,
) -> ApiResult<()> {
    crate::workflow_versions::lock_workflow(tx, actor, row.try_get("workflow_id")?).await?;
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM studio_builder_sessions s JOIN workflow_drafts d ON d.tenant_id=s.tenant_id AND d.id=s.draft_id JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.working_draft_id=d.id WHERE s.tenant_id=? AND s.id=? AND s.document_generation=d.generation AND v.archived=FALSE)").bind(actor.tenant_id).bind(row.try_get::<Uuid,_>("id")?).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(ApiError::conflict(
            "EDITOR_BINDING_STALE",
            "Reload the version before editing",
        ));
    }
    Ok(())
}

async fn create_session(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((workflow, draft)): Path<(Uuid, Uuid)>,
    Json(input): Json<CreateBuilderSession>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:edit")?;
    crate::workflow_api::require_workflow_access(&state, &actor, workflow, true).await?;
    let mut tx = state.pool.begin().await?;
    crate::workflow_versions::lock_workflow(&mut tx, &actor, workflow).await?;
    let value = crate::workflow_api::load_draft(&state, actor.tenant_id, workflow, draft).await?;
    if value.archived {
        return Err(ApiError::conflict(
            "DRAFT_READ_ONLY",
            "The version is archived or this editor binding has expired",
        ));
    }
    if input.snapshot.server_revision != value.revision {
        return Err(ApiError::conflict(
            "DRAFT_REVISION_CONFLICT",
            "Reload the document before binding an editor",
        ));
    }
    verify_snapshot(&input.snapshot)?;
    let id = Uuid::now_v7();
    let core = AgentSessionStateV1 {
        session_id: id.to_string(),
        ..Default::default()
    };
    sqlx::query("INSERT INTO studio_builder_sessions(id,tenant_id,workflow_id,draft_id,user_id,editor_instance_id,snapshot_json,state_json,document_generation) VALUES(?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id")
        .bind(id).bind(actor.tenant_id).bind(workflow).bind(draft).bind(actor.user_id).bind(input.snapshot.editor_instance_id).bind(serde_json::to_value(&input.snapshot).map_err(ApiError::internal)?).bind(serde_json::to_value(core).map_err(ApiError::internal)?).bind(value.generation).execute(&mut *tx).await?;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM studio_builder_sessions WHERE tenant_id=? AND draft_id=? AND user_id=? AND editor_instance_id=? AND document_generation=?")
        .bind(actor.tenant_id).bind(draft).bind(actor.user_id).bind(input.snapshot.editor_instance_id).bind(value.generation).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    get_session(State(state), actor, Path(id)).await
}

async fn get_session(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let row = session_row(&state, &actor, session, false).await?;
    let runs = sqlx::query("SELECT id,status,model_alias_id,setting_version,created_at FROM studio_builder_runs WHERE tenant_id=? AND session_id=? ORDER BY created_at DESC LIMIT 20")
        .bind(actor.tenant_id).bind(session).fetch_all(&state.pool).await?.into_iter().map(|row| Ok(json!({"id":row.try_get::<Uuid,_>("id")?,"status":row.try_get::<String,_>("status")?,"modelAliasId":row.try_get::<Uuid,_>("model_alias_id")?,"settingVersion":row.try_get::<u64,_>("setting_version")?}))).collect::<Result<Vec<_>,sqlx::Error>>()?;
    Ok(Json(
        json!({"id":session,"draftId":row.try_get::<Uuid,_>("draft_id")?,"snapshot":row.try_get::<Value,_>("snapshot_json")?,"runs":runs,"canEdit":crate::workflow_api::require_workflow_access(&state,&actor,row.try_get("workflow_id")?,true).await.is_ok() && session_row(&state,&actor,session,true).await.is_ok()}),
    ))
}

async fn sync_snapshot(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
    Json(snapshot): Json<StudioEditorSnapshotV1>,
) -> ApiResult<Json<Value>> {
    let binding = session_row(&state, &actor, session, true).await?;
    verify_snapshot(&snapshot)?;
    let mut tx = state.pool.begin().await?;
    lock_binding(&mut tx, &actor, &binding).await?;
    let row = sqlx::query("SELECT snapshot_json,editor_instance_id FROM studio_builder_sessions WHERE tenant_id=? AND id=? FOR UPDATE").bind(actor.tenant_id).bind(session).fetch_one(&mut *tx).await?;
    let old: StudioEditorSnapshotV1 =
        serde_json::from_value(row.try_get("snapshot_json")?).map_err(ApiError::internal)?;
    if row.try_get::<Uuid, _>("editor_instance_id")? != snapshot.editor_instance_id
        || snapshot.local_document_revision < old.local_document_revision
        || (snapshot.local_document_revision == old.local_document_revision
            && snapshot.document_hash != old.document_hash)
    {
        return Err(ApiError::conflict(
            "EDITOR_BINDING_STALE",
            "Editor snapshot is stale",
        ));
    }
    sqlx::query("UPDATE studio_builder_sessions SET snapshot_json=? WHERE tenant_id=? AND id=?")
        .bind(serde_json::to_value(snapshot).map_err(ApiError::internal)?)
        .bind(actor.tenant_id)
        .bind(session)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"synced":true})))
}

async fn create_run(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
    Json(input): Json<CreateBuilderRun>,
) -> ApiResult<Json<Value>> {
    let row = session_row(&state, &actor, session, true).await?;
    if input.prompt.trim().is_empty() || input.prompt.len() > 65536 {
        return Err(ApiError::bad_request(
            "BUILDER_PROMPT_INVALID",
            "A prompt between 1 and 65536 bytes is required",
        ));
    }
    if let agentx_runtime_contracts::StudioEditScopeV1::Nodes { node_ids } = &input.edit_scope {
        if node_ids.is_empty()
            || node_ids.iter().any(|id| {
                !input
                    .snapshot
                    .document
                    .definition
                    .nodes
                    .iter()
                    .any(|node| &node.id == id)
            })
        {
            return Err(ApiError::bad_request(
                "BUILDER_EDIT_SCOPE_INVALID",
                "Select existing nodes before starting a scoped run",
            ));
        }
    }
    // Idempotency is the caller's run ID, before resolving a potentially changed tenant model setting.
    if let Some(existing) = sqlx::query(
        "SELECT prompt,status,edit_scope_json FROM studio_builder_runs WHERE tenant_id=? AND session_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(session)
    .bind(input.run_id)
    .fetch_optional(&state.pool)
    .await?
    {
        if existing.try_get::<String, _>("prompt")? != input.prompt || existing.try_get::<Value,_>("edit_scope_json")? != serde_json::to_value(&input.edit_scope).map_err(ApiError::internal)? {
            return Err(ApiError::conflict(
                "BUILDER_RUN_IDEMPOTENCY_CONFLICT",
                "Run identity has a different prompt",
            ));
        }
        return Ok(Json(
            json!({"id":input.run_id,"status":existing.try_get::<String,_>("status")?}),
        ));
    }
    let _ = sync_snapshot(
        State(state.clone()),
        actor.clone(),
        Path(session),
        Json(input.snapshot),
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    lock_binding(&mut tx, &actor, &row).await?;
    sqlx::query("SELECT version FROM tenant_settings WHERE tenant_id=? FOR UPDATE")
        .bind(actor.tenant_id)
        .fetch_one(&mut *tx)
        .await?;
    let selected=sqlx::query("SELECT a.id,d.credential_id FROM tenant_settings s JOIN model_aliases a ON a.tenant_id=s.tenant_id AND a.id=s.platform_ai_model_alias_id JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE s.tenant_id=? FOR SHARE").bind(actor.tenant_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::conflict("BUILDER_MODEL_UNAVAILABLE","No platform AI model is configured"))?;
    if let Some(credential) = selected.try_get::<Option<Uuid>, _>("credential_id")? {
        sqlx::query("SELECT id FROM credentials WHERE tenant_id=? AND id=? FOR SHARE")
            .bind(actor.tenant_id)
            .bind(credential)
            .fetch_one(&mut *tx)
            .await?;
    }
    let open:i64=sqlx::query_scalar("SELECT COUNT(*) FROM studio_builder_runs WHERE tenant_id=? AND status IN ('queued','running','waiting')").bind(actor.tenant_id).fetch_one(&mut *tx).await?;
    if open >= 4 {
        return Err(ApiError::conflict(
            "BUILDER_CAPACITY_BUSY",
            "The tenant already has four open design runs",
        ));
    }
    let (model, credential, setting, binding, parameters, model_version, credential_version) =
        crate::platform_ai_model::freeze_model(&state, &actor).await?;
    let deadline = OffsetDateTime::now_utc() + time::Duration::minutes(10);
    sqlx::query("INSERT INTO studio_builder_runs(id,tenant_id,session_id,token_version,prompt,edit_scope_json,model_alias_id,model_alias_version,credential_id,credential_version,setting_version,model_binding_json,model_parameters_json,deadline_at,delta_cursors_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,JSON_OBJECT())")
        .bind(input.run_id).bind(actor.tenant_id).bind(session).bind(actor.token_version).bind(&input.prompt).bind(serde_json::to_value(&input.edit_scope).map_err(ApiError::internal)?).bind(model).bind(model_version).bind(credential).bind(credential_version).bind(setting).bind(serde_json::to_value(binding).map_err(ApiError::internal)?).bind(parameters).bind(deadline).execute(&mut *tx).await.map_err(|error| match &error {sqlx::Error::Database(db) if db.is_unique_violation() => ApiError::conflict("BUILDER_SESSION_BUSY","This session already has an open run"),_=>error.into()})?;
    tx.commit().await?;
    let target = StudioBuilderTargetV1 {
        tenant_id: actor.tenant_id,
        workflow_id: row.try_get("workflow_id")?,
        draft_id: row.try_get("draft_id")?,
        session_id: session,
        run_id: input.run_id,
        user_id: actor.user_id,
    };
    append_entry(
        &state,
        &target,
        &format!("user:{}", input.run_id),
        json!({"type":"user","text":input.prompt}),
    )
    .await?;
    Ok(Json(json!({"id":input.run_id,"status":"queued"})))
}

async fn entries(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
    Query(cursor): Query<Cursor>,
) -> ApiResult<Json<Value>> {
    session_row(&state, &actor, session, false).await?;
    Ok(Json(
        read_entries(&state, actor.tenant_id, session, cursor.after.unwrap_or(0)).await?,
    ))
}
async fn read_entries(
    state: &ControlApiState,
    tenant: Uuid,
    session: Uuid,
    after: u64,
) -> ApiResult<Value> {
    let rows = sqlx::query("SELECT id,run_id,payload_json FROM studio_builder_entries FORCE INDEX(idx_studio_builder_cursor) WHERE tenant_id=? AND session_id=? AND id>? ORDER BY id LIMIT 200").bind(tenant).bind(session).bind(after).fetch_all(&state.pool).await?;
    let items = rows.into_iter().map(|row| Ok(json!({"sequence":row.try_get::<u64,_>("id")?,"runId":row.try_get::<Option<Uuid>,_>("run_id")?,"payload":row.try_get::<Value,_>("payload_json")?}))).collect::<Result<Vec<_>,sqlx::Error>>()?;
    let next = items
        .last()
        .and_then(|item| item["sequence"].as_u64())
        .unwrap_or(after);
    Ok(json!({"items":items,"nextCursor":next}))
}
async fn events(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
    Query(cursor): Query<Cursor>,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    session_row(&state, &actor, session, false).await?;
    let stream = async_stream::stream! {
        let mut after = cursor.after.unwrap_or(0);
        loop {
            let Ok(current_actor)=crate::control_api::reload_actor(&state,actor.tenant_id,actor.user_id,actor.token_version).await else {break;};
            if session_row(&state,&current_actor,session,false).await.is_err() { break; }
            let Ok(page) = read_entries(&state,actor.tenant_id,session,after).await else { break; };
            after = page["nextCursor"].as_u64().unwrap_or(after);
            if let Some(items) = page["items"].as_array() { for item in items { yield Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().id(item["sequence"].to_string()).json_data(item).unwrap()); } }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    };
    let mut response = axum::response::Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response();
    // Commands must reach the editor before the Host can resume. Proxy
    // buffering can otherwise hold the final small SSE frame indefinitely.
    response.headers_mut().insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    Ok(response)
}

async fn acknowledge(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
    Json(input): Json<StudioEditorAcknowledgementV1>,
) -> ApiResult<Json<Value>> {
    let session_data = session_row(&state, &actor, session, true).await?;
    verify_snapshot(&input.snapshot)?;
    let mut tx = state.pool.begin().await?;
    lock_binding(&mut tx, &actor, &session_data).await?;
    lock_effect_run(&mut tx, actor.tenant_id, session, input.command_id).await?;
    let effect = sqlx::query("SELECT e.*,r.status run_status FROM studio_builder_effects e JOIN studio_builder_runs r ON r.id=e.run_id AND r.tenant_id=e.tenant_id WHERE e.tenant_id=? AND e.session_id=? AND e.id=? FOR UPDATE").bind(actor.tenant_id).bind(session).bind(input.command_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Editor command"))?;
    let receipt = serde_json::to_value(&input).map_err(ApiError::internal)?;
    if let Some(saved) = effect.try_get::<Option<Value>, _>("acknowledgement_json")? {
        if saved != receipt {
            return Err(ApiError::conflict(
                "EDITOR_ACK_CONFLICT",
                "Command already has a different acknowledgement",
            ));
        }
        tx.commit().await?;
        append_ack(
            &state,
            &actor,
            &session_data,
            effect.try_get("run_id")?,
            session,
            &input,
        )
        .await?;
        return Ok(Json(json!({"acknowledged":true,"replayed":true})));
    }
    if !matches!(
        effect.try_get::<String, _>("run_status")?.as_str(),
        "queued" | "running" | "waiting"
    ) {
        return Err(ApiError::conflict(
            "BUILDER_RUN_CLOSED",
            "The run has stopped",
        ));
    }
    let command: StudioEditorCommandV1 =
        serde_json::from_value(effect.try_get("command_json")?).map_err(ApiError::internal)?;
    if command.editor_instance_id != input.snapshot.editor_instance_id {
        return Err(ApiError::conflict(
            "EDITOR_BINDING_STALE",
            "The command belongs to another editor",
        ));
    }
    let applied = matches!(input.status, StudioAcknowledgementStatusV1::Applied);
    if applied
        && (!crate::studio_builder_layout::matches_result(&command, &input.snapshot.document)
            || input.snapshot.local_document_revision
                != command.expected_local_document_revision + 1)
    {
        return Err(ApiError::conflict(
            "EDITOR_ACK_INVALID",
            "Applied command receipt does not match the resulting document",
        ));
    }
    let outcome = json!({"commandId":input.command_id,"status":input.status,"documentHash":input.snapshot.document_hash,"localDocumentRevision":input.snapshot.local_document_revision,"changedNodeIds":command.changed_node_ids,"issues":input.issues,"saveStatus":input.snapshot.save_status});
    let result = ToolResultV1 {
        content: outcome.to_string(),
        structured_result: Some(outcome),
        artifact_refs: Vec::new(),
        truncated: false,
        is_error: !applied,
        terminate: false,
    };
    sqlx::query("UPDATE studio_builder_effects SET status='settled',result_json=?,acknowledgement_json=? WHERE id=? AND status='pending'").bind(serde_json::to_value(result).map_err(ApiError::internal)?).bind(receipt).bind(input.command_id).execute(&mut *tx).await?;
    if applied {
        sqlx::query("UPDATE studio_builder_sessions SET snapshot_json=? WHERE tenant_id=? AND id=? AND JSON_EXTRACT(snapshot_json,'$.localDocumentRevision')<=?").bind(serde_json::to_value(&input.snapshot).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(session).bind(command.expected_local_document_revision+1).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    append_ack(
        &state,
        &actor,
        &session_data,
        effect.try_get("run_id")?,
        session,
        &input,
    )
    .await?;
    Ok(Json(json!({"acknowledged":true})))
}

async fn answer(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
    Json(input): Json<Answer>,
) -> ApiResult<Json<Value>> {
    let row = session_row(&state, &actor, session, true).await?;
    if row.try_get::<Uuid, _>("editor_instance_id")? != input.editor_instance_id
        || input.answer.len() > 65536
    {
        return Err(ApiError::conflict(
            "EDITOR_BINDING_STALE",
            "The answer is invalid for this editor",
        ));
    }
    let result = ToolResultV1 {
        content: json!({"answer":input.answer,"scope":"design_only"}).to_string(),
        structured_result: None,
        artifact_refs: vec![],
        truncated: false,
        is_error: false,
        terminate: false,
    };
    let mut tx = state.pool.begin().await?;
    lock_binding(&mut tx, &actor, &row).await?;
    lock_effect_run(&mut tx, actor.tenant_id, session, input.question_id).await?;
    let effect = sqlx::query("SELECT e.run_id,e.status,e.result_json,r.status run_status FROM studio_builder_effects e JOIN studio_builder_runs r ON r.tenant_id=e.tenant_id AND r.id=e.run_id WHERE e.tenant_id=? AND e.session_id=? AND e.id=? AND e.tool_name='ask_user' FOR UPDATE").bind(actor.tenant_id).bind(session).bind(input.question_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Builder question"))?;
    let result_json = serde_json::to_value(result).map_err(ApiError::internal)?;
    if let Some(saved) = effect.try_get::<Option<Value>, _>("result_json")? {
        if saved != result_json {
            return Err(ApiError::conflict(
                "BUILDER_ANSWER_CONFLICT",
                "Question already has a different answer",
            ));
        }
    } else {
        if effect.try_get::<String, _>("status")? != "pending"
            || !matches!(
                effect.try_get::<String, _>("run_status")?.as_str(),
                "queued" | "running" | "waiting"
            )
        {
            return Err(ApiError::conflict(
                "BUILDER_QUESTION_CLOSED",
                "Question or run has stopped",
            ));
        }
        sqlx::query("UPDATE studio_builder_effects SET status='settled',result_json=? WHERE tenant_id=? AND id=?").bind(result_json).bind(actor.tenant_id).bind(input.question_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let target = target_for(&actor, &row, session, effect.try_get("run_id")?)?;
    append_entry(
        &state,
        &target,
        &format!("answer:{}", input.question_id),
        json!({"type":"answer","questionId":input.question_id,"text":input.answer}),
    )
    .await?;
    Ok(Json(json!({"answered":true})))
}

async fn cancel_run(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((session, run)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    let row = session_row(&state, &actor, session, false).await?;
    let current: String = sqlx::query_scalar(
        "SELECT status FROM studio_builder_runs WHERE tenant_id=? AND session_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(session)
    .bind(run)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Builder run"))?;
    if !matches!(
        current.as_str(),
        "queued" | "running" | "waiting" | "cancelled"
    ) {
        return Ok(Json(json!({"cancelled":false,"status":current})));
    }
    sqlx::query("UPDATE studio_builder_runs SET status='cancelled',completed_at=UTC_TIMESTAMP(6),lease_owner=NULL,lease_until=NULL WHERE tenant_id=? AND session_id=? AND id=? AND status IN ('queued','running','waiting')").bind(actor.tenant_id).bind(session).bind(run).execute(&state.pool).await?;
    sqlx::query("UPDATE studio_builder_effects SET status='cancelled' WHERE tenant_id=? AND session_id=? AND run_id=? AND status='pending'").bind(actor.tenant_id).bind(session).bind(run).execute(&state.pool).await?;
    let target = StudioBuilderTargetV1 {
        tenant_id: actor.tenant_id,
        workflow_id: row.try_get("workflow_id")?,
        draft_id: row.try_get("draft_id")?,
        session_id: session,
        run_id: run,
        user_id: actor.user_id,
    };
    let state_json: AgentSessionStateV1 =
        serde_json::from_value(row.try_get("state_json")?).map_err(ApiError::internal)?;
    if let Some(operation) = state_json.operation.filter(|op| {
        op.operation_id.starts_with(&run.to_string())
            && matches!(
                op.operation_kind,
                agentx_agent_core::OperationKindV1::Model
                    | agentx_agent_core::OperationKindV1::Compaction
            )
    }) {
        let effect =
            agentx_runtime_contracts::deterministic_uuid(run, operation.effect_id.as_bytes());
        let _ = crate::studio_builder_runtime::cancel(&state, &target, effect).await;
    }
    append_entry(
        &state,
        &target,
        &format!("terminal:{run}"),
        json!({"type":"terminal","status":"cancelled"}),
    )
    .await?;
    Ok(Json(json!({"cancelled":true})))
}

fn target_for(
    actor: &Actor,
    row: &sqlx::mysql::MySqlRow,
    session: Uuid,
    run: Uuid,
) -> ApiResult<StudioBuilderTargetV1> {
    Ok(StudioBuilderTargetV1 {
        tenant_id: actor.tenant_id,
        workflow_id: row.try_get("workflow_id")?,
        draft_id: row.try_get("draft_id")?,
        session_id: session,
        run_id: run,
        user_id: actor.user_id,
    })
}
async fn lock_effect_run(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    session: Uuid,
    effect: Uuid,
) -> ApiResult<()> {
    sqlx::query("SELECT id FROM studio_builder_runs WHERE tenant_id=? AND session_id=? AND id=(SELECT run_id FROM studio_builder_effects WHERE tenant_id=? AND session_id=? AND id=?) FOR UPDATE")
        .bind(tenant).bind(session).bind(tenant).bind(session).bind(effect)
        .fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::not_found("Builder effect"))?;
    Ok(())
}

async fn append_ack(
    state: &ControlApiState,
    actor: &Actor,
    row: &sqlx::mysql::MySqlRow,
    run: Uuid,
    session: Uuid,
    input: &StudioEditorAcknowledgementV1,
) -> ApiResult<()> {
    append_entry(state,&target_for(actor,row,session,run)?,&format!("ack:{}",input.command_id),json!({"type":"command_acknowledged","commandId":input.command_id,"status":input.status,"documentHash":input.snapshot.document_hash,"saveStatus":input.snapshot.save_status,"issues":input.issues})).await
}
