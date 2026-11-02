//! Immutable application MCP publications coexist until explicitly retracted.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};
use agentx_runtime_contracts::{
    APPLICATION_MCP_PROTOCOL, ApplicationMcpPublicationV1, ApplicationMcpRemovalV1,
    ExecutionSpecPayloadV2, content_hash,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublishMcp {
    deployment_id: Uuid,
    expected_revision: u64,
    slug: String,
    tool_name: String,
    description: String,
    enabled: bool,
}
#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpRevision {
    publication: ApplicationMcpPublicationV1,
    status: String,
    error: Option<String>,
    created_at: String,
    deleted: bool,
    deletion_status: Option<String>,
}
pub(crate) fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/applications/{application}/mcp/publications",
            get(list).post(publish),
        )
        .route(
            "/api/v1/applications/{application}/mcp/publications/{publication}",
            delete(remove),
        )
        .route(
            "/api/v1/applications/{application}/mcp/diagnostics",
            get(crate::application_mcp_diagnostics::inspect),
        )
}
async fn revisions(
    state: &ControlApiState,
    tenant: Uuid,
    application: Uuid,
) -> ApiResult<Vec<McpRevision>> {
    let rows=sqlx::query("SELECT p.manifest_json,o.status,COALESCE(d.last_error,o.last_error) last_error,p.created_at,p.deleted_at IS NOT NULL deleted,d.status deletion_status FROM application_mcp_publications p JOIN outbox o ON o.id=p.outbox_id LEFT JOIN outbox d ON d.id=p.deletion_outbox_id WHERE p.tenant_id=? AND p.application_id=? ORDER BY p.revision DESC").bind(tenant).bind(application).fetch_all(&state.pool).await?;
    rows.into_iter()
        .map(|row| {
            Ok(McpRevision {
                publication: serde_json::from_value(row.try_get("manifest_json")?)
                    .map_err(ApiError::internal)?,
                status: row.try_get("status")?,
                error: row.try_get("last_error")?,
                created_at: row
                    .try_get::<time::OffsetDateTime, _>("created_at")?
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(ApiError::internal)?,
                deleted: row.try_get("deleted")?,
                deletion_status: row.try_get("deletion_status")?,
            })
        })
        .collect()
}
async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
) -> ApiResult<Json<Vec<McpRevision>>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    Ok(Json(revisions(&state, actor.tenant_id, application).await?))
}
async fn publish(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
    Json(input): Json<PublishMcp>,
) -> ApiResult<Json<McpRevision>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    if !(3..=80).contains(&input.slug.len())
        || !input.slug.as_bytes()[0].is_ascii_lowercase()
        || !input
            .slug
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || input.slug.ends_with('-')
        || input.slug.contains("--")
    {
        return Err(ApiError::bad_request(
            "MCP_SLUG_INVALID",
            "Use 3–80 lowercase letters, numbers and single hyphens, starting with a letter",
        ));
    }
    let mut tx = state.pool.begin().await?;
    let title: String = sqlx::query_scalar(
        "SELECT name FROM applications WHERE tenant_id=? AND id=? AND status='active' FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(application)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Application"))?;
    let revision=sqlx::query_scalar::<_,u64>("SELECT revision FROM application_mcp_heads WHERE tenant_id=? AND application_id=? FOR UPDATE").bind(actor.tenant_id).bind(application).fetch_optional(&mut *tx).await?.unwrap_or(0);
    if revision != input.expected_revision {
        return Err(ApiError::conflict(
            "MCP_REVISION_CONFLICT",
            "Refresh publications before editing",
        ));
    }
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM application_mcp_publications p JOIN outbox o ON o.id=p.outbox_id LEFT JOIN outbox d ON d.id=p.deletion_outbox_id WHERE p.tenant_id=? AND p.application_id=? AND (o.status<>'published' OR (d.id IS NOT NULL AND d.status<>'published')))").bind(actor.tenant_id).bind(application).fetch_one(&mut *tx).await?;
    if pending {
        return Err(ApiError::conflict(
            "MCP_PUBLICATION_PENDING",
            "Wait for the previous publication or deletion to settle",
        ));
    }
    sqlx::query(
        "INSERT IGNORE INTO application_mcp_routes(slug,tenant_id,application_id) VALUES(?,?,?)",
    )
    .bind(&input.slug)
    .bind(actor.tenant_id)
    .bind(application)
    .execute(&mut *tx)
    .await?;
    let owner = sqlx::query(
        "SELECT tenant_id,application_id FROM application_mcp_routes WHERE slug=? FOR UPDATE",
    )
    .bind(&input.slug)
    .fetch_one(&mut *tx)
    .await?;
    if owner.try_get::<Uuid, _>("tenant_id")? != actor.tenant_id
        || owner.try_get::<Uuid, _>("application_id")? != application
    {
        return Err(ApiError::conflict(
            "MCP_SLUG_CONFLICT",
            "This MCP address belongs to another application",
        ));
    }
    let raw:Value=sqlx::query_scalar("SELECT b.payload_json FROM application_deployments d JOIN execution_spec_bundles b ON b.tenant_id=d.tenant_id AND b.deployment_id=d.id WHERE d.tenant_id=? AND d.application_id=? AND d.id=? AND d.status IN ('active','superseded') AND b.status='published'").bind(actor.tenant_id).bind(application).bind(input.deployment_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::conflict("MCP_DEPLOYMENT_REQUIRED","Select a published application deployment"))?;
    let bundle: ExecutionSpecPayloadV2 = serde_json::from_value(raw).map_err(ApiError::internal)?;
    let chat_mapping:Option<Value>=sqlx::query_scalar("SELECT mapping_json FROM application_playground_configs WHERE tenant_id=? AND deployment_id=? AND publish_status='active' AND published_version=version").bind(actor.tenant_id).bind(bundle.deployment_id).fetch_optional(&mut *tx).await?.flatten();
    let chat_mapping: Option<agentx_runtime_contracts::ChatMappingV1> = chat_mapping
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?;
    let definition =
        serde_json::from_value(bundle.definition.clone()).map_err(ApiError::internal)?;
    let waits =
        crate::workflow_wait_inputs::discover(&state.pool, actor.tenant_id, &definition).await?;
    let mut schemas = Vec::new();
    for wait in waits {
        let schema = wait.response_schema.unwrap_or_else(|| json!({}));
        if !schemas.contains(&schema) {
            schemas.push(schema);
        }
    }
    let wait_input_schema = match schemas.len() {
        0 => json!({"not":{}}),
        1 => schemas.remove(0),
        _ => json!({"anyOf":schemas}),
    };
    let publication=ApplicationMcpPublicationV1 { tenant_id:actor.tenant_id,application_id:application,publication_id:Uuid::now_v7(),deployment_id:bundle.deployment_id,bundle_id:bundle.bundle_id,revision:revision+1,expected_revision:revision,slug:input.slug,tool_name:input.tool_name,title,description:input.description,input_schema:bundle.input_contract.clone(),output_schema:bundle.output_contract.clone(),wait_input_schema:wait_input_schema.clone(),chat_mapping:chat_mapping.clone(),contract_hash:content_hash(&json!({"input":bundle.input_contract,"output":bundle.output_contract,"waitInputSchema":wait_input_schema,"chatMapping":chat_mapping})).map_err(ApiError::internal)?,enabled:input.enabled,protocol_version:APPLICATION_MCP_PROTOCOL.into() };
    publication
        .validate()
        .map_err(|message| ApiError::bad_request("MCP_PUBLICATION_INVALID", message))?;
    let outbox = Uuid::now_v7();
    let payload = json!({"publication":publication,"admissionEpoch":publication.revision});
    sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,'ApplicationMcpChanged','application_mcp_admission',?,?,'pending',?,?)").bind(outbox).bind(actor.tenant_id).bind(application.to_string()).bind(&payload).bind(content_hash(&payload).map_err(ApiError::internal)?.as_str()).bind(format!("application-mcp:{}",publication.publication_id)).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO application_mcp_publications(id,tenant_id,application_id,revision,slug,manifest_json,outbox_id,created_by) VALUES(?,?,?,?,?,?,?,?)").bind(publication.publication_id).bind(actor.tenant_id).bind(application).bind(publication.revision).bind(&publication.slug).bind(serde_json::to_value(&publication).map_err(ApiError::internal)?).bind(outbox).bind(actor.user_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO application_mcp_heads(tenant_id,application_id,revision) VALUES(?,?,?) ON DUPLICATE KEY UPDATE revision=VALUES(revision)").bind(actor.tenant_id).bind(application).bind(publication.revision).execute(&mut *tx).await?;
    tx.commit().await?;
    let result = revisions(&state, actor.tenant_id, application)
        .await?
        .into_iter()
        .find(|row| row.publication.publication_id == publication.publication_id)
        .ok_or_else(|| ApiError::not_found("MCP publication"))?;
    Ok(Json(result))
}
async fn remove(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, publication)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<McpRevision>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM applications WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id)
        .bind(application)
        .fetch_one(&mut *tx)
        .await?;
    let row=sqlx::query("SELECT p.deletion_outbox_id,p.revision,o.status FROM application_mcp_publications p JOIN outbox o ON o.id=p.outbox_id WHERE p.tenant_id=? AND p.application_id=? AND p.id=? FOR UPDATE").bind(actor.tenant_id).bind(application).bind(publication).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("MCP publication"))?;
    if row.try_get::<String, _>("status")? != "published" {
        return Err(ApiError::conflict(
            "MCP_PUBLICATION_PENDING",
            "Wait for publication to settle before deleting",
        ));
    }
    if row
        .try_get::<Option<Uuid>, _>("deletion_outbox_id")?
        .is_none()
    {
        let removal = ApplicationMcpRemovalV1 {
            tenant_id: actor.tenant_id,
            application_id: application,
            publication_id: publication,
        };
        let payload = json!({"removal":removal,"admissionEpoch":row.try_get::<u64,_>("revision")?});
        let outbox = Uuid::now_v7();
        sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,'ApplicationMcpRemoved','application_mcp_admission',?,?,'pending',?,?)").bind(outbox).bind(actor.tenant_id).bind(application.to_string()).bind(&payload).bind(content_hash(&payload).map_err(ApiError::internal)?.as_str()).bind(format!("application-mcp-remove:{publication}")).execute(&mut *tx).await?;
        sqlx::query("UPDATE application_mcp_publications SET deleted_at=UTC_TIMESTAMP(6),deletion_outbox_id=? WHERE tenant_id=? AND id=?").bind(outbox).bind(actor.tenant_id).bind(publication).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let result = revisions(&state, actor.tenant_id, application)
        .await?
        .into_iter()
        .find(|row| row.publication.publication_id == publication)
        .ok_or_else(|| ApiError::not_found("MCP publication"))?;
    Ok(Json(result))
}
/// Release an address only in the same transaction as a fenced deletion receipt.
pub(crate) async fn release_deleted_routes(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    application: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM application_mcp_routes WHERE tenant_id=? AND application_id=? AND NOT EXISTS(SELECT 1 FROM application_mcp_publications p LEFT JOIN outbox d ON d.id=p.deletion_outbox_id WHERE p.tenant_id=? AND p.application_id=? AND p.slug=application_mcp_routes.slug AND (p.deleted_at IS NULL OR d.status<>'published'))").bind(tenant).bind(application).bind(tenant).bind(application).execute(&mut **tx).await?;
    Ok(())
}
pub(crate) fn apply_openapi(document: &mut Value) {
    let parameter = |name| json!({"in":"path","name":name,"required":true,"schema":{"type":"string","format":"uuid"}});
    for (path, method, array, input, diagnostic) in [
        (
            "/api/v1/applications/{application}/mcp/publications",
            "get",
            true,
            None,
            false,
        ),
        (
            "/api/v1/applications/{application}/mcp/publications",
            "post",
            false,
            Some("PublishApplicationMcp"),
            false,
        ),
        (
            "/api/v1/applications/{application}/mcp/publications/{publication}",
            "delete",
            false,
            None,
            false,
        ),
        (
            "/api/v1/applications/{application}/mcp/diagnostics",
            "get",
            false,
            None,
            true,
        ),
    ] {
        let mut parameters = vec![parameter("application")];
        if diagnostic {
            parameters.push(json!({"in":"query","name":"publicationId","required":false,"schema":{"type":"string","format":"uuid"}}));
        }

        if path.ends_with("{publication}") {
            parameters.push(parameter("publication"));
        }
        let schema = if diagnostic {
            json!({"$ref":"#/components/schemas/ApplicationMcpDiagnostic"})
        } else if array {
            json!({"type":"array","items":{"$ref":"#/components/schemas/ApplicationMcpRevision"}})
        } else {
            json!({"$ref":"#/components/schemas/ApplicationMcpRevision"})
        };
        let mut operation = json!({"parameters":parameters,"security":[{"bearerAuth":[]}],"responses":{"200":{"description":"Application MCP publication result","content":{"application/json":{"schema":schema}}}}});
        if let Some(input) = input {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        document["paths"][path][method] = operation;
    }
}
