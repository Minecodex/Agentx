use std::collections::BTreeSet;

use agentx_domain::{ContextDefinition, ExecutionOrder, WorkflowEnd, WorkflowStart};
use agentx_runtime_contracts::{
    ApplyReceiptV1, BUNDLE_SCHEMA_VERSION, CompiledNodeV1, CompiledWorkflowV1, DependencyClosureV1,
    ExecutionDetailV1, ExecutionOriginV1, ExecutionResult, ExecutionSearchPageV1,
    ExecutionSearchRequestV1, ExecutionSpecBundleV2, ExecutionSpecPayloadV2, ExecutionSummaryV1,
    IR_SCHEMA_VERSION, PublishReceiptStatusV1, PublishReceiptV1, RuntimeAuthorizationSnapshotV1,
    RuntimeCallPurposeV1, RuntimeCommand, RuntimeCommandType, RuntimeEventEnvelope,
    RuntimeEventPayloadV1, RuntimeMcpTransportV2, RuntimeObjectReferenceV1, RuntimePolicyV1,
    RuntimeRetentionItemV1, RuntimeWorkPackageOverlayV1, RuntimeWorkPackagePayloadV1,
    RuntimeWorkPackageV1, StorageDomain, TraceContentKindV1, TraceContentV1, TraceEventEnvelopeV1,
    TraceEventKindV1, TraceSpanDetailV1, TraceSpanKindV1, TraceSpanSummaryV1,
    WORK_PACKAGE_SCHEMA_VERSION, WorkPackagePurpose, WorkerCompatibilityV1,
};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

#[test]
fn unknown_fields_and_unsupported_versions_are_rejected() {
    let mut value = serde_json::to_value(compiled_workflow()).unwrap();
    value["unknownField"] = json!(true);
    assert!(serde_json::from_value::<CompiledWorkflowV1>(value).is_err());

    let mut value = serde_json::to_value(compiled_workflow()).unwrap();
    value["contractVersion"] = json!(2);
    assert!(serde_json::from_value::<CompiledWorkflowV1>(value).is_err());
}

#[test]
fn mcp_runtime_transport_is_strictly_tagged_and_scope_safe() {
    let sandbox_id = Uuid::now_v7();
    let sandbox_version_id = Uuid::now_v7();
    let valid = json!({
        "kind":"stdio",
        "command":"/usr/local/bin/mcp-server",
        "args":["--stdio"],
        "environmentCredentialRefs":[],
        "runtimeSandbox":{
            "resourceId":sandbox_id,
            "resourceVersionId":sandbox_version_id
        }
    });
    assert!(serde_json::from_value::<RuntimeMcpTransportV2>(valid).is_ok());

    let old_flat = json!({
        "transport":"streamable_http",
        "endpoint":"https://mcp.example/rpc"
    });
    assert!(serde_json::from_value::<RuntimeMcpTransportV2>(old_flat).is_err());

    let http_with_sandbox = json!({
        "kind":"streamable_http",
        "endpoint":"https://mcp.example/rpc",
        "runtimeSandbox":{
            "resourceId":sandbox_id,
            "resourceVersionId":sandbox_version_id
        }
    });
    assert!(serde_json::from_value::<RuntimeMcpTransportV2>(http_with_sandbox).is_err());

    let stdio_with_endpoint = json!({
        "kind":"stdio",
        "command":"/usr/local/bin/mcp-server",
        "args":[],
        "environmentCredentialRefs":[],
        "runtimeSandbox":{
            "resourceId":sandbox_id,
            "resourceVersionId":sandbox_version_id
        },
        "endpoint":"https://mcp.example/rpc"
    });
    assert!(serde_json::from_value::<RuntimeMcpTransportV2>(stdio_with_endpoint).is_err());
}

#[test]
fn governance_event_payloads_round_trip_without_execution_context() {
    let event = RuntimeEventPayloadV1::RetentionChanged {
        run_id: Uuid::now_v7(),
        run_version: 7,
        status: "completed".into(),
        marked_count: 2,
        deleted_count: 1,
        failed_count: 0,
        dry_run: false,
        items: vec![RuntimeRetentionItemV1 {
            id: Uuid::now_v7(),
            data_type: "execution".into(),
            target_id: Uuid::now_v7().to_string(),
            status: "deleted".into(),
            reason: None,
            attempt_count: 1,
        }],
    };

    let encoded = serde_json::to_value(&event).unwrap();
    let decoded: RuntimeEventPayloadV1 = serde_json::from_value(encoded).unwrap();
    assert!(matches!(
        decoded,
        RuntimeEventPayloadV1::RetentionChanged { .. }
    ));
}

