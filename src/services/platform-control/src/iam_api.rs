use std::collections::{HashMap, HashSet};

use agentx_api_types::{PageRequest, PageResponse};
use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/departments",
            get(list_departments).post(create_department),
        )
        .route("/api/v1/departments/search", get(search_departments))
        .route(
            "/api/v1/departments/{id}",
            patch(update_department).delete(delete_department),
        )
        .route("/api/v1/users", get(list_users).post(create_user))
        .route("/api/v1/users/{id}", patch(update_user))
        .route("/api/v1/users/{id}/disable", post(disable_user))
        .route("/api/v1/roles", get(list_roles).post(create_role))
        .route("/api/v1/roles/{id}", patch(update_role).delete(delete_role))
        .route("/api/v1/permissions", get(list_permissions))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DepartmentResponse {
    id: Uuid,
    parent_id: Option<Uuid>,
    name: String,
    is_root: bool,
    status: String,
    version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateDepartmentRequest {
    parent_id: Uuid,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateDepartmentRequest {
    name: String,
    parent_id: Option<Uuid>,
    version: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UserResponse {
    id: Uuid,
    username: String,
    display_name: String,
    status: String,
    password_change_required: bool,
    department_id: Uuid,
    department_name: String,
    roles: Vec<String>,
    version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateUserRequest {
    username: String,
    display_name: String,
    department_id: Uuid,
    role_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateUserRequest {
    display_name: String,
    department_id: Uuid,
    role_id: Uuid,
    version: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RoleResponse {
    id: Uuid,
    code: String,
    name: String,
    description: Option<String>,
    data_scope: String,
    is_builtin: bool,
    status: String,
    permissions: Vec<String>,
    member_count: u64,
    version: u64,
}

#[derive(Serialize)]
struct PermissionResponse {
    key: String,
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRoleRequest {
    code: String,
    name: String,
    description: Option<String>,
    data_scope: String,
    permissions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRoleRequest {
    name: String,
    description: Option<String>,
    data_scope: String,
    permissions: Vec<String>,
    version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    search: Option<String>,
    status: Option<String>,
    department_id: Option<Uuid>,
}

async fn list_departments(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<Json<Vec<DepartmentResponse>>> {
    actor.require("department:view")?;
    let rows = sqlx::query("SELECT id,parent_id,name,is_root,status,version FROM departments WHERE tenant_id=? ORDER BY is_root DESC,name")
        .bind(actor.tenant_id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(department_from_row)
            .collect::<Result<_, _>>()?,
    ))
}

async fn search_departments(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<DepartmentResponse>>> {
    actor.require("department:view")?;
    let search = query.search.unwrap_or_default().trim().to_lowercase();
    let status = query.status.unwrap_or_default();
    let rows = sqlx::query("SELECT id,parent_id,name,is_root,status,version FROM departments WHERE tenant_id=? AND (?='' OR status=?) ORDER BY is_root DESC,name")
        .bind(actor.tenant_id)
        .bind(&status)
        .bind(&status)
        .fetch_all(&state.pool)
        .await?;
    let items = rows
        .into_iter()
        .map(department_from_row)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|item| search.is_empty() || item.name.to_lowercase().contains(&search))
        .collect();
    Ok(Json(page(items, query.page, query.page_size)))
}

async fn create_department(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateDepartmentRequest>,
) -> ApiResult<(StatusCode, Json<DepartmentResponse>)> {
    actor.require("department:manage")?;
    validate_name(&input.name)?;
    require_department(&state, actor.tenant_id, input.parent_id).await?;
    let id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO departments(id,tenant_id,parent_id,name,normalized_name) VALUES(?,?,?,?,?)",
    )
    .bind(id)
    .bind(actor.tenant_id)
    .bind(input.parent_id)
    .bind(input.name.trim())
    .bind(normalize(&input.name))
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        map_unique(
            error,
            "name",
            "DEPARTMENT_NAME_EXISTS",
            "A department with this name already exists under the selected parent",
        )
    })?;
    sqlx::query("INSERT INTO department_closure(tenant_id,ancestor_id,descendant_id,depth) SELECT tenant_id,ancestor_id,?,depth+1 FROM department_closure WHERE tenant_id=? AND descendant_id=? UNION ALL SELECT ?,?,?,0")
        .bind(id).bind(actor.tenant_id).bind(input.parent_id).bind(actor.tenant_id).bind(id).bind(id).execute(&mut *tx).await?;
    audit(
        &mut tx,
        &actor,
        "department.create",
        "department",
        id,
        json!({"name":input.name}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(DepartmentResponse {
            id,
            parent_id: Some(input.parent_id),
            name: input.name.trim().into(),
            is_root: false,
            status: "active".into(),
            version: 1,
        }),
    ))
}

async fn update_department(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateDepartmentRequest>,
) -> ApiResult<Json<DepartmentResponse>> {
    actor.require("department:manage")?;
    validate_name(&input.name)?;
    let current = sqlx::query("SELECT is_root FROM departments WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Department"))?;
    if current.try_get::<bool, _>("is_root")? && input.parent_id.is_some() {
        return Err(ApiError::bad_request(
            "ROOT_DEPARTMENT_IMMUTABLE",
            "Root department cannot be moved",
        ));
    }
    if let Some(parent) = input.parent_id {
        require_department(&state, actor.tenant_id, parent).await?;
        let cycle: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM department_closure WHERE tenant_id=? AND ancestor_id=? AND descendant_id=?)")
            .bind(actor.tenant_id).bind(id).bind(parent).fetch_one(&state.pool).await?;
        if parent == id || cycle {
            return Err(ApiError::bad_request(
                "DEPARTMENT_CYCLE",
                "Department cannot move below itself",
            ));
        }
    }
    let mut tx = state.pool.begin().await?;
    let result = sqlx::query("UPDATE departments SET name=?,normalized_name=?,parent_id=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?")
        .bind(input.name.trim()).bind(normalize(&input.name)).bind(input.parent_id).bind(actor.tenant_id).bind(id).bind(input.version)
        .execute(&mut *tx).await.map_err(|error| map_unique(error, "name", "DEPARTMENT_NAME_EXISTS", "A department with this name already exists under the selected parent"))?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "VERSION_CONFLICT",
            "Department was modified by another request",
        ));
    }
    rebuild_closure(&mut tx, actor.tenant_id).await?;
    let affected_users = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM user_departments WHERE tenant_id=? AND department_id=? UNION SELECT user_id FROM user_roles WHERE tenant_id=? AND scope_department_id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    for user_id in affected_users {
        sqlx::query("UPDATE users SET token_version=token_version+1 WHERE tenant_id=? AND id=?")
            .bind(actor.tenant_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        emit_user_admission(&mut tx, actor.tenant_id, user_id, true).await?;
    }
    audit(
        &mut tx,
        &actor,
        "department.update",
        "department",
        id,
        json!({"name":input.name,"parentId":input.parent_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(department_from_row(
        sqlx::query("SELECT id,parent_id,name,is_root,status,version FROM departments WHERE id=?")
            .bind(id)
            .fetch_one(&state.pool)
            .await?,
    )?))
}

async fn delete_department(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("department:delete")?;
    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM departments d WHERE d.tenant_id=? AND d.id=? AND (d.is_root=TRUE OR EXISTS(SELECT 1 FROM departments c WHERE c.parent_id=d.id AND c.status='active') OR EXISTS(SELECT 1 FROM user_departments ud WHERE ud.department_id=d.id)))")
        .bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if blocked {
        return Err(ApiError::conflict(
            "DELETE_REFERENCED",
            "Department is still referenced",
        ));
    }
    sqlx::query(
        "UPDATE departments SET status='disabled',version=version+1 WHERE tenant_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_users(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<UserResponse>>> {
    actor.require("user:view")?;
    let rows = sqlx::query("SELECT u.id,u.username,u.display_name,u.status,u.password_change_required,u.version,ud.department_id,d.name department_name FROM users u JOIN user_departments ud ON ud.user_id=u.id JOIN departments d ON d.id=ud.department_id WHERE u.tenant_id=? ORDER BY u.created_at DESC")
        .bind(actor.tenant_id).fetch_all(&state.pool).await?;
    let search = query.search.unwrap_or_default().to_lowercase();
    let mut items = Vec::new();
    for row in rows {
        let status: String = row.try_get("status")?;
        let department_id: Uuid = row.try_get("department_id")?;
        let username: String = row.try_get("username")?;
        let display_name: String = row.try_get("display_name")?;
        if query
            .status
            .as_ref()
            .is_some_and(|v| v != "all" && v != &status)
            || query.department_id.is_some_and(|v| v != department_id)
            || (!search.is_empty()
                && !username.to_lowercase().contains(&search)
                && !display_name.to_lowercase().contains(&search))
        {
            continue;
        }
        let id = row.try_get("id")?;
        items.push(UserResponse {
            id,
            username,
            display_name,
            status,
            password_change_required: row.try_get("password_change_required")?,
            department_id,
            department_name: row.try_get("department_name")?,
            roles: user_roles(&state, id).await?,
            version: row.try_get("version")?,
        });
    }
    Ok(Json(page(items, query.page, query.page_size)))
}

async fn create_user(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateUserRequest>,
) -> ApiResult<(StatusCode, Json<UserResponse>)> {
    actor.require("user:create")?;
    validate_username(&input.username)?;
    validate_name(&input.display_name)?;
    let department_name = require_department(&state, actor.tenant_id, input.department_id).await?;
    let role = load_role(&state, actor.tenant_id, input.role_id).await?;
    let id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    let password = Argon2::default()
        .hash_password(b"123456", &SaltString::generate(&mut OsRng))
        .map_err(ApiError::internal)?
        .to_string();
    sqlx::query("INSERT INTO users(id,tenant_id,username,username_normalized,display_name,status,password_change_required) VALUES(?,?,?,?,?,'active',TRUE)")
        .bind(id).bind(actor.tenant_id).bind(input.username.trim()).bind(normalize(&input.username)).bind(input.display_name.trim()).execute(&mut *tx).await.map_err(|error| map_unique(error, "username", "USERNAME_EXISTS", "This username is already in use"))?;
    sqlx::query("INSERT INTO user_credentials(user_id,password_hash) VALUES(?,?)")
        .bind(id)
        .bind(password)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO user_departments(tenant_id,user_id,department_id) VALUES(?,?,?)")
        .bind(actor.tenant_id)
        .bind(id)
        .bind(input.department_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO user_roles(id,tenant_id,user_id,role_id,scope_department_id) VALUES(?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(input.role_id).bind((role=="department_admin").then_some(input.department_id)).execute(&mut *tx).await?;
    emit_user_admission(&mut tx, actor.tenant_id, id, true).await?;
    audit(
        &mut tx,
        &actor,
        "user.create",
        "user",
        id,
        json!({"role":role}),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(UserResponse {
            id,
            username: input.username.trim().into(),
            display_name: input.display_name.trim().into(),
            status: "active".into(),
            password_change_required: true,
            department_id: input.department_id,
            department_name,
            roles: vec![role],
            version: 1,
        }),
    ))
}

async fn update_user(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateUserRequest>,
) -> ApiResult<Json<UserResponse>> {
    actor.require("user:update")?;
    validate_name(&input.display_name)?;
    let department_name = require_department(&state, actor.tenant_id, input.department_id).await?;
    let role = load_role(&state, actor.tenant_id, input.role_id).await?;
    let mut tx = state.pool.begin().await?;
    let result=sqlx::query("UPDATE users SET display_name=?,token_version=token_version+1,version=version+1 WHERE tenant_id=? AND id=? AND version=?")
        .bind(input.display_name.trim()).bind(actor.tenant_id).bind(id).bind(input.version).execute(&mut *tx).await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "VERSION_CONFLICT",
            "User was modified by another request",
        ));
    }
    sqlx::query("UPDATE user_departments SET department_id=? WHERE tenant_id=? AND user_id=?")
        .bind(input.department_id)
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM user_roles WHERE tenant_id=? AND user_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO user_roles(id,tenant_id,user_id,role_id,scope_department_id) VALUES(?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(input.role_id).bind((role=="department_admin").then_some(input.department_id)).execute(&mut *tx).await?;
    sqlx::query("UPDATE refresh_sessions SET revoked_at=COALESCE(revoked_at,UTC_TIMESTAMP(6)) WHERE user_id=?").bind(id).execute(&mut *tx).await?;
    emit_user_admission(&mut tx, actor.tenant_id, id, true).await?;
    tx.commit().await?;
    Ok(Json(
        load_user(&state, actor.tenant_id, id, department_name, vec![role]).await?,
    ))
}

async fn disable_user(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("user:disable")?;
    if id == actor.user_id {
        return Err(ApiError::bad_request(
            "SELF_DISABLE_DENIED",
            "You cannot disable your own account",
        ));
    }
    let mut tx = state.pool.begin().await?;
    let result=sqlx::query("UPDATE users SET status='disabled',token_version=token_version+1,version=version+1 WHERE tenant_id=? AND id=? AND status='active'")
        .bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::not_found("User"));
    }
    sqlx::query("UPDATE refresh_sessions SET revoked_at=COALESCE(revoked_at,UTC_TIMESTAMP(6)) WHERE user_id=?").bind(id).execute(&mut *tx).await?;
    emit_user_admission(&mut tx, actor.tenant_id, id, false).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_roles(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<RoleResponse>>> {
    actor.require("role:view")?;
    let search = query.search.unwrap_or_default().to_lowercase();
    let rows=sqlx::query("SELECT r.id,r.code,r.name,r.description,r.data_scope,r.is_builtin,r.status,r.version,COUNT(DISTINCT ur.user_id) member_count FROM roles r LEFT JOIN user_roles ur ON ur.role_id=r.id WHERE r.tenant_id=? GROUP BY r.id ORDER BY r.is_builtin DESC,r.name")
        .bind(actor.tenant_id).fetch_all(&state.pool).await?;
    let mut items = Vec::new();
    for row in rows {
        let name: String = row.try_get("name")?;
        let code: String = row.try_get("code")?;
        if !search.is_empty()
            && !name.to_lowercase().contains(&search)
            && !code.to_lowercase().contains(&search)
        {
            continue;
        }
        let id = row.try_get("id")?;
        items.push(RoleResponse {
            id,
            code,
            name,
            description: row.try_get("description")?,
            data_scope: row.try_get("data_scope")?,
            is_builtin: row.try_get("is_builtin")?,
            status: row.try_get("status")?,
            permissions: role_permissions(&state, id).await?,
            member_count: row.try_get::<i64, _>("member_count")? as u64,
            version: row.try_get("version")?,
        });
    }
    Ok(Json(page(items, query.page, query.page_size)))
}

async fn create_role(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateRoleRequest>,
) -> ApiResult<(StatusCode, Json<RoleResponse>)> {
    actor.require("role:manage")?;
    validate_role(&input.name, &input.data_scope, &input.permissions, &actor)?;
    let id = Uuid::now_v7();
    let code = normalize_code(&input.code);
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO roles(id,tenant_id,code,name,description,data_scope) VALUES(?,?,?,?,?,?)",
    )
    .bind(id)
    .bind(actor.tenant_id)
    .bind(&code)
    .bind(input.name.trim())
    .bind(&input.description)
    .bind(&input.data_scope)
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        map_unique(
            error,
            "code",
            "ROLE_CODE_EXISTS",
            "A role with this code already exists",
        )
    })?;
    replace_permissions(&mut tx, actor.tenant_id, id, &input.permissions).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(RoleResponse {
            id,
            code,
            name: input.name.trim().into(),
            description: input.description,
            data_scope: input.data_scope,
            is_builtin: false,
            status: "active".into(),
            permissions: input.permissions,
            member_count: 0,
            version: 1,
        }),
    ))
}

