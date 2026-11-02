//! Page publication management; copying and activation are durable Publisher work.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};
use agentx_runtime_contracts::{ApplicationPagePublicationV1, ApplicationPageViewV1};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

pub(crate) fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/applications/{application}/ui/publications",
            get(list).post(publish),
        )
        .route(
            "/api/v1/applications/{application}/ui/drafts/{draft}/checks",
            post(record_check),
        )
        .route(
            "/api/v1/applications/{application}/ui/publications/{publication}/preview",
            get(preview),
        )
}
pub(crate) fn apply_openapi(document: &mut Value) {
    for (path, method, input, output) in [
        (
            "/api/v1/applications/{application}/ui/publications",
            "get",
            None,
            "PagePublications",
        ),
        (
            "/api/v1/applications/{application}/ui/publications",
            "post",
            Some("PublishPage"),
            "PagePublicationStatus",
        ),
        (
            "/api/v1/applications/{application}/ui/drafts/{draft}/checks",
            "post",
            Some("PageCheckReport"),
            "PageCheckReceipt",
        ),
        (
            "/api/v1/applications/{application}/ui/publications/{publication}/preview",
            "get",
            None,
            "ApplicationPageViewV1",
        ),
    ] {
        let parameters=path.split('/').filter_map(|part|part.strip_prefix('{').and_then(|s|s.strip_suffix('}'))).map(|name|json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}})).collect::<Vec<_>>();
        let mut operation = json!({"parameters":parameters,"responses":{"200":{"description":"Application page publication result","content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{output}")}}}},"409":{"description":"Page revision or address conflict"}},"security":[{"bearerAuth":[]}]});
        if let Some(input) = input {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}});
        }
        document["paths"][path][method] = operation;
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublishPage {
    pub draft_id: Option<Uuid>,
    pub ui_version_id: Option<Uuid>,
    pub expected_draft_revision: Option<u64>,
    pub expected_publication_revision: u64,
    pub slug: String,
    pub enabled: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicationStatus {
    id: Uuid,
    ui_version_id: Uuid,
    revision: u64,
    status: String,
    slug: String,
    error: Option<String>,
    manifest: Option<agentx_runtime_contracts::ApplicationPagePublicationV1>,
}
#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PagePublications {
    revision: u64,
    slug: Option<String>,
    enabled: bool,
    active_publication_id: Option<Uuid>,
    publications: Vec<PublicationStatus>,
}

async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
) -> ApiResult<Json<PagePublications>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    let head =
        sqlx::query("SELECT * FROM application_web_heads WHERE tenant_id=? AND application_id=?")
            .bind(actor.tenant_id)
            .bind(application)
            .fetch_optional(&state.pool)
            .await?;
    let rows=sqlx::query("SELECT * FROM application_ui_publications WHERE tenant_id=? AND application_id=? ORDER BY revision DESC LIMIT 50").bind(actor.tenant_id).bind(application).fetch_all(&state.pool).await?;
    Ok(Json(PagePublications {
        revision: head
            .as_ref()
            .map(|r| r.try_get("revision"))
            .transpose()?
            .unwrap_or(0),
        slug: head.as_ref().map(|r| r.try_get("slug")).transpose()?,
        enabled: head
            .as_ref()
            .map(|r| r.try_get("enabled"))
            .transpose()?
            .unwrap_or(false),
        active_publication_id: head
            .as_ref()
            .map(|r| r.try_get("publication_id"))
            .transpose()?
            .flatten(),
        publications: rows.into_iter().map(status).collect::<ApiResult<_>>()?,
    }))
}
fn status(row: sqlx::mysql::MySqlRow) -> ApiResult<PublicationStatus> {
    let source: Option<Value> = row.try_get("manifest_json")?;
    let manifest = source
        .filter(|value| value.get("publicationId").is_some())
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?;
    Ok(PublicationStatus {
        id: row.try_get("id")?,
        ui_version_id: row.try_get("ui_version_id")?,
        revision: row.try_get("revision")?,
        status: row.try_get("status")?,
        slug: row.try_get("slug")?,
        error: row.try_get("last_error")?,
        manifest,
    })
}

