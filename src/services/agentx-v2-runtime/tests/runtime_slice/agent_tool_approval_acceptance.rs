async fn protected_agent_tool_pending_and_resume(fixture:&Fixture,original:&agentx_v2_runtime::engine::ClaimedWorkerAttempt,tool:Uuid,other_grant:Uuid) {
    use agentx_runtime_contracts::{RuntimeMcpToolPolicyV1,RuntimeResourceConfigurationV1,ToolApprovalRuleV1};
    let old_status:String=sqlx::query_scalar("SELECT status FROM workflow_executions WHERE id=?").bind(original.task.execution_id).fetch_one(&fixture.state.pool).await.unwrap();
    sqlx::query("UPDATE workflow_executions SET status='running' WHERE id=?").bind(original.task.execution_id).execute(&fixture.state.pool).await.unwrap();
    sqlx::query("INSERT IGNORE INTO runtime_user_admission(tenant_id,user_id,user_name,department_id,department_name,token_version,status,tenant_query_enabled,role_assignments_json,admission_epoch) VALUES(?,?,'Approver',UUID_TO_BIN(UUID()),'Approvals',1,'active',FALSE,JSON_ARRAY(),1)").bind(fixture.tenant_id).bind(fixture.identity_id).execute(&fixture.state.pool).await.unwrap();
    sqlx::query("UPDATE runtime_user_admission SET can_approve=TRUE WHERE tenant_id=? AND user_id=?").bind(fixture.tenant_id).bind(fixture.identity_id).execute(&fixture.state.pool).await.unwrap();
    for (approved, stored_receipt) in [(true,None),(false,None),(true,Some("succeeded")),(true,Some("sent")),(true,Some("revoked_unsent"))] {
        sqlx::query("UPDATE resource_grant_projection SET status='active' WHERE grant_id=?").bind(other_grant).execute(&fixture.state.pool).await.unwrap();
        let mut claim=original.clone();
        claim.task.attempt_id=Uuid::now_v7();claim.task.node_execution_id=Uuid::now_v7();claim.lease.attempt_id=claim.task.attempt_id;
        claim.task.deadline_at=OffsetDateTime::now_utc()+time::Duration::minutes(5);
        let policy=RuntimeMcpToolPolicyV1{revision:1,content_hash:agentx_runtime_contracts::content_hash(&json!({"approval":"manager"})).unwrap(),approval:Some(ToolApprovalRuleV1{user_ids:vec![fixture.identity_id],role_ids:vec![],timeout_seconds:300})};
        let binding=claim.resources.iter_mut().find(|binding|binding.resource_id==tool).unwrap();
        if let RuntimeResourceConfigurationV1::Mcp{tool_policy,side_effect,..}=&mut binding.configuration {*tool_policy=Some(policy.clone());*side_effect="irreversible".into();}
        binding.content_hash=agentx_runtime_contracts::content_hash(&binding.configuration).unwrap();
        sqlx::query("UPDATE runtime_resource_states SET content_hash=? WHERE tenant_id=? AND resource_id=?").bind(binding.content_hash.as_str()).bind(fixture.tenant_id).bind(tool).execute(&fixture.state.pool).await.unwrap();
        sqlx::query("INSERT INTO mcp_tool_policy_projection(tenant_id,tool_id,revision,content_hash,enabled) VALUES(?,?,1,?,TRUE) ON DUPLICATE KEY UPDATE enabled=TRUE").bind(fixture.tenant_id).bind(tool).bind(policy.content_hash.as_str()).execute(&fixture.state.pool).await.unwrap();
        sqlx::query("INSERT INTO node_executions(id,tenant_id,execution_id,node_id,node_key,node_name,node_type,node_version,generation,activation_slot,run_index,capability,status) VALUES(?,?,?,?,'approval-agent','Approval Agent','agent',1,0,0,0,'agent','running')").bind(claim.task.node_execution_id).bind(fixture.tenant_id).bind(claim.task.execution_id).bind(format!("approval-agent-{}",claim.task.attempt_id)).execute(&fixture.state.pool).await.unwrap();
        sqlx::query("INSERT INTO node_attempts(id,tenant_id,execution_id,node_execution_id,attempt_number,capability,compiler_version,manifest_version,status,idempotency_key,lease_token,fencing_token,locked_until,deadline_at) VALUES(?,?,?,?,1,'agent','test','test','running',?,?,?,TIMESTAMPADD(MINUTE,5,UTC_TIMESTAMP(6)),?)").bind(claim.task.attempt_id).bind(fixture.tenant_id).bind(claim.task.execution_id).bind(claim.task.node_execution_id).bind(format!("approval:spike:{}",claim.task.attempt_id)).bind(claim.lease.worker_id).bind(claim.lease.fencing_token).bind(claim.task.deadline_at).execute(&fixture.state.pool).await.unwrap();
        let probe=Arc::new(AgentAuthorizationProbe{pool:fixture.state.pool.clone(),tenant_id:fixture.tenant_id,revoked_grant_id:other_grant,tool_to_call:"tool_b".into(),model_calls:std::sync::atomic::AtomicUsize::new(0),visible_tools:Mutex::new(Vec::new()),mcp_writes:std::sync::atomic::AtomicUsize::new(0)});
        let worker=test_worker(fixture,StubWorkerMode::AgentAuthorization(probe.clone()));
        let first=worker.execute(&claim).await;
        assert_eq!(first.status,WorkerResultStatusV1::Suspended,"{:?} {:?}",first.error_code,first.error_message);
        assert_eq!(probe.mcp_writes.load(Ordering::SeqCst),0,"No tool write is allowed before approval");
        let task:(Uuid,Value)=sqlx::query_as("SELECT id,subject_json FROM approval_tasks WHERE tenant_id=? AND tool_effect_id IN (SELECT id FROM agent_tool_effects WHERE attempt_id=?)").bind(fixture.tenant_id).bind(claim.task.attempt_id).fetch_one(&fixture.state.pool).await.unwrap();
        assert_eq!(task.1["kind"],"agent_tool_effect");
        let operation:String=sqlx::query_scalar("SELECT operation_id FROM agent_tool_effects WHERE attempt_id=?").bind(claim.task.attempt_id).fetch_one(&fixture.state.pool).await.unwrap();
        let again=worker.execute(&claim).await;
        assert_eq!(again.status,WorkerResultStatusV1::Suspended);
        assert_eq!(probe.model_calls.load(Ordering::SeqCst),1,"Pending must retain the original model result");
        sqlx::query("UPDATE approval_tasks SET status='decided',decision_id=?,decided_by=?,decided_at=UTC_TIMESTAMP(6),decision_reason='business review',resume_status='pending' WHERE id=?").bind(if approved{"approved"}else{"rejected"}).bind(fixture.identity_id).bind(task.0).execute(&fixture.state.pool).await.unwrap();
        // A replacement Worker owns a new lease, but all business identities stay fixed.
        claim.lease.worker_id=Uuid::now_v7();claim.lease.fencing_token+=1;
        sqlx::query("UPDATE node_attempts SET lease_token=?,fencing_token=? WHERE id=?").bind(claim.lease.worker_id).bind(claim.lease.fencing_token).bind(claim.task.attempt_id).execute(&fixture.state.pool).await.unwrap();
        if stored_receipt==Some("revoked_unsent") {
            sqlx::query("UPDATE mcp_tool_policy_projection SET enabled=FALSE WHERE tenant_id=? AND tool_id=?").bind(fixture.tenant_id).bind(tool).execute(&fixture.state.pool).await.unwrap();
        } else if let Some(status) = stored_receipt {
            // Inject the durable crash boundary, not a second execution: the
            // original Provider receipt exists, while Core has not consumed it.
            let (run,index):(Uuid,u32)=sqlx::query_as("SELECT agent_run_id,runtime_call_index FROM agent_tool_effects WHERE attempt_id=?").bind(claim.task.attempt_id).fetch_one(&fixture.state.pool).await.unwrap();
            sqlx::query("INSERT INTO runtime_calls(id,tenant_id,execution_id,node_execution_id,attempt_id,agent_run_id,call_index,call_kind,idempotency_key,request_fingerprint,resource_type,resource_id,side_effect,status,response_json) VALUES(?,?,?,?,?,?,?,'mcp_tool',?,?,'mcp_tool',?,'irreversible',?,?)")
                .bind(Uuid::now_v7()).bind(fixture.tenant_id).bind(claim.task.execution_id).bind(claim.task.node_execution_id).bind(claim.task.attempt_id).bind(run).bind(index)
                .bind(format!("approval-receipt:{}",claim.task.attempt_id)).bind("0".repeat(64)).bind(tool).bind(status)
                .bind(if status=="succeeded"{Some(json!({"content":[{"type":"text","text":"previously persisted tool-result"}]}))}else{None})
                .execute(&fixture.state.pool).await.unwrap();
            // A later policy change must not erase a known or unknown result.
            sqlx::query("UPDATE mcp_tool_policy_projection SET enabled=FALSE WHERE tenant_id=? AND tool_id=?").bind(fixture.tenant_id).bind(tool).execute(&fixture.state.pool).await.unwrap();
        }
        let resumed=worker.execute(&claim).await;
        if stored_receipt==Some("revoked_unsent") {
            assert_eq!(resumed.status,WorkerResultStatusV1::Failed,"{:?} {:?}",resumed.error_code,resumed.error_message);
            assert_eq!(probe.mcp_writes.load(Ordering::SeqCst),0);
            let (result,unknown):(Value,bool)=sqlx::query_as("SELECT result_json,outcome_unknown FROM agent_tool_effects WHERE attempt_id=?").bind(claim.task.attempt_id).fetch_one(&fixture.state.pool).await.unwrap();
            assert!(result.to_string().contains("MCP_TOOL_POLICY_CHANGED"));assert!(!unknown);
            let duplicate=worker.execute(&claim).await;assert_eq!(duplicate.status,WorkerResultStatusV1::Failed);
            assert_eq!(probe.mcp_writes.load(Ordering::SeqCst),0,"A denied Effect remains a durable denial");
            continue;
        }
        if stored_receipt==Some("sent") {
            assert_eq!(resumed.status,WorkerResultStatusV1::OutcomeUnknown,"{:?} {:?}",resumed.error_code,resumed.error_message);
            assert_eq!(probe.mcp_writes.load(Ordering::SeqCst),0,"A dispatched write cannot replay after policy revocation");
            let unknown:bool=sqlx::query_scalar("SELECT outcome_unknown FROM agent_tool_effects WHERE attempt_id=?").bind(claim.task.attempt_id).fetch_one(&fixture.state.pool).await.unwrap();
            assert!(unknown);
            continue;
        }
        assert_eq!(resumed.status,WorkerResultStatusV1::Succeeded,"{:?} {:?}",resumed.error_code,resumed.error_message);
        let expected_writes=usize::from(approved && stored_receipt.is_none());
        assert_eq!(probe.mcp_writes.load(Ordering::SeqCst),expected_writes);
        if stored_receipt==Some("succeeded") {
            let result:Value=sqlx::query_scalar("SELECT result_json FROM agent_tool_effects WHERE attempt_id=?").bind(claim.task.attempt_id).fetch_one(&fixture.state.pool).await.unwrap();
            assert!(result.to_string().contains("previously persisted tool-result"),"Core must receive the original receipt, not a later authorization error");
        }
        let identity:(String,u64)=sqlx::query_as("SELECT f.operation_id,CAST(COUNT(*) OVER() AS UNSIGNED) effects FROM agent_tool_effects f WHERE f.attempt_id=?").bind(claim.task.attempt_id).fetch_one(&fixture.state.pool).await.unwrap();
        assert_eq!(identity,(operation,1));
        let duplicate=worker.execute(&claim).await;
        assert_eq!(duplicate.status,WorkerResultStatusV1::Succeeded);
        assert_eq!(probe.mcp_writes.load(Ordering::SeqCst),expected_writes,"Settled effect must not write again");
    }
    sqlx::query("UPDATE workflow_executions SET status=? WHERE id=?").bind(old_status).bind(original.task.execution_id).execute(&fixture.state.pool).await.unwrap();
}
