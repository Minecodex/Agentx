use bytes::Bytes;
use object_store::path::Path as ObjectPath;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use agentx_runtime_contracts::{ContentHash, RuntimeObjectReferenceV1};

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

pub async fn externalize_one(state: &RuntimeState) -> RuntimeResult<bool> {
    if let Some(row) = load_candidate(
        &state.pool,
        "SELECT id FROM workflow_executions e FORCE INDEX(idx_runtime_execution_input_size) WHERE input_size_bytes>? AND NOT EXISTS(SELECT 1 FROM artifact_references r WHERE r.tenant_id=e.tenant_id AND r.owner_type='execution' AND r.owner_id=CAST(BIN_TO_UUID(e.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci AND r.reference_role='workflow_input') ORDER BY id LIMIT 1",
        "SELECT id,tenant_id,input_json FROM workflow_executions WHERE id=? AND input_json IS NOT NULL",
        16 * 1024,
    ).await?
    {
        let execution_id: Uuid = row.try_get("id")?;
        let tenant_id: Uuid = row.try_get("tenant_id")?;
        let payload: Value = row.try_get("input_json")?;
        let artifact = persist_json(
            state,
            tenant_id,
            trace_object_id(execution_id, "input"),
            &payload,
            &format!("trace:{execution_id}:input"),
        )
        .await?;
        let mut tx = state.pool.begin().await?;
        insert_artifact_rows(&mut tx, tenant_id, &artifact).await?;
        sqlx::query("INSERT IGNORE INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'execution',?,'workflow_input')")
            .bind(tenant_id).bind(artifact.object_id).bind(execution_id.to_string()).execute(&mut *tx).await?;
        let mut trace = crate::trace_delivery::TraceDraft::execution(
            tenant_id, execution_id, "execution.input_externalized", "running",
        );
        trace.content_ref = Some(artifact.object_id);
        trace.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::WorkflowInput);
        trace.attributes = serde_json::json!({"contentExternalized":true,"encodedBytes":artifact.size_bytes});
        crate::trace_delivery::enqueue_best_effort(&mut tx, trace).await;
        tx.commit().await?;
        return Ok(true);
    }
    if let Some(row) = load_candidate(
        &state.pool,
        "SELECT id FROM node_attempts a FORCE INDEX(idx_node_attempt_input_size) WHERE input_size_bytes>? AND NOT EXISTS(SELECT 1 FROM artifact_references r WHERE r.tenant_id=a.tenant_id AND r.owner_type='node_attempt' AND r.owner_id=CAST(BIN_TO_UUID(a.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci AND r.reference_role='attempt_input') ORDER BY id LIMIT 1",
        "SELECT a.id,a.tenant_id,a.execution_id,a.node_execution_id,a.attempt_number,a.input_json,n.node_name FROM node_attempts a JOIN node_executions n ON n.id=a.node_execution_id WHERE a.id=? AND a.input_json IS NOT NULL",
        16 * 1024,
    ).await?
    {
        let attempt_id: Uuid = row.try_get("id")?;
        let tenant_id: Uuid = row.try_get("tenant_id")?;
        let execution_id: Uuid = row.try_get("execution_id")?;
        let node_execution_id: Uuid = row.try_get("node_execution_id")?;
        let payload: Value = row.try_get("input_json")?;
        let artifact = persist_json(
            state,
            tenant_id,
            trace_object_id(attempt_id, "input"),
            &payload,
            &format!("trace:{attempt_id}:input"),
        )
        .await?;
        let role = "attempt_input";
        let mut tx = state.pool.begin().await?;
        insert_artifact_rows(&mut tx, tenant_id, &artifact).await?;
        sqlx::query("INSERT IGNORE INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'node_execution',?,?)")
            .bind(tenant_id).bind(artifact.object_id).bind(node_execution_id.to_string()).bind(role).execute(&mut *tx).await?;
        sqlx::query("INSERT IGNORE INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'node_attempt',?,?)")
            .bind(tenant_id).bind(artifact.object_id).bind(attempt_id.to_string()).bind(role).execute(&mut *tx).await?;
        let mut node = crate::trace_delivery::TraceDraft::span(
            tenant_id, execution_id, node_execution_id,
            Some((execution_id, agentx_runtime_contracts::TraceSpanKindV1::Execution)),
            agentx_runtime_contracts::TraceSpanKindV1::Node, row.try_get::<String, _>("node_name")?,
            agentx_runtime_contracts::TraceEventKindV1::Updated, "node.input_externalized", "running",
        );
        node.node_execution_id = Some(node_execution_id);
        node.attempt_id = Some(attempt_id);
        node.content_ref = Some(artifact.object_id);
        node.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::NodeInput);
        node.attributes = serde_json::json!({"contentExternalized":true});
        crate::trace_delivery::enqueue_best_effort(&mut tx, node).await;
        let mut attempt = crate::trace_delivery::TraceDraft::span(
            tenant_id, execution_id, attempt_id,
            Some((node_execution_id, agentx_runtime_contracts::TraceSpanKindV1::Node)),
            agentx_runtime_contracts::TraceSpanKindV1::Attempt,
            format!("Attempt {}", row.try_get::<u16, _>("attempt_number")?),
            agentx_runtime_contracts::TraceEventKindV1::Updated, "attempt.input_externalized", "running",
        );
        attempt.node_execution_id = Some(node_execution_id);
        attempt.attempt_id = Some(attempt_id);
        attempt.content_ref = Some(artifact.object_id);
        attempt.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::AttemptInput);
        attempt.attributes = serde_json::json!({"contentExternalized":true,"encodedBytes":artifact.size_bytes});
        crate::trace_delivery::enqueue_best_effort(&mut tx, attempt).await;
        tx.commit().await?;
        return Ok(true);
    }
    if let Some(row) = load_candidate(
        &state.pool,
        "SELECT id FROM checkpoints FORCE INDEX(idx_checkpoint_externalize) WHERE payload_artifact_id IS NULL AND payload_size_bytes>? ORDER BY id LIMIT 1",
        "SELECT id,tenant_id,payload_hash,payload_json FROM checkpoints WHERE id=? AND payload_artifact_id IS NULL AND payload_json IS NOT NULL",
        agentx_runtime_contracts::INLINE_RESULT_LIMIT_BYTES,
    ).await?
    {
        let checkpoint_id: Uuid = row.try_get("id")?;
        let tenant_id: Uuid = row.try_get("tenant_id")?;
        let payload: Value = row.try_get("payload_json")?;
        let expected_hash: String = row.try_get("payload_hash")?;
        let artifact = persist_json(
            state,
            tenant_id,
            stable_id(checkpoint_id, b"checkpoint-payload"),
            &payload,
            &format!("checkpoint:{checkpoint_id}:payload"),
        )
        .await?;
        if artifact.content_hash.as_str() != expected_hash {
            return Err(invalid("CHECKPOINT_OBJECT_HASH_MISMATCH"));
        }
        let mut tx = state.pool.begin().await?;
        insert_artifact_rows(&mut tx, tenant_id, &artifact).await?;
        sqlx::query(
            "INSERT IGNORE INTO checkpoint_artifacts(tenant_id,checkpoint_id,artifact_id,role) VALUES(?,?,?,'state')",
        )
        .bind(tenant_id)
        .bind(checkpoint_id)
        .bind(artifact.object_id)
        .execute(&mut *tx)
        .await?;
        let changed = sqlx::query(
            "UPDATE checkpoints SET payload_json=NULL,payload_artifact_id=? WHERE tenant_id=? AND id=? AND payload_hash=? AND payload_artifact_id IS NULL",
        )
        .bind(artifact.object_id)
        .bind(tenant_id)
        .bind(checkpoint_id)
        .bind(&expected_hash)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(changed.rows_affected() == 1);
    }
    if let Some(row) = load_candidate(
        &state.pool,
        "SELECT id FROM workflow_executions FORCE INDEX(idx_runtime_terminal_externalize) WHERE terminal_result_object_id IS NULL AND terminal_result_size_bytes>? ORDER BY id LIMIT 1",
        "SELECT id,tenant_id,terminal_result_hash,terminal_result_json FROM workflow_executions WHERE id=? AND terminal_result_object_id IS NULL AND terminal_result_json IS NOT NULL",
        agentx_runtime_contracts::INLINE_RESULT_LIMIT_BYTES,
    ).await?
    {
        let execution_id: Uuid = row.try_get("id")?;
        let tenant_id: Uuid = row.try_get("tenant_id")?;
        let payload: Value = row.try_get("terminal_result_json")?;
        let expected_hash: String = row.try_get("terminal_result_hash")?;
        let artifact = persist_json(
            state,
            tenant_id,
            stable_id(execution_id, b"terminal-result"),
            &payload,
            &format!("execution:{execution_id}:terminal-result"),
        )
        .await?;
        if artifact.content_hash.as_str() != expected_hash {
            return Err(invalid("EXECUTION_RESULT_OBJECT_HASH_MISMATCH"));
        }
        let mut tx = state.pool.begin().await?;
        insert_artifact_rows(&mut tx, tenant_id, &artifact).await?;
        let changed = sqlx::query(
            "UPDATE workflow_executions SET output_json=NULL,terminal_result_json=NULL,terminal_result_object_id=? WHERE tenant_id=? AND id=? AND terminal_result_hash=? AND terminal_result_object_id IS NULL",
        )
        .bind(artifact.object_id)
        .bind(tenant_id)
        .bind(execution_id)
        .bind(&expected_hash)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE execution_runtime_state SET terminal_result_json=NULL,terminal_result_object_id=? WHERE tenant_id=? AND execution_id=? AND terminal_result_hash=?",
        )
        .bind(artifact.object_id)
        .bind(tenant_id)
        .bind(execution_id)
        .bind(&expected_hash)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE execution_snapshots SET output_json=NULL WHERE tenant_id=? AND execution_id=?",
        )
        .bind(tenant_id)
        .bind(execution_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(changed.rows_affected() == 1);
    }
    Ok(false)
}