#[test]
fn runtime_trigger_contract_rejects_unknown_fields_and_version_two() {
    let trigger = json!({
        "schemaVersion":1,
        "triggerId":Uuid::now_v7(),
        "triggerName":"Nightly schedule",
        "applicationId":Uuid::now_v7(),
        "nodeId":"schedule",
        "revision":1,
        "configurationHash":format!("sha256:{}","a".repeat(64)),
        "enabled":true,
        "configuration":{"kind":"schedule","cronExpression":"0 0 * * * * *","timezone":"UTC","misfirePolicy":"fire_once","graceSeconds":60,"input":{}}
    });
    assert!(
        serde_json::from_value::<agentx_runtime_contracts::RuntimeTriggerSpecV1>(trigger.clone())
            .is_ok()
    );
    let mut wrong = trigger.clone();
    wrong["schemaVersion"] = json!(2);
    assert!(
        serde_json::from_value::<agentx_runtime_contracts::RuntimeTriggerSpecV1>(wrong).is_err()
    );
    let mut unknown = trigger;
    unknown["extra"] = json!(true);
    assert!(
        serde_json::from_value::<agentx_runtime_contracts::RuntimeTriggerSpecV1>(unknown).is_err()
    );
}

#[test]
fn execution_origin_and_search_filters_round_trip_without_compatibility_fields() {
    let user_id = Uuid::now_v7();
    let department_id = Uuid::now_v7();
    let trigger_source_id = Uuid::now_v7();
    let origin = ExecutionOriginV1 {
        initiator_user_id: Some(user_id),
        initiator_user_name: Some("Historical user".into()),
        initiator_department_id: Some(department_id),
        initiator_department_name: Some("Historical department".into()),
        trigger_source_id: Some(trigger_source_id),
        trigger_name: Some("Historical trigger".into()),
        role_assignments: vec![],
    };
    let encoded = serde_json::to_value(&origin).unwrap();
    assert_eq!(encoded["initiatorUserId"], user_id.to_string());
    assert_eq!(encoded["triggerName"], "Historical trigger");
    assert!(serde_json::from_value::<ExecutionOriginV1>(encoded.clone()).is_ok());
    let mut unknown = encoded;
    unknown["currentUserName"] = json!("compatibility alias");
    assert!(serde_json::from_value::<ExecutionOriginV1>(unknown).is_err());

    let request = ExecutionSearchRequestV1 {
        api_version: 1,
        tenant_id: Uuid::now_v7(),
        application_ids: vec![Uuid::now_v7()],
        workflow_ids: vec![Uuid::now_v7()],
        tool_ids: vec![Uuid::now_v7()],
        initiator_user_ids: vec![user_id],
        initiator_department_ids: vec![department_id],
        trigger_types: vec!["schedule".into()],
        trigger_name: Some("nightly".into()),
        statuses: vec!["succeeded".into()],
        session_mode: agentx_runtime_contracts::ExecutionSessionModeV1::All,
        created_after: Some(OffsetDateTime::UNIX_EPOCH),
        created_before: None,
        search: Some("TRACE_FAILURE".into()),
        cursor: None,
        limit: 8,
    };
    let encoded = serde_json::to_value(&request).unwrap();
    let decoded: ExecutionSearchRequestV1 = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded.tool_ids, request.tool_ids);
    assert_eq!(decoded.initiator_department_ids, vec![department_id]);
    assert_eq!(decoded.trigger_name.as_deref(), Some("nightly"));
    let mut unknown = encoded;
    unknown["departmentIds"] = json!([]);
    assert!(serde_json::from_value::<ExecutionSearchRequestV1>(unknown).is_err());
}

