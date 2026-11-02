//! Design history uses the tenant's existing trace retention window, retaining unresolved work.
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn collect(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    run: Uuid,
    days: u32,
    dry_run: bool,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let rows=sqlx::query("SELECT s.id FROM design_agent_sessions s WHERE s.tenant_id=? AND s.updated_at<DATE_SUB(UTC_TIMESTAMP(6),INTERVAL ? DAY) AND NOT EXISTS(SELECT 1 FROM design_agent_runs r WHERE r.tenant_id=s.tenant_id AND r.session_id=s.id AND ((r.status IN ('queued','running','waiting','outcome_unknown') OR EXISTS(SELECT 1 FROM design_agent_entries mi WHERE mi.tenant_id=r.tenant_id AND mi.run_id=r.id AND mi.event_key LIKE 'model-dispatch:%' AND NOT EXISTS(SELECT 1 FROM design_agent_entries mr WHERE mr.tenant_id=mi.tenant_id AND mr.session_id=mi.session_id AND mr.event_key=REPLACE(mi.event_key,'model-dispatch:','model-resolved:')))) OR r.completed_at>=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL ? DAY))) AND NOT EXISTS(SELECT 1 FROM design_agent_effects e WHERE e.tenant_id=s.tenant_id AND e.session_id=s.id AND e.status='pending') ORDER BY s.updated_at,s.id LIMIT 100 FOR UPDATE SKIP LOCKED")
        .bind(tenant).bind(days).bind(days).fetch_all(&mut *tx).await?;
    for row in rows {
        let session: Uuid = row.try_get("id")?;
        sqlx::query("INSERT INTO retention_items(id,tenant_id,retention_run_id,data_type,target_id,status,reason) VALUES(?,?,?,'studio_builder_session',?,?,'settled_design_history') ON DUPLICATE KEY UPDATE status=VALUES(status)")
            .bind(Uuid::now_v7()).bind(tenant).bind(run).bind(session.to_string()).bind(if dry_run{"candidate"}else{"deleted"}).execute(&mut *tx).await?;
        if !dry_run {
            sqlx::query("DELETE FROM design_agent_runs WHERE tenant_id=? AND session_id=?")
                .bind(tenant)
                .bind(session)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM design_agent_sessions WHERE tenant_id=? AND id=?")
                .bind(tenant)
                .bind(session)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
