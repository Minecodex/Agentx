use agentx_runtime_contracts::{
    RuntimeEvaluationCaseResultV1, RuntimeEvaluationMetricsV1, RuntimeEvaluationReportV1,
    RuntimeEvaluationRuleResultV1, RuntimeEvaluatorV1, RuntimeEventPayloadV1,
    RuntimeWorkPackagePayloadV1, RuntimeWorkPackageSpecV1,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

pub struct StartedWorkPackage {
    pub result: Value,
}

pub async fn start(
    tx: &mut Transaction<'_, MySql>,
    package: &RuntimeWorkPackagePayloadV1,
    request_input: &Value,
) -> RuntimeResult<StartedWorkPackage> {
    match &package.spec {
        RuntimeWorkPackageSpecV1::Debug { .. } => {
            let execution_id = Uuid::now_v7();
            let command_id = Uuid::now_v7();
            insert_execution(
                tx,
                package,
                execution_id,
                command_id,
                request_input,
                "debug",
            )
            .await?;
            Ok(StartedWorkPackage {
                result: json!({
                    "executionId":execution_id,
                    "commandId":command_id,
                    "packageId":package.package_id
                }),
            })
        }
        RuntimeWorkPackageSpecV1::Evaluation {
            dataset_version_id,
            profile_version_id,
            cases,
            evaluators,
        } => {
            let run_id = stable_id(package.package_id, b"evaluation-run");
            sqlx::query(
                "INSERT INTO evaluation_runs(id,work_package_id,bundle_id,tenant_id,name,workflow_version_id,dataset_version_id,evaluation_profile_version_id,parameters_json,version,status,created_by,owner_department_id,visibility,started_at) VALUES(?,?,NULL,?,'Runtime Evaluation',?,?,?, ?,1,'running',?,?,'private',UTC_TIMESTAMP(6))",
            )
            .bind(run_id)
            .bind(package.package_id)
            .bind(package.tenant_id)
            .bind(package.package_id)
            .bind(dataset_version_id)
            .bind(profile_version_id)
            .bind(json!({"packageId":package.package_id,"evaluatorCount":evaluators.len()}))
            .bind(package.authorization.service_identity_id)
            .bind(package.authorization.service_identity_id)
            .execute(&mut **tx)
            .await?;
            let mut execution_ids = Vec::with_capacity(cases.len());
            for case in cases {
                let execution_id = stable_id(package.package_id, case.case_id.as_bytes());
                let command_id = stable_id(execution_id, b"target-command");
                let run_case_id = stable_id(execution_id, b"evaluation-case");
                insert_execution(
                    tx,
                    package,
                    execution_id,
                    command_id,
                    &case.input,
                    "evaluation",
                )
                .await?;
                sqlx::query(
                    "INSERT INTO evaluation_run_cases(id,tenant_id,evaluation_run_id,source_case_id,target_command_id,target_execution_id,status) VALUES(?,?,?,?,?,?,'running')",
                )
                .bind(run_case_id)
                .bind(package.tenant_id)
                .bind(run_id)
                .bind(case.case_id)
                .bind(command_id)
                .bind(execution_id)
                .execute(&mut **tx)
                .await?;
                for evaluator in evaluators {
                    let (evaluator_id, detail) = match evaluator {
                        RuntimeEvaluatorV1::DeterministicRule {
                            evaluator_id,
                            expression,
                        } => (
                            *evaluator_id,
                            json!({"kind":"deterministic_rule","expression":expression,"expectedOutput":case.expected_output}),
                        ),
                        RuntimeEvaluatorV1::Model {
                            evaluator_id,
                            resource_id,
                            prompt_object_id,
                        } => (
                            *evaluator_id,
                            json!({"kind":"model","resourceId":resource_id,"promptObjectId":prompt_object_id,"expectedOutput":case.expected_output}),
                        ),
                    };
                    sqlx::query(
                        "INSERT INTO evaluation_rule_results(id,tenant_id,evaluation_run_case_id,profile_rule_id,status,detail_json) VALUES(?,?,?,?,'queued',?)",
                    )
                    .bind(stable_id(run_case_id, evaluator_id.as_bytes()))
                    .bind(package.tenant_id)
                    .bind(run_case_id)
                    .bind(evaluator_id)
                    .bind(detail)
                    .execute(&mut **tx)
                    .await?;
                }
                execution_ids.push(execution_id);
            }
            execution_ids.first().ok_or_else(|| {
                RuntimeError::Internal(anyhow::anyhow!("Evaluation contains no Case"))
            })?;
            Ok(StartedWorkPackage {
                result: json!({
                    "evaluationRunId":run_id,
                    "executionIds":execution_ids,
                    "packageId":package.package_id
                }),
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn converge_execution(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    work_package_id: Option<Uuid>,
    terminal_status: &str,
    output: &Value,
    error: &Option<Value>,
    result_hash: &str,
) -> RuntimeResult<()> {
    let is_composite_child: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM execution_children WHERE tenant_id=? AND child_execution_id=? AND relationship='composite')",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .fetch_one(&mut **tx)
    .await?;
    if is_composite_child {
        return Ok(());
    }
    let Some(work_package_id) = work_package_id else {
        return Ok(());
    };
    let Some(package) = sqlx::query(
        "SELECT id,purpose,status,version,expires_at FROM runtime_work_packages WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(work_package_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(());
    };
    if package.try_get::<String, _>("status")? != "running" {
        return Ok(());
    }
    let package_id: Uuid = package.try_get("id")?;
    match package.try_get::<String, _>("purpose")?.as_str() {
        "debug" => {
            let package_status = package_terminal_status(terminal_status);
            sqlx::query(
                "UPDATE runtime_work_packages SET status=?,version=version+1,result_json=?,result_hash=?,error_code=JSON_UNQUOTE(JSON_EXTRACT(?,'$.code')),error_message=JSON_UNQUOTE(JSON_EXTRACT(?,'$.message')),completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND status='running'",
            )
            .bind(package_status)
            .bind(output)
            .bind(result_hash)
            .bind(error)
            .bind(error)
            .bind(tenant_id)
            .bind(package_id)
            .execute(&mut **tx)
            .await?;
            let event = RuntimeEventPayloadV1::DebugChanged {
                package_id,
                package_version: package.try_get::<u64, _>("version")? + 1,
                work_package_id: package_id,
                status: package_status.into(),
                result: Some(output.clone()),
                expires_at: package.try_get("expires_at")?,
            };
            insert_governance_event(tx, tenant_id, execution_id, &event).await?;
        }
        "evaluation" => {
            converge_evaluation(
                tx,
                tenant_id,
                package_id,
                execution_id,
                terminal_status,
                output,
                error,
            )
            .await?;
        }
        purpose => {
            return Err(RuntimeError::Internal(anyhow::anyhow!(
                "unsupported Work Package purpose {purpose}"
            )));
        }
    }
    Ok(())
}

async fn converge_evaluation(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    package_id: Uuid,
    execution_id: Uuid,
    terminal_status: &str,
    output: &Value,
    error: &Option<Value>,
) -> RuntimeResult<()> {
    let Some(case) = sqlx::query(
        "SELECT c.id,c.evaluation_run_id,c.status FROM evaluation_run_cases c FORCE INDEX(idx_evaluation_case_target) JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id WHERE c.tenant_id=? AND c.target_execution_id=? AND r.work_package_id=? FOR UPDATE OF c",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .bind(package_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return converge_model_evaluator(
            tx,
            tenant_id,
            package_id,
            execution_id,
            terminal_status,
            output,
            error,
        )
        .await;
    };
    let case_id: Uuid = case.try_get("id")?;
    let run_id: Uuid = case.try_get("evaluation_run_id")?;
    if matches!(
        case.try_get::<String, _>("status")?.as_str(),
        "completed" | "failed" | "cancelled"
    ) {
        return Ok(());
    }

    let case_status = if terminal_status == "completed" {
        evaluate_deterministic_rules(tx, tenant_id, case_id, output).await?;
        start_model_evaluators(tx, tenant_id, package_id, execution_id, case_id, output).await?;
        evaluation_scoring_status(tx, tenant_id, case_id).await?
    } else if terminal_status == "cancelled" {
        "cancelled"
    } else {
        "failed"
    };
    let (duration_ms, cost_micros) = execution_measurements(tx, tenant_id, execution_id).await?;
    sqlx::query(
        "UPDATE evaluation_run_cases SET status=?,actual_output_json=?,duration_ms=?,cost_micros=?,error_code=JSON_UNQUOTE(JSON_EXTRACT(?,'$.code')),error_message=JSON_UNQUOTE(JSON_EXTRACT(?,'$.message')),completed_at=IF(? IN ('completed','failed','cancelled'),UTC_TIMESTAMP(6),NULL) WHERE tenant_id=? AND id=?",
    )
    .bind(case_status)
    .bind(output)
    .bind(duration_ms)
    .bind(cost_micros)
    .bind(error)
    .bind(error)
    .bind(case_status)
    .bind(tenant_id)
    .bind(case_id)
    .execute(&mut **tx)
    .await?;
    if case_status != "completed" && case_status != "scoring" {
        sqlx::query(
            "UPDATE evaluation_rule_results SET status='cancelled',completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND evaluation_run_case_id=? AND status IN ('queued','running')",
        )
        .bind(tenant_id)
        .bind(case_id)
        .execute(&mut **tx)
        .await?;
    }
    converge_evaluation_run(tx, tenant_id, package_id, run_id, execution_id).await
}

async fn start_model_evaluators(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    package_id: Uuid,
    target_execution_id: Uuid,
    case_id: Uuid,
    actual_output: &Value,
) -> RuntimeResult<()> {
    let rules = sqlx::query(
        "SELECT id,profile_rule_id,detail_json FROM evaluation_rule_results WHERE tenant_id=? AND evaluation_run_case_id=? AND status='queued' FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(case_id)
    .fetch_all(&mut **tx)
    .await?;
    if rules.is_empty() {
        return Ok(());
    }
    let payload: RuntimeWorkPackagePayloadV1 = serde_json::from_value(
        sqlx::query_scalar(
            "SELECT payload_json FROM runtime_work_packages WHERE tenant_id=? AND id=?",
        )
        .bind(tenant_id)
        .bind(package_id)
        .fetch_one(&mut **tx)
        .await?,
    )
    .map_err(|error| RuntimeError::Internal(error.into()))?;
    for rule in rules {
        let mut detail: Value = rule.try_get("detail_json")?;
        if detail.get("kind").and_then(Value::as_str) != Some("model") {
            continue;
        }
        let evaluator_id: Uuid = rule.try_get("profile_rule_id")?;
        let execution = payload
            .model_evaluator_executions
            .iter()
            .find(|execution| execution.evaluator_id == evaluator_id)
            .ok_or_else(|| {
                RuntimeError::Internal(anyhow::anyhow!(
                    "model evaluator {evaluator_id} has no signed execution snapshot"
                ))
            })?;
        let rule_id: Uuid = rule.try_get("id")?;
        let evaluator_execution_id = stable_id(rule_id, b"model-evaluator-execution");
        let evaluator_command_id = stable_id(evaluator_execution_id, b"start-command");
        let expected_output = detail.get("expectedOutput").cloned().unwrap_or(Value::Null);
        let input = json!({
            "actualOutput": actual_output,
            "expectedOutput": expected_output,
            "targetExecutionId": target_execution_id,
            "promptObjectId": execution.prompt_object_id,
        });
        insert_evaluator_execution(
            tx,
            &payload,
            execution,
            evaluator_execution_id,
            evaluator_command_id,
            target_execution_id,
            rule_id,
            &input,
        )
        .await?;
        detail["actualOutput"] = actual_output.clone();
        sqlx::query(
            "UPDATE evaluation_rule_results SET status='running',evaluator_command_id=?,evaluator_execution_id=?,detail_json=? WHERE tenant_id=? AND id=? AND status='queued'",
        )
        .bind(evaluator_command_id)
        .bind(evaluator_execution_id)
        .bind(detail)
        .bind(tenant_id)
        .bind(rule_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_evaluator_execution(
    tx: &mut Transaction<'_, MySql>,
    package: &RuntimeWorkPackagePayloadV1,
    evaluator: &agentx_runtime_contracts::RuntimeModelEvaluatorExecutionV1,
    execution_id: Uuid,
    command_id: Uuid,
    target_execution_id: Uuid,
    rule_id: Uuid,
    input: &Value,
) -> RuntimeResult<()> {
    let started_at = time::OffsetDateTime::now_utc();
    let state_hash = agentx_runtime_contracts::content_hash(&json!({
        "status":"queued",
        "input":input,
        "targetExecutionId":target_execution_id,
    }))
    .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query(
        "INSERT INTO workflow_executions(id,tenant_id,workflow_id,workflow_version_id,bundle_id,work_package_id,parent_execution_id,parent_node_execution_id,admission_epoch,state_version,trace_id,trigger_type,initiator_user_id,initiator_user_name,initiator_department_id,initiator_department_name,trigger_source_id,trigger_name,status,started_at,input_json) VALUES(?,?,?,?,?,?,?,?,1,1,?,'evaluation',?,?,?,?,?,?,'queued',?,?)",
    )
    .bind(execution_id)
    .bind(package.tenant_id)
    .bind(package.authorization.workflow_id)
    .bind(evaluator.evaluator_id)
    .bind(package.package_id)
    .bind(package.package_id)
    .bind(target_execution_id)
    .bind(rule_id)
    .bind(Uuid::now_v7())
    .bind(package.origin.initiator_user_id)
    .bind(&package.origin.initiator_user_name)
    .bind(package.origin.initiator_department_id)
    .bind(&package.origin.initiator_department_name)
    .bind(target_execution_id)
    .bind(&package.origin.trigger_name)
    .bind(started_at)
    .bind(input)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO execution_snapshots(execution_id,tenant_id,workflow_version_id,bundle_id,work_package_id,admission_epoch,state_version,definition_json,compiled_ir_json,compiled_ir_hash,compiler_version,resource_snapshot_json,authorization_snapshot_json,policy_snapshot_json,execution_context_json,worker_compatibility_json,object_manifest_json,runtime_settings_json,state_hash) VALUES(?,?,?,?,?,1,1,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(execution_id)
    .bind(package.tenant_id)
    .bind(evaluator.evaluator_id)
    .bind(package.package_id)
    .bind(package.package_id)
    .bind(&evaluator.definition)
    .bind(serde_json::to_value(&evaluator.compiled_ir).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(&evaluator.compiled_ir.canonical_hash)
    .bind(&evaluator.compiled_ir.compiler_version)
    .bind(serde_json::to_value(
        package
            .resources
            .iter()
            .filter(|resource| {
                resource.resource_kind == agentx_runtime_contracts::RuntimeResourceKindV1::Model
                    && resource.resource_id == evaluator.resource_id
            })
            .collect::<Vec<_>>(),
    ).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(&package.authorization).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(&package.runtime_policy).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(crate::execution_context::create_snapshot(
        crate::execution_context::NewExecutionContext {
            execution_id,
            started_at,
            parent_execution_id: Some(target_execution_id),
            workflow: agentx_runtime_contracts::ExecutionWorkflowSnapshotV1 {
                id: package.workflow.id,
                name: format!("{} / Model evaluator", package.workflow.name),
                version_id: evaluator.evaluator_id,
                version_number: package.workflow.version_number,
                owner_department: package.workflow.owner_department.clone(),
            },
            trigger_type: "evaluation",
            origin: &package.origin,
            application_id: None,
            invocation_id: None,
            session: None,
        },
    )).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(&package.worker_compatibility).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(&package.objects).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(json!({
        "runtimePolicy": &package.runtime_policy,
        "workPackageSpec": &package.spec,
        "workPackageOverlay": &package.overlay,
        "agentBundle": &package.agent_bundle,
    }))
    .bind(state_hash.as_str())
    .execute(&mut **tx)
    .await?;
    let overlay = json!({});
    let overlay_hash = agentx_runtime_contracts::content_hash(&overlay)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query(
        "INSERT INTO execution_children(tenant_id,parent_execution_id,parent_node_execution_id,child_execution_id,child_bundle_id,relationship,context_overlay_json,context_overlay_hash) VALUES(?,?,?,?,?,'evaluation',?,?)",
    )
    .bind(package.tenant_id)
    .bind(target_execution_id)
    .bind(rule_id)
    .bind(execution_id)
    .bind(package.package_id)
    .bind(overlay)
    .bind(overlay_hash.as_str())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'start_execution','execution',?,?,?,'pending')",
    )
    .bind(command_id)
    .bind(package.tenant_id)
    .bind(execution_id.to_string())
    .bind(format!("evaluation:evaluator:start:{execution_id}"))
    .bind(json!({
        "executionId":execution_id,
        "targetExecutionId":target_execution_id,
        "evaluatorId":evaluator.evaluator_id,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn converge_model_evaluator(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    package_id: Uuid,
    execution_id: Uuid,
    terminal_status: &str,
    output: &Value,
    error: &Option<Value>,
) -> RuntimeResult<()> {
    let Some(rule) = sqlx::query(
        "SELECT rr.id,rr.evaluation_run_case_id,rr.status,rr.detail_json,c.evaluation_run_id FROM evaluation_rule_results rr FORCE INDEX(idx_evaluation_rule_evaluator) JOIN evaluation_run_cases c ON c.id=rr.evaluation_run_case_id AND c.tenant_id=rr.tenant_id JOIN evaluation_runs r ON r.id=c.evaluation_run_id AND r.tenant_id=c.tenant_id WHERE rr.tenant_id=? AND rr.evaluator_execution_id=? AND r.work_package_id=? FOR UPDATE OF rr",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .bind(package_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(RuntimeError::Internal(anyhow::anyhow!(
            "evaluation execution {execution_id} is not a target or evaluator"
        )));
    };
    if rule.try_get::<String, _>("status")? != "running" {
        return Ok(());
    }
    let rule_id: Uuid = rule.try_get("id")?;
    let case_id: Uuid = rule.try_get("evaluation_run_case_id")?;
    let run_id: Uuid = rule.try_get("evaluation_run_id")?;
    let mut detail: Value = rule.try_get("detail_json")?;
    let evaluation = output.get("evaluation").unwrap_or(output);
    let passed = terminal_status == "completed"
        && evaluation
            .get("passed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let score = evaluation
        .get("score")
        .and_then(Value::as_f64)
        .unwrap_or(if passed { 1.0 } else { 0.0 });
    let rule_status = if terminal_status == "cancelled" {
        "cancelled"
    } else if terminal_status != "completed" {
        "error"
    } else if passed {
        "passed"
    } else {
        "failed"
    };
    detail["modelResult"] = output.clone();
    if let Some(error) = error {
        detail["error"] = error.clone();
    }
    let (duration_ms, cost_micros) = execution_measurements(tx, tenant_id, execution_id).await?;
    sqlx::query(
        "UPDATE evaluation_rule_results SET status=?,passed=?,score=?,detail_json=?,duration_ms=?,cost_micros=?,completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND status='running'",
    )
    .bind(rule_status)
    .bind(if terminal_status == "completed" { Some(passed) } else { None })
    .bind(score)
    .bind(detail)
    .bind(duration_ms)
    .bind(cost_micros)
    .bind(tenant_id)
    .bind(rule_id)
    .execute(&mut **tx)
    .await?;
    let case_status = evaluation_scoring_status(tx, tenant_id, case_id).await?;
    if case_status != "scoring" {
        sqlx::query(
            "UPDATE evaluation_run_cases SET status=?,completed_at=UTC_TIMESTAMP(6),cost_micros=cost_micros+(SELECT COALESCE(SUM(cost_micros),0) FROM evaluation_rule_results WHERE tenant_id=? AND evaluation_run_case_id=?) WHERE tenant_id=? AND id=? AND status='scoring'",
        )
        .bind(case_status)
        .bind(tenant_id)
        .bind(case_id)
        .bind(tenant_id)
        .bind(case_id)
        .execute(&mut **tx)
        .await?;
    }
    converge_evaluation_run(tx, tenant_id, package_id, run_id, execution_id).await
}

async fn evaluation_scoring_status(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    case_id: Uuid,
) -> RuntimeResult<&'static str> {
    let statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM evaluation_rule_results WHERE tenant_id=? AND evaluation_run_case_id=?",
    )
    .bind(tenant_id)
    .bind(case_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(scoring_status(&statuses))
}

fn scoring_status(statuses: &[String]) -> &'static str {
    if statuses
        .iter()
        .any(|status| matches!(status.as_str(), "queued" | "running"))
    {
        "scoring"
    } else if statuses.iter().any(|status| status == "error") {
        "failed"
    } else if statuses.iter().any(|status| status == "cancelled") {
        "cancelled"
    } else {
        "completed"
    }
}

async fn execution_measurements(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
) -> RuntimeResult<(Option<u64>, u64)> {
    Ok(sqlx::query_as("SELECT duration_ms,(SELECT CAST(COALESCE(SUM(cost_micros),0) AS UNSIGNED) FROM runtime_calls WHERE tenant_id=? AND execution_id=?) FROM workflow_executions WHERE tenant_id=? AND id=?")
        .bind(tenant_id).bind(execution_id).bind(tenant_id).bind(execution_id).fetch_one(&mut **tx).await?)
}

async fn evaluate_deterministic_rules(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    case_id: Uuid,
    actual_output: &Value,
) -> RuntimeResult<()> {
    let rules = sqlx::query(
        "SELECT id,detail_json FROM evaluation_rule_results WHERE tenant_id=? AND evaluation_run_case_id=? AND status='queued' FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(case_id)
    .fetch_all(&mut **tx)
    .await?;
    for rule in rules {
        let rule_id: Uuid = rule.try_get("id")?;
        let mut detail: Value = rule.try_get("detail_json")?;
        if detail.get("kind").and_then(Value::as_str) != Some("deterministic_rule") {
            continue;
        }
        let expression = detail
            .get("expression")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let expected = detail.get("expectedOutput").cloned().unwrap_or(Value::Null);
        let (passed, evaluation_error) =
            evaluate_deterministic_expression(&expression, actual_output, &expected);
        detail["actualOutput"] = actual_output.clone();
        if let Some(error) = evaluation_error {
            detail["error"] = json!(error);
        }
        sqlx::query(
            "UPDATE evaluation_rule_results SET status=?,passed=?,score=?,detail_json=?,completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND status='queued'",
        )
        .bind(if passed { "passed" } else { "failed" })
        .bind(passed)
        .bind(if passed { 1.0_f64 } else { 0.0_f64 })
        .bind(detail)
        .bind(tenant_id)
        .bind(rule_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn evaluate_deterministic_expression(
    expression: &str,
    actual: &Value,
    expected: &Value,
) -> (bool, Option<String>) {
    if expression == "exact_match" {
        return (*actual == *expected, None);
    }
    if expression == "not_null" {
        return (!actual.is_null(), None);
    }
    let Ok(spec) = serde_json::from_str::<Value>(expression) else {
        return (
            false,
            Some("unsupported deterministic evaluator expression".into()),
        );
    };
    let kind = spec.get("type").and_then(Value::as_str).unwrap_or_default();
    let configuration = spec.get("configuration").unwrap_or(&Value::Null);
    match kind {
        "exact" => (*actual == *expected, None),
        "contains" => {
            let actual = actual.as_str().unwrap_or_default();
            let expected = expected.as_str().unwrap_or_default();
            (actual.contains(expected), None)
        }
        "regex" => {
            let Some(pattern) = configuration.get("pattern").and_then(Value::as_str) else {
                return (false, Some("regex evaluator requires pattern".into()));
            };
            match regex::Regex::new(pattern) {
                Ok(pattern) => (pattern.is_match(actual.as_str().unwrap_or_default()), None),
                Err(error) => (false, Some(format!("invalid regex evaluator: {error}"))),
            }
        }
        "json_schema" => {
            let Some(schema) = configuration.get("schema") else {
                return (false, Some("JSON Schema evaluator requires schema".into()));
            };
            match jsonschema::validator_for(schema) {
                Ok(validator) => (validator.is_valid(actual), None),
                Err(error) => (
                    false,
                    Some(format!("invalid JSON Schema evaluator: {error}")),
                ),
            }
        }
        _ => (
            false,
            Some("unsupported deterministic evaluator expression".into()),
        ),
    }
}

async fn converge_evaluation_run(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    package_id: Uuid,
    run_id: Uuid,
    execution_id: Uuid,
) -> RuntimeResult<()> {
    sqlx::query("SELECT status FROM evaluation_runs WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(tenant_id)
        .bind(run_id)
        .fetch_one(&mut **tx)
        .await?;
    // The Work Package parent serializes convergence and cancellation. This
    // single consistent-view aggregate does not need locks on all old Cases.
    let counts = sqlx::query(
        "SELECT COUNT(*) total,CAST(COALESCE(SUM(status IN ('completed','failed','cancelled')),0) AS UNSIGNED) terminal,CAST(COALESCE(SUM(status='failed'),0) AS UNSIGNED) failed,CAST(COALESCE(SUM(status='cancelled'),0) AS UNSIGNED) cancelled FROM evaluation_run_cases WHERE tenant_id=? AND evaluation_run_id=?",
    )
    .bind(tenant_id)
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    let total: i64 = counts.try_get("total")?;
    let terminal: u64 = counts.try_get("terminal")?;
    let failed: u64 = counts.try_get("failed")?;
    let cancelled: u64 = counts.try_get("cancelled")?;
    let run_status = if total as u64 == terminal {
        if cancelled == total as u64 {
            "cancelled"
        } else if failed > 0 {
            "failed"
        } else {
            "completed"
        }
    } else {
        "running"
    };
    let changed = sqlx::query(
        "UPDATE evaluation_runs SET status=?,version=version+1,completed_at=IF(? IN ('completed','failed','cancelled'),UTC_TIMESTAMP(6),NULL) WHERE tenant_id=? AND id=? AND status<>?",
    )
    .bind(run_status)
    .bind(run_status)
    .bind(tenant_id)
    .bind(run_id)
    .bind(run_status)
    .execute(&mut **tx)
    .await?;
    if run_status != "running" {
        let summary = json!({
            "evaluationRunId": run_id,
            "packageId": package_id,
            "status": run_status,
            "completedCases": terminal,
            "totalCases": total,
        });
        let summary_hash = agentx_runtime_contracts::content_hash(&summary)
            .map_err(|error| RuntimeError::Internal(error.into()))?;
        sqlx::query(
            "UPDATE runtime_work_packages SET status=?,version=version+1,result_json=?,result_hash=?,completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND status='running'",
        )
        .bind(package_terminal_status(run_status))
        .bind(&summary)
        .bind(summary_hash.as_str())
        .bind(tenant_id)
        .bind(package_id)
        .execute(&mut **tx)
        .await?;
    }
    if changed.rows_affected() == 1 {
        let run_version: u64 =
            sqlx::query_scalar("SELECT version FROM evaluation_runs WHERE tenant_id=? AND id=?")
                .bind(tenant_id)
                .bind(run_id)
                .fetch_one(&mut **tx)
                .await?;
        let report = if run_status == "running" {
            None
        } else {
            Some(load_evaluation_report(tx, tenant_id, run_id).await?)
        };
        let event = RuntimeEventPayloadV1::EvaluationChanged {
            run_id,
            run_version,
            work_package_id: package_id,
            status: run_status.into(),
            completed_cases: terminal,
            total_cases: total as u64,
            report,
        };
        insert_governance_event(tx, tenant_id, execution_id, &event).await?;
    }
    Ok(())
}

async fn load_evaluation_report(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    run_id: Uuid,
) -> RuntimeResult<RuntimeEvaluationReportV1> {
    let case_rows = sqlx::query("SELECT id,source_case_id,target_command_id,target_execution_id,status,actual_output_json,duration_ms,cost_micros,error_code,error_message FROM evaluation_run_cases WHERE tenant_id=? AND evaluation_run_id=? ORDER BY created_at,id")
        .bind(tenant_id)
        .bind(run_id)
        .fetch_all(&mut **tx)
        .await?;
    // All convergence writers hold the Work Package parent. Read rules in
    // one scoped query instead of holding that lock for one query per Case.
    let rule_rows = sqlx::query("SELECT rr.evaluation_run_case_id,rr.id,rr.profile_rule_id,rr.evaluator_execution_id,rr.status,rr.passed,CAST(rr.score AS DOUBLE) score,rr.detail_json,rr.duration_ms,rr.cost_micros FROM evaluation_rule_results rr JOIN evaluation_run_cases c ON c.tenant_id=rr.tenant_id AND c.id=rr.evaluation_run_case_id WHERE rr.tenant_id=? AND c.evaluation_run_id=? ORDER BY rr.created_at,rr.id")
        .bind(tenant_id)
        .bind(run_id)
        .fetch_all(&mut **tx)
        .await?;
    let mut rules_by_case = std::collections::HashMap::new();
    for rule in rule_rows {
        let case_id: Uuid = rule.try_get("evaluation_run_case_id")?;
        rules_by_case
            .entry(case_id)
            .or_insert_with(Vec::new)
            .push(rule);
    }
    let mut cases = Vec::with_capacity(case_rows.len());
    let mut completed_cases = 0_u64;
    let mut passed_rules = 0_u64;
    let mut failed_rules = 0_u64;
    let mut error_rules = 0_u64;
    let mut total_cost_micros = 0_u64;
    for row in case_rows {
        let case_id: Uuid = row.try_get("id")?;
        let rule_rows = rules_by_case.remove(&case_id).unwrap_or_default();
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
        let cost_micros: u64 = row.try_get("cost_micros")?;
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

async fn insert_governance_event(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    event: &RuntimeEventPayloadV1,
) -> RuntimeResult<()> {
    sqlx::query(
        "INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,payload_json,status) VALUES(?,?,?,'runtime_event',?,'pending')",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(execution_id)
    .bind(serde_json::to_value(event).map_err(|error| RuntimeError::Internal(error.into()))?)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn package_terminal_status(status: &str) -> &'static str {
    match status {
        "completed" | "succeeded" => "succeeded",
        "cancelled" => "cancelled",
        _ => "failed",
    }
}

async fn insert_execution(
    tx: &mut Transaction<'_, MySql>,
    package: &RuntimeWorkPackagePayloadV1,
    execution_id: Uuid,
    command_id: Uuid,
    input: &Value,
    trigger_type: &str,
) -> RuntimeResult<()> {
    let started_at = time::OffsetDateTime::now_utc();
    let state_hash =
        agentx_runtime_contracts::content_hash(&json!({"status":"queued","input":input}))
            .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query(
        "INSERT INTO workflow_executions(id,tenant_id,workflow_id,workflow_version_id,bundle_id,work_package_id,admission_epoch,state_version,trace_id,trigger_type,initiator_user_id,initiator_user_name,initiator_department_id,initiator_department_name,trigger_source_id,trigger_name,status,started_at,input_json) VALUES(?,?,?,?,?,?,1,1,?,?,?,?,?,?,?,?,'queued',?,?)",
    )
    .bind(execution_id)
    .bind(package.tenant_id)
    .bind(package.authorization.workflow_id)
    .bind(package.package_id)
    .bind(package.package_id)
    .bind(package.package_id)
    .bind(Uuid::now_v7())
    .bind(trigger_type)
    .bind(package.origin.initiator_user_id)
    .bind(&package.origin.initiator_user_name)
    .bind(package.origin.initiator_department_id)
    .bind(&package.origin.initiator_department_name)
    .bind(package.origin.trigger_source_id)
    .bind(&package.origin.trigger_name)
    .bind(started_at)
    .bind(input)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO execution_snapshots(execution_id,tenant_id,workflow_version_id,bundle_id,work_package_id,admission_epoch,state_version,definition_json,compiled_ir_json,compiled_ir_hash,compiler_version,resource_snapshot_json,authorization_snapshot_json,policy_snapshot_json,execution_context_json,worker_compatibility_json,object_manifest_json,runtime_settings_json,state_hash) VALUES(?,?,?,?,?,1,1,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(execution_id)
    .bind(package.tenant_id)
    .bind(package.package_id)
    .bind(package.package_id)
    .bind(package.package_id)
    .bind(&package.definition)
    .bind(serde_json::to_value(&package.compiled_ir).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(&package.compiled_ir.canonical_hash)
    .bind(&package.compiled_ir.compiler_version)
    .bind(serde_json::to_value(&package.resources).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(&package.authorization).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(&package.runtime_policy).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(crate::execution_context::create_snapshot(
        crate::execution_context::NewExecutionContext {
            execution_id,
            started_at,
            parent_execution_id: None,
            workflow: package.workflow.clone(),
            trigger_type,
            origin: &package.origin,
            application_id: None,
            invocation_id: None,
            session: None,
        },
    )).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(&package.worker_compatibility).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(serde_json::to_value(&package.objects).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(json!({
        "runtimePolicy": &package.runtime_policy,
        "workPackageSpec": &package.spec,
        "workPackageOverlay": &package.overlay,
        "agentBundle": &package.agent_bundle,
    }))
    .bind(state_hash.as_str())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'start_execution','execution',?,?,?,'pending')",
    )
    .bind(command_id)
    .bind(package.tenant_id)
    .bind(execution_id.to_string())
    .bind(format!("work-package:start:{}:{execution_id}", package.package_id))
    .bind(json!({"executionId":execution_id,"workPackageId":package.package_id,"input":input}))
    .execute(&mut **tx)
    .await?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{evaluate_deterministic_expression, scoring_status};

    #[test]
    fn multiple_judges_settle_without_losing_an_earlier_error() {
        for statuses in [["passed", "error"], ["error", "passed"]] {
            assert_eq!(scoring_status(&statuses.map(str::to_owned)), "failed");
        }
        assert_eq!(
            scoring_status(&["failed".into(), "passed".into()]),
            "completed"
        );
        assert_eq!(
            scoring_status(&["error".into(), "running".into()]),
            "scoring"
        );
        assert_eq!(
            scoring_status(&["passed".into(), "cancelled".into()]),
            "cancelled"
        );
    }

    #[test]
    fn deterministic_evaluators_cover_the_public_profile_types() {
        assert!(
            evaluate_deterministic_expression(
                r#"{"type":"exact","configuration":{}}"#,
                &json!({"answer": 42}),
                &json!({"answer": 42})
            )
            .0
        );
        assert!(
            evaluate_deterministic_expression(
                r#"{"type":"contains","configuration":{}}"#,
                &json!("agentx runtime"),
                &json!("runtime")
            )
            .0
        );
        assert!(
            evaluate_deterministic_expression(
                r#"{"type":"regex","configuration":{"pattern":"^agentx"}}"#,
                &json!("agentx runtime"),
                &json!(null)
            )
            .0
        );
        assert!(evaluate_deterministic_expression(
            r#"{"type":"json_schema","configuration":{"schema":{"type":"integer","minimum":1}}}"#,
            &json!(2),
            &json!(null)
        )
        .0);
    }
}
