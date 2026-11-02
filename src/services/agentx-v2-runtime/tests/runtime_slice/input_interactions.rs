async fn raw_input_waits_are_durable_idempotent_and_use_the_same_execution(fixture: &Fixture) {
    use agentx_v2_runtime::input_waits;
    let execution = start_input_interaction(fixture, None, json!({})).await;
    let waits = input_waits::list(
        &fixture.state.pool,
        fixture.tenant_id,
        None,
        Some(execution),
        true,
    )
    .await
    .unwrap();
    assert_eq!(waits.len(), 1);
    let wait = waits[0].wait_id;
    let active_workers:i64=sqlx::query_scalar("SELECT COUNT(*) FROM node_attempts WHERE execution_id=? AND status IN ('queued','running')")
        .bind(execution).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(active_workers, 0, "suspension must release worker capacity");
    let content = json!({"asr":"我再考虑一下","confirmed":false});
    let first = input_waits::submit(
        &fixture.state.pool,
        fixture.tenant_id,
        wait,
        "asr-1",
        &content,
        "api",
    );
    let duplicate = input_waits::submit(
        &fixture.state.pool,
        fixture.tenant_id,
        wait,
        "asr-1",
        &content,
        "api",
    );
    let (first, duplicate) = tokio::join!(first, duplicate);
    let first = first.unwrap();
    assert_eq!(first.execution_id, execution);
    assert_eq!(first.receipt_id, duplicate.unwrap().receipt_id);
    assert!(matches!(
        input_waits::submit(
            &fixture.state.pool,
            fixture.tenant_id,
            wait,
            "other",
            &json!("确认"),
            "api"
        )
        .await,
        Err(RuntimeError::StateConflict("WAIT_ALREADY_FINISHED", _))
    ));
    assert!(matches!(
        input_waits::submit(
            &fixture.state.pool,
            fixture.tenant_id,
            wait,
            "asr-1",
            &json!("changed"),
            "api"
        )
        .await,
        Err(RuntimeError::Conflict(..))
    ));
    assert!(matches!(
        input_waits::submit(
            &fixture.state.pool,
            Uuid::now_v7(),
            wait,
            "asr-1",
            &content,
            "api"
        )
        .await,
        Err(RuntimeError::NotFound)
    ));
    recover_dispatches(&fixture.state.pool, 100).await.unwrap();
    process_execution_commands(fixture, execution).await;
    let status: String = sqlx::query_scalar("SELECT status FROM workflow_executions WHERE id=?")
        .bind(execution)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "succeeded");
    let row = sqlx::query("SELECT content_json,resume_status FROM input_waits WHERE id=?")
        .bind(wait)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(row.try_get::<Value, _>("content_json").unwrap(), content);
    assert_eq!(
        row.try_get::<String, _>("resume_status").unwrap(),
        "succeeded"
    );
    assert_eq!(
        input_waits::submit(
            &fixture.state.pool,
            fixture.tenant_id,
            wait,
            "asr-1",
            &content,
            "api"
        )
        .await
        .unwrap()
        .receipt_id,
        first.receipt_id
    );

    // A child that reaches suspension after its parent has ended must not
    // reopen a channel slot or publish a fresh input-required notification.
    let late_child = prepare_input_interaction(fixture, None, json!({})).await;
    sqlx::query("UPDATE workflow_executions SET parent_execution_id=?,parent_node_execution_id=? WHERE id=?")
        .bind(execution).bind(waits[0].node_execution_id).bind(late_child)
        .execute(&fixture.state.pool).await.unwrap();
    process_execution_commands(fixture, late_child).await;
    let late_waits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM input_waits WHERE execution_id=?")
            .bind(late_child)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(
        late_waits, 0,
        "an ended parent must not acquire another input wait"
    );
    let late_error: String = sqlx::query_scalar(
        "SELECT error_code FROM node_executions WHERE execution_id=? AND node_id='ask'",
    )
    .bind(late_child)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(late_error, "WAIT_EXECUTION_FINISHED");

    let timed = start_input_interaction(fixture, None, json!({"timeoutMs":1000})).await;
    sqlx::query("UPDATE input_waits SET deadline_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE execution_id=?").bind(timed).execute(&fixture.state.pool).await.unwrap();
    assert_eq!(
        input_waits::enqueue_due(&fixture.state.pool, 100)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        input_waits::enqueue_due(&fixture.state.pool, 100)
            .await
            .unwrap(),
        0
    );
    process_execution_commands(fixture, timed).await;
    let timed_waits = input_waits::list(
        &fixture.state.pool,
        fixture.tenant_id,
        None,
        Some(timed),
        false,
    )
    .await
    .unwrap();
    assert_eq!(timed_waits[0].status, "timed_out");
    assert!(
        input_waits::submit(
            &fixture.state.pool,
            fixture.tenant_id,
            timed_waits[0].wait_id,
            "late",
            &json!("确认"),
            "api"
        )
        .await
        .is_err()
    );

    let cancelled = start_input_interaction(fixture, None, json!({})).await;
    let command = Uuid::now_v7();
    sqlx::query("INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'cancel_execution','execution',?,?,JSON_OBJECT(),'pending')")
        .bind(command).bind(fixture.tenant_id).bind(cancelled.to_string()).bind(format!("cancel:{cancelled}")).execute(&fixture.state.pool).await.unwrap();
    process_execution_commands(fixture, cancelled).await;
    assert!(
        input_waits::list(
            &fixture.state.pool,
            fixture.tenant_id,
            None,
            Some(cancelled),
            true
        )
        .await
        .unwrap()
        .is_empty()
    );
    let cancelled_waits = input_waits::list(
        &fixture.state.pool,
        fixture.tenant_id,
        None,
        Some(cancelled),
        false,
    )
    .await
    .unwrap();
    assert_eq!(cancelled_waits[0].status, "cancelled");

    let preset = start_input_interaction(
        fixture,
        Some(json!({"/ask":["不是确认"]})),
        json!({"hookUrl":{"kind":"literal","value":"https://customer.example/hook"}}),
    )
    .await;
    process_execution_commands(fixture, preset).await;
    let injected = input_waits::list(
        &fixture.state.pool,
        fixture.tenant_id,
        None,
        Some(preset),
        false,
    )
    .await
    .unwrap();
    assert_eq!(injected[0].content, Some(json!("不是确认")));
    assert_eq!(injected[0].source.as_deref(), Some("preset"));
    let hooks: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_outbox WHERE execution_id=? AND provider='input_hook'",
    )
    .bind(preset)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(hooks, 0);
    let exhausted = start_input_interaction(fixture, Some(json!({})), json!({})).await;
    let error: String = sqlx::query_scalar(
        "SELECT error_code FROM node_executions WHERE execution_id=? AND node_id='ask'",
    )
    .bind(exhausted)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(error, "WAIT_INPUT_EXHAUSTED");
    structured_input_contract_is_frozen_and_invalid_content_does_not_consume_the_wait(fixture).await;
    resumed_input_is_visible_to_an_immediately_following_approval(fixture).await;
}

