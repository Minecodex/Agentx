use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
    sync::LazyLock,
};

use agentx_api_types::PageResponse;
use agentx_node_protocol::{
    NodeCapability, NodeManifestVersion, PluginNodeBinding, PluginRuntimeArtifact,
    PluginTraceRenderer, plugin_runtime_object_id,
};
use agentx_runtime::NodeRegistry;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
    routing::{get, patch, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bytes::Bytes;
use object_store::{ObjectStore, path::Path as ObjectPath};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use zip::ZipArchive;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};

pub(crate) use crate::canvas_plugin_resolution::{
    ResolvedWorkflowPlugins, invoke_plugin_method, lock_resolved_plugin_closure,
    resolve_workflow_plugin_closure,
};

const MAX_BUNDLE_BYTES: usize = 20 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FILES: usize = 128;
static PACKAGE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9._-]*/[a-z0-9][a-z0-9._-]*$").unwrap());
static SEMVER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$").unwrap());

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/canvas-plugins", get(list_plugins))
        .route(
            "/api/v1/canvas-plugins/{id}",
            get(get_plugin).patch(update_plugin).delete(delete_plugin),
        )
        .route(
            "/api/v1/canvas-plugins/{id}/versions/{version_id}",
            patch(update_version).delete(delete_version),
        )
        .route(
            "/api/v1/canvas-plugins/{id}/versions/{version_id}/download",
            get(download_version),
        )
        .route(
            "/api/v1/canvas-plugins/{id}/references",
            get(plugin_references),
        )
        .route("/api/v1/canvas-plugins/{id}/audit", get(plugin_audit))
        .route("/api/v1/canvas-plugin-imports", post(create_import))
        .route(
            "/api/v1/canvas-plugin-imports/{id}",
            get(get_import).delete(cancel_import),
        )
        .route(
            "/api/v1/canvas-plugin-imports/{id}/install",
            post(install_import),
        )
        .route("/api/v1/canvas-plugin-sdk/template", get(download_template))
        .layer(DefaultBodyLimit::max(MAX_BUNDLE_BYTES))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackageManifest {
    protocol_version: u32,
    sdk_api_version: u32,
    package_id: String,
    package_version: String,
    display_name: String,
    #[serde(default)]
    description: String,
    nodes: Vec<String>,
    runtime_entry: String,
    #[serde(default)]
    ui_entry: Option<String>,
    #[serde(default)]
    ui_styles_entry: Option<String>,
    #[serde(default)]
    ui_assets: BTreeMap<String, String>,
    #[serde(default)]
    trace_renderers: Vec<PluginTraceRenderer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValidatedPackage {
    package: PackageManifest,
    nodes: Vec<NodeManifestVersion>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    search: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginSummary {
    id: Uuid,
    package_id: String,
    display_name: String,
    description: String,
    source_type: String,
    default_version_id: Option<Uuid>,
    version: u64,
    versions: Vec<PluginVersion>,
    node_count: u64,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginVersion {
    id: Uuid,
    package_version: String,
    bundle_digest: String,
    status: String,
    sdk_api_version: u32,
    node_types: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginAuditItem {
    id: Uuid,
    actor_user_id: Uuid,
    action: String,
    detail: Value,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportResponse {
    id: Uuid,
    status: String,
    bundle_digest: String,
    package_id: String,
    package_version: String,
    display_name: String,
    description: String,
    node_types: Vec<String>,
    issues: Value,
    installed_version_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallRequest {
    bundle_digest: String,
    #[serde(default = "default_true")]
    enable: bool,
    #[serde(default = "default_true")]
    set_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdatePluginRequest {
    default_version_id: Uuid,
    expected_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateVersionRequest {
    enabled: bool,
    expected_revision: u64,
}

fn default_true() -> bool {
    true
}

async fn list_plugins(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<PluginSummary>>> {
    actor.require("canvas_plugin:view")?;
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let status = query.status.unwrap_or_default();
    let rows = sqlx::query("SELECT id,package_id,display_name,description,source_type,default_version_id,version,updated_at FROM canvas_plugins WHERE tenant_id=? AND (?='%%' OR package_id LIKE ? OR display_name LIKE ?) AND (?='' OR EXISTS(SELECT 1 FROM canvas_plugin_versions v WHERE v.tenant_id=canvas_plugins.tenant_id AND v.plugin_id=canvas_plugins.id AND v.status=?)) ORDER BY updated_at DESC,id")
        .bind(actor.tenant_id).bind(&search).bind(&search).bind(&search).bind(&status).bind(&status)
        .fetch_all(&state.pool).await?;
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(plugin_from_row(&state, actor.tenant_id, row).await?);
    }
    values.extend(builtin_plugin_summaries()?);
    values.retain(|plugin| {
        let query = search.trim_matches('%').to_ascii_lowercase();
        (query.is_empty()
            || plugin.package_id.to_ascii_lowercase().contains(&query)
            || plugin.display_name.to_ascii_lowercase().contains(&query))
            && (status.is_empty()
                || plugin
                    .versions
                    .iter()
                    .any(|version| version.status == status))
    });
    values.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then(left.package_id.cmp(&right.package_id))
    });
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 100);
    let total = values.len() as u64;
    let items = values
        .into_iter()
        .skip(((page - 1) * page_size) as usize)
        .take(page_size as usize)
        .collect();
    Ok(Json(PageResponse {
        items,
        page,
        page_size,
        total,
    }))
}

async fn get_plugin(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PluginSummary>> {
    actor.require("canvas_plugin:view")?;
    let Some(row) = sqlx::query("SELECT id,package_id,display_name,description,source_type,default_version_id,version,updated_at FROM canvas_plugins WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await? else {
        return builtin_plugin_summaries()?
            .into_iter()
            .find(|plugin| plugin.id == id)
            .map(Json)
            .ok_or_else(|| ApiError::not_found("Canvas plugin"));
    };
    Ok(Json(plugin_from_row(&state, actor.tenant_id, row).await?))
}

fn builtin_plugin_summaries() -> ApiResult<Vec<PluginSummary>> {
    let mut packages =
        std::collections::BTreeMap::<(String, String, String), Vec<NodeManifestVersion>>::new();
    for manifest in NodeRegistry::m5_defaults().studio_manifests() {
        if let Some(binding) = manifest
            .plugin
            .as_ref()
            .filter(|binding| binding.package_id.starts_with("agentx/"))
        {
            packages
                .entry((
                    binding.package_id.clone(),
                    binding.package_version.clone(),
                    binding.bundle_digest.clone(),
                ))
                .or_default()
                .push(manifest.clone());
        }
    }
    Ok(packages
        .into_iter()
        .map(|((package_id, package_version, bundle_digest), nodes)| {
            let id = deterministic_plugin_id(&package_id);
            let version_id = deterministic_plugin_id(&format!("{package_id}@{package_version}"));
            let display_name = match package_id.as_str() {
                "agentx/core" => "Agentx Core",
                "agentx/data" => "Agentx Data",
                "agentx/http" => "Agentx HTTP",
                _ => "Agentx Built-in",
            };
            PluginSummary {
                id,
                package_id,
                display_name: display_name.into(),
                description: "Built-in package using the public Canvas Plugin runtime contract."
                    .into(),
                source_type: "builtin".into(),
                default_version_id: Some(version_id),
                version: 1,
                node_count: nodes.len() as u64,
                versions: vec![PluginVersion {
                    id: version_id,
                    package_version,
                    bundle_digest,
                    status: "enabled".into(),
                    sdk_api_version: 1,
                    node_types: nodes.into_iter().map(|node| node.node_type).collect(),
                    created_at: OffsetDateTime::UNIX_EPOCH,
                }],
                updated_at: OffsetDateTime::UNIX_EPOCH,
            }
        })
        .collect())
}

fn deterministic_plugin_id(value: &str) -> Uuid {
    agentx_runtime_contracts::deterministic_uuid(
        Uuid::nil(),
        format!("agentx-canvas-plugin-v1:{value}").as_bytes(),
    )
}

async fn plugin_from_row(
    state: &ControlApiState,
    tenant_id: Uuid,
    row: sqlx::mysql::MySqlRow,
) -> ApiResult<PluginSummary> {
    let id: Uuid = row.try_get("id")?;
    let version_rows = sqlx::query("SELECT id,package_version,bundle_digest,status,sdk_api_version,manifest_json,created_at FROM canvas_plugin_versions WHERE tenant_id=? AND plugin_id=? ORDER BY created_at DESC,id")
        .bind(tenant_id).bind(id).fetch_all(&state.pool).await?;
    let mut node_count = 0_u64;
    let versions = version_rows
        .into_iter()
        .map(|version| {
            let manifest: ValidatedPackage =
                serde_json::from_value(version.try_get("manifest_json")?)
                    .map_err(ApiError::internal)?;
            node_count = node_count.max(manifest.nodes.len() as u64);
            Ok(PluginVersion {
                id: version.try_get("id")?,
                package_version: version.try_get("package_version")?,
                bundle_digest: version.try_get("bundle_digest")?,
                status: version.try_get("status")?,
                sdk_api_version: version.try_get("sdk_api_version")?,
                node_types: manifest
                    .nodes
                    .into_iter()
                    .map(|node| node.node_type)
                    .collect(),
                created_at: version.try_get("created_at")?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(PluginSummary {
        id,
        package_id: row.try_get("package_id")?,
        display_name: row.try_get("display_name")?,
        description: row.try_get("description")?,
        source_type: row.try_get("source_type")?,
        default_version_id: row.try_get("default_version_id")?,
        version: row.try_get("version")?,
        versions,
        node_count,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn create_import(
    State(state): State<ControlApiState>,
    actor: Actor,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<ImportResponse>)> {
    actor.require("canvas_plugin:manage")?;
    cleanup_expired_imports(&state, actor.tenant_id).await?;
    let mut bytes = None;
    while let Some(field) = multipart.next_field().await.map_err(ApiError::internal)? {
        if field.name() == Some("file") {
            let value = field.bytes().await.map_err(ApiError::internal)?;
            if value.len() > MAX_BUNDLE_BYTES {
                return Err(ApiError::bad_request(
                    "PLUGIN_PACKAGE_TOO_LARGE",
                    "Plugin package exceeds 20 MiB",
                ));
            }
            bytes = Some(value.to_vec());
        }
    }
    let bytes = bytes.ok_or_else(|| {
        ApiError::bad_request("PLUGIN_FILE_REQUIRED", "Plugin package file is required")
    })?;
    let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
    if let Some(row) = sqlx::query("SELECT id,status,bundle_digest,artifact_key,manifest_json,issues_json,installed_version_id,expires_at FROM canvas_plugin_imports WHERE tenant_id=? AND bundle_digest=?")
        .bind(actor.tenant_id).bind(&digest).fetch_optional(&state.pool).await? {
        let status: String = row.try_get("status")?;
        let installed_version_id: Option<Uuid> = row.try_get("installed_version_id")?;
        let installed_version_exists = if let Some(version_id) = installed_version_id {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM canvas_plugin_versions WHERE tenant_id=? AND id=?",
            )
            .bind(actor.tenant_id)
            .bind(version_id)
            .fetch_one(&state.pool)
            .await?
                == 1
        } else {
            false
        };
        if status == "installed" && installed_version_exists {
            return Ok((StatusCode::OK, Json(import_from_row(row)?)));
        }
        let previous_key: String = row.try_get("artifact_key")?;
        if matches!(status.as_str(), "ready" | "installed") {
            state
                .control_objects
                .put(
                    &ObjectPath::from(previous_key.clone()),
                    Bytes::from(bytes.clone()).into(),
                )
                .await
                .map_err(ApiError::internal)?;
            let expires_at = OffsetDateTime::now_utc() + Duration::hours(24);
            sqlx::query("UPDATE canvas_plugin_imports SET status='ready',installed_version_id=NULL,expires_at=? WHERE tenant_id=? AND id=?")
                .bind(expires_at)
                .bind(actor.tenant_id)
                .bind(row.try_get::<Uuid, _>("id")?)
                .execute(&state.pool)
                .await?;
            let repaired = sqlx::query("SELECT id,status,bundle_digest,artifact_key,manifest_json,issues_json,installed_version_id,expires_at FROM canvas_plugin_imports WHERE tenant_id=? AND bundle_digest=?")
                .bind(actor.tenant_id)
                .bind(&digest)
                .fetch_one(&state.pool)
                .await?;
            return Ok((StatusCode::OK, Json(import_from_row(repaired)?)));
        }
        delete_control_object(&state.control_objects, &previous_key).await.map_err(ApiError::internal)?;
        sqlx::query("DELETE FROM canvas_plugin_imports WHERE tenant_id=? AND bundle_digest=? AND status IN ('cancelled','failed')")
            .bind(actor.tenant_id)
            .bind(&digest)
            .execute(&state.pool)
            .await?;
    }
    let id = Uuid::now_v7();
    let artifact_key = format!(
        "canvas-plugins/imports/{}/{id}.agentx-plugin",
        actor.tenant_id
    );
    let expires_at = OffsetDateTime::now_utc() + Duration::hours(24);
    let package = match validate_bundle(&bytes, &digest) {
        Ok(package) => package,
        Err(error) => {
            state
                .control_objects
                .put(
                    &ObjectPath::from(artifact_key.clone()),
                    Bytes::from(bytes).into(),
                )
                .await
                .map_err(ApiError::internal)?;
            let message = format!("{error:?}");
            let failed = json!({"package":{"protocolVersion":1,"sdkApiVersion":2,"packageId":"invalid/package","packageVersion":"0.0.0","displayName":"Invalid plugin","description":"","nodes":[],"runtimeEntry":"","uiEntry":null,"uiStylesEntry":null,"uiAssets":{},"traceRenderers":[]},"nodes":[]});
            sqlx::query("INSERT INTO canvas_plugin_imports(id,tenant_id,bundle_digest,artifact_key,status,manifest_json,issues_json,created_by,expires_at) VALUES(?,?,?,?,'failed',?,?,?,?)")
                .bind(id).bind(actor.tenant_id).bind(&digest).bind(&artifact_key).bind(failed)
                .bind(json!([{"path":"","code":"PLUGIN_PACKAGE_INVALID","message":message}]))
                .bind(actor.user_id).bind(expires_at).execute(&state.pool).await?;
            return Ok((
                StatusCode::CREATED,
                Json(ImportResponse {
                    id,
                    status: "failed".into(),
                    bundle_digest: digest,
                    package_id: "invalid/package".into(),
                    package_version: "0.0.0".into(),
                    display_name: "Invalid plugin".into(),
                    description: String::new(),
                    node_types: vec![],
                    issues: json!([{"path":"","code":"PLUGIN_PACKAGE_INVALID","message":message}]),
                    installed_version_id: None,
                    expires_at,
                }),
            ));
        }
    };
    state
        .control_objects
        .put(
            &ObjectPath::from(artifact_key.clone()),
            Bytes::from(bytes).into(),
        )
        .await
        .map_err(ApiError::internal)?;
    let inserted = sqlx::query("INSERT INTO canvas_plugin_imports(id,tenant_id,bundle_digest,artifact_key,status,manifest_json,issues_json,created_by,expires_at) VALUES(?,?,?,?,'ready',?,JSON_ARRAY(),?,?)")
        .bind(id).bind(actor.tenant_id).bind(&digest).bind(&artifact_key)
        .bind(serde_json::to_value(&package).map_err(ApiError::internal)?)
        .bind(actor.user_id).bind(expires_at).execute(&state.pool).await;
    if let Err(error) = inserted {
        let _ = state
            .control_objects
            .delete(&ObjectPath::from(artifact_key))
            .await;
        return Err(error.into());
    }
    Ok((
        StatusCode::CREATED,
        Json(ImportResponse {
            id,
            status: "ready".into(),
            bundle_digest: digest,
            package_id: package.package.package_id,
            package_version: package.package.package_version,
            display_name: package.package.display_name,
            description: package.package.description,
            node_types: package
                .nodes
                .into_iter()
                .map(|node| node.node_type)
                .collect(),
            issues: json!([]),
            installed_version_id: None,
            expires_at,
        }),
    ))
}

async fn cleanup_expired_imports(state: &ControlApiState, tenant: Uuid) -> ApiResult<()> {
    let rows = sqlx::query("SELECT id,artifact_key FROM canvas_plugin_imports WHERE tenant_id=? AND status IN ('ready','cancelled','failed') AND expires_at<UTC_TIMESTAMP(6) LIMIT 100")
        .bind(tenant)
        .fetch_all(&state.pool)
        .await?;
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let key: String = row.try_get("artifact_key")?;
        let _ = state.control_objects.delete(&ObjectPath::from(key)).await;
        sqlx::query("DELETE FROM canvas_plugin_imports WHERE tenant_id=? AND id=? AND status IN ('ready','cancelled','failed') AND expires_at<UTC_TIMESTAMP(6)")
            .bind(tenant)
            .bind(id)
            .execute(&state.pool)
            .await?;
    }
    Ok(())
}

async fn enqueue_object_gc(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    object_key: &str,
    reason: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO canvas_plugin_object_gc(id,tenant_id,object_key,reason) VALUES(?,?,?,?) ON DUPLICATE KEY UPDATE reason=VALUES(reason),available_at=LEAST(available_at,UTC_TIMESTAMP(6))")
        .bind(Uuid::now_v7()).bind(tenant).bind(object_key).bind(reason).execute(&mut **tx).await?;
    Ok(())
}

async fn clear_object_gc(pool: &sqlx::MySqlPool, tenant: Uuid, object_key: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM canvas_plugin_object_gc WHERE tenant_id=? AND object_key=?")
        .bind(tenant)
        .bind(object_key)
        .execute(pool)
        .await?;
    Ok(())
}

async fn delete_control_object(
    objects: &std::sync::Arc<dyn ObjectStore>,
    object_key: &str,
) -> Result<(), object_store::Error> {
    match objects.delete(&ObjectPath::from(object_key)).await {
        Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) async fn cleanup_background_objects(
    pool: &sqlx::MySqlPool,
    objects: &std::sync::Arc<dyn ObjectStore>,
) -> anyhow::Result<u64> {
    let mut cleaned = 0_u64;
    let imports = sqlx::query("SELECT id,tenant_id,artifact_key FROM canvas_plugin_imports WHERE status IN ('ready','cancelled','failed') AND expires_at<UTC_TIMESTAMP(6) ORDER BY expires_at,id LIMIT 100")
        .fetch_all(pool).await?;
    for row in imports {
        let id: Uuid = row.try_get("id")?;
        let tenant: Uuid = row.try_get("tenant_id")?;
        let key: String = row.try_get("artifact_key")?;
        match delete_control_object(objects, &key).await {
            Ok(()) => {
                cleaned += sqlx::query("DELETE FROM canvas_plugin_imports WHERE tenant_id=? AND id=? AND status IN ('ready','cancelled','failed') AND expires_at<UTC_TIMESTAMP(6)")
                    .bind(tenant).bind(id).execute(pool).await?.rows_affected();
            }
            Err(error) => {
                tracing::warn!(%error, %tenant, import_id=%id, "Expired plugin import cleanup will retry")
            }
        }
    }
    let pending = sqlx::query("SELECT id,tenant_id,object_key,attempt_count FROM canvas_plugin_object_gc WHERE available_at<=UTC_TIMESTAMP(6) ORDER BY available_at,id LIMIT 100")
        .fetch_all(pool).await?;
    for row in pending {
        let id: Uuid = row.try_get("id")?;
        let tenant: Uuid = row.try_get("tenant_id")?;
        let key: String = row.try_get("object_key")?;
        match delete_control_object(objects, &key).await {
            Ok(()) => {
                cleaned +=
                    sqlx::query("DELETE FROM canvas_plugin_object_gc WHERE tenant_id=? AND id=?")
                        .bind(tenant)
                        .bind(id)
                        .execute(pool)
                        .await?
                        .rows_affected();
            }
            Err(error) => {
                let message = error.to_string().chars().take(1000).collect::<String>();
                sqlx::query("UPDATE canvas_plugin_object_gc SET attempt_count=attempt_count+1,last_error=?,available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND) WHERE tenant_id=? AND id=?")
                    .bind(message).bind(tenant).bind(id).execute(pool).await?;
            }
        }
    }
    Ok(cleaned)
}

fn validate_bundle(bytes: &[u8], digest: &str) -> ApiResult<ValidatedPackage> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| ApiError::bad_request("PLUGIN_PACKAGE_INVALID", error.to_string()))?;
    if archive.is_empty() || archive.len() > MAX_FILES {
        return Err(ApiError::bad_request(
            "PLUGIN_PACKAGE_INVALID",
            "Plugin package file count is invalid",
        ));
    }
    let mut entries = std::collections::BTreeMap::<String, Vec<u8>>::new();
    let mut uncompressed_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| ApiError::bad_request("PLUGIN_PACKAGE_INVALID", error.to_string()))?;
        if file.is_dir() {
            continue;
        }
        let path = file
            .enclosed_name()
            .ok_or_else(|| {
                ApiError::bad_request(
                    "PLUGIN_PACKAGE_INVALID",
                    "Plugin package contains an unsafe path",
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        uncompressed_bytes = uncompressed_bytes.saturating_add(file.size());
        let symlink = file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000);
        let lower_path = path.to_ascii_lowercase();
        let native_dependency = [".node", ".so", ".dll", ".dylib", ".exe"]
            .iter()
            .any(|suffix| lower_path.ends_with(suffix));
        if file.size() > MAX_ENTRY_BYTES
            || uncompressed_bytes > 50 * 1024 * 1024
            || symlink
            || path.starts_with("node_modules/")
            || matches!(
                path.as_str(),
                "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock"
            )
            || native_dependency
            || entries.contains_key(&path)
        {
            return Err(ApiError::bad_request(
                "PLUGIN_PACKAGE_INVALID",
                "Plugin package contains an invalid entry",
            ));
        }
        let mut content = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut content).map_err(ApiError::internal)?;
        entries.insert(path, content);
    }
    let package: PackageManifest =
        serde_json::from_slice(entries.get("manifest.json").ok_or_else(|| {
            ApiError::bad_request("PLUGIN_PACKAGE_INVALID", "manifest.json is required")
        })?)
        .map_err(|error| ApiError::bad_request("PLUGIN_PACKAGE_INVALID", error.to_string()))?;
    if package.protocol_version != 1 || package.sdk_api_version != 2 {
        return Err(ApiError::unprocessable(
            "PLUGIN_SDK_UNSUPPORTED",
            "Only plugin package protocol 1 and SDK API 2 are supported",
        ));
    }
    if !PACKAGE_ID.is_match(&package.package_id)
        || package.package_id.starts_with("agentx/")
        || !SEMVER.is_match(&package.package_version)
    {
        return Err(ApiError::unprocessable(
            "PLUGIN_PACKAGE_INVALID",
            "Package ID or version is invalid",
        ));
    }
    if package.nodes.is_empty()
        || package.display_name.trim().is_empty()
        || package.display_name.len() > 160
    {
        return Err(ApiError::unprocessable(
            "PLUGIN_PACKAGE_INVALID",
            "Plugin identity or node list is invalid",
        ));
    }
    let runtime_source = String::from_utf8(
        entries
            .get(&package.runtime_entry)
            .ok_or_else(|| {
                ApiError::bad_request("PLUGIN_PACKAGE_INVALID", "Runtime entry is missing")
            })?
            .clone(),
    )
    .map_err(|_| ApiError::bad_request("PLUGIN_PACKAGE_INVALID", "Runtime entry must be UTF-8"))?;
    reject_unbundled_module(&runtime_source, "Runtime")?;
    let runtime_content_hash = format!("sha256:{:x}", Sha256::digest(runtime_source.as_bytes()));
    let runtime_artifact = PluginRuntimeArtifact {
        object_id: plugin_runtime_object_id(digest),
        content_hash: runtime_content_hash,
        size_bytes: runtime_source.len() as u64,
        media_type: "text/javascript".into(),
    };
    let ui_source = package
        .ui_entry
        .as_ref()
        .map(|entry| {
            entries.get(entry).ok_or_else(|| {
                ApiError::bad_request("PLUGIN_PACKAGE_INVALID", "UI entry is missing")
            })
        })
        .transpose()?
        .map(|bytes| {
            String::from_utf8(bytes.clone()).map_err(|_| {
                ApiError::bad_request("PLUGIN_PACKAGE_INVALID", "UI entry must be UTF-8")
            })
        })
        .transpose()?;
    if let Some(source) = &ui_source {
        reject_unbundled_module(source, "UI")?;
    }
    let ui_styles = package
        .ui_styles_entry
        .as_ref()
        .map(|entry| {
            entries.get(entry).ok_or_else(|| {
                ApiError::bad_request("PLUGIN_PACKAGE_INVALID", "UI styles entry is missing")
            })
        })
        .transpose()?
        .map(|bytes| {
            String::from_utf8(bytes.clone()).map_err(|_| {
                ApiError::bad_request("PLUGIN_PACKAGE_INVALID", "UI styles entry must be UTF-8")
            })
        })
        .transpose()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut renderer_keys = std::collections::BTreeSet::new();
    for renderer in &package.trace_renderers {
        if !renderer_keys.insert((renderer.content_type.clone(), renderer.content_version))
            || jsonschema::validator_for(&renderer.schema).is_err()
        {
            return Err(ApiError::unprocessable(
                "PLUGIN_PACKAGE_INVALID",
                "Trace renderer declarations must be unique and use valid JSON Schema",
            ));
        }
    }
    let mut ui_assets = BTreeMap::new();
    for (name, path) in &package.ui_assets {
        let bytes = entries.get(path).ok_or_else(|| {
            ApiError::bad_request(
                "PLUGIN_PACKAGE_INVALID",
                format!("UI asset is missing: {path}"),
            )
        })?;
        let media_type = match path
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "svg" => "image/svg+xml",
            "woff2" => "font/woff2",
            _ => "application/octet-stream",
        };
        ui_assets.insert(
            name.clone(),
            format!("data:{media_type};base64,{}", BASE64_STANDARD.encode(bytes)),
        );
    }
    let mut nodes = Vec::with_capacity(package.nodes.len());
    for path in &package.nodes {
        let mut node: NodeManifestVersion =
            serde_json::from_slice(entries.get(path).ok_or_else(|| {
                ApiError::bad_request("PLUGIN_PACKAGE_INVALID", "Node manifest is missing")
            })?)
            .map_err(|error| {
                ApiError::unprocessable("PLUGIN_PACKAGE_INVALID", error.to_string())
            })?;
        if node.capability != NodeCapability::PluginNodejs
            || !seen.insert(node.node_type.clone())
            || node.node_type.starts_with("workflow.")
        {
            return Err(ApiError::unprocessable(
                "PLUGIN_PACKAGE_INVALID",
                "Plugin nodes require unique plugin_nodejs identities",
            ));
        }
        node.plugin = Some(PluginNodeBinding {
            package_id: package.package_id.clone(),
            package_version: package.package_version.clone(),
            bundle_digest: digest.into(),
            runtime_entry: package.runtime_entry.clone(),
            runtime_source: runtime_source.clone(),
            runtime_artifact: Some(runtime_artifact.clone()),
            ui_entry: package.ui_entry.clone(),
            ui_source: ui_source.clone(),
            ui_styles: ui_styles.clone(),
            ui_assets: ui_assets.clone(),
            trace_renderers: package.trace_renderers.clone(),
        });
        node.validate_plugin()
            .map_err(|error| ApiError::unprocessable("PLUGIN_PACKAGE_INVALID", error))?;
        let mut registry = NodeRegistry::default();
        registry.register(node.clone()).map_err(|error| {
            ApiError::unprocessable("PLUGIN_PACKAGE_INVALID", error.to_string())
        })?;
        nodes.push(node);
    }
    Ok(ValidatedPackage { package, nodes })
}

fn reject_unbundled_module(source: &str, label: &str) -> ApiResult<()> {
    if source.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("import ") || line.starts_with("export * from ")
    }) {
        return Err(ApiError::unprocessable(
            "PLUGIN_PACKAGE_NOT_BUNDLED",
            format!("{label} entry contains an unresolved module import"),
        ));
    }
    Ok(())
}

async fn get_import(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ImportResponse>> {
    actor.require("canvas_plugin:view")?;
    let row = sqlx::query("SELECT id,status,bundle_digest,manifest_json,issues_json,installed_version_id,expires_at FROM canvas_plugin_imports WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::not_found("Canvas plugin import"))?;
    Ok(Json(import_from_row(row)?))
}

fn import_from_row(row: sqlx::mysql::MySqlRow) -> ApiResult<ImportResponse> {
    let manifest: ValidatedPackage =
        serde_json::from_value(row.try_get("manifest_json")?).map_err(ApiError::internal)?;
    Ok(ImportResponse {
        id: row.try_get("id")?,
        status: row.try_get("status")?,
        bundle_digest: row.try_get("bundle_digest")?,
        package_id: manifest.package.package_id,
        package_version: manifest.package.package_version,
        display_name: manifest.package.display_name,
        description: manifest.package.description,
        node_types: manifest
            .nodes
            .into_iter()
            .map(|node| node.node_type)
            .collect(),
        issues: row.try_get("issues_json")?,
        installed_version_id: row.try_get("installed_version_id")?,
        expires_at: row.try_get("expires_at")?,
    })
}

async fn cancel_import(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("canvas_plugin:manage")?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT artifact_key,status FROM canvas_plugin_imports WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(|| ApiError::not_found("Canvas plugin import"))?;
    if row.try_get::<String, _>("status")? == "installed" {
        return Err(ApiError::conflict(
            "PLUGIN_IMPORT_CONSUMED",
            "Installed imports cannot be cancelled",
        ));
    }
    sqlx::query("UPDATE canvas_plugin_imports SET status='cancelled' WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let artifact_key: String = row.try_get("artifact_key")?;
    enqueue_object_gc(&mut tx, actor.tenant_id, &artifact_key, "import_cancelled").await?;
    tx.commit().await?;
    if delete_control_object(&state.control_objects, &artifact_key)
        .await
        .is_ok()
    {
        clear_object_gc(&state.pool, actor.tenant_id, &artifact_key).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn install_import(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<InstallRequest>,
) -> ApiResult<(StatusCode, Json<PluginSummary>)> {
    actor.require("canvas_plugin:manage")?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT status,bundle_digest,manifest_json,artifact_key,installed_version_id,expires_at FROM canvas_plugin_imports WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(|| ApiError::not_found("Canvas plugin import"))?;
    let digest: String = row.try_get("bundle_digest")?;
    if digest != input.bundle_digest {
        return Err(ApiError::conflict(
            "PLUGIN_IMPORT_CHANGED",
            "Import digest does not match",
        ));
    }
    if row.try_get::<OffsetDateTime, _>("expires_at")? < OffsetDateTime::now_utc() {
        return Err(ApiError::conflict(
            "PLUGIN_IMPORT_EXPIRED",
            "Plugin import expired",
        ));
    }
    if let Some(version_id) = row.try_get::<Option<Uuid>, _>("installed_version_id")? {
        let plugin_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT plugin_id FROM canvas_plugin_versions WHERE tenant_id=? AND id=?",
        )
        .bind(actor.tenant_id)
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(plugin_id) = plugin_id else {
            return Err(ApiError::conflict(
                "PLUGIN_IMPORT_STALE",
                "The installed plugin version was deleted; upload the package again",
            ));
        };
        tx.commit().await?;
        return Ok((
            StatusCode::OK,
            get_plugin(State(state), actor, Path(plugin_id)).await?,
        ));
    }
    if row.try_get::<String, _>("status")? != "ready" {
        return Err(ApiError::conflict(
            "PLUGIN_IMPORT_NOT_READY",
            "Plugin import is not ready",
        ));
    }
    let package: ValidatedPackage =
        serde_json::from_value(row.try_get("manifest_json")?).map_err(ApiError::internal)?;
    for node in &package.nodes {
        if let Some(existing) = sqlx::query("SELECT d.source_type,p.package_id FROM node_definitions d LEFT JOIN node_definition_versions nv ON nv.node_definition_id=d.id LEFT JOIN canvas_plugin_versions pv ON pv.id=nv.plugin_version_id LEFT JOIN canvas_plugins p ON p.id=pv.plugin_id AND p.tenant_id=pv.tenant_id WHERE (d.tenant_id IS NULL OR d.tenant_id=?) AND d.node_type=? LIMIT 1")
            .bind(actor.tenant_id).bind(&node.node_type).fetch_optional(&mut *tx).await? {
            let source: String = existing.try_get("source_type")?;
            let owner: Option<String> = existing.try_get("package_id")?;
            if source != "plugin" || owner.as_deref().is_some_and(|owner| owner != package.package.package_id) {
                return Err(ApiError::conflict("PLUGIN_NODE_ID_CONFLICT", format!("Node type {} is already reserved", node.node_type)));
            }
        }
    }
    let plugin_id = if let Some(existing) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM canvas_plugins WHERE tenant_id=? AND package_id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(&package.package.package_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        existing
    } else {
        let value = Uuid::now_v7();
        sqlx::query("INSERT INTO canvas_plugins(id,tenant_id,package_id,display_name,description,created_by) VALUES(?,?,?,?,?,?)")
            .bind(value).bind(actor.tenant_id).bind(&package.package.package_id).bind(&package.package.display_name).bind(&package.package.description).bind(actor.user_id).execute(&mut *tx).await?;
        value
    };
    if let Some(existing) = sqlx::query("SELECT id,bundle_digest FROM canvas_plugin_versions WHERE tenant_id=? AND plugin_id=? AND package_version=?")
        .bind(actor.tenant_id).bind(plugin_id).bind(&package.package.package_version).fetch_optional(&mut *tx).await? {
        if existing.try_get::<String, _>("bundle_digest")? != digest { return Err(ApiError::conflict("PLUGIN_VERSION_CONFLICT", "The package version already has different content")); }
        let version_id: Uuid = existing.try_get("id")?;
        sqlx::query("UPDATE canvas_plugin_imports SET status='installed',installed_version_id=? WHERE tenant_id=? AND id=?").bind(version_id).bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok((StatusCode::OK, get_plugin(State(state), actor, Path(plugin_id)).await?));
    }
    let version_id = Uuid::now_v7();
    sqlx::query("INSERT INTO canvas_plugin_versions(id,tenant_id,plugin_id,package_version,bundle_digest,status,sdk_api_version,manifest_json,artifact_key,created_by) VALUES(?,?,?,?,?,?,?,?,?,?)")
        .bind(version_id).bind(actor.tenant_id).bind(plugin_id).bind(&package.package.package_version).bind(&digest)
        .bind(if input.enable { "enabled" } else { "disabled" }).bind(package.package.sdk_api_version)
        .bind(serde_json::to_value(&package).map_err(ApiError::internal)?).bind(row.try_get::<String, _>("artifact_key")?).bind(actor.user_id).execute(&mut *tx).await?;
    for node in &package.nodes {
        let definition_id = if let Some(value) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM node_definitions WHERE tenant_id=? AND node_type=? FOR UPDATE",
        )
        .bind(actor.tenant_id)
        .bind(&node.node_type)
        .fetch_optional(&mut *tx)
        .await?
        {
            value
        } else {
            let value = Uuid::now_v7();
            sqlx::query("INSERT INTO node_definitions(id,tenant_id,node_type,display_name,source_type,status) VALUES(?,?,?,?,'plugin','active')")
                .bind(value).bind(actor.tenant_id).bind(&node.node_type).bind(&node.display_name).execute(&mut *tx).await?;
            value
        };
        let manifest_value = serde_json::to_value(node).map_err(ApiError::internal)?;
        let hash =
            agentx_domain::canonical_content_hash(&manifest_value).map_err(ApiError::internal)?;
        sqlx::query("INSERT INTO node_definition_versions(id,node_definition_id,version_number,protocol_version,manifest_json,manifest_hash,capability,execution_style,side_effect_level,plugin_version_id) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(Uuid::now_v7()).bind(definition_id).bind(node.version).bind(&node.protocol_version).bind(manifest_value).bind(hash)
            .bind(node.capability.as_str()).bind(execution_style(node)).bind(side_effect(node)).bind(version_id).execute(&mut *tx).await
            .map_err(|error| if error.as_database_error().is_some_and(|db| db.is_unique_violation()) { ApiError::conflict("PLUGIN_NODE_VERSION_CONFLICT", format!("Node {}@{} already exists", node.node_type, node.version)) } else { ApiError::from(error) })?;
    }
    if input.set_default && input.enable {
        sqlx::query("UPDATE canvas_plugins SET default_version_id=?,version=version+1 WHERE tenant_id=? AND id=?").bind(version_id).bind(actor.tenant_id).bind(plugin_id).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE canvas_plugin_imports SET status='installed',installed_version_id=? WHERE tenant_id=? AND id=?").bind(version_id).bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
    audit(
        &mut tx,
        &actor,
        "canvas_plugin.installed",
        plugin_id,
        json!({"versionId":version_id,"bundleDigest":digest}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        get_plugin(State(state), actor, Path(plugin_id)).await?,
    ))
}

fn execution_style(node: &NodeManifestVersion) -> String {
    serde_json::to_value(&node.execution_style)
        .unwrap()
        .as_str()
        .unwrap()
        .into()
}
fn side_effect(node: &NodeManifestVersion) -> String {
    serde_json::to_value(&node.side_effect_level)
        .unwrap()
        .as_str()
        .unwrap()
        .into()
}

async fn update_plugin(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdatePluginRequest>,
) -> ApiResult<Json<PluginSummary>> {
    actor.require("canvas_plugin:manage")?;
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("UPDATE canvas_plugins p SET default_version_id=?,version=version+1 WHERE tenant_id=? AND id=? AND version=? AND EXISTS(SELECT 1 FROM canvas_plugin_versions v WHERE v.tenant_id=p.tenant_id AND v.plugin_id=p.id AND v.id=? AND v.status='enabled')")
        .bind(input.default_version_id).bind(actor.tenant_id).bind(id).bind(input.expected_revision).bind(input.default_version_id).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "PLUGIN_REVISION_CONFLICT",
            "Plugin changed or the target version is disabled",
        ));
    }
    audit(
        &mut tx,
        &actor,
        "canvas_plugin.default_version_changed",
        id,
        json!({"versionId":input.default_version_id}),
    )
    .await?;
    tx.commit().await?;
    get_plugin(State(state), actor, Path(id)).await
}

async fn update_version(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, version_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<UpdateVersionRequest>,
) -> ApiResult<Json<PluginSummary>> {
    actor.require("canvas_plugin:manage")?;
    let mut tx = state.pool.begin().await?;
    let current: u64 = sqlx::query_scalar(
        "SELECT version FROM canvas_plugins WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Canvas plugin"))?;
    if current != input.expected_revision {
        return Err(ApiError::conflict(
            "PLUGIN_REVISION_CONFLICT",
            "Plugin changed",
        ));
    }
    let status = if input.enabled { "enabled" } else { "disabled" };
    let changed = sqlx::query(
        "UPDATE canvas_plugin_versions SET status=? WHERE tenant_id=? AND plugin_id=? AND id=?",
    )
    .bind(status)
    .bind(actor.tenant_id)
    .bind(id)
    .bind(version_id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::not_found("Canvas plugin version"));
    }
    sqlx::query("UPDATE canvas_plugins SET default_version_id=IF(default_version_id=? AND ?='disabled',NULL,default_version_id),version=version+1 WHERE tenant_id=? AND id=?").bind(version_id).bind(status).bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
    audit(
        &mut tx,
        &actor,
        "canvas_plugin.version_status_changed",
        id,
        json!({"versionId":version_id,"status":status}),
    )
    .await?;
    tx.commit().await?;
    get_plugin(State(state), actor, Path(id)).await
}

async fn plugin_references(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    actor.require("canvas_plugin:view")?;
    let nodes = plugin_node_identities(&state, actor.tenant_id, id, None).await?;
    let (drafts, versions, deployments, executions) =
        reference_counts(&state, actor.tenant_id, &nodes).await?;
    Ok(Json(
        json!({"drafts":drafts,"workflowVersions":versions,"deployments":deployments,"executions":executions,"total":drafts+versions+deployments+executions}),
    ))
}

async fn plugin_audit(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<PluginAuditItem>>> {
    actor.require("canvas_plugin:manage")?;
    let rows = sqlx::query("SELECT id,actor_user_id,action,detail_json,created_at FROM audit_events WHERE tenant_id=? AND target_type='canvas_plugin' AND target_id=? ORDER BY created_at DESC,id DESC LIMIT 200")
        .bind(actor.tenant_id).bind(id.to_string()).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                Ok(PluginAuditItem {
                    id: row.try_get("id")?,
                    actor_user_id: row.try_get("actor_user_id")?,
                    action: row.try_get("action")?,
                    detail: row.try_get("detail_json")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?,
    ))
}

async fn delete_version(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, version_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    actor.require("canvas_plugin:manage")?;
    let mut tx = state.pool.begin().await?;
    let artifact_key: String = sqlx::query_scalar(
        "SELECT artifact_key FROM canvas_plugin_versions WHERE tenant_id=? AND plugin_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .bind(version_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Canvas plugin version"))?;
    let nodes = plugin_node_identities(&state, actor.tenant_id, id, Some(version_id)).await?;
    let (drafts, versions, deployments, executions) =
        reference_counts(&state, actor.tenant_id, &nodes).await?;
    if drafts + versions + deployments + executions > 0 {
        return Err(ApiError::conflict(
            "PLUGIN_VERSION_IN_USE",
            "Plugin version is referenced by a Workflow",
        ));
    }
    sqlx::query("DELETE FROM node_definition_versions WHERE plugin_version_id=?")
        .bind(version_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE d FROM node_definitions d LEFT JOIN node_definition_versions v ON v.node_definition_id=d.id WHERE d.tenant_id=? AND d.source_type='plugin' AND v.id IS NULL").bind(actor.tenant_id).execute(&mut *tx).await?;
    let changed = sqlx::query(
        "DELETE FROM canvas_plugin_versions WHERE tenant_id=? AND plugin_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .bind(version_id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::not_found("Canvas plugin version"));
    }
    sqlx::query("UPDATE canvas_plugins SET default_version_id=IF(default_version_id=?,NULL,default_version_id),version=version+1 WHERE tenant_id=? AND id=?").bind(version_id).bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
    let retained_imports = sqlx::query("UPDATE canvas_plugin_imports SET status='ready',installed_version_id=NULL,expires_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 24 HOUR) WHERE tenant_id=? AND installed_version_id=?")
        .bind(actor.tenant_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    audit(
        &mut tx,
        &actor,
        "canvas_plugin.version_deleted",
        id,
        json!({"versionId":version_id}),
    )
    .await?;
    if retained_imports == 0 {
        enqueue_object_gc(&mut tx, actor.tenant_id, &artifact_key, "version_deleted").await?;
    }
    tx.commit().await?;
    if retained_imports == 0 {
        match delete_control_object(&state.control_objects, &artifact_key).await {
            Ok(()) => clear_object_gc(&state.pool, actor.tenant_id, &artifact_key).await?,
            Err(error) => {
                tracing::warn!(%error, %id, %version_id, "Deleted plugin version queued orphan cleanup")
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_plugin(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("canvas_plugin:manage")?;
    let mut tx = state.pool.begin().await?;
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM canvas_plugins WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if locked.is_none() {
        return Err(ApiError::not_found("Canvas plugin"));
    }
    let nodes = plugin_node_identities(&state, actor.tenant_id, id, None).await?;
    let (drafts, versions, deployments, executions) =
        reference_counts(&state, actor.tenant_id, &nodes).await?;
    if drafts + versions + deployments + executions > 0 {
        return Err(ApiError::conflict(
            "PLUGIN_VERSION_IN_USE",
            "Plugin is referenced by a Workflow",
        ));
    }
    let artifact_keys = sqlx::query(
        "SELECT artifact_key FROM canvas_plugin_versions WHERE tenant_id=? AND plugin_id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|row| row.try_get::<String, _>("artifact_key"))
    .collect::<Result<Vec<_>, _>>()?;
    sqlx::query("DELETE FROM node_definition_versions WHERE plugin_version_id IN (SELECT id FROM canvas_plugin_versions WHERE tenant_id=? AND plugin_id=?)").bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE d FROM node_definitions d LEFT JOIN node_definition_versions v ON v.node_definition_id=d.id WHERE d.tenant_id=? AND d.source_type='plugin' AND v.id IS NULL").bind(actor.tenant_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM canvas_plugin_versions WHERE tenant_id=? AND plugin_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let changed = sqlx::query("DELETE FROM canvas_plugins WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::not_found("Canvas plugin"));
    }
    audit(&mut tx, &actor, "canvas_plugin.deleted", id, json!({})).await?;
    for artifact_key in &artifact_keys {
        enqueue_object_gc(&mut tx, actor.tenant_id, artifact_key, "plugin_deleted").await?;
    }
    tx.commit().await?;
    for artifact_key in artifact_keys {
        match delete_control_object(&state.control_objects, &artifact_key).await {
            Ok(()) => clear_object_gc(&state.pool, actor.tenant_id, &artifact_key).await?,
            Err(error) => tracing::warn!(%error, %id, "Deleted plugin queued orphan cleanup"),
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn plugin_node_identities(
    state: &ControlApiState,
    tenant: Uuid,
    plugin: Uuid,
    version: Option<Uuid>,
) -> ApiResult<Vec<(String, u32, String)>> {
    if let Some(plugin) = builtin_plugin_summaries()?
        .into_iter()
        .find(|value| value.id == plugin)
    {
        let selected = version
            .map(|version| plugin.versions.iter().any(|value| value.id == version))
            .unwrap_or(true);
        if !selected {
            return Err(ApiError::not_found("Canvas plugin version"));
        }
        let registry = NodeRegistry::m5_defaults();
        return Ok(registry
            .studio_manifests()
            .filter_map(|manifest| {
                let binding = manifest.plugin.as_ref()?;
                (binding.package_id == plugin.package_id).then(|| {
                    (
                        manifest.node_type.clone(),
                        manifest.version,
                        binding.bundle_digest.clone(),
                    )
                })
            })
            .collect());
    }
    let rows=sqlx::query("SELECT manifest_json,bundle_digest FROM canvas_plugin_versions WHERE tenant_id=? AND plugin_id=? AND (? IS NULL OR id=?)").bind(tenant).bind(plugin).bind(version).bind(version).fetch_all(&state.pool).await?;
    if rows.is_empty() {
        return Err(ApiError::not_found("Canvas plugin"));
    }
    let mut result = Vec::new();
    for row in rows {
        let digest: String = row.try_get("bundle_digest")?;
        let package: ValidatedPackage =
            serde_json::from_value(row.try_get("manifest_json")?).map_err(ApiError::internal)?;
        result.extend(
            package
                .nodes
                .into_iter()
                .map(|node| (node.node_type, node.version, digest.clone())),
        );
    }
    result.sort();
    result.dedup();
    Ok(result)
}

async fn reference_counts(
    state: &ControlApiState,
    tenant: Uuid,
    nodes: &[(String, u32, String)],
) -> ApiResult<(u64, u64, u64, u64)> {
    let mut drafts = 0_i64;
    let mut versions = 0_i64;
    let mut deployments = 0_i64;
    let mut executions = 0_i64;
    for (node_type, node_version, digest) in nodes {
        // Revisions remain runnable after the node is removed from the Head.
        // Keep their package until the owning Draft is actually deleted.
        drafts+=sqlx::query_scalar::<_,i64>("SELECT COUNT(DISTINCT r.draft_id) FROM workflow_draft_revisions r JOIN JSON_TABLE(r.definition_json,'$.nodes[*]' COLUMNS(node_type VARCHAR(255) PATH '$.type',node_version INT PATH '$.typeVersion')) n ON TRUE WHERE r.tenant_id=? AND n.node_type=? AND n.node_version=?").bind(tenant).bind(node_type).bind(node_version).fetch_one(&state.pool).await?;
        versions+=sqlx::query_scalar::<_,i64>("SELECT COUNT(DISTINCT v.id) FROM workflow_versions v JOIN JSON_TABLE(v.definition_json,'$.nodes[*]' COLUMNS(node_type VARCHAR(255) PATH '$.type',node_version INT PATH '$.typeVersion')) n ON TRUE WHERE v.tenant_id=? AND n.node_type=? AND n.node_version=?").bind(tenant).bind(node_type).bind(node_version).fetch_one(&state.pool).await?;
        deployments+=sqlx::query_scalar::<_,i64>("SELECT COUNT(DISTINCT d.id) FROM application_deployments d JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.id=d.workflow_version_id JOIN JSON_TABLE(v.definition_json,'$.nodes[*]' COLUMNS(node_type VARCHAR(255) PATH '$.type',node_version INT PATH '$.typeVersion')) n ON TRUE WHERE d.tenant_id=? AND n.node_type=? AND n.node_version=?").bind(tenant).bind(node_type).bind(node_version).fetch_one(&state.pool).await?;
        executions+=sqlx::query_scalar::<_,i64>("SELECT (SELECT COUNT(DISTINCT id) FROM execution_spec_bundles WHERE tenant_id=? AND JSON_SEARCH(payload_json,'one',?) IS NOT NULL)+(SELECT COUNT(DISTINCT id) FROM runtime_work_package_publications WHERE tenant_id=? AND JSON_SEARCH(package_json,'one',?) IS NOT NULL)").bind(tenant).bind(digest).bind(tenant).bind(digest).fetch_one(&state.pool).await?;
    }
    Ok((
        drafts.max(0) as u64,
        versions.max(0) as u64,
        deployments.max(0) as u64,
        executions.max(0) as u64,
    ))
}

async fn download_version(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, version_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Response> {
    actor.require("canvas_plugin:manage")?;
    let key: String=sqlx::query_scalar("SELECT artifact_key FROM canvas_plugin_versions WHERE tenant_id=? AND plugin_id=? AND id=?").bind(actor.tenant_id).bind(id).bind(version_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Canvas plugin version"))?;
    let bytes = state
        .control_objects
        .get(&ObjectPath::from(key))
        .await
        .map_err(ApiError::internal)?
        .bytes()
        .await
        .map_err(ApiError::internal)?;
    Ok(download_response(bytes, "canvas-plugin.agentx-plugin"))
}

async fn download_template(actor: Actor) -> ApiResult<Response> {
    actor.require("canvas_plugin:view")?;
    Ok(download_response(
        Bytes::from(template_zip()?),
        "agentx-canvas-plugin-template.zip",
    ))
}

fn template_zip() -> ApiResult<Vec<u8>> {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in [
        (
            "AGENTS.md",
            include_str!("../../../plugins/templates/canvas-plugin/AGENTS.md"),
        ),
        (
            "README.md",
            include_str!("../../../plugins/templates/canvas-plugin/README.md"),
        ),
        (
            "package.json",
            include_str!("../../../plugins/templates/canvas-plugin/package.json"),
        ),
        (
            "pnpm-lock.yaml",
            include_str!("../../../plugins/templates/canvas-plugin/pnpm-lock.yaml"),
        ),
        (
            "tsconfig.json",
            include_str!("../../../plugins/templates/canvas-plugin/tsconfig.json"),
        ),
        (
            "manifest.json",
            include_str!("../../../plugins/templates/canvas-plugin/manifest.json"),
        ),
        (
            "nodes/json_mapper.json",
            include_str!("../../../plugins/templates/canvas-plugin/nodes/json_mapper.json"),
        ),
        (
            "src/runtime/entry.ts",
            include_str!("../../../plugins/templates/canvas-plugin/src/runtime/entry.ts"),
        ),
        (
            "src/ui/entry.ts",
            include_str!("../../../plugins/templates/canvas-plugin/src/ui/entry.ts"),
        ),
        (
            "src/ui/styles.css",
            include_str!("../../../plugins/templates/canvas-plugin/src/ui/styles.css"),
        ),
        (
            "src/ui/logo.svg",
            include_str!("../../../plugins/templates/canvas-plugin/src/ui/logo.svg"),
        ),
        (
            "scripts/check.mjs",
            include_str!("../../../plugins/templates/canvas-plugin/scripts/check.mjs"),
        ),
        (
            "scripts/pack_plugin.py",
            include_str!("../../../plugins/templates/canvas-plugin/scripts/pack_plugin.py"),
        ),
        (
            "tests/runtime.test.mjs",
            include_str!("../../../plugins/templates/canvas-plugin/tests/runtime.test.mjs"),
        ),
        (
            "docs/node-contract.md",
            include_str!("../../../plugins/templates/canvas-plugin/docs/node-contract.md"),
        ),
        (
            "docs/ui-sdk.md",
            include_str!("../../../plugins/templates/canvas-plugin/docs/ui-sdk.md"),
        ),
        (
            "docs/runtime-sdk.md",
            include_str!("../../../plugins/templates/canvas-plugin/docs/runtime-sdk.md"),
        ),
        (
            "docs/trace.md",
            include_str!("../../../plugins/templates/canvas-plugin/docs/trace.md"),
        ),
        (
            "docs/testing.md",
            include_str!("../../../plugins/templates/canvas-plugin/docs/testing.md"),
        ),
        (
            "vendor/plugin-sdk/package.json",
            include_str!("../../../plugins/templates/canvas-plugin/vendor/plugin-sdk/package.json"),
        ),
        (
            "vendor/plugin-sdk/index.d.ts",
            include_str!("../../../plugins/templates/canvas-plugin/vendor/plugin-sdk/index.d.ts"),
        ),
        (
            "vendor/plugin-ui/package.json",
            include_str!("../../../plugins/templates/canvas-plugin/vendor/plugin-ui/package.json"),
        ),
        (
            "vendor/plugin-ui/index.d.ts",
            include_str!("../../../plugins/templates/canvas-plugin/vendor/plugin-ui/index.d.ts"),
        ),
        (
            "vendor/plugin-runner/runner.mjs",
            include_str!("../../../plugins/packages/plugin-runner/runner.mjs"),
        ),
        (
            "src/examples/http-trace.ts",
            include_str!("../../../plugins/templates/canvas-plugin/src/examples/http-trace.ts"),
        ),
        (
            "src/examples/files.ts",
            include_str!("../../../plugins/templates/canvas-plugin/src/examples/files.ts"),
        ),
    ] {
        archive
            .start_file(name, options)
            .map_err(ApiError::internal)?;
        archive
            .write_all(content.as_bytes())
            .map_err(ApiError::internal)?;
    }
    Ok(archive.finish().map_err(ApiError::internal)?.into_inner())
}

fn download_response(bytes: Bytes, name: &str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).unwrap(),
    );
    response
}

pub(crate) async fn plugin_manifests_for_tenant(
    state: &ControlApiState,
    tenant: Uuid,
) -> ApiResult<Vec<NodeManifestVersion>> {
    plugin_manifests_for_pool(&state.pool, tenant, false)
        .await
        .map_err(ApiError::internal)
}

pub(crate) async fn plugin_manifest_by_digest(
    state: &ControlApiState,
    tenant: Uuid,
    node_type: &str,
    version: u32,
    digest: &str,
) -> ApiResult<(NodeManifestVersion, String)> {
    let value: Value = sqlx::query_scalar(
        "SELECT manifest_json FROM canvas_plugin_versions WHERE tenant_id=? AND bundle_digest=?",
    )
    .bind(tenant)
    .bind(digest)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Canvas plugin artifact"))?;
    let package: ValidatedPackage = serde_json::from_value(value).map_err(ApiError::internal)?;
    let manifest = package
        .nodes
        .into_iter()
        .find(|manifest| manifest.node_type == node_type && manifest.version == version)
        .ok_or_else(|| ApiError::not_found("Node definition"))?;
    let manifest_value = serde_json::to_value(&manifest).map_err(ApiError::internal)?;
    let hash =
        agentx_domain::canonical_content_hash(&manifest_value).map_err(ApiError::internal)?;
    Ok((manifest, hash))
}

pub(crate) async fn plugin_manifest_by_identity(
    state: &ControlApiState,
    tenant: Uuid,
    node_type: &str,
    version: u32,
) -> ApiResult<Option<(NodeManifestVersion, String)>> {
    let rows = sqlx::query(
        "SELECT manifest_json FROM canvas_plugin_versions WHERE tenant_id=? ORDER BY created_at DESC,id DESC",
    )
    .bind(tenant)
    .fetch_all(&state.pool)
    .await?;
    for row in rows {
        let package: ValidatedPackage =
            serde_json::from_value(row.try_get("manifest_json")?).map_err(ApiError::internal)?;
        if let Some(manifest) = package
            .nodes
            .into_iter()
            .find(|manifest| manifest.node_type == node_type && manifest.version == version)
        {
            let value = serde_json::to_value(&manifest).map_err(ApiError::internal)?;
            let hash = agentx_domain::canonical_content_hash(&value).map_err(ApiError::internal)?;
            return Ok(Some((manifest, hash)));
        }
    }
    Ok(None)
}

pub(crate) async fn plugin_manifests_for_pool(
    pool: &sqlx::MySqlPool,
    tenant: Uuid,
    include_disabled: bool,
) -> anyhow::Result<Vec<NodeManifestVersion>> {
    let rows=sqlx::query("SELECT v.manifest_json FROM canvas_plugin_versions v JOIN canvas_plugins p ON p.tenant_id=v.tenant_id AND p.id=v.plugin_id WHERE v.tenant_id=? AND (? OR v.status='enabled')").bind(tenant).bind(include_disabled).fetch_all(pool).await?;
    let mut result = Vec::new();
    for row in rows {
        let package: ValidatedPackage = serde_json::from_value(row.try_get("manifest_json")?)?;
        result.extend(package.nodes);
    }
    Ok(result)
}

pub(crate) async fn registry_for_tenant(
    state: &ControlApiState,
    tenant: Uuid,
    dependencies: &std::collections::BTreeMap<Uuid, agentx_domain::WorkflowDefinition>,
    include_disabled: bool,
) -> ApiResult<NodeRegistry> {
    let plugins = plugin_manifests_for_pool(&state.pool, tenant, include_disabled)
        .await
        .map_err(ApiError::internal)?;
    agentx_bundle_builder::node_registry_with_plugins(dependencies, &plugins)
        .map_err(|error| ApiError::unprocessable("WORKFLOW_COMPILE_FAILED", error.to_string()))
}

pub(crate) async fn require_workflow_version_plugins_enabled(
    state: &ControlApiState,
    tenant: Uuid,
    workflow_version_id: Uuid,
) -> ApiResult<()> {
    let definition: Value = sqlx::query_scalar(
        "SELECT definition_json FROM workflow_versions WHERE tenant_id=? AND id=?",
    )
    .bind(tenant)
    .bind(workflow_version_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Workflow version"))?;
    let definition: agentx_domain::WorkflowDefinition =
        serde_json::from_value(definition).map_err(ApiError::internal)?;
    require_definition_plugins_enabled(state, tenant, &definition).await
}

pub(crate) async fn require_definition_plugins_enabled(
    state: &ControlApiState,
    tenant: Uuid,
    definition: &agentx_domain::WorkflowDefinition,
) -> ApiResult<()> {
    require_definition_plugin_closure_enabled(
        state,
        tenant,
        definition,
        &std::collections::BTreeMap::new(),
    )
    .await
}

pub(crate) async fn require_definition_plugin_closure_enabled(
    state: &ControlApiState,
    tenant: Uuid,
    definition: &agentx_domain::WorkflowDefinition,
    dependencies: &std::collections::BTreeMap<Uuid, agentx_domain::WorkflowDefinition>,
) -> ApiResult<()> {
    let all = plugin_manifests_for_pool(&state.pool, tenant, true)
        .await
        .map_err(ApiError::internal)?;
    let enabled = plugin_manifests_for_pool(&state.pool, tenant, false)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|manifest| manifest.key())
        .collect::<std::collections::BTreeSet<_>>();
    let plugin_keys = all
        .into_iter()
        .map(|manifest| manifest.key())
        .collect::<std::collections::BTreeSet<_>>();
    let disabled_reference = std::iter::once(definition)
        .chain(dependencies.values())
        .flat_map(|definition| definition.nodes.iter())
        .any(|node| {
            let key = (node.node_type.clone(), node.type_version);
            plugin_keys.contains(&key) && !enabled.contains(&key)
        });
    if disabled_reference {
        return Err(ApiError::conflict(
            "PLUGIN_VERSION_DISABLED",
            "Workflow version references a disabled Canvas Plugin version",
        ));
    }
    Ok(())
}

pub(crate) async fn require_bundle_plugins_enabled(
    state: &ControlApiState,
    tenant: Uuid,
    bundle_id: Uuid,
) -> ApiResult<()> {
    let payload = execution_plugin_payload(state, tenant, bundle_id).await?;
    let disabled = sqlx::query(
        "SELECT bundle_digest FROM canvas_plugin_versions WHERE tenant_id=? AND status='disabled'",
    )
    .bind(tenant)
    .fetch_all(&state.pool)
    .await?;
    for row in disabled {
        let digest: String = row.try_get("bundle_digest")?;
        if contains_json_string(&payload, &digest) {
            return Err(ApiError::conflict(
                "PLUGIN_VERSION_DISABLED",
                "Execution bundle references a disabled Canvas Plugin version",
            ));
        }
    }
    Ok(())
}

async fn execution_plugin_payload(
    state: &ControlApiState,
    tenant: Uuid,
    bundle_id: Uuid,
) -> ApiResult<Value> {
    let payload: Option<Value> = sqlx::query_scalar(
        "SELECT payload_json FROM execution_spec_bundles WHERE tenant_id=? AND id=?",
    )
    .bind(tenant)
    .bind(bundle_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(payload) = payload {
        Ok(payload)
    } else {
        Ok(sqlx::query_scalar(
            "SELECT package_json FROM runtime_work_package_publications WHERE tenant_id=? AND package_id=?",
        )
        .bind(tenant)
        .bind(bundle_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Execution bundle"))?)
    }
}

pub(crate) async fn plugin_manifest_for_execution_bundle(
    state: &ControlApiState,
    tenant: Uuid,
    bundle_id: Uuid,
    node_type: &str,
    version: u32,
    digest: &str,
) -> ApiResult<(NodeManifestVersion, String)> {
    let payload = execution_plugin_payload(state, tenant, bundle_id).await?;
    if !contains_json_string(&payload, digest) {
        return Err(ApiError::forbidden(
            "The requested plugin artifact is not part of this execution",
        ));
    }
    plugin_manifest_by_digest(state, tenant, node_type, version, digest).await
}

fn contains_json_string(value: &Value, target: &str) -> bool {
    match value {
        Value::String(value) => value == target,
        Value::Array(values) => values
            .iter()
            .any(|value| contains_json_string(value, target)),
        Value::Object(values) => values
            .values()
            .any(|value| contains_json_string(value, target)),
        _ => false,
    }
}

async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    actor: &Actor,
    action: &str,
    target: Uuid,
    detail: Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO audit_events(id,tenant_id,actor_user_id,action,target_type,target_id,request_id,detail_json) VALUES(?,?,?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(actor.user_id).bind(action).bind("canvas_plugin").bind(target.to_string()).bind(Uuid::now_v7()).bind(detail).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package_bytes() -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        let package = json!({
            "protocolVersion":1,"sdkApiVersion":2,"packageId":"acme/test","packageVersion":"1.0.0",
            "displayName":"Test","description":"Test plugin","nodes":["nodes/test.json"],
            "runtimeEntry":"runtime/entry.js","uiEntry":"ui/entry.js","uiStylesEntry":null,
            "traceRenderers":[]
        });
        let mut node: Value = serde_json::from_str(include_str!(
            "../../../plugins/templates/canvas-plugin/nodes/json_mapper.json"
        ))
        .unwrap();
        node["nodeType"] = Value::String("acme.test".into());
        for (name, bytes) in [
            ("manifest.json", serde_json::to_vec(&package).unwrap()),
            ("nodes/test.json", serde_json::to_vec(&node).unwrap()),
            (
                "runtime/entry.js",
                b"export async function execute(){return {status:'completed',outputs:{main:[]}}}"
                    .to_vec(),
            ),
            (
                "ui/entry.js",
                b"export function createUi(){return {Panel(){return null}}}".to_vec(),
            ),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(&bytes).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    fn rewrite_package(
        update_manifest: impl FnOnce(&mut Value),
        update_node: impl FnOnce(&mut Value),
    ) -> Vec<u8> {
        let bytes = package_bytes();
        let mut input = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut entries = BTreeMap::new();
        for index in 0..input.len() {
            let mut entry = input.by_index(index).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            entries.insert(entry.name().to_owned(), bytes);
        }
        let mut manifest: Value = serde_json::from_slice(&entries["manifest.json"]).unwrap();
        let mut node: Value = serde_json::from_slice(&entries["nodes/test.json"]).unwrap();
        update_manifest(&mut manifest);
        update_node(&mut node);
        entries.insert(
            "manifest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        );
        entries.insert("nodes/test.json".into(), serde_json::to_vec(&node).unwrap());
        let cursor = Cursor::new(Vec::new());
        let mut output = zip::ZipWriter::new(cursor);
        for (name, bytes) in entries {
            output
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            output.write_all(&bytes).unwrap();
        }
        output.finish().unwrap().into_inner()
    }

    #[test]
    fn validates_and_freezes_plugin_sources() {
        let bytes = package_bytes();
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        let package = validate_bundle(&bytes, &digest).unwrap();
        assert_eq!(package.package.package_id, "acme/test");
        assert_eq!(package.nodes[0].capability, NodeCapability::PluginNodejs);
        let binding = package.nodes[0].plugin.as_ref().unwrap();
        assert_eq!(binding.bundle_digest, digest);
        assert!(binding.runtime_source.contains("execute"));
    }

    #[test]
    fn rejects_reserved_package_ids() {
        let bytes = package_bytes();
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut manifest = String::new();
        archive
            .by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        assert!(manifest.contains("acme/test"));
        assert!(!PACKAGE_ID.is_match("Agentx/Core"));
        assert!(!SEMVER.is_match("latest"));
    }

    #[test]
    fn rejects_unbundled_runtime_and_ui_imports() {
        let error = reject_unbundled_module(
            "import React from 'react'; export const execute = () => null",
            "UI",
        )
        .unwrap_err();
        assert!(format!("{error:?}").contains("PLUGIN_PACKAGE_NOT_BUNDLED"));
    }

    #[test]
    fn rejects_wrong_sdk_invalid_manifest_and_missing_entry() {
        let wrong_sdk = rewrite_package(
            |manifest| manifest["sdkApiVersion"] = Value::from(1),
            |_| {},
        );
        let digest = format!("sha256:{:x}", Sha256::digest(&wrong_sdk));
        assert!(
            format!("{:?}", validate_bundle(&wrong_sdk, &digest).unwrap_err())
                .contains("PLUGIN_SDK_UNSUPPORTED")
        );

        let wrong_node = rewrite_package(
            |_| {},
            |node| node["protocolVersion"] = Value::String("2.0".into()),
        );
        let digest = format!("sha256:{:x}", Sha256::digest(&wrong_node));
        assert!(validate_bundle(&wrong_node, &digest).is_err());

        let missing = rewrite_package(
            |manifest| manifest["runtimeEntry"] = Value::String("runtime/missing.js".into()),
            |_| {},
        );
        let digest = format!("sha256:{:x}", Sha256::digest(&missing));
        assert!(
            format!("{:?}", validate_bundle(&missing, &digest).unwrap_err())
                .contains("Runtime entry is missing")
        );
    }

    #[test]
    fn rejects_unsafe_paths_and_duplicate_node_identities() {
        let cursor = Cursor::new(Vec::new());
        let mut output = zip::ZipWriter::new(cursor);
        output
            .start_file("../escape", zip::write::SimpleFileOptions::default())
            .unwrap();
        output.write_all(b"bad").unwrap();
        let bytes = output.finish().unwrap().into_inner();
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        assert!(validate_bundle(&bytes, &digest).is_err());

        let duplicate = rewrite_package(
            |manifest| manifest["nodes"] = json!(["nodes/test.json", "nodes/test.json"]),
            |_| {},
        );
        let digest = format!("sha256:{:x}", Sha256::digest(&duplicate));
        assert!(
            format!("{:?}", validate_bundle(&duplicate, &digest).unwrap_err())
                .contains("unique plugin_nodejs identities")
        );
    }

    #[test]
    fn developer_template_is_a_valid_zip() {
        let bytes = template_zip().unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(archive.by_name("AGENTS.md").is_ok());
        assert!(archive.by_name("pnpm-lock.yaml").is_ok());
        assert!(archive.by_name("vendor/plugin-sdk/index.d.ts").is_ok());
        let mut package = String::new();
        archive
            .by_name("package.json")
            .unwrap()
            .read_to_string(&mut package)
            .unwrap();
        let package: Value = serde_json::from_str(&package).unwrap();
        assert_eq!(package["packageManager"], "pnpm@11.9.0");
    }
}
