use std::collections::{BTreeSet, HashMap};

use agentx_api_types::PageResponse;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySql, QueryBuilder, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/resources/grantable", get(list_grantable))
        .route(
            "/api/v1/resources/{resource_type}/{resource_id}/grants",
            get(list_grants).post(create_grant),
        )
        .route(
            "/api/v1/resources/{resource_type}/{resource_id}/grants/{grant_id}",
            axum::routing::delete(delete_grant),
        )
        .route(
            "/api/v1/workflows/{id}/resource-options",
            get(resource_options),
        )
        .route(
            "/api/v1/workflows/{id}/resource-authorizations",
            post(authorize_resource),
        )
        .route(
            "/api/v1/workflows/{id}/resource-grant-requests",
            post(create_request),
        )
        .route(
            "/api/v1/workflows/{id}/resource-validation",
            get(validate_resources),
        )
        .route(
            "/api/v1/departments/{id}/resource-options",
            get(department_resource_options),
        )
        .route(
            "/api/v1/departments/{id}/resource-authorizations",
            post(authorize_department_resource),
        )
        .route(
            "/api/v1/departments/{id}/resource-grant-requests",
            post(create_department_request),
        )
        .route("/api/v1/resource-grant-requests", get(list_requests))
        .route("/api/v1/resource-grant-requests/{id}", get(get_request))
        .route(
            "/api/v1/resource-grant-requests/{id}/cancel",
            post(cancel_request),
        )
        .route(
            "/api/v1/resource-grant-requests/{id}/reviews/{department_id}/approve",
            post(approve_request),
        )
        .route(
            "/api/v1/resource-grant-requests/{id}/reviews/{department_id}/reject",
            post(reject_request),
        )
}

macro_rules! grantable_query {
    ($tail:literal) => {
        concat!(
            "WITH grantable AS (",
            " SELECT tenant_id,id,CONVERT('credential' USING utf8mb4) COLLATE utf8mb4_0900_ai_ci resource_type,CONVERT(name USING utf8mb4) COLLATE utf8mb4_0900_ai_ci name,CONVERT(credential_type USING utf8mb4) COLLATE utf8mb4_0900_ai_ci detail,CONVERT(status USING utf8mb4) COLLATE utf8mb4_0900_ai_ci status,owner_department_id,updated_at FROM credentials",
            " UNION ALL SELECT a.tenant_id,a.id,CONVERT('model' USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(a.alias USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(CONCAT(d.connection_name,' / ',d.model_name) USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(a.status USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,d.owner_department_id,a.updated_at FROM model_aliases a JOIN model_deployments d ON d.id=a.deployment_id",
            " UNION ALL SELECT s.tenant_id,s.id,CONVERT('mcp_server' USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(s.name USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(sv.endpoint USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(s.status USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,s.owner_department_id,s.updated_at FROM mcp_servers s JOIN mcp_server_versions sv ON sv.server_id=s.id AND sv.version_number=s.current_version_number",
            " UNION ALL SELECT t.tenant_id,t.id,CONVERT('mcp_tool' USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(COALESCE(t.title,t.name) USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(CONCAT(s.name,' / ',t.name) USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(IF(t.availability='available' AND p.enabled=TRUE AND s.status='active','active','unavailable') USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,s.owner_department_id,GREATEST(t.updated_at,p.updated_at,s.updated_at) FROM mcp_tools t JOIN mcp_servers s ON s.id=t.server_id AND s.tenant_id=t.tenant_id JOIN mcp_tool_policies p ON p.tool_id=t.id AND p.tenant_id=t.tenant_id",
            " UNION ALL SELECT tenant_id,id,CONVERT('skill' USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(name USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(COALESCE(description,'') USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(status USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,owner_department_id,updated_at FROM skills",
            " UNION ALL SELECT tenant_id,id,CONVERT('rag' USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(name USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(external_resource_id USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(status USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,owner_department_id,updated_at FROM rag_resources",
            " UNION ALL SELECT tenant_id,id,CONVERT('memory' USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(name USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(external_namespace USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(status USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,owner_department_id,updated_at FROM memory_namespaces",
            " UNION ALL SELECT p.tenant_id,p.id,CONVERT('sandbox_profile' USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(p.name USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(v.image_digest USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,CONVERT(p.status USING utf8mb4) COLLATE utf8mb4_0900_ai_ci,p.owner_department_id,p.updated_at FROM sandbox_profiles p JOIN sandbox_profile_versions v ON v.profile_id=p.id AND v.version_number=p.current_version_number",
            ") ",
            $tail
        )
    };
}

