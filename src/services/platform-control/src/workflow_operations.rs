use std::collections::BTreeMap;

use agentx_domain::{
    EditorDocument, WorkflowDefinition, canonical_content_hash, validate_editor_document,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    control_helpers::required_name,
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/workflows/{id}/drafts/{draft_id}/export",
            get(export_workflow),
        )
        .route("/api/v1/workflows/import", post(import_workflow))
        .route("/api/v1/workflows/{id}/run", post(run_workflow))
        .route(
            "/api/v1/workflows/{id}/drafts/{draft_id}/debug-executions",
            post(debug_execution),
        )
        .route(
            "/api/v1/workflow-versions/{version_id}/executions",
            post(start_version_execution),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunRequest {
    #[serde(default)]
    wait_inputs: agentx_runtime_contracts::WaitInputsV1,
    #[serde(default)]
    input: Value,
    idempotency_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DebugRequest {
    #[serde(default)]
    wait_inputs: agentx_runtime_contracts::WaitInputsV1,
    expected_revision: u64,
    mode: String,
    target_node_id: Option<String>,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    context: Value,
    #[serde(default)]
    input_source: Value,
    #[serde(default)]
    overlay_ids: Vec<Uuid>,
    #[serde(default)]
    side_effect_decisions: Value,
    idempotency_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportRequest {
    name: String,
    description: Option<String>,
    visibility: String,
    package: Value,
}

async fn export_workflow(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:view")?;
    crate::workflow_api::require_workflow_access(&state, &actor, id, false).await?;
    let row=sqlx::query("SELECT w.name,d.definition_json,d.editor_json FROM workflows w JOIN workflow_drafts d ON d.tenant_id=w.tenant_id AND d.workflow_id=w.id WHERE w.tenant_id=? AND w.id=? AND d.id=?").bind(actor.tenant_id).bind(id).bind(draft_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Workflow"))?;
    let definition: Value = row.try_get("definition_json")?;
    let editor: Value = row
        .try_get::<Option<Value>, _>("editor_json")?
        .unwrap_or_else(|| json!({}));
    let package_body = json!({"workflowDefinition":definition,"editorDocument":editor,"nodeLock":[],"subWorkflowReferences":[],"apiBinding":{}});
    let hash = canonical_content_hash(&package_body).map_err(ApiError::internal)?;
    let signature = state.work_packages.sign_content_hash(&hash);
    Ok(Json(
        json!({"manifest":{"packageSchemaVersion":"1.0","workflowSchemaVersion":agentx_domain::WORKFLOW_SCHEMA_VERSION,"name":row.try_get::<String,_>("name")?,"contentHash":hash,"signatureAlgorithm":"ed25519","signingKeyId":state.work_packages.signing_key_id(),"signature":signature},"workflowDefinition":definition,"editorDocument":editor,"nodeLock":[],"subWorkflowReferences":[],"apiBinding":{}}),
    ))
}

async fn import_workflow(
    State(state): State<ControlApiState>,
    actor: Actor,
    headers: axum::http::HeaderMap,
    Json(input): Json<ImportRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("workflow:create")?;
    let name = required_name(&input.name)?;
    validate_visibility(&input.visibility)?;
    let manifest = input.package.get("manifest").cloned().ok_or_else(|| {
        ApiError::bad_request("WORKFLOW_PACKAGE_INVALID", "Package manifest is required")
    })?;
    if manifest.get("packageSchemaVersion").and_then(Value::as_str) != Some("1.0")
        || manifest
            .get("workflowSchemaVersion")
            .and_then(Value::as_str)
            != Some(agentx_domain::WORKFLOW_SCHEMA_VERSION)
    {
        return Err(ApiError::unprocessable(
            "WORKFLOW_PACKAGE_VERSION_UNSUPPORTED",
            "Only Workflow Package 1.0 / Definition 8.0 is supported",
        ));
    }
    let definition = input
        .package
        .get("workflowDefinition")
        .cloned()
        .ok_or_else(|| {
            ApiError::bad_request(
                "WORKFLOW_PACKAGE_INVALID",
                "Workflow Definition is required",
            )
        })?;
    let editor = input
        .package
        .get("editorDocument")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let body = json!({"workflowDefinition":&definition,"editorDocument":&editor,"nodeLock":input.package.get("nodeLock").cloned().unwrap_or_else(||json!([])),"subWorkflowReferences":input.package.get("subWorkflowReferences").cloned().unwrap_or_else(||json!([])),"apiBinding":input.package.get("apiBinding").cloned().unwrap_or_else(||json!({}))});
    let hash = canonical_content_hash(&body).map_err(ApiError::internal)?;
    let expected = manifest
        .get("contentHash")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ApiError::bad_request("WORKFLOW_PACKAGE_INVALID", "Content hash is required")
        })?;
    let signature = manifest
        .get("signature")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ApiError::bad_request("WORKFLOW_PACKAGE_INVALID", "Signature is required")
        })?;
    if expected != hash || !state.work_packages.verify_content_hash(expected, signature) {
        return Err(ApiError::unprocessable(
            "WORKFLOW_PACKAGE_INTEGRITY_FAILED",
            "Workflow Package hash or signature is invalid",
        ));
    }
    let definition: WorkflowDefinition = serde_json::from_value(definition)
        .map_err(|e| ApiError::unprocessable("INVALID_WORKFLOW_DEFINITION", e.to_string()))?;
    let editor_doc: EditorDocument = serde_json::from_value(editor.clone())
        .map_err(|e| ApiError::unprocessable("INVALID_EDITOR_DOCUMENT", e.to_string()))?;
    if let Some(issue) = validate_editor_document(&definition, &editor_doc).first() {
        return Err(ApiError::unprocessable(
            "INVALID_EDITOR_DOCUMENT",
            issue.message.clone(),
        ));
    }
    let definition = serde_json::to_value(definition).map_err(ApiError::internal)?;
    let workflow = Uuid::now_v7();
    let draft = Uuid::now_v7();
    let identity = Uuid::now_v7();
    let definition_hash = canonical_content_hash(&definition).map_err(ApiError::internal)?;
    let editor_hash = canonical_content_hash(&editor).map_err(ApiError::internal)?;
    let request = json!({"name":input.name,"description":input.description,"visibility":input.visibility,"package":input.package});
    let keyed = headers.contains_key("Idempotency-Key");
    let mut tx = state.pool.begin().await?;
    if keyed {
        sqlx::query("SELECT id FROM users WHERE tenant_id=? AND id=? FOR UPDATE")
            .bind(actor.tenant_id)
            .bind(actor.user_id)
            .fetch_one(&mut *tx)
            .await?;
        if let Some(result) =
            crate::workflow_versions::replay(&mut tx, &actor, "import-workflow", &headers, &request)
                .await?
        {
            tx.commit().await?;
            return Ok((StatusCode::CREATED, Json(result)));
        }
    }
    sqlx::query("INSERT INTO workflows(id,tenant_id,name,description,visibility,owner_user_id,owner_department_id) VALUES(?,?,?,?,?,?,?)").bind(workflow).bind(actor.tenant_id).bind(name).bind(input.description).bind(input.visibility).bind(actor.user_id).bind(actor.department_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO workflow_service_identities(id,tenant_id,workflow_id) VALUES(?,?,?)")
        .bind(identity)
        .bind(actor.tenant_id)
        .bind(workflow)
        .execute(&mut *tx)
        .await?;
    crate::runtime_admission::emit_new_service_identity(&mut tx, actor.tenant_id, identity).await?;
    sqlx::query("INSERT INTO workflow_members(tenant_id,workflow_id,user_id,member_role,created_by) VALUES(?,?,?,'manager',?)").bind(actor.tenant_id).bind(workflow).bind(actor.user_id).bind(actor.user_id).execute(&mut *tx).await?;
    crate::workflow_api::emit_workflow_query_grant(
        &mut tx,
        actor.tenant_id,
        workflow,
        actor.user_id,
        1,
        true,
    )
    .await?;
    sqlx::query("INSERT INTO workflow_drafts(id,tenant_id,workflow_id,schema_version,revision,definition_json,editor_json,content_hash,editor_hash,updated_by,created_by) VALUES(?,?,?,'8.0',0,?,?,?,?,?,?)").bind(draft).bind(actor.tenant_id).bind(workflow).bind(definition).bind(editor).bind(&definition_hash).bind(editor_hash).bind(actor.user_id).bind(actor.user_id).execute(&mut *tx).await?;
    crate::workflow_drafts::record_initial_revision(&mut tx, &actor, workflow, draft).await?;
    let parsed = serde_json::from_value(
        sqlx::query_scalar::<_, Value>("SELECT definition_json FROM workflow_drafts WHERE id=?")
            .bind(draft)
            .fetch_one(&mut *tx)
            .await?,
    )
    .map_err(ApiError::internal)?;
    crate::workflow_resources::replace_draft_resources(
        &mut tx,
        actor.tenant_id,
        workflow,
        draft,
        &parsed,
    )
    .await?;
    let version =
        crate::workflow_versions::insert_unpublished(&mut tx, &actor, workflow, draft).await?;
    let result =
        json!({"workflowId":workflow,"createdVersionId":version,"definitionHash":definition_hash});
    if keyed {
        crate::workflow_versions::receipt(
            &mut tx,
            &actor,
            "import-workflow",
            &headers,
            &request,
            &result,
        )
        .await?;
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn run_workflow(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<RunRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("execution:run")?;
    crate::workflow_api::require_workflow_access(&state, &actor, id, false).await?;
    let version:Uuid=sqlx::query_scalar("SELECT id FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND publication_status='published' ORDER BY version_number DESC LIMIT 1").bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::unprocessable("WORKFLOW_VERSION_REQUIRED","Publish a Workflow Version before running"))?;
    invoke_version(
        &state,
        &actor,
        version,
        input.input,
        input.wait_inputs,
        input.idempotency_key,
    )
    .await
}
async fn start_version_execution(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(version): Path<Uuid>,
    Json(input): Json<RunRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("execution:run")?;
    invoke_version(
        &state,
        &actor,
        version,
        input.input,
        input.wait_inputs,
        input.idempotency_key,
    )
    .await
}

async fn invoke_version(
    state: &ControlApiState,
    actor: &Actor,
    version: Uuid,
    input: Value,
    wait_inputs: agentx_runtime_contracts::WaitInputsV1,
    key: Option<String>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let key = key.unwrap_or_else(|| Uuid::now_v7().to_string());
    let result = crate::work_packages::start_version_execution(
        state,
        actor,
        version,
        input,
        wait_inputs,
        key,
    )
    .await?;
    let execution_id = result.execution_id.ok_or_else(|| {
        ApiError::conflict(
            "WORK_PACKAGE_EXECUTION_RECEIPT_INVALID",
            "Runtime did not return an Execution identifier",
        )
    })?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "executionId":execution_id,
            "status":result.status,
            "replayed":result.replayed,
        })),
    ))
}