async fn update_role(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateRoleRequest>,
) -> ApiResult<Json<RoleResponse>> {
    actor.require("role:manage")?;
    validate_role(&input.name, &input.data_scope, &input.permissions, &actor)?;
    let mut tx = state.pool.begin().await?;
    let result=sqlx::query("UPDATE roles SET name=?,description=?,data_scope=?,version=version+1 WHERE tenant_id=? AND id=? AND version=? AND is_builtin=FALSE")
        .bind(input.name.trim()).bind(&input.description).bind(&input.data_scope).bind(actor.tenant_id).bind(id).bind(input.version).execute(&mut *tx).await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "VERSION_CONFLICT",
            "Role is built-in or was modified",
        ));
    }
    replace_permissions(&mut tx, actor.tenant_id, id, &input.permissions).await?;
    let member_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM user_roles WHERE tenant_id=? AND role_id=? ORDER BY user_id FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    for user_id in member_ids {
        sqlx::query("UPDATE users SET token_version=token_version+1 WHERE tenant_id=? AND id=?")
            .bind(actor.tenant_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        emit_user_admission(&mut tx, actor.tenant_id, user_id, true).await?;
    }
    tx.commit().await?;
    let row = sqlx::query("SELECT code,status FROM roles WHERE id=?")
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(RoleResponse {
        id,
        code: row.try_get("code")?,
        name: input.name,
        description: input.description,
        data_scope: input.data_scope,
        is_builtin: false,
        status: row.try_get("status")?,
        permissions: input.permissions,
        member_count: sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM user_roles WHERE role_id=?",
        )
        .bind(id)
        .fetch_one(&state.pool)
        .await? as u64,
        version: input.version + 1,
    }))
}

