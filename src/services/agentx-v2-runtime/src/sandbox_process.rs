//! Long-lived stdio MCP process sessions backed exclusively by OpenSandbox.

use std::{collections::BTreeMap, time::Duration};

use agentx_runtime_contracts::{
    ContentHash, ProcessSessionControlRequestV1, ProcessSessionControlResponseV1,
    ProcessSessionFrameV1, ProcessSessionFramesResponseV1, ProcessSessionLeaseV1,
    ProcessSessionProofV1, ProcessSessionReadRequestV1, ProcessSessionStartRequestV1,
    ProcessSessionStartResponseV1, ProcessSessionStatusV1, ProcessSessionWriteRequestV1,
    RuntimeObjectReferenceV1, RuntimeResourceBindingV1, RuntimeResourceConfigurationV1,
    SandboxEgressModeV1, StorageDomain,
};
use axum::extract::Path;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use object_store::path::Path as ObjectPath;
use reqwest::{Url, header::HeaderMap};
use serde_json::{Value, json};
use sqlx::Row;
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config,
    tungstenite::{
        Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};

use super::*;

const MAX_JSON_RPC_FRAME: usize = 1024 * 1024;
const MAX_INLINE_FRAME: usize = 64 * 1024;

type ProviderSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Clone, Default)]
pub struct ProcessSocketRegistry {
    sockets: std::sync::Arc<
        tokio::sync::Mutex<BTreeMap<Uuid, std::sync::Arc<tokio::sync::Mutex<ProviderSocket>>>>,
    >,
}

impl ProcessSocketRegistry {
    async fn get(&self, id: Uuid) -> Option<std::sync::Arc<tokio::sync::Mutex<ProviderSocket>>> {
        self.sockets.lock().await.get(&id).cloned()
    }

    async fn insert(&self, id: Uuid, socket: ProviderSocket) {
        self.sockets
            .lock()
            .await
            .insert(id, std::sync::Arc::new(tokio::sync::Mutex::new(socket)));
    }

    async fn remove(&self, id: Uuid) -> Option<std::sync::Arc<tokio::sync::Mutex<ProviderSocket>>> {
        self.sockets.lock().await.remove(&id)
    }
}

async fn close_process_socket(socket: &mut ProviderSocket) {
    let _ = socket
        .close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: "".into(),
        }))
        .await;
}

pub(super) async fn start(
    State(state): State<SandboxManagerState>,
    Json(request): Json<ProcessSessionStartRequestV1>,
) -> RuntimeResult<Json<ProcessSessionStartResponseV1>> {
    validate_start(&request)?;
    workspace::ensure_attempt_lease_values(
        &state,
        request.identity.tenant_id,
        request.execution_id,
        request.node_execution_id,
        request.attempt_id,
        request.worker_id,
        request.fencing_token,
    )
    .await?;
    let profile_version = request.profile.resource_version.clone();
    if profile_version != request.identity.sandbox_profile_version_id.to_string() {
        return Err(bad_request(
            "Process Session Sandbox Profile version does not match",
        ));
    }
    let identity_hash = raw_hash(
        &agentx_runtime_contracts::canonical_bytes(&request.identity)
            .map_err(|error| RuntimeError::Internal(error.into()))?,
    );
    let process_session_id = stable_id(Uuid::nil(), identity_hash.as_bytes());
    let lease_id = stable_id(request.attempt_id, b"mcp-process-lease");
    let ttl = process_profile_ttl(&request.profile).clamp(1, 86_400);
    let now = time::OffsetDateTime::now_utc();
    if let Some(row) = sqlx::query("SELECT identity_hash,status,sandbox_id,provider_operation_id,lease_id,attempt_id,worker_id,fencing_token,lease_expires_at,process_expires_at FROM sandbox_process_sessions WHERE process_session_id=?")
        .bind(process_session_id)
        .fetch_optional(&state.pool)
        .await?
    {
        if row.try_get::<String, _>("identity_hash")? != identity_hash {
            return Err(conflict("Process Session identity collision"));
        }
        let status: String = row.try_get("status")?;
        let lease_expires_at: time::OffsetDateTime = row.try_get("lease_expires_at")?;
        let process_expires_at: time::OffsetDateTime = row.try_get("process_expires_at")?;
        if status == "running"
            && process_expires_at > now
            && row.try_get::<Uuid, _>("attempt_id")? == request.attempt_id
            && row.try_get::<Uuid, _>("worker_id")? == request.worker_id
            && row.try_get::<u64, _>("fencing_token")? == request.fencing_token
            && lease_expires_at > now
        {
            return Ok(Json(start_response(
                &request,
                process_session_id,
                row.try_get("lease_id")?,
                row.try_get("sandbox_id")?,
                request.fencing_token,
                lease_expires_at,
            )));
        }
        if matches!(status.as_str(), "acquiring" | "starting" | "running")
            && lease_expires_at > now
        {
            return Err(conflict("Process Session is leased by another Worker"));
        }
        if status == "running" && process_expires_at > now && lease_expires_at <= now {
            let changed = sqlx::query(
                "UPDATE sandbox_process_sessions SET lease_id=?,attempt_id=?,worker_id=?,fencing_token=?,lease_expires_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND) WHERE process_session_id=? AND lease_expires_at<=UTC_TIMESTAMP(6) AND status='running'",
            )
            .bind(lease_id)
            .bind(request.attempt_id)
            .bind(request.worker_id)
            .bind(request.fencing_token)
            .bind(LEASE_SECONDS)
            .bind(process_session_id)
            .execute(&state.pool)
            .await?;
            if changed.rows_affected() == 1 {
                return Ok(Json(start_response(
                    &request,
                    process_session_id,
                    lease_id,
                    row.try_get("sandbox_id")?,
                    request.fencing_token,
                    now + time::Duration::seconds(i64::from(LEASE_SECONDS)),
                )));
            }
            return Err(conflict("Process Session Lease takeover lost a race"));
        }
        return Err(RuntimeError::Unavailable);
    }

    let command_json = json!({"command":request.command,"args":request.args});
    let credential_refs_json = serde_json::to_value(&request.environment_credentials)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query("INSERT INTO sandbox_process_sessions(process_session_id,identity_hash,tenant_id,agent_run_id,mcp_server_version_id,sandbox_profile_version_id,profile_json,command_json,credential_refs_json,status,lease_id,attempt_id,worker_id,fencing_token,lease_expires_at,process_expires_at) VALUES(?,?,?,?,?,?,?,?,?,'acquiring',?,?,?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND))")
        .bind(process_session_id)
        .bind(&identity_hash)
        .bind(request.identity.tenant_id)
        .bind(request.identity.agent_run_id)
        .bind(request.identity.mcp_server_version_id)
        .bind(request.identity.sandbox_profile_version_id)
        .bind(serde_json::to_value(&request.profile).map_err(|error| RuntimeError::Internal(error.into()))?)
        .bind(command_json)
        .bind(credential_refs_json)
        .bind(lease_id)
        .bind(request.attempt_id)
        .bind(request.worker_id)
        .bind(request.fencing_token)
        .bind(LEASE_SECONDS)
        .bind(ttl)
        .execute(&state.pool)
        .await?;

    let result = start_provider_process(&state, &request, process_session_id, ttl).await;
    let (sandbox_id, provider_process_id) = match result {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                error = %error.message,
                outcome_unknown = error.outcome_unknown,
                process_session_id = %process_session_id,
                "OpenSandbox Process Session start failed"
            );
            let status = if error.outcome_unknown {
                "unknown_outcome"
            } else {
                "failed"
            };
            let _ = sqlx::query("UPDATE sandbox_process_sessions SET status=?,last_error=? WHERE process_session_id=?")
                .bind(status)
                .bind(&error.message)
                .bind(process_session_id)
                .execute(&state.pool)
                .await;
            return Err(if error.outcome_unknown {
                RuntimeError::Unavailable
            } else {
                RuntimeError::ProviderRejected
            });
        }
    };
    let changed = sqlx::query("UPDATE sandbox_process_sessions SET sandbox_id=?,provider_operation_id=?,status='running' WHERE process_session_id=? AND status='acquiring' AND lease_id=? AND fencing_token=?")
        .bind(&sandbox_id)
        .bind(&provider_process_id)
        .bind(process_session_id)
        .bind(lease_id)
        .bind(request.fencing_token)
        .execute(&state.pool)
        .await?;
    if changed.rows_affected() != 1 {
        if let Some(socket) = state.process_sockets.remove(process_session_id).await {
            let mut socket = socket.lock().await;
            close_process_socket(&mut socket).await;
        }
        let _ = delete_provider_process(&state, &sandbox_id, &provider_process_id).await;
        let _ = terminate_provider(&state, &sandbox_id, &request.idempotency_key).await;
        return Err(conflict("Process Session Lease was lost while starting"));
    }
    Ok(Json(start_response(
        &request,
        process_session_id,
        lease_id,
        Some(sandbox_id),
        request.fencing_token,
        now + time::Duration::seconds(i64::from(LEASE_SECONDS)),
    )))
}

