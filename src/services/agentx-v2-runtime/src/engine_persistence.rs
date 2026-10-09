use agentx_domain::{ContextScope, ContextWriteOperation, NodeExecutionId};
use agentx_node_protocol::Item;
use agentx_runtime::{
    ExecutionMachine, ExpressionContext, ExpressionEngine, RuntimeExecutionStatus,
};
use agentx_runtime_contracts::{RuntimeAuthorizationSnapshotV1, RuntimeResourceBindingV1};
use serde_json::{Map, Value, json};
use sqlx::{MySql, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::{
    engine_names::{activation_status, machine_status, side_effect_name},
    engine_protocol::runtime_bad_request,
    error::{RuntimeError, RuntimeResult},
};

pub(crate) fn apply_context_write(
    context: &mut Value,
    path: &str,
    operation: ContextWriteOperation,
    value: Value,
) -> RuntimeResult<()> {
    let segments = path
        .split('.')
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return Err(runtime_bad_request(
            "INVALID_CONTEXT_PATH",
            "Context path is empty",
        ));
    }
    let mut target = context;
    for segment in &segments[..segments.len() - 1] {
        let object = target.as_object_mut().ok_or_else(|| {
            runtime_bad_request("INVALID_CONTEXT_PATH", "Context path is not an object")
        })?;
        target = object
            .entry((*segment).to_owned())
            .or_insert_with(|| json!({}));
    }
    let object = target.as_object_mut().ok_or_else(|| {
        runtime_bad_request("INVALID_CONTEXT_PATH", "Context parent is not an object")
    })?;
    let key = segments[segments.len() - 1];
    let current = object.get(key).cloned().unwrap_or(Value::Null);
    let next = match operation {
        ContextWriteOperation::Set => Some(value),
        ContextWriteOperation::SetIfAbsent => current.is_null().then_some(value),
        ContextWriteOperation::Delete => None,
        ContextWriteOperation::Append => {
            let mut values = current.as_array().cloned().unwrap_or_default();
            match value {
                Value::Array(items) => values.extend(items),
                value => values.push(value),
            }
            Some(Value::Array(values))
        }
        ContextWriteOperation::MergeObject => {
            let mut values = current.as_object().cloned().unwrap_or_default();
            values.extend(value.as_object().cloned().unwrap_or_default());
            Some(Value::Object(values))
        }
        ContextWriteOperation::Increment => Some(json!(
            current.as_f64().unwrap_or(0.0) + value.as_f64().unwrap_or(0.0)
        )),
        ContextWriteOperation::Min => Some(json!(
            current
                .as_f64()
                .unwrap_or(f64::INFINITY)
                .min(value.as_f64().unwrap_or(f64::INFINITY))
        )),
        ContextWriteOperation::Max => Some(json!(
            current
                .as_f64()
                .unwrap_or(f64::NEG_INFINITY)
                .max(value.as_f64().unwrap_or(f64::NEG_INFINITY))
        )),
        ContextWriteOperation::CompareAndSet => {
            let expected = value.get("expected").cloned().unwrap_or(Value::Null);
            (current == expected).then(|| value.get("value").cloned().unwrap_or(Value::Null))
        }
    };
    if let Some(next) = next {
        object.insert(key.to_owned(), next);
    } else if operation == ContextWriteOperation::Delete {
        object.remove(key);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_checkpoint(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    bundle_id: Uuid,
    work_package_id: Option<Uuid>,
    state_version: u64,
    node_execution_id: Option<NodeExecutionId>,
    machine: &ExecutionMachine,
    context: &Value,
    checkpoint_type: &str,
) -> RuntimeResult<Uuid> {
    let sequence: u64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(MAX(sequence_number),0)+1 AS UNSIGNED) FROM checkpoints WHERE execution_id=?",
    )
    .bind(execution_id)
    .fetch_one(&mut **tx)
    .await?;
    let payload = serde_json::to_value(crate::fork_runtime::checkpoint_payload(machine, context))
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let hash = agentx_runtime_contracts::content_hash(&payload)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let checkpoint_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO checkpoints(id,tenant_id,execution_id,bundle_id,work_package_id,node_execution_id,sequence_number,checkpoint_type,state_hash,payload_hash,state_version,payload_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(checkpoint_id)
    .bind(tenant_id)
    .bind(execution_id)
    .bind(bundle_id)
    .bind(work_package_id)
    .bind(node_execution_id.map(NodeExecutionId::as_uuid))
    .bind(sequence)
    .bind(checkpoint_type)
    .bind(hash.as_str())
    .bind(hash.as_str())
    .bind(state_version)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT IGNORE INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'checkpoint_fork_source',?)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(bundle_id)
    .bind(checkpoint_id)
    .execute(&mut **tx)
    .await?;
    Ok(checkpoint_id)
}

