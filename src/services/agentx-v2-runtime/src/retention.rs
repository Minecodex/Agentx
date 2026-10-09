use std::time::Duration;

use object_store::path::Path as ObjectPath;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

const BATCH_SIZE: u32 = 100;
const LEASE_SECONDS: u32 = 30;
const OBJECT_DELETE_TIMEOUT_SECONDS: u64 = 20;

#[derive(Clone, Debug)]
struct RetentionRunClaim {
    run_id: Uuid,
    tenant_id: Uuid,
    owner: Uuid,
    fencing_token: u64,
    dry_run: bool,
}

pub async fn run_once(state: &RuntimeState, owner: Uuid) -> RuntimeResult<bool> {
    let Some(claim) = claim_run(&state.pool, owner).await? else {
        return Ok(false);
    };
    mark_candidates(state, &claim).await?;
    if claim.dry_run {
        complete_dry_run(state, &claim).await?;
        return Ok(true);
    }
    for _ in 0..BATCH_SIZE {
        if sweep_artifact(state, &claim).await? {
            continue;
        }
        if !sweep_database_record(state, &claim).await? {
            break;
        }
    }
    complete_if_drained(state, &claim).await?;
    Ok(true)
}

async fn claim_run(
    pool: &sqlx::MySqlPool,
    owner: Uuid,
) -> RuntimeResult<Option<RetentionRunClaim>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT id,tenant_id,dry_run FROM retention_runs WHERE (status='queued' AND available_at<=UTC_TIMESTAMP(6)) OR (status='running' AND locked_until<=UTC_TIMESTAMP(6)) ORDER BY available_at,created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let run_id: Uuid = row.try_get("id")?;
    sqlx::query(
        "UPDATE retention_runs SET status='running',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1,started_at=COALESCE(started_at,UTC_TIMESTAMP(6)) WHERE id=?",
    )
    .bind(owner)
    .bind(LEASE_SECONDS)
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    let fencing_token: u64 =
        sqlx::query_scalar("SELECT fencing_token FROM retention_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
    let claim = RetentionRunClaim {
        run_id,
        tenant_id: row.try_get("tenant_id")?,
        owner,
        fencing_token,
        dry_run: row.try_get("dry_run")?,
    };
    tx.commit().await?;
    Ok(Some(claim))
}