#[test]
fn every_top_level_wire_family_rejects_version_two() {
    let tenant_id = agentx_domain::TenantId::new();
    assert_version_rejected(
        RuntimeCommand::new(
            tenant_id,
            RuntimeCommandType::StartExecution,
            "workflow",
            Uuid::now_v7().to_string(),
            "command-v1",
            json!({}),
        ),
        "schemaVersion",
    );
    assert_version_rejected(
        RuntimeEventEnvelope::new(
            tenant_id,
            "execution.started",
            "execution",
            Uuid::now_v7().to_string(),
            None,
            Some(1),
            json!({}),
        ),
        "schemaVersion",
    );
    assert_version_rejected(compiled_workflow(), "contractVersion");
    assert_version_rejected(
        WorkerCompatibilityV1 {
            protocol_version: 1,
            ir_versions: BTreeSet::from([1]),
            compiler_versions: BTreeSet::new(),
            capabilities: BTreeSet::new(),
            manifest_versions: BTreeSet::new(),
        },
        "protocolVersion",
    );
    assert_version_rejected(
        ApplyReceiptV1 {
            api_version: 1,
            event_id: Uuid::now_v7(),
            applied: true,
            replayed: false,
            object_version: 1,
            result: json!({}),
        },
        "apiVersion",
    );
    assert_version_rejected(
        ExecutionSearchPageV1 {
            api_version: 1,
            snapshot_id: Uuid::now_v7(),
            snapshot_upper_bound: "2026-01-01T00:00:00Z".into(),
            total: 0,
            items: vec![],
            next: None,
        },
        "apiVersion",
    );
    assert_version_rejected(
        ExecutionResult {
            schema_version: 1,
            outputs: json!({}),
            output_hash: format!("sha256:{}", "0".repeat(64)),
            error: None,
        },
        "schemaVersion",
    );
}

#[test]
fn internal_api_response_wrappers_reject_version_two() {
    let summary = ExecutionSummaryV1 {
        execution_id: Uuid::now_v7(),
        invocation_id: None,
        application_id: None,
        workflow_id: Uuid::now_v7(),
        workflow_version_id: Uuid::now_v7(),
        session_id: None,
        parent_execution_id: None,
        bundle_id: Uuid::now_v7(),
        trace_id: Uuid::now_v7(),
        trigger_type: "application".into(),
        initiator_user_id: None,
        initiator_user_name: None,
        initiator_department_id: None,
        initiator_department_name: None,
        trigger_source_id: None,
        trigger_name: None,
        status: "running".into(),
        duration_ms: None,
        cost_micros: 0,
        cost_currency: None,
        input_tokens: 0,
        output_tokens: 0,
        error_code: None,
        created_at: OffsetDateTime::UNIX_EPOCH,
        completed_at: None,
    };
    assert_version_rejected(
        ExecutionDetailV1 {
            api_version: 1,
            summary,
            state_version: 1,
            admission_epoch: 1,
            trace_watermark: 0,
            parent_execution_id: None,
            work_package_id: None,
            input: None,
            output: None,
            error: None,
        },
        "apiVersion",
    );
    assert_version_rejected(
        PublishReceiptV1 {
            api_version: 1,
            receipt: ApplyReceiptV1 {
                api_version: 1,
                event_id: Uuid::now_v7(),
                applied: true,
                replayed: false,
                object_version: 1,
                result: json!({}),
            },
            bundle_id: Uuid::now_v7(),
            head_version: Some(1),
            activation_sequence: Some(1),
            status: PublishReceiptStatusV1::Accepted,
            rejection: None,
            accepted_at: OffsetDateTime::UNIX_EPOCH,
        },
        "apiVersion",
    );
}

