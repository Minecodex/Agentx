//! The same authorized Invocation projection is consumed by employee Web and application MCP.
use std::collections::BTreeSet;

use agentx_runtime_contracts::{
    ArtifactUploadResponseV1, MyTasksPageV1, TaskActionsV1, TaskApprovalV1, TaskStageV1,
    TaskStateV1, TaskViewV1, WorkflowMessageV1,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{MySql, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{Caller, authenticate, authenticate_resource};
use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

// Child executions retain their immutable parent identity. Do not infer them from Trace.
use crate::artifact_access::EXECUTION_TREE as TREE;

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

pub(super) async fn load(
    state: &RuntimeState,
    caller: &Caller,
    id: Uuid,
) -> RuntimeResult<TaskViewV1> {
    // A read transaction provides one consistent MySQL snapshot across state, waits and output.
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT i.*,b.deployment_id FROM application_invocations i JOIN deployment_bundles b ON b.tenant_id=i.tenant_id AND b.id=i.bundle_id WHERE i.tenant_id=? AND i.application_id=? AND i.id=? AND i.caller_type=? AND i.caller_id=?")
        .bind(caller.tenant_id).bind(caller.application_id).bind(id).bind(caller.caller_type).bind(caller.subject_id)
        .fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    let execution: Option<Uuid> = row.try_get("execution_id")?;
    let status: String = row.try_get("status")?;
    let outputs: Option<Value> = row.try_get("result_json")?;
    let pending_inputs = sqlx::query("SELECT * FROM input_waits WHERE tenant_id=? AND application_id=? AND invocation_id=? AND status='waiting' ORDER BY created_at,id")
        .bind(caller.tenant_id).bind(caller.application_id).bind(id).fetch_all(&mut *tx).await?
        .into_iter().map(crate::input_waits::from_row).collect::<RuntimeResult<Vec<_>>>()?;
    let pending_approvals = approvals(&mut tx, caller.tenant_id, execution).await?;
    let stages = stages(&mut tx, caller.tenant_id, execution).await?;
    let unknown: bool = sqlx::query_scalar(&format!("{TREE}SELECT EXISTS(SELECT 1 FROM runtime_calls c JOIN task_executions t ON t.id=c.execution_id WHERE c.tenant_id=? AND c.call_kind IN ('mcp_tool','sandbox') AND c.status='outcome_unknown' AND c.side_effect<>'none') OR EXISTS(SELECT 1 FROM agent_tool_effects f JOIN task_executions t ON t.id=f.execution_id WHERE f.tenant_id=? AND f.outcome_unknown=TRUE)"))
        .bind(caller.tenant_id).bind(execution).bind(caller.tenant_id).bind(caller.tenant_id).bind(caller.tenant_id).fetch_one(&mut *tx).await?;
    let messages = sqlx::query("SELECT * FROM workflow_messages WHERE tenant_id=? AND invocation_id=? ORDER BY created_at,id")
        .bind(caller.tenant_id).bind(id).fetch_all(&mut *tx).await?.into_iter().map(|row| -> RuntimeResult<_> {
            Ok(WorkflowMessageV1 { id:row.try_get("id")?, execution_id:row.try_get("execution_id")?,
                node_execution_id:row.try_get("node_execution_id")?,content:row.try_get("content")?,
                created_at:crate::workflow_interaction::timestamp(row.try_get("created_at")?)? })
        }).collect::<RuntimeResult<Vec<_>>>()?;
    let files = files(&mut tx, caller.tenant_id, execution, outputs.as_ref()).await?;
    let revision: u64 = row.try_get("state_version")?;
    let cursor: u64 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(sequence_number),0) AS UNSIGNED) FROM invocation_events WHERE tenant_id=? AND invocation_id=?")
        .bind(caller.tenant_id).bind(id).fetch_one(&mut *tx).await?;
    let task_state = derive_state(
        &status,
        unknown,
        !pending_inputs.is_empty(),
        !pending_approvals.is_empty(),
    );
    let active = !super::terminal(&status) && !unknown;
    let actions = TaskActionsV1 {
        can_cancel: active,
        input_wait_ids: pending_inputs
            .iter()
            .filter(|wait| {
                active
                    && wait.expires_at.as_ref().is_none_or(|expires| {
                        OffsetDateTime::parse(
                            expires,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .is_ok_and(|expires| expires > OffsetDateTime::now_utc())
                    })
            })
            .map(|wait| wait.wait_id)
            .collect(),
        downloadable_artifact_ids: files.iter().map(|file| file.artifact_id).collect(),
    };
    let task_url = if caller.caller_type == "user" {
        let slug:Option<String>=sqlx::query_scalar("SELECT h.slug FROM application_web_heads h WHERE h.tenant_id=? AND h.application_id=? AND h.enabled=TRUE AND EXISTS(SELECT 1 FROM application_page_publications p WHERE p.tenant_id=h.tenant_id AND p.application_id=h.application_id AND p.bundle_id=? AND p.status IN ('active','superseded'))").bind(caller.tenant_id).bind(caller.application_id).bind(row.try_get::<Uuid,_>("bundle_id")?).fetch_optional(&mut *tx).await?;
        slug.and_then(|slug| {
            std::env::var("AGENTX_RUNTIME_CORS_ALLOWED_ORIGIN")
                .ok()
                .and_then(|origin| url::Url::parse(&origin).ok())
                .and_then(|origin| origin.join(&format!("/a/{slug}?taskId={id}")).ok())
                .map(|url| url.to_string())
        })
    } else {
        None
    };
    let view = TaskViewV1 {
        task_url,
        task_id: id,
        application_id: caller.application_id,
        deployment_id: row.try_get("deployment_id")?,
        workflow_version_id: row.try_get("workflow_version_id")?,
        bundle_id: row.try_get("bundle_id")?,
        session_id: row.try_get("session_id")?,
        caller_type: caller.caller_type.into(),
        access_binding: row
            .try_get::<Option<Value>, _>("access_binding_json")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| RuntimeError::Internal(e.into()))?,
        state: task_state,
        invocation_status: status,
        stages,
        pending_inputs,
        pending_approvals,
        messages,
        outputs,
        files,
        error: row.try_get("error_json")?,
        allowed_actions: actions,
        revision,
        event_cursor: cursor,
        created_at: row.try_get("created_at")?,
    };
    tx.commit().await?;
    Ok(view)
}