#[path = "resource_request_audit.rs"]
mod request_audit;
#[path = "resource_request_validation.rs"]
mod request_validation;
#[path = "resource_requirements.rs"]
mod requirements;
pub(crate) use requirements::validate_skill_dependency;
use requirements::{ensure_resource, expand_requirements};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    search: Option<String>,
    resource_type: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceSummary {
    id: Uuid,
    resource_type: String,
    name: String,
    detail: String,
    status: String,
    owner_department_id: Uuid,
    grant_count: u64,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantResponse {
    id: Uuid,
    subject_type: String,
    subject_id: Uuid,
    resource_type: String,
    resource_id: Uuid,
    resource_version_id: Option<Uuid>,
    operation: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateGrantRequest {
    subject_type: String,
    subject_id: Uuid,
    resource_version_id: Option<Uuid>,
    operation: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AuthorizationInput {
    resource_type: String,
    resource_id: Uuid,
    resource_version_id: Option<Uuid>,
    operation: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestInput {
    resource_type: String,
    resource_id: Uuid,
    resource_version_id: Option<Uuid>,
    operation: String,
    source_node_id: Option<String>,
    source_revision: Option<u64>,
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewInput {
    expected_version: u64,
    comment: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CancelInput {
    expected_version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OptionQuery {
    resource_type: String,
    operation: Option<String>,
    search: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestListQuery {
    status: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
}

#[derive(Clone)]
struct Requirement {
    resource_type: String,
    resource_id: Uuid,
    resource_version_id: Option<Uuid>,
    operation: String,
    required_by: Option<Uuid>,
    owner_department_id: Uuid,
}

#[derive(Clone, Copy)]
enum GrantSubject {
    Department(Uuid),
    WorkflowServiceIdentity {
        workflow_id: Uuid,
        identity_id: Uuid,
    },
}

impl GrantSubject {
    fn kind(self) -> &'static str {
        match self {
            Self::Department(_) => "department",
            Self::WorkflowServiceIdentity { .. } => "workflow_service_identity",
        }
    }

    fn id(self) -> Uuid {
        match self {
            Self::Department(id) => id,
            Self::WorkflowServiceIdentity { identity_id, .. } => identity_id,
        }
    }

    fn workflow(self) -> Option<Uuid> {
        match self {
            Self::Department(_) => None,
            Self::WorkflowServiceIdentity { workflow_id, .. } => Some(workflow_id),
        }
    }
}

async fn list_grantable(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<ResourceSummary>>> {
    actor.require("resource:grant")?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let kind = query.resource_type.unwrap_or_default();
    if !kind.is_empty() {
        validate_kind(&kind)?;
    }
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let rows = sqlx::query(grantable_query!(
        "SELECT r.*,(SELECT COUNT(*) FROM resource_grants g WHERE g.tenant_id=r.tenant_id AND g.resource_type=r.resource_type AND g.resource_id=r.id) grant_count,COUNT(*) OVER() total_count FROM grantable r WHERE r.tenant_id=? AND (?='' OR r.resource_type=?) AND (?='%%' OR r.name LIKE ? OR r.detail LIKE ?) ORDER BY r.resource_type,r.name LIMIT ? OFFSET ?"
    ))
        .bind(actor.tenant_id)
        .bind(&kind)
        .bind(&kind)
        .bind(&search)
        .bind(&search)
        .bind(&search)
        .bind(page_size)
        .bind(u64::from((page - 1) * page_size))
        .fetch_all(&state.pool)
        .await?;
    let total = rows
        .first()
        .map_or(Ok(0_i64), |row| row.try_get("total_count"))? as u64;
    let items = rows
        .into_iter()
        .map(|row| {
            Ok(ResourceSummary {
                id: row.try_get("id")?,
                resource_type: row.try_get("resource_type")?,
                name: row.try_get("name")?,
                detail: row.try_get("detail")?,
                status: row.try_get("status")?,
                owner_department_id: row.try_get("owner_department_id")?,
                grant_count: row.try_get::<i64, _>("grant_count")? as u64,
                updated_at: row.try_get("updated_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(Json(PageResponse {
        items,
        page,
        page_size,
        total,
    }))
}

async fn list_grants(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((kind, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Vec<GrantResponse>>> {
    actor.require("resource:grant")?;
    ensure_resource(&state, actor.tenant_id, &kind, id).await?;
    let rows = sqlx::query("SELECT id,subject_type,subject_id,resource_type,resource_id,resource_version_id,operation_key,created_at FROM resource_grants WHERE tenant_id=? AND resource_type=? AND resource_id=? ORDER BY created_at,id")
        .bind(actor.tenant_id).bind(kind).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(grant_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

async fn create_grant(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((kind, id)): Path<(String, Uuid)>,
    Json(input): Json<CreateGrantRequest>,
) -> ApiResult<(StatusCode, Json<GrantResponse>)> {
    actor.require("resource:grant")?;
    validate_operation(&input.operation)?;
    ensure_resource(&state, actor.tenant_id, &kind, id).await?;
    match input.subject_type.as_str() {
        "department" => ensure_department(&state, actor.tenant_id, input.subject_id).await?,
        "workflow_service_identity" => {
            ensure_identity(&state, actor.tenant_id, input.subject_id).await?
        }
        _ => {
            return Err(ApiError::bad_request(
                "INVALID_GRANT_SUBJECT",
                "Grant subject type is invalid",
            ));
        }
    }
    let mut tx = state.pool.begin().await?;
    let grant_id = stable_grant_id(
        actor.tenant_id,
        &input.subject_type,
        input.subject_id,
        &kind,
        id,
        &input.operation,
    );
    sqlx::query("INSERT INTO resource_grants(id,tenant_id,subject_type,subject_id,resource_type,resource_id,resource_version_id,operation_key,created_by) VALUES(?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE resource_version_id=VALUES(resource_version_id)")
        .bind(grant_id).bind(actor.tenant_id).bind(&input.subject_type).bind(input.subject_id).bind(&kind).bind(id).bind(input.resource_version_id).bind(&input.operation).bind(actor.user_id).execute(&mut *tx).await?;
    let row = sqlx::query("SELECT id,subject_type,subject_id,resource_type,resource_id,resource_version_id,operation_key,created_at FROM resource_grants WHERE tenant_id=? AND subject_type=? AND subject_id=? AND resource_type=? AND resource_id=? AND operation_key=?")
        .bind(actor.tenant_id).bind(&input.subject_type).bind(input.subject_id).bind(&kind).bind(id).bind(&input.operation).fetch_one(&mut *tx).await?;
    if input.subject_type == "workflow_service_identity" {
        let commands = crate::runtime_admission::advance_service_identity(
            &mut tx,
            actor.tenant_id,
            input.subject_id,
            &[],
        )
        .await?;
        tx.commit().await?;
        crate::runtime_admission::publish_barrier(&state, commands).await?;
        return Ok((StatusCode::CREATED, Json(grant_from_row(row)?)));
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(grant_from_row(row)?)))
}

async fn delete_grant(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((kind, id, grant)): Path<(String, Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    actor.require("resource:grant")?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT subject_type,subject_id,resource_type,resource_id,operation_key FROM resource_grants WHERE tenant_id=? AND id=? AND resource_type=? AND resource_id=? FOR UPDATE")
        .bind(actor.tenant_id).bind(grant).bind(&kind).bind(id).fetch_optional(&mut *tx).await?
        .ok_or_else(|| ApiError::not_found("Resource grant"))?;
    sqlx::query("DELETE FROM resource_grants WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(grant)
        .execute(&mut *tx)
        .await?;
    if row.try_get::<String, _>("subject_type")? == "workflow_service_identity" {
        let identity_id: Uuid = row.try_get("subject_id")?;
        let commands = crate::runtime_admission::advance_service_identity(
            &mut tx,
            actor.tenant_id,
            identity_id,
            &[crate::runtime_admission::RevokedResourceGrant {
                grant_id: grant,
                identity_id,
                resource_type: row.try_get("resource_type")?,
                resource_id: row.try_get("resource_id")?,
                operation: row.try_get("operation_key")?,
            }],
        )
        .await?;
        tx.commit().await?;
        crate::runtime_admission::publish_barrier(&state, commands).await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resource_options(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow): Path<Uuid>,
    Query(query): Query<OptionQuery>,
) -> ApiResult<Json<PageResponse<Value>>> {
    actor.require("workflow:view")?;
    let identity = workflow_identity(&state, actor.tenant_id, workflow).await?;
    validate_kind(&query.resource_type)?;
    let operation = query.operation.unwrap_or_else(|| "use".into());
    validate_operation(&operation)?;
    let can_grant = actor
        .permissions
        .iter()
        .any(|value| value == "resource:grant")
        && actor
            .permissions
            .iter()
            .any(|value| value == "workflow:edit");
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(100).clamp(1, 100);
    let search = format!("%{}%", query.search.unwrap_or_default());
    let rows = sqlx::query(grantable_query!(
        "SELECT r.*,COUNT(*) OVER() total_count FROM grantable r WHERE r.tenant_id=? AND r.resource_type=? AND (?='%%' OR r.name LIKE ? OR r.detail LIKE ?) ORDER BY r.name LIMIT ? OFFSET ?"
    ))
        .bind(actor.tenant_id)
        .bind(&query.resource_type)
        .bind(&search)
        .bind(&search)
        .bind(&search)
        .bind(page_size)
        .bind(u64::from((page - 1) * page_size))
        .fetch_all(&state.pool)
        .await?;
    let total = rows
        .first()
        .map_or(Ok(0_i64), |row| row.try_get("total_count"))? as u64;
    let mut items = Vec::new();
    for row in rows {
        let resource_id: Uuid = row.try_get("id")?;
        let resource_version_id =
            current_resource_version(&state, actor.tenant_id, &query.resource_type, resource_id)
                .await?;
        let requirements = expand_requirements(
            &state,
            actor.tenant_id,
            &AuthorizationInput {
                resource_type: query.resource_type.clone(),
                resource_id,
                resource_version_id,
                operation: operation.clone(),
            },
        )
        .await?;
        let mut requirement_values = Vec::with_capacity(requirements.len());
        let mut authorized = true;
        for requirement in requirements {
            let summary = sqlx::query(grantable_query!(
                "SELECT name,status FROM grantable WHERE tenant_id=? AND resource_type=? AND id=?"
            ))
            .bind(actor.tenant_id)
            .bind(&requirement.resource_type)
            .bind(requirement.resource_id)
            .fetch_one(&state.pool)
            .await?;
            let item_authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grants WHERE tenant_id=? AND subject_type='workflow_service_identity' AND subject_id=? AND resource_type=? AND resource_id=? AND operation_key IN (?, 'manage'))")
                .bind(actor.tenant_id).bind(identity).bind(&requirement.resource_type).bind(requirement.resource_id).bind(&requirement.operation).fetch_one(&state.pool).await?;
            authorized &= item_authorized;
            let item_status: String = summary.try_get("status")?;
            requirement_values.push(json!({
                "resourceType":requirement.resource_type,
                "resourceId":requirement.resource_id,
                "resourceVersionId":requirement.resource_version_id,
                "operation":requirement.operation,
                "name":summary.try_get::<String,_>("name")?,
                "requiredByResourceId":requirement.required_by,
                "ownerDepartmentId":requirement.owner_department_id,
                "authorized":item_authorized,
                "active":item_status=="active"
            }));
        }
        let pending: Option<Uuid> = sqlx::query_scalar("SELECT id FROM resource_grant_requests WHERE tenant_id=? AND workflow_service_identity_id=? AND primary_resource_type=? AND primary_resource_id=? AND operation_key=? AND status='pending' LIMIT 1")
            .bind(actor.tenant_id).bind(identity).bind(&query.resource_type).bind(resource_id).bind(&operation).fetch_optional(&state.pool).await?;
        let rejected: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grant_requests WHERE tenant_id=? AND workflow_service_identity_id=? AND primary_resource_type=? AND primary_resource_id=? AND operation_key=? AND status='rejected')")
            .bind(actor.tenant_id).bind(identity).bind(&query.resource_type).bind(resource_id).bind(&operation).fetch_one(&state.pool).await?;
        let name: String = row.try_get("name")?;
        let status: String = row.try_get("status")?;
        let access_state =
            resource_access_state(authorized, pending.is_some(), can_grant, rejected, &status);
        items.push(json!({
                "id":resource_id,"resourceType":query.resource_type,"name":name,
            "detail":row.try_get::<String,_>("detail")?,"status":status,
            "resourceVersionId":resource_version_id,
            "accessState":access_state,
            "pendingRequestId":pending,
            "requirements":requirement_values
        }));
    }
    attach_model_metadata(&state, actor.tenant_id, &query.resource_type, &mut items).await?;
    Ok(Json(PageResponse {
        items,
        page,
        page_size,
        total,
    }))
}

// Studio budget defaults follow the selected model, so model options carry the
// deployment token limits and the latest price currency alongside the label.
async fn attach_model_metadata(
    state: &ControlApiState,
    tenant_id: Uuid,
    resource_type: &str,
    items: &mut [Value],
) -> ApiResult<()> {
    if resource_type != "model" || items.is_empty() {
        return Ok(());
    }
    // model_aliases.id is BINARY(16); binding the hyphenated string would
    // never match, so parse each option id back to a Uuid before binding.
    let ids: Vec<Uuid> = items
        .iter()
        .filter_map(|item| item["id"].as_str().and_then(|id| Uuid::parse_str(id).ok()))
        .collect();
    let mut query = QueryBuilder::<MySql>::new(
        "SELECT a.id,d.max_input_tokens,d.max_output_tokens,pv.currency FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id LEFT JOIN model_price_versions pv ON pv.tenant_id=d.tenant_id AND pv.deployment_id=d.id AND pv.id=(SELECT latest.id FROM model_price_versions latest WHERE latest.tenant_id=d.tenant_id AND latest.deployment_id=d.id ORDER BY latest.version_number DESC LIMIT 1) WHERE a.tenant_id=",
    );
    query.push_bind(tenant_id).push(" AND a.id IN (");
    let mut separated = query.separated(",");
    for id in &ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(")");
    let metadata: HashMap<String, Value> = query
        .build()
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id")?;
            Ok((
                id.to_string(),
                json!({
                    "maxInputTokens":row.try_get::<u64,_>("max_input_tokens")?,
                    "maxOutputTokens":row.try_get::<u64,_>("max_output_tokens")?,
                    "currency":row.try_get::<Option<String>,_>("currency")?,
                }),
            ))
        })
        .collect::<Result<HashMap<_, _>, sqlx::Error>>()
        .map_err(ApiError::from)?;
    for item in items.iter_mut() {
        if let Some(metadata) = item["id"].as_str().and_then(|id| metadata.get(id)) {
            item["metadata"] = metadata.clone();
        }
    }
    Ok(())
}

async fn department_resource_options(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(department): Path<Uuid>,
    Query(query): Query<OptionQuery>,
) -> ApiResult<Json<PageResponse<Value>>> {
    actor.require("mcp:manage")?;
    ensure_department_in_actor_scope(&state, &actor, department).await?;
    validate_mcp_dependency_kind(&query.resource_type)?;
    let operation = query.operation.unwrap_or_else(|| "use".into());
    if operation != "use" {
        return Err(ApiError::bad_request(
            "MCP_DEPENDENCY_OPERATION_INVALID",
            "MCP configuration dependencies only support the use operation",
        ));
    }
    let can_grant = actor
        .permissions
        .iter()
        .any(|value| value == "resource:grant");
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(100).clamp(1, 100);
    let search = format!("%{}%", query.search.unwrap_or_default());
    let rows = sqlx::query(grantable_query!(
        "SELECT r.*,COUNT(*) OVER() total_count FROM grantable r WHERE r.tenant_id=? AND r.resource_type=? AND (?='%%' OR r.name LIKE ? OR r.detail LIKE ?) ORDER BY r.name LIMIT ? OFFSET ?"
    ))
    .bind(actor.tenant_id)
    .bind(&query.resource_type)
    .bind(&search)
    .bind(&search)
    .bind(&search)
    .bind(page_size)
    .bind(u64::from((page - 1) * page_size))
    .fetch_all(&state.pool)
    .await?;
    let total = rows
        .first()
        .map_or(Ok(0_i64), |row| row.try_get("total_count"))? as u64;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let resource_id: Uuid = row.try_get("id")?;
        let owner_department_id: Uuid = row.try_get("owner_department_id")?;
        let version_id =
            current_resource_version(&state, actor.tenant_id, &query.resource_type, resource_id)
                .await?;
        let authorized = department_resource_authorized(
            &state,
            actor.tenant_id,
            department,
            &query.resource_type,
            resource_id,
            version_id,
            owner_department_id,
            "use",
        )
        .await?;
        let can_direct_grant =
            can_grant && actor_in_department_scope(&state, &actor, owner_department_id).await?;
        let pending: Option<Uuid> = sqlx::query_scalar("SELECT id FROM resource_grant_requests WHERE tenant_id=? AND subject_type='department' AND subject_id=? AND primary_resource_type=? AND primary_resource_id=? AND operation_key='use' AND status='pending' LIMIT 1")
            .bind(actor.tenant_id).bind(department).bind(&query.resource_type).bind(resource_id).fetch_optional(&state.pool).await?;
        let rejected: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grant_requests WHERE tenant_id=? AND subject_type='department' AND subject_id=? AND primary_resource_type=? AND primary_resource_id=? AND operation_key='use' AND status='rejected')")
            .bind(actor.tenant_id).bind(department).bind(&query.resource_type).bind(resource_id).fetch_one(&state.pool).await?;
        let status: String = row.try_get("status")?;
        items.push(json!({
            "id":resource_id,
            "resourceType":query.resource_type,
            "name":row.try_get::<String,_>("name")?,
            "detail":row.try_get::<String,_>("detail")?,
            "status":status,
            "resourceVersionId":version_id,
            "accessState":resource_access_state(authorized,pending.is_some(),can_direct_grant,rejected,&status),
            "pendingRequestId":pending,
            "requirements":[{
                "resourceType":query.resource_type,
                "resourceId":resource_id,
                "resourceVersionId":version_id,
                "operation":"use",
                "name":row.try_get::<String,_>("name")?,
                "requiredByResourceId":Value::Null,
                "ownerDepartmentId":owner_department_id,
                "authorized":authorized,
                "active":status=="active"
            }]
        }));
    }
    Ok(Json(PageResponse {
        items,
        page,
        page_size,
        total,
    }))
}

async fn authorize_department_resource(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(department): Path<Uuid>,
    Json(input): Json<AuthorizationInput>,
) -> ApiResult<Json<Value>> {
    actor.require("mcp:manage")?;
    actor.require("resource:grant")?;
    ensure_department_in_actor_scope(&state, &actor, department).await?;
    validate_mcp_dependency(&state, actor.tenant_id, &input).await?;
    let requirements = expand_requirements(&state, actor.tenant_id, &input).await?;
    for requirement in &requirements {
        if !actor_in_department_scope(&state, &actor, requirement.owner_department_id).await? {
            return Err(ApiError::forbidden(
                "Resource owner department review is required",
            ));
        }
    }
    let subject = GrantSubject::Department(department);
    let mut tx = state.pool.begin().await?;
    let (granted, existing) = insert_subject_grants(&mut tx, &actor, subject, requirements).await?;
    sqlx::query("UPDATE resource_grant_requests SET status='approved',open_dedupe_key=NULL,resolved_at=UTC_TIMESTAMP(6),version=version+1 WHERE tenant_id=? AND subject_type='department' AND subject_id=? AND primary_resource_type=? AND primary_resource_id=? AND operation_key='use' AND status='pending'")
        .bind(actor.tenant_id).bind(department).bind(&input.resource_type).bind(input.resource_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({
        "departmentId":department,
        "resourceType":input.resource_type,
        "resourceId":input.resource_id,
        "grantedCount":granted,
        "alreadyGrantedCount":existing
    })))
}

async fn create_department_request(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(department): Path<Uuid>,
    Json(input): Json<RequestInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("mcp:manage")?;
    ensure_department_in_actor_scope(&state, &actor, department).await?;
    validate_mcp_dependency(
        &state,
        actor.tenant_id,
        &AuthorizationInput {
            resource_type: input.resource_type.clone(),
            resource_id: input.resource_id,
            resource_version_id: input.resource_version_id,
            operation: input.operation.clone(),
        },
    )
    .await?;
    create_subject_request(&state, &actor, GrantSubject::Department(department), input).await
}

async fn authorize_resource(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow): Path<Uuid>,
    Json(input): Json<AuthorizationInput>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:edit")?;
    actor.require("resource:grant")?;
    let identity = workflow_identity(&state, actor.tenant_id, workflow).await?;
    let requirements = expand_requirements(&state, actor.tenant_id, &input).await?;
    let mut tx = state.pool.begin().await?;
    let mut granted = 0_u64;
    let mut existing = 0_u64;
    for item in requirements {
        let grant_id = stable_grant_id(
            actor.tenant_id,
            "workflow_service_identity",
            identity,
            &item.resource_type,
            item.resource_id,
            &item.operation,
        );
        let result = sqlx::query("INSERT IGNORE INTO resource_grants(id,tenant_id,subject_type,subject_id,resource_type,resource_id,resource_version_id,operation_key,created_by) VALUES(?,?,'workflow_service_identity',?,?,?,?,?,?)")
            .bind(grant_id).bind(actor.tenant_id).bind(identity).bind(item.resource_type).bind(item.resource_id).bind(item.resource_version_id).bind(item.operation).bind(actor.user_id).execute(&mut *tx).await?;
        if result.rows_affected() == 1 {
            granted += 1
        } else {
            existing += 1
        }
    }
    sqlx::query("UPDATE resource_grant_requests SET status='approved',open_dedupe_key=NULL,resolved_at=UTC_TIMESTAMP(6),version=version+1 WHERE tenant_id=? AND workflow_service_identity_id=? AND primary_resource_type=? AND primary_resource_id=? AND operation_key=? AND status='pending'")
        .bind(actor.tenant_id).bind(identity).bind(&input.resource_type).bind(input.resource_id).bind(&input.operation).execute(&mut *tx).await?;
    let commands = if granted != 0 {
        crate::runtime_admission::advance_service_identity(&mut tx, actor.tenant_id, identity, &[])
            .await?
    } else {
        Vec::new()
    };
    tx.commit().await?;
    crate::runtime_admission::publish_barrier(&state, commands).await?;
    Ok(Json(
        json!({"workflowId":workflow,"resourceType":input.resource_type,"resourceId":input.resource_id,"grantedCount":granted,"alreadyGrantedCount":existing}),
    ))
}

async fn validate_resources(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    actor.require("workflow:view")?;
    let identity = workflow_identity(&state, actor.tenant_id, workflow).await?;
    let definition: Value = sqlx::query_scalar(
        "SELECT definition_json FROM workflow_drafts WHERE tenant_id=? AND workflow_id=?",
    )
    .bind(actor.tenant_id)
    .bind(workflow)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Workflow draft"))?;
    let mut references = Vec::new();
    collect_references(&definition, &mut references);
    let mut missing = Vec::new();
    for (node, kind, id, operation) in references {
        let granted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grants WHERE tenant_id=? AND subject_type='workflow_service_identity' AND subject_id=? AND resource_type=? AND resource_id=? AND operation_key IN (?, 'manage'))")
            .bind(actor.tenant_id).bind(identity).bind(&kind).bind(id).bind(&operation).fetch_one(&state.pool).await?;
        if !granted {
            missing.push(json!({"nodeId":node,"resourceType":kind,"resourceId":id,"operation":operation,"reason":"grant_missing","requiredByResourceId":Value::Null}));
        }
    }
    Ok(Json(
        json!({"valid":missing.is_empty(),"missingGrants":missing}),
    ))
}

async fn create_request(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow): Path<Uuid>,
    Json(input): Json<RequestInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("workflow:edit")?;
    let identity = workflow_identity(&state, actor.tenant_id, workflow).await?;
    create_subject_request(
        &state,
        &actor,
        GrantSubject::WorkflowServiceIdentity {
            workflow_id: workflow,
            identity_id: identity,
        },
        input,
    )
    .await
}

async fn create_subject_request(
    state: &ControlApiState,
    actor: &Actor,
    subject: GrantSubject,
    input: RequestInput,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let root = AuthorizationInput {
        resource_type: input.resource_type.clone(),
        resource_id: input.resource_id,
        resource_version_id: input.resource_version_id,
        operation: input.operation.clone(),
    };
    let requirements = expand_requirements(state, actor.tenant_id, &root).await?;
    if input
        .message
        .as_ref()
        .is_some_and(|value| value.chars().count() > 1000)
    {
        return Err(ApiError::bad_request(
            "RESOURCE_GRANT_REQUEST_MESSAGE_INVALID",
            "Request message must not exceed 1000 characters",
        ));
    }
    if let Some(id) = sqlx::query_scalar::<_,Uuid>("SELECT id FROM resource_grant_requests WHERE tenant_id=? AND subject_type=? AND subject_id=? AND primary_resource_type=? AND primary_resource_id=? AND operation_key=? AND status='pending'")
        .bind(actor.tenant_id).bind(subject.kind()).bind(subject.id()).bind(&input.resource_type).bind(input.resource_id).bind(&input.operation).fetch_optional(&state.pool).await? {
        return Ok((StatusCode::OK,Json(load_request(state,actor,id).await?)));
    }
    let hash = format!(
        "sha256:{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({
                "subjectType":subject.kind(),
                "subjectId":subject.id(),
                "request":input
            }))
            .map_err(ApiError::internal)?,
        )
    );
    let request = Uuid::now_v7();
    let dependency_hash =
        request_validation::dependency_hash(state, actor.tenant_id, &requirements)
            .await?
            .ok_or_else(|| {
                ApiError::conflict("RESOURCE_GRANT_REQUEST_STALE", "Resource package changed")
            })?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO resource_grant_requests(id,tenant_id,subject_type,subject_id,workflow_id,workflow_service_identity_id,primary_resource_type,primary_resource_id,primary_resource_version_id,operation_key,source_node_id,source_revision,request_message,dependency_hash,open_dedupe_key,requested_by) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(request).bind(actor.tenant_id).bind(subject.kind()).bind(subject.id()).bind(subject.workflow()).bind(match subject { GrantSubject::WorkflowServiceIdentity { identity_id, .. } => Some(identity_id), GrantSubject::Department(_) => None }).bind(&input.resource_type).bind(input.resource_id).bind(input.resource_version_id).bind(&input.operation).bind(input.source_node_id).bind(input.source_revision).bind(input.message).bind(&dependency_hash).bind(&hash).bind(actor.user_id).execute(&mut *tx).await?;
    let mut departments = BTreeSet::new();
    for item in requirements {
        departments.insert(item.owner_department_id);
        sqlx::query("INSERT INTO resource_grant_request_items(id,tenant_id,request_id,resource_type,resource_id,resource_version_id,operation_key,required_by_resource_id,owner_department_id) VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(request).bind(item.resource_type).bind(item.resource_id).bind(item.resource_version_id).bind(item.operation).bind(item.required_by).bind(item.owner_department_id).execute(&mut *tx).await?;
    }
    for department in departments {
        let review_id = Uuid::now_v7();
        sqlx::query("INSERT INTO resource_grant_request_reviews(id,tenant_id,request_id,owner_department_id) VALUES(?,?,?,?)")
            .bind(review_id).bind(actor.tenant_id).bind(request).bind(department).execute(&mut *tx).await?;
        notify_department_reviewers(&mut tx, actor.tenant_id, review_id, request, department)
            .await?;
    }
    request_audit::record(&mut tx, actor, request, "submitted").await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_request(state, actor, request).await?),
    ))
}

async fn list_requests(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<RequestListQuery>,
) -> ApiResult<Json<PageResponse<Value>>> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(100).clamp(1, 100);
    let status = query.status.unwrap_or_default();
    let offset = u64::from((page - 1) * page_size);
    let rows = if actor.roles.iter().any(|role| role == "company_admin") {
        sqlx::query("SELECT id,COUNT(*) OVER() total_count FROM resource_grant_requests WHERE tenant_id=? AND (?='' OR status=?) ORDER BY updated_at DESC,id DESC LIMIT ? OFFSET ?")
            .bind(actor.tenant_id).bind(&status).bind(&status).bind(page_size).bind(offset)
            .fetch_all(&state.pool).await?
    } else {
        sqlx::query("SELECT r.id,COUNT(*) OVER() total_count FROM resource_grant_requests r WHERE r.tenant_id=? AND (?='' OR r.status=?) AND (r.requested_by=? OR EXISTS(SELECT 1 FROM resource_grant_request_reviews rv JOIN user_roles ur ON ur.tenant_id=rv.tenant_id AND ur.user_id=? JOIN roles role ON role.id=ur.role_id AND role.tenant_id=ur.tenant_id AND role.status='active' AND role.code IN ('company_admin','department_admin') JOIN department_closure dc ON dc.tenant_id=ur.tenant_id AND dc.ancestor_id=COALESCE(ur.scope_department_id,?) WHERE rv.tenant_id=r.tenant_id AND rv.request_id=r.id AND dc.descendant_id=rv.owner_department_id)) ORDER BY r.updated_at DESC,r.id DESC LIMIT ? OFFSET ?")
            .bind(actor.tenant_id).bind(&status).bind(&status).bind(actor.user_id).bind(actor.user_id)
            .bind(actor.department_id).bind(page_size).bind(offset).fetch_all(&state.pool).await?
    };
    let total = rows
        .first()
        .map_or(Ok(0_i64), |r| r.try_get("total_count"))? as u64;
    let mut items = Vec::new();
    for row in rows {
        items.push(load_request(&state, &actor, row.try_get("id")?).await?);
    }
    Ok(Json(PageResponse {
        items,
        page,
        page_size,
        total,
    }))
}