#[test]
fn trace_content_contract_is_strongly_typed_and_has_no_legacy_aggregates() {
    let kinds = [
        (TraceContentKindV1::WorkflowInput, "workflow_input"),
        (TraceContentKindV1::WorkflowOutput, "workflow_output"),
        (TraceContentKindV1::NodeInput, "node_input"),
        (TraceContentKindV1::NodeOutput, "node_output"),
        (TraceContentKindV1::AttemptInput, "attempt_input"),
        (TraceContentKindV1::AttemptOutput, "attempt_output"),
        (
            TraceContentKindV1::ResolvedParameters,
            "resolved_parameters",
        ),
        (TraceContentKindV1::RuntimeRequest, "runtime_request"),
        (TraceContentKindV1::RuntimeResponse, "runtime_response"),
        (TraceContentKindV1::AgentInput, "agent_input"),
        (TraceContentKindV1::AgentOutput, "agent_output"),
        (TraceContentKindV1::IterationInput, "iteration_input"),
        (TraceContentKindV1::IterationOutput, "iteration_output"),
        (TraceContentKindV1::SandboxRequest, "sandbox_request"),
        (TraceContentKindV1::SandboxResponse, "sandbox_response"),
        (TraceContentKindV1::WaitRequest, "wait_request"),
        (TraceContentKindV1::WaitResponse, "wait_response"),
        (TraceContentKindV1::ConversionRecord, "conversion_record"),
    ];
    for (kind, expected) in kinds {
        assert_eq!(serde_json::to_value(kind).unwrap(), expected);
    }

    let event = trace_fixture_event(TraceContentKindV1::RuntimeRequest);
    let detail = TraceSpanDetailV1 {
        api_version: 1,
        execution_id: event.execution_id,
        span: TraceSpanSummaryV1 {
            span_id: event.span_id,
            parent_span_id: None,
            span_kind: TraceSpanKindV1::Boundary,
            span_name: "Start".into(),
            status: "succeeded".into(),
            started_at: event.occurred_at,
            ended_at: Some(event.occurred_at),
            duration_ms: Some(0),
            node_execution_id: None,
            attempt_id: None,
            agent_run_id: None,
            agent_iteration_id: None,
            runtime_call_id: None,
            sandbox_lease_id: None,
            wait_id: None,
            resource_type: None,
            resource_id: None,
            resource_version: None,
            input_tokens: None,
            output_tokens: None,
            cost_micros: 0,
            error_code: None,
            error_message: None,
            has_details: true,
        },
        contents: vec![TraceContentV1 {
            event_id: event.event_id,
            kind: TraceContentKindV1::RuntimeRequest,
            preview: event.content_preview.clone(),
            content_ref: None,
            occurred_at: event.occurred_at,
        }],
        events: vec![event],
    };
    let mut encoded = serde_json::to_value(detail).unwrap();
    assert!(encoded.get("input").is_none());
    assert!(encoded.get("output").is_none());
    assert!(encoded.get("attributes").is_none());
    encoded["input"] = json!({"legacy":true});
    assert!(serde_json::from_value::<TraceSpanDetailV1>(encoded).is_err());
}

fn trace_fixture_event(content_kind: TraceContentKindV1) -> TraceEventEnvelopeV1 {
    let occurred_at = OffsetDateTime::UNIX_EPOCH;
    TraceEventEnvelopeV1 {
        schema_version: 1,
        event_id: Uuid::now_v7(),
        tenant_id: Uuid::now_v7(),
        execution_id: Uuid::now_v7(),
        execution_sequence: 1,
        trace_id: Uuid::now_v7(),
        workflow_id: None,
        application_id: None,
        span_id: Uuid::now_v7(),
        parent_span_id: None,
        event_kind: TraceEventKindV1::Started,
        span_kind: TraceSpanKindV1::Boundary,
        span_name: "Start".into(),
        node_execution_id: None,
        attempt_id: None,
        agent_run_id: None,
        agent_iteration_id: None,
        runtime_call_id: None,
        sandbox_lease_id: None,
        wait_id: None,
        resource_type: None,
        resource_id: None,
        resource_version: None,
        event_type: "boundary.started".into(),
        status: "succeeded".into(),
        duration_ms: Some(0),
        input_tokens: None,
        output_tokens: None,
        cost_micros: 0,
        error_code: None,
        error_message: None,
        attributes: json!({}),
        content_ref: None,
        content_kind: Some(content_kind),
        content_preview: Some(json!({"question":"hello"})),
        occurred_at,
        content_hash: agentx_runtime_contracts::content_hash(&json!({"question":"hello"})).unwrap(),
    }
}

fn assert_version_rejected<T>(value: T, field: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let mut encoded = serde_json::to_value(value).expect("serialize versioned contract");
    assert_eq!(encoded[field], json!(1));
    encoded[field] = json!(2);
    assert!(
        serde_json::from_value::<T>(encoded).is_err(),
        "{field}=2 must be rejected"
    );
}

