//! Context write resolution and optimistic persistence for worker results.
use super::*;

pub(super) struct ContextWriteRequest<'a> {
    pub(super) tenant_id: Uuid,
    pub(super) execution_id: Uuid,
    pub(super) node_execution_id: NodeExecutionId,
    pub(super) attempt_id: Uuid,
    pub(super) invocation_id: Option<Uuid>,
    pub(super) input: Value,
    pub(super) machine: &'a ExecutionMachine,
    pub(super) current_context: &'a Value,
    pub(super) context_version: u64,
    pub(super) resolved_outputs: &'a BTreeMap<String, Vec<Item>>,
}

pub(super) async fn apply_context_writes(
    tx: &mut Transaction<'_, MySql>,
    request: ContextWriteRequest<'_>,
) -> RuntimeResult<ContextWriteOutcome> {
    let ContextWriteRequest {
        tenant_id,
        execution_id,
        node_execution_id,
        attempt_id,
        invocation_id,
        input,
        machine,
        current_context,
        context_version,
        resolved_outputs,
    } = request;
    let activation = machine
        .activation(node_execution_id)
        .ok_or_else(|| RuntimeError::Internal(anyhow::anyhow!("activation disappeared")))?;
    let node = &machine.workflow().nodes[activation.node_index];
    if node.context_writes.is_empty() {
        return Ok(ContextWriteOutcome::Applied {
            context: current_context.clone(),
            version: context_version,
            conversions: Vec::new(),
        });
    }
    let mut outputs = load_output_namespace(tx, tenant_id, execution_id).await?;
    if let Some(namespace) = outputs.as_object_mut() {
        let current = serde_json::to_value(resolved_outputs)
            .map_err(|error| RuntimeError::Internal(error.into()))?;
        merge_output_namespace(namespace, &node.key, activation.run_index, &current);
    }
    let current_item = resolved_outputs
        .values()
        .flat_map(|items| items.iter())
        .next()
        .map(|item| item.json.clone())
        .unwrap_or(Value::Null);
    let expression_context = ExpressionContext {
        json: current_item,
        inputs: input,
        outputs,
        contexts: current_context.clone(),
        execution: crate::execution_context::with_node(
            crate::execution_context::load(tx, tenant_id, execution_id).await?,
            &node.id,
            node_execution_id.as_uuid(),
            activation.run_index,
            None,
            None,
        ),
        output_node_keys: machine
            .workflow()
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node.key.clone()))
            .collect(),
        ..ExpressionContext::default()
    };
    let mut updated = current_context.clone();
    let mut patches = Vec::with_capacity(node.context_writes.len());
    let mut conversions = Vec::new();
    let mut session_writes = 0_u64;
    for write in &node.context_writes {
        let target_schema = context_write_schema(&machine.workflow().contexts, &write.path)
            .unwrap_or_else(|| json!({}));
        let (value, mut write_conversions) = ExpressionEngine
            .resolve_input_with_schema(
                &write.value,
                &target_schema,
                &expression_context,
                format!("contextWrites.{}", write.path),
            )
            .map_err(|error| RuntimeError::Deterministic {
                code: "DYNAMIC_VALUE_EVALUATION_FAILED",
                message: error.to_string(),
            })?;
        conversions.append(&mut write_conversions);
        let Some(value) = value else {
            continue;
        };
        let before = context_value(&updated, &write.path).cloned();
        apply_context_write(&mut updated, &write.path, write.operation, value.clone())?;
        let root = write.path.split('.').next().unwrap_or_default();
        let session_scoped = machine
            .workflow()
            .contexts
            .get(root)
            .is_some_and(|definition| definition.scope == ContextScope::Session);
        if session_scoped {
            session_writes = session_writes.saturating_add(1);
        }
        patches.push((write, value, before, session_scoped));
    }
    let next_context_version = context_version.saturating_add(session_writes);
    if session_writes > 0
        && let Some(invocation_id) = invocation_id
    {
        let session = sqlx::query("SELECT s.id,s.application_deployment_id FROM application_invocations i JOIN application_sessions s ON s.tenant_id=i.tenant_id AND s.id=i.session_id WHERE i.tenant_id=? AND i.id=?")
            .bind(tenant_id).bind(invocation_id).fetch_optional(&mut **tx).await?;
        if let Some(session) = session {
            let session_context = Value::Object(
                machine
                    .workflow()
                    .contexts
                    .iter()
                    .filter(|(_, definition)| definition.scope == ContextScope::Session)
                    .filter_map(|(name, _)| {
                        updated
                            .get(name)
                            .cloned()
                            .map(|value| (name.clone(), value))
                    })
                    .collect(),
            );
            let changed = sqlx::query("UPDATE application_session_contexts SET context_json=?,context_version=? WHERE tenant_id=? AND application_deployment_id=? AND session_id=? AND context_version=?")
                .bind(session_context).bind(next_context_version).bind(tenant_id)
                .bind(session.try_get::<Uuid,_>("application_deployment_id")?)
                .bind(session.try_get::<Uuid,_>("id")?).bind(context_version)
                .execute(&mut **tx).await?;
            if changed.rows_affected() != 1 {
                return Ok(ContextWriteOutcome::SessionConflict);
            }
        }
    }
    let mut patch_version = context_version;
    for (index, (write, value, before, session_scoped)) in patches.into_iter().enumerate() {
        let version_before = patch_version;
        if session_scoped {
            patch_version = patch_version.saturating_add(1);
        }
        sqlx::query("INSERT INTO workflow_context_patches(id,tenant_id,execution_id,node_execution_id,attempt_id,patch_index,operation_key,context_path,value_json,context_version_before,context_version_after) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
            .bind(Uuid::now_v7()).bind(tenant_id).bind(execution_id).bind(node_execution_id.as_uuid())
            .bind(attempt_id).bind(index as u32).bind(context_operation_name(write.operation))
            .bind(&write.path).bind(json!({"before":before,"value":value}))
            .bind(version_before).bind(patch_version).execute(&mut **tx).await?;
    }
    Ok(ContextWriteOutcome::Applied {
        context: updated,
        version: next_context_version,
        conversions,
    })
}

fn context_write_schema(
    contexts: &BTreeMap<String, agentx_domain::ContextDefinition>,
    path: &str,
) -> Option<Value> {
    let mut segments = path.split('.');
    let root = segments.next()?;
    let mut schema = &contexts.get(root)?.schema;
    for segment in segments {
        if segment.parse::<usize>().is_ok() {
            schema = schema.get("items")?;
        } else {
            schema = schema.get("properties")?.get(segment)?;
        }
    }
    Some(schema.clone())
}
