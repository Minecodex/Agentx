use agentx_runtime_contracts::{
    ContentHash, RuntimeApprovalCandidateKindV1, RuntimeApprovalCandidateV1,
    RuntimeEvaluationCaseResultV1, RuntimeEvaluationMetricsV1, RuntimeEvaluationReportV1,
    RuntimeEvaluationRuleResultV1, RuntimeEventPayloadV1, RuntimeIntegrationEventEnvelopeV1,
    RuntimeRetentionItemV1,
};
use axum::response::IntoResponse;
use secrecy::SecretString;
use serde_json::json;
use sqlx::{Row, mysql::MySqlPoolOptions};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Projector;

const PRIVATE_KEY: &str =
    include_str!("../../../../crates/agentx-runtime-contracts/tests/fixtures/service-private.pem");

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn governance_projection_is_versioned_replay_safe_and_complete() {
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
        .expect("Control MySQL container should start");
    let port = container.get_host_port_ipv4(3306.tcp()).await.unwrap();
    let pool = connect_with_retry(port).await;
    agentx_control_infrastructure::migrate_control_mysql(&pool)
        .await
        .unwrap();

    assert_eq!(
        crate::governance_api::projection(&pool)
            .await
            .unwrap_err()
            .into_response()
            .status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    let projector = Projector {
        pool: pool.clone(),
        runtime_url: "http://127.0.0.1:1".into(),
        http: reqwest::Client::new(),
        kid: "projector-test".into(),
        key: SecretString::from(PRIVATE_KEY),
        owner: Uuid::now_v7(),
    };
    projector.bootstrap().await.unwrap();
    assert_eq!(
        crate::governance_api::projection(&pool)
            .await
            .unwrap_err()
            .into_response()
            .status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    sqlx::query("UPDATE runtime_projection_status SET state='rebuilding',current_cursor=9,active_generation=1,building_generation=2 WHERE projection_name='runtime_governance_v1' AND partition_key='global'")
        .execute(&pool)
        .await
        .unwrap();
    let view = crate::governance_api::projection(&pool).await.unwrap();
    assert_eq!(view.generation, 1);
    let headers = crate::governance_api::projection_headers(&view).unwrap();
    assert_eq!(headers["x-agentx-projection-state"], "rebuilding");
    assert_eq!(headers["x-agentx-projection-cursor"], "9");

    let tenant_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let task_id = Uuid::now_v7();
    let approval = RuntimeEventPayloadV1::ApprovalChanged {
        task_id,
        task_version: 2,
        execution_id: Uuid::now_v7(),
        workflow_id: Uuid::now_v7(),
        node_id: "approval-node".into(),
        title: "Review".into(),
        description: None,
        request: Some(json!({"amount":42})),
        buttons: vec![],
        status: "claimed".into(),
        resume_status: "not_requested".into(),
        claimed_by: Some(user_id),
        deadline_at: None,
        decision: None,
        candidates: vec![RuntimeApprovalCandidateV1 {
            candidate_type: RuntimeApprovalCandidateKindV1::User,
            candidate_id: user_id,
        }],
    };
    let approval_event = event(1, tenant_id, 2, "approval.changed", approval);
    apply(&projector, &approval_event).await.unwrap();
    apply(&projector, &approval_event).await.unwrap();
    let projected = sqlx::query("SELECT status,version,projection_deleted FROM approval_task_projection WHERE tenant_id=? AND id=?")
        .bind(tenant_id)
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(projected.try_get::<String, _>("status").unwrap(), "claimed");
    assert_eq!(projected.try_get::<u64, _>("version").unwrap(), 2);
    assert!(!projected.try_get::<bool, _>("projection_deleted").unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM approval_candidate_projection WHERE tenant_id=? AND approval_task_id=? AND candidate_id=? AND projection_generation=1")
            .bind(tenant_id).bind(task_id).bind(user_id).fetch_one(&pool).await.unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM projection_receipts WHERE projector_name='runtime_governance_v1' AND event_id=?")
            .bind(approval_event.event_id).fetch_one(&pool).await.unwrap(),
        1
    );

    let old = event(
        2,
        tenant_id,
        1,
        "approval.changed",
        RuntimeEventPayloadV1::ApprovalChanged {
            task_id,
            task_version: 1,
            execution_id: Uuid::now_v7(),
            workflow_id: Uuid::now_v7(),
            node_id: "old-node".into(),
            title: "Old".into(),
            description: None,
            request: None,
            buttons: vec![],
            status: "pending".into(),
            resume_status: "not_requested".into(),
            claimed_by: None,
            deadline_at: None,
            decision: None,
            candidates: vec![],
        },
    );
    apply(&projector, &old).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM approval_task_projection WHERE tenant_id=? AND id=?"
        )
        .bind(tenant_id)
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "claimed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT outcome FROM projection_receipts WHERE projector_name='runtime_governance_v1' AND event_id=?")
            .bind(old.event_id).fetch_one(&pool).await.unwrap(),
        "ignored_old_version"
    );
    let mut conflict = approval_event.clone();
    conflict.content_hash = hash('b');
    assert!(
        apply(&projector, &conflict)
            .await
            .unwrap_err()
            .to_string()
            .contains("PROJECTION_EVENT_HASH_CONFLICT")
    );

    let deleted = event(
        3,
        tenant_id,
        3,
        "approval.deleted",
        RuntimeEventPayloadV1::ApprovalChanged {
            task_id,
            task_version: 3,
            execution_id: Uuid::now_v7(),
            workflow_id: Uuid::now_v7(),
            node_id: "approval-node".into(),
            title: "Deleted".into(),
            description: None,
            request: None,
            buttons: vec![],
            status: "cancelled".into(),
            resume_status: "not_requested".into(),
            claimed_by: None,
            deadline_at: None,
            decision: None,
            candidates: vec![],
        },
    );
    apply(&projector, &deleted).await.unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT projection_deleted FROM approval_task_projection WHERE tenant_id=? AND id=?"
        )
        .bind(tenant_id)
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap()
    );

    project_evaluation_retention_and_notification(&projector, tenant_id, user_id).await;
}

