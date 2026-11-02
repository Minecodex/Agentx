//! Artifact input ownership follows the same caller as the original Invocation.
use crate::error::{RuntimeError, RuntimeResult};
use serde_json::Value;
use sqlx::{MySql, Row, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

pub(crate) const EXECUTION_TREE: &str = "WITH RECURSIVE task_executions AS (SELECT id FROM workflow_executions WHERE tenant_id=? AND id=? UNION ALL SELECT e.id FROM workflow_executions e JOIN task_executions t ON e.parent_execution_id=t.id WHERE e.tenant_id=?) ";

pub(crate) async fn require_input(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    caller_type: &str,
    caller_id: Uuid,
    artifact: Uuid,
) -> RuntimeResult<()> {
    // Output reuse is permitted only through another Invocation owned by this caller.
    let allowed: bool = sqlx::query_scalar("WITH RECURSIVE owned_executions AS (SELECT e.id FROM workflow_executions e JOIN application_invocations i ON i.tenant_id=e.tenant_id AND i.execution_id=e.id WHERE i.tenant_id=? AND i.caller_type=? AND i.caller_id=? UNION ALL SELECT e.id FROM workflow_executions e JOIN owned_executions p ON e.parent_execution_id=p.id WHERE e.tenant_id=?) SELECT EXISTS(SELECT 1 FROM artifacts a WHERE a.tenant_id=? AND a.id=? AND a.deleted_at IS NULL AND (a.idempotency_scope=? OR EXISTS(SELECT 1 FROM artifact_references r WHERE r.tenant_id=a.tenant_id AND r.artifact_id=a.id AND ((r.owner_type='execution' AND r.owner_id IN (SELECT BIN_TO_UUID(id) FROM owned_executions)) OR (r.owner_type='node_execution' AND r.owner_id IN (SELECT BIN_TO_UUID(n.id) FROM node_executions n JOIN owned_executions e ON e.id=n.execution_id WHERE n.tenant_id=?))))))")
        .bind(tenant).bind(caller_type).bind(caller_id).bind(tenant).bind(tenant).bind(artifact)
        .bind(format!("artifact:{caller_type}:{caller_id}")).bind(tenant).fetch_one(&mut **tx).await?;
    if !allowed {
        return Err(RuntimeError::InvalidRequest(
            "ARTIFACT_REFERENCE_INVALID",
            "The file is unavailable to this caller".into(),
        ));
    }
    Ok(())
}

pub(crate) fn collect(value: &Value, ids: &mut BTreeSet<Uuid>) {
    match value {
        Value::Object(fields) => {
            if let Some(id) = fields
                .get("artifactId")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                ids.insert(id);
            }
            for value in fields.values() {
                collect(value, ids);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect(value, ids);
            }
        }
        _ => {}
    }
}

pub(crate) async fn require_wait_content(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    root: Uuid,
    execution: Uuid,
    content: &Value,
) -> RuntimeResult<()> {
    let mut ids = BTreeSet::new();
    collect(content, &mut ids);
    if ids.is_empty() {
        return Ok(());
    }
    let row=sqlx::query("SELECT i.caller_type,i.caller_id,e.initiator_user_id FROM workflow_executions e LEFT JOIN application_invocations i ON i.tenant_id=e.tenant_id AND i.id=e.invocation_id WHERE e.tenant_id=? AND e.id=?")
        .bind(tenant).bind(root).fetch_one(&mut **tx).await?;
    let caller_type: Option<String> = row.try_get("caller_type")?;
    let (kind, caller) = if let Some(kind) = caller_type {
        (kind, row.try_get::<Uuid, _>("caller_id")?)
    } else {
        (
            "user".into(),
            row.try_get::<Option<Uuid>, _>("initiator_user_id")?
                .ok_or(RuntimeError::Unauthorized)?,
        )
    };
    for id in &ids {
        require_input(tx, tenant, &kind, caller, *id).await?;
    }
    crate::execution::reference_execution_input_artifacts(
        tx,
        tenant,
        execution,
        &ids.into_iter().collect::<Vec<_>>(),
    )
    .await
}