pub(super) async fn write(
    State(state): State<SandboxManagerState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ProcessSessionWriteRequestV1>,
) -> RuntimeResult<Json<ProcessSessionFramesResponseV1>> {
    validate_proof(&state, id, &request.proof).await?;
    if let Some(request_id) = request.frame.get("id") {
        let maximum = maximum_sequence(&state, id).await?;
        let existing = response_after(&state, id, maximum.saturating_sub(1_024), 1_024).await?;
        if existing
            .frames
            .iter()
            .any(|frame| frame.stream == "stdout" && frame.payload.get("id") == Some(request_id))
        {
            return Ok(Json(existing));
        }
    }
    let encoded = encode_json_rpc_frame(&request.frame)?;
    let row = load_process(&state, id, &request.proof).await?;
    let sandbox_id: String = row
        .try_get::<Option<String>, _>("sandbox_id")?
        .ok_or(RuntimeError::Unavailable)?;
    let provider_process_id: String = row
        .try_get::<Option<String>, _>("provider_operation_id")?
        .ok_or(RuntimeError::Unavailable)?;
    let offset: u64 = row.try_get("provider_output_offset")?;
    let before = maximum_sequence(&state, id).await?;
    let socket =
        active_process_socket(&state, id, &sandbox_id, &provider_process_id, offset).await?;
    let mut socket = socket.lock().await;
    let envelope = serde_json::to_vec(&json!({
        "type":"stdin",
        "data":STANDARD.encode([encoded.as_slice(), b"\n"].concat())
    }))
    .map_err(|error| RuntimeError::Internal(error.into()))?;
    let mut wire = Vec::with_capacity(envelope.len() + 2);
    wire.push(0_u8);
    wire.extend_from_slice(&envelope);
    wire.push(b'\n');
    socket
        .send(Message::Binary(wire.into()))
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    let wait_millis = if request.frame.get("id").is_some() {
        5_000
    } else {
        100
    };
    let collected = collect_socket(
        &state,
        id,
        &mut socket,
        offset,
        request.proof.deadline,
        wait_millis,
    )
    .await?;
    if let Some(request_id) = request.frame.get("id")
        && !collected
            .iter()
            .any(|frame| frame.stream == "stdout" && frame.payload.get("id") == Some(request_id))
    {
        return Err(RuntimeError::Unavailable);
    }
    response_after(&state, id, before, 256).await.map(Json)
}

pub(super) async fn read(
    State(state): State<SandboxManagerState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ProcessSessionReadRequestV1>,
) -> RuntimeResult<Json<ProcessSessionFramesResponseV1>> {
    validate_proof(&state, id, &request.proof).await?;
    reconcile_output(&state, id, &request.proof, request.wait_millis).await?;
    response_after(
        &state,
        id,
        request.after_sequence,
        request.maximum_frames.min(1024),
    )
    .await
    .map(Json)
}

pub(super) async fn wait(
    State(state): State<SandboxManagerState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ProcessSessionReadRequestV1>,
) -> RuntimeResult<Json<ProcessSessionFramesResponseV1>> {
    read(State(state), Path(id), Json(request)).await
}

pub(super) async fn interrupt(
    State(state): State<SandboxManagerState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ProcessSessionControlRequestV1>,
) -> RuntimeResult<Json<ProcessSessionControlResponseV1>> {
    control(&state, id, &request.proof, false).await.map(Json)
}

pub(super) async fn terminate(
    State(state): State<SandboxManagerState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ProcessSessionControlRequestV1>,
) -> RuntimeResult<Json<ProcessSessionControlResponseV1>> {
    control(&state, id, &request.proof, true).await.map(Json)
}

pub(super) async fn reconcile(
    State(state): State<SandboxManagerState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ProcessSessionControlRequestV1>,
) -> RuntimeResult<Json<ProcessSessionControlResponseV1>> {
    validate_proof(&state, id, &request.proof).await?;
    reconcile_output(&state, id, &request.proof, 0).await?;
    let row = sqlx::query(
        "SELECT status,exit_code FROM sandbox_process_sessions WHERE process_session_id=?",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(ProcessSessionControlResponseV1 {
        api_version: 1,
        process_session_id: id,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        exit_code: row.try_get("exit_code")?,
    }))
}

