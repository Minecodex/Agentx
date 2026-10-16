use agentx_api_types::PageResponse;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Row, mysql::MySqlRow};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    control_helpers::required_name,
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/sandbox-profiles",
            get(list_profiles).post(create_profile),
        )
        .route(
            "/api/v1/sandbox-profiles/{id}",
            get(get_profile)
                .patch(update_profile)
                .delete(delete_profile),
        )
        .route(
            "/api/v1/sandbox-profiles/{id}/versions",
            axum::routing::post(create_version),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    search: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionResponse {
    id: Uuid,
    version_number: u64,
    runner: String,
    image_digest: String,
    cpu_millis: u32,
    memory_bytes: u64,
    pids_limit: u32,
    disk_bytes: u64,
    timeout_seconds: u32,
    output_limit_bytes: u64,
    network_policy: Value,
    configuration_hash: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileResponse {
    id: Uuid,
    name: String,
    description: Option<String>,
    status: String,
    current_version_number: u64,
    owner_department_id: Uuid,
    version: u64,
    current: VersionResponse,
    versions: Vec<VersionResponse>,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionInput {
    runner: String,
    image_digest: String,
    cpu_millis: u32,
    memory_bytes: u64,
    pids_limit: u32,
    disk_bytes: u64,
    timeout_seconds: u32,
    output_limit_bytes: u64,
    network_policy: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    name: String,
    description: Option<String>,
    owner_department_id: Uuid,
    #[serde(flatten)]
    configuration: VersionInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    name: String,
    description: Option<String>,
    status: String,
    version: u64,
}

async fn list_profiles(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<ProfileResponse>>> {
    actor.require("sandbox:view")?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let status = query.status.unwrap_or_default();
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let (rows, total) = if administrator {
        let rows=sqlx::query("SELECT id FROM sandbox_profiles WHERE tenant_id=? AND (?='' OR status=?) AND (?='%%' OR name LIKE ?) ORDER BY updated_at DESC,id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM sandbox_profiles WHERE tenant_id=? AND (?='' OR status=?) AND (?='%%' OR name LIKE ?)").bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    } else {
        let rows=sqlx::query("SELECT p.id FROM sandbox_profiles p JOIN department_closure dc ON dc.tenant_id=p.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=p.owner_department_id WHERE p.tenant_id=? AND (?='' OR p.status=?) AND (?='%%' OR p.name LIKE ?) ORDER BY p.updated_at DESC,p.id DESC LIMIT ? OFFSET ?").bind(actor.department_id).bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM sandbox_profiles p JOIN department_closure dc ON dc.tenant_id=p.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=p.owner_department_id WHERE p.tenant_id=? AND (?='' OR p.status=?) AND (?='%%' OR p.name LIKE ?)").bind(actor.department_id).bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    };
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(load(&state, actor.tenant_id, row.try_get("id")?).await?);
    }
    Ok(Json(PageResponse {
        items,
        page,
        page_size,
        total: total.try_into().unwrap_or_default(),
    }))
}

async fn create_profile(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(mut input): Json<CreateRequest>,
) -> ApiResult<(StatusCode, Json<ProfileResponse>)> {
    actor.require("sandbox:manage")?;
    require_department_scope(&state, &actor, input.owner_department_id).await?;
    normalize_configuration(&mut input.configuration)?;
    let name = required_name(&input.name)?;
    let description = validate_description(input.description)?;
    let id = Uuid::now_v7();
    let hash = configuration_hash(&input.configuration)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO sandbox_profiles(id,tenant_id,name,description,owner_department_id,created_by) VALUES(?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(name).bind(description).bind(input.owner_department_id).bind(actor.user_id).execute(&mut *tx).await.map_err(map_name_error)?;
    insert_version(&mut tx, &actor, id, 1, &input.configuration, &hash).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load(&state, actor.tenant_id, id).await?),
    ))
}

async fn get_profile(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ProfileResponse>> {
    actor.require("sandbox:view")?;
    require_visible(&state, &actor, id).await?;
    Ok(Json(load(&state, actor.tenant_id, id).await?))
}

async fn update_profile(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateRequest>,
) -> ApiResult<Json<ProfileResponse>> {
    actor.require("sandbox:manage")?;
    require_visible(&state, &actor, id).await?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_STATUS",
            "Sandbox Profile status is invalid",
        ));
    }
    let changed=sqlx::query("UPDATE sandbox_profiles SET name=?,description=?,status=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?").bind(required_name(&input.name)?).bind(validate_description(input.description)?).bind(input.status).bind(actor.tenant_id).bind(id).bind(input.version).execute(&state.pool).await.map_err(map_name_error)?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "SANDBOX_PROFILE_VERSION_CONFLICT",
            "Sandbox Profile changed",
        ));
    }
    Ok(Json(load(&state, actor.tenant_id, id).await?))
}