async fn stages(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Option<Uuid>,
) -> RuntimeResult<Vec<TaskStageV1>> {
    let rows=sqlx::query(&format!("{TREE}SELECT n.id,n.node_name,n.status FROM node_executions n JOIN task_executions t ON t.id=n.execution_id WHERE n.tenant_id=? AND n.status IN ('ready','queued','running','waiting') ORDER BY n.created_at,n.id"))
        .bind(tenant).bind(execution).bind(tenant).bind(tenant).fetch_all(&mut **tx).await?;
    rows.into_iter()
        .map(|row| {
            Ok(TaskStageV1 {
                node_execution_id: row.try_get("id")?,
                title: row.try_get("node_name")?,
                status: row.try_get("status")?,
            })
        })
        .collect()
}

async fn approvals(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Option<Uuid>,
) -> RuntimeResult<Vec<TaskApprovalV1>> {
    let rows=sqlx::query(&format!("{TREE}SELECT a.id,a.title,a.status,a.version,a.deadline_at FROM approval_tasks a JOIN task_executions t ON t.id=a.execution_id WHERE a.tenant_id=? AND a.status IN ('pending','claimed') ORDER BY a.created_at,a.id"))
        .bind(tenant).bind(execution).bind(tenant).bind(tenant).fetch_all(&mut **tx).await?;
    rows.into_iter()
        .map(|row| {
            Ok(TaskApprovalV1 {
                approval_id: row.try_get("id")?,
                title: row.try_get("title")?,
                status: row.try_get("status")?,
                version: row.try_get("version")?,
                expires_at: row.try_get("deadline_at")?,
            })
        })
        .collect()
}

async fn files(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    execution: Option<Uuid>,
    output: Option<&Value>,
) -> RuntimeResult<Vec<ArtifactUploadResponseV1>> {
    let mut ids = BTreeSet::new();
    if let Some(output) = output {
        crate::artifact_access::collect(output, &mut ids);
    }
    let mut files = Vec::new();
    for id in ids {
        let row=sqlx::query(&format!("{TREE}SELECT a.id,a.content_type,a.size_bytes,a.sha256 FROM artifacts a WHERE a.tenant_id=? AND a.id=? AND a.deleted_at IS NULL AND EXISTS(SELECT 1 FROM artifact_references r WHERE r.tenant_id=a.tenant_id AND r.artifact_id=a.id AND ((r.owner_type='execution' AND r.owner_id IN (SELECT BIN_TO_UUID(id) FROM task_executions)) OR (r.owner_type='node_execution' AND r.owner_id IN (SELECT BIN_TO_UUID(n.id) FROM node_executions n JOIN task_executions t ON t.id=n.execution_id WHERE n.tenant_id=?))))"))
            .bind(tenant).bind(execution).bind(tenant).bind(tenant).bind(id).bind(tenant).fetch_optional(&mut **tx).await?;
        if let Some(row) = row {
            files.push(ArtifactUploadResponseV1 {
                artifact_id: row.try_get("id")?,
                content_type: row.try_get("content_type")?,
                size_bytes: row.try_get("size_bytes")?,
                sha256: row.try_get("sha256")?,
            });
        }
    }
    Ok(files)
}

fn derive_state(status: &str, unknown: bool, input: bool, approval: bool) -> TaskStateV1 {
    if unknown {
        return TaskStateV1::OutcomeUnknown;
    }
    match status {
        "completed" => TaskStateV1::Completed,
        "failed" => TaskStateV1::Failed,
        "cancelled" => TaskStateV1::Cancelled,
        _ if input => TaskStateV1::WaitingInput,
        _ if approval => TaskStateV1::WaitingApproval,
        "queued" => TaskStateV1::Accepted,
        _ => TaskStateV1::Running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_waits_and_terminal_authority_have_a_fixed_priority() {
        assert_eq!(
            derive_state("running", false, true, true),
            TaskStateV1::WaitingInput
        );
        assert_eq!(
            derive_state("running", false, false, true),
            TaskStateV1::WaitingApproval
        );
        assert_eq!(
            derive_state("completed", false, true, true),
            TaskStateV1::Completed
        );
        assert_eq!(
            derive_state("failed", true, false, false),
            TaskStateV1::OutcomeUnknown
        );
        assert_eq!(
            derive_state("queued", false, false, false),
            TaskStateV1::Accepted
        );
    }
}
