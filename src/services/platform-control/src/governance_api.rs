use std::collections::{BTreeMap, BTreeSet};

use agentx_runtime_contracts::{
    AdmissionTargetV1, ApplyReceiptV1, ApprovalActionValueV1, CommandEnvelopeV1, ControlRole,
    Plane, RuntimeAdmissionCommandV1, RuntimeApprovalActionV1, ServiceClaimsV1, content_hash,
    issue_service_token, now_unix,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    routing::{get, post},
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};

const PROJECTION: &str = "runtime_governance_v1";
const PARTITION: &str = "global";

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/approvals", get(list_approvals))
        .route("/api/v1/approvals/{id}", get(get_approval))
        .route("/api/v1/approvals/{id}/actions", get(list_approval_actions))
        .route(
            "/api/v1/approvals/{id}/candidates",
            get(list_approval_candidates),
        )
        .route("/api/v1/approvals/{id}/claim", post(claim_approval))
        .route("/api/v1/approvals/{id}/release", post(release_approval))
        .route("/api/v1/approvals/{id}/reassign", post(reassign_approval))
        .route("/api/v1/approvals/{id}/decide", post(decide_approval))
        .route("/api/v1/approvals/{id}/cancel", post(cancel_approval))
        .route("/api/v1/evaluations", get(list_evaluations))
        .route("/api/v1/evaluations/compare", get(compare_evaluations))
        .route(
            "/api/v1/evaluations/{id}/report",
            get(get_evaluation_report),
        )
        .route("/api/v1/notifications", get(list_notifications))
        .route(
            "/api/v1/notifications/read-all",
            post(read_all_notifications),
        )
        .route("/api/v1/notifications/{id}/read", post(read_notification))
        .route(
            "/api/v1/retention-runs",
            get(list_retention_runs).post(create_retention_run),
        )
        .route(
            "/api/v1/retention-runs/{id}/items",
            get(list_retention_items),
        )
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectionView {
    pub(crate) state: String,
    pub(crate) cursor: u64,
    pub(crate) generation: u64,
}

pub(crate) async fn projection(pool: &MySqlPool) -> ApiResult<ProjectionView> {
    let row = sqlx::query("SELECT state,current_cursor,active_generation FROM runtime_projection_status WHERE projection_name=? AND partition_key=?")
        .bind(PROJECTION)
        .bind(PARTITION)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Err(ApiError::unavailable(
            "PROJECTION_REBUILDING",
            "Runtime governance projection has not been initialized",
        ));
    };
    let generation: u64 = row.try_get("active_generation")?;
    if generation == 0 {
        return Err(ApiError::unavailable(
            "PROJECTION_REBUILDING",
            "Runtime governance projection is rebuilding",
        ));
    }
    Ok(ProjectionView {
        state: row.try_get("state")?,
        cursor: row.try_get("current_cursor")?,
        generation,
    })
}

pub(crate) fn projection_headers(view: &ProjectionView) -> ApiResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("x-agentx-projection-state"),
        HeaderValue::from_str(&view.state).map_err(ApiError::internal)?,
    );
    headers.insert(
        HeaderName::from_static("x-agentx-projection-cursor"),
        HeaderValue::from_str(&view.cursor.to_string()).map_err(ApiError::internal)?,
    );
    Ok(headers)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    status: Option<String>,
    search: Option<String>,
}

async fn list_approvals(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("approval:view")?;
    let view = projection(&state.pool).await?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let status = query.status.unwrap_or_default();
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let rows = sqlx::query("SELECT a.id,a.execution_id,a.workflow_id,CONVERT(COALESCE(w.name,BIN_TO_UUID(a.workflow_id)) USING utf8mb4) workflow_name,a.node_id,a.title,a.description,a.request_payload_json,a.buttons_json,a.decision_json,a.status,a.resume_status,a.claimed_by,u.display_name claimed_by_name,a.deadline_at,a.version,a.created_at FROM approval_task_projection a LEFT JOIN workflows w ON w.tenant_id=a.tenant_id AND w.id=a.workflow_id LEFT JOIN users u ON u.tenant_id=a.tenant_id AND u.id=a.claimed_by WHERE a.tenant_id=? AND a.projection_generation=? AND a.projection_deleted=FALSE AND (?='' OR a.status=?) AND (?='%%' OR a.title LIKE ? OR w.name LIKE ?) ORDER BY a.created_at DESC,a.id DESC LIMIT ? OFFSET ?")
        .bind(actor.tenant_id).bind(view.generation).bind(&status).bind(&status).bind(&search).bind(&search).bind(&search)
        .bind(page_size).bind(u64::from((page - 1) * page_size)).fetch_all(&state.pool).await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM approval_task_projection a LEFT JOIN workflows w ON w.tenant_id=a.tenant_id AND w.id=a.workflow_id WHERE a.tenant_id=? AND a.projection_generation=? AND a.projection_deleted=FALSE AND (?='' OR a.status=?) AND (?='%%' OR a.title LIKE ? OR w.name LIKE ?)")
        .bind(actor.tenant_id).bind(view.generation).bind(&status).bind(&status).bind(&search).bind(&search).bind(&search).fetch_one(&state.pool).await?;
    let items = rows
        .into_iter()
        .map(approval_json)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok((
        projection_headers(&view)?,
        Json(json!({"items":items,"page":page,"pageSize":page_size,"total":total})),
    ))
}

async fn get_approval(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("approval:view")?;
    let view = projection(&state.pool).await?;
    let row = approval_row(&state.pool, actor.tenant_id, id, view.generation).await?;
    Ok((projection_headers(&view)?, Json(approval_json(row)?)))
}

async fn approval_row(
    pool: &MySqlPool,
    tenant_id: Uuid,
    id: Uuid,
    generation: u64,
) -> ApiResult<sqlx::mysql::MySqlRow> {
    sqlx::query("SELECT a.id,a.execution_id,a.workflow_id,CONVERT(COALESCE(w.name,BIN_TO_UUID(a.workflow_id)) USING utf8mb4) workflow_name,a.node_id,a.title,a.description,a.request_payload_json,a.buttons_json,a.decision_json,a.status,a.resume_status,a.claimed_by,u.display_name claimed_by_name,a.deadline_at,a.version,a.created_at FROM approval_task_projection a LEFT JOIN workflows w ON w.tenant_id=a.tenant_id AND w.id=a.workflow_id LEFT JOIN users u ON u.tenant_id=a.tenant_id AND u.id=a.claimed_by WHERE a.tenant_id=? AND a.id=? AND a.projection_generation=? AND a.projection_deleted=FALSE")
        .bind(tenant_id).bind(id).bind(generation).fetch_optional(pool).await?.ok_or_else(||ApiError::not_found("Approval"))
}

