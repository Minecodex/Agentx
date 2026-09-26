async fn fork_uses_checkpoint_machine_and_preserves_source(
    fixture: &Fixture,
    source_execution_id: Uuid,
) {
    let checkpoint_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM checkpoints WHERE tenant_id=? AND execution_id=? ORDER BY sequence_number DESC LIMIT 1",
    )
    .bind(fixture.tenant_id)
    .bind(source_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let source_version: u64 = sqlx::query_scalar(
        "SELECT state_version FROM workflow_executions WHERE tenant_id=? AND id=?",
    )
    .bind(fixture.tenant_id)
    .bind(source_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let command_id = Uuid::now_v7();
    let fork_user_id = Uuid::now_v7();
    let fork_department_id = Uuid::now_v7();
    let _ = apply_runtime_command(
        State(fixture.state.clone()),
        publisher_headers("runtime.commands.apply"),
        Json(agentx_runtime_contracts::RuntimeCommandApplyRequestV1 {
            api_version: 1,
            tenant_id: fixture.tenant_id,
            command_id,
            object_version: source_version,
            idempotency_key: format!("fork:{source_execution_id}:pass"),
            command: agentx_runtime_contracts::ExecutionCommandV1::Fork {
                source_execution_id,
                checkpoint_id,
                origin: agentx_runtime_contracts::ExecutionOriginV1 {
                    initiator_user_id: Some(fork_user_id),
                    initiator_user_name: Some("Fork Operator".into()),
                    initiator_department_id: Some(fork_department_id),
                    initiator_department_name: Some("Fork Department".into()),
                    trigger_source_id: Some(source_execution_id),
                    trigger_name: Some("Fork from checkpoint".into()),
                    role_assignments: vec![],
                },
                mode: agentx_runtime_contracts::PartialExecutionModeV1::Node,
                node_id: Some("pass".into()),
                side_effect_resolution: SideEffectResolutionV1::ReuseOutput,
            },
        }),
    )
    .await
    .unwrap();
    let owner = Uuid::now_v7();
    let fork_command = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|command| command.command_id == command_id)
        .unwrap();
    process_command(&fixture.state.pool, &fork_command)
        .await
        .unwrap();
    let fork_execution_id: Uuid = sqlx::query_scalar(
        "SELECT fork_execution_id FROM execution_forks WHERE tenant_id=? AND source_execution_id=? AND source_checkpoint_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(source_execution_id)
    .bind(checkpoint_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let fork_origin = sqlx::query("SELECT trigger_type,initiator_user_id,initiator_user_name,initiator_department_id,initiator_department_name,trigger_source_id,trigger_name FROM workflow_executions WHERE tenant_id=? AND id=?")
        .bind(fixture.tenant_id)
        .bind(fork_execution_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(fork_origin.get::<String, _>("trigger_type"), "fork");
    assert_eq!(
        fork_origin.get::<Uuid, _>("initiator_user_id"),
        fork_user_id
    );
    assert_eq!(
        fork_origin.get::<String, _>("initiator_user_name"),
        "Fork Operator"
    );
    assert_eq!(
        fork_origin.get::<Uuid, _>("initiator_department_id"),
        fork_department_id
    );
    assert_eq!(
        fork_origin.get::<String, _>("initiator_department_name"),
        "Fork Department"
    );
    assert_eq!(
        fork_origin.get::<Uuid, _>("trigger_source_id"),
        source_execution_id
    );
    assert_eq!(
        fork_origin.get::<String, _>("trigger_name"),
        "Fork from checkpoint"
    );
    let start = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|command| command.execution_id == fork_execution_id)
        .unwrap();
    process_command(&fixture.state.pool, &start).await.unwrap();
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
    agentx_v2_runtime::engine::submit_worker_result(
        &fixture.state.pool,
        &successful_worker_result(&claim),
    )
    .await
    .unwrap();
    let fork_result: Value = sqlx::query_scalar(
        "SELECT terminal_result_json FROM workflow_executions WHERE tenant_id=? AND id=?",
    )
    .bind(fixture.tenant_id)
    .bind(fork_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(fork_result["message"], "agentx-v2");
    let source_status: String =
        sqlx::query_scalar("SELECT status FROM workflow_executions WHERE tenant_id=? AND id=?")
            .bind(fixture.tenant_id)
            .bind(source_execution_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(source_status, "succeeded");

    let mut checkpoint_payload: Value =
        sqlx::query_scalar("SELECT payload_json FROM checkpoints WHERE tenant_id=? AND id=?")
            .bind(fixture.tenant_id)
            .bind(checkpoint_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    checkpoint_payload["machine"]["workflow"]["nodes"][0]["sideEffectLevel"] =
        json!("irreversible");
    let checkpoint_hash = agentx_runtime_contracts::content_hash(&checkpoint_payload).unwrap();
    sqlx::query(
        "UPDATE checkpoints SET payload_json=?,payload_hash=?,state_hash=? WHERE tenant_id=? AND id=?",
    )
    .bind(&checkpoint_payload)
    .bind(checkpoint_hash.as_str())
    .bind(checkpoint_hash.as_str())
    .bind(fixture.tenant_id)
    .bind(checkpoint_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();

    let dry_run_command_id = Uuid::now_v7();
    let _ = apply_runtime_command(
        State(fixture.state.clone()),
        publisher_headers("runtime.commands.apply"),
        Json(agentx_runtime_contracts::RuntimeCommandApplyRequestV1 {
            api_version: 1,
            tenant_id: fixture.tenant_id,
            command_id: dry_run_command_id,
            object_version: source_version,
            idempotency_key: format!("fork:{source_execution_id}:dry-run"),
            command: agentx_runtime_contracts::ExecutionCommandV1::Fork {
                source_execution_id,
                checkpoint_id,
                origin: agentx_runtime_contracts::ExecutionOriginV1::system(Some(
                    source_execution_id,
                )),
                mode: agentx_runtime_contracts::PartialExecutionModeV1::Whole,
                node_id: None,
                side_effect_resolution: SideEffectResolutionV1::DryRun,
            },
        }),
    )
    .await
    .unwrap();
    let fork_command = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|command| command.command_id == dry_run_command_id)
        .unwrap();
    process_command(&fixture.state.pool, &fork_command)
        .await
        .unwrap();
    let dry_run_execution_id: Uuid = sqlx::query_scalar(
        "SELECT fork_execution_id FROM execution_forks WHERE tenant_id=? AND idempotency_key=?",
    )
    .bind(fixture.tenant_id)
    .bind(format!("runtime-command:{dry_run_command_id}"))
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let start = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|command| command.execution_id == dry_run_execution_id)
        .unwrap();
    process_command(&fixture.state.pool, &start).await.unwrap();
    let dry_run_result: Value = sqlx::query_scalar(
        "SELECT terminal_result_json FROM workflow_executions WHERE tenant_id=? AND id=? AND status='succeeded'",
    )
    .bind(fixture.tenant_id)
    .bind(dry_run_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(dry_run_result["message"], "agentx-v2");
    let dry_run_dispatches: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_outbox WHERE tenant_id=? AND execution_id=? AND message_type='dispatch_node'",
    )
    .bind(fixture.tenant_id)
    .bind(dry_run_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(
        dry_run_dispatches, 0,
        "dry-run Fork must not dispatch the side-effect node"
    );
}

async fn composite_child_uses_immutable_runtime_snapshot_and_merges_on_success(fixture: &Fixture) {
    let now = OffsetDateTime::now_utc();
    let package_id = Uuid::now_v7();
    let child_version_id = Uuid::now_v7();
    let child_definition = definition();
    let child_ir = compile_workflow_version(&child_definition, child_version_id).unwrap();
    let definition_bytes = agentx_runtime_contracts::canonical_bytes(&child_definition).unwrap();
    let ir_bytes = agentx_runtime_contracts::canonical_bytes(&child_ir).unwrap();
    let definition_hash = agentx_runtime_contracts::content_hash(&child_definition).unwrap();
    let ir_hash = agentx_runtime_contracts::content_hash(&child_ir).unwrap();
    let ir_object_id = composite_ir_object_id(child_version_id);
    let definition_object = RuntimeObjectReferenceV1 {
        tenant_id: fixture.tenant_id,
        storage_domain: StorageDomain::Runtime,
        object_id: child_version_id,
        object_key: RuntimeObjectReferenceV1::canonical_key(
            fixture.tenant_id,
            child_version_id,
            &definition_hash,
        ),
        content_hash: definition_hash,
        size_bytes: definition_bytes.len() as u64,
        media_type: "application/vnd.agentx.workflow-definition+json".into(),
    };
    let ir_object = RuntimeObjectReferenceV1 {
        tenant_id: fixture.tenant_id,
        storage_domain: StorageDomain::Runtime,
        object_id: ir_object_id,
        object_key: RuntimeObjectReferenceV1::canonical_key(
            fixture.tenant_id,
            ir_object_id,
            &ir_hash,
        ),
        content_hash: ir_hash,
        size_bytes: ir_bytes.len() as u64,
        media_type: "application/vnd.agentx.compiled-workflow.v1+json".into(),
    };
    for (object, bytes) in [
        (definition_object.clone(), definition_bytes),
        (ir_object.clone(), ir_bytes),
    ] {
        persist_upload(
            &fixture.state,
            RuntimeObjectUploadMetadataV1 {
                api_version: 1,
                idempotency_key: format!("composite:upload:{}", object.object_id),
                tenant_id: object.tenant_id,
                object_id: object.object_id,
                content_hash: object.content_hash.clone(),
                size_bytes: object.size_bytes,
                media_type: object.media_type.clone(),
            },
            Bytes::from(bytes),
        )
        .await
        .unwrap();
    }
    let configuration = agentx_runtime_contracts::RuntimeResourceConfigurationV1::Composite {
        workflow: agentx_runtime_contracts::ExecutionWorkflowSnapshotV1 {
            id: Uuid::now_v7(),
            name: "Composite Child".into(),
            version_id: child_version_id,
            version_number: 3,
            owner_department: None,
        },
        definition_object_id: child_version_id,
        ir_object_id,
    };
    let mut source = fixture.work_package_source(package_id, now, now + time::Duration::hours(1));
    let root_user_id = Uuid::now_v7();
    let root_department_id = Uuid::now_v7();
    source.origin = agentx_runtime_contracts::ExecutionOriginV1 {
        initiator_user_id: Some(root_user_id),
        initiator_user_name: Some("Composite Root User".into()),
        initiator_department_id: Some(root_department_id),
        initiator_department_name: Some("Composite Root Department".into()),
        trigger_source_id: None,
        trigger_name: None,
        role_assignments: vec![],
    };
    source.definition = composite_definition(child_version_id);
    source
        .dependency_versions
        .insert(child_version_id, child_definition);
    let compiled = agentx_bundle_builder::compile_workflow_version_with_dependencies(
        &source.definition,
        package_id,
        &source.dependency_versions,
    )
    .unwrap();
    source.spec = agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Debug {
        draft_revision: 1,
        debug_plan: agentx_runtime_contracts::RuntimeDebugPlanV1::whole(&compiled),
    };
    source.objects = vec![definition_object, ir_object];
    source.resources = vec![agentx_runtime_contracts::RuntimeResourceBindingV1 {
        resource_kind: agentx_runtime_contracts::RuntimeResourceKindV1::Composite,
        resource_id: child_version_id,
        resource_version: child_version_id.to_string(),
        state_epoch: 1,
        content_hash: agentx_runtime_contracts::content_hash(&configuration).unwrap(),
        configuration,
        object_ids: vec![child_version_id, ir_object_id],
    }];
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
            idempotency_key: format!("composite:prepare:{package_id}"),
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
            idempotency_key: format!("composite:execute:{package_id}"),
            package_id,
            input: json!({"message":"composite-v2"}),
        }),
    )
    .await
    .unwrap()
    .0;
    let parent_execution_id =
        Uuid::parse_str(started.result["executionId"].as_str().unwrap()).unwrap();
    let owner = Uuid::now_v7();
    let parent_start = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.execution_id == parent_execution_id)
        .unwrap();
    process_command(&fixture.state.pool, &parent_start)
        .await
        .unwrap();
    let child_execution_id: Uuid = sqlx::query_scalar(
        "SELECT child_execution_id FROM execution_children WHERE tenant_id=? AND parent_execution_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(parent_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let child_origin = sqlx::query("SELECT e.trigger_type,e.initiator_user_id,e.initiator_user_name,e.initiator_department_id,e.initiator_department_name,e.trigger_source_id,e.trigger_name,c.parent_node_execution_id,s.execution_context_json FROM workflow_executions e JOIN execution_children c ON c.tenant_id=e.tenant_id AND c.child_execution_id=e.id JOIN execution_snapshots s ON s.tenant_id=e.tenant_id AND s.execution_id=e.id WHERE e.tenant_id=? AND e.id=?")
        .bind(fixture.tenant_id)
        .bind(child_execution_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(child_origin.get::<String, _>("trigger_type"), "composite");
    assert_eq!(
        child_origin.get::<Uuid, _>("initiator_user_id"),
        root_user_id
    );
    assert_eq!(
        child_origin.get::<String, _>("initiator_user_name"),
        "Composite Root User"
    );
    assert_eq!(
        child_origin.get::<Uuid, _>("initiator_department_id"),
        root_department_id
    );
    assert_eq!(
        child_origin.get::<String, _>("initiator_department_name"),
        "Composite Root Department"
    );
    assert_eq!(
        child_origin.get::<Uuid, _>("trigger_source_id"),
        child_origin.get::<Uuid, _>("parent_node_execution_id")
    );
    assert!(!child_origin.get::<String, _>("trigger_name").is_empty());
    let child_context: Value = child_origin.get("execution_context_json");
    assert_eq!(
        child_context.pointer("/workflow/name"),
        Some(&json!("Composite Child"))
    );
    assert_eq!(
        child_context.pointer("/workflow/versionNumber"),
        Some(&json!(3))
    );
    assert_eq!(
        child_context.pointer("/workflow/versionId"),
        Some(&json!(child_version_id))
    );
    let child_start = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|claim| claim.execution_id == child_execution_id)
        .unwrap();
    process_command(&fixture.state.pool, &child_start)
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
    agentx_v2_runtime::engine::submit_worker_result(
        &fixture.state.pool,
        &successful_worker_result(&claim),
    )
    .await
    .unwrap();
    let parent_resume = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|command| command.execution_id == parent_execution_id)
        .unwrap();
    process_command(&fixture.state.pool, &parent_resume)
        .await
        .unwrap();
    let relation: (String, Value) = sqlx::query_as(
        "SELECT merge_status,context_overlay_json FROM execution_children WHERE tenant_id=? AND child_execution_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(child_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(relation.0, "merged");
    let result: Value = sqlx::query_scalar(
        "SELECT terminal_result_json FROM workflow_executions WHERE tenant_id=? AND id=?",
    )
    .bind(fixture.tenant_id)
    .bind(parent_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(result["message"], "composite-v2");
}

async fn agent_worker_runs_a_bounded_tool_loop_and_persists_usage(fixture: &Fixture) {
    let model_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let endpoint = "https://provider.example.test";
    // Trace projection is anchored to an existing workflow execution. The
    // synthetic claim may use a fresh attempt/node id, but its execution must
    // resolve through the Runtime authority row.
    let execution_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow_executions WHERE tenant_id=? ORDER BY created_at LIMIT 1",
    )
    .bind(fixture.tenant_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let model_id = Uuid::now_v7();
    let model_configuration = agentx_runtime_contracts::RuntimeResourceConfigurationV1::Model {
        provider: "openai_compatible".into(),
        endpoint: format!("{endpoint}/model"),
        model: "fixture-model".into(),
        context_window: 128_000,
        price: agentx_runtime_contracts::RuntimeModelPriceV1 {
            version_id: "price:1".into(),
            currency: "USD".into(),
            input_per_million: "0.5".into(),
            output_per_million: "0.5".into(),
        },
        credential: None,
    };
    let attempt_id = Uuid::now_v7();
    let node_execution_id = Uuid::now_v7();
    let claim = agentx_v2_runtime::engine::ClaimedWorkerAttempt {
        lease: agentx_runtime_contracts::WorkerAttemptLeaseV1 {
            protocol_version: 1,
            attempt_id,
            worker_id: Uuid::now_v7(),
            fencing_token: 1,
            locked_until: OffsetDateTime::now_utc() + time::Duration::seconds(30),
        },
        task: agentx_runtime_contracts::WorkerTaskV1 {
            protocol_version: 1,
            task_id: Uuid::now_v7(),
            tenant_id: fixture.tenant_id,
            execution_id,
            node_execution_id,
            attempt_id,
            capability: agentx_node_protocol::NodeCapability::Agent,
            bundle_id: Uuid::now_v7(),
            work_package_id: None,
            state_version: 1,
            compatibility_hash: agentx_runtime_contracts::content_hash(&json!({"agent":1}))
                .unwrap(),
            deadline_at: OffsetDateTime::now_utc() + time::Duration::seconds(30),
        },
        node_type: "agent".into(),
        invocation_id: None,
        node_version: 1,
        run_index: 0,
        iteration_index: 0,
        timeout_ms: 30_000,
        node_parameters: json!({
            "budget":{"maxIterations":3,"maxModelCalls":3,"maxTokens":100,"maxCost":0.0001},
            "_agent":{
                "contractVersion":"1.1",
                "bundleHash":"bundle-fixture",
                "definitionHash":"definition-fixture",
                "stableAgentNodeKey":"agent-fixture",
                "sessionPolicy":"invocation",
                "model":{"resourceId":model_id.to_string(),"resourceVersionId":"model:1"},
                "attachments":[],"coreTools":[],"followUpInputs":[{"messageId":"follow-up-1","role":"user","content":"continue","toolCalls":[],"toolCallId":null,"isError":false}]
            }
        }),
        per_item_parameters: vec![],
        string_conversions: json!({"common":[],"perItem":[]}),
        inputs: BTreeMap::from([(
            "main".into(),
            vec![agentx_node_protocol::Item {
                json: json!({"question":"agentx"}),
                ..Default::default()
            }],
        )]),
        resources: vec![
            agentx_runtime_contracts::RuntimeResourceBindingV1 {
                resource_kind: agentx_runtime_contracts::RuntimeResourceKindV1::Model,
                resource_id: model_id,
                resource_version: "model:1".into(),
                state_epoch: 1,
                content_hash: agentx_runtime_contracts::content_hash(&model_configuration).unwrap(),
                configuration: model_configuration,
                object_ids: vec![],
            },
        ],
        context: json!({}),
        plugin: None,
        trace_parent_span_entity_id: None,
    };
    let worker = test_worker(fixture, StubWorkerMode::Agent(model_calls.clone()));
    let mut unsupported_contract_claim = claim.clone();
    unsupported_contract_claim.node_parameters["_agent"]["contractVersion"] = json!("1.0");
    let unsupported_contract = worker.execute(&unsupported_contract_claim).await;
    assert_eq!(
        unsupported_contract.error_code.as_deref(),
        Some("AGENT_CORE_CONTRACT_UNSUPPORTED")
    );
    let mut attachment_claim = claim.clone();
    attachment_claim.node_parameters["_agent"]["attachments"] =
        json!([{"resourceType":"mcp_tool"}]);
    let rejected_attachment = worker.execute(&attachment_claim).await;
    assert_eq!(
        rejected_attachment.error_code.as_deref(),
        Some("AGENT_ATTACHMENT_REGISTRY_MISSING")
    );
    let mut sandbox_claim = claim.clone();
    sandbox_claim.node_parameters["_agent"]["workspaceSandbox"] =
        json!({"resourceId":Uuid::now_v7(),"resourceVersionId":"sandbox:1"});
    let rejected_sandbox = worker.execute(&sandbox_claim).await;
    assert_eq!(
        rejected_sandbox.error_code.as_deref(),
        Some("AGENT_WORKSPACE_PROFILE_BINDING_MISSING")
    );
    assert_eq!(model_calls.load(Ordering::SeqCst), 0);
    let output = worker.execute(&claim).await;
    assert_eq!(
        output.status,
        WorkerResultStatusV1::Succeeded,
        "skill failed: {:?} {:?}",
        output.error_code,
        output.error_message
    );
    assert_eq!(
        model_calls.load(Ordering::SeqCst),
        2,
        "agent output: {:?} {:?}",
        output.error_code,
        output.error_message
    );
    let run = sqlx::query(
        "SELECT iteration_count,model_call_count,tool_call_count,input_tokens,cost_micros,status FROM agent_runs WHERE node_execution_id=?",
    )
    .bind(node_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(run.try_get::<u32, _>("iteration_count").unwrap(), 2);
    assert_eq!(run.try_get::<u32, _>("model_call_count").unwrap(), 2);
    assert_eq!(run.try_get::<u32, _>("tool_call_count").unwrap(), 0);
    assert_eq!(run.try_get::<u64, _>("input_tokens").unwrap(), 10);
    assert_eq!(run.try_get::<u64, _>("cost_micros").unwrap(), 10);
    assert_eq!(run.try_get::<String, _>("status").unwrap(), "succeeded");
    let call_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM runtime_calls WHERE attempt_id=?")
            .bind(attempt_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(call_count, 2);
    let ledger = sqlx::query("SELECT COUNT(DISTINCT provider_request_id) provider_requests,CAST(SUM(input_tokens) AS UNSIGNED) input_tokens,CAST(SUM(output_tokens) AS UNSIGNED) output_tokens,CAST(SUM(cost_micros) AS UNSIGNED) cost_micros FROM runtime_calls WHERE attempt_id=? AND status='succeeded'")
        .bind(attempt_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(ledger.try_get::<i64, _>("provider_requests").unwrap(), 2);
    assert_eq!(ledger.try_get::<u64, _>("input_tokens").unwrap(), 10);
    assert_eq!(ledger.try_get::<u64, _>("output_tokens").unwrap(), 10);
    assert_eq!(ledger.try_get::<u64, _>("cost_micros").unwrap(), 10);

    let recovered = worker.execute(&claim).await;
    assert_eq!(
        recovered.status,
        WorkerResultStatusV1::Succeeded,
        "Agent recovery failed: {:?} {:?}",
        recovered.error_code,
        recovered.error_message
    );
    assert_eq!(
        model_calls.load(Ordering::SeqCst),
        2,
        "a recovered Agent run must reconcile settled Model Effects"
    );
    let recovered_call_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM runtime_calls WHERE attempt_id=?")
            .bind(attempt_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(recovered_call_count, 2);
    let core_trace_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_events WHERE tenant_id=? AND execution_id=? AND event_type IN ('agent_run.started','agent_turn.started','agent_model.intent','agent_model.settled','agent_message.added','agent_run.finished')")
        .bind(fixture.tenant_id)
        .bind(claim.task.execution_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert!(core_trace_events >= 6, "Core events must be projected into Trace");
}

async fn application_session_agent_restores_context_across_executions(fixture: &Fixture) {
    let session_id: Uuid = sqlx::query_scalar("SELECT session_id FROM application_invocations WHERE tenant_id=? AND session_id IS NOT NULL GROUP BY session_id HAVING COUNT(*)>=2 ORDER BY MIN(created_at) LIMIT 1")
        .bind(fixture.tenant_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    let execution_ids = sqlx::query_scalar::<_, Uuid>("SELECT e.id FROM workflow_executions e JOIN application_invocations i ON i.tenant_id=e.tenant_id AND i.id=e.invocation_id WHERE e.tenant_id=? AND i.session_id=? ORDER BY e.created_at,e.id LIMIT 2")
        .bind(fixture.tenant_id)
        .bind(session_id)
        .fetch_all(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(execution_ids.len(), 2);

    let model_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let model_id = Uuid::now_v7();
    let configuration = agentx_runtime_contracts::RuntimeResourceConfigurationV1::Model {
        provider: "openai_compatible".into(),
        endpoint: "https://provider.example.test/model".into(),
        model: "fixture-model".into(),
        context_window: 128_000,
        price: agentx_runtime_contracts::RuntimeModelPriceV1 {
            version_id: "price:1".into(),
            currency: "USD".into(),
            input_per_million: "0.5".into(),
            output_per_million: "0.5".into(),
        },
        credential: None,
    };
    let worker = test_worker(fixture, StubWorkerMode::Agent(model_calls.clone()));
    for (index, execution_id) in execution_ids.into_iter().enumerate() {
        let attempt_id = Uuid::now_v7();
        let claim = agentx_v2_runtime::engine::ClaimedWorkerAttempt {
            lease: agentx_runtime_contracts::WorkerAttemptLeaseV1 {
                protocol_version: 1,
                attempt_id,
                worker_id: Uuid::now_v7(),
                fencing_token: index as u64 + 1,
                locked_until: OffsetDateTime::now_utc() + time::Duration::seconds(30),
            },
            task: agentx_runtime_contracts::WorkerTaskV1 {
                protocol_version: 1,
                task_id: Uuid::now_v7(),
                tenant_id: fixture.tenant_id,
                execution_id,
                node_execution_id: Uuid::now_v7(),
                attempt_id,
                capability: agentx_node_protocol::NodeCapability::Agent,
                bundle_id: Uuid::now_v7(),
                work_package_id: None,
                state_version: 1,
                compatibility_hash: agentx_runtime_contracts::content_hash(&json!({"agent":1})).unwrap(),
                deadline_at: OffsetDateTime::now_utc() + time::Duration::seconds(30),
            },
            node_type: "agent".into(),
            invocation_id: None,
            node_version: 1,
            run_index: 0,
            iteration_index: 0,
            timeout_ms: 30_000,
            node_parameters: json!({
                "budget":{"maxIterations":2,"maxModelCalls":2,"maxTokens":100,"maxCost":0.0001},
                "_agent":{
                    "contractVersion":"1.1","bundleHash":"session-bundle","definitionHash":"session-definition",
                    "stableAgentNodeKey":"session-agent","sessionPolicy":"application_session",
                    "model":{"resourceId":model_id.to_string(),"resourceVersionId":"model:1"},
                    "attachments":[],"coreTools":[]
                }
            }),
            per_item_parameters: vec![],
            string_conversions: json!({"common":[],"perItem":[]}),
            inputs: BTreeMap::from([(
                "main".into(),
                vec![agentx_node_protocol::Item {
                    json: json!(format!("session-prompt-{index}")),
                    ..Default::default()
                }],
            )]),
            resources: vec![agentx_runtime_contracts::RuntimeResourceBindingV1 {
                resource_kind: agentx_runtime_contracts::RuntimeResourceKindV1::Model,
                resource_id: model_id,
                resource_version: "model:1".into(),
                state_epoch: 1,
                content_hash: agentx_runtime_contracts::content_hash(&configuration).unwrap(),
                configuration: configuration.clone(),
                object_ids: vec![],
            }],
            context: json!({}),
            plugin: None,
            trace_parent_span_entity_id: None,
        };
        let output = worker.execute(&claim).await;
        assert_eq!(output.status, WorkerResultStatusV1::Succeeded);
    }
    assert_eq!(model_calls.load(Ordering::SeqCst), 2);
    let entries: Vec<Value> = sqlx::query_scalar("SELECT payload_json FROM agent_session_entries WHERE tenant_id=? AND application_id=? AND session_key=? AND stable_agent_node_key='session-agent' AND entry_kind IN ('message.user','message.assistant') ORDER BY sequence_number")
        .bind(fixture.tenant_id)
        .bind(fixture.application_id)
        .bind(format!("application:{}:{}", fixture.application_id, session_id))
        .fetch_all(&fixture.state.pool)
        .await
        .unwrap();
    assert!(entries.iter().any(|message| message["content"] == "session-prompt-0"));
    assert!(entries.iter().any(|message| message["content"] == "session-prompt-1"));
}

async fn sandbox_manager_is_fenced_and_idempotent(fixture: &Fixture) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider_endpoint = format!("http://{}", listener.local_addr().unwrap());
    let execd_endpoint = provider_endpoint.clone();
    let takeover_command_blocked = Arc::new(AtomicBool::new(false));
    let command_block = takeover_command_blocked.clone();
    let provider = Router::new()
        .route(
            "/v1/sandboxes",
            post(|| async {
                Json(json!({
                    "id":"sandbox-v2",
                    "status":{"state":"Running"},
                    "metadata":{}
                }))
            }),
        )
        .route(
            "/v1/sandboxes/{id}/endpoints/44772",
            get(move |Path(id): Path<String>| {
                let endpoint = execd_endpoint.clone();
                async move {
                    Json(json!({
                        "endpoint":format!("{endpoint}/v1/sandboxes/{id}/proxy/44772"),
                        "headers":{}
                    }))
                }
            }),
        )
        .route(
            "/v1/sandboxes/{id}/proxy/44772/command",
            post(move |Json(body): Json<Value>| {
                let command_block = command_block.clone();
                async move {
                    let command = body.get("command").and_then(Value::as_str).unwrap_or_default();
                    if command.contains("cHJpbnRmIHRha2VvdmVy")
                        && !command_block.swap(true, Ordering::SeqCst)
                    {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    if command.contains("ZXhpdCAx") {
                        (
                            axum::http::StatusCode::OK,
                            "data: {\"type\":\"stderr\",\"text\":\"failed\"}\n\ndata: {\"type\":\"result\",\"exit_code\":1}\n\n",
                        )
                    } else {
                        (
                            axum::http::StatusCode::OK,
                            "data: {\"type\":\"stdout\",\"text\":\"ok\\n__AGENTX_RESULT__eyJvayI6dHJ1ZX0=\"}\n\ndata: {\"type\":\"result\",\"exit_code\":0}\n\n",
                        )
                    }
                }
            }),
        )
        .route(
            "/v1/sandboxes/{id}/proxy/44772/pty/{process_id}",
            axum::routing::delete(
                |Path((_id, process_id)): Path<(String, String)>| async move {
                    Json(json!({"sessionId":process_id,"terminated":true}))
                },
            ),
        )
        .route(
            "/v1/sandboxes/{id}",
            axum::routing::delete(|Path(id): Path<String>| async move {
                Json(json!({"sandboxId":id,"terminated":true}))
            }),
        );
    let provider_task = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
    let worker_id = Uuid::now_v7();
    let execution_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow_executions WHERE tenant_id=? ORDER BY created_at,id LIMIT 1",
    )
    .bind(fixture.tenant_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let node_execution_id = Uuid::now_v7();
    let attempt_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO node_attempts(id,tenant_id,execution_id,node_execution_id,attempt_number,capability,worker_protocol_version,ir_schema_version,compiler_version,manifest_version,status,idempotency_key,worker_instance_id,locked_until,fencing_token,deadline_at,input_json) VALUES(?,?,?,?,1,'sandbox',1,1,? ,?,'running',?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),7,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),JSON_OBJECT())",
    )
    .bind(attempt_id)
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .bind(node_execution_id)
    .bind(env!("CARGO_PKG_VERSION"))
    .bind(agentx_node_protocol::NODE_PROTOCOL_VERSION)
    .bind(format!("sandbox-test:{attempt_id}"))
    .bind(worker_id.to_string())
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let configuration = agentx_runtime_contracts::RuntimeResourceConfigurationV1::SandboxProfile {
        provider: "opensandbox".into(),
        image: "python:3.13".into(),
        cpu_millis: 500,
        memory_bytes: 256 * 1024 * 1024,
        disk_bytes: 1024 * 1024 * 1024,
        pid_limit: 64,
        egress_mode: agentx_runtime_contracts::SandboxEgressModeV1::None,
        maximum_ttl_seconds: 300,
    };
    let request = agentx_v2_runtime::sandbox::SandboxExecuteRequestV1 {
        api_version: 1,
        tenant_id: fixture.tenant_id,
        execution_id,
        node_execution_id,
        attempt_id,
        worker_id,
        fencing_token: 7,
        idempotency_key: format!("sandbox:execute:{attempt_id}"),
        profile: agentx_runtime_contracts::RuntimeResourceBindingV1 {
            resource_kind: agentx_runtime_contracts::RuntimeResourceKindV1::SandboxProfile,
            resource_id: Uuid::now_v7(),
            resource_version: "profile:1".into(),
            state_epoch: 1,
            content_hash: agentx_runtime_contracts::content_hash(&configuration).unwrap(),
            configuration,
            object_ids: vec![],
        },
        input: json!({"code":"print('ok')"}),
        parameters: json!({"runner":"shell","inputs":{},"source":"printf ok; printf '{\"ok\":true}' > \"$AGENTX_OUTPUT_PATH\"","outputSchema":{"type":"object"},"networkPolicy":{"mode":"deny","destinations":[]}}),
    };
    let manager_state = agentx_v2_runtime::sandbox::SandboxManagerState {
        pool: fixture.state.pool.clone(),
        client: reqwest::Client::new(),
        provider_endpoint,
        provider_api_key: None,
        provider_secure_access: false,
        owner: Uuid::now_v7(),
        vault: None,
        objects: std::sync::Arc::new(object_store::memory::InMemory::new()),
        process_sockets: Default::default(),
    };
    let manager = agentx_v2_runtime::sandbox::router(manager_state.clone());
    let first = call_sandbox_manager(manager.clone(), &request).await;
    assert_eq!(first.0, axum::http::StatusCode::OK);
    assert!(!first.1.replayed);
    let replay = call_sandbox_manager(manager.clone(), &request).await;
    assert_eq!(replay.0, axum::http::StatusCode::OK);
    assert!(replay.1.replayed);
    let mut conflicting = request.clone();
    conflicting.input = json!({"code":"different"});
    let conflict = sandbox_request(manager.clone(), &conflicting).await;
    assert_eq!(conflict.status(), axum::http::StatusCode::CONFLICT);
    let mut stale = request.clone();
    stale.idempotency_key.push_str(":stale");
    stale.fencing_token = 6;
    let stale = sandbox_request(manager.clone(), &stale).await;
    assert_eq!(stale.status(), axum::http::StatusCode::CONFLICT);
    let replacement_worker_id = Uuid::now_v7();
    sqlx::query(
        "UPDATE node_attempts SET worker_instance_id=?,lease_token=?,fencing_token=8,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND) WHERE id=?",
    )
    .bind(replacement_worker_id.to_string())
    .bind(replacement_worker_id)
    .bind(attempt_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let mut replacement = request.clone();
    replacement.worker_id = replacement_worker_id;
    replacement.fencing_token = 8;
    let replay_after_takeover = call_sandbox_manager(manager.clone(), &replacement).await;
    assert_eq!(replay_after_takeover.0, axum::http::StatusCode::OK);
    assert!(replay_after_takeover.1.replayed);
    assert_eq!(replay_after_takeover.1.output, first.1.output);

    let process_session_id = Uuid::now_v7();
    let process_lease_id = Uuid::now_v7();
    let agent_run_id = Uuid::now_v7();
    sqlx::query("INSERT INTO sandbox_process_sessions(process_session_id,identity_hash,tenant_id,agent_run_id,mcp_server_version_id,sandbox_profile_version_id,profile_json,command_json,credential_refs_json,sandbox_id,provider_operation_id,status,lease_id,attempt_id,worker_id,fencing_token,lease_expires_at,process_expires_at) VALUES(?,REPEAT('a',64),?,?,?,?,?,?,?,'sandbox-v2','process-v2','running',?,?,?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 5 MINUTE))")
        .bind(process_session_id)
        .bind(fixture.tenant_id)
        .bind(agent_run_id)
        .bind(Uuid::now_v7())
        .bind(Uuid::now_v7())
        .bind(json!({"profile":"runtime"}))
        .bind(json!({"argv":["/usr/bin/mcp"]}))
        .bind(json!([]))
        .bind(process_lease_id)
        .bind(attempt_id)
        .bind(replacement_worker_id)
        .bind(8_u64)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    let proof = agentx_runtime_contracts::ProcessSessionProofV1 {
        api_version: 1,
        tenant_id: fixture.tenant_id,
        execution_id,
        node_execution_id,
        attempt_id,
        agent_run_id,
        worker_id: replacement_worker_id,
        fencing_token: 8,
        process_session_id,
        lease_id: process_lease_id,
        operation_id: "process:terminate".into(),
        effect_id: "process:terminate:effect".into(),
        idempotency_key: "process:terminate:idempotency".into(),
        deadline: OffsetDateTime::now_utc() + time::Duration::seconds(30),
    };
    let mut stale_proof = proof.clone();
    stale_proof.fencing_token = 7;
    let stale_process = process_control_request(
        manager.clone(),
        process_session_id,
        "terminate",
        &agentx_runtime_contracts::ProcessSessionControlRequestV1 { proof: stale_proof },
    )
    .await;
    assert_eq!(stale_process.status(), axum::http::StatusCode::CONFLICT);
    let terminated_process = process_control_request(
        manager.clone(),
        process_session_id,
        "terminate",
        &agentx_runtime_contracts::ProcessSessionControlRequestV1 { proof },
    )
    .await;
    assert_eq!(terminated_process.status(), axum::http::StatusCode::OK);
    let terminated_process_body =
        to_bytes(terminated_process.into_body(), 1024 * 1024).await.unwrap();
    let terminated_process: agentx_runtime_contracts::ProcessSessionControlResponseV1 =
        serde_json::from_slice(&terminated_process_body).unwrap();
    assert_eq!(
        terminated_process.status,
        agentx_runtime_contracts::ProcessSessionStatusV1::Exited
    );
    let persisted_process: (String, Option<i64>) = sqlx::query_as(
        "SELECT status,exit_code FROM sandbox_process_sessions WHERE process_session_id=?",
    )
    .bind(process_session_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(persisted_process, ("exited".into(), Some(143)));

    let abandoned_attempt_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO node_attempts(id,tenant_id,execution_id,node_execution_id,attempt_number,capability,worker_protocol_version,ir_schema_version,compiler_version,manifest_version,status,idempotency_key,worker_instance_id,locked_until,fencing_token,deadline_at,input_json) VALUES(?,?,?,?,2,'sandbox',1,1,? ,?,'running',?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),7,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 5 MINUTE),JSON_OBJECT())",
    )
    .bind(abandoned_attempt_id)
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .bind(node_execution_id)
    .bind(env!("CARGO_PKG_VERSION"))
    .bind(agentx_node_protocol::NODE_PROTOCOL_VERSION)
    .bind(format!("sandbox-test:{abandoned_attempt_id}"))
    .bind(worker_id.to_string())
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let mut abandoned_request = request.clone();
    abandoned_request.attempt_id = abandoned_attempt_id;
    abandoned_request.idempotency_key = format!("sandbox:execute:{abandoned_attempt_id}");
    abandoned_request.parameters = json!({"runner":"shell","inputs":{},"source":"printf takeover; printf '{\"ok\":true}' > \"$AGENTX_OUTPUT_PATH\"","outputSchema":{"type":"object"},"networkPolicy":{"mode":"deny","destinations":[]}});
    let abandoned_manager = manager.clone();
    let abandoned_payload = abandoned_request.clone();
    let abandoned =
        tokio::spawn(async move { sandbox_request(abandoned_manager, &abandoned_payload).await });
    for _ in 0..100 {
        let status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM sandbox_leases WHERE tenant_id=? AND idempotency_key=?",
        )
        .bind(fixture.tenant_id)
        .bind(&abandoned_request.idempotency_key)
        .fetch_optional(&fixture.state.pool)
        .await
        .unwrap();
        if status.as_deref() == Some("interrupting") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    abandoned.abort();
    let _ = abandoned.await;
    let abandoned_status: String = sqlx::query_scalar(
        "SELECT status FROM sandbox_leases WHERE tenant_id=? AND idempotency_key=?",
    )
    .bind(fixture.tenant_id)
    .bind(&abandoned_request.idempotency_key)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(abandoned_status, "interrupting");
    let takeover_worker_id = Uuid::now_v7();
    sqlx::query(
        "UPDATE node_attempts SET worker_instance_id=?,lease_token=?,fencing_token=8,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND) WHERE id=?",
    )
    .bind(takeover_worker_id.to_string())
    .bind(takeover_worker_id)
    .bind(abandoned_attempt_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE sandbox_leases SET locked_until=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE tenant_id=? AND idempotency_key=?",
    )
    .bind(fixture.tenant_id)
    .bind(&abandoned_request.idempotency_key)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    abandoned_request.worker_id = takeover_worker_id;
    abandoned_request.fencing_token = 8;
    let recovered = call_sandbox_manager(manager.clone(), &abandoned_request).await;
    assert_eq!(recovered.0, axum::http::StatusCode::OK);
    assert!(recovered.1.replayed);
    let recovered_lease: (String, u64) = sqlx::query_as(
        "SELECT status,fencing_token FROM sandbox_leases WHERE tenant_id=? AND idempotency_key=?",
    )
    .bind(fixture.tenant_id)
    .bind(&abandoned_request.idempotency_key)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(recovered_lease, ("terminated".into(), 2));

    let failed_attempt_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO node_attempts(id,tenant_id,execution_id,node_execution_id,attempt_number,capability,worker_protocol_version,ir_schema_version,compiler_version,manifest_version,status,idempotency_key,worker_instance_id,locked_until,fencing_token,deadline_at,input_json) VALUES(?,?,?,?,3,'sandbox',1,1,? ,?,'running',?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),7,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),JSON_OBJECT())",
    )
    .bind(failed_attempt_id)
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .bind(node_execution_id)
    .bind(env!("CARGO_PKG_VERSION"))
    .bind(agentx_node_protocol::NODE_PROTOCOL_VERSION)
    .bind(format!("sandbox-test:{failed_attempt_id}"))
    .bind(worker_id.to_string())
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let mut failed = request;
    failed.attempt_id = failed_attempt_id;
    failed.idempotency_key = format!("sandbox:execute:{failed_attempt_id}");
    failed.input = json!({"fail":true});
    failed.parameters = json!({"runner":"shell","inputs":{},"source":"exit 1","outputSchema":{"type":"object"},"networkPolicy":{"mode":"deny","destinations":[]}});
    let failed_response = sandbox_request(manager, &failed).await;
    assert_eq!(
        failed_response.status(),
        axum::http::StatusCode::BAD_GATEWAY
    );
    let failed_status: String = sqlx::query_scalar(
        "SELECT status FROM sandbox_leases WHERE tenant_id=? AND idempotency_key=?",
    )
    .bind(fixture.tenant_id)
    .bind(&failed.idempotency_key)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(failed_status, "failed");
    let status: String = sqlx::query_scalar("SELECT status FROM sandbox_leases WHERE id=?")
        .bind(first.1.lease_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "terminated");
    let orphan_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sandbox_leases(id,tenant_id,execution_id,node_execution_id,attempt_id,worker_lease_token,sandbox_id,lease_token_hash,profile_version_id,idempotency_key,status,provider_labels_json,request_hash,expires_at,fencing_token,outcome_unknown) VALUES(?,?,?,?,?,?,?,?,?,?,'orphaned',?, ?,DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND),1,TRUE)",
    )
    .bind(orphan_id)
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .bind(node_execution_id)
    .bind(attempt_id)
    .bind(Uuid::now_v7())
    .bind("orphan-v2")
    .bind("b".repeat(64))
    .bind(Uuid::now_v7())
    .bind(format!("sandbox:orphan:{orphan_id}"))
    .bind(json!({"agentxLeaseId":orphan_id}))
    .bind(agentx_runtime_contracts::content_hash(&json!({"orphan":orphan_id})).unwrap().as_str())
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    assert!(
        agentx_v2_runtime::sandbox::reconcile_one(&manager_state)
            .await
            .unwrap()
    );
    let reconciled: (String, u64, u64) = sqlx::query_as(
        "SELECT status,fencing_token,termination_attempts FROM sandbox_leases WHERE id=?",
    )
    .bind(orphan_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(reconciled, ("terminated".into(), 2, 1));
    for (status, last_error, expected_event) in [
        (
            "interrupting",
            Some("execution_cancelled"),
            "sandbox.cancelled",
        ),
        ("running", None, "sandbox.timed_out"),
    ] {
        let lease_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO sandbox_leases(id,tenant_id,execution_id,node_execution_id,attempt_id,worker_lease_token,sandbox_id,lease_token_hash,profile_version_id,idempotency_key,status,provider_labels_json,request_hash,expires_at,fencing_token,outcome_unknown,last_error) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND),1,FALSE,?)",
        )
        .bind(lease_id)
        .bind(fixture.tenant_id)
        .bind(execution_id)
        .bind(node_execution_id)
        .bind(attempt_id)
        .bind(Uuid::now_v7())
        .bind(format!("sandbox-{lease_id}"))
        .bind(format!("{:x}", Sha256::digest(lease_id.as_bytes())))
        .bind(Uuid::now_v7())
        .bind(format!("sandbox:terminal:{lease_id}"))
        .bind(status)
        .bind(json!({"agentxLeaseId":lease_id}))
        .bind(
            agentx_runtime_contracts::content_hash(&json!({"terminal":lease_id}))
                .unwrap()
                .as_str(),
        )
        .bind(last_error)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
        let mut reconciled_target = false;
        for _ in 0..16 {
            assert!(
                agentx_v2_runtime::sandbox::reconcile_one(&manager_state)
                    .await
                    .unwrap()
            );
            let lease_status: String =
                sqlx::query_scalar("SELECT status FROM sandbox_leases WHERE id=?")
                    .bind(lease_id)
                    .fetch_one(&fixture.state.pool)
                    .await
                    .unwrap();
            if lease_status == "terminated" {
                reconciled_target = true;
                break;
            }
        }
        assert!(
            reconciled_target,
            "Sandbox Reaper must reach the target Lease"
        );
        let traced: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM trace_outbox WHERE tenant_id=? AND execution_id=? AND JSON_UNQUOTE(JSON_EXTRACT(payload_json,'$.sandboxLeaseId'))=? AND JSON_UNQUOTE(JSON_EXTRACT(payload_json,'$.eventType'))=?",
        )
        .bind(fixture.tenant_id)
        .bind(execution_id)
        .bind(lease_id.to_string())
        .bind(expected_event)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
        assert_eq!(traced, 1, "Sandbox terminal path must close its Span");
    }
    provider_task.abort();
}

