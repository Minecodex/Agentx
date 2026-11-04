use super::*;

pub struct BatchCommandClaim {
    pub id: Uuid,
    pub tenant: Uuid,
    pub batch: Uuid,
    pub kind: String,
    pub owner: Uuid,
    pub fence: u64,
    pub payload: Value,
}

/// Batch commands share the existing command table, fencing and recovery loop; their target is never an Execution.
pub async fn run_once(state: &RuntimeState, owner: Uuid) -> RuntimeResult<bool> {
    let mut tx = state.pool.begin().await?;
    let row=sqlx::query("SELECT id FROM runtime_commands WHERE aggregate_type='batch' AND status='pending' AND available_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY available_at,created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(false);
    };
    let id: Uuid = row.try_get("id")?;
    sqlx::query("UPDATE runtime_commands SET status='processing',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=?").bind(owner).bind(id).execute(&mut *tx).await?;
    let row = sqlx::query("SELECT * FROM runtime_commands WHERE id=?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let claim = BatchCommandClaim {
        id,
        tenant: row.try_get("tenant_id")?,
        batch: Uuid::parse_str(&row.try_get::<String, _>("aggregate_id")?)
            .map_err(|e| RuntimeError::Internal(e.into()))?,
        kind: row.try_get("command_type")?,
        owner,
        fence: row.try_get("fencing_token")?,
        payload: row.try_get("payload_json")?,
    };
    tx.commit().await?;
    if let Err(error) = process(state, &claim).await {
        let terminal = matches!(
            error,
            RuntimeError::InvalidRequest(..)
                | RuntimeError::Validation { .. }
                | RuntimeError::Unauthorized
                | RuntimeError::NotFound
        );
        sqlx::query("UPDATE runtime_commands SET status=?,available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 2 SECOND),locked_by=NULL,locked_until=NULL,error_code=?,error_message=? WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
            .bind(if terminal{"failed"}else{"pending"}).bind(if terminal{"BATCH_OPERATION_REJECTED"}else{"BATCH_OPERATION_RETRY"}).bind(error.to_string().chars().take(1000).collect::<String>()).bind(claim.id).bind(owner).bind(claim.fence).execute(&state.pool).await?;
        tracing::warn!(command_id=%claim.id,batch_id=%claim.batch,%error,"Batch command did not finish");
    }
    Ok(true)
}

pub async fn process(state: &RuntimeState, claim: &BatchCommandClaim) -> RuntimeResult<()> {
    match claim.kind.as_str() {
        "dispatch_batch" => dispatch(state, claim).await,
        "export_batch" => super::files::process_export(state, claim).await,
        "cancel_batch" => super::operations::process_cancel(state, claim).await,
        _ => Err(invalid("Unknown batch command")),
    }
}

pub(super) async fn command_tx(
    tx: &mut Transaction<'_, MySql>,
    claim: &BatchCommandClaim,
) -> RuntimeResult<()> {
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_commands WHERE id=? AND tenant_id=? AND aggregate_type='batch' AND aggregate_id=? AND status='processing' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6))")
        .bind(claim.id).bind(claim.tenant).bind(claim.batch.to_string()).bind(claim.owner).bind(claim.fence).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(conflict("Batch command lease was lost"));
    }
    Ok(())
}

pub(super) async fn finish_tx(
    tx: &mut Transaction<'_, MySql>,
    claim: &BatchCommandClaim,
    result: Value,
    pending: bool,
) -> RuntimeResult<()> {
    let changed=sqlx::query("UPDATE runtime_commands SET status=?,result_json=?,available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 1 SECOND),completed_at=IF(?=FALSE,UTC_TIMESTAMP(6),NULL),locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(if pending{"pending"}else{"completed"}).bind(result).bind(pending).bind(claim.id).bind(claim.owner).bind(claim.fence).execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(conflict("Batch command lease was lost"));
    }
    Ok(())
}

