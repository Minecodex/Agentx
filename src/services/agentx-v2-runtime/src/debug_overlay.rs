use std::collections::BTreeMap;

use agentx_domain::NodeExecutionId;
use agentx_node_protocol::Item;
use agentx_runtime::{CompiledNode, ExecutionMachine};
use agentx_runtime_contracts::{RuntimeDebugInputSourceV1, RuntimeDebugPlanV1};
use serde_json::{Value, json};
use sqlx::{MySql, MySqlPool, Row, Transaction};
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

#[derive(Clone, Debug)]
pub(crate) struct DebugOverlay {
    pub kind: String,
    pub payload: Value,
}

pub(crate) enum DebugCompletion {
    Valid {
        kind: String,
        outputs: BTreeMap<String, Vec<Item>>,
    },
    ContractViolation {
        kind: String,
        code: &'static str,
        message: String,
    },
}

impl DebugCompletion {
    pub(crate) fn succeeded(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }
}

pub(crate) struct ApplyDebugCompletion<'a> {
    pub tenant_id: Uuid,
    pub execution_id: Uuid,
    pub node_execution_id: NodeExecutionId,
    pub attempt_id: Uuid,
    pub node: &'a CompiledNode,
    pub machine: &'a mut ExecutionMachine,
}

pub(crate) fn for_node(runtime_settings: &Value, node_id: &str) -> Option<DebugOverlay> {
    let overlay = runtime_settings
        .get("workPackageOverlay")?
        .get("nodeParameters")?
        .get(node_id)?
        .get("debugOverlay")?;
    Some(DebugOverlay {
        kind: overlay.get("kind")?.as_str()?.to_owned(),
        payload: overlay.get("payload")?.clone(),
    })
}

pub(crate) fn items(payload: &Value) -> BTreeMap<String, Vec<Item>> {
    serde_json::from_value(payload.clone()).unwrap_or_else(|_| {
        BTreeMap::from([(
            "main".into(),
            vec![Item {
                json: payload.clone(),
                ..Item::default()
            }],
        )])
    })
}

pub(crate) fn completes_node(kind: &str) -> bool {
    matches!(
        kind,
        "pin_data" | "mock_output" | "history_output" | "artifact"
    )
}

pub(crate) fn completion(overlay: &DebugOverlay, node: &CompiledNode) -> Option<DebugCompletion> {
    if !completes_node(&overlay.kind) {
        return None;
    }
    let outputs = items(&overlay.payload);
    Some(
        match crate::output_contract::validate_node_output_contract(node, &outputs) {
            Ok(()) => DebugCompletion::Valid {
                kind: overlay.kind.clone(),
                outputs,
            },
            Err(message) => DebugCompletion::ContractViolation {
                kind: overlay.kind.clone(),
                code: crate::output_contract::violation_code(node),
                message,
            },
        },
    )
}

pub(crate) async fn apply_completion(
    tx: &mut Transaction<'_, MySql>,
    request: ApplyDebugCompletion<'_>,
    completion: DebugCompletion,
) -> RuntimeResult<()> {
    let (mut trace, failure) = match completion {
        DebugCompletion::Valid { kind, outputs } => {
            request
                .machine
                .complete(request.node_execution_id, outputs.clone())
                .map_err(crate::engine_protocol::machine_error)?;
            sqlx::query(
                "UPDATE node_attempts SET output_json=?,ended_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND status='succeeded'",
            )
            .bind(serde_json::to_value(&outputs).map_err(|error| RuntimeError::Internal(error.into()))?)
            .bind(request.tenant_id)
            .bind(request.attempt_id)
            .execute(&mut **tx)
            .await?;
            // The node query and subsequent Pin read the execution row. Mock
            // and replay results must expose the same validated port map there.
            sqlx::query("UPDATE node_executions SET output_json=? WHERE tenant_id=? AND id=?")
                .bind(serde_json::to_value(&outputs).map_err(RuntimeError::from)?)
                .bind(request.tenant_id)
                .bind(request.node_execution_id.as_uuid())
                .execute(&mut **tx)
                .await?;
            let mut trace = crate::trace_delivery::TraceDraft::execution(
                request.tenant_id,
                request.execution_id,
                "node.debug_overlay_applied",
                "succeeded",
            );
            trace.attributes = json!({"nodeId":request.node.id,"overlayKind":kind});
            (trace, None)
        }
        DebugCompletion::ContractViolation {
            kind,
            code,
            message,
        } => {
            request
                .machine
                .fail(request.node_execution_id, code, &message, false)
                .map_err(crate::engine_protocol::machine_error)?;
            sqlx::query("UPDATE node_attempts SET error_code=?,error_message=?,ended_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND status='failed'")
                .bind(code)
                .bind(&message)
                .bind(request.tenant_id)
                .bind(request.attempt_id)
                .execute(&mut **tx)
                .await?;
            let mut trace = crate::trace_delivery::TraceDraft::execution(
                request.tenant_id,
                request.execution_id,
                "node.failed",
                "failed",
            );
            trace.error_code = Some(code.into());
            trace.error_message = Some(message.clone());
            trace.attributes = json!({"nodeId":request.node.id,"overlayKind":kind});
            (trace, Some((code, message)))
        }
    };
    let activation = request
        .machine
        .activation(request.node_execution_id)
        .cloned()
        .ok_or_else(|| RuntimeError::Internal(anyhow::anyhow!("activation disappeared")))?;
    if let Some((code, message)) = failure {
        crate::engine_persistence::upsert_failed_activation(
            tx,
            request.tenant_id,
            request.execution_id,
            &activation,
            request.node,
            code,
            &message,
        )
        .await?;
    } else {
        crate::engine_persistence::upsert_activation(
            tx,
            request.tenant_id,
            request.execution_id,
            &activation,
            request.node,
        )
        .await?;
    }
    trace.node_execution_id = Some(request.node_execution_id.as_uuid());
    trace.attempt_id = Some(request.attempt_id);
    crate::trace_delivery::enqueue_best_effort(tx, trace).await;
    Ok(())
}

