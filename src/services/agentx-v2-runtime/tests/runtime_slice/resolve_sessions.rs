async fn resolve_sessions_reuse_only_their_container_and_fence_cleanup(pool: &MySqlPool) {
    use agentx_runtime_contracts::{PluginDesignOperationRequestV1, PluginResolveSessionV1};
    use std::sync::atomic::AtomicUsize;
    let creates = Arc::new(AtomicUsize::new(0));
    let deletes = Arc::new(AtomicUsize::new(0));
    let commands = Arc::new(AtomicUsize::new(0));
    let runner_uploads = Arc::new(AtomicUsize::new(0));
    let input_uploads = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicBool::new(false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let create_count = creates.clone();
    let delete_count = deletes.clone();
    let command_count = commands.clone();
    let command_fail = fail.clone();
    let runner_count = runner_uploads.clone();
    let input_count = input_uploads.clone();
    let execd_endpoint = endpoint.clone();
    let provider=Router::new()
        .route("/v1/sandboxes",post(move || {let count=create_count.clone();async move {
            let id=count.fetch_add(1,Ordering::SeqCst)+1;
            Json(json!({"id":format!("resolve-{id}"),"status":{"state":"Running"}}))
        }}))
        .route("/v1/sandboxes/{id}/endpoints/44772",get(move |Path(id):Path<String>|{let endpoint=execd_endpoint.clone();async move {
            Json(json!({"endpoint":format!("{endpoint}/v1/sandboxes/{id}/proxy/44772"),"headers":{}}))
        }}))
        .route("/v1/sandboxes/{id}/proxy/44772/ping",get(||async {axum::http::StatusCode::OK}))
        .route("/v1/sandboxes/{id}/proxy/44772/files/upload",post(move |mut form:axum::extract::Multipart| {
            let runner_count=runner_count.clone();let input_count=input_count.clone();async move {
                let mut path=None;
                while let Some(field)=form.next_field().await.unwrap() {
                    if field.name()==Some("metadata") {
                        let metadata:Value=serde_json::from_slice(&field.bytes().await.unwrap()).unwrap();
                        path=metadata["path"].as_str().map(str::to_owned);
                    } else {let _=field.bytes().await.unwrap();}
                }
                match path.as_deref() {
                    Some("/tmp/agentx-resolve-runner.mjs")=>{runner_count.fetch_add(1,Ordering::SeqCst);},
                    Some("/tmp/agentx-resolve-input.jsonl")=>{input_count.fetch_add(1,Ordering::SeqCst);},
                    other=>panic!("unexpected resolve upload: {other:?}"),
                }
                axum::http::StatusCode::OK
            }
        }))
        .route("/v1/sandboxes/{id}/proxy/44772/command",post(move |Json(body):Json<Value>| {
            let count=command_count.clone();let fail=command_fail.clone();async move {
                count.fetch_add(1,Ordering::SeqCst);
                let command=body["command"].as_str().unwrap();
                assert!(command.len()<512 && command.contains("node --experimental-permission"));
                assert!(command.contains("--allow-fs-read=/tmp/agentx-resolve-runner.mjs"));
                assert!(!command.contains("untrusted-source"));
                let stdout=json!({"type":"stdout","text":"{\"jsonrpc\":\"2.0\",\"id\":\"resolve\",\"result\":{\"status\":\"complete\"}}"});
                let result=json!({"type":"result","exit_code":i32::from(fail.swap(false,Ordering::SeqCst))});
                format!("data: {stdout}\n\ndata: {result}\n\n")
            }
        }))
        .route("/v1/sandboxes/{id}",axum::routing::delete(move ||{let count=delete_count.clone();async move {
            count.fetch_add(1,Ordering::SeqCst);axum::http::StatusCode::NO_CONTENT
        }}));
    let provider_task = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
    let state = agentx_v2_runtime::sandbox::SandboxManagerState {
        pool: pool.clone(),
        client: reqwest::Client::new(),
        provider_endpoint: endpoint,
        provider_api_key: None,
        provider_secure_access: false,
        owner: Uuid::now_v7(),
        vault: None,
        objects: Arc::new(InMemory::new()),
        process_sockets: Default::default(),
    };
    let manager = agentx_v2_runtime::sandbox::router(state.clone());
    let tenant = Uuid::now_v7();
    let session = Uuid::now_v7();
    let mut request:PluginDesignOperationRequestV1=serde_json::from_value(json!({
        "protocolVersion":1,"tenantId":tenant,"operationId":Uuid::now_v7(),"resolveSessionId":session,
        "method":"node.resolveDefinition","plugin":{"packageId":"test/resolve","packageVersion":"1.0.0",
            "bundleDigest":format!("sha256:{}","a".repeat(64)),"runtimeEntry":"runtime.js","runtimeSource":"untrusted-source"},
        "parameters":{"configuration":{"nonce":1}}
    })).unwrap();
    let first = request.clone();
    for nonce in [1, 2] {
        request.operation_id = if nonce == 1 {
            first.operation_id
        } else {
            Uuid::now_v7()
        };
        request.parameters = json!({"configuration":{"nonce":nonce}});
        assert_eq!(
            resolve_session_request(manager.clone(), "sandboxes:resolve", &request)
                .await
                .status(),
            axum::http::StatusCode::OK
        );
    }
    assert_eq!(creates.load(Ordering::SeqCst), 1);
    assert_eq!(commands.load(Ordering::SeqCst), 2);
    assert_eq!(runner_uploads.load(Ordering::SeqCst), 1);
    assert_eq!(input_uploads.load(Ordering::SeqCst), 2);
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    assert_eq!(
        resolve_session_request(manager.clone(), "sandboxes:resolve", &first)
            .await
            .status(),
        axum::http::StatusCode::OK
    );
    assert_eq!(
        commands.load(Ordering::SeqCst),
        2,
        "completed operations replay without a process"
    );
    sqlx::query("UPDATE sandbox_design_leases SET status='executing',owner=? WHERE operation_id=?")
        .bind(Uuid::now_v7())
        .bind(session)
        .execute(pool)
        .await
        .unwrap();
    let mut busy = first.clone();
    busy.operation_id = Uuid::now_v7();
    assert_eq!(
        resolve_session_request(manager.clone(), "sandboxes:resolve", &busy)
            .await
            .status(),
        axum::http::StatusCode::CONFLICT
    );
    assert_eq!(
        commands.load(Ordering::SeqCst),
        2,
        "a session permits only one active process"
    );
    sqlx::query("UPDATE sandbox_design_leases SET status='idle' WHERE operation_id=?")
        .bind(session)
        .execute(pool)
        .await
        .unwrap();
    let mut conflict = first.clone();
    conflict.parameters = json!({"different":true});
    assert_eq!(
        resolve_session_request(manager.clone(), "sandboxes:resolve", &conflict)
            .await
            .status(),
        axum::http::StatusCode::CONFLICT
    );
    let mut other_tenant = first.clone();
    other_tenant.tenant_id = Uuid::now_v7();
    other_tenant.operation_id = Uuid::now_v7();
    assert_eq!(
        resolve_session_request(manager.clone(), "sandboxes:resolve", &other_tenant)
            .await
            .status(),
        axum::http::StatusCode::CONFLICT
    );
    let close = PluginResolveSessionV1 {
        protocol_version: 1,
        tenant_id: tenant,
        session_id: session,
    };
    let wrong_close = PluginResolveSessionV1 {
        tenant_id: other_tenant.tenant_id,
        ..close.clone()
    };
    assert_eq!(
        resolve_session_request(
            manager.clone(),
            "sandboxes:close-resolve-session",
            &wrong_close
        )
        .await
        .status(),
        axum::http::StatusCode::CONFLICT
    );
    for _ in 0..2 {
        assert_eq!(
            resolve_session_request(manager.clone(), "sandboxes:close-resolve-session", &close)
                .await
                .status(),
            axum::http::StatusCode::OK
        );
    }
    assert_eq!(deletes.load(Ordering::SeqCst), 1, "close is idempotent");
    assert_eq!(
        resolve_session_request(manager.clone(), "sandboxes:resolve", &first)
            .await
            .status(),
        axum::http::StatusCode::OK
    );
    // A late cleanup receipt must not keep the reaper cycling forever on a
    // child whose session has already been closed.
    sqlx::query("UPDATE sandbox_design_leases SET status='cleanup_pending',lease_until=UTC_TIMESTAMP(6)-INTERVAL 1 SECOND WHERE operation_id=?")
        .bind(first.operation_id).execute(pool).await.unwrap();
    assert!(
        agentx_v2_runtime::sandbox::reconcile_one(&state)
            .await
            .unwrap()
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM sandbox_design_leases WHERE operation_id=?")
            .bind(first.operation_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(deletes.load(Ordering::SeqCst), 1);
    request.operation_id = Uuid::now_v7();
    assert_eq!(
        resolve_session_request(manager.clone(), "sandboxes:resolve", &request)
            .await
            .status(),
        axum::http::StatusCode::CONFLICT
    );
    let cancelled = PluginResolveSessionV1 {
        session_id: Uuid::now_v7(),
        ..close.clone()
    };
    assert_eq!(
        resolve_session_request(
            manager.clone(),
            "sandboxes:close-resolve-session",
            &cancelled
        )
        .await
        .status(),
        axum::http::StatusCode::OK
    );
    request.resolve_session_id = Some(cancelled.session_id);
    assert_eq!(
        resolve_session_request(manager.clone(), "sandboxes:resolve", &request)
            .await
            .status(),
        axum::http::StatusCode::CONFLICT
    );
    assert_eq!(
        creates.load(Ordering::SeqCst),
        1,
        "a late request cannot reopen a closed session"
    );
    fail.store(true, Ordering::SeqCst);
    request.resolve_session_id = Some(Uuid::now_v7());
    request.operation_id = Uuid::now_v7();
    assert_eq!(
        resolve_session_request(manager.clone(), "sandboxes:resolve", &request)
            .await
            .status(),
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        deletes.load(Ordering::SeqCst),
        2,
        "a resolver failure tears its container down"
    );
    request.resolve_session_id = Some(Uuid::now_v7());
    request.operation_id = Uuid::now_v7();
    assert_eq!(
        resolve_session_request(manager.clone(), "sandboxes:resolve", &request)
            .await
            .status(),
        axum::http::StatusCode::OK
    );
    sqlx::query("UPDATE sandbox_design_leases SET lease_until=UTC_TIMESTAMP(6)-INTERVAL 1 SECOND WHERE operation_id=?")
        .bind(request.resolve_session_id).execute(pool).await.unwrap();
    assert!(
        agentx_v2_runtime::sandbox::reconcile_one(&state)
            .await
            .unwrap()
    );
    assert_eq!(
        deletes.load(Ordering::SeqCst),
        3,
        "an abandoned idle session is reaped"
    );
    let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM sandbox_design_leases WHERE tenant_id=? AND status IN ('creating','executing','idle','cleanup_pending')")
        .bind(tenant).fetch_one(pool).await.unwrap();
    assert_eq!(active, 0);
    provider_task.abort();
}

async fn resolve_session_request(
    manager: Router,
    action: &str,
    body: &impl serde::Serialize,
) -> axum::response::Response {
    manager
        .oneshot(
            Request::post(format!("/internal/runtime/v1/{action}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}
