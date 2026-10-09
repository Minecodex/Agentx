use std::{collections::BTreeMap, future::Future, sync::Arc};

use agentx_runtime_contracts::{
    EGRESS_SANDBOX_TOKEN_MAX_TTL_SECONDS, EGRESS_TOKEN_AUDIENCE, EGRESS_TOKEN_ISSUER,
    EgressConnectClaimsV1, EgressDestinationV1, EgressMode, EgressRole, RuntimeResourceBindingV1,
    RuntimeResourceConfigurationV1, SandboxEgressModeV1, ToolEffectRequestV1, ToolEffectResponseV1,
    WorkspaceAcquireRequestV1, WorkspaceAcquireResponseV1, WorkspaceLeaseStatusV1,
    WorkspaceReleaseRequestV1, content_hash, issue_egress_connect_token, now_unix,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use object_store::ObjectStore;
use reqwest::{StatusCode, header::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, Row};
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

const LEASE_SECONDS: u32 = 30;
const EXECD_PORT: u16 = 44_772;
const SANDBOX_EGRESS_CA_PATH: &str = "/tmp/agentx-egress-ca.pem";
const SANDBOX_EGRESS_CA_MODE: u16 = 600;
const SANDBOX_EGRESS_CA_CONTENT_TYPE: &str = "application/octet-stream";
const SANDBOX_EGRESS_CA_METADATA_FILENAME: &str = "metadata.json";

#[path = "sandbox_process.rs"]
mod process;
#[path = "sandbox_workspace.rs"]
mod workspace;

#[derive(Clone)]
pub struct SandboxManagerState {
    pub pool: MySqlPool,
    pub client: reqwest::Client,
    pub provider_endpoint: String,
    pub provider_api_key: Option<String>,
    pub provider_secure_access: bool,
    pub owner: Uuid,
    pub vault: Option<crate::vault::RuntimeVault>,
    pub objects: Arc<dyn ObjectStore>,
    pub process_sockets: process::ProcessSocketRegistry,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SandboxExecuteRequestV1 {
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub execution_id: Uuid,
    pub node_execution_id: Uuid,
    pub attempt_id: Uuid,
    pub worker_id: Uuid,
    pub fencing_token: u64,
    pub idempotency_key: String,
    pub profile: RuntimeResourceBindingV1,
    pub input: Value,
    pub parameters: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SandboxExecuteResponseV1 {
    pub api_version: u32,
    pub lease_id: Uuid,
    pub sandbox_id: Option<String>,
    pub replayed: bool,
    pub output: Value,
}

pub fn router(state: SandboxManagerState) -> Router {
    Router::new()
        .route(
            "/internal/runtime/v1/sandboxes:acquire",
            post(workspace::acquire_workspace),
        )
        .route(
            "/internal/runtime/v1/sandboxes:tool",
            post(workspace::workspace_tool),
        )
        .route(
            "/internal/runtime/v1/sandboxes:release",
            post(workspace::release_workspace),
        )
        .route("/internal/runtime/v1/sandboxes:execute", post(execute))
        .route(
            "/internal/runtime/v1/sandbox-process-sessions:start",
            post(process::start),
        )
        .route(
            "/internal/runtime/v1/sandbox-process-sessions/{id_action}",
            post(process_session_action),
        )
        .with_state(state)
}

async fn process_session_action(
    State(state): State<SandboxManagerState>,
    Path(id_action): Path<String>,
    body: Bytes,
) -> RuntimeResult<Response> {
    let (id, action) = id_action
        .rsplit_once(':')
        .ok_or_else(|| bad_request("invalid Process Session action path"))?;
    let id = Uuid::parse_str(id).map_err(|_| bad_request("invalid Process Session id"))?;
    match action {
        "write" => {
            let request = serde_json::from_slice(&body).map_err(|error| {
                bad_request(&format!("invalid Process Session request: {error}"))
            })?;
            Ok(process::write(State(state), Path(id), Json(request))
                .await?
                .into_response())
        }
        "read" => {
            let request = serde_json::from_slice(&body).map_err(|error| {
                bad_request(&format!("invalid Process Session request: {error}"))
            })?;
            Ok(process::read(State(state), Path(id), Json(request))
                .await?
                .into_response())
        }
        "wait" => {
            let request = serde_json::from_slice(&body).map_err(|error| {
                bad_request(&format!("invalid Process Session request: {error}"))
            })?;
            Ok(process::wait(State(state), Path(id), Json(request))
                .await?
                .into_response())
        }
        "interrupt" => {
            let request = serde_json::from_slice(&body).map_err(|error| {
                bad_request(&format!("invalid Process Session request: {error}"))
            })?;
            Ok(process::interrupt(State(state), Path(id), Json(request))
                .await?
                .into_response())
        }
        "terminate" => {
            let request = serde_json::from_slice(&body).map_err(|error| {
                bad_request(&format!("invalid Process Session request: {error}"))
            })?;
            Ok(process::terminate(State(state), Path(id), Json(request))
                .await?
                .into_response())
        }
        "reconcile" => {
            let request = serde_json::from_slice(&body).map_err(|error| {
                bad_request(&format!("invalid Process Session request: {error}"))
            })?;
            Ok(process::reconcile(State(state), Path(id), Json(request))
                .await?
                .into_response())
        }
        _ => Err(bad_request("unsupported Process Session action")),
    }
}

async fn execute(
    State(state): State<SandboxManagerState>,
    Json(request): Json<SandboxExecuteRequestV1>,
) -> RuntimeResult<Json<SandboxExecuteResponseV1>> {
    if request.api_version != 1 || request.idempotency_key.trim().is_empty() {
        return Err(bad_request("invalid Sandbox Manager request"));
    }
    let RuntimeResourceConfigurationV1::SandboxProfile {
        maximum_ttl_seconds,
        ..
    } = &request.profile.configuration
    else {
        return Err(bad_request(
            "Sandbox request requires an immutable Sandbox Profile",
        ));
    };
    let attempt_valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM node_attempts WHERE tenant_id=? AND execution_id=? AND node_execution_id=? AND id=? AND status='running' AND worker_instance_id=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6))",
    )
    .bind(request.tenant_id)
    .bind(request.execution_id)
    .bind(request.node_execution_id)
    .bind(request.attempt_id)
    .bind(request.worker_id.to_string())
    .bind(request.fencing_token)
    .fetch_one(&state.pool)
    .await?;
    if !attempt_valid {
        return Err(conflict("Sandbox Attempt Lease was lost"));
    }
    let lease_id = stable_id(request.attempt_id, b"sandbox-lease");
    let ttl = (*maximum_ttl_seconds).clamp(1, 86_400);
    let labels = json!({
        "agentxLeaseId": lease_id,
        "agentxTenantId": request.tenant_id,
        "agentxExecutionId": request.execution_id,
        "agentxAttemptId": request.attempt_id,
        "agentxFencingToken": request.fencing_token,
    });
    let request_hash = sandbox_request_hash(&request)?;
    let operation = loop {
        let mut tx = state.pool.begin().await?;
        let existing = sqlx::query(
            "SELECT id,status,request_hash,sandbox_id,result_json,locked_until,fencing_token,locked_until>UTC_TIMESTAMP(6) lease_active FROM sandbox_leases WHERE tenant_id=? AND idempotency_key=? FOR UPDATE",
        )
        .bind(request.tenant_id)
        .bind(&request.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = existing {
            if row.try_get::<String, _>("request_hash")? != request_hash.as_str() {
                return Err(conflict(
                    "Sandbox idempotency key was reused with a different request",
                ));
            }
            let status: String = row.try_get("status")?;
            if status == "terminated" {
                let response = sandbox_response(&row, true)?;
                tx.commit().await?;
                return Ok(Json(response));
            }
            if status == "failed" {
                return Err(RuntimeError::Unavailable);
            }
            if row.try_get::<bool, _>("lease_active")? {
                tx.commit().await?;
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            }
            ensure_attempt_lease(&mut tx, &request).await?;
            let fencing_token = row.try_get::<u64, _>("fencing_token")? + 1;
            let worker_lease_token =
                stable_id(request.attempt_id, &request.fencing_token.to_be_bytes());
            let changed = sqlx::query(
                "UPDATE sandbox_leases SET worker_lease_token=?,lease_token_hash=?,provider_labels_json=?,locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),heartbeat_at=UTC_TIMESTAMP(6),fencing_token=? WHERE id=? AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6))",
            )
            .bind(worker_lease_token)
            .bind(raw_hash(worker_lease_token.as_bytes()))
            .bind(&labels)
            .bind(state.owner)
            .bind(LEASE_SECONDS)
            .bind(fencing_token)
            .bind(lease_id)
            .execute(&mut *tx)
            .await?;
            if changed.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }
            let operation = SandboxOperation {
                lease_id,
                fencing_token,
                sandbox_id: row.try_get("sandbox_id")?,
                result: row.try_get("result_json")?,
                replayed: true,
            };
            tx.commit().await?;
            break operation;
        }
        let worker_lease_token =
            stable_id(request.attempt_id, &request.fencing_token.to_be_bytes());
        sqlx::query(
            "INSERT INTO sandbox_leases(id,tenant_id,execution_id,node_execution_id,attempt_id,worker_lease_token,lease_token_hash,profile_version_id,idempotency_key,status,provider_labels_json,request_hash,expires_at,locked_by,locked_until,fencing_token) VALUES(?,?,?,?,?,?,?,?,?,'creating',?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),1)",
        )
        .bind(lease_id)
        .bind(request.tenant_id)
        .bind(request.execution_id)
        .bind(request.node_execution_id)
        .bind(request.attempt_id)
        .bind(worker_lease_token)
        .bind(raw_hash(worker_lease_token.as_bytes()))
        .bind(request.profile.resource_id)
        .bind(&request.idempotency_key)
        .bind(&labels)
        .bind(request_hash.as_str())
        .bind(ttl)
        .bind(state.owner)
        .bind(LEASE_SECONDS)
        .execute(&mut *tx)
        .await?;
        let mut trace = crate::trace_delivery::TraceDraft::span(
            request.tenant_id,
            request.execution_id,
            lease_id,
            Some((
                request.attempt_id,
                agentx_runtime_contracts::TraceSpanKindV1::Attempt,
            )),
            agentx_runtime_contracts::TraceSpanKindV1::Sandbox,
            "OpenSandbox execution",
            agentx_runtime_contracts::TraceEventKindV1::Started,
            "sandbox.started",
            "creating",
        );
        trace.node_execution_id = Some(request.node_execution_id);
        trace.attempt_id = Some(request.attempt_id);
        trace.sandbox_lease_id = Some(lease_id);
        trace.resource_type = Some("sandbox_profile".into());
        trace.resource_id = Some(request.profile.resource_id);
        trace.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::SandboxRequest);
        trace.content_preview = crate::trace_delivery::bounded_preview(&request.input);
        crate::trace_delivery::enqueue_best_effort(&mut tx, trace).await;
        tx.commit().await?;
        break SandboxOperation {
            lease_id,
            fencing_token: 1,
            sandbox_id: None,
            result: None,
            replayed: false,
        };
    };

    run_sandbox_operation(&state, &request, operation, ttl)
        .await
        .map(Json)
}

