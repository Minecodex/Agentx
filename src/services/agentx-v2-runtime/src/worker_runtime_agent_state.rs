//! Durable Entry/Register/Usage StatePort for Agent Core.

use std::sync::Arc;

use agentx_agent_core::{
    AgentMessageV1, AgentSessionStateV1, CompactionSnapshotV1, MessageRole, StatePort,
    StatePortError,
};
use object_store::{ObjectStore, path::Path as ObjectPath};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySql, Pool, Row, Transaction};
use uuid::Uuid;

use super::ClaimedWorkerAttempt;

#[derive(Clone)]
pub(super) struct DurableStatePort {
    pool: Pool<MySql>,
    objects: Arc<dyn ObjectStore>,
    tenant_id: Uuid,
    application_id: Option<Uuid>,
    execution_id: Uuid,
    node_execution_id: Uuid,
    attempt_id: Uuid,
    session_key: String,
    node_key: String,
    bundle_hash: String,
    definition_hash: String,
    model_version: String,
    core_contract_version: String,
    deadline_at_millis: u64,
}
impl DurableStatePort {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        pool: Pool<MySql>,
        objects: Arc<dyn ObjectStore>,
        tenant_id: Uuid,
        application_id: Option<Uuid>,
        execution_id: Uuid,
        node_execution_id: Uuid,
        attempt_id: Uuid,
        session_key: String,
        node_key: String,
        bundle_hash: String,
        definition_hash: String,
        model_version: String,
        core_contract_version: String,
        deadline_at_millis: u64,
    ) -> Self {
        Self {
            pool,
            objects,
            tenant_id,
            application_id,
            execution_id,
            node_execution_id,
            attempt_id,
            session_key,
            node_key,
            bundle_hash,
            definition_hash,
            model_version,
            core_contract_version,
            deadline_at_millis,
        }
    }

    pub(super) fn session_busy(&self, _prompt_message_id: &str) -> Result<bool, StatePortError> {
        let this = self.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let Some(row) = sqlx::query("SELECT open_operation_id,lease_expires_at FROM agent_session_registers WHERE tenant_id=? AND application_id<=>? AND session_key=? AND stable_agent_node_key=?")
                    .bind(this.tenant_id).bind(this.application_id).bind(&this.session_key).bind(&this.node_key)
                    .fetch_optional(&this.pool).await.map_err(|e| StatePortError::Persistence(e.to_string()))? else { return Ok(false); };
                let open: Option<String> = row.try_get("open_operation_id").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                let Some(open) = open else { return Ok(false); };
                let lease_expires_at: Option<time::OffsetDateTime> = row
                    .try_get("lease_expires_at")
                    .map_err(|e| StatePortError::Persistence(e.to_string()))?;
                if lease_expires_at.is_some_and(|expires| expires <= time::OffsetDateTime::now_utc()) {
                    // A stale operation is recoverable by the next Worker;
                    // it must not permanently block the durable queue.
                    return Ok(false);
                }
                let owner_attempt: Option<Uuid> = sqlx::query_scalar("SELECT attempt_id FROM agent_session_operations WHERE operation_id=? AND tenant_id=? AND session_key=? AND stable_agent_node_key=?")
                    .bind(&open).bind(this.tenant_id).bind(&this.session_key).bind(&this.node_key)
                    .fetch_optional(&this.pool).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
                if owner_attempt == Some(this.attempt_id) {
                    return Ok(false);
                }
                let pending = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_session_pending_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? AND status='pending'")
                    .bind(this.tenant_id).bind(&this.session_key).bind(&this.node_key).fetch_one(&this.pool).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
                if pending >= 32 { return Ok(true); }
                let _ = open; // keeps the query's open-operation proof explicit
                Ok(true)
            })
        })
    }

    /// Queue a competing user input atomically and idempotently.  The retry
    /// wakeup worker will consume it once the current open Operation clears.  A full
    /// queue is deliberately reported as `None`, allowing the caller to
    /// return the stable backpressure error without creating an unbounded row.
    pub(super) fn enqueue_pending_prompt(
        &self,
        prompt_message_id: &str,
        prompt: &str,
    ) -> Result<Option<String>, StatePortError> {
        let this = self.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let mut tx = this
                    .pool
                    .begin()
                    .await
                    .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                // Serialize queue admission with the Session Register.  A
                // plain COUNT followed by INSERT lets concurrent Workers
                // both observe 31 rows and exceed the durable 32-item cap.
                let _register_lock = sqlx::query(
                    "SELECT state_version FROM agent_session_registers WHERE tenant_id=? AND application_id<=>? AND session_key=? AND stable_agent_node_key=? FOR UPDATE",
                )
                .bind(this.tenant_id)
                .bind(this.application_id)
                .bind(&this.session_key)
                .bind(&this.node_key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                let count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM agent_session_pending_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? AND status='pending'",
                )
                .bind(this.tenant_id)
                .bind(&this.session_key)
                .bind(&this.node_key)
                .fetch_one(&mut *tx)
                .await
                .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                let existing: Option<String> = sqlx::query_scalar(
                    "SELECT entry_id FROM agent_session_pending_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? AND idempotency_key=?",
                )
                .bind(this.tenant_id)
                .bind(&this.session_key)
                .bind(&this.node_key)
                .bind(prompt_message_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                if count >= 32 && existing.is_none() {
                    return Ok(None);
                }
                let is_new = existing.is_none();
                let entry_id = existing.unwrap_or_else(|| {
                    format!(
                        "pending:{}",
                        crate::worker_support::raw_hash(&json!({
                            "session": this.session_key,
                            "kind": "retry",
                            "message": prompt_message_id
                        }))
                    )
                });
                if is_new {
                    let sequence: u64 = sqlx::query_scalar(
                        "SELECT COALESCE(MAX(sequence_number),0)+1 FROM agent_session_pending_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=?",
                    )
                    .bind(this.tenant_id)
                    .bind(&this.session_key)
                    .bind(&this.node_key)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                    sqlx::query(
                        "INSERT IGNORE INTO agent_session_pending_entries(entry_id,tenant_id,session_key,stable_agent_node_key,execution_id,node_execution_id,attempt_id,queue_kind,idempotency_key,payload_json,sequence_number) VALUES(?,?,?,?,?,?,?,'retry',?,?,?)",
                    )
                    .bind(&entry_id)
                    .bind(this.tenant_id)
                    .bind(&this.session_key)
                    .bind(&this.node_key)
                    .bind(this.execution_id)
                    .bind(this.node_execution_id)
                    .bind(this.attempt_id)
                    .bind(prompt_message_id)
                    .bind(json!(AgentMessageV1::user(prompt_message_id, prompt)))
                    .bind(sequence)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                }
                tx.commit()
                    .await
                    .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                Ok(Some(entry_id))
            })
        })
    }

    /// Return the immutable Register snapshot used to construct the Core
    /// input.  The Worker must never invent a state version or leaf from local
    /// memory: a replacement Worker uses this same projection before it can
    /// issue the next CAS.
    pub(super) fn projection(
        &self,
        mode: agentx_agent_core::SessionPolicyModeV1,
        trusted_subject: Option<agentx_agent_core::ApplicationUserEvidenceV1>,
    ) -> Result<agentx_agent_core::AgentSessionProjectionV1, StatePortError> {
        let this = self.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let row = sqlx::query(
                    "SELECT state_version,leaf_entry_id,open_operation_id,bundle_hash,definition_hash,model_version,core_contract_version,retention_json FROM agent_session_registers WHERE tenant_id=? AND application_id<=>? AND session_key=? AND stable_agent_node_key=?",
                )
                .bind(this.tenant_id)
                .bind(this.application_id)
                .bind(&this.session_key)
                .bind(&this.node_key)
                .fetch_optional(&this.pool)
                .await
                .map_err(|error| StatePortError::Persistence(error.to_string()))?;

                let (state_version, leaf_entry_id, open_operation_id, retention) =
                    if let Some(row) = row {
                        let bundle: String = row
                            .try_get("bundle_hash")
                            .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                        let definition: String = row
                            .try_get("definition_hash")
                            .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                        let model: String = row
                            .try_get("model_version")
                            .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                        let contract: String = row
                            .try_get("core_contract_version")
                            .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                        if bundle != this.bundle_hash
                            || definition != this.definition_hash
                            || model != this.model_version
                            || contract != this.core_contract_version
                        {
                            return Err(StatePortError::Persistence(
                                "AGENT_CORE_VERSION_MISMATCH".into(),
                            ));
                        }
                        let retention_json: Option<Value> = row
                            .try_get("retention_json")
                            .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                        (
                            row.try_get("state_version")
                                .map_err(|error| StatePortError::Persistence(error.to_string()))?,
                            row.try_get("leaf_entry_id")
                                .map_err(|error| StatePortError::Persistence(error.to_string()))?,
                            row.try_get("open_operation_id")
                                .map_err(|error| StatePortError::Persistence(error.to_string()))?,
                            retention_json
                                .and_then(|value| serde_json::from_value(value).ok())
                                .unwrap_or_default(),
                        )
                    } else {
                        (0, None, None, agentx_agent_core::RetentionPolicyV1 {
                            session_expires_at_millis: None,
                            compaction_artifact_expires_at_millis: None,
                            protect_referenced_artifacts: true,
                        })
                    };
                Ok(agentx_agent_core::AgentSessionProjectionV1 {
                    mode,
                    session_key: this.session_key,
                    state_version,
                    leaf_entry_id,
                    open_operation_id,
                    trusted_subject,
                    retention,
                })
            })
        })
    }
}
impl StatePort for DurableStatePort {
    fn load(&mut self, _session_id: &str) -> Result<Option<AgentSessionStateV1>, StatePortError> {
        let this = self.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
            let row = sqlx::query("SELECT session_id,bundle_hash,definition_hash,model_version,core_contract_version,state_version,fencing_token,terminal_state,register_json FROM agent_session_registers WHERE tenant_id=? AND application_id<=>? AND session_key=? AND stable_agent_node_key=?")
                .bind(this.tenant_id).bind(this.application_id).bind(&this.session_key).bind(&this.node_key).fetch_optional(&this.pool).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let Some(row) = row else { return Ok(None); };
            let bundle: String = row.try_get("bundle_hash").map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let definition: String = row.try_get("definition_hash").map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let model: String = row.try_get("model_version").map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let contract: String = row.try_get("core_contract_version").map_err(|e| StatePortError::Persistence(e.to_string()))?;
            if bundle != this.bundle_hash || definition != this.definition_hash || model != this.model_version || contract != this.core_contract_version {
                return Err(StatePortError::Persistence("AGENT_CORE_VERSION_MISMATCH".into()));
            }
            let session_id: String = row.try_get("session_id").map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let state_version: u64 = row.try_get("state_version").map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let terminal: Option<String> = row.try_get("terminal_state").map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let register_json: Value = row.try_get("register_json").map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let entries = sqlx::query("SELECT entry_id,entry_kind,payload_json,payload_artifact_id FROM agent_session_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? AND lane='main' ORDER BY sequence_number,entry_id")
                .bind(this.tenant_id).bind(&this.session_key).bind(&this.node_key).fetch_all(&this.pool).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let mut messages = Vec::new();
            for entry in entries {
                let kind: String = entry.try_get("entry_kind").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                let inline_payload: Option<Value> = entry.try_get("payload_json").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                let payload = if let Some(payload) = inline_payload {
                    payload
                } else if let Some(artifact_id) = entry
                    .try_get::<Option<Uuid>, _>("payload_artifact_id")
                    .map_err(|error| StatePortError::Persistence(error.to_string()))?
                {
                    let object = sqlx::query(
                        "SELECT CAST(object_key AS CHAR CHARACTER SET utf8mb4) AS object_key,content_hash,size_bytes,status FROM runtime_objects WHERE tenant_id=? AND object_id=?",
                    )
                    .bind(this.tenant_id)
                    .bind(artifact_id)
                    .fetch_optional(&this.pool)
                    .await
                    .map_err(|error| StatePortError::Persistence(error.to_string()))?
                    .ok_or_else(|| {
                        StatePortError::Persistence(
                            "AGENT_SESSION_ARTIFACT_PAYLOAD_UNAVAILABLE".into(),
                        )
                    })?;
                    let status: String = object
                        .try_get("status")
                        .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                    let object_key: String = object
                        .try_get("object_key")
                        .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                    let expected_hash: String = object
                        .try_get("content_hash")
                        .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                    let expected_size: u64 = object
                        .try_get("size_bytes")
                        .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                    if status != "ready" || expected_size > 8 * 1024 * 1024 {
                        return Err(StatePortError::Persistence(
                            "AGENT_SESSION_ARTIFACT_PAYLOAD_UNAVAILABLE".into(),
                        ));
                    }
                    let bytes = this
                        .objects
                        .get(&ObjectPath::from(object_key))
                        .await
                        .map_err(|error| StatePortError::Persistence(error.to_string()))?
                        .bytes()
                        .await
                        .map_err(|error| StatePortError::Persistence(error.to_string()))?;
                    let actual_hash = format!("sha256:{:x}", Sha256::digest(&bytes));
                    if bytes.len() as u64 != expected_size || actual_hash != expected_hash {
                        return Err(StatePortError::Persistence(
                            "AGENT_SESSION_ARTIFACT_PAYLOAD_UNAVAILABLE".into(),
                        ));
                    }
                    serde_json::from_slice::<Value>(&bytes).map_err(|_| {
                        StatePortError::Persistence(
                            "AGENT_SESSION_ARTIFACT_PAYLOAD_UNAVAILABLE".into(),
                        )
                    })?
                } else {
                    return Err(StatePortError::Persistence(
                        "AGENT_SESSION_ENTRY_PAYLOAD_UNAVAILABLE".into(),
                    ));
                };
                match kind.as_str() {
                    "message.user" | "message.assistant" | "message.tool_call"
                    | "message.tool_result" | "custom.external_context" => {
                        let message = serde_json::from_value::<AgentMessageV1>(payload).map_err(|error| {
                            StatePortError::Persistence(format!("AGENT_SESSION_ENTRY_INVALID: {error}"))
                        })?;
                        messages.push(message);
                    }
                    "compaction" => {
                        serde_json::from_value::<CompactionSnapshotV1>(payload).map_err(|error| {
                            StatePortError::Persistence(format!("AGENT_COMPACTION_ENTRY_INVALID: {error}"))
                        })?;
                    }
                    _ => return Err(StatePortError::Persistence(format!(
                        "AGENT_SESSION_ENTRY_KIND_UNSUPPORTED: {kind}"
                    ))),
                }
            }
            let compaction = register_json
                .get("compaction")
                .cloned()
                .filter(|v| !v.is_null())
                .map(|value| {
                    serde_json::from_value::<agentx_agent_core::CompactionSnapshotV1>(value)
                        .map_err(|error| {
                            StatePortError::Persistence(format!(
                                "AGENT_COMPACTION_REGISTER_INVALID: {error}"
                            ))
                        })
                })
                .transpose()?;
            let pending = sqlx::query("SELECT queue_kind,payload_json FROM agent_session_pending_entries WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? AND status='pending' ORDER BY sequence_number,entry_id")
                .bind(this.tenant_id).bind(&this.session_key).bind(&this.node_key).fetch_all(&this.pool).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let mut steering_queue = Vec::new();
            let mut follow_up_queue = Vec::new();
            for row in pending {
                let payload: Value = row.try_get("payload_json").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                let message = serde_json::from_value::<AgentMessageV1>(payload).map_err(|error| {
                    StatePortError::Persistence(format!("AGENT_SESSION_PENDING_ENTRY_INVALID: {error}"))
                })?;
                match row.try_get::<String, _>("queue_kind").map_err(|e| StatePortError::Persistence(e.to_string()))?.as_str() {
                    "steering" => steering_queue.push(message),
                    "follow_up" | "retry" => follow_up_queue.push(message),
                    queue_kind => return Err(StatePortError::Persistence(format!("AGENT_SESSION_PENDING_KIND_UNSUPPORTED: {queue_kind}"))),
                }
            }
            // Register JSON is retained as a recovery snapshot for compaction
            // and older pending rows, but the durable pending table is the
            // queue authority whenever it exists.
            if steering_queue.is_empty() {
                steering_queue = register_json.get("steeringQueue").cloned()
                    .map(|v| serde_json::from_value(v).map_err(|error| StatePortError::Persistence(format!("AGENT_SESSION_QUEUE_INVALID: {error}"))))
                    .transpose()?.unwrap_or_default();
            }
            if follow_up_queue.is_empty() {
                follow_up_queue = register_json.get("followUpQueue").cloned()
                    .map(|v| serde_json::from_value(v).map_err(|error| StatePortError::Persistence(format!("AGENT_SESSION_QUEUE_INVALID: {error}"))))
                    .transpose()?.unwrap_or_default();
            }
            let operation = register_json.get("operation").cloned().filter(|v| !v.is_null())
                .map(|v| serde_json::from_value(v).map_err(|error| StatePortError::Persistence(format!("AGENT_SESSION_OPERATION_INVALID: {error}"))))
                .transpose()?;
            let overflow_retry_operation_id = register_json
                .get("overflowRetryOperationId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let terminal = terminal
                .map(|value| serde_json::from_str(&format!("\"{}\"", value)).map_err(|error| StatePortError::Persistence(format!("AGENT_SESSION_TERMINAL_INVALID: {error}"))))
                .transpose()?
                .or_else(|| register_json.get("terminal").cloned().filter(|v| !v.is_null()).and_then(|v| serde_json::from_value(v).ok()));
            Ok(Some(AgentSessionStateV1 { session_id, version: state_version, messages, compaction, steering_queue, follow_up_queue, operation, terminal, overflow_retry_operation_id }))
        })
        })
    }
    fn commit(
        &mut self,
        expected_version: u64,
        state: &AgentSessionStateV1,
        fencing_token: u64,
    ) -> Result<u64, StatePortError> {
        let this = self.clone();
        let mut persisted = state.clone();
        let next = expected_version.saturating_add(1);
        persisted.version = next;
        let register_json = json!({
            "compaction": persisted.compaction,
            "steeringQueue": persisted.steering_queue,
            "followUpQueue": persisted.follow_up_queue,
            "operation": persisted.operation,
            "terminal": persisted.terminal,
            "overflowRetryOperationId": persisted.overflow_retry_operation_id,
        });
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
            let mut tx: Transaction<'_, MySql> = this.pool.begin().await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
            let row = sqlx::query("SELECT state_version,fencing_token,bundle_hash,definition_hash,model_version,core_contract_version FROM agent_session_registers WHERE tenant_id=? AND application_id<=>? AND session_key=? AND stable_agent_node_key=? FOR UPDATE")
                .bind(this.tenant_id).bind(this.application_id).bind(&this.session_key).bind(&this.node_key).fetch_optional(&mut *tx).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
            match row {
                None if expected_version == 0 => {
                    sqlx::query("INSERT INTO agent_session_registers(tenant_id,application_id,session_key,stable_agent_node_key,session_id,bundle_hash,definition_hash,model_version,core_contract_version,leaf_entry_id,open_operation_id,state_version,fencing_token,terminal_state,operation_json,register_json,retention_json,lease_expires_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,FROM_UNIXTIME(? / 1000.0))")
                        .bind(this.tenant_id).bind(this.application_id).bind(&this.session_key).bind(&this.node_key).bind(&state.session_id).bind(&this.bundle_hash).bind(&this.definition_hash).bind(&this.model_version).bind(&this.core_contract_version).bind(state.messages.last().map(|m| scoped_entry_id(this.tenant_id, &this.session_key, &this.node_key, &m.message_id))).bind(open_operation_id(state.operation.as_ref())).bind(next).bind(fencing_token).bind(terminal_name(state.terminal.as_ref())).bind(state.operation.as_ref().map(|o| serde_json::to_value(o).unwrap_or(Value::Null))).bind(&register_json).bind(json!({"protectReferencedArtifacts":true})).bind(open_operation_id(state.operation.as_ref()).map(|_| this.deadline_at_millis)).execute(&mut *tx).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
                }
                Some(row) => {
                    let version: u64 = row.try_get("state_version").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                    let current_fence: u64 = row.try_get("fencing_token").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                    let bundle: String = row.try_get("bundle_hash").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                    let definition: String = row.try_get("definition_hash").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                    let model: String = row.try_get("model_version").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                    let contract: String = row.try_get("core_contract_version").map_err(|e| StatePortError::Persistence(e.to_string()))?;
                    if bundle != this.bundle_hash || definition != this.definition_hash || model != this.model_version || contract != this.core_contract_version { return Err(StatePortError::Persistence("AGENT_CORE_VERSION_MISMATCH".into())); }
                    if version != expected_version { return Err(StatePortError::Conflict); }
                    if current_fence > fencing_token { return Err(StatePortError::LeaseLost); }
                    let changed = sqlx::query("UPDATE agent_session_registers SET leaf_entry_id=?,open_operation_id=?,state_version=?,fencing_token=?,terminal_state=?,operation_json=?,register_json=?,retention_json=?,lease_expires_at=FROM_UNIXTIME(? / 1000.0) WHERE tenant_id=? AND application_id<=>? AND session_key=? AND stable_agent_node_key=? AND state_version=? AND fencing_token<=?")
                        .bind(state.messages.last().map(|m| scoped_entry_id(this.tenant_id, &this.session_key, &this.node_key, &m.message_id))).bind(open_operation_id(state.operation.as_ref())).bind(next).bind(fencing_token).bind(terminal_name(state.terminal.as_ref())).bind(state.operation.as_ref().map(|o| serde_json::to_value(o).unwrap_or(Value::Null))).bind(&register_json).bind(json!({"protectReferencedArtifacts":true})).bind(open_operation_id(state.operation.as_ref()).map(|_| this.deadline_at_millis)).bind(this.tenant_id).bind(this.application_id).bind(&this.session_key).bind(&this.node_key).bind(expected_version).bind(fencing_token).execute(&mut *tx).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
                    if changed.rows_affected() != 1 { return Err(StatePortError::Conflict); }
                }
                None => return Err(StatePortError::Conflict),
            }
            // Append message entries only.  INSERT IGNORE makes retries
            // idempotent while preserving the original immutable payload.
            for (index, message) in state.messages.iter().enumerate() {
                let kind = message_entry_kind(message);
                let payload = serde_json::to_value(message).map_err(|e| StatePortError::Persistence(e.to_string()))?;
                sqlx::query("INSERT IGNORE INTO agent_session_entries(entry_id,tenant_id,application_id,session_key,stable_agent_node_key,session_id,lane,sequence_number,parent_entry_id,entry_kind,payload_json,operation_id) VALUES(?,?,?,?,?,?, 'main', ?, ?, ?, ?, ?)")
                    .bind(scoped_entry_id(this.tenant_id, &this.session_key, &this.node_key, &message.message_id)).bind(this.tenant_id).bind(this.application_id).bind(&this.session_key).bind(&this.node_key).bind(&state.session_id).bind((index as u64).saturating_add(1)).bind(index.checked_sub(1).and_then(|parent| state.messages.get(parent)).map(|m| scoped_entry_id(this.tenant_id, &this.session_key, &this.node_key, &m.message_id))).bind(kind).bind(payload).bind(state.operation.as_ref().map(|o| o.operation_id.clone())).execute(&mut *tx).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
            }
            if let Some(snapshot) = &state.compaction {
                // Compaction is an append-only entry as well as a Register
                // projection.  Keep it outside the message sequence range so
                // a later message append cannot collide with an older cut.
                let compaction_id = compaction_entry_id(this.tenant_id, &this.session_key, &this.node_key, &snapshot.snapshot_id);
                let payload = serde_json::to_value(snapshot)
                    .map_err(|e| StatePortError::Persistence(e.to_string()))?;
                sqlx::query("INSERT IGNORE INTO agent_session_entries(entry_id,tenant_id,application_id,session_key,stable_agent_node_key,session_id,lane,sequence_number,parent_entry_id,entry_kind,payload_json,operation_id) VALUES(?,?,?,?,?,?, 'main', ?, ?, 'compaction', ?, ?)")
                    .bind(compaction_id)
                    .bind(this.tenant_id)
                    .bind(this.application_id)
                    .bind(&this.session_key)
                    .bind(&this.node_key)
                    .bind(&state.session_id)
                    .bind(9_000_000_000_u64.saturating_add(next))
                    .bind(state.messages.last().map(|message| scoped_entry_id(this.tenant_id, &this.session_key, &this.node_key, &message.message_id)))
                    .bind(payload)
                    .bind(state.operation.as_ref().map(|operation| operation.operation_id.clone()))
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| StatePortError::Persistence(e.to_string()))?;
            }
            for (queue_kind, queue) in [("steering", &state.steering_queue), ("follow_up", &state.follow_up_queue)] {
                for (index, message) in queue.iter().enumerate() {
                    let pending_id = format!("pending:{}", crate::worker_support::raw_hash(&json!({"session":this.session_key,"kind":queue_kind,"message":message.message_id})));
                    sqlx::query("INSERT IGNORE INTO agent_session_pending_entries(entry_id,tenant_id,session_key,stable_agent_node_key,queue_kind,idempotency_key,payload_json,sequence_number) VALUES(?,?,?,?,?,?,?,?)")
                        .bind(&pending_id).bind(this.tenant_id).bind(&this.session_key).bind(&this.node_key).bind(queue_kind).bind(&message.message_id).bind(serde_json::to_value(message).map_err(|e| StatePortError::Persistence(e.to_string()))?).bind(index as u64 + 1).execute(&mut *tx).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
                }
            }
            for message in &state.messages {
                sqlx::query("UPDATE agent_session_pending_entries SET status='consumed',consumed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND session_key=? AND stable_agent_node_key=? AND idempotency_key=? AND status='pending'")
                    .bind(this.tenant_id).bind(&this.session_key).bind(&this.node_key).bind(&message.message_id).execute(&mut *tx).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
            }
            if let Some(operation) = &state.operation {
                let op_json = serde_json::to_value(operation).map_err(|e| StatePortError::Persistence(e.to_string()))?;
                let phase = serde_json::to_value(&operation.phase)
                    .ok()
                    .and_then(|value| {
                        value.as_str().map(str::to_owned).or_else(|| {
                            value.get("phase").and_then(Value::as_str).map(str::to_owned)
                        })
                    })
                    .unwrap_or_else(|| "checkpoint".into());
                sqlx::query("INSERT INTO agent_session_operations(operation_id,tenant_id,session_key,stable_agent_node_key,attempt_id,fencing_token,expected_state_version,phase,operation_json,input_hash,bundle_hash,model_version,registry_hash,replay_policy,deadline_at,recovery_action) VALUES(?,?,?,?,?,?,?,?,?,?,?, ?,? ,?,FROM_UNIXTIME(? / 1000.0),?) ON DUPLICATE KEY UPDATE operation_json=VALUES(operation_json),phase=VALUES(phase),fencing_token=VALUES(fencing_token),expected_state_version=VALUES(expected_state_version),recovery_action=VALUES(recovery_action)")
                    .bind(&operation.operation_id).bind(this.tenant_id).bind(&this.session_key).bind(&this.node_key).bind(this.attempt_id).bind(fencing_token).bind(expected_version).bind(phase).bind(op_json).bind(crate::worker_support::raw_hash(&operation.intent)).bind(&this.bundle_hash).bind(&this.model_version).bind(crate::worker_support::raw_hash(&json!({"registry":"frozen"}))).bind(replay_policy_name(operation.replay_policy)).bind(this.deadline_at_millis).bind(recovery_action(&operation.phase)).execute(&mut *tx).await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
            }
            tx.commit().await.map_err(|e| StatePortError::Persistence(e.to_string()))?;
            Ok(next)
        })
        })
    }
}