async fn call_sandbox_manager(
    manager: Router,
    request: &agentx_v2_runtime::sandbox::SandboxExecuteRequestV1,
) -> (
    axum::http::StatusCode,
    agentx_v2_runtime::sandbox::SandboxExecuteResponseV1,
) {
    let response = sandbox_request(manager, request).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let parsed = serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "Sandbox Manager returned {status} with an unexpected body: {error}; {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, parsed)
}

async fn sandbox_request(
    manager: Router,
    request: &agentx_v2_runtime::sandbox::SandboxExecuteRequestV1,
) -> axum::response::Response {
    manager
        .oneshot(
            Request::post("/internal/runtime/v1/sandboxes:execute")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn process_control_request(
    manager: Router,
    process_session_id: Uuid,
    action: &str,
    request: &agentx_runtime_contracts::ProcessSessionControlRequestV1,
) -> axum::response::Response {
    manager
        .oneshot(
            Request::post(format!(
                "/internal/runtime/v1/sandbox-process-sessions/{process_session_id}:{action}"
            ))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(request).unwrap()))
            .unwrap(),
        )
        .await
        .unwrap()
}

async fn retention_dry_run_reference_block_and_object_sweep_are_fenced(fixture: &Fixture) {
    let deletable = Uuid::now_v7();
    let protected = Uuid::now_v7();
    for (artifact_id, key) in [
        (deletable, format!("artifacts/{deletable}")),
        (protected, format!("artifacts/{protected}")),
    ] {
        fixture
            .state
            .objects
            .put(
                &ObjectPath::from(key.clone()),
                Bytes::from_static(b"retention").into(),
            )
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO artifacts(id,tenant_id,content_type,size_bytes,sha256,storage_key,created_at) VALUES(?,?,'application/octet-stream',9,REPEAT('a',64),?,DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 30 DAY))",
        )
        .bind(artifact_id)
        .bind(fixture.tenant_id)
        .bind(key)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'execution','retention-test','output')",
    )
    .bind(fixture.tenant_id)
    .bind(protected)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let retained_execution: Uuid = sqlx::query_scalar(
        "SELECT target_execution_id FROM evaluation_run_cases WHERE tenant_id=? AND status='completed' ORDER BY created_at LIMIT 1",
    )
    .bind(fixture.tenant_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM checkpoints WHERE tenant_id=? AND execution_id=?")
        .bind(fixture.tenant_id)
        .bind(retained_execution)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_executions SET ended_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 30 DAY) WHERE tenant_id=? AND id=?",
    )
    .bind(fixture.tenant_id)
    .bind(retained_execution)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let retained_message = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO application_messages(id,tenant_id,session_id,sequence_number,role,created_at) VALUES(?,?,?,1,'assistant',DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 30 DAY))",
    )
    .bind(retained_message)
    .bind(fixture.tenant_id)
    .bind(Uuid::now_v7())
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let retained_evaluation: Uuid = sqlx::query_scalar(
        "SELECT id FROM evaluation_runs WHERE tenant_id=? AND status='completed' ORDER BY created_at LIMIT 1",
    )
    .bind(fixture.tenant_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE evaluation_runs SET completed_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 30 DAY) WHERE id=?",
    )
    .bind(retained_evaluation)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    apply_target(
        fixture,
        3,
        AdmissionTargetV1::RetentionPolicy {
            state: RuntimeRetentionPolicyV1 {
                tenant_id: fixture.tenant_id,
                policy_version: 1,
                retention_days: BTreeMap::from([
                    (RuntimeRetentionDataTypeV1::Artifact, 14),
                    (RuntimeRetentionDataTypeV1::Execution, 14),
                    (RuntimeRetentionDataTypeV1::ApplicationMessage, 14),
                    (RuntimeRetentionDataTypeV1::EvaluationReport, 14),
                ]),
                enabled: true,
            },
        },
    )
    .await;

    let dry_run = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO retention_runs(id,tenant_id,dry_run,policy_version,idempotency_key,status,requested_by) VALUES(?,?,TRUE,1,?,'queued',?)",
    )
    .bind(dry_run)
    .bind(fixture.tenant_id)
    .bind(format!("retention:dry:{dry_run}"))
    .bind(Uuid::now_v7())
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    assert!(
        run_retention_once(&fixture.state, Uuid::now_v7())
            .await
            .unwrap()
    );
    let dry_status: String = sqlx::query_scalar("SELECT status FROM retention_runs WHERE id=?")
        .bind(dry_run)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(dry_status, "completed");
    let dry_deleted: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE id=? AND deleted_at IS NOT NULL")
            .bind(deletable)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(dry_deleted, 0);

    let run = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO retention_runs(id,tenant_id,dry_run,policy_version,idempotency_key,status,requested_by) VALUES(?,?,FALSE,1,?,'queued',?)",
    )
    .bind(run)
    .bind(fixture.tenant_id)
    .bind(["retention:delete:", &run.to_string()].concat())
    .bind(Uuid::now_v7())
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    assert!(
        run_retention_once(&fixture.state, Uuid::now_v7())
            .await
            .unwrap()
    );
    let rows = sqlx::query(
        "SELECT target_id,status FROM retention_items WHERE retention_run_id=? ORDER BY target_id",
    )
    .bind(run)
    .fetch_all(&fixture.state.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        (
            row.try_get::<String, _>("target_id").unwrap(),
            row.try_get::<String, _>("status").unwrap(),
        )
    })
    .collect::<HashMap<_, _>>();
    assert_eq!(
        rows.get(&deletable.to_string()).map(String::as_str),
        Some("deleted")
    );
    assert_eq!(
        rows.get(&protected.to_string()).map(String::as_str),
        Some("blocked")
    );
    assert!(
        fixture
            .state
            .objects
            .get(&ObjectPath::from(format!("artifacts/{deletable}")))
            .await
            .is_err()
    );
    assert!(
        fixture
            .state
            .objects
            .get(&ObjectPath::from(format!("artifacts/{protected}")))
            .await
            .is_ok()
    );
    for (table, id) in [
        ("workflow_executions", retained_execution),
        ("application_messages", retained_message),
        ("evaluation_runs", retained_evaluation),
    ] {
        let deleted: bool = match table {
            "workflow_executions" => sqlx::query_scalar(
                "SELECT retention_deleted_at IS NOT NULL FROM workflow_executions WHERE id=?",
            )
            .bind(id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap(),
            "application_messages" => sqlx::query_scalar(
                "SELECT retention_deleted_at IS NOT NULL FROM application_messages WHERE id=?",
            )
            .bind(id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap(),
            "evaluation_runs" => sqlx::query_scalar(
                "SELECT retention_deleted_at IS NOT NULL FROM evaluation_runs WHERE id=?",
            )
            .bind(id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap(),
            _ => unreachable!(),
        };
        assert!(deleted, "{table} retention candidate was not tombstoned");
    }

    let retryable = Uuid::now_v7();
    let retryable_key = format!("artifacts/{retryable}");
    fixture
        .state
        .objects
        .put(
            &ObjectPath::from(retryable_key.clone()),
            Bytes::from_static(b"retention").into(),
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO artifacts(id,tenant_id,content_type,size_bytes,sha256,storage_key,created_at) VALUES(?,?,'application/octet-stream',9,REPEAT('b',64),?,DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 30 DAY))",
    )
    .bind(retryable)
    .bind(fixture.tenant_id)
    .bind(&retryable_key)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let retry_run = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO retention_runs(id,tenant_id,dry_run,policy_version,idempotency_key,status,requested_by) VALUES(?,?,FALSE,1,?,'queued',?)",
    )
    .bind(retry_run)
    .bind(fixture.tenant_id)
    .bind(format!("retention:retry:{retry_run}"))
    .bind(Uuid::now_v7())
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    let failing_state = RuntimeState {
        pool: fixture.state.pool.clone(),
        objects: Arc::new(FailOnceDeleteStore::new(fixture.state.objects.clone())),
        trust: fixture.state.trust.clone(),
        wakeups: Default::default(),
        vault: None,
    };
    assert!(
        run_retention_once(&failing_state, Uuid::now_v7())
            .await
            .unwrap()
    );
    let failed = sqlx::query(
        "SELECT status,attempt_count FROM retention_items WHERE retention_run_id=? AND object_id=?",
    )
    .bind(retry_run)
    .bind(retryable)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(failed.get::<String, _>("status"), "failed");
    assert_eq!(failed.get::<u32, _>("attempt_count"), 1);
    sqlx::query(
        "UPDATE retention_runs SET locked_until=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE id=?",
    )
    .bind(retry_run)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    assert!(
        run_retention_once(&fixture.state, Uuid::now_v7())
            .await
            .unwrap()
    );
    let retried = sqlx::query(
        "SELECT status,attempt_count FROM retention_items WHERE retention_run_id=? AND object_id=?",
    )
    .bind(retry_run)
    .bind(retryable)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(retried.get::<String, _>("status"), "deleted");
    assert_eq!(retried.get::<u32, _>("attempt_count"), 2);
}