#[test]
fn bundle_hash_and_signature_cover_the_immutable_payload() {
    let key = SigningKey::generate(&mut OsRng);
    let mut bundle = ExecutionSpecBundleV2::signed(bundle_payload(), "bundle-current", &key)
        .expect("sign bundle");
    bundle.verify(&key.verifying_key()).expect("verify bundle");

    bundle.payload.bundle_sequence += 1;
    assert!(bundle.verify(&key.verifying_key()).is_err());
}

#[test]
fn work_package_uses_an_independent_key_and_rejects_payload_tampering() {
    let bundle_key = SigningKey::generate(&mut OsRng);
    let package_key = SigningKey::generate(&mut OsRng);
    let tenant_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    let compiled = compiled_workflow();
    let mut package = RuntimeWorkPackageV1::signed(
        RuntimeWorkPackagePayloadV1 {
            schema_version: WORK_PACKAGE_SCHEMA_VERSION,
            package_id: Uuid::now_v7(),
            tenant_id,
            workflow: agentx_runtime_contracts::ExecutionWorkflowSnapshotV1 {
                id: workflow_id,
                name: "Workflow".into(),
                version_id: Uuid::now_v7(),
                version_number: 7,
                owner_department: None,
            },
            origin: agentx_runtime_contracts::ExecutionOriginV1::system(None),
            purpose: WorkPackagePurpose::Debug,
            call_purpose: RuntimeCallPurposeV1::Debug,
            spec: agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Debug {
                draft_revision: 7,
                debug_plan: agentx_runtime_contracts::RuntimeDebugPlanV1::whole(&compiled),
            },
            model_evaluator_executions: vec![],
            source_revision: "draft:7".into(),
            definition: json!({"schemaVersion":"6.0"}),
            compiled_ir: compiled,
            node_manifests: vec![],
            agent_bundle: agentx_runtime_contracts::AgentBundleContractV2::empty(
                "test-definition",
                "test-compiler",
            ),
            overlay: RuntimeWorkPackageOverlayV1 {
                input: json!({"question":"test"}),
                ..RuntimeWorkPackageOverlayV1::default()
            },
            resource_closure: DependencyClosureV1 { entries: vec![] },
            resources: vec![],
            authorization: authorization(tenant_id, workflow_id),
            objects: vec![RuntimeObjectReferenceV1 {
                tenant_id,
                storage_domain: StorageDomain::Runtime,
                object_key: RuntimeObjectReferenceV1::canonical_key(
                    tenant_id,
                    Uuid::from_u128(42),
                    &agentx_runtime_contracts::ContentHash::parse(format!(
                        "sha256:{}",
                        "b".repeat(64)
                    ))
                    .unwrap(),
                ),
                object_id: Uuid::from_u128(42),
                content_hash: agentx_runtime_contracts::ContentHash::parse(format!(
                    "sha256:{}",
                    "b".repeat(64)
                ))
                .unwrap(),
                size_bytes: 4,
                media_type: "application/json".into(),
            }],
            runtime_policy: runtime_policy(30),
            worker_compatibility: compatibility(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH + time::Duration::minutes(10),
        },
        "work-package-current",
        &package_key,
    )
    .expect("sign work package");
    package
        .verify(&package_key.verifying_key())
        .expect("verify work package");
    assert!(package.verify(&bundle_key.verifying_key()).is_err());

    package.payload.source_revision = "draft:8".into();
    assert!(package.verify(&package_key.verifying_key()).is_err());
}

#[test]
fn v2_03_v1_bundle_and_work_package_fixtures_are_rejected_after_the_destructive_rewrite() {
    let mut old_bundle = serde_json::to_value(bundle_payload()).unwrap();
    for field in [
        "runtimePolicy",
        "authorization",
        "workerCompatibility",
        "dependencyClosure",
    ] {
        old_bundle.as_object_mut().unwrap().remove(field);
    }
    assert!(serde_json::from_value::<ExecutionSpecPayloadV2>(old_bundle).is_err());

    let tenant_id = Uuid::now_v7();
    let mut old_package = json!({
        "schemaVersion": 1,
        "packageId": Uuid::now_v7(),
        "tenantId": tenant_id,
        "purpose": "debug",
        "sourceRevision": "draft:1",
        "compiledIr": compiled_workflow(),
        "createdAt": "1970-01-01T00:00:00Z",
        "expiresAt": "1970-01-01T00:10:00Z"
    });
    assert!(serde_json::from_value::<RuntimeWorkPackagePayloadV1>(old_package.clone()).is_err());
    old_package["unknownCompatibilityAlias"] = json!({});
    assert!(serde_json::from_value::<RuntimeWorkPackagePayloadV1>(old_package).is_err());

    let mut otherwise_current = serde_json::to_value(evaluation_work_package_payload()).unwrap();
    otherwise_current
        .as_object_mut()
        .unwrap()
        .remove("modelEvaluatorExecutions");
    assert!(
        serde_json::from_value::<RuntimeWorkPackagePayloadV1>(otherwise_current).is_err(),
        "the V2-03 shape must not receive a default evaluator execution closure"
    );
}

#[test]
fn model_evaluator_resource_and_prompt_tampering_are_rejected() {
    let key = SigningKey::generate(&mut OsRng);
    let package = RuntimeWorkPackageV1::signed(
        evaluation_work_package_payload(),
        "evaluation-current",
        &key,
    )
    .unwrap();
    package.verify(&key.verifying_key()).unwrap();

    let mut resource_tampered = package.clone();
    resource_tampered.payload.model_evaluator_executions[0].resource_id = Uuid::now_v7();
    assert!(resource_tampered.verify(&key.verifying_key()).is_err());

    let mut prompt_tampered = package;
    prompt_tampered.payload.model_evaluator_executions[0].prompt_object_id = Uuid::now_v7();
    assert!(prompt_tampered.verify(&key.verifying_key()).is_err());
}

#[test]
fn external_worker_result_rejects_hash_size_and_tenant_tampering() {
    let tenant_id = Uuid::now_v7();
    let object_id = Uuid::now_v7();
    let object_hash =
        agentx_runtime_contracts::ContentHash::parse(format!("sha256:{}", "c".repeat(64))).unwrap();
    let object = RuntimeObjectReferenceV1 {
        tenant_id,
        storage_domain: StorageDomain::Runtime,
        object_id,
        object_key: RuntimeObjectReferenceV1::canonical_key(tenant_id, object_id, &object_hash),
        content_hash: object_hash,
        size_bytes: 17,
        media_type: "application/json".into(),
    };
    let placeholder =
        agentx_runtime_contracts::ContentHash::parse(format!("sha256:{}", "0".repeat(64))).unwrap();
    let mut result = agentx_runtime_contracts::WorkerResultV1 {
        protocol_version: 1,
        attempt_id: Uuid::now_v7(),
        worker_id: Uuid::now_v7(),
        fencing_token: 3,
        status: agentx_runtime_contracts::WorkerResultStatusV1::Succeeded,
        result_hash: placeholder,
        outputs: std::collections::BTreeMap::new(),
        output_object: Some(object),
        error_code: None,
        error_message: None,
        retryable: None,
        partial_output_object: None,
    };
    result.result_hash = result.computed_result_hash().unwrap();
    result.validate_integrity().unwrap();

    let mut hash_tampered = result.clone();
    hash_tampered.output_object.as_mut().unwrap().content_hash =
        agentx_runtime_contracts::ContentHash::parse(format!("sha256:{}", "d".repeat(64))).unwrap();
    assert!(hash_tampered.validate_integrity().is_err());

    let mut size_tampered = result.clone();
    size_tampered.output_object.as_mut().unwrap().size_bytes += 1;
    assert!(size_tampered.validate_integrity().is_err());

    let mut tenant_tampered = result;
    tenant_tampered.output_object.as_mut().unwrap().tenant_id = Uuid::now_v7();
    assert!(tenant_tampered.validate_integrity().is_err());
}

#[test]
fn runtime_object_key_is_tenant_object_and_hash_canonical() {
    let tenant_id = Uuid::from_u128(1);
    let object_id = Uuid::from_u128(2);
    let hash =
        agentx_runtime_contracts::ContentHash::parse(format!("sha256:{}", "a".repeat(64))).unwrap();
    let mut object = RuntimeObjectReferenceV1 {
        tenant_id,
        storage_domain: StorageDomain::Runtime,
        object_id,
        object_key: RuntimeObjectReferenceV1::canonical_key(tenant_id, object_id, &hash),
        content_hash: hash,
        size_bytes: 1,
        media_type: "application/octet-stream".into(),
    };
    assert!(object.has_canonical_key());
    object.object_key.push_str("/mutable");
    assert!(!object.has_canonical_key());
}

fn evaluation_work_package_payload() -> RuntimeWorkPackagePayloadV1 {
    let tenant_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    let evaluator_id = Uuid::now_v7();
    let resource_id = Uuid::now_v7();
    let prompt_object_id = Uuid::now_v7();
    let prompt_hash =
        agentx_runtime_contracts::ContentHash::parse(format!("sha256:{}", "e".repeat(64))).unwrap();
    let model_configuration = agentx_runtime_contracts::RuntimeResourceConfigurationV1::Model {
        capabilities: Vec::new(),
        provider: "fixture".into(),
        endpoint: "https://model.fixture/v1".into(),
        model: "evaluator-v1".into(),
        context_window: 128_000,
        price: agentx_runtime_contracts::RuntimeModelPriceV1 {
            version_id: "price:1".into(),
            currency: "USD".into(),
            input_per_million: "1".into(),
            output_per_million: "2".into(),
        },
        credential: None,
    };
    RuntimeWorkPackagePayloadV1 {
        schema_version: 1,
        package_id: Uuid::now_v7(),
        tenant_id,
        workflow: agentx_runtime_contracts::ExecutionWorkflowSnapshotV1 {
            id: workflow_id,
            name: "Workflow".into(),
            version_id: Uuid::now_v7(),
            version_number: 1,
            owner_department: None,
        },
        origin: agentx_runtime_contracts::ExecutionOriginV1::system(None),
        purpose: WorkPackagePurpose::Evaluation,
        call_purpose: RuntimeCallPurposeV1::Evaluation,
        spec: agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Evaluation {
            dataset_version_id: Uuid::now_v7(),
            profile_version_id: Uuid::now_v7(),
            cases: vec![agentx_runtime_contracts::RuntimeEvaluationCaseV1 {
                case_id: Uuid::now_v7(),
                input: json!({"message":"evaluate"}),
                expected_output: Some(json!({"message":"evaluate"})),
            }],
            evaluators: vec![agentx_runtime_contracts::RuntimeEvaluatorV1::Model {
                evaluator_id,
                resource_id,
                prompt_object_id,
            }],
        },
        model_evaluator_executions: vec![
            agentx_runtime_contracts::RuntimeModelEvaluatorExecutionV1 {
                evaluator_id,
                resource_id,
                prompt_object_id,
                definition: json!({"schemaVersion":"6.0","kind":"model_evaluator"}),
                compiled_ir: compiled_workflow(),
                node_manifests: vec![],
            },
        ],
        source_revision: "evaluation:1".into(),
        definition: json!({"schemaVersion":"6.0"}),
        compiled_ir: compiled_workflow(),
        node_manifests: vec![],
        agent_bundle: agentx_runtime_contracts::AgentBundleContractV2::empty(
            "test-definition",
            "test-compiler",
        ),
        overlay: RuntimeWorkPackageOverlayV1 {
            input: json!({}),
            ..RuntimeWorkPackageOverlayV1::default()
        },
        resource_closure: DependencyClosureV1 { entries: vec![] },
        resources: vec![agentx_runtime_contracts::RuntimeResourceBindingV1 {
            resource_kind: agentx_runtime_contracts::RuntimeResourceKindV1::Model,
            resource_id,
            resource_version: "model:1".into(),
            state_epoch: 1,
            content_hash: agentx_runtime_contracts::content_hash(&model_configuration).unwrap(),
            configuration: model_configuration,
            object_ids: vec![],
        }],
        authorization: authorization(tenant_id, workflow_id),
        objects: vec![RuntimeObjectReferenceV1 {
            tenant_id,
            storage_domain: StorageDomain::Runtime,
            object_id: prompt_object_id,
            object_key: RuntimeObjectReferenceV1::canonical_key(
                tenant_id,
                prompt_object_id,
                &prompt_hash,
            ),
            content_hash: prompt_hash,
            size_bytes: 17,
            media_type: "application/json".into(),
        }],
        runtime_policy: runtime_policy(60),
        worker_compatibility: compatibility(),
        created_at: OffsetDateTime::UNIX_EPOCH,
        expires_at: OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1),
    }
}

