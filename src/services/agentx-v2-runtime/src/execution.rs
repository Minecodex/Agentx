use agentx_runtime::ExecutionMachine;
use agentx_runtime_contracts::{ExecutionSpecPayloadV2, RuntimePublishErrorCodeV1, WorkerTaskV1};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationRequestV1 {
    pub input: Value,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvocationAcceptedV1 {
    pub invocation_id: Uuid,
    pub execution_id: Uuid,
    pub bundle_id: Uuid,
    pub admission_epoch: u64,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct RuntimeCommandClaim {
    pub command_id: Uuid,
    pub tenant_id: Uuid,
    pub command_type: String,
    pub execution_id: Uuid,
    pub payload: Value,
    pub owner_id: Uuid,
    pub fencing_token: u64,
}

pub struct ApiKeyCaller {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub key_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct InvocationCaller {
    pub caller_type: &'static str,
    pub caller_id: Uuid,
    pub token_version: Option<u64>,
    pub origin: agentx_runtime_contracts::ExecutionOriginV1,
}

pub struct DispatchClaim {
    pub id: Uuid,
    pub payload: Value,
    pub owner: Uuid,
    pub fencing_token: u64,
}

pub struct RuntimeEventClaim {
    pub id: Uuid,
    pub invocation_id: Option<Uuid>,
    pub owner: Uuid,
    pub fencing_token: u64,
}

impl DispatchClaim {
    pub fn task(&self) -> RuntimeResult<WorkerTaskV1> {
        serde_json::from_value(self.payload.clone())
            .map_err(|error| RuntimeError::Internal(error.into()))
    }
}

pub async fn authenticate_api_key(
    pool: &sqlx::MySqlPool,
    route_key: &str,
    token: &str,
) -> RuntimeResult<ApiKeyCaller> {
    if !token.starts_with("axk_") || token.chars().count() < 12 {
        return Err(RuntimeError::Unauthorized);
    }
    let prefix = token.chars().take(12).collect::<String>();
    let row = sqlx::query("SELECT k.key_id,k.tenant_id,k.application_id,k.secret_hash,r.route_key FROM api_key_admission k JOIN application_routes r ON r.application_id=k.application_id AND r.tenant_id=k.tenant_id JOIN tenant_admission t ON t.tenant_id=k.tenant_id JOIN deployment_heads h ON h.tenant_id=k.tenant_id AND h.application_id=k.application_id JOIN deployment_bundles b ON b.tenant_id=h.tenant_id AND b.id=h.bundle_id WHERE k.key_prefix=? AND k.status='active' AND (k.expires_at IS NULL OR k.expires_at>UTC_TIMESTAMP(6)) AND r.status='active' AND r.active_bundle_id=h.bundle_id AND t.status='active' AND b.status='active'")
        .bind(prefix).fetch_optional(pool).await?.ok_or(RuntimeError::Unauthorized)?;
    if row.try_get::<String, _>("route_key")? != route_key {
        return Err(RuntimeError::Unauthorized);
    }
    let expected: Vec<u8> = row.try_get("secret_hash")?;
    let actual = Sha256::digest(token.as_bytes());
    if expected.len() != actual.len()
        || expected
            .iter()
            .zip(actual.iter())
            .fold(0_u8, |value, (left, right)| value | (left ^ right))
            != 0
    {
        return Err(RuntimeError::Unauthorized);
    }
    Ok(ApiKeyCaller {
        tenant_id: row.try_get("tenant_id")?,
        application_id: row.try_get("application_id")?,
        key_id: row.try_get("key_id")?,
    })
}

pub async fn claim_dispatch(
    pool: &sqlx::MySqlPool,
    owner: Uuid,
) -> RuntimeResult<Option<DispatchClaim>> {
    let mut tx = pool.begin().await?;
    let row=sqlx::query("SELECT id,payload_json FROM execution_outbox WHERE status='pending' AND message_type='dispatch_node' AND available_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let id: Uuid = row.try_get("id")?;
    sqlx::query("UPDATE execution_outbox SET locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=? AND status='pending'").bind(owner).bind(id).execute(&mut *tx).await?;
    let token: u64 = sqlx::query_scalar("SELECT fencing_token FROM execution_outbox WHERE id=?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(DispatchClaim {
        id,
        payload: row.try_get("payload_json")?,
        owner,
        fencing_token: token,
    }))
}

pub async fn complete_dispatch(pool: &sqlx::MySqlPool, claim: &DispatchClaim) -> RuntimeResult<()> {
    let changed=sqlx::query("UPDATE execution_outbox SET status='published',published_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6) AND status='pending'").bind(claim.id).bind(claim.owner).bind(claim.fencing_token).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Outbox Lease was lost".into(),
        ));
    }
    Ok(())
}

pub async fn release_dispatch(
    pool: &sqlx::MySqlPool,
    claim: &DispatchClaim,
    error: &str,
) -> RuntimeResult<()> {
    let changed = sqlx::query("UPDATE execution_outbox SET locked_by=NULL,locked_until=NULL,available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 250000 MICROSECOND),last_error=? WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6) AND status='pending'")
        .bind(error.chars().take(1000).collect::<String>())
        .bind(claim.id)
        .bind(claim.owner)
        .bind(claim.fencing_token)
        .execute(pool)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Outbox Lease was lost".into(),
        ));
    }
    Ok(())
}

pub async fn claim_runtime_event(
    pool: &sqlx::MySqlPool,
    owner: Uuid,
) -> RuntimeResult<Option<RuntimeEventClaim>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT o.id,e.invocation_id FROM execution_outbox o JOIN workflow_executions e ON e.tenant_id=o.tenant_id AND e.id=o.execution_id WHERE o.status='pending' AND o.message_type='runtime_event' AND o.available_at<=UTC_TIMESTAMP(6) AND (o.locked_until IS NULL OR o.locked_until<=UTC_TIMESTAMP(6)) ORDER BY o.created_at,o.id LIMIT 1 FOR UPDATE SKIP LOCKED")
        .fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let id: Uuid = row.try_get("id")?;
    sqlx::query("UPDATE execution_outbox SET locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=? AND status='pending'")
        .bind(owner).bind(id).execute(&mut *tx).await?;
    let fencing_token: u64 =
        sqlx::query_scalar("SELECT fencing_token FROM execution_outbox WHERE id=?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Some(RuntimeEventClaim {
        id,
        invocation_id: row.try_get("invocation_id")?,
        owner,
        fencing_token,
    }))
}

pub async fn complete_runtime_event(
    pool: &sqlx::MySqlPool,
    claim: &RuntimeEventClaim,
) -> RuntimeResult<()> {
    let changed = sqlx::query("UPDATE execution_outbox SET status='published',published_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6) AND status='pending'")
        .bind(claim.id).bind(claim.owner).bind(claim.fencing_token).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Runtime Event Outbox Lease was lost".into(),
        ));
    }
    Ok(())
}

