use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};

use agentx_bundle_builder::{
    BundleBuildSource, WorkPackageBuildSource, build_bundle, build_work_package,
    compile_workflow_version, composite_ir_object_id,
};
use agentx_domain::WorkflowDefinition;
use agentx_runtime_contracts::{
    ActivateDeploymentRequestV1, ActivationManifestV1, AdmissionStatusV1, AdmissionTargetV1,
    ApiKeyAdmissionV1, ApplicationRouteAdmissionV1, ApplyChatMappingRequestV1,
    ApprovalActionValueV1, CancelWorkPackageRequestV1, ChatMappingV1, CommandEnvelopeV1,
    ControlRole, CreateSessionRequestV1, DelegationClaimsV1, DisableDeploymentRequestV1,
    ExecuteWorkPackageRequestV1, ExecutionSearchRequestV1, InvocationResponseV1,
    MessagePartInputV1, MessageRequestV1, MessageResponseV1, Plane, PrepareBundleRequestV1,
    PrepareWorkPackageRequestV1, PublishReceiptStatusV1, RollbackDeploymentRequestV1,
    RuntimeAdmissionCommandV1, RuntimeApprovalActionV1, RuntimeAuthorizationSnapshotV1,
    RuntimeCallPurposeV1, RuntimeEventPayloadV1, RuntimeGrantStateV1, RuntimeObjectReferenceV1,
    RuntimeObjectUploadMetadataV1, RuntimePolicyV1, RuntimeResourceKindV1,
    RuntimeRetentionDataTypeV1, RuntimeRetentionPolicyV1, RuntimeTriggerConfigurationV1,
    RuntimeTriggerSpecV1, RuntimeUserWorkflowGrantV1, RuntimeWorkPackageOverlayV1, ServiceClaimsV1,
    ServiceIdentityAdmissionV1, SessionResponseV1, SideEffectResolutionV1, StorageDomain,
    WorkPackagePurpose, WorkerResultStatusV1, WorkerResultV1, issue_delegation_token,
    issue_service_token, now_unix,
};
use agentx_v2_runtime::{
    RuntimeState,
    agent_session_queue::wake_pending_sessions,
    auth::RuntimeTrust,
    error::RuntimeError,
    execution::{
        InvocationCaller, InvocationRequestV1, authenticate_api_key, claim_commands,
        claim_dispatch, complete_dispatch, create_invocation, create_runtime_invocation_tx,
        process_command, process_command_with_state, recover_dispatches, release_dispatch,
    },
    gc::{cleanup_expired_temporary_objects, mark_collectable, sweep_one},
    internal_engine::{
        apply_runtime_command, cancel_work_package, execute_work_package, prepare_work_package,
    },
    object_upload::persist_upload,
    publish::{
        activate_deployment, apply_admission, apply_chat_mapping, disable_deployment,
        prepare_bundle, rollback_deployment,
    },
    query::{
        get_execution, get_execution_artifact, get_execution_runtime_details, search_executions,
    },
    retention::run_once as run_retention_once,
    worker_runtime::{RuntimeWorker, WorkerProvider, WorkerProviderError, WorkerProviderResponse},
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{HeaderMap, Request, header::AUTHORIZATION},
    routing::{get, post},
};
use bytes::Bytes;
use ed25519_dalek::SigningKey;
use object_store::{
    GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, PutMultipartOpts, PutOptions,
    PutPayload, PutResult,
};
use object_store::{ObjectStore, memory::InMemory, path::Path as ObjectPath};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, Row, mysql::MySqlPoolOptions};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

const PRIVATE_KEY: &[u8] =
    include_bytes!("../../../crates/agentx-runtime-contracts/tests/fixtures/service-private.pem");
const PUBLIC_KEY: &[u8] =
    include_bytes!("../../../crates/agentx-runtime-contracts/tests/fixtures/service-public.pem");

struct Fixture {
    state: RuntimeState,
    signing_key: SigningKey,
    work_package_signing_key: SigningKey,
    tenant_id: Uuid,
    application_id: Uuid,
    workflow_id: Uuid,
    identity_id: Uuid,
    key_id: Uuid,
    api_key: String,
}