fn approval_json(row: sqlx::mysql::MySqlRow) -> ApiResult<Value> {
    Ok(json!({
        "id":row.try_get::<Uuid,_>("id")?,"executionId":row.try_get::<Uuid,_>("execution_id")?,
        "workflowId":row.try_get::<Uuid,_>("workflow_id")?,"workflowName":row.try_get::<String,_>("workflow_name")?,
        "nodeId":row.try_get::<String,_>("node_id")?,"title":row.try_get::<String,_>("title")?,
        "description":row.try_get::<Option<String>,_>("description")?,"requestPayload":row.try_get::<Option<Value>,_>("request_payload_json")?,
        "buttons":row.try_get::<Value,_>("buttons_json")?,"decision":row.try_get::<Option<Value>,_>("decision_json")?,
        "status":row.try_get::<String,_>("status")?,"resumeStatus":row.try_get::<String,_>("resume_status")?,
        "claimedBy":row.try_get::<Option<Uuid>,_>("claimed_by")?,"claimedByName":row.try_get::<Option<String>,_>("claimed_by_name")?,
        "deadlineAt":row.try_get::<Option<OffsetDateTime>,_>("deadline_at")?,"version":row.try_get::<u64,_>("version")?,
        "createdAt":row.try_get::<OffsetDateTime,_>("created_at")?
    }))
}

async fn list_approval_actions(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("approval:view")?;
    let view = projection(&state.pool).await?;
    approval_row(&state.pool, actor.tenant_id, id, view.generation).await?;
    let rows = sqlx::query("SELECT a.id,a.action_type,a.actor_user_id,CONVERT(COALESCE(u.display_name,BIN_TO_UUID(a.actor_user_id)) USING utf8mb4) actor_name,a.from_status,a.to_status,a.input_json,a.created_at FROM approval_action_submissions a LEFT JOIN users u ON u.tenant_id=a.tenant_id AND u.id=a.actor_user_id WHERE a.tenant_id=? AND a.approval_task_id=? ORDER BY a.created_at,a.id")
        .bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    let items = rows.into_iter().map(|row| -> ApiResult<Value> { Ok(json!({
        "id":row.try_get::<Uuid,_>("id")?,"actionType":row.try_get::<String,_>("action_type")?,
        "actorUserId":row.try_get::<Uuid,_>("actor_user_id")?,"actorName":row.try_get::<String,_>("actor_name")?,
        "fromStatus":row.try_get::<String,_>("from_status")?,"toStatus":row.try_get::<String,_>("to_status")?,
        "input":row.try_get::<Option<Value>,_>("input_json")?,"createdAt":row.try_get::<OffsetDateTime,_>("created_at")?
    })) }).collect::<ApiResult<Vec<_>>>()?;
    Ok((projection_headers(&view)?, Json(json!(items))))
}