async fn load_candidate(
    pool: &sqlx::MySqlPool,
    candidate_query: &str,
    payload_query: &str,
    minimum_size: u64,
) -> RuntimeResult<Option<sqlx::mysql::MySqlRow>> {
    // Discover candidates through the indexed size/owner columns. Fetch the
    // large immutable JSON by key, rechecking that retention or another
    // artifact worker has not already removed/externalized the payload.
    let id: Option<Uuid> = sqlx::query_scalar(candidate_query)
        .bind(minimum_size)
        .fetch_optional(pool)
        .await
        .inspect_err(|error| tracing::error!(%error, candidate_query, "Runtime artifact candidate query failed"))?;
    let Some(id) = id else { return Ok(None) };
    Ok(sqlx::query(payload_query)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn load_artifact(
    state: &RuntimeState,
    tenant_id: Uuid,
    artifact_id: Uuid,
    expected_hash: &str,
) -> RuntimeResult<Bytes> {
    let row = sqlx::query(
        "SELECT content_type,size_bytes,sha256,storage_key FROM artifacts WHERE tenant_id=? AND id=? AND deleted_at IS NULL",
    )
    .bind(tenant_id)
    .bind(artifact_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(RuntimeError::NotFound)?;
    let sha256: String = row.try_get("sha256")?;
    if row.try_get::<String, _>("content_type")? != "application/json"
        || format!("sha256:{sha256}") != expected_hash
    {
        return Err(invalid("RUNTIME_ARTIFACT_METADATA_INVALID"));
    }
    let bytes = state
        .objects
        .get(&ObjectPath::from(row.try_get::<String, _>("storage_key")?))
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?
        .bytes()
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    if bytes.len() as u64 != row.try_get::<u64, _>("size_bytes")?
        || format!("{:x}", Sha256::digest(&bytes)) != sha256
    {
        return Err(invalid("RUNTIME_ARTIFACT_CONTENT_INVALID"));
    }
    Ok(bytes)
}

async fn persist_json(
    state: &RuntimeState,
    tenant_id: Uuid,
    object_id: Uuid,
    payload: &Value,
    idempotency_key: &str,
) -> RuntimeResult<RuntimeObjectReferenceV1> {
    let bytes = agentx_runtime_contracts::canonical_bytes(payload)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let content_hash = ContentHash::parse(format!("sha256:{:x}", Sha256::digest(&bytes)))
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let object_key = RuntimeObjectReferenceV1::canonical_key(tenant_id, object_id, &content_hash);
    state
        .objects
        .put(
            &ObjectPath::from(object_key.clone()),
            Bytes::from(bytes.clone()).into(),
        )
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let request_hash = agentx_runtime_contracts::content_hash(&serde_json::json!({
        "idempotencyKey":idempotency_key,
        "contentHash":content_hash,
        "sizeBytes":bytes.len(),
    }))
    .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query(
        "INSERT INTO runtime_objects(object_id,tenant_id,object_key,content_hash,size_bytes,media_type,status,idempotency_key,request_hash,temporary_expires_at,ready_at) VALUES(?,?,?,?,?,'application/json','ready',?,?,UTC_TIMESTAMP(6),UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE object_id=IF(content_hash=VALUES(content_hash) AND size_bytes=VALUES(size_bytes),object_id,NULL)",
    )
    .bind(object_id)
    .bind(tenant_id)
    .bind(&object_key)
    .bind(content_hash.as_str())
    .bind(bytes.len() as u64)
    .bind(idempotency_key)
    .bind(request_hash.as_str())
    .execute(&state.pool)
    .await?;
    Ok(RuntimeObjectReferenceV1 {
        tenant_id,
        storage_domain: agentx_runtime_contracts::StorageDomain::Runtime,
        object_id,
        object_key,
        content_hash,
        size_bytes: bytes.len() as u64,
        media_type: "application/json".into(),
    })
}

async fn insert_artifact_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant_id: Uuid,
    artifact: &RuntimeObjectReferenceV1,
) -> RuntimeResult<()> {
    let sha256 = artifact
        .content_hash
        .as_str()
        .strip_prefix("sha256:")
        .ok_or_else(|| invalid("RUNTIME_ARTIFACT_HASH_INVALID"))?;
    sqlx::query(
        "INSERT INTO artifacts(id,tenant_id,content_type,size_bytes,sha256,storage_key) VALUES(?,?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id",
    )
    .bind(artifact.object_id)
    .bind(tenant_id)
    .bind(&artifact.media_type)
    .bind(artifact.size_bytes)
    .bind(sha256)
    .bind(&artifact.object_key)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn stable_id(namespace: Uuid, label: &[u8]) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update(label);
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn trace_object_id(entity_id: Uuid, role: &str) -> Uuid {
    agentx_runtime_contracts::deterministic_uuid(
        entity_id,
        format!("agentx-trace-content-v1:{role}").as_bytes(),
    )
}

fn invalid(code: &str) -> RuntimeError {
    RuntimeError::BadRequest(
        agentx_runtime_contracts::RuntimePublishErrorCodeV1::ObjectHashMismatch,
        code.into(),
    )
}
