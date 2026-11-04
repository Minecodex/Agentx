//! Transactional project permits also cover resuming human waits and composite children.
use super::*;

async fn project(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Uuid,
) -> RuntimeResult<Option<(Uuid, Uuid, Uuid)>> {
    let project = sqlx::query("WITH RECURSIVE parents AS (SELECT id,invocation_id,0 AS depth FROM workflow_executions WHERE tenant_id=? AND id=? UNION ALL SELECT e.id,e.invocation_id,p.depth+1 FROM parents p JOIN execution_children c ON c.tenant_id=? AND c.child_execution_id=p.id AND c.relationship='composite' JOIN workflow_executions e ON e.tenant_id=c.tenant_id AND e.id=c.parent_execution_id WHERE p.depth<64) SELECT i.id,i.batch_id,p.id AS root FROM parents p JOIN application_batch_items i ON i.tenant_id=? AND i.invocation_id=p.invocation_id LIMIT 1")
        .bind(tenant).bind(execution).bind(tenant).bind(tenant).fetch_optional(&mut **tx).await?;
    let Some(project) = project else {
        return Ok(None);
    };
    let batch: Uuid = project.try_get("batch_id")?;
    sqlx::query("SELECT id FROM application_batches WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(tenant)
        .bind(batch)
        .fetch_one(&mut **tx)
        .await?;
    Ok(Some((
        project.try_get("id")?,
        batch,
        project.try_get("root")?,
    )))
}

pub(super) async fn occupied(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    batch: Uuid,
) -> RuntimeResult<usize> {
    // Locking reads see the preceding permit holder's commit even under REPEATABLE READ.
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM application_batch_items WHERE tenant_id=? AND batch_id=? AND active_slot=TRUE FOR UPDATE")
        .bind(tenant).bind(batch).fetch_all(&mut **tx).await?;
    Ok(ids.len())
}

pub(crate) async fn acquire(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Uuid,
) -> RuntimeResult<()> {
    let Some((item, batch, _)) = project(tx, tenant, execution).await? else {
        return Ok(());
    };
    let active: bool = sqlx::query_scalar(
        "SELECT active_slot FROM application_batch_items WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(tenant)
    .bind(item)
    .fetch_one(&mut **tx)
    .await?;
    if active {
        return Ok(());
    }
    if occupied(tx, tenant, batch).await? >= APPLICATION_BATCH_CONCURRENCY {
        return Err(RuntimeError::AdmissionRejected {
            retry_after_seconds: 1,
        });
    }
    sqlx::query("UPDATE application_batch_items SET active_slot=TRUE WHERE tenant_id=? AND id=?")
        .bind(tenant)
        .bind(item)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE application_batches SET revision=revision+1 WHERE tenant_id=? AND id=?")
        .bind(tenant)
        .bind(batch)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The coordinator locks the batch before its first consistent read. Engine
/// scheduling holds the same lock, so an idle project cannot enqueue work while
/// its permit is being released, including work in composite children.
pub(super) async fn refresh(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    items: &[BatchItemViewV1],
) -> RuntimeResult<()> {
    for item in items {
        if !matches!(
            item.state.as_str(),
            "waiting_input"
                | "waiting_approval"
                | "completed"
                | "failed"
                | "cancelled"
                | "outcome_unknown"
        ) {
            continue;
        }
        let Some(task) = &item.task else { continue };
        let root: Option<Uuid> = sqlx::query_scalar(
            "SELECT execution_id FROM application_invocations WHERE tenant_id=? AND id=?",
        )
        .bind(tenant)
        .bind(task.task_id)
        .fetch_one(&mut **tx)
        .await?;
        let tree = crate::artifact_access::EXECUTION_TREE;
        let active: bool = sqlx::query_scalar(&format!("{tree} SELECT EXISTS(SELECT 1 FROM node_attempts a JOIN task_executions e ON e.id=a.execution_id WHERE a.tenant_id=? AND a.status IN ('queued','running'))"))
            .bind(tenant).bind(root).bind(tenant).bind(tenant).fetch_one(&mut **tx).await?;
        if !active {
            sqlx::query("UPDATE application_batch_items SET active_slot=FALSE WHERE tenant_id=? AND id=? AND active_slot=TRUE")
                .bind(tenant).bind(item.item_id).execute(&mut **tx).await?;
        }
    }
    Ok(())
}