async fn list_approval_candidates(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("approval:view")?;
    let view = projection(&state.pool).await?;
    approval_row(&state.pool, actor.tenant_id, id, view.generation).await?;
    let rows = sqlx::query("SELECT DISTINCT u.id,u.display_name FROM users u JOIN approval_candidate_projection c ON c.tenant_id=u.tenant_id AND c.approval_task_id=? AND c.projection_generation=? WHERE u.tenant_id=? AND u.status='active' AND ((c.candidate_type='user' AND c.candidate_id=u.id) OR (c.candidate_type='role' AND EXISTS(SELECT 1 FROM user_roles ur WHERE ur.tenant_id=u.tenant_id AND ur.user_id=u.id AND ur.role_id=c.candidate_id)) OR (c.candidate_type='department' AND EXISTS(SELECT 1 FROM user_departments ud JOIN department_closure dc ON dc.tenant_id=ud.tenant_id AND dc.ancestor_id=c.candidate_id AND dc.descendant_id=ud.department_id WHERE ud.tenant_id=u.tenant_id AND ud.user_id=u.id))) ORDER BY u.display_name,u.id")
        .bind(id).bind(view.generation).bind(actor.tenant_id).fetch_all(&state.pool).await?;
    let items=rows.into_iter().map(|row|->ApiResult<Value>{Ok(json!({"userId":row.try_get::<Uuid,_>("id")?,"displayName":row.try_get::<String,_>("display_name")?}))}).collect::<ApiResult<Vec<_>>>()?;
    Ok((projection_headers(&view)?, Json(json!(items))))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovalActionRequest {
    version: u64,
    target_user_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovalDecideRequest {
    version: u64,
    decision_id: String,
    reason: Option<String>,
    idempotency_key: String,
}

struct ApprovalActionInvocation<'a> {
    value: ApprovalActionValueV1,
    name: &'a str,
    input: ApprovalActionRequest,
    action_input: Option<Value>,
    requested_idempotency_key: Option<&'a str>,
}

async fn apply_approval_action(
    state: &ControlApiState,
    actor: Actor,
    id: Uuid,
    invocation: ApprovalActionInvocation<'_>,
) -> ApiResult<Json<Value>> {
    let ApprovalActionInvocation {
        value: action_value,
        name: action_name,
        input,
        action_input,
        requested_idempotency_key,
    } = invocation;
    actor.require(
        if matches!(
            &action_value,
            ApprovalActionValueV1::Reassign
                | ApprovalActionValueV1::Cancel
                | ApprovalActionValueV1::Timeout
        ) {
            "approval:manage"
        } else {
            "approval:act"
        },
    )?;
    let target = AdmissionTargetV1::ApprovalAction {
        state: RuntimeApprovalActionV1 {
            task_id: id,
            task_version: input.version,
            action: action_value.clone(),
            actor_id: actor.user_id,
            target_user_id: input.target_user_id,
            input: action_input.clone(),
        },
    };
    let idempotency_key = requested_idempotency_key.map_or_else(
        || {
            format!(
                "approval:{id}:{}:{action_name}:{}",
                input.version, actor.user_id
            )
        },
        |key| format!("approval:{id}:decide:{key}"),
    );
    let request_hash = content_hash(&target).map_err(ApiError::internal)?;
    let command_id = deterministic_id(&idempotency_key);
    let existing = sqlx::query("SELECT request_hash,response_json,from_status FROM approval_action_submissions WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(command_id)
        .fetch_optional(&state.pool)
        .await?;
    if let Some(existing) = &existing {
        if existing.try_get::<String, _>("request_hash")? != request_hash.as_str() {
            return Err(ApiError::conflict(
                "APPROVAL_IDEMPOTENCY_CONFLICT",
                "idempotencyKey was reused with a different Approval decision",
            ));
        }
        if let Some(response) = existing.try_get::<Option<Value>, _>("response_json")? {
            return Ok(Json(response));
        }
    }
    let view = projection(&state.pool).await?;
    let row = approval_row(&state.pool, actor.tenant_id, id, view.generation).await?;
    let projected_version: u64 = row.try_get("version")?;
    let projected_status: String = row.try_get("status")?;
    let projected_claimed_by: Option<Uuid> = row.try_get("claimed_by")?;
    let (current_version, from_status, claimed_by) = approval_action_state(
        &state.pool,
        actor.tenant_id,
        id,
        projected_version,
        projected_status,
        projected_claimed_by,
    )
    .await?;
    if existing.is_none() && input.version != current_version {
        return Err(ApiError::conflict(
            "APPROVAL_STATE_CONFLICT",
            "Approval changed on the server",
        ));
    }
    if existing.is_none() {
        match action_value {
            ApprovalActionValueV1::Claim => {
                require_approval_candidate(&state.pool, &actor, id, view.generation).await?;
            }
            ApprovalActionValueV1::Release => {
                if claimed_by != Some(actor.user_id) {
                    return Err(ApiError::forbidden(
                        "Only the current claimant can release this Approval",
                    ));
                }
            }
            ApprovalActionValueV1::Decide { .. } => {
                require_approval_candidate(&state.pool, &actor, id, view.generation).await?;
                if claimed_by != Some(actor.user_id) {
                    return Err(ApiError::forbidden(
                        "Only the current claimant can decide this Approval",
                    ));
                }
            }
            ApprovalActionValueV1::Reassign => {
                let target = input.target_user_id.ok_or_else(|| {
                    ApiError::bad_request("APPROVAL_TARGET_REQUIRED", "A target user is required")
                })?;
                require_candidate_user(&state.pool, actor.tenant_id, id, target, view.generation)
                    .await?;
            }
            ApprovalActionValueV1::Cancel | ApprovalActionValueV1::Timeout => {}
        }
    }
    let to_status = action_status(&action_value);
    let next_claimed_by = match &action_value {
        ApprovalActionValueV1::Claim => Some(actor.user_id),
        ApprovalActionValueV1::Release => None,
        ApprovalActionValueV1::Reassign => input.target_user_id,
        _ => claimed_by,
    };
    let from_status = if let Some(existing) = &existing {
        existing.try_get::<String, _>("from_status")?
    } else {
        from_status
    };
    sqlx::query("INSERT INTO approval_action_submissions(id,tenant_id,approval_task_id,actor_user_id,action_type,input_json,idempotency_key,request_hash,runtime_command_id,task_version,from_status,to_status) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id")
        .bind(command_id).bind(actor.tenant_id).bind(id).bind(actor.user_id).bind(action_name).bind(&action_input).bind(&idempotency_key).bind(request_hash.as_str()).bind(command_id).bind(input.version).bind(&from_status).bind(&to_status).execute(&state.pool).await?;
    let command = RuntimeAdmissionCommandV1 {
        api_version: 1,
        command: CommandEnvelopeV1 {
            schema_version: 1,
            event_id: command_id,
            source_plane: Plane::Control,
            tenant_id: actor.tenant_id,
            aggregate_type: "approval".into(),
            aggregate_id: id.to_string(),
            object_version: input.version,
            occurred_at: OffsetDateTime::now_utc(),
            payload: json!({}),
            content_hash: request_hash,
            correlation_id: id,
            causation_id: None,
            idempotency_key: idempotency_key.clone(),
        },
        admission_epoch: input.version.max(1),
        target,
    };
    let receipt = post_runtime_admission(state, &command).await?;
    let mut response = approval_json(row)?;
    if let Some(object) = response.as_object_mut() {
        object.insert("status".into(), json!(to_status));
        object.insert("version".into(), json!(input.version + 1));
        object.insert("claimedBy".into(), json!(next_claimed_by));
    }
    sqlx::query("UPDATE approval_action_submissions SET runtime_receipt_json=?,response_json=? WHERE tenant_id=? AND id=?")
        .bind(serde_json::to_value(&receipt).map_err(ApiError::internal)?).bind(&response).bind(actor.tenant_id).bind(command_id).execute(&state.pool).await?;
    Ok(Json(response))
}

async fn approval_action_state(
    pool: &MySqlPool,
    tenant_id: Uuid,
    task_id: Uuid,
    projected_version: u64,
    projected_status: String,
    projected_claimed_by: Option<Uuid>,
) -> ApiResult<(u64, String, Option<Uuid>)> {
    let latest: Option<Value> = sqlx::query_scalar(
        "SELECT response_json FROM approval_action_submissions WHERE tenant_id=? AND approval_task_id=? AND runtime_receipt_json IS NOT NULL AND response_json IS NOT NULL AND task_version>=? ORDER BY task_version DESC,created_at DESC,id DESC LIMIT 1",
    )
    .bind(tenant_id)
    .bind(task_id)
    .bind(projected_version)
    .fetch_optional(pool)
    .await?;
    let Some(latest) = latest else {
        return Ok((projected_version, projected_status, projected_claimed_by));
    };
    let version = latest
        .get("version")
        .and_then(Value::as_u64)
        .unwrap_or(projected_version);
    if version <= projected_version {
        return Ok((projected_version, projected_status, projected_claimed_by));
    }
    let status = latest
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or(&projected_status)
        .to_owned();
    let claimed_by = latest
        .get("claimedBy")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    Ok((version, status, claimed_by))
}

macro_rules! approval_action_handler {
    ($name:ident, $action:ident, $action_name:literal) => {
        async fn $name(
            State(state): State<ControlApiState>,
            actor: Actor,
            Path(id): Path<Uuid>,
            Json(input): Json<ApprovalActionRequest>,
        ) -> ApiResult<Json<Value>> {
            apply_approval_action(
                &state,
                actor,
                id,
                ApprovalActionInvocation {
                    value: ApprovalActionValueV1::$action,
                    name: $action_name,
                    input,
                    action_input: None,
                    requested_idempotency_key: None,
                },
            )
            .await
        }
    };
}

approval_action_handler!(claim_approval, Claim, "claim");
approval_action_handler!(release_approval, Release, "release");
approval_action_handler!(reassign_approval, Reassign, "reassign");
approval_action_handler!(cancel_approval, Cancel, "cancel");

async fn decide_approval(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<ApprovalDecideRequest>,
) -> ApiResult<Json<Value>> {
    if input.decision_id.is_empty()
        || !input
            .decision_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::bad_request(
            "INVALID_APPROVAL_DECISION",
            "decisionId is invalid",
        ));
    }
    let idempotency_key = input.idempotency_key.trim();
    if idempotency_key.is_empty() || idempotency_key.len() > 200 {
        return Err(ApiError::bad_request(
            "INVALID_IDEMPOTENCY_KEY",
            "idempotencyKey must contain between 1 and 200 characters",
        ));
    }
    apply_approval_action(
        &state,
        actor,
        id,
        ApprovalActionInvocation {
            value: ApprovalActionValueV1::Decide {
                decision_id: input.decision_id,
            },
            name: "decide",
            input: ApprovalActionRequest {
                version: input.version,
                target_user_id: None,
            },
            action_input: Some(json!({"reason":input.reason})),
            requested_idempotency_key: Some(idempotency_key),
        },
    )
    .await
}