async fn debug_execution(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, draft_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<DebugRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("execution:run")?;
    crate::workflow_api::require_workflow_access(&state, &actor, id, true).await?;
    let runtime_mode = runtime_debug_mode(&input.mode)?;
    let target_node_id = (runtime_mode != agentx_runtime_contracts::PartialExecutionModeV1::Whole)
        .then(|| input.target_node_id.clone())
        .flatten();
    if input.mode != "full" && input.target_node_id.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::unprocessable(
            "DEBUG_TARGET_REQUIRED",
            "Partial debug execution requires a target node",
        ));
    }
    if matches!(input.mode.as_str(), "single_node" | "from_node")
        && input
            .input_source
            .get("kind")
            .and_then(Value::as_str)
            .is_none()
    {
        return Err(ApiError::unprocessable(
            "DEBUG_INPUT_SOURCE_REQUIRED",
            "This debug mode requires an explicit input source",
        ));
    }
    let context = input
        .context
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let node_parameters = load_debug_overlays(
        &state.pool,
        actor.tenant_id,
        id,
        draft_id,
        &input.overlay_ids,
    )
    .await?;
    let input_source = (!input.input_source.is_null()
        && input
            .input_source
            .as_object()
            .is_none_or(|value| !value.is_empty()))
    .then(|| {
        serde_json::from_value(input.input_source).map_err(|error| {
            ApiError::unprocessable(
                "DEBUG_INPUT_SOURCE_INVALID",
                format!("Debug input source is invalid: {error}"),
            )
        })
    })
    .transpose()?;
    let side_effect_decisions =
        serde_json::from_value(input.side_effect_decisions).map_err(|error| {
            ApiError::unprocessable(
                "DEBUG_SIDE_EFFECT_DECISIONS_INVALID",
                format!("Debug side-effect decisions are invalid: {error}"),
            )
        })?;
    let (_, Json(result)) = crate::work_packages::start_debug_run(
        State(state),
        actor,
        Path(id),
        Json(crate::work_packages::StartDebugRunRequest {
            wait_inputs: input.wait_inputs,
            draft_id,
            revision: input.expected_revision,
            idempotency_key: input
                .idempotency_key
                .unwrap_or_else(|| Uuid::now_v7().to_string()),
            input: input.input,
            context,
            node_parameters,
            mode: Some(runtime_mode),
            target_node_id,
            input_source,
            side_effect_decisions,
        }),
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::to_value(result).map_err(ApiError::internal)?),
    ))
}

