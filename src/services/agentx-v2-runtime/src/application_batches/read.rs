use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

pub async fn get(state: &RuntimeState, scope: &BatchScope, id: Uuid) -> RuntimeResult<BatchViewV1> {
    let mut tx = crate::task_view::snapshot(&state.pool).await?;
    let batch = row(&mut tx, scope, id, false).await?;
    let items = views_tx(&mut tx, &batch).await?;
    let view = view(&batch, &items)?;
    tx.commit().await?;
    Ok(view)
}

pub async fn items(
    state: &RuntimeState,
    scope: &BatchScope,
    id: Uuid,
    query: &BatchQueryV1,
) -> RuntimeResult<BatchItemsPageV1> {
    let limit = limit(query)?;
    let cursor = decode_cursor(scope, Some(id), query)?;
    let mut tx = crate::task_view::snapshot(&state.pool).await?;
    let batch = row(&mut tx, scope, id, false).await?;
    let mut items = views_tx(&mut tx, &batch)
        .await?
        .into_iter()
        .filter(|i| {
            query.state.as_ref().is_none_or(|s| s == &i.state)
                && cursor.as_ref().is_none_or(|c| i.ordinal > c.position)
        })
        .collect::<Vec<_>>();
    let next = if items.len() > limit {
        items.truncate(limit);
        items
            .last()
            .map(|item| encode_cursor(scope, Some(id), query, None, None, item.ordinal))
            .transpose()?
    } else {
        None
    };
    if scope.management {
        for item in &mut items {
            if let Some(task) = &item.task {
                item.execution_id = sqlx::query_scalar(
                    "SELECT execution_id FROM application_invocations WHERE tenant_id=? AND id=?",
                )
                .bind(scope.caller.tenant_id)
                .bind(task.task_id)
                .fetch_one(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(BatchItemsPageV1 { items, next })
}

pub async fn list(
    state: &RuntimeState,
    scope: &BatchScope,
    query: &BatchQueryV1,
) -> RuntimeResult<BatchesPageV1> {
    let limit = limit(query)?;
    let cursor = decode_cursor(scope, None, query)?;
    let mut tx = crate::task_view::snapshot(&state.pool).await?;
    // Walk bounded windows until a complete filtered page is found, without filtering after pagination.
    let mut before_id = cursor.as_ref().and_then(|c| c.id);
    let mut before_time = cursor.as_ref().and_then(|c| c.created_at.clone());
    let mut items = Vec::new();
    let mut exhausted = false;
    let mut scanned = 0;
    while items.len() <= limit && !exhausted && scanned < 1000 {
        let rows=sqlx::query("SELECT * FROM application_batches WHERE tenant_id=? AND application_id=? AND retention_deleted_at IS NULL AND (?=TRUE OR (caller_type=? AND caller_id=?)) AND (? IS NULL OR source=?) AND (? IS NULL OR idempotency_key=?) AND (? IS NULL OR created_at<? OR (created_at=? AND id<?)) ORDER BY created_at DESC,id DESC LIMIT 50")
            .bind(scope.caller.tenant_id).bind(scope.caller.application_id).bind(scope.management).bind(&scope.caller.caller_type).bind(scope.caller.caller_id)
            .bind(&query.source).bind(&query.source).bind(&query.request_key).bind(&query.request_key).bind(cursor_time(before_time.as_deref())?).bind(cursor_time(before_time.as_deref())?).bind(cursor_time(before_time.as_deref())?).bind(before_id).fetch_all(&mut *tx).await?;
        exhausted = rows.len() < 50;
        for batch in rows {
            scanned += 1;
            before_id = Some(batch.try_get("id")?);
            before_time = Some(
                batch
                    .try_get::<time::OffsetDateTime, _>("created_at")?
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|e| RuntimeError::Internal(e.into()))?,
            );
            let projects = views_tx(&mut tx, &batch).await?;
            let v = view(&batch, &projects)?;
            if query.state.as_ref().is_none_or(|s| s == &v.state) {
                items.push(v);
            }
            if items.len() > limit {
                break;
            }
        }
    }
    let next = if items.len() > limit {
        items.truncate(limit);
        items
            .last()
            .map(|b| {
                encode_cursor(
                    scope,
                    None,
                    query,
                    Some(b.batch_id),
                    Some(b.created_at.clone()),
                    0,
                )
            })
            .transpose()?
    } else if !exhausted {
        Some(encode_cursor(
            scope,
            None,
            query,
            before_id,
            before_time,
            0,
        )?)
    } else {
        None
    };
    tx.commit().await?;
    Ok(BatchesPageV1 { items, next })
}

pub(super) async fn views_tx(
    tx: &mut Transaction<'_, MySql>,
    batch: &sqlx::mysql::MySqlRow,
) -> RuntimeResult<Vec<BatchItemViewV1>> {
    let caller = owner(batch)?.task_caller()?;
    let batch_id: Uuid = batch.try_get("id")?;
    let rows = sqlx::query(
        "SELECT * FROM application_batch_items WHERE tenant_id=? AND batch_id=? ORDER BY ordinal",
    )
    .bind(caller.tenant_id)
    .bind(batch_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for item in rows {
        let invocation: Option<Uuid> = item.try_get("invocation_id")?;
        let task = if let Some(id) = invocation {
            Some(crate::task_view::load_tx(tx, &caller, id).await?)
        } else {
            None
        };
        let dispatch: String = item.try_get("dispatch_state")?;
        let mut state = if let Some(task) = &task {
            serde_json::to_value(task.state)
                .map_err(|e| RuntimeError::Internal(e.into()))?
                .as_str()
                .unwrap_or("outcome_unknown")
                .to_owned()
        } else {
            match dispatch.as_str() {
                "pending" => "queued",
                "cancelled" => "cancelled",
                "rejected" => "failed",
                _ => return Err(RuntimeError::Unavailable),
            }
            .into()
        };
        if matches!(state.as_str(), "accepted" | "running")
            && !item.try_get::<bool, _>("active_slot")?
        {
            state = "queued".into();
        }
        let can_rerun = if state == "failed" {
            if let Some(task) = &task {
                safe_to_rerun(tx, caller.tenant_id, task).await?
            } else {
                dispatch == "rejected"
            }
        } else {
            false
        };
        items.push(BatchItemViewV1 {
            item_id: item.try_get("id")?,
            client_item_id: String::from_utf8(item.try_get("client_item_id")?)
                .map_err(|error| RuntimeError::Internal(error.into()))?,
            ordinal: item.try_get("ordinal")?,
            input: item.try_get("input_json")?,
            dispatch_state: dispatch,
            state,
            task,
            error: item.try_get("error_json")?,
            can_rerun,
            source_item_id: item.try_get("source_item_id")?,
            source_invocation_id: item.try_get("source_invocation_id")?,
            execution_id: None,
        });
    }
    Ok(items)
}

async fn safe_to_rerun(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    task: &TaskViewV1,
) -> RuntimeResult<bool> {
    let root: Option<Uuid> = sqlx::query_scalar(
        "SELECT execution_id FROM application_invocations WHERE tenant_id=? AND id=?",
    )
    .bind(tenant)
    .bind(task.task_id)
    .fetch_one(&mut **tx)
    .await?;
    let tree = crate::artifact_access::EXECUTION_TREE;
    // A new Invocation has a new external intent. Old provider idempotency alone never authorizes replay.
    let dangerous:bool=sqlx::query_scalar(&format!("{tree} SELECT EXISTS(SELECT 1 FROM runtime_calls c JOIN task_executions t ON t.id=c.execution_id WHERE c.tenant_id=? AND c.call_kind IN ('mcp_tool','sandbox','http') AND c.side_effect NOT IN ('none','read_only') AND c.status NOT IN ('rejected','not_started')) OR EXISTS(SELECT 1 FROM agent_tool_effects f JOIN task_executions t ON t.id=f.execution_id WHERE f.tenant_id=?)"))
        .bind(tenant).bind(root).bind(tenant).bind(tenant).bind(tenant).fetch_one(&mut **tx).await?;
    Ok(!dangerous)
}

pub(super) fn view(
    batch: &sqlx::mysql::MySqlRow,
    items: &[BatchItemViewV1],
) -> RuntimeResult<BatchViewV1> {
    let mut counts = BatchCountsV1 {
        total: items.len() as u32,
        ..Default::default()
    };
    for item in items {
        match item.state.as_str() {
            "queued" | "accepted" => counts.queued += 1,
            "running" => counts.running += 1,
            "waiting_input" => counts.waiting_input += 1,
            "waiting_approval" => counts.waiting_approval += 1,
            "completed" => counts.completed += 1,
            "failed" => counts.failed += 1,
            "cancelled" => counts.cancelled += 1,
            _ => counts.outcome_unknown += 1,
        }
    }
    counts.ended = counts.completed + counts.failed + counts.cancelled;
    let dispatch_state: String = batch.try_get("dispatch_state")?;
    let state = if counts.outcome_unknown > 0 {
        "outcome_unknown"
    } else if counts.running + counts.queued > 0 {
        "running"
    } else if counts.waiting_input + counts.waiting_approval > 0 {
        "waiting"
    } else if counts.failed > 0 {
        "partial_failure"
    } else if counts.cancelled > 0 {
        "cancelled"
    } else {
        "completed"
    };
    let created: time::OffsetDateTime = batch.try_get("created_at")?;
    Ok(BatchViewV1 {
        batch_id: batch.try_get("id")?,
        application_id: batch.try_get("application_id")?,
        name: batch.try_get("name")?,
        source: batch.try_get("source")?,
        caller_type: batch.try_get("caller_type")?,
        caller_id: batch.try_get("caller_id")?,
        caller_name: owner(batch)?
            .origin
            .initiator_user_name
            .or(owner(batch)?.origin.trigger_name),
        bundle_id: batch.try_get("bundle_id")?,
        workflow_version_id: batch.try_get("workflow_version_id")?,
        workflow_version_number: batch.try_get("workflow_version_number")?,
        access_binding: binding(batch)?,
        page_publication_id: batch.try_get("history_page_id")?,
        source_batch_id: batch.try_get("source_batch_id")?,
        dispatch_state: dispatch_state.clone(),
        state: state.into(),
        revision: batch.try_get("revision")?,
        created_at: created
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| RuntimeError::Internal(e.into()))?,
        can_stop: dispatch_state == "active" && items.iter().any(|i| i.dispatch_state == "pending"),
        can_cancel: items.iter().any(|i| {
            i.dispatch_state == "pending"
                || i.task
                    .as_ref()
                    .is_some_and(|t| t.allowed_actions.can_cancel)
        }),
        can_rerun: items.iter().any(|i| i.can_rerun),
        counts,
    })
}

#[derive(Deserialize, Serialize)]
struct Cursor {
    scope: String,
    id: Option<Uuid>,
    created_at: Option<String>,
    position: u32,
}
fn limit(q: &BatchQueryV1) -> RuntimeResult<usize> {
    let n = q.limit.unwrap_or(20);
    if !(1..=50).contains(&n) {
        return Err(invalid("Batch page limit must be 1–50"));
    }
    Ok(n as usize)
}
fn cursor_scope(s: &BatchScope, id: Option<Uuid>, q: &BatchQueryV1) -> RuntimeResult<String> {
    hash(&json!([
        s.caller.tenant_id,
        s.caller.application_id,
        s.caller.caller_id,
        s.caller.caller_type,
        s.management,
        id,
        q.state,
        q.source,
        q.request_key
    ]))
}
fn decode_cursor(
    s: &BatchScope,
    id: Option<Uuid>,
    q: &BatchQueryV1,
) -> RuntimeResult<Option<Cursor>> {
    q.cursor
        .as_ref()
        .map(|value| {
            if value.len() > 1024 {
                return Err(invalid("Invalid batch cursor"));
            }
            let c: Cursor = serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(value)
                    .map_err(|_| invalid("Invalid batch cursor"))?,
            )
            .map_err(|_| invalid("Invalid batch cursor"))?;
            if c.scope != cursor_scope(s, id, q)? {
                return Err(invalid("The cursor belongs to another query or caller"));
            }
            Ok(c)
        })
        .transpose()
}
fn encode_cursor(
    s: &BatchScope,
    id: Option<Uuid>,
    q: &BatchQueryV1,
    last: Option<Uuid>,
    created_at: Option<String>,
    position: u32,
) -> RuntimeResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&Cursor {
            scope: cursor_scope(s, id, q)?,
            id: last,
            created_at,
            position,
        })
        .map_err(|e| RuntimeError::Internal(e.into()))?,
    ))
}

fn cursor_time(value: Option<&str>) -> RuntimeResult<Option<time::OffsetDateTime>> {
    value
        .map(|v| {
            time::OffsetDateTime::parse(v, &time::format_description::well_known::Rfc3339)
                .map_err(|_| invalid("Invalid batch cursor timestamp"))
        })
        .transpose()
}
