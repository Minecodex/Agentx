async fn authentication_failures_do_not_write_receipts(fixture: &Fixture) {
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM publish_receipts")
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    let bundle = fixture.bundle(99).await;
    let rejected = prepare_bundle(
        State(fixture.state.clone()),
        publisher_headers("runtime.deployments.activate"),
        Json(PrepareBundleRequestV1 {
            api_version: 1,
            idempotency_key: "auth-must-not-write".into(),
            bundle,
        }),
    )
    .await;
    assert!(matches!(rejected, Err(RuntimeError::Unauthorized)));
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM publish_receipts")
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(after, before);
}

async fn authentication_requires_active_route_tenant_and_head(fixture: &Fixture) {
    assert!(
        authenticate_api_key(&fixture.state.pool, "runtime-slice", &fixture.api_key)
            .await
            .is_ok()
    );

    sqlx::query(
        "UPDATE application_routes SET status='disabled' WHERE tenant_id=? AND application_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.application_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();
    assert_authentication_rejected(fixture).await;
    sqlx::query(
        "UPDATE application_routes SET status='active' WHERE tenant_id=? AND application_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(fixture.application_id)
    .execute(&fixture.state.pool)
    .await
    .unwrap();

    sqlx::query("UPDATE tenant_admission SET status='disabled' WHERE tenant_id=?")
        .bind(fixture.tenant_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    assert_authentication_rejected(fixture).await;
    sqlx::query("UPDATE tenant_admission SET status='active' WHERE tenant_id=?")
        .bind(fixture.tenant_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();

    sqlx::query("UPDATE deployment_bundles SET status='superseded' WHERE tenant_id=? AND id=(SELECT bundle_id FROM deployment_heads WHERE tenant_id=? AND application_id=?)")
        .bind(fixture.tenant_id)
        .bind(fixture.tenant_id)
        .bind(fixture.application_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
    assert_authentication_rejected(fixture).await;
    sqlx::query("UPDATE deployment_bundles SET status='active' WHERE tenant_id=? AND id=(SELECT bundle_id FROM deployment_heads WHERE tenant_id=? AND application_id=?)")
        .bind(fixture.tenant_id)
        .bind(fixture.tenant_id)
        .bind(fixture.application_id)
        .execute(&fixture.state.pool)
        .await
        .unwrap();
}

async fn assert_authentication_rejected(fixture: &Fixture) {
    assert!(matches!(
        authenticate_api_key(&fixture.state.pool, "runtime-slice", &fixture.api_key).await,
        Err(RuntimeError::Unauthorized | RuntimeError::NotFound)
    ));
}

impl Fixture {
    fn new(pool: MySqlPool, observer: MySqlPool) -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        let work_package_signing_key = SigningKey::generate(&mut OsRng);
        let trust = RuntimeTrust::new(
            "agentx-control",
            "agentx-runtime-internal",
            HashMap::from([("publisher-current".into(), PUBLIC_KEY.to_vec())]),
        )
        .with_bundle_key("bundle-current", signing_key.verifying_key())
        .with_work_package_key(
            "work-package-current",
            work_package_signing_key.verifying_key(),
        );
        Self {
            observer,
            state: RuntimeState {
                pool,
                objects: Arc::new(InMemory::new()),
                trust: Arc::new(trust),
                wakeups: Default::default(),
                vault: None,
            },
            signing_key,
            work_package_signing_key,
            tenant_id: Uuid::now_v7(),
            application_id: Uuid::now_v7(),
            workflow_id: Uuid::now_v7(),
            identity_id: Uuid::now_v7(),
            key_id: Uuid::now_v7(),
            api_key: "axk_runtime-slice-secret-value".into(),
        }
    }

    async fn bundle(&self, sequence: u64) -> agentx_runtime_contracts::ExecutionSpecBundleV2 {
        let definition = definition();
        let workflow_version_id = Uuid::now_v7();
        let bytes = agentx_runtime_contracts::canonical_bytes(&definition).unwrap();
        let hash = agentx_runtime_contracts::content_hash(&definition).unwrap();
        let object = RuntimeObjectReferenceV1 {
            tenant_id: self.tenant_id,
            storage_domain: StorageDomain::Runtime,
            object_id: workflow_version_id,
            object_key: RuntimeObjectReferenceV1::canonical_key(
                self.tenant_id,
                workflow_version_id,
                &hash,
            ),
            content_hash: hash,
            size_bytes: bytes.len() as u64,
            media_type: "application/vnd.agentx.workflow-definition+json".into(),
        };
        build_bundle(
            BundleBuildSource {
                bundle_id: Uuid::now_v7(),
                tenant_id: self.tenant_id,
                application_id: self.application_id,
                deployment_id: Uuid::now_v7(),
                workflow_id: self.workflow_id,
                workflow_version_id,
                workflow_name: "Runtime slice Workflow".into(),
                workflow_version_number: sequence,
                workflow_owner_department: None,
                sequence,
                definition,
                dependency_versions: BTreeMap::new(),
                plugin_manifests: vec![],
                supported_capabilities: BTreeSet::from([
                    "builtin".into(),
                    "plugin_nodejs".into(),
                ]),
                input_contract: json!({"type":"object","required":["message"],"properties":{"message":{"type":"string","minLength":2}},"additionalProperties":true}),
                output_contract: json!({"type":"object","required":["message"],"properties":{"message":{"type":"string"}},"additionalProperties":false}),
                resources: vec![],
                authorization: RuntimeAuthorizationSnapshotV1 {
                    schema_version: 1,
                    tenant_id: self.tenant_id,
                    service_identity_id: self.identity_id,
                    workflow_id: self.workflow_id,
                    policy_epoch: 1,
                    capabilities: BTreeSet::from([
                        "builtin".into(),
                        "plugin_nodejs".into(),
                    ]),
                    grant_ids: vec![],
                    grant_bindings: vec![],
                    maximum_policy_staleness_seconds: 72 * 60 * 60,
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
                triggers: vec![],
                runtime_policy: RuntimePolicyV1 {
                    timeout_seconds: 30,
                    operation_deadline_seconds: 30,
                    ..RuntimePolicyV1::default()
                },
                objects: vec![object],
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
            "bundle-current",
            &self.signing_key,
        )
        .unwrap()
    }

    fn work_package_source(
        &self,
        package_id: Uuid,
        created_at: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> WorkPackageBuildSource {
        let definition = definition();
        let compiled = agentx_bundle_builder::compile_workflow_version(&definition, package_id)
            .expect("debug Workflow compiles");
        WorkPackageBuildSource {
            package_id,
            tenant_id: self.tenant_id,
            workflow: agentx_runtime_contracts::ExecutionWorkflowSnapshotV1 {
                id: self.workflow_id,
                name: "Runtime slice Workflow".into(),
                version_id: package_id,
                version_number: 1,
                owner_department: None,
            },
            origin: agentx_runtime_contracts::ExecutionOriginV1::system(None),
            purpose: WorkPackagePurpose::Debug,
            call_purpose: RuntimeCallPurposeV1::Debug,
            spec: agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Debug {
                draft_revision: 1,
                debug_plan: agentx_runtime_contracts::RuntimeDebugPlanV1::whole(&compiled),
            },
            source_revision: "draft:1".into(),
            definition,
            dependency_versions: BTreeMap::new(),
            plugin_manifests: vec![],
            supported_capabilities: BTreeSet::from([
                "builtin".into(),
                "plugin_nodejs".into(),
            ]),
            overlay: RuntimeWorkPackageOverlayV1 {
                input: json!({"message":"work-package"}),
                ..RuntimeWorkPackageOverlayV1::default()
            },
            resources: vec![],
            authorization: RuntimeAuthorizationSnapshotV1 {
                schema_version: 1,
                tenant_id: self.tenant_id,
                service_identity_id: self.identity_id,
                workflow_id: self.workflow_id,
                policy_epoch: 1,
                capabilities: BTreeSet::from([
                    "builtin".into(),
                    "plugin_nodejs".into(),
                ]),
                grant_ids: vec![],
                grant_bindings: vec![],
                maximum_policy_staleness_seconds: 72 * 60 * 60,
                captured_at: created_at,
            },
            objects: vec![builtin_data_runtime_object(self.tenant_id).0],
            runtime_policy: RuntimePolicyV1::default(),
            created_at,
            expires_at,
        }
    }

    fn evaluation_work_package_source(
        &self,
        package_id: Uuid,
        created_at: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> WorkPackageBuildSource {
        let mut source = self.work_package_source(package_id, created_at, expires_at);
        source.purpose = WorkPackagePurpose::Evaluation;
        source.call_purpose = RuntimeCallPurposeV1::Evaluation;
        source.source_revision = "evaluation:1".into();
        source.spec = agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Evaluation {
            dataset_version_id: Uuid::now_v7(),
            profile_version_id: Uuid::now_v7(),
            cases: vec![
                agentx_runtime_contracts::RuntimeEvaluationCaseV1 {
                    case_id: Uuid::now_v7(),
                    input: json!({"message":"case-one"}),
                    expected_output: Some(json!({"message":"case-one"})),
                },
                agentx_runtime_contracts::RuntimeEvaluationCaseV1 {
                    case_id: Uuid::now_v7(),
                    input: json!({"message":"case-two"}),
                    expected_output: Some(json!({"message":"case-two"})),
                },
            ],
            evaluators: vec![
                agentx_runtime_contracts::RuntimeEvaluatorV1::DeterministicRule {
                    evaluator_id: Uuid::now_v7(),
                    expression: "exact_match".into(),
                },
            ],
        };
        source
    }

    async fn upload_bundle_object(
        &self,
        bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
        idempotency_key: &str,
    ) -> agentx_runtime_contracts::RuntimeObjectUploadReceiptV1 {
        let object = bundle.payload.objects[0].clone();
        let bytes = agentx_runtime_contracts::canonical_bytes(&bundle.payload.definition).unwrap();
        persist_upload(
            &self.state,
            RuntimeObjectUploadMetadataV1 {
                api_version: 1,
                idempotency_key: idempotency_key.into(),
                tenant_id: object.tenant_id,
                object_id: object.object_id,
                content_hash: object.content_hash,
                size_bytes: object.size_bytes,
                media_type: object.media_type,
            },
            Bytes::from(bytes),
        )
        .await
        .unwrap()
    }
}

fn builtin_data_runtime_object(tenant_id: Uuid) -> (RuntimeObjectReferenceV1, Vec<u8>) {
    let registry = agentx_runtime::NodeRegistry::m5_defaults();
    let plugin = registry
        .get("set", 1)
        .and_then(|manifest| manifest.plugin.as_ref())
        .expect("agentx/data binding");
    let artifact = plugin
        .runtime_artifact
        .as_ref()
        .expect("agentx/data Runtime artifact");
    let content_hash = agentx_runtime_contracts::ContentHash::parse(&artifact.content_hash).unwrap();
    (
        RuntimeObjectReferenceV1 {
            tenant_id,
            storage_domain: StorageDomain::Runtime,
            object_id: artifact.object_id,
            object_key: RuntimeObjectReferenceV1::canonical_key(
                tenant_id,
                artifact.object_id,
                &content_hash,
            ),
            content_hash,
            size_bytes: artifact.size_bytes,
            media_type: artifact.media_type.clone(),
        },
        plugin.runtime_source.as_bytes().to_vec(),
    )
}

async fn ensure_builtin_data_runtime_object(fixture: &Fixture) {
    let (object, bytes) = builtin_data_runtime_object(fixture.tenant_id);
    fixture
        .state
        .objects
        .put(&ObjectPath::from(object.object_key.clone()), bytes.into())
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtime_objects(object_id,tenant_id,object_key,content_hash,size_bytes,media_type,status,idempotency_key,request_hash,temporary_expires_at,ready_at,deleted_at) VALUES(?,?,?,?,?,?,'ready',?,?,UTC_TIMESTAMP(6),UTC_TIMESTAMP(6),NULL) ON DUPLICATE KEY UPDATE object_key=VALUES(object_key),content_hash=VALUES(content_hash),size_bytes=VALUES(size_bytes),media_type=VALUES(media_type),status='ready',ready_at=UTC_TIMESTAMP(6),deleted_at=NULL")
        .bind(object.object_id)
        .bind(object.tenant_id)
        .bind(&object.object_key)
        .bind(object.content_hash.as_str())
        .bind(object.size_bytes)
        .bind(&object.media_type)
        .bind(format!("builtin-data:{}", object.object_id))
        .bind(object.content_hash.as_str())
        .execute(&fixture.state.pool)
        .await
        .unwrap();
}

async fn work_package_prepare_execute_cancel_are_independently_signed_and_idempotent(
    fixture: &Fixture,
) {
    ensure_builtin_data_runtime_object(fixture).await;
    let now = OffsetDateTime::now_utc();
    let package_id = Uuid::now_v7();
    let source = fixture.work_package_source(package_id, now, now + time::Duration::hours(1));
    let wrongly_signed =
        build_work_package(source.clone(), "work-package-current", &fixture.signing_key).unwrap();
    let rejected = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(PrepareWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: "work-package:wrong-key".into(),
            work_package: wrongly_signed,
        }),
    )
    .await;
    assert!(matches!(
        rejected,
        Err(RuntimeError::BadRequest(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::InvalidSignature,
            _
        ))
    ));

    let package = build_work_package(
        source.clone(),
        "work-package-current",
        &fixture.work_package_signing_key,
    )
    .unwrap();
    let request = PrepareWorkPackageRequestV1 {
        api_version: 1,
        idempotency_key: "work-package:prepare".into(),
        work_package: package,
    };
    let prepared = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(request.clone()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(prepared.status, PublishReceiptStatusV1::Accepted);
    assert!(!prepared.receipt.replayed);
    let replayed = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(request),
    )
    .await
    .unwrap()
    .0;
    assert!(replayed.receipt.replayed);
    let call_purpose: String =
        sqlx::query_scalar("SELECT call_purpose FROM runtime_work_packages WHERE id=?")
            .bind(package_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(call_purpose, "debug");

    let mut conflicting_source = source;
    conflicting_source.source_revision = "draft:2".into();
    let conflict = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(PrepareWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: "work-package:prepare".into(),
            work_package: build_work_package(
                conflicting_source,
                "work-package-current",
                &fixture.work_package_signing_key,
            )
            .unwrap(),
        }),
    )
    .await;
    assert!(matches!(conflict, Err(RuntimeError::Conflict(_, _))));

    let execute_request = ExecuteWorkPackageRequestV1 {
        api_version: 1,
        idempotency_key: "work-package:execute".into(),
        package_id,
        input: Value::Null,
    };
    let executed = execute_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(execute_request.clone()),
    )
    .await
    .unwrap()
    .0;
    assert!(!executed.replayed);
    let debug_execution_id = serde_json::from_value::<Uuid>(executed.result["executionId"].clone())
        .expect("debug Work Package returns one Execution ID");
    let debug_context: Value = sqlx::query_scalar(
        "SELECT execution_context_json FROM execution_snapshots WHERE execution_id=?",
    )
    .bind(debug_execution_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(debug_context["trigger"]["type"], "debug");
    assert_eq!(debug_context["workflow"]["name"], "Runtime slice Workflow");
    assert!(debug_context["initiator"].get("user").is_none());
    let execute_replay = execute_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(execute_request),
    )
    .await
    .unwrap()
    .0;
    assert!(execute_replay.replayed);
    let execute_conflict = execute_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(ExecuteWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: "work-package:execute".into(),
            package_id,
            input: json!({"message":"different"}),
        }),
    )
    .await;
    assert!(matches!(
        execute_conflict,
        Err(RuntimeError::Conflict(_, _))
    ));

    let cancel_request = CancelWorkPackageRequestV1 {
        api_version: 1,
        tenant_id: fixture.tenant_id,
        package_id,
        expected_version: executed.object_version,
        idempotency_key: "work-package:cancel".into(),
    };
    let cancelled = cancel_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.cancel"),
        Json(cancel_request.clone()),
    )
    .await
    .unwrap()
    .0;
    assert!(!cancelled.replayed);
    let cancel_replay = cancel_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.cancel"),
        Json(cancel_request),
    )
    .await
    .unwrap()
    .0;
    assert!(cancel_replay.replayed);

    let expired_source = fixture.work_package_source(
        Uuid::now_v7(),
        now - time::Duration::hours(2),
        now - time::Duration::hours(1),
    );
    let expired = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(PrepareWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: "work-package:expired".into(),
            work_package: build_work_package(
                expired_source,
                "work-package-current",
                &fixture.work_package_signing_key,
            )
            .unwrap(),
        }),
    )
    .await;
    assert!(matches!(expired, Err(RuntimeError::Conflict(_, _))));
}

