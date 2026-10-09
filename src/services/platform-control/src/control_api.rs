use std::{
    collections::{BTreeSet, HashMap},
    env,
    sync::Arc,
};

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, Query, State},
    http::{StatusCode, header::AUTHORIZATION, request::Parts},
    middleware,
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, Row};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use agentx_runtime_contracts::{
    ContentHash, DELEGATION_TOKEN_TTL_SECONDS, DelegationClaimsV1, ExecutionOriginV1,
    RuntimeTriggerConfigurationV1, RuntimeTriggerSpecV1, ScheduleMisfirePolicyV1, SessionDetailV1,
    SessionSearchPageV1, SessionSearchRequestV1, SessionUpgradeCommandV1, UserAccessClaimsV1,
    VaultSecretReferenceV1, content_hash, issue_delegation_token, issue_user_access_token,
    now_unix, verify_user_access_token,
};

use crate::api_error::{ApiError, ApiResult, invalid_credentials};
use crate::control_helpers::{
    allocate_activation_sequence, generate_api_key, hex, parse_action_id, random_url_token,
    required_name, validate_schedule,
};

const REFRESH_COOKIE: &str = "agentx_refresh";
#[path = "application_webhooks.rs"]
mod application_webhooks;
mod playground_config;
#[path = "webhook_provider_templates.rs"]
mod webhook_provider_templates;
#[path = "control_api_webhooks.rs"]
mod webhooks;
use application_webhooks::{CreateWebhookRequest, UpdateWebhookRequest, WebhookResponse};
use webhooks::{
    create_webhook, delete_webhook, get_webhook, list_webhook_provider_templates, list_webhooks,
    rebuild_trigger_revision, update_webhook,
};

#[derive(Clone)]
pub struct ControlApiState {
    pub(crate) pool: MySqlPool,
    pub(crate) control_objects: Arc<dyn object_store::ObjectStore>,
    pub(crate) auth: Arc<AuthSettings>,
    pub(crate) runtime_query_url: String,
    pub(crate) observability_query_url: String,
    pub(crate) delegation_kid: String,
    pub(crate) delegation_key: SecretString,
    pub(crate) runtime_command_kid: String,
    pub(crate) runtime_command_key: SecretString,
    pub(crate) vault_endpoint: String,
    pub(crate) vault_mount: String,
    pub(crate) vault_token: SecretString,
    pub(crate) http: reqwest::Client,
    pub(crate) work_packages: crate::work_packages::WorkPackageClient,
}

#[derive(Clone)]
pub(crate) struct AuthSettings {
    refresh_secret: SecretString,
    access_kid: String,
    access_private_key: SecretString,
    access_public_keys: HashMap<String, Vec<u8>>,
    issuer: String,
    audience: String,
    cookie_secure: bool,
}