#[derive(Debug)]
struct SandboxOperation {
    lease_id: Uuid,
    fencing_token: u64,
    sandbox_id: Option<String>,
    result: Option<Value>,
    replayed: bool,
}

async fn ensure_attempt_lease(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    request: &SandboxExecuteRequestV1,
) -> RuntimeResult<()> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM node_attempts WHERE tenant_id=? AND execution_id=? AND node_execution_id=? AND id=? AND status='running' AND worker_instance_id=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6))",
    )
    .bind(request.tenant_id)
    .bind(request.execution_id)
    .bind(request.node_execution_id)
    .bind(request.attempt_id)
    .bind(request.worker_id.to_string())
    .bind(request.fencing_token)
    .fetch_one(&mut **tx)
    .await?;
    if !valid {
        return Err(conflict("Sandbox Attempt Lease was lost"));
    }
    Ok(())
}

async fn run_sandbox_operation(
    state: &SandboxManagerState,
    request: &SandboxExecuteRequestV1,
    operation: SandboxOperation,
    ttl: u32,
) -> RuntimeResult<SandboxExecuteResponseV1> {
    if operation.result.is_some() {
        if let Some(sandbox_id) = operation.sandbox_id.as_deref() {
            let termination = with_lease_heartbeat(
                state,
                operation.lease_id,
                operation.fencing_token,
                terminate_provider(state, sandbox_id, &request.idempotency_key),
            )
            .await?;
            if let Err(error) = termination {
                mark_orphaned(
                    state,
                    operation.lease_id,
                    operation.fencing_token,
                    &error.message,
                )
                .await?;
                return Err(RuntimeError::Unavailable);
            }
        }
        complete_lease(
            state,
            operation.lease_id,
            operation.fencing_token,
            operation.sandbox_id.as_deref(),
        )
        .await?;
        return Ok(SandboxExecuteResponseV1 {
            api_version: 1,
            lease_id: operation.lease_id,
            sandbox_id: operation.sandbox_id,
            replayed: true,
            output: operation.result.unwrap_or(Value::Null),
        });
    }

    let (image, cpu_millis, memory_bytes, disk_bytes, pid_limit, profile_egress_mode) =
        match &request.profile.configuration {
            RuntimeResourceConfigurationV1::SandboxProfile {
                image,
                cpu_millis,
                memory_bytes,
                disk_bytes,
                pid_limit,
                egress_mode,
                ..
            } => (
                image,
                cpu_millis,
                memory_bytes,
                disk_bytes,
                pid_limit,
                egress_mode,
            ),
            _ => unreachable!(),
        };
    let (requested_egress_mode, _) = requested_egress_policy(&request.parameters)?;
    if requested_egress_mode == SandboxEgressModeV1::TcpProxy
        && *profile_egress_mode != SandboxEgressModeV1::TcpProxy
    {
        return Err(bad_request(
            "Code node TCP proxy exceeds the Sandbox Profile network capability",
        ));
    }
    let labels = json!({
        "agentxLeaseId": operation.lease_id,
        "agentxTenantId": request.tenant_id,
        "agentxExecutionId": request.execution_id,
        "agentxAttemptId": request.attempt_id,
        "agentxFencingToken": request.fencing_token,
    });
    let metadata = labels
        .as_object()
        .expect("Sandbox labels are constructed as an object")
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut sandbox_id = operation.sandbox_id.clone();
    let mut create_operation_id = None;
    if sandbox_id.is_none() {
        sandbox_id = find_provider_sandbox(state, operation.lease_id).await;
    }
    if sandbox_id.is_none() {
        let create = with_lease_heartbeat(
            state,
            operation.lease_id,
            operation.fencing_token,
            create_provider_sandbox(
                state,
                image,
                ttl,
                *cpu_millis,
                *memory_bytes,
                *disk_bytes,
                *pid_limit,
                requested_egress_mode,
                metadata,
                &request.idempotency_key,
            ),
        )
        .await?;
        let (create_payload, provider_operation_id) = match create {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    lease_id = %operation.lease_id,
                    outcome_unknown = error.outcome_unknown,
                    provider_error = %error.message,
                    "OpenSandbox create failed"
                );
                fail_lease(
                    state,
                    operation.lease_id,
                    operation.fencing_token,
                    &error.message,
                    error.outcome_unknown,
                )
                .await?;
                return Err(if error.outcome_unknown {
                    RuntimeError::Unavailable
                } else {
                    RuntimeError::ProviderRejected
                });
            }
        };
        sandbox_id = create_payload
            .get("sandboxId")
            .or_else(|| create_payload.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        create_operation_id = provider_operation_id;
    }
    let sandbox_id = sandbox_id.ok_or_else(|| {
        RuntimeError::Internal(anyhow::anyhow!(
            "OpenSandbox create/reconciliation has no sandbox id"
        ))
    })?;
    advance_lease(
        state,
        operation.lease_id,
        operation.fencing_token,
        "running",
        Some(&sandbox_id),
        create_operation_id.as_deref(),
    )
    .await?;
    advance_lease(
        state,
        operation.lease_id,
        operation.fencing_token,
        "interrupting",
        Some(&sandbox_id),
        None,
    )
    .await?;
    let execution = with_lease_heartbeat(
        state,
        operation.lease_id,
        operation.fencing_token,
        execute_provider_command(
            state,
            &sandbox_id,
            &request.parameters,
            &request.idempotency_key,
            request,
            requested_egress_mode,
            ttl,
        ),
    )
    .await?;
    let (result, execute_operation_id) = match execution {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                lease_id = %operation.lease_id,
                sandbox_id = %sandbox_id,
                outcome_unknown = error.outcome_unknown,
                provider_error = %error.message,
                "OpenSandbox command failed"
            );
            if error.outcome_unknown {
                mark_orphaned(
                    state,
                    operation.lease_id,
                    operation.fencing_token,
                    &error.message,
                )
                .await?;
                return Err(RuntimeError::Unavailable);
            }
            let _ = with_lease_heartbeat(
                state,
                operation.lease_id,
                operation.fencing_token,
                terminate_provider(state, &sandbox_id, &request.idempotency_key),
            )
            .await?;
            fail_lease(
                state,
                operation.lease_id,
                operation.fencing_token,
                &error.message,
                false,
            )
            .await?;
            return Err(RuntimeError::ProviderRejected);
        }
    };
    record_result(
        state,
        operation.lease_id,
        operation.fencing_token,
        &sandbox_id,
        execute_operation_id.or(create_operation_id).as_deref(),
        &result,
    )
    .await?;
    let termination = with_lease_heartbeat(
        state,
        operation.lease_id,
        operation.fencing_token,
        terminate_provider(state, &sandbox_id, &request.idempotency_key),
    )
    .await?;
    if let Err(error) = termination {
        mark_orphaned(
            state,
            operation.lease_id,
            operation.fencing_token,
            &error.message,
        )
        .await?;
        return Err(RuntimeError::Unavailable);
    }
    complete_lease(
        state,
        operation.lease_id,
        operation.fencing_token,
        Some(&sandbox_id),
    )
    .await?;
    Ok(SandboxExecuteResponseV1 {
        api_version: 1,
        lease_id: operation.lease_id,
        sandbox_id: Some(sandbox_id),
        replayed: operation.replayed,
        output: result,
    })
}

