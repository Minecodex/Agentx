use super::*;

async fn build_evaluation(
    state: &ControlApiState,
    actor: &Actor,
    run_id: Uuid,
) -> ApiResult<(
    agentx_runtime_contracts::RuntimeWorkPackageV1,
    Vec<PendingObject>,
)> {
    let row = sqlx::query("SELECT r.workflow_version_id,r.dataset_version_id,r.evaluation_profile_version_id,r.parameters_json,r.status,b.model_snapshot_json,b.system_prompt,b.origin_snapshot_json,b.created_at batch_created_at, pv.criteria,CAST(pv.pass_threshold AS DOUBLE) pass_threshold,v.workflow_id,v.version_number workflow_version_number,v.definition_json,i.id service_identity_id,i.version identity_version FROM evaluation_runs r JOIN evaluation_batches b ON b.tenant_id=r.tenant_id AND b.id=r.batch_id JOIN evaluation_profile_versions pv ON pv.tenant_id=r.tenant_id AND pv.id=r.evaluation_profile_version_id JOIN workflow_versions v ON v.tenant_id=r.tenant_id AND v.id=r.workflow_version_id JOIN workflow_service_identities i ON i.tenant_id=v.tenant_id AND i.workflow_id=v.workflow_id WHERE r.tenant_id=? AND r.id=? AND v.publication_status='published'")
        .bind(actor.tenant_id).bind(run_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Evaluation Run"))?;
    if row.try_get::<String, _>("status")? != "created" {
        return Err(ApiError::conflict(
            "EVALUATION_ALREADY_STARTED",
            "Evaluation Run has already been started",
        ));
    }
    let workflow_id: Uuid = row.try_get("workflow_id")?;
    let workflow_version_id: Uuid = row.try_get("workflow_version_id")?;
    let dataset_version_id: Uuid = row.try_get("dataset_version_id")?;
    let profile_version_id: Uuid = row.try_get("evaluation_profile_version_id")?;
    let case_rows=sqlx::query("SELECT source_case_id,input_json,expected_output_json,wait_inputs_json FROM dataset_version_cases WHERE tenant_id=? AND dataset_version_id=? ORDER BY sort_order,source_case_id")
        .bind(actor.tenant_id).bind(dataset_version_id).fetch_all(&state.pool).await?;
    if case_rows.is_empty() {
        return Err(ApiError::unprocessable(
            "EVALUATION_DATASET_EMPTY",
            "Dataset Version has no Cases",
        ));
    }
    let cases = case_rows
        .into_iter()
        .map(|case| {
            Ok(RuntimeEvaluationCaseV1 {
                wait_inputs: serde_json::from_value(case.try_get("wait_inputs_json")?)
                    .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                case_id: case.try_get("source_case_id")?,
                input: case.try_get("input_json")?,
                expected_output: case.try_get("expected_output_json")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let service_identity_id: Uuid = row.try_get("service_identity_id")?;
    let definition: WorkflowDefinition = serde_json::from_value(row.try_get("definition_json")?)
        .map_err(|error| {
            ApiError::unprocessable("INVALID_WORKFLOW_DEFINITION", error.to_string())
        })?;
    let (dependencies, mut resources, mut pending_objects) =
        load_published_version_closure(state, actor.tenant_id, workflow_version_id, &definition)
            .await?;
    crate::canvas_plugin_api::require_definition_plugin_closure_enabled(
        state,
        actor.tenant_id,
        &definition,
        &dependencies,
    )
    .await?;
    let model: agentx_runtime_contracts::RuntimePlatformEvaluationModelV1 =
        serde_json::from_value(row.try_get("model_snapshot_json")?).map_err(ApiError::internal)?;
    let evaluator_id = profile_version_id;
    let prompt_object_id =
        agentx_runtime_contracts::deterministic_uuid(model.batch_id, b"judge-prompt");
    let prompt = json!({"prompt":row.try_get::<String,_>("system_prompt")?,"criteria":row.try_get::<String,_>("criteria")?});
    push_pending_object(
        actor.tenant_id,
        prompt_object_id,
        "application/json",
        serde_json::to_vec(&prompt).map_err(ApiError::internal)?,
        &mut pending_objects,
    )?;
    let evaluators = vec![RuntimeEvaluatorV1::Model {
        evaluator_id,
        resource_id: model.binding.resource_id,
        prompt_object_id,
        model,
        pass_threshold: row.try_get("pass_threshold")?,
    }];
    let (grant_ids, grant_bindings) = crate::runtime_resource_binding::authorization_grants(
        &state.pool,
        actor.tenant_id,
        service_identity_id,
    )
    .await?;
    let package_id = run_id;
    let created_at: OffsetDateTime = row.try_get("batch_created_at")?;
    let expires_at = created_at + time::Duration::hours(24);
    let plugin_manifests =
        crate::canvas_plugin_api::plugin_manifests_for_tenant(state, actor.tenant_id).await?;
    let plugin_registry =
        node_registry_with_plugins(&dependencies, &plugin_manifests).map_err(ApiError::internal)?;
    let resolved = crate::canvas_plugin_api::resolve_workflow_plugin_closure(
        state,
        actor.tenant_id,
        &definition,
        &dependencies,
        &plugin_registry,
    )
    .await?;
    require_complete_plugin_definitions(&resolved)?;
    add_composite_closure(
        &state.pool,
        actor.tenant_id,
        &dependencies,
        &plugin_manifests,
        &resolved.dependency_manifests,
        &mut resources,
        &mut pending_objects,
    )
    .await?;
    add_plugin_runtime_objects(actor.tenant_id, &resolved, &mut pending_objects)?;
    let package = build_work_package_with_resolved(
        WorkPackageBuildSource {
            package_id,
            tenant_id: actor.tenant_id,
            workflow: workflow_snapshot(
                &state.pool,
                actor.tenant_id,
                workflow_id,
                row.try_get("workflow_version_id")?,
                row.try_get("workflow_version_number")?,
            )
            .await?,
            origin: serde_json::from_value(row.try_get("origin_snapshot_json")?)
                .map_err(ApiError::internal)?,
            purpose: WorkPackagePurpose::Evaluation,
            call_purpose: RuntimeCallPurposeV1::Evaluation,
            spec: RuntimeWorkPackageSpecV1::Evaluation {
                dataset_version_id,
                profile_version_id,
                cases,
                evaluators,
            },
            source_revision: format!(
                "evaluation:{run_id}:{}",
                row.try_get::<Uuid, _>("workflow_version_id")?
            ),
            definition,
            dependency_versions: dependencies,
            plugin_manifests,
            supported_capabilities: agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
                .iter()
                .map(ToString::to_string)
                .collect(),
            overlay: RuntimeWorkPackageOverlayV1 {
                input: row.try_get("parameters_json")?,
                ..Default::default()
            },
            resources,
            authorization: RuntimeAuthorizationSnapshotV1 {
                schema_version: 1,
                tenant_id: actor.tenant_id,
                service_identity_id,
                workflow_id,
                policy_epoch: row.try_get("identity_version")?,
                grant_ids,
                grant_bindings,
                capabilities: agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                maximum_policy_staleness_seconds: 72 * 60 * 60,
                captured_at: created_at,
            },
            objects: pending_objects
                .iter()
                .map(|object| object.reference.clone())
                .collect(),
            runtime_policy: RuntimePolicyV1::default(),
            created_at,
            expires_at,
        },
        &resolved.manifests,
        &resolved.dependency_manifests,
        &state.work_packages.signing_kid,
        &state.work_packages.signing_key,
    )
    .map_err(|error| ApiError::unprocessable("WORK_PACKAGE_BUILD_FAILED", error.to_string()))?;
    Ok((package, pending_objects))
}

pub(crate) async fn start_evaluation(
    state: &ControlApiState,
    actor: &Actor,
    run_id: Uuid,
) -> ApiResult<()> {
    let saved=sqlx::query("SELECT package_json,pending_objects_json FROM runtime_work_package_publications WHERE tenant_id=? AND package_id=?").bind(actor.tenant_id).bind(run_id).fetch_optional(&state.pool).await?;
    let (package, pending_objects) = if let Some(row) = saved {
        (
            serde_json::from_value::<agentx_runtime_contracts::RuntimeWorkPackageV1>(
                row.try_get("package_json")?,
            )
            .map_err(ApiError::internal)?,
            serde_json::from_value::<Vec<PendingObject>>(row.try_get("pending_objects_json")?)
                .map_err(ApiError::internal)?,
        )
    } else {
        let (package, pending_objects) = build_evaluation(state, actor, run_id).await?;
        let package_id = run_id;
        let request_hash =
            agentx_runtime_contracts::content_hash(&package).map_err(ApiError::internal)?;
        let mut tx = state.pool.begin().await?;
        let status: String = sqlx::query_scalar(
            "SELECT status FROM evaluation_runs WHERE tenant_id=? AND id=? FOR UPDATE",
        )
        .bind(actor.tenant_id)
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
        if status == "cancelled" {
            return Ok(());
        }
        let inserted=sqlx::query("INSERT INTO runtime_work_package_publications(id,tenant_id,package_id,purpose,source_type,source_id,source_revision,content_hash,signature_key_id,package_json,object_manifest_json,pending_objects_json,status,prepare_idempotency_key,request_hash,expires_at,created_by) VALUES(?,?,?,'evaluation','evaluation_run',?,?,?,?,?,?,?,'building',?,?,?,?) ON DUPLICATE KEY UPDATE id=id")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(package_id).bind(run_id).bind(&package.payload.source_revision).bind(package.content_hash.as_str()).bind(&package.signature.key_id).bind(serde_json::to_value(&package).map_err(ApiError::internal)?).bind(serde_json::to_value(&package.payload.objects).map_err(ApiError::internal)?).bind(serde_json::to_value(&pending_objects).map_err(ApiError::internal)?).bind(format!("evaluation:prepare:{run_id}")).bind(request_hash.as_str()).bind(package.payload.expires_at).bind(actor.user_id).execute(&mut *tx).await?;
        if inserted.rows_affected() == 0 {
            tx.rollback().await?;
            return Box::pin(start_evaluation(state, actor, run_id)).await;
        }
        sqlx::query("UPDATE evaluation_runs SET work_package_id=?,status='queued',started_at=UTC_TIMESTAMP(6),total_cases=? WHERE tenant_id=? AND id=? AND status='created'").bind(package_id).bind(package.payload.spec.evaluation_case_count()).bind(actor.tenant_id).bind(run_id).execute(&mut *tx).await?;
        tx.commit().await?;
        (package, pending_objects)
    };
    let package_id = run_id;
    for object in pending_objects {
        let receipt = state
            .work_packages
            .upload_object(&object.metadata, object.content)
            .await;
        match receipt {
            Ok(receipt)
                if receipt.object.object_id == object.reference.object_id
                    && receipt.object.content_hash == object.reference.content_hash => {}
            Ok(_) => {
                return Err(ApiError::conflict(
                    "RUNTIME_OBJECT_RECEIPT_MISMATCH",
                    "Runtime returned a different immutable evaluator object",
                ));
            }
            Err(error) => {
                return Err(error);
            }
        }
    }
    let prepare: agentx_runtime_contracts::PublishReceiptV1 = state
        .work_packages
        .post(
            "runtime.work-packages.prepare",
            "/internal/runtime/v1/work-packages:prepare",
            &PrepareWorkPackageRequestV1 {
                api_version: 1,
                idempotency_key: format!("evaluation:prepare:{run_id}"),
                work_package: package,
            },
        )
        .await?;
    if prepare.status != PublishReceiptStatusV1::Accepted {
        return Err(ApiError::conflict(
            "WORK_PACKAGE_PREPARE_REJECTED",
            "Runtime rejected the Evaluation Work Package",
        ));
    }
    let execute: agentx_runtime_contracts::ApplyReceiptV1 = state
        .work_packages
        .post(
            "runtime.work-packages.execute",
            &format!("/internal/runtime/v1/work-packages/{package_id}:execute"),
            &ExecuteWorkPackageRequestV1 {
                api_version: 1,
                idempotency_key: format!("evaluation:execute:{run_id}"),
                package_id,
                input: Value::Null,
            },
        )
        .await?;
    sqlx::query("UPDATE runtime_work_package_publications SET status='running',runtime_version=?,prepare_receipt_json=? WHERE tenant_id=? AND package_id=?").bind(execute.object_version).bind(serde_json::to_value(&prepare).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(package_id).execute(&state.pool).await?;
    sqlx::query("UPDATE evaluation_runs SET runtime_command_id=?,runtime_receipt_json=?,status='running' WHERE tenant_id=? AND id=? AND status IN ('created','queued','running')").bind(execute.event_id).bind(serde_json::to_value(execute).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(run_id).execute(&state.pool).await?;
    Ok(())
}

pub(crate) async fn cancel_evaluation(
    state: &ControlApiState,
    actor: &Actor,
    run_id: Uuid,
) -> ApiResult<()> {
    let row=sqlx::query("SELECT work_package_id,intent_version,status FROM evaluation_runs WHERE tenant_id=? AND id=?").bind(actor.tenant_id).bind(run_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Evaluation Run"))?;
    let package_id: Uuid = row
        .try_get::<Option<Uuid>, _>("work_package_id")?
        .ok_or_else(|| {
            ApiError::conflict(
                "EVALUATION_NOT_STARTED",
                "Evaluation has no Runtime Work Package",
            )
        })?;
    if matches!(
        row.try_get::<String, _>("status")?.as_str(),
        "completed" | "failed" | "cancelled"
    ) {
        return Ok(());
    }
    let expected:u64=sqlx::query_scalar("SELECT runtime_version FROM runtime_work_package_publications WHERE tenant_id=? AND package_id=?").bind(actor.tenant_id).bind(package_id).fetch_one(&state.pool).await?;
    if expected == 0 {
        return Err(ApiError::unavailable(
            "RUNTIME_UNAVAILABLE",
            "Waiting for Work Package preparation before cancellation",
        ));
    }
    let receipt: agentx_runtime_contracts::ApplyReceiptV1 = state
        .work_packages
        .post(
            "runtime.work-packages.cancel",
            &format!("/internal/runtime/v1/work-packages/{package_id}:cancel"),
            &CancelWorkPackageRequestV1 {
                api_version: 1,
                tenant_id: actor.tenant_id,
                package_id,
                expected_version: expected,
                idempotency_key: format!("evaluation:cancel:{run_id}"),
            },
        )
        .await?;
    let terminal = match receipt.result.get("status").and_then(Value::as_str) {
        Some("succeeded" | "completed") => "completed",
        Some("failed") => "failed",
        _ => "cancelled",
    };
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE runtime_work_package_publications SET status=?,runtime_version=?,cancel_idempotency_key=?,cancel_receipt_json=? WHERE tenant_id=? AND package_id=?").bind(if terminal=="completed" {"completed"} else {terminal}).bind(receipt.object_version).bind(format!("evaluation:cancel:{run_id}")).bind(serde_json::to_value(&receipt).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(package_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE evaluation_runs SET status=?,completed_at=UTC_TIMESTAMP(6),intent_version=intent_version+1,runtime_receipt_json=? WHERE tenant_id=? AND id=?").bind(terminal).bind(serde_json::to_value(receipt).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(run_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

trait EvaluationSpecCount {
    fn evaluation_case_count(&self) -> u64;
}
impl EvaluationSpecCount for RuntimeWorkPackageSpecV1 {
    fn evaluation_case_count(&self) -> u64 {
        match self {
            RuntimeWorkPackageSpecV1::Evaluation { cases, .. } => cases.len() as u64,
            RuntimeWorkPackageSpecV1::Debug { .. } => 0,
        }
    }
}
