//! Signed, immutable page publications. Runtime never fetches HTML from Control.
use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};
use agentx_runtime_contracts::{
    ActivatePagePublicationV1, ApplicationAccessBindingV1, ApplicationPagePublicationV1,
    PreparePagePublicationV1, content_hash, verify_canonical,
};
use axum::{Json, extract::State, http::HeaderMap};
use object_store::path::Path as ObjectPath;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

pub(crate) fn conflict(message: &str) -> RuntimeError {
    RuntimeError::StateConflict("PAGE_PUBLICATION_CONFLICT", message.into())
}

pub fn validate_slug(slug: &str) -> RuntimeResult<()> {
    if !(3..=80).contains(&slug.len())
        || !slug.as_bytes()[0].is_ascii_lowercase()
        || !slug
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || slug.ends_with('-')
        || slug.contains("--")
    {
        return Err(RuntimeError::InvalidRequest(
            "PAGE_SLUG_INVALID",
            "Use 3–80 lowercase letters, numbers and single hyphens, starting with a letter".into(),
        ));
    }
    Ok(())
}

pub async fn prepare(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<PreparePagePublicationV1>,
) -> RuntimeResult<Json<Value>> {
    state.trust.publisher(&headers, "runtime.pages.prepare")?;
    let page = &request.publication;
    verify_canonical(
        state.trust.bundle_key(&request.signature.key_id)?,
        page,
        &request.signature,
    )
    .map_err(|_| conflict("The page publication signature is invalid"))?;
    if content_hash(page).map_err(|e| RuntimeError::Internal(e.into()))? != request.content_hash
        || content_hash(&page.contract).map_err(|e| RuntimeError::Internal(e.into()))?
            != page.contract_hash
        || page.application_id != page.contract.application_id
        || page.tenant_id != page.html.tenant_id
        || page.revision <= page.expected_revision
        || page.contract.sdk_version != agentx_runtime_contracts::APPLICATION_PAGE_SDK_VERSION
        || page.style_version != agentx_runtime_contracts::APPLICATION_PAGE_STYLE_VERSION
        || !page.html.has_canonical_key()
        || page.html.media_type != "text/html"
        || page.html.size_bytes > agentx_runtime_contracts::APPLICATION_PAGE_MAX_BYTES as u64
    {
        return Err(conflict(
            "Page materials, versions or frozen contract do not match",
        ));
    }
    validate_slug(&page.slug)?;
    let html = read_asset(&state, &page.html).await?;
    if content_hash(&html).map_err(|e| RuntimeError::Internal(e.into()))? != page.html_hash {
        return Err(conflict("HTML content failed its immutable hash check"));
    }
    for (reference, kind, expected) in [
        (
            &page.sdk,
            "application/javascript",
            agentx_runtime_contracts::APPLICATION_PAGE_SDK_SOURCE,
        ),
        (
            &page.stylesheet,
            "text/css",
            agentx_runtime_contracts::APPLICATION_PAGE_STYLE_SOURCE,
        ),
    ] {
        if reference.tenant_id != page.tenant_id
            || reference.media_type != kind
            || reference.size_bytes > 256 * 1024
            || !reference.has_canonical_key()
        {
            return Err(conflict(
                "SDK or stylesheet metadata does not match this page",
            ));
        }
        let source = read_asset(&state, reference).await?;
        if source != expected {
            return Err(conflict(
                "SDK or stylesheet bytes do not match the declared platform version",
            ));
        }
    }
    let mut tx = state.pool.begin().await?;
    // Lock the Bundle before references, matching bundle GC's lock order.
    let bundle=sqlx::query("SELECT payload_json FROM deployment_bundles WHERE tenant_id=? AND application_id=? AND deployment_id=? AND id=? AND status IN ('active','superseded','retained') FOR SHARE")
        .bind(page.tenant_id).bind(page.application_id).bind(page.contract.deployment_id).bind(page.contract.bundle_id).fetch_optional(&mut *tx).await?.ok_or_else(||conflict("The published business Bundle is unavailable"))?;
    let spec: agentx_runtime_contracts::ExecutionSpecPayloadV2 =
        serde_json::from_value(bundle.try_get("payload_json")?)
            .map_err(|e| RuntimeError::Internal(e.into()))?;
    if spec.workflow_version_id != page.contract.workflow_version_id
        || spec.input_contract != page.contract.input_schema
        || spec.output_contract != page.contract.output_schema
    {
        return Err(conflict(
            "The page contract belongs to a different business Bundle",
        ));
    }
    for asset in [&page.html, &page.sdk, &page.stylesheet] {
        let object_ok=sqlx::query("SELECT object_id FROM runtime_objects WHERE tenant_id=? AND object_id=? AND content_hash=? AND size_bytes=? AND media_type=? AND status='ready' FOR SHARE").bind(page.tenant_id).bind(asset.object_id).bind(asset.content_hash.as_str()).bind(asset.size_bytes).bind(&asset.media_type).fetch_optional(&mut *tx).await?;
        if object_ok.is_none() {
            return Err(conflict(
                "Page materials have not completed Runtime preparation",
            ));
        }
    }
    if let Some(existing)=sqlx::query_scalar::<_,String>("SELECT content_hash FROM application_page_publications WHERE tenant_id=? AND id=? FOR UPDATE").bind(page.tenant_id).bind(page.publication_id).fetch_optional(&mut *tx).await? {
        if existing!=request.content_hash.as_str(){return Err(conflict("Publication identity was reused with different materials"));}
        return Ok(Json(json!({"prepared":true,"replayed":true})));
    }
    sqlx::query("INSERT INTO application_page_publications(id,tenant_id,application_id,ui_version_id,bundle_id,revision,content_hash,manifest_json,html_object_id,sdk_object_id,style_object_id,status) VALUES(?,?,?,?,?,?,?,?,?,?,?,'prepared')")
        .bind(page.publication_id).bind(page.tenant_id).bind(page.application_id).bind(page.ui_version_id).bind(page.contract.bundle_id).bind(page.revision).bind(request.content_hash.as_str()).bind(serde_json::to_value(page).map_err(|e|RuntimeError::Internal(e.into()))?).bind(page.html.object_id).bind(page.sdk.object_id).bind(page.stylesheet.object_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'application_page',?) ON DUPLICATE KEY UPDATE id=id")
        .bind(Uuid::now_v7()).bind(page.tenant_id).bind(page.contract.bundle_id).bind(page.publication_id).execute(&mut *tx).await?;
    for (asset, role) in [
        (&page.html, "html"),
        (&page.sdk, "sdk"),
        (&page.stylesheet, "style"),
    ] {
        let existing: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifacts WHERE tenant_id=? AND id=?)")
                .bind(page.tenant_id)
                .bind(asset.object_id)
                .fetch_one(&mut *tx)
                .await?;
        if !existing {
            crate::quota::reserve(
                &mut tx,
                page.tenant_id,
                "artifact_bytes",
                "artifact",
                &asset.object_id.to_string(),
                &format!("page-artifact:{}", asset.object_id),
                asset.size_bytes,
                300,
            )
            .await?;
            sqlx::query("INSERT INTO artifacts(id,tenant_id,content_type,size_bytes,sha256,storage_key) VALUES(?,?,?,?,?,?)").bind(asset.object_id).bind(page.tenant_id).bind(&asset.media_type).bind(asset.size_bytes).bind(asset.content_hash.as_str().strip_prefix("sha256:").expect("validated ContentHash")).bind(&asset.object_key).execute(&mut *tx).await?;
            crate::quota::settle(
                &mut tx,
                page.tenant_id,
                "artifact_bytes",
                "artifact",
                &asset.object_id.to_string(),
                asset.size_bytes,
                &format!("page-artifact:{}:usage", asset.object_id),
            )
            .await?;
        }
        sqlx::query("INSERT IGNORE INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'application_page',?,?)").bind(page.tenant_id).bind(asset.object_id).bind(page.publication_id.to_string()).bind(role).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"prepared":true,"replayed":false})))
}