async fn publish(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
    Json(input): Json<PublishPage>,
) -> ApiResult<Json<PublicationStatus>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    validate_slug(&input.slug)?;
    let mut tx = state.pool.begin().await?;
    // Serialize first publication too; there is always an Application row to lock.
    sqlx::query(
        "SELECT id FROM applications WHERE tenant_id=? AND id=? AND status='active' FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(application)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Active application"))?;
    let head=sqlx::query("SELECT revision FROM application_web_heads WHERE tenant_id=? AND application_id=? FOR UPDATE").bind(actor.tenant_id).bind(application).fetch_optional(&mut *tx).await?;
    let revision = head
        .as_ref()
        .map(|r| r.try_get::<u64, _>("revision"))
        .transpose()?
        .unwrap_or(0);
    if revision != input.expected_publication_revision {
        return Err(ApiError::conflict(
            "PAGE_PUBLICATION_CONFLICT",
            "The active page changed; reload before publishing",
        ));
    }
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM application_ui_publications WHERE tenant_id=? AND application_id=? AND status IN ('copying','preparing','activating'))").bind(actor.tenant_id).bind(application).fetch_one(&mut *tx).await?;
    if pending {
        return Err(ApiError::conflict(
            "PAGE_PUBLICATION_PENDING",
            "A page publication is already being prepared",
        ));
    }
    let version = match (input.draft_id, input.ui_version_id) {
        (Some(draft), None) => {
            let page=sqlx::query("SELECT * FROM application_ui_drafts WHERE tenant_id=? AND application_id=? AND id=? FOR UPDATE").bind(actor.tenant_id).bind(application).bind(draft).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Page draft"))?;
            if Some(page.try_get::<u64, _>("revision")?) != input.expected_draft_revision {
                return Err(ApiError::conflict(
                    "PAGE_REVISION_CONFLICT",
                    "Save and review the exact page before publishing",
                ));
            }
            let html: String = page.try_get("html")?;
            if html.trim().is_empty() {
                return Err(ApiError::unprocessable(
                    "PAGE_EMPTY",
                    "Generate a page before publishing",
                ));
            }
            let checks:i64=sqlx::query_scalar("SELECT COUNT(DISTINCT mode) FROM application_ui_checks WHERE tenant_id=? AND ui_draft_id=? AND html_hash=? AND contract_hash=? AND passed=TRUE")
                .bind(actor.tenant_id).bind(draft).bind(page.try_get::<String,_>("html_hash")?).bind(page.try_get::<String,_>("contract_hash")?).fetch_one(&mut *tx).await?;
            if checks != 2 {
                return Err(ApiError::unprocessable(
                    "PAGE_CHECKS_REQUIRED",
                    "Run both static and isolated preview checks for the saved page",
                ));
            }
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO application_ui_versions(id,tenant_id,application_id,ui_draft_id,draft_revision,contract_json,contract_hash,html,html_hash,html_object_id,created_by) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
                .bind(id).bind(actor.tenant_id).bind(application).bind(draft).bind(page.try_get::<u64,_>("revision")?).bind(page.try_get::<Value,_>("contract_json")?).bind(page.try_get::<String,_>("contract_hash")?).bind(html).bind(page.try_get::<String,_>("html_hash")?).bind(Uuid::now_v7()).bind(actor.user_id).execute(&mut *tx).await?;
            id
        }
        (None, Some(version)) => {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM application_ui_versions v JOIN application_ui_publications p ON p.tenant_id=v.tenant_id AND p.ui_version_id=v.id WHERE v.tenant_id=? AND v.application_id=? AND v.id=? AND p.status IN ('active','superseded'))").bind(actor.tenant_id).bind(application).bind(version).fetch_one(&mut *tx).await?;
            if !exists {
                return Err(ApiError::not_found("Previously published page version"));
            }
            version
        }
        _ => {
            return Err(ApiError::bad_request(
                "PAGE_SOURCE_REQUIRED",
                "Choose exactly one saved draft or immutable page version",
            ));
        }
    };
    let id = Uuid::now_v7();
    let next:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(revision),0)+1 AS UNSIGNED) FROM application_ui_publications WHERE tenant_id=? AND application_id=?").bind(actor.tenant_id).bind(application).fetch_one(&mut *tx).await?;
    // Reserve the address before any remote activation. The Runtime head remains untouched.
    sqlx::query("INSERT INTO application_web_heads(tenant_id,application_id,slug) VALUES(?,?,?) ON DUPLICATE KEY UPDATE slug=IF(application_id=VALUES(application_id),VALUES(slug),slug)")
        .bind(actor.tenant_id).bind(application).bind(&input.slug).execute(&mut *tx).await.map_err(slug_error)?;
    let reserved: Option<Uuid> = sqlx::query_scalar(
        "SELECT application_id FROM application_web_heads WHERE tenant_id=? AND slug=?",
    )
    .bind(actor.tenant_id)
    .bind(&input.slug)
    .fetch_optional(&mut *tx)
    .await?;
    if reserved != Some(application) {
        return Err(ApiError::conflict(
            "PAGE_SLUG_TAKEN",
            "This suffix belongs to another application",
        ));
    }
    sqlx::query("INSERT INTO application_ui_publications(id,tenant_id,application_id,ui_version_id,slug,revision,expected_revision,status,manifest_json) VALUES(?,?,?,?,?,?,?,'copying',?)")
        .bind(id).bind(actor.tenant_id).bind(application).bind(version).bind(input.slug).bind(next).bind(revision).bind(json!({"enabled":input.enabled})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(status(
        sqlx::query("SELECT * FROM application_ui_publications WHERE id=?")
            .bind(id)
            .fetch_one(&state.pool)
            .await?,
    )?))
}

