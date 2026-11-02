async fn page_publications_are_atomic_and_history_retains_its_exact_page(base: &Fixture) {
    use agentx_runtime_contracts::{
        ActivatePagePublicationV1, ApplicationPageContractV1, ApplicationPagePublicationV1,
        ApplicationPageViewV1, PreparePagePublicationV1, TaskViewV1, UserAccessClaimsV1,
        content_hash, issue_user_access_token, sign_canonical,
    };
    use agentx_v2_runtime::application_pages;
    let mut fixture = Fixture::new(
        base.state.pool.clone(),
        base.observer.clone(),
        base.state.admission_redis.clone().unwrap(),
    );
    fixture.api_key = format!("axk_{}", Uuid::new_v4().simple());
    let bundle = fixture.bundle(1).await;
    fixture
        .upload_bundle_object(&bundle, "page-fixture:bundle")
        .await;
    prepare(&fixture, &bundle, "page-fixture:prepare").await;
    apply_initial_admission(&fixture, 1).await;
    activate(&fixture, &bundle, None, 1, 1).await;
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO runtime_user_admission(tenant_id,user_id,user_name,department_id,department_name,token_version,status,tenant_query_enabled,role_assignments_json,admission_epoch) VALUES(?,?,'Employee',?,'Sales',1,'active',FALSE,JSON_ARRAY(),1)").bind(fixture.tenant_id).bind(user).bind(Uuid::now_v7()).execute(&fixture.state.pool).await.unwrap();
    sqlx::query("INSERT INTO runtime_user_application_grants(tenant_id,user_id,application_id,grant_version,status,can_invoke,can_query,admission_epoch) VALUES(?,?,?,1,'active',TRUE,FALSE,1)").bind(fixture.tenant_id).bind(user).bind(fixture.application_id).execute(&fixture.state.pool).await.unwrap();
    let now = now_unix();
    let token = issue_user_access_token(
        "page-user",
        PRIVATE_KEY,
        &UserAccessClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: user,
            tenant_id: fixture.tenant_id,
            token_version: 1,
            kind: "access".into(),
            origin: agentx_runtime_contracts::ExecutionOriginV1::system(None),
            iat: now,
            exp: now + 300,
            jti: Uuid::now_v7(),
        },
    )
    .unwrap();
    let router = agentx_v2_runtime::gateway::router().with_state(fixture.state.clone());
    // Omission is a formal external wait, independent of this caller being a user.
    // An explicitly empty testing plan must remain preset rather than fall back.
    for (plan, expected) in [(None, "external"), (Some(json!({})), "preset")] {
        let mut body = json!({"input":{"message":"Personal formal request"}});
        if let Some(plan) = plan {
            body["waitInputs"] = plan;
        }
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/applications/runtime-slice/invocations")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .header(
                        "Idempotency-Key",
                        format!("page-fixture:wait-mode:{expected}"),
                    )
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            status,
            axum::http::StatusCode::ACCEPTED,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let invocation: Value = serde_json::from_slice(&bytes).unwrap();
        let execution: Uuid = serde_json::from_value(invocation["executionId"].clone()).unwrap();
        let settings:Value=sqlx::query_scalar("SELECT runtime_settings_json FROM execution_snapshots WHERE tenant_id=? AND execution_id=?").bind(fixture.tenant_id).bind(execution).fetch_one(&fixture.state.pool).await.unwrap();
        assert_eq!(settings["waitInputMode"], expected);
        assert_eq!(
            settings["waitInputs"],
            if expected == "preset" {
                json!({})
            } else {
                Value::Null
            }
        );
    }
    let rejected = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/applications/runtime-slice/invocations")
                .header(AUTHORIZATION, format!("Bearer {}", fixture.api_key))
                .header("content-type", "application/json")
                .header("Idempotency-Key", "page-fixture:key-cannot-preset")
                .body(Body::from(
                    json!({"input":{"message":"Service request"},"waitInputs":{}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), axum::http::StatusCode::BAD_REQUEST);
    let rejected: Value =
        serde_json::from_slice(&to_bytes(rejected.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(rejected["code"], "WAIT_PRESET_NOT_ALLOWED");
    let mut assets = Vec::new();
    for (name, source, media_type) in [
        (
            "sdk",
            agentx_runtime_contracts::APPLICATION_PAGE_SDK_SOURCE,
            "application/javascript",
        ),
        (
            "style",
            agentx_runtime_contracts::APPLICATION_PAGE_STYLE_SOURCE,
            "text/css",
        ),
    ] {
        let id = Uuid::now_v7();
        let bytes = Bytes::from(source.as_bytes().to_vec());
        let receipt = persist_upload(
            &fixture.state,
            RuntimeObjectUploadMetadataV1 {
                api_version: 1,
                idempotency_key: format!("page-fixture:{name}"),
                tenant_id: fixture.tenant_id,
                object_id: id,
                content_hash: agentx_runtime_contracts::ContentHash::parse(format!(
                    "sha256:{:x}",
                    Sha256::digest(&bytes)
                ))
                .unwrap(),
                size_bytes: bytes.len() as u64,
                media_type: media_type.into(),
            },
            bytes,
        )
        .await
        .unwrap();
        assets.push(receipt.object);
    }
    let mut pages = Vec::new();
    for (revision, html) in [
        (1_u64, "<h1>Original page fixture</h1>"),
        (2, "<h1>Updated page fixture</h1>"),
    ] {
        let object = Uuid::now_v7();
        let bytes = Bytes::from(html.as_bytes().to_vec());
        let receipt = persist_upload(
            &fixture.state,
            RuntimeObjectUploadMetadataV1 {
                api_version: 1,
                idempotency_key: format!("page-fixture:html:{revision}"),
                tenant_id: fixture.tenant_id,
                object_id: object,
                content_hash: agentx_runtime_contracts::ContentHash::parse(format!(
                    "sha256:{:x}",
                    Sha256::digest(&bytes)
                ))
                .unwrap(),
                size_bytes: bytes.len() as u64,
                media_type: "text/html".into(),
            },
            bytes,
        )
        .await
        .unwrap();
        let contract = ApplicationPageContractV1 {
            application_id: fixture.application_id,
            application_name: "Page fixture".into(),
            description: None,
            deployment_id: bundle.payload.deployment_id,
            workflow_version_id: bundle.payload.workflow_version_id,
            bundle_id: bundle.payload.bundle_id,
            input_schema: bundle.payload.input_contract.clone(),
            output_schema: bundle.payload.output_contract.clone(),
            wait_input_nodes: vec![],
            approval_stages: vec![],
            chat_mapping: None,
            sdk_version: 1,
            max_upload_bytes: 50 * 1024 * 1024,
        };
        let page = ApplicationPagePublicationV1 {
            tenant_id: fixture.tenant_id,
            application_id: fixture.application_id,
            publication_id: Uuid::now_v7(),
            ui_version_id: Uuid::now_v7(),
            revision,
            expected_revision: revision - 1,
            slug: "employee-page".into(),
            enabled: true,
            contract_hash: content_hash(&contract).unwrap(),
            contract,
            html: receipt.object,
            html_hash: content_hash(&html).unwrap(),
            style_version: 1,
            sdk: assets[0].clone(),
            stylesheet: assets[1].clone(),
        };
        let prepared = PreparePagePublicationV1 {
            content_hash: content_hash(&page).unwrap(),
            signature: sign_canonical("bundle-current", &fixture.signing_key, &page).unwrap(),
            publication: page.clone(),
        };
        let _ = application_pages::prepare(
            State(fixture.state.clone()),
            publisher_headers("runtime.pages.prepare"),
            Json(prepared.clone()),
        )
        .await
        .unwrap();
        let _ = application_pages::prepare(
            State(fixture.state.clone()),
            publisher_headers("runtime.pages.prepare"),
            Json(prepared.clone()),
        )
        .await
        .unwrap();
        let current: Option<u64> = sqlx::query_scalar(
            "SELECT revision FROM application_web_heads WHERE tenant_id=? AND application_id=?",
        )
        .bind(fixture.tenant_id)
        .bind(fixture.application_id)
        .fetch_optional(&fixture.state.pool)
        .await
        .unwrap();
        assert_eq!(
            current,
            if revision == 1 { None } else { Some(1) },
            "preparation must not change the active pair"
        );
        let mut invalid = prepared;
        invalid.publication.html_hash = content_hash(&"tampered").unwrap();
        assert!(
            application_pages::prepare(
                State(fixture.state.clone()),
                publisher_headers("runtime.pages.prepare"),
                Json(invalid)
            )
            .await
            .is_err()
        );
        let activation = ActivatePagePublicationV1 {
            tenant_id: fixture.tenant_id,
            application_id: fixture.application_id,
            publication_id: page.publication_id,
            expected_revision: revision - 1,
            enabled: true,
        };
        let _ = application_pages::activate(
            State(fixture.state.clone()),
            publisher_headers("runtime.pages.activate"),
            Json(activation.clone()),
        )
        .await
        .unwrap();
        let _ = application_pages::activate(
            State(fixture.state.clone()),
            publisher_headers("runtime.pages.activate"),
            Json(activation),
        )
        .await
        .unwrap();
        if revision == 1 {
            let response=router.clone().oneshot(Request::builder().method("POST").uri("/pages/employee-page/invocations").header(AUTHORIZATION,format!("Bearer {token}")).header("content-type","application/json").header("Idempotency-Key","page-fixture:original").body(Body::from(json!({"publicationId":page.publication_id,"revision":revision,"input":{"message":"employee quotation"}}).to_string())).unwrap()).await.unwrap();
            let status = response.status();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(
                status,
                axum::http::StatusCode::ACCEPTED,
                "{}",
                String::from_utf8_lossy(&body)
            );
            let task: TaskViewV1 = serde_json::from_slice(&body).unwrap();
            assert_eq!(task.bundle_id, bundle.payload.bundle_id);
            let current = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/pages/employee-page?taskId={}", task.task_id))
                        .header(AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(current.status(), axum::http::StatusCode::OK);
            let current: ApplicationPageViewV1 =
                serde_json::from_slice(&to_bytes(current.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert!(
                !current.historical,
                "Reopening a task on the current active page must still allow its declared conversation"
            );
            pages.push((page, task.task_id));
        } else {
            pages.push((page, pages[0].1));
        }
    }
    let original = &pages[0].0;
    let task = pages[0].1;
    let stale=router.clone().oneshot(Request::builder().method("POST").uri("/pages/employee-page/invocations").header(AUTHORIZATION,format!("Bearer {token}")).header("content-type","application/json").header("Idempotency-Key","page-fixture:stale-new-request").body(Body::from(json!({"publicationId":original.publication_id,"revision":1,"input":{"message":"stale input"}}).to_string())).unwrap()).await.unwrap();
    assert_eq!(stale.status(), axum::http::StatusCode::CONFLICT);
    for (path, expected) in [
        ("/pages/employee-page".to_owned(), &pages[1].0),
        (format!("/pages/employee-page?taskId={task}"), original),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&body)
        );
        let view: ApplicationPageViewV1 = serde_json::from_slice(&body).unwrap();
        assert_eq!(view.publication.ui_version_id, expected.ui_version_id);
        assert_eq!(content_hash(&view.html).unwrap(), expected.html_hash);
    }
    // A second service identity cannot read this employee's task, even in the same application.
    let denied = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/tasks/{task}"))
                .header(AUTHORIZATION, format!("Bearer {}", fixture.api_key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), axum::http::StatusCode::NOT_FOUND);
    // A conflicting suffix must never update another application's head through a unique-key upsert.
    let other_app = Uuid::now_v7();
    let mut other = pages[1].0.clone();
    other.application_id = other_app;
    other.publication_id = Uuid::now_v7();
    other.expected_revision = 0;
    other.revision = 1;
    sqlx::query("INSERT INTO application_routes(application_id,tenant_id,route_key,active_bundle_id,admission_epoch,status) VALUES(?,?,'other-page-fixture',?,1,'active')").bind(other_app).bind(fixture.tenant_id).bind(bundle.payload.bundle_id).execute(&fixture.state.pool).await.unwrap();
    sqlx::query("INSERT INTO application_page_publications(id,tenant_id,application_id,ui_version_id,bundle_id,revision,content_hash,manifest_json,html_object_id,sdk_object_id,style_object_id,status) VALUES(?,?,?,?,?,1,?,?,?,?,?,'prepared')").bind(other.publication_id).bind(fixture.tenant_id).bind(other_app).bind(other.ui_version_id).bind(bundle.payload.bundle_id).bind(content_hash(&other).unwrap().as_str()).bind(serde_json::to_value(&other).unwrap()).bind(other.html.object_id).bind(other.sdk.object_id).bind(other.stylesheet.object_id).execute(&fixture.state.pool).await.unwrap();
    assert!(
        application_pages::activate(
            State(fixture.state.clone()),
            publisher_headers("runtime.pages.activate"),
            Json(ActivatePagePublicationV1 {
                tenant_id: fixture.tenant_id,
                application_id: other_app,
                publication_id: other.publication_id,
                expected_revision: 0,
                enabled: true
            })
        )
        .await
        .is_err()
    );
    let owner:Uuid=sqlx::query_scalar("SELECT application_id FROM application_web_heads WHERE tenant_id=? AND slug='employee-page'").bind(fixture.tenant_id).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(owner, fixture.application_id);
    sqlx::query("UPDATE runtime_objects SET ready_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 2 HOUR) WHERE tenant_id=? AND media_type IN ('text/html','text/css','application/javascript')").bind(fixture.tenant_id).execute(&fixture.state.pool).await.unwrap();
    mark_collectable(&fixture.state, Uuid::now_v7())
        .await
        .unwrap();
    let protected:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runtime_objects WHERE tenant_id=? AND media_type IN ('text/html','text/css','application/javascript') AND status='ready'").bind(fixture.tenant_id).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(protected, 4);
}

async fn publication_and_rollback_keep_revoked_service_keys_revoked(base: &Fixture) {
    let mut fixture = Fixture::new(
        base.state.pool.clone(),
        base.observer.clone(),
        base.state.admission_redis.clone().unwrap(),
    );
    fixture.api_key = format!("axk_{}", Uuid::new_v4().simple());
    let first = fixture.bundle(1).await;
    fixture
        .upload_bundle_object(&first, "key-independent:first-object")
        .await;
    prepare(&fixture, &first, "key-independent:first-prepare").await;
    apply_initial_admission(&fixture, 1).await;
    apply_api_key(&fixture, 2, AdmissionStatusV1::Revoked).await;
    // There is no active service key even on this first activation.
    activate(&fixture, &first, None, 1, 1).await;
    let second = fixture.bundle(2).await;
    fixture
        .upload_bundle_object(&second, "key-independent:second-object")
        .await;
    prepare(&fixture, &second, "key-independent:second-prepare").await;
    activate(&fixture, &second, Some(1), 2, 1).await;
    rollback(&fixture, &first, Some(2), 3, 1).await;
    let state: String =
        sqlx::query_scalar("SELECT status FROM api_key_admission WHERE tenant_id=? AND key_id=?")
            .bind(fixture.tenant_id)
            .bind(fixture.key_id)
            .fetch_one(&fixture.state.pool)
            .await
            .unwrap();
    assert_eq!(state, "revoked");
    assert!(matches!(
        authenticate_api_key(&fixture.state.pool, "runtime-slice", &fixture.api_key).await,
        Err(RuntimeError::Unauthorized)
    ));
}