fn scoped_entry_id(tenant_id: Uuid, session_key: &str, node_key: &str, message_id: &str) -> String {
    format!(
        "entry:{}",
        crate::worker_support::raw_hash(&json!({
            "tenant": tenant_id,
            "session": session_key,
            "node": node_key,
            "message": message_id
        }))
    )
}

fn compaction_entry_id(
    tenant_id: Uuid,
    session_key: &str,
    node_key: &str,
    snapshot_id: &str,
) -> String {
    format!(
        "compaction:{}",
        crate::worker_support::raw_hash(&json!({
            "tenant": tenant_id,
            "session": session_key,
            "node": node_key,
            "snapshot": snapshot_id
        }))
    )
}

fn message_entry_kind(message: &AgentMessageV1) -> &'static str {
    match message.role {
        MessageRole::User => "message.user",
        MessageRole::Assistant if !message.tool_calls.is_empty() => "message.tool_call",
        MessageRole::Assistant => "message.assistant",
        MessageRole::ToolResult => "message.tool_result",
        MessageRole::ExternalContext => "custom.external_context",
    }
}

fn terminal_name(value: Option<&agentx_agent_core::TerminalReasonV1>) -> Option<String> {
    value.map(|reason| {
        serde_json::to_value(reason)
            .unwrap_or(Value::Null)
            .as_str()
            .unwrap_or_default()
            .to_owned()
    })
}

