use std::collections::BTreeSet;

use agentx_runtime_contracts::{
    ControlRole, DesignModelEffectRequestV1, DesignModelReceiptRequestV1, DesignModelReceiptV1,
    ServiceClaimsV1, StudioBuilderTargetV1, content_hash, issue_service_token, now_unix,
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
    target: &StudioBuilderTargetV1,
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
            sub: format!("studio-builder:{}:{}", target.tenant_id, target.run_id),
            role: ControlRole::Publisher,
            scope: BTreeSet::from([
                format!("runtime.studio-model.{action}"),
                format!("studio-body:{}", hash.as_str()),
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
            "{}/internal/runtime/v1/studio-model:{action}",
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
        return Err(ApiError::unavailable(
            "DESIGN_MODEL_REJECTED",
            "The design-time model service rejected the effect",
        ));
    }
    response.json().await.map(Some).map_err(ApiError::internal)
}

pub(crate) async fn submit(
    state: &ControlApiState,
    request: &DesignModelEffectRequestV1,
) -> ApiResult<DesignModelReceiptV1> {
    call(state, &request.target, "submit", request)
        .await?
        .ok_or_else(|| ApiError::internal("Model effect has no receipt"))
}
pub(crate) async fn inspect(
    state: &ControlApiState,
    target: &StudioBuilderTargetV1,
    effect: Uuid,
    after: u64,
) -> ApiResult<Option<DesignModelReceiptV1>> {
    call(
        state,
        target,
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
    target: &StudioBuilderTargetV1,
    effect: Uuid,
) -> ApiResult<Option<DesignModelReceiptV1>> {
    call(
        state,
        target,
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
    target: &StudioBuilderTargetV1,
    key: &str,
    payload: Value,
) -> ApiResult<()> {
    append_entries(state, target, vec![(key.to_owned(), payload)]).await
}

pub(crate) async fn append_entries(
    state: &ControlApiState,
    target: &StudioBuilderTargetV1,
    entries: Vec<(String, Value)>,
) -> ApiResult<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut tx = state.pool.begin().await?;
    // Inserting an Entry checks its Session FK before its Run FK. Acquire
    // Run first so that journal publication follows the Core/ACK lock order.
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM studio_builder_runs WHERE tenant_id=? AND session_id=? AND id=? FOR SHARE",
    )
    .bind(target.tenant_id)
    .bind(target.session_id)
    .bind(target.run_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Builder run"))?;
    for (key, payload) in entries {
        sqlx::query("INSERT INTO studio_builder_entries(tenant_id,session_id,run_id,event_key,payload_json) VALUES(?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id")
            .bind(target.tenant_id).bind(target.session_id).bind(target.run_id).bind(key).bind(payload).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn compaction_snapshot_id(
    state: &ControlApiState,
    target: &StudioBuilderTargetV1,
) -> ApiResult<String> {
    let core: Value = sqlx::query_scalar(
        "SELECT state_json FROM studio_builder_sessions WHERE tenant_id=? AND id=?",
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