pub async fn activate(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<ActivatePagePublicationV1>,
) -> RuntimeResult<Json<Value>> {
    state.trust.publisher(&headers, "runtime.pages.activate")?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT application_id FROM application_routes WHERE tenant_id=? AND application_id=? AND status='active' FOR UPDATE").bind(request.tenant_id).bind(request.application_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    let row=sqlx::query("SELECT manifest_json FROM application_page_publications WHERE tenant_id=? AND application_id=? AND id=? FOR UPDATE").bind(request.tenant_id).bind(request.application_id).bind(request.publication_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    let page: ApplicationPagePublicationV1 = serde_json::from_value(row.try_get("manifest_json")?)
        .map_err(|e| RuntimeError::Internal(e.into()))?;
    if page.expected_revision != request.expected_revision || page.enabled != request.enabled {
        return Err(conflict("Activation does not match the signed publication"));
    }
    let head=sqlx::query("SELECT publication_id,revision FROM application_web_heads WHERE tenant_id=? AND application_id=? FOR UPDATE").bind(request.tenant_id).bind(request.application_id).fetch_optional(&mut *tx).await?;
    if head
        .as_ref()
        .is_some_and(|r| r.try_get::<Uuid, _>("publication_id").ok() == Some(page.publication_id))
    {
        return Ok(Json(
            json!({"activated":true,"revision":page.revision,"replayed":true}),
        ));
    }
    let revision = head
        .as_ref()
        .map(|row| row.try_get::<u64, _>("revision"))
        .transpose()?
        .unwrap_or(0);
    if revision != request.expected_revision {
        return Err(conflict("Another page publication is already active"));
    }
    // One pointer switches the HTML, SDK, contract and Bundle together.
    let result = if head.is_some() {
        sqlx::query("UPDATE application_web_heads SET publication_id=?,slug=?,revision=?,enabled=? WHERE tenant_id=? AND application_id=? AND revision=?")
            .bind(page.publication_id).bind(&page.slug).bind(page.revision).bind(page.enabled).bind(page.tenant_id).bind(page.application_id).bind(request.expected_revision).execute(&mut *tx).await
    } else {
        sqlx::query("INSERT INTO application_web_heads(tenant_id,application_id,publication_id,slug,revision,enabled) VALUES(?,?,?,?,?,?)")
            .bind(page.tenant_id).bind(page.application_id).bind(page.publication_id).bind(&page.slug).bind(page.revision).bind(page.enabled).execute(&mut *tx).await
    };
    result.map_err(|error| {
        if error
            .as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            conflict("This page suffix belongs to another application")
        } else {
            RuntimeError::from(error)
        }
    })?;
    sqlx::query("UPDATE application_page_publications SET status=IF(id=?,'active','superseded'),activated_at=IF(id=?,UTC_TIMESTAMP(6),activated_at) WHERE tenant_id=? AND application_id=? AND (status='active' OR id=?)")
        .bind(page.publication_id).bind(page.publication_id).bind(page.tenant_id).bind(page.application_id).bind(page.publication_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"activated":true,"revision":page.revision,"replayed":false}),
    ))
}

