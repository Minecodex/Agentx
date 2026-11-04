//! Batch history follows the existing enabled execution retention policy.
use super::*;

pub(crate) async fn mark(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    run: Uuid,
    days: u32,
    limit: u32,
) -> RuntimeResult<u64> {
    let rows=sqlx::query("SELECT b.id,EXISTS(SELECT 1 FROM application_batch_items i LEFT JOIN application_invocations v ON v.tenant_id=i.tenant_id AND v.id=i.invocation_id WHERE i.tenant_id=b.tenant_id AND i.batch_id=b.id AND (i.dispatch_state='pending' OR v.status IN ('queued','running') OR v.completed_at>DATE_SUB(UTC_TIMESTAMP(6),INTERVAL ? DAY))) OR EXISTS(SELECT 1 FROM runtime_commands c WHERE c.tenant_id=b.tenant_id AND c.aggregate_type='batch' AND c.aggregate_id=CAST(BIN_TO_UUID(b.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci AND c.status IN ('pending','processing')) OR EXISTS(SELECT 1 FROM application_batches child WHERE child.tenant_id=b.tenant_id AND child.source_batch_id=b.id AND child.retention_deleted_at IS NULL) OR EXISTS(SELECT 1 FROM runtime_retention_holds h WHERE h.tenant_id=b.tenant_id AND h.aggregate_type='application_batch' AND h.aggregate_id=b.id AND h.released_at IS NULL AND (h.expires_at IS NULL OR h.expires_at>UTC_TIMESTAMP(6))) AS protected FROM application_batches b WHERE b.tenant_id=? AND b.dispatch_state<>'active' AND b.updated_at<DATE_SUB(UTC_TIMESTAMP(6),INTERVAL ? DAY) AND b.retention_deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM retention_items r WHERE r.retention_run_id=? AND r.data_type='application_batch' AND r.target_id=CAST(BIN_TO_UUID(b.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci) ORDER BY b.updated_at,b.id LIMIT ? FOR UPDATE SKIP LOCKED")
        .bind(days).bind(tenant).bind(days).bind(run).bind(limit).fetch_all(&mut **tx).await?;
    let count = rows.len() as u64;
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let protected: bool = row.try_get("protected")?;
        sqlx::query("INSERT INTO retention_items(id,tenant_id,retention_run_id,data_type,target_id,status,reason) VALUES(?,?,?,'application_batch',?,?,?)").bind(Uuid::now_v7()).bind(tenant).bind(run).bind(id.to_string()).bind(if protected{"blocked"}else{"candidate"}).bind(if protected{Some("live_reference")}else{None}).execute(&mut **tx).await?;
    }
    Ok(count)
}

pub(crate) async fn purge(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    id: Uuid,
) -> RuntimeResult<bool> {
    let batch=sqlx::query("SELECT * FROM application_batches WHERE tenant_id=? AND id=? AND retention_deleted_at IS NULL FOR UPDATE").bind(tenant).bind(id).fetch_optional(&mut **tx).await?;
    let Some(batch) = batch else { return Ok(false) };
    if batch.try_get::<String, _>("dispatch_state")? == "active" {
        return Ok(false);
    }
    let projects = super::read::views_tx(tx, &batch).await?;
    if projects
        .iter()
        .any(|i| !matches!(i.state.as_str(), "completed" | "failed" | "cancelled"))
    {
        return Ok(false);
    }
    let protected:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_commands WHERE tenant_id=? AND aggregate_type='batch' AND aggregate_id=? AND status IN ('pending','processing')) OR EXISTS(SELECT 1 FROM application_batches WHERE tenant_id=? AND source_batch_id=? AND retention_deleted_at IS NULL)")
        .bind(tenant).bind(id.to_string()).bind(tenant).bind(id).fetch_one(&mut **tx).await?;
    if protected {
        return Ok(false);
    }
    sqlx::query("DELETE r FROM artifact_references r JOIN application_batch_items i ON i.tenant_id=r.tenant_id AND r.owner_type='batch_item' AND r.owner_id=CAST(BIN_TO_UUID(i.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci WHERE i.tenant_id=? AND i.batch_id=?").bind(tenant).bind(id).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM artifact_references WHERE tenant_id=? AND owner_type='batch_export' AND owner_id=?").bind(tenant).bind(id.to_string()).execute(&mut **tx).await?;
    sqlx::query("UPDATE bundle_references SET released_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND reference_kind IN ('application_batch','batch_page') AND owner_id=? AND released_at IS NULL").bind(tenant).bind(id).execute(&mut **tx).await?;
    sqlx::query("UPDATE application_batch_items SET input_json=CAST('null' AS JSON),user_identity_json=NULL,error_json=NULL WHERE tenant_id=? AND batch_id=?").bind(tenant).bind(id).execute(&mut **tx).await?;
    sqlx::query("UPDATE runtime_commands SET payload_json=JSON_OBJECT('batchId',?),result_json=NULL WHERE tenant_id=? AND aggregate_type='batch' AND aggregate_id=? AND status IN ('completed','failed')").bind(id.to_string()).bind(tenant).bind(id.to_string()).execute(&mut **tx).await?;
    sqlx::query("UPDATE application_batches SET retention_deleted_at=UTC_TIMESTAMP(6),caller_json=JSON_OBJECT(),identity_policy_json=JSON_OBJECT(),name='[retained tombstone]' WHERE tenant_id=? AND id=?").bind(tenant).bind(id).execute(&mut **tx).await?;
    Ok(true)
}