fn open_operation_id(value: Option<&agentx_agent_core::OperationRecordV1>) -> Option<String> {
    value.and_then(|operation| match operation.phase {
        agentx_agent_core::OperationPhaseV1::SettledSuccess { .. }
        | agentx_agent_core::OperationPhaseV1::SettledFailure { .. }
        | agentx_agent_core::OperationPhaseV1::UnknownOutcome { .. } => None,
        _ => Some(operation.operation_id.clone()),
    })
}

fn recovery_action(phase: &agentx_agent_core::OperationPhaseV1) -> Option<String> {
    Some(
        match phase {
            agentx_agent_core::OperationPhaseV1::Pending { .. } => "resume_pending",
            agentx_agent_core::OperationPhaseV1::IntentPersisted => "execute_or_reconcile",
            agentx_agent_core::OperationPhaseV1::EffectStarted
            | agentx_agent_core::OperationPhaseV1::EffectCompleted { .. } => "reconcile_effect",
            agentx_agent_core::OperationPhaseV1::SettledSuccess { .. }
            | agentx_agent_core::OperationPhaseV1::SettledFailure { .. } => "resume",
            agentx_agent_core::OperationPhaseV1::UnknownOutcome { .. } => "pause_unknown",
        }
        .into(),
    )
}

fn replay_policy_name(policy: agentx_agent_core::ReplayPolicyV1) -> &'static str {
    match policy {
        agentx_agent_core::ReplayPolicyV1::Safe => "safe",
        agentx_agent_core::ReplayPolicyV1::Never => "never",
        agentx_agent_core::ReplayPolicyV1::IdempotencyRequired => "idempotency_required",
        agentx_agent_core::ReplayPolicyV1::LedgerDependent => "ledger_dependent",
    }
}

