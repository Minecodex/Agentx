//! Expand and validate the complete resource dependency package.

use std::collections::BTreeMap;

use super::*;

pub(super) async fn expand_requirements(
    state: &ControlApiState,
    tenant: Uuid,
    input: &AuthorizationInput,
) -> ApiResult<Vec<Requirement>> {
    let mut pending = vec![(input.clone(), None)];
    let mut seen = BTreeMap::new();
    let mut result = Vec::new();
    while let Some((mut input, parent)) = pending.pop() {
        if input.resource_version_id.is_none() {
            input.resource_version_id =
                current_resource_version(state, tenant, &input.resource_type, input.resource_id)
                    .await?;
        }
        let key = (
            input.resource_type.clone(),
            input.resource_id,
            input.operation.clone(),
        );
        if let Some(version) = seen.get(&key) {
            if *version != input.resource_version_id {
                return Err(ApiError::unprocessable(
                    "RESOURCE_DEPENDENCY_VERSION_CONFLICT",
                    "The dependency package selects conflicting versions of one resource",
                ));
            }
            continue;
        }
        validate_version(state, tenant, &input).await?;
        seen.insert(key, input.resource_version_id);
        let mut direct = direct_requirements(state, tenant, &input).await?;
        let mut primary = direct.remove(0);
        primary.required_by = parent;
        result.push(primary);
        for child in direct.into_iter().rev() {
            pending.push((
                AuthorizationInput {
                    resource_type: child.resource_type,
                    resource_id: child.resource_id,
                    resource_version_id: child.resource_version_id,
                    operation: child.operation,
                },
                child.required_by,
            ));
        }
    }
    Ok(result)
}

async fn validate_version(
    state: &ControlApiState,
    tenant: Uuid,
    input: &AuthorizationInput,
) -> ApiResult<()> {
    let Some(version) = input.resource_version_id else {
        return Ok(());
    };
    let relation = match input.resource_type.as_str() {
        "skill" => Some(("skill_versions", "skill_id")),
        "mcp_server" => Some(("mcp_server_versions", "server_id")),
        "mcp_tool" => Some(("mcp_tool_versions", "tool_id")),
        "sandbox_profile" => Some(("sandbox_profile_versions", "profile_id")),
        _ => None,
    };
    let valid = if let Some((table, column)) = relation {
        sqlx::query_scalar::<_, bool>(&format!(
            "SELECT EXISTS(SELECT 1 FROM {table} WHERE tenant_id=? AND {column}=? AND id=?)"
        ))
        .bind(tenant)
        .bind(input.resource_id)
        .bind(version)
        .fetch_one(&state.pool)
        .await?
    } else {
        current_resource_version(state, tenant, &input.resource_type, input.resource_id).await?
            == Some(version)
    };
    if !valid {
        return Err(ApiError::bad_request(
            "RESOURCE_VERSION_INVALID",
            "Resource version does not belong to the selected resource",
        ));
    }
    Ok(())
}

pub(crate) async fn validate_skill_dependency(
    state: &ControlApiState,
    tenant: Uuid,
    department: Uuid,
    input: &AuthorizationInput,
) -> ApiResult<()> {
    for item in expand_requirements(state, tenant, input).await? {
        if !department_resource_authorized(
            state,
            tenant,
            department,
            &item.resource_type,
            item.resource_id,
            item.resource_version_id,
            item.owner_department_id,
            &item.operation,
        )
        .await?
        {
            return Err(ApiError::forbidden(
                "Skill owner department is not authorized to use the complete dependency package",
            ));
        }
    }
    Ok(())
}