enum StubWorkerMode {
    Reject,
    Agent(Arc<std::sync::atomic::AtomicUsize>),
    AgentAuthorization(Arc<AgentAuthorizationProbe>),
    Evaluator,
}

struct AgentAuthorizationProbe {
    pool: MySqlPool,
    tenant_id: Uuid,
    revoked_grant_id: Uuid,
    tool_to_call: String,
    model_calls: std::sync::atomic::AtomicUsize,
    visible_tools: Mutex<Vec<Vec<String>>>,
}

struct StubWorkerProvider {
    mode: StubWorkerMode,
}

#[async_trait::async_trait]
impl WorkerProvider for StubWorkerProvider {
    async fn post_json_stream(
        &self,
        endpoint: &str,
        context: agentx_v2_runtime::egress::EgressRequestContext,
        timeout: Duration,
        headers: reqwest::header::HeaderMap,
        body: &Value,
    ) -> Result<agentx_v2_runtime::worker_runtime::WorkerStreamResponse, WorkerProviderError> {
        // Synthesize an OpenAI-style SSE token stream from the buffered
        // fixture response so streaming model calls replay identically.
        let buffered = self
            .post_json(endpoint, context, timeout, headers, body)
            .await?;
        let payload = serde_json::from_slice::<Value>(&buffered.body).unwrap_or(Value::Null);
        let message = payload
            .pointer("/choices/0/message")
            .cloned()
            .unwrap_or(Value::Null);
        let finish_reason = payload
            .pointer("/choices/0/finish_reason")
            .cloned()
            .unwrap_or(Value::Null);
        let mut delta_message = json!({"role":"assistant"});
        if let Some(content) = message.get("content").filter(|value| !value.is_null()) {
            delta_message["content"] = content.clone();
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            delta_message["tool_calls"] = json!(tool_calls
                .iter()
                .enumerate()
                .map(|(index, call)| json!({
                    "index": index,
                    "id": call.get("id").cloned().unwrap_or(Value::Null),
                    "type": "function",
                    "function": {
                        "name": call.pointer("/function/name").cloned().unwrap_or(Value::Null),
                        "arguments": call.pointer("/function/arguments").cloned().unwrap_or(Value::Null),
                    }
                }))
                .collect::<Vec<_>>());
        }
        let mut frames = String::new();
        let delta = json!({"choices":[{"index":0,"delta":delta_message,"finish_reason":null}],"usage":Value::Null});
        frames.push_str(&format!("data: {delta}\n\n"));
        let usage = payload.get("usage").cloned().unwrap_or(Value::Null);
        let finish =
            json!({"choices":[{"index":0,"delta":{},"finish_reason":finish_reason}],"usage":usage});
        frames.push_str(&format!("data: {finish}\n\n"));
        frames.push_str("data: [DONE]\n\n");
        let mut response = axum::http::Response::builder()
            .status(200)
            .body(reqwest::Body::from(frames))
            .expect("fixture SSE response");
        for (name, value) in &buffered.headers {
            response.headers_mut().insert(
                name,
                reqwest::header::HeaderValue::from_bytes(value.as_bytes())
                    .expect("fixture header value"),
            );
        }
        Ok(agentx_v2_runtime::worker_runtime::WorkerStreamResponse {
            status: buffered.status,
            response: reqwest::Response::from(response),
        })
    }
    async fn post_json(
        &self,
        endpoint: &str,
        _context: agentx_v2_runtime::egress::EgressRequestContext,
        _timeout: Duration,
        _headers: reqwest::header::HeaderMap,
        body: &Value,
    ) -> Result<WorkerProviderResponse, WorkerProviderError> {
        let payload = match &self.mode {
            StubWorkerMode::Reject => {
                return Err(WorkerProviderError::Denied(
                    "unexpected provider request in test".into(),
                ));
            }
            StubWorkerMode::Evaluator => json!({
                "id":"fixture-evaluator-response",
                "object":"chat.completion",
                "choices":[{"index":0,"message":{"role":"assistant","content":"{\"passed\":true,\"score\":0.95,\"reason\":\"fixture accepted the target output\",\"usage\":{\"tokens\":7,\"costMicros\":23}}"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":0,"completion_tokens":7,"total_tokens":7}
            }),
            StubWorkerMode::Agent(calls) if endpoint.ends_with("/chat/completions") => {
                let index = calls.fetch_add(1, Ordering::SeqCst);
                json!({
                    "id":format!("fixture-response-{index}"),
                    "object":"chat.completion",
                    "choices":[{"index":0,"message":{"role":"assistant","content":if index == 0 { "first-turn" } else { "second-turn" }},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":5,"completion_tokens":5,"total_tokens":10}
                })
            }
            StubWorkerMode::Agent(_) if endpoint.ends_with("/mcp") => {
                if body.get("id").is_none() {
                    Value::Null
                } else if body.get("method").and_then(Value::as_str) == Some("initialize") {
                    json!({"jsonrpc":"2.0","id":body["id"],"result":{}})
                } else {
                    json!({"jsonrpc":"2.0","id":body["id"],"result":{"content":{"value":"tool-result"}}})
                }
            }
            StubWorkerMode::Agent(_) => {
                return Err(WorkerProviderError::Denied(format!(
                    "unexpected Agent fixture endpoint: {endpoint}"
                )));
            }
            StubWorkerMode::AgentAuthorization(probe)
                if endpoint.ends_with("/chat/completions") =>
            {
                let index = probe.model_calls.fetch_add(1, Ordering::SeqCst);
                let visible = body
                    .get("tools")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| {
                        tool.pointer("/function/name")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .collect::<Vec<_>>();
                probe.visible_tools.lock().unwrap().push(visible);
                if index == 0 {
                    sqlx::query("UPDATE resource_grant_projection SET status='revoked',policy_epoch=policy_epoch+1 WHERE tenant_id=? AND grant_id=?")
                        .bind(probe.tenant_id)
                        .bind(probe.revoked_grant_id)
                        .execute(&probe.pool)
                        .await
                        .unwrap();
                    json!({
                        "id":"authorization-turn-1",
                        "object":"chat.completion",
                        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-authorized-tool","type":"function","function":{"name":probe.tool_to_call,"arguments":"{\"value\":\"ok\"}"}}]},"finish_reason":"tool_calls"}],
                        "usage":{"prompt_tokens":5,"completion_tokens":5,"total_tokens":10}
                    })
                } else {
                    json!({
                        "id":"authorization-turn-2",
                        "object":"chat.completion",
                        "choices":[{"index":0,"message":{"role":"assistant","content":"authorized tool remained available"},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":5,"completion_tokens":5,"total_tokens":10}
                    })
                }
            }
            StubWorkerMode::AgentAuthorization(_) if endpoint.ends_with("/mcp") => {
                if body.get("id").is_none() {
                    Value::Null
                } else if body.get("method").and_then(Value::as_str) == Some("initialize") {
                    json!({"jsonrpc":"2.0","id":body["id"],"result":{}})
                } else {
                    json!({"jsonrpc":"2.0","id":body["id"],"result":{"content":[{"type":"text","text":"tool-result"}]}})
                }
            }
            StubWorkerMode::AgentAuthorization(_) => {
                return Err(WorkerProviderError::Denied(format!(
                    "unexpected Agent authorization fixture endpoint: {endpoint}"
                )));
            }
        };
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(request_id) = payload.get("id").and_then(Value::as_str) {
            headers.insert("x-request-id", request_id.parse().unwrap());
        }
        Ok(WorkerProviderResponse {
            status: reqwest::StatusCode::OK,
            headers,
            body: Bytes::from(serde_json::to_vec(&payload).unwrap()),
        })
    }
}

fn test_worker(fixture: &Fixture, mode: StubWorkerMode) -> RuntimeWorker {
    RuntimeWorker::new_with_provider(
        fixture.state.pool.clone(),
        fixture.state.objects.clone(),
        Arc::new(StubWorkerProvider { mode }),
    )
}

include!("runtime_slice/publish_and_suspension.rs");
include!("runtime_slice/fork_sandbox_and_retention.rs");
include!("runtime_slice/agent_attachments.rs");
include!("runtime_slice/lifecycle_and_work_packages.rs");
include!("runtime_slice/session_recovery_and_gc.rs");