async fn create_version(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(mut input): Json<VersionInput>,
) -> ApiResult<(StatusCode, Json<VersionResponse>)> {
    actor.require("sandbox:manage")?;
    require_visible(&state, &actor, id).await?;
    normalize_configuration(&mut input)?;
    let hash = configuration_hash(&input)?;
    let mut tx = state.pool.begin().await?;
    let current:Option<u64>=sqlx::query_scalar("SELECT current_version_number FROM sandbox_profiles WHERE tenant_id=? AND id=? AND status='active' FOR UPDATE").bind(actor.tenant_id).bind(id).fetch_optional(&mut *tx).await?;
    let next = current
        .ok_or_else(|| ApiError::not_found("Sandbox Profile"))?
        .checked_add(1)
        .ok_or_else(|| ApiError::internal("sandbox version exhausted"))?;
    let duplicate: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sandbox_profile_versions WHERE tenant_id=? AND profile_id=? AND configuration_hash=?)")
        .bind(actor.tenant_id).bind(id).bind(&hash).fetch_one(&mut *tx).await?;
    if duplicate {
        return Err(ApiError::conflict(
            "SANDBOX_PROFILE_CONFIGURATION_EXISTS",
            "This Sandbox Profile configuration already exists",
        ));
    }
    let version_id = insert_version(&mut tx, &actor, id, next, &input, &hash).await?;
    sqlx::query("UPDATE sandbox_profiles SET current_version_number=?,version=version+1 WHERE tenant_id=? AND id=?").bind(next).bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_version(&state, actor.tenant_id, version_id).await?),
    ))
}