pub(crate) fn plan(runtime_settings: &Value) -> RuntimeResult<Option<RuntimeDebugPlanV1>> {
    let Some(value) = runtime_settings
        .get("workPackageSpec")
        .and_then(|spec| {
            (spec.get("kind").and_then(Value::as_str) == Some("debug")).then_some(spec)
        })
        .and_then(|spec| spec.get("debugPlan"))
    else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|error| invalid("DEBUG_PLAN_INVALID", &error.to_string()))
}

pub(crate) async fn resolve_input_source(
    state: Option<&crate::RuntimeState>,
    pool: &MySqlPool,
    tenant_id: Uuid,
    plan: Option<&RuntimeDebugPlanV1>,
) -> RuntimeResult<Option<Vec<Item>>> {
    let Some(source) = plan.and_then(|plan| plan.input_source.as_ref()) else {
        return Ok(None);
    };
    match source {
        RuntimeDebugInputSourceV1::Manual { value } => Ok(Some(vec![Item {
            json: value.clone(),
            ..Item::default()
        }])),
        RuntimeDebugInputSourceV1::HistoryOutput {
            execution_id,
            node_execution_id,
            output_port,
        } => {
            let output: Value = sqlx::query_scalar(
                "SELECT output_json FROM node_executions WHERE tenant_id=? AND execution_id=? AND id=? AND status='succeeded' AND output_json IS NOT NULL",
            )
            .bind(tenant_id)
            .bind(execution_id)
            .bind(node_execution_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| {
                invalid(
                    "DEBUG_HISTORY_OUTPUT_NOT_FOUND",
                    "The selected successful node output is unavailable",
                )
            })?;
            let outputs: BTreeMap<String, Vec<Item>> = serde_json::from_value(output)
                .map_err(|error| invalid("DEBUG_HISTORY_OUTPUT_INVALID", &error.to_string()))?;
            outputs.get(output_port).cloned().map(Some).ok_or_else(|| {
                invalid(
                    "DEBUG_HISTORY_PORT_NOT_FOUND",
                    "The selected node output port is unavailable",
                )
            })
        }
        RuntimeDebugInputSourceV1::Artifact {
            execution_id,
            artifact_id,
        } => {
            let state = state.ok_or_else(|| {
                invalid(
                    "DEBUG_ARTIFACT_SOURCE_UNAVAILABLE",
                    "Artifact Debug input requires the Runtime object store",
                )
            })?;
            let row = sqlx::query(
                "SELECT a.sha256 FROM artifacts a WHERE a.tenant_id=? AND a.id=? AND a.deleted_at IS NULL AND EXISTS(SELECT 1 FROM artifact_references r WHERE r.tenant_id=a.tenant_id AND r.artifact_id=a.id AND (r.owner_type='execution' AND r.owner_id=? OR EXISTS(SELECT 1 FROM node_executions n WHERE n.tenant_id=a.tenant_id AND n.execution_id=? AND r.owner_id=BIN_TO_UUID(n.id))))",
            )
            .bind(tenant_id)
            .bind(artifact_id)
            .bind(execution_id.to_string())
            .bind(execution_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| {
                invalid(
                    "DEBUG_ARTIFACT_NOT_FOUND",
                    "The selected Execution artifact is unavailable",
                )
            })?;
            let expected_hash = format!("sha256:{}", row.try_get::<String, _>("sha256")?);
            let bytes =
                crate::artifact::load_artifact(state, tenant_id, *artifact_id, &expected_hash)
                    .await?;
            let payload: Value = serde_json::from_slice(&bytes)
                .map_err(|error| invalid("DEBUG_ARTIFACT_INVALID", &error.to_string()))?;
            let mut outputs = items(&payload);
            outputs
                .remove("main")
                .or_else(|| outputs.into_values().next())
                .map(Some)
                .ok_or_else(|| invalid("DEBUG_ARTIFACT_EMPTY", "Debug artifact contains no items"))
        }
    }
}

fn invalid(code: &'static str, message: &str) -> RuntimeError {
    RuntimeError::InvalidRequest(code, message.to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{completes_node, for_node, items};

    #[test]
    fn reads_the_signed_work_package_overlay_snapshot() {
        let settings = json!({
            "workPackageOverlay": {
                "nodeParameters": {
                    "agent-1": {
                        "debugOverlay": {
                            "id":"00000000-0000-0000-0000-000000000001",
                            "kind":"mock_output",
                            "payload":{"main":[{"json":{"answer":"mocked"}}]},
                            "artifactId":null
                        }
                    }
                }
            }
        });
        let overlay = for_node(&settings, "agent-1").unwrap();
        assert_eq!(overlay.kind, "mock_output");
        assert_eq!(
            items(&overlay.payload)["main"][0].json,
            json!({"answer":"mocked"})
        );
        assert!(completes_node(&overlay.kind));
    }
}
