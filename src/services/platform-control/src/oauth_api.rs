//! Login and explicit consent bridge to self-hosted Hydra. No token minting in Agentx.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RegisterClient {
    request_id: Uuid,
    publication_id: Uuid,
    name: String,
    redirect_uris: Vec<String>,
}
#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientView {
    client_id: Uuid,
    publication_id: Uuid,
    resource: String,
    name: String,
    redirect_uris: Vec<String>,
    scope: String,
    status: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Decision {
    accept: bool,
}
#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthorizationRequest {
    kind: String,
    client_name: String,
    application_name: String,
    scopes: Vec<String>,
    resource: String,
}
#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Redirect {
    redirect_to: String,
}
#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GrantView {
    client_id: Uuid,
    client_name: String,
    application_name: String,
}
#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct Revocation {
    revoked: bool,
}
pub(crate) fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/applications/{application}/mcp/clients",
            get(clients).post(register),
        )
        .route(
            "/api/v1/applications/{application}/mcp/clients/{client}",
            delete(remove),
        )
        .route(
            "/api/v1/oauth/requests/{kind}/{challenge}",
            get(request).post(decide),
        )
        .route("/api/v1/oauth/grants", get(grants))
        .route("/api/v1/oauth/grants/{client}", delete(revoke))
}
async fn hydra(
    state: &ControlApiState,
    method: reqwest::Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<Value>,
) -> ApiResult<Value> {
    let admin = std::env::var("AGENTX_HYDRA_ADMIN_URL").map_err(|_| {
        ApiError::unavailable("OAUTH_UNAVAILABLE", "OAuth service is not configured")
    })?;
    let deleting = method == reqwest::Method::DELETE;
    let mut request = state
        .http
        .request(method, format!("{}{path}", admin.trim_end_matches('/')))
        .query(query)
        .timeout(std::time::Duration::from_secs(10));
    if let Some(body) = body {
        request = request.json(&body)
    }
    let response = request
        .send()
        .await
        .map_err(|_| ApiError::unavailable("OAUTH_UNAVAILABLE", "OAuth service is unavailable"))?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    if deleting && response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Value::Null);
    }
    if !response.status().is_success() {
        return Err(ApiError::bad_request(
            "OAUTH_REQUEST_REJECTED",
            "The OAuth request expired, was already used or was rejected",
        ));
    }
    response.json().await.map_err(|_| {
        ApiError::unavailable(
            "OAUTH_INVALID_RESPONSE",
            "OAuth service returned an invalid response",
        )
    })
}
pub(crate) fn origin() -> ApiResult<reqwest::Url> {
    let value = std::env::var("AGENTX_RUNTIME_PUBLIC_ORIGIN").map_err(|_| {
        ApiError::unavailable(
            "OAUTH_UNAVAILABLE",
            "Public runtime address is not configured",
        )
    })?;
    reqwest::Url::parse(&value).map_err(ApiError::internal)
}
pub(crate) fn resource(slug: &str) -> ApiResult<String> {
    Ok(origin()?
        .join(&format!("gateway/v1/mcp/applications/{slug}"))
        .map_err(ApiError::internal)?
        .to_string())
}
async fn clients(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
) -> ApiResult<Json<Vec<ClientView>>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    let rows=sqlx::query("SELECT c.id,c.name,c.publication_id,c.redirect_uris_json,c.status,p.slug FROM application_oauth_clients c JOIN application_mcp_publications p ON p.tenant_id=c.tenant_id AND p.id=c.publication_id WHERE c.tenant_id=? AND c.application_id=? ORDER BY c.created_at,c.id").bind(actor.tenant_id).bind(application).fetch_all(&state.pool).await?;
    let mut items = Vec::new();
    for row in rows {
        items.push(ClientView {
            client_id: row.try_get("id")?,
            publication_id: row.try_get("publication_id")?,
            resource: resource(&row.try_get::<String, _>("slug")?)?,
            name: row.try_get("name")?,
            redirect_uris: serde_json::from_value(row.try_get("redirect_uris_json")?)
                .map_err(ApiError::internal)?,
            scope: format!("agentx:application:{application} offline_access"),
            status: row.try_get("status")?,
        });
    }
    Ok(Json(items))
}
async fn register(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
    Json(input): Json<RegisterClient>,
) -> ApiResult<Json<ClientView>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    if input.name.trim().is_empty()
        || input.name.len() > 100
        || input.redirect_uris.is_empty()
        || input.redirect_uris.len() > 8
    {
        return Err(ApiError::bad_request(
            "OAUTH_CLIENT_INVALID",
            "Provide a client name and 1–8 callback addresses",
        ));
    }
    for value in &input.redirect_uris {
        let url = reqwest::Url::parse(value).map_err(|_| {
            ApiError::bad_request("OAUTH_REDIRECT_INVALID", "Callback address is invalid")
        })?;
        if value.len() > 2048
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        {
            return Err(ApiError::bad_request(
                "OAUTH_REDIRECT_INVALID",
                "Use HTTPS or an HTTP loopback callback of at most 2048 bytes",
            ));
        }
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM applications WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id)
        .bind(application)
        .fetch_one(&mut *tx)
        .await?;
    let slug:String=sqlx::query_scalar("SELECT p.slug FROM application_mcp_publications p JOIN outbox o ON o.id=p.outbox_id WHERE p.tenant_id=? AND p.application_id=? AND p.id=? AND p.deleted_at IS NULL AND o.status='published'").bind(actor.tenant_id).bind(application).bind(input.publication_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::conflict("MCP_PUBLICATION_REQUIRED","Publish the application MCP entrance before registering a client"))?;
    let hash = agentx_runtime_contracts::content_hash(
        &json!({"application":application,"publicationId":input.publication_id,"name":input.name,"redirectUris":input.redirect_uris}),
    )
    .map_err(ApiError::internal)?;
    let existing=sqlx::query("SELECT request_hash,status FROM application_oauth_clients WHERE tenant_id=? AND id=? FOR UPDATE").bind(actor.tenant_id).bind(input.request_id).fetch_optional(&mut *tx).await?;
    if let Some(existing) = existing {
        if existing.try_get::<String, _>("request_hash")? != hash.as_str() {
            return Err(ApiError::conflict(
                "OAUTH_CLIENT_CONFLICT",
                "Client request ID was reused with another configuration",
            ));
        }
        if !matches!(
            existing.try_get::<String, _>("status")?.as_str(),
            "preparing" | "active"
        ) {
            return Err(ApiError::conflict(
                "OAUTH_CLIENT_REVOKED",
                "Create a new registration after revoking a client",
            ));
        }
    }
    sqlx::query("INSERT IGNORE INTO application_oauth_clients(id,tenant_id,application_id,publication_id,name,redirect_uris_json,request_hash,status,created_by) VALUES(?,?,?,?,?,?,?,'preparing',?)").bind(input.request_id).bind(actor.tenant_id).bind(application).bind(input.publication_id).bind(&input.name).bind(json!(input.redirect_uris)).bind(hash.as_str()).bind(actor.user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    let scope = format!("agentx:application:{application} offline_access");
    let client = json!({"client_id":input.request_id,"client_name":input.name,"redirect_uris":input.redirect_uris,"grant_types":["authorization_code","refresh_token"],"response_types":["code"],"scope":scope,"audience":[resource(&slug)?],"token_endpoint_auth_method":"none","access_token_strategy":"opaque","skip_consent":false});
    // A fixed request identity survives a lost registration response without creating a second client.
    if hydra(
        &state,
        reqwest::Method::POST,
        "/admin/clients",
        &[],
        Some(client.clone()),
    )
    .await
    .is_err()
    {
        let saved = hydra(
            &state,
            reqwest::Method::GET,
            &format!("/admin/clients/{}", input.request_id),
            &[],
            None,
        )
        .await?;
        for key in [
            "client_id",
            "redirect_uris",
            "scope",
            "audience",
            "token_endpoint_auth_method",
            "access_token_strategy",
        ] {
            if saved.get(key) != client.get(key) {
                return Err(ApiError::conflict(
                    "OAUTH_CLIENT_CONFLICT",
                    "The authorization service has a different client registration",
                ));
            }
        }
    }
    sqlx::query("UPDATE application_oauth_clients SET status='active' WHERE tenant_id=? AND id=? AND status='preparing'").bind(actor.tenant_id).bind(input.request_id).execute(&state.pool).await?;
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM application_oauth_clients WHERE tenant_id=? AND id=? AND status='active')").bind(actor.tenant_id).bind(input.request_id).fetch_one(&state.pool).await?;
    if !active {
        hydra(
            &state,
            reqwest::Method::DELETE,
            &format!("/admin/clients/{}", input.request_id),
            &[],
            None,
        )
        .await?;
        return Err(ApiError::conflict(
            "OAUTH_CLIENT_REVOKED",
            "The client was revoked during registration",
        ));
    }
    Ok(Json(ClientView {
        client_id: input.request_id,
        publication_id: input.publication_id,
        resource: resource(&slug)?,
        name: input.name,
        redirect_uris: input.redirect_uris,
        scope,
        status: "active".into(),
    }))
}
async fn remove(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, client)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Revocation>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM application_oauth_clients WHERE tenant_id=? AND application_id=? AND id=?)").bind(actor.tenant_id).bind(application).bind(client).fetch_one(&state.pool).await?;
    if !exists {
        return Err(ApiError::not_found("OAuth client"));
    }
    sqlx::query("UPDATE application_oauth_clients SET status='revoking' WHERE tenant_id=? AND application_id=? AND id=?").bind(actor.tenant_id).bind(application).bind(client).execute(&state.pool).await?;
    hydra(
        &state,
        reqwest::Method::DELETE,
        "/admin/oauth2/tokens",
        &[("client_id", client.to_string())],
        None,
    )
    .await?;
    hydra(
        &state,
        reqwest::Method::DELETE,
        &format!("/admin/clients/{client}"),
        &[],
        None,
    )
    .await?;
    sqlx::query("UPDATE application_oauth_clients SET status='revoked' WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(client)
        .execute(&state.pool)
        .await?;
    Ok(Json(Revocation { revoked: true }))
}
async fn challenge(
    state: &ControlApiState,
    actor: &Actor,
    kind: &str,
    challenge: &str,
) -> ApiResult<(Value, Uuid, String, String)> {
    if !matches!(kind, "login" | "consent") || challenge.len() > 16 * 1024 || challenge.is_empty() {
        return Err(ApiError::bad_request(
            "OAUTH_CHALLENGE_INVALID",
            "Invalid OAuth challenge",
        ));
    }
    let data = hydra(
        state,
        reqwest::Method::GET,
        &format!("/admin/oauth2/auth/requests/{kind}"),
        &[(
            if kind == "login" {
                "login_challenge"
            } else {
                "consent_challenge"
            },
            challenge.into(),
        )],
        None,
    )
    .await?;
    let client = data
        .pointer("/client/client_id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| {
            ApiError::forbidden("This OAuth client is not registered for an Agentx application")
        })?;
    let row=sqlx::query("SELECT c.application_id,c.name,a.name application_name,p.slug FROM application_oauth_clients c JOIN applications a ON a.tenant_id=c.tenant_id AND a.id=c.application_id JOIN application_mcp_publications p ON p.tenant_id=c.tenant_id AND p.id=c.publication_id JOIN application_mcp_routes r ON r.tenant_id=c.tenant_id AND r.application_id=c.application_id AND r.slug=p.slug WHERE c.tenant_id=? AND c.id=? AND c.status='active' AND a.status='active'").bind(actor.tenant_id).bind(client).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::forbidden("The OAuth client is unavailable to this account"))?;
    let app: Uuid = row.try_get("application_id")?;
    if !crate::runtime_grants::authorize_user_application(
        &state.pool,
        actor.tenant_id,
        app,
        actor.user_id,
    )
    .await?
    {
        return Err(ApiError::forbidden(
            "The current account cannot use this application",
        ));
    }
    if kind == "consent"
        && data.get("subject").and_then(Value::as_str)
            != Some(&format!("{}:{}", actor.tenant_id, actor.user_id))
    {
        return Err(ApiError::forbidden(
            "Continue authorization with the account that started this login",
        ));
    }
    let required = format!("agentx:application:{app}");
    let scopes = data
        .get("requested_scope")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiError::bad_request(
                "OAUTH_SCOPE_REQUIRED",
                "The client must request this application's scope",
            )
        })?;
    if !scopes.iter().any(|scope| scope == &required)
        || scopes.iter().any(|scope| {
            scope.as_str() != Some(&required) && scope.as_str() != Some("offline_access")
        })
    {
        return Err(ApiError::bad_request(
            "OAUTH_SCOPE_INVALID",
            "The OAuth request must target exactly this application",
        ));
    }
    Ok((
        data,
        app,
        row.try_get("application_name")?,
        row.try_get("slug")?,
    ))
}
async fn request(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((kind, id)): Path<(String, String)>,
) -> ApiResult<Json<AuthorizationRequest>> {
    let (data, _, application_name, slug) = challenge(&state, &actor, &kind, &id).await?;
    Ok(Json(AuthorizationRequest {
        kind,
        client_name: data
            .pointer("/client/client_name")
            .and_then(Value::as_str)
            .unwrap_or("MCP client")
            .into(),
        application_name,
        scopes: data["requested_scope"]
            .as_array()
            .expect("validated scopes")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        resource: resource(&slug)?,
    }))
}
async fn decide(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((kind, id)): Path<(String, String)>,
    Json(decision): Json<Decision>,
) -> ApiResult<Json<Redirect>> {
    let (data, app, _, slug) = challenge(&state, &actor, &kind, &id).await?;
    let action = if decision.accept { "accept" } else { "reject" };
    let body = if !decision.accept {
        json!({"error":"access_denied","error_description":"The user denied application access"})
    } else if kind == "login" {
        json!({"subject":format!("{}:{}",actor.tenant_id,actor.user_id),"remember":false})
    } else {
        json!({"grant_scope":data["requested_scope"],"grant_access_token_audience":[resource(&slug)?],"remember":false,"session":{"access_token":{"agentx":{"tenantId":actor.tenant_id,"userId":actor.user_id,"tokenVersion":actor.token_version,"applicationId":app}}}})
    };
    let result = hydra(
        &state,
        reqwest::Method::PUT,
        &format!("/admin/oauth2/auth/requests/{kind}/{action}"),
        &[(
            if kind == "login" {
                "login_challenge"
            } else {
                "consent_challenge"
            },
            id,
        )],
        Some(body),
    )
    .await?;
    let target = result
        .get("redirect_to")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ApiError::unavailable(
                "OAUTH_INVALID_RESPONSE",
                "Authorization did not return a continuation address",
            )
        })?;
    let url = reqwest::Url::parse(target).map_err(ApiError::internal)?;
    if url.origin() != origin()?.origin() {
        return Err(ApiError::unavailable(
            "OAUTH_REDIRECT_INVALID",
            "OAuth continuation address did not match its issuer",
        ));
    }
    Ok(Json(Redirect {
        redirect_to: target.into(),
    }))
}
async fn grants(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<Json<Vec<GrantView>>> {
    let data = hydra(
        &state,
        reqwest::Method::GET,
        "/admin/oauth2/auth/sessions/consent",
        &[
            ("subject", format!("{}:{}", actor.tenant_id, actor.user_id)),
            ("page_size", "100".into()),
        ],
        None,
    )
    .await?;
    let mut items = Vec::new();
    for grant in data.as_array().into_iter().flatten() {
        let Some(client) = grant
            .pointer("/consent_request/client/client_id")
            .and_then(Value::as_str)
            .or_else(|| grant.pointer("/client/client_id").and_then(Value::as_str))
        else {
            continue;
        };
        if let Ok(client) = Uuid::parse_str(client) {
            let row=sqlx::query("SELECT c.name,a.name application_name FROM application_oauth_clients c JOIN applications a ON a.tenant_id=c.tenant_id AND a.id=c.application_id WHERE c.tenant_id=? AND c.id=?").bind(actor.tenant_id).bind(client).fetch_optional(&state.pool).await?;
            if let Some(row) = row {
                items.push(GrantView {
                    client_id: client,
                    client_name: row.try_get("name")?,
                    application_name: row.try_get("application_name")?,
                });
            }
        }
    }
    items.sort_by_key(|grant| grant.client_id);
    items.dedup_by_key(|grant| grant.client_id);
    Ok(Json(items))
}
async fn revoke(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(client): Path<Uuid>,
) -> ApiResult<Json<Revocation>> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM application_oauth_clients WHERE tenant_id=? AND id=?)",
    )
    .bind(actor.tenant_id)
    .bind(client)
    .fetch_one(&state.pool)
    .await?;
    if !exists {
        return Err(ApiError::not_found("OAuth client"));
    }
    hydra(
        &state,
        reqwest::Method::DELETE,
        "/admin/oauth2/auth/sessions/consent",
        &[
            ("subject", format!("{}:{}", actor.tenant_id, actor.user_id)),
            ("client", client.to_string()),
        ],
        None,
    )
    .await?;
    Ok(Json(Revocation { revoked: true }))
}

