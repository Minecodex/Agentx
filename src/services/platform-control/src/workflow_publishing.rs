//! Freeze/derive a version and move an environment in one Control transaction.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    workflow_api::{
        DeploymentResponse, audit, deployment_from_row, load_composite_definitions_for_actor,
        require_workflow_access,
    },
    workflow_resources::{build_version_snapshots, insert_version_snapshots},
    workflow_versions::{append_snapshot, lock_workflow, receipt, replay},
};
use agentx_domain::WorkflowDefinition;
use agentx_runtime::{CompileContext, WorkflowCompiler};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublishWorkflowRequest {
    environment_id: Uuid,
    workflow_version_id: Uuid,
    content_source: PublishContent,
    expected_revision: Option<u64>,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PublishContent {
    Version,
    Draft,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RollbackWorkflowRequest {
    target_workflow_version_id: Uuid,
}

pub(crate) async fn publish(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<PublishWorkflowRequest>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:publish")?;
    require_workflow_access(&state, &actor, workflow, true).await?;
    let mut tx = state.pool.begin().await?;
    lock_workflow(&mut tx, &actor, workflow).await?;
    let operation = format!("publish:{workflow}");
    let request = json!(input);
    if let Some(value) = replay(&mut tx, &actor, &operation, &headers, &request).await? {
        tx.commit().await?;
        return Ok(Json(value));
    }
    let version = sqlx::query(
        "SELECT * FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(workflow)
    .bind(input.workflow_version_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Workflow version"))?;
    let published = version.try_get::<String, _>("publication_status")? == "published";
    let target = if !published || input.content_source == PublishContent::Draft {
        let draft = version
            .try_get::<Option<Uuid>, _>("working_draft_id")?
            .ok_or_else(|| {
                ApiError::conflict("DRAFT_REQUIRED", "This version has no working document")
            })?;
        let expected = input.expected_revision.ok_or_else(|| {
            ApiError::bad_request(
                "REVISION_REQUIRED",
                "Select an exact saved revision before publishing",
            )
        })?;
        let source=sqlx::query("SELECT r.* FROM workflow_drafts d JOIN workflow_draft_revisions r ON r.tenant_id=d.tenant_id AND r.draft_id=d.id AND r.revision=d.revision WHERE d.tenant_id=? AND d.workflow_id=? AND d.id=? AND d.revision=? FOR UPDATE").bind(actor.tenant_id).bind(workflow).bind(draft).bind(expected).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::conflict("DRAFT_REVISION_CONFLICT","The saved document changed; review it before publishing"))?;
        if published
            && source.try_get::<String, _>("content_hash")?
                == version.try_get::<String, _>("content_hash")?
            && source.try_get::<String, _>("editor_hash")?
                == version.try_get::<String, _>("editor_hash")?
        {
            return Err(ApiError::conflict(
                "DRAFT_UNCHANGED",
                "There are no saved draft changes to publish",
            ));
        }
        let definition: WorkflowDefinition =
            serde_json::from_value(source.try_get("definition_json")?)
                .map_err(ApiError::internal)?;
        let dependencies =
            load_composite_definitions_for_actor(&state, &actor, workflow, &definition).await?;
        crate::canvas_plugin_api::require_definition_plugin_closure_enabled(
            &state,
            actor.tenant_id,
            &definition,
            &dependencies,
        )
        .await?;
        let registry = crate::canvas_plugin_api::registry_for_tenant(
            &state,
            actor.tenant_id,
            &dependencies,
            false,
        )
        .await?;
        let resolved = crate::canvas_plugin_api::resolve_workflow_plugin_closure(
            &state,
            actor.tenant_id,
            &definition,
            &dependencies,
            &registry,
        )
        .await?;
        if !resolved.incomplete.is_empty() || !resolved.invalid.is_empty() {
            return Err(ApiError::unprocessable(
                "PLUGIN_DEFINITION_NOT_READY",
                resolved
                    .incomplete
                    .iter()
                    .chain(resolved.invalid.iter())
                    .map(|issue| format!("{}: {}", issue.path, issue.message))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        WorkflowCompiler::new(&registry)
            .with_resolved_manifests(&resolved.manifests)
            .compile(&definition, &CompileContext::default())
            .map_err(|error| {
                ApiError::unprocessable("WORKFLOW_COMPILE_FAILED", error.to_string())
            })?;
        let frozen: Vec<agentx_domain::ResourceVersionSnapshot> =
            serde_json::from_value(source.try_get("resource_snapshot_json")?)
                .map_err(ApiError::internal)?;
        let snapshots =
            build_version_snapshots(&state, actor.tenant_id, workflow, &definition, &frozen)
                .await?;
        crate::workflow_revision_resources::lock_for_save(&mut tx, actor.tenant_id, &snapshots)
            .await?;
        crate::canvas_plugin_api::lock_resolved_plugin_closure(&mut tx, actor.tenant_id, &resolved)
            .await?;
        let manifests = definition
            .nodes
            .iter()
            .filter_map(|node| {
                resolved
                    .manifests
                    .get(&node.id)
                    .or_else(|| {
                        registry.resolve_definition_manifest(
                            &node.node_type,
                            node.type_version,
                            &node.parameters,
                        )
                    })
                    .map(|manifest| (node.id.clone(), manifest.clone()))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let manifests = serde_json::to_value(manifests).map_err(ApiError::internal)?;
        // The frozen source revision must include the exact resource facts used to compile.
        // Store publication facts in version_resources; original revisions remain immutable.
        let target = if published {
            let target = Uuid::now_v7();
            let number:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(version_number),0)+1 AS UNSIGNED) FROM workflow_versions WHERE tenant_id=? AND workflow_id=?").bind(actor.tenant_id).bind(workflow).fetch_one(&mut *tx).await?;
            sqlx::query("INSERT INTO workflow_versions(id,tenant_id,workflow_id,version_number,publication_status,published_at,source_draft_id,source_revision_id,source_revision,base_version_id,schema_version,definition_json,editor_json,content_hash,editor_hash,node_manifests_json,created_by,updated_by) SELECT ?,tenant_id,workflow_id,?,'published',UTC_TIMESTAMP(6),draft_id,id,revision,?,schema_version,definition_json,editor_json,content_hash,editor_hash,?,?,? FROM workflow_draft_revisions WHERE tenant_id=? AND id=?")
                .bind(target).bind(number).bind(input.workflow_version_id).bind(&manifests).bind(actor.user_id).bind(actor.user_id).bind(actor.tenant_id).bind(source.try_get::<Uuid,_>("id")?).execute(&mut *tx).await?;
            let baseline = sqlx::query(
                "SELECT r.* FROM workflow_draft_revisions r WHERE r.tenant_id=? AND r.id=?",
            )
            .bind(actor.tenant_id)
            .bind(version.try_get::<Uuid, _>("source_revision_id")?)
            .fetch_one(&mut *tx)
            .await?;
            let resources = crate::workflow_versions::frozen_resources(
                &mut tx,
                actor.tenant_id,
                input.workflow_version_id,
            )
            .await?;
            append_snapshot(
                &mut tx,
                &actor,
                workflow,
                draft,
                &baseline,
                Some(&resources),
            )
            .await?;
            target
        } else {
            sqlx::query("UPDATE workflow_versions SET publication_status='published',published_at=UTC_TIMESTAMP(6),node_manifests_json=?,updated_by=?,updated_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=?").bind(&manifests).bind(actor.user_id).bind(actor.tenant_id).bind(input.workflow_version_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE workflow_drafts SET base_version_id=? WHERE tenant_id=? AND id=?")
                .bind(input.workflow_version_id)
                .bind(actor.tenant_id)
                .bind(draft)
                .execute(&mut *tx)
                .await?;
            append_snapshot(
                &mut tx,
                &actor,
                workflow,
                draft,
                &source,
                Some(&serde_json::to_value(&snapshots).map_err(ApiError::internal)?),
            )
            .await?;
            input.workflow_version_id
        };
        insert_version_snapshots(&mut tx, actor.tenant_id, target, &snapshots).await?;
        audit(&mut tx,&actor,"workflow.version_published","workflow",workflow,json!({"versionId":target,"baseVersionId":if published {Some(input.workflow_version_id)}else{None}})).await?;
        target
    } else {
        input.workflow_version_id
    };
    if published && input.content_source == PublishContent::Version {
        validate_published_content(&state, &mut tx, &actor, workflow, target).await?;
    }
    let deployment = deploy_in_tx(
        &mut tx,
        &actor,
        workflow,
        input.environment_id,
        target,
        "publish",
    )
    .await?;
    let result = serde_json::to_value(deployment).map_err(ApiError::internal)?;
    receipt(&mut tx, &actor, &operation, &headers, &request, &result).await?;
    tx.commit().await?;
    crate::workflow_versions::cancel_expired_models(&state, actor.tenant_id, workflow).await;
    Ok(Json(result))
}

pub(crate) async fn rollback(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((workflow, environment)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<RollbackWorkflowRequest>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:publish")?;
    require_workflow_access(&state, &actor, workflow, true).await?;
    let mut tx = state.pool.begin().await?;
    lock_workflow(&mut tx, &actor, workflow).await?;
    let operation = format!("rollback:{workflow}:{environment}");
    let request = json!({"targetWorkflowVersionId":input.target_workflow_version_id});
    if let Some(result) = replay(&mut tx, &actor, &operation, &headers, &request).await? {
        tx.commit().await?;
        return Ok(Json(result));
    }
    validate_published_content(
        &state,
        &mut tx,
        &actor,
        workflow,
        input.target_workflow_version_id,
    )
    .await?;
    let result = serde_json::to_value(
        deploy_in_tx(
            &mut tx,
            &actor,
            workflow,
            environment,
            input.target_workflow_version_id,
            "rollback",
        )
        .await?,
    )
    .map_err(ApiError::internal)?;
    receipt(&mut tx, &actor, &operation, &headers, &request, &result).await?;
    tx.commit().await?;
    crate::workflow_versions::cancel_expired_models(&state, actor.tenant_id, workflow).await;
    Ok(Json(result))
}

async fn validate_published_content(
    state: &ControlApiState,
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    workflow: Uuid,
    version: Uuid,
) -> ApiResult<()> {
    let definition: Value=sqlx::query_scalar("SELECT definition_json FROM workflow_versions WHERE tenant_id=? AND workflow_id=? AND id=? AND publication_status='published'").bind(actor.tenant_id).bind(workflow).bind(version).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::not_found("Published workflow version"))?;
    let definition: WorkflowDefinition =
        serde_json::from_value(definition).map_err(ApiError::internal)?;
    crate::canvas_plugin_api::require_workflow_version_plugins_enabled(
        state,
        actor.tenant_id,
        version,
    )
    .await?;
    let frozen: Vec<agentx_domain::ResourceVersionSnapshot> = serde_json::from_value(
        crate::workflow_versions::frozen_resources(tx, actor.tenant_id, version).await?,
    )
    .map_err(ApiError::internal)?;
    // Recheck availability and current grants without replacing any frozen publication facts.
    build_version_snapshots(state, actor.tenant_id, workflow, &definition, &frozen).await?;
    Ok(())
}

async fn deploy_in_tx(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    workflow_id: Uuid,
    environment_id: Uuid,
    version_id: Uuid,
    source: &str,
) -> ApiResult<DeploymentResponse> {
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_versions v JOIN workflows w ON w.tenant_id=v.tenant_id AND w.id=v.workflow_id AND w.status='active' JOIN workflow_environments e ON e.tenant_id=v.tenant_id AND e.id=? AND e.status='active' WHERE v.tenant_id=? AND v.id=? AND v.workflow_id=? AND v.publication_status='published')").bind(environment_id).bind(actor.tenant_id).bind(version_id).bind(workflow_id).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(ApiError::bad_request(
            "INVALID_DEPLOYMENT_TARGET",
            "Environment or workflow version is invalid",
        ));
    }
    let head:Option<Uuid>=sqlx::query_scalar("SELECT active_deployment_id FROM workflow_deployment_heads WHERE tenant_id=? AND workflow_id=? AND environment_id=? FOR UPDATE").bind(actor.tenant_id).bind(workflow_id).bind(environment_id).fetch_optional(&mut **tx).await?;
    if let Some(previous) = head {
        sqlx::query("UPDATE workflow_deployments SET status=? WHERE id=?")
            .bind(if source == "rollback" {
                "rolled_back"
            } else {
                "superseded"
            })
            .bind(previous)
            .execute(&mut **tx)
            .await?;
    }
    let sequence:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(sequence_number),0)+1 AS UNSIGNED) FROM workflow_deployments WHERE tenant_id=? AND workflow_id=? AND environment_id=?").bind(actor.tenant_id).bind(workflow_id).bind(environment_id).fetch_one(&mut **tx).await?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO workflow_deployments(id,tenant_id,workflow_id,environment_id,workflow_version_id,sequence_number,source,created_by) VALUES(?,?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(workflow_id).bind(environment_id).bind(version_id).bind(sequence).bind(source).bind(actor.user_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO workflow_deployment_heads(tenant_id,workflow_id,environment_id,active_deployment_id) VALUES(?,?,?,?) ON DUPLICATE KEY UPDATE active_deployment_id=VALUES(active_deployment_id),version=version+1").bind(actor.tenant_id).bind(workflow_id).bind(environment_id).bind(id).execute(&mut **tx).await?;
    let row=sqlx::query("SELECT d.id,d.workflow_id,d.environment_id,e.name environment_name,d.workflow_version_id,v.version_number,d.sequence_number,d.status,d.source,d.created_at FROM workflow_deployments d JOIN workflow_environments e ON e.tenant_id=d.tenant_id AND e.id=d.environment_id JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.id=d.workflow_version_id WHERE d.tenant_id=? AND d.id=?").bind(actor.tenant_id).bind(id).fetch_one(&mut **tx).await?;
    audit(
        tx,
        actor,
        if source == "rollback" {
            "workflow.rolled_back"
        } else {
            "workflow.published"
        },
        "workflow",
        workflow_id,
        json!({"deploymentId":id,"workflowVersionId":version_id,"environmentId":environment_id}),
    )
    .await?;
    let response = deployment_from_row(row)?;
    Ok(response)
}