fn validate_slug(slug: &str) -> ApiResult<()> {
    if !(3..=80).contains(&slug.len())
        || !slug.as_bytes()[0].is_ascii_lowercase()
        || !slug
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || slug.ends_with('-')
        || slug.contains("--")
    {
        return Err(ApiError::bad_request(
            "PAGE_SLUG_INVALID",
            "Use 3–80 lowercase letters, numbers and single hyphens, starting with a letter",
        ));
    }
    Ok(())
}
fn slug_error(error: sqlx::Error) -> ApiError {
    if error
        .as_database_error()
        .is_some_and(|e| e.is_unique_violation())
    {
        ApiError::conflict(
            "PAGE_SLUG_TAKEN",
            "This suffix belongs to another application",
        )
    } else {
        ApiError::from(error)
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PageCheckReport {
    html_hash: agentx_runtime_contracts::ContentHash,
    contract_hash: agentx_runtime_contracts::ContentHash,
    sdk_version: u32,
    mode: String,
    synthetic: bool,
    formal_requests: u32,
    errors: Vec<Value>,
    fields: Vec<Value>,
    actions: Vec<String>,
    #[schemars(length(max = 8000))]
    diagnostic_text: String,
}
#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct PageCheckReceipt {
    recorded: bool,
}
async fn record_check(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, draft)): Path<(Uuid, Uuid)>,
    Json(input): Json<PageCheckReport>,
) -> ApiResult<Json<PageCheckReceipt>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    if !matches!(input.mode.as_str(), "static" | "preview")
        || !input.synthetic
        || input.formal_requests != 0
        || input.sdk_version != agentx_runtime_contracts::APPLICATION_PAGE_SDK_VERSION
    {
        return Err(ApiError::bad_request(
            "PAGE_CHECK_INVALID",
            "Only isolated checks for the supported SDK are accepted",
        ));
    }
    if input.diagnostic_text.chars().count() > 8000 {
        return Err(ApiError::bad_request(
            "PAGE_REPORT_TOO_LARGE",
            "Narrow the visible preview diagnostic to 8000 characters",
        ));
    }
    let mut tx = state.pool.begin().await?;
    let row=sqlx::query("SELECT html_hash,contract_hash FROM application_ui_drafts WHERE tenant_id=? AND application_id=? AND id=? FOR SHARE").bind(actor.tenant_id).bind(application).bind(draft).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Page draft"))?;
    if input.html_hash.as_str() != row.try_get::<String, _>("html_hash")?
        || input.contract_hash.as_str() != row.try_get::<String, _>("contract_hash")?
    {
        return Err(ApiError::conflict(
            "PAGE_CHECK_STALE",
            "The saved page changed during inspection",
        ));
    }
    let report = json!({"htmlHash":input.html_hash,"contractHash":input.contract_hash,"sdkVersion":input.sdk_version,"mode":input.mode,"synthetic":true,"formalRequests":0,"errors":input.errors,"fields":input.fields,"actions":input.actions,"diagnosticText":input.diagnostic_text});
    if report.to_string().len() > 65536 {
        return Err(ApiError::bad_request(
            "PAGE_REPORT_TOO_LARGE",
            "Narrow the check report",
        ));
    }
    sqlx::query("INSERT INTO application_ui_checks(tenant_id,ui_draft_id,html_hash,contract_hash,mode,passed,report_json) VALUES(?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE passed=VALUES(passed),report_json=VALUES(report_json),checked_at=UTC_TIMESTAMP(6)")
        .bind(actor.tenant_id).bind(draft).bind(input.html_hash.as_str()).bind(input.contract_hash.as_str()).bind(input.mode).bind(input.errors.is_empty()).bind(report).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(PageCheckReceipt { recorded: true }))
}