pub async fn recover_dispatches(
    pool: &sqlx::MySqlPool,
    limit: u32,
) -> RuntimeResult<Vec<WorkerTaskV1>> {
    timeout_expired_attempts(pool, limit).await?;
    // Read candidates without range locks. A broad UPDATE of an empty expiry
    // range can still lock live leases and the gaps used by new dispatches.
    let commands: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM runtime_commands WHERE status='processing' AND locked_until<=UTC_TIMESTAMP(6) ORDER BY locked_until,id LIMIT ?")
        .bind(limit.clamp(1, 100)).fetch_all(pool).await?;
    for id in commands {
        sqlx::query("UPDATE runtime_commands SET status='pending',locked_by=NULL,locked_until=NULL WHERE id=? AND status='processing' AND locked_until<=UTC_TIMESTAMP(6)")
            .bind(id).execute(pool).await?;
    }
    let attempts: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM node_attempts WHERE status='running' AND locked_until<=UTC_TIMESTAMP(6) ORDER BY locked_until,id LIMIT ?")
        .bind(limit.clamp(1, 100)).fetch_all(pool).await?;
    for id in attempts {
        let mut tx = pool.begin().await?;
        let tenant: Option<Uuid> = sqlx::query_scalar("SELECT tenant_id FROM node_attempts WHERE id=? AND status='running' AND locked_until<=UTC_TIMESTAMP(6) FOR UPDATE SKIP LOCKED")
            .bind(id).fetch_optional(&mut *tx).await?;
        if let Some(tenant) = tenant {
            sqlx::query("UPDATE worker_leases SET released_at=COALESCE(released_at,UTC_TIMESTAMP(6)) WHERE tenant_id=? AND node_attempt_id=? AND released_at IS NULL")
                .bind(tenant).bind(id).execute(&mut *tx).await?;
            sqlx::query("UPDATE node_attempts SET status='queued',lease_token=NULL,worker_instance_id=NULL,locked_until=NULL,heartbeat_at=NULL WHERE id=? AND status='running' AND locked_until<=UTC_TIMESTAMP(6)")
                .bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }
    let reservations: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM quota_reservations WHERE status='active' AND expires_at<=UTC_TIMESTAMP(6) ORDER BY expires_at,id LIMIT ?")
        .bind(limit.clamp(1, 100)).fetch_all(pool).await?;
    for id in reservations {
        sqlx::query("UPDATE quota_reservations SET status='expired',release_reason='reservation_expired',settled_at=UTC_TIMESTAMP(6) WHERE id=? AND status='active' AND expires_at<=UTC_TIMESTAMP(6)")
            .bind(id).execute(pool).await?;
    }
    // Historical published rows are metadata candidates, not a batch of
    // payloads to sort and lock. Recheck each selected outbox/attempt by key
    // so concurrent recovery or a Worker claim cannot cause stale dispatch.
    let outboxes: Vec<Uuid> = sqlx::query_scalar("SELECT o.id FROM execution_outbox o FORCE INDEX(idx_execution_outbox_recovery) JOIN node_attempts a ON a.id=o.attempt_id AND a.tenant_id=o.tenant_id WHERE o.message_type='dispatch_node' AND o.status='published' AND a.status='queued' AND o.published_at<=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 5 SECOND) ORDER BY o.published_at,o.id LIMIT ?")
        .bind(limit.clamp(1, 100)).fetch_all(pool).await?;
    let mut messages = Vec::with_capacity(outboxes.len());
    for outbox_id in outboxes {
        let mut tx = pool.begin().await?;
        let payload: Option<Value> = sqlx::query_scalar("SELECT o.payload_json FROM execution_outbox o JOIN node_attempts a ON a.id=o.attempt_id AND a.tenant_id=o.tenant_id WHERE o.id=? AND o.message_type='dispatch_node' AND o.status='published' AND a.status='queued' AND o.published_at<=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 5 SECOND) FOR UPDATE SKIP LOCKED")
            .bind(outbox_id).fetch_optional(&mut *tx).await?;
        let Some(payload) = payload else {
            tx.rollback().await?;
            continue;
        };
        sqlx::query("UPDATE execution_outbox SET published_at=UTC_TIMESTAMP(6) WHERE id=? AND status='published'")
            .bind(outbox_id).execute(&mut *tx).await?;
        let message = serde_json::from_value(payload)
            .map_err(|error| RuntimeError::Internal(error.into()))?;
        tx.commit().await?;
        messages.push(message);
    }
    Ok(messages)
}

async fn timeout_expired_attempts(pool: &sqlx::MySqlPool, limit: u32) -> RuntimeResult<()> {
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT e.id FROM workflow_executions e WHERE e.status IN ('queued','running','waiting','suspended') AND EXISTS(SELECT 1 FROM node_attempts a WHERE a.tenant_id=e.tenant_id AND a.execution_id=e.id AND a.status IN ('queued','running') AND a.deadline_at IS NOT NULL AND a.deadline_at<=UTC_TIMESTAMP(6)) ORDER BY e.started_at,e.id LIMIT ?",
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    for execution_id in candidates {
        let mut tx = pool.begin().await?;
        let row = sqlx::query("SELECT e.tenant_id,e.id execution_id,e.work_package_id,e.invocation_id,r.state_version,r.context_json,r.machine_state_json FROM workflow_executions e JOIN execution_runtime_state r ON r.tenant_id=e.tenant_id AND r.execution_id=e.id WHERE e.id=? AND e.status IN ('queued','running','waiting','suspended') AND EXISTS(SELECT 1 FROM node_attempts a WHERE a.tenant_id=e.tenant_id AND a.execution_id=e.id AND a.status IN ('queued','running') AND a.deadline_at IS NOT NULL AND a.deadline_at<=UTC_TIMESTAMP(6)) FOR UPDATE SKIP LOCKED")
            .bind(execution_id).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.commit().await?;
            continue;
        };
        let tenant_id: Uuid = row.try_get("tenant_id")?;
        let execution_id: Uuid = row.try_get("execution_id")?;
        let state_version: u64 = row.try_get("state_version")?;
        let context: Value = row.try_get("context_json")?;
        let mut machine: ExecutionMachine =
            serde_json::from_value(row.try_get("machine_state_json")?)
                .map_err(|error| RuntimeError::Internal(error.into()))?;
        machine.timeout();
        sqlx::query("UPDATE node_attempts SET status='timed_out',error_code='NODE_EXECUTION_TIMED_OUT',error_message='Node execution exceeded its operation deadline',locked_until=NULL,heartbeat_at=NULL,ended_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND execution_id=? AND status IN ('queued','running')")
            .bind(tenant_id).bind(execution_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE node_executions SET status='timed_out',error_code='NODE_EXECUTION_TIMED_OUT',error_message='Node execution exceeded its operation deadline',ended_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND execution_id=? AND status IN ('ready','queued','running','waiting')")
            .bind(tenant_id).bind(execution_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE worker_leases SET released_at=COALESCE(released_at,UTC_TIMESTAMP(6)) WHERE tenant_id=? AND node_attempt_id IN (SELECT id FROM node_attempts WHERE tenant_id=? AND execution_id=?)")
            .bind(tenant_id).bind(tenant_id).bind(execution_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE execution_runtime_state SET state_version=?,machine_state_json=? WHERE tenant_id=? AND execution_id=?")
            .bind(state_version + 1)
            .bind(serde_json::to_value(&machine).map_err(|error| RuntimeError::Internal(error.into()))?)
            .bind(tenant_id).bind(execution_id).execute(&mut *tx).await?;
        crate::engine_persistence::finish_execution(
            &mut tx,
            tenant_id,
            execution_id,
            row.try_get("work_package_id")?,
            row.try_get("invocation_id")?,
            state_version + 1,
            &machine,
            &context,
        )
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

pub async fn create_invocation(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    application_id: Uuid,
    caller_id: Uuid,
    request: &InvocationRequestV1,
) -> RuntimeResult<InvocationAcceptedV1> {
    let key_name: String = sqlx::query_scalar(
        "SELECT key_name FROM api_key_admission WHERE tenant_id=? AND application_id=? AND key_id=?",
    )
    .bind(tenant_id)
    .bind(application_id)
    .bind(caller_id)
    .fetch_optional(pool)
    .await?
    .ok_or(RuntimeError::Unauthorized)?;
    create_runtime_invocation(
        pool,
        tenant_id,
        application_id,
        InvocationCaller {
            caller_type: "api_key",
            caller_id,
            token_version: None,
            origin: agentx_runtime_contracts::ExecutionOriginV1 {
                trigger_source_id: Some(caller_id),
                trigger_name: Some(key_name),
                ..agentx_runtime_contracts::ExecutionOriginV1::system(None)
            },
        },
        None,
        &request.input,
        &request.idempotency_key,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_runtime_invocation(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    application_id: Uuid,
    caller: InvocationCaller,
    session_id: Option<Uuid>,
    input: &Value,
    idempotency_key: &str,
) -> RuntimeResult<InvocationAcceptedV1> {
    let mut tx = pool.begin().await?;
    let accepted = create_runtime_invocation_tx(
        &mut tx,
        tenant_id,
        application_id,
        caller,
        session_id,
        input,
        idempotency_key,
    )
    .await?;
    tx.commit().await?;
    Ok(accepted)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_runtime_invocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant_id: Uuid,
    application_id: Uuid,
    caller: InvocationCaller,
    session_id: Option<Uuid>,
    input: &Value,
    idempotency_key: &str,
) -> RuntimeResult<InvocationAcceptedV1> {
    create_runtime_invocation_tx_inner(
        tx,
        tenant_id,
        application_id,
        caller,
        session_id,
        input,
        idempotency_key,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_chat_runtime_invocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant_id: Uuid,
    application_id: Uuid,
    caller: InvocationCaller,
    session_id: Uuid,
    input: &Value,
    idempotency_key: &str,
) -> RuntimeResult<InvocationAcceptedV1> {
    create_runtime_invocation_tx_inner(
        tx,
        tenant_id,
        application_id,
        caller,
        Some(session_id),
        input,
        idempotency_key,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_runtime_invocation_tx_inner(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant_id: Uuid,
    application_id: Uuid,
    caller: InvocationCaller,
    session_id: Option<Uuid>,
    input: &Value,
    idempotency_key: &str,
    chat_message: bool,
) -> RuntimeResult<InvocationAcceptedV1> {
    let request_hash = format!(
        "{:x}",
        Sha256::digest(
            agentx_runtime_contracts::canonical_bytes(&json!({
                "input": input,
                "sessionId": session_id,
                "callerType": caller.caller_type,
                "callerId": caller.caller_id,
            }))
            .map_err(|error| RuntimeError::Internal(error.into()))?
        )
    );
    if let Some(row) = sqlx::query("SELECT id,execution_id,bundle_id,admission_epoch,status,request_hash FROM application_invocations WHERE tenant_id=? AND application_id=? AND caller_type=? AND caller_id=? AND idempotency_key=?")
        .bind(tenant_id).bind(application_id).bind(caller.caller_type).bind(caller.caller_id).bind(idempotency_key).fetch_optional(&mut **tx).await?
    {
        if row.try_get::<String, _>("request_hash")? != request_hash {
            return Err(RuntimeError::Conflict(RuntimePublishErrorCodeV1::IdempotencyConflict, "Invocation idempotency key was reused with different input".into()));
        }
        return Ok(InvocationAcceptedV1 { invocation_id: row.try_get("id")?, execution_id: row.try_get("execution_id")?, bundle_id: row.try_get("bundle_id")?, admission_epoch: row.try_get("admission_epoch")?, status: row.try_get("status")? });
    }
    let row = sqlx::query("SELECT h.bundle_id,h.admission_epoch,b.payload_json FROM deployment_heads h JOIN deployment_bundles b ON b.id=h.bundle_id JOIN application_routes r ON r.application_id=h.application_id AND r.tenant_id=h.tenant_id JOIN tenant_admission t ON t.tenant_id=h.tenant_id WHERE h.tenant_id=? AND h.application_id=? AND b.status='active' AND r.status='active' AND r.active_bundle_id=h.bundle_id AND t.status='active' FOR SHARE")
        .bind(tenant_id).bind(application_id).fetch_optional(&mut **tx).await?.ok_or(RuntimeError::NotFound)?;
    let bundle_id: Uuid = row.try_get("bundle_id")?;
    let head_epoch: u64 = row.try_get("admission_epoch")?;
    let caller_epoch: u64 = match caller.caller_type {
        "api_key" => sqlx::query_scalar("SELECT admission_epoch FROM api_key_admission WHERE key_id=? AND tenant_id=? AND application_id=? AND status='active' AND (expires_at IS NULL OR expires_at>UTC_TIMESTAMP(6))")
            .bind(caller.caller_id).bind(tenant_id).bind(application_id).fetch_optional(&mut **tx).await?.ok_or(RuntimeError::Unauthorized)?,
        "user" => sqlx::query_scalar("SELECT GREATEST(u.admission_epoch,g.admission_epoch) FROM runtime_user_admission u JOIN runtime_user_application_grants g ON g.tenant_id=u.tenant_id AND g.user_id=u.user_id WHERE u.tenant_id=? AND u.user_id=? AND u.status='active' AND u.token_version=? AND g.application_id=? AND g.status='active' AND g.can_invoke=TRUE")
            .bind(tenant_id).bind(caller.caller_id).bind(caller.token_version.ok_or(RuntimeError::Unauthorized)?).bind(application_id).fetch_optional(&mut **tx).await?.ok_or(RuntimeError::Unauthorized)?,
        _ => head_epoch,
    };
    let origin = if caller.caller_type == "user" {
        crate::execution_context::runtime_user_origin(tx, tenant_id, caller.caller_id).await?
    } else {
        caller.origin.clone()
    };
    let mut selected_bundle_id = bundle_id;
    let mut session_external_user_id = None;
    if let Some(session_id) = session_id {
        let session = sqlx::query("SELECT application_id,bundle_id,version_policy,status,external_user_id FROM application_sessions WHERE tenant_id=? AND id=? FOR UPDATE")
            .bind(tenant_id).bind(session_id).fetch_optional(&mut **tx).await?.ok_or(RuntimeError::NotFound)?;
        if session.try_get::<Uuid, _>("application_id")? != application_id
            || session.try_get::<String, _>("status")? != "active"
        {
            return Err(RuntimeError::Unauthorized);
        }
        session_external_user_id = session.try_get("external_user_id")?;
        if session.try_get::<String, _>("version_policy")? != "follow_deployment" {
            selected_bundle_id = session
                .try_get::<Option<Uuid>, _>("bundle_id")?
                .ok_or(RuntimeError::NotFound)?;
        }
    }
    let admission_epoch = head_epoch.max(caller_epoch);
    let payload: Value = if selected_bundle_id == bundle_id {
        row.try_get("payload_json")?
    } else {
        sqlx::query_scalar("SELECT payload_json FROM deployment_bundles WHERE tenant_id=? AND id=? AND status IN ('active','superseded','retained')")
            .bind(tenant_id).bind(selected_bundle_id).fetch_optional(&mut **tx).await?.ok_or(RuntimeError::NotFound)?
    };
    let spec: ExecutionSpecPayloadV2 = serde_json::from_value(payload.clone())
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let (mut input, chat_mapping_version, chat_mapping_json) = if chat_message {
        let row = sqlx::query("SELECT version,mapping_json FROM application_chat_mappings WHERE tenant_id=? AND application_id=? AND bundle_id=? FOR SHARE")
            .bind(tenant_id)
            .bind(application_id)
            .bind(selected_bundle_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| RuntimeError::InvalidRequest("CHAT_MAPPING_NOT_CONFIGURED", "Conversation testing is not configured for this Deployment".into()))?;
        let version: u64 = row.try_get("version")?;
        let mapping_value: Value = row.try_get("mapping_json")?;
        let mapping = serde_json::from_value::<Option<agentx_runtime_contracts::ChatMappingV1>>(
            mapping_value.clone(),
        )
        .map_err(|error| RuntimeError::Internal(error.into()))?
        .ok_or_else(|| {
            RuntimeError::InvalidRequest(
                "CHAT_MAPPING_NOT_CONFIGURED",
                "Conversation testing is not configured for this Deployment".into(),
            )
        })?;
        validate_runtime_chat_mapping(&mapping, &spec.input_contract, &spec.output_contract)?;
        (
            project_chat_message_input(input, &mapping, &spec.input_contract)?,
            Some(version),
            Some(
                serde_json::to_value(&mapping)
                    .map_err(|error| RuntimeError::Internal(error.into()))?,
            ),
        )
    } else {
        (input.clone(), None, None)
    };
    agentx_runtime::materialize_and_validate_start_input(&mut input, &spec.input_contract)
        .map_err(|error| {
            RuntimeError::InvalidRequest("INPUT_SCHEMA_VALIDATION_FAILED", error.to_string())
        })?;
    let input_artifact_ids =
        validate_invocation_artifacts(tx, tenant_id, &input, &spec.input_contract).await?;
    let invocation_id = Uuid::now_v7();
    let execution_id = Uuid::now_v7();
    let started_at = OffsetDateTime::now_utc();
    let command_id = Uuid::now_v7();
    let trace_id = Uuid::now_v7();
    let state_hash =
        agentx_runtime_contracts::content_hash(&json!({"status":"queued","input":input}))
            .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query("INSERT INTO application_invocations(id,tenant_id,application_id,session_id,workflow_version_id,bundle_id,admission_epoch,state_version,execution_id,caller_type,caller_id,caller_token_version,chat_mapping_version,chat_mapping_json,request_hash,idempotency_key,status,input_json) VALUES(?,?,?,?,?,?,?,1,?,?,?,?,?,?,?,?,'queued',?) ON DUPLICATE KEY UPDATE id=id")
        .bind(invocation_id).bind(tenant_id).bind(application_id).bind(session_id).bind(spec.workflow_version_id).bind(selected_bundle_id).bind(admission_epoch).bind(execution_id).bind(caller.caller_type).bind(caller.caller_id).bind(caller.token_version).bind(chat_mapping_version).bind(&chat_mapping_json).bind(&request_hash).bind(idempotency_key).bind(&input).execute(&mut **tx).await?;
    let stored = sqlx::query("SELECT id,execution_id,bundle_id,admission_epoch,status,request_hash FROM application_invocations WHERE tenant_id=? AND application_id=? AND caller_type=? AND caller_id=? AND idempotency_key=? FOR UPDATE")
        .bind(tenant_id).bind(application_id).bind(caller.caller_type).bind(caller.caller_id).bind(idempotency_key).fetch_one(&mut **tx).await?;
    let stored_id: Uuid = stored.try_get("id")?;
    if stored_id != invocation_id {
        if stored.try_get::<String, _>("request_hash")? != request_hash {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Invocation idempotency key was reused with different input".into(),
            ));
        }
        return Ok(InvocationAcceptedV1 {
            invocation_id: stored_id,
            execution_id: stored.try_get("execution_id")?,
            bundle_id: stored.try_get("bundle_id")?,
            admission_epoch: stored.try_get("admission_epoch")?,
            status: stored.try_get("status")?,
        });
    }
    sqlx::query("INSERT INTO workflow_executions(id,tenant_id,workflow_id,workflow_version_id,application_id,session_id,bundle_id,admission_epoch,state_version,invocation_id,trace_id,trigger_type,initiator_user_id,initiator_user_name,initiator_department_id,initiator_department_name,trigger_source_id,trigger_name,status,started_at,input_json) VALUES(?,?,?,?,?,?,?,?,1,?,?,?,?,?,?,?,?,?,'queued',?,?)")
        .bind(execution_id).bind(tenant_id).bind(spec.workflow_id).bind(spec.workflow_version_id).bind(application_id).bind(session_id).bind(selected_bundle_id).bind(admission_epoch).bind(invocation_id).bind(trace_id).bind(caller.caller_type)
        .bind(origin.initiator_user_id).bind(&origin.initiator_user_name).bind(origin.initiator_department_id).bind(&origin.initiator_department_name).bind(origin.trigger_source_id).bind(&origin.trigger_name)
        .bind(started_at)
        .bind(&input).execute(&mut **tx).await?;
    for artifact_id in input_artifact_ids {
        sqlx::query("INSERT IGNORE INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'execution',?,'input')")
            .bind(tenant_id)
            .bind(artifact_id)
            .bind(execution_id.to_string())
            .execute(&mut **tx)
            .await?;
    }
    let compiled_ir = serde_json::to_value(&spec.compiled_ir)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let resource_snapshot = serde_json::to_value(&spec.resources)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let authorization_snapshot = serde_json::to_value(&spec.authorization)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let policy_snapshot = serde_json::to_value(&spec.runtime_policy)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let worker_compatibility = serde_json::to_value(&spec.worker_compatibility)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let object_manifest = serde_json::to_value(&spec.objects)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let execution_context = serde_json::to_value(crate::execution_context::create_snapshot(
        crate::execution_context::NewExecutionContext {
            execution_id,
            started_at,
            parent_execution_id: None,
            workflow: agentx_runtime_contracts::ExecutionWorkflowSnapshotV1 {
                id: spec.workflow_id,
                name: spec.workflow_name.clone(),
                version_id: spec.workflow_version_id,
                version_number: spec.workflow_version_number,
                owner_department: spec.workflow_owner_department.clone(),
            },
            trigger_type: caller.caller_type,
            origin: &origin,
            application_id: Some(application_id),
            invocation_id: Some(invocation_id),
            session: session_id.map(|id| (id, session_external_user_id)),
        },
    ))
    .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query(
        "INSERT INTO execution_snapshots(execution_id,tenant_id,workflow_version_id,bundle_id,admission_epoch,state_version,definition_json,compiled_ir_json,compiled_ir_hash,compiler_version,resource_snapshot_json,authorization_snapshot_json,policy_snapshot_json,execution_context_json,worker_compatibility_json,object_manifest_json,runtime_settings_json,state_hash) VALUES(?,?,?,?,?,1,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(execution_id)
    .bind(tenant_id)
    .bind(spec.workflow_version_id)
    .bind(selected_bundle_id)
    .bind(admission_epoch)
    .bind(&spec.definition)
    .bind(compiled_ir)
    .bind(&spec.compiled_ir.canonical_hash)
    .bind(&spec.compiled_ir.compiler_version)
    .bind(resource_snapshot)
    .bind(authorization_snapshot)
    .bind(&policy_snapshot)
    .bind(execution_context)
    .bind(worker_compatibility)
    .bind(object_manifest)
    .bind(json!({
        "runtimePolicy": &spec.runtime_policy,
        "agentBundle": &spec.agent_bundle,
    }))
    .bind(state_hash.as_str())
    .execute(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'active_execution',?)")
        .bind(Uuid::now_v7()).bind(tenant_id).bind(selected_bundle_id).bind(execution_id).execute(&mut **tx).await?;
    let command_payload = json!({"invocationId":invocation_id,"executionId":execution_id,"bundleId":selected_bundle_id,"admissionEpoch":admission_epoch,"input":input});
    sqlx::query("INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'start_execution','execution',?,?,?,'pending')")
        .bind(command_id).bind(tenant_id).bind(execution_id.to_string()).bind(format!("execution:start:{execution_id}")).bind(&command_payload).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,capability,payload_json,status) VALUES(?,?,?,'runtime_event',NULL,?,'pending')")
        .bind(Uuid::now_v7()).bind(tenant_id).bind(execution_id).bind(json!({"type":"invocation.accepted","commandId":command_id})).execute(&mut **tx).await?;
    let mut trace = crate::trace_delivery::TraceDraft::execution(
        tenant_id,
        execution_id,
        "execution.accepted",
        "queued",
    );
    trace.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::WorkflowInput);
    trace.content_preview = crate::trace_delivery::bounded_preview(&input);
    crate::trace_delivery::enqueue_best_effort(tx, trace).await;
    let mut start = crate::trace_delivery::TraceDraft::span(
        tenant_id,
        execution_id,
        crate::trace_delivery::boundary_entity_id(execution_id, "start"),
        Some((
            execution_id,
            agentx_runtime_contracts::TraceSpanKindV1::Execution,
        )),
        agentx_runtime_contracts::TraceSpanKindV1::Boundary,
        "Start",
        agentx_runtime_contracts::TraceEventKindV1::Finished,
        "boundary.start.finished",
        "succeeded",
    );
    start.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::WorkflowInput);
    start.content_preview = crate::trace_delivery::bounded_preview(&input);
    crate::trace_delivery::enqueue_best_effort(tx, start).await;
    sqlx::query("INSERT INTO invocation_events(tenant_id,invocation_id,event_id,sequence_number,event_type,payload_json) VALUES(?,?,?,1,'invocation.accepted',?)")
        .bind(tenant_id).bind(invocation_id).bind(Uuid::now_v7()).bind(json!({"executionId":execution_id,"bundleId":selected_bundle_id,"admissionEpoch":admission_epoch,"status":"queued"})).execute(&mut **tx).await?;
    Ok(InvocationAcceptedV1 {
        invocation_id,
        execution_id,
        bundle_id: selected_bundle_id,
        admission_epoch,
        status: "queued".into(),
    })
}

fn project_chat_message_input(
    input: &Value,
    mapping: &agentx_runtime_contracts::ChatMappingV1,
    input_contract: &Value,
) -> RuntimeResult<Value> {
    let question = input
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let files = input
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut projected = serde_json::Map::from_iter([(
        mapping.question_input.clone(),
        Value::String(question.to_owned()),
    )]);
    // plan7 P7-B B5: inputs marked x-agentx-modality receive the artifact
    // references of the matching message parts natively, so the model request
    // builder can lift them into multimodal content parts.
    if let Some(properties) = input_contract.get("properties").and_then(Value::as_object) {
        for (field, schema) in properties {
            let Some(modality) = schema.get("x-agentx-modality").and_then(Value::as_str) else {
                continue;
            };
            let selected: Vec<Value> = files
                .iter()
                .filter(|file| {
                    let part_type = file.get("type").and_then(Value::as_str).unwrap_or_default();
                    match modality {
                        "image" => part_type == "image",
                        "audio" => part_type == "audio",
                        _ => false,
                    }
                })
                .cloned()
                .collect();
            if !selected.is_empty() {
                projected.insert(field.clone(), Value::Array(selected));
            }
        }
    }
    if files.is_empty() {
        return Ok(Value::Object(projected));
    }
    let file_field = mapping.file_input.as_ref().ok_or_else(|| {
        RuntimeError::InvalidRequest(
            "CHAT_FILE_MAPPING_NOT_CONFIGURED",
            "This Chat Mapping does not accept files".into(),
        )
    })?;
    let schema = input_contract
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(file_field))
        .ok_or_else(|| {
            RuntimeError::InvalidRequest(
                "CHAT_MAPPING_FIELD_NOT_FOUND",
                "Mapped file input does not exist".into(),
            )
        })?;
    let value = if schema
        .get("x-agentx-artifact-array")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Value::Array(files)
    } else {
        if files.len() != 1 {
            return Err(RuntimeError::InvalidRequest(
                "CHAT_FILE_CARDINALITY_INVALID",
                "The mapped input accepts exactly one file".into(),
            ));
        }
        files.into_iter().next().unwrap_or(Value::Null)
    };
    projected.insert(file_field.clone(), value);
    Ok(Value::Object(projected))
}

fn validate_runtime_chat_mapping(
    mapping: &agentx_runtime_contracts::ChatMappingV1,
    input_contract: &Value,
    output_contract: &Value,
) -> RuntimeResult<()> {
    let inputs = input_contract
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            RuntimeError::InvalidRequest(
                "CHAT_INPUT_SCHEMA_INCOMPATIBLE",
                "Input Schema is not an object".into(),
            )
        })?;
    let outputs = output_contract
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            RuntimeError::InvalidRequest(
                "CHAT_OUTPUT_SCHEMA_INCOMPATIBLE",
                "Output Schema is not an object".into(),
            )
        })?;
    let question = inputs.get(&mapping.question_input).ok_or_else(|| {
        RuntimeError::InvalidRequest(
            "CHAT_MAPPING_FIELD_NOT_FOUND",
            "Mapped question input does not exist".into(),
        )
    })?;
    if question.get("type").and_then(Value::as_str) != Some("string")
        || question
            .get("x-agentx-artifact")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Err(RuntimeError::InvalidRequest(
            "CHAT_MAPPING_TYPE_MISMATCH",
            "Mapped question input must be a string".into(),
        ));
    }
    if let Some(name) = &mapping.file_input {
        let schema = inputs.get(name).ok_or_else(|| {
            RuntimeError::InvalidRequest(
                "CHAT_MAPPING_FIELD_NOT_FOUND",
                "Mapped file input does not exist".into(),
            )
        })?;
        if !schema
            .get("x-agentx-artifact")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(RuntimeError::InvalidRequest(
                "CHAT_MAPPING_TYPE_MISMATCH",
                "Mapped file input must be an Artifact field".into(),
            ));
        }
    }
    let answer = outputs.get(&mapping.answer_output).ok_or_else(|| {
        RuntimeError::InvalidRequest(
            "CHAT_MAPPING_FIELD_NOT_FOUND",
            "Mapped answer output does not exist".into(),
        )
    })?;
    if answer.get("type").and_then(Value::as_str) != Some("string")
        || answer
            .get("x-agentx-sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Err(RuntimeError::InvalidRequest(
            "CHAT_MAPPING_TYPE_MISMATCH",
            "Mapped answer output must be a non-sensitive string".into(),
        ));
    }
    if let Some(name) = &mapping.answer_files_output {
        let schema = outputs.get(name).ok_or_else(|| {
            RuntimeError::InvalidRequest(
                "CHAT_MAPPING_FIELD_NOT_FOUND",
                "Mapped answer file output does not exist".into(),
            )
        })?;
        if !schema
            .get("x-agentx-artifact")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(RuntimeError::InvalidRequest(
                "CHAT_MAPPING_TYPE_MISMATCH",
                "Mapped answer file output must be an Artifact field".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ArtifactInputConstraint {
    path: String,
    artifact_ids: Vec<Uuid>,
    content_types: Vec<String>,
    max_size_bytes: Option<u64>,
    max_total_size_bytes: Option<u64>,
}

async fn validate_invocation_artifacts(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant_id: Uuid,
    input: &Value,
    schema: &Value,
) -> RuntimeResult<Vec<Uuid>> {
    let mut constraints = Vec::new();
    collect_artifact_constraints(input, schema, "input", &mut constraints)?;
    let mut referenced = Vec::new();
    for constraint in constraints {
        let mut total_size = 0_u64;
        for artifact_id in constraint.artifact_ids {
            let row = sqlx::query(
                "SELECT content_type,size_bytes FROM artifacts WHERE tenant_id=? AND id=? AND deleted_at IS NULL",
            )
            .bind(tenant_id)
            .bind(artifact_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| {
                RuntimeError::InvalidRequest(
                    "ARTIFACT_REFERENCE_INVALID",
                    format!("{} references an unavailable Artifact", constraint.path),
                )
            })?;
            let content_type: String = row.try_get("content_type")?;
            let size_bytes: u64 = row.try_get("size_bytes")?;
            if !constraint.content_types.is_empty()
                && !constraint
                    .content_types
                    .iter()
                    .any(|allowed| content_type_matches(allowed, &content_type))
            {
                return Err(RuntimeError::InvalidRequest(
                    "ARTIFACT_CONTENT_TYPE_NOT_ALLOWED",
                    format!(
                        "{} does not allow content type {content_type}",
                        constraint.path
                    ),
                ));
            }
            if constraint
                .max_size_bytes
                .is_some_and(|maximum| size_bytes > maximum)
            {
                return Err(RuntimeError::InvalidRequest(
                    "ARTIFACT_TOO_LARGE",
                    format!(
                        "{} contains an Artifact larger than its configured limit",
                        constraint.path
                    ),
                ));
            }
            total_size = total_size.saturating_add(size_bytes);
            referenced.push(artifact_id);
        }
        if constraint
            .max_total_size_bytes
            .is_some_and(|maximum| total_size > maximum)
        {
            return Err(RuntimeError::InvalidRequest(
                "ARTIFACT_TOTAL_SIZE_EXCEEDED",
                format!(
                    "{} exceeds its configured total size limit",
                    constraint.path
                ),
            ));
        }
    }
    referenced.sort_unstable();
    referenced.dedup();
    Ok(referenced)
}

fn collect_artifact_constraints(
    value: &Value,
    schema: &Value,
    path: &str,
    constraints: &mut Vec<ArtifactInputConstraint>,
) -> RuntimeResult<()> {
    if schema
        .get("x-agentx-artifact")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let references = if schema
            .get("x-agentx-artifact-array")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            value.as_array().cloned().unwrap_or_default()
        } else {
            vec![value.clone()]
        };
        let artifact_ids = references
            .iter()
            .map(|reference| {
                reference
                    .get("artifactId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        RuntimeError::InvalidRequest(
                            "ARTIFACT_REFERENCE_INVALID",
                            format!("{path} requires an artifactId"),
                        )
                    })?
                    .parse::<Uuid>()
                    .map_err(|_| {
                        RuntimeError::InvalidRequest(
                            "ARTIFACT_REFERENCE_INVALID",
                            format!("{path} contains an invalid artifactId"),
                        )
                    })
            })
            .collect::<RuntimeResult<Vec<_>>>()?;
        constraints.push(ArtifactInputConstraint {
            path: path.to_owned(),
            artifact_ids,
            content_types: schema
                .get("x-agentx-content-types")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            max_size_bytes: schema
                .get("x-agentx-max-size-bytes")
                .and_then(Value::as_u64),
            max_total_size_bytes: schema
                .get("x-agentx-max-total-size-bytes")
                .and_then(Value::as_u64),
        });
        return Ok(());
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        for (name, property_schema) in properties {
            if let Some(property_value) = object.get(name) {
                collect_artifact_constraints(
                    property_value,
                    property_schema,
                    &format!("{path}.{name}"),
                    constraints,
                )?;
            }
        }
    }
    if let (Some(item_schema), Some(items)) = (schema.get("items"), value.as_array()) {
        for (index, item) in items.iter().enumerate() {
            collect_artifact_constraints(
                item,
                item_schema,
                &format!("{path}[{index}]"),
                constraints,
            )?;
        }
    }
    Ok(())
}