impl ControlApiState {
    pub fn from_env(
        pool: MySqlPool,
        control_objects: Arc<dyn object_store::ObjectStore>,
    ) -> anyhow::Result<Self> {
        let refresh_secret = env::var("AGENTX_CONTROL_API_REFRESH_JWT_SIGNING_SECRET")?;
        anyhow::ensure!(
            refresh_secret.len() >= 32,
            "Control API Refresh JWT secret is too short"
        );
        let access_public_keys = serde_json::from_str::<HashMap<String, String>>(&env::var(
            "AGENTX_CONTROL_USER_JWT_PUBLIC_KEYS_JSON",
        )?)?
        .into_iter()
        .map(|(kid, pem)| (kid, pem.into_bytes()))
        .collect();
        Ok(Self {
            pool,
            control_objects,
            work_packages: crate::work_packages::WorkPackageClient::from_env()?,
            runtime_query_url: env::var("AGENTX_RUNTIME_INTERNAL_URL").unwrap_or_else(|_| {
                "http://runtime-gateway-internal.agentx-runtime.svc:8080".into()
            }),
            observability_query_url: env::var("AGENTX_OBSERVABILITY_INTERNAL_URL")
                .unwrap_or_else(|_| "http://observability.agentx-runtime.svc:8080".into()),
            delegation_kid: env::var("AGENTX_CONTROL_BFF_JWT_KID")?,
            delegation_key: SecretString::from(env::var("AGENTX_CONTROL_BFF_JWT_PRIVATE_KEY_PEM")?),
            runtime_command_kid: env::var("AGENTX_CONTROL_PUBLISHER_JWT_KID")?,
            runtime_command_key: SecretString::from(env::var(
                "AGENTX_CONTROL_PUBLISHER_JWT_PRIVATE_KEY_PEM",
            )?),
            vault_endpoint: env::var("AGENTX_CONTROL_VAULT_ENDPOINT")?
                .trim_end_matches('/')
                .to_owned(),
            vault_mount: env::var("AGENTX_CONTROL_VAULT_MOUNT")?,
            vault_token: SecretString::from(env::var("AGENTX_CONTROL_VAULT_TOKEN")?),
            http: agentx_service_kit::reqwest_client_builder_with_ca(
                "AGENTX_CONTROL_VAULT_TLS_CA_PATH",
            )?
            .timeout(std::time::Duration::from_secs(5))
            .build()?,
            auth: Arc::new(AuthSettings {
                refresh_secret: SecretString::from(refresh_secret),
                access_kid: env::var("AGENTX_CONTROL_USER_JWT_KID")?,
                access_private_key: SecretString::from(env::var(
                    "AGENTX_CONTROL_USER_JWT_PRIVATE_KEY_PEM",
                )?),
                access_public_keys,
                issuer: env::var("AGENTX_CONTROL_API_JWT_ISSUER")
                    .unwrap_or_else(|_| "agentx-platform".into()),
                audience: env::var("AGENTX_CONTROL_API_JWT_AUDIENCE")
                    .unwrap_or_else(|_| "agentx-web".into()),
                cookie_secure: env::var("AGENTX_CONTROL_API_COOKIE_SECURE")
                    .map(|value| !matches!(value.as_str(), "false" | "0"))
                    .unwrap_or(true),
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        pool: MySqlPool,
        control_objects: Arc<dyn object_store::ObjectStore>,
        runtime_url: String,
        vault_url: String,
    ) -> Self {
        let private_key = include_str!(
            "../../../crates/agentx-runtime-contracts/tests/fixtures/service-private.pem"
        );
        let public_key = include_str!(
            "../../../crates/agentx-runtime-contracts/tests/fixtures/service-public.pem"
        );
        Self {
            pool,
            control_objects,
            auth: Arc::new(AuthSettings {
                refresh_secret: SecretString::from("test-refresh-secret-with-at-least-32-bytes"),
                access_kid: "test-user".into(),
                access_private_key: SecretString::from(private_key),
                access_public_keys: HashMap::from([(
                    "test-user".into(),
                    public_key.as_bytes().to_vec(),
                )]),
                issuer: "agentx-platform".into(),
                audience: "agentx-web".into(),
                cookie_secure: false,
            }),
            runtime_query_url: runtime_url.clone(),
            observability_query_url: runtime_url.clone(),
            delegation_kid: "test-bff".into(),
            delegation_key: SecretString::from(private_key),
            runtime_command_kid: "test-service".into(),
            runtime_command_key: SecretString::from(private_key),
            vault_endpoint: vault_url,
            vault_mount: "secret".into(),
            vault_token: SecretString::from("test-vault-token"),
            http: reqwest::Client::new(),
            work_packages: crate::work_packages::WorkPackageClient::for_test(runtime_url),
        }
    }
}

pub fn router(state: ControlApiState) -> Router {
    routes()
        .layer(middleware::from_fn(
            crate::api_error::normalize_json_rejection,
        ))
        .with_state(state)
}

fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
        .route(
            "/api/v1/workflows/{id}/debug-runs",
            get(crate::governance_api::list_debug_runs).post(crate::work_packages::start_debug_run),
        )
        .route(
            "/api/v1/applications/{id}",
            get(get_application)
                .patch(update_application)
                .delete(delete_application),
        )
        .route(
            "/api/v1/applications/{id}/deployments",
            get(list_deployments).post(create_deployment),
        )
        .route(
            "/api/v1/applications/{id}/deployments/{deployment_id}/playground-config",
            get(playground_config::get).put(playground_config::put),
        )
        .route(
            "/api/v1/applications/{id}/publish-attempts/{action}",
            get(get_publish_attempt).post(retry_publish_attempt),
        )
        .route(
            "/api/v1/applications/{id}/deployments/{action}",
            post(rollback_deployment),
        )
        .route(
            "/api/v1/applications/{id}/api-keys",
            get(list_api_keys).post(create_api_key),
        )
        .route(
            "/api/v1/applications/{id}/api-keys/{key_id}/rotate",
            post(rotate_api_key),
        )
        .route(
            "/api/v1/applications/{id}/api-keys/{key_id}/revoke",
            post(revoke_api_key),
        )
        .route(
            "/api/v1/applications/{id}/webhooks",
            get(list_webhooks).post(create_webhook),
        )
        .route(
            "/api/v1/webhook-provider-templates",
            get(list_webhook_provider_templates),
        )
        .route(
            "/api/v1/applications/{id}/webhooks/{webhook_id}",
            get(get_webhook)
                .patch(update_webhook)
                .delete(delete_webhook),
        )
        .route(
            "/api/v1/applications/{id}/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/v1/applications/{id}/schedules/{schedule_id}",
            axum::routing::patch(update_schedule).delete(delete_schedule),
        )
        .route("/api/v1/applications/{id}/sessions", get(list_sessions))
        .route("/api/v1/sessions/{id}/upgrade", post(upgrade_session))
        .merge(crate::bootstrap_api::routes())
        .merge(crate::iam_api::routes())
        .merge(crate::workflow_api::routes())
        .merge(crate::workflow_operations::routes())
        .merge(crate::application_catalog_api::routes())
        .merge(crate::catalog_api::routes())
        .merge(crate::canvas_plugin_api::routes())
        .merge(crate::credential_api::routes())
        .merge(crate::dataset_api::routes())
        .merge(crate::deletion_api::routes())
        .merge(crate::sandbox_profile_api::routes())
        .merge(crate::model_api::routes())
        .merge(crate::operations_api::routes())
        .merge(crate::external_resource_api::routes())
        .merge(crate::knowledge_document_api::routes())
        .merge(crate::insights_api::routes())
        .merge(crate::mcp_api::routes())
        .merge(crate::skill_api::routes())
        .merge(crate::resource_api::routes())
        .merge(crate::governance_api::routes())
        .merge(crate::runtime_bff::routes())
}

#[cfg(test)]
#[test]
fn public_router_has_no_conflicting_paths() {
    let _ = routes();
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Claims {
    pub(crate) sub: Uuid,
    pub(crate) tid: Uuid,
    pub(crate) ver: u64,
    kind: String,
    jti: Uuid,
    family: Option<Uuid>,
    iss: String,
    aud: String,
    iat: i64,
    exp: i64,
}

#[derive(Clone)]
pub(crate) struct Actor {
    pub(crate) tenant_id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) token_version: u64,
    pub(crate) department_id: Uuid,
    pub(crate) username: String,
    pub(crate) display_name: String,
    pub(crate) roles: Vec<String>,
    pub(crate) permissions: Vec<String>,
}

impl Actor {
    pub(crate) fn require(&self, permission: &str) -> ApiResult<()> {
        self.permissions
            .iter()
            .any(|value| value == permission)
            .then_some(())
            .ok_or_else(|| ApiError::forbidden(format!("Missing permission {permission}")))
    }
}

pub(crate) async fn execution_origin(
    state: &ControlApiState,
    actor: &Actor,
) -> ApiResult<ExecutionOriginV1> {
    let department_name: String =
        sqlx::query_scalar("SELECT name FROM departments WHERE tenant_id=? AND id=?")
            .bind(actor.tenant_id)
            .bind(actor.department_id)
            .fetch_one(&state.pool)
            .await?;
    let role_assignments =
        crate::runtime_grants::user_role_assignments(&state.pool, actor.tenant_id, actor.user_id)
            .await?;
    Ok(ExecutionOriginV1 {
        initiator_user_id: Some(actor.user_id),
        initiator_user_name: Some(actor.display_name.clone()),
        initiator_department_id: Some(actor.department_id),
        initiator_department_name: Some(department_name),
        trigger_source_id: None,
        trigger_name: None,
        role_assignments,
    })
}

impl FromRequestParts<ControlApiState> for Actor {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ControlApiState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| {
                ApiError::unauthorized("AUTHENTICATION_REQUIRED", "Authentication is required")
            })?;
        let claims = decode_token(&state.auth, token, "access")?;
        load_actor(state, &claims).await
    }
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthResponse {
    pub(crate) access_token: Option<String>,
    pub(crate) expires_in: Option<i64>,
    password_change_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) change_password_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user: Option<MeResponse>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MeResponse {
    id: Uuid,
    username: String,
    display_name: String,
    company_id: Uuid,
    company_name: String,
    department_id: Uuid,
    department_name: String,
    roles: Vec<String>,
    permissions: Vec<String>,
    locale: String,
    timezone: String,
}

async fn login(
    State(state): State<ControlApiState>,
    jar: CookieJar,
    Json(input): Json<LoginRequest>,
) -> ApiResult<(CookieJar, Json<AuthResponse>)> {
    let normalized = input.username.trim().to_ascii_lowercase();
    let row = sqlx::query("SELECT u.id,u.tenant_id,u.token_version,u.password_change_required,c.password_hash FROM users u JOIN user_credentials c ON c.user_id=u.id WHERE u.username_normalized=? AND u.status='active'")
        .bind(normalized).fetch_optional(&state.pool).await?
        .ok_or_else(invalid_credentials)?;
    let hash: String = row.try_get("password_hash")?;
    let valid = PasswordHash::new(&hash).ok().is_some_and(|hash| {
        Argon2::default()
            .verify_password(input.password.as_bytes(), &hash)
            .is_ok()
    });
    if !valid {
        return Err(invalid_credentials());
    }
    if row.try_get::<bool, _>("password_change_required")? {
        let tenant_id = row.try_get("tenant_id")?;
        let user_id = row.try_get("id")?;
        let version = row.try_get("token_version")?;
        return Ok((
            jar,
            Json(AuthResponse {
                access_token: None,
                expires_in: None,
                password_change_required: true,
                change_password_token: Some(issue_control_token(
                    &state.auth,
                    user_id,
                    tenant_id,
                    version,
                    "change_password",
                    600,
                    None,
                )?),
                user: None,
            }),
        ));
    }
    create_session(
        &state,
        jar,
        row.try_get("tenant_id")?,
        row.try_get("id")?,
        row.try_get("token_version")?,
    )
    .await
}

async fn refresh(
    State(state): State<ControlApiState>,
    jar: CookieJar,
) -> ApiResult<(CookieJar, Json<AuthResponse>)> {
    let token = jar
        .get(REFRESH_COOKIE)
        .map(|cookie| cookie.value().to_owned())
        .ok_or_else(|| ApiError::unauthorized("REFRESH_REQUIRED", "Refresh session is missing"))?;
    let claims = decode_token(&state.auth, &token, "refresh")?;
    create_session(&state, jar, claims.tid, claims.sub, claims.ver).await
}

async fn logout(jar: CookieJar) -> (StatusCode, CookieJar) {
    let cookie = Cookie::build((REFRESH_COOKIE, ""))
        .path("/api/v1/auth")
        .max_age(Duration::ZERO)
        .build();
    (StatusCode::NO_CONTENT, jar.remove(cookie))
}

async fn me(State(state): State<ControlApiState>, actor: Actor) -> ApiResult<Json<MeResponse>> {
    Ok(Json(load_me(&state, &actor).await?))
}

pub(crate) async fn create_session(
    state: &ControlApiState,
    jar: CookieJar,
    tenant_id: Uuid,
    user_id: Uuid,
    version: u64,
) -> ApiResult<(CookieJar, Json<AuthResponse>)> {
    let actor = load_actor(
        state,
        &Claims {
            sub: user_id,
            tid: tenant_id,
            ver: version,
            kind: "access".into(),
            jti: Uuid::nil(),
            family: None,
            iss: state.auth.issuer.clone(),
            aud: state.auth.audience.clone(),
            iat: 0,
            exp: i64::MAX,
        },
    )
    .await?;
    let me = load_me(state, &actor).await?;
    let origin = execution_origin(state, &actor).await?;
    let access = issue_access_token(&state.auth, user_id, tenant_id, version, 900, origin)?;
    let family = Uuid::now_v7();
    let refresh = issue_control_token(
        &state.auth,
        user_id,
        tenant_id,
        version,
        "refresh",
        604_800,
        Some(family),
    )?;
    let cookie = Cookie::build((REFRESH_COOKIE, refresh))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.auth.cookie_secure)
        .path("/api/v1/auth")
        .max_age(Duration::seconds(604_800))
        .build();
    Ok((
        jar.add(cookie),
        Json(AuthResponse {
            access_token: Some(access),
            expires_in: Some(900),
            password_change_required: false,
            change_password_token: None,
            user: Some(me),
        }),
    ))
}

fn issue_access_token(
    settings: &AuthSettings,
    user_id: Uuid,
    tenant_id: Uuid,
    version: u64,
    ttl: i64,
    origin: ExecutionOriginV1,
) -> ApiResult<String> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    issue_user_access_token(
        &settings.access_kid,
        settings.access_private_key.expose_secret().as_bytes(),
        &UserAccessClaimsV1 {
            iss: settings.issuer.clone(),
            aud: "agentx-runtime-gateway".into(),
            sub: user_id,
            tenant_id,
            token_version: version,
            kind: "access".into(),
            origin,
            iat: now,
            exp: now + ttl,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)
}

pub(crate) fn issue_control_token(
    settings: &AuthSettings,
    user_id: Uuid,
    tenant_id: Uuid,
    version: u64,
    kind: &str,
    ttl: i64,
    family: Option<Uuid>,
) -> ApiResult<String> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    encode(
        &Header::new(Algorithm::HS256),
        &Claims {
            sub: user_id,
            tid: tenant_id,
            ver: version,
            kind: kind.into(),
            jti: Uuid::now_v7(),
            family,
            iss: settings.issuer.clone(),
            aud: settings.audience.clone(),
            iat: now,
            exp: now + ttl,
        },
        &EncodingKey::from_secret(settings.refresh_secret.expose_secret().as_bytes()),
    )
    .map_err(ApiError::internal)
}