async fn mark_candidates(state: &RuntimeState, claim: &RetentionRunClaim) -> RuntimeResult<()> {
    let mut tx = state.pool.begin().await?;
    ensure_run_lease(&mut tx, claim).await?;
    let policies = sqlx::query(
        "SELECT data_type,retention_days FROM retention_policy_projection WHERE tenant_id=? AND enabled=TRUE ORDER BY data_type",
    )
    .bind(claim.tenant_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut marked = 0_u64;
    for policy in policies {
        if marked >= u64::from(BATCH_SIZE) {
            break;
        }
        let data_type: String = policy.try_get("data_type")?;
        let limit = BATCH_SIZE - u32::try_from(marked).unwrap_or(BATCH_SIZE);
        let retention_days = policy.try_get::<u32, _>("retention_days")?;
        let rows = match data_type.as_str() {
            "artifact" => mark_artifact_rows(&mut tx, claim, retention_days, limit).await?,
            "execution" => mark_execution_rows(&mut tx, claim, retention_days, limit).await?,
            "application_message" => {
                mark_message_rows(&mut tx, claim, retention_days, limit).await?
            }
            "evaluation_report" => {
                mark_evaluation_rows(&mut tx, claim, retention_days, limit).await?
            }
            // Trace is owned by the V2-05 Observability retention path.
            "trace" => continue,
            _ => continue,
        };
        for row in rows {
            let target_id: Uuid = row.try_get("id")?;
            let protected: bool = row.try_get("protected")?;
            sqlx::query(
                "INSERT INTO retention_items(id,tenant_id,retention_run_id,data_type,target_id,status,reason,object_id) VALUES(?,?,?,?,?,?,?,?)",
            )
            .bind(Uuid::now_v7())
            .bind(claim.tenant_id)
            .bind(claim.run_id)
            .bind(&data_type)
            .bind(target_id.to_string())
            .bind(if protected { "blocked" } else { "candidate" })
            .bind(if protected { Some("live_reference") } else { None })
            .bind(if data_type == "artifact" {
                Some(target_id)
            } else {
                None
            })
            .execute(&mut *tx)
            .await?;
            marked += 1;
        }
    }
    let changed = sqlx::query(
        "UPDATE retention_runs SET candidate_count=(SELECT COUNT(*) FROM retention_items WHERE retention_run_id=?) WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(claim.run_id)
    .bind(claim.run_id)
    .bind(claim.owner)
    .bind(claim.fencing_token)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(lease_lost("Retention Run while marking"));
    }
    tx.commit().await?;
    Ok(())
}

async fn mark_artifact_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    claim: &RetentionRunClaim,
    retention_days: u32,
    limit: u32,
) -> RuntimeResult<Vec<sqlx::mysql::MySqlRow>> {
    Ok(sqlx::query(
        "SELECT a.id,EXISTS(SELECT 1 FROM artifact_references r WHERE r.tenant_id=a.tenant_id AND r.artifact_id=a.id AND (r.retention_until IS NULL OR r.retention_until>UTC_TIMESTAMP(6))) OR EXISTS(SELECT 1 FROM runtime_retention_holds h WHERE h.tenant_id=a.tenant_id AND h.aggregate_type='artifact' AND h.aggregate_id=a.id AND h.released_at IS NULL AND (h.expires_at IS NULL OR h.expires_at>UTC_TIMESTAMP(6))) AS protected FROM artifacts a WHERE a.tenant_id=? AND a.deleted_at IS NULL AND a.created_at<DATE_SUB(UTC_TIMESTAMP(6),INTERVAL ? DAY) AND NOT EXISTS(SELECT 1 FROM retention_items i WHERE i.retention_run_id=? AND i.data_type='artifact' AND i.object_id=a.id) ORDER BY a.created_at,a.id LIMIT ? FOR UPDATE SKIP LOCKED",
    )
    .bind(claim.tenant_id)
    .bind(retention_days)
    .bind(claim.run_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?)
}

async fn mark_execution_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    claim: &RetentionRunClaim,
    retention_days: u32,
    limit: u32,
) -> RuntimeResult<Vec<sqlx::mysql::MySqlRow>> {
    Ok(sqlx::query(
        "SELECT e.id,EXISTS(SELECT 1 FROM checkpoints c WHERE c.tenant_id=e.tenant_id AND c.execution_id=e.id) OR EXISTS(SELECT 1 FROM execution_forks f WHERE f.tenant_id=e.tenant_id AND f.source_execution_id=e.id) OR EXISTS(SELECT 1 FROM runtime_retention_holds h WHERE h.tenant_id=e.tenant_id AND h.aggregate_type='execution' AND h.aggregate_id=e.id AND h.released_at IS NULL AND (h.expires_at IS NULL OR h.expires_at>UTC_TIMESTAMP(6))) AS protected FROM workflow_executions e WHERE e.tenant_id=? AND e.status IN ('succeeded','failed','cancelled','timed_out') AND e.ended_at<DATE_SUB(UTC_TIMESTAMP(6),INTERVAL ? DAY) AND e.retention_deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM retention_items i WHERE i.retention_run_id=? AND i.data_type='execution' AND i.target_id=CAST(BIN_TO_UUID(e.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci) ORDER BY e.ended_at,e.id LIMIT ? FOR UPDATE SKIP LOCKED",
    )
    .bind(claim.tenant_id)
    .bind(retention_days)
    .bind(claim.run_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?)
}

async fn mark_message_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    claim: &RetentionRunClaim,
    retention_days: u32,
    limit: u32,
) -> RuntimeResult<Vec<sqlx::mysql::MySqlRow>> {
    Ok(sqlx::query(
        "SELECT m.id,EXISTS(SELECT 1 FROM artifact_references r WHERE r.tenant_id=m.tenant_id AND r.owner_type='message' AND r.owner_id=CAST(BIN_TO_UUID(m.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci AND (r.retention_until IS NULL OR r.retention_until>UTC_TIMESTAMP(6))) OR EXISTS(SELECT 1 FROM runtime_retention_holds h WHERE h.tenant_id=m.tenant_id AND h.aggregate_type='application_message' AND h.aggregate_id=m.id AND h.released_at IS NULL AND (h.expires_at IS NULL OR h.expires_at>UTC_TIMESTAMP(6))) AS protected FROM application_messages m WHERE m.tenant_id=? AND m.created_at<DATE_SUB(UTC_TIMESTAMP(6),INTERVAL ? DAY) AND m.retention_deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM retention_items i WHERE i.retention_run_id=? AND i.data_type='application_message' AND i.target_id=CAST(BIN_TO_UUID(m.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci) ORDER BY m.created_at,m.id LIMIT ? FOR UPDATE SKIP LOCKED",
    )
    .bind(claim.tenant_id)
    .bind(retention_days)
    .bind(claim.run_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?)
}

async fn mark_evaluation_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    claim: &RetentionRunClaim,
    retention_days: u32,
    limit: u32,
) -> RuntimeResult<Vec<sqlx::mysql::MySqlRow>> {
    Ok(sqlx::query(
        "SELECT r.id,EXISTS(SELECT 1 FROM runtime_retention_holds h WHERE h.tenant_id=r.tenant_id AND h.aggregate_type='evaluation_report' AND h.aggregate_id=r.id AND h.released_at IS NULL AND (h.expires_at IS NULL OR h.expires_at>UTC_TIMESTAMP(6))) AS protected FROM evaluation_runs r WHERE r.tenant_id=? AND r.status IN ('completed','failed','cancelled') AND r.completed_at<DATE_SUB(UTC_TIMESTAMP(6),INTERVAL ? DAY) AND r.retention_deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM retention_items i WHERE i.retention_run_id=? AND i.data_type='evaluation_report' AND i.target_id=CAST(BIN_TO_UUID(r.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci) ORDER BY r.completed_at,r.id LIMIT ? FOR UPDATE SKIP LOCKED",
    )
    .bind(claim.tenant_id)
    .bind(retention_days)
    .bind(claim.run_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?)
}

async fn sweep_artifact(state: &RuntimeState, claim: &RetentionRunClaim) -> RuntimeResult<bool> {
    let mut tx = state.pool.begin().await?;
    ensure_run_lease(&mut tx, claim).await?;
    let row = sqlx::query(
        "SELECT i.id,i.object_id,CAST(a.storage_key AS CHAR CHARACTER SET utf8mb4) storage_key FROM retention_items i JOIN artifacts a ON a.tenant_id=i.tenant_id AND a.id=i.object_id WHERE i.retention_run_id=? AND i.data_type='artifact' AND ((i.status IN ('candidate','failed') AND (i.locked_until IS NULL OR i.locked_until<=UTC_TIMESTAMP(6))) OR (i.status='deleting' AND i.locked_until<=UTC_TIMESTAMP(6))) ORDER BY i.created_at,i.id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .bind(claim.run_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(false);
    };
    let item_id: Uuid = row.try_get("id")?;
    let artifact_id: Uuid = row.try_get("object_id")?;
    let protected: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM artifact_references WHERE tenant_id=? AND artifact_id=? AND (retention_until IS NULL OR retention_until>UTC_TIMESTAMP(6))) OR EXISTS(SELECT 1 FROM runtime_retention_holds WHERE tenant_id=? AND aggregate_type='artifact' AND aggregate_id=? AND released_at IS NULL AND (expires_at IS NULL OR expires_at>UTC_TIMESTAMP(6)))",
    )
    .bind(claim.tenant_id)
    .bind(artifact_id)
    .bind(claim.tenant_id)
    .bind(artifact_id)
    .fetch_one(&mut *tx)
    .await?;
    if protected {
        sqlx::query(
            "UPDATE retention_items SET status='blocked',reason='live_reference',locked_by=NULL,locked_until=NULL WHERE id=?",
        )
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(true);
    }
    sqlx::query(
        "UPDATE retention_items SET status='deleting',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=?",
    )
    .bind(claim.owner)
    .bind(LEASE_SECONDS)
    .bind(item_id)
    .execute(&mut *tx)
    .await?;
    let item_token: u64 =
        sqlx::query_scalar("SELECT fencing_token FROM retention_items WHERE id=?")
            .bind(item_id)
            .fetch_one(&mut *tx)
            .await?;
    let storage_key: String = row.try_get("storage_key")?;
    tx.commit().await?;

    let deletion = tokio::time::timeout(
        Duration::from_secs(OBJECT_DELETE_TIMEOUT_SECONDS),
        state.objects.delete(&ObjectPath::from(storage_key)),
    )
    .await;
    let failure = match deletion {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(_) => Some(format!(
            "Runtime object deletion exceeded {OBJECT_DELETE_TIMEOUT_SECONDS} seconds"
        )),
    };
    if let Some(error) = failure {
        fail_item(state, item_id, claim.owner, item_token, &error).await?;
        return Ok(false);
    }

    let mut tx = state.pool.begin().await?;
    ensure_item_lease(&mut tx, item_id, claim.owner, item_token).await?;
    sqlx::query(
        "UPDATE artifacts SET deleted_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND deleted_at IS NULL",
    )
    .bind(claim.tenant_id)
    .bind(artifact_id)
    .execute(&mut *tx)
    .await?;
    let changed = sqlx::query(
        "UPDATE retention_items SET status='deleted',deleted_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(item_id)
    .bind(claim.owner)
    .bind(item_token)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(lease_lost("Retention Item after object deletion"));
    }
    sqlx::query("UPDATE retention_runs SET deleted_count=deleted_count+1 WHERE id=?")
        .bind(claim.run_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

async fn sweep_database_record(
    state: &RuntimeState,
    claim: &RetentionRunClaim,
) -> RuntimeResult<bool> {
    let mut tx = state.pool.begin().await?;
    ensure_run_lease(&mut tx, claim).await?;
    let row = sqlx::query(
        "SELECT id,data_type,target_id FROM retention_items WHERE retention_run_id=? AND data_type IN ('execution','application_message','evaluation_report') AND status IN ('candidate','failed') AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .bind(claim.run_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(false);
    };
    let item_id: Uuid = row.try_get("id")?;
    let data_type: String = row.try_get("data_type")?;
    let target_id = Uuid::parse_str(row.try_get::<String, _>("target_id")?.as_str())
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_retention_holds WHERE tenant_id=? AND aggregate_type=? AND aggregate_id=? AND released_at IS NULL AND (expires_at IS NULL OR expires_at>UTC_TIMESTAMP(6)))",
    )
    .bind(claim.tenant_id)
    .bind(&data_type)
    .bind(target_id)
    .fetch_one(&mut *tx)
    .await?;
    if held {
        sqlx::query(
            "UPDATE retention_items SET status='blocked',reason='live_reference' WHERE id=?",
        )
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(true);
    }
    let changed = match data_type.as_str() {
        "execution" => sqlx::query(
            "UPDATE workflow_executions SET retention_deleted_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND retention_deleted_at IS NULL AND status IN ('succeeded','failed','cancelled','timed_out') AND NOT EXISTS(SELECT 1 FROM checkpoints WHERE tenant_id=? AND execution_id=?) AND NOT EXISTS(SELECT 1 FROM execution_forks WHERE tenant_id=? AND source_execution_id=?)",
        )
        .bind(claim.tenant_id)
        .bind(target_id)
        .bind(claim.tenant_id)
        .bind(target_id)
        .bind(claim.tenant_id)
        .bind(target_id)
        .execute(&mut *tx)
        .await?,
        "application_message" => sqlx::query(
            "UPDATE application_messages SET retention_deleted_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND retention_deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM artifact_references WHERE tenant_id=? AND owner_type='message' AND owner_id=CAST(BIN_TO_UUID(?) AS CHAR) COLLATE utf8mb4_0900_ai_ci AND (retention_until IS NULL OR retention_until>UTC_TIMESTAMP(6)))",
        )
        .bind(claim.tenant_id)
        .bind(target_id)
        .bind(claim.tenant_id)
        .bind(target_id)
        .execute(&mut *tx)
        .await?,
        "evaluation_report" => sqlx::query(
            "UPDATE evaluation_runs SET retention_deleted_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND retention_deleted_at IS NULL AND status IN ('completed','failed','cancelled')",
        )
        .bind(claim.tenant_id)
        .bind(target_id)
        .execute(&mut *tx)
        .await?,
        _ => unreachable!("query only claims supported database retention types"),
    };
    sqlx::query(
        "UPDATE retention_items SET status=?,reason=?,deleted_at=IF(?='deleted',UTC_TIMESTAMP(6),NULL),locked_by=NULL,locked_until=NULL WHERE id=?",
    )
    .bind(if changed.rows_affected() == 1 { "deleted" } else { "blocked" })
    .bind(if changed.rows_affected() == 1 { None } else { Some("live_reference") })
    .bind(if changed.rows_affected() == 1 { "deleted" } else { "blocked" })
    .bind(item_id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() == 1 {
        sqlx::query("UPDATE retention_runs SET deleted_count=deleted_count+1 WHERE id=?")
            .bind(claim.run_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM execution_model_deltas WHERE tenant_id=? AND execution_id=?")
            .bind(claim.tenant_id)
            .bind(target_id)
            .execute(&mut *tx)
            .await?;
        // plan7 P7-B: model.delta rows amplify invocation_events growth, so
        // the events of a retained invocation are dropped with the same
        // execution retention decision (live executions keep their events).
        sqlx::query(
            "DELETE FROM invocation_events WHERE tenant_id=? AND invocation_id IN (SELECT id FROM application_invocations WHERE tenant_id=? AND execution_id=?)",
        )
        .bind(claim.tenant_id)
        .bind(claim.tenant_id)
        .bind(target_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

async fn complete_dry_run(state: &RuntimeState, claim: &RetentionRunClaim) -> RuntimeResult<()> {
    let mut tx = state.pool.begin().await?;
    ensure_run_lease(&mut tx, claim).await?;
    sqlx::query(
        "UPDATE retention_items SET status='blocked',reason='dry_run' WHERE retention_run_id=? AND status='candidate'",
    )
    .bind(claim.run_id)
    .execute(&mut *tx)
    .await?;
    complete_run(&mut tx, claim).await?;
    tx.commit().await?;
    Ok(())
}

async fn complete_if_drained(state: &RuntimeState, claim: &RetentionRunClaim) -> RuntimeResult<()> {
    let mut tx = state.pool.begin().await?;
    ensure_run_lease(&mut tx, claim).await?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM retention_items WHERE retention_run_id=? AND status IN ('candidate','deleting','failed')",
    )
    .bind(claim.run_id)
    .fetch_one(&mut *tx)
    .await?;
    if pending == 0 {
        complete_run(&mut tx, claim).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn complete_run(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    claim: &RetentionRunClaim,
) -> RuntimeResult<()> {
    let changed = sqlx::query(
        "UPDATE retention_runs SET status='completed',locked_by=NULL,locked_until=NULL,completed_at=UTC_TIMESTAMP(6) WHERE id=? AND status='running' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(claim.run_id)
    .bind(claim.owner)
    .bind(claim.fencing_token)
    .execute(&mut **tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(lease_lost("Retention Run"));
    }
    let row = sqlx::query("SELECT tenant_id,dry_run,policy_version,candidate_count,deleted_count,CAST((SELECT COUNT(*) FROM retention_items WHERE retention_run_id=? AND status='failed') AS UNSIGNED) failed_count FROM retention_runs WHERE id=?")
        .bind(claim.run_id)
        .bind(claim.run_id)
        .fetch_one(&mut **tx)
        .await?;
    let item_rows = sqlx::query("SELECT id,data_type,target_id,status,reason,attempt_count FROM retention_items WHERE retention_run_id=? ORDER BY created_at,id")
        .bind(claim.run_id)
        .fetch_all(&mut **tx)
        .await?;
    let items = item_rows
        .into_iter()
        .map(|item| {
            Ok(agentx_runtime_contracts::RuntimeRetentionItemV1 {
                id: item.try_get("id")?,
                data_type: item.try_get("data_type")?,
                target_id: item.try_get("target_id")?,
                status: item.try_get("status")?,
                reason: item.try_get("reason")?,
                attempt_count: item.try_get("attempt_count")?,
            })
        })
        .collect::<RuntimeResult<Vec<_>>>()?;
    crate::event_export::enqueue_governance_event(
        tx,
        row.try_get("tenant_id")?,
        None,
        claim.run_id,
        &agentx_runtime_contracts::RuntimeEventPayloadV1::RetentionChanged {
            run_id: claim.run_id,
            run_version: row.try_get("policy_version")?,
            status: "completed".into(),
            marked_count: row.try_get("candidate_count")?,
            deleted_count: row.try_get("deleted_count")?,
            failed_count: row.try_get("failed_count")?,
            dry_run: row.try_get("dry_run")?,
            items,
        },
    )
    .await?;
    Ok(())
}

async fn ensure_run_lease(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    claim: &RetentionRunClaim,
) -> RuntimeResult<()> {
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM retention_runs WHERE id=? AND status='running' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6))",
    )
    .bind(claim.run_id)
    .bind(claim.owner)
    .bind(claim.fencing_token)
    .fetch_one(&mut **tx)
    .await?;
    if !live {
        return Err(lease_lost("Retention Run"));
    }
    Ok(())
}

async fn ensure_item_lease(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    item_id: Uuid,
    owner: Uuid,
    fencing_token: u64,
) -> RuntimeResult<()> {
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM retention_items WHERE id=? AND status='deleting' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6))",
    )
    .bind(item_id)
    .bind(owner)
    .bind(fencing_token)
    .fetch_one(&mut **tx)
    .await?;
    if !live {
        return Err(lease_lost("Retention Item"));
    }
    Ok(())
}

async fn fail_item(
    state: &RuntimeState,
    item_id: Uuid,
    owner: Uuid,
    fencing_token: u64,
    error: &str,
) -> RuntimeResult<()> {
    let changed = sqlx::query(
        "UPDATE retention_items SET status='failed',reason=?,locked_by=NULL,locked_until=NULL WHERE id=? AND status='deleting' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(error.chars().take(255).collect::<String>())
    .bind(item_id)
    .bind(owner)
    .bind(fencing_token)
    .execute(&state.pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(lease_lost("Retention Item"));
    }
    Ok(())
}

fn lease_lost(subject: &str) -> RuntimeError {
    RuntimeError::Internal(anyhow::anyhow!("{subject} LeaseLost"))
}