fn content_type_matches(allowed: &str, actual: &str) -> bool {
    allowed == actual
        || allowed
            .strip_suffix("/*")
            .is_some_and(|prefix| actual.starts_with(&format!("{prefix}/")))
}

pub async fn claim_commands(
    pool: &sqlx::MySqlPool,
    owner_id: Uuid,
    limit: u32,
) -> RuntimeResult<Vec<RuntimeCommandClaim>> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query("SELECT id FROM runtime_commands WHERE status='pending' AND available_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY created_at,id LIMIT ? FOR UPDATE SKIP LOCKED")
        .bind(limit.clamp(1,100)).fetch_all(&mut *tx).await?;
    let mut claims = Vec::with_capacity(rows.len());
    for row in &rows {
        let command_id = row.try_get::<Uuid, _>("id")?;
        sqlx::query("UPDATE runtime_commands SET status='processing',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=? AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6))")
            .bind(owner_id).bind(command_id).execute(&mut *tx).await?;
        let claimed = sqlx::query("SELECT id,tenant_id,command_type,aggregate_id,payload_json,fencing_token FROM runtime_commands WHERE id=? AND locked_by=? AND status='processing' AND locked_until>UTC_TIMESTAMP(6)")
            .bind(command_id).bind(owner_id).fetch_one(&mut *tx).await?;
        claims.push(RuntimeCommandClaim {
            command_id: claimed.try_get("id")?,
            tenant_id: claimed.try_get("tenant_id")?,
            command_type: claimed.try_get("command_type")?,
            execution_id: Uuid::parse_str(&claimed.try_get::<String, _>("aggregate_id")?)
                .map_err(|error| RuntimeError::Internal(error.into()))?,
            payload: claimed.try_get("payload_json")?,
            owner_id,
            fencing_token: claimed.try_get("fencing_token")?,
        });
    }
    tx.commit().await?;
    Ok(claims)
}