async fn load_debug_overlays(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    workflow_id: Uuid,
    draft_id: Uuid,
    overlay_ids: &[Uuid],
) -> ApiResult<BTreeMap<String, Value>> {
    let mut overlays = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for overlay_id in overlay_ids {
        if !seen.insert(*overlay_id) {
            return Err(ApiError::bad_request(
                "DEBUG_OVERLAY_DUPLICATE",
                "Debug overlay identifiers must be unique",
            ));
        }
        let row = sqlx::query(
            "SELECT node_id,kind,payload_json,artifact_id,stale FROM workflow_debug_overlays WHERE tenant_id=? AND workflow_id=? AND draft_id=? AND id=?",
        )
        .bind(tenant_id)
        .bind(workflow_id)
        .bind(draft_id)
        .bind(overlay_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Debug overlay"))?;
        if row.try_get::<bool, _>("stale")? {
            return Err(ApiError::conflict(
                "DEBUG_OVERLAY_STALE",
                "Debug overlay must be refreshed before execution",
            ));
        }
        let node_id: String = row.try_get("node_id")?;
        if overlays.contains_key(&node_id) {
            return Err(ApiError::conflict(
                "DEBUG_OVERLAY_NODE_CONFLICT",
                "Only one Debug overlay can be applied to a node",
            ));
        }
        overlays.insert(
            node_id,
            json!({
                "debugOverlay": {
                    "id": overlay_id,
                    "kind": row.try_get::<String, _>("kind")?,
                    "payload": row.try_get::<Value, _>("payload_json")?,
                    "artifactId": row.try_get::<Option<Uuid>, _>("artifact_id")?,
                }
            }),
        );
    }
    Ok(overlays)
}

fn runtime_debug_mode(mode: &str) -> ApiResult<agentx_runtime_contracts::PartialExecutionModeV1> {
    use agentx_runtime_contracts::PartialExecutionModeV1;

    match mode {
        "full" => Ok(PartialExecutionModeV1::Whole),
        "single_node" => Ok(PartialExecutionModeV1::Node),
        "to_node" => Ok(PartialExecutionModeV1::ToNode),
        "from_node" => Ok(PartialExecutionModeV1::FromNode),
        _ => Err(ApiError::bad_request(
            "INVALID_DEBUG_MODE",
            "Debug mode is invalid",
        )),
    }
}

fn validate_visibility(value: &str) -> ApiResult<()> {
    if !matches!(value, "private" | "department" | "company") {
        return Err(ApiError::bad_request(
            "INVALID_VISIBILITY",
            "Visibility is invalid",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_debug_modes_map_to_runtime_modes() {
        use agentx_runtime_contracts::PartialExecutionModeV1;

        assert_eq!(
            runtime_debug_mode("full").unwrap(),
            PartialExecutionModeV1::Whole
        );
        assert_eq!(
            runtime_debug_mode("single_node").unwrap(),
            PartialExecutionModeV1::Node
        );
        assert_eq!(
            runtime_debug_mode("to_node").unwrap(),
            PartialExecutionModeV1::ToNode
        );
        assert_eq!(
            runtime_debug_mode("from_node").unwrap(),
            PartialExecutionModeV1::FromNode
        );
        assert!(runtime_debug_mode("whole").is_err());
    }
}
