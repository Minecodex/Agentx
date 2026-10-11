use std::{collections::BTreeMap, sync::Arc};

use agentx_node_protocol::Item;
use agentx_runtime_contracts::{
    ContentHash, RuntimePublishErrorCodeV1, WorkerResultStatusV1, WorkerResultV1, WorkerTaskV1,
};
use object_store::{ObjectStore, path::Path as ObjectPath};
use serde_json::json;
use sha2::Digest as _;
use sqlx::{MySql, MySqlPool, Row, Transaction};
use uuid::Uuid;

use crate::{
    error::{RuntimeError, RuntimeResult},
    execution::RuntimeCommandClaim,
};

pub(crate) async fn complete_command(
    tx: &mut Transaction<'_, MySql>,
    claim: &RuntimeCommandClaim,
    result: serde_json::Value,
) -> RuntimeResult<()> {
    let updated = sqlx::query(
        "UPDATE runtime_commands SET status='completed',result_json=?,completed_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(result)
    .bind(claim.command_id)
    .bind(claim.owner_id)
    .bind(claim.fencing_token)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(lease_conflict("Runtime command Lease was lost"));
    }
    Ok(())
}

pub(crate) fn ensure_command_lease(
    row: &sqlx::mysql::MySqlRow,
    claim: &RuntimeCommandClaim,
) -> RuntimeResult<()> {
    if row.try_get::<Option<Uuid>, _>("locked_by")? != Some(claim.owner_id)
        || row.try_get::<u64, _>("fencing_token")? != claim.fencing_token
        || !row.try_get::<bool, _>("lease_active")?
    {
        return Err(lease_conflict("Runtime command Lease was lost"));
    }
    Ok(())
}

pub(crate) fn ensure_task_matches(
    row: &sqlx::mysql::MySqlRow,
    capability: &str,
    task: &WorkerTaskV1,
) -> RuntimeResult<()> {
    let bindings = [
        (
            "tenant",
            row.try_get::<Uuid, _>("tenant_id")? == task.tenant_id,
        ),
        (
            "execution",
            row.try_get::<Uuid, _>("execution_id")? == task.execution_id,
        ),
        (
            "node_execution",
            row.try_get::<Uuid, _>("node_execution_id")? == task.node_execution_id,
        ),
        (
            "attempt_capability",
            row.try_get::<String, _>("capability")? == capability,
        ),
        ("task_capability", task.capability.as_str() == capability),
        (
            "protocol_version",
            row.try_get::<u16, _>("worker_protocol_version")? == task.protocol_version as u16,
        ),
    ];
    let mismatches: Vec<_> = bindings
        .iter()
        .filter_map(|(name, matches)| (!matches).then_some(*name))
        .collect();
    if !mismatches.is_empty() {
        return Err(runtime_bad_request(
            "WORKER_TASK_MISMATCH",
            &format!(
                "Worker Task does not match the authoritative Attempt: {}",
                mismatches.join(", ")
            ),
        ));
    }
    Ok(())
}

pub(crate) async fn resolve_worker_outputs(
    pool: &MySqlPool,
    objects: Arc<dyn ObjectStore>,
    result: &WorkerResultV1,
) -> RuntimeResult<BTreeMap<String, Vec<Item>>> {
    let tenant_id: Uuid = sqlx::query_scalar("SELECT tenant_id FROM node_attempts WHERE id=?")
        .bind(result.attempt_id)
        .fetch_optional(pool)
        .await?
        .ok_or(RuntimeError::NotFound)?;
    if let Some(partial) = &result.partial_output_object {
        validate_runtime_object(pool, objects.as_ref(), tenant_id, partial).await?;
    }
    let Some(reference) = &result.output_object else {
        let encoded = agentx_runtime_contracts::canonical_bytes(&result.outputs)
            .map_err(|error| RuntimeError::Internal(error.into()))?;
        if encoded.len() > agentx_runtime_contracts::INLINE_RESULT_LIMIT_BYTES as usize {
            return Err(runtime_bad_request(
                "WORKER_RESULT_EXTERNALIZATION_REQUIRED",
                "Worker Result larger than 64 KiB must use a Runtime object",
            ));
        }
        return Ok(result.outputs.clone());
    };
    if !result.outputs.is_empty() {
        return Err(runtime_bad_request(
            "WORKER_RESULT_INLINE_AND_OBJECT_CONFLICT",
            "external Worker Result cannot also carry inline outputs",
        ));
    }
    let bytes = validate_runtime_object(pool, objects.as_ref(), tenant_id, reference).await?;
    serde_json::from_slice(&bytes).map_err(|error| {
        runtime_bad_request(
            "WORKER_RESULT_OBJECT_INVALID",
            &format!("external Worker Result is not the expected JSON payload: {error}"),
        )
    })
}

