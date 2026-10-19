use agentx_runtime_contracts::{
    ApplyReceiptV1, CancelWorkPackageRequestV1, ExecuteWorkPackageRequestV1, ExecutionCommandV1,
    PrepareWorkPackageRequestV1, PublishReceiptStatusV1, PublishReceiptV1, ReferenceCheckReceiptV1,
    ReferenceCheckRequestV1, RetentionCommandRequestV1, RuntimeCallPurposeV1,
    RuntimeCommandApplyRequestV1, RuntimePublishErrorCodeV1, RuntimeReferenceBlockV1,
    RuntimeResourceBindingV1, WorkPackagePurpose,
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

pub async fn apply_work_package_action(
    State(state): State<RuntimeState>,
    Path(package_action): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> RuntimeResult<Json<ApplyReceiptV1>> {
    let (package_id, action) = package_action
        .rsplit_once(':')
        .ok_or(RuntimeError::NotFound)?;
    let package_id = Uuid::parse_str(package_id).map_err(|_| RuntimeError::NotFound)?;
    match action {
        "execute" => {
            let request =
                serde_json::from_slice::<ExecuteWorkPackageRequestV1>(&body).map_err(|error| {
                    RuntimeError::BadRequest(
                        RuntimePublishErrorCodeV1::BundleReferenceConflict,
                        format!("Invalid Work Package execute request: {error}"),
                    )
                })?;
            execute_work_package(State(state), Path(package_id), headers, Json(request)).await
        }
        "cancel" => {
            let request =
                serde_json::from_slice::<CancelWorkPackageRequestV1>(&body).map_err(|error| {
                    RuntimeError::BadRequest(
                        RuntimePublishErrorCodeV1::BundleReferenceConflict,
                        format!("Invalid Work Package cancel request: {error}"),
                    )
                })?;
            cancel_work_package(State(state), Path(package_id), headers, Json(request)).await
        }
        _ => Err(RuntimeError::NotFound),
    }
}

pub async fn prepare_work_package(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<PrepareWorkPackageRequestV1>,
) -> RuntimeResult<Json<PublishReceiptV1>> {
    state
        .trust
        .publisher(&headers, "runtime.work-packages.prepare")?;
    let package = &request.work_package;
    let key = state.trust.work_package_key(&package.signature.key_id)?;
    package.verify(key).map_err(|error| {
        RuntimeError::BadRequest(
            if matches!(
                error,
                agentx_runtime_contracts::ContractError::ContentHashMismatch
            ) {
                RuntimePublishErrorCodeV1::ContentHashMismatch
            } else {
                RuntimePublishErrorCodeV1::InvalidSignature
            },
            error.to_string(),
        )
    })?;
    if package.payload.expires_at <= OffsetDateTime::now_utc() {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::BundleReferenceConflict,
            "Work Package is already expired".into(),
        ));
    }
    for object in &package.payload.objects {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_objects WHERE tenant_id=? AND object_id=? AND object_key=? AND content_hash=? AND size_bytes=? AND media_type=? AND status='ready')",
        )
        .bind(object.tenant_id)
        .bind(object.object_id)
        .bind(&object.object_key)
        .bind(object.content_hash.as_str())
        .bind(object.size_bytes)
        .bind(&object.media_type)
        .fetch_one(&state.pool)
        .await?;
        if !found {
            return Err(RuntimeError::BadRequest(
                RuntimePublishErrorCodeV1::ObjectMissing,
                format!("Work Package object {} is unavailable", object.object_id),
            ));
        }
    }
    let composites = crate::composite::materialize(
        &state,
        package.payload.tenant_id,
        &package.payload.resources,
        &package.payload.objects,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    if let Some(row) = sqlx::query(
        "SELECT content_hash,prepare_idempotency_key FROM runtime_work_packages WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(package.payload.tenant_id)
    .bind(package.payload.package_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        if row.try_get::<String, _>("content_hash")? != package.content_hash.as_str()
            || row.try_get::<String, _>("prepare_idempotency_key")? != request.idempotency_key
        {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Work Package identity is already bound to different immutable content".into(),
            ));
        }
        tx.commit().await?;
        return Ok(Json(package_receipt(package.payload.package_id, true)));
    }
    let signature = STANDARD
        .decode(&package.signature.signature_base64)
        .map_err(|_| {
            RuntimeError::BadRequest(
                RuntimePublishErrorCodeV1::InvalidSignature,
                "Work Package signature is not base64".into(),
            )
        })?;
    sqlx::query(
        "INSERT INTO runtime_work_packages(id,tenant_id,purpose,call_purpose,source_revision,schema_version,content_hash,signature_key_id,signature,prepare_idempotency_key,payload_json,worker_compatibility_json,status,expires_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,'prepared',?)",
    )
    .bind(package.payload.package_id)
    .bind(package.payload.tenant_id)
    .bind(work_package_purpose(package.payload.purpose))
    .bind(runtime_call_purpose(package.payload.call_purpose))
    .bind(&package.payload.source_revision)
    .bind(package.payload.schema_version)
    .bind(package.content_hash.as_str())
    .bind(&package.signature.key_id)
    .bind(signature)
    .bind(&request.idempotency_key)
    .bind(serde_json::to_value(&package.payload).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(&package.payload.worker_compatibility).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(package.payload.expires_at)
    .execute(&mut *tx)
    .await?;
    for object in &package.payload.objects {
        sqlx::query(
            "INSERT INTO runtime_work_package_objects(tenant_id,work_package_id,object_id,content_hash,reference_role) VALUES(?,?,?,?,'immutable_closure')",
        )
        .bind(package.payload.tenant_id)
        .bind(package.payload.package_id)
        .bind(object.object_id)
        .bind(object.content_hash.as_str())
        .execute(&mut *tx)
        .await?;
    }
    persist_resource_bindings(
        &mut tx,
        package.payload.tenant_id,
        package.payload.package_id,
        &package.payload.resources,
        &composites,
    )
    .await?;
    if let agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Evaluation { cases, .. } =
        &package.payload.spec
    {
        let input_schema = &package.payload.definition["start"]["inputs"];
        let all_complete = package.payload.definition["end"]["completion"] == "all_complete";
        let output_schema = serde_json::json!({"type":"object","properties":package.payload.definition["end"]["outputs"].as_object().map(|outputs|outputs.iter().map(|(key,output)| {
            let schema = output["schema"].clone();
            let schema = if all_complete { json!({"type":"array","items":if output["required"]==true {schema} else {json!({"anyOf":[schema,{"type":"null"}]})}}) } else {schema};
            (key.clone(),schema)
        }).collect::<serde_json::Map<_,_>>()).unwrap_or_default()});
        for case in cases {
            for (value, schema) in [
                (&case.input, input_schema),
                (
                    case.expected_output
                        .as_ref()
                        .unwrap_or(&serde_json::Value::Null),
                    &output_schema,
                ),
            ] {
                let ids = crate::execution::validate_input_artifacts(
                    &mut tx,
                    package.payload.tenant_id,
                    value,
                    schema,
                )
                .await?;
                for id in ids {
                    sqlx::query("INSERT IGNORE INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role,retention_until) VALUES(?,?,'evaluation',?,'test_snapshot',?)").bind(package.payload.tenant_id).bind(id).bind(package.payload.package_id.to_string()).bind(package.payload.expires_at).execute(&mut *tx).await?;
                }
            }
        }
    }
    for judge in &package.payload.model_evaluator_executions {
        crate::platform_evaluation::prepare(&mut tx, &judge.model).await?;
        let binding = &judge.model.binding;
        sqlx::query("INSERT IGNORE INTO runtime_resource_states(tenant_id,resource_kind,resource_id,resource_version,state_epoch,status,content_hash) VALUES(?,'model',?,?,?,'active',?)")
            .bind(package.payload.tenant_id).bind(binding.resource_id).bind(&binding.resource_version).bind(binding.state_epoch).bind(binding.content_hash.as_str()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(package_receipt(package.payload.package_id, false)))
}

pub async fn execute_work_package(
    State(state): State<RuntimeState>,
    Path(package_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<ExecuteWorkPackageRequestV1>,
) -> RuntimeResult<Json<ApplyReceiptV1>> {
    state
        .trust
        .publisher(&headers, "runtime.work-packages.execute")?;
    if request.package_id != package_id {
        return Err(RuntimeError::BadRequest(
            RuntimePublishErrorCodeV1::BundleReferenceConflict,
            "Work Package path and body IDs differ".into(),
        ));
    }
    let request_hash = agentx_runtime_contracts::content_hash(&request)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "SELECT tenant_id,status,version,execute_idempotency_key,execute_request_hash,payload_json,result_json FROM runtime_work_packages WHERE id=? FOR UPDATE",
    )
    .bind(package_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(RuntimeError::NotFound)?;
    if let Some(key) = row.try_get::<Option<String>, _>("execute_idempotency_key")? {
        if key != request.idempotency_key
            || row
                .try_get::<Option<String>, _>("execute_request_hash")?
                .as_deref()
                != Some(request_hash.as_str())
        {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Work Package execute key was reused with different input".into(),
            ));
        }
        let result: Value = row
            .try_get::<Option<Value>, _>("result_json")?
            .ok_or_else(|| {
                RuntimeError::Internal(anyhow::anyhow!(
                    "executed Work Package {package_id} has no persisted result"
                ))
            })?;
        tx.commit().await?;
        return Ok(Json(apply_receipt(
            package_id,
            row.try_get("version")?,
            true,
            result,
        )));
    }
    if row.try_get::<String, _>("status")? != "prepared" {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::BundleReferenceConflict,
            "Only a prepared Work Package can execute".into(),
        ));
    }
    let package: agentx_runtime_contracts::RuntimeWorkPackagePayloadV1 =
        serde_json::from_value(row.try_get("payload_json")?)
            .map_err(|error| RuntimeError::Internal(error.into()))?;
    if package.expires_at <= OffsetDateTime::now_utc() {
        sqlx::query(
            "UPDATE runtime_work_packages SET status='expired',version=version+1 WHERE id=?",
        )
        .bind(package_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::BundleReferenceConflict,
            "Work Package expired".into(),
        ));
    }
    let input = if request.input.is_null() {
        package.overlay.input.clone()
    } else {
        request.input.clone()
    };
    let started = crate::work_package_execution::start(&mut tx, &package, &input).await?;
    sqlx::query(
        "UPDATE runtime_work_packages SET status='running',version=version+1,execute_idempotency_key=?,execute_request_hash=?,result_json=?,started_at=UTC_TIMESTAMP(6) WHERE id=? AND status='prepared'",
    )
    .bind(&request.idempotency_key)
    .bind(request_hash.as_str())
    .bind(&started.result)
    .bind(package_id)
    .execute(&mut *tx)
    .await?;
    let version = row.try_get::<u64, _>("version")? + 1;
    tx.commit().await?;
    Ok(Json(apply_receipt(
        package_id,
        version,
        false,
        started.result,
    )))
}