pub(super) async fn reconcile_one(state: &SandboxManagerState) -> RuntimeResult<bool> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT process_session_id,sandbox_id,provider_operation_id,status FROM sandbox_process_sessions WHERE (process_expires_at<=UTC_TIMESTAMP(6) OR status IN ('terminating','unknown_outcome','expired')) AND lease_expires_at<=UTC_TIMESTAMP(6) ORDER BY process_expires_at,process_session_id LIMIT 1 FOR UPDATE SKIP LOCKED")
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(false);
    };
    let id: Uuid = row.try_get("process_session_id")?;
    let sandbox_id: Option<String> = row.try_get("sandbox_id")?;
    let provider_process_id: Option<String> = row.try_get("provider_operation_id")?;
    sqlx::query("UPDATE sandbox_process_sessions SET status='terminating',fencing_token=fencing_token+1 WHERE process_session_id=?")
        .bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    if let Some(socket) = state.process_sockets.remove(id).await {
        let mut socket = socket.lock().await;
        close_process_socket(&mut socket).await;
    }
    let cleanup = async {
        if let (Some(sandbox_id), Some(process_id)) =
            (sandbox_id.as_deref(), provider_process_id.as_deref())
        {
            if let Err(error) = delete_provider_process(state, sandbox_id, process_id).await {
                tracing::warn!(error = %error.message, process_session_id = %id, "Reaper PTY deletion required Sandbox termination");
            }
        }
        if let Some(sandbox_id) = sandbox_id.as_deref() {
            terminate_provider(state, sandbox_id, &format!("process-reaper:{id}")).await?;
        }
        Ok::<(), ProviderError>(())
    }
    .await;
    match cleanup {
        Ok(()) => {
            sqlx::query("UPDATE sandbox_process_sessions SET status='expired',exit_code=COALESCE(exit_code,143),last_error=NULL WHERE process_session_id=? AND status='terminating'")
                .bind(id).execute(&state.pool).await?;
        }
        Err(error) => {
            sqlx::query("UPDATE sandbox_process_sessions SET status='unknown_outcome',last_error=? WHERE process_session_id=? AND status='terminating'")
                .bind(error.message.chars().take(1000).collect::<String>()).bind(id).execute(&state.pool).await?;
        }
    }
    Ok(true)
}

fn validate_start(request: &ProcessSessionStartRequestV1) -> RuntimeResult<()> {
    if request.api_version != 1
        || request.command.trim().is_empty()
        || !request.command.starts_with('/')
        || request.command.contains('\0')
        || request.args.iter().any(|argument| argument.contains('\0'))
        || request.idempotency_key.trim().is_empty()
        || request.operation_id.trim().is_empty()
        || request.effect_id.trim().is_empty()
        || request.deadline <= time::OffsetDateTime::now_utc()
    {
        return Err(bad_request("invalid Process Session start request"));
    }
    let mut names = std::collections::BTreeSet::new();
    for credential in &request.environment_credentials {
        if !valid_environment_name(&credential.name) || !names.insert(credential.name.clone()) {
            return Err(bad_request(
                "Process environment Credential name is invalid or duplicated",
            ));
        }
    }
    if !matches!(
        request.profile.configuration,
        RuntimeResourceConfigurationV1::SandboxProfile { .. }
    ) {
        return Err(bad_request(
            "Process Session requires an immutable Sandbox Profile",
        ));
    }
    Ok(())
}

fn encode_json_rpc_frame(frame: &Value) -> RuntimeResult<Vec<u8>> {
    let encoded = serde_json::to_vec(frame).map_err(|error| bad_request(&error.to_string()))?;
    if encoded.len() > MAX_JSON_RPC_FRAME
        || !frame.is_object()
        || frame.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
    {
        return Err(bad_request(
            "MCP stdio frame must be a JSON-RPC 2.0 object no larger than 1 MiB",
        ));
    }
    Ok(encoded)
}

fn parse_stdout_json_rpc_frame(decoded: &[u8]) -> RuntimeResult<Value> {
    if decoded.len() > MAX_JSON_RPC_FRAME {
        return Err(RuntimeError::ProviderRejected);
    }
    let decoded = decoded
        .strip_suffix(b"\n")
        .unwrap_or(decoded)
        .strip_suffix(b"\r")
        .unwrap_or_else(|| decoded.strip_suffix(b"\n").unwrap_or(decoded));
    if decoded.contains(&b'\n') || decoded.contains(&b'\r') {
        return Err(RuntimeError::ProviderRejected);
    }
    let frame: Value =
        serde_json::from_slice(decoded).map_err(|_| RuntimeError::ProviderRejected)?;
    if !frame.is_object() || frame.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(RuntimeError::ProviderRejected);
    }
    Ok(frame)
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| matches!(byte, b'A'..=b'Z' | b'0'..=b'9' | b'_'))
        && name.len() <= 128
}

async fn validate_proof(
    state: &SandboxManagerState,
    id: Uuid,
    proof: &ProcessSessionProofV1,
) -> RuntimeResult<()> {
    if proof.api_version != 1
        || proof.process_session_id != id
        || proof.deadline <= time::OffsetDateTime::now_utc()
        || proof.operation_id.trim().is_empty()
        || proof.effect_id.trim().is_empty()
        || proof.idempotency_key.trim().is_empty()
    {
        return Err(bad_request("invalid Process Session proof"));
    }
    workspace::ensure_attempt_lease_values(
        state,
        proof.tenant_id,
        proof.execution_id,
        proof.node_execution_id,
        proof.attempt_id,
        proof.worker_id,
        proof.fencing_token,
    )
    .await?;
    let renewed = sqlx::query(
        "UPDATE sandbox_process_sessions SET lease_expires_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND) WHERE process_session_id=? AND tenant_id=? AND agent_run_id=? AND lease_id=? AND attempt_id=? AND worker_id=? AND fencing_token=? AND process_expires_at>UTC_TIMESTAMP(6) AND status='running'",
    )
    .bind(LEASE_SECONDS)
    .bind(id)
    .bind(proof.tenant_id)
    .bind(proof.agent_run_id)
    .bind(proof.lease_id)
    .bind(proof.attempt_id)
    .bind(proof.worker_id)
    .bind(proof.fencing_token)
    .execute(&state.pool)
    .await?;
    if renewed.rows_affected() != 1 {
        return Err(conflict("Process Session Lease was lost"));
    }
    Ok(())
}

async fn load_process(
    state: &SandboxManagerState,
    id: Uuid,
    proof: &ProcessSessionProofV1,
) -> RuntimeResult<sqlx::mysql::MySqlRow> {
    sqlx::query("SELECT sandbox_id,provider_operation_id,provider_output_offset,status,exit_code FROM sandbox_process_sessions WHERE process_session_id=? AND tenant_id=? AND agent_run_id=? AND lease_id=? AND attempt_id=? AND worker_id=? AND fencing_token=? AND process_expires_at>UTC_TIMESTAMP(6) AND status='running'")
        .bind(id)
        .bind(proof.tenant_id)
        .bind(proof.agent_run_id)
        .bind(proof.lease_id)
        .bind(proof.attempt_id)
        .bind(proof.worker_id)
        .bind(proof.fencing_token)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| conflict("Process Session Lease was lost"))
}