async fn trigger_claim_takeover_is_fenced(pool: &MySqlPool) {
    let tenant_id = Uuid::now_v7();
    let application_id = Uuid::now_v7();
    let binding_id = Uuid::now_v7();
    let bundle_id = Uuid::now_v7();
    let specification = RuntimeTriggerSpecV1 {
        schema_version: 1,
        trigger_id: binding_id,
        trigger_name: "Schedule source".into(),
        application_id,
        node_id: "schedule-source".into(),
        revision: 1,
        configuration_hash: agentx_runtime_contracts::content_hash(&json!({"schedule":"v1"})).unwrap(),
        enabled: true,
        configuration: RuntimeTriggerConfigurationV1::Schedule {
            cron_expression: "*/5 * * * *".into(),
            timezone: "UTC".into(),
            misfire_policy: agentx_runtime_contracts::ScheduleMisfirePolicyV1::FireOnce,
            grace_seconds: 60,
            input: json!({}),
        },
    };
    sqlx::query("INSERT INTO trigger_bindings(id,tenant_id,application_id,application_deployment_id,bundle_id,workflow_version_id,node_id,configuration_revision,configuration_hash,trigger_kind,configuration_json,status,next_poll_at) VALUES(?,?,?,?,?,?,?,?,?,'schedule',?,'active',UTC_TIMESTAMP(6))")
        .bind(binding_id).bind(tenant_id).bind(application_id).bind(Uuid::now_v7()).bind(bundle_id).bind(Uuid::now_v7()).bind("poll-source").bind(1_u64).bind(specification.configuration_hash.as_str()).bind(serde_json::to_value(&specification).unwrap()).execute(pool).await.unwrap();

    let first_owner = Uuid::now_v7();
    let second_owner = Uuid::now_v7();
    let (first, second) = tokio::join!(
        agentx_v2_runtime::trigger::claim(pool, first_owner, 100),
        agentx_v2_runtime::trigger::claim(pool, second_owner, 100),
    );
    let mut claims = first.unwrap();
    claims.extend(second.unwrap());
    assert_eq!(claims.len(), 1, "two Trigger replicas claimed one Binding");
    let stale = claims.pop().unwrap();
    assert_eq!(stale.fencing_token, 1);

    sqlx::query("UPDATE trigger_bindings SET locked_until=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE id=?")
        .bind(binding_id).execute(pool).await.unwrap();
    let current_owner = if stale.owner == first_owner {
        second_owner
    } else {
        first_owner
    };
    let current = agentx_v2_runtime::trigger::claim(pool, current_owner, 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(current.fencing_token, 2);
    assert!(
        agentx_v2_runtime::trigger::heartbeat(pool, &stale)
            .await
            .is_err()
    );
}
