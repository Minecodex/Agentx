use std::collections::BTreeSet;

use agentx_runtime_contracts::{
    ControlRole, DesignModelEffectRequestV1, DesignModelReceiptRequestV1, DesignModelReceiptV1,
    DesignModelTargetV1, ServiceClaimsV1, content_hash, issue_service_token, now_unix,
};
use secrecy::ExposeSecret;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::ControlApiState,
};

async fn call<T: Serialize>(
    state: &ControlApiState,
    target: &DesignModelTargetV1,
    action: &str,
    input: &T,
) -> ApiResult<Option<DesignModelReceiptV1>> {
    let body = serde_json::to_value(input).map_err(ApiError::internal)?;
    let hash = content_hash(&body).map_err(ApiError::internal)?;
    let now = now_unix();
    let token = issue_service_token(
        &state.runtime_command_kid,
        state.runtime_command_key.expose_secret().as_bytes(),
        &ServiceClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: format!(
                "design-agent:{}:{}:{}",
                target.source_kind(),
                target.identity().tenant_id,
                target.identity().run_id
            ),
            role: ControlRole::Publisher,
            scope: BTreeSet::from([
                format!("runtime.design-model.{action}"),
                format!("design-body:{}", hash.as_str()),
            ]),
            iat: now,
            exp: now + 60,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/design-model:{action}",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|_| {
            ApiError::unavailable(
                "DESIGN_MODEL_UNAVAILABLE",
                "The design-time model service is unavailable",
            )
        })?;
    if action == "inspect" && response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        let status = response.status();
        return Err(
            if status.is_server_error()
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            {
                ApiError::unavailable(
                    "DESIGN_MODEL_UNAVAILABLE",
                    "The design-time model service is temporarily unavailable",
                )
            } else if matches!(status.as_u16(), 401 | 403) {
                ApiError::forbidden("The design-time model service denied the effect")
            } else {
                ApiError::bad_request(
                    "DESIGN_MODEL_REJECTED",
                    "The design-time model service rejected the effect",
                )
            },
        );
    }
    let receipt: DesignModelReceiptV1 = response.json().await.map_err(ApiError::internal)?;
    // This journal is a source-retention reference, not another model state
    // machine. Only Runtime can confirm that the original effect has settled.
    if matches!(
        receipt.status.as_str(),
        "succeeded" | "failed" | "cancelled"
    ) {
        append_entry(
            state,
            target,
            &format!("model-resolved:{}", receipt.effect_id),
            serde_json::json!({"type":"model_receipt","effectId":receipt.effect_id,"status":receipt.status}),
        )
        .await?;
    }
    Ok(Some(receipt))
}

pub(crate) async fn submit(
    state: &ControlApiState,
    request: &DesignModelEffectRequestV1,
) -> ApiResult<DesignModelReceiptV1> {
    let effect = agentx_runtime_contracts::deterministic_uuid(
        request.target.identity().run_id,
        request.context.effect_id.as_bytes(),
    );
    // Persist before dispatch: a lost response, stopped Run or later Session
    // turn cannot remove the original model/credential/source reference.
    append_entry(
        state,
        &request.target,
        &format!("model-dispatch:{effect}"),
        serde_json::json!({"type":"model_dispatch","effectId":effect}),
    )
    .await?;
    call(state, &request.target, "submit", request)
        .await?
        .ok_or_else(|| ApiError::internal("Model effect has no receipt"))
}
pub(crate) async fn inspect(
    state: &ControlApiState,
    target: impl Into<DesignModelTargetV1>,
    effect: Uuid,
    after: u64,
) -> ApiResult<Option<DesignModelReceiptV1>> {
    let target = target.into();
    call(
        state,
        &target,
        "inspect",
        &DesignModelReceiptRequestV1 {
            target: target.clone(),
            effect_id: effect,
            after,
        },
    )
    .await
}
pub(crate) async fn cancel(
    state: &ControlApiState,
    target: impl Into<DesignModelTargetV1>,
    effect: Uuid,
) -> ApiResult<Option<DesignModelReceiptV1>> {
    let target = target.into();
    call(
        state,
        &target,
        "cancel",
        &DesignModelReceiptRequestV1 {
            target: target.clone(),
            effect_id: effect,
            after: 0,
        },
    )
    .await
}

pub(crate) async fn append_entry(
    state: &ControlApiState,
    target: impl Into<DesignModelTargetV1>,
    key: &str,
    payload: Value,
) -> ApiResult<()> {
    append_entries(state, target, vec![(key.to_owned(), payload)]).await
}

pub(crate) async fn append_entries(
    state: &ControlApiState,
    target: impl Into<DesignModelTargetV1>,
    entries: Vec<(String, Value)>,
) -> ApiResult<()> {
    let target = target.into();
    let identity = target.identity();
    if entries.is_empty() {
        return Ok(());
    }
    let mut tx = state.pool.begin().await?;
    // Inserting an Entry checks its Session FK before its Run FK. Acquire
    // Run first so that journal publication follows the Core/ACK lock order.
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM design_agent_runs WHERE tenant_id=? AND session_id=? AND id=? FOR SHARE",
    )
    .bind(identity.tenant_id)
    .bind(identity.session_id)
    .bind(identity.run_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Builder run"))?;
    for (key, payload) in entries {
        append_entry_tx(&mut tx, &target, &key, payload).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The caller holds the Run lock. ACK and its journal entry commit together.
pub(crate) async fn append_entry_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    target: &DesignModelTargetV1,
    key: &str,
    payload: Value,
) -> ApiResult<()> {
    let identity = target.identity();
    sqlx::query("INSERT INTO design_agent_entries(tenant_id,session_id,run_id,event_key,payload_json) VALUES(?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id")
        .bind(identity.tenant_id).bind(identity.session_id).bind(identity.run_id).bind(key).bind(payload).execute(&mut **tx).await?;
    Ok(())
}

pub(crate) async fn compaction_snapshot_id(
    state: &ControlApiState,
    target: impl Into<DesignModelTargetV1>,
) -> ApiResult<String> {
    let target = target.into().identity();
    let core: Value = sqlx::query_scalar(
        "SELECT state_json FROM design_agent_sessions WHERE tenant_id=? AND id=?",
    )
    .bind(target.tenant_id)
    .bind(target.session_id)
    .fetch_one(&state.pool)
    .await?;
    core.pointer("/compaction/snapshotId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::internal("Compaction snapshot is missing"))
}