pub(crate) fn decode_token(settings: &AuthSettings, token: &str, kind: &str) -> ApiResult<Claims> {
    if kind == "access" {
        let claims = verify_user_access_token(
            token,
            &settings.access_public_keys,
            &settings.issuer,
            "agentx-runtime-gateway",
        )
        .map_err(|_| ApiError::unauthorized("INVALID_TOKEN", "Token is invalid or expired"))?;
        return Ok(Claims {
            sub: claims.sub,
            tid: claims.tenant_id,
            ver: claims.token_version,
            kind: claims.kind,
            jti: claims.jti,
            family: None,
            iss: claims.iss,
            aud: claims.aud,
            iat: claims.iat,
            exp: claims.exp,
        });
    }
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[&settings.issuer]);
    validation.set_audience(&[&settings.audience]);
    let claims = decode::<Claims>(
        token,
        &DecodingKey::from_secret(settings.refresh_secret.expose_secret().as_bytes()),
        &validation,
    )
    .map_err(|_| ApiError::unauthorized("INVALID_TOKEN", "Token is invalid or expired"))?
    .claims;
    if claims.kind != kind {
        return Err(ApiError::unauthorized(
            "INVALID_TOKEN",
            "Token type is invalid",
        ));
    }
    Ok(claims)
}

async fn load_actor(state: &ControlApiState, claims: &Claims) -> ApiResult<Actor> {
    let user = sqlx::query("SELECT u.username,u.display_name,u.token_version,u.status,ud.department_id FROM users u JOIN user_departments ud ON ud.user_id=u.id WHERE u.id=? AND u.tenant_id=?")
        .bind(claims.sub).bind(claims.tid).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::unauthorized("INVALID_TOKEN", "User no longer exists"))?;
    if user.try_get::<String, _>("status")? != "active"
        || user.try_get::<u64, _>("token_version")? != claims.ver
    {
        return Err(ApiError::unauthorized(
            "SESSION_REVOKED",
            "Session has been revoked",
        ));
    }
    let rows = sqlx::query("SELECT r.code,p.permission_key FROM user_roles ur JOIN roles r ON r.id=ur.role_id AND r.tenant_id=ur.tenant_id LEFT JOIN role_permissions rp ON rp.role_id=r.id LEFT JOIN permissions p ON p.id=rp.permission_id WHERE ur.user_id=? AND ur.tenant_id=? AND r.status='active'")
        .bind(claims.sub).bind(claims.tid).fetch_all(&state.pool).await?;
    let mut roles = Vec::new();
    let mut permissions = Vec::new();
    for row in rows {
        let role: String = row.try_get("code")?;
        if !roles.contains(&role) {
            roles.push(role);
        }
        if let Some(permission) = row.try_get::<Option<String>, _>("permission_key")? {
            if !permissions.contains(&permission) {
                permissions.push(permission);
            }
        }
    }
    Ok(Actor {
        tenant_id: claims.tid,
        user_id: claims.sub,
        token_version: claims.ver,
        department_id: user.try_get("department_id")?,
        username: user.try_get("username")?,
        display_name: user.try_get("display_name")?,
        roles,
        permissions,
    })
}