async fn resumed_input_is_visible_to_an_immediately_following_approval(fixture: &Fixture) {
    use agentx_v2_runtime::input_waits;
    let selector = json!({"namespace":"outputs","sourceNodeId":"ask","port":"main","run":{"kind":"current"},"item":{"kind":"first"},"path":["content","purpose"]});
    let definition = serde_json::from_value(json!({
        "schemaVersion":"8.0","start":{"inputs":{"type":"object"},"contexts":{}},
        "nodes":[
            {"id":"ask","key":"ask","type":"await_input","typeVersion":1,"name":"Details","parameters":{},"contextWrites":[],"resourceReferences":[],"settings":{}},
            {"id":"review","key":"review","type":"approval","typeVersion":1,"name":"Review","parameters":{
                "candidateUserId":fixture.identity_id,"timeoutMs":300000,
                "title":{"kind":"template","segments":[{"kind":"text","text":"Review: "},{"kind":"reference","selector":selector,"missingPolicy":{"kind":"error"}}]}
            },"contextWrites":[],"resourceReferences":[],"settings":{}},
            {"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","protected":true,"parameters":{"outputs":{},"errorOutputs":{}},"contextWrites":[],"resourceReferences":[],"settings":{}}
        ],
        "connections":[
            {"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"ask","targetHandle":"main","order":0},
            {"id":"details-review","sourceNodeId":"ask","sourceHandle":"main","targetNodeId":"review","targetHandle":"main","order":0},
            {"id":"approved","sourceNodeId":"review","sourceHandle":"decision:approved","targetNodeId":"exit","targetHandle":"main","order":0}
        ],"end":{"outputs":{}},"settings":{"activationBudget":20,"executionOrder":"deterministic"}
    })).unwrap();
    let execution = prepare_input_definition(fixture, None, definition).await;
    process_execution_commands(fixture, execution).await;
    let waits = input_waits::list(&fixture.state.pool, fixture.tenant_id, None, Some(execution), true).await.unwrap();
    assert_eq!(waits.len(), 1);
    input_waits::submit(&fixture.state.pool, fixture.tenant_id, waits[0].wait_id, "details-before-approval", &json!({"purpose":"Customer visit"}), "api").await.unwrap();
    process_execution_commands(fixture, execution).await;
    let status:String=sqlx::query_scalar("SELECT status FROM workflow_executions WHERE id=?").bind(execution).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(status, "waiting", "the next suspended node must read the resumed output in the same transaction");
    let approvals:Vec<(String,String)>=sqlx::query_as("SELECT title,status FROM approval_tasks WHERE execution_id=?").bind(execution).fetch_all(&fixture.state.pool).await.unwrap();
    assert_eq!(approvals,vec![("Review: Customer visit".into(),"pending".into())]);
    let command = Uuid::now_v7();
    sqlx::query("INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'cancel_execution','execution',?,?,JSON_OBJECT(),'pending')")
        .bind(command).bind(fixture.tenant_id).bind(execution.to_string()).bind(format!("cancel:{execution}")).execute(&fixture.state.pool).await.unwrap();
    process_execution_commands(fixture, execution).await;
}

async fn structured_input_contract_is_frozen_and_invalid_content_does_not_consume_the_wait(fixture: &Fixture) {
    use agentx_v2_runtime::input_waits;
    let schema = json!({"type":"object","required":["deliveryDate","contact"],"properties":{"deliveryDate":{"type":"string","format":"date"},"contact":{"type":"string","minLength":1}},"additionalProperties":false});
    let execution = start_input_interaction(fixture, None, json!({"responseSchema":schema})).await;
    let waits = input_waits::list(&fixture.state.pool, fixture.tenant_id, None, Some(execution), true).await.unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].response_schema, Some(schema.clone()));
    let rejected = input_waits::submit(&fixture.state.pool, fixture.tenant_id, waits[0].wait_id, "delivery-1", &json!({"deliveryDate":"2026-02-30"}), "api").await;
    assert!(matches!(rejected, Err(RuntimeError::Validation { code:"WAIT_INPUT_SCHEMA_VALIDATION_FAILED", .. })));
    let row = sqlx::query("SELECT status,input_key,receipt_id FROM input_waits WHERE id=?").bind(waits[0].wait_id).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(row.try_get::<String,_>("status").unwrap(), "waiting");
    assert!(row.try_get::<Option<String>,_>("input_key").unwrap().is_none());
    assert!(row.try_get::<Option<Uuid>,_>("receipt_id").unwrap().is_none());
    let content = json!({"deliveryDate":"2026-11-02","contact":"Li"});
    let accepted = input_waits::submit(&fixture.state.pool, fixture.tenant_id, waits[0].wait_id, "delivery-1", &content, "api").await.unwrap();
    assert_eq!(accepted.execution_id, execution);
    process_execution_commands(fixture, execution).await;
    let completed = input_waits::list(&fixture.state.pool, fixture.tenant_id, None, Some(execution), false).await.unwrap();
    assert_eq!(completed[0].response_schema, Some(schema.clone()));
    assert_eq!(completed[0].content, Some(content));
    let invalid_preset = start_input_interaction(fixture, Some(json!({"/ask":[{"deliveryDate":"bad"}]})), json!({"responseSchema":schema})).await;
    let error:String=sqlx::query_scalar("SELECT error_code FROM node_executions WHERE execution_id=? AND node_id='ask'").bind(invalid_preset).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(error,"WAIT_INPUT_SCHEMA_VALIDATION_FAILED");
    let open:i64=sqlx::query_scalar("SELECT COUNT(*) FROM input_waits WHERE execution_id=? AND status='waiting'").bind(invalid_preset).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(open,0,"invalid presets must not leave an orphaned active wait");
}