async fn require_approval_candidate(
    pool: &MySqlPool,
    actor: &Actor,
    task_id: Uuid,
    generation: u64,
) -> ApiResult<()> {
    require_candidate_user(pool, actor.tenant_id, task_id, actor.user_id, generation).await
}

async fn require_candidate_user(
    pool: &MySqlPool,
    tenant_id: Uuid,
    task_id: Uuid,
    user_id: Uuid,
    generation: u64,
) -> ApiResult<()> {
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM approval_candidate_projection c WHERE c.tenant_id=? AND c.approval_task_id=? AND c.projection_generation=? AND ((c.candidate_type='user' AND c.candidate_id=?) OR (c.candidate_type='role' AND EXISTS(SELECT 1 FROM user_roles ur WHERE ur.tenant_id=c.tenant_id AND ur.user_id=? AND ur.role_id=c.candidate_id)) OR (c.candidate_type='department' AND EXISTS(SELECT 1 FROM user_departments ud JOIN department_closure dc ON dc.tenant_id=ud.tenant_id AND dc.descendant_id=ud.department_id WHERE ud.tenant_id=c.tenant_id AND ud.user_id=? AND dc.ancestor_id=c.candidate_id))))")
        .bind(tenant_id).bind(task_id).bind(generation).bind(user_id).bind(user_id).bind(user_id).fetch_one(pool).await?;
    if allowed {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "The user is not an eligible Approval candidate",
        ))
    }
}

fn action_status(value: &ApprovalActionValueV1) -> String {
    match value {
        ApprovalActionValueV1::Claim | ApprovalActionValueV1::Reassign => "claimed".into(),
        ApprovalActionValueV1::Release => "pending".into(),
        ApprovalActionValueV1::Decide { .. } => "decided".into(),
        ApprovalActionValueV1::Cancel => "cancelled".into(),
        ApprovalActionValueV1::Timeout => "timed_out".into(),
    }
}

pub(crate) async fn post_runtime_admission(
    state: &ControlApiState,
    command: &RuntimeAdmissionCommandV1,
) -> ApiResult<ApplyReceiptV1> {
    let now = now_unix();
    let token = issue_service_token(
        &state.runtime_command_kid,
        state.runtime_command_key.expose_secret().as_bytes(),
        &ServiceClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: "platform-control-governance".into(),
            role: ControlRole::Publisher,
            scope: BTreeSet::from(["runtime.admission.apply".into()]),
            iat: now,
            exp: now + 60,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/admission-commands:apply",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(command)
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error,"Runtime governance command is unavailable");
            ApiError::unavailable("RUNTIME_UNAVAILABLE", "Runtime service is unavailable")
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(%status,response_body=%body,"Runtime rejected governance command");
        return Err(ApiError::conflict(
            "RUNTIME_COMMAND_REJECTED",
            "Runtime rejected the governance command",
        ));
    }
    response.json().await.map_err(ApiError::internal)
}

async fn list_evaluations(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("evaluation:view")?;
    let view = projection(&state.pool).await?;
    let rows=sqlx::query("SELECT er.id,er.name,er.workflow_version_id,CONVERT(COALESCE(w.name,BIN_TO_UUID(er.workflow_version_id)) USING utf8mb4) workflow_name,er.dataset_version_id,CONVERT(COALESCE(d.name,BIN_TO_UUID(er.dataset_version_id)) USING utf8mb4) dataset_name,er.evaluation_profile_version_id,er.visibility,er.owner_department_id,er.status,er.completed_cases result_count,er.parameters_json,er.created_at FROM evaluation_runs er LEFT JOIN workflow_versions wv ON wv.tenant_id=er.tenant_id AND wv.id=er.workflow_version_id LEFT JOIN workflows w ON w.tenant_id=er.tenant_id AND w.id=wv.workflow_id LEFT JOIN dataset_versions dv ON dv.tenant_id=er.tenant_id AND dv.id=er.dataset_version_id LEFT JOIN datasets d ON d.tenant_id=er.tenant_id AND d.id=dv.dataset_id WHERE er.tenant_id=? AND er.projection_generation=? AND er.projection_deleted=FALSE ORDER BY er.created_at DESC,er.id DESC").bind(actor.tenant_id).bind(view.generation).fetch_all(&state.pool).await?;
    let items = rows
        .into_iter()
        .map(evaluation_json)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok((projection_headers(&view)?, Json(json!(items))))
}

