use axum::Router;
use serde_json::{Value, json};
use uuid::Uuid;

use super::request;
use crate::control_api::ControlApiState;

pub(super) async fn assert_cancelled_model_references(
    app: &Router,
    state: &ControlApiState,
    token: &str,
    tenant: Uuid,
    draft: &Value,
    model: &Value,
) {
    let workflow = draft["workflowId"].as_str().unwrap();
    let document =
        json!({"definition":draft["definition"],"editorDocument":draft["editorDocument"]});
    let (status, session) = request(
        app,
        "POST",
        &format!(
            "/api/v1/workflows/{workflow}/drafts/{}/builder-sessions",
            draft["id"].as_str().unwrap()
        ),
        Some(token),
        Some(json!({"snapshot":{
            "editorInstanceId":Uuid::now_v7(),"localDocumentRevision":0,
            "documentHash":agentx_domain::canonical_content_hash(&document).unwrap(),
            "serverRevision":draft["revision"],"saveStatus":"saved","document":document,
            "selectedNodeIds":[],"selectedExecutionId":null
        }})),
    )
    .await;
    assert_eq!(status, 200, "{session}");
    let session = Uuid::parse_str(session["id"].as_str().unwrap()).unwrap();
    let run = Uuid::now_v7();
    let effect = Uuid::now_v7();
    let model_id = Uuid::parse_str(model["id"].as_str().unwrap()).unwrap();
    let credential = Uuid::parse_str(model["credentialId"].as_str().unwrap()).unwrap();
    sqlx::query("INSERT INTO design_agent_runs(id,tenant_id,session_id,token_version,prompt,edit_scope_json,model_alias_id,model_alias_version,credential_id,setting_version,model_binding_json,model_parameters_json,status,deadline_at,completed_at,delta_cursors_json) VALUES(?,?,?,1,'cancelled model reference',JSON_OBJECT(),?,1,?,1,JSON_OBJECT(),JSON_OBJECT(),'cancelled',UTC_TIMESTAMP(6)-INTERVAL 40 DAY,UTC_TIMESTAMP(6)-INTERVAL 40 DAY,JSON_OBJECT())")
        .bind(run).bind(tenant).bind(session).bind(model_id).bind(credential).execute(&state.pool).await.unwrap();
    // A crash can lose the receipt while the external request remains unresolved.
    sqlx::query("INSERT INTO design_agent_entries(tenant_id,session_id,run_id,event_key,payload_json) VALUES(?,?,?,?,?)")
        .bind(tenant).bind(session).bind(run).bind(format!("model-dispatch:{effect}")).bind(json!({"type":"model_dispatch","effectId":effect})).execute(&state.pool).await.unwrap();
    for (kind, id) in [("model", model_id), ("credential", credential)] {
        let (status, impact) = request(
            app,
            "GET",
            &format!("/api/v1/deletion-impact/{kind}/{id}?pageSize=100"),
            Some(token),
            None,
        )
        .await;
        assert_eq!(status, 200, "{impact}");
        assert!(
            impact["references"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["sourceType"] == "studio_builder_run"
                    && item["sourceId"] == run.to_string()),
            "A cancelled Run must retain its unresolved model: {impact}"
        );
    }
    let (status, _) = request(
        app,
        "DELETE",
        &format!("/api/v1/workflows/{workflow}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(
        status, 409,
        "Workflow history still owns an unresolved model"
    );
    let (status, error) = request(
        app,
        "DELETE",
        &format!("/api/v1/models/aliases/{model_id}"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, 409, "{error}");
    assert_eq!(error["code"], "RESOURCE_REFERENCED");
    sqlx::query(
        "UPDATE design_agent_sessions SET updated_at=UTC_TIMESTAMP(6)-INTERVAL 40 DAY WHERE id=?",
    )
    .bind(session)
    .execute(&state.pool)
    .await
    .unwrap();
    let retention = Uuid::now_v7();
    crate::studio_builder_retention::collect(&state.pool, tenant, retention, 1, true)
        .await
        .unwrap();
    let candidates: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM retention_items WHERE retention_run_id=? AND target_id=?",
    )
    .bind(retention)
    .bind(session.to_string())
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        candidates, 0,
        "Stopped creation cannot authorize deleting unknown effects"
    );
    // Only an actual known Runtime receipt may release this reference.
    sqlx::query("INSERT INTO design_agent_entries(tenant_id,session_id,run_id,event_key,payload_json) VALUES(?,?,?,?,?)")
        .bind(tenant).bind(session).bind(run).bind(format!("model-resolved:{effect}")).bind(json!({"type":"model_receipt","effectId":effect,"status":"cancelled"})).execute(&state.pool).await.unwrap();
    let (status, impact) = request(
        app,
        "GET",
        &format!("/api/v1/deletion-impact/model/{model_id}?pageSize=100"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        !impact["references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["sourceId"] == run.to_string()),
        "Known cancellation must release the original reference: {impact}"
    );
    crate::studio_builder_retention::collect(&state.pool, tenant, retention, 1, true)
        .await
        .unwrap();
    let candidates: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM retention_items WHERE retention_run_id=? AND target_id=?",
    )
    .bind(retention)
    .bind(session.to_string())
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        candidates, 1,
        "Settled old history remains eligible for normal retention"
    );
    sqlx::query("DELETE FROM retention_items WHERE retention_run_id=?")
        .bind(retention)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM design_agent_runs WHERE id=?")
        .bind(run)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM design_agent_sessions WHERE id=?")
        .bind(session)
        .execute(&state.pool)
        .await
        .unwrap();
}
