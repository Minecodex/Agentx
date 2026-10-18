//! Judge authority is separate from the business Workflow's resource grants.
use crate::error::{RuntimeError, RuntimeResult};
use agentx_runtime_contracts::{
    RuntimePlatformEvaluationAuthorizationV1, RuntimePlatformEvaluationModelV1, content_hash,
};
use serde_json::Value;
use sqlx::{MySql, Transaction};
use uuid::Uuid;

pub(crate) async fn prepare(
    tx: &mut Transaction<'_, MySql>,
    model: &RuntimePlatformEvaluationModelV1,
) -> RuntimeResult<()> {
    let hash = content_hash(model).map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query("INSERT IGNORE INTO platform_evaluation_authorizations(tenant_id,application_id,batch_id,model_hash,epoch,enabled) VALUES(?,?,?,?,?,TRUE)")
        .bind(model.tenant_id).bind(model.application_id).bind(model.batch_id).bind(hash.as_str()).bind(model.setting_version).execute(&mut **tx).await?;
    authorize(
        tx,
        model.tenant_id,
        &serde_json::json!({"platformEvaluation":model}),
    )
    .await
}
pub(crate) async fn apply(
    tx: &mut Transaction<'_, MySql>,
    state: &RuntimePlatformEvaluationAuthorizationV1,
) -> RuntimeResult<()> {
    sqlx::query("INSERT INTO platform_evaluation_authorizations(tenant_id,application_id,batch_id,model_hash,epoch,enabled) VALUES(?,?,?,?,?,?) ON DUPLICATE KEY UPDATE enabled=IF(epoch<=VALUES(epoch),VALUES(enabled),enabled),epoch=GREATEST(epoch,VALUES(epoch))")
        .bind(state.tenant_id).bind(state.application_id).bind(state.batch_id).bind(state.model_hash.as_str()).bind(state.epoch).bind(state.enabled).execute(&mut **tx).await?;
    Ok(())
}
pub(crate) async fn authorize(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    settings: &Value,
) -> RuntimeResult<()> {
    let Some(value) = settings.get("platformEvaluation") else {
        return Ok(());
    };
    let model: RuntimePlatformEvaluationModelV1 = serde_json::from_value(value.clone())
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let hash = content_hash(&model).map_err(|error| RuntimeError::Internal(error.into()))?;
    // Use a locking current read: an older repeatable-read snapshot cannot
    // issue a new Provider lease after the revocation has committed.
    let valid: Option<bool> = sqlx::query_scalar("SELECT enabled AND epoch>=? AND model_hash=? FROM platform_evaluation_authorizations WHERE tenant_id=? AND application_id=? AND batch_id=? FOR SHARE")
        .bind(model.setting_version).bind(hash.as_str()).bind(tenant).bind(model.application_id).bind(model.batch_id).fetch_optional(&mut **tx).await?;
    if model.tenant_id != tenant || valid != Some(true) {
        return Err(RuntimeError::InvalidRequest(
            "PLATFORM_EVALUATION_AUTHORIZATION_REVOKED",
            "The frozen platform evaluation model authorization is unavailable or revoked".into(),
        ));
    }
    Ok(())
}

/// Settle a denied queued attempt through the normal result/convergence path,
/// without exposing a usable lease to a Provider worker.
pub(crate) async fn reject_claim(
    pool: &sqlx::MySqlPool,
    task: &agentx_runtime_contracts::WorkerTaskV1,
    worker: Uuid,
    fencing_token: u64,
    code: &'static str,
    message: String,
) -> RuntimeResult<()> {
    use agentx_runtime_contracts::{WorkerResultStatusV1, WorkerResultV1};
    let outputs = std::collections::BTreeMap::new();
    let result_hash = crate::engine::worker_result_hash(
        WorkerResultStatusV1::Failed,
        &outputs,
        None,
        Some(code),
        Some(&message),
        Some(false),
        None,
    )
    .map_err(|error| RuntimeError::Internal(error.into()))?;
    crate::engine::submit_worker_result(
        pool,
        &WorkerResultV1 {
            protocol_version: 1,
            attempt_id: task.attempt_id,
            worker_id: worker,
            fencing_token,
            status: WorkerResultStatusV1::Failed,
            result_hash,
            outputs,
            output_object: None,
            error_code: Some(code.into()),
            error_message: Some(message),
            retryable: Some(false),
            partial_output_object: None,
        },
    )
    .await?;
    Ok(())
}