async fn load_me(state: &ControlApiState, actor: &Actor) -> ApiResult<MeResponse> {
    let row = sqlx::query("SELECT t.name company_name,d.name department_name,ts.locale,ts.timezone FROM tenants t JOIN departments d ON d.tenant_id=t.id AND d.id=? LEFT JOIN tenant_settings ts ON ts.tenant_id=t.id WHERE t.id=?")
        .bind(actor.department_id).bind(actor.tenant_id).fetch_one(&state.pool).await?;
    Ok(MeResponse {
        id: actor.user_id,
        username: actor.username.clone(),
        display_name: actor.display_name.clone(),
        company_id: actor.tenant_id,
        company_name: row.try_get("company_name")?,
        department_id: actor.department_id,
        department_name: row.try_get("department_name")?,
        roles: actor.roles.clone(),
        permissions: actor.permissions.clone(),
        locale: row
            .try_get::<Option<String>, _>("locale")?
            .unwrap_or_else(|| "zh-CN".into()),
        timezone: row
            .try_get::<Option<String>, _>("timezone")?
            .unwrap_or_else(|| "Asia/Shanghai".into()),
    })
}

#[derive(Serialize)]
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

async fn get_application(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApplicationResponse>> {
    actor.require("application:view")?;
    Ok(Json(load_application(&state, &actor, id).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateApplicationRequest {
    name: String,
    description: Option<String>,
    visibility: String,
    status: String,
    version: u64,
}

async fn update_application(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateApplicationRequest>,
) -> ApiResult<Json<ApplicationResponse>> {
    actor.require("application:manage")?;
    require_application(&state, &actor, id).await?;
    if !matches!(input.status.as_str(), "draft" | "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_APPLICATION_STATUS",
            "Application status is invalid",
        ));
    }
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("UPDATE applications SET name=?,description=?,visibility=?,status=?,version=version+1 WHERE id=? AND tenant_id=? AND version=?")
        .bind(input.name.trim()).bind(input.description).bind(input.visibility).bind(&input.status)
        .bind(id).bind(actor.tenant_id).bind(input.version).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "APPLICATION_VERSION_CONFLICT",
            "Application changed on the server",
        ));
    }
    if input.status == "disabled" {
        admission_outbox(
            &mut tx,
            actor.tenant_id,
            id,
            "ApplicationAdmissionChanged",
            json!({"applicationId":id,"status":"disabled"}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(load_application(&state, &actor, id).await?))
}

async fn delete_application(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    actor.require("application:manage")?;
    // A Runtime Session can only be created from a published deployment. The
    // Control-side deployment reference therefore fail-closes deletion without
    // reading the Runtime-owned application_sessions table.
    let referenced: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM application_deployments WHERE tenant_id=? AND application_id=? UNION ALL SELECT 1 FROM api_keys k WHERE k.tenant_id=? AND k.application_id=?)")
        .bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if referenced {
        return Err(ApiError::conflict(
            "RESOURCE_REFERENCED",
            "Application has deployments or API keys and cannot be deleted",
        ));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM application_webhooks WHERE tenant_id=? AND application_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM application_schedules WHERE tenant_id=? AND application_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query("DELETE FROM applications WHERE tenant_id=? AND id=? AND version=?")
        .bind(actor.tenant_id)
        .bind(id)
        .bind(query.expected_version)
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "APPLICATION_VERSION_CONFLICT",
            "Application changed",
        ));
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn load_application(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<ApplicationResponse> {
    require_application(state, actor, id).await?;
    let row = sqlx::query("SELECT a.id,a.workflow_id,w.name workflow_name,a.name,a.slug,a.description,a.visibility,a.status,a.owner_department_id,h.deployment_id active_deployment_id,wv.version_number active_version_number,a.runtime_config_revision,a.published_runtime_config_revision,a.version,a.updated_at FROM applications a JOIN workflows w ON w.id=a.workflow_id LEFT JOIN application_deployment_heads h ON h.tenant_id=a.tenant_id AND h.application_id=a.id LEFT JOIN application_deployments ad ON ad.id=h.deployment_id LEFT JOIN workflow_versions wv ON wv.id=ad.workflow_version_id WHERE a.id=? AND a.tenant_id=?")
        .bind(id).bind(actor.tenant_id).fetch_one(&state.pool).await?;
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

async fn require_application(state: &ControlApiState, actor: &Actor, id: Uuid) -> ApiResult<()> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM applications WHERE id=? AND tenant_id=?)")
            .bind(id)
            .bind(actor.tenant_id)
            .fetch_one(&state.pool)
            .await?;
    exists
        .then_some(())
        .ok_or_else(|| ApiError::not_found("Application"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentResponse {
    id: Uuid,
    application_id: Uuid,
    workflow_version_id: Uuid,
    workflow_version_number: u64,
    environment_id: Uuid,
    environment_name: String,
    sequence_number: u64,
    input_schema: Value,
    output_schema: Value,
    session_version_policy: String,
    status: String,
    publish_attempt_id: Option<Uuid>,
    publish_error_code: Option<String>,
    publish_error_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateDeploymentRequest {
    workflow_version_id: Uuid,
    environment_id: Uuid,
    session_version_policy: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleResponse {
    id: Uuid,
    name: String,
    cron_expression: String,
    timezone: String,
    input: Value,
    misfire_policy: String,
    status: String,
    version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateScheduleRequest {
    name: String,
    cron_expression: String,
    timezone: String,
    input: Value,
    misfire_policy: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateScheduleRequest {
    name: String,
    cron_expression: String,
    timezone: String,
    input: Value,
    misfire_policy: String,
    status: String,
    version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteQuery {
    expected_version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpgradeSessionRequest {
    workflow_version_id: Uuid,
    version: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionResponse {
    id: Uuid,
    application_id: Uuid,
    application_deployment_id: Uuid,
    workflow_version_id: Option<Uuid>,
    version_policy: String,
    external_user_id: Option<String>,
    title: Option<String>,
    status: String,
    version: u64,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

async fn list_deployments(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<DeploymentResponse>>> {
    actor.require("application:view")?;
    require_application(&state, &actor, id).await?;
    let rows = sqlx::query(DEPLOYMENT_SELECT)
        .bind(actor.tenant_id)
        .bind(id)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(deployment_from_row)
            .collect::<ApiResult<_>>()?,
    ))
}

async fn create_deployment(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<CreateDeploymentRequest>,
) -> ApiResult<(StatusCode, Json<DeploymentResponse>)> {
    actor.require("application:manage")?;
    require_application(&state, &actor, id).await?;
    crate::canvas_plugin_api::require_workflow_version_plugins_enabled(
        &state,
        actor.tenant_id,
        input.workflow_version_id,
    )
    .await?;
    if !matches!(
        input.session_version_policy.as_str(),
        "pinned" | "follow_deployment" | "manual_upgrade"
    ) {
        return Err(ApiError::bad_request(
            "INVALID_SESSION_VERSION_POLICY",
            "Session version policy is invalid",
        ));
    }
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM applications a JOIN workflow_versions wv ON wv.workflow_id=a.workflow_id AND wv.tenant_id=a.tenant_id JOIN workflow_deployment_heads h ON h.tenant_id=a.tenant_id AND h.workflow_id=a.workflow_id AND h.environment_id=? JOIN workflow_deployments wd ON wd.id=h.active_deployment_id AND wd.workflow_version_id=wv.id WHERE a.id=? AND a.tenant_id=? AND wv.id=?)")
        .bind(input.environment_id).bind(id).bind(actor.tenant_id).bind(input.workflow_version_id).fetch_one(&state.pool).await?;
    if !valid {
        return Err(ApiError::unprocessable(
            "WORKFLOW_VERSION_NOT_DEPLOYED",
            "Workflow Version is not active in the selected Environment",
        ));
    }
    let definition: Value = sqlx::query_scalar(
        "SELECT definition_json FROM workflow_versions WHERE tenant_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(input.workflow_version_id)
    .fetch_one(&state.pool)
    .await?;
    let definition: agentx_domain::WorkflowDefinition =
        serde_json::from_value(definition).map_err(ApiError::internal)?;
    let deployment_id = Uuid::now_v7();
    let attempt_id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    let trigger_revision: u64 = sqlx::query_scalar(
        "SELECT runtime_config_revision FROM applications WHERE id=? AND tenant_id=? FOR UPDATE",
    )
    .bind(id)
    .bind(actor.tenant_id)
    .fetch_one(&mut *tx)
    .await?;
    let trigger_manifest_hash: Option<String> = sqlx::query_scalar("SELECT manifest_hash FROM application_runtime_trigger_revisions WHERE tenant_id=? AND application_id=? AND revision=?")
        .bind(actor.tenant_id).bind(id).bind(trigger_revision).fetch_optional(&mut *tx).await?;
    let sequence: u64 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(sequence_number),0)+1 AS UNSIGNED) FROM application_deployments WHERE tenant_id=? AND application_id=? FOR UPDATE")
        .bind(actor.tenant_id).bind(id).fetch_one(&mut *tx).await?;
    let (input_schema, output_schema) = application_webhooks::workflow_schemas(&definition);
    sqlx::query("INSERT INTO application_deployments(id,tenant_id,application_id,workflow_version_id,environment_id,sequence_number,input_schema_json,output_schema_json,session_version_policy,trigger_revision,trigger_manifest_hash,status,created_by) VALUES(?,?,?,?,?,?,?,?,?,?,?,'building',?)")
        .bind(deployment_id).bind(actor.tenant_id).bind(id).bind(input.workflow_version_id).bind(input.environment_id).bind(sequence)
        .bind(input_schema).bind(output_schema).bind(input.session_version_policy).bind(trigger_revision).bind(trigger_manifest_hash).bind(actor.user_id).execute(&mut *tx).await?;
    let expected: Option<u64> = sqlx::query_scalar(
        "SELECT version FROM application_deployment_heads WHERE tenant_id=? AND application_id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let epoch: u64 = sqlx::query_scalar(
        "SELECT current_epoch FROM admission_epochs WHERE tenant_id=? AND application_id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or(1);
    let activation_sequence = allocate_activation_sequence(&mut tx, actor.tenant_id, id).await?;
    sqlx::query("INSERT INTO publish_attempts(id,tenant_id,application_id,deployment_id,requested_action,state,next_action,idempotency_key,expected_head_version,activation_sequence,minimum_admission_epoch,created_by) VALUES(?,?,?,?,'publish','building','build',?,?,?,?,?)")
        .bind(attempt_id).bind(actor.tenant_id).bind(id).bind(deployment_id).bind(format!("application-publish:{deployment_id}"))
        .bind(expected).bind(activation_sequence).bind(epoch).bind(actor.user_id).execute(&mut *tx).await?;
    publish_outbox(
        &mut tx,
        actor.tenant_id,
        id,
        deployment_id,
        attempt_id,
        "publish",
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(load_deployment(&state, actor.tenant_id, deployment_id).await?),
    ))
}

const DEPLOYMENT_SELECT: &str = "SELECT ad.id,ad.application_id,ad.workflow_version_id,wv.version_number,ad.input_schema_json,ad.output_schema_json,ad.environment_id,e.name environment_name,ad.sequence_number,ad.session_version_policy,IF(pa.state IS NULL OR pa.state='active',ad.status,pa.state) status,ad.created_at,pa.id publish_attempt_id,pa.last_error_code publish_error_code,pa.last_error_message publish_error_message FROM application_deployments ad JOIN workflow_versions wv ON wv.id=ad.workflow_version_id JOIN workflow_environments e ON e.id=ad.environment_id LEFT JOIN publish_attempts pa ON pa.tenant_id=ad.tenant_id AND pa.deployment_id=ad.id AND pa.id=(SELECT pa2.id FROM publish_attempts pa2 WHERE pa2.tenant_id=ad.tenant_id AND pa2.deployment_id=ad.id ORDER BY pa2.created_at DESC LIMIT 1) WHERE ad.tenant_id=? AND ad.application_id=? ORDER BY ad.sequence_number DESC";

fn deployment_from_row(row: sqlx::mysql::MySqlRow) -> ApiResult<DeploymentResponse> {
    Ok(DeploymentResponse {
        id: row.try_get("id")?,
        application_id: row.try_get("application_id")?,
        workflow_version_id: row.try_get("workflow_version_id")?,
        workflow_version_number: row.try_get("version_number")?,
        environment_id: row.try_get("environment_id")?,
        environment_name: row.try_get("environment_name")?,
        sequence_number: row.try_get("sequence_number")?,
        input_schema: row.try_get("input_schema_json")?,
        output_schema: row.try_get("output_schema_json")?,
        session_version_policy: row.try_get("session_version_policy")?,
        status: row.try_get("status")?,
        publish_attempt_id: row.try_get("publish_attempt_id")?,
        publish_error_code: row.try_get("publish_error_code")?,
        publish_error_message: row.try_get("publish_error_message")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn load_deployment(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<DeploymentResponse> {
    let row = sqlx::query(&DEPLOYMENT_SELECT.replace("ad.application_id=?", "ad.id=?"))
        .bind(tenant)
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    deployment_from_row(row)
}

async fn list_schedules(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ScheduleResponse>>> {
    actor.require("application:view")?;
    require_application(&state, &actor, application_id).await?;
    let rows = sqlx::query("SELECT id,name,cron_expression,timezone,input_json,misfire_policy,status,version FROM application_schedules WHERE tenant_id=? AND application_id=? ORDER BY created_at DESC")
        .bind(actor.tenant_id).bind(application_id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(schedule_from_row)
            .collect::<ApiResult<_>>()?,
    ))
}

async fn create_schedule(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application_id): Path<Uuid>,
    Json(input): Json<CreateScheduleRequest>,
) -> ApiResult<(StatusCode, Json<ScheduleResponse>)> {
    actor.require("application:manage")?;
    validate_schedule(
        &input.cron_expression,
        &input.timezone,
        &input.misfire_policy,
    )?;
    let id = Uuid::now_v7();
    let configuration = json!({"cronExpression":input.cron_expression,"timezone":input.timezone,"input":input.input,"misfirePolicy":input.misfire_policy,"enabled":true});
    let hash =
        agentx_runtime_contracts::content_hash(&configuration).map_err(ApiError::internal)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO application_schedules(id,tenant_id,application_id,name,cron_expression,timezone,input_json,status,misfire_policy,configuration_revision,configuration_hash,created_by) VALUES(?,?,?,?,?,?,?,'active',?,1,?,?)")
        .bind(id).bind(actor.tenant_id).bind(application_id).bind(required_name(&input.name)?).bind(&input.cron_expression).bind(&input.timezone).bind(&input.input).bind(&input.misfire_policy).bind(hash.as_str()).bind(actor.user_id).execute(&mut *tx).await?;
    rebuild_trigger_revision(&mut tx, &actor, application_id).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_schedule(&state, actor.tenant_id, id).await?),
    ))
}

async fn update_schedule(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application_id, schedule_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<UpdateScheduleRequest>,
) -> ApiResult<Json<ScheduleResponse>> {
    actor.require("application:manage")?;
    validate_schedule(
        &input.cron_expression,
        &input.timezone,
        &input.misfire_policy,
    )?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_SCHEDULE_STATUS",
            "Schedule status is invalid",
        ));
    }
    let configuration = json!({"cronExpression":input.cron_expression,"timezone":input.timezone,"input":input.input,"misfirePolicy":input.misfire_policy,"enabled":input.status == "active"});
    let hash =
        agentx_runtime_contracts::content_hash(&configuration).map_err(ApiError::internal)?;
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("UPDATE application_schedules SET name=?,cron_expression=?,timezone=?,input_json=?,misfire_policy=?,status=?,configuration_revision=configuration_revision+1,configuration_hash=?,version=version+1 WHERE tenant_id=? AND application_id=? AND id=? AND version=?")
        .bind(required_name(&input.name)?).bind(&input.cron_expression).bind(&input.timezone).bind(&input.input).bind(&input.misfire_policy).bind(&input.status).bind(hash.as_str()).bind(actor.tenant_id).bind(application_id).bind(schedule_id).bind(input.version).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "SCHEDULE_VERSION_CONFLICT",
            "Schedule changed on the server",
        ));
    }
    rebuild_trigger_revision(&mut tx, &actor, application_id).await?;
    tx.commit().await?;
    Ok(Json(
        load_schedule(&state, actor.tenant_id, schedule_id).await?,
    ))
}

async fn delete_schedule(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application_id, schedule_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    actor.require("application:delete")?;
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("DELETE FROM application_schedules WHERE tenant_id=? AND application_id=? AND id=? AND version=?")
        .bind(actor.tenant_id).bind(application_id).bind(schedule_id).bind(query.expected_version).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "VERSION_CONFLICT",
            "Schedule changed before deletion",
        ));
    }
    rebuild_trigger_revision(&mut tx, &actor, application_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn schedule_from_row(row: sqlx::mysql::MySqlRow) -> ApiResult<ScheduleResponse> {
    Ok(ScheduleResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        cron_expression: row.try_get("cron_expression")?,
        timezone: row.try_get("timezone")?,
        input: row.try_get("input_json")?,
        misfire_policy: row.try_get("misfire_policy")?,
        status: row.try_get("status")?,
        version: row.try_get("version")?,
    })
}