async fn start_input_interaction(
    fixture: &Fixture,
    preset: Option<Value>,
    parameters: Value,
) -> Uuid {
    let execution = prepare_input_interaction(fixture, preset, parameters).await;
    process_execution_commands(fixture, execution).await;
    execution
}

async fn prepare_input_interaction(
    fixture: &Fixture,
    preset: Option<Value>,
    parameters: Value,
) -> Uuid {
    let definition=serde_json::from_value(json!({
        "schemaVersion":"8.0","start":{"inputs":{"type":"object","additionalProperties":true},"contexts":{}},
        "nodes":[{"id":"ask","key":"ask","type":"await_input","typeVersion":1,"name":"Wait","parameters":parameters,"contextWrites":[],"resourceReferences":[],"settings":{}},
        {"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","protected":true,"parameters":{"outputs":{},"errorOutputs":{}},"contextWrites":[],"resourceReferences":[],"settings":{}}],
        "connections":[{"id":"start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"ask","targetHandle":"main","order":0},
        {"id":"received","sourceNodeId":"ask","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0},
        {"id":"timeout","sourceNodeId":"ask","sourceHandle":"timed_out","targetNodeId":"exit","targetHandle":"main","order":1}],
        "end":{"outputs":{}},"settings":{"activationBudget":20,"executionOrder":"deterministic"}
    })).unwrap();
    prepare_input_definition(fixture, preset, definition).await
}

async fn prepare_input_definition(
    fixture: &Fixture,
    preset: Option<Value>,
    definition: WorkflowDefinition,
) -> Uuid {
    let now = OffsetDateTime::now_utc();
    let package_id = Uuid::now_v7();
    let mut source = fixture.work_package_source(package_id, now, now + time::Duration::hours(1));
    source.definition = definition;
    let compiled = compile_workflow_version(&source.definition, package_id).unwrap();
    source.spec = agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Debug {
        source: agentx_runtime_contracts::WorkflowDesignSourceV1::DraftRevision {
            draft_id: Uuid::now_v7(),
            revision_id: Uuid::now_v7(),
            revision: 1,
        },
        debug_plan: agentx_runtime_contracts::RuntimeDebugPlanV1::whole(&compiled),
    };
    source.overlay.wait_inputs =
        serde_json::from_value(preset.clone().unwrap_or_else(|| json!({}))).unwrap();
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
            idempotency_key: format!("input:prepare:{package_id}"),
            work_package: package,
        }),
    )
    .await
    .unwrap();
    let receipt = execute_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(ExecuteWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("input:execute:{package_id}"),
            package_id,
            input: json!({}),
        }),
    )
    .await
    .unwrap()
    .0;
    let execution: Uuid = serde_json::from_value(receipt.result["executionId"].clone()).unwrap();
    if preset.is_none() {
        // The external admission variant is exercised through real API keys in Kubernetes E2E.
        sqlx::query("UPDATE execution_snapshots SET runtime_settings_json=JSON_SET(runtime_settings_json,'$.waitInputMode','external') WHERE execution_id=?").bind(execution).execute(&fixture.state.pool).await.unwrap();
    }
    execution
}
