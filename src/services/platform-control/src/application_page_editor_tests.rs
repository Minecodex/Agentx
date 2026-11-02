use super::request;
use axum::Router;
use serde_json::json;
use sqlx::{MySqlPool, Row};
use uuid::Uuid;

pub(super) async fn persistent_page_conversation(
    app: &Router,
    pool: &MySqlPool,
    token: &str,
    application: Uuid,
    user: Uuid,
) {
    let previous_status: String = sqlx::query_scalar("SELECT status FROM applications WHERE id=?")
        .bind(application)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE applications SET status='active' WHERE id=?")
        .bind(application)
        .execute(pool)
        .await
        .unwrap();
    let tenant: Uuid = sqlx::query_scalar("SELECT tenant_id FROM applications WHERE id=?")
        .bind(application)
        .fetch_one(pool)
        .await
        .unwrap();
    let draft = Uuid::now_v7();
    let html = "<h1>Continuously edited page</h1>";
    let hash = agentx_runtime_contracts::content_hash(&html).unwrap();
    let bundle = Uuid::now_v7();
    let contract = json!({"applicationId":application,"applicationName":"Editor test","description":null,"deploymentId":Uuid::now_v7(),"workflowVersionId":Uuid::now_v7(),"bundleId":bundle,"inputSchema":{"type":"object"},"outputSchema":{"type":"object"},"waitInputNodes":[],"approvalStages":[],"chatMapping":null,"sdkVersion":1,"maxUploadBytes":1024});
    let contract_hash = agentx_runtime_contracts::content_hash(&contract).unwrap();
    sqlx::query("INSERT INTO application_ui_drafts(id,tenant_id,application_id,bundle_id,contract_json,contract_hash,html,html_hash,created_by) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(draft).bind(tenant).bind(application).bind(bundle).bind(contract).bind(contract_hash.as_str()).bind(html).bind(hash.as_str()).bind(user).execute(pool).await.unwrap();
    let base = format!("/api/v1/applications/{application}/ui/drafts");
    for _ in 0..2 {
        let (status, page) = request(
            app,
            "POST",
            &base,
            Some(token),
            Some(json!({"deploymentId":Uuid::now_v7()})),
        )
        .await;
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["id"], json!(draft));
    }
    let snapshot = json!({"editorInstanceId":Uuid::now_v7(),"localRevision":0,"serverRevision":1,"html":html,"htmlHash":hash,"saveStatus":"saved"});
    let (status, first) = request(
        app,
        "POST",
        &format!("{base}/{draft}/builder-sessions"),
        Some(token),
        Some(json!({"snapshot":snapshot})),
    )
    .await;
    assert_eq!(status, 200, "{first}");
    let session = first["id"].as_str().unwrap();
    sqlx::query("UPDATE design_agent_sessions SET state_json=JSON_SET(state_json,'$.lastConversationMarker','preserved') WHERE id=UUID_TO_BIN(?)").bind(session).execute(pool).await.unwrap();
    let mut replacement = snapshot.clone();
    replacement["editorInstanceId"] = json!(Uuid::now_v7());
    let (status, second) = request(
        app,
        "POST",
        &format!("{base}/{draft}/builder-sessions"),
        Some(token),
        Some(json!({"snapshot":replacement})),
    )
    .await;
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["id"], first["id"]);
    assert_eq!(second["snapshot"]["serverRevision"], 2);
    assert_eq!(second["snapshot"]["html"], html);
    let row=sqlx::query("SELECT state_json,(SELECT COUNT(*) FROM design_agent_sessions WHERE ui_draft_id=?) session_count FROM design_agent_sessions WHERE id=UUID_TO_BIN(?)").bind(draft).bind(session).fetch_one(pool).await.unwrap();
    let state: serde_json::Value = serde_json::from_str(row.get("state_json")).unwrap();
    assert_eq!(state["lastConversationMarker"], "preserved");
    assert_eq!(row.get::<i64, _>("session_count"), 1);
    let (status, _) = request(
        app,
        "PUT",
        &format!("/api/v1/application-ui/sessions/{session}/snapshot"),
        Some(token),
        Some(snapshot),
    )
    .await;
    assert_eq!(status, 409, "The previous window must lose its binding");
    let (status, _) = request(
        app,
        "PUT",
        &format!("{base}/{draft}"),
        Some(token),
        Some(json!({"saveId":Uuid::now_v7(),"baseRevision":1,"html":"Old window write"})),
    )
    .await;
    assert_eq!(
        status, 409,
        "A previous-window save must not overwrite the current HTML"
    );
    let (status, listed) = request(app, "GET", &base, Some(token), None).await;
    assert_eq!(status, 200);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["html"], html);
    sqlx::query("UPDATE applications SET status=? WHERE id=?")
        .bind(previous_status)
        .bind(application)
        .execute(pool)
        .await
        .unwrap();
}