pub async fn cancel_work_package(
    State(state): State<RuntimeState>,
    Path(package_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<CancelWorkPackageRequestV1>,
) -> RuntimeResult<Json<ApplyReceiptV1>> {
    state
        .trust
        .publisher(&headers, "runtime.work-packages.cancel")?;
    if request.package_id != package_id {
        return Err(RuntimeError::BadRequest(
            RuntimePublishErrorCodeV1::BundleReferenceConflict,
            "Work Package path and body IDs differ".into(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "SELECT tenant_id,status,version,cancel_idempotency_key FROM runtime_work_packages WHERE id=? FOR UPDATE",
    )
    .bind(package_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(RuntimeError::NotFound)?;
    let version: u64 = row.try_get("version")?;
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    if tenant_id != request.tenant_id {
        return Err(RuntimeError::Unauthorized);
    }
    let current_status: String = row.try_get("status")?;
    if !matches!(
        current_status.as_str(),
        "prepared" | "running" | "cancelled"
    ) {
        tx.commit().await?;
        return Ok(Json(apply_receipt(
            package_id,
            version,
            true,
            json!({"packageId":package_id,"status":current_status}),
        )));
    }
    if let Some(key) = row.try_get::<Option<String>, _>("cancel_idempotency_key")? {
        if key != request.idempotency_key {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Work Package cancel key conflicts".into(),
            ));
        }
        tx.commit().await?;
        return Ok(Json(apply_receipt(
            package_id,
            version,
            true,
            json!({"packageId":package_id,"status":"cancelled"}),
        )));
    }
    if version != request.expected_version {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::HeadVersionConflict,
            "Work Package Version changed".into(),
        ));
    }
    sqlx::query(
        "UPDATE runtime_work_packages SET status='cancelled',version=version+1,cancellation_version=cancellation_version+1,cancel_idempotency_key=?,completed_at=UTC_TIMESTAMP(6) WHERE id=? AND status IN ('prepared','running')",
    )
    .bind(&request.idempotency_key)
    .bind(package_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) SELECT UUID_TO_BIN(UUID()),tenant_id,'cancel_execution','execution',CAST(BIN_TO_UUID(id) AS CHAR),CONCAT(?,':',BIN_TO_UUID(id)),JSON_OBJECT('workPackageId',?), 'pending' FROM workflow_executions WHERE tenant_id=? AND work_package_id=? AND status IN ('queued','running','waiting')",
    )
    .bind(format!("work-package:cancel:{package_id}:{}", request.idempotency_key))
    .bind(package_id.to_string())
    .bind(tenant_id)
    .bind(package_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE evaluation_rule_results rr JOIN evaluation_run_cases c ON c.id=rr.evaluation_run_case_id AND c.tenant_id=rr.tenant_id JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id SET rr.status='cancelled',rr.completed_at=UTC_TIMESTAMP(6) WHERE r.tenant_id=? AND r.work_package_id=? AND rr.status IN ('queued','running')",
    )
    .bind(tenant_id)
    .bind(package_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE evaluation_run_cases c JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id SET c.status='cancelled',c.completed_at=UTC_TIMESTAMP(6) WHERE r.tenant_id=? AND r.work_package_id=? AND c.status IN ('queued','running','scoring')",
    )
    .bind(tenant_id)
    .bind(package_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE evaluation_runs SET status='cancelled',version=version+1,cancellation_version=cancellation_version+1,completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND work_package_id=? AND status IN ('created','queued','running')",
    )
    .bind(tenant_id)
    .bind(package_id)
    .execute(&mut *tx)
    .await?;
    crate::work_package_execution::publish_cancellation(&mut tx, tenant_id, package_id).await?;
    tx.commit().await?;
    Ok(Json(apply_receipt(
        package_id,
        version + 1,
        false,
        json!({"packageId":package_id,"status":"cancelled"}),
    )))
}

