use std::{collections::BTreeSet, env, time::Duration};

use agentx_mysql_lease::DEFAULT_BATCH_SIZE;
use agentx_runtime_contracts::{
    ControlRole, EventExportPageV1, GovernanceSnapshotPageV1, GovernanceSnapshotRequestV1,
    RuntimeEventPayloadV1, RuntimeGovernanceObjectKindV1, RuntimeGovernanceSnapshotItemV1,
    RuntimeGovernanceSnapshotPayloadV1, RuntimeIntegrationEventEnvelopeV1, ServiceClaimsV1,
    content_hash, issue_service_token, now_unix,
};
use anyhow::Result;
use reqwest::StatusCode;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use sqlx::{MySql, MySqlPool, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

const PROJECTION: &str = "runtime_governance_v1";
const PARTITION: &str = "global";
const BATCH_SIZE: u32 = DEFAULT_BATCH_SIZE;

pub struct Projector {
    pool: MySqlPool,
    runtime_url: String,
    http: reqwest::Client,
    kid: String,
    key: SecretString,
    owner: Uuid,
}

#[derive(Clone, Copy)]
struct Lease {
    cursor: u64,
    fencing_token: u64,
}

impl Projector {
    pub fn from_env(pool: MySqlPool) -> Result<Self> {
        Ok(Self {
            pool,
            runtime_url: env::var("AGENTX_RUNTIME_INTERNAL_URL").unwrap_or_else(|_| {
                "http://runtime-gateway-internal.agentx-runtime.svc:8080".into()
            }),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(35))
                .build()?,
            kid: env::var("AGENTX_CONTROL_PROJECTOR_JWT_KID")?,
            key: SecretString::from(env::var("AGENTX_CONTROL_PROJECTOR_JWT_PRIVATE_KEY_PEM")?),
            owner: agentx_mysql_lease::LeaseOwner::for_process()?.0,
        })
    }

    pub async fn run_loop(
        self,
        lifecycle: agentx_service_kit::ServiceLifecycle,
        progress: agentx_service_kit::RoleProgressWatchdog,
    ) -> Result<()> {
        self.bootstrap().await?;
        loop {
            if lifecycle.is_draining() {
                return Ok(());
            }
            let started = std::time::Instant::now();
            match self.claim().await? {
                Some(lease) => {
                    if let Err(error) = self.pull_once_with_heartbeat(lease).await {
                        tracing::warn!(%error, "Runtime governance projection failed");
                        self.record_error(lease, &error).await?;
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                None => tokio::time::sleep(Duration::from_millis(500)).await,
            }
            progress.processed_since(started).await;
        }
    }

    async fn pull_once_with_heartbeat(&self, lease: Lease) -> Result<()> {
        let pull = self.pull_once(lease);
        tokio::pin!(pull);
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(10),
            Duration::from_secs(10),
        );
        loop {
            tokio::select! {
                biased;
                result = &mut pull => return result,
                _ = heartbeat.tick() => self.heartbeat(lease).await?,
            }
        }
    }

    async fn bootstrap(&self) -> Result<()> {
        sqlx::query("INSERT IGNORE INTO runtime_projection_cursors(projection_name,partition_key) VALUES(?,?)")
            .bind(PROJECTION)
            .bind(PARTITION)
            .execute(&self.pool)
            .await?;
        sqlx::query("INSERT IGNORE INTO runtime_projection_status(projection_name,partition_key,state) VALUES(?,?,'rebuilding')")
            .bind(PROJECTION)
            .bind(PARTITION)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn claim(&self) -> Result<Option<Lease>> {
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("UPDATE runtime_projection_cursors SET locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1 WHERE projection_name=? AND partition_key=? AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6))")
            .bind(self.owner).bind(PROJECTION).bind(PARTITION).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(None);
        }
        let row = sqlx::query("SELECT last_cursor,fencing_token FROM runtime_projection_cursors WHERE projection_name=? AND partition_key=? AND locked_by=? FOR UPDATE")
            .bind(PROJECTION).bind(PARTITION).bind(self.owner).fetch_one(&mut *tx).await?;
        let lease = Lease {
            cursor: row.try_get("last_cursor")?,
            fencing_token: row.try_get("fencing_token")?,
        };
        tx.commit().await?;
        Ok(Some(lease))
    }

    async fn pull_once(&self, lease: Lease) -> Result<()> {
        let active_generation: u64 = sqlx::query_scalar("SELECT active_generation FROM runtime_projection_status WHERE projection_name=? AND partition_key=?")
            .bind(PROJECTION).bind(PARTITION).fetch_one(&self.pool).await?;
        if active_generation == 0 {
            return self.rebuild(lease).await;
        }
        let token = self.service_token("runtime.events.read")?;
        let response = self
            .http
            .get(format!(
                "{}/internal/runtime/v1/events:export",
                self.runtime_url
            ))
            .bearer_auth(token)
            .query(&[
                ("apiVersion", 1_u64),
                ("afterCursor", lease.cursor),
                ("limit", u64::from(BATCH_SIZE)),
                ("waitSeconds", 1_u64),
            ])
            .send()
            .await?;
        if response.status() == StatusCode::GONE {
            return self.rebuild(lease).await;
        }
        anyhow::ensure!(
            response.status().is_success(),
            "Runtime Event Export returned {}",
            response.status()
        );
        let page: EventExportPageV1 = response.json().await?;
        self.apply_page(lease, page).await
    }

    async fn apply_page(&self, lease: Lease, page: EventExportPageV1) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.verify_lease(&mut tx, lease).await?;
        let mut cursor = lease.cursor;
        for event in page.events {
            anyhow::ensure!(
                event.cursor == cursor + 1,
                "Runtime Event Cursor is not continuous"
            );
            self.apply_event(&mut tx, &event).await?;
            cursor = event.cursor;
        }
        let changed = sqlx::query("UPDATE runtime_projection_cursors SET last_cursor=?,locked_until=NULL,locked_by=NULL WHERE projection_name=? AND partition_key=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
            .bind(cursor).bind(PROJECTION).bind(PARTITION).bind(self.owner).bind(lease.fencing_token).execute(&mut *tx).await?;
        anyhow::ensure!(changed.rows_affected() == 1, "Projector Lease was lost");
        sqlx::query("UPDATE runtime_projection_status SET state='ready',current_cursor=?,retention_floor_cursor=?,last_error_code=NULL,last_error_message=NULL,last_success_at=UTC_TIMESTAMP(6) WHERE projection_name=? AND partition_key=?")
            .bind(cursor).bind(page.retention_floor_cursor).bind(PROJECTION).bind(PARTITION).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn apply_event(
        &self,
        tx: &mut Transaction<'_, MySql>,
        event: &RuntimeIntegrationEventEnvelopeV1,
    ) -> Result<()> {
        if let Some(receipt) = sqlx::query(
            "SELECT content_hash FROM projection_receipts WHERE projector_name=? AND event_id=?",
        )
        .bind(PROJECTION)
        .bind(event.event_id)
        .fetch_optional(&mut **tx)
        .await?
        {
            anyhow::ensure!(
                receipt.try_get::<String, _>("content_hash")? == event.content_hash.as_str(),
                "PROJECTION_EVENT_HASH_CONFLICT"
            );
            return Ok(());
        }
        let generation: u64 = sqlx::query_scalar("SELECT active_generation FROM runtime_projection_status WHERE projection_name=? AND partition_key=?")
            .bind(PROJECTION).bind(PARTITION).fetch_one(&mut **tx).await?;
        let outcome = self.apply_payload(tx, event, generation).await?;
        if event.event_type.ends_with(".deleted") {
            self.apply_tombstone(
                tx,
                generation,
                event.tenant_id,
                event.aggregate_version,
                event.cursor,
                &event.payload,
            )
            .await?;
        }
        sqlx::query("INSERT INTO projection_receipts(projector_name,event_id,tenant_id,content_hash,object_version,event_cursor,outcome) VALUES(?,?,?,?,?,?,?)")
            .bind(PROJECTION).bind(event.event_id).bind(event.tenant_id).bind(event.content_hash.as_str())
            .bind(event.aggregate_version).bind(event.cursor).bind(outcome).execute(&mut **tx).await?;
        Ok(())
    }

    async fn apply_payload(
        &self,
        tx: &mut Transaction<'_, MySql>,
        event: &RuntimeIntegrationEventEnvelopeV1,
        generation: u64,
    ) -> Result<&'static str> {
        let applied = match &event.payload {
            RuntimeEventPayloadV1::ApprovalChanged {
                task_id,
                task_version,
                execution_id,
                workflow_id,
                node_id,
                title,
                description,
                request,
                buttons,
                status,
                resume_status,
                claimed_by,
                deadline_at,
                decision,
                candidates,
            } => {
                let current: Option<u64> = sqlx::query_scalar(
                    "SELECT version FROM approval_task_projection WHERE tenant_id=? AND id=?",
                )
                .bind(event.tenant_id)
                .bind(task_id)
                .fetch_optional(&mut **tx)
                .await?;
                let eligible = current.is_none_or(|version| version <= *task_version);
                let changed = sqlx::query("INSERT INTO approval_task_projection(id,tenant_id,execution_id,workflow_id,node_id,title,description,request_payload_json,buttons_json,status,resume_status,claimed_by,deadline_at,version,projection_generation,source_event_id,source_event_cursor,projection_deleted,decision_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE execution_id=IF(version<=VALUES(version),VALUES(execution_id),execution_id),workflow_id=IF(version<=VALUES(version),VALUES(workflow_id),workflow_id),node_id=IF(version<=VALUES(version),VALUES(node_id),node_id),title=IF(version<=VALUES(version),VALUES(title),title),description=IF(version<=VALUES(version),VALUES(description),description),request_payload_json=IF(version<=VALUES(version),VALUES(request_payload_json),request_payload_json),buttons_json=IF(version<=VALUES(version),VALUES(buttons_json),buttons_json),status=IF(version<=VALUES(version),VALUES(status),status),resume_status=IF(version<=VALUES(version),VALUES(resume_status),resume_status),claimed_by=IF(version<=VALUES(version),VALUES(claimed_by),claimed_by),deadline_at=IF(version<=VALUES(version),VALUES(deadline_at),deadline_at),decision_json=IF(version<=VALUES(version),VALUES(decision_json),decision_json),projection_generation=IF(version<=VALUES(version),VALUES(projection_generation),projection_generation),source_event_id=IF(version<=VALUES(version),VALUES(source_event_id),source_event_id),source_event_cursor=IF(version<=VALUES(version),GREATEST(source_event_cursor,VALUES(source_event_cursor)),source_event_cursor),projection_deleted=IF(version<=VALUES(version),FALSE,projection_deleted),version=GREATEST(version,VALUES(version))")
                    .bind(task_id).bind(event.tenant_id).bind(execution_id).bind(workflow_id).bind(node_id).bind(title).bind(description).bind(request).bind(serde_json::to_value(buttons)?).bind(status).bind(resume_status).bind(claimed_by).bind(deadline_at).bind(task_version).bind(generation).bind(event.event_id).bind(event.cursor).bind(false).bind(decision).execute(&mut **tx).await?.rows_affected() > 0;
                let applied = eligible && changed;
                if applied {
                    self.replace_approval_candidates(
                        tx,
                        event.tenant_id,
                        *task_id,
                        *task_version,
                        generation,
                        candidates,
                    )
                    .await?;
                }
                applied
            }
            RuntimeEventPayloadV1::EvaluationChanged {
                work_package_id,
                run_version,
                status,
                completed_cases,
                total_cases,
                report,
                ..
            } => {
                let current: Option<u64> = sqlx::query_scalar("SELECT intent_version FROM evaluation_runs WHERE tenant_id=? AND work_package_id=?")
                    .bind(event.tenant_id).bind(work_package_id).fetch_optional(&mut **tx).await?;
                let eligible = current.is_some_and(|version| version <= *run_version);
                let report_json = report.as_ref().map(serde_json::to_value).transpose()?;
                let changed = sqlx::query("UPDATE evaluation_runs SET status=?,intent_version=GREATEST(intent_version,?),completed_cases=?,total_cases=?,runtime_report_json=?,projection_generation=?,source_event_id=?,source_event_cursor=?,projection_deleted=FALSE WHERE tenant_id=? AND work_package_id=? AND intent_version<=?")
                    .bind(status).bind(run_version).bind(completed_cases).bind(total_cases).bind(report_json).bind(generation).bind(event.event_id).bind(event.cursor).bind(event.tenant_id).bind(work_package_id).bind(run_version).execute(&mut **tx).await?.rows_affected() > 0;
                let applied = eligible && changed;
                if applied && let Some(report) = report {
                    self.replace_evaluation_report(
                        tx,
                        event.tenant_id,
                        *work_package_id,
                        *run_version,
                        event.cursor,
                        generation,
                        report,
                    )
                    .await?;
                }
                applied
            }
            RuntimeEventPayloadV1::DebugChanged {
                work_package_id,
                package_version,
                status,
                result,
                expires_at,
                ..
            } => {
                let current: Option<u64> = sqlx::query_scalar("SELECT command_version FROM workflow_debug_runs WHERE tenant_id=? AND work_package_id=?")
                    .bind(event.tenant_id).bind(work_package_id).fetch_optional(&mut **tx).await?;
                let eligible = current.is_some_and(|version| version <= *package_version);
                let changed = sqlx::query("UPDATE workflow_debug_runs SET status=?,command_version=GREATEST(command_version,?),result_receipt_json=?,expires_at=?,projection_generation=?,source_event_id=?,source_event_cursor=?,projection_deleted=FALSE WHERE tenant_id=? AND work_package_id=? AND command_version<=?")
                    .bind(status).bind(package_version).bind(result).bind(expires_at).bind(generation).bind(event.event_id).bind(event.cursor).bind(event.tenant_id).bind(work_package_id).bind(package_version).execute(&mut **tx).await?.rows_affected() > 0;
                eligible && changed
            }
            RuntimeEventPayloadV1::RetentionChanged {
                run_id,
                run_version,
                status,
                marked_count,
                deleted_count,
                failed_count,
                dry_run,
                items,
            } => {
                let current: Option<u64> = sqlx::query_scalar("SELECT policy_version FROM retention_runs WHERE tenant_id=? AND runtime_command_id=?")
                    .bind(event.tenant_id).bind(run_id).fetch_optional(&mut **tx).await?;
                let eligible = current.is_some_and(|version| version <= *run_version);
                let changed = sqlx::query("UPDATE retention_runs SET status=?,policy_version=GREATEST(policy_version,?),candidate_count=?,deleted_count=?,failed_count=?,dry_run=?,projection_generation=?,source_event_id=?,source_event_cursor=?,projection_deleted=FALSE WHERE tenant_id=? AND runtime_command_id=? AND policy_version<=?")
                    .bind(status).bind(run_version).bind(marked_count).bind(deleted_count).bind(failed_count).bind(dry_run).bind(generation).bind(event.event_id).bind(event.cursor).bind(event.tenant_id).bind(run_id).bind(run_version).execute(&mut **tx).await?.rows_affected() > 0;
                let applied = eligible && changed;
                if applied {
                    self.replace_retention_items(
                        tx,
                        event.tenant_id,
                        *run_id,
                        *run_version,
                        event.cursor,
                        generation,
                        items,
                    )
                    .await?;
                }
                applied
            }
            RuntimeEventPayloadV1::NotificationChanged {
                notification_id,
                notification_version,
                notification_type,
                title_key,
                body_key,
                arguments,
                target_type,
                target_id,
                target_path,
                tone,
            } => {
                let current: Option<u64> = sqlx::query_scalar(
                    "SELECT runtime_object_version FROM notifications WHERE tenant_id=? AND id=?",
                )
                .bind(event.tenant_id)
                .bind(notification_id)
                .fetch_optional(&mut **tx)
                .await?;
                let eligible = current.is_none_or(|version| version <= *notification_version);
                let changed = sqlx::query("INSERT INTO notifications(id,tenant_id,source_event_id,source_plane,runtime_object_version,notification_type,title_key,body_key,arguments_json,target_type,target_id,target_path,tone,projection_generation,source_event_cursor,projection_deleted) VALUES(?,?,?,'runtime',?,?,?,?,?,?,?,?,?,?,?,FALSE) ON DUPLICATE KEY UPDATE source_event_id=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(source_event_id),source_event_id),notification_type=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(notification_type),notification_type),title_key=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(title_key),title_key),body_key=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(body_key),body_key),arguments_json=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(arguments_json),arguments_json),target_type=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(target_type),target_type),target_id=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(target_id),target_id),target_path=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(target_path),target_path),tone=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(tone),tone),projection_generation=IF(runtime_object_version<=VALUES(runtime_object_version),VALUES(projection_generation),projection_generation),source_event_cursor=IF(runtime_object_version<=VALUES(runtime_object_version),GREATEST(source_event_cursor,VALUES(source_event_cursor)),source_event_cursor),projection_deleted=IF(runtime_object_version<=VALUES(runtime_object_version),FALSE,projection_deleted),runtime_object_version=GREATEST(runtime_object_version,VALUES(runtime_object_version))")
                    .bind(notification_id).bind(event.tenant_id).bind(event.event_id).bind(notification_version).bind(notification_type).bind(title_key).bind(body_key).bind(arguments).bind(target_type).bind(target_id).bind(target_path).bind(tone).bind(generation).bind(event.cursor).execute(&mut **tx).await?.rows_affected() > 0;
                let applied = eligible && changed;
                if applied && target_type == "user" {
                    sqlx::query("INSERT INTO notification_receipts(tenant_id,notification_id,user_id) VALUES(?,?,?) ON DUPLICATE KEY UPDATE notification_id=notification_id")
                        .bind(event.tenant_id).bind(notification_id).bind(target_id).execute(&mut **tx).await?;
                }
                applied
            }
            _ => false,
        };
        Ok(if applied {
            "applied"
        } else {
            "ignored_old_version"
        })
    }

    async fn replace_approval_candidates(
        &self,
        tx: &mut Transaction<'_, MySql>,
        tenant_id: Uuid,
        task_id: Uuid,
        task_version: u64,
        generation: u64,
        candidates: &[agentx_runtime_contracts::RuntimeApprovalCandidateV1],
    ) -> Result<()> {
        let current: Option<u64> = sqlx::query_scalar(
            "SELECT version FROM approval_task_projection WHERE tenant_id=? AND id=?",
        )
        .bind(tenant_id)
        .bind(task_id)
        .fetch_optional(&mut **tx)
        .await?;
        if current != Some(task_version) {
            return Ok(());
        }
        sqlx::query("DELETE FROM approval_candidate_projection WHERE tenant_id=? AND approval_task_id=? AND projection_generation=?")
            .bind(tenant_id).bind(task_id).bind(generation).execute(&mut **tx).await?;
        for candidate in candidates {
            let kind = match candidate.candidate_type {
                agentx_runtime_contracts::RuntimeApprovalCandidateKindV1::User => "user",
                agentx_runtime_contracts::RuntimeApprovalCandidateKindV1::Role => "role",
                agentx_runtime_contracts::RuntimeApprovalCandidateKindV1::Department => {
                    "department"
                }
            };
            sqlx::query("INSERT INTO approval_candidate_projection(tenant_id,approval_task_id,candidate_type,candidate_id,projection_generation) VALUES(?,?,?,?,?) ON DUPLICATE KEY UPDATE projection_generation=VALUES(projection_generation)")
                .bind(tenant_id).bind(task_id).bind(kind).bind(candidate.candidate_id).bind(generation).execute(&mut **tx).await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn replace_evaluation_report(
        &self,
        tx: &mut Transaction<'_, MySql>,
        tenant_id: Uuid,
        work_package_id: Uuid,
        run_version: u64,
        cursor: u64,
        generation: u64,
        report: &agentx_runtime_contracts::RuntimeEvaluationReportV1,
    ) -> Result<()> {
        let control_run_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM evaluation_runs WHERE tenant_id=? AND work_package_id=? AND intent_version=?")
            .bind(tenant_id).bind(work_package_id).bind(run_version).fetch_optional(&mut **tx).await?;
        let Some(control_run_id) = control_run_id else {
            return Ok(());
        };
        sqlx::query("DELETE rr FROM evaluation_rule_results rr JOIN evaluation_case_projection c ON c.tenant_id=rr.tenant_id AND c.id=rr.evaluation_run_case_id WHERE c.tenant_id=? AND c.evaluation_run_id=? AND c.projection_generation=?")
            .bind(tenant_id).bind(control_run_id).bind(generation).execute(&mut **tx).await?;
        sqlx::query("DELETE FROM evaluation_case_projection WHERE tenant_id=? AND evaluation_run_id=? AND projection_generation=?")
            .bind(tenant_id).bind(control_run_id).bind(generation).execute(&mut **tx).await?;
        for case in &report.cases {
            sqlx::query("INSERT INTO evaluation_case_projection(id,tenant_id,evaluation_run_id,source_case_id,target_command_id,target_execution_id,status,projection_generation,source_event_cursor,actual_output_json,duration_ms,cost_micros,error_code,error_message,completed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,IF(? IN ('completed','failed','cancelled'),UTC_TIMESTAMP(6),NULL)) ON DUPLICATE KEY UPDATE evaluation_run_id=VALUES(evaluation_run_id),source_case_id=VALUES(source_case_id),target_command_id=VALUES(target_command_id),target_execution_id=VALUES(target_execution_id),status=VALUES(status),projection_generation=VALUES(projection_generation),source_event_cursor=VALUES(source_event_cursor),actual_output_json=VALUES(actual_output_json),duration_ms=VALUES(duration_ms),cost_micros=VALUES(cost_micros),error_code=VALUES(error_code),error_message=VALUES(error_message),completed_at=VALUES(completed_at)")
                .bind(case.id).bind(tenant_id).bind(control_run_id).bind(case.source_case_id).bind(case.target_command_id).bind(case.target_execution_id).bind(&case.status).bind(generation).bind(cursor).bind(&case.actual_output).bind(case.duration_ms).bind(case.cost_micros).bind(&case.error_code).bind(&case.error_message).bind(&case.status).execute(&mut **tx).await?;
            for rule in &case.rules {
                sqlx::query("INSERT INTO evaluation_rule_results(id,tenant_id,evaluation_run_case_id,profile_rule_id,evaluator_execution_id,status,projection_generation,source_event_cursor,passed,score,detail_json,duration_ms,cost_micros,completed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,IF(? IN ('passed','failed','error','cancelled'),UTC_TIMESTAMP(6),NULL)) ON DUPLICATE KEY UPDATE evaluation_run_case_id=VALUES(evaluation_run_case_id),profile_rule_id=VALUES(profile_rule_id),evaluator_execution_id=VALUES(evaluator_execution_id),status=VALUES(status),projection_generation=VALUES(projection_generation),source_event_cursor=VALUES(source_event_cursor),passed=VALUES(passed),score=VALUES(score),detail_json=VALUES(detail_json),duration_ms=VALUES(duration_ms),cost_micros=VALUES(cost_micros),completed_at=VALUES(completed_at)")
                    .bind(rule.id).bind(tenant_id).bind(case.id).bind(rule.profile_rule_id).bind(rule.evaluator_execution_id).bind(&rule.status).bind(generation).bind(cursor).bind(rule.passed).bind(rule.score).bind(&rule.detail).bind(rule.duration_ms).bind(rule.cost_micros).bind(&rule.status).execute(&mut **tx).await?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn replace_retention_items(
        &self,
        tx: &mut Transaction<'_, MySql>,
        tenant_id: Uuid,
        runtime_run_id: Uuid,
        run_version: u64,
        cursor: u64,
        generation: u64,
        items: &[agentx_runtime_contracts::RuntimeRetentionItemV1],
    ) -> Result<()> {
        let control_run_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM retention_runs WHERE tenant_id=? AND runtime_command_id=? AND policy_version=?")
            .bind(tenant_id).bind(runtime_run_id).bind(run_version).fetch_optional(&mut **tx).await?;
        let Some(control_run_id) = control_run_id else {
            return Ok(());
        };
        sqlx::query("DELETE FROM retention_items WHERE tenant_id=? AND retention_run_id=? AND projection_generation=?")
            .bind(tenant_id).bind(control_run_id).bind(generation).execute(&mut **tx).await?;
        for item in items {
            sqlx::query("INSERT INTO retention_items(id,tenant_id,retention_run_id,data_type,target_id,status,projection_generation,source_event_cursor,projection_deleted,reason,attempt_count) VALUES(?,?,?,?,?,?,?,?,FALSE,?,?) ON DUPLICATE KEY UPDATE retention_run_id=VALUES(retention_run_id),data_type=VALUES(data_type),target_id=VALUES(target_id),status=VALUES(status),projection_generation=VALUES(projection_generation),source_event_cursor=VALUES(source_event_cursor),projection_deleted=FALSE,reason=VALUES(reason),attempt_count=VALUES(attempt_count)")
                .bind(item.id).bind(tenant_id).bind(control_run_id).bind(&item.data_type).bind(&item.target_id).bind(&item.status).bind(generation).bind(cursor).bind(&item.reason).bind(item.attempt_count).execute(&mut **tx).await?;
        }
        Ok(())
    }

    async fn rebuild(&self, lease: Lease) -> Result<()> {
        self.heartbeat(lease).await?;
        let generation: u64 = sqlx::query_scalar("SELECT active_generation+1 FROM runtime_projection_status WHERE projection_name=? AND partition_key=?")
            .bind(PROJECTION).bind(PARTITION).fetch_one(&self.pool).await?;
        sqlx::query("UPDATE runtime_projection_status SET state='rebuilding',building_generation=?,last_error_code=NULL,last_error_message=NULL WHERE projection_name=? AND partition_key=?")
            .bind(generation).bind(PROJECTION).bind(PARTITION).execute(&self.pool).await?;
        sqlx::query("DELETE FROM projection_rebuild_items WHERE projection_name=? AND partition_key=? AND generation=?")
            .bind(PROJECTION).bind(PARTITION).bind(generation).execute(&self.pool).await?;
        let tenants: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tenants ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        let mut upper = None;
        let mut floor = 0_u64;
        for tenant in tenants {
            self.heartbeat(lease).await?;
            let (tenant_upper, tenant_floor) =
                self.snapshot_tenant(tenant, generation, upper).await?;
            if let Some(frozen) = upper {
                anyhow::ensure!(
                    tenant_upper == frozen,
                    "Runtime Snapshot Upper Cursor changed during rebuild"
                );
            }
            upper = Some(tenant_upper);
            floor = floor.max(tenant_floor);
        }
        let upper = upper.unwrap_or(lease.cursor);
        let (catchup, catchup_upper, catchup_floor) = self.catchup_events(upper, lease).await?;
        floor = floor.max(catchup_floor);
        self.heartbeat(lease).await?;
        let mut tx = self.pool.begin().await?;
        self.verify_lease(&mut tx, lease).await?;
        let rows = sqlx::query("SELECT tenant_id,object_type,object_id,object_version,source_event_cursor,payload_json,projection_deleted FROM projection_rebuild_items WHERE projection_name=? AND partition_key=? AND generation=? ORDER BY tenant_id,object_type,object_id")
            .bind(PROJECTION).bind(PARTITION).bind(generation).fetch_all(&mut *tx).await?;
        for row in rows {
            let item = RuntimeGovernanceSnapshotItemV1 {
                object_type: serde_json::from_value(Value::String(row.try_get("object_type")?))?,
                object_id: row.try_get("object_id")?,
                object_version: row.try_get("object_version")?,
                last_event_cursor: row.try_get("source_event_cursor")?,
                deleted: row.try_get("projection_deleted")?,
                payload: serde_json::from_value(row.try_get("payload_json")?)?,
            };
            self.apply_snapshot_item(&mut tx, generation, row.try_get("tenant_id")?, &item)
                .await?;
        }
        let mut cursor = upper;
        for event in catchup {
            anyhow::ensure!(
                event.cursor == cursor + 1,
                "Runtime Event Cursor is not continuous during rebuild"
            );
            self.apply_event_for_generation(&mut tx, &event, generation)
                .await?;
            cursor = event.cursor;
        }
        anyhow::ensure!(
            cursor == catchup_upper,
            "Runtime Snapshot catch-up did not reach its frozen upper cursor"
        );
        let changed = sqlx::query("UPDATE runtime_projection_cursors SET last_cursor=?,snapshot_version=snapshot_version+1,locked_by=NULL,locked_until=NULL WHERE projection_name=? AND partition_key=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
            .bind(catchup_upper).bind(PROJECTION).bind(PARTITION).bind(self.owner).bind(lease.fencing_token).execute(&mut *tx).await?;
        anyhow::ensure!(
            changed.rows_affected() == 1,
            "Projector Lease was lost during rebuild"
        );
        sqlx::query("UPDATE runtime_projection_status SET state='ready',current_cursor=?,retention_floor_cursor=?,active_generation=?,building_generation=NULL,snapshot_upper_cursor=?,last_success_at=UTC_TIMESTAMP(6) WHERE projection_name=? AND partition_key=?")
            .bind(catchup_upper).bind(floor).bind(generation).bind(upper).bind(PROJECTION).bind(PARTITION).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn snapshot_tenant(
        &self,
        tenant: Uuid,
        generation: u64,
        frozen_upper: Option<u64>,
    ) -> Result<(u64, u64)> {
        let mut page_cursor = None;
        let mut upper = frozen_upper;
        loop {
            let request = GovernanceSnapshotRequestV1 {
                api_version: 1,
                tenant_id: tenant,
                object_types: vec![
                    RuntimeGovernanceObjectKindV1::Approval,
                    RuntimeGovernanceObjectKindV1::Evaluation,
                    RuntimeGovernanceObjectKindV1::Notification,
                    RuntimeGovernanceObjectKindV1::Debug,
                    RuntimeGovernanceObjectKindV1::Retention,
                ],
                snapshot_upper_cursor: upper,
                page_cursor: page_cursor.clone(),
                limit: 1000,
            };
            let response = self
                .http
                .post(format!(
                    "{}/internal/runtime/v1/governance-snapshots:export",
                    self.runtime_url
                ))
                .bearer_auth(self.service_token("runtime.snapshots.read")?)
                .json(&request)
                .send()
                .await?;
            anyhow::ensure!(
                response.status().is_success(),
                "Runtime Snapshot Export returned {}",
                response.status()
            );
            let page: GovernanceSnapshotPageV1 = response.json().await?;
            upper = Some(page.snapshot_upper_cursor);
            let floor = page.retention_floor_cursor;
            for item in page.objects {
                let hash = content_hash(&item)?;
                sqlx::query("INSERT INTO projection_rebuild_items(projection_name,partition_key,generation,tenant_id,object_type,object_id,object_version,source_event_cursor,content_hash,payload_json,projection_deleted) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
                    .bind(PROJECTION).bind(PARTITION).bind(generation).bind(tenant).bind(kind_name(item.object_type)).bind(item.object_id).bind(item.object_version).bind(item.last_event_cursor).bind(hash.as_str()).bind(serde_json::to_value(&item.payload)?).bind(item.deleted).execute(&self.pool).await?;
            }
            page_cursor = page.next_page_cursor;
            if page_cursor.is_none() {
                return Ok((upper.unwrap_or_default(), floor));
            }
        }
    }

    async fn apply_snapshot_item(
        &self,
        tx: &mut Transaction<'_, MySql>,
        generation: u64,
        tenant_id: Uuid,
        item: &RuntimeGovernanceSnapshotItemV1,
    ) -> Result<()> {
        let event_id = item.object_id;
        let event = RuntimeIntegrationEventEnvelopeV1 {
            schema_version: 1,
            cursor: item.last_event_cursor,
            event_id,
            source_outbox_id: event_id,
            tenant_id,
            aggregate_type: kind_name(item.object_type).into(),
            aggregate_id: item.object_id.to_string(),
            aggregate_version: item.object_version,
            event_type: "snapshot".into(),
            occurred_at: OffsetDateTime::now_utc(),
            payload: snapshot_event_payload(item)?,
            content_hash: content_hash(item)?,
            correlation_id: event_id,
            causation_id: None,
        };
        self.apply_payload(tx, &event, generation).await?;
        if item.deleted {
            self.apply_tombstone(
                tx,
                generation,
                tenant_id,
                item.object_version,
                item.last_event_cursor,
                &event.payload,
            )
            .await?;
        }
        Ok(())
    }

    async fn apply_event_for_generation(
        &self,
        tx: &mut Transaction<'_, MySql>,
        event: &RuntimeIntegrationEventEnvelopeV1,
        generation: u64,
    ) -> Result<()> {
        if let Some(receipt) = sqlx::query(
            "SELECT content_hash FROM projection_receipts WHERE projector_name=? AND event_id=?",
        )
        .bind(PROJECTION)
        .bind(event.event_id)
        .fetch_optional(&mut **tx)
        .await?
        {
            anyhow::ensure!(
                receipt.try_get::<String, _>("content_hash")? == event.content_hash.as_str(),
                "PROJECTION_EVENT_HASH_CONFLICT"
            );
            return Ok(());
        }
        let outcome = self.apply_payload(tx, event, generation).await?;
        if event.event_type.ends_with(".deleted") {
            self.apply_tombstone(
                tx,
                generation,
                event.tenant_id,
                event.aggregate_version,
                event.cursor,
                &event.payload,
            )
            .await?;
        }
        sqlx::query("INSERT INTO projection_receipts(projector_name,event_id,tenant_id,content_hash,object_version,event_cursor,outcome) VALUES(?,?,?,?,?,?,?)")
            .bind(PROJECTION).bind(event.event_id).bind(event.tenant_id).bind(event.content_hash.as_str()).bind(event.aggregate_version).bind(event.cursor).bind(outcome).execute(&mut **tx).await?;
        Ok(())
    }

    async fn apply_tombstone(
        &self,
        tx: &mut Transaction<'_, MySql>,
        generation: u64,
        tenant_id: Uuid,
        version: u64,
        cursor: u64,
        payload: &RuntimeEventPayloadV1,
    ) -> Result<()> {
        match payload {
            RuntimeEventPayloadV1::ApprovalChanged { task_id, .. } => {
                sqlx::query("UPDATE approval_task_projection SET projection_deleted=TRUE,projection_generation=?,source_event_cursor=GREATEST(source_event_cursor,?) WHERE tenant_id=? AND id=? AND version<=?").bind(generation).bind(cursor).bind(tenant_id).bind(task_id).bind(version).execute(&mut **tx).await?;
            }
            RuntimeEventPayloadV1::EvaluationChanged {
                work_package_id, ..
            } => {
                sqlx::query("UPDATE evaluation_runs SET projection_deleted=TRUE,projection_generation=?,source_event_cursor=GREATEST(source_event_cursor,?) WHERE tenant_id=? AND work_package_id=? AND intent_version<=?").bind(generation).bind(cursor).bind(tenant_id).bind(work_package_id).bind(version).execute(&mut **tx).await?;
            }
            RuntimeEventPayloadV1::NotificationChanged {
                notification_id, ..
            } => {
                sqlx::query("UPDATE notifications SET projection_deleted=TRUE,projection_generation=?,source_event_cursor=GREATEST(source_event_cursor,?),runtime_object_version=GREATEST(runtime_object_version,?) WHERE tenant_id=? AND id=? AND runtime_object_version<=?").bind(generation).bind(cursor).bind(version).bind(tenant_id).bind(notification_id).bind(version).execute(&mut **tx).await?;
            }
            RuntimeEventPayloadV1::DebugChanged {
                work_package_id, ..
            } => {
                sqlx::query("UPDATE workflow_debug_runs SET projection_deleted=TRUE,projection_generation=?,source_event_cursor=GREATEST(source_event_cursor,?) WHERE tenant_id=? AND work_package_id=? AND command_version<=?").bind(generation).bind(cursor).bind(tenant_id).bind(work_package_id).bind(version).execute(&mut **tx).await?;
            }
            RuntimeEventPayloadV1::RetentionChanged { run_id, .. } => {
                sqlx::query("UPDATE retention_runs SET projection_deleted=TRUE,projection_generation=?,source_event_cursor=GREATEST(source_event_cursor,?) WHERE tenant_id=? AND runtime_command_id=? AND policy_version<=?").bind(generation).bind(cursor).bind(tenant_id).bind(run_id).bind(version).execute(&mut **tx).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn catchup_events(
        &self,
        mut cursor: u64,
        lease: Lease,
    ) -> Result<(Vec<RuntimeIntegrationEventEnvelopeV1>, u64, u64)> {
        let mut events = Vec::new();
        let mut target = None;
        let mut floor = 0;
        loop {
            self.heartbeat(lease).await?;
            let after = cursor;
            let response = self
                .http
                .get(format!(
                    "{}/internal/runtime/v1/events:export",
                    self.runtime_url
                ))
                .bearer_auth(self.service_token("runtime.events.read")?)
                .query(&[
                    ("apiVersion", 1_u64),
                    ("afterCursor", cursor),
                    ("limit", 1000_u64),
                    ("waitSeconds", 0_u64),
                ])
                .send()
                .await?;
            anyhow::ensure!(
                response.status() != StatusCode::GONE,
                "Runtime Event Cursor expired during Snapshot catch-up"
            );
            anyhow::ensure!(
                response.status().is_success(),
                "Runtime Event Export returned {} during Snapshot catch-up",
                response.status()
            );
            let page: EventExportPageV1 = response.json().await?;
            let frozen = *target.get_or_insert(page.upper_cursor);
            floor = floor.max(page.retention_floor_cursor);
            for event in page.events {
                if event.cursor <= frozen {
                    cursor = event.cursor;
                    events.push(event);
                }
            }
            if cursor >= frozen {
                return Ok((events, frozen, floor));
            }
            anyhow::ensure!(
                page.next_cursor > after,
                "Runtime Event Export did not advance during Snapshot catch-up"
            );
            cursor = page.next_cursor;
        }
    }

    async fn heartbeat(&self, lease: Lease) -> Result<()> {
        let changed=sqlx::query("UPDATE runtime_projection_cursors SET locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND) WHERE projection_name=? AND partition_key=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)").bind(PROJECTION).bind(PARTITION).bind(self.owner).bind(lease.fencing_token).execute(&self.pool).await?;
        anyhow::ensure!(changed.rows_affected() == 1, "Projector Lease was lost");
        Ok(())
    }

    async fn verify_lease(&self, tx: &mut Transaction<'_, MySql>, lease: Lease) -> Result<()> {
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_projection_cursors WHERE projection_name=? AND partition_key=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6))")
            .bind(PROJECTION).bind(PARTITION).bind(self.owner).bind(lease.fencing_token).fetch_one(&mut **tx).await?;
        anyhow::ensure!(valid, "Projector Lease was lost");
        Ok(())
    }

    async fn record_error(&self, lease: Lease, error: &anyhow::Error) -> Result<()> {
        let message = error.to_string().chars().take(1000).collect::<String>();
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("UPDATE runtime_projection_cursors SET locked_by=NULL,locked_until=NULL WHERE projection_name=? AND partition_key=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
            .bind(PROJECTION).bind(PARTITION).bind(self.owner).bind(lease.fencing_token).execute(&mut *tx).await?;
        anyhow::ensure!(changed.rows_affected() == 1, "Projector Lease was lost");
        sqlx::query("UPDATE runtime_projection_status SET state='error',last_error_code='PROJECTOR_FAILED',last_error_message=? WHERE projection_name=? AND partition_key=?")
            .bind(message).bind(PROJECTION).bind(PARTITION).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    fn service_token(&self, scope: &str) -> Result<String> {
        let now = now_unix();
        issue_service_token(
            &self.kid,
            self.key.expose_secret().as_bytes(),
            &ServiceClaimsV1 {
                iss: "agentx-control".into(),
                aud: "agentx-runtime-internal".into(),
                sub: "platform-control-projector".into(),
                role: ControlRole::Projector,
                scope: BTreeSet::from([scope.into()]),
                iat: now,
                exp: now + 60,
                jti: Uuid::now_v7(),
            },
        )
        .map_err(Into::into)
    }
}

fn kind_name(kind: RuntimeGovernanceObjectKindV1) -> &'static str {
    match kind {
        RuntimeGovernanceObjectKindV1::Approval => "approval",
        RuntimeGovernanceObjectKindV1::Evaluation => "evaluation",
        RuntimeGovernanceObjectKindV1::Notification => "notification",
        RuntimeGovernanceObjectKindV1::Debug => "debug",
        RuntimeGovernanceObjectKindV1::Retention => "retention",
    }
}

fn snapshot_event_payload(item: &RuntimeGovernanceSnapshotItemV1) -> Result<RuntimeEventPayloadV1> {
    Ok(match &item.payload {
        RuntimeGovernanceSnapshotPayloadV1::Approval {
            execution_id,
            workflow_id,
            node_id,
            title,
            description,
            request,
            buttons,
            status,
            resume_status,
            claimed_by,
            deadline_at,
            decision,
            candidates,
        } => RuntimeEventPayloadV1::ApprovalChanged {
            task_id: item.object_id,
            task_version: item.object_version,
            execution_id: *execution_id,
            workflow_id: *workflow_id,
            node_id: node_id.clone(),
            title: title.clone(),
            description: description.clone(),
            request: request.clone(),
            buttons: buttons.clone(),
            status: status.clone(),
            resume_status: resume_status.clone(),
            claimed_by: *claimed_by,
            deadline_at: *deadline_at,
            decision: decision.clone(),
            candidates: candidates.clone(),
        },
        RuntimeGovernanceSnapshotPayloadV1::Evaluation {
            work_package_id,
            status,
            completed_cases,
            total_cases,
            report,
        } => RuntimeEventPayloadV1::EvaluationChanged {
            run_id: item.object_id,
            run_version: item.object_version,
            work_package_id: *work_package_id,
            status: status.clone(),
            completed_cases: *completed_cases,
            total_cases: *total_cases,
            report: report.clone(),
        },
        RuntimeGovernanceSnapshotPayloadV1::Notification {
            notification_type,
            title_key,
            body_key,
            arguments,
            target_type,
            target_id,
            target_path,
            tone,
        } => RuntimeEventPayloadV1::NotificationChanged {
            notification_id: item.object_id,
            notification_version: item.object_version,
            notification_type: notification_type.clone(),
            title_key: title_key.clone(),
            body_key: body_key.clone(),
            arguments: arguments.clone(),
            target_type: target_type.clone(),
            target_id: *target_id,
            target_path: target_path.clone(),
            tone: tone.clone(),
        },
        RuntimeGovernanceSnapshotPayloadV1::Debug {
            work_package_id,
            status,
            result,
            expires_at,
        } => RuntimeEventPayloadV1::DebugChanged {
            package_id: item.object_id,
            package_version: item.object_version,
            work_package_id: *work_package_id,
            status: status.clone(),
            result: result.clone(),
            expires_at: *expires_at,
        },
        RuntimeGovernanceSnapshotPayloadV1::Retention {
            status,
            marked_count,
            deleted_count,
            failed_count,
            dry_run,
            items,
        } => RuntimeEventPayloadV1::RetentionChanged {
            run_id: item.object_id,
            run_version: item.object_version,
            status: status.clone(),
            marked_count: *marked_count,
            deleted_count: *deleted_count,
            failed_count: *failed_count,
            dry_run: *dry_run,
            items: items.clone(),
        },
    })
}

#[cfg(test)]
mod projector_tests;
