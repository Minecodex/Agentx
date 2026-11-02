use sqlx::Row;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

const GC_BATCH_SIZE: usize = 100;

pub async fn mark_collectable(state: &RuntimeState, run_id: Uuid) -> RuntimeResult<u64> {
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO bundle_gc_runs(id,status,started_at) VALUES(?,'marking',UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE id=id")
        .bind(run_id)
        .execute(&mut *tx)
        .await?;

    let bundles = sqlx::query("SELECT b.tenant_id,b.id FROM deployment_bundles b WHERE b.status IN ('superseded','disabled','retained','garbage_collectable') AND COALESCE(b.retained_until,b.created_at)<=UTC_TIMESTAMP(6) AND NOT EXISTS(SELECT 1 FROM deployment_heads h WHERE h.bundle_id=b.id) AND NOT EXISTS(SELECT 1 FROM bundle_references r WHERE r.tenant_id=b.tenant_id AND r.bundle_id=b.id AND r.released_at IS NULL AND (r.retained_until IS NULL OR r.retained_until>UTC_TIMESTAMP(6))) AND NOT EXISTS(SELECT 1 FROM bundle_retention_holds h WHERE h.tenant_id=b.tenant_id AND h.bundle_id=b.id AND h.released_at IS NULL AND (h.expires_at IS NULL OR h.expires_at>UTC_TIMESTAMP(6))) AND NOT EXISTS(SELECT 1 FROM bundle_gc_items i WHERE i.tenant_id=b.tenant_id AND i.bundle_id=b.id AND i.status IN ('marked','deleting','failed')) ORDER BY b.created_at,b.id LIMIT 100 FOR UPDATE SKIP LOCKED")
        .fetch_all(&mut *tx)
        .await?;
    let mut marked = 0_usize;
    for row in bundles {
        let tenant: Uuid = row.try_get("tenant_id")?;
        let bundle: Uuid = row.try_get("id")?;
        sqlx::query(
            "UPDATE deployment_bundles SET status='garbage_collectable' WHERE tenant_id=? AND id=?",
        )
        .bind(tenant)
        .bind(bundle)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO bundle_gc_items(id,gc_run_id,tenant_id,bundle_id,item_kind,status) VALUES(?,?,?,?, 'bundle','marked')")
            .bind(Uuid::now_v7())
            .bind(run_id)
            .bind(tenant)
            .bind(bundle)
            .execute(&mut *tx)
            .await?;
        marked += 1;
    }

    let remaining = GC_BATCH_SIZE.saturating_sub(marked) as u32;
    if remaining > 0 {
        let objects = sqlx::query("SELECT o.tenant_id,o.object_id FROM runtime_objects o WHERE o.status='ready' AND o.ready_at<=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 HOUR) AND NOT EXISTS(SELECT 1 FROM bundle_objects bo WHERE bo.tenant_id=o.tenant_id AND bo.object_id=o.object_id) AND NOT EXISTS(SELECT 1 FROM application_page_publications p WHERE p.tenant_id=o.tenant_id AND (p.html_object_id=o.object_id OR p.sdk_object_id=o.object_id OR p.style_object_id=o.object_id)) AND NOT EXISTS(SELECT 1 FROM bundle_gc_items i WHERE i.tenant_id=o.tenant_id AND i.object_id=o.object_id AND i.status IN ('marked','deleting','failed')) ORDER BY o.ready_at,o.object_id LIMIT ? FOR UPDATE SKIP LOCKED")
            .bind(remaining)
            .fetch_all(&mut *tx)
            .await?;
        for row in objects {
            let tenant: Uuid = row.try_get("tenant_id")?;
            let object: Uuid = row.try_get("object_id")?;
            let changed = sqlx::query("UPDATE runtime_objects o SET o.status='deleting' WHERE o.tenant_id=? AND o.object_id=? AND o.status='ready' AND NOT EXISTS(SELECT 1 FROM bundle_objects bo WHERE bo.tenant_id=o.tenant_id AND bo.object_id=o.object_id) AND NOT EXISTS(SELECT 1 FROM application_page_publications p WHERE p.tenant_id=o.tenant_id AND (p.html_object_id=o.object_id OR p.sdk_object_id=o.object_id OR p.style_object_id=o.object_id))")
                .bind(tenant)
                .bind(object)
                .execute(&mut *tx)
                .await?;
            if changed.rows_affected() == 1 {
                sqlx::query("INSERT INTO bundle_gc_items(id,gc_run_id,tenant_id,object_id,item_kind,status) VALUES(?,?,?,?, 'object','marked')")
                    .bind(Uuid::now_v7())
                    .bind(run_id)
                    .bind(tenant)
                    .bind(object)
                    .execute(&mut *tx)
                    .await?;
                marked += 1;
            }
        }
    }

    sqlx::query("UPDATE bundle_gc_runs SET status=IF(?=0,'completed','sweeping'),marked_count=?,completed_at=IF(?=0,UTC_TIMESTAMP(6),NULL) WHERE id=?")
        .bind(marked as u32)
        .bind(marked as u32)
        .bind(marked as u32)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(marked as u64)
}

pub async fn sweep_one(state: &RuntimeState, run_id: Uuid, owner: Uuid) -> RuntimeResult<bool> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT id,tenant_id,bundle_id,object_id,item_kind FROM bundle_gc_items WHERE gc_run_id=? AND status IN ('marked','failed') AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED")
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(false);
    };
    let item: Uuid = row.try_get("id")?;
    let tenant: Uuid = row.try_get("tenant_id")?;
    let bundle: Option<Uuid> = row.try_get("bundle_id")?;
    let object: Option<Uuid> = row.try_get("object_id")?;
    let kind: String = row.try_get("item_kind")?;
    sqlx::query("UPDATE bundle_gc_items SET status='deleting',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=?")
        .bind(owner)
        .bind(item)
        .execute(&mut *tx)
        .await?;
    let token: u64 = sqlx::query_scalar("SELECT fencing_token FROM bundle_gc_items WHERE id=?")
        .bind(item)
        .fetch_one(&mut *tx)
        .await?;

    let objects = if kind == "bundle" {
        let bundle = bundle.ok_or_else(|| {
            RuntimeError::Internal(anyhow::anyhow!("Bundle GC item has no Bundle ID"))
        })?;
        let protected: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM deployment_heads WHERE tenant_id=? AND bundle_id=?) + (SELECT COUNT(*) FROM deployment_bundles WHERE tenant_id=? AND id=? AND status IN ('active','prepared')) + (SELECT COUNT(*) FROM bundle_references WHERE tenant_id=? AND bundle_id=? AND released_at IS NULL AND (retained_until IS NULL OR retained_until>UTC_TIMESTAMP(6))) + (SELECT COUNT(*) FROM bundle_retention_holds WHERE tenant_id=? AND bundle_id=? AND released_at IS NULL AND (expires_at IS NULL OR expires_at>UTC_TIMESTAMP(6)))")
            .bind(tenant).bind(bundle).bind(tenant).bind(bundle).bind(tenant).bind(bundle).bind(tenant).bind(bundle)
            .fetch_one(&mut *tx)
            .await?;
        if protected > 0 {
            complete_item(&mut tx, item, owner, token, "skipped_referenced").await?;
            tx.commit().await?;
            return Ok(true);
        }
        let rows = sqlx::query("SELECT o.object_id,CAST(o.object_key AS CHAR CHARACTER SET utf8mb4) AS object_key FROM bundle_objects bo JOIN runtime_objects o ON o.tenant_id=bo.tenant_id AND o.object_id=bo.object_id WHERE bo.tenant_id=? AND bo.bundle_id=? AND o.status IN ('ready','deleting') AND NOT EXISTS(SELECT 1 FROM bundle_objects other WHERE other.tenant_id=bo.tenant_id AND other.object_id=bo.object_id AND other.bundle_id<>bo.bundle_id) FOR UPDATE")
            .bind(tenant)
            .bind(bundle)
            .fetch_all(&mut *tx)
            .await?;
        for object in &rows {
            sqlx::query("UPDATE runtime_objects SET status='deleting' WHERE tenant_id=? AND object_id=? AND status IN ('ready','deleting')")
                .bind(tenant)
                .bind(object.try_get::<Uuid, _>("object_id")?)
                .execute(&mut *tx)
                .await?;
        }
        rows.into_iter()
            .map(|row| Ok((row.try_get("object_id")?, row.try_get("object_key")?)))
            .collect::<Result<Vec<(Uuid, String)>, sqlx::Error>>()?
    } else {
        let object = object.ok_or_else(|| {
            RuntimeError::Internal(anyhow::anyhow!("Object GC item has no Object ID"))
        })?;
        let row = sqlx::query("SELECT CAST(o.object_key AS CHAR CHARACTER SET utf8mb4) AS object_key FROM runtime_objects o WHERE o.tenant_id=? AND o.object_id=? AND o.status='deleting' AND NOT EXISTS(SELECT 1 FROM bundle_objects bo WHERE bo.tenant_id=o.tenant_id AND bo.object_id=o.object_id) AND NOT EXISTS(SELECT 1 FROM application_page_publications p WHERE p.tenant_id=o.tenant_id AND (p.html_object_id=o.object_id OR p.sdk_object_id=o.object_id OR p.style_object_id=o.object_id)) FOR UPDATE")
            .bind(tenant)
            .bind(object)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            complete_item(&mut tx, item, owner, token, "skipped_referenced").await?;
            tx.commit().await?;
            return Ok(true);
        };
        vec![(object, row.try_get("object_key")?)]
    };
    tx.commit().await?;

    for (_, key) in &objects {
        if let Err(error) = state
            .objects
            .delete(&object_store::path::Path::from(key.clone()))
            .await
        {
            fail_item(state, item, owner, token, &error.to_string()).await?;
            return Ok(true);
        }
    }

    let mut tx = state.pool.begin().await?;
    assert_live_lease(&mut tx, item, owner, token).await?;
    for (object, _) in &objects {
        sqlx::query("UPDATE runtime_objects SET status='deleted',deleted_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND object_id=? AND status='deleting'")
            .bind(tenant)
            .bind(object)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(bundle) = bundle {
        sqlx::query("DELETE FROM bundle_objects WHERE tenant_id=? AND bundle_id=?")
            .bind(tenant)
            .bind(bundle)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM deployment_bundles WHERE tenant_id=? AND id=? AND status='garbage_collectable'")
            .bind(tenant)
            .bind(bundle)
            .execute(&mut *tx)
            .await?;
    }
    complete_item(&mut tx, item, owner, token, "deleted").await?;
    sqlx::query("UPDATE bundle_gc_items SET deleted_at=UTC_TIMESTAMP(6) WHERE id=?")
        .bind(item)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE bundle_gc_runs SET deleted_count=deleted_count+1,status=IF(NOT EXISTS(SELECT 1 FROM bundle_gc_items WHERE gc_run_id=? AND status IN ('marked','deleting','failed')),'completed',status),completed_at=IF(NOT EXISTS(SELECT 1 FROM bundle_gc_items WHERE gc_run_id=? AND status IN ('marked','deleting','failed')),UTC_TIMESTAMP(6),completed_at) WHERE id=?")
        .bind(run_id).bind(run_id).bind(run_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

async fn assert_live_lease(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    item: Uuid,
    owner: Uuid,
    token: u64,
) -> RuntimeResult<()> {
    let live: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM bundle_gc_items WHERE id=? AND status='deleting' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6))")
        .bind(item).bind(owner).bind(token)
        .fetch_one(&mut **tx)
        .await?;
    if live {
        Ok(())
    } else {
        Err(RuntimeError::Internal(anyhow::anyhow!(
            "Bundle GC Lease was lost"
        )))
    }
}