async fn start_provider_process(
    state: &SandboxManagerState,
    request: &ProcessSessionStartRequestV1,
    process_session_id: Uuid,
    ttl: u32,
) -> Result<(String, String), ProviderError> {
    let (image, cpu, memory, disk, _, egress) =
        process_profile_limits(&request.profile).map_err(|error| ProviderError {
            message: format!("{error:?}"),
            outcome_unknown: false,
        })?;
    let metadata = BTreeMap::from([
        (
            "agentxProcessSessionId".into(),
            process_session_id.to_string(),
        ),
        (
            "agentxTenantId".into(),
            request.identity.tenant_id.to_string(),
        ),
        (
            "agentxAgentRunId".into(),
            request.identity.agent_run_id.to_string(),
        ),
    ]);
    let created = create_provider_sandbox(
        state,
        &image,
        ttl,
        cpu,
        memory,
        disk,
        egress,
        metadata,
        &request.idempotency_key,
    )
    .await?;
    let sandbox_id = created
        .0
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError {
            message: "OpenSandbox create response has no sandbox id".into(),
            outcome_unknown: false,
        })?
        .to_owned();
    let process = async {
    let endpoint = provider_execd_endpoint(state, &sandbox_id).await?;
    wait_for_provider_execd(state, &endpoint.0, &endpoint.1).await?;
    let command = launcher_command();
    let response = state.client
        .post(endpoint.0.join("pty").map_err(provider_url_error)?)
        .headers(endpoint.1.clone())
        .json(&json!({"cwd":"/workspace","command":command}))
        .send()
        .await
        .map_err(provider_request_error)?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError { message: format!("OpenSandbox PTY create HTTP {status}: {body}"), outcome_unknown: false });
    }
    let payload: Value = response.json().await.map_err(provider_request_error)?;
    let provider_process_id = payload.get("session_id").and_then(Value::as_str).ok_or_else(|| ProviderError {
        message: "OpenSandbox PTY create response has no session_id".into(),
        outcome_unknown: false,
    })?.to_owned();
    let mut socket = open_socket(endpoint, &provider_process_id, 0, false).await.map_err(|error| ProviderError {
        message: format!("OpenSandbox PTY connect failed: {error:?}"),
        outcome_unknown: true,
    })?;
    wait_connected_provider(&mut socket).await?;
    let mut environment = serde_json::Map::new();
    for credential in &request.environment_credentials {
        let vault = state.vault.as_ref().ok_or_else(|| ProviderError {
            message: "Runtime Vault is unavailable for stdio MCP".into(),
            outcome_unknown: false,
        })?;
        let value = vault.read(&credential.credential).await.map_err(|_| ProviderError {
            message: format!("Runtime Vault reference for {} is unavailable", credential.name),
            outcome_unknown: false,
        })?;
        let value = String::from_utf8(value).map_err(|_| ProviderError {
            message: format!("Runtime Vault value for {} is not UTF-8", credential.name),
            outcome_unknown: false,
        })?;
        environment.insert(credential.name.clone(), Value::String(value));
    }
    let configuration = serde_json::to_vec(&json!({
        "argv": std::iter::once(request.command.clone()).chain(request.args.clone()).collect::<Vec<_>>(),
        "env": environment,
    })).map_err(|error| ProviderError { message:error.to_string(), outcome_unknown:false })?;
    let mut frame = Vec::with_capacity(configuration.len() + 2);
    frame.push(0_u8);
    frame.extend_from_slice(&configuration);
    frame.push(b'\n');
    socket.send(Message::Binary(frame.into())).await.map_err(|error| ProviderError {
        message: error.to_string(),
        outcome_unknown: true,
    })?;
    state
        .process_sockets
        .insert(process_session_id, socket)
        .await;
    Ok::<String, ProviderError>(provider_process_id)
    }.await;
    match process {
        Ok(provider_process_id) => Ok((sandbox_id, provider_process_id)),
        Err(error) => {
            let _ = terminate_provider(state, &sandbox_id, &request.idempotency_key).await;
            Err(error)
        }
    }
}

async fn wait_for_provider_execd(
    state: &SandboxManagerState,
    endpoint: &Url,
    headers: &HeaderMap,
) -> Result<(), ProviderError> {
    let command_url = endpoint.join("command").map_err(provider_url_error)?;
    let mut last_error = "OpenSandbox execd did not accept a readiness command".to_owned();
    for attempt in 0..4_u64 {
        match state
            .client
            .post(command_url.clone())
            .headers(headers.clone())
            .timeout(Duration::from_secs(10))
            .json(&json!({
                "command":"true",
                "cwd":"/workspace",
                "background":false,
                "timeout":5_000,
                "envs":{}
            }))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                last_error = format!("OpenSandbox execd readiness HTTP {status}: {body}");
            }
            Err(error) => {
                last_error = format!("OpenSandbox execd readiness failed: {error}");
            }
        }
        tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await;
    }
    Err(ProviderError {
        message: last_error,
        outcome_unknown: false,
    })
}