async fn project_evaluation_retention_and_notification(
    projector: &Projector,
    tenant_id: Uuid,
    user_id: Uuid,
) {
    let evaluation_id = Uuid::now_v7();
    let package_id = Uuid::now_v7();
    sqlx::query("INSERT INTO evaluation_runs(id,tenant_id,name,workflow_version_id,dataset_version_id,evaluation_profile_version_id,work_package_id,intent_version,parameters_json,status,created_by,owner_department_id) VALUES(?,?,?,?,?,?,?,1,JSON_OBJECT(),'created',?,?)")
        .bind(evaluation_id).bind(tenant_id).bind("Projection test").bind(Uuid::now_v7()).bind(Uuid::now_v7()).bind(Uuid::now_v7()).bind(package_id).bind(user_id).bind(Uuid::now_v7()).execute(&projector.pool).await.unwrap();
    let case_id = Uuid::now_v7();
    let judge_execution_id = Uuid::now_v7();
    let report = RuntimeEvaluationReportV1 {
        cases: vec![RuntimeEvaluationCaseResultV1 {
            id: case_id,
            source_case_id: Uuid::now_v7(),
            target_command_id: Uuid::now_v7(),
            target_execution_id: Some(Uuid::now_v7()),
            status: "completed".into(),
            actual_output: Some(json!({"answer":"ok"})),
            duration_ms: Some(12),
            cost_micros: 7,
            error_code: None,
            error_message: None,
            rules: vec![RuntimeEvaluationRuleResultV1 {
                id: Uuid::now_v7(),
                profile_rule_id: Uuid::now_v7(),
                evaluator_execution_id: Some(judge_execution_id),
                status: "passed".into(),
                passed: Some(true),
                score: Some(1.0),
                detail: json!({"matched":true}),
                duration_ms: Some(2),
                cost_micros: 1,
            }],
        }],
        metrics: RuntimeEvaluationMetricsV1 {
            total_cases: 1,
            completed_cases: 1,
            passed_rules: 1,
            failed_rules: 0,
            error_rules: 0,
            total_cost_micros: 7,
        },
    };
    apply(
        projector,
        &event(
            4,
            tenant_id,
            1,
            "evaluation.changed",
            RuntimeEventPayloadV1::EvaluationChanged {
                run_id: Uuid::now_v7(),
                run_version: 1,
                work_package_id: package_id,
                status: "completed".into(),
                completed_cases: 1,
                total_cases: 1,
                report: Some(report),
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT evaluator_execution_id FROM evaluation_rule_results WHERE tenant_id=? AND evaluation_run_case_id=?")
            .bind(tenant_id).bind(case_id).fetch_one(&projector.pool).await.unwrap(),
        Some(judge_execution_id)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM evaluation_case_projection WHERE tenant_id=? AND evaluation_run_id=? AND projection_generation=1")
            .bind(tenant_id).bind(evaluation_id).fetch_one(&projector.pool).await.unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM evaluation_rule_results WHERE tenant_id=? AND evaluation_run_case_id=? AND projection_generation=1")
            .bind(tenant_id).bind(case_id).fetch_one(&projector.pool).await.unwrap(),
        1
    );

    let control_retention_id = Uuid::now_v7();
    let runtime_retention_id = Uuid::now_v7();
    sqlx::query("INSERT INTO retention_runs(id,tenant_id,dry_run,runtime_command_id,policy_version,idempotency_key,status,requested_by) VALUES(?,?,TRUE,?,1,?,'queued',?)")
        .bind(control_retention_id).bind(tenant_id).bind(runtime_retention_id).bind(format!("retention:{runtime_retention_id}")).bind(user_id).execute(&projector.pool).await.unwrap();
    let retention_item_id = Uuid::now_v7();
    apply(
        projector,
        &event(
            5,
            tenant_id,
            1,
            "retention.changed",
            RuntimeEventPayloadV1::RetentionChanged {
                run_id: runtime_retention_id,
                run_version: 1,
                status: "completed".into(),
                marked_count: 1,
                deleted_count: 0,
                failed_count: 0,
                dry_run: true,
                items: vec![RuntimeRetentionItemV1 {
                    id: retention_item_id,
                    data_type: "artifact".into(),
                    target_id: Uuid::now_v7().to_string(),
                    status: "blocked".into(),
                    reason: Some("dry_run".into()),
                    attempt_count: 0,
                }],
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT retention_run_id FROM retention_items WHERE tenant_id=? AND id=?"
        )
        .bind(tenant_id)
        .bind(retention_item_id)
        .fetch_one(&projector.pool)
        .await
        .unwrap(),
        control_retention_id
    );

    let notification_id = Uuid::now_v7();
    apply(
        projector,
        &event(
            6,
            tenant_id,
            1,
            "notification.changed",
            RuntimeEventPayloadV1::NotificationChanged {
                notification_id,
                notification_version: 1,
                notification_type: "approval_ready".into(),
                title_key: "notifications.approval.title".into(),
                body_key: "notifications.approval.body".into(),
                arguments: json!({"taskId":Uuid::now_v7()}),
                target_type: "user".into(),
                target_id: user_id,
                target_path: "/approvals".into(),
                tone: "primary".into(),
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notification_receipts WHERE tenant_id=? AND notification_id=? AND user_id=?")
            .bind(tenant_id).bind(notification_id).bind(user_id).fetch_one(&projector.pool).await.unwrap(),
        1
    );
}

async fn apply(
    projector: &Projector,
    event: &RuntimeIntegrationEventEnvelopeV1,
) -> anyhow::Result<()> {
    let mut tx = projector.pool.begin().await?;
    projector.apply_event(&mut tx, event).await?;
    tx.commit().await?;
    Ok(())
}

fn event(
    cursor: u64,
    tenant_id: Uuid,
    aggregate_version: u64,
    event_type: &str,
    payload: RuntimeEventPayloadV1,
) -> RuntimeIntegrationEventEnvelopeV1 {
    RuntimeIntegrationEventEnvelopeV1 {
        schema_version: 1,
        cursor,
        event_id: Uuid::now_v7(),
        source_outbox_id: Uuid::now_v7(),
        tenant_id,
        aggregate_type: event_type.split('.').next().unwrap_or("governance").into(),
        aggregate_id: Uuid::now_v7().to_string(),
        aggregate_version,
        event_type: event_type.into(),
        occurred_at: OffsetDateTime::now_utc(),
        payload,
        content_hash: hash('a'),
        correlation_id: Uuid::now_v7(),
        causation_id: None,
    }
}

fn hash(character: char) -> ContentHash {
    ContentHash::parse(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

async fn connect_with_retry(port: u16) -> sqlx::MySqlPool {
    let url = format!("mysql://agentx:agentx-test-password@127.0.0.1:{port}/agentx_control");
    let mut last_error = None;
    for _ in 0..40 {
        match MySqlPoolOptions::new()
            .max_connections(10)
            .connect(&url)
            .await
        {
            Ok(pool) => return pool,
            Err(error) => last_error = Some(error),
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("Control MySQL did not become ready: {last_error:?}");
}