fn sandbox_response(
    row: &sqlx::mysql::MySqlRow,
    replayed: bool,
) -> RuntimeResult<SandboxExecuteResponseV1> {
    Ok(SandboxExecuteResponseV1 {
        api_version: 1,
        lease_id: row.try_get("id")?,
        sandbox_id: row.try_get("sandbox_id")?,
        replayed,
        output: row
            .try_get::<Option<Value>, _>("result_json")?
            .unwrap_or(Value::Null),
    })
}

async fn find_provider_sandbox(state: &SandboxManagerState, lease_id: Uuid) -> Option<String> {
    let list_url = format!(
        "{}/v1/sandboxes?metadata=agentxLeaseId%3D{}&pageSize=100",
        state.provider_endpoint.trim_end_matches('/'),
        lease_id
    );
    provider_json(authenticated(state, state.client.get(list_url)))
        .await
        .ok()
        .and_then(|(payload, _)| {
            payload
                .get("items")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("sandboxId").or_else(|| item.get("id")))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

async fn with_lease_heartbeat<F, T>(
    state: &SandboxManagerState,
    lease_id: Uuid,
    fencing_token: u64,
    operation: F,
) -> RuntimeResult<Result<T, ProviderError>>
where
    F: Future<Output = Result<T, ProviderError>>,
{
    tokio::pin!(operation);
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(10));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result = &mut operation => return Ok(result),
            _ = heartbeat.tick() => heartbeat_lease(state, lease_id, fencing_token).await?,
        }
    }
}

