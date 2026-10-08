use agentx_runtime_contracts::{
    ContentHash, EventExportPageV1, EventExportRequestV1, GovernanceSnapshotPageV1,
    GovernanceSnapshotRequestV1, RuntimeApprovalButtonV1, RuntimeApprovalCandidateKindV1,
    RuntimeApprovalCandidateV1, RuntimeEvaluationCaseResultV1, RuntimeEvaluationMetricsV1,
    RuntimeEvaluationReportV1, RuntimeEvaluationRuleResultV1, RuntimeEventPayloadV1,
    RuntimeGovernanceObjectKindV1, RuntimeGovernanceSnapshotItemV1,
    RuntimeGovernanceSnapshotPayloadV1, RuntimeIntegrationEventEnvelopeV1, RuntimeRetentionItemV1,
    content_hash,
};
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{MySql, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

#[derive(Clone, Debug)]
pub struct SequencedEvent {
    pub invocation_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotCursor {
    tenant_id: Uuid,
    object_types: Vec<RuntimeGovernanceObjectKindV1>,
    type_index: usize,
    last_object_id: Option<Uuid>,
    upper_cursor: u64,
}

pub async fn export_events(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Query(request): Query<EventExportRequestV1>,
) -> RuntimeResult<Json<EventExportPageV1>> {
    state.trust.projector(&headers, "runtime.events.read")?;
    if request.api_version != 1
        || request.limit == 0
        || request.limit > 1000
        || request.wait_seconds > 30
    {
        return Err(RuntimeError::QueryBudgetExceeded);
    }
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(u64::from(request.wait_seconds));
    loop {
        let page = load_event_page(&state, &request).await?;
        if !page.events.is_empty()
            || request.wait_seconds == 0
            || tokio::time::Instant::now() >= deadline
        {
            return Ok(Json(page));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

pub async fn export_governance_snapshot(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<GovernanceSnapshotRequestV1>,
) -> RuntimeResult<Json<GovernanceSnapshotPageV1>> {
    state.trust.projector(&headers, "runtime.snapshots.read")?;
    if request.object_types.is_empty() || request.limit == 0 || request.limit > 1000 {
        return Err(RuntimeError::QueryBudgetExceeded);
    }
    let (upper_cursor, retention_floor) = event_bounds(&state).await?;
    let mut cursor = if let Some(value) = request.page_cursor.as_deref() {
        decode_snapshot_cursor(value)?
    } else {
        SnapshotCursor {
            tenant_id: request.tenant_id,
            object_types: request.object_types.clone(),
            type_index: 0,
            last_object_id: None,
            upper_cursor: request.snapshot_upper_cursor.unwrap_or(upper_cursor),
        }
    };
    if cursor.tenant_id != request.tenant_id
        || cursor.object_types != request.object_types
        || request
            .snapshot_upper_cursor
            .is_some_and(|value| value != cursor.upper_cursor)
    {
        return Err(RuntimeError::QueryCursorExpired);
    }
    let mut objects = Vec::new();
    while cursor.type_index < cursor.object_types.len() && objects.len() < request.limit as usize {
        let remaining = request.limit as usize - objects.len();
        let kind = cursor.object_types[cursor.type_index];
        let mut page = load_snapshot_kind(
            &state,
            request.tenant_id,
            kind,
            cursor.upper_cursor,
            cursor.last_object_id,
            remaining + 1,
        )
        .await?;
        if page.len() > remaining {
            page.truncate(remaining);
            cursor.last_object_id = page.last().map(|item| item.object_id);
            objects.extend(page);
            break;
        }
        objects.extend(page);
        cursor.type_index += 1;
        cursor.last_object_id = None;
    }
    let next_page_cursor = (cursor.type_index < cursor.object_types.len())
        .then(|| encode_snapshot_cursor(&cursor))
        .transpose()?;
    Ok(Json(GovernanceSnapshotPageV1 {
        api_version: 1,
        snapshot_upper_cursor: cursor.upper_cursor,
        retention_floor_cursor: retention_floor,
        objects,
        next_page_cursor,
        generated_at: OffsetDateTime::now_utc(),
    }))
}

pub async fn sequence_one(
    pool: &sqlx::MySqlPool,
    _owner: Uuid,
) -> RuntimeResult<Option<SequencedEvent>> {
    let mut tx = pool.begin().await?;
    let outbox_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM execution_outbox WHERE status='pending' AND message_type='runtime_event' AND available_at<=UTC_TIMESTAMP(6) ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED")
        .fetch_optional(&mut *tx).await?;
    let Some(outbox_id) = outbox_id else {
        tx.commit().await?;
        return Ok(None);
    };
    // Lock/order only the small identity row; a WorkPackage event can carry
    // an output larger than MySQL's sort buffer. Read it by the claimed key.
    let row = sqlx::query("SELECT id,tenant_id,execution_id,payload_json,event_type,aggregate_type,aggregate_id,aggregate_version,correlation_id,causation_id,created_at FROM execution_outbox WHERE id=?")
        .bind(outbox_id).fetch_one(&mut *tx).await?;
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    let execution_id: Option<Uuid> = row.try_get("execution_id")?;
    // SKIP LOCKED must apply only to the Outbox authority row. A LEFT JOIN in
    // the locking query can turn a concurrently locked Execution into NULL
    // columns and falsely quarantine a valid event. This consistent read does
    // not participate in claiming the Execution row.
    let execution = match execution_id {
        Some(execution_id) => sqlx::query("SELECT invocation_id,application_id,workflow_id,workflow_version_id,bundle_id,trace_id,status,state_version,admission_epoch,CAST(COALESCE((SELECT MAX(t.sequence_number) FROM execution_events t WHERE t.tenant_id=workflow_executions.tenant_id AND t.execution_id=workflow_executions.id),0) AS UNSIGNED) trace_watermark,terminal_result_hash,terminal_result_object_id,error_json FROM workflow_executions WHERE tenant_id=? AND id=?")
            .bind(tenant_id)
            .bind(execution_id)
            .fetch_optional(&mut *tx)
            .await?,
        None => None,
    };
    let payload = match typed_event_payload(
        &row,
        execution
            .as_ref()
            .map(|execution| execution.try_get::<u64, _>("trace_watermark"))
            .transpose()?
            .unwrap_or_default(),
        execution.as_ref(),
    ) {
        Ok(payload) => payload,
        Err(error) => {
            let diagnostic = format!("EVENT_PAYLOAD_INVALID: {error}");
            sqlx::query("UPDATE execution_outbox SET status='failed',last_error=?,locked_by=NULL,locked_until=NULL WHERE id=? AND status='pending'")
                .bind(diagnostic.chars().take(1000).collect::<String>())
                .bind(outbox_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Err(RuntimeError::Internal(anyhow::anyhow!(
                "Integration Event Outbox {outbox_id} has an invalid payload: {error}"
            )));
        }
    };
    let cursor: u64 = sqlx::query_scalar("SELECT next_cursor FROM integration_event_sequence WHERE sequence_key='runtime' FOR UPDATE")
        .fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE integration_event_sequence SET next_cursor=next_cursor+1 WHERE sequence_key='runtime'")
        .execute(&mut *tx).await?;
    let event_type = row
        .try_get::<Option<String>, _>("event_type")?
        .unwrap_or_else(|| event_type(&payload));
    let aggregate_type = row
        .try_get::<Option<String>, _>("aggregate_type")?
        .unwrap_or_else(|| aggregate_type(&payload).into());
    let aggregate_id = row
        .try_get::<Option<String>, _>("aggregate_id")?
        .map(Ok)
        .unwrap_or_else(|| aggregate_id(&payload, execution_id))?;
    let aggregate_version = row
        .try_get::<Option<u64>, _>("aggregate_version")?
        .unwrap_or_else(|| {
            let state_version = execution
                .as_ref()
                .and_then(|execution| execution.try_get("state_version").ok())
                .unwrap_or(1);
            aggregate_version(&payload, state_version)
        });
    let event_hash =
        content_hash(&payload).map_err(|error| RuntimeError::Internal(error.into()))?;
    let correlation_id = row
        .try_get::<Option<Uuid>, _>("correlation_id")?
        .unwrap_or(outbox_id);
    let causation_id: Option<Uuid> = row.try_get("causation_id")?;
    sqlx::query("INSERT INTO integration_event_log(event_cursor,event_id,source_outbox_id,tenant_id,event_type,aggregate_type,aggregate_id,aggregate_version,schema_version,payload_json,content_hash,correlation_id,causation_id,occurred_at) VALUES(?,?,?,?,?,?,?,?,1,?,?,?,?,?)")
        .bind(cursor).bind(outbox_id).bind(outbox_id).bind(tenant_id).bind(&event_type).bind(&aggregate_type).bind(&aggregate_id).bind(aggregate_version)
        .bind(serde_json::to_value(&payload)?).bind(event_hash.as_str()).bind(correlation_id).bind(causation_id).bind(row.try_get::<OffsetDateTime,_>("created_at")?).execute(&mut *tx).await?;
    update_governance_cursor(&mut tx, tenant_id, cursor, &payload).await?;
    sqlx::query("UPDATE execution_outbox SET status='published',published_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL WHERE id=? AND status='pending'")
        .bind(outbox_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(SequencedEvent {
        invocation_id: execution
            .as_ref()
            .map(|execution| execution.try_get("invocation_id"))
            .transpose()?
            .flatten(),
    }))
}

async fn load_event_page(
    state: &RuntimeState,
    request: &EventExportRequestV1,
) -> RuntimeResult<EventExportPageV1> {
    let (upper_cursor, retention_floor) = event_bounds(state).await?;
    if request.after_cursor.saturating_add(1) < retention_floor {
        return Err(RuntimeError::EventCursorExpired);
    }
    let rows = sqlx::query("SELECT event_cursor,event_id,source_outbox_id,tenant_id,event_type,aggregate_type,aggregate_id,aggregate_version,schema_version,payload_json,content_hash,correlation_id,causation_id,occurred_at FROM integration_event_log WHERE event_cursor>? ORDER BY event_cursor LIMIT ?")
        .bind(request.after_cursor).bind(request.limit + 1).fetch_all(&state.pool).await?;
    let has_more = rows.len() > request.limit as usize;
    let events = rows
        .into_iter()
        .take(request.limit as usize)
        .map(event_from_row)
        .collect::<RuntimeResult<Vec<_>>>()?;
    let next_cursor = events
        .last()
        .map_or(request.after_cursor, |event| event.cursor);
    Ok(EventExportPageV1 {
        api_version: 1,
        from_cursor: request.after_cursor,
        next_cursor,
        upper_cursor,
        retention_floor_cursor: retention_floor,
        has_more,
        events,
    })
}

fn event_from_row(row: sqlx::mysql::MySqlRow) -> RuntimeResult<RuntimeIntegrationEventEnvelopeV1> {
    Ok(RuntimeIntegrationEventEnvelopeV1 {
        schema_version: row.try_get::<u16, _>("schema_version")?.into(),
        cursor: row.try_get("event_cursor")?,
        event_id: row.try_get("event_id")?,
        source_outbox_id: row.try_get("source_outbox_id")?,
        tenant_id: row.try_get("tenant_id")?,
        aggregate_type: row.try_get("aggregate_type")?,
        aggregate_id: row.try_get("aggregate_id")?,
        aggregate_version: row.try_get("aggregate_version")?,
        event_type: row.try_get("event_type")?,
        occurred_at: row.try_get("occurred_at")?,
        payload: serde_json::from_value(row.try_get("payload_json")?)
            .map_err(|error| RuntimeError::Internal(error.into()))?,
        content_hash: parse_hash(row.try_get("content_hash")?)?,
        correlation_id: row.try_get("correlation_id")?,
        causation_id: row.try_get("causation_id")?,
    })
}

async fn event_bounds(state: &RuntimeState) -> RuntimeResult<(u64, u64)> {
    let row = sqlx::query("SELECT next_cursor-1 upper_cursor,retention_floor_cursor FROM integration_event_sequence WHERE sequence_key='runtime'")
        .fetch_one(&state.pool).await?;
    Ok((
        row.try_get("upper_cursor")?,
        row.try_get("retention_floor_cursor")?,
    ))
}

fn typed_event_payload(
    row: &sqlx::mysql::MySqlRow,
    trace_watermark: u64,
    execution: Option<&sqlx::mysql::MySqlRow>,
) -> RuntimeResult<RuntimeEventPayloadV1> {
    let raw: Value = row.try_get("payload_json")?;
    let contract_error = match serde_json::from_value(raw) {
        Ok(payload) => return Ok(payload),
        Err(error) => error,
    };
    let execution = execution.ok_or_else(|| {
        RuntimeError::Internal(anyhow::anyhow!(
            "payload does not match RuntimeEventPayloadV1 and has no Execution context: {contract_error}"
        ))
    })?;
    Ok(RuntimeEventPayloadV1::ExecutionChanged {
        execution_id: row.try_get("execution_id")?,
        invocation_id: execution.try_get("invocation_id")?,
        application_id: execution.try_get("application_id")?,
        workflow_id: execution.try_get("workflow_id")?,
        workflow_version_id: execution.try_get("workflow_version_id")?,
        bundle_id: execution
            .try_get::<Option<Uuid>, _>("bundle_id")?
            .ok_or_else(|| {
                RuntimeError::Internal(anyhow::anyhow!("Execution Event has no Bundle"))
            })?,
        status: execution.try_get("status")?,
        state_version: execution.try_get("state_version")?,
        admission_epoch: execution.try_get("admission_epoch")?,
        trace_watermark,
        result_hash: execution
            .try_get::<Option<String>, _>("terminal_result_hash")?
            .map(parse_hash)
            .transpose()?,
        output_object: None,
        error: execution.try_get("error_json")?,
    })
}

async fn update_governance_cursor(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    cursor: u64,
    payload: &RuntimeEventPayloadV1,
) -> RuntimeResult<()> {
    match payload {
        RuntimeEventPayloadV1::ApprovalChanged {
            task_id,
            task_version,
            ..
        } => {
            sqlx::query("UPDATE approval_tasks SET last_event_cursor=? WHERE tenant_id=? AND id=? AND version<=?").bind(cursor).bind(tenant_id).bind(task_id).bind(task_version).execute(&mut **tx).await?;
        }
        RuntimeEventPayloadV1::EvaluationChanged {
            run_id,
            run_version,
            ..
        } => {
            sqlx::query("UPDATE evaluation_runs SET last_event_cursor=? WHERE tenant_id=? AND id=? AND version<=?").bind(cursor).bind(tenant_id).bind(run_id).bind(run_version).execute(&mut **tx).await?;
        }
        RuntimeEventPayloadV1::DebugChanged {
            work_package_id,
            package_version,
            ..
        } => {
            sqlx::query("UPDATE runtime_work_packages SET last_event_cursor=? WHERE tenant_id=? AND id=? AND version<=?").bind(cursor).bind(tenant_id).bind(work_package_id).bind(package_version).execute(&mut **tx).await?;
        }
        RuntimeEventPayloadV1::RetentionChanged {
            run_id,
            run_version,
            ..
        } => {
            sqlx::query("UPDATE retention_runs SET last_event_cursor=? WHERE tenant_id=? AND id=? AND policy_version<=?").bind(cursor).bind(tenant_id).bind(run_id).bind(run_version).execute(&mut **tx).await?;
        }
        RuntimeEventPayloadV1::NotificationChanged {
            notification_id,
            notification_version,
            ..
        } => {
            sqlx::query("UPDATE notifications SET last_event_cursor=? WHERE tenant_id=? AND id=? AND version<=?").bind(cursor).bind(tenant_id).bind(notification_id).bind(notification_version).execute(&mut **tx).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn load_snapshot_kind(
    state: &RuntimeState,
    tenant_id: Uuid,
    kind: RuntimeGovernanceObjectKindV1,
    upper: u64,
    after: Option<Uuid>,
    limit: usize,
) -> RuntimeResult<Vec<RuntimeGovernanceSnapshotItemV1>> {
    match kind {
        RuntimeGovernanceObjectKindV1::Approval => {
            approval_snapshot(state, tenant_id, upper, after, limit).await
        }
        RuntimeGovernanceObjectKindV1::Evaluation => {
            evaluation_snapshot(state, tenant_id, upper, after, limit).await
        }
        RuntimeGovernanceObjectKindV1::Notification => {
            notification_snapshot(state, tenant_id, upper, after, limit).await
        }
        RuntimeGovernanceObjectKindV1::Debug => {
            debug_snapshot(state, tenant_id, upper, after, limit).await
        }
        RuntimeGovernanceObjectKindV1::Retention => {
            retention_snapshot(state, tenant_id, upper, after, limit).await
        }
    }
}

async fn approval_snapshot(
    state: &RuntimeState,
    tenant: Uuid,
    upper: u64,
    after: Option<Uuid>,
    limit: usize,
) -> RuntimeResult<Vec<RuntimeGovernanceSnapshotItemV1>> {
    let rows = sqlx::query("SELECT id,version,last_event_cursor,projection_deleted,execution_id,workflow_id,node_id,title,description,request_payload_json,buttons_json,status,resume_status,claimed_by,deadline_at,decision_receipt_json FROM approval_tasks WHERE tenant_id=? AND last_event_cursor<=? AND (? IS NULL OR id>?) ORDER BY id LIMIT ?")
        .bind(tenant).bind(upper).bind(after).bind(after).bind(limit as u32).fetch_all(&state.pool).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let task_id = row.try_get("id")?;
        items.push(RuntimeGovernanceSnapshotItemV1 {
            object_type: RuntimeGovernanceObjectKindV1::Approval,
            object_id: task_id,
            object_version: row.try_get("version")?,
            last_event_cursor: row.try_get("last_event_cursor")?,
            deleted: row.try_get("projection_deleted")?,
            payload: RuntimeGovernanceSnapshotPayloadV1::Approval {
                execution_id: row.try_get("execution_id")?,
                workflow_id: row.try_get("workflow_id")?,
                node_id: row.try_get("node_id")?,
                title: row.try_get("title")?,
                description: row.try_get("description")?,
                request: row.try_get("request_payload_json")?,
                buttons: approval_buttons(row.try_get("buttons_json")?)?,
                status: row.try_get("status")?,
                resume_status: row.try_get("resume_status")?,
                claimed_by: row.try_get("claimed_by")?,
                deadline_at: row.try_get("deadline_at")?,
                decision: row.try_get("decision_receipt_json")?,
                candidates: approval_candidates_from_pool(state, tenant, task_id).await?,
            },
        });
    }
    Ok(items)
}

async fn evaluation_snapshot(
    state: &RuntimeState,
    tenant: Uuid,
    upper: u64,
    after: Option<Uuid>,
    limit: usize,
) -> RuntimeResult<Vec<RuntimeGovernanceSnapshotItemV1>> {
    let rows = sqlx::query("SELECT r.id,r.version,r.last_event_cursor,r.projection_deleted,r.work_package_id,r.status,CAST((SELECT COUNT(*) FROM evaluation_run_cases c WHERE c.tenant_id=r.tenant_id AND c.evaluation_run_id=r.id AND c.status IN ('completed','failed','cancelled')) AS UNSIGNED) completed_cases,CAST((SELECT COUNT(*) FROM evaluation_run_cases c WHERE c.tenant_id=r.tenant_id AND c.evaluation_run_id=r.id) AS UNSIGNED) total_cases FROM evaluation_runs r WHERE r.tenant_id=? AND r.last_event_cursor<=? AND (? IS NULL OR r.id>?) ORDER BY r.id LIMIT ?")
        .bind(tenant).bind(upper).bind(after).bind(after).bind(limit as u32).fetch_all(&state.pool).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let run_id = row.try_get("id")?;
        let status: String = row.try_get("status")?;
        let report = if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
            Some(load_evaluation_report_from_pool(state, tenant, run_id).await?)
        } else {
            None
        };
        items.push(RuntimeGovernanceSnapshotItemV1 {
            object_type: RuntimeGovernanceObjectKindV1::Evaluation,
            object_id: run_id,
            object_version: row.try_get("version")?,
            last_event_cursor: row.try_get("last_event_cursor")?,
            deleted: row.try_get("projection_deleted")?,
            payload: RuntimeGovernanceSnapshotPayloadV1::Evaluation {
                work_package_id: row.try_get("work_package_id")?,
                status: status.clone(),
                completed_cases: row.try_get("completed_cases")?,
                total_cases: row.try_get("total_cases")?,
                report,
            },
        });
    }
    Ok(items)
}

async fn notification_snapshot(
    state: &RuntimeState,
    tenant: Uuid,
    upper: u64,
    after: Option<Uuid>,
    limit: usize,
) -> RuntimeResult<Vec<RuntimeGovernanceSnapshotItemV1>> {
    sqlx::query("SELECT id,version,last_event_cursor,projection_deleted,notification_type,title_key,body_key,arguments_json,target_type,target_id,target_path,tone FROM notifications WHERE tenant_id=? AND last_event_cursor<=? AND (? IS NULL OR id>?) ORDER BY id LIMIT ?")
        .bind(tenant).bind(upper).bind(after).bind(after).bind(limit as u32).fetch_all(&state.pool).await?.into_iter().map(|r|Ok(RuntimeGovernanceSnapshotItemV1{object_type:RuntimeGovernanceObjectKindV1::Notification,object_id:r.try_get("id")?,object_version:r.try_get("version")?,last_event_cursor:r.try_get("last_event_cursor")?,deleted:r.try_get("projection_deleted")?,payload:RuntimeGovernanceSnapshotPayloadV1::Notification{notification_type:r.try_get("notification_type")?,title_key:r.try_get("title_key")?,body_key:r.try_get("body_key")?,arguments:r.try_get("arguments_json")?,target_type:r.try_get("target_type")?,target_id:r.try_get("target_id")?,target_path:r.try_get("target_path")?,tone:r.try_get("tone")?}})).collect()
}

async fn debug_snapshot(
    state: &RuntimeState,
    tenant: Uuid,
    upper: u64,
    after: Option<Uuid>,
    limit: usize,
) -> RuntimeResult<Vec<RuntimeGovernanceSnapshotItemV1>> {
    sqlx::query("SELECT id,version,last_event_cursor,projection_deleted,status,result_json,expires_at FROM runtime_work_packages WHERE tenant_id=? AND purpose='debug' AND last_event_cursor<=? AND (? IS NULL OR id>?) ORDER BY id LIMIT ?")
        .bind(tenant).bind(upper).bind(after).bind(after).bind(limit as u32).fetch_all(&state.pool).await?.into_iter().map(|r|{let id=r.try_get("id")?;Ok(RuntimeGovernanceSnapshotItemV1{object_type:RuntimeGovernanceObjectKindV1::Debug,object_id:id,object_version:r.try_get("version")?,last_event_cursor:r.try_get("last_event_cursor")?,deleted:r.try_get("projection_deleted")?,payload:RuntimeGovernanceSnapshotPayloadV1::Debug{work_package_id:id,status:r.try_get("status")?,result:r.try_get("result_json")?,expires_at:r.try_get("expires_at")?}})}).collect()
}

async fn retention_snapshot(
    state: &RuntimeState,
    tenant: Uuid,
    upper: u64,
    after: Option<Uuid>,
    limit: usize,
) -> RuntimeResult<Vec<RuntimeGovernanceSnapshotItemV1>> {
    let rows = sqlx::query("SELECT id,policy_version,last_event_cursor,projection_deleted,status,candidate_count,deleted_count,dry_run,CAST((SELECT COUNT(*) FROM retention_items i WHERE i.tenant_id=r.tenant_id AND i.retention_run_id=r.id AND i.status='failed') AS UNSIGNED) failed_count FROM retention_runs r WHERE tenant_id=? AND last_event_cursor<=? AND (? IS NULL OR id>?) ORDER BY id LIMIT ?")
        .bind(tenant).bind(upper).bind(after).bind(after).bind(limit as u32).fetch_all(&state.pool).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let run_id = row.try_get("id")?;
        items.push(RuntimeGovernanceSnapshotItemV1 {
            object_type: RuntimeGovernanceObjectKindV1::Retention,
            object_id: run_id,
            object_version: row.try_get("policy_version")?,
            last_event_cursor: row.try_get("last_event_cursor")?,
            deleted: row.try_get("projection_deleted")?,
            payload: RuntimeGovernanceSnapshotPayloadV1::Retention {
                status: row.try_get("status")?,
                marked_count: row.try_get("candidate_count")?,
                deleted_count: row.try_get("deleted_count")?,
                failed_count: row.try_get("failed_count")?,
                dry_run: row.try_get("dry_run")?,
                items: retention_items_from_pool(state, tenant, run_id).await?,
            },
        });
    }
    Ok(items)
}

async fn approval_candidates_from_pool(
    state: &RuntimeState,
    tenant_id: Uuid,
    task_id: Uuid,
) -> RuntimeResult<Vec<RuntimeApprovalCandidateV1>> {
    let rows = sqlx::query("SELECT candidate_type,candidate_id FROM approval_candidates WHERE tenant_id=? AND approval_task_id=? ORDER BY candidate_type,candidate_id")
        .bind(tenant_id).bind(task_id).fetch_all(&state.pool).await?;
    rows.into_iter()
        .map(|row| {
            let candidate_type = match row.try_get::<String, _>("candidate_type")?.as_str() {
                "user" => RuntimeApprovalCandidateKindV1::User,
                "role" => RuntimeApprovalCandidateKindV1::Role,
                "department" => RuntimeApprovalCandidateKindV1::Department,
                value => {
                    return Err(RuntimeError::Internal(anyhow::anyhow!(
                        "unsupported Approval candidate type {value}"
                    )));
                }
            };
            Ok(RuntimeApprovalCandidateV1 {
                candidate_type,
                candidate_id: row.try_get("candidate_id")?,
            })
        })
        .collect()
}

fn approval_buttons(value: Value) -> RuntimeResult<Vec<RuntimeApprovalButtonV1>> {
    serde_json::from_value(value).map_err(|error| RuntimeError::Internal(error.into()))
}

async fn load_evaluation_report_from_pool(
    state: &RuntimeState,
    tenant_id: Uuid,
    run_id: Uuid,
) -> RuntimeResult<RuntimeEvaluationReportV1> {
    let case_rows = sqlx::query("SELECT id,source_case_id,target_command_id,target_execution_id,status,actual_output_json,duration_ms,cost_micros,error_code,error_message FROM evaluation_run_cases WHERE tenant_id=? AND evaluation_run_id=? ORDER BY created_at,id")
        .bind(tenant_id).bind(run_id).fetch_all(&state.pool).await?;
    let mut cases = Vec::with_capacity(case_rows.len());
    let mut completed_cases = 0_u64;
    let mut passed_rules = 0_u64;
    let mut failed_rules = 0_u64;
    let mut error_rules = 0_u64;
    let mut total_cost_micros = 0_u64;
    for row in case_rows {
        let case_id: Uuid = row.try_get("id")?;
        let rule_rows = sqlx::query("SELECT id,profile_rule_id,evaluator_execution_id,status,passed,CAST(score AS DOUBLE) score,detail_json,duration_ms,cost_micros FROM evaluation_rule_results WHERE tenant_id=? AND evaluation_run_case_id=? ORDER BY created_at,id")
            .bind(tenant_id).bind(case_id).fetch_all(&state.pool).await?;
        let mut rules = Vec::with_capacity(rule_rows.len());
        for rule in rule_rows {
            let status: String = rule.try_get("status")?;
            match status.as_str() {
                "passed" => passed_rules += 1,
                "failed" => failed_rules += 1,
                "error" => error_rules += 1,
                _ => {}
            }
            rules.push(RuntimeEvaluationRuleResultV1 {
                id: rule.try_get("id")?,
                profile_rule_id: rule.try_get("profile_rule_id")?,
                evaluator_execution_id: rule.try_get("evaluator_execution_id")?,
                status,
                passed: rule.try_get("passed")?,
                score: rule.try_get("score")?,
                detail: rule.try_get("detail_json")?,
                duration_ms: rule.try_get("duration_ms")?,
                cost_micros: rule.try_get("cost_micros")?,
            });
        }
        let status: String = row.try_get("status")?;
        if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
            completed_cases += 1;
        }
        let cost_micros = row.try_get("cost_micros")?;
        total_cost_micros = total_cost_micros.saturating_add(cost_micros);
        cases.push(RuntimeEvaluationCaseResultV1 {
            id: case_id,
            source_case_id: row.try_get("source_case_id")?,
            target_command_id: row.try_get("target_command_id")?,
            target_execution_id: row.try_get("target_execution_id")?,
            status,
            actual_output: row.try_get("actual_output_json")?,
            duration_ms: row.try_get("duration_ms")?,
            cost_micros,
            error_code: row.try_get("error_code")?,
            error_message: row.try_get("error_message")?,
            rules,
        });
    }
    Ok(RuntimeEvaluationReportV1 {
        metrics: RuntimeEvaluationMetricsV1 {
            total_cases: cases.len() as u64,
            completed_cases,
            passed_rules,
            failed_rules,
            error_rules,
            total_cost_micros,
        },
        cases,
    })
}

async fn retention_items_from_pool(
    state: &RuntimeState,
    tenant_id: Uuid,
    run_id: Uuid,
) -> RuntimeResult<Vec<RuntimeRetentionItemV1>> {
    sqlx::query("SELECT id,data_type,target_id,status,reason,attempt_count FROM retention_items WHERE tenant_id=? AND retention_run_id=? ORDER BY created_at,id")
        .bind(tenant_id).bind(run_id).fetch_all(&state.pool).await?.into_iter().map(|row| Ok(RuntimeRetentionItemV1 {
            id: row.try_get("id")?, data_type: row.try_get("data_type")?, target_id: row.try_get("target_id")?,
            status: row.try_get("status")?, reason: row.try_get("reason")?, attempt_count: row.try_get("attempt_count")?,
        })).collect()
}

fn event_type(payload: &RuntimeEventPayloadV1) -> String {
    serde_json::to_value(payload)
        .ok()
        .and_then(|v| v.get("kind").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "runtime_event".into())
}
fn aggregate_type(payload: &RuntimeEventPayloadV1) -> &'static str {
    match payload {
        RuntimeEventPayloadV1::ApprovalChanged { .. } => "approval",
        RuntimeEventPayloadV1::EvaluationChanged { .. } => "evaluation",
        RuntimeEventPayloadV1::DebugChanged { .. } => "debug",
        RuntimeEventPayloadV1::RetentionChanged { .. } => "retention",
        RuntimeEventPayloadV1::NotificationChanged { .. } => "notification",
        RuntimeEventPayloadV1::RuntimeCallChanged { .. } => "runtime_call",
        _ => "execution",
    }
}
fn aggregate_id(payload: &RuntimeEventPayloadV1, execution: Option<Uuid>) -> RuntimeResult<String> {
    Ok(match payload {
        RuntimeEventPayloadV1::ApprovalChanged { task_id, .. } => task_id.to_string(),
        RuntimeEventPayloadV1::EvaluationChanged { run_id, .. } => run_id.to_string(),
        RuntimeEventPayloadV1::DebugChanged { package_id, .. } => package_id.to_string(),
        RuntimeEventPayloadV1::RetentionChanged { run_id, .. } => run_id.to_string(),
        RuntimeEventPayloadV1::NotificationChanged {
            notification_id, ..
        } => notification_id.to_string(),
        RuntimeEventPayloadV1::RuntimeCallChanged { call_id, .. } => call_id.to_string(),
        _ => execution
            .ok_or_else(|| {
                RuntimeError::Internal(anyhow::anyhow!(
                    "Execution Integration Event has no Execution ID"
                ))
            })?
            .to_string(),
    })
}
fn aggregate_version(payload: &RuntimeEventPayloadV1, fallback: u64) -> u64 {
    match payload {
        RuntimeEventPayloadV1::ApprovalChanged { task_version, .. } => *task_version,
        RuntimeEventPayloadV1::EvaluationChanged { run_version, .. } => *run_version,
        RuntimeEventPayloadV1::DebugChanged {
            package_version, ..
        } => *package_version,
        RuntimeEventPayloadV1::RetentionChanged { run_version, .. } => *run_version,
        RuntimeEventPayloadV1::NotificationChanged {
            notification_version,
            ..
        } => *notification_version,
        _ => fallback,
    }
}

pub(crate) async fn enqueue_governance_event(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Option<Uuid>,
    correlation_id: Uuid,
    payload: &RuntimeEventPayloadV1,
) -> RuntimeResult<Uuid> {
    let outbox_id = Uuid::now_v7();
    let aggregate_id = aggregate_id(payload, execution_id)?;
    sqlx::query("INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,event_type,aggregate_type,aggregate_id,aggregate_version,correlation_id,payload_json,status) VALUES(?,?,?,'runtime_event',?,?,?,?,?,?,'pending')")
        .bind(outbox_id)
        .bind(tenant_id)
        .bind(execution_id)
        .bind(event_type(payload))
        .bind(aggregate_type(payload))
        .bind(aggregate_id)
        .bind(aggregate_version(payload, 1))
        .bind(correlation_id)
        .bind(serde_json::to_value(payload).map_err(|error| RuntimeError::Internal(error.into()))?)
        .execute(&mut **tx)
        .await?;
    Ok(outbox_id)
}

pub(crate) async fn enqueue_approval_event_from_task(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    task_id: Uuid,
) -> RuntimeResult<Uuid> {
    let row = sqlx::query("SELECT execution_id,workflow_id,node_id,title,description,request_payload_json,buttons_json,status,resume_status,claimed_by,deadline_at,version,decision_receipt_json FROM approval_tasks WHERE tenant_id=? AND id=?")
        .bind(tenant_id)
        .bind(task_id)
        .fetch_one(&mut **tx)
        .await?;
    let execution_id: Uuid = row.try_get("execution_id")?;
    let status: String = row.try_get("status")?;
    let claimed_by: Option<Uuid> = row.try_get("claimed_by")?;
    let candidates = approval_candidates(tx, tenant_id, task_id).await?;
    let notification_target = (status == "pending")
        .then(|| single_user_candidate(&candidates))
        .flatten()
        .or(claimed_by);
    let approval_event_id = enqueue_governance_event(
        tx,
        tenant_id,
        Some(execution_id),
        execution_id,
        &RuntimeEventPayloadV1::ApprovalChanged {
            task_id,
            task_version: row.try_get("version")?,
            execution_id,
            workflow_id: row.try_get("workflow_id")?,
            node_id: row.try_get("node_id")?,
            title: row.try_get("title")?,
            description: row.try_get("description")?,
            request: row.try_get("request_payload_json")?,
            buttons: approval_buttons(row.try_get("buttons_json")?)?,
            status,
            resume_status: row.try_get("resume_status")?,
            claimed_by,
            deadline_at: row.try_get("deadline_at")?,
            decision: row.try_get("decision_receipt_json")?,
            candidates,
        },
    )
    .await?;
    if let Some(target_user_id) = notification_target {
        enqueue_approval_notification(
            tx,
            tenant_id,
            execution_id,
            approval_event_id,
            task_id,
            target_user_id,
        )
        .await?;
    }
    Ok(approval_event_id)
}

async fn enqueue_approval_notification(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    source_event_id: Uuid,
    task_id: Uuid,
    target_user_id: Uuid,
) -> RuntimeResult<()> {
    const NOTIFICATION_TYPE: &str = "approval_reassigned";
    let notification_id = deterministic_notification_id(source_event_id, NOTIFICATION_TYPE);
    let inserted = sqlx::query("INSERT IGNORE INTO notifications(id,tenant_id,source_event_id,notification_type,title_key,body_key,arguments_json,target_type,target_id,target_path,tone,version) VALUES(?,?,?,?,'notifications.approvalReassigned.title','notifications.approvalReassigned.body',?,'user',?,?,'primary',1)")
        .bind(notification_id)
        .bind(tenant_id)
        .bind(source_event_id)
        .bind(NOTIFICATION_TYPE)
        .bind(serde_json::json!({"taskId": task_id}))
        .bind(target_user_id)
        .bind(format!("/approvals/{task_id}"))
        .execute(&mut **tx)
        .await?;
    if inserted.rows_affected() == 1 {
        enqueue_governance_event(
            tx,
            tenant_id,
            Some(execution_id),
            source_event_id,
            &RuntimeEventPayloadV1::NotificationChanged {
                notification_id,
                notification_version: 1,
                notification_type: NOTIFICATION_TYPE.into(),
                title_key: "notifications.approvalReassigned.title".into(),
                body_key: "notifications.approvalReassigned.body".into(),
                arguments: serde_json::json!({"taskId": task_id}),
                target_type: "user".into(),
                target_id: target_user_id,
                target_path: format!("/approvals/{task_id}"),
                tone: "primary".into(),
            },
        )
        .await?;
    }
    Ok(())
}

fn deterministic_notification_id(source_event_id: Uuid, notification_type: &str) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(source_event_id.as_bytes());
    digest.update([0]);
    digest.update(notification_type.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

async fn approval_candidates(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    task_id: Uuid,
) -> RuntimeResult<Vec<RuntimeApprovalCandidateV1>> {
    let rows = sqlx::query("SELECT candidate_type,candidate_id FROM approval_candidates WHERE tenant_id=? AND approval_task_id=? ORDER BY candidate_type,candidate_id")
        .bind(tenant_id)
        .bind(task_id)
        .fetch_all(&mut **tx)
        .await?;
    rows.into_iter()
        .map(|row| {
            let candidate_type = match row.try_get::<String, _>("candidate_type")?.as_str() {
                "user" => RuntimeApprovalCandidateKindV1::User,
                "role" => RuntimeApprovalCandidateKindV1::Role,
                "department" => RuntimeApprovalCandidateKindV1::Department,
                value => {
                    return Err(RuntimeError::Internal(anyhow::anyhow!(
                        "unsupported Approval candidate type {value}"
                    )));
                }
            };
            Ok(RuntimeApprovalCandidateV1 {
                candidate_type,
                candidate_id: row.try_get("candidate_id")?,
            })
        })
        .collect()
}

fn single_user_candidate(candidates: &[RuntimeApprovalCandidateV1]) -> Option<Uuid> {
    let mut users = candidates
        .iter()
        .filter(|candidate| candidate.candidate_type == RuntimeApprovalCandidateKindV1::User)
        .map(|candidate| candidate.candidate_id);
    let candidate = users.next()?;
    users.next().is_none().then_some(candidate)
}

fn parse_hash(value: String) -> RuntimeResult<ContentHash> {
    ContentHash::parse(value).map_err(|e| RuntimeError::Internal(e.into()))
}
fn encode_snapshot_cursor(value: &SnapshotCursor) -> RuntimeResult<String> {
    serde_json::to_vec(value)
        .map(|v| URL_SAFE_NO_PAD.encode(v))
        .map_err(|e| RuntimeError::Internal(e.into()))
}
fn decode_snapshot_cursor(value: &str) -> RuntimeResult<SnapshotCursor> {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .ok_or(RuntimeError::QueryCursorExpired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_notification_requires_exactly_one_user_candidate() {
        assert_eq!(single_user_candidate(&[]), None);
        let user = Uuid::now_v7();
        let candidate = RuntimeApprovalCandidateV1 {
            candidate_type: RuntimeApprovalCandidateKindV1::User,
            candidate_id: user,
        };
        assert_eq!(
            single_user_candidate(std::slice::from_ref(&candidate)),
            Some(user)
        );
        assert_eq!(
            single_user_candidate(&[
                candidate,
                RuntimeApprovalCandidateV1 {
                    candidate_type: RuntimeApprovalCandidateKindV1::User,
                    candidate_id: Uuid::now_v7(),
                },
            ]),
            None
        );
    }
}
