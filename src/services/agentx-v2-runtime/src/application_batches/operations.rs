use super::dispatch::{BatchCommandClaim, command_tx, finish_tx};
use super::*;

pub async fn stop(
    state: &RuntimeState,
    scope: &BatchScope,
    id: Uuid,
) -> RuntimeResult<BatchViewV1> {
    let mut tx = state.pool.begin().await?;
    let batch = row(&mut tx, scope, id, true).await?;
    if batch.try_get::<String, _>("dispatch_state")? == "active" {
        sqlx::query("UPDATE application_batch_items SET dispatch_state='cancelled' WHERE tenant_id=? AND batch_id=? AND dispatch_state='pending'").bind(scope.caller.tenant_id).bind(id).execute(&mut *tx).await?;
        sqlx::query("UPDATE application_batches SET dispatch_state='stopped',revision=revision+1 WHERE tenant_id=? AND id=?").bind(scope.caller.tenant_id).bind(id).execute(&mut *tx).await?;
        audit(&mut tx, scope, id, "application.batch.stopped", json!({})).await?;
    }
    tx.commit().await?;
    get(state, scope, id).await
}

pub async fn cancel(
    state: &RuntimeState,
    scope: &BatchScope,
    id: Uuid,
    key: &str,
) -> RuntimeResult<BatchViewV1> {
    stop(state, scope, id).await?;
    let mut tx = state.pool.begin().await?;
    row(&mut tx, scope, id, true).await?;
    let command_key = format!(
        "batch:cancel:{}",
        hash(&json!([id, scope.caller.caller_id, key]))?
    );
    let result=sqlx::query("INSERT IGNORE INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json) VALUES(?,?,'cancel_batch','batch',?,?,?)")
        .bind(Uuid::now_v7()).bind(scope.caller.tenant_id).bind(id.to_string()).bind(command_key).bind(json!({"batchId":id,"actorType":scope.caller.caller_type,"actor":scope.caller.caller_id})).execute(&mut *tx).await?;
    if result.rows_affected() > 0 {
        audit(
            &mut tx,
            scope,
            id,
            "application.batch.cancel.requested",
            json!({}),
        )
        .await?;
    }
    tx.commit().await?;
    get(state, scope, id).await
}

pub(super) async fn process_cancel(
    state: &RuntimeState,
    claim: &BatchCommandClaim,
) -> RuntimeResult<()> {
    let mut tx = state.pool.begin().await?;
    command_tx(&mut tx, claim).await?;
    let batch = sqlx::query("SELECT * FROM application_batches WHERE tenant_id=? AND id=?")
        .bind(claim.tenant)
        .bind(claim.batch)
        .fetch_one(&mut *tx)
        .await?;
    let owner = owner(&batch)?;
    let actor_type = claim.payload["actorType"]
        .as_str()
        .filter(|kind| matches!(*kind, "user" | "api_key"))
        .ok_or_else(|| invalid("Missing cancellation actor type"))?;
    let actor_id: Uuid = serde_json::from_value(claim.payload["actor"].clone())
        .map_err(|e| RuntimeError::Internal(e.into()))?;
    let items = super::read::views_tx(&mut tx, &batch).await?;
    tx.commit().await?;
    for item in items {
        if let Some(task) = item.task.filter(|t| t.allowed_actions.can_cancel) {
            crate::invocation_cancel::cancel_authorized_invocation(
                state,
                &owner.task_caller()?,
                task.task_id,
                &format!("batch-cancel:{}:{}", claim.id, item.item_id),
                actor_type,
                actor_id,
            )
            .await?;
        }
    }
    let mut tx = state.pool.begin().await?;
    command_tx(&mut tx, claim).await?;
    finish_tx(&mut tx, claim, json!({"accepted":true}), false).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn rerun(
    state: &RuntimeState,
    scope: &BatchScope,
    id: Uuid,
    request: BatchRerunV1,
    key: &str,
) -> RuntimeResult<BatchViewV1> {
    if request.item_ids.is_empty() || request.item_ids.len() > 100 {
        return Err(invalid("Select 1–100 failed items"));
    }
    let mut ids = std::collections::BTreeSet::new();
    if request.item_ids.iter().any(|id| !ids.insert(*id)) {
        return Err(invalid("Duplicate item IDs"));
    }
    // Replaying the same derived intent must succeed even if the original state later changes.
    let derived_key = format!("rerun:{}", hash(&json!([id, key]))?);
    let derived_id = deterministic_uuid(
        scope.caller.tenant_id,
        format!(
            "batch:{}:{}:{}:{derived_key}",
            scope.caller.application_id, scope.caller.caller_type, scope.caller.caller_id
        )
        .as_bytes(),
    );
    let mut tx = state.pool.begin().await?;
    let batch = row(&mut tx, scope, id, false).await?;
    let projects = super::read::views_tx(&mut tx, &batch).await?;
    let selected = projects
        .iter()
        .filter(|i| ids.contains(&i.item_id))
        .collect::<Vec<_>>();
    if selected.len() != ids.len() {
        return Err(RuntimeError::NotFound);
    }
    let existing = sqlx::query("SELECT * FROM application_batches WHERE tenant_id=? AND id=?")
        .bind(scope.caller.tenant_id)
        .bind(derived_id)
        .fetch_optional(&mut *tx)
        .await?;
    if existing.is_none() && selected.iter().any(|i| !i.can_rerun) {
        return Err(conflict(
            "Only confirmed replay-safe failed items can be executed again",
        ));
    }
    let current:Value=sqlx::query_scalar("SELECT b.payload_json FROM deployment_heads h JOIN deployment_bundles b ON b.tenant_id=h.tenant_id AND b.id=h.bundle_id WHERE h.tenant_id=? AND h.application_id=? AND b.status='active'").bind(scope.caller.tenant_id).bind(scope.caller.application_id).fetch_one(&mut *tx).await?;
    let spec: ExecutionSpecPayloadV2 =
        serde_json::from_value(current).map_err(|e| RuntimeError::Internal(e.into()))?;
    if existing.is_none()
        && hash(&spec.input_contract)? != batch.try_get::<String, _>("input_contract_hash")?
    {
        return Err(conflict(
            "The input contract changed; create and explicitly correct a new batch",
        ));
    }
    let access = if let Some(existing) = &existing {
        binding(existing)?
    } else if binding(&batch)?.is_some() {
        let p=sqlx::query("SELECT publication_id,revision FROM application_web_heads WHERE tenant_id=? AND application_id=? AND enabled=TRUE").bind(scope.caller.tenant_id).bind(scope.caller.application_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
        Some(ApplicationAccessBindingV1::Web {
            publication_id: p.try_get("publication_id")?,
            revision: p.try_get("revision")?,
        })
    } else {
        None
    };
    let sources = selected
        .iter()
        .map(|i| (i.item_id, i.task.as_ref().map(|t| t.task_id)))
        .collect::<Vec<_>>();
    let request = CreateBatchV1 {
        name: format!("{} · retry", batch.try_get::<String, _>("name")?)
            .chars()
            .take(200)
            .collect(),
        items: selected
            .iter()
            .map(|i| BatchInputV1 {
                client_item_id: i.client_item_id.clone(),
                input: i.input.clone(),
            })
            .collect(),
    };
    tx.commit().await?;
    create(
        state,
        &scope.caller,
        request,
        &derived_key,
        access,
        Some(id),
        &sources,
    )
    .await
}
