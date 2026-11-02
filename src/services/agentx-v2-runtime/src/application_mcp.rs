use crate::error::{RuntimeError, RuntimeResult};
use agentx_runtime_contracts::{
    ApplicationAccessBindingV1, ApplicationMcpPublicationV1, ApplicationMcpRemovalV1,
    ExecutionSpecPayloadV2, content_hash, deterministic_uuid,
};
use serde_json::Value;
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;
fn conflict(message: &str) -> RuntimeError {
    RuntimeError::StateConflict("MCP_PUBLICATION_CONFLICT", message.into())
}
pub(crate) async fn apply(
    tx: &mut Transaction<'_, MySql>,
    publication: &ApplicationMcpPublicationV1,
) -> RuntimeResult<()> {
    publication.validate().map_err(|m| conflict(&m))?;
    crate::application_pages::validate_slug(&publication.slug)?;
    let tenant = publication.tenant_id;
    let application = publication.application_id;
    let bundle:Value=sqlx::query_scalar("SELECT payload_json FROM deployment_bundles WHERE tenant_id=? AND application_id=? AND deployment_id=? AND id=? AND status IN ('active','superseded','retained') FOR SHARE")
        .bind(tenant).bind(application).bind(publication.deployment_id).bind(publication.bundle_id).fetch_optional(&mut **tx).await?.ok_or_else(||conflict("The selected published business Bundle is unavailable"))?;
    let bundle: ExecutionSpecPayloadV2 =
        serde_json::from_value(bundle).map_err(|e| RuntimeError::Internal(e.into()))?;
    if bundle.input_contract != publication.input_schema
        || bundle.output_contract != publication.output_schema
    {
        return Err(conflict(
            "MCP schema differs from the immutable business Bundle",
        ));
    }
    if let Some(mapping) = &publication.chat_mapping {
        crate::execution::validate_runtime_chat_mapping(
            mapping,
            &bundle.input_contract,
            &bundle.output_contract,
        )?;
    }
    let hash = content_hash(publication).map_err(|e| RuntimeError::Internal(e.into()))?;
    if let Some(saved) = sqlx::query_scalar::<_, String>(
        "SELECT content_hash FROM application_mcp_publications WHERE tenant_id=? AND id=?",
    )
    .bind(tenant)
    .bind(publication.publication_id)
    .fetch_optional(&mut **tx)
    .await?
    {
        return if saved == hash.as_str() {
            Ok(())
        } else {
            Err(conflict(
                "Publication identity was reused with different content",
            ))
        };
    }
    let head=sqlx::query("SELECT revision FROM application_mcp_heads WHERE tenant_id=? AND application_id=? FOR UPDATE").bind(tenant).bind(application).fetch_optional(&mut **tx).await?;
    let current = head
        .as_ref()
        .map(|h| h.try_get::<u64, _>("revision"))
        .transpose()?
        .unwrap_or(0);
    if current != publication.expected_revision {
        return Err(conflict(
            "Refresh the active MCP revision before publishing",
        ));
    }
    sqlx::query(
        "INSERT IGNORE INTO application_mcp_routes(slug,tenant_id,application_id) VALUES(?,?,?)",
    )
    .bind(&publication.slug)
    .bind(tenant)
    .bind(application)
    .execute(&mut **tx)
    .await?;
    let owner = sqlx::query(
        "SELECT tenant_id,application_id FROM application_mcp_routes WHERE slug=? FOR UPDATE",
    )
    .bind(&publication.slug)
    .fetch_one(&mut **tx)
    .await?;
    if owner.try_get::<Uuid, _>("tenant_id")? != tenant
        || owner.try_get::<Uuid, _>("application_id")? != application
    {
        return Err(conflict("This MCP address belongs to another application"));
    }
    sqlx::query("INSERT INTO application_mcp_heads(tenant_id,application_id,revision) VALUES(?,?,?) ON DUPLICATE KEY UPDATE revision=VALUES(revision)").bind(tenant).bind(application).bind(publication.revision).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO application_mcp_publications(id,tenant_id,application_id,bundle_id,revision,slug,manifest_json,content_hash,status) VALUES(?,?,?,?,?,?,?,?,'active')").bind(publication.publication_id).bind(tenant).bind(application).bind(publication.bundle_id).bind(publication.revision).bind(&publication.slug).bind(serde_json::to_value(publication).map_err(|e|RuntimeError::Internal(e.into()))?).bind(hash.as_str()).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'application_mcp',?)").bind(deterministic_uuid(publication.publication_id,b"bundle")).bind(tenant).bind(publication.bundle_id).bind(publication.publication_id).execute(&mut **tx).await?;
    Ok(())
}
pub(crate) async fn resolve(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    application: Uuid,
    access: &ApplicationAccessBindingV1,
) -> RuntimeResult<Uuid> {
    let ApplicationAccessBindingV1::Mcp {
        publication_id,
        revision,
    } = access
    else {
        return Err(conflict("MCP publication binding required"));
    };
    sqlx::query_scalar("SELECT bundle_id FROM application_mcp_publications WHERE tenant_id=? AND application_id=? AND id=? AND revision=? AND JSON_EXTRACT(manifest_json,'$.enabled')=TRUE AND status='active' FOR SHARE").bind(tenant).bind(application).bind(publication_id).bind(revision).fetch_optional(&mut **tx).await?.ok_or_else(||conflict("MCP schema changed or its entrance is disabled; discover the application again"))
}

/// Retract exactly one immutable publication; never cancel its invocations.
pub(crate) async fn remove(
    tx: &mut Transaction<'_, MySql>,
    removal: &ApplicationMcpRemovalV1,
) -> RuntimeResult<()> {
    let row=sqlx::query("SELECT slug,status FROM application_mcp_publications WHERE tenant_id=? AND application_id=? AND id=? FOR UPDATE").bind(removal.tenant_id).bind(removal.application_id).bind(removal.publication_id).fetch_optional(&mut **tx).await?.ok_or_else(||conflict("The MCP publication does not exist"))?;
    if row.try_get::<String, _>("status")? == "deleted" {
        return Ok(());
    }
    let slug: String = row.try_get("slug")?;
    sqlx::query(
        "UPDATE application_mcp_publications SET status='deleted' WHERE tenant_id=? AND id=?",
    )
    .bind(removal.tenant_id)
    .bind(removal.publication_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM bundle_references WHERE tenant_id=? AND reference_kind='application_mcp' AND owner_id=?").bind(removal.tenant_id).bind(removal.publication_id).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM application_mcp_routes WHERE tenant_id=? AND application_id=? AND slug=? AND NOT EXISTS(SELECT 1 FROM application_mcp_publications p WHERE p.tenant_id=? AND p.application_id=? AND p.slug=? AND p.status='active')").bind(removal.tenant_id).bind(removal.application_id).bind(&slug).bind(removal.tenant_id).bind(removal.application_id).bind(&slug).execute(&mut **tx).await?;
    Ok(())
}