pub async fn process_command(
    pool: &sqlx::MySqlPool,
    claim: &RuntimeCommandClaim,
) -> RuntimeResult<()> {
    match claim.command_type.as_str() {
        "start_execution" => crate::engine::start_execution(pool, claim).await,
        "cancel_execution" => process_cancel_command(pool, claim).await,
        "resume_execution" => crate::engine::resume_execution(pool, claim).await,
        "fork_execution" => crate::engine::fork_execution(pool, claim).await,
        "confirm_side_effect" => crate::engine::confirm_side_effect(pool, claim).await,
        command => Err(RuntimeError::BadRequest(
            RuntimePublishErrorCodeV1::UnsupportedCapability,
            format!("unsupported Runtime command {command}"),
        )),
    }
}

pub async fn process_command_with_state(
    state: &crate::RuntimeState,
    claim: &RuntimeCommandClaim,
) -> RuntimeResult<()> {
    match claim.command_type.as_str() {
        "start_execution" => match crate::engine::start_execution_with_state(state, claim).await {
            Err(RuntimeError::BadRequest(_, message)) => {
                let (code, detail) = deterministic_rejection(&message);
                reject_start_execution(&state.pool, claim, code, detail).await
            }
            result => result,
        },
        _ => process_command(&state.pool, claim).await,
    }
}