async fn load_schedule(
    state: &ControlApiState,
    tenant_id: Uuid,
    id: Uuid,
) -> ApiResult<ScheduleResponse> {
    let row = sqlx::query("SELECT id,name,cron_expression,timezone,input_json,misfire_policy,status,version FROM application_schedules WHERE tenant_id=? AND id=?")
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Schedule"))?;
    schedule_from_row(row)
}

async fn list_sessions(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application_id): Path<Uuid>,
) -> ApiResult<Json<Vec<SessionResponse>>> {
    actor.require("application:view")?;
    require_application(&state, &actor, application_id).await?;
    let request = SessionSearchRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        application_id: Some(application_id),
        statuses: Vec::new(),
        after: None,
        limit: 100,
    };
    let token = delegation_token(
        &state,
        &actor,
        BTreeSet::from(["runtime.query.sessions".into()]),
        BTreeSet::from([application_id]),
        BTreeSet::new(),
        content_hash(&json!({"operation":"session-search","request":request}))
            .map_err(ApiError::internal)?,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/query/sessions:search",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_query_unavailable)?;
    let page: SessionSearchPageV1 = runtime_json(response, "RUNTIME_QUERY_REJECTED").await?;
    Ok(Json(page.items.into_iter().map(session_response).collect()))
}

