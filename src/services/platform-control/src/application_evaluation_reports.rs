use super::*;

pub(crate) async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
) -> ApiResult<Json<Vec<Value>>> {
    authorize(&state, &actor, application, "evaluation:view").await?;
    let rows=sqlx::query("SELECT id FROM evaluation_batches WHERE tenant_id=? AND application_id=? ORDER BY created_at DESC,id DESC").bind(actor.tenant_id).bind(application).fetch_all(&state.pool).await?;
    let mut items = Vec::new();
    for row in rows {
        items.push(report(&state, &actor, application, row.try_get("id")?, false).await?)
    }
    Ok(Json(items))
}
pub(crate) async fn get(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, batch)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    authorize(&state, &actor, application, "evaluation:view").await?;
    Ok(Json(
        report(&state, &actor, application, batch, true).await?,
    ))
}
pub(crate) async fn report(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    batch: Uuid,
    details: bool,
) -> ApiResult<Value> {
    let row=sqlx::query("SELECT b.*,v.criteria,CAST(v.pass_threshold AS DOUBLE) pass_threshold,p.name profile_name FROM evaluation_batches b JOIN evaluation_profile_versions v ON v.tenant_id=b.tenant_id AND v.id=b.profile_version_id JOIN evaluation_profiles p ON p.tenant_id=v.tenant_id AND p.id=v.profile_id WHERE b.tenant_id=? AND b.application_id=? AND b.id=?").bind(actor.tenant_id).bind(application).bind(batch).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Evaluation Batch"))?;
    let model: agentx_runtime_contracts::RuntimePlatformEvaluationModelV1 =
        serde_json::from_value(row.try_get("model_snapshot_json")?).map_err(ApiError::internal)?;
    let children=sqlx::query("SELECT r.*,wv.version_number,dv.dataset_id,dv.source_revision,dv.case_count,d.name dataset_name,d.contract_fingerprint FROM evaluation_runs r JOIN workflow_versions wv ON wv.tenant_id=r.tenant_id AND wv.id=r.workflow_version_id JOIN dataset_versions dv ON dv.tenant_id=r.tenant_id AND dv.id=r.dataset_version_id JOIN datasets d ON d.tenant_id=dv.tenant_id AND d.id=dv.dataset_id WHERE r.tenant_id=? AND r.batch_id=? ORDER BY d.id,wv.version_number").bind(actor.tenant_id).bind(batch).fetch_all(&state.pool).await?;
    let last_completed = children
        .iter()
        .filter_map(|row| {
            row.try_get::<Option<time::OffsetDateTime>, _>("completed_at")
                .ok()
                .flatten()
        })
        .max();
    let view = crate::governance_api::projection(&state.pool).await?;
    let mut runs = Vec::new();
    let mut statuses = Vec::new();
    for child in children {
        let run: Uuid = child.try_get("id")?;
        let status: String = child.try_get("status")?;
        statuses.push(status.clone());
        let results=sqlx::query("SELECT c.*,dc.name,dc.case_key,dc.lineage_id,dc.input_json,dc.expected_output_json,rr.status score_status,rr.passed,CAST(rr.score AS DOUBLE) score,rr.detail_json,rr.evaluator_execution_id,rr.cost_by_currency_json judge_cost_by_currency_json,rr.cost_micros judge_cost_micros,rr.duration_ms judge_duration_ms FROM evaluation_case_projection c JOIN evaluation_runs r ON r.tenant_id=c.tenant_id AND r.id=c.evaluation_run_id JOIN dataset_version_cases dc ON dc.tenant_id=c.tenant_id AND dc.dataset_version_id=r.dataset_version_id AND dc.source_case_id=c.source_case_id LEFT JOIN evaluation_rule_results rr ON rr.tenant_id=c.tenant_id AND rr.evaluation_run_case_id=c.id AND rr.projection_generation=c.projection_generation WHERE c.tenant_id=? AND c.evaluation_run_id=? AND c.projection_generation=? ORDER BY dc.sort_order,dc.source_case_id").bind(actor.tenant_id).bind(run).bind(view.generation).fetch_all(&state.pool).await?;
        let mut cases = Vec::new();
        let (
            mut passed,
            mut failed,
            mut errors,
            mut scored,
            mut score_sum,
            mut business_cost,
            mut judge_cost,
        ) = (0u64, 0u64, 0u64, 0u64, 0.0, 0u64, 0u64);
        let (mut business_by_currency, mut judge_by_currency) = (
            std::collections::BTreeMap::<String, u64>::new(),
            std::collections::BTreeMap::<String, u64>::new(),
        );
        for case in results {
            let score_status: Option<String> = case.try_get("score_status")?;
            let case_status: String = case.try_get("status")?;
            let score: Option<f64> = case.try_get("score")?;
            let bad = matches!(score_status.as_deref(), Some("error")) || case_status == "failed";
            if bad {
                errors += 1
            } else if let Some(value) = score {
                scored += 1;
                score_sum += value;
                if case.try_get::<Option<bool>, _>("passed")? == Some(true) {
                    passed += 1
                } else {
                    failed += 1
                }
            }
            let jc = case
                .try_get::<Option<u64>, _>("judge_cost_micros")?
                .unwrap_or(0);
            let total = case.try_get::<u64, _>("cost_micros")?;
            let bc = total.saturating_sub(
                if matches!(case_status.as_str(), "completed" | "failed" | "cancelled") {
                    jc
                } else {
                    0
                },
            );
            business_cost += bc;
            judge_cost += jc;
            let bc_currency: std::collections::BTreeMap<String, u64> = serde_json::from_value(
                case.try_get::<Option<Value>, _>("cost_by_currency_json")?
                    .unwrap_or_else(|| json!({})),
            )
            .map_err(ApiError::internal)?;
            let jc_currency: std::collections::BTreeMap<String, u64> = serde_json::from_value(
                case.try_get::<Option<Value>, _>("judge_cost_by_currency_json")?
                    .unwrap_or_else(|| json!({})),
            )
            .map_err(ApiError::internal)?;
            for (currency, cost) in &bc_currency {
                *business_by_currency.entry(currency.clone()).or_default() += cost;
            }
            for (currency, cost) in &jc_currency {
                *judge_by_currency.entry(currency.clone()).or_default() += cost;
            }
            if details {
                let detail = case
                    .try_get::<Option<Value>, _>("detail_json")?
                    .unwrap_or(Value::Null);
                let reason = detail
                    .pointer("/modelResult/reason")
                    .and_then(Value::as_str);
                cases.push(json!({"id":case.try_get::<Uuid,_>("source_case_id")?,"name":case.try_get::<String,_>("name")?,"lineageId":case.try_get::<Uuid,_>("lineage_id")?,"input":case.try_get::<Value,_>("input_json")?,"expectedOutput":case.try_get::<Option<Value>,_>("expected_output_json")?,"actualOutput":case.try_get::<Option<Value>,_>("actual_output_json")?,"inputWaits":case.try_get::<Value,_>("input_waits_json")?,"messages":case.try_get::<Value,_>("messages_json")?,"status":if bad{"error"}else if score.is_some(){if case.try_get::<Option<bool>,_>("passed")?==Some(true){"passed"}else{"failed"}}else{case_status.as_str()},"score":score,"reason":reason,"error":detail.get("error"),"errorCode":case.try_get::<Option<String>,_>("error_code")?,"errorMessage":case.try_get::<Option<String>,_>("error_message")?.or_else(||detail.pointer("/error/message").and_then(Value::as_str).map(str::to_owned)),"executionId":case.try_get::<Option<Uuid>,_>("target_execution_id")?,"judgeExecutionId":case.try_get::<Option<Uuid>,_>("evaluator_execution_id")?,"durationMs":case.try_get::<Option<u64>,_>("duration_ms")?,"judgeDurationMs":case.try_get::<Option<u64>,_>("judge_duration_ms")?,"businessCostMicros":bc,"judgeCostMicros":jc,"businessCostByCurrency":bc_currency,"judgeCostByCurrency":jc_currency}));
            }
        }
        if child
            .try_get::<Option<Value>, _>("dispatch_error_json")?
            .is_some()
        {
            errors += 1;
        }
        runs.push(json!({"id":run,"datasetId":child.try_get::<Uuid,_>("dataset_id")?,"datasetName":child.try_get::<String,_>("dataset_name")?,"contractFingerprint":child.try_get::<String,_>("contract_fingerprint")?,"datasetVersionId":child.try_get::<Uuid,_>("dataset_version_id")?,"sourceRevision":child.try_get::<u64,_>("source_revision")?,"workflowVersionId":child.try_get::<Uuid,_>("workflow_version_id")?,"workflowVersionNumber":child.try_get::<u64,_>("version_number")?,"status":status,"totalCases":child.try_get::<u64,_>("case_count")?,"passedCount":passed,"failedCount":failed,"errorCount":errors,"scoredCount":scored,"averageScore":if scored>0{Some(score_sum/scored as f64)}else{None},"passRate":if scored>0{Some(passed as f64/scored as f64)}else{None},"businessCostMicros":business_cost,"judgeCostMicros":judge_cost,"businessCostByCurrency":business_by_currency,"judgeCostByCurrency":judge_by_currency,"dispatchError":child.try_get::<Option<Value>,_>("dispatch_error_json")?,"cases":cases}));
    }
    let status = if row.try_get::<String, _>("status")? == "cancelled" {
        "cancelled"
    } else if statuses
        .iter()
        .all(|s| matches!(s.as_str(), "completed" | "failed" | "cancelled"))
    {
        if statuses.iter().any(|s| s == "failed")
            || runs
                .iter()
                .any(|r| r["errorCount"].as_u64().unwrap_or(0) > 0)
        {
            "failed"
        } else {
            "completed"
        }
    } else if statuses.iter().all(|s| s == "created") {
        "queued"
    } else {
        "running"
    };

    let created: time::OffsetDateTime = row.try_get("created_at")?;
    Ok(
        json!({"id":batch,"applicationId":application,"name":row.try_get::<String,_>("name")?,"status":status,"profileName":row.try_get::<String,_>("profile_name")?,"profileVersionId":row.try_get::<Uuid,_>("profile_version_id")?,"criteria":row.try_get::<String,_>("criteria")?,"passThreshold":row.try_get::<f64,_>("pass_threshold")?,"modelId":model.binding.resource_id,"modelName":match &model.binding.configuration{agentx_runtime_contracts::RuntimeResourceConfigurationV1::Model{model,..}=>model.as_str(),_=>""},"modelVersion":model.binding.resource_version,"modelParameters":model.parameters,"durationMs":((if statuses.iter().all(|status|matches!(status.as_str(),"completed"|"failed"|"cancelled")){last_completed.unwrap_or(created)}else{time::OffsetDateTime::now_utc()})-created).whole_milliseconds().max(0) as u64,"createdAt":created.format(&time::format_description::well_known::Rfc3339).map_err(ApiError::internal)?,"runs":runs}),
    )
}