async fn get_request(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    Ok(Json(load_request(&state, &actor, id).await?))
}

async fn cancel_request(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<CancelInput>,
) -> ApiResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let result=sqlx::query("UPDATE resource_grant_requests SET status='cancelled',open_dedupe_key=NULL,resolved_at=UTC_TIMESTAMP(6),version=version+1 WHERE tenant_id=? AND id=? AND status='pending' AND version=? AND requested_by=?")
        .bind(actor.tenant_id).bind(id).bind(input.expected_version).bind(actor.user_id).execute(&mut *tx).await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "RESOURCE_GRANT_REQUEST_STATE_CONFLICT",
            "Resource grant request changed or cannot be cancelled",
        ));
    }
    request_audit::record(&mut tx, &actor, id, "cancelled").await?;
    tx.commit().await?;
    Ok(Json(load_request(&state, &actor, id).await?))
}

async fn approve_request(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, department)): Path<(Uuid, Uuid)>,
    Json(input): Json<ReviewInput>,
) -> ApiResult<Json<Value>> {
    review(&state, &actor, id, department, input, true).await?;
    Ok(Json(load_request(&state, &actor, id).await?))
}
async fn reject_request(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, department)): Path<(Uuid, Uuid)>,
    Json(input): Json<ReviewInput>,
) -> ApiResult<Json<Value>> {
    review(&state, &actor, id, department, input, false).await?;
    Ok(Json(load_request(&state, &actor, id).await?))
}