async fn upgrade_session(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(session_id): Path<Uuid>,
    Json(input): Json<UpgradeSessionRequest>,
) -> ApiResult<Json<SessionResponse>> {
    actor.require("application:manage")?;
    let detail = get_runtime_session(&state, &actor, session_id).await?;
    let application_id = detail.summary.application_id;
    require_application(&state, &actor, application_id).await?;
    let target_bundle_id: Uuid = sqlx::query_scalar("SELECT id FROM execution_spec_bundles WHERE tenant_id=? AND application_id=? AND workflow_version_id=? AND status='published' ORDER BY sequence_number DESC LIMIT 1")
        .bind(actor.tenant_id).bind(application_id).bind(input.workflow_version_id)
        .fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::unprocessable("WORKFLOW_VERSION_NOT_PUBLISHED", "Workflow Version has no published Runtime Bundle"))?;
    let command = SessionUpgradeCommandV1 {
        api_version: 1,
        idempotency_key: format!(
            "session-upgrade:{session_id}:{}:{target_bundle_id}",
            input.version
        ),
        tenant_id: actor.tenant_id,
        session_id,
        application_id,
        expected_session_version: input.version,
        target_bundle_id,
        actor_user_id: actor.user_id,
    };
    let token = delegation_token(
        &state,
        &actor,
        BTreeSet::from(["runtime.sessions.upgrade".into()]),
        BTreeSet::from([application_id]),
        BTreeSet::from([session_id]),
        content_hash(&json!({"operation":"session_upgrade","request":command}))
            .map_err(ApiError::internal)?,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/session-commands:apply",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&command)
        .send()
        .await
        .map_err(runtime_query_unavailable)?;
    let _: agentx_runtime_contracts::SessionUpgradeReceiptV1 =
        runtime_json(response, "RUNTIME_SESSION_COMMAND_REJECTED").await?;
    Ok(Json(session_response(
        get_runtime_session(&state, &actor, session_id)
            .await?
            .summary,
    )))
}

