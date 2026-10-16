use std::{collections::BTreeMap, future::Future, sync::Arc, time::Duration};

use agentx_runtime_contracts::{
    AdmissionTargetV1, ApplyReceiptV1, CommandEnvelopeV1, Plane, RetentionCommandRequestV1,
    RuntimeAdmissionCommandV1, RuntimeRetentionDataTypeV1, RuntimeRetentionPolicyV1,
};
use anyhow::{Context, Result};
use serde_json::json;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::Publisher;

struct RetentionClaim {
    id: Uuid,
    tenant_id: Uuid,
    runtime_command_id: Uuid,
    policy_version: u64,
    dry_run: bool,
    fencing_token: u64,
}

pub(super) async fn run_loop(
    publisher: Arc<Publisher>,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    loop {
        if lifecycle.is_draining() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        if let Err(error) = crate::canvas_plugin_api::cleanup_background_objects(
            &publisher.pool,
            &publisher.control_objects,
        )
        .await
        {
            tracing::warn!(%error, "Canvas Plugin object cleanup failed; retrying");
        }
        if let Some(claim) = claim(&publisher.pool, publisher.owner).await? {
            if let Err(error) =
                with_heartbeat(&publisher, &claim, publish(&publisher, &claim)).await
            {
                tracing::warn!(
                    retention_run_id = %claim.id,
                    runtime_command_id = %claim.runtime_command_id,
                    %error,
                    "Control Retention Run publish failed"
                );
                let message = error.to_string().chars().take(1000).collect::<String>();
                let changed = sqlx::query(
                    "UPDATE retention_runs SET status='queued',available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 2 SECOND),error_message=?,locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
                )
                .bind(message)
                .bind(claim.id)
                .bind(publisher.owner.0)
                .bind(claim.fencing_token)
                .execute(&publisher.pool)
                .await?;
                anyhow::ensure!(changed.rows_affected() == 1, "Control Retention LeaseLost");
            }
            progress.processed_since(started).await;
            continue;
        }
        progress.processed_since(started).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn with_heartbeat<F, T>(
    publisher: &Publisher,
    claim: &RetentionClaim,
    operation: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    tokio::pin!(operation);
    let start = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut heartbeat = tokio::time::interval_at(start, Duration::from_secs(10));
    loop {
        tokio::select! {
            result = &mut operation => return result,
            _ = heartbeat.tick() => {
                let changed = sqlx::query("UPDATE retention_runs SET locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND) WHERE id=? AND status='running' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
                    .bind(claim.id).bind(publisher.owner.0).bind(claim.fencing_token).execute(&publisher.pool).await?;
                anyhow::ensure!(changed.rows_affected() == 1, "Control Retention LeaseLost");
            }
        }
    }
}

async fn claim(
    pool: &sqlx::MySqlPool,
    owner: agentx_mysql_lease::LeaseOwner,
) -> Result<Option<RetentionClaim>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT id FROM retention_runs WHERE status='queued' AND runtime_receipt_json IS NULL AND available_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let id: Uuid = row.try_get("id")?;
    let runtime_command_id = Uuid::now_v7();
    sqlx::query(
        "UPDATE retention_runs SET status='running',runtime_command_id=COALESCE(runtime_command_id,?),policy_version=(SELECT COALESCE(MAX(version),1) FROM retention_policies p WHERE p.tenant_id=retention_runs.tenant_id),idempotency_key=COALESCE(idempotency_key,?),locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1,started_at=COALESCE(started_at,UTC_TIMESTAMP(6)) WHERE id=?",
    )
    .bind(runtime_command_id)
    .bind(format!("retention:{id}"))
    .bind(owner.0)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query(
        "SELECT id,tenant_id,runtime_command_id,policy_version,dry_run,fencing_token FROM retention_runs WHERE id=?",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let claim = RetentionClaim {
        id,
        tenant_id: row.try_get("tenant_id")?,
        runtime_command_id: row.try_get("runtime_command_id")?,
        policy_version: row.try_get("policy_version")?,
        dry_run: row.try_get("dry_run")?,
        fencing_token: row.try_get("fencing_token")?,
    };
    tx.commit().await?;
    tracing::info!(
        retention_run_id = %claim.id,
        runtime_command_id = %claim.runtime_command_id,
        tenant_id = %claim.tenant_id,
        policy_version = claim.policy_version,
        dry_run = claim.dry_run,
        "Control Retention Run claimed"
    );
    Ok(Some(claim))
}

async fn publish(publisher: &Publisher, claim: &RetentionClaim) -> Result<()> {
    let rows = sqlx::query(
        "SELECT data_type,retention_days,enabled FROM retention_policies WHERE tenant_id=? ORDER BY data_type",
    )
    .bind(claim.tenant_id)
    .fetch_all(&publisher.pool)
    .await?;
    let mut days = BTreeMap::new();
    let mut enabled = true;
    for row in rows {
        let data_type = match row.try_get::<String, _>("data_type")?.as_str() {
            "artifact" => RuntimeRetentionDataTypeV1::Artifact,
            "execution" => RuntimeRetentionDataTypeV1::Execution,
            "application_message" => RuntimeRetentionDataTypeV1::ApplicationMessage,
            "evaluation_report" => RuntimeRetentionDataTypeV1::EvaluationReport,
            "trace" => RuntimeRetentionDataTypeV1::Trace,
            value => anyhow::bail!("unsupported Runtime retention data type {value}"),
        };
        days.insert(data_type, row.try_get("retention_days")?);
        enabled &= row.try_get::<bool, _>("enabled")?;
    }
    anyhow::ensure!(!days.is_empty(), "Retention Run has no versioned policy");
    if enabled {
        if let Some(days) = days.get(&RuntimeRetentionDataTypeV1::Trace) {
            crate::studio_builder_retention::collect(
                &publisher.pool,
                claim.tenant_id,
                claim.id,
                *days,
                claim.dry_run,
            )
            .await?;
        }
    }
    let target = AdmissionTargetV1::RetentionPolicy {
        state: RuntimeRetentionPolicyV1 {
            tenant_id: claim.tenant_id,
            policy_version: claim.policy_version,
            retention_days: days,
            enabled,
        },
    };
    let admission_event = derived_id(claim.runtime_command_id, 1);
    let admission = RuntimeAdmissionCommandV1 {
        api_version: 1,
        command: CommandEnvelopeV1 {
            schema_version: 1,
            event_id: admission_event,
            source_plane: Plane::Control,
            tenant_id: claim.tenant_id,
            aggregate_type: "retention_policy".into(),
            aggregate_id: claim.tenant_id.to_string(),
            object_version: claim.policy_version,
            occurred_at: OffsetDateTime::UNIX_EPOCH,
            payload: json!({}),
            content_hash: agentx_runtime_contracts::content_hash(&target)?,
            correlation_id: claim.id,
            causation_id: None,
            idempotency_key: format!("retention:{}:policy", claim.id),
        },
        admission_epoch: claim.policy_version.max(1),
        target,
    };
    let policy_receipt: ApplyReceiptV1 = publisher
        .post(
            "runtime.admission.apply",
            "/internal/runtime/v1/admission-commands:apply",
            &admission,
        )
        .await?;
    anyhow::ensure!(policy_receipt.applied, "Runtime rejected Retention Policy");

    let request = RetentionCommandRequestV1 {
        api_version: 1,
        tenant_id: claim.tenant_id,
        run_id: claim.runtime_command_id,
        policy_version: claim.policy_version,
        dry_run: claim.dry_run,
        idempotency_key: format!("retention:{}:run", claim.id),
    };
    let run_receipt: ApplyReceiptV1 = publisher
        .post(
            "runtime.retention.apply",
            "/internal/runtime/v1/retention-commands:apply",
            &request,
        )
        .await?;
    anyhow::ensure!(run_receipt.applied, "Runtime rejected Retention Run");
    let changed = sqlx::query(
        "UPDATE retention_runs SET runtime_receipt_json=?,error_message=NULL,locked_by=NULL,locked_until=NULL WHERE id=? AND status='running' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(json!({"policy":policy_receipt,"run":run_receipt}))
    .bind(claim.id)
    .bind(publisher.owner.0)
    .bind(claim.fencing_token)
    .execute(&publisher.pool)
    .await
    .context("persist Control Retention Receipt")?;
    anyhow::ensure!(changed.rows_affected() == 1, "Control Retention LeaseLost");
    Ok(())
}

fn derived_id(source: Uuid, discriminator: u8) -> Uuid {
    let mut bytes = *source.as_bytes();
    bytes[15] ^= discriminator;
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use agentx_mysql_lease::LeaseOwner;
    use sqlx::{MySqlPool, mysql::MySqlPoolOptions};
    use testcontainers::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    };
    use uuid::Uuid;

    use super::claim;

    #[tokio::test]
    async fn projected_queued_run_with_runtime_receipt_is_not_dispatched_again() {
        let container = GenericImage::new("mysql", "8.4")
            .with_exposed_port(3306.tcp())
            .with_wait_for(WaitFor::message_on_stdout(
                "MySQL init process done. Ready for start up.",
            ))
            .with_wait_for(WaitFor::message_on_stderr("ready for connections"))
            .with_env_var("MYSQL_DATABASE", "agentx_control")
            .with_env_var("MYSQL_USER", "agentx")
            .with_env_var("MYSQL_PASSWORD", "agentx-test-password")
            .with_env_var("MYSQL_ROOT_PASSWORD", "agentx-root-password")
            .start()
            .await
            .unwrap();
        let port = container.get_host_port_ipv4(3306.tcp()).await.unwrap();
        let pool = connect(port).await;
        sqlx::raw_sql(
            "CREATE TABLE retention_policies(tenant_id BINARY(16) NOT NULL,data_type VARCHAR(64) NOT NULL,version BIGINT UNSIGNED NOT NULL DEFAULT 1,PRIMARY KEY(tenant_id,data_type));\
             CREATE TABLE retention_runs(id BINARY(16) NOT NULL PRIMARY KEY,tenant_id BINARY(16) NOT NULL,runtime_command_id BINARY(16) NULL,policy_version BIGINT UNSIGNED NOT NULL DEFAULT 1,dry_run BOOLEAN NOT NULL,status VARCHAR(16) NOT NULL,runtime_receipt_json JSON NULL,idempotency_key VARCHAR(192) NULL,available_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),locked_by BINARY(16) NULL,locked_until TIMESTAMP(6) NULL,fencing_token BIGINT UNSIGNED NOT NULL DEFAULT 0,attempt_count INT UNSIGNED NOT NULL DEFAULT 0,started_at TIMESTAMP(6) NULL,created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6));",
        )
        .execute(&pool)
        .await
        .unwrap();
        let tenant_id = Uuid::now_v7();
        let dispatched_id = Uuid::now_v7();
        let pending_id = Uuid::now_v7();
        sqlx::query("INSERT INTO retention_policies(tenant_id,data_type) VALUES(?,'artifact')")
            .bind(tenant_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO retention_runs(id,tenant_id,dry_run,status,runtime_receipt_json) VALUES(?,?,TRUE,'queued',JSON_OBJECT('applied',TRUE)),(?,?,TRUE,'queued',NULL)")
            .bind(dispatched_id)
            .bind(tenant_id)
            .bind(pending_id)
            .bind(tenant_id)
            .execute(&pool)
            .await
            .unwrap();

        let owner = LeaseOwner::new();
        let claimed = claim(&pool, owner).await.unwrap().unwrap();
        assert_eq!(claimed.id, pending_id);
        assert!(claim(&pool, owner).await.unwrap().is_none());
        let dispatched_status: String =
            sqlx::query_scalar("SELECT status FROM retention_runs WHERE id=?")
                .bind(dispatched_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(dispatched_status, "queued");
    }

    async fn connect(port: u16) -> MySqlPool {
        let url = format!("mysql://agentx:agentx-test-password@127.0.0.1:{port}/agentx_control");
        for _ in 0..40 {
            if let Ok(pool) = MySqlPoolOptions::new().connect(&url).await {
                return pool;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        panic!("MySQL test container did not become ready")
    }
}