async fn complete_item(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    item: Uuid,
    owner: Uuid,
    token: u64,
    status: &str,
) -> RuntimeResult<()> {
    let changed = sqlx::query("UPDATE bundle_gc_items SET status=?,locked_by=NULL,locked_until=NULL WHERE id=? AND status='deleting' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(status).bind(item).bind(owner).bind(token)
        .execute(&mut **tx)
        .await?;
    if changed.rows_affected() == 1 {
        Ok(())
    } else {
        Err(RuntimeError::Internal(anyhow::anyhow!(
            "Bundle GC Lease was lost"
        )))
    }
}

async fn fail_item(
    state: &RuntimeState,
    item: Uuid,
    owner: Uuid,
    token: u64,
    error: &str,
) -> RuntimeResult<()> {
    let changed = sqlx::query("UPDATE bundle_gc_items SET status='failed',last_error=?,locked_by=NULL,locked_until=NULL WHERE id=? AND status='deleting' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(error.chars().take(1000).collect::<String>())
        .bind(item).bind(owner).bind(token)
        .execute(&state.pool)
        .await?;
    if changed.rows_affected() == 1 {
        Ok(())
    } else {
        Err(RuntimeError::Internal(anyhow::anyhow!(
            "Bundle GC Lease was lost"
        )))
    }
}

pub async fn cleanup_expired_temporary_objects(
    state: &RuntimeState,
    limit: u32,
) -> RuntimeResult<u64> {
    let rows = sqlx::query("SELECT object_id,tenant_id,temporary_key FROM runtime_objects WHERE status='uploading' AND temporary_expires_at<=UTC_TIMESTAMP(6) ORDER BY temporary_expires_at,object_id LIMIT ?")
        .bind(limit.clamp(1, 100)).fetch_all(&state.pool).await?;
    let mut deleted = 0_u64;
    for row in rows {
        let object_id: Uuid = row.try_get("object_id")?;
        let tenant_id: Uuid = row.try_get("tenant_id")?;
        if let Some(key) = row.try_get::<Option<String>, _>("temporary_key")? {
            state
                .objects
                .delete(&object_store::path::Path::from(key))
                .await
                .map_err(|error| RuntimeError::Internal(error.into()))?;
        }
        let changed = sqlx::query("DELETE FROM runtime_objects WHERE object_id=? AND tenant_id=? AND status='uploading' AND temporary_expires_at<=UTC_TIMESTAMP(6)")
            .bind(object_id).bind(tenant_id).execute(&state.pool).await?;
        deleted += changed.rows_affected();
    }
    Ok(deleted)
}