fn launcher_command() -> String {
    let source = r#"import sys,json,base64,subprocess,threading,os
lock=threading.Lock()
def emit(stream,data):
  with lock:
    sys.stdout.write(json.dumps({'stream':stream,'data':base64.b64encode(data).decode()})+'\n');sys.stdout.flush()
cfg=json.loads(sys.stdin.readline())
env=os.environ.copy();env.update(cfg.get('env',{}))
p=subprocess.Popen(cfg['argv'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,cwd='/workspace',env=env,bufsize=0)
def pump(src,name):
  while True:
    data=src.readline()
    if not data: break
    emit(name,data)
threading.Thread(target=pump,args=(p.stdout,'stdout'),daemon=True).start()
threading.Thread(target=pump,args=(p.stderr,'stderr'),daemon=True).start()
def finish():
  rc=p.wait();emit('exit',str(rc).encode());os._exit(0)
threading.Thread(target=finish,daemon=True).start()
for line in sys.stdin:
  try:
    msg=json.loads(line)
    if msg.get('type')=='stdin': p.stdin.write(base64.b64decode(msg['data']));p.stdin.flush()
  except Exception as e: emit('stderr',str(e).encode())
"#;
    format!(
        "python3 -c \"import base64;exec(base64.b64decode('{}'))\"",
        STANDARD.encode(source)
    )
}

async fn provider_execd_endpoint(
    state: &SandboxManagerState,
    sandbox_id: &str,
) -> Result<(Url, HeaderMap), ProviderError> {
    let endpoint_url = format!(
        "{}/v1/sandboxes/{sandbox_id}/endpoints/{EXECD_PORT}?use_server_proxy=true",
        state.provider_endpoint.trim_end_matches('/')
    );
    let (payload, _) = provider_json(authenticated(state, state.client.get(endpoint_url))).await?;
    let provider = payload
        .get("endpoint")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError {
            message: "OpenSandbox execd endpoint response is missing endpoint".into(),
            outcome_unknown: false,
        })?;
    let endpoint = server_proxy_endpoint(&state.provider_endpoint, provider, sandbox_id)?;
    let mut headers = HeaderMap::new();
    if let Some(values) = payload.get("headers").and_then(Value::as_object) {
        for (name, value) in values {
            let name =
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                    ProviderError {
                        message: error.to_string(),
                        outcome_unknown: false,
                    }
                })?;
            let value = reqwest::header::HeaderValue::from_str(value.as_str().unwrap_or_default())
                .map_err(|error| ProviderError {
                    message: error.to_string(),
                    outcome_unknown: false,
                })?;
            headers.insert(name, value);
        }
    }
    if let Some(key) = &state.provider_api_key {
        headers.insert(
            "open-sandbox-api-key",
            reqwest::header::HeaderValue::from_str(key).map_err(|error| ProviderError {
                message: error.to_string(),
                outcome_unknown: false,
            })?,
        );
    }
    Ok((endpoint, headers))
}

async fn open_provider_socket(
    state: &SandboxManagerState,
    sandbox_id: &str,
    provider_process_id: &str,
    since: u64,
    viewer: bool,
) -> RuntimeResult<ProviderSocket> {
    let endpoint = provider_execd_endpoint(state, sandbox_id)
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    open_socket(endpoint, provider_process_id, since, viewer).await
}

async fn active_process_socket(
    state: &SandboxManagerState,
    process_session_id: Uuid,
    sandbox_id: &str,
    provider_process_id: &str,
    offset: u64,
) -> RuntimeResult<std::sync::Arc<tokio::sync::Mutex<ProviderSocket>>> {
    if let Some(socket) = state.process_sockets.get(process_session_id).await {
        return Ok(socket);
    }
    let mut socket =
        open_provider_socket(state, sandbox_id, provider_process_id, offset, false).await?;
    wait_connected(&mut socket).await?;
    state
        .process_sockets
        .insert(process_session_id, socket)
        .await;
    state
        .process_sockets
        .get(process_session_id)
        .await
        .ok_or(RuntimeError::Unavailable)
}

async fn open_socket(
    (endpoint, headers): (Url, HeaderMap),
    provider_process_id: &str,
    since: u64,
    viewer: bool,
) -> RuntimeResult<ProviderSocket> {
    use tokio_rustls::rustls::{
        ClientConfig, RootCertStore,
        pki_types::{CertificateDer, pem::PemObject},
    };
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mode = if viewer { "viewer" } else { "holder" };
    let mut url = endpoint
        .join(&format!(
            "pty/{provider_process_id}/ws?pty=0&takeover=1&mode={mode}&since={since}"
        ))
        .map_err(|_| RuntimeError::Unavailable)?;
    url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
        .map_err(|_| RuntimeError::Unavailable)?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| RuntimeError::Unavailable)?;
    for (name, value) in headers.iter() {
        request.headers_mut().insert(name, value.clone());
    }
    let mut roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    if let Some(path) =
        std::env::var_os("AGENTX_OPENSANDBOX_TLS_CA_PATH").filter(|value| !value.is_empty())
    {
        let pem = std::fs::read(path).map_err(|_| RuntimeError::Unavailable)?;
        let certificates = CertificateDer::pem_slice_iter(&pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| RuntimeError::Unavailable)?;
        if certificates.is_empty() {
            return Err(RuntimeError::Unavailable);
        }
        for certificate in certificates {
            roots
                .add(certificate)
                .map_err(|_| RuntimeError::Unavailable)?;
        }
    }
    agentx_service_kit::install_tls_provider();
    let connector = Connector::Rustls(std::sync::Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ));
    connect_async_tls_with_config(request, None, false, Some(connector))
        .await
        .map(|value| value.0)
        .map_err(|error| {
            tracing::warn!(%error, "OpenSandbox PTY WebSocket connection failed");
            RuntimeError::Unavailable
        })
}

async fn wait_connected(socket: &mut ProviderSocket) -> RuntimeResult<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(message) = socket.next().await {
            match message.map_err(|_| RuntimeError::Unavailable)? {
                Message::Text(text)
                    if serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|value| {
                            value.get("type").and_then(Value::as_str).map(str::to_owned)
                        })
                        .as_deref()
                        == Some("connected") =>
                {
                    return Ok(());
                }
                Message::Text(text) if text.contains("error") => {
                    return Err(RuntimeError::ProviderRejected);
                }
                _ => {}
            }
        }
        Err(RuntimeError::Unavailable)
    })
    .await
    .map_err(|_| RuntimeError::Unavailable)?
}

async fn wait_connected_provider(socket: &mut ProviderSocket) -> Result<(), ProviderError> {
    wait_connected(socket).await.map_err(|error| ProviderError {
        message: format!("{error:?}"),
        outcome_unknown: true,
    })
}

#[derive(Clone)]
struct CollectedFrame {
    stream: String,
    payload: Value,
}

