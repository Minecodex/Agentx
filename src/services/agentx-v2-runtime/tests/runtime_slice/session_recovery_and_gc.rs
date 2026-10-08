async fn session_message_appends_one_assistant_response(fixture: &Fixture) {
    let router = agentx_v2_runtime::gateway::router().with_state(fixture.state.clone());
    let session_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/applications/runtime-slice/sessions")
                .header(AUTHORIZATION, format!("Bearer {}", fixture.api_key))
                .header("content-type", "application/json")
                .header("idempotency-key", "runtime-slice-session")
                .body(Body::from(
                    serde_json::to_vec(&CreateSessionRequestV1 {
                        external_user_id: Some("runtime-slice-user".into()),
                        title: None,
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(session_response.status(), axum::http::StatusCode::CREATED);
    let session: SessionResponseV1 = serde_json::from_slice(
        &to_bytes(session_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    let message_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{}/messages", session.id))
                .header(AUTHORIZATION, format!("Bearer {}", fixture.api_key))
                .header("content-type", "application/json")
                .header("idempotency-key", "runtime-slice-session-message")
                .body(Body::from(
                    serde_json::to_vec(&MessageRequestV1 {
                        parts: vec![MessagePartInputV1 {
                            part_type: "text".into(),
                            content: Some(json!("session-message")),
                            artifact_id: None,
                        }],
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let message_status = message_response.status();
    let message_body = to_bytes(message_response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        message_status,
        axum::http::StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&message_body)
    );
    let invocation: InvocationResponseV1 = serde_json::from_slice(&message_body).unwrap();
    assert_eq!(invocation.session_id, Some(session.id));
    let inferred_title: Option<String> = sqlx::query_scalar(
        "SELECT title FROM application_sessions WHERE tenant_id=? AND id=?",
    )
    .bind(fixture.tenant_id)
    .bind(session.id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(inferred_title.as_deref(), Some("session-message"));
    let mapping_snapshot = sqlx::query("SELECT chat_mapping_version,chat_mapping_json FROM application_invocations WHERE tenant_id=? AND id=?")
        .bind(fixture.tenant_id)
        .bind(invocation.id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(
        mapping_snapshot
            .try_get::<Option<u64>, _>("chat_mapping_version")
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        mapping_snapshot
            .try_get::<Option<Value>, _>("chat_mapping_json")
            .unwrap(),
        Some(
            json!({"questionInput":"message","fileInput":null,"answerOutput":"message","answerFilesOutput":null})
        )
    );
    let result = invocation_and_dispatch_recovery_are_fenced(
        fixture,
        invocation
            .execution_id
            .expect("Session Invocation has an Execution"),
    )
    .await;
    assert!(
        agentx_v2_runtime::engine::submit_worker_result(&fixture.state.pool, &result)
            .await
            .unwrap(),
        "a duplicate Worker Result must replay its original Receipt"
    );

    let rows = sqlx::query(
        "SELECT sequence_number,role,invocation_id FROM application_messages WHERE tenant_id=? AND session_id=? ORDER BY sequence_number,id",
    )
    .bind(fixture.tenant_id)
    .bind(session.id)
    .fetch_all(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].try_get::<u64, _>("sequence_number").unwrap(), 1);
    assert_eq!(rows[0].try_get::<String, _>("role").unwrap(), "user");
    assert_eq!(rows[1].try_get::<u64, _>("sequence_number").unwrap(), 2);
    assert_eq!(rows[1].try_get::<String, _>("role").unwrap(), "assistant");
    assert!(rows.iter().all(|row| {
        row.try_get::<Option<Uuid>, _>("invocation_id").unwrap() == Some(invocation.id)
    }));

    let list_response = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/sessions/{}/messages", session.id))
                .header(AUTHORIZATION, format!("Bearer {}", fixture.api_key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_response.status(), axum::http::StatusCode::OK);
    let messages: Vec<MessageResponseV1> = serde_json::from_slice(
        &to_bytes(list_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| (message.sequence, message.role.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "user"), (2, "assistant")]
    );
    assert_eq!(messages[1].parts[0].part_type, "text");
    assert_eq!(messages[1].parts[0].content, Some(json!("session-message")));

    sqlx::query(
        "UPDATE application_chat_mappings SET version=2 WHERE tenant_id=? AND application_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.application_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let next_response = agentx_v2_runtime::gateway::router()
        .with_state(fixture.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{}/messages", session.id))
                .header(AUTHORIZATION, format!("Bearer {}", fixture.api_key))
                .header("content-type", "application/json")
                .header("idempotency-key", "runtime-slice-session-message-v2")
                .body(Body::from(
                    serde_json::to_vec(&MessageRequestV1 {
                        parts: vec![MessagePartInputV1 {
                            part_type: "text".into(),
                            content: Some(json!("session-message-v2")),
                            artifact_id: None,
                        }],
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(next_response.status(), axum::http::StatusCode::ACCEPTED);
    let next_invocation: InvocationResponseV1 = serde_json::from_slice(
        &to_bytes(next_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let next_result =
        invocation_and_dispatch_recovery_are_fenced(fixture, next_invocation.execution_id.unwrap())
            .await;
    assert!(
        agentx_v2_runtime::engine::submit_worker_result(&fixture.state.pool, &next_result)
            .await
            .unwrap()
    );
    let snapshot_versions: Vec<u64> = sqlx::query_scalar("SELECT chat_mapping_version FROM application_invocations WHERE tenant_id=? AND session_id=? ORDER BY created_at,id")
        .bind(fixture.tenant_id)
        .bind(session.id)
        .fetch_all(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(snapshot_versions, vec![1, 2]);
    let unchanged_title: Option<String> = sqlx::query_scalar(
        "SELECT title FROM application_sessions WHERE tenant_id=? AND id=?",
    )
    .bind(fixture.tenant_id)
    .bind(session.id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(unchanged_title.as_deref(), Some("session-message"));
}

async fn durable_agent_pending_wakeups_are_ordered_and_idempotent(fixture: &Fixture) {
    let session_key = format!("application:{}:pending-wakeup", fixture.application_id);
    let node_key = "pending-agent";
    sqlx::query("INSERT INTO agent_session_registers(tenant_id,application_id,session_key,stable_agent_node_key,session_id,bundle_hash,definition_hash,model_version,core_contract_version,open_operation_id,state_version,fencing_token,register_json,lease_expires_at) VALUES(?,?,?,?,?,'bundle','definition','model:1','1.1','open-operation',1,7,JSON_OBJECT(),DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 5 MINUTE))")
        .bind(fixture.tenant_id)
        .bind(fixture.application_id)
        .bind(&session_key)
        .bind(node_key)
        .bind("pending-session")
        .execute(&fixture.state.pool)
        .await
        .unwrap();

    for sequence in 1_u64..=32 {
        let execution_id = Uuid::now_v7();
        let node_execution_id = Uuid::now_v7();
        let attempt_id = Uuid::now_v7();
        sqlx::query("INSERT INTO agent_session_pending_entries(entry_id,tenant_id,session_key,stable_agent_node_key,execution_id,node_execution_id,attempt_id,queue_kind,idempotency_key,payload_json,sequence_number) VALUES(?,?,?,?,?,?,?,'retry',?,JSON_OBJECT('sequence',?),?)")
            .bind(format!("pending:{sequence:02}"))
            .bind(fixture.tenant_id)
            .bind(&session_key)
            .bind(node_key)
            .bind(execution_id)
            .bind(node_execution_id)
            .bind(attempt_id)
            .bind(format!("pending-input:{sequence:02}"))
            .bind(sequence)
            .bind(sequence)
            .execute(&fixture.state.pool)
            .await
            .unwrap();
    }

    assert_eq!(
        wake_pending_sessions(&fixture.state.pool, 100)
            .await
            .unwrap(),
        0,
        "an active open operation must keep pending inputs durable and asleep"
    );
    sqlx::query("UPDATE agent_session_registers SET open_operation_id=NULL,lease_expires_at=NULL WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?")
        .bind(fixture.tenant_id)
        .bind(&session_key)
        .bind(node_key)
        .execute(&fixture.state.pool)
        .await
        .unwrap();

    assert_eq!(
        wake_pending_sessions(&fixture.state.pool, 2).await.unwrap(),
        2
    );
    let first_woken = sqlx::query_scalar::<_, u64>("SELECT sequence_number FROM agent_session_pending_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? AND wake_command_id IS NOT NULL ORDER BY sequence_number")
        .bind(fixture.tenant_id)
        .bind(&session_key)
        .bind(node_key)
        .fetch_all(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(first_woken, vec![1, 2]);
    assert_eq!(
        wake_pending_sessions(&fixture.state.pool, 100)
            .await
            .unwrap(),
        30
    );
    assert_eq!(
        wake_pending_sessions(&fixture.state.pool, 100)
            .await
            .unwrap(),
        0
    );
    let command_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_commands WHERE tenant_id=? AND idempotency_key LIKE 'agent-session-wakeup:pending:%'")
        .bind(fixture.tenant_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(command_count, 32);

    sqlx::query("DELETE FROM runtime_commands WHERE tenant_id=? AND idempotency_key LIKE 'agent-session-wakeup:pending:%'")
        .bind(fixture.tenant_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_session_pending_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?")
        .bind(fixture.tenant_id)
        .bind(&session_key)
        .bind(node_key)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_session_registers WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?")
        .bind(fixture.tenant_id)
        .bind(&session_key)
        .bind(node_key)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
}

async fn chat_mapping_publication_is_idempotent(
    fixture: &Fixture,
    bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
) {
    let mapping = ChatMappingV1 {
        question_input: "message".into(),
        file_input: None,
        answer_output: "message".into(),
        answer_files_output: None,
    };
    let hash = agentx_runtime_contracts::content_hash(&Some(mapping.clone())).unwrap();
    let request = ApplyChatMappingRequestV1 {
        api_version: 1,
        idempotency_key: "runtime-slice-chat-mapping-v1".into(),
        tenant_id: fixture.tenant_id,
        application_id: fixture.application_id,
        deployment_id: bundle.payload.deployment_id,
        bundle_id: bundle.payload.bundle_id,
        version: 1,
        mapping: Some(mapping.clone()),
        content_hash: hash,
    };
    let first = apply_chat_mapping(
        State(fixture.state.clone()),
        publisher_headers("runtime.chat_mappings.apply"),
        Json(request.clone()),
    )
        .await
        .unwrap();
    assert!(!first.replayed);
    let replay = apply_chat_mapping(
        State(fixture.state.clone()),
        publisher_headers("runtime.chat_mappings.apply"),
        Json(request.clone()),
    )
    .await
    .unwrap();
    assert!(replay.replayed);

    let conflicting_mapping = ChatMappingV1 {
        answer_output: "other".into(),
        ..mapping
    };
    let conflict = apply_chat_mapping(
        State(fixture.state.clone()),
        publisher_headers("runtime.chat_mappings.apply"),
        Json(ApplyChatMappingRequestV1 {
            mapping: Some(conflicting_mapping.clone()),
            content_hash: agentx_runtime_contracts::content_hash(&Some(conflicting_mapping)).unwrap(),
            ..request
        }),
    )
    .await;
    assert!(matches!(
        conflict,
        Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            _
        ))
    ));
}

async fn invocation_and_dispatch_recovery_are_fenced(
    fixture: &Fixture,
    execution_id: Uuid,
) -> WorkerResultV1 {
    let owner = Uuid::now_v7();
    let command = claim_commands(&fixture.state.pool, owner, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.execution_id == execution_id)
        .unwrap();
    process_command(&fixture.state.pool, &command)
        .await
        .unwrap();
    let failed_dispatch = claim_dispatch(&fixture.state.pool, owner)
        .await
        .unwrap()
        .unwrap();
    release_dispatch(
        &fixture.state.pool,
        &failed_dispatch,
        "Redis connection was rebuilt",
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let dispatch = claim_dispatch(&fixture.state.pool, owner)
        .await
        .unwrap()
        .unwrap();
    let task = dispatch.task().unwrap();
    assert_eq!(dispatch.id, failed_dispatch.id);
    assert_eq!(task.attempt_id, failed_dispatch.task().unwrap().attempt_id);
    assert!(dispatch.fencing_token > failed_dispatch.fencing_token);
    assert!(matches!(
        complete_dispatch(&fixture.state.pool, &failed_dispatch).await,
        Err(RuntimeError::Conflict(_, _))
    ));
    let attempt_id = task.attempt_id;
    complete_dispatch(&fixture.state.pool, &dispatch)
        .await
        .unwrap();
    assert!(matches!(
        complete_dispatch(&fixture.state.pool, &dispatch).await,
        Err(RuntimeError::Conflict(_, _))
    ));
    sqlx::query("UPDATE execution_outbox SET published_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 6 SECOND) WHERE id=?")
        .bind(dispatch.id).execute(&fixture.state.pool).await.unwrap();
    let (first, second) = tokio::join!(
        recover_dispatches(&fixture.state.pool, 100),
        recover_dispatches(&fixture.state.pool, 100)
    );
    let recovered: Vec<_> = first.unwrap().into_iter().chain(second.unwrap()).collect();
    assert_eq!(
        recovered.iter().filter(|message| message.attempt_id == attempt_id).count(),
        1,
        "concurrent recovery must re-dispatch an eligible attempt only once"
    );

    let workers = (0..20).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    for worker in &workers {
        agentx_v2_runtime::engine::register_worker(
            &fixture.state.pool,
            *worker,
            task.capability.as_str(),
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .unwrap();
    }
    // Freeze unrelated authority/snapshot rows. Claiming an Attempt must
    // still succeed; only that Attempt row is a claim lock.
    let mut held_snapshot = fixture.state.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM workflow_executions WHERE id=? FOR UPDATE")
        .bind(task.execution_id).fetch_one(&mut *held_snapshot).await.unwrap();
    sqlx::query("SELECT id FROM node_executions WHERE id=? FOR UPDATE")
        .bind(task.node_execution_id).fetch_one(&mut *held_snapshot).await.unwrap();
    for statement in [
        "SELECT execution_id FROM execution_snapshots WHERE execution_id=? FOR UPDATE",
        "SELECT execution_id FROM execution_runtime_state WHERE execution_id=? FOR UPDATE",
    ] {
        sqlx::query(statement)
            .bind(task.execution_id).fetch_one(&mut *held_snapshot).await.unwrap();
    }
    let mut contenders = tokio::task::JoinSet::new();
    for worker in workers {
        let pool = fixture.state.pool.clone();
        let task = task.clone();
        contenders.spawn(async move {
            agentx_v2_runtime::engine::claim_worker_attempt(
                &pool,
                worker,
                task.capability.as_str(),
                &task,
            )
            .await
        });
    }
    let mut claims = Vec::new();
    tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(result) = contenders.join_next().await {
            if let Some(claim) = result.unwrap().unwrap() {
                claims.push(claim);
            }
        }
    }).await.expect("Worker claims must not lock unrelated Workflow/snapshot rows");
    held_snapshot.rollback().await.unwrap();
    assert_eq!(
        claims.len(),
        1,
        "20 concurrent Workers must produce one Lease"
    );
    let first_claim = claims.pop().unwrap();
    let mut held = fixture.state.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM node_attempts WHERE id=? FOR UPDATE")
        .bind(attempt_id)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_millis(200),
        recover_dispatches(&fixture.state.pool, 100),
    )
    .await
    .expect("recovery must not lock healthy, unexpired attempts")
    .unwrap();
    held.rollback().await.unwrap();
    sqlx::query("UPDATE node_attempts SET locked_until=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE id=?")
        .bind(attempt_id).execute(&fixture.state.pool).await.unwrap();
    recover_dispatches(&fixture.state.pool, 100).await.unwrap();
    let replacement = Uuid::now_v7();
    agentx_v2_runtime::engine::register_worker(
        &fixture.state.pool,
        replacement,
        task.capability.as_str(),
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .unwrap();
    let replacement_claim = agentx_v2_runtime::engine::claim_worker_attempt(
        &fixture.state.pool,
        replacement,
        task.capability.as_str(),
        &task,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(replacement_claim.lease.fencing_token > first_claim.lease.fencing_token);
    let first_result = successful_worker_result(&first_claim);
    assert!(matches!(
        agentx_v2_runtime::engine::submit_worker_result(&fixture.state.pool, &first_result).await,
        Err(RuntimeError::Conflict(_, _))
    ));
    let replacement_result = successful_worker_result(&replacement_claim);
    let unrelated_reservation = Uuid::now_v7();
    sqlx::query("INSERT INTO quota_reservations(id,tenant_id,dimension_key,scope_type,scope_id,idempotency_key,amount,expires_at) VALUES(?,?,'execution_concurrency','execution',?, ?,1,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 300 SECOND))")
        .bind(unrelated_reservation).bind(fixture.tenant_id)
        .bind(Uuid::now_v7().to_string()).bind(format!("held:{unrelated_reservation}"))
        .execute(&fixture.state.pool).await.unwrap();
    let mut held = fixture.state.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM quota_reservations WHERE id=? FOR UPDATE")
        .bind(unrelated_reservation).fetch_one(&mut *held).await.unwrap();
    tokio::time::timeout(
        Duration::from_millis(500),
        agentx_v2_runtime::engine::submit_worker_result(&fixture.state.pool, &replacement_result),
    )
    .await
    .expect("settlement must not lock reservations belonging to another execution")
    .unwrap();
    held.rollback().await.unwrap();
    sqlx::query("DELETE FROM quota_reservations WHERE id=?")
        .bind(unrelated_reservation).execute(&fixture.state.pool).await.unwrap();
    replacement_result
}

async fn expired_attempt_deadline_is_terminal_and_not_requeued(fixture: &Fixture) {
    let accepted = create_invocation(
        &fixture.state.pool,
        fixture.tenant_id,
        fixture.application_id,
        fixture.key_id,
        &InvocationRequestV1 {
            input: json!({"message":"deadline"}),
            idempotency_key: "runtime-slice-expired-deadline".into(),
        },
    )
    .await
    .unwrap();
    let owner = Uuid::now_v7();
    let command = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.execution_id == accepted.execution_id)
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
    sqlx::query(
        "UPDATE node_attempts SET deadline_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE tenant_id=? AND id=?",
    )
    .bind(fixture.tenant_id)
    .bind(claim.task.attempt_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();

    let recovered = recover_dispatches(&fixture.state.pool, 100).await.unwrap();
    assert!(
        !recovered
            .iter()
            .any(|message| message.attempt_id == task.attempt_id)
    );
    let state: (String, String, String, bool) = sqlx::query_as(
        "SELECT e.status,a.status,n.status,l.released_at IS NOT NULL FROM workflow_executions e JOIN node_attempts a ON a.execution_id=e.id AND a.tenant_id=e.tenant_id JOIN node_executions n ON n.id=a.node_execution_id AND n.tenant_id=a.tenant_id JOIN worker_leases l ON l.node_attempt_id=a.id AND l.tenant_id=a.tenant_id WHERE e.tenant_id=? AND e.id=? AND a.id=?",
    )
    .bind(fixture.tenant_id)
    .bind(accepted.execution_id)
    .bind(task.attempt_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(
        state,
        (
            "timed_out".into(),
            "timed_out".into(),
            "timed_out".into(),
            true
        )
    );

    let stale_worker = Uuid::now_v7();
    agentx_v2_runtime::engine::register_worker(
        &fixture.state.pool,
        stale_worker,
        task.capability.as_str(),
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .unwrap();
    assert!(
        agentx_v2_runtime::engine::claim_worker_attempt(
            &fixture.state.pool,
            stale_worker,
            task.capability.as_str(),
            &task,
        )
        .await
        .unwrap()
        .is_none(),
        "a stale dispatch message must be ACK-safe after the deadline becomes terminal"
    );
}

async fn retry_policy_creates_a_second_attempt_and_trace(fixture: &Fixture) {
    let accepted = create_invocation(
        &fixture.state.pool,
        fixture.tenant_id,
        fixture.application_id,
        fixture.key_id,
        &InvocationRequestV1 {
            input: json!({"message":"retry-me"}),
            idempotency_key: "runtime-slice-retry".into(),
        },
    )
    .await
    .unwrap();
    let owner = Uuid::now_v7();
    let command = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.execution_id == accepted.execution_id)
        .unwrap();
    process_command(&fixture.state.pool, &command)
        .await
        .unwrap();
    let dispatch = claim_dispatch(&fixture.state.pool, owner)
        .await
        .unwrap()
        .unwrap();
    let first_task = dispatch.task().unwrap();
    complete_dispatch(&fixture.state.pool, &dispatch)
        .await
        .unwrap();
    let first_worker = Uuid::now_v7();
    agentx_v2_runtime::engine::register_worker(
        &fixture.state.pool,
        first_worker,
        first_task.capability.as_str(),
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .unwrap();
    let first = agentx_v2_runtime::engine::claim_worker_attempt(
        &fixture.state.pool,
        first_worker,
        first_task.capability.as_str(),
        &first_task,
    )
    .await
    .unwrap()
    .unwrap();
    let failed_status = WorkerResultStatusV1::Failed;
    let failed = WorkerResultV1 {
        protocol_version: 1,
        attempt_id: first.task.attempt_id,
        worker_id: first.lease.worker_id,
        fencing_token: first.lease.fencing_token,
        status: failed_status,
        result_hash: agentx_v2_runtime::engine::worker_result_hash(
            failed_status,
            &BTreeMap::new(),
            None,
            Some("CONTROLLED_FIRST_FAILURE"),
            Some("The first Attempt fails for retry verification"),
            None,
            None,
        )
        .unwrap(),
        outputs: BTreeMap::new(),
        output_object: None,
        error_code: Some("CONTROLLED_FIRST_FAILURE".into()),
        error_message: Some("The first Attempt fails for retry verification".into()),
        retryable: None,
        partial_output_object: None,
    };
    agentx_v2_runtime::engine::submit_worker_result(&fixture.state.pool, &failed)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let attempts = sqlx::query("SELECT id,attempt_number,status FROM node_attempts WHERE tenant_id=? AND execution_id=? ORDER BY attempt_number")
        .bind(fixture.tenant_id).bind(accepted.execution_id).fetch_all(&fixture.state.pool).await.unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].try_get::<u16, _>("attempt_number").unwrap(), 1);
    assert_eq!(
        attempts[0].try_get::<String, _>("status").unwrap(),
        "failed"
    );
    assert_eq!(attempts[1].try_get::<u16, _>("attempt_number").unwrap(), 2);
    let dispatch = claim_dispatch(&fixture.state.pool, owner)
        .await
        .unwrap()
        .unwrap();
    let second_task = dispatch.task().unwrap();
    assert_eq!(
        second_task.attempt_id,
        attempts[1].try_get::<Uuid, _>("id").unwrap()
    );
    complete_dispatch(&fixture.state.pool, &dispatch)
        .await
        .unwrap();
    let second_worker = Uuid::now_v7();
    agentx_v2_runtime::engine::register_worker(
        &fixture.state.pool,
        second_worker,
        second_task.capability.as_str(),
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .unwrap();
    let second = agentx_v2_runtime::engine::claim_worker_attempt(
        &fixture.state.pool,
        second_worker,
        second_task.capability.as_str(),
        &second_task,
    )
    .await
    .unwrap()
    .unwrap();
    agentx_v2_runtime::engine::submit_worker_result(
        &fixture.state.pool,
        &successful_worker_result(&second),
    )
    .await
    .unwrap();
    let execution_status: String =
        sqlx::query_scalar("SELECT status FROM workflow_executions WHERE tenant_id=? AND id=?")
            .bind(fixture.tenant_id)
            .bind(accepted.execution_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(execution_status, "succeeded");
    let traced_attempts: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT JSON_UNQUOTE(JSON_EXTRACT(payload_json,'$.attemptId'))) FROM trace_outbox WHERE tenant_id=? AND execution_id=? AND JSON_UNQUOTE(JSON_EXTRACT(payload_json,'$.spanKind'))='attempt'")
        .bind(fixture.tenant_id).bind(accepted.execution_id).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(traced_attempts, 2);
}

fn successful_worker_result(
    claim: &agentx_v2_runtime::engine::ClaimedWorkerAttempt,
) -> WorkerResultV1 {
    let outputs = BTreeMap::from([(
        "main".into(),
        claim.inputs.get("main").cloned().unwrap_or_default(),
    )]);
    let status = WorkerResultStatusV1::Succeeded;
    let result_hash =
        agentx_v2_runtime::engine::worker_result_hash(
            status, &outputs, None, None, None, None, None,
        )
            .unwrap();
    WorkerResultV1 {
        protocol_version: 1,
        attempt_id: claim.task.attempt_id,
        worker_id: claim.lease.worker_id,
        fencing_token: claim.lease.fencing_token,
        status,
        result_hash,
        outputs,
        output_object: None,
        error_code: None,
        error_message: None,
        retryable: None,
        partial_output_object: None,
    }
}

async fn query_is_tenant_application_and_execution_scoped(fixture: &Fixture, execution_id: Uuid) {
    let subject_id = Uuid::now_v7();
    let initiator_department_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO runtime_user_admission(tenant_id,user_id,user_name,department_id,department_name,token_version,status,tenant_query_enabled,role_assignments_json,admission_epoch) VALUES(?,?,'Historical User',?,'Historical Department',1,'active',FALSE,JSON_ARRAY(),1)",
    )
    .bind(fixture.tenant_id)
    .bind(subject_id)
    .bind(initiator_department_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO runtime_user_application_grants(tenant_id,user_id,application_id,grant_version,status,can_invoke,can_query,admission_epoch) VALUES(?,?,?,1,'active',TRUE,TRUE,1)",
    )
    .bind(fixture.tenant_id)
    .bind(subject_id)
    .bind(fixture.application_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();

    let exact = get_execution(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            subject_id,
            BTreeSet::from(["runtime.query.execution".into()]),
            BTreeSet::new(),
            BTreeSet::from([execution_id]),
            agentx_runtime_contracts::content_hash(
                &json!({"operation":"get_execution","executionId":execution_id}),
            )
            .unwrap(),
        ),
        Path(execution_id),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(exact.summary.execution_id, execution_id);

    let runtime_details = get_execution_runtime_details(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            subject_id,
            BTreeSet::from(["runtime.query.execution".into()]),
            BTreeSet::new(),
            BTreeSet::from([execution_id]),
            agentx_runtime_contracts::content_hash(
                &json!({"operation":"execution_runtime_details","executionId":execution_id}),
            )
            .unwrap(),
        ),
        Path(execution_id),
    )
    .await
    .unwrap()
    .0;
    assert!(!runtime_details.attempts.is_empty());
    assert!(
        runtime_details
            .attempts
            .iter()
            .any(|attempt| attempt.worker_id.is_some()),
        "Runtime Details must decode the text-formatted worker UUID"
    );

    let denied = get_execution(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            subject_id,
            BTreeSet::from(["runtime.query.execution".into()]),
            BTreeSet::new(),
            BTreeSet::from([Uuid::now_v7()]),
            agentx_runtime_contracts::content_hash(
                &json!({"operation":"get_execution","executionId":execution_id}),
            )
            .unwrap(),
        ),
        Path(execution_id),
    )
    .await;
    assert!(matches!(denied, Err(RuntimeError::Unauthorized)));

    sqlx::query("UPDATE workflow_executions SET application_id=NULL WHERE id=?")
        .bind(execution_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    apply_target(
        fixture,
        10,
        AdmissionTargetV1::RuntimeUserWorkflowGrant {
            state: RuntimeUserWorkflowGrantV1 {
                tenant_id: fixture.tenant_id,
                user_id: subject_id,
                workflow_id: fixture.workflow_id,
                grant_version: 10,
                can_query: true,
            },
        },
    )
    .await;
    let app_less = get_execution(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            subject_id,
            BTreeSet::from(["runtime.query.execution".into()]),
            BTreeSet::new(),
            BTreeSet::from([execution_id]),
            agentx_runtime_contracts::content_hash(
                &json!({"operation":"get_execution","executionId":execution_id}),
            )
            .unwrap(),
        ),
        Path(execution_id),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(app_less.summary.execution_id, execution_id);

    apply_target(
        fixture,
        11,
        AdmissionTargetV1::RuntimeUserWorkflowGrant {
            state: RuntimeUserWorkflowGrantV1 {
                tenant_id: fixture.tenant_id,
                user_id: subject_id,
                workflow_id: fixture.workflow_id,
                grant_version: 11,
                can_query: false,
            },
        },
    )
    .await;
    apply_target(
        fixture,
        10,
        AdmissionTargetV1::RuntimeUserWorkflowGrant {
            state: RuntimeUserWorkflowGrantV1 {
                tenant_id: fixture.tenant_id,
                user_id: subject_id,
                workflow_id: fixture.workflow_id,
                grant_version: 10,
                can_query: true,
            },
        },
    )
    .await;
    let workflow_grant: (String, u64) = sqlx::query_as(
        "SELECT status,admission_epoch FROM runtime_user_workflow_grants WHERE tenant_id=? AND user_id=? AND workflow_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(subject_id)
    .bind(fixture.workflow_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(workflow_grant, ("revoked".into(), 11));
    let revoked = get_execution(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            subject_id,
            BTreeSet::from(["runtime.query.execution".into()]),
            BTreeSet::new(),
            BTreeSet::from([execution_id]),
            agentx_runtime_contracts::content_hash(
                &json!({"operation":"get_execution","executionId":execution_id}),
            )
            .unwrap(),
        ),
        Path(execution_id),
    )
    .await;
    assert!(matches!(revoked, Err(RuntimeError::Unauthorized)));
    sqlx::query("UPDATE workflow_executions SET application_id=? WHERE id=?")
        .bind(fixture.application_id)
        .bind(execution_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();

    let called_tool_id = Uuid::now_v7();
    sqlx::query("UPDATE workflow_executions SET trigger_type='api_key',initiator_user_id=?,initiator_user_name='Historical User',initiator_department_id=?,initiator_department_name='Historical Department',trigger_source_id=?,trigger_name='Historical Build Hook' WHERE tenant_id=? AND id=?")
        .bind(subject_id)
        .bind(initiator_department_id)
        .bind(fixture.key_id)
        .bind(fixture.tenant_id)
        .bind(execution_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtime_calls(id,tenant_id,execution_id,node_execution_id,attempt_id,call_index,call_kind,idempotency_key,request_fingerprint,resource_type,resource_id,tool_name_snapshot,status) VALUES(?,?,?,?,?,0,'mcp_tool',?,?,'mcp_tool',?,?,'succeeded')")
        .bind(Uuid::now_v7())
        .bind(fixture.tenant_id)
        .bind(execution_id)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(format!("query-filter-tool:{execution_id}"))
        .bind("f".repeat(64))
        .bind(called_tool_id)
        .bind("Historical Tool")
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    let configured_tool_id = Uuid::now_v7();
    let execution_bundle_id: Uuid =
        sqlx::query_scalar("SELECT bundle_id FROM workflow_executions WHERE tenant_id=? AND id=?")
            .bind(fixture.tenant_id)
            .bind(execution_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO runtime_resource_bindings(tenant_id,binding_id,bundle_id,work_package_id,resource_kind,resource_id,resource_version,state_epoch,content_hash,configuration_json,object_ids_json) VALUES(?,?,?,NULL,'mcp_tool',?,'configured-only',1,CONCAT('sha256:',REPEAT('0',64)),JSON_OBJECT('toolName','Configured Only Tool'),JSON_ARRAY())")
        .bind(fixture.tenant_id)
        .bind(Uuid::now_v7())
        .bind(execution_bundle_id)
        .bind(configured_tool_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();

    let search_request = ExecutionSearchRequestV1 {
        api_version: 1,
        tenant_id: fixture.tenant_id,
        application_ids: vec![fixture.application_id],
        workflow_ids: vec![],
        tool_ids: vec![],
        initiator_user_ids: vec![],
        initiator_department_ids: vec![],
        trigger_types: vec![],
        trigger_name: None,
        statuses: vec!["succeeded".into()],
        error_codes: vec![],
        session_mode: agentx_runtime_contracts::ExecutionSessionModeV1::All,
        created_after: None,
        created_before: None,
        search: None,
        cursor: None,
        limit: 50,
    };
    let page = search_executions(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            subject_id,
            BTreeSet::from(["runtime.query.executions".into()]),
            BTreeSet::from([fixture.application_id]),
            BTreeSet::new(),
            agentx_runtime_contracts::content_hash(
                &json!({"operation":"execution-search","request":search_request}),
            )
            .unwrap(),
        ),
        Json(search_request.clone()),
    )
    .await
    .unwrap()
    .0;
    assert!(
        page.items
            .iter()
            .any(|item| item.execution_id == execution_id)
    );
    let mut stateless_request = search_request.clone();
    stateless_request.session_mode = agentx_runtime_contracts::ExecutionSessionModeV1::Stateless;
    let stateless = scoped_execution_search(fixture, subject_id, stateless_request)
        .await
        .unwrap();
    assert!(stateless.items.iter().all(|item| item.session_id.is_none()));
    assert!(
        stateless
            .items
            .iter()
            .any(|item| item.execution_id == execution_id)
    );
    let mut session_request = search_request.clone();
    session_request.session_mode = agentx_runtime_contracts::ExecutionSessionModeV1::Session;
    let sessions = scoped_execution_search(fixture, subject_id, session_request)
        .await
        .unwrap();
    assert!(!sessions.items.is_empty());
    assert!(sessions.items.iter().all(|item| item.session_id.is_some()));

    let mut first_page_request = search_request.clone();
    first_page_request.limit = 1;
    let first_page = scoped_execution_search(fixture, subject_id, first_page_request.clone())
        .await
        .unwrap();
    assert_eq!(first_page.items.len(), 1);
    assert!(first_page.total > 1);
    let first_execution_id = first_page.items[0].execution_id;
    first_page_request.cursor = first_page.next.clone();
    let second_page = scoped_execution_search(fixture, subject_id, first_page_request.clone())
        .await
        .unwrap();
    assert_eq!(second_page.total, first_page.total);
    assert_eq!(second_page.snapshot_id, first_page.snapshot_id);
    assert_ne!(second_page.items[0].execution_id, first_execution_id);

    let mut changed_filter = first_page_request;
    changed_filter.statuses = vec!["failed".into()];
    assert!(matches!(
        scoped_execution_search(fixture, subject_id, changed_filter).await,
        Err(RuntimeError::QueryCursorExpired)
    ));

    let combined_request = ExecutionSearchRequestV1 {
        api_version: 1,
        tenant_id: fixture.tenant_id,
        application_ids: vec![fixture.application_id],
        workflow_ids: vec![],
        tool_ids: vec![called_tool_id, Uuid::now_v7()],
        initiator_user_ids: vec![Uuid::now_v7(), subject_id],
        initiator_department_ids: vec![Uuid::now_v7(), initiator_department_id],
        trigger_types: vec!["webhook".into(), "api_key".into()],
        trigger_name: Some("build hook".into()),
        statuses: vec!["failed".into(), "succeeded".into()],
        error_codes: vec![],
        session_mode: agentx_runtime_contracts::ExecutionSessionModeV1::All,
        created_after: Some(OffsetDateTime::UNIX_EPOCH),
        created_before: Some(OffsetDateTime::now_utc() + time::Duration::minutes(1)),
        search: None,
        cursor: None,
        limit: 8,
    };
    let combined = scoped_execution_search(fixture, subject_id, combined_request.clone())
        .await
        .unwrap();
    let summary = combined
        .items
        .iter()
        .find(|item| item.execution_id == execution_id)
        .expect("same-dimension OR and cross-dimension AND should match");
    assert_eq!(
        summary.initiator_user_name.as_deref(),
        Some("Historical User")
    );
    assert_eq!(
        summary.initiator_department_name.as_deref(),
        Some("Historical Department")
    );
    assert_eq!(
        summary.trigger_name.as_deref(),
        Some("Historical Build Hook")
    );

    let mut scope_only_request = combined_request.clone();
    scope_only_request.application_ids.clear();
    let scope_only = scoped_execution_search(fixture, subject_id, scope_only_request)
        .await
        .unwrap();
    assert!(
        scope_only
            .items
            .iter()
            .any(|item| item.execution_id == execution_id)
    );
    assert!(
        scope_only
            .items
            .iter()
            .all(|item| item.application_id == Some(fixture.application_id))
    );

    sqlx::query("UPDATE runtime_user_admission SET user_name='Renamed User',department_name='Renamed Department' WHERE tenant_id=? AND user_id=?")
        .bind(fixture.tenant_id)
        .bind(subject_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    let renamed = scoped_execution_search(fixture, subject_id, combined_request.clone())
        .await
        .unwrap();
    let historical = renamed
        .items
        .iter()
        .find(|item| item.execution_id == execution_id)
        .unwrap();
    assert_eq!(
        historical.initiator_user_name.as_deref(),
        Some("Historical User")
    );
    assert_eq!(
        historical.trigger_name.as_deref(),
        Some("Historical Build Hook")
    );

    let mut exact_department = combined_request.clone();
    exact_department.initiator_department_ids = vec![Uuid::now_v7()];
    assert!(
        scoped_execution_search(fixture, subject_id, exact_department)
            .await
            .unwrap()
            .items
            .is_empty()
    );

    let mut configured_but_not_called = combined_request;
    configured_but_not_called.tool_ids = vec![configured_tool_id];
    assert!(
        scoped_execution_search(fixture, subject_id, configured_but_not_called)
            .await
            .unwrap()
            .items
            .is_empty()
    );

    let wrong_application_request = ExecutionSearchRequestV1 {
        api_version: 1,
        tenant_id: fixture.tenant_id,
        application_ids: vec![fixture.application_id],
        workflow_ids: vec![],
        tool_ids: vec![],
        initiator_user_ids: vec![],
        initiator_department_ids: vec![],
        trigger_types: vec![],
        trigger_name: None,
        statuses: vec![],
        error_codes: vec![],
        session_mode: agentx_runtime_contracts::ExecutionSessionModeV1::All,
        created_after: None,
        created_before: None,
        search: None,
        cursor: None,
        limit: 50,
    };
    let wrong_application = search_executions(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            subject_id,
            BTreeSet::from(["runtime.query.executions".into()]),
            BTreeSet::from([Uuid::now_v7()]),
            BTreeSet::new(),
            agentx_runtime_contracts::content_hash(
                &json!({"operation":"execution-search","request":wrong_application_request}),
            )
            .unwrap(),
        ),
        Json(wrong_application_request),
    )
    .await;
    assert!(matches!(wrong_application, Err(RuntimeError::Unauthorized)));
}

async fn scoped_execution_search(
    fixture: &Fixture,
    subject_id: Uuid,
    request: ExecutionSearchRequestV1,
) -> Result<agentx_runtime_contracts::ExecutionSearchPageV1, RuntimeError> {
    let request_hash = agentx_runtime_contracts::content_hash(
        &json!({"operation":"execution-search","request":request}),
    )
    .unwrap();
    search_executions(
        State(fixture.state.clone()),
        delegation_headers(
            fixture.tenant_id,
            subject_id,
            BTreeSet::from(["runtime.query.executions".into()]),
            BTreeSet::from([fixture.application_id]),
            BTreeSet::new(),
            request_hash,
        ),
        Json(request),
    )
    .await
    .map(|page| page.0)
}

async fn gc_protects_heads_references_and_holds_then_sweeps(
    fixture: &Fixture,
    bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
) {
    sqlx::query("UPDATE deployment_bundles SET retained_until=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 DAY) WHERE id=?")
        .bind(bundle.payload.bundle_id).execute(&fixture.state.pool).await.unwrap();
    let reference = Uuid::now_v7();
    sqlx::query("INSERT INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'checkpoint_fork_source',?)")
        .bind(reference).bind(fixture.tenant_id).bind(bundle.payload.bundle_id).bind(Uuid::now_v7())
        .execute(&fixture.state.pool).await.unwrap();
    let hold = Uuid::now_v7();
    sqlx::query("INSERT INTO bundle_retention_holds(id,tenant_id,bundle_id,reason,held_by) VALUES(?,?,?,'test','runtime-slice')")
        .bind(hold).bind(fixture.tenant_id).bind(bundle.payload.bundle_id)
        .execute(&fixture.state.pool).await.unwrap();
    assert_eq!(
        mark_collectable(&fixture.state, Uuid::now_v7())
            .await
            .unwrap(),
        0
    );
    sqlx::query("UPDATE bundle_references SET released_at=UTC_TIMESTAMP(6) WHERE id=?")
        .bind(reference)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE bundle_retention_holds SET released_at=UTC_TIMESTAMP(6) WHERE id=?")
        .bind(hold)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    let run = Uuid::now_v7();
    assert_eq!(mark_collectable(&fixture.state, run).await.unwrap(), 1);
    assert!(
        sweep_one(&fixture.state, run, Uuid::now_v7())
            .await
            .unwrap()
    );
    assert!(
        !sweep_one(&fixture.state, run, Uuid::now_v7())
            .await
            .unwrap()
    );
    let bundles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployment_bundles WHERE id=?")
        .bind(bundle.payload.bundle_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(bundles, 0);
    let object_status: String =
        sqlx::query_scalar("SELECT status FROM runtime_objects WHERE object_id=?")
            .bind(bundle.payload.objects[0].object_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(object_status, "deleted");
    let run_status: String = sqlx::query_scalar("SELECT status FROM bundle_gc_runs WHERE id=?")
        .bind(run)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(run_status, "completed");
}

async fn expired_temporary_objects_are_removed(fixture: &Fixture) {
    let object_id = Uuid::now_v7();
    let key = format!("temporary/{}/{}", fixture.tenant_id, object_id);
    fixture
        .state
        .objects
        .put(
            &ObjectPath::from(key.clone()),
            Bytes::from_static(b"orphan").into(),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtime_objects(object_id,tenant_id,object_key,content_hash,size_bytes,media_type,status,temporary_key,idempotency_key,request_hash,temporary_expires_at) VALUES(?,?,?,CONCAT('sha256:',REPEAT('0',64)),6,'application/octet-stream','uploading',?,'orphan','sha256:orphan',DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND))")
        .bind(object_id).bind(fixture.tenant_id).bind(format!("runtime/{}/{}/{}",fixture.tenant_id,object_id,"0".repeat(64))).bind(&key)
        .execute(&fixture.state.pool).await.unwrap();
    assert_eq!(
        cleanup_expired_temporary_objects(&fixture.state, 100)
            .await
            .unwrap(),
        1
    );
    assert!(
        fixture
            .state
            .objects
            .get(&ObjectPath::from(key))
            .await
            .is_err()
    );
}

async fn gc_object_delete_failure_is_recorded_and_retryable(fixture: &Fixture) {
    let bundle = fixture.bundle(50).await;
    fixture
        .upload_bundle_object(&bundle, "bundle-object:gc-retry")
        .await;
    prepare(fixture, &bundle, "prepare:gc-retry").await;
    sqlx::query("UPDATE deployment_bundles SET status='retained',retained_until=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 DAY) WHERE id=?")
        .bind(bundle.payload.bundle_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    let run = Uuid::now_v7();
    let failing = Arc::new(FailOnceDeleteStore::new(fixture.state.objects.clone()));
    let state = RuntimeState {
        pool: fixture.state.pool.clone(),
        objects: failing.clone(),
        trust: fixture.state.trust.clone(),
        wakeups: Default::default(),
        vault: None,
    };
    assert_eq!(mark_collectable(&state, run).await.unwrap(), 1);
    assert!(sweep_one(&state, run, Uuid::now_v7()).await.unwrap());
    let failed: String =
        sqlx::query_scalar("SELECT status FROM bundle_gc_items WHERE gc_run_id=? AND bundle_id=?")
            .bind(run)
            .bind(bundle.payload.bundle_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(failed, "failed");
    assert!(sweep_one(&state, run, Uuid::now_v7()).await.unwrap());
    let deleted: String =
        sqlx::query_scalar("SELECT status FROM bundle_gc_items WHERE gc_run_id=? AND bundle_id=?")
            .bind(run)
            .bind(bundle.payload.bundle_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(deleted, "deleted");
    assert!(failing.failed.load(Ordering::SeqCst));
}

async fn ready_orphan_objects_are_swept_and_reuploadable(fixture: &Fixture) {
    let orphan = fixture.bundle(60).await;
    let first = fixture
        .upload_bundle_object(&orphan, "bundle-object:ready-orphan")
        .await;
    sqlx::query(
        "UPDATE runtime_objects SET ready_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 2 HOUR) WHERE tenant_id=? AND object_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(first.object.object_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();

    let run = Uuid::now_v7();
    assert_eq!(mark_collectable(&fixture.state, run).await.unwrap(), 1);
    let kind: String = sqlx::query_scalar(
        "SELECT item_kind FROM bundle_gc_items WHERE gc_run_id=? AND object_id=?",
    )
    .bind(run)
    .bind(first.object.object_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(kind, "object");
    assert!(
        sweep_one(&fixture.state, run, Uuid::now_v7())
            .await
            .unwrap()
    );
    let deleted: String =
        sqlx::query_scalar("SELECT status FROM runtime_objects WHERE object_id=?")
            .bind(first.object.object_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(deleted, "deleted");

    let restored = fixture
        .upload_bundle_object(&orphan, "bundle-object:ready-orphan")
        .await;
    assert!(!restored.replayed);
    let ready: String = sqlx::query_scalar("SELECT status FROM runtime_objects WHERE object_id=?")
        .bind(first.object.object_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(ready, "ready");

    let retry_orphan = fixture.bundle(62).await;
    let retry_receipt = fixture
        .upload_bundle_object(&retry_orphan, "bundle-object:orphan-gc-retry")
        .await;
    sqlx::query("UPDATE runtime_objects SET ready_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 2 HOUR) WHERE tenant_id=? AND object_id=?")
        .bind(fixture.tenant_id)
        .bind(retry_receipt.object.object_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    let failing = Arc::new(FailOnceDeleteStore::new(fixture.state.objects.clone()));
    let failing_state = RuntimeState {
        pool: fixture.state.pool.clone(),
        objects: failing,
        trust: fixture.state.trust.clone(),
        wakeups: Default::default(),
        vault: None,
    };
    let retry_run = Uuid::now_v7();
    assert_eq!(
        mark_collectable(&failing_state, retry_run).await.unwrap(),
        1
    );
    assert!(
        sweep_one(&failing_state, retry_run, Uuid::now_v7())
            .await
            .unwrap()
    );
    let failed: String =
        sqlx::query_scalar("SELECT status FROM bundle_gc_items WHERE gc_run_id=? AND object_id=?")
            .bind(retry_run)
            .bind(retry_receipt.object.object_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(failed, "failed");
    assert!(
        sweep_one(&failing_state, retry_run, Uuid::now_v7())
            .await
            .unwrap()
    );
    let retried: String =
        sqlx::query_scalar("SELECT status FROM runtime_objects WHERE object_id=?")
            .bind(retry_receipt.object.object_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(retried, "deleted");

    let bound = fixture.bundle(61).await;
    fixture
        .upload_bundle_object(&bound, "bundle-object:bound-not-orphan")
        .await;
    prepare(fixture, &bound, "prepare:bound-not-orphan").await;
    sqlx::query(
        "UPDATE runtime_objects SET ready_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 2 HOUR) WHERE tenant_id=? AND object_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(bound.payload.objects[0].object_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(
        mark_collectable(&fixture.state, Uuid::now_v7())
            .await
            .unwrap(),
        0
    );
}

async fn disable_is_scoped_idempotent_and_preserves_the_head(
    fixture: &Fixture,
    active: &agentx_runtime_contracts::ExecutionSpecBundleV2,
) {
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM publish_receipts")
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    let wrong_scope = disable_deployment(
        State(fixture.state.clone()),
        publisher_headers("runtime.deployments.activate"),
        Json(DisableDeploymentRequestV1 {
            api_version: 1,
            idempotency_key: "disable:wrong-scope".into(),
            tenant_id: fixture.tenant_id,
            application_id: fixture.application_id,
            admission_epoch: 3,
        }),
    )
    .await;
    assert!(matches!(wrong_scope, Err(RuntimeError::Unauthorized)));
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM publish_receipts")
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(after, before);

    apply_api_key(fixture, 3, AdmissionStatusV1::Active).await;
    let request = DisableDeploymentRequestV1 {
        api_version: 1,
        idempotency_key: "disable:accepted".into(),
        tenant_id: fixture.tenant_id,
        application_id: fixture.application_id,
        admission_epoch: 4,
    };
    let receipt = disable_deployment(
        State(fixture.state.clone()),
        publisher_headers("runtime.deployments.disable"),
        Json(request.clone()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(receipt.status, PublishReceiptStatusV1::Accepted);
    let replay = disable_deployment(
        State(fixture.state.clone()),
        publisher_headers("runtime.deployments.disable"),
        Json(request),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(replay.bundle_id, receipt.bundle_id);

    let head: Uuid = sqlx::query_scalar(
        "SELECT bundle_id FROM deployment_heads WHERE tenant_id=? AND application_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.application_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(head, active.payload.bundle_id);
    let route: String = sqlx::query_scalar(
        "SELECT status FROM application_routes WHERE tenant_id=? AND application_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.application_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(route, "disabled");
    let retained: bool = sqlx::query_scalar(
        "SELECT status='disabled' AND retained_until>=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 13 DAY) FROM deployment_bundles WHERE id=?",
    )
    .bind(active.payload.bundle_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert!(retained);
    assert!(matches!(
        create_invocation(
            &fixture.state.pool,
            fixture.tenant_id,
            fixture.application_id,
            fixture.key_id,
            &InvocationRequestV1 {
                input: json!({"message":"disabled"}),
                idempotency_key: "invocation:after-disable".into(),
            },
        )
        .await,
        Err(RuntimeError::Unauthorized | RuntimeError::NotFound)
    ));

    let stale = disable_deployment(
        State(fixture.state.clone()),
        publisher_headers("runtime.deployments.disable"),
        Json(DisableDeploymentRequestV1 {
            api_version: 1,
            idempotency_key: "disable:stale-epoch".into(),
            tenant_id: fixture.tenant_id,
            application_id: fixture.application_id,
            admission_epoch: 4,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(stale.status, PublishReceiptStatusV1::Rejected);
    let rejection = stale.rejection.unwrap();
    assert!(matches!(
        rejection.code,
        agentx_runtime_contracts::RuntimePublishErrorCodeV1::ActivationSequenceConflict
    ));
    let persisted: String = sqlx::query_scalar("SELECT status FROM publish_receipts WHERE tenant_id=? AND operation='disable' AND idempotency_key='disable:stale-epoch'")
        .bind(fixture.tenant_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(persisted, "rejected");
}

#[derive(Debug)]
struct FailOnceDeleteStore {
    inner: Arc<dyn ObjectStore>,
    failed: AtomicBool,
}

impl FailOnceDeleteStore {
    fn new(inner: Arc<dyn ObjectStore>) -> Self {
        Self {
            inner,
            failed: AtomicBool::new(false),
        }
    }
}

impl fmt::Display for FailOnceDeleteStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("fail-once-delete")
    }
}

#[async_trait::async_trait]
impl ObjectStore for FailOnceDeleteStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        options: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.inner.put_opts(location, payload, options).await
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjectPath,
        options: PutMultipartOpts,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, options).await
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.inner.get_opts(location, options).await
    }

    async fn delete(&self, location: &ObjectPath) -> object_store::Result<()> {
        if !self.failed.swap(true, Ordering::SeqCst) {
            return Err(object_store::Error::Generic {
                store: "fail-once-delete",
                source: "injected delete failure".into(),
            });
        }
        self.inner.delete(location).await
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> futures::stream::BoxStream<'_, object_store::Result<ObjectMeta>> {
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> object_store::Result<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy(&self, from: &ObjectPath, to: &ObjectPath) -> object_store::Result<()> {
        self.inner.copy(from, to).await
    }

    async fn copy_if_not_exists(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
    ) -> object_store::Result<()> {
        self.inner.copy_if_not_exists(from, to).await
    }
}

fn publisher_headers(scope: &str) -> HeaderMap {
    let now = now_unix();
    let token = issue_service_token(
        "publisher-current",
        PRIVATE_KEY,
        &ServiceClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: "publisher-test".into(),
            role: ControlRole::Publisher,
            scope: BTreeSet::from([scope.into()]),
            iat: now,
            exp: now + 300,
            jti: Uuid::now_v7(),
        },
    )
    .unwrap();
    bearer_headers(token)
}

fn delegation_headers(
    tenant_id: Uuid,
    subject_id: Uuid,
    scope: BTreeSet<String>,
    application_ids: BTreeSet<Uuid>,
    execution_ids: BTreeSet<Uuid>,
    request_hash: agentx_runtime_contracts::ContentHash,
) -> HeaderMap {
    let now = now_unix();
    bearer_headers(
        issue_delegation_token(
            "publisher-current",
            PRIVATE_KEY,
            &DelegationClaimsV1 {
                iss: "agentx-control".into(),
                aud: "agentx-runtime-internal".into(),
                sub: subject_id,
                tenant_id,
                token_version: 1,
                tenant_wide: false,
                scope,
                application_ids,
                workflow_ids: BTreeSet::new(),
                execution_ids,
                session_ids: BTreeSet::new(),
                request_hash,
                iat: now,
                exp: now + 60,
                jti: Uuid::now_v7(),
            },
        )
        .unwrap(),
    )
}

fn bearer_headers(token: String) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    headers
}

fn definition() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"],"additionalProperties":false},"contexts":{}},
        "nodes":[{"id":"pass","key":"pass","type":"set","typeVersion":1,"name":"Pass","parameters":{},"settings":{"retryOnFail":true,"maxTries":2,"waitBetweenTriesMs":5},"contextWrites":[],"resourceReferences":[]},{"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","disabled":false,"protected":true,"parameters":{"outputs":{"message":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"pass","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":["message"]},"missingPolicy":{"kind":"error"}}},"errorOutputs":{}},"contextWrites":[],"resourceReferences":[],"settings":{}}],
        "connections":[
            {"id":"start-pass","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"pass","targetHandle":"main","order":0},
            {"id":"pass-end","sourceNodeId":"pass","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{"message":{"schema":{"type":"string"},"required":true}}},
        "settings":{"activationBudget":20,"executionOrder":"deterministic"}
    }))
    .unwrap()
}

fn composite_definition(child_version_id: Uuid) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"],"additionalProperties":false},"contexts":{}},
        "nodes":[{
            "id":"child",
            "key":"child",
            "type":"sub_workflow",
            "typeVersion":1,
            "name":"Child",
            "parameters":{"workflowVersionId":child_version_id,"inputs":{"kind":"object","fields":{"message":{"kind":"reference","selector":{"namespace":"inputs","run":{"kind":"current"},"item":{"kind":"current"},"path":["message"]},"missingPolicy":{"kind":"error"}}}}},

            "contextWrites":[],
            "resourceReferences":[]
        },{
            "id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","disabled":false,"protected":true,
            "parameters":{"outputs":{"message":{"kind":"reference","selector":{"namespace":"outputs","sourceNodeId":"child","port":"main","run":{"kind":"current"},"item":{"kind":"current"},"path":["message"]},"missingPolicy":{"kind":"error"}}},"errorOutputs":{}},
            "contextWrites":[],"resourceReferences":[],"settings":{}
        }],
        "connections":[
            {"id":"start-child","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"child","targetHandle":"main","order":0},
            {"id":"child-end","sourceNodeId":"child","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{"message":{"schema":{"type":"string"},"required":true}}},
        "settings":{"activationBudget":20,"executionOrder":"deterministic"}
    }))
    .unwrap()
}

async fn connect_with_retry(port: u16) -> MySqlPool {
    let settings = agentx_runtime_infrastructure::RuntimeMySqlSettings {
        host: "127.0.0.1".into(), port, database: "agentx_runtime".into(),
        username: "agentx".into(), password: "agentx-test-password".into(), max_connections: 64,
        tls_mode: agentx_runtime_infrastructure::MySqlTlsMode::Disabled,
        tls_ca_path: None, tls_client_cert_path: None, tls_client_key_path: None,
    };
    let mut last_error = None;
    for _ in 0..40 {
        match agentx_runtime_infrastructure::connect_runtime_mysql(&settings).await
        {
            Ok(pool) => return pool,
            Err(error) => last_error = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("connect to Runtime MySQL: {last_error:?}");
}
