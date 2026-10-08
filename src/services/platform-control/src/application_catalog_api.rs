use agentx_api_types::PageResponse;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Row, mysql::MySqlRow};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    control_helpers::required_name,
};

pub fn routes() -> Router<ControlApiState> {
    Router::new().route(
        "/api/v1/applications",
        get(list_applications).post(create_application),
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
struct ApplicationResponse {
    id: Uuid,
    workflow_id: Uuid,
    workflow_name: String,
    name: String,
    slug: String,
    description: Option<String>,
    visibility: String,
    status: String,
    owner_department_id: Uuid,
    active_deployment_id: Option<Uuid>,
    active_version_number: Option<u64>,
    runtime_config_revision: u64,
    published_runtime_config_revision: u64,
    version: u64,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateApplicationRequest {
    workflow_id: Uuid,
    name: String,
    slug: String,
    description: Option<String>,
    visibility: String,
}

async fn list_applications(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<ApplicationResponse>>> {
    actor.require("application:view")?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let status = query.status.unwrap_or_default();
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let (rows, total) = if administrator {
        let rows = sqlx::query("SELECT a.id,a.workflow_id,w.name workflow_name,a.name,a.slug,a.description,a.visibility,a.status,a.owner_department_id,h.deployment_id active_deployment_id,wv.version_number active_version_number,a.runtime_config_revision,a.published_runtime_config_revision,a.version,a.updated_at FROM applications a JOIN workflows w ON w.tenant_id=a.tenant_id AND w.id=a.workflow_id LEFT JOIN application_deployment_heads h ON h.tenant_id=a.tenant_id AND h.application_id=a.id LEFT JOIN application_deployments ad ON ad.tenant_id=a.tenant_id AND ad.id=h.deployment_id LEFT JOIN workflow_versions wv ON wv.tenant_id=a.tenant_id AND wv.id=ad.workflow_version_id WHERE a.tenant_id=? AND (?='' OR a.status=?) AND (?='%%' OR a.name LIKE ?) ORDER BY a.updated_at DESC,a.id DESC LIMIT ? OFFSET ?")
            .bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM applications a WHERE a.tenant_id=? AND (?='' OR a.status=?) AND (?='%%' OR a.name LIKE ?)")
            .bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    } else {
        let rows = sqlx::query("SELECT a.id,a.workflow_id,w.name workflow_name,a.name,a.slug,a.description,a.visibility,a.status,a.owner_department_id,h.deployment_id active_deployment_id,wv.version_number active_version_number,a.runtime_config_revision,a.published_runtime_config_revision,a.version,a.updated_at FROM applications a JOIN workflows w ON w.tenant_id=a.tenant_id AND w.id=a.workflow_id LEFT JOIN application_deployment_heads h ON h.tenant_id=a.tenant_id AND h.application_id=a.id LEFT JOIN application_deployments ad ON ad.tenant_id=a.tenant_id AND ad.id=h.deployment_id LEFT JOIN workflow_versions wv ON wv.tenant_id=a.tenant_id AND wv.id=ad.workflow_version_id WHERE a.tenant_id=? AND (a.visibility='company' OR a.owner_user_id=? OR (a.visibility='department' AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=a.tenant_id AND dc.ancestor_id=a.owner_department_id AND dc.descendant_id=?))) AND (?='' OR a.status=?) AND (?='%%' OR a.name LIKE ?) ORDER BY a.updated_at DESC,a.id DESC LIMIT ? OFFSET ?")
            .bind(actor.tenant_id).bind(actor.user_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM applications a WHERE a.tenant_id=? AND (a.visibility='company' OR a.owner_user_id=? OR (a.visibility='department' AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=a.tenant_id AND dc.ancestor_id=a.owner_department_id AND dc.descendant_id=?))) AND (?='' OR a.status=?) AND (?='%%' OR a.name LIKE ?)")
            .bind(actor.tenant_id).bind(actor.user_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    };
    Ok(Json(PageResponse {
        items: rows.into_iter().map(from_row).collect::<Result<_, _>>()?,
        page,
        page_size,
        total: total.try_into().unwrap_or_default(),
    }))
}

async fn create_application(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateApplicationRequest>,
) -> ApiResult<(StatusCode, Json<ApplicationResponse>)> {
    actor.require("application:manage")?;
    validate_visibility(&input.visibility)?;
    let name = required_name(&input.name)?;
    let slug = validate_slug(&input.slug)?;
    let workflow_department: Uuid = sqlx::query_scalar(
        "SELECT owner_department_id FROM workflows WHERE tenant_id=? AND id=? AND status='active'",
    )
    .bind(actor.tenant_id)
    .bind(input.workflow_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Workflow"))?;
    if !actor.roles.iter().any(|role| role == "company_admin") {
        let in_scope: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM department_closure WHERE tenant_id=? AND ancestor_id=? AND descendant_id=?)")
            .bind(actor.tenant_id).bind(actor.department_id).bind(workflow_department).fetch_one(&state.pool).await?;
        if !in_scope {
            return Err(ApiError::forbidden(
                "Workflow is outside the actor department scope",
            ));
        }
    }
    let id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO applications(id,tenant_id,workflow_id,name,slug,description,visibility,owner_user_id,owner_department_id) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(id).bind(actor.tenant_id).bind(input.workflow_id).bind(name).bind(&slug).bind(input.description).bind(input.visibility).bind(actor.user_id).bind(actor.department_id).execute(&mut *tx).await.map_err(map_slug_error)?;
    // Runtime activations start at epoch 1. Persist the same baseline when the
    // Application is created so the first later admission change is allocated
    // as 2 and can never collide with an already-active Deployment route.
    sqlx::query(
        "INSERT INTO admission_epochs(tenant_id,application_id,current_epoch) VALUES(?,?,1)",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO audit_events(id,tenant_id,actor_user_id,action,target_type,target_id,request_id,detail_json) VALUES(?,?,?,?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(actor.user_id).bind("application.created").bind("application").bind(id.to_string()).bind(Uuid::now_v7()).bind(json!({"workflowId":input.workflow_id})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load(&state, actor.tenant_id, id).await?),
    ))
}

async fn load(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<ApplicationResponse> {
    let row=sqlx::query("SELECT a.id,a.workflow_id,w.name workflow_name,a.name,a.slug,a.description,a.visibility,a.status,a.owner_department_id,h.deployment_id active_deployment_id,wv.version_number active_version_number,a.runtime_config_revision,a.published_runtime_config_revision,a.version,a.updated_at FROM applications a JOIN workflows w ON w.tenant_id=a.tenant_id AND w.id=a.workflow_id LEFT JOIN application_deployment_heads h ON h.tenant_id=a.tenant_id AND h.application_id=a.id LEFT JOIN application_deployments ad ON ad.tenant_id=a.tenant_id AND ad.id=h.deployment_id LEFT JOIN workflow_versions wv ON wv.tenant_id=a.tenant_id AND wv.id=ad.workflow_version_id WHERE a.tenant_id=? AND a.id=?").bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Application"))?;
    Ok(from_row(row)?)
}

fn from_row(row: MySqlRow) -> Result<ApplicationResponse, sqlx::Error> {
    Ok(ApplicationResponse {
        id: row.try_get("id")?,
        workflow_id: row.try_get("workflow_id")?,
        workflow_name: row.try_get("workflow_name")?,
        name: row.try_get("name")?,
        slug: row.try_get("slug")?,
        description: row.try_get("description")?,
        visibility: row.try_get("visibility")?,
        status: row.try_get("status")?,
        owner_department_id: row.try_get("owner_department_id")?,
        active_deployment_id: row.try_get("active_deployment_id")?,
        active_version_number: row.try_get("active_version_number")?,
        runtime_config_revision: row.try_get("runtime_config_revision")?,
        published_runtime_config_revision: row.try_get("published_runtime_config_revision")?,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn validate_visibility(value: &str) -> ApiResult<()> {
    if matches!(value, "private" | "department" | "company") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "INVALID_VISIBILITY",
            "Visibility must be private, department, or company",
        ))
    }
}
fn validate_slug(value: &str) -> ApiResult<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 96
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Err(ApiError::bad_request(
            "INVALID_APPLICATION_SLUG",
            "Slug must contain lowercase letters, digits, and hyphens",
        ))
    } else {
        Ok(value)
    }
}
fn map_slug_error(error: sqlx::Error) -> ApiError {
    match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => ApiError::conflict(
            "APPLICATION_SLUG_EXISTS",
            "An application with this slug already exists",
        )
        .with_field_error(
            "slug",
            "APPLICATION_SLUG_EXISTS",
            "An application with this slug already exists",
        ),
        _ => ApiError::from(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_public_input() {
        assert_eq!(validate_slug("demo-app").unwrap(), "demo-app");
        assert!(validate_slug("Bad_App").is_err());
        assert!(validate_visibility("company").is_ok());
    }
}
