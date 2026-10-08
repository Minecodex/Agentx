#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn v2_publish_execution_query_recovery_and_gc_are_fenced_and_idempotent() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::ERROR)
        .try_init();
    let container = GenericImage::new("mysql", "8.4")
        .with_exposed_port(3306.tcp())
        .with_wait_for(WaitFor::message_on_stdout(
            "MySQL init process done. Ready for start up.",
        ))
        .with_wait_for(WaitFor::message_on_stderr("ready for connections"))
        .with_env_var("MYSQL_DATABASE", "agentx_runtime")
        .with_env_var("MYSQL_USER", "agentx")
        .with_env_var("MYSQL_PASSWORD", "agentx-test-password")
        .with_env_var("MYSQL_ROOT_PASSWORD", "agentx-root-password")
        .start()
        .await
        .expect("Runtime MySQL container should start");
    let port = container.get_host_port_ipv4(3306.tcp()).await.unwrap();
    let pool = connect_with_retry(port).await;
    let observer = sqlx::mysql::MySqlPoolOptions::new().max_connections(1)
        .connect(&format!("mysql://root:agentx-root-password@127.0.0.1:{port}/agentx_runtime"))
        .await.unwrap();
    agentx_runtime_infrastructure::migrate_runtime_mysql(&pool)
        .await
        .unwrap();
    trace_watermarks_are_atomic_under_concurrency(&pool).await;
    trace_batches_are_disjoint_fenced_and_published(&pool).await;
    composite_timeout_commands_are_idempotent(&pool).await;
    quota_projection_claim_is_single_owner(&pool).await;
    trigger_claim_takeover_is_fenced(&pool).await;

    let fixture = Fixture::new(pool, observer);
    command_claim_returns_only_the_current_batch(&fixture).await;
    authentication_failures_do_not_write_receipts(&fixture).await;
    let first = fixture.bundle(1).await;
    object_upload_is_immutable_and_replayable(&fixture, &first).await;
    prepare_is_idempotent_and_does_not_route_traffic(&fixture, &first).await;
    apply_initial_admission(&fixture, 1).await;
    concurrent_admission_delivery_converges_to_one_receipt(&fixture).await;
    activate(&fixture, &first, None, 1, 1).await;
    concurrent_quota_reservations_are_independent_and_enforce_the_limit(&fixture).await;
    chat_mapping_publication_is_idempotent(&fixture, &first).await;
    authentication_requires_active_route_tenant_and_head(&fixture).await;
    let before_invalid: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM application_invocations WHERE tenant_id=?")
            .bind(fixture.tenant_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert!(matches!(
        create_invocation(
            &fixture.state.pool,
            fixture.tenant_id,
            fixture.application_id,
            fixture.key_id,
            &InvocationRequestV1 {
                input: json!({"message":"x"}),
                idempotency_key: "runtime-slice-invalid-input".into(),
            },
        )
        .await,
        Err(RuntimeError::InvalidRequest(
            "INPUT_SCHEMA_VALIDATION_FAILED",
            _
        ))
    ));
    let after_invalid: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM application_invocations WHERE tenant_id=?")
            .bind(fixture.tenant_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(
        before_invalid, after_invalid,
        "invalid input must not create Runtime facts"
    );

    let accepted = create_invocation(
        &fixture.state.pool,
        fixture.tenant_id,
        fixture.application_id,
        fixture.key_id,
        &InvocationRequestV1 {
            input: json!({"message":"agentx-v2"}),
            idempotency_key: "runtime-slice-invocation".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(accepted.bundle_id, first.payload.bundle_id);
    assert_eq!(accepted.admission_epoch, 1);
    let execution_context: Value = sqlx::query_scalar(
        "SELECT execution_context_json FROM execution_snapshots WHERE tenant_id=? AND execution_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(accepted.execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(execution_context["id"], json!(accepted.execution_id));
    assert_eq!(
        execution_context["workflow"]["name"],
        "Runtime slice Workflow"
    );
    assert_eq!(execution_context["trigger"]["type"], "api_key");
    assert_eq!(
        execution_context["application"]["id"],
        json!(fixture.application_id)
    );
    assert!(execution_context["initiator"].get("user").is_none());
    assert!(execution_context.get("node").is_none());
    execution_context_projection_changes_only_affect_future_executions(&fixture).await;

    invocation_and_dispatch_recovery_are_fenced(&fixture, accepted.execution_id).await;
    retry_policy_creates_a_second_attempt_and_trace(&fixture).await;
    session_message_appends_one_assistant_response(&fixture).await;
    fork_uses_checkpoint_machine_and_preserves_source(&fixture, accepted.execution_id).await;
    query_is_tenant_application_and_execution_scoped(&fixture, accepted.execution_id).await;
    deterministic_start_rejection_is_terminal(&fixture).await;
    revoked_grant_rejection_is_terminal_and_monotonic(&fixture).await;
    expired_attempt_deadline_is_terminal_and_not_requeued(&fixture).await;

    let second = fixture.bundle(2).await;
    fixture
        .upload_bundle_object(&second, "bundle-object:second")
        .await;
    prepare(&fixture, &second, "prepare:second").await;
    let before_activation = create_invocation(
        &fixture.state.pool,
        fixture.tenant_id,
        fixture.application_id,
        fixture.key_id,
        &InvocationRequestV1 {
            input: json!({"message":"still-first"}),
            idempotency_key: "prepare-only-cannot-route".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(before_activation.bundle_id, first.payload.bundle_id);

    activate(&fixture, &second, Some(1), 2, 1).await;
    stale_head_and_sequence_are_rejected(&fixture, &first).await;
    rollback(&fixture, &first, Some(2), 3, 1).await;

    apply_api_key(&fixture, 2, AdmissionStatusV1::Revoked).await;
    assert!(matches!(
        authenticate_api_key(&fixture.state.pool, "runtime-slice", &fixture.api_key).await,
        Err(RuntimeError::Unauthorized)
    ));
    let rejected = rollback_request(&fixture, &second, Some(3), 4, 1, "rollback:revoked").await;
    assert_eq!(rejected.status, PublishReceiptStatusV1::Rejected);
    assert!(matches!(
        rejected.rejection.unwrap().code,
        agentx_runtime_contracts::RuntimePublishErrorCodeV1::AdmissionPrerequisiteMissing
    ));
    let key_status: String =
        sqlx::query_scalar("SELECT status FROM api_key_admission WHERE tenant_id=? AND key_id=?")
            .bind(fixture.tenant_id)
            .bind(fixture.key_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(key_status, "revoked");

    gc_protects_heads_references_and_holds_then_sweeps(&fixture, &second).await;
    gc_object_delete_failure_is_recorded_and_retryable(&fixture).await;
    ready_orphan_objects_are_swept_and_reuploadable(&fixture).await;
    expired_temporary_objects_are_removed(&fixture).await;
    disable_is_scoped_idempotent_and_preserves_the_head(&fixture, &first).await;
    work_package_prepare_execute_cancel_are_independently_signed_and_idempotent(&fixture).await;
    completed_history_does_not_block_the_next_node(&fixture).await;
    evaluation_work_package_creates_cases_converges_and_cancels_atomically(&fixture).await;
    wait_and_approval_resume_exactly_once(&fixture).await;
    large_worker_results_are_externalized_and_verified(&fixture).await;
    composite_child_uses_immutable_runtime_snapshot_and_merges_on_success(&fixture).await;
    sandbox_manager_is_fenced_and_idempotent(&fixture).await;
    agent_worker_runs_a_bounded_tool_loop_and_persists_usage(&fixture).await;
    agent_attachment_revocation_is_tool_scoped(&fixture).await;
    application_session_agent_restores_context_across_executions(&fixture).await;
    durable_agent_pending_wakeups_are_ordered_and_idempotent(&fixture).await;
    quota_projection_covers_all_dimensions_and_has_no_terminal_residue(&fixture).await;
    retention_dry_run_reference_block_and_object_sweep_are_fenced(&fixture).await;
    event_sequencer_quarantines_invalid_payload_without_blocking_valid_events(&fixture).await;
}

async fn execution_context_projection_changes_only_affect_future_executions(fixture: &Fixture) {
    let user_id = Uuid::now_v7();
    let first_department_id = Uuid::now_v7();
    let second_department_id = Uuid::now_v7();
    let role_id = Uuid::now_v7();
    let mut tx = fixture.state.pool.begin().await.unwrap();
    let first_roles = json!([{
        "id": role_id,
        "code": "workflow_operator",
        "name": "Workflow Operator",
        "dataScope": "department_tree",
        "scopeDepartment": { "id": first_department_id, "name": "Platform" }
    }]);
    sqlx::query("INSERT INTO runtime_user_admission(tenant_id,user_id,user_name,department_id,department_name,token_version,status,tenant_query_enabled,role_assignments_json,admission_epoch) VALUES(?,?,'Historical Operator',?,'Platform',1,'active',FALSE,?,1)")
        .bind(fixture.tenant_id)
        .bind(user_id)
        .bind(first_department_id)
        .bind(first_roles)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtime_user_application_grants(tenant_id,user_id,application_id,grant_version,status,can_invoke,can_query,admission_epoch) VALUES(?,?,?,1,'active',TRUE,FALSE,1)")
        .bind(fixture.tenant_id)
        .bind(user_id)
        .bind(fixture.application_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let first = create_runtime_invocation_tx(
        &mut tx,
        fixture.tenant_id,
        fixture.application_id,
        InvocationCaller {
            caller_type: "user",
            caller_id: user_id,
            token_version: Some(1),
            origin: agentx_runtime_contracts::ExecutionOriginV1::system(None),
        },
        None,
        &json!({"message":"first-user-snapshot"}),
        "execution-context:user:first",
    )
    .await
    .unwrap();
    let first_context: Value = sqlx::query_scalar(
        "SELECT execution_context_json FROM execution_snapshots WHERE execution_id=?",
    )
    .bind(first.execution_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let first_authorization: Value = sqlx::query_scalar(
        "SELECT authorization_snapshot_json FROM execution_snapshots WHERE execution_id=?",
    )
    .bind(first.execution_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();

    let second_roles = json!([{
        "id": role_id,
        "code": "workflow_operator",
        "name": "Renamed Workflow Operator",
        "dataScope": "department_tree",
        "scopeDepartment": { "id": second_department_id, "name": "Engineering" }
    }]);
    sqlx::query("UPDATE runtime_user_admission SET user_name='Current Operator',department_id=?,department_name='Engineering',token_version=2,role_assignments_json=?,admission_epoch=2 WHERE tenant_id=? AND user_id=?")
        .bind(second_department_id)
        .bind(second_roles)
        .bind(fixture.tenant_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let second = create_runtime_invocation_tx(
        &mut tx,
        fixture.tenant_id,
        fixture.application_id,
        InvocationCaller {
            caller_type: "user",
            caller_id: user_id,
            token_version: Some(2),
            origin: agentx_runtime_contracts::ExecutionOriginV1::system(None),
        },
        None,
        &json!({"message":"second-user-snapshot"}),
        "execution-context:user:second",
    )
    .await
    .unwrap();
    let second_context: Value = sqlx::query_scalar(
        "SELECT execution_context_json FROM execution_snapshots WHERE execution_id=?",
    )
    .bind(second.execution_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let second_authorization: Value = sqlx::query_scalar(
        "SELECT authorization_snapshot_json FROM execution_snapshots WHERE execution_id=?",
    )
    .bind(second.execution_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        first_context.pointer("/initiator/user/name"),
        Some(&json!("Historical Operator"))
    );
    assert_eq!(
        first_context.pointer("/initiator/department/name"),
        Some(&json!("Platform"))
    );
    assert_eq!(
        first_context.pointer("/initiator/roles/names/0"),
        Some(&json!("Workflow Operator"))
    );
    assert_eq!(
        second_context.pointer("/initiator/user/name"),
        Some(&json!("Current Operator"))
    );
    assert_eq!(
        second_context.pointer("/initiator/department/name"),
        Some(&json!("Engineering"))
    );
    assert_eq!(
        second_context.pointer("/initiator/roles/names/0"),
        Some(&json!("Renamed Workflow Operator"))
    );
    assert_eq!(
        first_context.pointer("/initiator/department/name"),
        Some(&json!("Platform")),
        "historical snapshots must remain immutable"
    );
    assert_eq!(
        first_authorization, second_authorization,
        "role variables must not alter Workflow Service Identity authorization"
    );
    assert_eq!(
        second_authorization["serviceIdentityId"],
        json!(fixture.identity_id)
    );

    for trigger_type in ["webhook", "schedule"] {
        let source_id = Uuid::now_v7();
        let accepted = create_runtime_invocation_tx(
            &mut tx,
            fixture.tenant_id,
            fixture.application_id,
            InvocationCaller {
                caller_type: trigger_type,
                caller_id: source_id,
                token_version: None,
                origin: agentx_runtime_contracts::ExecutionOriginV1 {
                    trigger_source_id: Some(source_id),
                    trigger_name: Some(format!("{trigger_type} fixture")),
                    ..agentx_runtime_contracts::ExecutionOriginV1::system(None)
                },
            },
            None,
            &json!({"message":format!("{trigger_type}-snapshot")}),
            &format!("execution-context:{trigger_type}"),
        )
        .await
        .unwrap();
        let context: Value = sqlx::query_scalar(
            "SELECT execution_context_json FROM execution_snapshots WHERE execution_id=?",
        )
        .bind(accepted.execution_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(context["trigger"]["type"], trigger_type);
        assert_eq!(context["trigger"]["sourceId"], json!(source_id));
        assert!(context["initiator"].get("user").is_none());
        assert!(context["initiator"].get("department").is_none());
        assert!(context["initiator"].get("roles").is_none());
    }
    tx.rollback().await.unwrap();
}

async fn trace_watermarks_are_atomic_under_concurrency(pool: &MySqlPool) {
    const EVENT_COUNT: u64 = 12;
    let tenant_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let execution_id = Uuid::now_v7();
    sqlx::query("INSERT INTO workflow_executions(id,tenant_id,workflow_id,workflow_version_id,trace_id,trigger_type,status,started_at) VALUES(?,?,?,?,?,'debug','running',UTC_TIMESTAMP(6))")
        .bind(execution_id)
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(version_id)
        .bind(Uuid::now_v7())
        .execute(pool)
        .await
        .unwrap();

    let mut held_execution = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM workflow_executions WHERE id=? FOR UPDATE")
        .bind(execution_id).fetch_one(&mut *held_execution).await.unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..EVENT_COUNT {
        let pool = pool.clone();
        tasks.spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            let mut draft = agentx_v2_runtime::trace_delivery::TraceDraft::execution(
                tenant_id,
                execution_id,
                format!("execution.concurrent_{index}"),
                "running",
            );
            draft.attributes = json!({"index":index});
            let sequence = agentx_v2_runtime::trace_delivery::enqueue(&mut tx, draft)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            sequence
        });
    }
    let mut sequences = Vec::with_capacity(EVENT_COUNT as usize);
    tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(result) = tasks.join_next().await {
            sequences.push(result.unwrap());
        }
    }).await.expect("Trace allocation must not wait for a locked Execution authority row");
    held_execution.rollback().await.unwrap();
    sequences.sort_unstable();
    assert_eq!(sequences.len(), EVENT_COUNT as usize);
    assert!(sequences[0] > 0);
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    let (watermark, count): (u64, u64) =
        sqlx::query_as("SELECT CAST(COALESCE(MAX(sequence_number),0) AS UNSIGNED),CAST(COUNT(*) AS UNSIGNED) FROM execution_events WHERE tenant_id=? AND execution_id=?")
            .bind(tenant_id).bind(execution_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(watermark, *sequences.last().unwrap());
    assert_eq!(count, EVENT_COUNT);
    let mut rollback = pool.begin().await.unwrap();
    agentx_v2_runtime::trace_delivery::enqueue(&mut rollback,
        agentx_v2_runtime::trace_delivery::TraceDraft::execution(tenant_id,execution_id,"execution.rollback","running"))
        .await.unwrap();
    rollback.rollback().await.unwrap();
    let after_rollback: (u64, u64) = sqlx::query_as("SELECT CAST(COALESCE(MAX(sequence_number),0) AS UNSIGNED),CAST(COUNT(*) AS UNSIGNED) FROM execution_events WHERE tenant_id=? AND execution_id=?")
        .bind(tenant_id).bind(execution_id).fetch_one(pool).await.unwrap();
    assert_eq!(after_rollback, (watermark, count));
    let persisted: Vec<u64> = sqlx::query_scalar(
        "SELECT execution_sequence FROM trace_outbox WHERE tenant_id=? AND execution_id=? ORDER BY execution_sequence",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(persisted, sequences);

    sqlx::query("DELETE FROM trace_outbox WHERE tenant_id=? AND execution_id=?")
        .bind(tenant_id)
        .bind(execution_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM execution_events WHERE tenant_id=? AND execution_id=?")
        .bind(tenant_id)
        .bind(execution_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM workflow_executions WHERE tenant_id=? AND id=?")
        .bind(tenant_id)
        .bind(execution_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn composite_timeout_commands_are_idempotent(pool: &MySqlPool) {
    let tenant_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();
    let bundle_id = Uuid::now_v7();
    let parent_execution_id = Uuid::now_v7();
    let child_execution_id = Uuid::now_v7();
    let parent_node_execution_id = Uuid::now_v7();
    for (execution_id, trace_id, status, trigger_type) in [
        (
            parent_execution_id,
            Uuid::now_v7(),
            "waiting",
            "application",
        ),
        (child_execution_id, Uuid::now_v7(), "running", "composite"),
    ] {
        sqlx::query("INSERT INTO workflow_executions(id,tenant_id,workflow_id,workflow_version_id,bundle_id,admission_epoch,state_version,trace_id,trigger_type,status,started_at) VALUES(?,?,?,?,?,1,1,?,?,?,UTC_TIMESTAMP(6))")
            .bind(execution_id)
            .bind(tenant_id)
            .bind(workflow_id)
            .bind(version_id)
            .bind(bundle_id)
            .bind(trace_id)
            .bind(trigger_type)
            .bind(status)
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO execution_children(tenant_id,parent_execution_id,parent_node_execution_id,child_execution_id,child_bundle_id,relationship,context_overlay_json,context_overlay_hash,deadline_at) VALUES(?,?,?,?,?,'composite',JSON_OBJECT(),'sha256:0000000000000000000000000000000000000000000000000000000000000000',DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND))")
        .bind(tenant_id)
        .bind(parent_execution_id)
        .bind(parent_node_execution_id)
        .bind(child_execution_id)
        .bind(bundle_id)
        .execute(pool)
        .await
        .unwrap();

    assert_eq!(
        agentx_v2_runtime::composite_execution::enqueue_overdue(pool, 100)
            .await
            .unwrap(),
        1
    );
    agentx_v2_runtime::composite_execution::enqueue_overdue(pool, 100)
        .await
        .unwrap();
    let commands: Vec<String> = sqlx::query_scalar(
        "SELECT command_type FROM runtime_commands WHERE tenant_id=? ORDER BY command_type",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(commands, vec!["cancel_execution", "resume_execution"]);

    sqlx::query("DELETE FROM runtime_commands WHERE tenant_id=?")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM execution_children WHERE tenant_id=?")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM workflow_executions WHERE tenant_id=?")
        .bind(tenant_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn event_sequencer_quarantines_invalid_payload_without_blocking_valid_events(
    fixture: &Fixture,
) {
    let owner = Uuid::now_v7();
    for _ in 0..10_000 {
        match agentx_v2_runtime::event_export::sequence_one(&fixture.state.pool, owner).await {
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(error) => panic!("a Runtime-produced Integration Event was invalid: {error}"),
        }
    }
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_outbox WHERE status='pending' AND message_type='runtime_event'",
    )
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(
        remaining, 0,
        "the Event Sequencer did not drain its valid backlog"
    );

    let locked_execution_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow_executions WHERE application_id IS NOT NULL AND bundle_id IS NOT NULL ORDER BY created_at,id LIMIT 1",
    )
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let raced_event_id = Uuid::now_v7();
    sqlx::query("INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,payload_json,status) VALUES(?,?,?,'runtime_event',?,'pending')")
        .bind(raced_event_id)
        .bind(fixture.tenant_id)
        .bind(locked_execution_id)
        .bind(json!({"type":"invocation.accepted","commandId":Uuid::now_v7()}))
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    let mut execution_lock = fixture.state.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM workflow_executions WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(fixture.tenant_id)
        .bind(locked_execution_id)
        .fetch_one(&mut *execution_lock)
        .await
        .unwrap();
    let raced = tokio::time::timeout(
        Duration::from_secs(2),
        agentx_v2_runtime::event_export::sequence_one(&fixture.state.pool, owner),
    )
    .await
    .expect("Event Sequencing must not wait for the Execution row lock")
    .expect("the locked Execution must remain readable from its committed snapshot");
    assert!(raced.is_some());
    execution_lock.rollback().await.unwrap();
    let raced_status: String = sqlx::query_scalar("SELECT status FROM execution_outbox WHERE id=?")
        .bind(raced_event_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(raced_status, "published");

    let invalid_id = Uuid::now_v7();
    sqlx::query("INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,payload_json,status) VALUES(?,?,NULL,'runtime_event',?,'pending')")
        .bind(invalid_id)
        .bind(fixture.tenant_id)
        .bind(json!({"kind":"retention_changed","unknownField":true}))
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    assert!(
        agentx_v2_runtime::event_export::sequence_one(&fixture.state.pool, owner)
            .await
            .is_err()
    );
    let invalid_status: (String, Option<String>) =
        sqlx::query_as("SELECT status,last_error FROM execution_outbox WHERE id=?")
            .bind(invalid_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(invalid_status.0, "failed");
    assert!(
        invalid_status
            .1
            .as_deref()
            .is_some_and(|error| error.starts_with("EVENT_PAYLOAD_INVALID:"))
    );

    let valid_id = Uuid::now_v7();
    let run_id = Uuid::now_v7();
    let payload = RuntimeEventPayloadV1::RetentionChanged {
        run_id,
        run_version: 1,
        status: "completed".into(),
        marked_count: 0,
        deleted_count: 0,
        failed_count: 0,
        dry_run: true,
        items: vec![],
    };
    sqlx::query("INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,event_type,aggregate_type,aggregate_id,aggregate_version,correlation_id,payload_json,status) VALUES(?,?,NULL,'runtime_event','retention_changed','retention',?,1,?,?, 'pending')")
        .bind(valid_id)
        .bind(fixture.tenant_id)
        .bind(run_id.to_string())
        .bind(run_id)
        .bind(serde_json::to_value(payload).unwrap())
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    assert!(
        agentx_v2_runtime::event_export::sequence_one(&fixture.state.pool, owner)
            .await
            .unwrap()
            .is_some()
    );
    let valid_status: String = sqlx::query_scalar("SELECT status FROM execution_outbox WHERE id=?")
        .bind(valid_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(valid_status, "published");
}

async fn quota_projection_claim_is_single_owner(pool: &MySqlPool) {
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            agentx_v2_runtime::quota::claim_projection(&pool, Uuid::now_v7()).await
        }));
    }
    let mut claims = Vec::new();
    for task in tasks {
        if let Some(claim) = task.await.unwrap().unwrap() {
            claims.push(claim);
        }
    }
    assert_eq!(
        claims.len(),
        1,
        "only one Quota Projection replica may lead"
    );
    let stale = claims[0];
    sqlx::query(
        "UPDATE runtime_role_leases SET locked_until=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE role_key='quota_projection'",
    )
    .execute(pool)
    .await
    .unwrap();
    let replacement = agentx_v2_runtime::quota::claim_projection(pool, Uuid::now_v7())
        .await
        .unwrap()
        .expect("expired Quota Projection Lease must be taken over");
    assert!(replacement.fencing_token > stale.fencing_token);
    assert!(
        agentx_v2_runtime::quota::heartbeat_projection(pool, stale)
            .await
            .is_err(),
        "stale Quota Projection fencing token must be rejected"
    );
}

async fn command_claim_returns_only_the_current_batch(fixture: &Fixture) {
    let command_id = Uuid::now_v7();
    let execution_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'claim_fixture','execution',?,?,JSON_OBJECT(),'pending')",
    )
    .bind(command_id)
    .bind(fixture.tenant_id)
    .bind(execution_id.to_string())
    .bind(format!("claim-fixture:{command_id}"))
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let owner = Uuid::now_v7();
    let first = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap();
    assert!(first.iter().any(|claim| claim.command_id == command_id));
    let second = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap();
    assert!(
        second.iter().all(|claim| claim.command_id != command_id),
        "an in-flight command must not be returned again to the same owner"
    );
    sqlx::query("DELETE FROM runtime_commands WHERE id=?")
        .bind(command_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
}

async fn deterministic_start_rejection_is_terminal(fixture: &Fixture) {
    let accepted = create_invocation(
        &fixture.state.pool,
        fixture.tenant_id,
        fixture.application_id,
        fixture.key_id,
        &InvocationRequestV1 {
            input: json!({"message":"stale-authorization"}),
            idempotency_key: "runtime-slice-stale-authorization".into(),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE service_identity_projection SET updated_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 73 HOUR) WHERE identity_id=?")
        .bind(fixture.identity_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    let owner = Uuid::now_v7();
    let claim = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.execution_id == accepted.execution_id)
        .expect("stale-authorization Start Command must be claimable");
    process_command_with_state(&fixture.state, &claim)
        .await
        .unwrap();
    let facts = sqlx::query(
        "SELECT e.status,CAST(JSON_UNQUOTE(JSON_EXTRACT(e.error_json,'$.code')) AS CHAR) AS error_code,c.status AS command_status,(SELECT COUNT(*) FROM bundle_references r WHERE r.tenant_id=e.tenant_id AND r.reference_kind='active_execution' AND r.owner_id=e.id AND r.released_at IS NULL) AS live_references FROM workflow_executions e JOIN runtime_commands c ON c.id=? WHERE e.id=?",
    )
    .bind(claim.command_id)
    .bind(accepted.execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(facts.get::<String, _>("status"), "failed");
    assert_eq!(
        facts.get::<String, _>("error_code"),
        "RUNTIME_AUTHORIZATION_STALE"
    );
    assert_eq!(facts.get::<String, _>("command_status"), "failed");
    assert_eq!(facts.get::<i64, _>("live_references"), 0);
    sqlx::query(
        "UPDATE service_identity_projection SET updated_at=UTC_TIMESTAMP(6) WHERE identity_id=?",
    )
    .bind(fixture.identity_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
}

async fn revoked_grant_rejection_is_terminal_and_monotonic(fixture: &Fixture) {
    let accepted = create_invocation(
        &fixture.state.pool,
        fixture.tenant_id,
        fixture.application_id,
        fixture.key_id,
        &InvocationRequestV1 {
            input: json!({"message":"revoked-grant"}),
            idempotency_key: "runtime-slice-revoked-grant".into(),
        },
    )
    .await
    .unwrap();
    let grant_id = Uuid::now_v7();
    let resource_id = Uuid::now_v7();
    let mut authorization: RuntimeAuthorizationSnapshotV1 = serde_json::from_value(
        sqlx::query_scalar(
            "SELECT authorization_snapshot_json FROM execution_snapshots WHERE execution_id=?",
        )
        .bind(accepted.execution_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap(),
    )
    .unwrap();
    authorization.grant_ids.push(grant_id);
    sqlx::query(
        "UPDATE execution_snapshots SET authorization_snapshot_json=? WHERE execution_id=?",
    )
    .bind(serde_json::to_value(authorization).unwrap())
    .bind(accepted.execution_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let grant_target = |policy_epoch, enabled| AdmissionTargetV1::ResourceGrant {
        state: RuntimeGrantStateV1 {
            tenant_id: fixture.tenant_id,
            identity_id: fixture.identity_id,
            grant_id,
            resource_type: "model".into(),
            resource_id,
            operations: BTreeSet::from(["use".into()]),
            policy_epoch,
            enabled,
        },
    };
    apply_target(fixture, 2, grant_target(2, false)).await;
    apply_target(fixture, 1, grant_target(1, true)).await;
    let projection: (String, u64, String) = sqlx::query_as(
        "SELECT status,policy_epoch,resource_type FROM resource_grant_projection WHERE tenant_id=? AND grant_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(grant_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(projection, ("revoked".into(), 2, "model".into()));

    let owner = Uuid::now_v7();
    let claim = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.execution_id == accepted.execution_id)
        .expect("revoked Grant Start Command must be claimable");
    process_command_with_state(&fixture.state, &claim)
        .await
        .unwrap();
    let facts: (String, String, String, String) = sqlx::query_as(
        "SELECT e.status,CAST(JSON_UNQUOTE(JSON_EXTRACT(e.error_json,'$.code')) AS CHAR),i.status,c.status FROM workflow_executions e JOIN application_invocations i ON i.execution_id=e.id JOIN runtime_commands c ON c.id=? WHERE e.id=?",
    )
    .bind(claim.command_id)
    .bind(accepted.execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(
        facts,
        (
            "failed".into(),
            "RUNTIME_GRANT_REVOKED".into(),
            "failed".into(),
            "failed".into(),
        )
    );
}

async fn quota_projection_covers_all_dimensions_and_has_no_terminal_residue(fixture: &Fixture) {
    let dimensions = [
        "execution_concurrency",
        "node_concurrency",
        "sandbox_concurrency",
        "agent_iterations",
        "tokens",
        "cost_micros",
        "artifact_bytes",
        "cpu_millis",
        "memory_bytes",
        "pids",
        "disk_bytes",
        "ttl_seconds",
    ];
    for dimension in dimensions {
        sqlx::query(
            "INSERT INTO quota_policy_projection(tenant_id,dimension_key,hard_limit,period_seconds,version,updated_by) VALUES(?,?,1000000000,3600,1,?) ON DUPLICATE KEY UPDATE hard_limit=VALUES(hard_limit),period_seconds=VALUES(period_seconds)",
        )
        .bind(fixture.tenant_id)
        .bind(dimension)
        .bind(Uuid::now_v7())
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    }
    let projection = agentx_v2_runtime::quota::counter_projection(&fixture.state.pool)
        .await
        .unwrap()
        .into_iter()
        .filter(|counter| counter.tenant_id == fixture.tenant_id)
        .collect::<Vec<_>>();
    assert_eq!(projection.len(), dimensions.len());
    assert!(projection.iter().all(|counter| counter.hard_limit > 0));
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM quota_reservations WHERE tenant_id=? AND status='active' AND expires_at>UTC_TIMESTAMP(6)",
    )
    .bind(fixture.tenant_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(active, 0);
}

async fn wait_and_approval_resume_exactly_once(fixture: &Fixture) {
    let approval_execution = start_suspending_work_package(fixture, "approval").await;
    let task: (Uuid, u64, String) = sqlx::query_as(
        "SELECT id,version,title FROM approval_tasks WHERE tenant_id=? AND execution_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(approval_execution)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(task.2, "Runtime slice Workflow");
    let notification_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications WHERE tenant_id=? AND notification_type='approval_reassigned' AND target_type='user' AND target_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.identity_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(
        notification_count, 1,
        "a single-candidate Approval must create one deterministic Runtime notification"
    );
    let unauthorized_claim = admission_request(
        fixture,
        99,
        AdmissionTargetV1::ApprovalAction {
            state: RuntimeApprovalActionV1 {
                task_id: task.0,
                task_version: task.1,
                action: ApprovalActionValueV1::Claim,
                actor_id: Uuid::now_v7(),
                target_user_id: None,
                input: None,
            },
        },
        "approval:unauthorized-claim",
    );
    let unauthorized_receipt = apply_admission(
        State(fixture.state.clone()),
        publisher_headers("runtime.admission.apply"),
        Json(unauthorized_claim),
    )
    .await
    .unwrap()
    .0;
    assert!(!unauthorized_receipt.applied);
    let claim = admission_request(
        fixture,
        100,
        AdmissionTargetV1::ApprovalAction {
            state: RuntimeApprovalActionV1 {
                task_id: task.0,
                task_version: task.1,
                action: ApprovalActionValueV1::Claim,
                actor_id: fixture.identity_id,
                target_user_id: None,
                input: None,
            },
        },
        "approval:claim",
    );
    let _ = apply_admission(
        State(fixture.state.clone()),
        publisher_headers("runtime.admission.apply"),
        Json(claim),
    )
    .await
    .unwrap();
    let invalid_button = admission_request(
        fixture,
        101,
        AdmissionTargetV1::ApprovalAction {
            state: RuntimeApprovalActionV1 {
                task_id: task.0,
                task_version: task.1 + 1,
                action: ApprovalActionValueV1::Decide {
                    decision_id: "missing-button".into(),
                },
                actor_id: fixture.identity_id,
                target_user_id: None,
                input: None,
            },
        },
        "approval:decision:missing-button",
    );
    assert!(
        !apply_admission(
            State(fixture.state.clone()),
            publisher_headers("runtime.admission.apply"),
            Json(invalid_button),
        )
        .await
        .unwrap()
        .0
        .applied
    );
    let approved = admission_request(
        fixture,
        101,
        AdmissionTargetV1::ApprovalAction {
            state: RuntimeApprovalActionV1 {
                task_id: task.0,
                task_version: task.1 + 1,
                action: ApprovalActionValueV1::Decide {
                    decision_id: "approved".into(),
                },
                actor_id: fixture.identity_id,
                target_user_id: None,
                input: Some(json!({"reason":"approved by runtime slice"})),
            },
        },
        "approval:decision:approved",
    );
    let rejected = admission_request(
        fixture,
        101,
        AdmissionTargetV1::ApprovalAction {
            state: RuntimeApprovalActionV1 {
                task_id: task.0,
                task_version: task.1 + 1,
                action: ApprovalActionValueV1::Decide {
                    decision_id: "rejected".into(),
                },
                actor_id: fixture.identity_id,
                target_user_id: None,
                input: Some(json!({"reason":"concurrent rejection"})),
            },
        },
        "approval:decision:rejected",
    );
    let (approved_result, rejected_result) = tokio::join!(
        apply_admission(
            State(fixture.state.clone()),
            publisher_headers("runtime.admission.apply"),
            Json(approved.clone())
        ),
        apply_admission(
            State(fixture.state.clone()),
            publisher_headers("runtime.admission.apply"),
            Json(rejected.clone())
        )
    );
    let approved_receipt = approved_result.unwrap().0;
    let rejected_receipt = rejected_result.unwrap().0;
    assert_ne!(approved_receipt.applied, rejected_receipt.applied);
    let winner = if approved_receipt.applied {
        approved
    } else {
        rejected
    };
    let replay = apply_admission(
        State(fixture.state.clone()),
        publisher_headers("runtime.admission.apply"),
        Json(winner),
    )
    .await
    .unwrap()
    .0;
    assert!(replay.replayed);
    let decision_commands: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_commands WHERE tenant_id=? AND aggregate_id=? AND command_type='resume_execution'",
    )
    .bind(fixture.tenant_id)
    .bind(approval_execution.to_string())
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(decision_commands, 1);
    process_execution_commands(fixture, approval_execution).await;
    let approval_state: (String, String, String, Option<String>, u64) = sqlx::query_as(
        "SELECT e.status,a.status,a.resume_status,a.decision_id,a.version FROM workflow_executions e JOIN approval_tasks a ON a.execution_id=e.id AND a.tenant_id=e.tenant_id WHERE e.tenant_id=? AND e.id=?",
    )
    .bind(fixture.tenant_id)
    .bind(approval_execution)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(approval_state.0, "succeeded");
    assert_eq!(approval_state.1, "decided");
    assert_eq!(approval_state.2, "succeeded");
    assert!(matches!(
        approval_state.3.as_deref(),
        Some("approved" | "rejected")
    ));
    // Claim, decision acceptance, and resume completion are three distinct
    // authoritative transitions after the initial task version.
    assert_eq!(approval_state.4, 4);

    let timeout_execution = start_suspending_work_package_with_parameters(
        fixture,
        "approval",
        json!({
            "title":{"kind":"template","segments":[{"kind":"text","text":"Timeout approval"}]},
            "timeoutMs":1,
            "candidateUserId":fixture.identity_id
        }),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(
        agentx_v2_runtime::enqueue_due_approval_timeouts(&fixture.state.pool, Uuid::now_v7(), 100)
            .await
            .unwrap(),
        1
    );
    process_execution_commands(fixture, timeout_execution).await;
    let timeout_state: (String, String, String) = sqlx::query_as(
        "SELECT e.status,a.status,a.resume_status FROM workflow_executions e JOIN approval_tasks a ON a.tenant_id=e.tenant_id AND a.execution_id=e.id WHERE e.tenant_id=? AND e.id=?",
    )
    .bind(fixture.tenant_id)
    .bind(timeout_execution)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(
        timeout_state,
        ("succeeded".into(), "timed_out".into(), "succeeded".into())
    );
}

async fn start_suspending_work_package(fixture: &Fixture, node_type: &str) -> Uuid {
    assert_eq!(
        node_type, "approval",
        "approval is the only suspending builtin"
    );
    let parameters = json!({
        "title":{"kind":"template","segments":[{
            "kind":"reference","selector":{
                "namespace":"execution",
                "run":{"kind":"current"},
                "item":{"kind":"current"},
                "path":["workflow","name"]
            },"missingPolicy":{"kind":"error"}
        }]},
        "timeoutMs":300000,
        "candidateUserId":fixture.identity_id
    });
    start_suspending_work_package_with_parameters(fixture, node_type, parameters).await
}

async fn start_suspending_work_package_with_parameters(
    fixture: &Fixture,
    node_type: &str,
    parameters: Value,
) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let package_id = Uuid::now_v7();
    let mut source = fixture.work_package_source(package_id, now, now + time::Duration::hours(1));
    source.source_revision = format!("{node_type}:1");
    source.definition = suspension_definition(node_type, parameters);
    let compiled = compile_workflow_version(&source.definition, package_id).unwrap();
    source.spec = agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Debug {
        draft_revision: 1,
        debug_plan: agentx_runtime_contracts::RuntimeDebugPlanV1::whole(&compiled),
    };
    let package = build_work_package(
        source,
        "work-package-current",
        &fixture.work_package_signing_key,
    )
    .unwrap();
    let _ = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(PrepareWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("{node_type}:prepare:{package_id}"),
            work_package: package,
        }),
    )
    .await
    .unwrap();
    let started = execute_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(ExecuteWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("{node_type}:execute:{package_id}"),
            package_id,
            input: json!({"message":"suspend"}),
        }),
    )
    .await
    .unwrap()
    .0;
    let execution_id = serde_json::from_value(started.result["executionId"].clone()).unwrap();
    process_execution_commands(fixture, execution_id).await;
    execution_id
}

async fn process_execution_commands(fixture: &Fixture, execution_id: Uuid) {
    let owner = Uuid::now_v7();
    let commands = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap();
    let mut matched = 0;
    for command in commands {
        if command.execution_id == execution_id {
            process_command(&fixture.state.pool, &command)
                .await
                .unwrap();
            matched += 1;
        }
    }
    assert_eq!(
        matched, 1,
        "expected one Runtime Command for {execution_id}"
    );
}

fn suspension_definition(node_type: &str, parameters: Value) -> WorkflowDefinition {
    assert_eq!(
        node_type, "approval",
        "approval is the only suspending builtin"
    );
    let connections = vec![
        json!({"id":"start-suspend","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"suspend","targetHandle":"main","order":0}),
        json!({"id":"approved-end","sourceNodeId":"suspend","sourceHandle":"decision:approved","targetNodeId":"exit","targetHandle":"main","order":0}),
        json!({"id":"rejected-end","sourceNodeId":"suspend","sourceHandle":"decision:rejected","targetNodeId":"exit","targetHandle":"main","order":1}),
        json!({"id":"timed-out-end","sourceNodeId":"suspend","sourceHandle":"timed_out","targetNodeId":"exit","targetHandle":"main","order":2}),
    ];
    serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{"missingCandidate":{"type":"string"}},"additionalProperties":true},"contexts":{}},
        "nodes":[{
            "id":"suspend",
            "key":"suspend",
            "type":node_type,
            "typeVersion":1,
            "name":"Suspend",
            "parameters":parameters,

            "contextWrites":[],
            "resourceReferences":[]
        },{
            "id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","disabled":false,"protected":true,
            "parameters":{"outputs":{},"errorOutputs":{}},"contextWrites":[],
            "resourceReferences":[],"settings":{}
        }],
        "connections":connections,
        "end":{"outputs":{}},
        "settings":{"activationBudget":20,"executionOrder":"deterministic"}
    }))
    .unwrap()
}

async fn large_worker_results_are_externalized_and_verified(fixture: &Fixture) {
    let package_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    let mut source = fixture.work_package_source(package_id, now, now + time::Duration::hours(1));
    // Larger than the bundled MySQL default sort buffer. Candidate discovery
    // must sort identifiers, without copying these JSON values into filesort.
    let message = "v2-large-result".repeat(30_000);
    source.overlay.input = json!({"message":message});
    let package = build_work_package(
        source,
        "work-package-current",
        &fixture.work_package_signing_key,
    )
    .unwrap();
    let _ = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(PrepareWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("large:prepare:{package_id}"),
            work_package: package,
        }),
    )
    .await
    .unwrap();
    let started = execute_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(ExecuteWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("large:execute:{package_id}"),
            package_id,
            input: Value::Null,
        }),
    )
    .await
    .unwrap()
    .0;
    let execution_id: Uuid = serde_json::from_value(started.result["executionId"].clone()).unwrap();
    let owner = Uuid::now_v7();
    let command = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.execution_id == execution_id)
        .unwrap();
    process_command(&fixture.state.pool, &command)
        .await
        .unwrap();
    let dispatch = claim_dispatch(&fixture.state.pool, owner)
        .await
        .unwrap()
        .unwrap();
    let task = dispatch.task().unwrap();
    complete_dispatch(&fixture.state.pool, &dispatch)
        .await
        .unwrap();
    let worker_id = Uuid::now_v7();
    agentx_v2_runtime::engine::register_worker(
        &fixture.state.pool,
        worker_id,
        task.capability.as_str(),
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .unwrap();
    let claim = agentx_v2_runtime::engine::claim_worker_attempt(
        &fixture.state.pool,
        worker_id,
        task.capability.as_str(),
        &task,
    )
    .await
    .unwrap()
    .unwrap();
    let worker = test_worker(fixture, StubWorkerMode::Reject);
    let execution = worker.execute(&claim).await;
    let result = worker.build_result(&claim, execution).await.unwrap();
    let object = result.output_object.clone().unwrap();
    assert!(result.outputs.is_empty());
    agentx_v2_runtime::engine::submit_worker_result_with_objects(
        &fixture.state.pool,
        fixture.state.objects.clone(),
        &result,
    )
    .await
    .unwrap();
    let stored: (Option<Value>, Option<Uuid>) =
        sqlx::query_as("SELECT output_json,result_object_id FROM node_attempts WHERE id=?")
            .bind(result.attempt_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert!(stored.0.is_none());
    assert_eq!(stored.1, Some(object.object_id));
    let terminal: Value = sqlx::query_scalar(
        "SELECT terminal_result_json FROM workflow_executions WHERE tenant_id=? AND id=?",
    )
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(terminal["message"], message);
    while agentx_v2_runtime::artifact::externalize_one(&fixture.state)
        .await
        .unwrap()
    {}
    // Explain the exact statement executed by the production adapter. The
    // selector must be covered by metadata; it must not read historical JSON.
    let selector: String = sqlx::query_scalar("SELECT SQL_TEXT FROM performance_schema.prepared_statements_instances WHERE SQL_TEXT LIKE 'SELECT id FROM checkpoints %payload_size_bytes>%' LIMIT 1")
        .fetch_one(&fixture.observer).await.unwrap();
    // MySQL reports the inner SELECT's prepared-statement metadata for
    // EXPLAIN. Use the text protocol with this fixed numeric parameter.
    let explain = format!(
        "EXPLAIN FORMAT=TRADITIONAL {}",
        selector.replace('?', &agentx_runtime_contracts::INLINE_RESULT_LIMIT_BYTES.to_string())
    );
    let plan = sqlx::raw_sql(&explain)
        .fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(plan.try_get::<String, _>("key").unwrap(), "idx_checkpoint_externalize");
    assert!(plan.try_get::<String, _>("Extra").unwrap().contains("Using index"), "checkpoint discovery must not read payload pages");
    let recovery_selector: String = sqlx::query_scalar("SELECT SQL_TEXT FROM performance_schema.prepared_statements_instances WHERE SQL_TEXT LIKE 'SELECT o.id FROM execution_outbox o FORCE INDEX(idx_execution_outbox_recovery)%' LIMIT 1")
        .fetch_one(&fixture.observer).await.unwrap();
    let explain = format!("EXPLAIN FORMAT=TRADITIONAL {}", recovery_selector.replace('?', "100"));
    let recovery_plan = sqlx::raw_sql(&explain).fetch_all(&fixture.state.pool).await.unwrap();
    let outbox_plan = recovery_plan.iter().find(|row| row.try_get::<String, _>("table").unwrap() == "o").unwrap();
    assert_eq!(outbox_plan.try_get::<String, _>("key").unwrap(), "idx_execution_outbox_recovery");
    assert!(outbox_plan.try_get::<String, _>("Extra").unwrap().split(';').any(|part| part.trim() == "Using index"), "recovery discovery must not read historical payload pages");
    let terminal_object_id: Uuid = sqlx::query_scalar(
        "SELECT terminal_result_object_id FROM workflow_executions WHERE tenant_id=? AND id=? AND terminal_result_json IS NULL",
    )
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_ne!(terminal_object_id, object.object_id);
    let input_artifact_id: Uuid = sqlx::query_scalar(
        "SELECT r.artifact_id FROM artifact_references r WHERE r.tenant_id=? AND r.owner_type='node_attempt' AND r.owner_id=? AND r.reference_role='attempt_input' LIMIT 1",
    )
    .bind(fixture.tenant_id)
    .bind(result.attempt_id.to_string())
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_ne!(input_artifact_id, object.object_id);
    let trace_refs: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT CAST(JSON_UNQUOTE(JSON_EXTRACT(payload_json,'$.contentRef')) AS CHAR(36)) FROM trace_outbox WHERE tenant_id=? AND execution_id=? AND JSON_EXTRACT(payload_json,'$.contentRef') IS NOT NULL",
    )
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .fetch_all(&fixture.state.pool)
    .await
    .unwrap();
    assert!(trace_refs.contains(&input_artifact_id.to_string()));
    assert!(trace_refs.contains(&object.object_id.to_string()));
    let artifact_subject = Uuid::now_v7();
    let artifact_department = Uuid::now_v7();
    sqlx::query("INSERT INTO runtime_user_admission(tenant_id,user_id,user_name,department_id,department_name,token_version,status,tenant_query_enabled,role_assignments_json,admission_epoch) VALUES(?,?,'Artifact Reader',?,'Artifact Department',1,'active',FALSE,JSON_ARRAY(),1)")
        .bind(fixture.tenant_id)
        .bind(artifact_subject)
        .bind(artifact_department)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtime_user_application_grants(tenant_id,user_id,application_id,grant_version,status,can_invoke,can_query,admission_epoch) VALUES(?,?,?,1,'active',TRUE,TRUE,1)")
        .bind(fixture.tenant_id).bind(artifact_subject).bind(fixture.application_id).execute(&fixture.state.pool).await.unwrap();
    sqlx::query("INSERT INTO runtime_user_workflow_grants(tenant_id,user_id,workflow_id,grant_version,status,admission_epoch) VALUES(?,?,?,1,'active',1)")
        .bind(fixture.tenant_id).bind(artifact_subject).bind(fixture.workflow_id).execute(&fixture.state.pool).await.unwrap();
    let artifact_hash = agentx_runtime_contracts::content_hash(
        &json!({"operation":"execution_artifact","executionId":execution_id}),
    )
    .unwrap();
    let downloaded = get_execution_artifact(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            artifact_subject,
            BTreeSet::from(["runtime.query.execution".into()]),
            BTreeSet::new(),
            BTreeSet::from([execution_id]),
            artifact_hash.clone(),
        ),
        Path((execution_id, input_artifact_id)),
    )
    .await
    .unwrap();
    assert!(
        !to_bytes(downloaded.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
    let denied = get_execution_artifact(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            artifact_subject,
            BTreeSet::from(["runtime.query.execution".into()]),
            BTreeSet::new(),
            BTreeSet::from([Uuid::now_v7()]),
            artifact_hash,
        ),
        Path((execution_id, input_artifact_id)),
    )
    .await;
    assert!(matches!(denied, Err(RuntimeError::Unauthorized)));
    let checkpoint: (Uuid, Uuid) = sqlx::query_as(
        "SELECT id,payload_artifact_id FROM checkpoints WHERE tenant_id=? AND execution_id=? AND payload_json IS NULL ORDER BY sequence_number DESC LIMIT 1",
    )
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let source_version: u64 = sqlx::query_scalar(
        "SELECT state_version FROM workflow_executions WHERE tenant_id=? AND id=?",
    )
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let fork_command_id = Uuid::now_v7();
    let _ = apply_runtime_command(
        State(fixture.state.clone()),
        publisher_headers("runtime.commands.apply"),
        Json(agentx_runtime_contracts::RuntimeCommandApplyRequestV1 {
            api_version: 1,
            command_id: fork_command_id,
            tenant_id: fixture.tenant_id,
            object_version: source_version,
            idempotency_key: format!("large:fork:{execution_id}"),
            command: agentx_runtime_contracts::ExecutionCommandV1::Fork {
                source_execution_id: execution_id,
                checkpoint_id: checkpoint.0,
                origin: agentx_runtime_contracts::ExecutionOriginV1::system(Some(execution_id)),
                mode: agentx_runtime_contracts::PartialExecutionModeV1::Whole,
                node_id: None,
                side_effect_resolution: SideEffectResolutionV1::Execute,
            },
        }),
    )
    .await
    .unwrap();
    let fork_claim = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.command_id == fork_command_id)
        .unwrap();
    process_command(&fixture.state.pool, &fork_claim)
        .await
        .unwrap();
    let fork_execution_id: Uuid = sqlx::query_scalar(
        "SELECT fork_execution_id FROM execution_forks WHERE tenant_id=? AND source_execution_id=? AND source_checkpoint_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .bind(checkpoint.0)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let start_claim = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.execution_id == fork_execution_id)
        .unwrap();
    agentx_v2_runtime::execution::process_command_with_state(&fixture.state, &start_claim)
        .await
        .unwrap();
    let fork_status: String =
        sqlx::query_scalar("SELECT status FROM workflow_executions WHERE id=?")
            .bind(fork_execution_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(fork_status, "running");
    let fork_dispatch = claim_dispatch(&fixture.state.pool, owner)
        .await
        .unwrap()
        .unwrap();
    let fork_task = fork_dispatch.task().unwrap();
    assert_eq!(fork_task.execution_id, fork_execution_id);
    complete_dispatch(&fixture.state.pool, &fork_dispatch)
        .await
        .unwrap();
    let fork_worker_id = Uuid::now_v7();
    agentx_v2_runtime::engine::register_worker(
        &fixture.state.pool,
        fork_worker_id,
        fork_task.capability.as_str(),
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .unwrap();
    let fork_worker_claim = agentx_v2_runtime::engine::claim_worker_attempt(
        &fixture.state.pool,
        fork_worker_id,
        fork_task.capability.as_str(),
        &fork_task,
    )
    .await
    .unwrap()
    .unwrap();
    let fork_execution = worker.execute(&fork_worker_claim).await;
    let fork_result = worker
        .build_result(&fork_worker_claim, fork_execution)
        .await
        .unwrap();
    agentx_v2_runtime::engine::submit_worker_result_with_objects(
        &fixture.state.pool,
        fixture.state.objects.clone(),
        &fork_result,
    )
    .await
    .unwrap();
}