async fn direct_requirements(
    state: &ControlApiState,
    tenant: Uuid,
    input: &AuthorizationInput,
) -> ApiResult<Vec<Requirement>> {
    validate_operation(&input.operation)?;
    let owner = ensure_resource(state, tenant, &input.resource_type, input.resource_id).await?;
    let mut values = vec![Requirement {
        resource_type: input.resource_type.clone(),
        resource_id: input.resource_id,
        resource_version_id: input.resource_version_id,
        operation: input.operation.clone(),
        required_by: None,
        owner_department_id: owner,
    }];
    match input.resource_type.as_str() {
        "model" => {
            let credential: Option<Uuid> = sqlx::query_scalar("SELECT d.credential_id FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND a.id=?")
                .bind(tenant).bind(input.resource_id).fetch_optional(&state.pool).await?.flatten();
            if let Some(credential) = credential {
                push_requirement(
                    state,
                    tenant,
                    &mut values,
                    "credential",
                    credential,
                    "use",
                    input.resource_id,
                )
                .await?;
            }
        }
        "mcp_server" => {
            let row = sqlx::query("SELECT credential_id,runtime_sandbox_profile_id,runtime_sandbox_profile_version_id,configuration_json FROM mcp_server_versions WHERE tenant_id=? AND server_id=? AND id=?")
                .bind(tenant).bind(input.resource_id).bind(input.resource_version_id).fetch_one(&state.pool).await?;
            if let Some(credential) = row.try_get::<Option<Uuid>, _>("credential_id")? {
                push_requirement(
                    state,
                    tenant,
                    &mut values,
                    "credential",
                    credential,
                    "use",
                    input.resource_id,
                )
                .await?;
            }
            if let Some(profile) = row.try_get::<Option<Uuid>, _>("runtime_sandbox_profile_id")? {
                let owner = ensure_resource(state, tenant, "sandbox_profile", profile).await?;
                values.push(Requirement {
                    resource_type: "sandbox_profile".into(),
                    resource_id: profile,
                    resource_version_id: row.try_get("runtime_sandbox_profile_version_id")?,
                    operation: "use".into(),
                    required_by: Some(input.resource_id),
                    owner_department_id: owner,
                });
            }
            let configuration: Value = row.try_get("configuration_json")?;
            if let Some(environment) = configuration
                .pointer("/transport/environmentCredentialRefs")
                .and_then(Value::as_array)
            {
                for entry in environment {
                    let credential = entry
                        .get("credentialId")
                        .and_then(Value::as_str)
                        .and_then(|value| Uuid::parse_str(value).ok())
                        .ok_or_else(|| {
                            ApiError::bad_request(
                                "INVALID_MCP_STDIO_ENVIRONMENT",
                                "Invalid frozen MCP credential reference",
                            )
                        })?;
                    push_requirement(
                        state,
                        tenant,
                        &mut values,
                        "credential",
                        credential,
                        "use",
                        input.resource_id,
                    )
                    .await?;
                }
            }
        }
        "mcp_tool" => {
            let row = sqlx::query("SELECT t.server_id,tv.server_version_id FROM mcp_tools t JOIN mcp_tool_versions tv ON tv.tenant_id=t.tenant_id AND tv.tool_id=t.id WHERE t.tenant_id=? AND t.id=? AND tv.id=?")
                .bind(tenant).bind(input.resource_id).bind(input.resource_version_id).fetch_one(&state.pool).await?;
            let server: Uuid = row.try_get("server_id")?;
            values.push(Requirement {
                resource_type: "mcp_server".into(),
                resource_id: server,
                resource_version_id: Some(row.try_get("server_version_id")?),
                operation: "use".into(),
                required_by: Some(input.resource_id),
                owner_department_id: ensure_resource(state, tenant, "mcp_server", server).await?,
            });
        }
        "rag" => {
            let credential: Option<Uuid> = sqlx::query_scalar("SELECT c.credential_id FROM rag_resources r JOIN rag_connections c ON c.tenant_id=r.tenant_id AND c.id=r.connection_id WHERE r.tenant_id=? AND r.id=?")
                .bind(tenant).bind(input.resource_id).fetch_optional(&state.pool).await?.flatten();
            if let Some(credential) = credential {
                push_requirement(
                    state,
                    tenant,
                    &mut values,
                    "credential",
                    credential,
                    "use",
                    input.resource_id,
                )
                .await?;
            }
        }
        "memory" => {
            let credential: Option<Uuid> = sqlx::query_scalar("SELECT c.credential_id FROM memory_namespaces n JOIN memory_connections c ON c.tenant_id=n.tenant_id AND c.id=n.connection_id WHERE n.tenant_id=? AND n.id=?")
                .bind(tenant).bind(input.resource_id).fetch_optional(&state.pool).await?.flatten();
            if let Some(credential) = credential {
                push_requirement(
                    state,
                    tenant,
                    &mut values,
                    "credential",
                    credential,
                    "use",
                    input.resource_id,
                )
                .await?;
            }
        }
        _ => {}
    }
    if input.resource_type == "skill" {
        let version = if let Some(id) = input.resource_version_id {
            Some(id)
        } else {
            sqlx::query_scalar("SELECT id FROM skill_versions WHERE tenant_id=? AND skill_id=? ORDER BY version_number DESC LIMIT 1").bind(tenant).bind(input.resource_id).fetch_optional(&state.pool).await?
        };
        if let Some(version) = version {
            let rows=sqlx::query("SELECT resource_type,resource_id,resource_version_id,operation_key FROM skill_dependencies WHERE tenant_id=? AND skill_version_id=?").bind(tenant).bind(version).fetch_all(&state.pool).await?;
            for row in rows {
                let kind: String = row.try_get("resource_type")?;
                let id: Uuid = row.try_get("resource_id")?;
                let department = ensure_resource(state, tenant, &kind, id).await?;
                values.push(Requirement {
                    resource_type: kind,
                    resource_id: id,
                    resource_version_id: row.try_get("resource_version_id")?,
                    operation: row.try_get("operation_key")?,
                    required_by: Some(input.resource_id),
                    owner_department_id: department,
                });
            }
        }
    }
    let mut seen = BTreeSet::new();
    values.retain(|item| {
        seen.insert((
            item.resource_type.clone(),
            item.resource_id,
            item.operation.clone(),
        ))
    });
    Ok(values)
}

async fn push_requirement(
    state: &ControlApiState,
    tenant: Uuid,
    values: &mut Vec<Requirement>,
    resource_type: &str,
    resource_id: Uuid,
    operation: &str,
    required_by: Uuid,
) -> ApiResult<()> {
    let owner = ensure_resource(state, tenant, resource_type, resource_id).await?;
    values.push(Requirement {
        resource_type: resource_type.into(),
        resource_id,
        resource_version_id: None,
        operation: operation.into(),
        required_by: Some(required_by),
        owner_department_id: owner,
    });
    Ok(())
}

pub(super) async fn ensure_resource(
    state: &ControlApiState,
    tenant: Uuid,
    kind: &str,
    id: Uuid,
) -> ApiResult<Uuid> {
    validate_kind(kind)?;
    sqlx::query_scalar(grantable_query!(
        "SELECT owner_department_id FROM grantable WHERE tenant_id=? AND resource_type=? AND id=? AND status='active'"
    ))
        .bind(tenant)
        .bind(kind)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Active resource"))
}
