use agentx_api_types::PageResponse;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
            "/api/v1/credentials",
            get(list_credentials).post(create_credential),
        )
        .route(
            "/api/v1/credentials/{id}",
            get(get_credential)
                .patch(update_credential)
                .delete(delete_credential),
        )
        .route(
            "/api/v1/credentials/{id}/rotate",
            axum::routing::post(rotate_credential),
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
struct CredentialResponse {
    id: Uuid,
    name: String,
    credential_type: String,
    storage_mode: String,
    masked_hint: String,
    status: String,
    current_secret_version: u64,
    owner_department_id: Uuid,
    version: u64,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateCredentialRequest {
    name: String,
    credential_type: String,
    secret: Value,
    owner_department_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateCredentialRequest {
    name: String,
    status: String,
    version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RotateCredentialRequest {
    secret: Value,
    version: u64,
}

#[derive(Deserialize)]
struct VaultWriteResponse {
    data: VaultWriteData,
}

#[derive(Deserialize)]
struct VaultWriteData {
    version: u64,
}

async fn list_credentials(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<CredentialResponse>>> {
    actor.require("credential:view")?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let status = query.status.unwrap_or_default();
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let (rows, total) = if administrator {
        let rows=sqlx::query("SELECT id,name,credential_type,storage_mode,masked_hint,status,current_secret_version,owner_department_id,version,updated_at FROM credentials WHERE tenant_id=? AND (?='' OR status=?) AND (?='%%' OR name LIKE ?) ORDER BY updated_at DESC,id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM credentials WHERE tenant_id=? AND (?='' OR status=?) AND (?='%%' OR name LIKE ?)").bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    } else {
        let rows=sqlx::query("SELECT c.id,c.name,c.credential_type,c.storage_mode,c.masked_hint,c.status,c.current_secret_version,c.owner_department_id,c.version,c.updated_at FROM credentials c WHERE c.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=c.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=c.owner_department_id) AND (?='' OR c.status=?) AND (?='%%' OR c.name LIKE ?) ORDER BY c.updated_at DESC,c.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM credentials c WHERE c.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=c.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=c.owner_department_id) AND (?='' OR c.status=?) AND (?='%%' OR c.name LIKE ?)").bind(actor.tenant_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    };
    Ok(Json(PageResponse {
        items: rows.into_iter().map(from_row).collect::<Result<_, _>>()?,
        page,
        page_size,
        total: total.try_into().unwrap_or_default(),
    }))
}

async fn create_credential(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateCredentialRequest>,
) -> ApiResult<(StatusCode, Json<CredentialResponse>)> {
    actor.require("credential:manage")?;
    require_department_scope(&state, &actor, input.owner_department_id).await?;
    validate_type(&input.credential_type)?;
    validate_secret(&input.secret)?;
    let name = required_name(&input.name)?;
    let id = Uuid::now_v7();
    let path = format!("tenants/{}/credentials/{id}", actor.tenant_id);
    let provider_version = write_secret(&state, &path, &input.secret).await?;
    let result=async{
        let mut tx=state.pool.begin().await?;
        sqlx::query("INSERT INTO credentials(id,tenant_id,name,credential_type,storage_mode,masked_hint,owner_department_id,created_by) VALUES(?,?,?,?,'external_reference',?,?,?)").bind(id).bind(actor.tenant_id).bind(name).bind(&input.credential_type).bind(mask_hint(&input.secret)).bind(input.owner_department_id).bind(actor.user_id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO credential_secret_versions(id,tenant_id,credential_id,version_number,provider,secret_ref,provider_version,created_by) VALUES(?,?,?,1,'vault_kv_v2',?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(&path).bind(provider_version.to_string()).bind(actor.user_id).execute(&mut *tx).await?;
        audit(&mut tx,&actor,"credential.created",id,json!({"credentialType":input.credential_type})).await?;tx.commit().await?;ApiResult::Ok(())
    }.await;
    if let Err(error) = result {
        destroy_secret(&state, &path, provider_version).await;
        return Err(error);
    }
    Ok((
        StatusCode::CREATED,
        Json(load(&state, actor.tenant_id, id).await?),
    ))
}

async fn get_credential(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<CredentialResponse>> {
    actor.require("credential:view")?;
    require_visible(&state, &actor, id).await?;
    Ok(Json(load(&state, actor.tenant_id, id).await?))
}

async fn update_credential(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateCredentialRequest>,
) -> ApiResult<Json<CredentialResponse>> {
    actor.require("credential:manage")?;
    require_visible(&state, &actor, id).await?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_STATUS",
            "Credential status is invalid",
        ));
    }
    let changed=sqlx::query("UPDATE credentials SET name=?,status=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?").bind(required_name(&input.name)?).bind(&input.status).bind(actor.tenant_id).bind(id).bind(input.version).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "CREDENTIAL_VERSION_CONFLICT",
            "Credential changed",
        ));
    }
    if input.status == "disabled" {
        crate::studio_builder_api::revoke_runs(
            &state,
            actor.tenant_id,
            crate::studio_builder_api::DesignRevocation::Credential(id, input.version),
        )
        .await?;
    }
    Ok(Json(load(&state, actor.tenant_id, id).await?))
}

async fn rotate_credential(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<RotateCredentialRequest>,
) -> ApiResult<Json<CredentialResponse>> {
    actor.require("credential:manage")?;
    require_visible(&state, &actor, id).await?;
    validate_secret(&input.secret)?;
    let row = sqlx::query(
        "SELECT current_secret_version,version FROM credentials WHERE tenant_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    if row.try_get::<u64, _>("version")? != input.version {
        return Err(ApiError::conflict(
            "CREDENTIAL_VERSION_CONFLICT",
            "Credential changed",
        ));
    }
    let next = row
        .try_get::<u64, _>("current_secret_version")?
        .checked_add(1)
        .ok_or_else(|| ApiError::internal("credential version exhausted"))?;
    let path = format!("tenants/{}/credentials/{id}", actor.tenant_id);
    let provider_version = write_secret(&state, &path, &input.secret).await?;
    let result=async{let mut tx=state.pool.begin().await?;let changed=sqlx::query("UPDATE credentials SET current_secret_version=?,masked_hint=?,version=version+1 WHERE tenant_id=? AND id=? AND version=? AND current_secret_version=?").bind(next).bind(mask_hint(&input.secret)).bind(actor.tenant_id).bind(id).bind(input.version).bind(next-1).execute(&mut *tx).await?;if changed.rows_affected()!=1{return Err(ApiError::conflict("CREDENTIAL_VERSION_CONFLICT","Credential changed"));}sqlx::query("INSERT INTO credential_secret_versions(id,tenant_id,credential_id,version_number,provider,secret_ref,provider_version,created_by) VALUES(?,?,?,?, 'vault_kv_v2',?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(next).bind(&path).bind(provider_version.to_string()).bind(actor.user_id).execute(&mut *tx).await?;audit(&mut tx,&actor,"credential.rotated",id,json!({"secretVersion":next})).await?;tx.commit().await?;ApiResult::Ok(())}.await;
    if let Err(error) = result {
        destroy_secret(&state, &path, provider_version).await;
        return Err(error);
    }
    Ok(Json(load(&state, actor.tenant_id, id).await?))
}

async fn delete_credential(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("credential:manage")?;
    require_visible(&state, &actor, id).await?;
    let mut tx = crate::workflow_revision_resources::begin_delete(
        &state.pool,
        actor.tenant_id,
        "credential",
        id,
    )
    .await?;
    let builder_reference: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenant_settings WHERE tenant_id=? AND studio_builder_credential_id=?) OR EXISTS(SELECT 1 FROM studio_builder_runs WHERE tenant_id=? AND credential_id=? AND status IN ('queued','running','waiting','outcome_unknown'))").bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if builder_reference {
        return Err(ApiError::conflict(
            "RESOURCE_REFERENCED",
            "Clear the canvas AI model setting before deleting this resource",
        ));
    }
    let references:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM model_deployments WHERE tenant_id=? AND credential_id=?)+(SELECT COUNT(*) FROM workflow_draft_resources WHERE tenant_id=? AND resource_type='credential' AND resource_id=?)+(SELECT COUNT(*) FROM workflow_version_resources WHERE tenant_id=? AND resource_type='credential' AND resource_id=?)").bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if references != 0 {
        return Err(ApiError::conflict(
            "CREDENTIAL_REFERENCED",
            "Credential is referenced and cannot be deleted",
        ));
    }
    let versions=sqlx::query("SELECT secret_ref,provider_version FROM credential_secret_versions WHERE tenant_id=? AND credential_id=?").bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;

    sqlx::query("DELETE FROM credential_secret_versions WHERE tenant_id=? AND credential_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM credentials WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, &actor, "credential.deleted", id, json!({})).await?;
    tx.commit().await?;
    for version in versions {
        if let Ok(number) = version.try_get::<String, _>("provider_version")?.parse() {
            destroy_secret(&state, &version.try_get::<String, _>("secret_ref")?, number).await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn write_secret(state: &ControlApiState, path: &str, value: &Value) -> ApiResult<u64> {
    let response = state
        .http
        .post(format!(
            "{}/v1/{}/data/{}",
            state.vault_endpoint,
            state.vault_mount.trim_matches('/'),
            path
        ))
        .header("X-Vault-Token", state.vault_token.expose_secret())
        .json(&json!({"data":{"value":value}}))
        .send()
        .await
        .map_err(|_| {
            ApiError::unavailable("VAULT_UNAVAILABLE", "Credential could not be stored")
        })?;
    if !response.status().is_success() {
        return Err(ApiError::unavailable(
            "VAULT_UNAVAILABLE",
            "Credential could not be stored",
        ));
    }
    Ok(response
        .json::<VaultWriteResponse>()
        .await
        .map_err(ApiError::internal)?
        .data
        .version)
}
async fn destroy_secret(state: &ControlApiState, path: &str, version: u64) {
    let _ = state
        .http
        .post(format!(
            "{}/v1/{}/destroy/{}",
            state.vault_endpoint,
            state.vault_mount.trim_matches('/'),
            path
        ))
        .header("X-Vault-Token", state.vault_token.expose_secret())
        .json(&json!({"versions":[version]}))
        .send()
        .await;
}
async fn require_visible(state: &ControlApiState, actor: &Actor, id: Uuid) -> ApiResult<()> {
    let visible: bool = if actor.roles.iter().any(|role| role == "company_admin") {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM credentials WHERE tenant_id=? AND id=?)")
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_one(&state.pool)
            .await?
    } else {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM credentials c JOIN department_closure dc ON dc.tenant_id=c.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=c.owner_department_id WHERE c.tenant_id=? AND c.id=?)").bind(actor.department_id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?
    };
    if visible {
        Ok(())
    } else {
        Err(ApiError::not_found("Credential"))
    }
}
async fn require_department_scope(
    state: &ControlApiState,
    actor: &Actor,
    department: Uuid,
) -> ApiResult<()> {
    if actor.roles.iter().any(|role| role == "company_admin") {
        return Ok(());
    }
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM department_closure WHERE tenant_id=? AND ancestor_id=? AND descendant_id=?)").bind(actor.tenant_id).bind(actor.department_id).bind(department).fetch_one(&state.pool).await?;
    if allowed {
        Ok(())
    } else {
        Err(ApiError::forbidden("Department is outside the actor scope"))
    }
}
async fn load(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<CredentialResponse> {
    let row=sqlx::query("SELECT id,name,credential_type,storage_mode,masked_hint,status,current_secret_version,owner_department_id,version,updated_at FROM credentials WHERE tenant_id=? AND id=?").bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Credential"))?;
    Ok(from_row(row)?)
}
fn from_row(row: MySqlRow) -> Result<CredentialResponse, sqlx::Error> {
    Ok(CredentialResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        credential_type: row.try_get("credential_type")?,
        storage_mode: row.try_get("storage_mode")?,
        masked_hint: row.try_get("masked_hint")?,
        status: row.try_get("status")?,
        current_secret_version: row.try_get("current_secret_version")?,
        owner_department_id: row.try_get("owner_department_id")?,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn validate_type(value: &str) -> ApiResult<()> {
    if matches!(value, "api_key" | "bearer" | "basic" | "custom_json") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "INVALID_CREDENTIAL_TYPE",
            "Credential type is invalid",
        ))
    }
}
fn validate_secret(value: &Value) -> ApiResult<()> {
    if value.is_null() {
        return Err(ApiError::bad_request(
            "SECRET_REQUIRED",
            "Credential secret is required",
        ));
    }
    let size = serde_json::to_vec(value).map_err(ApiError::internal)?.len();
    if size > 64 * 1024 {
        Err(ApiError::bad_request(
            "SECRET_TOO_LARGE",
            "Credential secret must not exceed 64 KiB",
        ))
    } else {
        Ok(())
    }
}
fn mask_hint(value: &Value) -> String {
    let raw = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| "JSON".into());
    let chars = raw.chars().collect::<Vec<_>>();
    if chars.len() <= 4 {
        "••••".into()
    } else {
        format!(
            "••••{}",
            chars[chars.len() - 4..].iter().collect::<String>()
        )
    }
}
async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    actor: &Actor,
    action: &str,
    id: Uuid,
    detail: Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO audit_events(id,tenant_id,actor_user_id,action,target_type,target_id,request_id,detail_json) VALUES(?,?,?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(actor.user_id).bind(action).bind("credential").bind(id.to_string()).bind(Uuid::now_v7()).bind(detail).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_are_validated_and_masked() {
        assert!(validate_type("bearer").is_ok());
        assert!(validate_type("plaintext").is_err());
        assert_eq!(mask_hint(&json!("abcdefgh")), "••••efgh");
        assert!(validate_secret(&Value::Null).is_err());
    }
}