async fn heartbeat_lease(
    state: &SandboxManagerState,
    lease_id: Uuid,
    fencing_token: u64,
) -> RuntimeResult<()> {
    let changed = sqlx::query(
        "UPDATE sandbox_leases SET heartbeat_at=UTC_TIMESTAMP(6),locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND) WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(LEASE_SECONDS)
    .bind(lease_id)
    .bind(state.owner)
    .bind(fencing_token)
    .execute(&state.pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(conflict("Sandbox operation Lease was lost"));
    }
    Ok(())
}

async fn record_result(
    state: &SandboxManagerState,
    lease_id: Uuid,
    fencing_token: u64,
    sandbox_id: &str,
    provider_operation_id: Option<&str>,
    result: &Value,
) -> RuntimeResult<()> {
    let changed = sqlx::query(
        "UPDATE sandbox_leases SET status='terminating',sandbox_id=?,provider_operation_id=COALESCE(?,provider_operation_id),result_json=?,heartbeat_at=UTC_TIMESTAMP(6),locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND) WHERE id=? AND status='interrupting' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(sandbox_id)
    .bind(provider_operation_id)
    .bind(result)
    .bind(LEASE_SECONDS)
    .bind(lease_id)
    .bind(state.owner)
    .bind(fencing_token)
    .execute(&state.pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(conflict(
            "Sandbox Lease was lost before persisting the Provider result",
        ));
    }
    Ok(())
}

async fn complete_lease(
    state: &SandboxManagerState,
    lease_id: Uuid,
    fencing_token: u64,
    sandbox_id: Option<&str>,
) -> RuntimeResult<()> {
    let changed = sqlx::query(
        "UPDATE sandbox_leases SET status='terminated',sandbox_id=COALESCE(?,sandbox_id),terminated_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL,outcome_unknown=FALSE,last_error=NULL WHERE id=? AND status IN ('terminating','orphaned') AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(sandbox_id)
    .bind(lease_id)
    .bind(state.owner)
    .bind(fencing_token)
    .execute(&state.pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(conflict(
            "Sandbox Lease was lost before recording termination",
        ));
    }
    emit_sandbox_finished(&state.pool, lease_id, "succeeded", None).await;
    Ok(())
}

pub async fn reconcile_one(state: &SandboxManagerState) -> RuntimeResult<bool> {
    if process::reconcile_one(state).await? {
        return Ok(true);
    }
    if workspace::reconcile_one(state).await? {
        return Ok(true);
    }
    let mut tx = state.pool.begin().await?;
    let Some(row) = sqlx::query(
        "SELECT id,sandbox_id,fencing_token,idempotency_key,status,last_error FROM sandbox_leases WHERE (status='orphaned' OR (status IN ('ready','running','interrupting','terminating') AND expires_at<=UTC_TIMESTAMP(6))) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY expires_at,id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        tx.commit().await?;
        return Ok(false);
    };
    let lease_id: Uuid = row.try_get("id")?;
    let previous_status: String = row.try_get("status")?;
    let previous_error: Option<String> = row.try_get("last_error")?;
    let fencing_token = row.try_get::<u64, _>("fencing_token")? + 1;
    sqlx::query(
        "UPDATE sandbox_leases SET status='terminating',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),fencing_token=?,termination_attempts=termination_attempts+1 WHERE id=?",
    )
    .bind(state.owner)
    .bind(LEASE_SECONDS)
    .bind(fencing_token)
    .bind(lease_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut sandbox_id = row.try_get::<Option<String>, _>("sandbox_id")?;
    if sandbox_id.is_none() {
        let list_url = format!(
            "{}/v1/sandboxes?metadata=agentxLeaseId%3D{}&pageSize=100",
            state.provider_endpoint.trim_end_matches('/'),
            lease_id
        );
        if let Ok((payload, _)) = with_lease_heartbeat(
            state,
            lease_id,
            fencing_token,
            provider_json(authenticated(state, state.client.get(list_url))),
        )
        .await?
        {
            sandbox_id = payload
                .get("items")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("sandboxId").or_else(|| item.get("id")))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
    }
    let termination = if let Some(sandbox_id) = sandbox_id.as_deref() {
        with_lease_heartbeat(
            state,
            lease_id,
            fencing_token,
            terminate_provider(
                state,
                sandbox_id,
                &row.try_get::<String, _>("idempotency_key")?,
            ),
        )
        .await?
    } else {
        Ok(())
    };
    let termination_succeeded = termination.is_ok();
    let changed = match termination {
        Ok(()) => sqlx::query(
            "UPDATE sandbox_leases SET status='terminated',sandbox_id=COALESCE(sandbox_id,?),terminated_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL,outcome_unknown=FALSE WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
        )
        .bind(sandbox_id)
        .bind(lease_id)
        .bind(state.owner)
        .bind(fencing_token)
        .execute(&state.pool)
        .await?,
        Err(error) => sqlx::query(
            "UPDATE sandbox_leases SET status='orphaned',outcome_unknown=TRUE,last_error=?,locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
        )
        .bind(error.message.chars().take(1000).collect::<String>())
        .bind(lease_id)
        .bind(state.owner)
        .bind(fencing_token)
        .execute(&state.pool)
        .await?,
    };
    if changed.rows_affected() != 1 {
        return Err(conflict("Sandbox Reaper Lease was lost"));
    }
    let (trace_status, trace_error) = if !termination_succeeded {
        (
            "outcome_unknown",
            Some("Sandbox Reaper could not confirm termination"),
        )
    } else if previous_error.as_deref() == Some("execution_cancelled") {
        ("cancelled", Some("Execution cancelled the active Sandbox"))
    } else if matches!(previous_status.as_str(), "ready" | "running") {
        (
            "timed_out",
            Some("Sandbox lease exceeded its configured TTL"),
        )
    } else if previous_status == "orphaned" {
        (
            "outcome_unknown",
            previous_error
                .as_deref()
                .or(Some("Sandbox outcome remained unknown during cleanup")),
        )
    } else {
        (
            "outcome_unknown",
            previous_error
                .as_deref()
                .or(Some("Sandbox termination required Reaper reconciliation")),
        )
    };
    emit_sandbox_finished(&state.pool, lease_id, trace_status, trace_error).await;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
async fn create_provider_sandbox(
    state: &SandboxManagerState,
    image: &str,
    ttl_seconds: u32,
    cpu_millis: u32,
    memory_bytes: u64,
    disk_bytes: u64,
    pid_limit: u32,
    egress_mode: SandboxEgressModeV1,
    metadata: BTreeMap<String, String>,
    idempotency_key: &str,
) -> Result<(Value, Option<String>), ProviderError> {
    let proxy_target = if egress_mode == SandboxEgressModeV1::TcpProxy {
        let proxy = sandbox_proxy_url()?;
        let target = proxy.host_str().ok_or_else(|| ProviderError {
            message: "Sandbox egress proxy URL has no host".into(),
            outcome_unknown: false,
        })?;
        Some(target.to_owned())
    } else {
        None
    };
    let network_policy = opensandbox_network_policy(egress_mode, proxy_target.as_deref());
    let create_url = format!(
        "{}/v1/sandboxes",
        state.provider_endpoint.trim_end_matches('/')
    );
    let request = json!({
        "image":{"uri":image},
        "timeout":ttl_seconds.max(60).saturating_add(30).min(86_400),
        "resourceLimits":{
            "cpu":format!("{cpu_millis}m"),
            "memory":memory_bytes.to_string(),
            "ephemeral-storage":disk_bytes.to_string(),
            "pids":pid_limit.to_string()
        },
        "entrypoint":["tail","-f","/dev/null"],
        "metadata":metadata,
        "networkPolicy":network_policy,
        "secureAccess":state.provider_secure_access
    });
    let mut response = provider_json(
        authenticated(state, state.client.post(create_url))
            .header("Idempotency-Key", idempotency_key)
            .json(&request),
    )
    .await?;
    let sandbox_id = response
        .0
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError {
            message: "OpenSandbox create response has no id".into(),
            outcome_unknown: false,
        })?
        .to_owned();
    for _ in 0..120 {
        let state_name = response
            .0
            .pointer("/status/state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if state_name == "Running" {
            return Ok(response);
        }
        if state_name != "Pending" {
            return Err(ProviderError {
                message: format!("OpenSandbox entered state {state_name}: {}", response.0),
                outcome_unknown: false,
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        response = provider_json(authenticated(
            state,
            state.client.get(format!(
                "{}/v1/sandboxes/{sandbox_id}",
                state.provider_endpoint.trim_end_matches('/')
            )),
        ))
        .await?;
    }
    Err(ProviderError {
        message: "OpenSandbox did not become ready within 30 seconds".into(),
        outcome_unknown: true,
    })
}

fn opensandbox_network_policy(
    egress_mode: SandboxEgressModeV1,
    proxy_target: Option<&str>,
) -> Value {
    let egress = match (egress_mode, proxy_target) {
        (SandboxEgressModeV1::TcpProxy, Some(target)) => {
            vec![json!({"action":"allow","target":target})]
        }
        _ => Vec::new(),
    };
    json!({"defaultAction":"deny","egress":egress})
}

fn requested_egress_policy(
    parameters: &Value,
) -> RuntimeResult<(SandboxEgressModeV1, Vec<EgressDestinationV1>)> {
    let policy = parameters.get("networkPolicy").and_then(Value::as_object);
    let mode = policy
        .and_then(|policy| policy.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("deny");
    let destinations = policy
        .and_then(|policy| policy.get("destinations"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let destinations =
        serde_json::from_value::<Vec<EgressDestinationV1>>(destinations).map_err(|error| {
            bad_request(&format!(
                "Code node network destinations are invalid: {error}"
            ))
        })?;
    match mode {
        "deny" if destinations.is_empty() => Ok((SandboxEgressModeV1::None, destinations)),
        "allowlist" if !destinations.is_empty() => {
            Ok((SandboxEgressModeV1::TcpProxy, destinations))
        }
        "deny" => Err(bad_request(
            "Code node deny network policy cannot contain destinations",
        )),
        "allowlist" => Err(bad_request(
            "Code node allowlist network policy requires at least one destination",
        )),
        _ => Err(bad_request(
            "Code node network mode must be deny or allowlist",
        )),
    }
}

fn sandbox_proxy_url() -> Result<reqwest::Url, ProviderError> {
    let value = std::env::var("AGENTX_EGRESS_SANDBOX_PROXY_URL").map_err(|_| ProviderError {
        message: "AGENTX_EGRESS_SANDBOX_PROXY_URL is required for TCP proxy access".into(),
        outcome_unknown: false,
    })?;
    let url = reqwest::Url::parse(&value).map_err(|error| ProviderError {
        message: format!("invalid Sandbox egress proxy URL: {error}"),
        outcome_unknown: false,
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProviderError {
            message: "Sandbox egress proxy URL must be an HTTPS origin without userinfo".into(),
            outcome_unknown: false,
        });
    }
    Ok(url)
}

fn sandbox_proxy_environment(
    request: &SandboxExecuteRequestV1,
    ttl_seconds: u32,
) -> Result<(String, Option<Vec<u8>>), ProviderError> {
    let mut proxy = sandbox_proxy_url()?;
    let key_id = std::env::var("AGENTX_EGRESS_JWT_KEY_ID").map_err(|_| ProviderError {
        message: "AGENTX_EGRESS_JWT_KEY_ID is required".into(),
        outcome_unknown: false,
    })?;
    let private_key =
        std::env::var("AGENTX_EGRESS_JWT_PRIVATE_KEY_PEM").map_err(|_| ProviderError {
            message: "AGENTX_EGRESS_JWT_PRIVATE_KEY_PEM is required".into(),
            outcome_unknown: false,
        })?;
    let now = now_unix();
    let token_ttl = i64::from(ttl_seconds).clamp(1, EGRESS_SANDBOX_TOKEN_MAX_TTL_SECONDS);
    let (_, destinations) =
        requested_egress_policy(&request.parameters).map_err(|error| ProviderError {
            message: error.to_string(),
            outcome_unknown: false,
        })?;
    let policy_hash = content_hash(&destinations).map_err(|error| ProviderError {
        message: format!("failed hashing Sandbox network policy: {error}"),
        outcome_unknown: false,
    })?;
    let claims = EgressConnectClaimsV1 {
        iss: EGRESS_TOKEN_ISSUER.into(),
        aud: EGRESS_TOKEN_AUDIENCE.into(),
        role: EgressRole::Sandbox,
        tenant_id: request.tenant_id,
        execution_id: Some(request.execution_id),
        request_id: Some(request.attempt_id),
        egress_mode: EgressMode::TcpProxy,
        destinations,
        policy_hash,
        iat: now,
        exp: now + token_ttl,
        jti: Uuid::now_v7(),
    };
    let token =
        issue_egress_connect_token(&key_id, private_key.as_bytes(), &claims).map_err(|_| {
            ProviderError {
                message: "failed signing Sandbox egress token".into(),
                outcome_unknown: false,
            }
        })?;
    proxy.set_username("agentx").map_err(|_| ProviderError {
        message: "failed setting Sandbox proxy identity".into(),
        outcome_unknown: false,
    })?;
    proxy
        .set_password(Some(&token))
        .map_err(|_| ProviderError {
            message: "failed setting Sandbox proxy token".into(),
            outcome_unknown: false,
        })?;
    let ca = std::env::var_os("AGENTX_EGRESS_SANDBOX_CA_PATH")
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::fs::read(&path).map_err(|error| ProviderError {
                message: format!("failed reading Sandbox proxy CA: {error}"),
                outcome_unknown: false,
            })
        })
        .transpose()?;
    Ok((proxy.to_string(), ca))
}

async fn upload_sandbox_ca(
    state: &SandboxManagerState,
    endpoint: &reqwest::Url,
    headers: HeaderMap,
    ca: Vec<u8>,
) -> Result<(), ProviderError> {
    let upload_url = endpoint
        .join("files/upload")
        .map_err(|error| ProviderError {
            message: format!("invalid OpenSandbox file upload endpoint: {error}"),
            outcome_unknown: false,
        })?;
    let metadata = serde_json::to_string(&json!({
        "path":SANDBOX_EGRESS_CA_PATH,
        // Execd models Unix permissions as their octal digits (600), not the
        // decimal value of Rust's 0o600 literal (384).
        "mode":SANDBOX_EGRESS_CA_MODE
    }))
    .map_err(|error| ProviderError {
        message: error.to_string(),
        outcome_unknown: false,
    })?;
    let form = reqwest::multipart::Form::new()
        .part(
            "metadata",
            reqwest::multipart::Part::text(metadata)
                // Execd reads metadata from MultipartForm.File rather than
                // MultipartForm.Value, so this part must carry a filename.
                .file_name(SANDBOX_EGRESS_CA_METADATA_FILENAME)
                .mime_str("application/json")
                .map_err(|error| ProviderError {
                    message: error.to_string(),
                    outcome_unknown: false,
                })?,
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(ca)
                .file_name("agentx-egress-ca.pem")
                .mime_str(SANDBOX_EGRESS_CA_CONTENT_TYPE)
                .map_err(|error| ProviderError {
                    message: error.to_string(),
                    outcome_unknown: false,
                })?,
        );
    let response = state
        .client
        .post(upload_url)
        .headers(headers)
        .multipart(form)
        .send()
        .await
        .map_err(|error| ProviderError {
            message: error.to_string(),
            outcome_unknown: !error.is_connect(),
        })?;
    if !response.status().is_success() {
        return Err(ProviderError {
            message: format!("OpenSandbox CA upload returned HTTP {}", response.status()),
            outcome_unknown: false,
        });
    }
    Ok(())
}

async fn execute_provider_command(
    state: &SandboxManagerState,
    sandbox_id: &str,
    parameters: &Value,
    idempotency_key: &str,
    sandbox_request: &SandboxExecuteRequestV1,
    egress_mode: SandboxEgressModeV1,
    ttl_seconds: u32,
) -> Result<(Value, Option<String>), ProviderError> {
    let endpoint_url = format!(
        "{}/v1/sandboxes/{sandbox_id}/endpoints/{EXECD_PORT}?use_server_proxy=true",
        state.provider_endpoint.trim_end_matches('/')
    );
    let (endpoint_payload, _) =
        provider_json(authenticated(state, state.client.get(endpoint_url))).await?;
    let endpoint = endpoint_payload
        .get("endpoint")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError {
            message: "OpenSandbox execd endpoint response is missing endpoint".into(),
            outcome_unknown: false,
        })?;
    let endpoint = server_proxy_endpoint(&state.provider_endpoint, endpoint, sandbox_id)?;
    let mut headers = HeaderMap::new();
    if let Some(values) = endpoint_payload.get("headers").and_then(Value::as_object) {
        for (name, value) in values {
            let Some(value) = value.as_str() else {
                return Err(ProviderError {
                    message: "OpenSandbox execd header is not a string".into(),
                    outcome_unknown: false,
                });
            };
            let name =
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                    ProviderError {
                        message: format!("invalid OpenSandbox execd header name: {error}"),
                        outcome_unknown: false,
                    }
                })?;
            let value =
                reqwest::header::HeaderValue::from_str(value).map_err(|error| ProviderError {
                    message: format!("invalid OpenSandbox execd header value: {error}"),
                    outcome_unknown: false,
                })?;
            headers.insert(name, value);
        }
    }
    if endpoint.host_str()
        == reqwest::Url::parse(&state.provider_endpoint)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .as_deref()
        && let Some(key) = &state.provider_api_key
    {
        headers.insert(
            reqwest::header::HeaderName::from_static("open-sandbox-api-key"),
            reqwest::header::HeaderValue::from_str(key).map_err(|error| ProviderError {
                message: format!("invalid OpenSandbox API key header: {error}"),
                outcome_unknown: false,
            })?,
        );
    }
    wait_execd_ready(state, &endpoint, &headers).await?;
    let mut envs = serde_json::Map::new();
    if egress_mode == SandboxEgressModeV1::TcpProxy {
        let (proxy, ca) = sandbox_proxy_environment(sandbox_request, ttl_seconds)?;
        envs.insert("HTTPS_PROXY".into(), Value::String(proxy.clone()));
        envs.insert("https_proxy".into(), Value::String(proxy.clone()));
        envs.insert("HTTP_PROXY".into(), Value::String(proxy.clone()));
        envs.insert("http_proxy".into(), Value::String(proxy.clone()));
        envs.insert("AGENTX_TCP_PROXY_URL".into(), Value::String(proxy));
        envs.insert("NO_PROXY".into(), Value::String(String::new()));
        envs.insert("no_proxy".into(), Value::String(String::new()));
        if let Some(ca) = ca {
            upload_sandbox_ca(state, &endpoint, headers.clone(), ca).await?;
            for name in ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "NODE_EXTRA_CA_CERTS"] {
                envs.insert(name.into(), Value::String(SANDBOX_EGRESS_CA_PATH.into()));
            }
        }
    }
    let command = sandbox_command(parameters, idempotency_key)?;
    let command_url = endpoint.join("command").map_err(|error| ProviderError {
        message: format!("invalid OpenSandbox command endpoint: {error}"),
        outcome_unknown: false,
    })?;
    let response = state
        .client
        .post(command_url)
        .headers(headers)
        .json(&json!({
            "command":command,
            "cwd":"/workspace",
            "background":false,
            "timeout":120_000,
            "envs":envs
        }))
        .send()
        .await
        .map_err(|error| ProviderError {
            message: error.to_string(),
            outcome_unknown: !error.is_connect(),
        })?;
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.text().await.map_err(|error| ProviderError {
        message: error.to_string(),
        outcome_unknown: true,
    })?;
    if !status.is_success() {
        return Err(ProviderError {
            message: format!("OpenSandbox execd HTTP {status}: {body}"),
            outcome_unknown: false,
        });
    }
    let (stdout, stderr, exit_code) = parse_command_stream(&body)?;
    if exit_code != 0 {
        return Err(ProviderError {
            message: format!("OpenSandbox command exited with {exit_code}: {stderr}"),
            outcome_unknown: false,
        });
    }
    let (stdout, structured_output) = extract_code_result(&stdout)?;
    Ok((
        json!({
            "stdout":stdout,
            "stderr":stderr,
            "exitCode":exit_code,
            "structuredOutput":structured_output,
            "partial":false,
            "files":[],
            "sandboxId":sandbox_id
        }),
        request_id,
    ))
}

fn extract_code_result(stdout: &str) -> Result<(String, Value), ProviderError> {
    const MARKER: &str = "\n__AGENTX_RESULT__";
    let Some((diagnostics, encoded)) = stdout.rsplit_once(MARKER) else {
        return Err(ProviderError {
            message: "Code did not produce the required structured output file".into(),
            outcome_unknown: false,
        });
    };
    let bytes = STANDARD
        .decode(encoded.trim())
        .map_err(|error| ProviderError {
            message: format!("Code output is not valid base64: {error}"),
            outcome_unknown: false,
        })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| ProviderError {
        message: format!("Code output is not valid JSON: {error}"),
        outcome_unknown: false,
    })?;
    if !value.is_object() {
        return Err(ProviderError {
            message: "Code output must be a JSON object".into(),
            outcome_unknown: false,
        });
    }
    Ok((diagnostics.to_owned(), value))
}

async fn wait_execd_ready(
    state: &SandboxManagerState,
    endpoint: &reqwest::Url,
    headers: &HeaderMap,
) -> Result<(), ProviderError> {
    let ping = endpoint.join("ping").map_err(|error| ProviderError {
        message: format!("invalid OpenSandbox health endpoint: {error}"),
        outcome_unknown: false,
    })?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut last_error = "no health response".to_owned();
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(
            remaining.min(std::time::Duration::from_secs(2)),
            state
                .client
                .get(ping.clone())
                .headers(headers.clone())
                .send(),
        )
        .await
        {
            Ok(Ok(response)) if response.status() == reqwest::StatusCode::OK => return Ok(()),
            Ok(Ok(response)) if response.status().is_server_error() => {
                last_error = format!("HTTP {}", response.status());
            }
            Ok(Ok(response)) => {
                return Err(ProviderError {
                    message: format!(
                        "OpenSandbox health check returned HTTP {}",
                        response.status()
                    ),
                    outcome_unknown: false,
                });
            }
            Ok(Err(error)) => last_error = error.to_string(),
            Err(_) => last_error = "health request timed out".into(),
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Err(ProviderError {
        message: format!("OpenSandbox execd was not healthy within 30 seconds: {last_error}"),
        outcome_unknown: false,
    })
}

fn parse_command_stream(body: &str) -> Result<(String, String, i64), ProviderError> {
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = 0_i64;
    let mut recognized_event = false;
    let mut terminal_event = false;

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(':') || line.starts_with("event:") {
            continue;
        }
        let data = line.strip_prefix("data:").unwrap_or(line).trim();
        let event = serde_json::from_str::<Value>(data).map_err(|error| ProviderError {
            message: format!("invalid OpenSandbox command stream event: {error}"),
            outcome_unknown: false,
        })?;
        let event_type =
            event
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| ProviderError {
                    message: "OpenSandbox command stream event has no type".into(),
                    outcome_unknown: false,
                })?;
        recognized_event = true;
        let text = event
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "stdout" => stdout.push_str(text),
            "stderr" => stderr.push_str(text),
            "result" => {
                if let Some(code) = event
                    .get("exit_code")
                    .or_else(|| event.get("exitCode"))
                    .and_then(Value::as_i64)
                {
                    exit_code = code;
                    terminal_event = true;
                }
            }
            "execution_complete" => terminal_event = true,
            "error" => {
                let message = event
                    .pointer("/error/evalue")
                    .or_else(|| event.get("evalue"))
                    .or_else(|| event.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("OpenSandbox command failed");
                let stderr = stderr.trim();
                let message = if stderr.is_empty() {
                    message.to_owned()
                } else {
                    format!(
                        "{message}: {}",
                        stderr.chars().take(2_000).collect::<String>()
                    )
                };
                return Err(ProviderError {
                    message,
                    outcome_unknown: false,
                });
            }
            "init" | "status" | "execution_count" | "ping" => {}
            other => {
                return Err(ProviderError {
                    message: format!("unsupported OpenSandbox command stream event type {other}"),
                    outcome_unknown: false,
                });
            }
        }
    }

    if !recognized_event || !terminal_event {
        return Err(ProviderError {
            message: "OpenSandbox command stream ended without a terminal event".into(),
            outcome_unknown: true,
        });
    }
    Ok((stdout, stderr, exit_code))
}

fn server_proxy_endpoint(
    lifecycle_endpoint: &str,
    provider_endpoint: &str,
    sandbox_id: &str,
) -> Result<reqwest::Url, ProviderError> {
    let lifecycle = reqwest::Url::parse(lifecycle_endpoint).map_err(|error| ProviderError {
        message: format!("invalid OpenSandbox lifecycle endpoint: {error}"),
        outcome_unknown: false,
    })?;
    let mut provider = if provider_endpoint.contains("://") {
        reqwest::Url::parse(provider_endpoint)
    } else {
        reqwest::Url::parse(&format!("{}://{provider_endpoint}", lifecycle.scheme()))
    }
    .map_err(|error| ProviderError {
        message: format!("invalid OpenSandbox execd endpoint: {error}"),
        outcome_unknown: false,
    })?;
    if matches!(provider.host_str(), Some("127.0.0.1" | "localhost" | "::1")) {
        provider
            .set_host(lifecycle.host_str())
            .map_err(|_| ProviderError {
                message: "OpenSandbox execd endpoint host cannot be rewritten".into(),
                outcome_unknown: false,
            })?;
    }
    if provider.scheme() != lifecycle.scheme()
        || provider.host_str() != lifecycle.host_str()
        || provider.port_or_known_default() != lifecycle.port_or_known_default()
    {
        return Err(ProviderError {
            message: "OpenSandbox execd endpoint is outside the lifecycle provider origin".into(),
            outcome_unknown: false,
        });
    }
    if !provider.username().is_empty()
        || provider.password().is_some()
        || provider.query().is_some()
        || provider.fragment().is_some()
    {
        return Err(ProviderError {
            message: "OpenSandbox server-proxy endpoint contains forbidden URL components".into(),
            outcome_unknown: false,
        });
    }
    let expected_path = format!("/v1/sandboxes/{sandbox_id}/proxy/{EXECD_PORT}");
    let expected_directory = format!("{expected_path}/");
    match provider.path() {
        path if path == expected_path => provider.set_path(&expected_directory),
        path if path == expected_directory => {}
        _ => {
            return Err(ProviderError {
                message: "OpenSandbox returned an invalid server-proxy endpoint path".into(),
                outcome_unknown: false,
            });
        }
    }
    Ok(provider)
}

fn sandbox_command(parameters: &Value, idempotency_key: &str) -> Result<String, ProviderError> {
    let runner = parameters
        .get("runner")
        .and_then(Value::as_str)
        .unwrap_or("python");
    let source = parameters
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError {
            message: "Sandbox code node requires parameters.source".into(),
            outcome_unknown: false,
        })?;
    let inputs = parameters
        .get("inputs")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !inputs.is_object() {
        return Err(ProviderError {
            message: "Code inputs must resolve to an object".into(),
            outcome_unknown: false,
        });
    }
    let input_encoded =
        STANDARD.encode(serde_json::to_vec(&inputs).map_err(|error| ProviderError {
            message: error.to_string(),
            outcome_unknown: false,
        })?);
    let input_path = "/tmp/agentx-input.json";
    let output_path = "/tmp/agentx-output.json";
    let workspace_tool = parameters.get("workspaceTool").and_then(Value::as_str);
    let (path, executable, program) = match runner {
        "python" if workspace_tool.is_some() => ("/tmp/agentx-v2.py", "python3", source.to_owned()),
        "python" => (
            "/tmp/agentx-v2.py",
            "python3",
            format!(
                "{source}\n\nif __name__ == '__main__':\n    import json, os\n    with open(os.environ['AGENTX_INPUT_PATH'], encoding='utf-8') as stream:\n        _agentx_inputs = json.load(stream)\n    _agentx_result = main(**_agentx_inputs)\n    with open(os.environ['AGENTX_OUTPUT_PATH'], 'w', encoding='utf-8') as stream:\n        json.dump(_agentx_result, stream, ensure_ascii=False)\n"
            ),
        ),
        "javascript" => (
            "/tmp/agentx-v2.js",
            "node",
            format!(
                "{source}\n\n(async () => {{\n  const fs = require('fs');\n  const inputs = JSON.parse(fs.readFileSync(process.env.AGENTX_INPUT_PATH, 'utf8'));\n  const result = await main(inputs);\n  fs.writeFileSync(process.env.AGENTX_OUTPUT_PATH, JSON.stringify(result));\n}})().catch((error) => {{ console.error(error); process.exit(1); }});\n"
            ),
        ),
        "shell" => ("/tmp/agentx-v2.sh", "sh", source.to_owned()),
        other => {
            return Err(ProviderError {
                message: format!("unsupported Sandbox runner {other}"),
                outcome_unknown: false,
            });
        }
    };
    let encoded = STANDARD.encode(program.as_bytes());
    let invocation = if workspace_tool.is_some() {
        // Agent Workspace tools are an internal Sandbox capability. Their
        // generated Python emits one JSON object, which is promoted to the
        // same structured output file used by public Code nodes.
        format!("{executable} '{path}' > '{output_path}'")
    } else {
        format!(
            "AGENTX_INPUT_PATH='{input_path}' AGENTX_OUTPUT_PATH='{output_path}' {executable} '{path}'"
        )
    };
    let operation_hash = raw_hash(idempotency_key.as_bytes());
    Ok(format!(
        "AGENTX_STATE='/tmp/agentx-{operation_hash}'; AGENTX_ACQUIRED=0; \
         while [ \"$AGENTX_ACQUIRED\" -eq 0 ] && [ ! -f \"${{AGENTX_STATE}}.exit\" ]; do \
           if mkdir \"${{AGENTX_STATE}}.lock\" 2>/dev/null; then \
             printf '%s' \"$$\" > \"${{AGENTX_STATE}}.lock/pid\"; AGENTX_ACQUIRED=1; \
           else \
             AGENTX_PID=$(cat \"${{AGENTX_STATE}}.lock/pid\" 2>/dev/null || true); \
             if [ -n \"$AGENTX_PID\" ] && ! kill -0 \"$AGENTX_PID\" 2>/dev/null; then rm -rf \"${{AGENTX_STATE}}.lock\"; else sleep 0.1; fi; \
           fi; \
          done; \
          if [ \"$AGENTX_ACQUIRED\" -eq 1 ]; then \
            rm -f '{output_path}'; printf '%s' '{input_encoded}' | base64 -d > '{input_path}'; \
            set +e; (printf '%s' '{encoded}' | base64 -d > '{path}' && {invocation}) > \"${{AGENTX_STATE}}.stdout.tmp\" 2> \"${{AGENTX_STATE}}.stderr.tmp\"; AGENTX_EXIT=$?; \
            mv \"${{AGENTX_STATE}}.stdout.tmp\" \"${{AGENTX_STATE}}.stdout\"; mv \"${{AGENTX_STATE}}.stderr.tmp\" \"${{AGENTX_STATE}}.stderr\"; if [ -f '{output_path}' ]; then mv '{output_path}' \"${{AGENTX_STATE}}.result\"; else rm -f \"${{AGENTX_STATE}}.result\"; fi; \
            printf '%s' \"$AGENTX_EXIT\" > \"${{AGENTX_STATE}}.exit.tmp\"; mv \"${{AGENTX_STATE}}.exit.tmp\" \"${{AGENTX_STATE}}.exit\"; rm -rf \"${{AGENTX_STATE}}.lock\"; \
          fi; \
          cat \"${{AGENTX_STATE}}.stdout\"; if [ -f \"${{AGENTX_STATE}}.result\" ]; then printf '\n__AGENTX_RESULT__'; base64 < \"${{AGENTX_STATE}}.result\" | tr -d '\n'; fi; \
          cat \"${{AGENTX_STATE}}.stderr\" >&2; exit $(cat \"${{AGENTX_STATE}}.exit\")"
    ))
}

#[derive(Debug)]
struct ProviderError {
    message: String,
    outcome_unknown: bool,
}

fn authenticated(
    state: &SandboxManagerState,
    request: reqwest::RequestBuilder,
) -> reqwest::RequestBuilder {
    if let Some(key) = &state.provider_api_key {
        request.header("OPEN-SANDBOX-API-KEY", key)
    } else {
        request
    }
}

async fn provider_json(
    request: reqwest::RequestBuilder,
) -> Result<(Value, Option<String>), ProviderError> {
    let response = request.send().await.map_err(|error| ProviderError {
        outcome_unknown: !error.is_connect(),
        message: error.to_string(),
    })?;
    let status = response.status();
    let operation_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response.bytes().await.map_err(|error| ProviderError {
        outcome_unknown: true,
        message: error.to_string(),
    })?;
    if !status.is_success() {
        let payload = serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| {
            Value::String(
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(1_000)
                    .collect(),
            )
        });
        return Err(ProviderError {
            outcome_unknown: false,
            message: format!("OpenSandbox HTTP {status}: {payload}"),
        });
    }
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).map_err(|error| ProviderError {
            outcome_unknown: false,
            message: format!("invalid OpenSandbox JSON response: {error}"),
        })?
    };
    Ok((payload, operation_id))
}