pub(super) async fn upsert_activation(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    activation: &agentx_runtime::NodeActivation,
    node: &agentx_runtime::CompiledNode,
) -> RuntimeResult<()> {
    let status = activation_status(activation.status);
    sqlx::query(
        "INSERT INTO node_executions(id,tenant_id,execution_id,node_id,node_key,node_name,node_type,node_version,generation,activation_slot,run_index,iteration_index,status,capability,side_effect_level,input_json,loop_frame_json,started_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,IF(?='ready',NULL,UTC_TIMESTAMP(6))) ON DUPLICATE KEY UPDATE status=VALUES(status),input_json=VALUES(input_json),loop_frame_json=VALUES(loop_frame_json),started_at=IF(started_at IS NULL AND VALUES(status)<>'ready',UTC_TIMESTAMP(6),started_at),updated_at=UTC_TIMESTAMP(6)",
    )
    .bind(activation.id.as_uuid())
    .bind(tenant_id)
    .bind(execution_id)
    .bind(&node.id)
    .bind(&node.key)
    .bind(&node.name)
    .bind(&node.node_type)
    .bind(node.type_version)
    .bind(activation.generation)
    .bind(activation.slot)
    .bind(activation.run_index)
    .bind(0_u32)
    .bind(status)
    .bind(node.capability.as_str())
    .bind(side_effect_name(&node.side_effect_level))
    .bind(serde_json::to_value(&activation.inputs).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(activation.loop_frame.clone())
    .bind(status)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn upsert_failed_activation(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    activation: &agentx_runtime::NodeActivation,
    node: &agentx_runtime::CompiledNode,
    error_code: &str,
    error_message: &str,
) -> RuntimeResult<()> {
    upsert_activation(tx, tenant_id, execution_id, activation, node).await?;
    let updated = sqlx::query(
        "UPDATE node_executions SET error_code=?,error_message=?,ended_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND execution_id=? AND id=? AND status='failed'",
    )
    .bind(error_code)
    .bind(error_message)
    .bind(tenant_id)
    .bind(execution_id)
    .bind(activation.id.as_uuid())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(RuntimeError::Internal(anyhow::anyhow!(
            "failed activation was not persisted"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn finish_execution(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    work_package_id: Option<Uuid>,
    invocation_id: Option<Uuid>,
    state_version: u64,
    machine: &ExecutionMachine,
    context: &Value,
) -> RuntimeResult<()> {
    let (status, output, error, string_conversions) =
        match materialize_result(tx, tenant_id, execution_id, machine, context).await {
            Ok((output, error, conversions)) => {
                (machine_status(machine.status()), output, error, conversions)
            }
            // Definition/value errors are terminal workflow data. Storage errors
            // must still abort the transaction so recovery can retry them.
            Err(RuntimeError::Deterministic { code, message }) => (
                "failed",
                json!({}),
                Some(json!({"code":code,"message":message})),
                Vec::new(),
            ),
            Err(
                RuntimeError::BadRequest(_, message) | RuntimeError::InvalidRequest(_, message),
            ) => (
                "failed",
                json!({}),
                Some(json!({
                    "code":"END_OUTPUT_EVALUATION_FAILED",
                    "message":message,
                })),
                Vec::new(),
            ),
            Err(error) => return Err(error),
        };
    super::engine::string_conversion_trace::enqueue_end_records(
        tx,
        tenant_id,
        execution_id,
        &string_conversions,
    )
    .await;
    let error_code = error
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str);
    let error_message = error
        .as_ref()
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str);
    let result_hash = agentx_runtime_contracts::content_hash(&output)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query(
        "UPDATE workflow_executions SET status=?,state_version=?,output_json=?,error_code=?,error_message=?,error_json=?,terminal_result_json=?,terminal_result_hash=?,ended_at=UTC_TIMESTAMP(6),duration_ms=TIMESTAMPDIFF(MICROSECOND,started_at,UTC_TIMESTAMP(6))/1000 WHERE tenant_id=? AND id=?",
    )
    .bind(status)
    .bind(state_version)
    .bind(&output)
    .bind(error_code)
    .bind(error_message)
    .bind(&error)
    .bind(&output)
    .bind(result_hash.as_str())
    .bind(tenant_id)
    .bind(execution_id)
    .execute(&mut **tx)
    .await?;
    let approval_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM approval_tasks WHERE tenant_id=? AND execution_id=? AND status IN ('pending','claimed') FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .fetch_all(&mut **tx)
    .await?;
    if !approval_ids.is_empty() {
        sqlx::query("UPDATE approval_tasks SET status='cancelled',version=version+1,resume_status='not_requested' WHERE tenant_id=? AND execution_id=? AND status IN ('pending','claimed')")
            .bind(tenant_id).bind(execution_id).execute(&mut **tx).await?;
        for task_id in approval_ids {
            crate::event_export::enqueue_approval_event_from_task(tx, tenant_id, task_id).await?;
        }
    }
    let child_ids = sqlx::query_scalar::<_, Uuid>("SELECT child_execution_id FROM execution_children WHERE tenant_id=? AND parent_execution_id=? AND relationship='composite' AND merge_status='pending'")
        .bind(tenant_id).bind(execution_id).fetch_all(&mut **tx).await?;
    for child_id in child_ids {
        let command_id = agentx_runtime_contracts::deterministic_uuid(
            execution_id,
            format!("terminal-cancel:{child_id}").as_bytes(),
        );
        sqlx::query("INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'cancel_execution','execution',?,?,?,'pending') ON DUPLICATE KEY UPDATE id=id")
            .bind(command_id).bind(tenant_id).bind(child_id.to_string())
            .bind(format!("parent-terminal:{execution_id}:{child_id}"))
            .bind(json!({"reason":"parent_terminal","parentExecutionId":execution_id}))
            .execute(&mut **tx).await?;
    }
    let invocation_status = if status == "succeeded" {
        "completed"
    } else if machine.status() == RuntimeExecutionStatus::Cancelled {
        "cancelled"
    } else {
        "failed"
    };
    if invocation_id.is_some() {
        sqlx::query(
            "UPDATE application_invocations SET status=?,state_version=state_version+1,result_json=?,error_json=?,completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND execution_id=?",
        )
        .bind(invocation_status)
        .bind(&output)
        .bind(&error)
        .bind(tenant_id)
        .bind(execution_id)
        .execute(&mut **tx)
        .await?;
    }
    if let Some(invocation_id) = invocation_id.filter(|_| invocation_status == "completed") {
        append_session_assistant_message(tx, tenant_id, invocation_id, &output).await?;
        enqueue_terminal_reply(tx, tenant_id, execution_id, invocation_id, &output).await?;
    }
    sqlx::query(
        "UPDATE execution_snapshots SET output_json=?,state_version=? WHERE tenant_id=? AND execution_id=?",
    )
    .bind(&output)
    .bind(state_version)
    .bind(tenant_id)
    .bind(execution_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE execution_runtime_state SET terminal_result_json=?,terminal_result_hash=? WHERE tenant_id=? AND execution_id=?",
    )
    .bind(&output)
    .bind(result_hash.as_str())
    .bind(tenant_id)
    .bind(execution_id)
    .execute(&mut **tx)
    .await?;
    crate::work_package_execution::converge_execution(
        tx,
        tenant_id,
        execution_id,
        work_package_id,
        invocation_status,
        &output,
        &error,
        result_hash.as_str(),
    )
    .await?;
    crate::composite_execution::converge_child(
        tx,
        tenant_id,
        execution_id,
        invocation_status,
        &output,
        &error,
        context,
    )
    .await?;
    sqlx::query(
        "UPDATE bundle_references SET released_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND reference_kind='active_execution' AND owner_id=? AND released_at IS NULL",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .execute(&mut **tx)
    .await?;
    crate::quota::release_execution(tx, tenant_id, execution_id, "execution_terminal").await?;
    if let Some(invocation_id) = invocation_id {
        insert_invocation_event(
            tx,
            tenant_id,
            invocation_id,
            if invocation_status == "completed" {
                "invocation.completed"
            } else {
                "invocation.failed"
            },
            json!({"executionId":execution_id,"status":invocation_status,"outputs":output,"error":error}),
        )
        .await?;
    }
    sqlx::query(
        "INSERT INTO execution_outbox(id,tenant_id,execution_id,message_type,payload_json,status) VALUES(?,?,?,'runtime_event',?,'pending')",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(execution_id)
    .bind(json!({"type":format!("execution.{status}"),"resultHash":result_hash}))
    .execute(&mut **tx)
    .await?;
    let mut trace = crate::trace_delivery::TraceDraft::execution(
        tenant_id,
        execution_id,
        format!("execution.{status}"),
        status,
    );
    trace.error_code = error
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    trace.error_message = error
        .as_ref()
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    trace.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::WorkflowOutput);
    trace.content_preview = crate::trace_delivery::bounded_preview(&output);
    let mut end = crate::trace_delivery::TraceDraft::span(
        tenant_id,
        execution_id,
        crate::trace_delivery::boundary_entity_id(execution_id, "end"),
        Some((
            execution_id,
            agentx_runtime_contracts::TraceSpanKindV1::Execution,
        )),
        agentx_runtime_contracts::TraceSpanKindV1::Boundary,
        "End",
        agentx_runtime_contracts::TraceEventKindV1::Finished,
        "boundary.end.finished",
        status,
    );
    end.error_code = trace.error_code.clone();
    end.error_message = trace.error_message.clone();
    end.content_kind = Some(agentx_runtime_contracts::TraceContentKindV1::WorkflowOutput);
    end.content_preview = crate::trace_delivery::bounded_preview(&output);
    crate::trace_delivery::enqueue_best_effort(tx, end).await;
    crate::trace_delivery::enqueue_best_effort(tx, trace).await;
    Ok(())
}

async fn append_session_assistant_message(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    invocation_id: Uuid,
    output: &Value,
) -> RuntimeResult<()> {
    let invocation = sqlx::query(
        "SELECT session_id,chat_mapping_json FROM application_invocations WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(invocation_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(invocation) = invocation else {
        return Ok(());
    };
    let session_id: Option<Uuid> = invocation.try_get("session_id")?;
    let Some(session_id) = session_id else {
        return Ok(());
    };
    let mapping = invocation
        .try_get::<Option<Value>, _>("chat_mapping_json")?
        .map(serde_json::from_value::<agentx_runtime_contracts::ChatMappingV1>)
        .transpose()
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let Some(mapping) = mapping else {
        return Ok(());
    };
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM application_messages WHERE tenant_id=? AND session_id=? AND invocation_id=? AND role='assistant')",
    )
    .bind(tenant_id)
    .bind(session_id)
    .bind(invocation_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        return Ok(());
    }
    let sequence: u64 = sqlx::query_scalar(
        "SELECT next_message_sequence FROM application_sessions WHERE tenant_id=? AND id=? AND status='active' FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(session_id)
    .fetch_one(&mut **tx)
    .await?;
    let message_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO application_messages(id,tenant_id,session_id,invocation_id,sequence_number,role) VALUES(?,?,?,?,?,'assistant')",
    )
    .bind(message_id)
    .bind(tenant_id)
    .bind(session_id)
    .bind(invocation_id)
    .bind(sequence)
    .execute(&mut **tx)
    .await?;
    let parts = crate::assistant_message::assistant_message_parts(output, &mapping)?;
    for (index, part) in parts.into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO application_message_parts(id,tenant_id,message_id,part_index,part_type,content_json,artifact_id) VALUES(?,?,?,?,?,?,?)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind(message_id)
        .bind(index as u32)
        .bind(part.part_type)
        .bind(part.content)
        .bind(part.artifact_id)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(
        "UPDATE application_sessions SET next_message_sequence=next_message_sequence+1,version=version+1 WHERE tenant_id=? AND id=?",
    )
    .bind(tenant_id)
    .bind(session_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn materialize_result(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    machine: &ExecutionMachine,
    context: &Value,
) -> RuntimeResult<(
    Value,
    Option<Value>,
    Vec<agentx_runtime::StringConversionRecord>,
)> {
    let execution =
        sqlx::query("SELECT input_json FROM workflow_executions WHERE tenant_id=? AND id=?")
            .bind(tenant_id)
            .bind(execution_id)
            .fetch_one(&mut **tx)
            .await?;
    let outputs = load_output_namespace(tx, tenant_id, execution_id).await?;
    let expression_context = ExpressionContext {
        inputs: execution
            .try_get::<Option<Value>, _>("input_json")?
            .unwrap_or(Value::Null),
        outputs,
        contexts: context.clone(),
        execution: crate::execution_context::load(tx, tenant_id, execution_id).await?,
        output_node_keys: machine
            .workflow()
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node.key.clone()))
            .collect(),
        ..ExpressionContext::default()
    };
    let engine = ExpressionEngine;
    let mut string_conversions = Vec::new();
    let workflow = machine.workflow();
    if machine.status() != RuntimeExecutionStatus::Succeeded {
        let error_deliveries = machine
            .end_deliveries()
            .iter()
            .filter(|delivery| delivery.target_port == "error")
            .collect::<Vec<_>>();
        let mut errors = error_deliveries
            .iter()
            .flat_map(|delivery| &delivery.items)
            .map(|item| item.json.clone())
            .collect::<Vec<_>>();
        if errors.is_empty() {
            let rows = sqlx::query("SELECT error_code,error_message,node_id,id FROM node_executions WHERE tenant_id=? AND execution_id=? AND error_code IS NOT NULL ORDER BY ended_at,id")
                .bind(tenant_id).bind(execution_id).fetch_all(&mut **tx).await?;
            errors.extend(rows.into_iter().map(|row| {
                json!({
                    "code":row.try_get::<String,_>("error_code").unwrap_or_else(|_| "WORKFLOW_FAILED".into()),
                    "message":row.try_get::<Option<String>,_>("error_message").ok().flatten().unwrap_or_else(|| "Workflow did not succeed".into()),
                    "sourceNodeId":row.try_get::<String,_>("node_id").ok(),
                    "nodeExecutionId":row.try_get::<Uuid,_>("id").ok()
                })
            }));
        }
        let primary = errors.first().cloned().unwrap_or_else(
            || json!({"code":"WORKFLOW_FAILED","message":"Workflow did not succeed"}),
        );
        let error_exit = error_deliveries
            .first()
            .and_then(|delivery| workflow.exits.get(&delivery.target_exit))
            .or_else(|| workflow.exits.values().next());
        let mut error_context = expression_context.clone();
        error_context.json = primary.clone();
        let mut error_outputs = Map::new();
        if let Some(exit) = error_exit {
            let exit_id = error_deliveries
                .first()
                .map_or_else(String::new, |delivery| delivery.target_exit.clone());
            for (name, dynamic) in &exit.error_outputs {
                let contract = workflow.end.error.outputs.get(name);
                let schema = contract.map_or_else(|| json!({}), |output| output.schema.clone());
                let (value, mut conversions) = engine
                    .resolve_input_with_schema(
                        dynamic,
                        &schema,
                        &error_context,
                        format!("exit.{exit_id}.errorOutputs.{name}"),
                    )
                    .map_err(|error| RuntimeError::Deterministic {
                        code: "END_OUTPUT_EVALUATION_FAILED",
                        message: error.to_string(),
                    })?;
                string_conversions.append(&mut conversions);
                let required = contract.is_some_and(|output| output.required);
                let Some(value) = value else {
                    if required {
                        return Err(RuntimeError::Deterministic {
                            code: "REQUIRED_END_ERROR_OUTPUT_OMITTED",
                            message: format!("required End error output {name} was omitted"),
                        });
                    }
                    continue;
                };
                if required && value.is_null() {
                    return Err(runtime_bad_request(
                        "REQUIRED_END_ERROR_OUTPUT_NULL",
                        &format!("required End error output {name} resolved to null"),
                    ));
                }
                if let Some(contract) = contract {
                    jsonschema::validator_for(&contract.schema)
                        .map_err(|error| RuntimeError::Deterministic {
                            code: "END_OUTPUT_SCHEMA_INVALID",
                            message: error.to_string(),
                        })?
                        .validate(&value)
                        .map_err(|error| RuntimeError::Deterministic {
                            code: "END_OUTPUT_SCHEMA_VALIDATION_FAILED",
                            message: error.to_string(),
                        })?;
                }
                error_outputs.insert(name.clone(), value);
            }
        }
        let code = primary
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("WORKFLOW_FAILED");
        let message = primary
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Workflow did not succeed");
        return Ok((
            json!({}),
            Some(json!({
                "code":code,
                "message":message,
                "primaryError":primary,
                "errors":errors,
                "outputs":error_outputs
            })),
            string_conversions,
        ));
    }
    let main_deliveries = machine
        .end_deliveries()
        .iter()
        .filter(|delivery| delivery.target_port == "main")
        .collect::<Vec<_>>();
    let all_complete = workflow.end.completion == agentx_domain::WorkflowCompletion::AllComplete;
    // Exit evaluation order follows the Definition's exit order so the
    // assembled output is deterministic regardless of delivery timing.
    let reached_exits = if all_complete {
        workflow
            .exit_order
            .iter()
            .filter(|exit_id| {
                main_deliveries
                    .iter()
                    .any(|delivery| &delivery.target_exit == *exit_id)
            })
            .filter_map(|exit_id| {
                let delivery = main_deliveries
                    .iter()
                    .find(|delivery| &delivery.target_exit == exit_id)?;
                workflow
                    .exits
                    .get(exit_id)
                    .map(|exit| (exit_id.clone(), exit, Some(*delivery)))
            })
            .collect::<Vec<_>>()
    } else {
        main_deliveries
            .first()
            .and_then(|delivery| {
                workflow
                    .exits
                    .get_key_value(&delivery.target_exit)
                    .map(|(exit_id, exit)| (exit_id.clone(), exit, Some(*delivery)))
            })
            .or_else(|| {
                workflow.start_to_exit.as_ref().and_then(|exit_id| {
                    workflow
                        .exits
                        .get_key_value(exit_id)
                        .map(|(exit_id, exit)| (exit_id.clone(), exit, None))
                })
            })
            .into_iter()
            .collect::<Vec<_>>()
    };
    let mut result = Map::new();
    for (exit_id, exit, delivery) in reached_exits {
        let mut exit_context = expression_context.clone();
        if let Some(item) = delivery.and_then(|delivery| delivery.items.last()) {
            exit_context.json = item.json.clone();
            exit_context.input = item.json.clone();
        }
        for (name, contract) in &workflow.end.outputs {
            let (value, mut conversions) = if let Some(dynamic) = exit.outputs.get(name) {
                engine
                    .resolve_input_with_schema(
                        dynamic,
                        &contract.schema,
                        &exit_context,
                        format!("exit.{exit_id}.outputs.{name}"),
                    )
                    .map_err(|error| RuntimeError::Deterministic {
                        code: "END_OUTPUT_EVALUATION_FAILED",
                        message: error.to_string(),
                    })?
            } else {
                (None, Vec::new())
            };
            string_conversions.append(&mut conversions);
            let required = contract.required;
            let Some(value) = value else {
                if required {
                    return Err(RuntimeError::Deterministic {
                        code: "REQUIRED_END_OUTPUT_OMITTED",
                        message: format!("required End output {name} was omitted"),
                    });
                }
                if all_complete {
                    match result.entry(name.clone()) {
                        serde_json::map::Entry::Vacant(slot) => {
                            slot.insert(Value::Array(vec![Value::Null]));
                        }
                        serde_json::map::Entry::Occupied(mut slot) => {
                            if let Value::Array(elements) = slot.get_mut() {
                                elements.push(Value::Null);
                            }
                        }
                    }
                }
                continue;
            };
            if required && value.is_null() {
                return Err(runtime_bad_request(
                    "REQUIRED_END_OUTPUT_NULL",
                    &format!("required End output {name} resolved to null"),
                ));
            }
            if !value.is_null() {
                jsonschema::validator_for(&contract.schema)
                    .map_err(|error| RuntimeError::Deterministic {
                        code: "END_OUTPUT_SCHEMA_INVALID",
                        message: error.to_string(),
                    })?
                    .validate(&value)
                    .map_err(|error| RuntimeError::Deterministic {
                        code: "END_OUTPUT_SCHEMA_VALIDATION_FAILED",
                        message: error.to_string(),
                    })?;
            }
            match result.entry(name.clone()) {
                serde_json::map::Entry::Vacant(slot) => {
                    // all_complete wraps every output field in an array with
                    // one element per reached exit (definition order).
                    slot.insert(if all_complete {
                        Value::Array(vec![value])
                    } else {
                        value
                    });
                }
                serde_json::map::Entry::Occupied(mut slot) => {
                    if let Value::Array(elements) = slot.get_mut() {
                        elements.push(value);
                    }
                }
            }
        }
    }
    if result.is_empty()
        && !all_complete
        && let Some(item) = main_deliveries
            .iter()
            .flat_map(|delivery| &delivery.items)
            .next()
    {
        return Ok((item.json.clone(), None, string_conversions));
    }
    Ok((Value::Object(result), None, string_conversions))
}

pub(super) async fn load_output_namespace(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
) -> RuntimeResult<Value> {
    let rows = sqlx::query(
        "SELECT node_key,run_index,output_json FROM node_executions WHERE tenant_id=? AND execution_id=? AND output_json IS NOT NULL ORDER BY run_index,id",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut namespace = Map::new();
    for row in rows {
        merge_output_namespace(
            &mut namespace,
            &row.try_get::<String, _>("node_key")?,
            row.try_get("run_index")?,
            &row.try_get::<Value, _>("output_json")?,
        );
    }
    let deliveries = sqlx::query(
        "SELECT n.node_key,n.run_index,d.source_port,d.payload_json FROM execution_edge_deliveries d JOIN node_executions n ON n.tenant_id=d.tenant_id AND n.execution_id=d.execution_id AND n.id=d.source_node_execution_id WHERE d.tenant_id=? AND d.execution_id=? AND d.payload_json IS NOT NULL UNION ALL SELECT n.node_key,n.run_index,d.source_port,d.payload_json FROM execution_end_deliveries d JOIN node_executions n ON n.tenant_id=d.tenant_id AND n.execution_id=d.execution_id AND n.id=d.source_node_execution_id WHERE d.tenant_id=? AND d.execution_id=?",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .bind(tenant_id)
    .bind(execution_id)
    .fetch_all(&mut **tx)
    .await?;
    for delivery in deliveries {
        let source_port: String = delivery.try_get("source_port")?;
        let payload: Value = delivery.try_get("payload_json")?;
        merge_output_namespace(
            &mut namespace,
            &delivery.try_get::<String, _>("node_key")?,
            delivery.try_get("run_index")?,
            &json!({source_port:payload}),
        );
    }
    Ok(Value::Object(namespace))
}

pub(super) fn merge_output_namespace(
    namespace: &mut Map<String, Value>,
    key: &str,
    run: u32,
    outputs: &Value,
) {
    let node = namespace
        .entry(key.to_owned())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("node output namespace is an object");
    node.entry("runs")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("node runs namespace is an object")
        .insert(run.to_string(), outputs.clone());
    let Some(ports) = outputs.as_object() else {
        return;
    };
    for (port, value) in ports {
        let items = value.as_array().cloned().unwrap_or_default();
        let first = items.first().cloned().unwrap_or(Value::Null);
        let last = items.last().cloned().unwrap_or(Value::Null);
        node.insert(
            port.clone(),
            json!({"current":first,"first":first,"last":last,"all":items,"runIndex":run}),
        );
        if port == "main"
            && let Some(fields) = first.get("json").and_then(Value::as_object)
        {
            for (field, value) in fields {
                if !matches!(field.as_str(), "runs" | "main" | "error") {
                    node.insert(field.clone(), value.clone());
                }
            }
        }
    }
}

pub(super) fn single_port_output(port: &str, payload: Value) -> Value {
    let mut outputs = Map::new();
    outputs.insert(port.to_owned(), json!([{"json": payload}]));
    Value::Object(outputs)
}

pub(super) async fn authorize_snapshot(
    tx: &mut Transaction<'_, MySql>,
    authorization: &RuntimeAuthorizationSnapshotV1,
) -> RuntimeResult<()> {
    authorize_identity_snapshot(tx, authorization).await?;
    for grant_id in &authorization.grant_ids {
        let grant_ok: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM resource_grant_projection WHERE tenant_id=? AND grant_id=? AND subject_id=? AND status='active' AND policy_epoch>=?)",
        )
        .bind(authorization.tenant_id)
        .bind(grant_id)
        .bind(authorization.service_identity_id)
        .bind(authorization.policy_epoch)
        .fetch_one(&mut **tx)
        .await?;
        if !grant_ok {
            return Err(runtime_bad_request(
                "RUNTIME_GRANT_REVOKED",
                "Runtime grant is missing or revoked",
            ));
        }
    }
    Ok(())
}

pub(super) async fn authorize_identity_snapshot(
    tx: &mut Transaction<'_, MySql>,
    authorization: &RuntimeAuthorizationSnapshotV1,
) -> RuntimeResult<()> {
    if authorization.maximum_policy_staleness_seconds
        > agentx_runtime_contracts::MAX_POLICY_STALENESS_SECONDS
    {
        return Err(runtime_bad_request(
            "POLICY_STALENESS_EXCEEDED",
            "Policy Last Known Good window exceeds 72 hours",
        ));
    }
    let identity_ok: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM service_identity_projection i JOIN tenant_admission t ON t.tenant_id=i.tenant_id WHERE i.tenant_id=? AND i.identity_id=? AND i.workflow_id=? AND i.status='active' AND i.policy_epoch>=? AND t.status='active' AND i.updated_at>=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL ? SECOND))",
    )
    .bind(authorization.tenant_id)
    .bind(authorization.service_identity_id)
    .bind(authorization.workflow_id)
    .bind(authorization.policy_epoch)
    .bind(authorization.maximum_policy_staleness_seconds)
    .fetch_one(&mut **tx)
    .await?;
    if !identity_ok {
        return Err(runtime_bad_request(
            "RUNTIME_AUTHORIZATION_STALE",
            "Runtime identity state is revoked, missing, or outside the Last Known Good window",
        ));
    }
    Ok(())
}