async fn preview(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, publication)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApplicationPageViewV1>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    let row = sqlx::query(
        "SELECT manifest_json FROM application_ui_publications WHERE tenant_id=? AND application_id=? AND id=? AND status IN ('active','superseded')",
    )
    .bind(actor.tenant_id)
    .bind(application)
    .bind(publication)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Published page"))?;
    let manifest: ApplicationPagePublicationV1 =
        serde_json::from_value(row.try_get("manifest_json")?).map_err(ApiError::internal)?;
    let asset =
        async |object: &agentx_runtime_contracts::RuntimeObjectReferenceV1| -> ApiResult<String> {
            let key: String =
                sqlx::query_scalar("SELECT storage_key FROM artifacts WHERE tenant_id=? AND id=?")
                    .bind(actor.tenant_id)
                    .bind(object.object_id)
                    .fetch_optional(&state.pool)
                    .await?
                    .ok_or_else(|| ApiError::not_found("Published page asset"))?;
            let bytes = state
                .control_objects
                .get(&object_store::path::Path::from(key))
                .await
                .map_err(ApiError::internal)?
                .bytes()
                .await
                .map_err(ApiError::internal)?;
            if bytes.len() as u64 != object.size_bytes
                || format!("sha256:{:x}", Sha256::digest(&bytes)) != object.content_hash.as_str()
            {
                return Err(ApiError::conflict(
                    "PAGE_ASSET_MISMATCH",
                    "The published page asset failed verification",
                ));
            }
            String::from_utf8(bytes.to_vec()).map_err(ApiError::internal)
        };
    let (html, sdk_script, stylesheet) = tokio::try_join!(
        asset(&manifest.html),
        asset(&manifest.sdk),
        asset(&manifest.stylesheet)
    )?;
    Ok(Json(ApplicationPageViewV1 {
        publication: manifest,
        html,
        sdk_script,
        stylesheet,
        route_key: application.to_string(),
        historical: true,
        task_id: None,
    }))
}