async fn dispatch(state: &RuntimeState, claim: &BatchCommandClaim) -> RuntimeResult<()> {
    let mut tx = state.pool.begin().await?;
    command_tx(&mut tx, claim).await?;
    let batch =
        sqlx::query("SELECT * FROM application_batches WHERE tenant_id=? AND id=? FOR UPDATE")
            .bind(claim.tenant)
            .bind(claim.batch)
            .fetch_one(&mut *tx)
            .await?;
    let dispatching = batch.try_get::<String, _>("dispatch_state")? == "active";
    let caller = owner(&batch)?;
    let access = binding(&batch)?;
    let items = super::read::views_tx(&mut tx, &batch).await?;
    super::capacity::refresh(&mut tx, claim.tenant, &items).await?;
    let active = super::capacity::occupied(&mut tx, claim.tenant, claim.batch).await?;
    let slots = APPLICATION_BATCH_CONCURRENCY.saturating_sub(active);
    for item in items
        .iter()
        .filter(|i| dispatching && i.dispatch_state == "pending")
        .take(slots)
    {
        match crate::admission::check_tx(&mut tx, state.admission_redis.as_ref(), claim.tenant)
            .await
        {
            Ok(()) => {}
            Err(
                RuntimeError::AdmissionRejected { .. }
                | RuntimeError::Unavailable
                | RuntimeError::DatabaseUnavailable,
            ) => break,
            Err(error) => return Err(error),
        }
        let identity: Option<Value> = sqlx::query_scalar(
            "SELECT user_identity_json FROM application_batch_items WHERE tenant_id=? AND id=?",
        )
        .bind(claim.tenant)
        .bind(item.item_id)
        .fetch_one(&mut *tx)
        .await?;
        // Savepoint allows a deterministic per-item admission rejection without discarding other accepted items.
        sqlx::Executor::execute(&mut *tx, "SAVEPOINT batch_item_admission").await?;
        match crate::execution::create_batch_runtime_invocation_tx(
            &mut tx,
            caller.tenant_id,
            caller.application_id,
            caller.invocation(access.clone(), identity)?,
            &item.input,
            claim.batch,
            item.item_id,
        )
        .await
        {
            Ok(accepted) => {
                sqlx::query("UPDATE application_batch_items SET dispatch_state='accepted',active_slot=TRUE,invocation_id=? WHERE tenant_id=? AND id=? AND dispatch_state='pending'").bind(accepted.invocation_id).bind(claim.tenant).bind(item.item_id).execute(&mut *tx).await?;
                sqlx::query("DELETE FROM artifact_references WHERE tenant_id=? AND owner_type='batch_item' AND owner_id=?").bind(claim.tenant).bind(item.item_id.to_string()).execute(&mut *tx).await?;
            }
            Err(error) => {
                sqlx::Executor::execute(&mut *tx, "ROLLBACK TO SAVEPOINT batch_item_admission")
                    .await?;
                if matches!(
                    error,
                    RuntimeError::Unauthorized
                        | RuntimeError::NotFound
                        | RuntimeError::InvalidRequest(..)
                        | RuntimeError::Validation { .. }
                        | RuntimeError::BadRequest(..)
                ) {
                    sqlx::query("UPDATE application_batch_items SET dispatch_state='rejected',error_json=? WHERE tenant_id=? AND id=? AND dispatch_state='pending'")
                        .bind(json!({"stage":"admission","code":"BATCH_ITEM_ADMISSION_REJECTED","message":error.to_string()})).bind(claim.tenant).bind(item.item_id).execute(&mut *tx).await?;
                } else if matches!(
                    error,
                    RuntimeError::AdmissionRejected { .. }
                        | RuntimeError::Unavailable
                        | RuntimeError::DatabaseUnavailable
                        | RuntimeError::SecretUnavailable
                ) {
                    break;
                } else {
                    return Err(error);
                }
            }
        }
        sqlx::Executor::execute(&mut *tx, "RELEASE SAVEPOINT batch_item_admission").await?;
    }
    let remaining:i64=sqlx::query_scalar("SELECT COUNT(*) FROM application_batch_items WHERE tenant_id=? AND batch_id=? AND dispatch_state='pending'").bind(claim.tenant).bind(claim.batch).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE application_batches SET dispatch_state=IF(dispatch_state='active' AND ?=0,'finished',dispatch_state),revision=revision+1 WHERE tenant_id=? AND id=?").bind(remaining).bind(claim.tenant).bind(claim.batch).execute(&mut *tx).await?;
    let unfinished = items
        .iter()
        .any(|item| !matches!(item.state.as_str(), "completed" | "failed" | "cancelled"));
    finish_tx(
        &mut tx,
        claim,
        json!({"remaining":remaining}),
        remaining > 0 || unfinished,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