pub(crate) fn apply_openapi(document: &mut Value) {
    for (path, method, input, output, array) in [
        (
            "/api/v1/applications/{application}/mcp/clients",
            "get",
            None,
            "ApplicationOAuthClient",
            true,
        ),
        (
            "/api/v1/applications/{application}/mcp/clients",
            "post",
            Some("RegisterApplicationOAuthClient"),
            "ApplicationOAuthClient",
            false,
        ),
        (
            "/api/v1/applications/{application}/mcp/clients/{client}",
            "delete",
            None,
            "ApplicationOAuthRevocation",
            false,
        ),
        (
            "/api/v1/oauth/requests/{kind}/{challenge}",
            "get",
            None,
            "ApplicationOAuthRequest",
            false,
        ),
        (
            "/api/v1/oauth/requests/{kind}/{challenge}",
            "post",
            Some("ApplicationOAuthDecision"),
            "ApplicationOAuthRedirect",
            false,
        ),
        (
            "/api/v1/oauth/grants",
            "get",
            None,
            "ApplicationOAuthGrant",
            true,
        ),
        (
            "/api/v1/oauth/grants/{client}",
            "delete",
            None,
            "ApplicationOAuthRevocation",
            false,
        ),
    ] {
        let parameters = path
            .split('/')
            .filter_map(|part| part.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
            .map(|name| json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}))
            .collect::<Vec<_>>();
        let schema = if array {
            json!({"type":"array","items":{"$ref":format!("#/components/schemas/{output}")}})
        } else {
            json!({"$ref":format!("#/components/schemas/{output}")})
        };
        let mut operation = json!({"parameters":parameters,"security":[{"bearerAuth":[]}],"responses":{"200":{"description":"Application OAuth result","content":{"application/json":{"schema":schema}}}}});
        if let Some(input) = input {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{input}")}}}})
        }
        document["paths"][path][method] = operation;
    }
}
