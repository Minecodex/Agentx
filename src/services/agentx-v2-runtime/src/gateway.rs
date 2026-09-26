use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    time::Duration,
};

use agentx_runtime_contracts::{
    ArtifactUploadResponseV1, CommandAcceptedV1, CreateSessionRequestV1, GatewayErrorV1,
    InvocationRequestV1, InvocationResponseV1, MessagePartInputV1, MessageRequestV1,
    MessageResponseV1, SessionResponseV1, SessionVersionPolicyV1, VaultSecretReferenceV1,
    WebhookProviderV1,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    middleware,
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::Stream;
use hmac::{Hmac, Mac};
use object_store::{ObjectStore, WriteMultipart, path::Path as ObjectPath};
use schemars::{JsonSchema, schema_for};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
    execution::{InvocationCaller, create_chat_runtime_invocation_tx, create_runtime_invocation},
};

const JSON_LIMIT: usize = 1024 * 1024;
const MULTIPART_LIMIT: usize = 50 * 1024 * 1024;
const IDEMPOTENCY_RETENTION_DAYS: u64 = 14;

#[derive(Clone)]
struct Caller {
    tenant_id: Uuid,
    application_id: Uuid,
    subject_id: Uuid,
    caller_type: &'static str,
    token_version: Option<u64>,
    origin: agentx_runtime_contracts::ExecutionOriginV1,
}

pub fn router() -> Router<RuntimeState> {
    let rate_limiter = crate::rate_limit::LocalCallerRateLimiter::from_env()
        .expect("Runtime Gateway rate limit configuration must be valid");
    Router::new()
        .route("/applications/{slug}/sessions", post(create_session))
        .route("/applications/{slug}/invocations", post(create_invocation))
        .route("/workflows/{slug}/invoke", post(invoke_workflow))
        .route(
            "/artifacts",
            post(upload_artifact).layer(DefaultBodyLimit::max(MULTIPART_LIMIT + 1024 * 1024)),
        )
        .route("/sessions/{id}", get(get_session))
        .route(
            "/sessions/{id}/messages",
            get(list_messages).post(send_message),
        )
        .route("/invocations/{id}", get(get_invocation))
        .route("/invocations/{id}/cancel", post(cancel_invocation))
        .route("/invocations/{id}/events", get(invocation_events))
        .route("/webhooks/{public_id}", post(webhook).get(webhook))
        .layer(middleware::from_fn_with_state(
            rate_limiter,
            crate::rate_limit::enforce,
        ))
        .layer(DefaultBodyLimit::max(JSON_LIMIT))
}