async fn collect_socket(
    state: &SandboxManagerState,
    process_session_id: Uuid,
    socket: &mut ProviderSocket,
    mut offset: u64,
    deadline: time::OffsetDateTime,
    wait_millis: u32,
) -> RuntimeResult<Vec<CollectedFrame>> {
    let remaining = (deadline - time::OffsetDateTime::now_utc())
        .whole_milliseconds()
        .max(1) as u64;
    let wait = remaining.min(u64::from(wait_millis.max(1))).min(30_000);
    let mut wrapper = Vec::new();
    let mut collected = Vec::new();
    let mut committed_offset = offset;
    if let Ok(result) = tokio::time::timeout(Duration::from_millis(wait), async {
        while let Some(message) = socket.next().await {
            match message.map_err(|_| RuntimeError::Unavailable)? {
                Message::Binary(data) if !data.is_empty() => {
                    let bytes = match data[0] {
                        1 | 2 => &data[1..],
                        3 if data.len() >= 9 => {
                            offset = u64::from_be_bytes(data[1..9].try_into().unwrap_or_default());
                            &data[9..]
                        }
                        _ => continue,
                    };
                    offset = offset.saturating_add(bytes.len() as u64);
                    wrapper.extend_from_slice(bytes);
                    while let Some(position) = wrapper.iter().position(|byte| *byte == b'\n') {
                        let line = wrapper.drain(..=position).collect::<Vec<_>>();
                        let line = &line[..line.len().saturating_sub(1)];
                        if line.is_empty() { continue; }
                        let outer: Value = serde_json::from_slice(line).map_err(|_| RuntimeError::ProviderRejected)?;
                        let stream = outer.get("stream").and_then(Value::as_str).unwrap_or("stderr").to_owned();
                        let decoded = STANDARD.decode(outer.get("data").and_then(Value::as_str).unwrap_or_default()).map_err(|_| RuntimeError::ProviderRejected)?;
                        if stream == "exit" {
                            let code = String::from_utf8_lossy(&decoded).trim().parse::<i64>().ok();
                            sqlx::query("UPDATE sandbox_process_sessions SET status='exited',exit_code=? WHERE process_session_id=?")
                                .bind(code).bind(process_session_id).execute(&state.pool).await?;
                            continue;
                        }
                        let payload = if stream == "stdout" {
                            parse_stdout_json_rpc_frame(&decoded)?
                        } else {
                            json!({"text":String::from_utf8_lossy(&decoded),"diagnostic":true})
                        };
                        let frame = CollectedFrame { stream, payload };
                        persist_frame(state, process_session_id, &frame).await?;
                        collected.push(frame);
                        committed_offset = offset.saturating_sub(wrapper.len() as u64);
                    }
                }
                Message::Text(text) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text)
                        && value.get("type").and_then(Value::as_str) == Some("exit")
                    {
                        let code = value.get("exit_code").or_else(|| value.get("exitCode")).and_then(Value::as_i64);
                        sqlx::query("UPDATE sandbox_process_sessions SET status='exited',exit_code=? WHERE process_session_id=?")
                            .bind(code).bind(process_session_id).execute(&state.pool).await?;
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok::<(), RuntimeError>(())
    }).await {
        result?;
    }
    sqlx::query("UPDATE sandbox_process_sessions SET provider_output_offset=GREATEST(provider_output_offset,?),lease_expires_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND) WHERE process_session_id=?")
        .bind(committed_offset).bind(LEASE_SECONDS).bind(process_session_id).execute(&state.pool).await?;
    Ok(collected)
}

async fn persist_frame(
    state: &SandboxManagerState,
    process_session_id: Uuid,
    frame: &CollectedFrame,
) -> RuntimeResult<()> {
    let encoded = serde_json::to_vec(&frame.payload).unwrap_or_default();
    let truncated = encoded.len() > MAX_INLINE_FRAME;
    let mut tx = state.pool.begin().await?;
    let session = sqlx::query(
        "SELECT tenant_id,agent_run_id FROM sandbox_process_sessions WHERE process_session_id=? FOR UPDATE",
    )
    .bind(process_session_id)
    .fetch_one(&mut *tx)
    .await?;
    let sequence = sqlx::query_scalar::<_, u64>(
        "SELECT CAST(COALESCE(MAX(sequence),0)+1 AS UNSIGNED) FROM sandbox_process_frames WHERE process_session_id=?",
    )
    .bind(process_session_id)
    .fetch_one(&mut *tx)
    .await?;
    let tenant_id: Uuid = session.try_get("tenant_id")?;
    let agent_run_id: Uuid = session.try_get("agent_run_id")?;
    let artifact = if truncated {
        Some(
            persist_process_artifact(
                state,
                &mut tx,
                tenant_id,
                process_session_id,
                agent_run_id,
                sequence,
                &frame.stream,
                encoded,
            )
            .await?,
        )
    } else {
        None
    };
    let payload = artifact.as_ref().map_or_else(
        || frame.payload.clone(),
        |artifact| {
            json!({
                "jsonrpc":frame.payload.get("jsonrpc").cloned().unwrap_or_else(|| json!("2.0")),
                "id":frame.payload.get("id").cloned().unwrap_or(Value::Null),
                "truncated":true,
                "sizeBytes":artifact.size_bytes,
                "artifactRefs":[artifact.object_id.to_string()]
            })
        },
    );
    sqlx::query("INSERT INTO sandbox_process_frames(process_session_id,sequence,stream,payload_json,artifact_id,truncated) VALUES(?,?,?,?,?,?)")
        .bind(process_session_id).bind(sequence).bind(&frame.stream).bind(payload)
        .bind(artifact.as_ref().map(|value| value.object_id)).bind(truncated)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn persist_process_artifact(
    state: &SandboxManagerState,
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant_id: Uuid,
    process_session_id: Uuid,
    agent_run_id: Uuid,
    sequence: u64,
    stream: &str,
    encoded: Vec<u8>,
) -> RuntimeResult<RuntimeObjectReferenceV1> {
    let content_hash = ContentHash::parse(format!("sha256:{:x}", Sha256::digest(&encoded)))
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let object_id = stable_id(
        process_session_id,
        format!(
            "process-frame:{sequence}:{stream}:{}",
            content_hash.as_str()
        )
        .as_bytes(),
    );
    let object_key = RuntimeObjectReferenceV1::canonical_key(tenant_id, object_id, &content_hash);
    let path = ObjectPath::from(object_key.clone());
    state
        .objects
        .put(&path, bytes::Bytes::from(encoded.clone()).into())
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let request_hash = agentx_runtime_contracts::content_hash(&json!({
        "processSessionId":process_session_id,
        "sequence":sequence,
        "stream":stream,
        "contentHash":content_hash,
        "sizeBytes":encoded.len(),
    }))
    .map_err(|error| RuntimeError::Internal(error.into()))?;
    if let Err(error) = sqlx::query(
        "INSERT INTO runtime_objects(object_id,tenant_id,object_key,content_hash,size_bytes,media_type,status,idempotency_key,request_hash,temporary_expires_at,ready_at) VALUES(?,?,?,?,?,'application/json','ready',?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 1 DAY),UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE object_id=object_id",
    )
    .bind(object_id)
    .bind(tenant_id)
    .bind(&object_key)
    .bind(content_hash.as_str())
    .bind(encoded.len() as u64)
    .bind(format!("process-frame:{process_session_id}:{sequence}"))
    .bind(request_hash.as_str())
    .execute(&mut **tx)
    .await
    {
        let _ = state.objects.delete(&path).await;
        return Err(error.into());
    }
    sqlx::query(
        "INSERT IGNORE INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'agent_run',?,'mcp_process_frame')",
    )
    .bind(tenant_id)
    .bind(object_id)
    .bind(agent_run_id.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(RuntimeObjectReferenceV1 {
        tenant_id,
        storage_domain: StorageDomain::Runtime,
        object_id,
        object_key,
        content_hash,
        size_bytes: encoded.len() as u64,
        media_type: "application/json".into(),
    })
}

async fn maximum_sequence(state: &SandboxManagerState, id: Uuid) -> RuntimeResult<u64> {
    Ok(sqlx::query_scalar::<_, u64>("SELECT CAST(COALESCE(MAX(sequence),0) AS UNSIGNED) FROM sandbox_process_frames WHERE process_session_id=?")
        .bind(id).fetch_one(&state.pool).await?)
}

async fn response_after(
    state: &SandboxManagerState,
    id: Uuid,
    after: u64,
    maximum: u32,
) -> RuntimeResult<ProcessSessionFramesResponseV1> {
    let status: String = sqlx::query_scalar(
        "SELECT status FROM sandbox_process_sessions WHERE process_session_id=?",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    let rows = sqlx::query("SELECT sequence,stream,payload_json,artifact_id,truncated,created_at FROM sandbox_process_frames WHERE process_session_id=? AND sequence>? ORDER BY sequence LIMIT ?")
        .bind(id).bind(after).bind(maximum.max(1)).fetch_all(&state.pool).await?;
    let frames = rows
        .into_iter()
        .map(|row| {
            Ok(ProcessSessionFrameV1 {
                sequence: row.try_get("sequence")?,
                stream: row.try_get("stream")?,
                payload: row.try_get("payload_json")?,
                artifact_id: row.try_get("artifact_id")?,
                truncated: row.try_get("truncated")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let next_sequence = frames.last().map(|frame| frame.sequence).unwrap_or(after);
    Ok(ProcessSessionFramesResponseV1 {
        api_version: 1,
        process_session_id: id,
        status: parse_status(&status)?,
        frames,
        next_sequence,
    })
}

async fn reconcile_output(
    state: &SandboxManagerState,
    id: Uuid,
    proof: &ProcessSessionProofV1,
    wait_millis: u32,
) -> RuntimeResult<()> {
    let row = load_process(state, id, proof).await?;
    let sandbox_id: String = row
        .try_get::<Option<String>, _>("sandbox_id")?
        .ok_or(RuntimeError::Unavailable)?;
    let provider_process_id: String = row
        .try_get::<Option<String>, _>("provider_operation_id")?
        .ok_or(RuntimeError::Unavailable)?;
    let offset: u64 = row.try_get("provider_output_offset")?;
    let socket =
        active_process_socket(state, id, &sandbox_id, &provider_process_id, offset).await?;
    let mut socket = socket.lock().await;
    collect_socket(state, id, &mut socket, offset, proof.deadline, wait_millis).await?;
    Ok(())
}

async fn control(
    state: &SandboxManagerState,
    id: Uuid,
    proof: &ProcessSessionProofV1,
    terminate: bool,
) -> RuntimeResult<ProcessSessionControlResponseV1> {
    validate_proof(state, id, proof).await?;
    let row = load_process(state, id, proof).await?;
    let sandbox_id: String = row
        .try_get::<Option<String>, _>("sandbox_id")?
        .ok_or(RuntimeError::Unavailable)?;
    let provider_process_id: String = row
        .try_get::<Option<String>, _>("provider_operation_id")?
        .ok_or(RuntimeError::Unavailable)?;
    if terminate {
        sqlx::query(
            "UPDATE sandbox_process_sessions SET status='terminating' WHERE process_session_id=?",
        )
        .bind(id)
        .execute(&state.pool)
        .await?;
        if let Some(socket) = state.process_sockets.remove(id).await {
            let mut socket = socket.lock().await;
            close_process_socket(&mut socket).await;
        }
        if let Err(error) = delete_provider_process(state, &sandbox_id, &provider_process_id).await
        {
            tracing::warn!(error = %error.message, process_session_id = %id, "OpenSandbox PTY deletion required Sandbox termination");
        }
        terminate_provider(state, &sandbox_id, &proof.idempotency_key)
            .await
            .map_err(|_| RuntimeError::Unavailable)?;
        sqlx::query("UPDATE sandbox_process_sessions SET status='exited',exit_code=COALESCE(exit_code,143) WHERE process_session_id=?")
            .bind(id).execute(&state.pool).await?;
        Ok(ProcessSessionControlResponseV1 {
            api_version: 1,
            process_session_id: id,
            status: ProcessSessionStatusV1::Exited,
            exit_code: Some(143),
        })
    } else {
        sqlx::query(
            "UPDATE sandbox_process_sessions SET status='interrupting' WHERE process_session_id=?",
        )
        .bind(id)
        .execute(&state.pool)
        .await?;
        let offset: u64 = row.try_get("provider_output_offset")?;
        let socket =
            active_process_socket(state, id, &sandbox_id, &provider_process_id, offset).await?;
        let mut socket = socket.lock().await;
        socket
            .send(Message::Text(
                json!({"type":"signal","signal":"SIGINT"})
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|_| RuntimeError::Unavailable)?;
        sqlx::query("UPDATE sandbox_process_sessions SET status='running' WHERE process_session_id=? AND status='interrupting'")
            .bind(id).execute(&state.pool).await?;
        Ok(ProcessSessionControlResponseV1 {
            api_version: 1,
            process_session_id: id,
            status: ProcessSessionStatusV1::Running,
            exit_code: None,
        })
    }
}

async fn delete_provider_process(
    state: &SandboxManagerState,
    sandbox_id: &str,
    provider_process_id: &str,
) -> Result<(), ProviderError> {
    let endpoint = provider_execd_endpoint(state, sandbox_id).await?;
    let response = state
        .client
        .delete(
            endpoint
                .0
                .join(&format!("pty/{provider_process_id}"))
                .map_err(provider_url_error)?,
        )
        .headers(endpoint.1)
        .send()
        .await
        .map_err(provider_request_error)?;
    if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
        return Err(ProviderError {
            message: format!("OpenSandbox PTY delete HTTP {}", response.status()),
            outcome_unknown: false,
        });
    }
    Ok(())
}

fn process_profile_ttl(profile: &RuntimeResourceBindingV1) -> u32 {
    match profile.configuration {
        RuntimeResourceConfigurationV1::SandboxProfile {
            maximum_ttl_seconds,
            ..
        } => maximum_ttl_seconds,
        _ => 0,
    }
}

fn process_profile_limits(
    profile: &RuntimeResourceBindingV1,
) -> RuntimeResult<(String, u32, u64, u64, u32, SandboxEgressModeV1)> {
    let RuntimeResourceConfigurationV1::SandboxProfile {
        ref image,
        cpu_millis,
        memory_bytes,
        disk_bytes,
        pid_limit,
        egress_mode,
        ..
    } = profile.configuration
    else {
        return Err(bad_request("Process Session profile is invalid"));
    };
    Ok((
        image.clone(),
        cpu_millis,
        memory_bytes,
        disk_bytes,
        pid_limit,
        egress_mode,
    ))
}

fn start_response(
    request: &ProcessSessionStartRequestV1,
    process_session_id: Uuid,
    lease_id: Uuid,
    provider_sandbox_id: Option<String>,
    fencing_token: u64,
    expires_at: time::OffsetDateTime,
) -> ProcessSessionStartResponseV1 {
    ProcessSessionStartResponseV1 {
        api_version: 1,
        identity: request.identity.clone(),
        lease: ProcessSessionLeaseV1 {
            process_session_id,
            lease_id,
            worker_id: request.worker_id,
            attempt_id: request.attempt_id,
            fencing_token,
            expires_at,
            status: ProcessSessionStatusV1::Running,
        },
        provider_sandbox_id,
    }
}

fn parse_status(status: &str) -> RuntimeResult<ProcessSessionStatusV1> {
    Ok(match status {
        "acquiring" => ProcessSessionStatusV1::Acquiring,
        "starting" => ProcessSessionStatusV1::Starting,
        "running" => ProcessSessionStatusV1::Running,
        "interrupting" => ProcessSessionStatusV1::Interrupting,
        "terminating" => ProcessSessionStatusV1::Terminating,
        "exited" => ProcessSessionStatusV1::Exited,
        "failed" => ProcessSessionStatusV1::Failed,
        "unknown_outcome" => ProcessSessionStatusV1::UnknownOutcome,
        "expired" => ProcessSessionStatusV1::Expired,
        _ => return Err(RuntimeError::Unavailable),
    })
}

fn provider_url_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError {
        message: error.to_string(),
        outcome_unknown: false,
    }
}
fn provider_request_error(error: reqwest::Error) -> ProviderError {
    ProviderError {
        message: error.to_string(),
        outcome_unknown: !error.is_connect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start_request() -> ProcessSessionStartRequestV1 {
        let profile_version = Uuid::from_u128(4);
        let configuration = RuntimeResourceConfigurationV1::SandboxProfile {
            provider: "opensandbox".into(),
            image: "mcp-fixture:latest".into(),
            cpu_millis: 500,
            memory_bytes: 256 * 1024 * 1024,
            disk_bytes: 1024 * 1024 * 1024,
            pid_limit: 64,
            egress_mode: SandboxEgressModeV1::None,
            maximum_ttl_seconds: 300,
        };
        ProcessSessionStartRequestV1 {
            api_version: 1,
            identity: agentx_runtime_contracts::ProcessSessionIdentityV1 {
                tenant_id: Uuid::from_u128(1),
                agent_run_id: Uuid::from_u128(2),
                mcp_server_version_id: Uuid::from_u128(3),
                sandbox_profile_version_id: profile_version,
            },
            execution_id: Uuid::from_u128(5),
            node_execution_id: Uuid::from_u128(6),
            attempt_id: Uuid::from_u128(7),
            worker_id: Uuid::from_u128(8),
            fencing_token: 1,
            operation_id: "start-operation".into(),
            effect_id: "start-effect".into(),
            idempotency_key: "start-idempotency".into(),
            command: "/usr/local/bin/mcp-server".into(),
            args: vec!["--stdio".into()],
            environment_credentials: vec![],
            profile: RuntimeResourceBindingV1 {
                resource_kind: agentx_runtime_contracts::RuntimeResourceKindV1::SandboxProfile,
                resource_id: Uuid::from_u128(9),
                resource_version: profile_version.to_string(),
                state_epoch: 1,
                content_hash: agentx_runtime_contracts::content_hash(&configuration)
                    .expect("profile hash"),
                configuration,
                object_ids: vec![],
            },
            deadline: time::OffsetDateTime::now_utc() + time::Duration::minutes(1),
        }
    }

    #[test]
    fn process_start_rejects_shell_strings_and_duplicate_secret_names() {
        let mut request = start_request();
        assert!(validate_start(&request).is_ok());
        request.command = "node mcp-server.js".into();
        assert!(validate_start(&request).is_err());

        request = start_request();
        let secret = agentx_runtime_contracts::VaultSecretReferenceV1 {
            mount: "secret".into(),
            path: "tenants/t/credentials/c".into(),
            key: "value".into(),
            version: 1,
        };
        request.environment_credentials = vec![
            agentx_runtime_contracts::ProcessEnvironmentCredentialV1 {
                name: "TOKEN".into(),
                credential: secret.clone(),
            },
            agentx_runtime_contracts::ProcessEnvironmentCredentialV1 {
                name: "TOKEN".into(),
                credential: secret,
            },
        ];
        assert!(validate_start(&request).is_err());
    }

    #[test]
    fn stdio_frames_are_single_line_bounded_json_rpc_objects() {
        assert!(
            encode_json_rpc_frame(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).is_ok()
        );
        assert!(encode_json_rpc_frame(&json!(["not-an-envelope"])).is_err());
        assert!(encode_json_rpc_frame(&json!({"id":1,"method":"tools/list"})).is_err());
        assert!(
            parse_stdout_json_rpc_frame(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n").is_ok()
        );
        assert!(
            parse_stdout_json_rpc_frame(b"{\"jsonrpc\":\"2.0\"}\n{\"jsonrpc\":\"2.0\"}").is_err()
        );
        assert!(parse_stdout_json_rpc_frame(b"not-json").is_err());
        let oversized = vec![b'x'; MAX_JSON_RPC_FRAME + 1];
        assert!(parse_stdout_json_rpc_frame(&oversized).is_err());
    }

    #[test]
    fn launcher_emits_newline_delimited_wrapper_frames() {
        let command = launcher_command();
        let encoded = command
            .strip_prefix("python3 -c \"import base64;exec(base64.b64decode('")
            .and_then(|value| value.strip_suffix("'))\""))
            .expect("launcher command must keep the fixed base64 wrapper");
        let source = STANDARD.decode(encoded).expect("launcher source is base64");

        assert!(
            source.windows(4).any(|window| window == b"'\\n'"),
            "launcher must write an actual newline delimiter"
        );
        assert!(
            !source.windows(5).any(|window| window == b"'\\\\n'"),
            "launcher must not write a literal backslash-n delimiter"
        );
    }

    #[test]
    fn every_persisted_process_status_has_a_frozen_domain_value() {
        for status in [
            "acquiring",
            "starting",
            "running",
            "interrupting",
            "terminating",
            "exited",
            "failed",
            "unknown_outcome",
            "expired",
        ] {
            assert!(parse_status(status).is_ok(), "missing status {status}");
        }
        assert!(parse_status("orphaned").is_err());
    }
}