#[derive(Clone, Copy)]
pub(super) struct SessionIdentity {
    pub(super) application_id: Option<Uuid>,
    pub(super) session_id: Option<Uuid>,
    pub(super) trusted_subject: Option<Uuid>,
}
pub(super) async fn session_identity(
    pool: &Pool<MySql>,
    claim: &ClaimedWorkerAttempt,
) -> anyhow::Result<SessionIdentity> {
    let row = sqlx::query("SELECT e.application_id,i.session_id,s.execution_context_json FROM workflow_executions e LEFT JOIN application_invocations i ON i.tenant_id=e.tenant_id AND i.id=e.invocation_id JOIN execution_snapshots s ON s.tenant_id=e.tenant_id AND s.execution_id=e.id WHERE e.tenant_id=? AND e.id=?")
        .bind(claim.task.tenant_id).bind(claim.task.execution_id).fetch_optional(pool).await?;
    Ok(row
        .map(|row| SessionIdentity {
            application_id: row.try_get("application_id").ok(),
            session_id: row.try_get("session_id").ok().flatten(),
            trusted_subject: row
                .try_get::<Value, _>("execution_context_json")
                .ok()
                .and_then(|context| context.get("endUser").cloned())
                .and_then(|value| {
                    serde_json::from_value::<agentx_runtime_contracts::ApplicationUserIdentityV1>(
                        value,
                    )
                    .ok()
                })
                .map(|user| user.subject_id()),
        })
        .unwrap_or(SessionIdentity {
            application_id: None,
            session_id: None,
            trusted_subject: None,
        }))
}

pub(super) fn effective_agent_budget(parameters: &Value) -> Value {
    let budget = parameters.get("budget").unwrap_or(parameters);
    json!({
        "maxIterations":budget.get("maxIterations").and_then(Value::as_u64).unwrap_or(12).clamp(1, 12),
        "maxModelCalls":budget.get("maxModelCalls").and_then(Value::as_u64).unwrap_or(12).clamp(1, 12),
        "maxTokens":budget.get("maxTotalTokens").or_else(|| budget.get("maxTokens")).and_then(Value::as_u64).unwrap_or(64_000),
        "maxOutputTokens":budget.get("maxOutputTokens").and_then(Value::as_u64).unwrap_or(4_096),
        "maxCostMicros":budget.get("maxCost").and_then(Value::as_f64).map(|cost|(cost*1_000_000.0) as u64).unwrap_or(1_000_000),
        "maxDurationMs":budget.get("maxDurationSeconds").and_then(Value::as_u64).map(|seconds|seconds.saturating_mul(1_000)).unwrap_or(300_000),
        "maxTraceEvents":budget.get("maxTraceEvents").and_then(Value::as_u64).unwrap_or(1_024).clamp(1, 10_000)
    })
}
