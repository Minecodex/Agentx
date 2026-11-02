use axum::Router;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use super::request;
use crate::control_api::ControlApiState;

/// The editor must reject the same protected-tool combination that Save rejects,
/// before issuing a command that leaves an unsaveable local recovery document.
pub(super) async fn assert_save_policy_is_checked_before_edit(
    app: &Router,
    state: &ControlApiState,
    token: &str,
    server: &Value,
    draft: &Value,
    template: &Value,
) {
    let server_id = Uuid::parse_str(server["id"].as_str().unwrap()).unwrap();
    let row = sqlx::query("SELECT tenant_id,created_by FROM mcp_servers WHERE id=?")
        .bind(server_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let tenant: Uuid = row.try_get("tenant_id").unwrap();
    let actor: Uuid = row.try_get("created_by").unwrap();
    let server_version: Uuid = sqlx::query_scalar(
        "SELECT id FROM mcp_server_versions WHERE tenant_id=? AND server_id=? ORDER BY version_number DESC LIMIT 1",
    )
    .bind(tenant)
    .bind(server_id)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let tool = Uuid::now_v7();
    sqlx::query("INSERT INTO mcp_tools(id,tenant_id,server_id,name,current_version_number) VALUES(?,?,?,'save_policy_probe',1)")
        .bind(tool).bind(tenant).bind(server_id).execute(&state.pool).await.unwrap();
    sqlx::query("INSERT INTO mcp_tool_versions(id,tenant_id,tool_id,discovery_run_id,server_version_id,version_number,input_schema,annotations_json,schema_hash) VALUES(?,?,?,?,?,1,JSON_OBJECT('type','object'),JSON_OBJECT(),?)")
        .bind(Uuid::now_v7()).bind(tenant).bind(tool).bind(Uuid::now_v7()).bind(server_version).bind("1".repeat(64)).execute(&state.pool).await.unwrap();
    let configuration = json!({"enabled":true,"debugEnabled":false,"timeoutSeconds":30,"sideEffect":"non_idempotent","approvalRule":{"userIds":[actor],"roleIds":[],"timeoutSeconds":600}});
    sqlx::query("INSERT INTO mcp_tool_policies(tenant_id,tool_id,enabled,debug_enabled,side_effect,approval_rule_json,updated_by) VALUES(?,?,TRUE,FALSE,'non_idempotent',?,?)")
        .bind(tenant).bind(tool).bind(&configuration["approvalRule"]).bind(actor).execute(&state.pool).await.unwrap();
    sqlx::query("INSERT INTO mcp_tool_policy_versions(tenant_id,tool_id,revision,content_hash,configuration_json,created_by) VALUES(?,?,1,?,?,?)")
        .bind(tenant).bind(tool).bind(agentx_runtime_contracts::content_hash(&configuration).unwrap().as_str()).bind(&configuration).bind(actor).execute(&state.pool).await.unwrap();

    let mut definition = template.clone();
    definition["nodes"][0]["resourceReferences"] =
        json!([{"resourceType":"mcp_tool","resourceId":tool,"operation":"use"}]);
    let path = format!(
        "/api/v1/workflows/{}/drafts/{}",
        draft["workflowId"].as_str().unwrap(),
        draft["id"].as_str().unwrap()
    );
    let body = json!({"definition":definition,"editorDocument":draft["editorDocument"]});
    let (status, validation) = request(
        app,
        "POST",
        &format!("{path}/validate"),
        Some(token),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 422,
        "protected tool must fail before the editor applies it: {validation}"
    );
    assert_eq!(validation["code"], "TOOL_APPROVAL_AGENT_REQUIRED");
    assert_eq!(
        validation["fieldErrors"][0]["field"],
        "nodes[0].resourceReferences"
    );
    let (status, saved) = request(app, "PUT", &path, Some(token), Some(json!({"expectedRevision":draft["revision"],"definition":body["definition"],"editorDocument":body["editorDocument"]}))).await;
    assert_eq!(status, 422, "Save must retain the same policy: {saved}");
    assert_eq!(saved["code"], validation["code"]);

    let mut unprotected = configuration.clone();
    unprotected["approvalRule"] = Value::Null;
    sqlx::query("INSERT INTO mcp_tool_policy_versions(tenant_id,tool_id,revision,content_hash,configuration_json,created_by) VALUES(?,?,2,?,?,?)")
        .bind(tenant).bind(tool).bind(agentx_runtime_contracts::content_hash(&unprotected).unwrap().as_str()).bind(&unprotected).bind(actor).execute(&state.pool).await.unwrap();
    sqlx::query("UPDATE mcp_tool_policies SET approval_rule_json=NULL,version=2 WHERE tenant_id=? AND tool_id=?")
        .bind(tenant).bind(tool).execute(&state.pool).await.unwrap();
    let (status, valid) = request(
        app,
        "POST",
        &format!("{path}/validate"),
        Some(token),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "the actual policy, not a blanket MCP ban, controls validation: {valid}"
    );
    assert_eq!(valid["issues"], json!([]));
    let (_, unchanged) = request(app, "GET", &path, Some(token), None).await;
    assert_eq!(unchanged["revision"], draft["revision"]);
    assert_eq!(unchanged["definition"], draft["definition"]);
    let retained: Value = sqlx::query_scalar("SELECT configuration_json FROM mcp_tool_policy_versions WHERE tenant_id=? AND tool_id=? AND revision=1")
        .bind(tenant).bind(tool).fetch_one(&state.pool).await.unwrap();
    assert_eq!(retained, configuration);
    for statement in [
        "DELETE FROM mcp_tool_policy_versions WHERE tenant_id=? AND tool_id=?",
        "DELETE FROM mcp_tool_policies WHERE tenant_id=? AND tool_id=?",
        "DELETE FROM mcp_tool_versions WHERE tenant_id=? AND tool_id=?",
    ] {
        sqlx::query(statement)
            .bind(tenant)
            .bind(tool)
            .execute(&state.pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM mcp_tools WHERE tenant_id=? AND id=?")
        .bind(tenant)
        .bind(tool)
        .execute(&state.pool)
        .await
        .unwrap();
}