fn evaluation_json(row: sqlx::mysql::MySqlRow) -> ApiResult<Value> {
    Ok(
        json!({"id":row.try_get::<Uuid,_>("id")?,"name":row.try_get::<String,_>("name")?,"workflowVersionId":row.try_get::<Uuid,_>("workflow_version_id")?,"workflowName":row.try_get::<String,_>("workflow_name")?,"datasetVersionId":row.try_get::<Uuid,_>("dataset_version_id")?,"datasetName":row.try_get::<String,_>("dataset_name")?,"evaluationProfileVersionId":row.try_get::<Uuid,_>("evaluation_profile_version_id")?,"visibility":row.try_get::<String,_>("visibility")?,"ownerDepartmentId":row.try_get::<Uuid,_>("owner_department_id")?,"status":row.try_get::<String,_>("status")?,"resultCount":row.try_get::<u64,_>("result_count")?,"parameters":row.try_get::<Value,_>("parameters_json")?,"createdAt":row.try_get::<OffsetDateTime,_>("created_at")?}),
    )
}

async fn get_evaluation_report(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("evaluation:view")?;
    let view = projection(&state.pool).await?;
    Ok((
        projection_headers(&view)?,
        Json(evaluation_report(&state, &actor, id, &view).await?),
    ))
}

async fn evaluation_report(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
    view: &ProjectionView,
) -> ApiResult<Value> {
    let row=sqlx::query("SELECT er.id,er.name,er.workflow_version_id,CONVERT(COALESCE(w.name,BIN_TO_UUID(er.workflow_version_id)) USING utf8mb4) workflow_name,er.dataset_version_id,CONVERT(COALESCE(d.name,BIN_TO_UUID(er.dataset_version_id)) USING utf8mb4) dataset_name,er.evaluation_profile_version_id,er.visibility,er.owner_department_id,er.status,er.completed_cases result_count,er.parameters_json,er.runtime_report_json,er.created_at FROM evaluation_runs er LEFT JOIN workflow_versions wv ON wv.tenant_id=er.tenant_id AND wv.id=er.workflow_version_id LEFT JOIN workflows w ON w.tenant_id=er.tenant_id AND w.id=wv.workflow_id LEFT JOIN dataset_versions dv ON dv.tenant_id=er.tenant_id AND dv.id=er.dataset_version_id LEFT JOIN datasets d ON d.tenant_id=er.tenant_id AND d.id=dv.dataset_id WHERE er.tenant_id=? AND er.id=? AND er.projection_generation=? AND er.projection_deleted=FALSE").bind(actor.tenant_id).bind(id).bind(view.generation).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Evaluation"))?;
    let report: Option<Value> = row.try_get("runtime_report_json")?;
    let run = evaluation_json(row)?;
    let rule_rows=sqlx::query("SELECT rr.id,rr.evaluation_run_case_id,pr.rule_key,pr.name,pr.evaluator_type,rr.status,rr.passed,CAST(rr.score AS DOUBLE) score,rr.detail_json,rr.evaluator_execution_id,rr.duration_ms,rr.cost_micros FROM evaluation_rule_results rr JOIN evaluation_case_projection c ON c.tenant_id=rr.tenant_id AND c.id=rr.evaluation_run_case_id JOIN evaluation_profile_rules pr ON pr.tenant_id=rr.tenant_id AND pr.id=rr.profile_rule_id WHERE c.tenant_id=? AND c.evaluation_run_id=? AND c.projection_generation=? AND rr.projection_generation=? ORDER BY pr.sort_order,rr.id").bind(actor.tenant_id).bind(id).bind(view.generation).bind(view.generation).fetch_all(&state.pool).await?;
    let mut rules = BTreeMap::<Uuid, Vec<Value>>::new();
    for rule in rule_rows {
        rules
            .entry(rule.try_get("evaluation_run_case_id")?)
            .or_default()
            .push(json!({
                "id":rule.try_get::<Uuid,_>("id")?,
                "key":rule.try_get::<String,_>("rule_key")?,
                "name":rule.try_get::<String,_>("name")?,
                "evaluatorType":rule.try_get::<String,_>("evaluator_type")?,
                "status":rule.try_get::<String,_>("status")?,
                "passed":rule.try_get::<Option<bool>,_>("passed")?,
                "score":rule.try_get::<Option<f64>,_>("score")?,
                "detail":rule.try_get::<Value,_>("detail_json")?,
                "evaluatorExecutionId":rule.try_get::<Option<Uuid>,_>("evaluator_execution_id")?,
                "durationMs":rule.try_get::<Option<u64>,_>("duration_ms")?,
                "costMicros":rule.try_get::<u64,_>("cost_micros")?,
            }));
    }
    let case_rows=sqlx::query("SELECT c.id,c.source_case_id,dvc.case_key,c.status,c.target_execution_id,c.actual_output_json,c.duration_ms,c.cost_micros,c.error_code,c.error_message FROM evaluation_case_projection c JOIN evaluation_runs er ON er.tenant_id=c.tenant_id AND er.id=c.evaluation_run_id JOIN dataset_version_cases dvc ON dvc.tenant_id=c.tenant_id AND dvc.dataset_version_id=er.dataset_version_id AND dvc.source_case_id=c.source_case_id WHERE c.tenant_id=? AND c.evaluation_run_id=? AND c.projection_generation=? ORDER BY dvc.sort_order,dvc.source_case_id").bind(actor.tenant_id).bind(id).bind(view.generation).fetch_all(&state.pool).await?;
    let mut results = Vec::with_capacity(case_rows.len());
    for case in case_rows {
        let case_id: Uuid = case.try_get("id")?;
        let rule_results = rules.remove(&case_id).unwrap_or_default();
        let scores = rule_results
            .iter()
            .filter_map(|rule| rule.get("score").and_then(Value::as_f64))
            .collect::<Vec<_>>();
        let score = (!scores.is_empty()).then(|| scores.iter().sum::<f64>() / scores.len() as f64);
        results.push(json!({
            "caseId":case_id,
            "sourceCaseId":case.try_get::<Uuid,_>("source_case_id")?,
            "caseKey":case.try_get::<String,_>("case_key")?,
            "status":case.try_get::<String,_>("status")?,
            "score":score,
            "detail":case.try_get::<Option<Value>,_>("actual_output_json")?,
            "targetExecutionId":case.try_get::<Option<Uuid>,_>("target_execution_id")?,
            "durationMs":case.try_get::<Option<u64>,_>("duration_ms")?,
            "costMicros":case.try_get::<u64,_>("cost_micros")?,
            "errorCode":case.try_get::<Option<String>,_>("error_code")?,
            "errorMessage":case.try_get::<Option<String>,_>("error_message")?,
            "ruleResults":rule_results,
        }));
    }
    let metrics = evaluation_metrics(
        report.as_ref().and_then(|value| value.get("metrics")),
        &results,
    );
    let report_status = run
        .get("status")
        .cloned()
        .unwrap_or_else(|| json!("not_started"));
    Ok(json!({"run":run,"results":results,"metrics":metrics,"reportStatus":report_status}))
}

