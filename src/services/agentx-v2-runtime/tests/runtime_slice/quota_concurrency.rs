async fn concurrent_quota_reservations_are_independent_and_enforce_the_limit(fixture: &Fixture) {
    let isolation: String = sqlx::query_scalar("SELECT @@SESSION.transaction_isolation")
        .fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(isolation, "READ-COMMITTED");
    for limit in [None, Some(2_u64)] {
        if let Some(limit) = limit {
            sqlx::query("INSERT INTO quota_policy_projection(tenant_id,dimension_key,hard_limit,updated_by) VALUES(?,'execution_concurrency',?,?)")
                .bind(fixture.tenant_id).bind(limit).bind(Uuid::now_v7())
                .execute(&fixture.state.pool).await.unwrap();
        }
        let mut executions = BTreeSet::new();
        for _ in 0..20 {
            let accepted = create_invocation(
                &fixture.state.pool, fixture.tenant_id, fixture.application_id, fixture.key_id,
                &InvocationRequestV1 {
                    input: json!({"message":"concurrent quota"}),
                    idempotency_key: format!("quota-concurrency:{}", Uuid::now_v7()),
                },
            ).await.unwrap();
            executions.insert(accepted.execution_id);
        }
        let owner = Uuid::now_v7();
        let commands = claim_commands(&fixture.state.pool, owner, 100).await.unwrap();
        let mut starts = tokio::task::JoinSet::new();
        for command in commands.into_iter().filter(|claim| executions.contains(&claim.execution_id)) {
            let state = fixture.state.clone();
            starts.spawn(async move { process_command_with_state(&state, &command).await });
        }
        assert_eq!(starts.len(), 20);
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(result) = starts.join_next().await {
                result.unwrap().expect("independent reservations must not deadlock");
            }
        }).await.expect("concurrent starts must not block on missing-key gap locks");
        let mut running = 0;
        let mut failed = 0;
        for id in &executions {
            let status: String = sqlx::query_scalar("SELECT status FROM workflow_executions WHERE id=?")
                .bind(id).fetch_one(&fixture.state.pool).await.unwrap();
            match status.as_str() {
                "running" => running += 1,
                "failed" => failed += 1,
                _ => panic!("unexpected execution status {status}"),
            }
        }
        let expected = limit.unwrap_or(20);
        assert_eq!(running, expected, "fresh reads under the policy lock must enforce the limit");
        assert_eq!(failed, 20 - expected);
        let worker = Uuid::now_v7();
        let mut claims = tokio::task::JoinSet::new();
        for _ in 0..expected {
            let dispatch = claim_dispatch(&fixture.state.pool, owner).await.unwrap().unwrap();
            let task = dispatch.task().unwrap();
            assert!(executions.contains(&task.execution_id));
            complete_dispatch(&fixture.state.pool, &dispatch).await.unwrap();
            agentx_v2_runtime::engine::register_worker(
                &fixture.state.pool, worker, task.capability.as_str(), env!("CARGO_PKG_VERSION"),
            ).await.unwrap();
            let pool = fixture.state.pool.clone();
            claims.spawn(async move {
                agentx_v2_runtime::engine::claim_worker_attempt(
                    &pool, worker, task.capability.as_str(), &task,
                ).await.unwrap().unwrap()
            });
        }
        let mut claimed = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(result) = claims.join_next().await {
                claimed.push(result.unwrap());
            }
        }).await.expect("one Worker must claim independent Attempts without a shared lease-token lock");
        let lease_rows: (i64, i64) = sqlx::query_as("SELECT COUNT(*),COUNT(DISTINCT node_attempt_id) FROM worker_leases WHERE tenant_id=? AND worker_id=? AND released_at IS NULL")
            .bind(fixture.tenant_id).bind(worker).fetch_one(&fixture.state.pool).await.unwrap();
        assert_eq!(lease_rows, (expected as i64, expected as i64), "each Attempt must own its lease row even on the same Worker");
        for claim in claimed {
            agentx_v2_runtime::engine::submit_worker_result(
                &fixture.state.pool, &successful_worker_result(&claim),
            ).await.unwrap();
        }
        let released: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM worker_leases WHERE tenant_id=? AND worker_id=? AND released_at IS NOT NULL")
            .bind(fixture.tenant_id).bind(worker).fetch_one(&fixture.state.pool).await.unwrap();
        assert_eq!(released, expected as i64);
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quota_reservations WHERE tenant_id=? AND status='active'")
            .bind(fixture.tenant_id).fetch_one(&fixture.state.pool).await.unwrap();
        assert_eq!(active, 0);
        sqlx::query("DELETE FROM quota_policy_projection WHERE tenant_id=? AND dimension_key='execution_concurrency'")
            .bind(fixture.tenant_id).execute(&fixture.state.pool).await.unwrap();
    }
}