async fn delete_profile(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("sandbox:manage")?;
    let mut tx = crate::workflow_revision_resources::begin_delete(
        &state.pool,
        actor.tenant_id,
        "sandbox_profile",
        id,
    )
    .await?;
    require_visible(&state, &actor, id).await?;
    let references:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM workflow_draft_resources WHERE tenant_id=? AND resource_type='sandbox_profile' AND resource_id=?)+(SELECT COUNT(*) FROM workflow_version_resources WHERE tenant_id=? AND resource_type='sandbox_profile' AND resource_id=?)").bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if references != 0 {
        return Err(ApiError::conflict(
            "SANDBOX_PROFILE_REFERENCED",
            "Sandbox Profile is referenced",
        ));
    }

    sqlx::query("DELETE FROM sandbox_profile_versions WHERE tenant_id=? AND profile_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sandbox_profiles WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn insert_version(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    actor: &Actor,
    profile: Uuid,
    version: u64,
    input: &VersionInput,
    hash: &str,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO sandbox_profile_versions(id,tenant_id,profile_id,version_number,runner,image_digest,cpu_millis,memory_bytes,pids_limit,disk_bytes,timeout_seconds,output_limit_bytes,network_policy_json,configuration_hash,created_by) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(profile).bind(version).bind(&input.runner).bind(&input.image_digest).bind(input.cpu_millis).bind(input.memory_bytes).bind(input.pids_limit).bind(input.disk_bytes).bind(input.timeout_seconds).bind(input.output_limit_bytes).bind(&input.network_policy).bind(hash).bind(actor.user_id).execute(&mut **tx).await?;
    Ok(id)
}
async fn load(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<ProfileResponse> {
    let row=sqlx::query("SELECT id,name,description,status,current_version_number,owner_department_id,version,updated_at FROM sandbox_profiles WHERE tenant_id=? AND id=?").bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Sandbox Profile"))?;
    let version_rows=sqlx::query("SELECT id,version_number,runner,image_digest,cpu_millis,memory_bytes,pids_limit,disk_bytes,timeout_seconds,output_limit_bytes,network_policy_json,configuration_hash,created_at FROM sandbox_profile_versions WHERE tenant_id=? AND profile_id=? ORDER BY version_number DESC").bind(tenant).bind(id).fetch_all(&state.pool).await?;
    let versions = version_rows
        .into_iter()
        .map(version_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let current_number = row.try_get("current_version_number")?;
    let current = versions
        .iter()
        .find(|value| value.version_number == current_number)
        .cloned()
        .ok_or_else(|| ApiError::internal("Sandbox Profile current version is missing"))?;
    Ok(ProfileResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
        current_version_number: current_number,
        owner_department_id: row.try_get("owner_department_id")?,
        version: row.try_get("version")?,
        current,
        versions,
        updated_at: row.try_get("updated_at")?,
    })
}
async fn load_version(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<VersionResponse> {
    let row=sqlx::query("SELECT id,version_number,runner,image_digest,cpu_millis,memory_bytes,pids_limit,disk_bytes,timeout_seconds,output_limit_bytes,network_policy_json,configuration_hash,created_at FROM sandbox_profile_versions WHERE tenant_id=? AND id=?").bind(tenant).bind(id).fetch_one(&state.pool).await?;
    Ok(version_from_row(row)?)
}
fn version_from_row(row: MySqlRow) -> Result<VersionResponse, sqlx::Error> {
    Ok(VersionResponse {
        id: row.try_get("id")?,
        version_number: row.try_get("version_number")?,
        runner: row.try_get("runner")?,
        image_digest: row.try_get("image_digest")?,
        cpu_millis: row.try_get("cpu_millis")?,
        memory_bytes: row.try_get("memory_bytes")?,
        pids_limit: row.try_get("pids_limit")?,
        disk_bytes: row.try_get("disk_bytes")?,
        timeout_seconds: row.try_get("timeout_seconds")?,
        output_limit_bytes: row.try_get("output_limit_bytes")?,
        network_policy: normalize_network_policy_value(row.try_get("network_policy_json")?),
        configuration_hash: row.try_get("configuration_hash")?,
        created_at: row.try_get("created_at")?,
    })
}
async fn require_visible(state: &ControlApiState, actor: &Actor, id: Uuid) -> ApiResult<()> {
    let visible: bool = if actor.roles.iter().any(|role| role == "company_admin") {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sandbox_profiles WHERE tenant_id=? AND id=?)",
        )
        .bind(actor.tenant_id)
        .bind(id)
        .fetch_one(&state.pool)
        .await?
    } else {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sandbox_profiles p JOIN department_closure dc ON dc.tenant_id=p.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=p.owner_department_id WHERE p.tenant_id=? AND p.id=?)").bind(actor.department_id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?
    };
    if visible {
        Ok(())
    } else {
        Err(ApiError::not_found("Sandbox Profile"))
    }
}
async fn require_department_scope(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<()> {
    if actor.roles.iter().any(|role| role == "company_admin") {
        return Ok(());
    }
    let visible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM department_closure WHERE tenant_id=? AND ancestor_id=? AND descendant_id=?)").bind(actor.tenant_id).bind(actor.department_id).bind(id).fetch_one(&state.pool).await?;
    if visible {
        Ok(())
    } else {
        Err(ApiError::forbidden("Department is outside the actor scope"))
    }
}
fn validate_configuration(input: &VersionInput) -> ApiResult<()> {
    if !matches!(
        input.runner.as_str(),
        "python" | "javascript" | "shell" | "browser"
    ) {
        return Err(ApiError::bad_request(
            "SANDBOX_RUNNER_INVALID",
            "Sandbox runner is invalid",
        ));
    }
    if !valid_digest(&input.image_digest) {
        return Err(ApiError::bad_request(
            "SANDBOX_IMAGE_DIGEST_INVALID",
            "Sandbox image must use an immutable sha256 digest",
        ));
    }
    if input.cpu_millis == 0
        || input.memory_bytes == 0
        || input.pids_limit == 0
        || input.disk_bytes == 0
        || input.output_limit_bytes == 0
        || !(60..=86_400).contains(&input.timeout_seconds)
    {
        return Err(ApiError::bad_request(
            "SANDBOX_LIMIT_INVALID",
            "Sandbox resource limits and TTL must be positive and bounded",
        ));
    }
    let policy = input.network_policy.as_object().ok_or_else(|| {
        ApiError::bad_request(
            "SANDBOX_NETWORK_POLICY_INVALID",
            "Sandbox network policy must be an object",
        )
    })?;
    if policy.get("defaultAction").and_then(Value::as_str) != Some("deny")
        || !matches!(
            policy.get("egressMode").and_then(Value::as_str),
            Some("none" | "tcp_proxy")
        )
        || policy
            .keys()
            .any(|key| !matches!(key.as_str(), "defaultAction" | "egressMode"))
    {
        return Err(ApiError::bad_request(
            "SANDBOX_NETWORK_POLICY_INVALID",
            "Sandbox network policy only accepts defaultAction=deny and egressMode=none|tcp_proxy",
        ));
    }
    Ok(())
}