async fn get_runtime_session(
    state: &ControlApiState,
    actor: &Actor,
    session_id: Uuid,
) -> ApiResult<SessionDetailV1> {
    let token = delegation_token(
        state,
        actor,
        BTreeSet::from(["runtime.query.sessions".into()]),
        BTreeSet::new(),
        BTreeSet::from([session_id]),
        content_hash(&json!({"operation":"get_session","sessionId":session_id}))
            .map_err(ApiError::internal)?,
    )?;
    let response = state
        .http
        .get(format!(
            "{}/internal/runtime/v1/query/sessions/{session_id}",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(runtime_query_unavailable)?;
    runtime_json(response, "RUNTIME_QUERY_REJECTED").await
}

fn session_response(value: agentx_runtime_contracts::SessionSummaryV1) -> SessionResponse {
    SessionResponse {
        id: value.session_id,
        application_id: value.application_id,
        application_deployment_id: value.application_deployment_id,
        workflow_version_id: value.workflow_version_id,
        version_policy: match value.version_policy {
            agentx_runtime_contracts::SessionVersionPolicyV1::Pinned => "pinned",
            agentx_runtime_contracts::SessionVersionPolicyV1::FollowDeployment => {
                "follow_deployment"
            }
            agentx_runtime_contracts::SessionVersionPolicyV1::ManualUpgrade => "manual_upgrade",
        }
        .into(),
        external_user_id: value.external_user_id,
        title: value.title,
        status: value.status,
        version: value.version,
        updated_at: value.updated_at,
    }
}

fn delegation_token(
    state: &ControlApiState,
    actor: &Actor,
    scope: BTreeSet<String>,
    application_ids: BTreeSet<Uuid>,
    session_ids: BTreeSet<Uuid>,
    request_hash: ContentHash,
) -> ApiResult<String> {
    let now = now_unix();
    issue_delegation_token(
        &state.delegation_kid,
        state.delegation_key.expose_secret().as_bytes(),
        &DelegationClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: actor.user_id,
            tenant_id: actor.tenant_id,
            token_version: actor.token_version,
            tenant_wide: false,
            scope,
            application_ids,
            workflow_ids: BTreeSet::new(),
            execution_ids: BTreeSet::new(),
            session_ids,
            request_hash,
            iat: now,
            exp: now + DELEGATION_TOKEN_TTL_SECONDS,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)
}

fn runtime_query_unavailable(error: reqwest::Error) -> ApiError {
    tracing::warn!(%error, "Runtime Internal API is unavailable");
    ApiError::unavailable("RUNTIME_UNAVAILABLE", "Runtime service is unavailable")
}

async fn runtime_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    code: &'static str,
) -> ApiResult<T> {
    if !response.status().is_success() {
        tracing::warn!(status=%response.status(), "Runtime Internal API rejected request");
        return Err(ApiError::conflict(
            code,
            "Runtime rejected the delegated request",
        ));
    }
    response.json().await.map_err(ApiError::internal)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishAttemptResponse {
    id: Uuid,
    deployment_id: Uuid,
    bundle_id: Option<Uuid>,
    previous_attempt_id: Option<Uuid>,
    state: String,
    next_action: String,
    activation_sequence: u64,
    error_code: Option<String>,
    error_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

async fn get_publish_attempt(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, attempt)): Path<(Uuid, String)>,
) -> ApiResult<Json<PublishAttemptResponse>> {
    actor.require("application:view")?;
    let attempt = parse_action_id(&attempt, None, "INVALID_PUBLISH_ATTEMPT_ACTION")?;
    Ok(Json(
        load_attempt(&state, actor.tenant_id, id, attempt).await?,
    ))
}

async fn retry_publish_attempt(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, attempt)): Path<(Uuid, String)>,
) -> ApiResult<(StatusCode, Json<PublishAttemptResponse>)> {
    actor.require("application:manage")?;
    let attempt = parse_action_id(&attempt, Some("retry"), "INVALID_PUBLISH_ATTEMPT_ACTION")?;
    let mut tx = state.pool.begin().await?;
    let activation_sequence = allocate_activation_sequence(&mut tx, actor.tenant_id, id).await?;
    let old = sqlx::query("SELECT deployment_id,bundle_id,activation_sequence,minimum_admission_epoch,expected_head_version FROM publish_attempts WHERE tenant_id=? AND application_id=? AND id=? AND state='rejected' FOR UPDATE")
        .bind(actor.tenant_id).bind(id).bind(attempt).fetch_optional(&mut *tx).await?.ok_or_else(|| ApiError::conflict("PUBLISH_ATTEMPT_NOT_RETRYABLE", "Publish Attempt is not rejected"))?;
    let retry = Uuid::now_v7();
    let deployment_id: Uuid = old.try_get("deployment_id")?;
    let bundle_id: Option<Uuid> = old.try_get("bundle_id")?;
    if let Some(bundle_id) = bundle_id {
        crate::canvas_plugin_api::require_bundle_plugins_enabled(
            &state,
            actor.tenant_id,
            bundle_id,
        )
        .await?;
    }
    sqlx::query("INSERT INTO publish_attempts(id,tenant_id,application_id,deployment_id,bundle_id,previous_attempt_id,requested_action,state,next_action,idempotency_key,expected_head_version,activation_sequence,minimum_admission_epoch,created_by) VALUES(?,?,?,?,?,?,'retry','building','build',?,?,?,?,?)")
        .bind(retry).bind(actor.tenant_id).bind(id).bind(deployment_id).bind(bundle_id).bind(attempt)
        .bind(format!("publish-retry:{retry}")).bind(old.try_get::<Option<u64>,_>("expected_head_version")?).bind(activation_sequence).bind(old.try_get::<u64,_>("minimum_admission_epoch")?).bind(actor.user_id).execute(&mut *tx).await?;
    publish_outbox(&mut tx, actor.tenant_id, id, deployment_id, retry, "retry").await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(load_attempt(&state, actor.tenant_id, id, retry).await?),
    ))
}

async fn rollback_deployment(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, deployment)): Path<(Uuid, String)>,
) -> ApiResult<(StatusCode, Json<PublishAttemptResponse>)> {
    actor.require("application:manage")?;
    let deployment = parse_action_id(&deployment, Some("rollback"), "INVALID_DEPLOYMENT_ACTION")?;
    let mut tx = state.pool.begin().await?;
    let sequence = allocate_activation_sequence(&mut tx, actor.tenant_id, id).await?;
    let bundle: Uuid = sqlx::query_scalar("SELECT id FROM execution_spec_bundles WHERE tenant_id=? AND application_id=? AND deployment_id=? AND status='published' FOR UPDATE")
        .bind(actor.tenant_id).bind(id).bind(deployment).fetch_optional(&mut *tx).await?.ok_or_else(|| ApiError::conflict("BUNDLE_NOT_ROLLBACKABLE", "Deployment Bundle is not published"))?;
    crate::canvas_plugin_api::require_bundle_plugins_enabled(&state, actor.tenant_id, bundle)
        .await?;
    let attempt = Uuid::now_v7();
    let epoch: u64 = sqlx::query_scalar(
        "SELECT current_epoch FROM admission_epochs WHERE tenant_id=? AND application_id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or(1);
    let head: Option<u64> = sqlx::query_scalar(
        "SELECT version FROM application_deployment_heads WHERE tenant_id=? AND application_id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO publish_attempts(id,tenant_id,application_id,deployment_id,bundle_id,requested_action,state,next_action,idempotency_key,expected_head_version,activation_sequence,minimum_admission_epoch,created_by) VALUES(?,?,?,?,?,'rollback','prepared','activate',?,?,?,?,?)")
        .bind(attempt).bind(actor.tenant_id).bind(id).bind(deployment).bind(bundle).bind(format!("rollback:{attempt}"))
        .bind(head).bind(sequence).bind(epoch).bind(actor.user_id).execute(&mut *tx).await?;
    publish_outbox(
        &mut tx,
        actor.tenant_id,
        id,
        deployment,
        attempt,
        "rollback",
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(load_attempt(&state, actor.tenant_id, id, attempt).await?),
    ))
}

