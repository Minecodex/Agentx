use agentx_agent_core::{
    AgentSessionStateV1, OperationKindV1, OperationPhaseV1, OperationRecordV1, ReplayPolicyV1,
};
use agentx_runtime_contracts::{DesignModelTargetV1, StudioBuilderTargetV1};
use axum::Router;
use serde_json::{Value, json};
use uuid::Uuid;

use super::request;
use crate::control_api::ControlApiState;

pub(super) async fn assert_running_input_protocol(
    app: &Router,
    state: &ControlApiState,
    token: &str,
    draft: &Value,
    tenant: Uuid,
    actor: Uuid,
) {
    let editor = Uuid::now_v7();
    let document =
        json!({"definition":draft["definition"],"editorDocument":draft["editorDocument"]});
    let snapshot = json!({
        "editorInstanceId":editor,"localDocumentRevision":0,
        "documentHash":agentx_domain::canonical_content_hash(&document).unwrap(),
        "serverRevision":draft["revision"],"saveStatus":"saved","document":document,
        "selectedNodeIds":[],"selectedExecutionId":null
    });
    let (status, session) = request(
        app,
        "POST",
        &format!(
            "/api/v1/workflows/{}/drafts/{}/builder-sessions",
            draft["workflowId"].as_str().unwrap(),
            draft["id"].as_str().unwrap()
        ),
        Some(token),
        Some(json!({"snapshot":snapshot})),
    )
    .await;
    assert_eq!(
        status, 200,
        "input test must bind a real authorized editor: {session}"
    );
    let session_id = Uuid::parse_str(session["id"].as_str().unwrap()).unwrap();
    let run = Uuid::now_v7();
    insert_run(state, tenant, session_id, run).await;
    let message = Uuid::now_v7();
    let body = json!({"messageId":message,"editorInstanceId":editor,"kind":"steering","text":"保留契约，修复补充表单的取值容器。"});
    let path = format!("/api/v1/design-agent/sessions/{session_id}/runs/{run}/inputs");
    let (status, receipt) = request(app, "POST", &path, Some(token), Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "a running authoring Run must accept durable guidance: {receipt}"
    );
    assert_eq!(receipt["messageId"], json!(message));
    assert_eq!(receipt["runId"], json!(run));
    assert_eq!(receipt["replayed"], false);
    let (status, replayed) = request(app, "POST", &path, Some(token), Some(body.clone())).await;
    assert_eq!(status, 200);
    assert_eq!(replayed["replayed"], true);
    let mut changed = body.clone();
    changed["text"] = json!("different guidance");
    let (status, error) = request(app, "POST", &path, Some(token), Some(changed)).await;
    assert_eq!(status, 409);
    assert_eq!(error["code"], "DESIGN_INPUT_CONFLICT");
    let mut stale = body.clone();
    stale["editorInstanceId"] = json!(Uuid::now_v7());
    let (status, error) = request(app, "POST", &path, Some(token), Some(stale)).await;
    assert_eq!(status, 409);
    assert_eq!(error["code"], "EDITOR_BINDING_STALE");
    let mut forged = body.clone();
    forged["profile"] = json!("application_ui_builder");
    let (status, _) = request(app, "POST", &path, Some(token), Some(forged)).await;
    assert!(matches!(status, 400 | 422));
    for text in [" ".to_owned(), "界".repeat(2049)] {
        let mut invalid = body.clone();
        invalid["text"] = json!(text);
        let (status, error) = request(app, "POST", &path, Some(token), Some(invalid)).await;
        assert_eq!(status, 400);
        assert_eq!(error["code"], "DESIGN_INPUT_INVALID");
    }
    let (status, other) = request(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"username":"api-operator","password":"api-operator-password"})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = request(
        app,
        "POST",
        &path,
        Some(other["accessToken"].as_str().unwrap()),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 404,
        "even another company administrator cannot steer this user's session"
    );
    let mut second_snapshot = snapshot.clone();
    second_snapshot["editorInstanceId"] = json!(Uuid::now_v7());
    let (status, second_session) = request(
        app,
        "POST",
        &format!(
            "/api/v1/workflows/{}/drafts/{}/builder-sessions",
            draft["workflowId"].as_str().unwrap(),
            draft["id"].as_str().unwrap()
        ),
        Some(token),
        Some(json!({"snapshot":second_snapshot})),
    )
    .await;
    assert_eq!(status, 200);
    let second = Uuid::parse_str(second_session["id"].as_str().unwrap()).unwrap();
    let mut cross_session = body.clone();
    cross_session["editorInstanceId"] = second_snapshot["editorInstanceId"].clone();
    let (status, _) = request(
        app,
        "POST",
        &format!("/api/v1/design-agent/sessions/{second}/runs/{run}/inputs"),
        Some(token),
        Some(cross_session),
    )
    .await;
    assert_eq!(
        status, 404,
        "a valid second editor cannot retarget the first editor's Run"
    );
    sqlx::query("DELETE FROM design_agent_sessions WHERE id=?")
        .bind(second)
        .execute(&state.pool)
        .await
        .unwrap();

    let workflow_target = StudioBuilderTargetV1 {
        tenant_id: tenant,
        user_id: actor,
        workflow_id: Uuid::parse_str(draft["workflowId"].as_str().unwrap()).unwrap(),
        draft_id: Uuid::parse_str(draft["id"].as_str().unwrap()).unwrap(),
        session_id,
        run_id: run,
    };
    let target: DesignModelTargetV1 = workflow_target.clone().into();
    let mut follow = body.clone();
    follow["messageId"] = json!(Uuid::now_v7());
    follow["kind"] = json!("follow_up");
    let (status, _) = request(app, "POST", &path, Some(token), Some(follow.clone())).await;
    assert_eq!(status, 200);
    let value: Value =
        sqlx::query_scalar("SELECT state_json FROM design_agent_sessions WHERE id=?")
            .bind(session_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    let mut core: AgentSessionStateV1 = serde_json::from_value(value).unwrap();
    core.operation = Some(OperationRecordV1 {
        operation_id: format!("{run}:model:1"),
        effect_id: "original-effect".into(),
        operation_kind: OperationKindV1::Model,
        attempt: 1,
        intent: json!({"request":{"messages":["original frozen request"]}}),
        effect_identity: "original-identity".into(),
        replay_policy: ReplayPolicyV1::Never,
        phase: OperationPhaseV1::Pending {
            token: "original-receipt".into(),
        },
        fencing_token: 1,
        created_at_millis: 1,
        updated_at_millis: 1,
    });
    let original = core.operation.clone();
    crate::design_agent_inputs::hydrate(state, &target, &mut core)
        .await
        .unwrap();
    crate::design_agent_inputs::hydrate(state, &target, &mut core)
        .await
        .unwrap();
    assert_eq!(core.steering_queue.len(), 1);
    assert_eq!(core.follow_up_queue.len(), 1);
    assert_eq!(
        core.operation, original,
        "new guidance cannot rewrite an in-flight model request"
    );
    sqlx::query("UPDATE design_agent_sessions SET state_json=? WHERE id=?")
        .bind(serde_json::to_value(&core).unwrap())
        .bind(session_id)
        .execute(&state.pool)
        .await
        .unwrap();
    let recovered: Value =
        sqlx::query_scalar("SELECT state_json FROM design_agent_sessions WHERE id=?")
            .bind(session_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    core = serde_json::from_value(recovered).unwrap();
    crate::design_agent_inputs::hydrate(state, &target, &mut core)
        .await
        .unwrap();
    assert_eq!(core.steering_queue.len(), 1);
    assert_eq!(core.follow_up_queue.len(), 1);
    assert_eq!(
        core.operation, original,
        "database recovery must retain the original pending Effect and each guidance once"
    );

    sqlx::query("UPDATE design_agent_runs SET status='running',fencing_token=1 WHERE id=?")
        .bind(run)
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        crate::design_agent_inputs::finish_run(state, &target, 1, "completed")
            .await
            .unwrap(),
        Some("waiting"),
        "guidance accepted before completion must get a Core step"
    );
    // The real Core retains full message history through compaction. Arrange a
    // consumed history, then prove hydration never reinserts those same inputs.
    core.messages.append(&mut core.steering_queue);
    core.messages.append(&mut core.follow_up_queue);
    core.operation = None;
    core.terminal = Some(agentx_agent_core::TerminalReasonV1::Completed);
    core.compaction = Some(agentx_agent_core::CompactionSnapshotV1 {
        snapshot_id: "retained-guidance".into(),
        kind: agentx_agent_core::CompactionKindV1::Threshold,
        summary: "已保留指导".into(),
        through_message_index: core.messages.len(),
        retained_tail: vec![],
        tokens_before: 100,
    });
    crate::design_agent_inputs::hydrate(state, &target, &mut core)
        .await
        .unwrap();
    assert!(core.steering_queue.is_empty() && core.follow_up_queue.is_empty());
    sqlx::query("UPDATE design_agent_sessions SET state_json=? WHERE id=?")
        .bind(serde_json::to_value(&core).unwrap())
        .bind(session_id)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE design_agent_runs SET status='running',fencing_token=2 WHERE id=?")
        .bind(run)
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        crate::design_agent_inputs::finish_run(state, &target, 1, "completed")
            .await
            .unwrap(),
        None,
        "old owner cannot finish a new fence"
    );
    assert_eq!(
        crate::design_agent_inputs::finish_run(state, &target, 2, "completed")
            .await
            .unwrap(),
        Some("completed")
    );
    let mut late = body.clone();
    late["messageId"] = json!(Uuid::now_v7());
    let (status, error) = request(app, "POST", &path, Some(token), Some(late)).await;
    assert_eq!(status, 409);
    assert_eq!(error["code"], "BUILDER_RUN_CLOSED");
    let (status, replayed) = request(app, "POST", &path, Some(token), Some(body.clone())).await;
    assert_eq!(status, 200);
    assert_eq!(replayed["replayed"], true);

    let race = Uuid::now_v7();
    insert_run(state, tenant, session_id, race).await;
    sqlx::query("UPDATE design_agent_runs SET status='running',fencing_token=1 WHERE id=?")
        .bind(race)
        .execute(&state.pool)
        .await
        .unwrap();
    let race_target: DesignModelTargetV1 = StudioBuilderTargetV1 {
        run_id: race,
        ..workflow_target.clone()
    }
    .into();
    let race_path = format!("/api/v1/design-agent/sessions/{session_id}/runs/{race}/inputs");
    let mut race_body = body.clone();
    race_body["messageId"] = json!(Uuid::now_v7());
    let ((status, _), finished) = tokio::join!(
        request(
            app,
            "POST",
            &race_path,
            Some(token),
            Some(race_body.clone())
        ),
        crate::design_agent_inputs::finish_run(state, &race_target, 1, "completed"),
    );
    match status {
        200 => assert_eq!(finished.unwrap(), Some("waiting")),
        409 => assert_eq!(finished.unwrap(), Some("completed")),
        other => panic!("guidance/completion race returned {other}"),
    }
    sqlx::query("UPDATE design_agent_runs SET status='cancelled' WHERE id=?")
        .bind(race)
        .execute(&state.pool)
        .await
        .unwrap();
    let mut after_stop = body.clone();
    after_stop["messageId"] = json!(Uuid::now_v7());
    let (status, _) = request(app, "POST", &race_path, Some(token), Some(after_stop)).await;
    assert_eq!(status, 409);
    assert_eq!(
        crate::design_agent_inputs::finish_run(state, &race_target, 1, "completed")
            .await
            .unwrap(),
        None
    );
    core.steering_queue = core
        .messages
        .iter()
        .filter(|message| message.message_id.starts_with("design-input:"))
        .cloned()
        .collect();
    crate::design_agent_inputs::hydrate(state, &race_target, &mut core)
        .await
        .unwrap();
    assert!(
        core.steering_queue.iter().all(|message| message
            .message_id
            .starts_with(&format!("design-input:{race}:"))),
        "a new Run cannot inherit an old Run's unconsumed queue"
    );

    for status in ["failed", "outcome_unknown"] {
        let closed = Uuid::now_v7();
        insert_run(state, tenant, session_id, closed).await;
        sqlx::query("UPDATE design_agent_runs SET status=? WHERE id=?")
            .bind(status)
            .bind(closed)
            .execute(&state.pool)
            .await
            .unwrap();
        let closed_target: DesignModelTargetV1 = StudioBuilderTargetV1 {
            run_id: closed,
            ..workflow_target.clone()
        }
        .into();
        let mut closed_body = body.clone();
        closed_body["messageId"] = json!(Uuid::now_v7());
        let (code, _) = request(
            app,
            "POST",
            &format!("/api/v1/design-agent/sessions/{session_id}/runs/{closed}/inputs"),
            Some(token),
            Some(closed_body),
        )
        .await;
        assert_eq!(code, 409, "{status} must not accept new guidance");
        assert_eq!(
            crate::design_agent_inputs::finish_run(state, &closed_target, 0, "completed")
                .await
                .unwrap(),
            None
        );
    }

    let quota = Uuid::now_v7();
    insert_run(state, tenant, session_id, quota).await;
    let quota_path = format!("/api/v1/design-agent/sessions/{session_id}/runs/{quota}/inputs");
    let mut last = body.clone();
    for _ in 0..32 {
        last["messageId"] = json!(Uuid::now_v7());
        let (status, _) = request(app, "POST", &quota_path, Some(token), Some(last.clone())).await;
        assert_eq!(status, 200);
    }
    let mut overflow = last.clone();
    overflow["messageId"] = json!(Uuid::now_v7());
    let (status, error) = request(app, "POST", &quota_path, Some(token), Some(overflow)).await;
    assert_eq!(status, 409);
    assert_eq!(error["code"], "DESIGN_INPUT_LIMIT");
    let (status, replayed) = request(app, "POST", &quota_path, Some(token), Some(last)).await;
    assert_eq!(status, 200);
    assert_eq!(replayed["replayed"], true);
    sqlx::query("DELETE FROM design_agent_runs WHERE tenant_id=? AND session_id=?")
        .bind(tenant)
        .bind(session_id)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM design_agent_sessions WHERE id=?")
        .bind(session_id)
        .execute(&state.pool)
        .await
        .unwrap();
}

async fn insert_run(state: &ControlApiState, tenant: Uuid, session: Uuid, run: Uuid) {
    sqlx::query("INSERT INTO design_agent_runs(id,tenant_id,session_id,token_version,prompt,edit_scope_json,model_alias_id,model_alias_version,setting_version,model_binding_json,model_parameters_json,deadline_at,delta_cursors_json) VALUES(?,?,?,1,'guidance test',JSON_OBJECT(),?,1,1,JSON_OBJECT(),JSON_OBJECT(),UTC_TIMESTAMP(6)+INTERVAL 10 MINUTE,JSON_OBJECT())")
        .bind(run).bind(tenant).bind(session).bind(Uuid::now_v7()).execute(&state.pool).await.unwrap();
}