pub async fn apply_runtime_command(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<RuntimeCommandApplyRequestV1>,
) -> RuntimeResult<Json<ApplyReceiptV1>> {
    match &request.command {
        ExecutionCommandV1::SubmitInput { execution_id, .. }
        | ExecutionCommandV1::Cancel { execution_id, .. }
        | ExecutionCommandV1::SideEffectConfirmation { execution_id, .. } => {
            authorize_execution_command_or_publisher(&state, &headers, &request, *execution_id)
                .await?;
        }
        ExecutionCommandV1::Fork {
            source_execution_id,
            ..
        } => {
            authorize_execution_command_or_publisher(
                &state,
                &headers,
                &request,
                *source_execution_id,
            )
            .await?;
        }
        _ => {
            state.trust.publisher(&headers, "runtime.commands.apply")?;
        }
    }
    let result = match &request.command {
        ExecutionCommandV1::SubmitInput {
            execution_id,
            wait_id,
            content,
        } => {
            let belongs: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM input_waits WHERE tenant_id=? AND id=? AND (execution_id=? OR root_execution_id=?))")
                .bind(request.tenant_id).bind(wait_id).bind(execution_id).bind(execution_id).fetch_one(&state.pool).await?;
            if !belongs {
                return Err(RuntimeError::NotFound);
            }
            serde_json::to_value(
                crate::input_waits::submit(
                    &state.pool,
                    request.tenant_id,
                    *wait_id,
                    &request.idempotency_key,
                    content,
                    "platform",
                )
                .await?,
            )
            .map_err(|error| RuntimeError::Internal(error.into()))?
        }
        ExecutionCommandV1::WorkPackageCancel {
            package_id,
            expected_version,
        } => {
            let changed = sqlx::query(
                "UPDATE runtime_work_packages SET status='cancelled',version=version+1,cancellation_version=cancellation_version+1,cancel_idempotency_key=? WHERE tenant_id=? AND id=? AND version=? AND status IN ('prepared','running')",
            )
            .bind(&request.idempotency_key)
            .bind(request.tenant_id)
            .bind(package_id)
            .bind(expected_version)
            .execute(&state.pool)
            .await?;
            json!({"packageId":package_id,"changed":changed.rows_affected()==1})
        }
        ExecutionCommandV1::RetentionRun {
            run_id,
            dry_run,
            policy_version,
        } => {
            insert_retention_run(
                &state,
                request.tenant_id,
                *run_id,
                *policy_version,
                *dry_run,
                &request.idempotency_key,
            )
            .await?;
            json!({"runId":run_id})
        }
        command => {
            let aggregate_id = match command {
                ExecutionCommandV1::Cancel { execution_id, .. } => *execution_id,
                ExecutionCommandV1::Fork {
                    source_execution_id,
                    ..
                } => *source_execution_id,
                ExecutionCommandV1::SideEffectConfirmation { execution_id, .. } => *execution_id,
                _ => unreachable!(),
            };
            sqlx::query(
                "INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,?,'execution',?,?,?,'pending') ON DUPLICATE KEY UPDATE id=id",
            )
            .bind(request.command_id)
            .bind(request.tenant_id)
            .bind(match command {
                ExecutionCommandV1::Cancel { .. } => "cancel_execution",
                ExecutionCommandV1::Fork { .. } => "fork_execution",
                _ => "confirm_side_effect",
            })
            .bind(aggregate_id.to_string())
            .bind(&request.idempotency_key)
            .bind(serde_json::to_value(command).map_err(|error| RuntimeError::Internal(error.into()))?)
            .execute(&state.pool)
            .await?;
            json!({"commandId":request.command_id,"aggregateId":aggregate_id})
        }
    };
    Ok(Json(apply_receipt(
        request.command_id,
        request.object_version,
        false,
        result,
    )))
}