async fn review(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
    department: Uuid,
    input: ReviewInput,
    approve: bool,
) -> ApiResult<()> {
    actor.require("approval:act")?;
    actor.require("resource:grant")?;
    let can_review = actor_in_department_scope(state, actor, department).await?;
    if !can_review {
        return Err(ApiError::forbidden("Department review scope is required"));
    }
    let mut tx = state.pool.begin().await?;
    let request = request_validation::lock_pending(&mut tx, actor.tenant_id, id).await?;
    if !request_validation::is_current(state, &mut tx, actor.tenant_id, &request).await? {
        sqlx::query("UPDATE resource_grant_requests SET status='stale',open_dedupe_key=NULL,resolved_at=UTC_TIMESTAMP(6),version=version+1 WHERE tenant_id=? AND id=?")
            .bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
        request_audit::record(&mut tx, actor, id, "stale").await?;
        notify_requester(
            &mut tx,
            actor.tenant_id,
            id,
            "resource_grant_request_stale",
            "notifications.resourceGrantStale.title",
            "notifications.resourceGrantStale.body",
            "warning",
        )
        .await?;
        tx.commit().await?;
        return Err(ApiError::conflict(
            "RESOURCE_GRANT_REQUEST_STALE",
            "Resource package or requester changed; submit a new request",
        ));
    }
    let mut admission_commands = Vec::new();
    let changed=sqlx::query("UPDATE resource_grant_request_reviews SET status=?,reviewed_by=?,review_comment=?,reviewed_at=UTC_TIMESTAMP(6),version=version+1 WHERE tenant_id=? AND request_id=? AND owner_department_id=? AND status='pending' AND version=?")
        .bind(if approve{"approved"}else{"rejected"}).bind(actor.user_id).bind(input.comment).bind(actor.tenant_id).bind(id).bind(department).bind(input.expected_version).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "RESOURCE_GRANT_REVIEW_CONFLICT",
            "Department review changed",
        ));
    }
    request_audit::record(
        &mut tx,
        actor,
        id,
        if approve {
            "review_approved"
        } else {
            "review_rejected"
        },
    )
    .await?;
    if !approve {
        sqlx::query("UPDATE resource_grant_requests SET status='rejected',open_dedupe_key=NULL,resolved_at=UTC_TIMESTAMP(6),version=version+1 WHERE tenant_id=? AND id=? AND status='pending'").bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
        request_audit::record(&mut tx, actor, id, "rejected").await?;
        notify_requester(
            &mut tx,
            actor.tenant_id,
            id,
            "resource_grant_request_rejected",
            "notifications.resourceGrantRejected.title",
            "notifications.resourceGrantRejected.body",
            "danger",
        )
        .await?;
    } else {
        let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM resource_grant_request_reviews WHERE tenant_id=? AND request_id=? AND status='pending'").bind(actor.tenant_id).bind(id).fetch_one(&mut *tx).await?;
        if pending == 0 {
            admission_commands = finalize_approved(&mut tx, actor, id).await?;
        }
    }
    tx.commit().await?;
    crate::runtime_admission::publish_barrier(state, admission_commands).await?;
    Ok(())
}

