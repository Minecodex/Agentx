//! Durable user guidance for both authoring Profiles; no new Run or tool capability.
use std::collections::HashSet;

use agentx_agent_core::{AgentMessageV1, AgentSessionStateV1};
use agentx_runtime_contracts::DesignModelTargetV1;
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    design_agent_runtime::append_entry_tx,
};

const MAX_INPUTS: i64 = 32;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputKind {
    Steering,
    FollowUp,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateInput {
    pub message_id: Uuid,
    pub editor_instance_id: Uuid,
    pub kind: InputKind,
    #[schemars(length(min = 1, max = 2048))]
    pub text: String,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InputReceipt {
    accepted: bool,
    run_id: Uuid,
    message_id: Uuid,
    replayed: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InputEntry {
    #[serde(rename = "type")]
    entry_type: String,
    message_id: Uuid,
    input_kind: InputKind,
    text: String,
}

impl InputEntry {
    fn core_id(&self, run: Uuid) -> String {
        format!("design-input:{run}:{}", self.message_id)
    }
}

pub(crate) async fn create(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((session, run)): Path<(Uuid, Uuid)>,
    Json(input): Json<CreateInput>,
) -> ApiResult<Json<InputReceipt>> {
    let binding = crate::studio_builder_api::session_row(&state, &actor, session, true).await?;
    if binding.try_get::<Uuid, _>("editor_instance_id")? != input.editor_instance_id {
        return Err(ApiError::conflict(
            "EDITOR_BINDING_STALE",
            "Guidance belongs to another editor",
        ));
    }
    if input.text.trim().is_empty() || input.text.chars().count() > 2048 {
        return Err(ApiError::bad_request(
            "DESIGN_INPUT_INVALID",
            "Guidance must contain 1 to 2048 characters",
        )
        .with_field_error(
            "text",
            "INVALID_LENGTH",
            "Use 1 to 2048 nonempty characters",
        ));
    }
    let mut tx = state.pool.begin().await?;
    crate::studio_builder_api::lock_binding(&mut tx, &actor, &binding).await?;
    let current = sqlx::query("SELECT status,deadline_at>UTC_TIMESTAMP(6) live FROM design_agent_runs WHERE tenant_id=? AND session_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id).bind(session).bind(run).fetch_optional(&mut *tx).await?
        .ok_or_else(|| ApiError::not_found("Design run"))?;
    let key = format!("input:{}", input.message_id);
    let payload = serde_json::to_value(InputEntry {
        entry_type: "user".into(),
        message_id: input.message_id,
        input_kind: input.kind,
        text: input.text,
    })
    .map_err(ApiError::internal)?;
    let existing = sqlx::query("SELECT run_id,payload_json FROM design_agent_entries WHERE tenant_id=? AND session_id=? AND event_key=?")
        .bind(actor.tenant_id).bind(session).bind(&key).fetch_optional(&mut *tx).await?;
    let replayed = if let Some(saved) = existing {
        let previous: Value = serde_json::from_str(&saved.try_get::<String, _>("payload_json")?)
            .map_err(ApiError::internal)?;
        if saved.try_get::<Uuid, _>("run_id")? != run || previous != payload {
            return Err(ApiError::conflict(
                "DESIGN_INPUT_CONFLICT",
                "Message identity is already bound to different guidance",
            ));
        }
        true
    } else {
        if !matches!(
            current.try_get::<String, _>("status")?.as_str(),
            "queued" | "running" | "waiting"
        ) || !current.try_get::<bool, _>("live")?
        {
            return Err(ApiError::conflict(
                "BUILDER_RUN_CLOSED",
                "The run has stopped; guidance was not queued",
            ));
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM design_agent_entries WHERE tenant_id=? AND session_id=? AND run_id=? AND event_key LIKE 'input:%'")
            .bind(actor.tenant_id).bind(session).bind(run).fetch_one(&mut *tx).await?;
        if count >= MAX_INPUTS {
            return Err(ApiError::conflict(
                "DESIGN_INPUT_LIMIT",
                "This run has reached its guidance limit",
            ));
        }
        let target = crate::design_agent_host::target_from_row(
            &binding,
            actor.tenant_id,
            actor.user_id,
            session,
            run,
        )?;
        append_entry_tx(&mut tx, &target, &key, payload).await?;
        false
    };
    tx.commit().await?;
    Ok(Json(InputReceipt {
        accepted: true,
        run_id: run,
        message_id: input.message_id,
        replayed,
    }))
}

async fn inputs(
    pool: &sqlx::MySqlPool,
    target: &DesignModelTargetV1,
) -> ApiResult<Vec<InputEntry>> {
    let id = target.identity();
    let rows = sqlx::query_scalar::<_, String>("SELECT payload_json FROM design_agent_entries WHERE tenant_id=? AND session_id=? AND run_id=? AND event_key LIKE 'input:%' ORDER BY id LIMIT 33")
        .bind(id.tenant_id).bind(id.session_id).bind(id.run_id).fetch_all(pool).await?;
    if rows.len() > MAX_INPUTS as usize {
        return Err(ApiError::internal(
            "Design guidance queue exceeds its bound",
        ));
    }
    rows.into_iter()
        .map(|row| serde_json::from_str(&row).map_err(ApiError::internal))
        .collect()
}

/// Core history is retained through compaction; its existing queues and message
/// identities are the delivery receipt, committed by the original StatePort CAS.
pub(crate) async fn hydrate(
    state: &ControlApiState,
    target: &DesignModelTargetV1,
    core: &mut AgentSessionStateV1,
) -> ApiResult<()> {
    let prefix = format!("design-input:{}:", target.identity().run_id);
    for queue in [&mut core.steering_queue, &mut core.follow_up_queue] {
        queue.retain(|message| {
            !message.message_id.starts_with("design-input:")
                || message.message_id.starts_with(&prefix)
        });
    }
    let mut known = core
        .messages
        .iter()
        .chain(core.steering_queue.iter())
        .chain(core.follow_up_queue.iter())
        .map(|message| message.message_id.clone())
        .collect::<HashSet<_>>();
    for input in inputs(&state.pool, target).await? {
        let id = input.core_id(target.identity().run_id);
        if !known.insert(id.clone()) {
            continue;
        }
        let message = AgentMessageV1::user(&id, &input.text);
        match input.input_kind {
            InputKind::Steering => core.steering_queue.push(message),
            InputKind::FollowUp => core.follow_up_queue.push(message),
        }
    }
    Ok(())
}

/// Admission and completion share the Run lock: accepted late guidance gets a
/// further Core step; stopped, failed, unknown, or fenced Runs are never revived.
pub(crate) async fn finish_run(
    state: &ControlApiState,
    target: &DesignModelTargetV1,
    fence: u64,
    proposed: &'static str,
) -> ApiResult<Option<&'static str>> {
    let id = target.identity();
    let mut tx = state.pool.begin().await?;
    let current = sqlx::query("SELECT id FROM design_agent_runs WHERE tenant_id=? AND session_id=? AND id=? AND fencing_token=? AND status='running' FOR UPDATE")
        .bind(id.tenant_id).bind(id.session_id).bind(id.run_id).bind(fence).fetch_optional(&mut *tx).await?;
    if current.is_none() {
        return Ok(None);
    }
    let status = if proposed == "completed" {
        let core: Value = sqlx::query_scalar(
            "SELECT state_json FROM design_agent_sessions WHERE tenant_id=? AND id=? FOR UPDATE",
        )
        .bind(id.tenant_id)
        .bind(id.session_id)
        .fetch_one(&mut *tx)
        .await?;
        let core: AgentSessionStateV1 = serde_json::from_value(core).map_err(ApiError::internal)?;
        let seen = core
            .messages
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<HashSet<_>>();
        let queued = sqlx::query_scalar::<_, String>("SELECT payload_json FROM design_agent_entries WHERE tenant_id=? AND session_id=? AND run_id=? AND event_key LIKE 'input:%' ORDER BY id LIMIT 33")
            .bind(id.tenant_id).bind(id.session_id).bind(id.run_id).fetch_all(&mut *tx).await?;
        let pending = queued
            .iter()
            .map(|row| serde_json::from_str::<InputEntry>(row).map_err(ApiError::internal))
            .collect::<ApiResult<Vec<_>>>()?
            .iter()
            .any(|input| !seen.contains(input.core_id(id.run_id).as_str()));
        if pending { "waiting" } else { proposed }
    } else {
        proposed
    };
    sqlx::query("UPDATE design_agent_runs SET status=?,completed_at=IF(?='waiting',NULL,UTC_TIMESTAMP(6)),lease_owner=NULL,lease_until=NULL,available_at=UTC_TIMESTAMP(6)+INTERVAL 1 SECOND WHERE tenant_id=? AND id=? AND fencing_token=? AND status='running'")
        .bind(status).bind(status).bind(id.tenant_id).bind(id.run_id).bind(fence).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(status))
}

pub(crate) fn apply_openapi(document: &mut Value) {
    document["paths"]["/api/v1/design-agent/sessions/{session}/runs/{run}/inputs"]["post"] = json!({
        "tags":["Design Agent"],"security":[{"bearerAuth":[]}],
        "parameters":[{"name":"session","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},{"name":"run","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}],
        "requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/CreateDesignInput"}}}},
        "responses":{"200":{"description":"Guidance accepted for this original run","content":{"application/json":{"schema":{"$ref":"#/components/schemas/DesignInputReceipt"}}}},"403":{"description":"Permission denied"},"404":{"description":"Session or run not found"},"409":{"description":"Closed run, stale editor, conflicting identity, or guidance limit"}}
    });
}
