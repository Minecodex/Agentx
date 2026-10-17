//! Resource facts belong to an immutable Draft revision, including inherited Version facts.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::ControlApiState,
};
use agentx_domain::{ResourceVersionSnapshot, WorkflowDefinition};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

fn resource_lock_query(kind: &str, exclusive: bool) -> ApiResult<&'static str> {
    match (kind, exclusive) {
        ("model", false) => Ok("SELECT id FROM model_aliases WHERE tenant_id=? AND id=? FOR SHARE"),
        ("model", true) => Ok("SELECT id FROM model_aliases WHERE tenant_id=? AND id=? FOR UPDATE"),
        ("credential", false) => {
            Ok("SELECT id FROM credentials WHERE tenant_id=? AND id=? FOR SHARE")
        }
        ("credential", true) => {
            Ok("SELECT id FROM credentials WHERE tenant_id=? AND id=? FOR UPDATE")
        }
        ("mcp_server", false) => {
            Ok("SELECT id FROM mcp_servers WHERE tenant_id=? AND id=? FOR SHARE")
        }
        ("mcp_server", true) => {
            Ok("SELECT id FROM mcp_servers WHERE tenant_id=? AND id=? FOR UPDATE")
        }
        ("mcp_tool", false) => Ok("SELECT id FROM mcp_tools WHERE tenant_id=? AND id=? FOR SHARE"),
        ("mcp_tool", true) => Ok("SELECT id FROM mcp_tools WHERE tenant_id=? AND id=? FOR UPDATE"),
        ("skill", false) => Ok("SELECT id FROM skills WHERE tenant_id=? AND id=? FOR SHARE"),
        ("skill", true) => Ok("SELECT id FROM skills WHERE tenant_id=? AND id=? FOR UPDATE"),
        ("rag", false) => Ok("SELECT id FROM rag_resources WHERE tenant_id=? AND id=? FOR SHARE"),
        ("rag", true) => Ok("SELECT id FROM rag_resources WHERE tenant_id=? AND id=? FOR UPDATE"),
        ("memory", false) => {
            Ok("SELECT id FROM memory_connections WHERE tenant_id=? AND id=? FOR SHARE")
        }
        ("memory", true) => {
            Ok("SELECT id FROM memory_connections WHERE tenant_id=? AND id=? FOR UPDATE")
        }
        ("sandbox_profile", false) => {
            Ok("SELECT id FROM sandbox_profiles WHERE tenant_id=? AND id=? FOR SHARE")
        }
        ("sandbox_profile", true) => {
            Ok("SELECT id FROM sandbox_profiles WHERE tenant_id=? AND id=? FOR UPDATE")
        }
        _ => Err(ApiError::internal("Unsupported revision resource type")),
    }
}

pub(crate) async fn lock_for_save(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    frozen: &[ResourceVersionSnapshot],
) -> ApiResult<()> {
    let mut resources = frozen
        .iter()
        .map(|snapshot| {
            (
                snapshot.reference.resource_type.as_str(),
                snapshot.reference.resource_id,
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    for snapshot in frozen {
        if snapshot.reference.resource_type == agentx_domain::ResourceType::McpTool {
            let server: Uuid =
                sqlx::query_scalar("SELECT server_id FROM mcp_tools WHERE tenant_id=? AND id=?")
                    .bind(tenant)
                    .bind(snapshot.reference.resource_id)
                    .fetch_optional(&mut **tx)
                    .await?
                    .ok_or_else(|| ApiError::not_found("MCP tool"))?;
            resources.insert(("mcp_server", server));
        }
    }
    for (kind, id) in resources {
        sqlx::query_scalar::<_, Uuid>(resource_lock_query(kind, false)?)
            .bind(tenant)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| {
                ApiError::conflict(
                    "RESOURCE_CHANGED",
                    "A resource was deleted while the revision was prepared",
                )
            })?;
    }
    Ok(())
}

pub(crate) async fn begin_delete(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    kind: &str,
    id: Uuid,
) -> ApiResult<sqlx::Transaction<'static, sqlx::MySql>> {
    let mut tx = pool.begin().await?;
    sqlx::query_scalar::<_, Uuid>(resource_lock_query(kind, true)?)
        .bind(tenant)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::not_found("Resource"))?;
    // Save holds these rows until the new immutable references commit. An
    // autocommit read after taking this lock sees any preceding Save winner.
    require_unreferenced(pool, tenant, kind, id).await?;
    Ok(tx)
}

