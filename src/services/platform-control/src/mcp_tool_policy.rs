//! Immutable tool policy history and its ordered Runtime admission projection.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::Actor,
};
use agentx_runtime_contracts::{
    McpToolPolicyAdmissionV1, RuntimeMcpToolPolicyV1, ToolApprovalRuleV1, content_hash,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{MySql, Transaction};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PolicyConfiguration {
    pub enabled: bool,
    pub debug_enabled: bool,
    pub timeout_seconds: u32,
    pub side_effect: String,
    pub approval_rule: Option<ToolApprovalRuleV1>,
}

pub(crate) async fn record(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    tool: Uuid,
    revision: u64,
    configuration: &PolicyConfiguration,
) -> ApiResult<RuntimeMcpToolPolicyV1> {
    if let Some(rule) = &configuration.approval_rule {
        rule.validate()
            .map_err(|message| ApiError::bad_request("MCP_APPROVAL_POLICY_INVALID", message))?;
        if configuration.debug_enabled {
            return Err(ApiError::bad_request(
                "MCP_APPROVAL_DEBUG_DENIED",
                "Disable direct debug calls before requiring tool approval",
            ));
        }
        for id in &rule.user_ids {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u WHERE u.tenant_id=? AND u.id=? AND u.status='active' AND EXISTS(SELECT 1 FROM user_roles ur JOIN roles r ON r.tenant_id=ur.tenant_id AND r.id=ur.role_id JOIN role_permissions rp ON rp.tenant_id=r.tenant_id AND rp.role_id=r.id JOIN permissions p ON p.id=rp.permission_id WHERE ur.tenant_id=u.tenant_id AND ur.user_id=u.id AND r.status='active' AND p.permission_key='approval:act'))").bind(actor.tenant_id).bind(id).fetch_one(&mut **tx).await?;
            if !exists {
                return Err(ApiError::bad_request(
                    "MCP_APPROVER_INVALID",
                    "An approval member is unavailable in this tenant",
                ));
            }
        }
        for id in &rule.role_ids {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM roles r JOIN role_permissions rp ON rp.tenant_id=r.tenant_id AND rp.role_id=r.id JOIN permissions p ON p.id=rp.permission_id WHERE r.tenant_id=? AND r.id=? AND r.status='active' AND p.permission_key='approval:act')").bind(actor.tenant_id).bind(id).fetch_one(&mut **tx).await?;
            if !exists {
                return Err(ApiError::bad_request(
                    "MCP_APPROVER_INVALID",
                    "An approval role is unavailable in this tenant",
                ));
            }
        }
    }
    let hash = content_hash(configuration).map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO mcp_tool_policy_versions(tenant_id,tool_id,revision,content_hash,configuration_json,created_by) VALUES(?,?,?,?,?,?)")
        .bind(actor.tenant_id).bind(tool).bind(revision).bind(hash.as_str()).bind(serde_json::to_value(configuration).map_err(ApiError::internal)?).bind(actor.user_id).execute(&mut **tx).await?;
    let target = McpToolPolicyAdmissionV1 {
        tenant_id: actor.tenant_id,
        tool_id: tool,
        revision,
        content_hash: hash.clone(),
        enabled: configuration.enabled,
    };
    let id = Uuid::now_v7();
    let payload = json!({"policy":target,"admissionEpoch":revision});
    sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,'McpToolPolicyChanged','mcp_policy_admission',?,?,'pending',?,?)")
        .bind(id).bind(actor.tenant_id).bind(tool.to_string()).bind(&payload).bind(content_hash(&payload).map_err(ApiError::internal)?.as_str()).bind(format!("mcp-policy:{tool}:{revision}")).execute(&mut **tx).await?;
    Ok(RuntimeMcpToolPolicyV1 {
        revision,
        content_hash: hash,
        approval: configuration.approval_rule.clone(),
    })
}
