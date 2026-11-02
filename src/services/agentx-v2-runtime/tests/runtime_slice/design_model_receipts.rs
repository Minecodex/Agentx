async fn design_model_receipts_follow_the_frozen_deadline(fixture: &Fixture) {
    use agentx_runtime_contracts::{
        ApplicationUiBuilderTargetV1, DesignModelReceiptRequestV1, DesignModelTargetV1,
    };
    let target = DesignModelTargetV1::ApplicationUiBuilder(ApplicationUiBuilderTargetV1 {
        tenant_id: fixture.tenant_id,
        application_id: fixture.application_id,
        ui_draft_id: Uuid::now_v7(),
        bundle_id: Uuid::now_v7(),
        contract_hash: agentx_runtime_contracts::content_hash(&json!({})).unwrap(),
        session_id: Uuid::now_v7(),
        run_id: Uuid::now_v7(),
        user_id: Uuid::now_v7(),
    });
    let now = (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as u64;
    for (deadline, expected) in [(now + 300_000, "sent"), (now - 1, "outcome_unknown")] {
        let effect_id = Uuid::now_v7();
        sqlx::query("INSERT INTO runtime_calls(id,tenant_id,source_kind,design_target_json,call_index,call_kind,idempotency_key,request_fingerprint,status,request_json,started_at) VALUES(?,?,'application_ui_builder',?,0,'model',?,?,'sent',?,UTC_TIMESTAMP(6)-INTERVAL 180 SECOND)")
            .bind(effect_id).bind(fixture.tenant_id).bind(json!(target))
            .bind(format!("deadline-test:{effect_id}")).bind("0".repeat(64))
            .bind(json!({"context":{"deadlineAtMillis":deadline}}))
            .execute(&fixture.state.pool).await.unwrap();
        let request = DesignModelReceiptRequestV1 { target: target.clone(), effect_id, after: 0 };
        let headers = design_model_headers(&request, "runtime.design-model.inspect");
        let receipt = agentx_v2_runtime::builder_model::inspect(
            State(fixture.state.clone()), headers, Json(request.clone()),
        ).await.unwrap().0;
        assert_eq!(receipt.status, expected, "Elapsed 150 seconds cannot override the frozen Run deadline");
        if expected == "sent" {
            let cancelled = agentx_v2_runtime::builder_model::cancel(
                State(fixture.state.clone()),
                design_model_headers(&request, "runtime.design-model.cancel"), Json(request),
            ).await.unwrap().0;
            assert_eq!(cancelled.status, "outcome_unknown", "A dispatched model request cannot be reported as unsent");
            assert!(!cancelled.usage_known, "Unknown usage must retain its reservation");
        }
    }
}

fn design_model_headers(request: &agentx_runtime_contracts::DesignModelReceiptRequestV1, scope: &str) -> HeaderMap {
    let target = &request.target;
    let hash = agentx_runtime_contracts::content_hash(&json!(request)).unwrap();
    let now = now_unix();
    bearer_headers(issue_service_token("publisher-current", PRIVATE_KEY, &ServiceClaimsV1 {
        iss: "agentx-control".into(), aud: "agentx-runtime-internal".into(),
        sub: format!("design-agent:{}:{}:{}", target.source_kind(), target.identity().tenant_id, target.identity().run_id),
        role: ControlRole::Publisher,
        scope: BTreeSet::from([scope.into(), format!("design-body:{}", hash.as_str())]),
        iat: now, exp: now + 300, jti: Uuid::now_v7(),
    }).unwrap())
}