async fn authorize_execution_command_or_publisher(
    state: &RuntimeState,
    headers: &HeaderMap,
    request: &RuntimeCommandApplyRequestV1,
    execution_id: Uuid,
) -> RuntimeResult<()> {
    match crate::query_authority::authorize_execution_command(state, headers, request, execution_id)
        .await
    {
        Ok(()) => Ok(()),
        Err(RuntimeError::Unauthorized) => state
            .trust
            .publisher(headers, "runtime.commands.apply")
            .map(|_| ()),
        Err(error) => Err(error),
    }
}

pub async fn check_references(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<ReferenceCheckRequestV1>,
) -> RuntimeResult<Json<ReferenceCheckReceiptV1>> {
    state
        .trust
        .publisher(&headers, "runtime.references.check")?;
    let mut blocks = Vec::new();
    for bundle_id in &request.bundle_ids {
        let rows = sqlx::query(
            "SELECT reference_kind,owner_id FROM bundle_references WHERE tenant_id=? AND bundle_id=? AND released_at IS NULL AND (retained_until IS NULL OR retained_until>UTC_TIMESTAMP(6))",
        )
        .bind(request.tenant_id)
        .bind(bundle_id)
        .fetch_all(&state.pool)
        .await?;
        blocks.extend(
            rows.into_iter()
                .map(|row| {
                    Ok(RuntimeReferenceBlockV1 {
                        reference_kind: row.try_get("reference_kind")?,
                        owner_id: row.try_get("owner_id")?,
                        object_id: None,
                        bundle_id: Some(*bundle_id),
                    })
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()?,
        );
    }
    for object_id in &request.object_ids {
        let rows = sqlx::query(
            "SELECT work_package_id FROM runtime_work_package_objects o JOIN runtime_work_packages p ON p.id=o.work_package_id AND p.tenant_id=o.tenant_id WHERE o.tenant_id=? AND o.object_id=? AND p.status IN ('prepared','running')",
        )
        .bind(request.tenant_id)
        .bind(object_id)
        .fetch_all(&state.pool)
        .await?;
        blocks.extend(
            rows.into_iter()
                .map(|row| {
                    Ok(RuntimeReferenceBlockV1 {
                        reference_kind: "work_package".into(),
                        owner_id: row.try_get("work_package_id")?,
                        object_id: Some(*object_id),
                        bundle_id: None,
                    })
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()?,
        );
    }
    Ok(Json(ReferenceCheckReceiptV1 {
        api_version: 1,
        safe_to_delete: blocks.is_empty(),
        blocking_references: blocks,
    }))
}

pub async fn apply_retention_command(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<RetentionCommandRequestV1>,
) -> RuntimeResult<Json<ApplyReceiptV1>> {
    state.trust.publisher(&headers, "runtime.retention.apply")?;
    insert_retention_run(
        &state,
        request.tenant_id,
        request.run_id,
        request.policy_version,
        request.dry_run,
        &request.idempotency_key,
    )
    .await?;
    Ok(Json(apply_receipt(
        request.run_id,
        request.policy_version,
        false,
        json!({"runId":request.run_id,"status":"queued","dryRun":request.dry_run}),
    )))
}

async fn insert_retention_run(
    state: &RuntimeState,
    tenant_id: Uuid,
    run_id: Uuid,
    policy_version: u64,
    dry_run: bool,
    idempotency_key: &str,
) -> RuntimeResult<()> {
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query(
        "INSERT INTO retention_runs(id,tenant_id,dry_run,policy_version,idempotency_key,status,requested_by) VALUES(?,?,?,?,?,'queued',?) ON DUPLICATE KEY UPDATE id=id",
    )
    .bind(run_id)
    .bind(tenant_id)
    .bind(dry_run)
    .bind(policy_version)
    .bind(idempotency_key)
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() == 1 {
        crate::event_export::enqueue_governance_event(
            &mut tx,
            tenant_id,
            None,
            run_id,
            &agentx_runtime_contracts::RuntimeEventPayloadV1::RetentionChanged {
                run_id,
                run_version: policy_version,
                status: "queued".into(),
                marked_count: 0,
                deleted_count: 0,
                failed_count: 0,
                dry_run,
                items: Vec::new(),
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn persist_resource_bindings(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant_id: Uuid,
    package_id: Uuid,
    resources: &[RuntimeResourceBindingV1],
    composites: &std::collections::BTreeMap<Uuid, crate::composite::MaterializedComposite>,
) -> RuntimeResult<()> {
    for resource in resources {
        let kind = resource_kind_name(resource.resource_kind);
        let binding_id = stable_id(
            package_id,
            format!(
                "{kind}:{}:{}",
                resource.resource_id, resource.resource_version
            )
            .as_bytes(),
        );
        sqlx::query(
            "INSERT INTO runtime_resource_bindings(tenant_id,binding_id,bundle_id,work_package_id,resource_kind,resource_id,resource_version,state_epoch,content_hash,configuration_json,object_ids_json) VALUES(?,?,NULL,?,?,?,?,?,?,?,?)",
        )
        .bind(tenant_id)
        .bind(binding_id)
        .bind(package_id)
        .bind(kind)
        .bind(resource.resource_id)
        .bind(&resource.resource_version)
        .bind(resource.state_epoch)
        .bind(resource.content_hash.as_str())
        .bind(serde_json::to_value(&resource.configuration).map_err(|error| RuntimeError::Internal(error.into()))?)
        .bind(serde_json::to_value(&resource.object_ids).map_err(|error| RuntimeError::Internal(error.into()))?)
        .execute(&mut **tx)
        .await?;
        if let agentx_runtime_contracts::RuntimeResourceConfigurationV1::Composite {
            workflow, ..
        } = &resource.configuration
            && let Some(version_id) = workflow.version_id
            && let Some(snapshot) = composites.get(&version_id)
        {
            crate::composite::persist(tx, tenant_id, None, Some(package_id), binding_id, snapshot)
                .await?;
        }
        sqlx::query(
            "INSERT IGNORE INTO runtime_resource_states(tenant_id,resource_kind,resource_id,resource_version,state_epoch,status,content_hash) VALUES(?,?,?,?,?,'active',?)",
        )
        .bind(tenant_id)
        .bind(kind)
        .bind(resource.resource_id)
        .bind(&resource.resource_version)
        .bind(resource.state_epoch)
        .bind(resource.content_hash.as_str())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn package_receipt(package_id: Uuid, replayed: bool) -> PublishReceiptV1 {
    PublishReceiptV1 {
        api_version: 1,
        receipt: apply_receipt(package_id, 1, replayed, json!({"packageId":package_id})),
        bundle_id: package_id,
        head_version: None,
        activation_sequence: None,
        status: PublishReceiptStatusV1::Accepted,
        rejection: None,
        accepted_at: OffsetDateTime::now_utc(),
    }
}

fn apply_receipt(
    event_id: Uuid,
    object_version: u64,
    replayed: bool,
    result: Value,
) -> ApplyReceiptV1 {
    ApplyReceiptV1 {
        api_version: 1,
        event_id,
        applied: true,
        replayed,
        object_version,
        result,
    }
}

fn work_package_purpose(purpose: WorkPackagePurpose) -> &'static str {
    match purpose {
        WorkPackagePurpose::Debug => "debug",
        WorkPackagePurpose::Evaluation => "evaluation",
    }
}

fn runtime_call_purpose(purpose: RuntimeCallPurposeV1) -> &'static str {
    match purpose {
        RuntimeCallPurposeV1::Production => "production",
        RuntimeCallPurposeV1::Debug => "debug",
        RuntimeCallPurposeV1::Evaluation => "evaluation",
        RuntimeCallPurposeV1::Composite => "composite",
        RuntimeCallPurposeV1::Recovery => "recovery",
    }
}

fn resource_kind_name(kind: agentx_runtime_contracts::RuntimeResourceKindV1) -> &'static str {
    match kind {
        agentx_runtime_contracts::RuntimeResourceKindV1::Model => "model",
        agentx_runtime_contracts::RuntimeResourceKindV1::Mcp => "mcp",
        agentx_runtime_contracts::RuntimeResourceKindV1::Rag => "rag",
        agentx_runtime_contracts::RuntimeResourceKindV1::Memory => "memory",
        agentx_runtime_contracts::RuntimeResourceKindV1::Skill => "skill",
        agentx_runtime_contracts::RuntimeResourceKindV1::Credential => "credential",
        agentx_runtime_contracts::RuntimeResourceKindV1::SandboxProfile => "sandbox_profile",
        agentx_runtime_contracts::RuntimeResourceKindV1::Composite => "composite",
    }
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
