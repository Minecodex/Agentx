use agentx_runtime_contracts::{ApplicationUserIdentityV1, MemoryScopeV1};
use serde_json::Value;
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

use crate::error::RuntimeResult;

pub(crate) async fn api_policy(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    app: Uuid,
) -> RuntimeResult<Value> {
    Ok(sqlx::query_scalar(
        "SELECT runtime_policy_json FROM application_routes WHERE tenant_id=? AND application_id=? FOR SHARE",
    )
    .bind(tenant)
    .bind(app)
    .fetch_one(&mut **tx)
    .await?)
}

pub(crate) fn from_call(
    caller_type: &str,
    caller_id: Uuid,
    input: &Value,
    policy: &Value,
) -> Option<ApplicationUserIdentityV1> {
    match caller_type {
        "user" => Some(ApplicationUserIdentityV1::platform(caller_id)),
        "api_key" => ApplicationUserIdentityV1::api(
            input,
            policy.get("apiUserIdInput").and_then(Value::as_str),
        ),
        _ => None,
    }
}

pub(crate) async fn memory_scope(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    execution: Uuid,
    node: &str,
) -> anyhow::Result<Option<MemoryScopeV1>> {
    let row = sqlx::query("SELECT e.workflow_id,e.application_id,s.execution_context_json,s.compiled_ir_json FROM workflow_executions e JOIN execution_snapshots s ON s.tenant_id=e.tenant_id AND s.execution_id=e.id WHERE e.tenant_id=? AND e.id=?")
        .bind(tenant).bind(execution).fetch_one(pool).await?;
    let context: Value = row.try_get("execution_context_json")?;
    let Some(user) = context.get("endUser").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let compiled: Value = row.try_get("compiled_ir_json")?;
    let stable_node_id = stable_node_id(&compiled, node)?;
    Ok(Some(MemoryScopeV1 {
        tenant_id: tenant,
        environment_id: context
            .get("environmentId")
            .and_then(Value::as_str)
            .map(Uuid::parse_str)
            .transpose()?,
        application_id: row.try_get("application_id")?,
        workflow_id: row.try_get("workflow_id")?,
        stable_node_id: stable_node_id.into(),
        user: serde_json::from_value(user.clone())?,
    }))
}

fn stable_node_id<'a>(compiled: &'a Value, node_key: &str) -> anyhow::Result<&'a str> {
    compiled
        .get("nodes")
        .and_then(Value::as_array)
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|node| node.get("key").and_then(Value::as_str) == Some(node_key))
        })
        .and_then(|node| node.get("id").and_then(Value::as_str))
        .ok_or_else(|| anyhow::anyhow!("MEMORY_NODE_ID_UNAVAILABLE: {node_key}"))
}

pub(crate) async fn agent_session_memory_targets(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    session_key: &str,
    node_key: &str,
) -> anyhow::Result<Vec<agentx_runtime_contracts::AgentMemoryTargetV1>> {
    let execution = sqlx::query_scalar::<_, Uuid>("SELECT a.execution_id FROM agent_session_operations o JOIN node_attempts a ON a.tenant_id=o.tenant_id AND a.id=o.attempt_id WHERE o.tenant_id=? AND o.session_key=? AND o.stable_agent_node_key=? ORDER BY o.updated_at DESC LIMIT 1")
        .bind(tenant).bind(session_key).bind(node_key).fetch_optional(pool).await?;
    let Some(execution) = execution else {
        return Ok(Vec::new());
    };
    let row = sqlx::query("SELECT runtime_settings_json,compiled_ir_json FROM execution_snapshots WHERE tenant_id=? AND execution_id=?")
        .bind(tenant).bind(execution).fetch_one(pool).await?;
    let compiled: Value = row.try_get("compiled_ir_json")?;
    let key = compiled
        .get("nodes")
        .and_then(Value::as_array)
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|node| node.get("id").and_then(Value::as_str) == Some(node_key))
        })
        .and_then(|node| node.get("key").and_then(Value::as_str))
        .ok_or_else(|| anyhow::anyhow!("MEMORY_NODE_ID_UNAVAILABLE: {node_key}"))?;
    let Some(scope) = memory_scope(pool, tenant, execution, key).await? else {
        return Ok(Vec::new());
    };
    let settings: Value = row.try_get("runtime_settings_json")?;
    let tools = settings
        .pointer("/agentBundle/agents")
        .and_then(Value::as_array)
        .and_then(|agents| {
            agents.iter().find(|agent| {
                agent.get("nodeId").and_then(Value::as_str) == Some(&scope.stable_node_id)
            })
        })
        .and_then(|agent| agent.pointer("/attachmentRegistry/tools"))
        .and_then(Value::as_array);
    let ids = tools
        .into_iter()
        .flatten()
        .filter(|tool| tool.get("origin").and_then(Value::as_str) == Some("memory"))
        .map(|tool| {
            tool.get("resourceId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("MEMORY_RESOURCE_ID_UNAVAILABLE"))?
                .parse::<Uuid>()
                .map_err(anyhow::Error::from)
        })
        .collect::<anyhow::Result<std::collections::BTreeSet<_>>>()?;
    Ok(ids
        .into_iter()
        .map(
            |memory_resource_id| agentx_runtime_contracts::AgentMemoryTargetV1 {
                scope: scope.clone(),
                memory_resource_id,
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn memory_uses_the_frozen_node_id_even_when_the_node_key_changes() {
        let compiled = json!({"nodes":[{"key":"renamed-agent", "id":"stable-agent"}]});
        assert_eq!(
            stable_node_id(&compiled, "renamed-agent").unwrap(),
            "stable-agent"
        );
        assert!(stable_node_id(&compiled, "missing-agent").is_err());
    }

    #[test]
    fn only_api_keys_read_the_declared_input_user() {
        let input = json!({"customer":"customer-42"});
        let policy = json!({"apiUserIdInput":"customer"});
        let caller = Uuid::from_u128(42);
        assert_eq!(
            from_call("api_key", caller, &input, &policy)
                .unwrap()
                .external_user_id,
            "customer-42"
        );
        assert_eq!(
            from_call("user", caller, &input, &policy),
            Some(ApplicationUserIdentityV1::platform(caller))
        );
        assert!(from_call("webhook", caller, &input, &policy).is_none());
        assert!(from_call("api_key", caller, &json!({}), &policy).is_none());
    }
}
