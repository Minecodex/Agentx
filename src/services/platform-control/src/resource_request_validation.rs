//! Revalidate the approved resource package while serializing request decisions.

use super::*;

pub(super) async fn dependency_hash(
    state: &ControlApiState,
    tenant: Uuid,
    requirements: &[Requirement],
) -> ApiResult<Option<String>> {
    let mut snapshots = Vec::new();
    for item in requirements {
        let row = sqlx::query(grantable_query!(
            "SELECT status,owner_department_id,updated_at FROM grantable WHERE tenant_id=? AND resource_type=? AND id=?"
        ))
        .bind(tenant)
        .bind(&item.resource_type)
        .bind(item.resource_id)
        .fetch_optional(&state.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let owner: Uuid = row.try_get("owner_department_id")?;
        if row.try_get::<String, _>("status")? != "active" || owner != item.owner_department_id {
            return Ok(None);
        }
        // Knowledge document changes do not revise resource configuration.
        let revision = match item.resource_type.as_str() {
            "rag" | "memory" => {
                let table = if item.resource_type == "rag" {
                    "rag_resources"
                } else {
                    "memory_namespaces"
                };
                json!(
                    sqlx::query_scalar::<_, u64>(&format!(
                        "SELECT version FROM {table} WHERE tenant_id=? AND id=?"
                    ))
                    .bind(tenant)
                    .bind(item.resource_id)
                    .fetch_one(&state.pool)
                    .await?
                )
            }
            _ => json!(row.try_get::<OffsetDateTime, _>("updated_at")?),
        };
        snapshots.push(json!({
            "resourceType":item.resource_type,"resourceId":item.resource_id,
            "resourceVersionId":item.resource_version_id,"operation":item.operation,
            "requiredByResourceId":item.required_by,"ownerDepartmentId":owner,
            "currentVersion":current_resource_version(state,tenant,&item.resource_type,item.resource_id).await?,
            "revision":revision
        }));
    }
    let mut encoded = snapshots
        .into_iter()
        .map(|value| serde_json::to_string(&value))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::internal)?;
    encoded.sort();
    Ok(Some(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&encoded).map_err(ApiError::internal)?)
    )))
}

pub(super) async fn lock_pending(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<sqlx::mysql::MySqlRow> {
    let row =
        sqlx::query("SELECT * FROM resource_grant_requests WHERE tenant_id=? AND id=? FOR UPDATE")
            .bind(tenant)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| ApiError::not_found("Resource grant request"))?;
    if row.try_get::<String, _>("status")? != "pending" {
        return Err(ApiError::conflict(
            "RESOURCE_GRANT_REQUEST_STATE_CONFLICT",
            "Request is no longer pending",
        ));
    }
    Ok(row)
}

pub(super) async fn is_current(
    state: &ControlApiState,
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    request: &sqlx::mysql::MySqlRow,
) -> ApiResult<bool> {
    let requester: Uuid = request.try_get("requested_by")?;
    let permission = if request.try_get::<String, _>("subject_type")? == "department" {
        "mcp:manage"
    } else {
        "workflow:edit"
    };
    let authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u JOIN user_roles ur ON ur.tenant_id=u.tenant_id AND ur.user_id=u.id JOIN roles r ON r.tenant_id=ur.tenant_id AND r.id=ur.role_id AND r.status='active' JOIN role_permissions rp ON rp.tenant_id=r.tenant_id AND rp.role_id=r.id JOIN permissions p ON p.id=rp.permission_id AND p.permission_key=? WHERE u.tenant_id=? AND u.id=? AND u.status='active')")
        .bind(permission).bind(tenant).bind(requester).fetch_one(&mut **tx).await?;
    if !authorized {
        return Ok(false);
    }
    if let Some(workflow) = request.try_get::<Option<Uuid>, _>("workflow_id")? {
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workflows w JOIN workflow_service_identities si ON si.tenant_id=w.tenant_id AND si.workflow_id=w.id JOIN workflow_drafts d ON d.tenant_id=w.tenant_id AND d.workflow_id=w.id WHERE w.tenant_id=? AND w.id=? AND w.status='active' AND si.status='active' AND (? IS NULL OR d.revision=?))")
            .bind(tenant).bind(workflow).bind(request.try_get::<Option<u64>,_>("source_revision")?)
            .bind(request.try_get::<Option<u64>,_>("source_revision")?).fetch_one(&mut **tx).await?;
        if !valid {
            return Ok(false);
        }
    }
    let id: Uuid = request.try_get("id")?;
    let rows = sqlx::query("SELECT * FROM resource_grant_request_items WHERE tenant_id=? AND request_id=? ORDER BY resource_type,resource_id")
        .bind(tenant).bind(id).fetch_all(&mut **tx).await?;
    let mut stored = Vec::new();
    for row in rows {
        let item = Requirement {
            resource_type: row.try_get("resource_type")?,
            resource_id: row.try_get("resource_id")?,
            resource_version_id: row.try_get("resource_version_id")?,
            operation: row.try_get("operation_key")?,
            required_by: row.try_get("required_by_resource_id")?,
            owner_department_id: row.try_get("owner_department_id")?,
        };
        let table = match item.resource_type.as_str() {
            "credential" => "credentials",
            "model" => "model_aliases",
            "mcp_server" => "mcp_servers",
            "mcp_tool" => "mcp_tools",
            "skill" => "skills",
            "rag" => "rag_resources",
            "memory" => "memory_namespaces",
            "sandbox_profile" => "sandbox_profiles",
            _ => return Ok(false),
        };
        let exists = sqlx::query(&format!(
            "SELECT id FROM {table} WHERE tenant_id=? AND id=? FOR SHARE"
        ))
        .bind(tenant)
        .bind(item.resource_id)
        .fetch_optional(&mut **tx)
        .await?;
        if exists.is_none() {
            return Ok(false);
        }
        stored.push(item);
    }
    let expected: String = request.try_get("dependency_hash")?;
    if dependency_hash(state, tenant, &stored).await?.as_ref() != Some(&expected) {
        return Ok(false);
    }
    let current = expand_requirements(
        state,
        tenant,
        &AuthorizationInput {
            resource_type: request.try_get("primary_resource_type")?,
            resource_id: request.try_get("primary_resource_id")?,
            resource_version_id: request.try_get("primary_resource_version_id")?,
            operation: request.try_get("operation_key")?,
        },
    )
    .await?;
    Ok(dependency_hash(state, tenant, &current).await?.as_ref() == Some(&expected))
}