pub(super) async fn authorize_resources(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    resources: &[RuntimeResourceBindingV1],
) -> RuntimeResult<()> {
    for resource in resources {
        let resource_kind = serde_json::to_value(resource.resource_kind)
            .map_err(|error| RuntimeError::Internal(error.into()))?
            .as_str()
            .ok_or_else(|| {
                RuntimeError::Internal(anyhow::anyhow!("resource kind is not a string"))
            })?
            .to_owned();
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_resource_states WHERE tenant_id=? AND resource_kind=? AND resource_id=? AND resource_version=? AND state_epoch>=? AND status='active' AND content_hash=?)",
        )
        .bind(tenant_id)
        .bind(resource_kind)
        .bind(resource.resource_id)
        .bind(&resource.resource_version)
        .bind(resource.state_epoch)
        .bind(resource.content_hash.as_str())
        .fetch_one(&mut **tx)
        .await?;
        if !valid {
            return Err(runtime_bad_request(
                "RUNTIME_RESOURCE_DISABLED",
                "Runtime resource state is missing, stale, revoked, or does not match the immutable binding",
            ));
        }
    }
    Ok(())
}

pub(super) fn policy_timeout_from_snapshot(row: &sqlx::mysql::MySqlRow) -> RuntimeResult<u32> {
    let policy: agentx_runtime_contracts::RuntimePolicyV1 =
        serde_json::from_value(row.try_get("policy_snapshot_json")?)
            .map_err(|error| RuntimeError::Internal(error.into()))?;
    Ok(policy.timeout_seconds)
}

