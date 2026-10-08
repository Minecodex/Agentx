async fn completed_history_does_not_block_the_next_node(fixture: &Fixture) {
    ensure_builtin_data_runtime_object(fixture).await;
    let now = OffsetDateTime::now_utc();
    let package_id = Uuid::now_v7();
    let mut source = fixture.work_package_source(package_id, now, now + time::Duration::hours(1));
    let mut definition = serde_json::to_value(&source.definition).unwrap();
    let mut second = definition["nodes"][0].clone();
    second["id"] = json!("second");
    second["key"] = json!("second");
    second["name"] = json!("Second");
    definition["nodes"]
        .as_array_mut()
        .unwrap()
        .insert(1, second);
    definition["nodes"][2]["parameters"]["outputs"]["message"]["selector"]["sourceNodeId"] =
        json!("second");
    definition["connections"][1]["targetNodeId"] = json!("second");
    definition["connections"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"second-end", "sourceNodeId":"second", "sourceHandle":"main",
            "targetNodeId":"exit", "targetHandle":"main", "order":0
        }));
    source.definition = serde_json::from_value(definition).unwrap();
    let compiled = compile_workflow_version(&source.definition, package_id).unwrap();
    source.spec = agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Debug {
        draft_revision: 1,
        debug_plan: agentx_runtime_contracts::RuntimeDebugPlanV1::whole(&compiled),
    };
    let _ = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(PrepareWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: "incremental:prepare".into(),
            work_package: build_work_package(
                source,
                "work-package-current",
                &fixture.work_package_signing_key,
            )
            .unwrap(),
        }),
    )
    .await
    .unwrap();
    let accepted = execute_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(ExecuteWorkPackageRequestV1 {
            api_version: 1,
            package_id,
            input: Value::Null,
            idempotency_key: "incremental:execute".into(),
        }),
    )
    .await
    .unwrap()
    .0;
    let execution_id: Uuid =
        serde_json::from_value(accepted.result["executionId"].clone()).unwrap();
    let owner = Uuid::now_v7();
    let commands = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap();
    let command = commands
        .iter()
        .find(|claim| claim.execution_id == execution_id)
        .unwrap();
    process_command(&fixture.state.pool, command).await.unwrap();
    let worker_id = Uuid::now_v7();
    let mut first_node = None;
    for index in 0..2 {
        let dispatch = claim_dispatch(&fixture.state.pool, owner)
            .await
            .unwrap()
            .unwrap();
        let task = dispatch.task().unwrap();
        assert_eq!(task.execution_id, execution_id);
        complete_dispatch(&fixture.state.pool, &dispatch)
            .await
            .unwrap();
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
        let mut result = successful_worker_result(&claim);
        let outputs = result.outputs.get_mut("main").unwrap();
        if index == 0 {
            // One delivery crosses multiple 256-row lineage insert batches.
            outputs.resize(600, outputs[0].clone());
        } else {
            outputs.truncate(1);
        }
        result.result_hash = agentx_v2_runtime::engine::worker_result_hash(
            result.status,
            &result.outputs,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        if index == 0 {
            first_node = Some(task.node_execution_id);
            agentx_v2_runtime::engine::submit_worker_result(&fixture.state.pool, &result)
                .await
                .unwrap();
            continue;
        }
        // Keep completed history exclusively locked while its successor settles.
        // Rewriting either the activation or its existing delivery would block.
        let mut history = fixture.state.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM node_executions WHERE id=? FOR UPDATE")
            .bind(first_node.unwrap())
            .fetch_one(&mut *history)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM execution_edge_deliveries WHERE execution_id=? AND source_node_execution_id=? FOR UPDATE")
            .bind(execution_id).bind(first_node.unwrap()).fetch_one(&mut *history).await.unwrap();
        let settled = tokio::time::timeout(
            Duration::from_millis(500),
            agentx_v2_runtime::engine::submit_worker_result(&fixture.state.pool, &result),
        )
        .await;
        history.rollback().await.unwrap();
        settled
            .expect("completed history must not be locked again")
            .unwrap();
    }
    let status: String = sqlx::query_scalar("SELECT status FROM workflow_executions WHERE id=?")
        .bind(execution_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "succeeded");
    let deliveries: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution_edge_deliveries WHERE execution_id=?")
            .bind(execution_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(deliveries, 1, "history must neither be lost nor duplicated");
    let ends: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution_end_deliveries WHERE execution_id=?")
            .bind(execution_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(ends, 1);
    let lineage: (i64, u32, u32, i64) = sqlx::query_as("SELECT COUNT(*),MIN(target_item_index),MAX(target_item_index),COUNT(DISTINCT source_item_index) FROM item_lineage WHERE execution_id=?")
        .bind(execution_id).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(lineage, (600, 0, 599, 600));
}