/// Heads can stop referring to a resource while an immutable, runnable revision
/// still needs it. Use the same frozen facts as versioning and debug preparation.
pub(crate) async fn require_unreferenced(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    resource_type: &str,
    id: Uuid,
) -> ApiResult<()> {
    let referenced: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_draft_revisions r JOIN JSON_TABLE(r.resource_snapshot_json,'$[*]' COLUMNS(resource_type VARCHAR(32) PATH '$.reference.resourceType',resource_id VARCHAR(36) PATH '$.reference.resourceId')) j ON TRUE WHERE r.tenant_id=? AND ((j.resource_type=? AND j.resource_id=BIN_TO_UUID(?)) OR (?='mcp_server' AND j.resource_type='mcp_tool' AND j.resource_id IN (SELECT BIN_TO_UUID(id) FROM mcp_tools WHERE tenant_id=r.tenant_id AND server_id=?))))")
        .bind(tenant).bind(resource_type).bind(id).bind(resource_type).bind(id).fetch_one(pool).await?;
    if referenced {
        return Err(ApiError::conflict(
            "RESOURCE_REFERENCED",
            "An immutable Workflow Draft revision still references this resource",
        ));
    }
    Ok(())
}

pub(crate) fn snapshot_from_row(row: sqlx::mysql::MySqlRow) -> ApiResult<ResourceVersionSnapshot> {
    Ok(ResourceVersionSnapshot {
        node_id: row.try_get("node_id")?,
        reference: serde_json::from_value(serde_json::json!({
            "resourceType":row.try_get::<String,_>("resource_type")?,
            "resourceId":row.try_get::<Uuid,_>("resource_id")?,
            "resourceVersionId":row.try_get::<Option<Uuid>,_>("resource_version_id")?,
            "operation":row.try_get::<String,_>("operation_key")?
        }))
        .map_err(ApiError::internal)?,
        snapshot: row.try_get("snapshot_json")?,
        snapshot_hash: row.try_get("snapshot_hash")?,
    })
}

pub(crate) async fn for_save(
    state: &ControlApiState,
    tenant: Uuid,
    workflow: Uuid,
    draft: Uuid,
    revision: u64,
    document: &WorkflowDefinition,
) -> ApiResult<Vec<ResourceVersionSnapshot>> {
    let row = sqlx::query("SELECT definition_json,resource_snapshot_json FROM workflow_draft_revisions WHERE tenant_id=? AND workflow_id=? AND draft_id=? AND revision=?")
        .bind(tenant).bind(workflow).bind(draft).bind(revision).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Draft revision"))?;
    let old: WorkflowDefinition =
        serde_json::from_value(row.try_get("definition_json")?).map_err(ApiError::internal)?;
    let frozen: Vec<ResourceVersionSnapshot> =
        serde_json::from_value(row.try_get::<Value, _>("resource_snapshot_json")?)
            .map_err(ApiError::internal)?;
    let retained = frozen
        .into_iter()
        .filter(|snapshot| {
            document.nodes.iter().any(|node| {
                node.id == snapshot.node_id
                    && old.nodes.iter().any(|previous| {
                        previous.id == node.id
                            && previous.node_type == node.node_type
                            && previous.type_version == node.type_version
                            && previous.parameters.get("workflowVersionId")
                                == node.parameters.get("workflowVersionId")
                            && serde_json::to_value(&previous.resource_references).ok()
                                == serde_json::to_value(&node.resource_references).ok()
                    })
            })
        })
        .collect::<Vec<_>>();
    crate::workflow_resources::resolve_snapshots(
        state, tenant, workflow, document, &retained, false,
    )
    .await
}