pub(super) async fn persist_lineage(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    delivery_id: Uuid,
    items: &[Item],
) -> RuntimeResult<()> {
    let mut batch = Vec::with_capacity(256);
    for (index, item) in items.iter().enumerate() {
        for source in &item.lineage {
            batch.push((
                index as u32,
                source.node_execution_id.as_uuid(),
                source.run_index,
                source.output_index,
                source.item_index,
            ));
            if batch.len() == 256 {
                persist_lineage_batch(tx, tenant_id, execution_id, delivery_id, &batch).await?;
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        persist_lineage_batch(tx, tenant_id, execution_id, delivery_id, &batch).await?;
    }
    Ok(())
}

async fn persist_lineage_batch(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    delivery_id: Uuid,
    rows: &[(u32, Uuid, u32, u32, u32)],
) -> RuntimeResult<()> {
    let mut query = QueryBuilder::<MySql>::new(
        "INSERT IGNORE INTO item_lineage(tenant_id,execution_id,delivery_id,target_item_index,source_node_execution_id,source_run_index,source_output_index,source_item_index) ",
    );
    query.push_values(
        rows.iter().copied(),
        |mut row, (target, source, run, output, item)| {
            row.push_bind(tenant_id)
                .push_bind(execution_id)
                .push_bind(delivery_id)
                .push_bind(target)
                .push_bind(source)
                .push_bind(run)
                .push_bind(output)
                .push_bind(item);
        },
    );
    query.build().execute(&mut **tx).await?;
    Ok(())
}

pub(super) async fn insert_invocation_event(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    invocation_id: Uuid,
    event_type: &str,
    payload: Value,
) -> RuntimeResult<()> {
    // Every writer (model deltas, terminal state and delivery) serializes on
    // the same parent row before allocating the next SSE cursor.
    sqlx::query("SELECT id FROM application_invocations WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(tenant_id)
        .bind(invocation_id)
        .fetch_one(&mut **tx)
        .await?;
    let next: u64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(MAX(sequence_number),0)+1 AS UNSIGNED) FROM invocation_events WHERE tenant_id=? AND invocation_id=? FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(invocation_id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO invocation_events(tenant_id,invocation_id,event_id,sequence_number,event_type,payload_json) VALUES(?,?,?,?,?,?)",
    )
    .bind(tenant_id)
    .bind(invocation_id)
    .bind(Uuid::now_v7())
    .bind(next)
    .bind(event_type)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Enqueues the L1 channel auto reply in the execution terminal transaction:
/// resolves the triggering webhook binding from the invocation's frozen
/// trigger context, renders the reply payload from the frozen output and
/// inserts an idempotent delivery_outbox row (plan7 P7-A).
pub(super) async fn enqueue_terminal_reply(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    invocation_id: Uuid,
    output: &Value,
) -> RuntimeResult<()> {
    let invocation = sqlx::query(
        "SELECT application_id,trigger_context_json FROM application_invocations WHERE tenant_id=? AND id=?",
    )
    .bind(tenant_id)
    .bind(invocation_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(invocation) = invocation else {
        return Ok(());
    };
    let application_id: Uuid = invocation.try_get("application_id")?;
    let context: Option<Value> = invocation.try_get("trigger_context_json")?;
    let Some(context) = context else {
        return Ok(());
    };
    let Some(binding_id) = context
        .get("webhookTriggerId")
        .map(|value| value.as_str().map(str::to_owned).unwrap_or_default())
    else {
        return Ok(());
    };
    let Ok(binding_id) = uuid::Uuid::parse_str(&binding_id) else {
        return Ok(());
    };
    let binding = sqlx::query(
        "SELECT provider_type,reply_config_json,secret_ref_json FROM webhook_bindings WHERE tenant_id=? AND id=? AND status='active'",
    )
    .bind(tenant_id)
    .bind(binding_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(binding) = binding else {
        return Ok(());
    };
    let reply: Option<agentx_runtime_contracts::WebhookReplyConfigV1> = binding
        .try_get::<Option<Value>, _>("reply_config_json")?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let Some(reply) = reply.filter(|reply| reply.enabled) else {
        return Ok(());
    };
    let field = output.get(&reply.output_field);
    let Some(field) = field else {
        tracing::warn!(%execution_id, field = %reply.output_field, "reply output field missing; skipping delivery");
        return Ok(());
    };
    let text = render_reply_text(&reply, output, field);
    let payload = json!({"text": text});
    let target = json!({
        "conversationId": context.get("conversation").and_then(|value| value.get("id")).and_then(Value::as_str),
        "conversationType": context.get("conversation").and_then(|value| value.get("conversationType")).and_then(Value::as_str),
        "senderId": context.get("sender").and_then(|value| value.get("id")).and_then(Value::as_str),
        "sessionWebhook": context.get("sessionWebhook").and_then(Value::as_str),
        "sessionWebhookExpiresAt": context.get("sessionWebhookExpiresAt").and_then(Value::as_i64),
    });
    let provider: String = binding.try_get::<String, _>("provider_type")?;
    let credential_ref: Option<Value> = binding.try_get("secret_ref_json")?;
    crate::delivery::enqueue(
        tx,
        &crate::delivery::DeliveryEnqueue {
            id: agentx_runtime_contracts::deterministic_uuid(execution_id, b"delivery:terminal"),
            tenant_id,
            application_id,
            invocation_id: Some(invocation_id),
            execution_id,
            channel_binding_id: binding_id,
            provider,
            origin: "terminal".into(),
            target,
            credential_ref,
            payload,
        },
    )
    .await
}

/// Enqueues the L2/L3 node delivery in the worker attempt settlement
/// transaction (plan7 P7-A). The builtin node rendered the intent into its
/// main output item; this resolves the channel binding and inserts the
/// delivery row. Resolution failures surface as deterministic node failures
/// (REPLY_TARGET_UNRESOLVED / SEND_CHANNEL_UNRESOLVED).
pub(super) struct NodeDeliveryIntent<'a> {
    pub tenant_id: Uuid,
    pub execution_id: Uuid,
    pub invocation_id: Option<Uuid>,
    pub node_execution_id: Uuid,
    pub node_type: &'a str,
    pub intent: &'a Value,
}

pub(super) async fn enqueue_node_delivery(
    tx: &mut Transaction<'_, MySql>,
    delivery: &NodeDeliveryIntent<'_>,
) -> RuntimeResult<Result<(), (&'static str, String)>> {
    let NodeDeliveryIntent {
        tenant_id,
        execution_id,
        invocation_id,
        node_execution_id,
        node_type,
        intent,
    } = *delivery;
    let content = intent
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if content.is_empty() {
        return Ok(Err((
            "DELIVERY_PAYLOAD_INVALID",
            "reply/send content resolved to empty text".into(),
        )));
    }
    let origin = format!("node:{node_execution_id}:0");
    let delivery_id = agentx_runtime_contracts::deterministic_uuid(execution_id, origin.as_bytes());
    let payload = json!({"text": content});
    let (binding_id, application_id, provider, credential_ref, target) = if node_type
        == "reply_message"
    {
        let Some(invocation_id) = invocation_id else {
            return Ok(Err((
                "REPLY_TARGET_UNRESOLVED",
                "execution has no invocation to derive a reply target from".into(),
            )));
        };
        let invocation = sqlx::query(
            "SELECT trigger_context_json FROM application_invocations WHERE tenant_id=? AND id=?",
        )
        .bind(tenant_id)
        .bind(invocation_id)
        .fetch_optional(&mut **tx)
        .await?;
        let context: Option<Value> = match invocation {
            Some(row) => row
                .try_get::<Option<Value>, _>("trigger_context_json")
                .map_err(RuntimeError::from)?,
            None => None,
        };
        let Some(context) = context else {
            return Ok(Err((
                "REPLY_TARGET_UNRESOLVED",
                "invocation carries no trigger context".into(),
            )));
        };
        let Some(binding_value) = context.get("webhookTriggerId").and_then(Value::as_str) else {
            return Ok(Err((
                "REPLY_TARGET_UNRESOLVED",
                "trigger context carries no webhook trigger id".into(),
            )));
        };
        let Ok(binding_id) = Uuid::parse_str(binding_value) else {
            return Ok(Err((
                "REPLY_TARGET_UNRESOLVED",
                "trigger context webhook trigger id is not a UUID".into(),
            )));
        };
        let binding = sqlx::query(
            "SELECT application_id,provider_type,secret_ref_json FROM webhook_bindings WHERE tenant_id=? AND id=? AND status='active'",
        )
        .bind(tenant_id)
        .bind(binding_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(binding) = binding else {
            return Ok(Err((
                "REPLY_TARGET_UNRESOLVED",
                "source channel binding is not active".into(),
            )));
        };
        let target = json!({
            "conversationId": context.pointer("/conversation/id").and_then(Value::as_str),
            "conversationType": context.pointer("/conversation/conversationType").and_then(Value::as_str),
            "senderId": context.pointer("/sender/id").and_then(Value::as_str),
            "sessionWebhook": context.get("sessionWebhook").and_then(Value::as_str),
            "sessionWebhookExpiresAt": context.get("sessionWebhookExpiresAt").and_then(Value::as_i64),
        });
        if target
            .get("conversationId")
            .and_then(Value::as_str)
            .is_none()
            && target.get("senderId").and_then(Value::as_str).is_none()
        {
            return Ok(Err((
                "REPLY_TARGET_UNRESOLVED",
                "trigger context carries no conversation or sender".into(),
            )));
        }
        (
            binding_id,
            binding.try_get("application_id")?,
            binding.try_get::<String, _>("provider_type")?,
            binding.try_get("secret_ref_json")?,
            target,
        )
    } else {
        let Some(channel) = intent.get("channelId").and_then(Value::as_str) else {
            return Ok(Err((
                "SEND_CHANNEL_UNRESOLVED",
                "send_message has no channelId".into(),
            )));
        };
        let Ok(binding_id) = Uuid::parse_str(channel) else {
            return Ok(Err((
                "SEND_CHANNEL_UNRESOLVED",
                "send_message channelId is not a channel id".into(),
            )));
        };
        let binding = sqlx::query(
            "SELECT application_id,provider_type,secret_ref_json FROM webhook_bindings WHERE tenant_id=? AND id=? AND status='active'",
        )
        .bind(tenant_id)
        .bind(binding_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(binding) = binding else {
            return Ok(Err((
                "SEND_CHANNEL_UNRESOLVED",
                "send_message channel binding is not active".into(),
            )));
        };
        let target = json!({
            "conversationId": intent.get("targetConversationId").and_then(Value::as_str),
            "conversationType": if intent.get("targetConversationId").is_some() { Some("group") } else { None },
            "senderId": intent.get("senderId").and_then(Value::as_str),
        });
        if target
            .get("conversationId")
            .and_then(Value::as_str)
            .is_none()
            && target.get("senderId").and_then(Value::as_str).is_none()
        {
            return Ok(Err((
                "SEND_CHANNEL_UNRESOLVED",
                "send_message carries neither conversation nor sender target".into(),
            )));
        }
        (
            binding_id,
            binding.try_get("application_id")?,
            binding.try_get::<String, _>("provider_type")?,
            binding.try_get("secret_ref_json")?,
            target,
        )
    };
    crate::delivery::enqueue(
        tx,
        &crate::delivery::DeliveryEnqueue {
            id: delivery_id,
            tenant_id,
            application_id,
            invocation_id,
            execution_id,
            channel_binding_id: binding_id,
            provider,
            origin,
            target,
            credential_ref,
            payload,
        },
    )
    .await?;
    Ok(Ok(()))
}

/// Renders the reply text: with a template, only `{{output.<field>}}`
/// references are substituted; without one the raw field value is stringified.
fn render_reply_text(
    reply: &agentx_runtime_contracts::WebhookReplyConfigV1,
    output: &Value,
    field: &Value,
) -> String {
    match reply.template.as_deref() {
        Some(template) if template.contains("{{") => {
            let mut rendered = template.to_owned();
            for (name, value) in output.as_object().into_iter().flatten() {
                let placeholder = format!("{{{{output.{name}}}}}");
                if rendered.contains(&placeholder) {
                    let replacement = match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    rendered = rendered.replace(&placeholder, &replacement);
                }
            }
            rendered
        }
        _ => match field {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        },
    }
}

pub(super) async fn initial_context_for_execution(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    workflow: &agentx_runtime::CompiledWorkflow,
) -> RuntimeResult<(Value, u64)> {
    let mut context = workflow
        .contexts
        .iter()
        .map(|(name, definition)| (name.clone(), definition.default.clone()))
        .collect::<Map<_, _>>();
    let session = sqlx::query("SELECT s.id,s.application_deployment_id FROM workflow_executions e JOIN application_invocations i ON i.tenant_id=e.tenant_id AND i.id=e.invocation_id JOIN application_sessions s ON s.tenant_id=i.tenant_id AND s.id=i.session_id WHERE e.tenant_id=? AND e.id=?")
        .bind(tenant_id).bind(execution_id).fetch_optional(&mut **tx).await?;
    let Some(session) = session else {
        return Ok((Value::Object(context), 0));
    };
    let session_id: Uuid = session.try_get("id")?;
    let deployment_id: Uuid = session.try_get("application_deployment_id")?;
    let session_defaults = Value::Object(
        workflow
            .contexts
            .iter()
            .filter(|(_, definition)| definition.scope == ContextScope::Session)
            .map(|(name, definition)| (name.clone(), definition.default.clone()))
            .collect(),
    );
    sqlx::query("INSERT IGNORE INTO application_session_contexts(tenant_id,application_deployment_id,session_id,context_json,context_version) VALUES(?,?,?,?,0)")
        .bind(tenant_id).bind(deployment_id).bind(session_id).bind(session_defaults)
        .execute(&mut **tx).await?;
    let stored = sqlx::query("SELECT context_json,context_version FROM application_session_contexts WHERE tenant_id=? AND application_deployment_id=? AND session_id=?")
        .bind(tenant_id).bind(deployment_id).bind(session_id).fetch_one(&mut **tx).await?;
    if let Some(values) = stored.try_get::<Value, _>("context_json")?.as_object() {
        context.extend(values.clone());
    }
    Ok((Value::Object(context), stored.try_get("context_version")?))
}