async fn finalize_approved(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<Vec<agentx_runtime_contracts::RuntimeAdmissionCommandV1>> {
    let row=sqlx::query("SELECT subject_type,subject_id,workflow_id,workflow_service_identity_id FROM resource_grant_requests WHERE tenant_id=? AND id=? AND status='pending' FOR UPDATE").bind(actor.tenant_id).bind(id).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::conflict("RESOURCE_GRANT_REQUEST_STATE_CONFLICT","Request is no longer pending"))?;
    let subject_type: String = row.try_get("subject_type")?;
    let subject_id: Uuid = row.try_get("subject_id")?;
    let subject = match subject_type.as_str() {
        "department" => GrantSubject::Department(subject_id),
        "workflow_service_identity" => GrantSubject::WorkflowServiceIdentity {
            workflow_id: row
                .try_get::<Option<Uuid>, _>("workflow_id")?
                .ok_or_else(|| {
                    ApiError::internal("Workflow grant request is missing workflow_id")
                })?,
            identity_id: row
                .try_get::<Option<Uuid>, _>("workflow_service_identity_id")?
                .ok_or_else(|| {
                    ApiError::internal("Workflow grant request is missing service identity")
                })?,
        },
        _ => {
            return Err(ApiError::internal(
                "Resource grant request subject is invalid",
            ));
        }
    };
    let items=sqlx::query("SELECT resource_type,resource_id,resource_version_id,operation_key FROM resource_grant_request_items WHERE tenant_id=? AND request_id=?").bind(actor.tenant_id).bind(id).fetch_all(&mut **tx).await?;
    let requirements = items
        .into_iter()
        .map(|item| {
            Ok(Requirement {
                resource_type: item.try_get("resource_type")?,
                resource_id: item.try_get("resource_id")?,
                resource_version_id: item.try_get("resource_version_id")?,
                operation: item.try_get("operation_key")?,
                required_by: None,
                owner_department_id: Uuid::nil(),
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let (granted, _) = insert_subject_grants(tx, actor, subject, requirements).await?;
    sqlx::query("UPDATE resource_grant_requests SET status='approved',open_dedupe_key=NULL,resolved_at=UTC_TIMESTAMP(6),version=version+1 WHERE tenant_id=? AND id=?").bind(actor.tenant_id).bind(id).execute(&mut **tx).await?;
    request_audit::record(tx, actor, id, "approved").await?;
    let commands = match subject {
        GrantSubject::WorkflowServiceIdentity { identity_id, .. } if granted != 0 => {
            crate::runtime_admission::advance_service_identity(
                tx,
                actor.tenant_id,
                identity_id,
                &[],
            )
            .await?
        }
        _ => Vec::new(),
    };
    notify_requester(
        tx,
        actor.tenant_id,
        id,
        "resource_grant_request_approved",
        "notifications.resourceGrantApproved.title",
        "notifications.resourceGrantApproved.body",
        "success",
    )
    .await?;
    Ok(commands)
}

async fn insert_subject_grants(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    subject: GrantSubject,
    requirements: Vec<Requirement>,
) -> ApiResult<(u64, u64)> {
    let mut granted = 0;
    let mut existing = 0;
    for item in requirements {
        let grant_id = stable_grant_id(
            actor.tenant_id,
            subject.kind(),
            subject.id(),
            &item.resource_type,
            item.resource_id,
            &item.operation,
        );
        let result = sqlx::query("INSERT IGNORE INTO resource_grants(id,tenant_id,subject_type,subject_id,resource_type,resource_id,resource_version_id,operation_key,created_by) VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(grant_id).bind(actor.tenant_id).bind(subject.kind()).bind(subject.id()).bind(item.resource_type).bind(item.resource_id).bind(item.resource_version_id).bind(item.operation).bind(actor.user_id).execute(&mut **tx).await?;
        if result.rows_affected() == 1 {
            granted += 1;
        } else {
            existing += 1;
        }
    }
    Ok((granted, existing))
}

async fn notify_department_reviewers(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    source_event: Uuid,
    request_id: Uuid,
    department: Uuid,
) -> ApiResult<()> {
    let mut users: Vec<Uuid> = sqlx::query_scalar("SELECT ur.user_id FROM user_roles ur JOIN roles r ON r.id=ur.role_id AND r.tenant_id=ur.tenant_id JOIN role_permissions rp ON rp.tenant_id=ur.tenant_id AND rp.role_id=r.id JOIN permissions p ON p.id=rp.permission_id JOIN department_closure dc ON dc.tenant_id=ur.tenant_id AND dc.ancestor_id=ur.scope_department_id JOIN users u ON u.id=ur.user_id AND u.tenant_id=ur.tenant_id WHERE ur.tenant_id=? AND r.code='department_admin' AND r.status='active' AND dc.descendant_id=? AND u.status='active' AND p.permission_key IN ('approval:act','resource:grant') GROUP BY ur.user_id HAVING COUNT(DISTINCT p.permission_key)=2")
        .bind(tenant).bind(department).fetch_all(&mut **tx).await?;
    if users.is_empty() {
        users = sqlx::query_scalar("SELECT ur.user_id FROM user_roles ur JOIN roles r ON r.id=ur.role_id AND r.tenant_id=ur.tenant_id JOIN role_permissions rp ON rp.tenant_id=ur.tenant_id AND rp.role_id=r.id JOIN permissions p ON p.id=rp.permission_id JOIN users u ON u.id=ur.user_id AND u.tenant_id=ur.tenant_id WHERE ur.tenant_id=? AND r.code='company_admin' AND r.status='active' AND u.status='active' AND p.permission_key IN ('approval:act','resource:grant') GROUP BY ur.user_id HAVING COUNT(DISTINCT p.permission_key)=2")
            .bind(tenant).fetch_all(&mut **tx).await?;
    }
    create_notification(
        tx,
        tenant,
        source_event,
        request_id,
        &users,
        "resource_grant_request_created",
        "notifications.resourceGrantRequested.title",
        "notifications.resourceGrantRequested.body",
        "warning",
    )
    .await
}

async fn notify_requester(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    request_id: Uuid,
    kind: &str,
    title: &str,
    body: &str,
    tone: &str,
) -> ApiResult<()> {
    let requester: Uuid = sqlx::query_scalar(
        "SELECT requested_by FROM resource_grant_requests WHERE tenant_id=? AND id=?",
    )
    .bind(tenant)
    .bind(request_id)
    .fetch_one(&mut **tx)
    .await?;
    create_notification(
        tx,
        tenant,
        request_id,
        request_id,
        &[requester],
        kind,
        title,
        body,
        tone,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_notification(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    source_event: Uuid,
    request_id: Uuid,
    users: &[Uuid],
    kind: &str,
    title: &str,
    body: &str,
    tone: &str,
) -> ApiResult<()> {
    let generation: Option<u64> = sqlx::query_scalar("SELECT NULLIF(active_generation,0) FROM runtime_projection_status WHERE projection_name='runtime_governance_v1' AND partition_key='global'")
        .fetch_optional(&mut **tx).await?.flatten();
    let notification_id = Uuid::now_v7();
    sqlx::query("INSERT INTO notifications(id,tenant_id,source_event_id,notification_type,title_key,body_key,arguments_json,target_type,target_id,target_path,tone,projection_generation) VALUES(?,?,?,?,?,?,JSON_OBJECT(),'resource_grant_request',?,?,?,?) ON DUPLICATE KEY UPDATE id=id")
        .bind(notification_id).bind(tenant).bind(source_event).bind(kind).bind(title).bind(body)
        .bind(request_id).bind(format!("/approvals/resource-grants/{request_id}")).bind(tone)
        .bind(generation.unwrap_or(1)).execute(&mut **tx).await?;
    let stored: Uuid = sqlx::query_scalar("SELECT id FROM notifications WHERE tenant_id=? AND source_event_id=? AND notification_type=?")
        .bind(tenant).bind(source_event).bind(kind).fetch_one(&mut **tx).await?;
    for user in users {
        sqlx::query("INSERT IGNORE INTO notification_receipts(tenant_id,notification_id,user_id) VALUES(?,?,?)")
            .bind(tenant).bind(stored).bind(user).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn actor_in_department_scope(
    state: &ControlApiState,
    actor: &Actor,
    department: Uuid,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_roles ur JOIN roles r ON r.tenant_id=ur.tenant_id AND r.id=ur.role_id AND r.status='active' AND r.code IN ('company_admin','department_admin') JOIN department_closure dc ON dc.tenant_id=ur.tenant_id AND dc.ancestor_id=COALESCE(ur.scope_department_id,?) WHERE ur.tenant_id=? AND ur.user_id=? AND dc.descendant_id=?)")
        .bind(actor.department_id)
        .bind(actor.tenant_id)
        .bind(actor.user_id)
        .bind(department)
        .fetch_one(&state.pool)
        .await?)
}

async fn load_request(state: &ControlApiState, actor: &Actor, id: Uuid) -> ApiResult<Value> {
    let row=sqlx::query("SELECT r.*,w.name workflow_name,d.name subject_department_name,u.display_name requester_name FROM resource_grant_requests r LEFT JOIN workflows w ON w.tenant_id=r.tenant_id AND w.id=r.workflow_id LEFT JOIN departments d ON d.tenant_id=r.tenant_id AND r.subject_type='department' AND d.id=r.subject_id JOIN users u ON u.tenant_id=r.tenant_id AND u.id=r.requested_by WHERE r.tenant_id=? AND r.id=?").bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Resource grant request"))?;
    let item_rows=sqlx::query("SELECT * FROM resource_grant_request_items WHERE tenant_id=? AND request_id=? ORDER BY created_at,id").bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    let requester: Uuid = row.try_get("requested_by")?;
    let full_view =
        requester == actor.user_id || actor.roles.iter().any(|role| role == "company_admin");
    let subject_type: String = row.try_get("subject_type")?;
    let subject_id: Uuid = row.try_get("subject_id")?;
    let mut items = Vec::new();
    let mut visible_resources = BTreeSet::new();
    for item in item_rows {
        let owner_department_id: Uuid = item.try_get("owner_department_id")?;
        let resource_visible = if full_view {
            true
        } else {
            actor_in_department_scope(state, actor, owner_department_id).await?
        };
        let resource_type: String = item.try_get("resource_type")?;
        let resource_id: Uuid = item.try_get("resource_id")?;
        if resource_visible {
            visible_resources.insert((resource_type.clone(), resource_id));
        }
        let operation: String = item.try_get("operation_key")?;
        let summary = sqlx::query(grantable_query!(
            "SELECT name,status FROM grantable WHERE tenant_id=? AND resource_type=? AND id=?"
        ))
        .bind(actor.tenant_id)
        .bind(&resource_type)
        .bind(resource_id)
        .fetch_optional(&state.pool)
        .await?;
        let authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grants WHERE tenant_id=? AND subject_type=? AND subject_id=? AND resource_type=? AND resource_id=? AND operation_key IN (?, 'manage'))")
            .bind(actor.tenant_id).bind(&subject_type).bind(subject_id).bind(&resource_type).bind(resource_id).bind(&operation).fetch_one(&state.pool).await?;
        let (name, active) = summary.map_or((None, false), |summary| {
            let name = resource_visible
                .then(|| summary.try_get::<String, _>("name").ok())
                .flatten();
            let active = summary
                .try_get::<String, _>("status")
                .is_ok_and(|status| status == "active");
            (name, active)
        });
        items.push(json!({
            "resourceType":resource_type,"resourceId":resource_id,
            "resourceVersionId":item.try_get::<Option<Uuid>,_>("resource_version_id")?,
            "operation":operation,"name":name,
            "requiredByResourceId":item.try_get::<Option<Uuid>,_>("required_by_resource_id")?,
            "ownerDepartmentId":owner_department_id,"authorized":authorized,"active":active
        }));
    }
    if !full_view && visible_resources.is_empty() {
        return Err(ApiError::forbidden(
            "Resource grant request is outside the actor department scope",
        ));
    }
    let primary_resource_type: String = row.try_get("primary_resource_type")?;
    let primary_resource_id: Uuid = row.try_get("primary_resource_id")?;
    let primary_visible = full_view
        || visible_resources.contains(&(primary_resource_type.clone(), primary_resource_id));
    let primary_resource_name: Option<String> = if primary_visible {
        sqlx::query_scalar(grantable_query!(
            "SELECT name FROM grantable WHERE tenant_id=? AND resource_type=? AND id=?"
        ))
        .bind(actor.tenant_id)
        .bind(&primary_resource_type)
        .bind(primary_resource_id)
        .fetch_optional(&state.pool)
        .await?
    } else {
        None
    };
    let review_rows=sqlx::query("SELECT r.*,d.name department_name,u.display_name reviewer_name FROM resource_grant_request_reviews r JOIN departments d ON d.tenant_id=r.tenant_id AND d.id=r.owner_department_id LEFT JOIN users u ON u.tenant_id=r.tenant_id AND u.id=r.reviewed_by WHERE r.tenant_id=? AND r.request_id=? ORDER BY d.name").bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    let mut reviews = Vec::new();
    for review in review_rows {
        let owner_department_id: Uuid = review.try_get("owner_department_id")?;
        let in_scope =
            full_view || actor_in_department_scope(state, actor, owner_department_id).await?;
        let can_act = in_scope
            && actor.permissions.iter().any(|p| p == "approval:act")
            && actor.permissions.iter().any(|p| p == "resource:grant");
        reviews.push(json!({
            "id":review.try_get::<Uuid,_>("id")?,
            "ownerDepartmentId":owner_department_id,
            "ownerDepartmentName":review.try_get::<String,_>("department_name")?,
            "status":review.try_get::<String,_>("status")?,
            "reviewedBy":review.try_get::<Option<Uuid>,_>("reviewed_by")?,
            "reviewedByName":review.try_get::<Option<String>,_>("reviewer_name")?,
            "reviewComment":review.try_get::<Option<String>,_>("review_comment")?,
            "version":review.try_get::<u64,_>("version")?,
            "canAct":can_act,
            "reviewedAt":review.try_get::<Option<OffsetDateTime>,_>("reviewed_at")?.map(request_audit::timestamp).transpose()?
        }));
    }
    Ok(json!({
        "id":id,
        "subjectType":subject_type,
        "subjectId":subject_id,
        "subjectDepartmentName":row.try_get::<Option<String>,_>("subject_department_name")?,
        "workflowId":row.try_get::<Option<Uuid>,_>("workflow_id")?,
        "workflowName":row.try_get::<Option<String>,_>("workflow_name")?,
        "workflowServiceIdentityId":row.try_get::<Option<Uuid>,_>("workflow_service_identity_id")?,
        "primaryResourceType":primary_resource_type,
        "primaryResourceId":primary_resource_id,
        "primaryResourceName":primary_resource_name,
        "operation":row.try_get::<String,_>("operation_key")?,
        "sourceNodeId":row.try_get::<Option<String>,_>("source_node_id")?,
        "sourceRevision":row.try_get::<Option<u64>,_>("source_revision")?,
        "message":row.try_get::<Option<String>,_>("request_message")?,
        "status":row.try_get::<String,_>("status")?,
        "requestedBy":requester,
        "requestedByName":row.try_get::<String,_>("requester_name")?,
        "version":row.try_get::<u64,_>("version")?,
        "items":items,
        "reviews":reviews,
        "history":request_audit::history(state, actor.tenant_id, id).await?,
        "createdAt":request_audit::timestamp(row.try_get("created_at")?)?,
        "updatedAt":request_audit::timestamp(row.try_get("updated_at")?)?
    }))
}

async fn validate_mcp_dependency(
    state: &ControlApiState,
    tenant: Uuid,
    input: &AuthorizationInput,
) -> ApiResult<()> {
    validate_mcp_dependency_kind(&input.resource_type)?;
    if input.operation != "use" {
        return Err(ApiError::bad_request(
            "MCP_DEPENDENCY_OPERATION_INVALID",
            "MCP configuration dependencies only support the use operation",
        ));
    }
    ensure_resource(state, tenant, &input.resource_type, input.resource_id).await?;
    if input.resource_type == "sandbox_profile" {
        let Some(version_id) = input.resource_version_id else {
            return Err(ApiError::bad_request(
                "MCP_STDIO_SANDBOX_REQUIRED",
                "stdio MCP requires an exact Runtime Sandbox version",
            ));
        };
        let current =
            current_resource_version(state, tenant, &input.resource_type, input.resource_id)
                .await?;
        if current != Some(version_id) {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sandbox_profile_versions WHERE tenant_id=? AND profile_id=? AND id=?)")
                .bind(tenant).bind(input.resource_id).bind(version_id).fetch_one(&state.pool).await?;
            if !exists {
                return Err(ApiError::bad_request(
                    "MCP_STDIO_SANDBOX_VERSION_INVALID",
                    "Runtime Sandbox version does not belong to the selected profile",
                ));
            }
        }
    } else if input.resource_version_id.is_some() {
        return Err(ApiError::bad_request(
            "MCP_CREDENTIAL_VERSION_INVALID",
            "Credential dependencies do not accept a resource version",
        ));
    }
    Ok(())
}

fn validate_mcp_dependency_kind(kind: &str) -> ApiResult<()> {
    if matches!(kind, "credential" | "sandbox_profile") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "MCP_DEPENDENCY_RESOURCE_TYPE_INVALID",
            "MCP configuration dependencies must be Credential or Sandbox Profile resources",
        ))
    }
}

async fn current_resource_version(
    state: &ControlApiState,
    tenant: Uuid,
    kind: &str,
    resource_id: Uuid,
) -> ApiResult<Option<Uuid>> {
    let version = match kind {
        "model" => sqlx::query_scalar(
            "SELECT a.deployment_id FROM model_aliases a WHERE a.tenant_id=? AND a.id=? AND a.status='active'",
        )
        .bind(tenant)
        .bind(resource_id)
        .fetch_optional(&state.pool)
        .await?,
        "mcp_server" => sqlx::query_scalar(
            "SELECT v.id FROM mcp_servers s JOIN mcp_server_versions v ON v.tenant_id=s.tenant_id AND v.server_id=s.id AND v.version_number=s.current_version_number WHERE s.tenant_id=? AND s.id=? AND s.status='active'",
        )
        .bind(tenant)
        .bind(resource_id)
        .fetch_optional(&state.pool)
        .await?,
        "mcp_tool" => sqlx::query_scalar(
            "SELECT v.id FROM mcp_tools t JOIN mcp_tool_versions v ON v.tenant_id=t.tenant_id AND v.tool_id=t.id WHERE t.tenant_id=? AND t.id=? AND t.availability='available' ORDER BY v.version_number DESC LIMIT 1",
        )
        .bind(tenant)
        .bind(resource_id)
        .fetch_optional(&state.pool)
        .await?,
        "skill" => sqlx::query_scalar(
            "SELECT v.id FROM skills s JOIN skill_versions v ON v.tenant_id=s.tenant_id AND v.skill_id=s.id WHERE s.tenant_id=? AND s.id=? AND s.status='active' ORDER BY v.version_number DESC LIMIT 1",
        )
        .bind(tenant)
        .bind(resource_id)
        .fetch_optional(&state.pool)
        .await?,
        "sandbox_profile" => sqlx::query_scalar(
            "SELECT v.id FROM sandbox_profiles p JOIN sandbox_profile_versions v ON v.tenant_id=p.tenant_id AND v.profile_id=p.id AND v.version_number=p.current_version_number WHERE p.tenant_id=? AND p.id=? AND p.status='active'",
        )
        .bind(tenant)
        .bind(resource_id)
        .fetch_optional(&state.pool)
        .await?,
        _ => None,
    };
    Ok(version)
}

#[allow(clippy::too_many_arguments)]
async fn department_resource_authorized(
    state: &ControlApiState,
    tenant: Uuid,
    department: Uuid,
    resource_type: &str,
    resource_id: Uuid,
    resource_version_id: Option<Uuid>,
    owner_department_id: Uuid,
    operation: &str,
) -> ApiResult<bool> {
    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM department_closure WHERE tenant_id=? AND ancestor_id=? AND descendant_id=?)")
        .bind(tenant).bind(department).bind(owner_department_id).fetch_one(&state.pool).await?;
    if owned {
        return Ok(true);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grants WHERE tenant_id=? AND subject_type='department' AND subject_id=? AND resource_type=? AND resource_id=? AND operation_key IN (?,'manage') AND (resource_version_id IS NULL OR resource_version_id=?))")
        .bind(tenant).bind(department).bind(resource_type).bind(resource_id).bind(operation).bind(resource_version_id).fetch_one(&state.pool).await?)
}

