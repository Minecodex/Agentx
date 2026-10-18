//! Durable evaluation publication through the existing fenced Control outbox.
use crate::{
    api_error::ApiResult,
    control_api::{Actor, ControlApiState},
};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn run_loop(
    state: ControlApiState,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> anyhow::Result<()> {
    let owner = Uuid::now_v7();
    while !lifecycle.is_draining() {
        let started = std::time::Instant::now();
        if let Err(error) = dispatch_one(&state, owner).await {
            tracing::warn!(?error, "Evaluation outbox dispatch failed");
        }
        if let Err(error)=sqlx::query("UPDATE evaluation_batches b JOIN (SELECT tenant_id,batch_id,SUM(status IN ('created','queued','running')) unfinished,SUM(status='failed') failures,SUM(status<>'created') started FROM evaluation_runs GROUP BY tenant_id,batch_id) r ON r.tenant_id=b.tenant_id AND r.batch_id=b.id SET b.status=CASE WHEN r.unfinished=0 THEN IF(r.failures>0,'failed','completed') WHEN r.started>0 THEN 'running' ELSE 'queued' END WHERE b.status IN ('queued','running')").execute(&state.pool).await{tracing::warn!(?error,"Evaluation batch convergence failed");}
        progress.processed_since(started).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Ok(())
}

pub(crate) async fn dispatch_one(state: &ControlApiState, owner: Uuid) -> ApiResult<()> {
    let mut tx = state.pool.begin().await?;
    let row=sqlx::query("SELECT id,tenant_id,payload_json,fencing_token,aggregate_type FROM outbox WHERE aggregate_type IN ('evaluation_dispatch','evaluation_model_admission') AND ((status IN ('pending','failed') AND available_at<=UTC_TIMESTAMP(6)) OR (status='processing' AND locked_until<UTC_TIMESTAMP(6))) ORDER BY occurred_at,id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(());
    };
    let id: Uuid = row.try_get("id")?;
    let tenant: Uuid = row.try_get("tenant_id")?;
    let payload: Value = row.try_get("payload_json")?;
    let model_admission =
        row.try_get::<String, _>("aggregate_type")? == "evaluation_model_admission";
    let run = if model_admission {
        None
    } else {
        Some(
            Uuid::parse_str(payload["runId"].as_str().unwrap_or(""))
                .map_err(crate::api_error::ApiError::internal)?,
        )
    };
    let fence = row.try_get::<u64, _>("fencing_token")? + 1;
    sqlx::query("UPDATE outbox SET status='processing',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 60 SECOND),fencing_token=?,attempt_count=attempt_count+1 WHERE id=?").bind(owner).bind(fence).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    let operation = async {
        if model_admission {
            let command: agentx_runtime_contracts::RuntimeAdmissionCommandV1 =
                serde_json::from_value(payload).map_err(crate::api_error::ApiError::internal)?;
            let receipt = crate::governance_api::post_runtime_admission(state, &command).await?;
            if !receipt.applied && !receipt.replayed {
                return Err(crate::api_error::ApiError::unavailable(
                    "RUNTIME_UNAVAILABLE",
                    "Model revocation was not acknowledged",
                ));
            }
            Ok(())
        } else {
            let batch = Uuid::parse_str(payload["batchId"].as_str().unwrap_or(""))
                .map_err(crate::api_error::ApiError::internal)?;
            dispatch_run(state, tenant, batch, run.expect("dispatch run")).await
        }
    };
    tokio::pin!(operation);
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_secs(15),
        std::time::Duration::from_secs(15),
    );
    let result = loop {
        tokio::select! {
            result=&mut operation=>break result,
            _=heartbeat.tick()=>{let changed=sqlx::query("UPDATE outbox SET locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 60 SECOND) WHERE id=? AND locked_by=? AND fencing_token=? AND status='processing' AND locked_until>UTC_TIMESTAMP(6)").bind(id).bind(owner).bind(fence).execute(&state.pool).await?;if changed.rows_affected()!=1{return Ok(())}}
        }
    };
    let (status, error) = match result {
        Ok(()) => ("published", None),
        Err(error) => {
            let detail = error.design_feedback();
            let retry = model_admission
                || matches!(
                    detail["code"].as_str(),
                    Some("RUNTIME_UNAVAILABLE" | "RUNTIME_TIMEOUT" | "RUNTIME_RATE_LIMITED")
                );
            if !retry && let Some(run) = run {
                sqlx::query("UPDATE evaluation_runs SET status='failed',completed_at=UTC_TIMESTAMP(6),dispatch_error_json=? WHERE tenant_id=? AND id=? AND status IN ('created','queued','running')").bind(&detail).bind(tenant).bind(run).execute(&state.pool).await?;
            }
            (
                if retry { "failed" } else { "published" },
                Some(detail.to_string()),
            )
        }
    };
    sqlx::query("UPDATE outbox SET status=?,last_error=?,available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 5 SECOND),published_at=IF(?='published',UTC_TIMESTAMP(6),NULL),locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND status='processing' AND locked_until>UTC_TIMESTAMP(6)")
        .bind(status).bind(error).bind(status).bind(id).bind(owner).bind(fence).execute(&state.pool).await?;
    Ok(())
}

async fn dispatch_run(
    state: &ControlApiState,
    tenant: Uuid,
    batch: Uuid,
    run: Uuid,
) -> ApiResult<()> {
    let row=sqlx::query("SELECT b.status batch_status,b.actor_snapshot_json,r.status,r.work_package_id FROM evaluation_batches b JOIN evaluation_runs r ON r.tenant_id=b.tenant_id AND r.batch_id=b.id WHERE b.tenant_id=? AND b.id=? AND r.id=?").bind(tenant).bind(batch).bind(run).fetch_one(&state.pool).await?;
    let actor: Actor = serde_json::from_value(row.try_get("actor_snapshot_json")?)
        .map_err(crate::api_error::ApiError::internal)?;
    if row.try_get::<String, _>("batch_status")? == "cancelled" {
        if row.try_get::<Option<Uuid>, _>("work_package_id")?.is_some() {
            crate::work_packages::cancel_evaluation(state, &actor, run).await?;
        } else {
            sqlx::query("UPDATE evaluation_runs SET status='cancelled',completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=?")
                .bind(tenant)
                .bind(run)
                .execute(&state.pool)
                .await?;
        }
        return Ok(());
    }
    if matches!(
        row.try_get::<String, _>("status")?.as_str(),
        "completed" | "failed" | "cancelled"
    ) {
        return Ok(());
    }
    crate::work_packages::start_evaluation(state, &actor, run).await?;
    let cancelled: bool = sqlx::query_scalar(
        "SELECT status='cancelled' FROM evaluation_batches WHERE tenant_id=? AND id=?",
    )
    .bind(tenant)
    .bind(batch)
    .fetch_one(&state.pool)
    .await?;
    if cancelled {
        crate::work_packages::cancel_evaluation(state, &actor, run).await?;
    }
    Ok(())
}