async fn load_attempt(
    state: &ControlApiState,
    tenant: Uuid,
    application: Uuid,
    id: Uuid,
) -> ApiResult<PublishAttemptResponse> {
    let row = sqlx::query("SELECT id,deployment_id,bundle_id,previous_attempt_id,state,next_action,activation_sequence,last_error_code,last_error_message,updated_at FROM publish_attempts WHERE tenant_id=? AND application_id=? AND id=?")
        .bind(tenant).bind(application).bind(id).fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::not_found("Publish Attempt"))?;
    Ok(PublishAttemptResponse {
        id: row.try_get("id")?,
        deployment_id: row.try_get("deployment_id")?,
        bundle_id: row.try_get("bundle_id")?,
        previous_attempt_id: row.try_get("previous_attempt_id")?,
        state: row.try_get("state")?,
        next_action: row.try_get("next_action")?,
        activation_sequence: row.try_get("activation_sequence")?,
        error_code: row.try_get("last_error_code")?,
        error_message: row.try_get("last_error_message")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeyResponse {
    id: Uuid,
    family_id: Uuid,
    name: String,
    prefix: String,
    status: String,
    secret: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    last_used_at: Option<OffsetDateTime>,
}

#[derive(Deserialize)]
struct CreateApiKeyRequest {
    name: String,
}

async fn list_api_keys(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<ApiKeyResponse>>> {
    actor.require("application:manage_key")?;
    require_application(&state, &actor, id).await?;
    let rows = sqlx::query("SELECT id,family_id,name,key_prefix,status,created_at,last_used_at FROM application_api_keys WHERE tenant_id=? AND application_id=? ORDER BY created_at DESC")
        .bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(api_key_from_row)
            .collect::<ApiResult<_>>()?,
    ))
}

async fn create_api_key(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<CreateApiKeyRequest>,
) -> ApiResult<(StatusCode, Json<ApiKeyResponse>)> {
    actor.require("application:manage_key")?;
    require_application(&state, &actor, id).await?;
    let (key, secret, prefix, hash) = generate_api_key();
    let family = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO application_api_keys(id,tenant_id,application_id,family_id,name,key_prefix,secret_hash,created_by) VALUES(?,?,?,?,?,?,?,?)")
        .bind(key).bind(actor.tenant_id).bind(id).bind(family).bind(input.name.trim()).bind(&prefix).bind(hash.as_slice()).bind(actor.user_id).execute(&mut *tx).await?;
    admission_outbox(&mut tx, actor.tenant_id, id, "ApiKeyAdmissionChanged", json!({"applicationId":id,"keyId":key,"keyName":input.name.trim(),"status":"active","keyPrefix":prefix,"secretHash":format!("sha256:{}",hex(&hash))})).await?;
    tx.commit().await?;
    let mut response = load_api_key(&state, actor.tenant_id, key).await?;
    response.secret = Some(secret);
    Ok((StatusCode::CREATED, Json(response)))
}

async fn rotate_api_key(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, key)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ApiKeyResponse>> {
    actor.require("application:manage_key")?;
    let mut tx = state.pool.begin().await?;
    let old = sqlx::query("SELECT family_id,name,key_prefix,secret_hash FROM application_api_keys WHERE id=? AND application_id=? AND tenant_id=? AND status='active' FOR UPDATE")
        .bind(key).bind(id).bind(actor.tenant_id).fetch_optional(&mut *tx).await?.ok_or_else(|| ApiError::not_found("API key"))?;
    let (new_key, secret, prefix, hash) = generate_api_key();
    sqlx::query(
        "UPDATE application_api_keys SET status='revoked',revoked_at=UTC_TIMESTAMP(6) WHERE id=?",
    )
    .bind(key)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO application_api_keys(id,tenant_id,application_id,family_id,name,key_prefix,secret_hash,created_by) VALUES(?,?,?,?,?,?,?,?)")
        .bind(new_key).bind(actor.tenant_id).bind(id).bind(old.try_get::<Uuid,_>("family_id")?).bind(old.try_get::<String,_>("name")?).bind(&prefix).bind(hash.as_slice()).bind(actor.user_id).execute(&mut *tx).await?;
    admission_outbox(&mut tx, actor.tenant_id, id, "ApiKeyAdmissionChanged", json!({"applicationId":id,"keyId":key,"keyName":old.try_get::<String,_>("name")?,"status":"revoked","keyPrefix":old.try_get::<String,_>("key_prefix")?,"secretHash":format!("sha256:{}",hex(&old.try_get::<Vec<u8>,_>("secret_hash")?))})).await?;
    admission_outbox(&mut tx, actor.tenant_id, id, "ApiKeyAdmissionChanged", json!({"applicationId":id,"keyId":new_key,"keyName":old.try_get::<String,_>("name")?,"status":"active","keyPrefix":prefix,"secretHash":format!("sha256:{}",hex(&hash))})).await?;
    tx.commit().await?;
    let mut response = load_api_key(&state, actor.tenant_id, new_key).await?;
    response.secret = Some(secret);
    Ok(Json(response))
}

async fn revoke_api_key(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, key)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    actor.require("application:manage_key")?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT name,key_prefix,secret_hash FROM application_api_keys WHERE id=? AND application_id=? AND tenant_id=? AND status='active' FOR UPDATE")
        .bind(key).bind(id).bind(actor.tenant_id).fetch_optional(&mut *tx).await?.ok_or_else(|| ApiError::not_found("API key"))?;
    sqlx::query(
        "UPDATE application_api_keys SET status='revoked',revoked_at=UTC_TIMESTAMP(6) WHERE id=?",
    )
    .bind(key)
    .execute(&mut *tx)
    .await?;
    admission_outbox(&mut tx, actor.tenant_id, id, "ApiKeyAdmissionChanged", json!({"applicationId":id,"keyId":key,"status":"revoked","keyPrefix":row.try_get::<String,_>("key_prefix")?,"secretHash":format!("sha256:{}",hex(&row.try_get::<Vec<u8>,_>("secret_hash")?))})).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn admission_outbox(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    application: Uuid,
    event: &str,
    mut payload: Value,
) -> ApiResult<u64> {
    sqlx::query("INSERT INTO admission_epochs(tenant_id,application_id,current_epoch) VALUES(?,?,1) ON DUPLICATE KEY UPDATE current_epoch=current_epoch+1")
        .bind(tenant).bind(application).execute(&mut **tx).await?;
    let epoch: u64 = sqlx::query_scalar("SELECT current_epoch FROM admission_epochs WHERE tenant_id=? AND application_id=? FOR UPDATE")
        .bind(tenant).bind(application).fetch_one(&mut **tx).await?;
    payload["admissionEpoch"] = json!(epoch);
    let request_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&payload).map_err(ApiError::internal)?)
    );
    sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,?,?,?,?,'pending',?,?)")
        .bind(Uuid::now_v7()).bind(tenant).bind(event).bind("application_admission").bind(application.to_string()).bind(payload)
        .bind(request_hash).bind(format!("admission:{application}:{epoch}:{event}")).execute(&mut **tx).await?;
    Ok(epoch)
}

async fn publish_outbox(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant_id: Uuid,
    application_id: Uuid,
    deployment_id: Uuid,
    attempt_id: Uuid,
    action: &str,
) -> ApiResult<()> {
    let payload = json!({
        "applicationId": application_id,
        "deploymentId": deployment_id,
        "attemptId": attempt_id,
        "action": action,
    });
    let request_hash =
        agentx_runtime_contracts::content_hash(&payload).map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,?,?,?,?,'pending',?,?)")
        .bind(Uuid::now_v7())
        .bind(tenant_id)
        .bind("BundlePublishRequested")
        .bind("bundle_publish")
        .bind(attempt_id.to_string())
        .bind(payload)
        .bind(request_hash.as_str())
        .bind(format!("bundle-publish:{attempt_id}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn api_key_from_row(row: sqlx::mysql::MySqlRow) -> ApiResult<ApiKeyResponse> {
    Ok(ApiKeyResponse {
        id: row.try_get("id")?,
        family_id: row.try_get("family_id")?,
        name: row.try_get("name")?,
        prefix: row.try_get("key_prefix")?,
        status: row.try_get("status")?,
        secret: None,
        created_at: row.try_get("created_at")?,
        last_used_at: row.try_get("last_used_at")?,
    })
}

async fn load_api_key(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<ApiKeyResponse> {
    api_key_from_row(sqlx::query("SELECT id,family_id,name,key_prefix,status,created_at,last_used_at FROM application_api_keys WHERE tenant_id=? AND id=?").bind(tenant).bind(id).fetch_one(&state.pool).await?)
}
