//! Opaque personal OAuth tokens are introspected for every request; no authorization cache.
use super::*;
use serde::Deserialize;
#[derive(Deserialize)]
struct Introspection {
    active: bool,
    sub: Option<String>,
    scope: Option<String>,
    aud: Option<Vec<String>>,
    ext: Option<Value>,
}
pub(super) fn public_origin() -> RuntimeResult<url::Url> {
    let value =
        std::env::var("AGENTX_RUNTIME_PUBLIC_ORIGIN").map_err(|_| RuntimeError::Unavailable)?;
    let url = url::Url::parse(&value).map_err(|_| RuntimeError::Unavailable)?;
    if url.host_str().is_none()
        || !matches!(url.scheme(), "https" | "http")
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RuntimeError::Unavailable);
    }
    Ok(url)
}
pub(super) fn issuer() -> RuntimeResult<String> {
    std::env::var("AGENTX_OAUTH_ISSUER").map_err(|_| RuntimeError::Unavailable)
}
pub(super) fn resource(slug: &str) -> RuntimeResult<String> {
    Ok(public_origin()?
        .join(&format!("gateway/v1/mcp/applications/{slug}"))
        .map_err(|_| RuntimeError::Unavailable)?
        .to_string())
}
pub(super) async fn authenticate(
    state: &RuntimeState,
    token: &str,
    route: Option<&str>,
    mcp_slug: Option<&str>,
) -> RuntimeResult<Caller> {
    let admin = std::env::var("AGENTX_HYDRA_ADMIN_URL").map_err(|_| RuntimeError::Unavailable)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| RuntimeError::Unavailable)?;
    let response = client
        .post(format!(
            "{}/admin/oauth2/introspect",
            admin.trim_end_matches('/')
        ))
        .form(&[("token", token)])
        .send()
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    if !response.status().is_success() {
        return Err(RuntimeError::Unavailable);
    }
    let data: Introspection = response
        .json()
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    if !data.active {
        return Err(RuntimeError::Unauthorized);
    }
    let ext = data
        .ext
        .as_ref()
        .and_then(|v| v.get("agentx"))
        .ok_or(RuntimeError::Unauthorized)?;
    let uuid = |field: &str| {
        ext.get(field)
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or(RuntimeError::Unauthorized)
    };
    let tenant = uuid("tenantId")?;
    let user = uuid("userId")?;
    let app = uuid("applicationId")?;
    if data.sub.as_deref() != Some(&format!("{tenant}:{user}")) {
        return Err(RuntimeError::Unauthorized);
    }
    let version = ext
        .get("tokenVersion")
        .and_then(Value::as_u64)
        .ok_or(RuntimeError::Unauthorized)?;
    let scope = format!("agentx:application:{app}");
    if !data
        .scope
        .as_deref()
        .is_some_and(|scopes| scopes.split_whitespace().any(|s| s == scope))
    {
        return Err(RuntimeError::Unauthorized);
    }
    let row=sqlx::query("SELECT u.user_name,u.department_id,u.department_name,u.role_assignments_json,r.route_key FROM runtime_user_admission u JOIN runtime_user_application_grants g ON g.tenant_id=u.tenant_id AND g.user_id=u.user_id JOIN application_routes r ON r.tenant_id=g.tenant_id AND r.application_id=g.application_id JOIN tenant_admission t ON t.tenant_id=u.tenant_id WHERE u.tenant_id=? AND u.user_id=? AND u.token_version=? AND u.status='active' AND g.application_id=? AND g.can_invoke=TRUE AND g.status='active' AND r.status='active' AND t.status='active'").bind(tenant).bind(user).bind(version).bind(app).fetch_optional(&state.pool).await?.ok_or(RuntimeError::Unauthorized)?;
    if route
        .is_some_and(|route| row.try_get::<String, _>("route_key").ok().as_deref() != Some(route))
    {
        return Err(RuntimeError::Unauthorized);
    }
    let slugs:Vec<String>=sqlx::query_scalar("SELECT DISTINCT slug FROM application_mcp_publications WHERE tenant_id=? AND application_id=? AND status='active' AND JSON_EXTRACT(manifest_json,'$.enabled')=TRUE").bind(tenant).bind(app).fetch_all(&state.pool).await?;
    if !slugs.iter().any(|slug| {
        mcp_slug.is_none_or(|expected| expected == slug)
            && data.aud.as_ref().is_some_and(|audiences| {
                resource(slug).is_ok_and(|value| audiences.contains(&value))
            })
    }) {
        return Err(RuntimeError::Unauthorized);
    }
    Ok(Caller {
        tenant_id: tenant,
        application_id: app,
        subject_id: user,
        caller_type: "user",
        token_version: Some(version),
        origin: agentx_runtime_contracts::ExecutionOriginV1 {
            initiator_user_id: Some(user),
            initiator_user_name: Some(row.try_get("user_name")?),
            initiator_department_id: Some(row.try_get("department_id")?),
            initiator_department_name: Some(row.try_get("department_name")?),
            role_assignments: serde_json::from_value(row.try_get("role_assignments_json")?)
                .map_err(|e| RuntimeError::Internal(e.into()))?,
            trigger_source_id: None,
            trigger_name: None,
        },
    })
}