pub(crate) async fn ensure_department_resource_authorized(
    state: &ControlApiState,
    tenant: Uuid,
    department: Uuid,
    resource_type: &str,
    resource_id: Uuid,
    resource_version_id: Option<Uuid>,
) -> ApiResult<()> {
    let owner = ensure_resource(state, tenant, resource_type, resource_id).await?;
    if department_resource_authorized(
        state,
        tenant,
        department,
        resource_type,
        resource_id,
        resource_version_id,
        owner,
        "use",
    )
    .await?
    {
        Ok(())
    } else {
        Err(ApiError::unprocessable(
            "MCP_DEPENDENCY_GRANT_REQUIRED",
            "MCP owner department is not authorized to use the selected dependency",
        ))
    }
}

async fn ensure_department_in_actor_scope(
    state: &ControlApiState,
    actor: &Actor,
    department: Uuid,
) -> ApiResult<()> {
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM departments d JOIN department_closure dc ON dc.tenant_id=d.tenant_id AND dc.descendant_id=d.id WHERE d.tenant_id=? AND d.id=? AND d.status='active' AND dc.ancestor_id=?)")
        .bind(actor.tenant_id).bind(department).bind(actor.department_id).fetch_one(&state.pool).await?;
    if !allowed {
        return Err(ApiError::forbidden(
            "MCP owner department is outside the actor scope",
        ));
    }
    Ok(())
}
async fn ensure_department(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<()> {
    let value: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM departments WHERE tenant_id=? AND id=? AND status='active')",
    )
    .bind(tenant)
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    if !value {
        return Err(ApiError::not_found("Department"));
    }
    Ok(())
}
async fn ensure_identity(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<()> {
    let value:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflow_service_identities WHERE tenant_id=? AND id=? AND status='active')").bind(tenant).bind(id).fetch_one(&state.pool).await?;
    if !value {
        return Err(ApiError::not_found("Workflow service identity"));
    }
    Ok(())
}
async fn workflow_identity(
    state: &ControlApiState,
    tenant: Uuid,
    workflow: Uuid,
) -> ApiResult<Uuid> {
    sqlx::query_scalar("SELECT id FROM workflow_service_identities WHERE tenant_id=? AND workflow_id=? AND status='active'").bind(tenant).bind(workflow).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Active Workflow service identity"))
}
fn validate_kind(value: &str) -> ApiResult<()> {
    if !matches!(
        value,
        "credential"
            | "model"
            | "mcp_server"
            | "mcp_tool"
            | "skill"
            | "rag"
            | "memory"
            | "sandbox_profile"
    ) {
        return Err(ApiError::bad_request(
            "INVALID_RESOURCE_TYPE",
            "Resource type is invalid",
        ));
    }
    Ok(())
}
fn validate_operation(value: &str) -> ApiResult<()> {
    if !matches!(value, "view" | "use" | "read" | "write" | "manage") {
        return Err(ApiError::bad_request(
            "INVALID_RESOURCE_OPERATION",
            "Resource operation is invalid",
        ));
    }
    Ok(())
}