pub(crate) async fn resolve_binding(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    application: Uuid,
    binding: &ApplicationAccessBindingV1,
) -> RuntimeResult<Uuid> {
    match binding {
        ApplicationAccessBindingV1::Web {
            publication_id,
            revision,
        } => {
            let bundle:Option<Uuid>=sqlx::query_scalar("SELECT p.bundle_id FROM application_web_heads h JOIN application_page_publications p ON p.tenant_id=h.tenant_id AND p.id=h.publication_id WHERE h.tenant_id=? AND h.application_id=? AND h.publication_id=? AND h.revision=? AND h.enabled=TRUE AND p.status='active' FOR SHARE")
                .bind(tenant).bind(application).bind(publication_id).bind(revision).fetch_optional(&mut **tx).await?;
            bundle.ok_or_else(|| {
                conflict("The application page changed or was disabled; reload before submitting")
            })
        }
        ApplicationAccessBindingV1::Mcp { .. } => {
            crate::application_mcp::resolve(tx, tenant, application, binding).await
        }
    }
}

pub(crate) async fn read_asset(
    state: &RuntimeState,
    asset: &agentx_runtime_contracts::RuntimeObjectReferenceV1,
) -> RuntimeResult<String> {
    if !asset.has_canonical_key()
        || asset.size_bytes > agentx_runtime_contracts::APPLICATION_PAGE_MAX_BYTES as u64
    {
        return Err(RuntimeError::Unavailable);
    }
    let bytes = state
        .objects
        .get(&ObjectPath::from(asset.object_key.clone()))
        .await
        .map_err(|_| RuntimeError::Unavailable)?
        .bytes()
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    if bytes.len() as u64 != asset.size_bytes
        || format!("sha256:{:x}", Sha256::digest(&bytes)) != asset.content_hash.as_str()
    {
        return Err(RuntimeError::Unavailable);
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| RuntimeError::Unavailable)
}