async fn create_session(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateSessionRequestV1>,
) -> RuntimeResult<(StatusCode, Json<SessionResponseV1>)> {
    let caller = authenticate(&state, &headers, Some(&slug)).await?;
    let idempotency_key = idempotency_key(&headers)?;
    let request_hash = request_hash(&request)?;
    let scope = format!(
        "session:create:{}:{}",
        caller.caller_type, caller.subject_id
    );
    if let Some(response) = replay::<SessionResponseV1>(
        &state,
        caller.tenant_id,
        &scope,
        idempotency_key,
        &request_hash,
    )
    .await?
    {
        return Ok((StatusCode::CREATED, Json(response)));
    }
    let mut tx = state.pool.begin().await?;
    let route = sqlx::query("SELECT r.active_bundle_id,b.deployment_id,b.workflow_version_id,CAST(JSON_UNQUOTE(JSON_EXTRACT(r.runtime_policy_json,'$.sessionVersionPolicy')) AS CHAR) session_policy FROM application_routes r JOIN deployment_bundles b ON b.id=r.active_bundle_id AND b.tenant_id=r.tenant_id WHERE r.tenant_id=? AND r.application_id=? AND r.status='active' FOR SHARE")
        .bind(caller.tenant_id).bind(caller.application_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    let policy = route
        .try_get::<Option<String>, _>("session_policy")?
        .unwrap_or_else(|| "pinned".into());
    let policy_contract = parse_policy(&policy)?;
    let bundle_id: Uuid = route.try_get("active_bundle_id")?;
    let pinned_bundle = (policy != "follow_deployment").then_some(bundle_id);
    let workflow_version_id: Uuid = route.try_get("workflow_version_id")?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO application_sessions(id,tenant_id,application_id,application_deployment_id,workflow_version_id,bundle_id,head_bundle_id,version_policy,external_user_id,title,created_by_user_id) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
        .bind(id).bind(caller.tenant_id).bind(caller.application_id).bind(route.try_get::<Uuid,_>("deployment_id")?).bind((policy != "follow_deployment").then_some(workflow_version_id)).bind(pinned_bundle).bind(bundle_id).bind(&policy).bind(&request.external_user_id).bind(&request.title).bind((caller.caller_type == "user").then_some(caller.subject_id)).execute(&mut *tx).await?;
    if let Some(bundle) = pinned_bundle {
        sqlx::query("INSERT INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'pinned_session',?)")
            .bind(Uuid::now_v7()).bind(caller.tenant_id).bind(bundle).bind(id).execute(&mut *tx).await?;
    }
    let response = SessionResponseV1 {
        id,
        application_id: caller.application_id,
        application_deployment_id: route.try_get("deployment_id")?,
        workflow_version_id: (policy != "follow_deployment").then_some(workflow_version_id),
        bundle_id: pinned_bundle,
        version_policy: policy_contract,
        external_user_id: request.external_user_id,
        title: request.title,
        status: "active".into(),
        version: 1,
        updated_at: OffsetDateTime::now_utc(),
    };
    persist_idempotency_tx(
        &mut tx,
        caller.tenant_id,
        &scope,
        idempotency_key,
        &request_hash,
        StatusCode::CREATED,
        &response,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn get_session(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<Json<SessionResponseV1>> {
    let caller = authenticate_resource(&state, &headers, "session", id).await?;
    Ok(Json(load_session(&state, &caller, id).await?))
}

async fn list_messages(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<Json<Vec<MessageResponseV1>>> {
    let caller = authenticate_resource(&state, &headers, "session", id).await?;
    load_session(&state, &caller, id).await?;
    let messages = sqlx::query("SELECT id,invocation_id,sequence_number,role,created_at FROM application_messages WHERE tenant_id=? AND session_id=? ORDER BY sequence_number,id LIMIT 1000")
        .bind(caller.tenant_id).bind(id).fetch_all(&state.pool).await?;
    let parts = sqlx::query("SELECT p.message_id,p.part_type,p.content_json,p.artifact_id FROM application_message_parts p JOIN application_messages m ON m.tenant_id=p.tenant_id AND m.id=p.message_id WHERE p.tenant_id=? AND m.session_id=? ORDER BY m.sequence_number,p.part_index")
        .bind(caller.tenant_id).bind(id).fetch_all(&state.pool).await?;
    let mut by_message = BTreeMap::<Uuid, Vec<MessagePartInputV1>>::new();
    for part in parts {
        by_message
            .entry(part.try_get("message_id")?)
            .or_default()
            .push(MessagePartInputV1 {
                part_type: part.try_get("part_type")?,
                content: part.try_get("content_json")?,
                artifact_id: part.try_get("artifact_id")?,
            });
    }
    Ok(Json(
        messages
            .into_iter()
            .map(|row| {
                let id: Uuid = row.try_get("id")?;
                Ok(MessageResponseV1 {
                    id,
                    invocation_id: row.try_get("invocation_id")?,
                    sequence: row.try_get("sequence_number")?,
                    role: row.try_get("role")?,
                    parts: by_message.remove(&id).unwrap_or_default(),
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect::<RuntimeResult<Vec<_>>>()?,
    ))
}

async fn create_invocation(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(request): Json<InvocationRequestV1>,
) -> RuntimeResult<(StatusCode, Json<InvocationResponseV1>)> {
    let caller = authenticate(&state, &headers, Some(&slug)).await?;
    invoke(&state, caller, &headers, request).await
}

async fn invoke_workflow(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Json(request): Json<InvocationRequestV1>,
) -> RuntimeResult<(StatusCode, Json<InvocationResponseV1>)> {
    let caller = authenticate(&state, &headers, Some(&slug)).await?;
    invoke(&state, caller, &headers, request).await
}

async fn invoke(
    state: &RuntimeState,
    caller: Caller,
    headers: &HeaderMap,
    request: InvocationRequestV1,
) -> RuntimeResult<(StatusCode, Json<InvocationResponseV1>)> {
    if let Err(reject) = crate::admission::check(&state.pool, caller.tenant_id).await {
        crate::admission::record_rejection();
        return Err(crate::admission::to_error(&reject));
    }
    let idempotency_key = idempotency_key(headers)?;
    let mode = request.response_mode.as_deref().unwrap_or("async");
    if !matches!(mode, "sync" | "async") {
        return Err(bad_request("INVALID_RESPONSE_MODE"));
    }
    let accepted = create_runtime_invocation(
        &state.pool,
        caller.tenant_id,
        caller.application_id,
        InvocationCaller {
            caller_type: caller.caller_type,
            caller_id: caller.subject_id,
            token_version: caller.token_version,
            origin: caller.origin.clone(),
        },
        request.session_id,
        &request.input,
        idempotency_key,
    )
    .await?;
    let mut response = load_invocation(state, &caller, accepted.invocation_id).await?;
    if mode == "sync" {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !terminal(&response.status) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            response = load_invocation(state, &caller, accepted.invocation_id).await?;
        }
    }
    let status = if terminal(&response.status) {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    Ok((status, Json(response)))
}

async fn send_message(
    State(state): State<RuntimeState>,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<MessageRequestV1>,
) -> RuntimeResult<(StatusCode, Json<InvocationResponseV1>)> {
    if request.parts.is_empty() {
        return Err(bad_request("MESSAGE_PARTS_REQUIRED"));
    }
    let caller = authenticate_resource(&state, &headers, "session", session_id).await?;
    let idempotency_key = idempotency_key(&headers)?;
    let request_hash = request_hash(&request)?;
    let mut tx = state.pool.begin().await?;
    let locked = sqlx::query("SELECT application_id,next_message_sequence,status FROM application_sessions WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(caller.tenant_id).bind(session_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    if locked.try_get::<Uuid, _>("application_id")? != caller.application_id
        || locked.try_get::<String, _>("status")? != "active"
    {
        return Err(RuntimeError::Unauthorized);
    }
    if let Some(row) = sqlx::query("SELECT i.id,m.request_hash FROM application_messages m JOIN application_invocations i ON i.id=m.invocation_id AND i.tenant_id=m.tenant_id WHERE m.tenant_id=? AND m.session_id=? AND m.idempotency_key=?")
        .bind(caller.tenant_id).bind(session_id).bind(idempotency_key).fetch_optional(&mut *tx).await? {
        if row.try_get::<String, _>("request_hash")? != request_hash.as_str() {
            return Err(RuntimeError::Conflict(
                agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Message idempotency key was reused with another request".into(),
            ));
        }
        let invocation_id: Uuid = row.try_get("id")?;
        tx.commit().await?;
        return Ok((StatusCode::ACCEPTED, Json(load_invocation(&state, &caller, invocation_id).await?)));
    }
    for part in &request.parts {
        if !matches!(
            part.part_type.as_str(),
            "text" | "json" | "image" | "audio" | "file"
        ) || (part.content.is_none() && part.artifact_id.is_none())
        {
            return Err(bad_request("INVALID_MESSAGE_PART"));
        }
        if let Some(artifact_id) = part.artifact_id {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifacts WHERE tenant_id=? AND id=? AND deleted_at IS NULL)").bind(caller.tenant_id).bind(artifact_id).fetch_one(&mut *tx).await?;
            if !exists {
                return Err(RuntimeError::NotFound);
            }
        }
    }
    let input = chat_message_payload(&request);
    let accepted = create_chat_runtime_invocation_tx(
        &mut tx,
        caller.tenant_id,
        caller.application_id,
        InvocationCaller {
            caller_type: caller.caller_type,
            caller_id: caller.subject_id,
            token_version: caller.token_version,
            origin: caller.origin.clone(),
        },
        session_id,
        &input,
        idempotency_key,
    )
    .await?;
    let sequence: u64 = locked.try_get("next_message_sequence")?;
    let message_id = Uuid::now_v7();
    sqlx::query("INSERT INTO application_messages(id,tenant_id,session_id,invocation_id,request_hash,idempotency_key,sequence_number,role) VALUES(?,?,?,?,?,?,?,'user')")
        .bind(message_id).bind(caller.tenant_id).bind(session_id).bind(accepted.invocation_id).bind(request_hash.as_str()).bind(idempotency_key).bind(sequence).execute(&mut *tx).await?;
    for (index, part) in request.parts.iter().enumerate() {
        sqlx::query("INSERT INTO application_message_parts(id,tenant_id,message_id,part_index,part_type,content_json,artifact_id) VALUES(?,?,?,?,?,?,?)")
            .bind(Uuid::now_v7()).bind(caller.tenant_id).bind(message_id).bind(index as u32).bind(&part.part_type).bind(&part.content).bind(part.artifact_id).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE application_sessions SET next_message_sequence=next_message_sequence+1,version=version+1,title=COALESCE(title,?) WHERE tenant_id=? AND id=?")
        .bind(inferred_session_title(&request)).bind(caller.tenant_id).bind(session_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(load_invocation(&state, &caller, accepted.invocation_id).await?),
    ))
}

fn chat_message_payload(request: &MessageRequestV1) -> Value {
    let question = request
        .parts
        .iter()
        .filter(|part| part.part_type == "text")
        .filter_map(|part| part.content.as_ref().and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let files = request
        .parts
        .iter()
        .filter_map(|part| {
            part.artifact_id.map(|id| {
                let mut reference = part
                    .content
                    .as_ref()
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                reference.insert("artifactId".into(), json!(id));
                reference
                    .entry("type")
                    .or_insert_with(|| json!(part.part_type));
                Value::Object(reference)
            })
        })
        .collect::<Vec<_>>();
    json!({"question": question, "files": files})
}

fn inferred_session_title(request: &MessageRequestV1) -> Option<String> {
    const MAX_TITLE_CHARS: usize = 255;
    let normalized = request
        .parts
        .iter()
        .filter(|part| part.part_type == "text")
        .filter_map(|part| part.content.as_ref().and_then(Value::as_str))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return None;
    }
    if normalized.chars().count() <= MAX_TITLE_CHARS {
        return Some(normalized);
    }
    let mut title = normalized
        .chars()
        .take(MAX_TITLE_CHARS - 1)
        .collect::<String>();
    title.push('…');
    Some(title)
}

async fn get_invocation(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<Json<InvocationResponseV1>> {
    let caller = authenticate_resource(&state, &headers, "invocation", id).await?;
    Ok(Json(load_invocation(&state, &caller, id).await?))
}

async fn cancel_invocation(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<(StatusCode, Json<CommandAcceptedV1>)> {
    let caller = authenticate_resource(&state, &headers, "invocation", id).await?;
    let key = idempotency_key(&headers)?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT execution_id,status FROM application_invocations WHERE id=? AND tenant_id=? AND application_id=? FOR UPDATE")
        .bind(id).bind(caller.tenant_id).bind(caller.application_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    if terminal(&row.try_get::<String, _>("status")?) {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Invocation is already terminal".into(),
        ));
    }
    let execution_id: Uuid = row.try_get("execution_id")?;
    let result = sqlx::query("INSERT IGNORE INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'cancel_execution','execution',?,?,?,'pending')")
        .bind(Uuid::now_v7()).bind(caller.tenant_id).bind(execution_id.to_string()).bind(key).bind(json!({"invocationId":id,"executionId":execution_id})).execute(&mut *tx).await?;
    let replayed = result.rows_affected() == 0;
    if replayed {
        let existing: String = sqlx::query_scalar("SELECT aggregate_id FROM runtime_commands WHERE tenant_id=? AND command_type='cancel_execution' AND idempotency_key=?")
            .bind(caller.tenant_id).bind(key).fetch_one(&mut *tx).await?;
        if existing != execution_id.to_string() {
            return Err(RuntimeError::Conflict(
                agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Cancel idempotency key was reused for another Invocation".into(),
            ));
        }
    } else {
        sqlx::query("INSERT INTO invocation_events(tenant_id,invocation_id,event_id,sequence_number,event_type,payload_json) SELECT ?,?,?,COALESCE(MAX(sequence_number),0)+1,'cancel.requested',? FROM invocation_events WHERE tenant_id=? AND invocation_id=?")
            .bind(caller.tenant_id).bind(id).bind(Uuid::now_v7()).bind(json!({"executionId":execution_id})).bind(caller.tenant_id).bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CommandAcceptedV1 {
            accepted: true,
            replayed,
        }),
    ))
}

async fn invocation_events(
    State(state): State<RuntimeState>,
    Extension(lifecycle): Extension<agentx_service_kit::ServiceLifecycle>,
    Extension(metrics): Extension<agentx_service_kit::MetricsRegistry>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let caller = authenticate_resource(&state, &headers, "invocation", id).await?;
    load_invocation(&state, &caller, id).await?;
    let after = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let pool = state.pool.clone();
    let tenant_id = caller.tenant_id;
    let mut wakeups = state.wakeups.subscribe(id);
    let stream = async_stream::stream! {
        metrics.add("agentx_sse_connections", 1.0).await;
        let mut cursor = after;
        loop {
            let mut terminal_seen = false;
            match sqlx::query("SELECT sequence_number,event_type,payload_json FROM invocation_events WHERE tenant_id=? AND invocation_id=? AND sequence_number>? ORDER BY sequence_number LIMIT 100")
                .bind(tenant_id).bind(id).bind(cursor).fetch_all(&pool).await {
                Ok(rows) => {
                    for row in rows {
                        let sequence: u64 = row.try_get("sequence_number").unwrap_or(cursor);
                        let event_type: String = row.try_get("event_type").unwrap_or_else(|_| "runtime.event".into());
                        let payload: Value = row.try_get("payload_json").unwrap_or(Value::Null);
                        terminal_seen |= matches!(event_type.as_str(), "invocation.completed" | "invocation.failed" | "invocation.cancelled");
                        cursor = sequence;
                        yield Ok(Event::default().id(sequence.to_string()).event(event_type).json_data(payload).unwrap_or_else(|_| Event::default().event("serialization.error")));
                    }
                }
                Err(error) => { tracing::warn!(%error, %id, "SSE MySQL replay failed"); }
            }
            if terminal_seen {
                break;
            }
            if lifecycle.is_draining() {
                break;
            }
            if let Some(receiver) = wakeups.as_mut() {
                let _ = tokio::time::timeout(Duration::from_secs(1), receiver.recv()).await;
            } else {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        metrics.add("agentx_sse_connections", -1.0).await;
    };
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

async fn upload_artifact(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> RuntimeResult<(StatusCode, Json<ArtifactUploadResponseV1>)> {
    let caller = authenticate_artifact(&state, &headers).await?;
    let key = idempotency_key(&headers)?;
    let scope = format!("artifact:{}:{}", caller.caller_type, caller.subject_id);
    let replay = sqlx::query("SELECT id,content_type,size_bytes,sha256,request_hash FROM artifacts WHERE tenant_id=? AND idempotency_scope=? AND idempotency_key=?")
        .bind(caller.tenant_id).bind(&scope).bind(key).fetch_optional(&state.pool).await?;
    let mut field = multipart
        .next_field()
        .await
        .map_err(|_| bad_request("INVALID_MULTIPART"))?
        .ok_or_else(|| bad_request("ARTIFACT_REQUIRED"))?;
    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_owned();
    if content_type.len() > 255 || content_type.contains(['\r', '\n']) {
        return Err(bad_request("INVALID_ARTIFACT_CONTENT_TYPE"));
    }
    let artifact_id = Uuid::now_v7();
    let temporary_key = format!("temporary/{}/{}", caller.tenant_id, artifact_id);
    let mut writer = if replay.is_none() {
        Some(WriteMultipart::new(
            state
                .objects
                .put_multipart(&ObjectPath::from(temporary_key.clone()))
                .await
                .map_err(|_| RuntimeError::Unavailable)?,
        ))
    } else {
        None
    };
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| bad_request("INVALID_MULTIPART"))?
    {
        size_bytes = size_bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| bad_request("ARTIFACT_TOO_LARGE"))?;
        if size_bytes > MULTIPART_LIMIT as u64 {
            if let Some(writer) = writer {
                let _ = writer.abort().await;
            }
            return Err(bad_request("ARTIFACT_TOO_LARGE"));
        }
        hasher.update(&chunk);
        if let Some(writer) = writer.as_mut() {
            writer
                .wait_for_capacity(4)
                .await
                .map_err(|_| RuntimeError::Unavailable)?;
            writer.put(chunk);
        }
    }
    drop(field);
    if multipart
        .next_field()
        .await
        .map_err(|_| bad_request("INVALID_MULTIPART"))?
        .is_some()
    {
        if let Some(writer) = writer {
            let _ = writer.abort().await;
        }
        return Err(bad_request("MULTIPLE_ARTIFACTS_NOT_ALLOWED"));
    }
    let hash = format!("{:x}", hasher.finalize());
    let request_hash = artifact_request_hash(&content_type, size_bytes, &hash)?;
    if let Some(row) = replay {
        if row.try_get::<String, _>("request_hash")? != request_hash.as_str() {
            return Err(RuntimeError::Conflict(
                agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Artifact idempotency key was reused with different content".into(),
            ));
        }
        return Ok((StatusCode::CREATED, Json(artifact_from_row(row)?)));
    }
    writer
        .expect("new Artifact owns a multipart upload")
        .finish()
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    let final_key = format!("runtime/{}/{}/{}", caller.tenant_id, artifact_id, hash);
    state
        .objects
        .copy(
            &ObjectPath::from(temporary_key.clone()),
            &ObjectPath::from(final_key.clone()),
        )
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    let response = ArtifactUploadResponseV1 {
        artifact_id,
        content_type: content_type.clone(),
        size_bytes,
        sha256: hash.clone(),
    };
    let commit_result = commit_artifact_upload_with_retry(
        &state,
        caller.tenant_id,
        artifact_id,
        &content_type,
        size_bytes,
        &hash,
        &final_key,
        &scope,
        key,
        request_hash.as_str(),
    )
    .await;
    if let Err(error) = commit_result {
        let _ = state.objects.delete(&ObjectPath::from(final_key)).await;
        let _ = state.objects.delete(&ObjectPath::from(temporary_key)).await;
        if let Some(row) = sqlx::query("SELECT id,content_type,size_bytes,sha256,request_hash FROM artifacts WHERE tenant_id=? AND idempotency_scope=? AND idempotency_key=?")
            .bind(caller.tenant_id).bind(&scope).bind(key).fetch_optional(&state.pool).await? {
            if row.try_get::<String, _>("request_hash")? != request_hash.as_str() {
                return Err(RuntimeError::Conflict(
                    agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
                    "Artifact idempotency key was reused with different content".into(),
                ));
            }
            return Ok((StatusCode::CREATED, Json(artifact_from_row(row)?)));
        }
        return Err(error);
    }
    let _ = state.objects.delete(&ObjectPath::from(temporary_key)).await;
    Ok((StatusCode::CREATED, Json(response)))
}

#[allow(clippy::too_many_arguments)]
async fn commit_artifact_upload_with_retry(
    state: &RuntimeState,
    tenant_id: Uuid,
    artifact_id: Uuid,
    content_type: &str,
    size_bytes: u64,
    hash: &str,
    final_key: &str,
    scope: &str,
    idempotency_key: &str,
    request_hash: &str,
) -> RuntimeResult<()> {
    const MAX_ATTEMPTS: u32 = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        match commit_artifact_upload(
            state,
            tenant_id,
            artifact_id,
            content_type,
            size_bytes,
            hash,
            final_key,
            scope,
            idempotency_key,
            request_hash,
        )
        .await
        {
            Err(RuntimeError::DatabaseUnavailable) if attempt < MAX_ATTEMPTS => {
                let backoff = Duration::from_millis(25 * u64::from(attempt));
                tracing::warn!(
                    %tenant_id,
                    %artifact_id,
                    commit_attempt = attempt,
                    backoff_milliseconds = backoff.as_millis(),
                    "Artifact commit hit a transient database failure; retrying"
                );
                tokio::time::sleep(backoff).await;
            }
            result => return result,
        }
    }
    unreachable!("Artifact commit retry loop always returns on its final attempt")
}

#[allow(clippy::too_many_arguments)]
async fn commit_artifact_upload(
    state: &RuntimeState,
    tenant_id: Uuid,
    artifact_id: Uuid,
    content_type: &str,
    size_bytes: u64,
    hash: &str,
    final_key: &str,
    scope: &str,
    idempotency_key: &str,
    request_hash: &str,
) -> RuntimeResult<()> {
    let mut tx = state.pool.begin().await?;
    crate::quota::reserve(
        &mut tx,
        tenant_id,
        "artifact_bytes",
        "artifact",
        &artifact_id.to_string(),
        &format!("artifact:{artifact_id}:bytes"),
        size_bytes,
        300,
    )
    .await?;
    sqlx::query("INSERT INTO artifacts(id,tenant_id,content_type,size_bytes,sha256,storage_key,idempotency_scope,idempotency_key,request_hash) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(artifact_id)
        .bind(tenant_id)
        .bind(content_type)
        .bind(size_bytes)
        .bind(hash)
        .bind(final_key)
        .bind(scope)
        .bind(idempotency_key)
        .bind(request_hash)
        .execute(&mut *tx)
        .await?;
    crate::quota::settle(
        &mut tx,
        tenant_id,
        "artifact_bytes",
        "artifact",
        &artifact_id.to_string(),
        size_bytes,
        &format!("artifact:{artifact_id}:usage:artifact_bytes"),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

fn artifact_request_hash(
    content_type: &str,
    size_bytes: u64,
    sha256: &str,
) -> RuntimeResult<agentx_runtime_contracts::ContentHash> {
    agentx_runtime_contracts::content_hash(
        &json!({"contentType":content_type,"sizeBytes":size_bytes,"sha256":sha256}),
    )
    .map_err(|error| RuntimeError::Internal(error.into()))
}

fn artifact_from_row(row: sqlx::mysql::MySqlRow) -> RuntimeResult<ArtifactUploadResponseV1> {
    Ok(ArtifactUploadResponseV1 {
        artifact_id: row.try_get("id")?,
        content_type: row.try_get("content_type")?,
        size_bytes: row.try_get("size_bytes")?,
        sha256: row.try_get("sha256")?,
    })
}

async fn webhook(
    State(state): State<RuntimeState>,
    Path(public_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> RuntimeResult<Response> {
    let key = idempotency_key(&headers).ok();
    let timestamp = headers
        .get("x-agentx-timestamp")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    let signature = headers
        .get("x-agentx-signature")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let row = sqlx::query("SELECT w.id,w.tenant_id,w.application_id,w.secret_ref_json,w.provider_type,w.input_mapping_json,w.fixed_inputs_json,w.configuration_revision,CAST(JSON_UNQUOTE(JSON_EXTRACT(t.configuration_json,'$.triggerName')) AS CHAR(255)) trigger_name FROM webhook_bindings w JOIN trigger_bindings t ON t.tenant_id=w.tenant_id AND t.id=w.id WHERE w.public_id=? AND w.status='active'")
        .bind(&public_id).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    let provider = match row
        .try_get::<Option<String>, _>("provider_type")?
        .as_deref()
        .unwrap_or("agentx")
    {
        "dingtalk" => WebhookProviderV1::Dingtalk,
        "wecom" => WebhookProviderV1::Wecom,
        "feishu" => WebhookProviderV1::Feishu,
        _ => WebhookProviderV1::Agentx,
    };
    if provider != WebhookProviderV1::Agentx {
        let secret_ref: VaultSecretReferenceV1 =
            serde_json::from_value(row.try_get("secret_ref_json")?)
                .map_err(|e| RuntimeError::Internal(e.into()))?;
        let secret = state
            .vault
            .as_ref()
            .ok_or(RuntimeError::SecretUnavailable)?
            .read(&secret_ref)
            .await?;
        let mappings: Vec<agentx_runtime_contracts::WebhookInputMappingV1> = row
            .try_get::<Option<Value>, _>("input_mapping_json")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| RuntimeError::Internal(e.into()))?
            .unwrap_or_default();
        let fixed = row
            .try_get::<Option<Value>, _>("fixed_inputs_json")?
            .unwrap_or_else(|| json!({}));
        let connection_id = row.try_get::<Uuid, _>("id")?.to_string();
        match crate::webhook::decode(
            provider,
            row.try_get("id")?,
            connection_id,
            &secret,
            &headers,
            &query,
            &body,
            &mappings,
            &fixed,
        ) {
            Ok(crate::webhook::WebhookDecode::Challenge(value)) => {
                if let Some(text) = value.as_str() {
                    return Ok((StatusCode::OK, text.to_owned()).into_response());
                }
                return Ok((StatusCode::OK, Json(value)).into_response());
            }
            Ok(crate::webhook::WebhookDecode::Ignore) => return Ok(StatusCode::OK.into_response()),
            Ok(crate::webhook::WebhookDecode::Event { context, input }) => {
                crate::webhook::dispatch_event(
                    &state.pool,
                    row.try_get("tenant_id")?,
                    row.try_get("application_id")?,
                    row.try_get("id")?,
                    row.try_get("trigger_name")?,
                    row.try_get("configuration_revision")?,
                    context,
                    input,
                )
                .await?;
                return Ok(StatusCode::OK.into_response());
            }
            Err(code) => {
                return Err(match code {
                    "UNAUTHORIZED" => RuntimeError::Unauthorized,
                    "INVALID_JSON" => {
                        RuntimeError::InvalidRequest("INVALID_JSON", "invalid JSON body".into())
                    }
                    "PROVIDER_EVENT_ID_REQUIRED" => RuntimeError::InvalidRequest(
                        "PROVIDER_EVENT_ID_REQUIRED",
                        "provider event ID is required".into(),
                    ),
                    "PROVIDER_CONVERSATION_ID_REQUIRED" => RuntimeError::InvalidRequest(
                        "PROVIDER_CONVERSATION_ID_REQUIRED",
                        "conversation ID is required".into(),
                    ),
                    other => RuntimeError::InvalidRequest(
                        other,
                        "provider webhook request is invalid".into(),
                    ),
                });
            }
        }
    }
    let key = key.ok_or(RuntimeError::Unauthorized)?;
    let timestamp = timestamp.ok_or(RuntimeError::Unauthorized)?;
    if (OffsetDateTime::now_utc().unix_timestamp() - timestamp).abs() > 300 {
        return Err(RuntimeError::Unauthorized);
    }
    let signature = signature.ok_or(RuntimeError::Unauthorized)?;
    let secret_ref: VaultSecretReferenceV1 =
        serde_json::from_value(row.try_get("secret_ref_json")?)
            .map_err(|e| RuntimeError::Internal(e.into()))?;
    let secret = state
        .vault
        .as_ref()
        .ok_or(RuntimeError::SecretUnavailable)?
        .read(&secret_ref)
        .await?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&secret).map_err(|e| RuntimeError::Internal(e.into()))?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(&body);
    let supplied = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| RuntimeError::Unauthorized)?;
    mac.verify_slice(&supplied)
        .map_err(|_| RuntimeError::Unauthorized)?;
    let input: Value = serde_json::from_slice(&body).map_err(|_| bad_request("INVALID_JSON"))?;
    let caller = Caller {
        tenant_id: row.try_get("tenant_id")?,
        application_id: row.try_get("application_id")?,
        subject_id: row.try_get("id")?,
        caller_type: "webhook",
        token_version: None,
        origin: agentx_runtime_contracts::ExecutionOriginV1 {
            trigger_source_id: Some(row.try_get("id")?),
            trigger_name: row.try_get("trigger_name")?,
            ..agentx_runtime_contracts::ExecutionOriginV1::system(None)
        },
    };
    let accepted = create_runtime_invocation(
        &state.pool,
        caller.tenant_id,
        caller.application_id,
        InvocationCaller {
            caller_type: "webhook",
            caller_id: caller.subject_id,
            token_version: None,
            origin: caller.origin.clone(),
        },
        None,
        &input,
        key,
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(load_invocation(&state, &caller, accepted.invocation_id).await?),
    )
        .into_response())
}

async fn authenticate(
    state: &RuntimeState,
    headers: &HeaderMap,
    route: Option<&str>,
) -> RuntimeResult<Caller> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(RuntimeError::Unauthorized)?;
    if token.starts_with("axk_") {
        let prefix = token.chars().take(12).collect::<String>();
        let row = sqlx::query("SELECT k.key_id,k.key_name,k.tenant_id,k.application_id,k.secret_hash,r.route_key FROM api_key_admission k JOIN application_routes r ON r.application_id=k.application_id AND r.tenant_id=k.tenant_id JOIN deployment_heads h ON h.application_id=k.application_id AND h.tenant_id=k.tenant_id JOIN tenant_admission t ON t.tenant_id=k.tenant_id WHERE k.key_prefix=? AND k.status='active' AND (k.expires_at IS NULL OR k.expires_at>UTC_TIMESTAMP(6)) AND r.status='active' AND r.active_bundle_id=h.bundle_id AND t.status='active'")
            .bind(prefix).fetch_optional(&state.pool).await?.ok_or(RuntimeError::Unauthorized)?;
        if route.is_some_and(|route| {
            row.try_get::<String, _>("route_key").ok().as_deref() != Some(route)
        }) {
            return Err(RuntimeError::Unauthorized);
        }
        verify_hash(token.as_bytes(), &row.try_get::<Vec<u8>, _>("secret_hash")?)?;
        Ok(Caller {
            tenant_id: row.try_get("tenant_id")?,
            application_id: row.try_get("application_id")?,
            subject_id: row.try_get("key_id")?,
            caller_type: "api_key",
            token_version: None,
            origin: agentx_runtime_contracts::ExecutionOriginV1 {
                trigger_source_id: Some(row.try_get("key_id")?),
                trigger_name: Some(row.try_get("key_name")?),
                ..agentx_runtime_contracts::ExecutionOriginV1::system(None)
            },
        })
    } else {
        let claims = state.trust.user(token)?;
        let row = sqlx::query("SELECT r.application_id,r.route_key FROM runtime_user_admission u JOIN runtime_user_application_grants g ON g.tenant_id=u.tenant_id AND g.user_id=u.user_id JOIN application_routes r ON r.tenant_id=g.tenant_id AND r.application_id=g.application_id JOIN deployment_heads h ON h.tenant_id=r.tenant_id AND h.application_id=r.application_id JOIN tenant_admission t ON t.tenant_id=r.tenant_id WHERE u.tenant_id=? AND u.user_id=? AND u.status='active' AND u.token_version=? AND g.status='active' AND g.can_invoke=TRUE AND r.status='active' AND r.active_bundle_id=h.bundle_id AND t.status='active' AND r.route_key=? LIMIT 1")
            .bind(claims.tenant_id).bind(claims.sub).bind(claims.token_version).bind(route.ok_or(RuntimeError::Unauthorized)?).fetch_optional(&state.pool).await?.ok_or(RuntimeError::Unauthorized)?;
        Ok(Caller {
            tenant_id: claims.tenant_id,
            application_id: row.try_get("application_id")?,
            subject_id: claims.sub,
            caller_type: "user",
            token_version: Some(claims.token_version),
            origin: claims.origin,
        })
    }
}

async fn authenticate_resource(
    state: &RuntimeState,
    headers: &HeaderMap,
    resource: &str,
    id: Uuid,
) -> RuntimeResult<Caller> {
    let token = bearer_token(headers)?;
    if token.starts_with("axk_") {
        return authenticate(state, headers, None).await;
    }
    let claims = state.trust.user(token)?;
    let application_id: Uuid = match resource {
        "session" => sqlx::query_scalar(
            "SELECT application_id FROM application_sessions WHERE tenant_id=? AND id=?",
        )
        .bind(claims.tenant_id)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(RuntimeError::NotFound)?,
        "invocation" => sqlx::query_scalar(
            "SELECT application_id FROM application_invocations WHERE tenant_id=? AND id=?",
        )
        .bind(claims.tenant_id)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(RuntimeError::NotFound)?,
        _ => return Err(RuntimeError::Unauthorized),
    };
    authorize_user_application(state, claims, application_id).await
}

async fn authenticate_artifact(state: &RuntimeState, headers: &HeaderMap) -> RuntimeResult<Caller> {
    let token = bearer_token(headers)?;
    if token.starts_with("axk_") {
        return authenticate(state, headers, None).await;
    }
    let claims = state.trust.user(token)?;
    let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_user_admission u JOIN tenant_admission t ON t.tenant_id=u.tenant_id WHERE u.tenant_id=? AND u.user_id=? AND u.status='active' AND u.token_version=? AND t.status='active')")
        .bind(claims.tenant_id).bind(claims.sub).bind(claims.token_version).fetch_one(&state.pool).await?;
    if !active {
        return Err(RuntimeError::Unauthorized);
    }
    Ok(Caller {
        tenant_id: claims.tenant_id,
        application_id: Uuid::nil(),
        subject_id: claims.sub,
        caller_type: "user",
        token_version: Some(claims.token_version),
        origin: claims.origin,
    })
}

async fn authorize_user_application(
    state: &RuntimeState,
    claims: agentx_runtime_contracts::UserAccessClaimsV1,
    application_id: Uuid,
) -> RuntimeResult<Caller> {
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_user_admission u JOIN runtime_user_application_grants g ON g.tenant_id=u.tenant_id AND g.user_id=u.user_id JOIN application_routes r ON r.tenant_id=g.tenant_id AND r.application_id=g.application_id JOIN tenant_admission t ON t.tenant_id=r.tenant_id WHERE u.tenant_id=? AND u.user_id=? AND u.status='active' AND u.token_version=? AND g.application_id=? AND g.status='active' AND g.can_invoke=TRUE AND r.status='active' AND t.status='active')")
        .bind(claims.tenant_id).bind(claims.sub).bind(claims.token_version).bind(application_id).fetch_one(&state.pool).await?;
    if !allowed {
        return Err(RuntimeError::Unauthorized);
    }
    Ok(Caller {
        tenant_id: claims.tenant_id,
        application_id,
        subject_id: claims.sub,
        caller_type: "user",
        token_version: Some(claims.token_version),
        origin: claims.origin,
    })
}

fn bearer_token(headers: &HeaderMap) -> RuntimeResult<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(RuntimeError::Unauthorized)
}

async fn load_session(
    state: &RuntimeState,
    caller: &Caller,
    id: Uuid,
) -> RuntimeResult<SessionResponseV1> {
    let row = sqlx::query("SELECT id,application_id,application_deployment_id,workflow_version_id,bundle_id,version_policy,external_user_id,title,status,version,updated_at FROM application_sessions WHERE tenant_id=? AND application_id=? AND id=?")
        .bind(caller.tenant_id).bind(caller.application_id).bind(id).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    Ok(SessionResponseV1 {
        id: row.try_get("id")?,
        application_id: row.try_get("application_id")?,
        application_deployment_id: row.try_get("application_deployment_id")?,
        workflow_version_id: row.try_get("workflow_version_id")?,
        bundle_id: row.try_get("bundle_id")?,
        version_policy: parse_policy(&row.try_get::<String, _>("version_policy")?)?,
        external_user_id: row.try_get("external_user_id")?,
        title: row.try_get("title")?,
        status: row.try_get("status")?,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_invocation(
    state: &RuntimeState,
    caller: &Caller,
    id: Uuid,
) -> RuntimeResult<InvocationResponseV1> {
    let row = sqlx::query("SELECT id,application_id,session_id,execution_id,bundle_id,admission_epoch,status,result_json,error_json,created_at,provider_event_id,conversation_id,trigger_context_json FROM application_invocations WHERE tenant_id=? AND application_id=? AND id=?")
        .bind(caller.tenant_id).bind(caller.application_id).bind(id).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    Ok(InvocationResponseV1 {
        id: row.try_get("id")?,
        application_id: row.try_get("application_id")?,
        session_id: row.try_get("session_id")?,
        execution_id: row.try_get("execution_id")?,
        bundle_id: row.try_get("bundle_id")?,
        admission_epoch: row.try_get("admission_epoch")?,
        status: row.try_get("status")?,
        outputs: row.try_get("result_json")?,
        error: row.try_get("error_json")?,
        provider: row
            .try_get::<Option<Value>, _>("trigger_context_json")?
            .and_then(|value| value.get("provider").cloned())
            .and_then(|value| serde_json::from_value(value).ok()),
        provider_event_id: row.try_get("provider_event_id")?,
        conversation_id: row.try_get("conversation_id")?,
        trigger_context: row.try_get("trigger_context_json")?,
        created_at: row.try_get("created_at")?,
    })
}

fn idempotency_key(headers: &HeaderMap) -> RuntimeResult<&str> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 192)
        .ok_or_else(|| bad_request("IDEMPOTENCY_KEY_REQUIRED"))
}
fn request_hash<T: serde::Serialize>(
    value: &T,
) -> RuntimeResult<agentx_runtime_contracts::ContentHash> {
    agentx_runtime_contracts::content_hash(value).map_err(|e| RuntimeError::Internal(e.into()))
}
async fn replay<T: serde::de::DeserializeOwned>(
    state: &RuntimeState,
    tenant: Uuid,
    scope: &str,
    key: &str,
    hash: &agentx_runtime_contracts::ContentHash,
) -> RuntimeResult<Option<T>> {
    let row = sqlx::query("SELECT request_hash,response_json FROM runtime_idempotency_keys WHERE tenant_id=? AND scope=? AND idempotency_key=? AND expires_at>UTC_TIMESTAMP(6)").bind(tenant).bind(scope).bind(key).fetch_optional(&state.pool).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<String, _>("request_hash")? != hash.as_str() {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Idempotency key was reused with another request".into(),
        ));
    }
    row.try_get::<Option<Value>, _>("response_json")?
        .map(|value| serde_json::from_value(value).map_err(|e| RuntimeError::Internal(e.into())))
        .transpose()
}
async fn persist_idempotency_tx<T: serde::Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    scope: &str,
    key: &str,
    hash: &agentx_runtime_contracts::ContentHash,
    status: StatusCode,
    response: &T,
) -> RuntimeResult<()> {
    sqlx::query("INSERT INTO runtime_idempotency_keys(tenant_id,scope,idempotency_key,request_hash,status,http_status,response_json,expires_at) VALUES(?,?,?,?,'completed',?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? DAY))")
        .bind(tenant).bind(scope).bind(key).bind(hash.as_str()).bind(status.as_u16()).bind(serde_json::to_value(response).map_err(|e| RuntimeError::Internal(e.into()))?).bind(IDEMPOTENCY_RETENTION_DAYS).execute(&mut **tx).await?;
    Ok(())
}
fn parse_policy(value: &str) -> RuntimeResult<SessionVersionPolicyV1> {
    match value {
        "pinned" => Ok(SessionVersionPolicyV1::Pinned),
        "follow_deployment" => Ok(SessionVersionPolicyV1::FollowDeployment),
        "manual_upgrade" => Ok(SessionVersionPolicyV1::ManualUpgrade),
        _ => Err(RuntimeError::Internal(anyhow::anyhow!(
            "invalid Session policy"
        ))),
    }
}
fn terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}
fn bad_request(code: &'static str) -> RuntimeError {
    RuntimeError::InvalidRequest(code, code.into())
}
fn verify_hash(actual: &[u8], expected: &[u8]) -> RuntimeResult<()> {
    let actual = Sha256::digest(actual);
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(expected)
            .fold(0u8, |sum, (a, b)| sum | (a ^ b))
            != 0
    {
        Err(RuntimeError::Unauthorized)
    } else {
        Ok(())
    }
}
pub fn public_openapi() -> Value {
    let mut schemas = Map::new();
    insert_public_schema::<CreateSessionRequestV1>(&mut schemas, "CreateSessionRequest");
    insert_public_schema::<SessionResponseV1>(&mut schemas, "SessionResponse");
    insert_public_schema::<InvocationRequestV1>(&mut schemas, "InvocationRequest");
    insert_public_schema::<InvocationResponseV1>(&mut schemas, "InvocationResponse");
    insert_public_schema::<MessagePartInputV1>(&mut schemas, "MessagePartInput");
    insert_public_schema::<MessageRequestV1>(&mut schemas, "MessageRequest");
    insert_public_schema::<MessageResponseV1>(&mut schemas, "MessageResponse");
    insert_public_schema::<ArtifactUploadResponseV1>(&mut schemas, "ArtifactUploadResponse");
    insert_public_schema::<CommandAcceptedV1>(&mut schemas, "CommandAccepted");
    insert_public_schema::<GatewayErrorV1>(&mut schemas, "GatewayError");
    json!({
        "openapi":"3.1.0",
        "info":{"title":"Agentx Runtime Gateway API","version":"1","description":"Production Runtime ingress. All write operations require Idempotency-Key."},
        "paths":{
            "/gateway/v1/applications/{slug}/sessions":{"post":public_post("createSession","CreateSessionRequest","SessionResponse","201")},
            "/gateway/v1/sessions/{id}":{"get":public_get("getSession","SessionResponse")},
            "/gateway/v1/sessions/{id}/messages":{
                "get":public_get_array("listMessages","MessageResponse"),
                "post":public_post("sendMessage","MessageRequest","InvocationResponse","202")
            },
            "/gateway/v1/applications/{slug}/invocations":{"post":public_post("createInvocation","InvocationRequest","InvocationResponse","202")},
            "/gateway/v1/workflows/{slug}/invoke":{"post":public_invoke()},
            "/gateway/v1/artifacts":{"post":{
                "operationId":"uploadArtifact",
                "parameters":[idempotency_parameter()],
                "requestBody":{"required":true,"content":{"multipart/form-data":{"schema":{"type":"object","required":["content"],"properties":{"content":{"type":"string","format":"binary"}}}}}},
                "responses":public_responses("ArtifactUploadResponse","201"),
                "security":[{"runtimeBearer":[]}]
            }},
            "/gateway/v1/invocations/{id}":{"get":public_get("getInvocation","InvocationResponse")},
            "/gateway/v1/invocations/{id}/cancel":{"post":public_command("cancelInvocation")},
            "/gateway/v1/invocations/{id}/events":{"get":{
                "operationId":"streamInvocationEvents",
                "parameters":[{"name":"Last-Event-ID","in":"header","required":false,"schema":{"type":"integer","minimum":0}}],
                "responses":{"200":{"description":"MySQL-cursor Server-Sent Event stream","content":{"text/event-stream":{"schema":{"type":"string"}}}},"401":error_response(),"503":error_response()},
                "security":[{"runtimeBearer":[]}]
            }},
            "/gateway/v1/webhooks/{public_id}":{"post":{
                "operationId":"invokeWebhook",
                "parameters":[{"name":"timestamp","in":"query","required":false,"schema":{"type":"string"}},{"name":"nonce","in":"query","required":false,"schema":{"type":"string"}},{"name":"signature","in":"query","required":false,"schema":{"type":"string"}},{"name":"msg_signature","in":"query","required":false,"schema":{"type":"string"}},{"name":"X-Lark-Request-Timestamp","in":"header","required":false,"schema":{"type":"string"}},{"name":"X-Lark-Request-Nonce","in":"header","required":false,"schema":{"type":"string"}},{"name":"X-Lark-Signature","in":"header","required":false,"schema":{"type":"string"}}],
                "requestBody":{"required":true,"content":{"application/json":{"schema":{}},"application/xml":{"schema":{"type":"string"}}}},
                "responses":{"200":{"description":"Provider-compatible acknowledgement"},"400":error_response(),"401":error_response(),"404":error_response(),"409":error_response(),"422":error_response(),"503":error_response()}
            },"get":{
                "operationId":"verifyWebhook",
                "parameters":[{"name":"timestamp","in":"query","required":true,"schema":{"type":"string"}},{"name":"nonce","in":"query","required":true,"schema":{"type":"string"}},{"name":"echostr","in":"query","required":true,"schema":{"type":"string"}},{"name":"msg_signature","in":"query","required":false,"schema":{"type":"string"}}],
                "responses":{"200":{"description":"Provider challenge acknowledgement"},"400":error_response(),"401":error_response(),"404":error_response(),"503":error_response()}
            }},
        },
        "components":{"securitySchemes":{"runtimeBearer":{"type":"http","scheme":"bearer","bearerFormat":"RS256 JWT or Agentx API Key"}},"schemas":schemas}
    })
}

fn insert_public_schema<T: JsonSchema>(schemas: &mut Map<String, Value>, name: &str) {
    let mut schema = serde_json::to_value(schema_for!(T)).expect("public schema serializes");
    rewrite_local_definitions(&mut schema, name);
    schemas.insert(name.to_owned(), schema);
}

fn rewrite_local_definitions(value: &mut Value, component: &str) {
    match value {
        Value::String(reference) if reference.starts_with("#/$defs/") => {
            *reference = format!(
                "#/components/schemas/{component}/$defs/{}",
                reference.trim_start_matches("#/$defs/")
            );
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| rewrite_local_definitions(value, component)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| rewrite_local_definitions(value, component)),
        _ => {}
    }
}

fn public_post(operation: &str, request: &str, response: &str, status: &str) -> Value {
    json!({
        "operationId":operation,
        "parameters":[idempotency_parameter()],
        "requestBody":{"required":true,"content":{"application/json":{"schema":schema_reference(request)}}},
        "responses":public_responses(response,status),
        "security":[{"runtimeBearer":[]}]
    })
}

fn public_invoke() -> Value {
    json!({
        "operationId":"invokeWorkflow",
        "parameters":[idempotency_parameter()],
        "requestBody":{"required":true,"content":{"application/json":{"schema":schema_reference("InvocationRequest")}}},
        "responses":public_responses("InvocationResponse","202"),
        "security":[{"runtimeBearer":[]}]
    })
}

fn public_get(operation: &str, response: &str) -> Value {
    json!({"operationId":operation,"responses":public_responses(response,"200"),"security":[{"runtimeBearer":[]}]})
}

fn public_get_array(operation: &str, item: &str) -> Value {
    let mut responses = Map::new();
    responses.insert("200".into(), json!({
        "description":"Successful Runtime response",
        "content":{"application/json":{"schema":{"type":"array","items":schema_reference(item)}}}
    }));
    for status in ["400", "401", "404", "409", "422", "503"] {
        responses.insert(status.into(), error_response());
    }
    json!({"operationId":operation,"responses":responses,"security":[{"runtimeBearer":[]}]})
}

fn public_command(operation: &str) -> Value {
    json!({"operationId":operation,"parameters":[idempotency_parameter()],"responses":public_responses("CommandAccepted","202"),"security":[{"runtimeBearer":[]}]})
}

fn public_responses(schema: &str, status: &str) -> Value {
    let mut responses = Map::new();
    responses.insert(status.to_owned(), json!({"description":"Successful Runtime response","content":{"application/json":{"schema":schema_reference(schema)}}}));
    for status in ["400", "401", "404", "409", "422", "503"] {
        responses.insert(status.to_owned(), error_response());
    }
    Value::Object(responses)
}

fn error_response() -> Value {
    json!({"description":"Runtime error","content":{"application/json":{"schema":schema_reference("GatewayError")}}})
}

fn schema_reference(schema: &str) -> Value {
    json!({"$ref":format!("#/components/schemas/{schema}")})
}

fn idempotency_parameter() -> Value {
    json!({"name":"Idempotency-Key","in":"header","required":true,"schema":{"type":"string","minLength":1,"maxLength":192}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_message_builds_mapping_neutral_chat_payload() {
        let input = chat_message_payload(&MessageRequestV1 {
            parts: vec![MessagePartInputV1 {
                part_type: "text".into(),
                content: Some(json!("agentx-v2")),
                artifact_id: None,
            }],
        });
        assert_eq!(input, json!({"question":"agentx-v2","files":[]}));
    }

    #[test]
    fn session_title_uses_normalized_first_question_and_a_bounded_ellipsis() {
        let request = MessageRequestV1 {
            parts: vec![MessagePartInputV1 {
                part_type: "text".into(),
                content: Some(json!(format!("  first\nquestion   {}", "x".repeat(300)))),
                artifact_id: None,
            }],
        };
        let title = inferred_session_title(&request).unwrap();
        assert!(title.starts_with("first question "));
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().count(), 255);
    }

    #[test]
    fn workflow_invocation_accepts_only_json_artifact_references() {
        let openapi = public_openapi();
        let content = &openapi["paths"]["/gateway/v1/workflows/{slug}/invoke"]["post"]["requestBody"]
            ["content"];
        assert!(content.get("application/json").is_some());
        assert!(content.get("multipart/form-data").is_none());
    }
}