async fn terminate_provider(
    state: &SandboxManagerState,
    sandbox_id: &str,
    idempotency_key: &str,
) -> Result<(), ProviderError> {
    let response = authenticated(
        state,
        state.client.delete(format!(
            "{}/v1/sandboxes/{sandbox_id}",
            state.provider_endpoint.trim_end_matches('/')
        )),
    )
    .header("Idempotency-Key", format!("{idempotency_key}:terminate"))
    .send()
    .await
    .map_err(|error| ProviderError {
        outcome_unknown: !error.is_connect(),
        message: error.to_string(),
    })?;
    if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(ProviderError {
        outcome_unknown: false,
        message: format!("OpenSandbox terminate HTTP {status}: {body}"),
    })
}

async fn renew_provider_expiration(
    state: &SandboxManagerState,
    sandbox_id: &str,
    ttl_seconds: u32,
    idempotency_key: &str,
) -> Result<(), ProviderError> {
    let expires_at = (time::OffsetDateTime::now_utc()
        + time::Duration::seconds(i64::from(ttl_seconds.max(60).saturating_add(30))))
    .format(&time::format_description::well_known::Rfc3339)
    .map_err(|error| ProviderError {
        message: error.to_string(),
        outcome_unknown: false,
    })?;
    provider_json(
        authenticated(
            state,
            state.client.post(format!(
                "{}/v1/sandboxes/{sandbox_id}/renew-expiration",
                state.provider_endpoint.trim_end_matches('/')
            )),
        )
        .header("Idempotency-Key", format!("{idempotency_key}:renew"))
        .json(&json!({"expiresAt": expires_at})),
    )
    .await
    .map(|_| ())
}