fn bundle_payload() -> ExecutionSpecPayloadV2 {
    let tenant_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    let object_id = Uuid::now_v7();
    let object_hash =
        agentx_runtime_contracts::ContentHash::parse(format!("sha256:{}", "a".repeat(64))).unwrap();
    ExecutionSpecPayloadV2 {
        schema_version: BUNDLE_SCHEMA_VERSION,
        bundle_id: Uuid::now_v7(),
        tenant_id,
        application_id: Uuid::now_v7(),
        deployment_id: Uuid::now_v7(),
        workflow_id,
        workflow_version_id: Uuid::now_v7(),
        workflow_name: "Workflow".into(),
        workflow_version_number: 7,
        workflow_owner_department: None,
        bundle_sequence: 7,
        definition: json!({"schemaVersion": "6.0"}),
        compiled_ir: compiled_workflow(),
        node_manifests: vec![],
        agent_bundle: agentx_runtime_contracts::AgentBundleContractV2::empty(
            "test-definition",
            "test-compiler",
        ),
        input_contract: json!({}),
        output_contract: json!({}),
        context_contract: json!({}),
        resources: vec![],
        authorization: authorization(tenant_id, workflow_id),
        dependency_closure: DependencyClosureV1 { entries: vec![] },
        triggers: vec![],
        runtime_policy: runtime_policy(60),
        objects: vec![RuntimeObjectReferenceV1 {
            tenant_id,
            storage_domain: StorageDomain::Runtime,
            object_id,
            object_key: RuntimeObjectReferenceV1::canonical_key(tenant_id, object_id, &object_hash),
            content_hash: object_hash,
            size_bytes: 2,
            media_type: "application/json".into(),
        }],
        worker_compatibility: compatibility(),
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn authorization(tenant_id: Uuid, workflow_id: Uuid) -> RuntimeAuthorizationSnapshotV1 {
    RuntimeAuthorizationSnapshotV1 {
        schema_version: 1,
        tenant_id,
        service_identity_id: Uuid::now_v7(),
        workflow_id,
        policy_epoch: 4,
        grant_ids: vec![],
        grant_bindings: vec![],
        capabilities: BTreeSet::from(["builtin".into()]),
        maximum_policy_staleness_seconds: 72 * 60 * 60,
        captured_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn runtime_policy(timeout_seconds: u32) -> RuntimePolicyV1 {
    RuntimePolicyV1 {
        timeout_seconds,
        operation_deadline_seconds: timeout_seconds,
        ..RuntimePolicyV1::default()
    }
}

fn compatibility() -> WorkerCompatibilityV1 {
    WorkerCompatibilityV1 {
        protocol_version: 1,
        ir_versions: BTreeSet::from([1]),
        compiler_versions: BTreeSet::from(["3".into()]),
        capabilities: BTreeSet::new(),
        manifest_versions: BTreeSet::new(),
    }
}

fn compiled_workflow() -> CompiledWorkflowV1 {
    CompiledWorkflowV1 {
        contract_version: IR_SCHEMA_VERSION,
        schema_version: "8.0".into(),
        compiler_version: "3".into(),
        canonical_hash: "canonical".into(),
        definition_hash: "definition".into(),
        execution_order: ExecutionOrder::Deterministic,
        activation_budget: 1,
        start: WorkflowStart::default(),
        contexts: std::collections::BTreeMap::<String, ContextDefinition>::new(),
        end: WorkflowEnd::default(),
        exits: std::collections::BTreeMap::new(),
        exit_order: vec![],
        nodes: Vec::<CompiledNodeV1>::new(),
        connections: vec![],
        terminal_connections: vec![],
        start_to_exit: None,
        start_nodes: vec![],
        strongly_connected_components: vec![],
        subworkflow_version_ids: vec![],
    }
}