fn evaluation_metrics(runtime_metrics: Option<&Value>, results: &[Value]) -> Vec<Value> {
    let number = |key: &str| {
        runtime_metrics
            .and_then(|metrics| metrics.get(key))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    let case_scores = results
        .iter()
        .filter_map(|result| result.get("score").and_then(Value::as_f64))
        .collect::<Vec<_>>();
    let passed_cases = results
        .iter()
        .filter(|result| {
            result
                .get("ruleResults")
                .and_then(Value::as_array)
                .is_some_and(|rules| {
                    !rules.is_empty()
                        && rules.iter().all(|rule| {
                            rule.get("status").and_then(Value::as_str) == Some("passed")
                        })
                })
        })
        .count();
    let pass_rate = if results.is_empty() {
        0.0
    } else {
        passed_cases as f64 / results.len() as f64
    };
    let average_score = if case_scores.is_empty() {
        0.0
    } else {
        case_scores.iter().sum::<f64>() / case_scores.len() as f64
    };
    let total_cost = runtime_metrics
        .and_then(|metrics| metrics.get("totalCostMicros"))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            results
                .iter()
                .filter_map(|result| result.get("costMicros").and_then(Value::as_u64))
                .sum()
        });
    [
        ("total_cases", number("totalCases")),
        ("completed_cases", number("completedCases")),
        ("passed_rules", number("passedRules")),
        ("failed_rules", number("failedRules")),
        ("error_rules", number("errorRules")),
        ("pass_rate", pass_rate),
        ("average_score", average_score),
        ("total_cost_micros", total_cost as f64),
    ]
    .into_iter()
    .map(|(key, value)| json!({"key":key,"value":value,"detail":null}))
    .collect()
}

async fn list_notifications(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("notification:view")?;
    let view = projection(&state.pool).await?;
    let rows=sqlx::query("SELECT n.id,n.notification_type,n.title_key,n.body_key,n.arguments_json,n.target_type,n.target_id,n.target_path,n.tone,(r.read_at IS NOT NULL) is_read,n.created_at FROM notifications n JOIN notification_receipts r ON r.tenant_id=n.tenant_id AND r.notification_id=n.id AND r.user_id=? WHERE n.tenant_id=? AND n.projection_generation=? AND n.projection_deleted=FALSE ORDER BY n.created_at DESC,n.id DESC LIMIT 100").bind(actor.user_id).bind(actor.tenant_id).bind(view.generation).fetch_all(&state.pool).await?;
    let items=rows.into_iter().map(|row|->ApiResult<Value>{Ok(json!({"id":row.try_get::<Uuid,_>("id")?,"notificationType":row.try_get::<String,_>("notification_type")?,"titleKey":row.try_get::<String,_>("title_key")?,"bodyKey":row.try_get::<String,_>("body_key")?,"arguments":row.try_get::<Value,_>("arguments_json")?,"targetType":row.try_get::<String,_>("target_type")?,"targetId":row.try_get::<Uuid,_>("target_id")?,"targetPath":row.try_get::<String,_>("target_path")?,"tone":row.try_get::<String,_>("tone")?,"read":row.try_get::<bool,_>("is_read")?,"createdAt":row.try_get::<OffsetDateTime,_>("created_at")?}))}).collect::<ApiResult<Vec<_>>>()?;
    let unread = items
        .iter()
        .filter(|item| item.get("read") == Some(&Value::Bool(false)))
        .count();
    Ok((
        projection_headers(&view)?,
        Json(json!({"items":items,"unreadCount":unread})),
    ))
}

