//! The same authorized Invocation projection is consumed by employee Web and application MCP.

use agentx_runtime_contracts::{MyTasksPageV1, TaskViewV1};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{authenticate, authenticate_resource};
use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

pub(super) use crate::task_view::load;

pub(super) async fn get(
    State(state): State<RuntimeState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> RuntimeResult<Json<TaskViewV1>> {
    let caller = authenticate_resource(&state, &headers, "invocation", id).await?;
    Ok(Json(load(&state, &caller, id).await?))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    active_only: Option<bool>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    application_id: Uuid,
    caller_type: String,
    subject_id: Uuid,
    active_only: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    id: Uuid,
}

pub(super) async fn list(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> RuntimeResult<Json<MyTasksPageV1>> {
    let caller = authenticate(&state, &headers, Some(&slug)).await?;
    let limit = query.limit.unwrap_or(20);
    if !(1..=50).contains(&limit) {
        return Err(invalid_cursor());
    }
    let active = query.active_only.unwrap_or(false);
    let cursor = query
        .cursor
        .map(|cursor| -> RuntimeResult<Cursor> {
            if cursor.len() > 1024 {
                return Err(invalid_cursor());
            }
            let cursor: Cursor = serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(cursor)
                    .map_err(|_| invalid_cursor())?,
            )
            .map_err(|_| invalid_cursor())?;
            if cursor.application_id != caller.application_id
                || cursor.caller_type != caller.caller_type
                || cursor.subject_id != caller.subject_id
                || cursor.active_only != active
            {
                return Err(invalid_cursor());
            }
            Ok(cursor)
        })
        .transpose()?;
    let rows = sqlx::query("SELECT id,created_at FROM application_invocations WHERE tenant_id=? AND application_id=? AND caller_type=? AND caller_id=? AND (?=FALSE OR status IN ('queued','running')) AND (? IS NULL OR created_at<? OR (created_at=? AND id<?)) ORDER BY created_at DESC,id DESC LIMIT ?")
        .bind(caller.tenant_id).bind(caller.application_id).bind(caller.caller_type).bind(caller.subject_id).bind(active)
        .bind(cursor.as_ref().map(|c|c.created_at)).bind(cursor.as_ref().map(|c|c.created_at))
        .bind(cursor.as_ref().map(|c|c.created_at)).bind(cursor.as_ref().map(|c|c.id)).bind(limit+1)
        .fetch_all(&state.pool).await?;
    let has_more = rows.len() > limit as usize;
    let mut items = Vec::new();
    for row in rows.into_iter().take(limit as usize) {
        items.push(load(&state, &caller, row.try_get("id")?).await?);
    }
    let next = if has_more {
        let last = items.last().expect("a nonzero page limit");
        Some(
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&Cursor {
                    application_id: caller.application_id,
                    caller_type: caller.caller_type.into(),
                    subject_id: caller.subject_id,
                    active_only: active,
                    created_at: last.created_at,
                    id: last.task_id,
                })
                .map_err(|error| RuntimeError::Internal(error.into()))?,
            ),
        )
    } else {
        None
    };
    Ok(Json(MyTasksPageV1 { items, next }))
}

fn invalid_cursor() -> RuntimeError {
    RuntimeError::InvalidRequest(
        "TASK_PAGE_INVALID",
        "Use a limit of 1–50 and a cursor from the same application, caller and filter".into(),
    )
}