fn deterministic_rejection(message: &str) -> (&str, &str) {
    let Some((code, detail)) = message.split_once(": ") else {
        return ("RUNTIME_START_REJECTED", message);
    };
    if !code.is_empty()
        && code
            .bytes()
            .all(|value| value.is_ascii_uppercase() || value.is_ascii_digit() || value == b'_')
    {
        (code, detail)
    } else {
        ("RUNTIME_START_REJECTED", message)
    }
}

async fn reject_start_execution(
    pool: &sqlx::MySqlPool,
    claim: &RuntimeCommandClaim,
    error_code: &str,
    error_message: &str,
) -> RuntimeResult<()> {
    let mut tx = pool.begin().await?;
    let command = sqlx::query("SELECT status FROM runtime_commands WHERE id=? FOR UPDATE")
        .bind(claim.command_id)
        .fetch_one(&mut *tx)
        .await?;
    if matches!(
        command.try_get::<String, _>("status")?.as_str(),
        "completed" | "failed"
    ) {
        tx.commit().await?;
        return Ok(());
    }
    let execution = sqlx::query(
        "SELECT status,state_version,invocation_id,work_package_id FROM workflow_executions WHERE id=? AND tenant_id=? FOR UPDATE",
    )
    .bind(claim.execution_id)
    .bind(claim.tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(RuntimeError::NotFound)?;
    let status: String = execution.try_get("status")?;
    if status != "queued" {
        let updated = sqlx::query("UPDATE runtime_commands SET status='completed',result_json=?,completed_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
            .bind(json!({"executionId":claim.execution_id,"replayed":true,"status":status}))
            .bind(claim.command_id).bind(claim.owner_id).bind(claim.fencing_token).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Runtime command Lease was lost".into(),
            ));
        }
        tx.commit().await?;
        return Ok(());
    }

    let state_version = execution.try_get::<u64, _>("state_version")? + 1;
    let invocation_id: Option<Uuid> = execution.try_get("invocation_id")?;
    let work_package_id: Option<Uuid> = execution.try_get("work_package_id")?;
    let output = json!({});
    let error = Some(json!({"code":error_code,"message":error_message}));
    let result_hash = agentx_runtime_contracts::content_hash(&output)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query("UPDATE workflow_executions SET status='failed',state_version=?,output_json=?,error_code=?,error_message=?,error_json=?,terminal_result_json=?,terminal_result_hash=?,ended_at=UTC_TIMESTAMP(6),duration_ms=TIMESTAMPDIFF(MICROSECOND,started_at,UTC_TIMESTAMP(6))/1000 WHERE id=? AND tenant_id=? AND status='queued'")
        .bind(state_version).bind(&output).bind(error_code).bind(error_message).bind(&error).bind(&output).bind(result_hash.as_str()).bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE execution_snapshots SET output_json=?,state_version=? WHERE execution_id=? AND tenant_id=?")
        .bind(&output).bind(state_version).bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE execution_runtime_state SET terminal_result_json=?,terminal_result_hash=? WHERE execution_id=? AND tenant_id=?")
        .bind(&output).bind(result_hash.as_str()).bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
    if invocation_id.is_some() {
        sqlx::query("UPDATE application_invocations SET status='failed',state_version=state_version+1,result_json=?,error_json=?,completed_at=UTC_TIMESTAMP(6) WHERE execution_id=? AND tenant_id=? AND status IN ('queued','running')")
            .bind(&output).bind(&error).bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
    }
    crate::work_package_execution::converge_execution(
        &mut tx,
        claim.tenant_id,
        claim.execution_id,
        work_package_id,
        "failed",
        &output,
        &error,
        result_hash.as_str(),
    )
    .await?;
    crate::composite_execution::converge_child(
        &mut tx,
        claim.tenant_id,
        claim.execution_id,
        "failed",
        &output,
        &error,
        &json!({}),
    )
    .await?;
    sqlx::query("UPDATE bundle_references SET released_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND reference_kind='active_execution' AND owner_id=? AND released_at IS NULL")
        .bind(claim.tenant_id).bind(claim.execution_id).execute(&mut *tx).await?;
    crate::quota::release_execution(
        &mut tx,
        claim.tenant_id,
        claim.execution_id,
        "execution_start_rejected",
    )
    .await?;
    if let Some(invocation_id) = invocation_id {
        sqlx::query("INSERT INTO invocation_events(tenant_id,invocation_id,event_id,sequence_number,event_type,payload_json) SELECT ?,?,?,COALESCE(MAX(sequence_number),0)+1,'invocation.failed',? FROM invocation_events WHERE tenant_id=? AND invocation_id=?")
            .bind(claim.tenant_id).bind(invocation_id).bind(Uuid::now_v7())
            .bind(json!({"executionId":claim.execution_id,"status":"failed","outputs":output,"error":error}))
            .bind(claim.tenant_id).bind(invocation_id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,capability,payload_json,status) VALUES(?,?,?,'runtime_event',NULL,?,'pending')")
        .bind(Uuid::now_v7()).bind(claim.tenant_id).bind(claim.execution_id)
        .bind(json!({"type":"execution.failed","resultHash":result_hash,"error":error}))
        .execute(&mut *tx).await?;
    let mut trace = crate::trace_delivery::TraceDraft::execution(
        claim.tenant_id,
        claim.execution_id,
        "execution.failed",
        "failed",
    );
    trace.error_code = Some(error_code.into());
    trace.error_message = Some(error_message.into());
    crate::trace_delivery::enqueue_best_effort(&mut tx, trace).await;
    let updated = sqlx::query("UPDATE runtime_commands SET status='failed',result_json=?,error_code=?,error_message=?,completed_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(json!({"executionId":claim.execution_id,"status":"failed"})).bind(error_code).bind(error_message)
        .bind(claim.command_id).bind(claim.owner_id).bind(claim.fencing_token).execute(&mut *tx).await?;
    if updated.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Runtime command Lease was lost".into(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

async fn process_cancel_command(
    pool: &sqlx::MySqlPool,
    claim: &RuntimeCommandClaim,
) -> RuntimeResult<()> {
    let mut tx = pool.begin().await?;
    let execution = sqlx::query(
        "SELECT status,invocation_id FROM workflow_executions WHERE id=? AND tenant_id=? FOR UPDATE",
    )
    .bind(claim.execution_id)
    .bind(claim.tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(RuntimeError::NotFound)?;
    let status: String = execution.try_get("status")?;
    let invocation_id: Option<Uuid> = execution.try_get("invocation_id")?;
    if !matches!(
        status.as_str(),
        "succeeded" | "failed" | "cancelled" | "timed_out"
    ) {
        sqlx::query("UPDATE workflow_executions SET status='cancelled',state_version=state_version+1,ended_at=UTC_TIMESTAMP(6),duration_ms=TIMESTAMPDIFF(MICROSECOND,started_at,UTC_TIMESTAMP(6))/1000 WHERE id=? AND tenant_id=?")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE application_invocations SET status='cancelled',state_version=state_version+1,completed_at=UTC_TIMESTAMP(6) WHERE execution_id=? AND tenant_id=? AND status NOT IN ('completed','failed','cancelled')")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE node_attempts SET status='cancelled',locked_until=NULL,ended_at=UTC_TIMESTAMP(6) WHERE execution_id=? AND tenant_id=? AND status IN ('queued','running','suspended')")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE node_executions SET status='cancelled',ended_at=UTC_TIMESTAMP(6) WHERE execution_id=? AND tenant_id=? AND status IN ('ready','queued','running','waiting')")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE execution_outbox SET status='published',published_at=UTC_TIMESTAMP(6),last_error='execution_cancelled_before_dispatch',locked_by=NULL,locked_until=NULL WHERE execution_id=? AND tenant_id=? AND message_type='dispatch_node' AND status='pending'")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE approval_tasks SET status='cancelled',version=version+1,locked_by=NULL,locked_until=NULL WHERE execution_id=? AND tenant_id=? AND status IN ('pending','claimed')")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE node_invocation_handles SET revoked_at=UTC_TIMESTAMP(6) WHERE execution_id=? AND tenant_id=? AND consumed_at IS NULL AND revoked_at IS NULL")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE runtime_calls SET status=CASE WHEN status='sent' THEN 'outcome_unknown' ELSE 'cancelled' END,ended_at=UTC_TIMESTAMP(6) WHERE execution_id=? AND tenant_id=? AND status IN ('reserved','sent')")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE sandbox_leases SET status='interrupting',expires_at=UTC_TIMESTAMP(6),last_error='execution_cancelled',locked_by=NULL,locked_until=NULL WHERE execution_id=? AND tenant_id=? AND status IN ('creating','ready','running')")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE agent_iterations i JOIN agent_runs r ON r.id=i.agent_run_id SET i.status='cancelled',i.ended_at=UTC_TIMESTAMP(6) WHERE r.execution_id=? AND r.tenant_id=? AND i.status='running'")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE agent_runs SET status='cancelled',ended_at=UTC_TIMESTAMP(6),stop_reason='cancelled' WHERE execution_id=? AND tenant_id=? AND status='running'")
            .bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        if let Err(error) = crate::engine_trace::finish_cancelled_spans(
            &mut tx,
            claim.tenant_id,
            claim.execution_id,
        )
        .await
        {
            tracing::warn!(%error, execution_id = %claim.execution_id, "Cancelled Span finalization failed");
        }
        sqlx::query("UPDATE bundle_references SET released_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND reference_kind='active_execution' AND owner_id=? AND released_at IS NULL")
            .bind(claim.tenant_id).bind(claim.execution_id).execute(&mut *tx).await?;
        crate::quota::release_execution(
            &mut tx,
            claim.tenant_id,
            claim.execution_id,
            "execution_cancelled",
        )
        .await?;
        let terminal_result = json!({"status":"cancelled"});
        let terminal_hash = agentx_runtime_contracts::content_hash(&terminal_result)
            .map_err(|error| RuntimeError::Internal(error.into()))?;
        sqlx::query("UPDATE execution_runtime_state SET terminal_result_json=?,terminal_result_hash=? WHERE execution_id=? AND tenant_id=?")
            .bind(&terminal_result).bind(terminal_hash.as_str()).bind(claim.execution_id).bind(claim.tenant_id).execute(&mut *tx).await?;
        if let Some(invocation_id) = invocation_id {
            sqlx::query("INSERT INTO invocation_events(tenant_id,invocation_id,event_id,sequence_number,event_type,payload_json) SELECT ?,?,?,COALESCE(MAX(sequence_number),0)+1,'invocation.cancelled',? FROM invocation_events WHERE tenant_id=? AND invocation_id=?")
                .bind(claim.tenant_id).bind(invocation_id).bind(Uuid::now_v7()).bind(json!({"executionId":claim.execution_id,"status":"cancelled"})).bind(claim.tenant_id).bind(invocation_id).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,capability,payload_json,status) VALUES(?,?,?,'runtime_event',NULL,?,'pending')")
            .bind(Uuid::now_v7()).bind(claim.tenant_id).bind(claim.execution_id)
            .bind(json!({"type":"execution.cancelled","invocationId":invocation_id}))
            .execute(&mut *tx).await?;
        crate::trace_delivery::enqueue_best_effort(
            &mut tx,
            crate::trace_delivery::TraceDraft::execution(
                claim.tenant_id,
                claim.execution_id,
                "execution.cancelled",
                "cancelled",
            ),
        )
        .await;
    }
    let updated = sqlx::query("UPDATE runtime_commands SET status='completed',result_json=?,completed_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(json!({"executionId":claim.execution_id,"status":if status == "cancelled" { "replayed" } else { "cancelled" }}))
        .bind(claim.command_id).bind(claim.owner_id).bind(claim.fencing_token).execute(&mut *tx).await?;
    if updated.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Runtime command Lease was lost".into(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{project_chat_message_input, validate_runtime_chat_mapping};
    use agentx_runtime_contracts::ChatMappingV1;
    use serde_json::json;

    #[test]
    fn chat_message_is_projected_only_to_explicitly_mapped_fields() {
        let mapping = ChatMappingV1 {
            question_input: "prompt".into(),
            file_input: Some("documents".into()),
            answer_output: "result".into(),
            answer_files_output: None,
        };
        let input = json!({"question":"hello","files":[{"artifactId":"00000000-0000-0000-0000-000000000001"}]});
        let schema = json!({"type":"object","properties":{
            "prompt":{"type":"string"},
            "documents":{"type":"array","x-agentx-artifact":true,"x-agentx-artifact-array":true}
        }});
        assert_eq!(
            project_chat_message_input(&input, &mapping, &schema).unwrap(),
            json!({"prompt":"hello","documents":[{"artifactId":"00000000-0000-0000-0000-000000000001"}]})
        );
    }

    #[test]
    fn chat_message_projects_one_file_to_a_single_artifact_field() {
        let mapping = ChatMappingV1 {
            question_input: "prompt".into(),
            file_input: Some("document".into()),
            answer_output: "result".into(),
            answer_files_output: None,
        };
        let reference = json!({"artifactId":"00000000-0000-0000-0000-000000000001"});
        let schema = json!({"type":"object","properties":{
            "prompt":{"type":"string"},
            "document":{"type":"object","x-agentx-artifact":true}
        }});
        assert_eq!(
            project_chat_message_input(
                &json!({"question":"hello","files":[reference.clone()]}),
                &mapping,
                &schema,
            )
            .unwrap(),
            json!({"prompt":"hello","document":reference})
        );
        assert!(
            project_chat_message_input(
                &json!({"question":"hello","files":[{}, {}]}),
                &mapping,
                &schema,
            )
            .is_err()
        );
    }

    #[test]
    fn runtime_mapping_validation_rejects_sensitive_answer_output() {
        let mapping = ChatMappingV1 {
            question_input: "prompt".into(),
            file_input: None,
            answer_output: "result".into(),
            answer_files_output: None,
        };
        let input = json!({"type":"object","properties":{"prompt":{"type":"string"}}});
        let output = json!({"type":"object","properties":{"result":{"type":"string","x-agentx-sensitive":true}}});
        assert!(validate_runtime_chat_mapping(&mapping, &input, &output).is_err());
    }

    #[test]
    fn modality_marked_inputs_receive_matching_artifact_references() {
        let mapping = ChatMappingV1 {
            question_input: "prompt".into(),
            file_input: Some("documents".into()),
            answer_output: "result".into(),
            answer_files_output: None,
        };
        let schema = json!({"type":"object","properties":{
            "prompt":{"type":"string"},
            "images":{"type":"array","x-agentx-modality":"image"},
            "documents":{"type":"array","x-agentx-artifact":true,"x-agentx-artifact-array":true}
        }});
        let image = json!({"artifactId":"00000000-0000-0000-0000-000000000002","type":"image"});
        let audio = json!({"artifactId":"00000000-0000-0000-0000-000000000003","type":"audio"});
        let projected = project_chat_message_input(
            &json!({"question":"describe this","files":[image.clone(), audio.clone()]}),
            &mapping,
            &schema,
        )
        .unwrap();
        assert_eq!(projected["prompt"], json!("describe this"));
        // Only parts matching the marked modality land in the marked input.
        assert_eq!(projected["images"], json!([image.clone()]));
        assert!(projected.get("audio").is_none());
        // The generic file mapping still receives every attachment.
        assert_eq!(projected["documents"], json!([image, audio]));
    }
}