async fn validate_runtime_object(
    pool: &MySqlPool,
    objects: &dyn ObjectStore,
    tenant_id: Uuid,
    reference: &agentx_runtime_contracts::RuntimeObjectReferenceV1,
) -> RuntimeResult<bytes::Bytes> {
    if reference.tenant_id != tenant_id
        || !reference.has_canonical_key()
        || reference.media_type != "application/json"
    {
        return Err(runtime_bad_request(
            "WORKER_RESULT_OBJECT_INVALID",
            "Worker Result object tenant, key or media type is invalid",
        ));
    }
    let ready: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_objects WHERE tenant_id=? AND object_id=? AND object_key=? AND content_hash=? AND size_bytes=? AND media_type=? AND status='ready')",
    )
    .bind(tenant_id)
    .bind(reference.object_id)
    .bind(&reference.object_key)
    .bind(reference.content_hash.as_str())
    .bind(reference.size_bytes)
    .bind(&reference.media_type)
    .fetch_one(pool)
    .await?;
    if !ready {
        return Err(runtime_bad_request(
            "WORKER_RESULT_OBJECT_MISSING",
            "Worker Result object is not ready in Runtime metadata",
        ));
    }
    let bytes = objects
        .get(&ObjectPath::from(reference.object_key.clone()))
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?
        .bytes()
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let actual = ContentHash::parse(format!("sha256:{:x}", sha2::Sha256::digest(&bytes)))
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    if bytes.len() as u64 != reference.size_bytes || actual != reference.content_hash {
        return Err(runtime_bad_request(
            "WORKER_RESULT_OBJECT_INVALID",
            "Worker Result object content does not match immutable metadata",
        ));
    }
    Ok(bytes)
}

pub(crate) fn ensure_result_lease(
    row: &sqlx::mysql::MySqlRow,
    result: &WorkerResultV1,
) -> RuntimeResult<()> {
    if row.try_get::<String, _>("status")? != "running"
        || row.try_get::<Option<Uuid>, _>("lease_token")? != Some(result.worker_id)
        || row.try_get::<u64, _>("fencing_token")? != result.fencing_token
        || !row.try_get::<bool, _>("lease_active")?
    {
        return Err(lease_conflict("Attempt Lease was lost"));
    }
    Ok(())
}

pub(crate) fn validate_result_hash(result: &WorkerResultV1) -> RuntimeResult<()> {
    if result.validate_integrity().is_err() {
        return Err(runtime_bad_request(
            "WORKER_RESULT_HASH_MISMATCH",
            "Worker Result hash or immutable object reference is invalid",
        ));
    }
    Ok(())
}

pub(crate) const fn node_event_type(status: WorkerResultStatusV1) -> &'static str {
    match status {
        WorkerResultStatusV1::Succeeded => "node.completed",
        WorkerResultStatusV1::Failed => "node.failed",
        WorkerResultStatusV1::Suspended => "node.suspended",
        WorkerResultStatusV1::Cancelled => "node.cancelled",
        WorkerResultStatusV1::OutcomeUnknown => "node.outcome_unknown",
    }
}

pub fn worker_result_hash(
    status: WorkerResultStatusV1,
    outputs: &BTreeMap<String, Vec<Item>>,
    output_object: Option<&agentx_runtime_contracts::RuntimeObjectReferenceV1>,
    error_code: Option<&str>,
    error_message: Option<&str>,
    retryable: Option<bool>,
    partial_output_object: Option<&agentx_runtime_contracts::RuntimeObjectReferenceV1>,
) -> RuntimeResult<ContentHash> {
    agentx_runtime_contracts::content_hash(&json!({
        "status": status,
        "outputs": outputs,
        "outputObject": output_object,
        "errorCode": error_code,
        "errorMessage": error_message,
        "retryable": retryable,
        "partialOutputObject": partial_output_object,
    }))
    .map_err(|error| RuntimeError::Internal(error.into()))
}

pub(crate) fn machine_error(error: agentx_runtime::MachineError) -> RuntimeError {
    RuntimeError::Internal(anyhow::anyhow!(error))
}

pub(crate) fn lease_conflict(message: &str) -> RuntimeError {
    RuntimeError::Conflict(
        RuntimePublishErrorCodeV1::IdempotencyConflict,
        message.into(),
    )
}

pub(crate) fn runtime_bad_request(code: &str, message: &str) -> RuntimeError {
    RuntimeError::BadRequest(
        RuntimePublishErrorCodeV1::UnsupportedCapability,
        format!("{code}: {message}"),
    )
}

#[cfg(test)]
mod tests {
    use agentx_runtime_contracts::WorkerResultStatusV1;

    use super::node_event_type;

    #[test]
    fn node_event_types_keep_completion_distinct_from_storage_status() {
        assert_eq!(
            node_event_type(WorkerResultStatusV1::Succeeded),
            "node.completed"
        );
        assert_eq!(
            node_event_type(WorkerResultStatusV1::Suspended),
            "node.suspended"
        );
    }
}