async fn advance_lease(
    state: &SandboxManagerState,
    lease_id: Uuid,
    fencing_token: u64,
    status: &str,
    sandbox_id: Option<&str>,
    provider_operation_id: Option<&str>,
) -> RuntimeResult<()> {
    let changed = sqlx::query(
        "UPDATE sandbox_leases SET status=?,sandbox_id=COALESCE(?,sandbox_id),provider_operation_id=COALESCE(?,provider_operation_id),heartbeat_at=UTC_TIMESTAMP(6),locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND) WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(status)
    .bind(sandbox_id)
    .bind(provider_operation_id)
    .bind(LEASE_SECONDS)
    .bind(lease_id)
    .bind(state.owner)
    .bind(fencing_token)
    .execute(&state.pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(conflict(
            "Sandbox Lease was lost between Provider operations",
        ));
    }
    Ok(())
}

async fn mark_orphaned(
    state: &SandboxManagerState,
    lease_id: Uuid,
    fencing_token: u64,
    error: &str,
) -> RuntimeResult<()> {
    let changed = sqlx::query(
        "UPDATE sandbox_leases SET status='orphaned',outcome_unknown=TRUE,last_error=?,locked_by=NULL,locked_until=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(error.chars().take(1000).collect::<String>())
    .bind(lease_id)
    .bind(state.owner)
    .bind(fencing_token)
    .execute(&state.pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(conflict("Sandbox Lease was lost while recording orphan"));
    }
    emit_sandbox_finished(&state.pool, lease_id, "outcome_unknown", Some(error)).await;
    Ok(())
}

async fn fail_lease(
    state: &SandboxManagerState,
    lease_id: Uuid,
    fencing_token: u64,
    error: &str,
    outcome_unknown: bool,
) -> RuntimeResult<()> {
    let changed = sqlx::query(
        "UPDATE sandbox_leases SET status=?,outcome_unknown=?,last_error=?,locked_by=NULL,locked_until=NULL WHERE id=? AND status IN ('creating','running','interrupting','terminating') AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(if outcome_unknown { "orphaned" } else { "failed" })
    .bind(outcome_unknown)
    .bind(error.chars().take(1000).collect::<String>())
    .bind(lease_id)
    .bind(state.owner)
    .bind(fencing_token)
    .execute(&state.pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(conflict("Sandbox Lease was lost while recording failure"));
    }
    emit_sandbox_finished(
        &state.pool,
        lease_id,
        if outcome_unknown {
            "outcome_unknown"
        } else {
            "failed"
        },
        Some(error),
    )
    .await;
    Ok(())
}

async fn emit_sandbox_finished(
    pool: &sqlx::MySqlPool,
    lease_id: Uuid,
    status: &str,
    error: Option<&str>,
) {
    let row = match sqlx::query("SELECT tenant_id,execution_id,node_execution_id,attempt_id,profile_version_id,result_json FROM sandbox_leases WHERE id=?")
        .bind(lease_id).fetch_optional(pool).await
    {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(error) => { tracing::warn!(%error, %lease_id, "Sandbox Trace lookup failed"); return; }
    };
    let (Ok(tenant_id), Ok(execution_id), Ok(attempt_id)) = (
        row.try_get::<Uuid, _>("tenant_id"),
        row.try_get::<Uuid, _>("execution_id"),
        row.try_get::<Uuid, _>("attempt_id"),
    ) else {
        return;
    };
    let mut trace = crate::trace_delivery::TraceDraft::span(
        tenant_id,
        execution_id,
        lease_id,
        Some((
            attempt_id,
            agentx_runtime_contracts::TraceSpanKindV1::Attempt,
        )),
        agentx_runtime_contracts::TraceSpanKindV1::Sandbox,
        "OpenSandbox execution",
        agentx_runtime_contracts::TraceEventKindV1::Finished,
        match status {
            "cancelled" => "sandbox.cancelled",
            "timed_out" => "sandbox.timed_out",
            "outcome_unknown" => "sandbox.outcome_unknown",
            "failed" => "sandbox.failed",
            _ => "sandbox.finished",
        },
        status,
    );
    trace.node_execution_id = row.try_get("node_execution_id").ok();
    trace.attempt_id = Some(attempt_id);
    trace.sandbox_lease_id = Some(lease_id);
    trace.resource_type = Some("sandbox_profile".into());
    trace.resource_id = row.try_get("profile_version_id").ok();
    trace.error_code = error.map(|_| match status {
        "cancelled" => "EXECUTION_CANCELLED".into(),
        "timed_out" => "SANDBOX_EXECUTION_TIMED_OUT".into(),
        "outcome_unknown" => "SANDBOX_OUTCOME_UNKNOWN".into(),
        _ => "SANDBOX_EXECUTION_FAILED".into(),
    });
    trace.error_message = error.map(|value| value.chars().take(1000).collect());
    trace.attributes =
        json!({"error":error.map(|value| value.chars().take(500).collect::<String>())});
    trace.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::SandboxResponse);
    trace.content_preview = row
        .try_get::<Option<Value>, _>("result_json")
        .ok()
        .flatten()
        .as_ref()
        .and_then(crate::trace_delivery::bounded_preview);
    let Ok(mut tx) = pool.begin().await else {
        return;
    };
    crate::trace_delivery::enqueue_best_effort(&mut tx, trace).await;
    let _ = tx.commit().await;
}

fn stable_id(namespace: Uuid, label: &[u8]) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update(label);
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn raw_hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn sandbox_request_hash(request: &SandboxExecuteRequestV1) -> RuntimeResult<String> {
    let mut stable_request =
        serde_json::to_value(request).map_err(|error| RuntimeError::Internal(error.into()))?;
    if let Some(object) = stable_request.as_object_mut() {
        object.remove("workerId");
        object.remove("fencingToken");
    }
    agentx_runtime_contracts::content_hash(&stable_request)
        .map(|hash| hash.to_string())
        .map_err(|error| RuntimeError::Internal(error.into()))
}

fn bad_request(message: &str) -> RuntimeError {
    RuntimeError::BadRequest(
        agentx_runtime_contracts::RuntimePublishErrorCodeV1::AdmissionPrerequisiteMissing,
        message.into(),
    )
}

fn conflict(message: &str) -> RuntimeError {
    RuntimeError::Conflict(
        agentx_runtime_contracts::RuntimePublishErrorCodeV1::BundleReferenceConflict,
        message.into(),
    )
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