async fn evaluation_work_package_creates_cases_converges_and_cancels_atomically(fixture: &Fixture) {
    ensure_builtin_data_runtime_object(fixture).await;
    let now = OffsetDateTime::now_utc();
    let package_id = Uuid::now_v7();
    let package = build_work_package(
        fixture.evaluation_work_package_source(package_id, now, now + time::Duration::hours(24)),
        "work-package-current",
        &fixture.work_package_signing_key,
    )
    .unwrap();
    let _ = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(PrepareWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("evaluation:prepare:{package_id}"),
            work_package: package,
        }),
    )
    .await
    .unwrap();
    let execute_request = ExecuteWorkPackageRequestV1 {
        api_version: 1,
        idempotency_key: format!("evaluation:execute:{package_id}"),
        package_id,
        input: Value::Null,
    };
    let executed = execute_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(execute_request.clone()),
    )
    .await
    .unwrap()
    .0;
    let replayed = execute_work_package(
        State(fixture.state.clone()),
        Path(package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(execute_request),
    )
    .await
    .unwrap()
    .0;
    assert!(replayed.replayed);
    assert_eq!(executed.result, replayed.result);
    let execution_ids = executed.result["executionIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| serde_json::from_value::<Uuid>(id.clone()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(execution_ids.len(), 2);
    for execution_id in &execution_ids {
        let evaluation_context: Value = sqlx::query_scalar(
            "SELECT execution_context_json FROM execution_snapshots WHERE execution_id=?",
        )
        .bind(execution_id)
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
        assert_eq!(evaluation_context["trigger"]["type"], "evaluation");
        assert_eq!(evaluation_context["workflow"]["name"], "Runtime slice Workflow");
        assert!(evaluation_context["initiator"].get("user").is_none());
    }
    let case_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM evaluation_run_cases c JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id WHERE r.tenant_id=? AND r.work_package_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(package_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(case_count, 2);
    let owner = Uuid::now_v7();
    let commands = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap();
    complete_claimed_work_package_executions(fixture, owner, &commands, &execution_ids[..1]).await;
    let first_case: Uuid = sqlx::query_scalar(
        "SELECT id FROM evaluation_run_cases WHERE tenant_id=? AND target_execution_id=? AND status='completed'",
    )
    .bind(fixture.tenant_id)
    .bind(execution_ids[0])
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    let mut historical = fixture.state.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM evaluation_run_cases WHERE id=? FOR UPDATE")
        .bind(first_case)
        .fetch_one(&mut *historical)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM evaluation_rule_results WHERE evaluation_run_case_id=? FOR UPDATE")
        .bind(first_case)
        .fetch_all(&mut *historical)
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_millis(500),
        complete_claimed_work_package_executions(fixture, owner, &commands, &execution_ids[1..]),
    )
    .await
    .expect("finishing another Case must not lock historical Cases or Rules");
    historical.rollback().await.unwrap();
    let run_status: String = sqlx::query_scalar(
        "SELECT status FROM evaluation_runs WHERE tenant_id=? AND work_package_id=?",
    )
    .bind(fixture.tenant_id)
    .bind(package_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(run_status, "completed");
    let rule_failures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM evaluation_rule_results rr JOIN evaluation_run_cases c ON c.id=rr.evaluation_run_case_id AND c.tenant_id=rr.tenant_id JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id WHERE r.work_package_id=? AND rr.status<>'passed'",
    )
    .bind(package_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(rule_failures, 0);
    let package_status: String =
        sqlx::query_scalar("SELECT status FROM runtime_work_packages WHERE id=?")
            .bind(package_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(package_status, "succeeded");

    let endpoint = "https://provider.example.test/evaluate".to_owned();
    let model_package_id = Uuid::now_v7();
    let model_id = Uuid::now_v7();
    let evaluator_id = Uuid::now_v7();
    let prompt_object_id = Uuid::now_v7();
    let prompt = Bytes::from_static(br#"{"prompt":"Judge {{actualOutput}} against {{expectedOutput}}. Return passed, score and reason."}"#);
    let prompt_hash = agentx_runtime_contracts::ContentHash::parse(format!(
        "sha256:{:x}",
        Sha256::digest(&prompt)
    ))
    .unwrap();
    let prompt_reference = RuntimeObjectReferenceV1 {
        tenant_id: fixture.tenant_id,
        storage_domain: StorageDomain::Runtime,
        object_id: prompt_object_id,
        object_key: RuntimeObjectReferenceV1::canonical_key(
            fixture.tenant_id,
            prompt_object_id,
            &prompt_hash,
        ),
        content_hash: prompt_hash.clone(),
        size_bytes: prompt.len() as u64,
        media_type: "application/json".into(),
    };
    persist_upload(
        &fixture.state,
        RuntimeObjectUploadMetadataV1 {
            api_version: 1,
            idempotency_key: format!("evaluation:prompt:{prompt_object_id}"),
            tenant_id: fixture.tenant_id,
            object_id: prompt_object_id,
            content_hash: prompt_hash,
            size_bytes: prompt.len() as u64,
            media_type: "application/json".into(),
        },
        prompt,
    )
    .await
    .unwrap();
    let mut model_source = fixture.evaluation_work_package_source(
        model_package_id,
        now,
        now + time::Duration::hours(24),
    );
    model_source.supported_capabilities.insert("model".into());
    model_source
        .authorization
        .capabilities
        .insert("model".into());
    model_source.objects.push(prompt_reference);
    model_source
        .resources
        .push(agentx_runtime_contracts::RuntimeResourceBindingV1 {
            resource_kind: agentx_runtime_contracts::RuntimeResourceKindV1::Model,
            resource_id: model_id,
            resource_version: "model-fixture:1".into(),
            state_epoch: 1,
            content_hash: agentx_runtime_contracts::content_hash(&json!({
                "modelId":model_id,
                "version":1
            }))
            .unwrap(),
            configuration: agentx_runtime_contracts::RuntimeResourceConfigurationV1::Model {
            capabilities: Vec::new(),
                provider: "openai_compatible".into(),
                endpoint,
                model: "evaluator-fixture".into(),
                context_window: 128_000,
                price: agentx_runtime_contracts::RuntimeModelPriceV1 {
                    version_id: "price:1".into(),
                    currency: "USD".into(),
                    input_per_million: "1".into(),
                    output_per_million: "2".into(),
                },
                credential: None,
            },
            object_ids: vec![],
        });
    let agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Evaluation { evaluators, .. } =
        &mut model_source.spec
    else {
        unreachable!()
    };
    evaluators.push(agentx_runtime_contracts::RuntimeEvaluatorV1::Model {
        evaluator_id,
        resource_id: model_id,
        prompt_object_id,
    });
    let model_package = build_work_package(
        model_source,
        "work-package-current",
        &fixture.work_package_signing_key,
    )
    .unwrap();
    let _ = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(PrepareWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("evaluation:prepare:{model_package_id}"),
            work_package: model_package,
        }),
    )
    .await
    .unwrap();
    let model_started = execute_work_package(
        State(fixture.state.clone()),
        Path(model_package_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(ExecuteWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("evaluation:execute:{model_package_id}"),
            package_id: model_package_id,
            input: Value::Null,
        }),
    )
    .await
    .unwrap()
    .0;
    let model_targets = model_started.result["executionIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| serde_json::from_value::<Uuid>(id.clone()).unwrap())
        .collect::<Vec<_>>();
    complete_work_package_executions(fixture, &model_targets).await;
    complete_model_evaluators(fixture, model_package_id).await;
    let model_results: (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),CAST(COALESCE(SUM(rr.evaluator_execution_id IS NOT NULL AND rr.evaluator_command_id IS NOT NULL),0) AS SIGNED),CAST(COALESCE(SUM(rr.cost_micros),0) AS SIGNED) FROM evaluation_rule_results rr JOIN evaluation_run_cases c ON c.id=rr.evaluation_run_case_id AND c.tenant_id=rr.tenant_id JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id WHERE r.tenant_id=? AND r.work_package_id=? AND JSON_UNQUOTE(JSON_EXTRACT(rr.detail_json,'$.kind'))='model' AND rr.status='passed'",
    )
    .bind(fixture.tenant_id)
    .bind(model_package_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    type ModelEvaluationDebug = (String, String, Option<Value>, Option<Value>, Value);
    let model_debug: Vec<ModelEvaluationDebug> = sqlx::query_as(
        "SELECT rr.status,e.status,e.output_json,e.error_json,rr.detail_json FROM evaluation_rule_results rr JOIN evaluation_run_cases c ON c.id=rr.evaluation_run_case_id AND c.tenant_id=rr.tenant_id JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id LEFT JOIN workflow_executions e ON e.id=rr.evaluator_execution_id AND e.tenant_id=rr.tenant_id WHERE r.tenant_id=? AND r.work_package_id=? AND JSON_UNQUOTE(JSON_EXTRACT(rr.detail_json,'$.kind'))='model' ORDER BY rr.created_at,rr.id",
    )
    .bind(fixture.tenant_id)
    .bind(model_package_id)
    .fetch_all(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(
        model_results.0, 2,
        "model evaluator state: {model_debug:#?}"
    );
    assert_eq!(model_results.1, 2);
    assert_eq!(model_results.2, 28);
    let model_package_status: String =
        sqlx::query_scalar("SELECT status FROM runtime_work_packages WHERE id=?")
            .bind(model_package_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(model_package_status, "succeeded");
    let cancelled_id = Uuid::now_v7();
    let cancelled_package = build_work_package(
        fixture.evaluation_work_package_source(cancelled_id, now, now + time::Duration::hours(24)),
        "work-package-current",
        &fixture.work_package_signing_key,
    )
    .unwrap();
    let _ = prepare_work_package(
        State(fixture.state.clone()),
        publisher_headers("runtime.work-packages.prepare"),
        Json(PrepareWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("evaluation:prepare:{cancelled_id}"),
            work_package: cancelled_package,
        }),
    )
    .await
    .unwrap();
    let started = execute_work_package(
        State(fixture.state.clone()),
        Path(cancelled_id),
        publisher_headers("runtime.work-packages.execute"),
        Json(ExecuteWorkPackageRequestV1 {
            api_version: 1,
            idempotency_key: format!("evaluation:execute:{cancelled_id}"),
            package_id: cancelled_id,
            input: Value::Null,
        }),
    )
    .await
    .unwrap()
    .0;
    let cancelled_execution_ids = started.result["executionIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| serde_json::from_value::<Uuid>(id.clone()).unwrap())
        .collect::<Vec<_>>();
    let start_owner = Uuid::now_v7();
    for command in claim_commands(&fixture.state.pool, start_owner, 100)
        .await
        .unwrap()
    {
        process_command(&fixture.state.pool, &command)
            .await
            .unwrap();
    }
    let _ = cancel_work_package(
        State(fixture.state.clone()),
        Path(cancelled_id),
        publisher_headers("runtime.work-packages.cancel"),
        Json(CancelWorkPackageRequestV1 {
            api_version: 1,
            tenant_id: fixture.tenant_id,
            package_id: cancelled_id,
            expected_version: started.object_version,
            idempotency_key: format!("evaluation:cancel:{cancelled_id}"),
        }),
    )
    .await
    .unwrap();
    let cancel_owner = Uuid::now_v7();
    for command in claim_commands(&fixture.state.pool, cancel_owner, 100)
        .await
        .unwrap()
    {
        process_command(&fixture.state.pool, &command)
            .await
            .unwrap();
    }
    let live_cases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM evaluation_run_cases c JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id WHERE r.work_package_id=? AND c.status<>'cancelled'",
    )
    .bind(cancelled_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(live_cases, 0);
    let cancelled_executions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_executions WHERE tenant_id=? AND work_package_id=? AND status='cancelled'",
    )
    .bind(fixture.tenant_id)
    .bind(cancelled_id)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(cancelled_executions, cancelled_execution_ids.len() as i64);
    let active_reservations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM quota_reservations WHERE tenant_id=? AND status='active' AND ((scope_type='execution' AND scope_id IN (?,?)) OR (scope_type='attempt' AND scope_id IN (SELECT CAST(BIN_TO_UUID(id) AS CHAR) COLLATE utf8mb4_0900_ai_ci FROM node_attempts WHERE tenant_id=? AND execution_id IN (?,?))))",
    )
    .bind(fixture.tenant_id)
    .bind(cancelled_execution_ids[0].to_string())
    .bind(cancelled_execution_ids[1].to_string())
    .bind(fixture.tenant_id)
    .bind(cancelled_execution_ids[0])
    .bind(cancelled_execution_ids[1])
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(active_reservations, 0);
}

async fn complete_work_package_executions(fixture: &Fixture, execution_ids: &[Uuid]) {
    let owner = Uuid::now_v7();
    let commands = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap();
    complete_claimed_work_package_executions(fixture, owner, &commands, execution_ids).await;
}

async fn complete_claimed_work_package_executions(
    fixture: &Fixture,
    owner: Uuid,
    commands: &[agentx_v2_runtime::execution::RuntimeCommandClaim],
    execution_ids: &[Uuid],
) {
    for execution_id in execution_ids {
        let command = commands
            .iter()
            .find(|claim| claim.execution_id == *execution_id)
            .unwrap();
        process_command(&fixture.state.pool, command).await.unwrap();
        let dispatch = claim_dispatch(&fixture.state.pool, owner)
            .await
            .unwrap()
            .unwrap();
        let task = dispatch.task().unwrap();
        assert_eq!(task.execution_id, *execution_id);
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
    }
}

async fn complete_model_evaluators(fixture: &Fixture, package_id: Uuid) {
    let execution_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT rr.evaluator_execution_id FROM evaluation_rule_results rr JOIN evaluation_run_cases c ON c.id=rr.evaluation_run_case_id AND c.tenant_id=rr.tenant_id JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id WHERE r.tenant_id=? AND r.work_package_id=? AND rr.status='running' ORDER BY rr.created_at,rr.id",
    )
    .bind(fixture.tenant_id)
    .bind(package_id)
    .fetch_all(&fixture.state.pool)
    .await
    .unwrap();
    let owner = Uuid::now_v7();
    let commands = claim_commands(&fixture.state.pool, owner, 100)
        .await
        .unwrap();
    let worker = test_worker(fixture, StubWorkerMode::Evaluator);
    for execution_id in execution_ids {
        let command = commands
            .iter()
            .find(|claim| claim.execution_id == execution_id)
            .unwrap();
        process_command(&fixture.state.pool, command).await.unwrap();
        let dispatch = claim_dispatch(&fixture.state.pool, owner)
            .await
            .unwrap()
            .unwrap();
        let task = dispatch.task().unwrap();
        assert_eq!(task.execution_id, execution_id);
        complete_dispatch(&fixture.state.pool, &dispatch)
            .await
            .unwrap();
        let worker_id = Uuid::now_v7();
        agentx_v2_runtime::engine::register_worker(
            &fixture.state.pool,
            worker_id,
            "model",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .unwrap();
        let claim = agentx_v2_runtime::engine::claim_worker_attempt(
            &fixture.state.pool,
            worker_id,
            "model",
            &task,
        )
        .await
        .unwrap()
        .unwrap();
        let execution = worker.execute(&claim).await;
        let result_hash = agentx_v2_runtime::engine::worker_result_hash(
            execution.status,
            &execution.outputs,
            None,
            execution.error_code.as_deref(),
            execution.error_message.as_deref(),
            None,
            None,
        )
        .unwrap();
        agentx_v2_runtime::engine::submit_worker_result(
            &fixture.state.pool,
            &WorkerResultV1 {
                protocol_version: 1,
                attempt_id: claim.task.attempt_id,
                worker_id,
                fencing_token: claim.lease.fencing_token,
                status: execution.status,
                result_hash,
                outputs: execution.outputs,
                output_object: None,
                error_code: execution.error_code,
                error_message: execution.error_message,
                retryable: None,
                partial_output_object: None,
            },
        )
        .await
        .unwrap();
    }
}

async fn object_upload_is_immutable_and_replayable(
    fixture: &Fixture,
    bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
) {
    let first = fixture
        .upload_bundle_object(bundle, "bundle-object:first")
        .await;
    assert!(!first.replayed);
    let replay = fixture
        .upload_bundle_object(bundle, "bundle-object:first")
        .await;
    assert!(replay.replayed);
    assert_eq!(replay.object.object_key, first.object.object_key);

    let object = &bundle.payload.objects[0];
    let wrong = persist_upload(
        &fixture.state,
        RuntimeObjectUploadMetadataV1 {
            api_version: 1,
            idempotency_key: "bundle-object:first".into(),
            tenant_id: object.tenant_id,
            object_id: object.object_id,
            content_hash: object.content_hash.clone(),
            size_bytes: object.size_bytes + 1,
            media_type: object.media_type.clone(),
        },
        Bytes::from_static(b"different"),
    )
    .await;
    assert!(matches!(wrong, Err(RuntimeError::BadRequest(_, _))));
}

async fn prepare_is_idempotent_and_does_not_route_traffic(
    fixture: &Fixture,
    bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
) {
    let first = prepare(fixture, bundle, "prepare:first").await;
    let replay = prepare(fixture, bundle, "prepare:first").await;
    assert_eq!(first.bundle_id, replay.bundle_id);
    let heads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployment_heads")
        .fetch_one(&fixture.state.pool)
        .await
        .unwrap();
    assert_eq!(heads, 0);

    let mut changed = bundle.clone();
    changed.payload.compiled_ir.activation_budget = 99;
    let conflict = prepare_bundle(
        State(fixture.state.clone()),
        publisher_headers("runtime.bundles.prepare"),
        Json(PrepareBundleRequestV1 {
            api_version: 1,
            idempotency_key: "prepare:first".into(),
            bundle: changed,
        }),
    )
    .await;
    assert!(matches!(conflict, Err(RuntimeError::Conflict(_, _))));
}

async fn prepare(
    fixture: &Fixture,
    bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
    key: &str,
) -> agentx_runtime_contracts::PublishReceiptV1 {
    prepare_bundle(
        State(fixture.state.clone()),
        publisher_headers("runtime.bundles.prepare"),
        Json(PrepareBundleRequestV1 {
            api_version: 1,
            idempotency_key: key.into(),
            bundle: bundle.clone(),
        }),
    )
    .await
    .unwrap()
    .0
}

async fn apply_initial_admission(fixture: &Fixture, epoch: u64) {
    for target in [
        AdmissionTargetV1::Tenant { enabled: true },
        AdmissionTargetV1::ApplicationRoute {
            state: ApplicationRouteAdmissionV1 {
                tenant_id: fixture.tenant_id,
                application_id: fixture.application_id,
                route_key: "runtime-slice".into(),
                status: AdmissionStatusV1::Active,
            },
        },
        api_key_target(fixture, AdmissionStatusV1::Active),
        AdmissionTargetV1::ServiceIdentity {
            state: ServiceIdentityAdmissionV1 {
                tenant_id: fixture.tenant_id,
                workflow_id: fixture.workflow_id,
                identity_id: fixture.identity_id,
                policy_epoch: epoch,
                status: AdmissionStatusV1::Active,
                capabilities: vec!["builtin".into()],
                grant_ids: vec![],
            },
        },
    ] {
        apply_target(fixture, epoch, target).await;
    }
}

async fn apply_api_key(fixture: &Fixture, epoch: u64, status: AdmissionStatusV1) {
    apply_target(fixture, epoch, api_key_target(fixture, status)).await;
}

fn api_key_target(fixture: &Fixture, status: AdmissionStatusV1) -> AdmissionTargetV1 {
    AdmissionTargetV1::ApiKey {
        state: ApiKeyAdmissionV1 {
            tenant_id: fixture.tenant_id,
            application_id: fixture.application_id,
            key_id: fixture.key_id,
            key_name: "Runtime slice key".into(),
            key_prefix: fixture.api_key.chars().take(12).collect(),
            secret_hash: format!("sha256:{:x}", Sha256::digest(fixture.api_key.as_bytes())),
            status,
            expires_at: None,
        },
    }
}

async fn apply_target(fixture: &Fixture, epoch: u64, target: AdmissionTargetV1) {
    let event_id = Uuid::now_v7();
    let receipt = apply_admission(
        State(fixture.state.clone()),
        publisher_headers("runtime.admission.apply"),
        Json(admission_request(
            fixture,
            epoch,
            target,
            &format!("admission:{event_id}"),
        )),
    )
    .await
    .unwrap()
    .0;
    assert!(receipt.applied);
}

fn admission_request(
    fixture: &Fixture,
    epoch: u64,
    target: AdmissionTargetV1,
    idempotency_key: &str,
) -> RuntimeAdmissionCommandV1 {
    let event_id = Uuid::now_v7();
    let hash = agentx_runtime_contracts::content_hash(&target).unwrap();
    RuntimeAdmissionCommandV1 {
        api_version: 1,
        command: CommandEnvelopeV1 {
            schema_version: 1,
            event_id,
            source_plane: Plane::Control,
            tenant_id: fixture.tenant_id,
            aggregate_type: "admission".into(),
            aggregate_id: fixture.application_id.to_string(),
            object_version: epoch,
            occurred_at: OffsetDateTime::now_utc(),
            payload: json!({}),
            content_hash: hash,
            correlation_id: event_id,
            causation_id: None,
            idempotency_key: idempotency_key.into(),
        },
        admission_epoch: epoch,
        target,
    }
}

async fn concurrent_admission_delivery_converges_to_one_receipt(fixture: &Fixture) {
    const CONCURRENCY: usize = 16;
    let request = admission_request(
        fixture,
        1,
        AdmissionTargetV1::Tenant { enabled: true },
        "admission:concurrent-delivery",
    );
    let barrier = Arc::new(tokio::sync::Barrier::new(CONCURRENCY));
    let mut deliveries = Vec::with_capacity(CONCURRENCY);
    for _ in 0..CONCURRENCY {
        let state = fixture.state.clone();
        let request = request.clone();
        let barrier = barrier.clone();
        deliveries.push(tokio::spawn(async move {
            barrier.wait().await;
            apply_admission(
                State(state),
                publisher_headers("runtime.admission.apply"),
                Json(request),
            )
            .await
            .map(|receipt| receipt.0)
        }));
    }
    let mut fresh = 0;
    let mut replayed = 0;
    for delivery in deliveries {
        let receipt = delivery
            .await
            .expect("concurrent Admission task should join")
            .expect("concurrent Admission delivery should converge");
        assert!(receipt.applied);
        if receipt.replayed {
            replayed += 1;
        } else {
            fresh += 1;
        }
    }
    assert_eq!(fresh, 1);
    assert_eq!(replayed, CONCURRENCY - 1);
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM publish_receipts WHERE tenant_id=? AND operation='admission' AND idempotency_key=?",
    )
    .bind(fixture.tenant_id)
    .bind(&request.command.idempotency_key)
    .fetch_one(&fixture.state.pool)
    .await
    .unwrap();
    assert_eq!(receipt_count, 1);
}

async fn activate(
    fixture: &Fixture,
    bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
    expected: Option<u64>,
    sequence: u64,
    epoch: u64,
) {
    let request = activation_request(
        bundle,
        expected,
        sequence,
        epoch,
        format!("activate:{sequence}"),
    );
    let receipt = activate_deployment(
        State(fixture.state.clone()),
        publisher_headers("runtime.deployments.activate"),
        Json(request.clone()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(receipt.status, PublishReceiptStatusV1::Accepted);
    let replay = activate_deployment(
        State(fixture.state.clone()),
        publisher_headers("runtime.deployments.activate"),
        Json(request),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(replay.bundle_id, receipt.bundle_id);
}

fn activation_request(
    bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
    expected: Option<u64>,
    sequence: u64,
    epoch: u64,
    key: String,
) -> ActivateDeploymentRequestV1 {
    ActivateDeploymentRequestV1 {
        api_version: 1,
        idempotency_key: key,
        manifest: ActivationManifestV1 {
            api_version: 1,
            tenant_id: bundle.payload.tenant_id,
            application_id: bundle.payload.application_id,
            deployment_id: bundle.payload.deployment_id,
            bundle_id: bundle.payload.bundle_id,
            expected_head_version: expected,
            activation_sequence: sequence,
            minimum_admission_epoch: epoch,
            runtime_config_revision: 1,
            runtime_policy: agentx_runtime_contracts::ApplicationRuntimePolicyV1 {
                session_version_policy: agentx_runtime_contracts::SessionVersionPolicyV1::Pinned,
                synchronous_wait_seconds: 30,
                maximum_json_bytes: 1_048_576,
                maximum_multipart_bytes: 52_428_800,
            },
        },
    }
}

async fn rollback(
    fixture: &Fixture,
    bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
    expected: Option<u64>,
    sequence: u64,
    epoch: u64,
) {
    let receipt = rollback_request(
        fixture,
        bundle,
        expected,
        sequence,
        epoch,
        &format!("rollback:{sequence}"),
    )
    .await;
    assert_eq!(receipt.status, PublishReceiptStatusV1::Accepted);
}

async fn rollback_request(
    fixture: &Fixture,
    bundle: &agentx_runtime_contracts::ExecutionSpecBundleV2,
    expected: Option<u64>,
    sequence: u64,
    epoch: u64,
    key: &str,
) -> agentx_runtime_contracts::PublishReceiptV1 {
    rollback_deployment(
        State(fixture.state.clone()),
        publisher_headers("runtime.deployments.rollback"),
        Json(RollbackDeploymentRequestV1 {
            api_version: 1,
            idempotency_key: key.into(),
            manifest: ActivationManifestV1 {
                api_version: 1,
                tenant_id: bundle.payload.tenant_id,
                application_id: bundle.payload.application_id,
                deployment_id: bundle.payload.deployment_id,
                bundle_id: bundle.payload.bundle_id,
                expected_head_version: expected,
                activation_sequence: sequence,
                minimum_admission_epoch: epoch,
                runtime_config_revision: 1,
                runtime_policy: agentx_runtime_contracts::ApplicationRuntimePolicyV1 {
                    session_version_policy:
                        agentx_runtime_contracts::SessionVersionPolicyV1::Pinned,
                    synchronous_wait_seconds: 30,
                    maximum_json_bytes: 1_048_576,
                    maximum_multipart_bytes: 52_428_800,
                },
            },
        }),
    )
    .await
    .unwrap()
    .0
}

async fn stale_head_and_sequence_are_rejected(
    fixture: &Fixture,
    first: &agentx_runtime_contracts::ExecutionSpecBundleV2,
) {
    let stale = rollback_request(fixture, first, Some(1), 3, 1, "rollback:stale-head").await;
    assert_eq!(stale.status, PublishReceiptStatusV1::Rejected);
    assert!(matches!(
        stale.rejection.unwrap().code,
        agentx_runtime_contracts::RuntimePublishErrorCodeV1::HeadVersionConflict
    ));
    let unordered = rollback_request(fixture, first, Some(2), 2, 1, "rollback:old-sequence").await;
    assert_eq!(unordered.status, PublishReceiptStatusV1::Rejected);
    assert!(matches!(
        unordered.rejection.unwrap().code,
        agentx_runtime_contracts::RuntimePublishErrorCodeV1::ActivationSequenceConflict
    ));
}