fn normalize_configuration(input: &mut VersionInput) -> ApiResult<()> {
    input.network_policy =
        normalize_network_policy_value(std::mem::take(&mut input.network_policy));
    validate_configuration(input)
}

fn normalize_network_policy_value(value: Value) -> Value {
    let Value::Object(mut policy) = value else {
        return value;
    };
    policy
        .entry("defaultAction".to_owned())
        .or_insert_with(|| Value::String("deny".into()));
    policy
        .entry("egressMode".to_owned())
        .or_insert_with(|| Value::String("none".into()));
    Value::Object(policy)
}
fn valid_digest(value: &str) -> bool {
    value
        .rsplit_once("@sha256:")
        .is_some_and(|(image, digest)| {
            !image.is_empty()
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}
fn configuration_hash(input: &VersionInput) -> ApiResult<String> {
    let bytes = serde_json::to_vec(input).map_err(ApiError::internal)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn validate_description(value: Option<String>) -> ApiResult<Option<String>> {
    match value {
        Some(value) if value.chars().count() > 1000 => Err(ApiError::bad_request(
            "DESCRIPTION_TOO_LONG",
            "Description must not exceed 1000 characters",
        )),
        Some(value) => Ok(Some(value.trim().to_owned())),
        None => Ok(None),
    }
}
fn map_name_error(error: sqlx::Error) -> ApiError {
    match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => ApiError::conflict(
            "SANDBOX_PROFILE_NAME_EXISTS",
            "A Sandbox Profile with this name already exists",
        ),
        _ => ApiError::from(error),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_digest_and_network_default() {
        assert!(valid_digest(
            "agentx/python@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        assert!(!valid_digest("agentx/python:latest"));
        let mut input=VersionInput{runner:"python".into(),image_digest:"agentx/python@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),cpu_millis:100,memory_bytes:1024,pids_limit:10,disk_bytes:1024,timeout_seconds:60,output_limit_bytes:1024,network_policy:serde_json::json!({"defaultAction":"deny"})};
        assert!(normalize_configuration(&mut input).is_ok());
        assert_eq!(input.network_policy["egressMode"], "none");
    }
}