async fn delete_role(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("role:delete")?;
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_roles WHERE tenant_id=? AND role_id=?)",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    if used {
        return Err(ApiError::conflict(
            "DELETE_REFERENCED",
            "Role is assigned to users",
        ));
    }
    let result = sqlx::query("DELETE FROM roles WHERE tenant_id=? AND id=? AND is_builtin=FALSE")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::not_found("Role"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_permissions(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<Json<Vec<PermissionResponse>>> {
    actor.require("role:view")?;
    let rows = sqlx::query(
        "SELECT permission_key,name,description FROM permissions ORDER BY permission_key",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                Ok(PermissionResponse {
                    key: r.try_get("permission_key")?,
                    name: r.try_get("name")?,
                    description: r.try_get("description")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?,
    ))
}

fn page<T>(items: Vec<T>, page: Option<u32>, page_size: Option<u32>) -> PageResponse<T> {
    let spec = PageRequest { page, page_size }.normalized();
    let total = items.len() as u64;
    let start = (spec.offset() as usize).min(items.len());
    let selected = items
        .into_iter()
        .skip(start)
        .take(spec.page_size as usize)
        .collect();
    PageResponse {
        items: selected,
        page: spec.page,
        page_size: spec.page_size,
        total,
    }
}
async fn require_department(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<String> {
    sqlx::query_scalar(
        "SELECT name FROM departments WHERE tenant_id=? AND id=? AND status='active'",
    )
    .bind(tenant)
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Department"))
}
async fn load_role(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<String> {
    sqlx::query_scalar("SELECT code FROM roles WHERE tenant_id=? AND id=? AND status='active'")
        .bind(tenant)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Role"))
}
async fn user_roles(state: &ControlApiState, id: Uuid) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT r.code FROM user_roles ur JOIN roles r ON r.id=ur.role_id WHERE ur.user_id=? ORDER BY r.name").bind(id).fetch_all(&state.pool).await?)
}
async fn role_permissions(state: &ControlApiState, id: Uuid) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT p.permission_key FROM role_permissions rp JOIN permissions p ON p.id=rp.permission_id WHERE rp.role_id=? ORDER BY p.permission_key").bind(id).fetch_all(&state.pool).await?)
}
async fn load_user(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
    department_name: String,
    roles: Vec<String>,
) -> ApiResult<UserResponse> {
    let row=sqlx::query("SELECT u.username,u.display_name,u.status,u.password_change_required,u.version,ud.department_id FROM users u JOIN user_departments ud ON ud.user_id=u.id WHERE u.tenant_id=? AND u.id=?").bind(tenant).bind(id).fetch_one(&state.pool).await?;
    Ok(UserResponse {
        id,
        username: row.try_get("username")?,
        display_name: row.try_get("display_name")?,
        status: row.try_get("status")?,
        password_change_required: row.try_get("password_change_required")?,
        department_id: row.try_get("department_id")?,
        department_name,
        roles,
        version: row.try_get("version")?,
    })
}
fn department_from_row(row: sqlx::mysql::MySqlRow) -> Result<DepartmentResponse, sqlx::Error> {
    Ok(DepartmentResponse {
        id: row.try_get("id")?,
        parent_id: row.try_get("parent_id")?,
        name: row.try_get("name")?,
        is_root: row.try_get("is_root")?,
        status: row.try_get("status")?,
        version: row.try_get("version")?,
    })
}

async fn rebuild_closure(tx: &mut Transaction<'_, MySql>, tenant: Uuid) -> ApiResult<()> {
    let rows = sqlx::query("SELECT id,parent_id FROM departments WHERE tenant_id=?")
        .bind(tenant)
        .fetch_all(&mut **tx)
        .await?;
    let mut parents = HashMap::new();
    for row in rows {
        parents.insert(
            row.try_get::<Uuid, _>("id")?,
            row.try_get::<Option<Uuid>, _>("parent_id")?,
        );
    }
    sqlx::query("DELETE FROM department_closure WHERE tenant_id=?")
        .bind(tenant)
        .execute(&mut **tx)
        .await?;
    for id in parents.keys() {
        let mut current = Some(*id);
        let mut depth = 0;
        let mut seen = HashSet::new();
        while let Some(ancestor) = current {
            if !seen.insert(ancestor) {
                return Err(ApiError::bad_request(
                    "DEPARTMENT_CYCLE",
                    "Department hierarchy contains a cycle",
                ));
            }
            sqlx::query("INSERT INTO department_closure(tenant_id,ancestor_id,descendant_id,depth) VALUES(?,?,?,?)").bind(tenant).bind(ancestor).bind(*id).bind(depth).execute(&mut **tx).await?;
            current = parents.get(&ancestor).copied().flatten();
            depth += 1;
        }
    }
    Ok(())
}
async fn replace_permissions(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    role: Uuid,
    permissions: &[String],
) -> ApiResult<()> {
    sqlx::query("DELETE FROM role_permissions WHERE tenant_id=? AND role_id=?")
        .bind(tenant)
        .bind(role)
        .execute(&mut **tx)
        .await?;
    for key in permissions {
        let result=sqlx::query("INSERT INTO role_permissions(tenant_id,role_id,permission_id) SELECT ?,?,id FROM permissions WHERE permission_key=?").bind(tenant).bind(role).bind(key).execute(&mut **tx).await?;
        if result.rows_affected() != 1 {
            return Err(ApiError::bad_request(
                "INVALID_PERMISSION",
                format!("Unknown permission {key}"),
            ));
        }
    }
    Ok(())
}
pub(crate) async fn emit_user_admission(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    user: Uuid,
    enabled: bool,
) -> ApiResult<()> {
    let token_version: u64 =
        sqlx::query_scalar("SELECT token_version FROM users WHERE tenant_id=? AND id=?")
            .bind(tenant)
            .bind(user)
            .fetch_one(&mut **tx)
            .await?;
    let tenant_query_enabled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_roles ur JOIN roles r ON r.tenant_id=ur.tenant_id AND r.id=ur.role_id AND r.status='active' JOIN role_permissions rp ON rp.tenant_id=r.tenant_id AND rp.role_id=r.id JOIN permissions p ON p.id=rp.permission_id AND p.permission_key='execution:view' WHERE ur.tenant_id=? AND ur.user_id=? AND r.data_scope='company')")
        .bind(tenant)
        .bind(user)
        .fetch_one(&mut **tx)
        .await?;
    let payload = json!({"userId":user,"tokenVersion":token_version,"enabled":enabled,"tenantQueryEnabled":tenant_query_enabled,"admissionEpoch":token_version});
    let hash = agentx_runtime_contracts::content_hash(&payload)
        .map_err(ApiError::internal)?
        .to_string();
    sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,?,?,?,?,'pending',?,?)")
        .bind(Uuid::now_v7()).bind(tenant).bind("RuntimeUserAdmissionChanged").bind("runtime_user_admission").bind(user.to_string()).bind(payload).bind(hash).bind(format!("user-admission:{user}:{token_version}")).execute(&mut **tx).await?;
    crate::runtime_grants::emit_user_application_grants(tx, tenant, user).await?;
    Ok(())
}
async fn audit(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    action: &str,
    target_type: &str,
    target: Uuid,
    detail: serde_json::Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO audit_events(id,tenant_id,actor_user_id,action,target_type,target_id,request_id,detail_json) VALUES(?,?,?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(actor.user_id).bind(action).bind(target_type).bind(target.to_string()).bind(Uuid::now_v7()).bind(detail).execute(&mut **tx).await?;
    Ok(())
}
fn validate_name(v: &str) -> ApiResult<()> {
    if v.trim().is_empty() || v.trim().len() > 100 {
        Err(ApiError::bad_request(
            "INVALID_NAME",
            "Name is required and must not exceed 100 characters",
        ))
    } else {
        Ok(())
    }
}
fn validate_username(v: &str) -> ApiResult<()> {
    let v = v.trim();
    if !(3..=64).contains(&v.len())
        || !v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        Err(ApiError::bad_request(
            "INVALID_USERNAME",
            "Username format is invalid",
        ))
    } else {
        Ok(())
    }
}
fn validate_role(name: &str, scope: &str, permissions: &[String], actor: &Actor) -> ApiResult<()> {
    validate_name(name)?;
    if !matches!(scope, "company" | "department_tree" | "own") {
        return Err(ApiError::bad_request(
            "INVALID_DATA_SCOPE",
            "Data scope is invalid",
        ));
    }
    if permissions.iter().any(|p| !actor.permissions.contains(p)) {
        return Err(ApiError::forbidden(
            "Role contains permissions you do not hold",
        ));
    }
    Ok(())
}
fn normalize(v: &str) -> String {
    v.trim().to_lowercase()
}
fn normalize_code(v: &str) -> String {
    v.trim().to_ascii_lowercase().replace(' ', "_")
}
fn map_unique(
    error: sqlx::Error,
    field: &'static str,
    code: &'static str,
    message: &'static str,
) -> ApiError {
    if let sqlx::Error::Database(db) = &error {
        if db.is_unique_violation() {
            return ApiError::conflict(code, message).with_field_error(field, code, message);
        }
    }
    ApiError::from(error)
}
