async fn agent_attachment_revocation_is_tool_scoped(fixture: &Fixture) {
    use agentx_runtime_contracts::{
        AgentAttachmentRegistryV1, AgentAttachmentToolV1,
        AgentCapabilityAuthorizationEvidenceV1, RuntimeMcpTransportV2,
        RuntimeResourceBindingV1, RuntimeResourceConfigurationV1,
    };

    let execution_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow_executions WHERE tenant_id=? ORDER BY created_at,id LIMIT 1",
    )
    .bind(fixture.tenant_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let original_authorization: Value = sqlx::query_scalar(
        "SELECT authorization_snapshot_json FROM execution_snapshots WHERE tenant_id=? AND execution_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let mut authorization: RuntimeAuthorizationSnapshotV1 =
        serde_json::from_value(original_authorization.clone()).unwrap();

    let server_id = Uuid::now_v7();
    let server_version_id = Uuid::now_v7();
    let tool_a_id = Uuid::now_v7();
    let tool_a_version_id = Uuid::now_v7();
    let tool_b_id = Uuid::now_v7();
    let tool_b_version_id = Uuid::now_v7();
    let server_grant_id = Uuid::now_v7();
    let tool_a_grant_id = Uuid::now_v7();
    let tool_b_grant_id = Uuid::now_v7();
    authorization.grant_ids = vec![server_grant_id, tool_a_grant_id, tool_b_grant_id];
    authorization.grant_bindings = vec![
        agentx_runtime_contracts::RuntimeGrantBindingV1 {
            grant_id: server_grant_id,
            resource_type: "mcp_server".into(),
            resource_id: server_id,
            resource_version_id: Some(server_version_id),
            operation: "use".into(),
        },
        agentx_runtime_contracts::RuntimeGrantBindingV1 {
            grant_id: tool_a_grant_id,
            resource_type: "mcp_tool".into(),
            resource_id: tool_a_id,
            resource_version_id: Some(tool_a_version_id),
            operation: "use".into(),
        },
        agentx_runtime_contracts::RuntimeGrantBindingV1 {
            grant_id: tool_b_grant_id,
            resource_type: "mcp_tool".into(),
            resource_id: tool_b_id,
            resource_version_id: Some(tool_b_version_id),
            operation: "use".into(),
        },
    ];
    sqlx::query(
        "UPDATE execution_snapshots SET authorization_snapshot_json=? WHERE tenant_id=? AND execution_id=?",
    )
    .bind(serde_json::to_value(&authorization).unwrap())
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();

    for (grant_id, resource_type, resource_id) in [
        (server_grant_id, "mcp_server", server_id),
        (tool_a_grant_id, "mcp_tool", tool_a_id),
        (tool_b_grant_id, "mcp_tool", tool_b_id),
    ] {
        sqlx::query("INSERT INTO resource_grant_projection(grant_id,tenant_id,subject_id,resource_type,resource_id,operations_json,policy_epoch,status) VALUES(?,?,?,?,?,? ,?,'active')")
            .bind(grant_id)
            .bind(fixture.tenant_id)
            .bind(authorization.service_identity_id)
            .bind(resource_type)
            .bind(resource_id)
            .bind(json!(["use"]))
            .bind(authorization.policy_epoch)
            .execute(&fixture.state.pool)
            .await
            .unwrap();
    }

    let transport = RuntimeMcpTransportV2::StreamableHttp {
        endpoint: "https://provider.example.test/mcp".into(),
    };
    let mcp_configuration = |tool_name: &str| RuntimeResourceConfigurationV1::Mcp {
        server_id,
        server_version_id,
        transport: transport.clone(),
        tool_name: tool_name.into(),
        tool_version: "1".into(),
        input_schema_hash: agentx_runtime_contracts::content_hash(
            &json!({"type":"object","required":["value"]}),
        )
        .unwrap(),
        input_schema: json!({
            "type":"object",
            "additionalProperties":false,
            "required":["value"],
            "properties":{"value":{"type":"string"}}
        }),
        output_schema: None,
        side_effect: "read_only".into(),
        timeout_seconds: 10,
        credential: None,
    };
    let server_configuration = mcp_configuration("__server__");
    let tool_a_configuration = mcp_configuration("tool_a");
    let tool_b_configuration = mcp_configuration("tool_b");
    let resources = vec![
        RuntimeResourceBindingV1 {
            resource_kind: RuntimeResourceKindV1::Mcp,
            resource_id: server_id,
            resource_version: server_version_id.to_string(),
            state_epoch: 1,
            content_hash: agentx_runtime_contracts::content_hash(&server_configuration).unwrap(),
            configuration: server_configuration,
            object_ids: vec![],
        },
        RuntimeResourceBindingV1 {
            resource_kind: RuntimeResourceKindV1::Mcp,
            resource_id: tool_a_id,
            resource_version: tool_a_version_id.to_string(),
            state_epoch: 1,
            content_hash: agentx_runtime_contracts::content_hash(&tool_a_configuration).unwrap(),
            configuration: tool_a_configuration,
            object_ids: vec![],
        },
        RuntimeResourceBindingV1 {
            resource_kind: RuntimeResourceKindV1::Mcp,
            resource_id: tool_b_id,
            resource_version: tool_b_version_id.to_string(),
            state_epoch: 1,
            content_hash: agentx_runtime_contracts::content_hash(&tool_b_configuration).unwrap(),
            configuration: tool_b_configuration,
            object_ids: vec![],
        },
    ];
    for resource in &resources {
        sqlx::query("INSERT INTO runtime_resource_states(tenant_id,resource_kind,resource_id,resource_version,state_epoch,status,content_hash) VALUES(?,'mcp',?,?,1,'active',?)")
            .bind(fixture.tenant_id)
            .bind(resource.resource_id)
            .bind(&resource.resource_version)
            .bind(resource.content_hash.as_str())
            .execute(&fixture.state.pool)
            .await
            .unwrap();
    }

    let evidence = |resource_type: &str,
                    resource_id: Uuid,
                    resource_version_id: Uuid,
                    grant_id: Uuid| AgentCapabilityAuthorizationEvidenceV1 {
        resource_type: resource_type.into(),
        resource_id,
        resource_version_id: Some(resource_version_id),
        operation: "use".into(),
        policy_epoch: authorization.policy_epoch,
        grant_ids: vec![grant_id],
    };
    let tool = |name: &str, resource_id: Uuid, resource_version_id: Uuid| {
        AgentAttachmentToolV1 {
            name: name.into(),
            description: format!("{name} authorization probe"),
            input_schema: json!({
                "type":"object","additionalProperties":false,"required":["value"],
                "properties":{"value":{"type":"string"}}
            }),
            origin: "mcp".into(),
            replay_policy: "safe".into(),
            resource_id,
            resource_version_id,
            operation: "use".into(),
            scope: "run".into(),
        }
    };
    let registry = AgentAttachmentRegistryV1 {
        contexts: vec![],
        tools: vec![
            tool("tool_a", tool_a_id, tool_a_version_id),
            tool("tool_b", tool_b_id, tool_b_version_id),
        ],
        authorization_evidence: vec![
            evidence("mcp_server", server_id, server_version_id, server_grant_id),
            evidence("mcp_tool", tool_a_id, tool_a_version_id, tool_a_grant_id),
            evidence("mcp_tool", tool_b_id, tool_b_version_id, tool_b_grant_id),
        ],
    };

    let model_id = Uuid::now_v7();
    let model_configuration = RuntimeResourceConfigurationV1::Model {
        provider: "openai_compatible".into(),
        endpoint: "https://provider.example.test/model".into(),
        model: "authorization-fixture".into(),
        context_window: 128_000,
        price: agentx_runtime_contracts::RuntimeModelPriceV1 {
            version_id: "price:authorization".into(),
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
            compatibility_hash: agentx_runtime_contracts::content_hash(&json!({"agent":1.1}))
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
                "contractVersion":"1.1","bundleHash":"attachment-bundle","definitionHash":"attachment-definition",
                "stableAgentNodeKey":"attachment-agent","sessionPolicy":"invocation",
                "model":{"resourceId":model_id.to_string(),"resourceVersionId":"model:authorization"},
                "attachments":[{"resourceType":"mcp_tool"}],"coreTools":[],
                "attachmentRegistry":registry
            }
        }),
        per_item_parameters: vec![],
        string_conversions: json!({"common":[],"perItem":[]}),
        inputs: BTreeMap::from([(
            "main".into(),
            vec![agentx_node_protocol::Item {
                json: json!({"question":"verify scoped revocation"}),
                ..Default::default()
            }],
        )]),
        resources: std::iter::once(RuntimeResourceBindingV1 {
            resource_kind: RuntimeResourceKindV1::Model,
            resource_id: model_id,
            resource_version: "model:authorization".into(),
            state_epoch: 1,
            content_hash: agentx_runtime_contracts::content_hash(&model_configuration).unwrap(),
            configuration: model_configuration,
            object_ids: vec![],
        })
        .chain(resources)
        .collect(),
        context: json!({}),
        plugin: None,
        trace_parent_span_entity_id: None,
    };
    let probe = Arc::new(AgentAuthorizationProbe {
        pool: fixture.state.pool.clone(),
        tenant_id: fixture.tenant_id,
        revoked_grant_id: tool_a_grant_id,
        tool_to_call: "tool_b".into(),
        model_calls: std::sync::atomic::AtomicUsize::new(0),
        visible_tools: Mutex::new(Vec::new()),
    });
    let worker = test_worker(
        fixture,
        StubWorkerMode::AgentAuthorization(probe.clone()),
    );
    let result = worker.execute(&claim).await;
    assert_eq!(
        result.status,
        WorkerResultStatusV1::Succeeded,
        "scoped revocation failed: {:?} {:?}",
        result.error_code,
        result.error_message
    );
    let visible = probe.visible_tools.lock().unwrap().clone();
    assert_eq!(visible.len(), 2);
    assert!(visible[0].contains(&"tool_a".to_owned()));
    assert!(visible[0].contains(&"tool_b".to_owned()));
    assert!(!visible[1].contains(&"tool_a".to_owned()));
    assert!(visible[1].contains(&"tool_b".to_owned()));

    let settled_calls: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_calls WHERE attempt_id=? AND status='succeeded'",
    )
    .bind(attempt_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(settled_calls, 3, "two Model Effects and one MCP Effect settle once");

    sqlx::query(
        "UPDATE execution_snapshots SET authorization_snapshot_json=? WHERE tenant_id=? AND execution_id=?",
    )
    .bind(original_authorization)
    .bind(fixture.tenant_id)
    .bind(execution_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
}