fn stable_grant_id(
    tenant_id: Uuid,
    subject_type: &str,
    subject_id: Uuid,
    resource_type: &str,
    resource_id: Uuid,
    operation: &str,
) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(b"agentx-control-resource-grant-v1\0");
    digest.update(tenant_id.as_bytes());
    digest.update(subject_type.as_bytes());
    digest.update([0]);
    digest.update(subject_id.as_bytes());
    digest.update(resource_type.as_bytes());
    digest.update([0]);
    digest.update(resource_id.as_bytes());
    digest.update(operation.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn resource_access_state(
    authorized: bool,
    pending: bool,
    can_grant: bool,
    rejected: bool,
    status: &str,
) -> &'static str {
    if status != "active" {
        "unavailable"
    } else if authorized {
        "authorized"
    } else if pending {
        "pending"
    } else if can_grant {
        "grantable"
    } else if rejected {
        "rejected"
    } else {
        "requestable"
    }
}
fn grant_from_row(row: sqlx::mysql::MySqlRow) -> Result<GrantResponse, sqlx::Error> {
    Ok(GrantResponse {
        id: row.try_get("id")?,
        subject_type: row.try_get("subject_type")?,
        subject_id: row.try_get("subject_id")?,
        resource_type: row.try_get("resource_type")?,
        resource_id: row.try_get("resource_id")?,
        resource_version_id: row.try_get("resource_version_id")?,
        operation: row.try_get("operation_key")?,
        created_at: row.try_get("created_at")?,
    })
}