async fn read_notification(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("notification:view")?;
    projection(&state.pool).await?;
    sqlx::query("INSERT INTO notification_receipts(tenant_id,notification_id,user_id,read_at) SELECT tenant_id,id,?,UTC_TIMESTAMP(6) FROM notifications WHERE tenant_id=? AND id=? AND projection_deleted=FALSE ON DUPLICATE KEY UPDATE read_at=COALESCE(read_at,VALUES(read_at))").bind(actor.user_id).bind(actor.tenant_id).bind(id).execute(&state.pool).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn read_all_notifications(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<StatusCode> {
    actor.require("notification:view")?;
    let view = projection(&state.pool).await?;
    sqlx::query("INSERT INTO notification_receipts(tenant_id,notification_id,user_id,read_at) SELECT tenant_id,id,?,UTC_TIMESTAMP(6) FROM notifications WHERE tenant_id=? AND projection_generation=? AND projection_deleted=FALSE ON DUPLICATE KEY UPDATE read_at=COALESCE(read_at,VALUES(read_at))").bind(actor.user_id).bind(actor.tenant_id).bind(view.generation).execute(&state.pool).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_debug_runs(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow_id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("workflow:view")?;
    let view = projection(&state.pool).await?;
    let rows=sqlx::query("SELECT id,work_package_id,draft_revision,status,result_receipt_json,error_code,error_message,expires_at,created_at,completed_at FROM workflow_debug_runs WHERE tenant_id=? AND workflow_id=? AND projection_generation=? AND projection_deleted=FALSE ORDER BY created_at DESC,id DESC LIMIT 100").bind(actor.tenant_id).bind(workflow_id).bind(view.generation).fetch_all(&state.pool).await?;
    let items=rows.into_iter().map(|r|->ApiResult<Value>{Ok(json!({"id":r.try_get::<Uuid,_>("id")?,"workPackageId":r.try_get::<Uuid,_>("work_package_id")?,"draftRevision":r.try_get::<u64,_>("draft_revision")?,"status":r.try_get::<String,_>("status")?,"result":r.try_get::<Option<Value>,_>("result_receipt_json")?,"errorCode":r.try_get::<Option<String>,_>("error_code")?,"errorMessage":r.try_get::<Option<String>,_>("error_message")?,"expiresAt":r.try_get::<OffsetDateTime,_>("expires_at")?,"createdAt":r.try_get::<OffsetDateTime,_>("created_at")?,"completedAt":r.try_get::<Option<OffsetDateTime>,_>("completed_at")?}))}).collect::<ApiResult<Vec<_>>>()?;
    Ok((projection_headers(&view)?, Json(json!({"items":items}))))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetentionRequest {
    dry_run: bool,
    artifact_retention_days: u32,
    trace_retention_days: u32,
    message_retention_days: u32,
    evaluation_retention_days: u32,
}
async fn create_retention_run(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<RetentionRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("runtime:manage")?;
    if [
        input.artifact_retention_days,
        input.trace_retention_days,
        input.message_retention_days,
        input.evaluation_retention_days,
    ]
    .into_iter()
    .any(|days| !(1..=3650).contains(&days))
    {
        return Err(ApiError::bad_request(
            "INVALID_RETENTION_DAYS",
            "Retention days must be between 1 and 3650",
        ));
    }
    let id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    for (data_type, days) in [
        ("artifact", input.artifact_retention_days),
        ("trace", input.trace_retention_days),
        ("application_message", input.message_retention_days),
        ("evaluation_report", input.evaluation_retention_days),
    ] {
        sqlx::query("INSERT INTO retention_policies(tenant_id,data_type,retention_days,enabled,updated_by) VALUES(?,?,?,TRUE,?) ON DUPLICATE KEY UPDATE retention_days=VALUES(retention_days),enabled=TRUE,updated_by=VALUES(updated_by),version=version+1").bind(actor.tenant_id).bind(data_type).bind(days).bind(actor.user_id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO retention_runs(id,tenant_id,dry_run,status,requested_by) VALUES(?,?,?,'queued',?)").bind(id).bind(actor.tenant_id).bind(input.dry_run).bind(actor.user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    tracing::info!(
        retention_run_id = %id,
        tenant_id = %actor.tenant_id,
        dry_run = input.dry_run,
        "Control Retention Run queued"
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(
            json!({"id":id,"dryRun":input.dry_run,"status":"queued","candidateCount":0,"deletedCount":0,"errorMessage":null,"createdAt":OffsetDateTime::now_utc(),"completedAt":null}),
        ),
    ))
}
async fn list_retention_runs(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("runtime:view")?;
    let view = projection(&state.pool).await?;
    let rows=sqlx::query("SELECT id,dry_run,status,candidate_count,deleted_count,error_message,created_at,completed_at FROM retention_runs WHERE tenant_id=? AND projection_generation=? AND projection_deleted=FALSE ORDER BY created_at DESC,id DESC LIMIT 100").bind(actor.tenant_id).bind(view.generation).fetch_all(&state.pool).await?;
    let items=rows.into_iter().map(|r|->ApiResult<Value>{Ok(json!({"id":r.try_get::<Uuid,_>("id")?,"dryRun":r.try_get::<bool,_>("dry_run")?,"status":r.try_get::<String,_>("status")?,"candidateCount":r.try_get::<u64,_>("candidate_count")?,"deletedCount":r.try_get::<u64,_>("deleted_count")?,"errorMessage":r.try_get::<Option<String>,_>("error_message")?,"createdAt":r.try_get::<OffsetDateTime,_>("created_at")?,"completedAt":r.try_get::<Option<OffsetDateTime>,_>("completed_at")?}))}).collect::<ApiResult<Vec<_>>>()?;
    Ok((projection_headers(&view)?, Json(json!(items))))
}
async fn list_retention_items(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("runtime:view")?;
    let view = projection(&state.pool).await?;
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM retention_runs WHERE tenant_id=? AND id=? AND projection_generation=? AND projection_deleted=FALSE)").bind(actor.tenant_id).bind(id).bind(view.generation).fetch_one(&state.pool).await?;
    if !exists {
        return Err(ApiError::not_found("Retention Run"));
    }
    let rows=sqlx::query("SELECT id,data_type,target_id,status,reason,attempt_count,updated_at FROM retention_items WHERE tenant_id=? AND retention_run_id=? AND projection_generation=? AND projection_deleted=FALSE ORDER BY created_at,id LIMIT 10000").bind(actor.tenant_id).bind(id).bind(view.generation).fetch_all(&state.pool).await?;
    let items=rows.into_iter().map(|r|->ApiResult<Value>{Ok(json!({"id":r.try_get::<Uuid,_>("id")?,"dataType":r.try_get::<String,_>("data_type")?,"targetId":r.try_get::<String,_>("target_id")?,"status":r.try_get::<String,_>("status")?,"reason":r.try_get::<Option<String>,_>("reason")?,"attemptCount":r.try_get::<u32,_>("attempt_count")?,"updatedAt":r.try_get::<OffsetDateTime,_>("updated_at")?}))}).collect::<ApiResult<Vec<_>>>()?;
    Ok((projection_headers(&view)?, Json(json!(items))))
}

fn deterministic_id(value: &str) -> Uuid {
    let digest = Sha256::digest(value.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompareEvaluationsQuery {
    run_ids: String,
}

fn compare_run_ids(raw: &str) -> ApiResult<Vec<Uuid>> {
    let runs = raw
        .split(',')
        .map(|value| {
            Uuid::parse_str(value).map_err(|_| {
                ApiError::bad_request(
                    "INVALID_COMPARE_RUN_IDS",
                    "runIds must be comma-separated UUIDs",
                )
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    if !(2..=5).contains(&runs.len()) || runs.iter().collect::<BTreeSet<_>>().len() != runs.len() {
        return Err(ApiError::bad_request(
            "INVALID_COMPARE_RUN_COUNT",
            "Compare requires 2 to 5 distinct evaluation runs",
        ));
    }
    Ok(runs)
}

async fn compare_evaluations(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<CompareEvaluationsQuery>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    actor.require("evaluation:view")?;
    let runs = compare_run_ids(&query.run_ids)?;
    let view = projection(&state.pool).await?;
    let mut reports = Vec::with_capacity(runs.len());
    for id in runs {
        reports.push(evaluation_report(&state, &actor, id, &view).await?);
    }
    Ok((projection_headers(&view)?, Json(compare_reports(&reports))))
}

fn compare_reports(reports: &[Value]) -> Value {
    let mut by_case = BTreeMap::<String, Vec<Option<Value>>>::new();
    let mut by_rule = BTreeMap::<String, Vec<Vec<Value>>>::new();
    let mut summaries = Vec::new();
    for (index, report) in reports.iter().enumerate() {
        let run = &report["run"];
        let metrics = report["metrics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|metric| {
                Some((metric["key"].as_str()?.to_owned(), metric["value"].clone()))
            })
            .collect::<serde_json::Map<_, _>>();
        summaries.push(json!({"runId":run["id"],"name":run["name"],"workflowVersionId":run["workflowVersionId"],"datasetVersionId":run["datasetVersionId"],"profileVersionId":run["evaluationProfileVersionId"],"status":run["status"],"metrics":metrics}));
        for case in report["results"].as_array().into_iter().flatten() {
            let Some(key) = case["caseKey"].as_str() else {
                continue;
            };
            by_case
                .entry(key.to_owned())
                .or_insert_with(|| vec![None; reports.len()])[index] = Some(case.clone());
            for rule in case["ruleResults"].as_array().into_iter().flatten() {
                let Some(key) = rule["key"].as_str() else {
                    continue;
                };
                by_rule
                    .entry(key.to_owned())
                    .or_insert_with(|| vec![Vec::new(); reports.len()])[index]
                    .push(rule.clone());
            }
        }
    }
    let total = by_case.len();
    let aligned = by_case
        .values()
        .filter(|values| values.iter().all(Option::is_some))
        .count();
    let cases = by_case.into_iter().map(|(key, values)| {
        let column = |name: &str| values.iter().map(|case| case.as_ref().map(|case| case[name].clone()).unwrap_or(Value::Null)).collect::<Vec<_>>();
        let costs = column("costMicros");
        let delta = costs.get(1).and_then(Value::as_i64).zip(costs.first().and_then(Value::as_i64)).map(|(candidate, baseline)| candidate - baseline);
        json!({"caseKey":key,"baselineStatus":values.first().and_then(Option::as_ref).map(|case|case["status"].clone()),"candidateStatus":values.get(1).and_then(Option::as_ref).map(|case|case["status"].clone()),"statusByRun":column("status"),"scoreByRun":column("score"),"durationMsByRun":column("durationMs"),"costMicrosByRun":costs,"targetExecutionIdByRun":column("targetExecutionId"),"errorCodeByRun":column("errorCode"),"costDelta":delta})
    }).collect::<Vec<_>>();
    let rules = by_rule.into_iter().map(|(key, values)| {
        let per_run = values.iter().map(|rules| {
            if rules.is_empty() { return Value::Null }
            let passed = rules.iter().filter(|rule| rule["passed"] == true).count();
            let scores = rules.iter().filter_map(|rule|rule["score"].as_f64()).collect::<Vec<_>>();
            json!({"evaluatorType":rules[0]["evaluatorType"],"total":rules.len(),"passed":passed,"passRate":passed as f64 / rules.len() as f64,"averageScore":(!scores.is_empty()).then(||scores.iter().sum::<f64>()/scores.len() as f64),"evaluatorExecutionIds":rules.iter().filter_map(|rule|rule["evaluatorExecutionId"].as_str()).collect::<Vec<_>>()})
        }).collect::<Vec<_>>();
        json!({"ruleKey":key,"passRateByRun":per_run.iter().map(|value|value["passRate"].clone()).collect::<Vec<_>>(),"resultsByRun":per_run})
    }).collect::<Vec<_>>();
    json!({"apiVersion":1,"runs":summaries,"alignedCaseCount":aligned,"totalCaseCount":total,"caseDeltas":cases,"ruleAggregates":rules})
}

#[cfg(test)]
mod compare_tests {
    use super::*;

    #[test]
    fn run_ids_require_distinct_valid_comma_separated_uuids() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        assert_eq!(compare_run_ids(&format!("{a},{b}")).unwrap(), vec![a, b]);
        assert!(compare_run_ids(&format!("{a},{a}")).is_err());
        assert!(compare_run_ids("invalid,invalid").is_err());
        assert!(compare_run_ids(&a.to_string()).is_err());
    }

    #[test]
    fn aligns_cases_and_marks_absent_rules_without_inventing_zeroes() {
        let reports = [
            json!({"run":{"id":"a"},"metrics":[],"results":[{"caseKey":"same","status":"completed","costMicros":5,"targetExecutionId":"trace-a","ruleResults":[{"key":"judge","passed":true,"score":1,"evaluatorType":"llm_judge"}]}]}),
            json!({"run":{"id":"b"},"metrics":[],"results":[{"caseKey":"same","status":"failed","costMicros":8,"ruleResults":[]},{"caseKey":"new","status":"completed","costMicros":10,"ruleResults":[]}]}),
        ];
        let result = compare_reports(&reports);
        assert_eq!(result["alignedCaseCount"], 1);
        assert_eq!(result["totalCaseCount"], 2);
        assert_eq!(result["caseDeltas"][0]["costDelta"], Value::Null);
        assert_eq!(result["caseDeltas"][1]["costDelta"], 3);
        assert_eq!(
            result["ruleAggregates"][0]["passRateByRun"],
            json!([1.0, null])
        );
        assert_eq!(
            result["caseDeltas"][1]["targetExecutionIdByRun"][0],
            "trace-a"
        );
    }
}
