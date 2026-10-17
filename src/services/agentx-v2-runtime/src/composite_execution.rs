use agentx_domain::NodeExecutionId;
use agentx_runtime::{
    CompiledWorkflow, ExpressionContext, ExpressionEngine, materialize_and_validate_start_input,
};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_child(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    parent_execution_id: Uuid,
    bundle_id: Uuid,
    work_package_id: Option<Uuid>,
    parent_node_execution_id: NodeExecutionId,
    activation: &agentx_runtime::NodeActivation,
    node: &agentx_runtime::CompiledNode,
    parent_workflow: &CompiledWorkflow,
    current_context: &Value,
) -> RuntimeResult<()> {
    let workflow_version_id = node
        .parameters
        .get("workflowVersionId")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            invalid(
                "COMPOSITE_VERSION_REQUIRED",
                "Composite node requires an immutable Workflow Version",
            )
        })?;
    let composite = if let Some(work_package_id) = work_package_id {
        sqlx::query(
            "SELECT workflow_json,definition_json,compiled_ir_json,definition_hash,ir_hash FROM runtime_composite_snapshots WHERE tenant_id=? AND work_package_id=? AND bundle_id IS NULL AND workflow_version_id=?",
        )
        .bind(tenant_id)
        .bind(work_package_id)
        .bind(workflow_version_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query(
            "SELECT workflow_json,definition_json,compiled_ir_json,definition_hash,ir_hash FROM runtime_composite_snapshots WHERE tenant_id=? AND bundle_id=? AND work_package_id IS NULL AND workflow_version_id=?",
        )
        .bind(tenant_id)
        .bind(bundle_id)
        .bind(workflow_version_id)
        .fetch_optional(&mut **tx)
        .await?
    }
    .ok_or_else(|| invalid("COMPOSITE_SNAPSHOT_MISSING", "Composite Definition and IR were not materialized during Prepare"))?;
    let parent = sqlx::query(
        "SELECT e.application_id,e.admission_epoch,e.input_json,e.initiator_user_id,e.initiator_user_name,e.initiator_department_id,e.initiator_department_name,s.resource_snapshot_json,s.authorization_snapshot_json,s.policy_snapshot_json,s.execution_context_json,s.worker_compatibility_json,s.object_manifest_json,s.runtime_settings_json FROM workflow_executions e JOIN execution_snapshots s ON s.tenant_id=e.tenant_id AND s.execution_id=e.id WHERE e.tenant_id=? AND e.id=?",
    )
    .bind(tenant_id)
    .bind(parent_execution_id)
    .fetch_one(&mut **tx)
    .await?;
    let compiled_ir: agentx_runtime_contracts::CompiledWorkflowV1 =
        serde_json::from_value(composite.try_get("compiled_ir_json")?)
            .map_err(|error| RuntimeError::Internal(error.into()))?;
    let workflow: agentx_runtime_contracts::ExecutionWorkflowSnapshotV1 =
        serde_json::from_value(composite.try_get("workflow_json")?)
            .map_err(|error| RuntimeError::Internal(error.into()))?;
    let child_workflow_id = workflow.id;
    let input_items = activation
        .inputs
        .get("main")
        .or_else(|| activation.inputs.values().next())
        .cloned()
        .unwrap_or_default();
    let current_input = if input_items.len() == 1 {
        input_items[0].json.clone()
    } else {
        Value::Array(input_items.into_iter().map(|item| item.json).collect())
    };
    let outputs =
        crate::engine_persistence::load_output_namespace(tx, tenant_id, parent_execution_id)
            .await?;
    let mut input = ExpressionEngine
        .resolve_parameters(
            node.parameters.get("inputs").ok_or_else(|| {
                invalid(
                    "COMPOSITE_INPUTS_REQUIRED",
                    "Sub-workflow inputs mapping is required",
                )
            })?,
            &ExpressionContext {
                json: current_input.clone(),
                input: current_input,
                inputs: parent
                    .try_get::<Option<Value>, _>("input_json")?
                    .unwrap_or(Value::Null),
                outputs,
                contexts: current_context.clone(),
                execution: parent.try_get("execution_context_json")?,
                loop_context: activation.loop_frame.clone().unwrap_or(Value::Null),
                output_node_keys: parent_workflow
                    .nodes
                    .iter()
                    .map(|node| (node.id.clone(), node.key.clone()))
                    .collect(),
                ..ExpressionContext::default()
            },
        )
        .map_err(|error| invalid("COMPOSITE_INPUT_MAPPING_INVALID", &error.to_string()))?;
    materialize_and_validate_start_input(&mut input, &compiled_ir.start.inputs)
        .map_err(|error| invalid("COMPOSITE_INPUT_INVALID", &error.to_string()))?;
    let context_overlay = current_context.clone();
    let overlay_hash = agentx_runtime_contracts::content_hash(&context_overlay)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let timeout_micros = node
        .settings
        .timeout_ms
        .map(|value| value.saturating_mul(1_000).min(i64::MAX as u64));
    let child_execution_id = crate::engine_names::deterministic_uuid(
        parent_node_execution_id.as_uuid(),
        b"composite-child-execution",
    );
    let command_id =
        crate::engine_names::deterministic_uuid(child_execution_id, b"composite-child-start");
    let state_hash = agentx_runtime_contracts::content_hash(&json!({
        "status":"queued",
        "input":input,
        "parentExecutionId":parent_execution_id,
        "parentNodeExecutionId":parent_node_execution_id.as_uuid(),
    }))
    .map_err(|error| RuntimeError::Internal(error.into()))?;
    let started_at = OffsetDateTime::now_utc();
    let mut execution_context: Value = parent.try_get("execution_context_json")?;
    if let Some(root) = execution_context.as_object_mut() {
        root.insert("id".into(), json!(child_execution_id));
        root.insert(
            "startedAt".into(),
            json!(
                started_at
                    .format(&Rfc3339)
                    .map_err(|error| RuntimeError::Internal(error.into()))?
            ),
        );
        root.insert("parentExecutionId".into(), json!(parent_execution_id));
        root.insert(
            "trigger".into(),
            json!({
                "type": "composite",
                "sourceId": parent_node_execution_id.as_uuid(),
                "name": node.name,
            }),
        );
        root.insert(
            "workflow".into(),
            serde_json::to_value(workflow).map_err(|error| RuntimeError::Internal(error.into()))?,
        );
        root.remove("node");
    }
    sqlx::query(
        "INSERT INTO workflow_executions(id,tenant_id,workflow_id,workflow_version_id,application_id,bundle_id,work_package_id,parent_execution_id,parent_node_execution_id,admission_epoch,state_version,trace_id,trigger_type,initiator_user_id,initiator_user_name,initiator_department_id,initiator_department_name,trigger_source_id,trigger_name,status,started_at,input_json) VALUES(?,?,?,?,?,?,?,?,?,?,1,?,'composite',?,?,?,?,?,?,'queued',?,?)",
    )
    .bind(child_execution_id)
    .bind(tenant_id)
    .bind(child_workflow_id)
    .bind(workflow_version_id)
    .bind(parent.try_get::<Option<Uuid>, _>("application_id")?)
    .bind(bundle_id)
    .bind(work_package_id)
    .bind(parent_execution_id)
    .bind(parent_node_execution_id.as_uuid())
    .bind(parent.try_get::<u64, _>("admission_epoch")?)
    .bind(Uuid::now_v7())
    .bind(parent.try_get::<Option<Uuid>, _>("initiator_user_id")?)
    .bind(parent.try_get::<Option<String>, _>("initiator_user_name")?)
    .bind(parent.try_get::<Option<Uuid>, _>("initiator_department_id")?)
    .bind(parent.try_get::<Option<String>, _>("initiator_department_name")?)
    .bind(parent_node_execution_id.as_uuid())
    .bind(Some(node.name.clone()))
    .bind(started_at)
    .bind(&input)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO execution_snapshots(execution_id,tenant_id,workflow_version_id,bundle_id,work_package_id,admission_epoch,state_version,definition_json,compiled_ir_json,compiled_ir_hash,compiler_version,resource_snapshot_json,authorization_snapshot_json,policy_snapshot_json,execution_context_json,worker_compatibility_json,object_manifest_json,runtime_settings_json,state_hash) VALUES(?,?,?,?,?,?,1,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(child_execution_id)
    .bind(tenant_id)
    .bind(workflow_version_id)
    .bind(bundle_id)
    .bind(work_package_id)
    .bind(parent.try_get::<u64, _>("admission_epoch")?)
    .bind(composite.try_get::<Value, _>("definition_json")?)
    .bind(serde_json::to_value(&compiled_ir).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(&compiled_ir.canonical_hash)
    .bind(&compiled_ir.compiler_version)
    .bind(parent.try_get::<Value, _>("resource_snapshot_json")?)
    .bind(parent.try_get::<Value, _>("authorization_snapshot_json")?)
    .bind(parent.try_get::<Value, _>("policy_snapshot_json")?)
    .bind(execution_context)
    .bind(parent.try_get::<Value, _>("worker_compatibility_json")?)
    .bind(parent.try_get::<Value, _>("object_manifest_json")?)
    .bind(parent.try_get::<Value, _>("runtime_settings_json")?)
    .bind(state_hash.as_str())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO execution_children(tenant_id,parent_execution_id,parent_node_execution_id,child_execution_id,child_bundle_id,relationship,context_overlay_json,context_overlay_hash,deadline_at) VALUES(?,?,?,?,?,'composite',?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? MICROSECOND))",
    )
    .bind(tenant_id)
    .bind(parent_execution_id)
    .bind(parent_node_execution_id.as_uuid())
    .bind(child_execution_id)
    .bind(bundle_id)
    .bind(&context_overlay)
    .bind(overlay_hash.as_str())
    .bind(timeout_micros)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'active_execution',?)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(bundle_id)
    .bind(child_execution_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'start_execution','execution',?,?,?,'pending')",
    )
    .bind(command_id)
    .bind(tenant_id)
    .bind(child_execution_id.to_string())
    .bind(format!("composite:start:{child_execution_id}"))
    .bind(json!({
        "executionId":child_execution_id,
        "parentExecutionId":parent_execution_id,
        "parentNodeExecutionId":parent_node_execution_id.as_uuid(),
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn enqueue_overdue(pool: &sqlx::MySqlPool, limit: u32) -> RuntimeResult<u64> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        "SELECT c.tenant_id,c.parent_execution_id,c.parent_node_execution_id,c.child_execution_id FROM execution_children c JOIN workflow_executions child ON child.tenant_id=c.tenant_id AND child.id=c.child_execution_id JOIN workflow_executions parent ON parent.tenant_id=c.tenant_id AND parent.id=c.parent_execution_id WHERE c.relationship='composite' AND c.merge_status='pending' AND c.deadline_at IS NOT NULL AND c.deadline_at<=UTC_TIMESTAMP(6) AND child.status NOT IN ('succeeded','failed','cancelled','timed_out') AND parent.status NOT IN ('succeeded','failed','cancelled','timed_out') ORDER BY c.deadline_at,c.child_execution_id LIMIT ? FOR UPDATE SKIP LOCKED",
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(&mut *tx)
    .await?;
    for row in &rows {
        let tenant_id: Uuid = row.try_get("tenant_id")?;
        let parent_execution_id: Uuid = row.try_get("parent_execution_id")?;
        let parent_node_execution_id: Uuid = row.try_get("parent_node_execution_id")?;
        let child_execution_id: Uuid = row.try_get("child_execution_id")?;
        let cancel_id = crate::engine_names::deterministic_uuid(
            child_execution_id,
            b"composite-timeout-cancel",
        );
        sqlx::query(
            "INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'cancel_execution','execution',?,?,?,'pending') ON DUPLICATE KEY UPDATE id=id",
        )
        .bind(cancel_id)
        .bind(tenant_id)
        .bind(child_execution_id.to_string())
        .bind(format!("composite:timeout:cancel:{child_execution_id}"))
        .bind(json!({
            "executionId": child_execution_id,
            "parentExecutionId": parent_execution_id,
            "reason": "COMPOSITE_TIMEOUT",
        }))
        .execute(&mut *tx)
        .await?;

        let resume_id = crate::engine_names::deterministic_uuid(
            child_execution_id,
            b"composite-timeout-resume-parent",
        );
        sqlx::query(
            "INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status,available_at) VALUES(?,?,'resume_execution','execution',?,?,?,'pending',DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 1 SECOND)) ON DUPLICATE KEY UPDATE id=id",
        )
        .bind(resume_id)
        .bind(tenant_id)
        .bind(parent_execution_id.to_string())
        .bind(format!("composite:timeout:resume:{child_execution_id}"))
        .bind(json!({
            "nodeExecutionId": parent_node_execution_id,
            "childExecutionId": child_execution_id,
            "childStatus": "timed_out",
            "outputPort": "main",
            "payload": Value::Null,
            "contextOverlay": {},
            "error": {
                "code": "COMPOSITE_TIMEOUT",
                "message": "Composite child exceeded the node timeout",
            },
        }))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(rows.len() as u64)
}

pub(crate) async fn converge_child(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    child_execution_id: Uuid,
    child_status: &str,
    output: &Value,
    error: &Option<Value>,
    child_context: &Value,
) -> RuntimeResult<()> {
    let Some(relation) = sqlx::query(
        "SELECT parent_execution_id,parent_node_execution_id,context_overlay_json,merge_status FROM execution_children WHERE tenant_id=? AND child_execution_id=? AND relationship='composite' FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(child_execution_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(());
    };
    if relation.try_get::<String, _>("merge_status")? != "pending" {
        return Ok(());
    }
    let parent_execution_id: Uuid = relation.try_get("parent_execution_id")?;
    let parent_node_execution_id: Uuid = relation.try_get("parent_node_execution_id")?;
    let command_id =
        crate::engine_names::deterministic_uuid(child_execution_id, b"composite-child-complete");
    let mut overlay: Value = relation.try_get("context_overlay_json")?;
    if child_status == "completed" {
        merge_context_overlay(&mut overlay, child_context);
    }
    sqlx::query(
        "INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'resume_execution','execution',?,?,?,'pending') ON DUPLICATE KEY UPDATE id=id",
    )
    .bind(command_id)
    .bind(tenant_id)
    .bind(parent_execution_id.to_string())
    .bind(format!("composite:complete:{child_execution_id}"))
    .bind(json!({
        "nodeExecutionId":parent_node_execution_id,
        "childExecutionId":child_execution_id,
        "childStatus":if child_status == "completed" { "succeeded" } else { child_status },
        "outputPort":"main",
        "payload":output,
        "contextOverlay":overlay,
        "error":error,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) fn merge_context_overlay(target: &mut Value, overlay: &Value) {
    let (Some(target), Some(overlay)) = (target.as_object_mut(), overlay.as_object()) else {
        *target = overlay.clone();
        return;
    };
    for (key, value) in overlay {
        if let Some(current) = target.get_mut(key) {
            merge_context_overlay(current, value);
        } else {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn invalid(code: &str, message: &str) -> RuntimeError {
    RuntimeError::BadRequest(
        agentx_runtime_contracts::RuntimePublishErrorCodeV1::UnsupportedCapability,
        format!("{code}: {message}"),
    )
}