fn collect_references(value: &Value, output: &mut Vec<(String, String, Uuid, String)>) {
    match value {
        Value::Object(map) => {
            if let (Some(kind), Some(id)) = (
                map.get("resourceType").and_then(Value::as_str),
                map.get("resourceId")
                    .and_then(Value::as_str)
                    .and_then(|v| Uuid::parse_str(v).ok()),
            ) {
                output.push((
                    map.get("nodeId")
                        .and_then(Value::as_str)
                        .unwrap_or("workflow")
                        .to_owned(),
                    kind.to_owned(),
                    id,
                    map.get("operation")
                        .and_then(Value::as_str)
                        .unwrap_or("use")
                        .to_owned(),
                ));
            }
            for child in map.values() {
                collect_references(child, output);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_references(item, output)
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_resources() {
        assert!(validate_kind("legacy").is_err());
        assert!(validate_operation("execute_sql").is_err());
    }
    #[test]
    fn finds_typed_references() {
        let id = Uuid::now_v7();
        let mut values = Vec::new();
        collect_references(
            &json!({"nodes":[{"nodeId":"model","resourceType":"model","resourceId":id,"operation":"use"}]}),
            &mut values,
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].2, id);
    }

    #[test]
    fn grant_identity_is_stable_for_revoke_and_regrant() {
        let tenant_id = Uuid::now_v7();
        let subject_id = Uuid::now_v7();
        let resource_id = Uuid::now_v7();
        let original = stable_grant_id(
            tenant_id,
            "workflow_service_identity",
            subject_id,
            "model",
            resource_id,
            "use",
        );
        assert_eq!(
            original,
            stable_grant_id(
                tenant_id,
                "workflow_service_identity",
                subject_id,
                "model",
                resource_id,
                "use",
            )
        );
        assert_ne!(
            original,
            stable_grant_id(
                tenant_id,
                "workflow_service_identity",
                subject_id,
                "model",
                resource_id,
                "manage",
            )
        );
    }

    #[test]
    fn resource_access_state_exposes_only_authorized_operator_actions() {
        assert_eq!(
            resource_access_state(false, false, true, false, "active"),
            "grantable"
        );
        assert_eq!(
            resource_access_state(false, false, false, false, "active"),
            "requestable"
        );
        assert_eq!(
            resource_access_state(false, true, true, true, "active"),
            "pending"
        );
        assert_eq!(
            resource_access_state(true, false, false, true, "active"),
            "authorized"
        );
        assert_eq!(
            resource_access_state(true, false, true, true, "disabled"),
            "unavailable"
        );
        assert_eq!(
            resource_access_state(false, false, false, true, "active"),
            "rejected"
        );
    }
}
