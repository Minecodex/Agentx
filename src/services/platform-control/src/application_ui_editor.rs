use super::{lock_binding, verify_snapshot};
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};
use agentx_agent_core::ToolResultV1;
use agentx_runtime_contracts::{
    PageEditCommandV1, PageEditorSnapshotV1, StudioAcknowledgementStatusV1, content_hash,
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

pub(super) async fn session_row(
    state: &ControlApiState,
    actor: &Actor,
    session: Uuid,
    write: bool,
) -> ApiResult<sqlx::mysql::MySqlRow> {
    let row = crate::studio_builder_api::session_row(state, actor, session, write).await?;
    if row.try_get::<String, _>("target_kind")? != "application_ui_builder" {
        return Err(ApiError::forbidden(
            "This session is not an application page",
        ));
    }
    Ok(row)
}

pub(super) async fn get_session(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
) -> ApiResult<Json<agentx_runtime_contracts::PageBuilderSessionV1>> {
    let row = session_row(&state, &actor, session, false).await?;
    let runs=sqlx::query("SELECT id,status,model_alias_id,setting_version FROM design_agent_runs WHERE tenant_id=? AND session_id=? ORDER BY created_at DESC,id DESC LIMIT 20").bind(actor.tenant_id).bind(session).fetch_all(&state.pool).await?.into_iter().map(|row|Ok(agentx_runtime_contracts::StudioBuilderRunSummaryV1{id:row.try_get("id")?,status:row.try_get("status")?,model_alias_id:row.try_get("model_alias_id")?,setting_version:row.try_get("setting_version")?})).collect::<Result<Vec<_>,sqlx::Error>>()?;
    Ok(Json(agentx_runtime_contracts::PageBuilderSessionV1 {
        id: session,
        ui_draft_id: row.try_get("ui_draft_id")?,
        snapshot: serde_json::from_value(row.try_get("snapshot_json")?)
            .map_err(ApiError::internal)?,
        runs,
    }))
}

pub(super) async fn snapshot(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
    Json(input): Json<PageEditorSnapshotV1>,
) -> ApiResult<Json<Value>> {
    let row = session_row(&state, &actor, session, true).await?;
    verify_snapshot(&input)?;
    let previous: PageEditorSnapshotV1 =
        serde_json::from_value(row.try_get("snapshot_json")?).map_err(ApiError::internal)?;
    if input.editor_instance_id != previous.editor_instance_id
        || input.local_revision < previous.local_revision
        || input.server_revision < previous.server_revision
    {
        return Err(ApiError::conflict(
            "PAGE_BINDING_STALE",
            "The editor snapshot is stale",
        ));
    }
    let page = super::load(
        &state,
        actor.tenant_id,
        row.try_get("application_id")?,
        row.try_get("ui_draft_id")?,
    )
    .await?;
    if input.server_revision != page.revision {
        return Err(ApiError::conflict(
            "PAGE_REVISION_CONFLICT",
            "The saved page changed",
        ));
    }
    if matches!(
        input.save_status,
        agentx_runtime_contracts::StudioSaveStatusV1::Saved
    ) && input.html_hash != page.html_hash
    {
        return Err(ApiError::conflict(
            "PAGE_SAVE_NOT_CONFIRMED",
            "The saved source does not match this snapshot",
        ));
    }
    let changed=sqlx::query("UPDATE design_agent_sessions SET snapshot_json=? WHERE tenant_id=? AND id=? AND editor_instance_id=? AND JSON_EXTRACT(snapshot_json,'$.localRevision')<=?").bind(serde_json::to_value(input.clone()).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(session).bind(input.editor_instance_id).bind(input.local_revision).execute(&state.pool).await?;
    if changed.rows_affected() == 0
        && serde_json::to_value(&previous).map_err(ApiError::internal)?
            != serde_json::to_value(input).map_err(ApiError::internal)?
    {
        return Err(ApiError::conflict(
            "PAGE_BINDING_STALE",
            "A newer editor snapshot is present",
        ));
    }
    Ok(Json(json!({"synced":true})))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateRun {
    pub run_id: Uuid,
    pub prompt: String,
    pub snapshot: PageEditorSnapshotV1,
}

pub(super) async fn create_run(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
    Json(input): Json<CreateRun>,
) -> ApiResult<Json<Value>> {
    let row = session_row(&state, &actor, session, true).await?;
    if input.prompt.trim().is_empty() || input.prompt.len() > 65536 {
        return Err(ApiError::bad_request(
            "BUILDER_PROMPT_INVALID",
            "A prompt between 1 and 65536 bytes is required",
        ));
    }
    if let Some(existing) = sqlx::query(
        "SELECT prompt,status FROM design_agent_runs WHERE tenant_id=? AND session_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(session)
    .bind(input.run_id)
    .fetch_optional(&state.pool)
    .await?
    {
        if existing.try_get::<String, _>("prompt")? != input.prompt {
            return Err(ApiError::conflict(
                "BUILDER_RUN_IDEMPOTENCY_CONFLICT",
                "The run has a different prompt",
            ));
        }
        return Ok(Json(
            json!({"id":input.run_id,"status":existing.try_get::<String,_>("status")?}),
        ));
    }
    let editor_instance_id = input.snapshot.editor_instance_id;
    let _ = snapshot(
        State(state.clone()),
        actor.clone(),
        Path(session),
        Json(input.snapshot),
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    lock_binding(&mut tx, &actor, &row).await?;
    let editor:Uuid=sqlx::query_scalar("SELECT editor_instance_id FROM design_agent_sessions WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id).bind(session).fetch_one(&mut *tx).await?;
    if editor != editor_instance_id {
        return Err(ApiError::conflict(
            "PAGE_BINDING_STALE",
            "Another window is editing this page",
        ));
    }
    sqlx::query("SELECT version FROM tenant_settings WHERE tenant_id=? FOR UPDATE")
        .bind(actor.tenant_id)
        .fetch_one(&mut *tx)
        .await?;
    let open:i64=sqlx::query_scalar("SELECT COUNT(*) FROM design_agent_runs WHERE tenant_id=? AND status IN ('queued','running','waiting')").bind(actor.tenant_id).fetch_one(&mut *tx).await?;
    if open >= 4 {
        return Err(ApiError::conflict(
            "BUILDER_CAPACITY_BUSY",
            "The tenant already has four open design runs",
        ));
    }
    let (model, credential, setting, binding, parameters, model_version, credential_version) =
        crate::platform_ai_model::freeze_model(&state, &actor).await?;
    sqlx::query("INSERT INTO design_agent_runs(id,tenant_id,session_id,token_version,prompt,edit_scope_json,model_alias_id,model_alias_version,credential_id,credential_version,setting_version,model_binding_json,model_parameters_json,deadline_at,delta_cursors_json) VALUES(?,?,?,?,?,JSON_OBJECT('kind','page'),?,?,?,?,?,?,?,?,JSON_OBJECT())")
        .bind(input.run_id).bind(actor.tenant_id).bind(session).bind(actor.token_version).bind(&input.prompt).bind(model).bind(model_version).bind(credential).bind(credential_version).bind(setting).bind(serde_json::to_value(binding).map_err(ApiError::internal)?).bind(parameters).bind(OffsetDateTime::now_utc()+time::Duration::minutes(10)).execute(&mut *tx).await.map_err(|error|if error.as_database_error().is_some_and(|error|error.is_unique_violation()){ApiError::conflict("BUILDER_SESSION_BUSY","This session already has an open run")}else{error.into()})?;
    let target = crate::design_agent_host::target_from_row(
        &row,
        actor.tenant_id,
        actor.user_id,
        session,
        input.run_id,
    )?;
    crate::design_agent_runtime::append_entry_tx(
        &mut tx,
        &target,
        &format!("user:{}", input.run_id),
        json!({"type":"user","text":input.prompt}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":input.run_id,"status":"queued"})))
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Acknowledgement {
    pub command_id: Uuid,
    pub status: StudioAcknowledgementStatusV1,
    pub snapshot: PageEditorSnapshotV1,
    pub report: Option<Value>,
}

pub(super) async fn acknowledge(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session): Path<Uuid>,
    Json(input): Json<Acknowledgement>,
) -> ApiResult<Json<Value>> {
    let row = session_row(&state, &actor, session, true).await?;
    verify_snapshot(&input.snapshot)?;
    if input.snapshot.editor_instance_id != row.try_get::<Uuid, _>("editor_instance_id")? {
        return Err(ApiError::conflict(
            "PAGE_BINDING_STALE",
            "The command belongs to another editor",
        ));
    }
    if input
        .report
        .as_ref()
        .is_some_and(|report| report.to_string().len() > 65536)
    {
        return Err(ApiError::bad_request(
            "PAGE_REPORT_TOO_LARGE",
            "Narrow the page check report",
        ));
    }
    let mut tx = state.pool.begin().await?;
    lock_binding(&mut tx, &actor, &row).await?;
    let effect_run: Uuid = sqlx::query_scalar(
        "SELECT run_id FROM design_agent_effects WHERE tenant_id=? AND session_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(session)
    .bind(input.command_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Page command"))?;
    let status: String = sqlx::query_scalar(
        "SELECT status FROM design_agent_runs WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(effect_run)
    .fetch_one(&mut *tx)
    .await?;
    let effect = sqlx::query(
        "SELECT * FROM design_agent_effects WHERE tenant_id=? AND session_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(session)
    .bind(input.command_id)
    .fetch_one(&mut *tx)
    .await?;
    let receipt = serde_json::to_value(&input).map_err(ApiError::internal)?;
    let editor:Uuid=sqlx::query_scalar("SELECT editor_instance_id FROM design_agent_sessions WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id).bind(session).fetch_one(&mut *tx).await?;
    if editor != input.snapshot.editor_instance_id {
        return Err(ApiError::conflict(
            "PAGE_BINDING_STALE",
            "Another window is editing this page",
        ));
    }
    if let Some(saved) = effect.try_get::<Option<Value>, _>("acknowledgement_json")? {
        if saved != receipt {
            return Err(ApiError::conflict(
                "EDITOR_ACK_CONFLICT",
                "The command already has a different acknowledgement",
            ));
        }
        return Ok(Json(json!({"acknowledged":true,"replayed":true})));
    }
    if !matches!(status.as_str(), "queued" | "running" | "waiting") {
        return Err(ApiError::conflict(
            "BUILDER_RUN_CLOSED",
            "The run has stopped",
        ));
    }
    let applied = matches!(input.status, StudioAcknowledgementStatusV1::Applied);
    let command: Value = effect.try_get("command_json")?;
    let tool: String = effect.try_get("tool_name")?;
    if tool == "page_edit" && applied {
        let command: PageEditCommandV1 =
            serde_json::from_value(command).map_err(ApiError::internal)?;
        let saved = sqlx::query(
            "SELECT revision,html_hash FROM application_ui_drafts WHERE tenant_id=? AND id=?",
        )
        .bind(actor.tenant_id)
        .bind(command.ui_draft_id)
        .fetch_one(&mut *tx)
        .await?;
        if command.editor_instance_id != input.snapshot.editor_instance_id
            || input.snapshot.html_hash != command.html_hash
            || input.snapshot.local_revision != command.base_local_revision + 1
            || input.snapshot.server_revision != saved.try_get::<u64, _>("revision")?
            || saved.try_get::<String, _>("html_hash")? != input.snapshot.html_hash.as_str()
            || !matches!(
                input.snapshot.save_status,
                agentx_runtime_contracts::StudioSaveStatusV1::Saved
            )
        {
            return Err(ApiError::conflict(
                "EDITOR_ACK_INVALID",
                "The page edit has not been applied and saved as declared",
            ));
        }
    } else if tool == "page_check" && applied {
        let report = input.report.as_ref();
        let expected_contract_hash: String = row.try_get("contract_hash")?;
        if command["htmlHash"] != json!(input.snapshot.html_hash)
            || report.is_none_or(|report| {
                report["htmlHash"] != json!(input.snapshot.html_hash)
                    || report["contractHash"] != json!(expected_contract_hash)
                    || report["sdkVersion"]
                        != json!(agentx_runtime_contracts::APPLICATION_PAGE_SDK_VERSION)
                    || report["mode"] != command["mode"]
                    || report["synthetic"] != json!(true)
                    || report["formalRequests"] != json!(0)
                    || !report["errors"].is_array()
            })
        {
            return Err(ApiError::conflict(
                "PAGE_CHECK_STALE",
                "The check does not belong to this page source",
            ));
        }
    } else if !matches!(tool.as_str(), "page_edit" | "page_check") {
        return Err(ApiError::forbidden("This is not a page command"));
    }
    let outcome = json!({"commandId":input.command_id,"status":input.status,"htmlHash":input.snapshot.html_hash,"localRevision":input.snapshot.local_revision,"serverRevision":input.snapshot.server_revision,"saveStatus":input.snapshot.save_status,"report":input.report});
    let errors = input
        .report
        .as_ref()
        .and_then(|report| report.get("errors"))
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty());
    let result = ToolResultV1 {
        content: outcome.to_string(),
        structured_result: Some(outcome.clone()),
        artifact_refs: vec![],
        truncated: false,
        is_error: !applied || errors,
        terminate: false,
    };
    sqlx::query("UPDATE design_agent_effects SET status='settled',result_json=?,acknowledgement_json=? WHERE tenant_id=? AND id=? AND status='pending'").bind(serde_json::to_value(result).map_err(ApiError::internal)?).bind(receipt).bind(actor.tenant_id).bind(input.command_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE design_agent_sessions SET snapshot_json=? WHERE tenant_id=? AND id=? AND JSON_EXTRACT(snapshot_json,'$.localRevision')<=?").bind(serde_json::to_value(&input.snapshot).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(session).bind(input.snapshot.local_revision).execute(&mut *tx).await?;
    let target = crate::design_agent_host::target_from_row(
        &row,
        actor.tenant_id,
        actor.user_id,
        session,
        effect_run,
    )?;
    crate::design_agent_runtime::append_entry_tx(
        &mut tx,
        &target,
        &format!("ack:{}", input.command_id),
        json!({"type":"page_acknowledged","result":outcome}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"acknowledged":true,"htmlHash":content_hash(&input.snapshot.html).map_err(ApiError::internal)?}),
    ))
}
