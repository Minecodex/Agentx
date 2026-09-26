use std::collections::BTreeSet;

use agentx_api_types::PageResponse;
use agentx_runtime_contracts::{
    ControlRole, RuntimeMcpTransportV2, RuntimeResourceBindingV1, RuntimeResourceCheckRequestV1,
    RuntimeResourceCheckResponseV1, RuntimeResourceOperationRequestV1,
    RuntimeResourceOperationResponseV1, RuntimeResourceOperationV1, RuntimeResourceProbeV1,
    ServiceClaimsV1, VaultSecretReferenceV1, issue_service_token, now_unix,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use reqwest::Url;
use rust_decimal::Decimal;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction, mysql::MySqlRow};
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
            "/api/v1/models/aliases",
            get(list_models).post(create_model),
        )
        .route(
            "/api/v1/models/aliases/{id}",
            get(get_model).patch(update_model).delete(delete_model),
        )
        .route(
            "/api/v1/models/aliases/{id}/deployment-history",
            get(list_history),
        )
        .route(
            "/api/v1/models/aliases/{id}/test-connection",
            post(test_model),
        )
        .route(
            "/api/v1/models/deployments/{id}/prices",
            get(list_prices).post(create_price),
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
struct ModelResponse {
    id: Uuid,
    alias: String,
    status: String,
    alias_version: u64,
    deployment_id: Uuid,
    connection_name: String,
    provider_type: String,
    endpoint: String,
    credential_id: Option<Uuid>,
    owner_department_id: Uuid,
    model_name: String,
    max_input_tokens: u64,
    max_output_tokens: u64,
    default_parameters: Value,
    #[serde(default)]
    capabilities: Vec<String>,
    revision_number: u64,
    connection_status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    connection_checked_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    connection_name: String,
    provider_type: String,
    endpoint: String,
    credential_id: Option<Uuid>,
    owner_department_id: Uuid,
    #[serde(default = "default_model_name")]
    alias: String,
    #[serde(default = "default_model_name")]
    model_name: String,
    #[serde(default = "default_max_input_tokens")]
    max_input_tokens: u64,
    #[serde(default = "default_max_output_tokens")]
    max_output_tokens: u64,
    #[serde(default = "default_parameters")]
    default_parameters: Value,
    #[serde(default)]
    capabilities: Vec<String>,
    price: PriceRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRequest {
    connection_name: String,
    provider_type: String,
    endpoint: String,
    credential_id: Option<Uuid>,
    owner_department_id: Uuid,
    alias: String,
    status: String,
    model_name: String,
    max_input_tokens: u64,
    max_output_tokens: u64,
    default_parameters: Value,
    #[serde(default)]
    capabilities: Vec<String>,
    expected_alias_version: u64,
    price: PriceRequest,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryResponse {
    id: Uuid,
    previous_deployment_id: Option<Uuid>,
    deployment_id: Uuid,
    revision_number: u64,
    connection_name: String,
    model_name: String,
    changed_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    changed_at: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PriceRequest {
    currency: String,
    input_per_million: String,
    output_per_million: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PriceResponse {
    id: Uuid,
    deployment_id: Uuid,
    version_number: u64,
    currency: String,
    input_per_million: String,
    output_per_million: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    status: String,
    latency_ms: Option<u64>,
    error_code: Option<String>,
    error_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    checked_at: OffsetDateTime,
}

async fn list_models(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<ModelResponse>>> {
    actor.require("model:view")?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let status = query.status.unwrap_or_default();
    let administrator = actor.roles.iter().any(|role| role == "company_admin");
    let (rows, total) = if administrator {
        let rows=sqlx::query("SELECT a.id,a.alias,a.status,a.version alias_version,a.updated_at,d.id deployment_id,d.connection_name,d.provider_type,d.endpoint,d.credential_id,d.owner_department_id,d.model_name,d.max_input_tokens,d.max_output_tokens,d.default_parameters,d.capabilities_json,d.revision_number,COALESCE((SELECT h.status FROM resource_health_checks h WHERE h.tenant_id=a.tenant_id AND h.resource_type='model' AND h.resource_id=a.id AND h.checked_at>=GREATEST(a.updated_at,d.updated_at) ORDER BY h.check_sequence DESC LIMIT 1),'untested') connection_status,(SELECT h.checked_at FROM resource_health_checks h WHERE h.tenant_id=a.tenant_id AND h.resource_type='model' AND h.resource_id=a.id AND h.checked_at>=GREATEST(a.updated_at,d.updated_at) ORDER BY h.check_sequence DESC LIMIT 1) connection_checked_at FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND (?='' OR a.status=?) AND (?='%%' OR a.alias LIKE ? OR d.connection_name LIKE ? OR d.model_name LIKE ?) ORDER BY a.updated_at DESC,a.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND (?='' OR a.status=?) AND (?='%%' OR a.alias LIKE ? OR d.connection_name LIKE ? OR d.model_name LIKE ?)").bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    } else {
        let rows=sqlx::query("SELECT a.id,a.alias,a.status,a.version alias_version,a.updated_at,d.id deployment_id,d.connection_name,d.provider_type,d.endpoint,d.credential_id,d.owner_department_id,d.model_name,d.max_input_tokens,d.max_output_tokens,d.default_parameters,d.capabilities_json,d.revision_number,COALESCE((SELECT h.status FROM resource_health_checks h WHERE h.tenant_id=a.tenant_id AND h.resource_type='model' AND h.resource_id=a.id AND h.checked_at>=GREATEST(a.updated_at,d.updated_at) ORDER BY h.check_sequence DESC LIMIT 1),'untested') connection_status,(SELECT h.checked_at FROM resource_health_checks h WHERE h.tenant_id=a.tenant_id AND h.resource_type='model' AND h.resource_id=a.id AND h.checked_at>=GREATEST(a.updated_at,d.updated_at) ORDER BY h.check_sequence DESC LIMIT 1) connection_checked_at FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=a.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=d.owner_department_id) AND (?='' OR a.status=?) AND (?='%%' OR a.alias LIKE ? OR d.connection_name LIKE ? OR d.model_name LIKE ?) ORDER BY a.updated_at DESC,a.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND EXISTS(SELECT 1 FROM department_closure dc WHERE dc.tenant_id=a.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=d.owner_department_id) AND (?='' OR a.status=?) AND (?='%%' OR a.alias LIKE ? OR d.connection_name LIKE ? OR d.model_name LIKE ?)").bind(actor.tenant_id).bind(actor.department_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(&search).bind(&search).fetch_one(&state.pool).await?;
        (rows, total)
    };
    Ok(Json(PageResponse {
        items: rows
            .into_iter()
            .map(model_from_row)
            .collect::<Result<_, _>>()?,
        page,
        page_size,
        total: total.try_into().unwrap_or_default(),
    }))
}

async fn get_model(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ModelResponse>> {
    actor.require("model:view")?;
    require_visible(&state, &actor, id).await?;
    Ok(Json(load_model(&state, actor.tenant_id, id).await?))
}

async fn create_model(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateRequest>,
) -> ApiResult<(StatusCode, Json<ModelResponse>)> {
    actor.require("model:manage")?;
    validate_model_input(
        &input.provider_type,
        &input.endpoint,
        input.max_input_tokens,
        input.max_output_tokens,
        &input.price,
    )?;
    require_department_scope(&state, &actor, input.owner_department_id).await?;
    require_credential(&state, actor.tenant_id, input.credential_id).await?;
    let id = Uuid::now_v7();
    let deployment = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    insert_deployment(
        &mut tx,
        &actor,
        deployment,
        None,
        1,
        &input.connection_name,
        &input.provider_type,
        &input.endpoint,
        input.credential_id,
        input.owner_department_id,
        &input.model_name,
        input.max_input_tokens,
        input.max_output_tokens,
        &input.default_parameters,
        &input.capabilities,
    )
    .await?;
    sqlx::query("INSERT INTO model_aliases(id,tenant_id,alias,deployment_id) VALUES(?,?,?,?)")
        .bind(id)
        .bind(actor.tenant_id)
        .bind(validate_alias(&input.alias)?)
        .bind(deployment)
        .execute(&mut *tx)
        .await
        .map_err(map_alias_error)?;
    insert_history(&mut tx, &actor, id, None, deployment).await?;
    insert_price(&mut tx, &actor, deployment, 1, &input.price).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_model(&state, actor.tenant_id, id).await?),
    ))
}

async fn update_model(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateRequest>,
) -> ApiResult<Json<ModelResponse>> {
    actor.require("model:manage")?;
    require_visible(&state, &actor, id).await?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_STATUS",
            "Model status is invalid",
        ));
    }
    validate_model_input(
        &input.provider_type,
        &input.endpoint,
        input.max_input_tokens,
        input.max_output_tokens,
        &input.price,
    )?;
    require_department_scope(&state, &actor, input.owner_department_id).await?;
    require_credential(&state, actor.tenant_id, input.credential_id).await?;
    let mut tx = state.pool.begin().await?;
    let row=sqlx::query("SELECT a.deployment_id,a.version,d.revision_number FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND a.id=? FOR UPDATE").bind(actor.tenant_id).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Model"))?;
    if row.try_get::<u64, _>("version")? != input.expected_alias_version {
        return Err(ApiError::conflict(
            "MODEL_VERSION_CONFLICT",
            "Model changed",
        ));
    }
    let previous: Uuid = row.try_get("deployment_id")?;
    let revision = row
        .try_get::<u64, _>("revision_number")?
        .checked_add(1)
        .ok_or_else(|| ApiError::internal("model revision exhausted"))?;
    let deployment = Uuid::now_v7();
    insert_deployment(
        &mut tx,
        &actor,
        deployment,
        Some(previous),
        revision,
        &input.connection_name,
        &input.provider_type,
        &input.endpoint,
        input.credential_id,
        input.owner_department_id,
        &input.model_name,
        input.max_input_tokens,
        input.max_output_tokens,
        &input.default_parameters,
        &input.capabilities,
    )
    .await?;
    insert_price(&mut tx, &actor, deployment, 1, &input.price).await?;
    let changed=sqlx::query("UPDATE model_aliases SET alias=?,status=?,deployment_id=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?").bind(validate_alias(&input.alias)?).bind(input.status).bind(deployment).bind(actor.tenant_id).bind(id).bind(input.expected_alias_version).execute(&mut *tx).await.map_err(map_alias_error)?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "MODEL_VERSION_CONFLICT",
            "Model changed",
        ));
    }
    insert_history(&mut tx, &actor, id, Some(previous), deployment).await?;
    tx.commit().await?;
    Ok(Json(load_model(&state, actor.tenant_id, id).await?))
}

async fn list_history(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<HistoryResponse>>> {
    actor.require("model:view")?;
    require_visible(&state, &actor, id).await?;
    let rows=sqlx::query("SELECT h.id,h.previous_deployment_id,h.deployment_id,d.revision_number,d.connection_name,d.model_name,h.changed_by,h.changed_at FROM model_alias_deployment_history h JOIN model_deployments d ON d.tenant_id=h.tenant_id AND d.id=h.deployment_id WHERE h.tenant_id=? AND h.alias_id=? ORDER BY h.changed_at DESC,h.id DESC").bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                Ok(HistoryResponse {
                    id: row.try_get("id")?,
                    previous_deployment_id: row.try_get("previous_deployment_id")?,
                    deployment_id: row.try_get("deployment_id")?,
                    revision_number: row.try_get("revision_number")?,
                    connection_name: row.try_get("connection_name")?,
                    model_name: row.try_get("model_name")?,
                    changed_by: row.try_get("changed_by")?,
                    changed_at: row.try_get("changed_at")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?,
    ))
}

async fn create_price(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<PriceRequest>,
) -> ApiResult<(StatusCode, Json<PriceResponse>)> {
    actor.require("model:manage")?;
    require_deployment_visible(&state, &actor, id).await?;
    validate_price(&input)?;
    let mut tx = state.pool.begin().await?;
    let next:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(version_number),0)+1 AS UNSIGNED) FROM model_price_versions WHERE tenant_id=? AND deployment_id=?").bind(actor.tenant_id).bind(id).fetch_one(&mut *tx).await?;
    let price_id = insert_price(&mut tx, &actor, id, next, &input).await?;
    tx.commit().await?;
    let row=sqlx::query("SELECT id,deployment_id,version_number,currency,CAST(input_per_million AS CHAR) input_per_million,CAST(output_per_million AS CHAR) output_per_million,created_at FROM model_price_versions WHERE tenant_id=? AND id=?").bind(actor.tenant_id).bind(price_id).fetch_one(&state.pool).await?;
    Ok((StatusCode::CREATED, Json(price_from_row(row)?)))
}

async fn list_prices(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<PriceResponse>>> {
    actor.require("model:view")?;
    require_deployment_visible(&state, &actor, id).await?;
    let rows=sqlx::query("SELECT id,deployment_id,version_number,currency,CAST(input_per_million AS CHAR) input_per_million,CAST(output_per_million AS CHAR) output_per_million,created_at FROM model_price_versions WHERE tenant_id=? AND deployment_id=? ORDER BY version_number DESC").bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(price_from_row)
            .collect::<Result<_, _>>()?,
    ))
}

async fn test_model(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<HealthResponse>> {
    actor.require("model:manage")?;
    require_visible(&state, &actor, id).await?;
    let row=sqlx::query("SELECT d.endpoint,d.credential_id,d.model_name FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND a.id=? AND a.status='active' AND d.status='active'").bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Model"))?;
    let endpoint: String = row.try_get("endpoint")?;
    let credential = row.try_get("credential_id")?;
    let model: String = row.try_get("model_name")?;
    let check = execute_runtime_resource_check(
        &state,
        actor.tenant_id,
        endpoint,
        credential,
        RuntimeResourceProbeV1::ModelChat { model },
    )
    .await?;
    record_health(
        &state,
        &actor,
        id,
        &check.status,
        check.latency_ms,
        check.error_code.as_deref(),
        check.error_message.as_deref(),
    )
    .await?;
    Ok(Json(HealthResponse {
        status: check.status,
        latency_ms: Some(check.latency_ms),
        error_code: check.error_code,
        error_message: check.error_message,
        checked_at: check.checked_at,
    }))
}

async fn delete_model(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("model:manage")?;
    require_visible(&state, &actor, id).await?;
    let references:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM workflow_draft_resources WHERE tenant_id=? AND resource_type='model' AND resource_id=?)+(SELECT COUNT(*) FROM workflow_version_resources WHERE tenant_id=? AND resource_type='model' AND resource_id=?)").bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if references != 0 {
        return Err(ApiError::conflict(
            "MODEL_REFERENCED",
            "Model is referenced",
        ));
    }
    let deployments = sqlx::query(
        "SELECT deployment_id FROM model_alias_deployment_history WHERE tenant_id=? AND alias_id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM model_alias_deployment_history WHERE tenant_id=? AND alias_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM model_aliases WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for row in deployments {
        let deployment: Uuid = row.try_get("deployment_id")?;
        sqlx::query("DELETE FROM model_price_versions WHERE tenant_id=? AND deployment_id=?")
            .bind(actor.tenant_id)
            .bind(deployment)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM model_deployments WHERE tenant_id=? AND id=?")
            .bind(actor.tenant_id)
            .bind(deployment)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[allow(clippy::too_many_arguments)]
async fn insert_deployment(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    id: Uuid,
    supersedes: Option<Uuid>,
    revision: u64,
    connection_name: &str,
    provider: &str,
    endpoint: &str,
    credential: Option<Uuid>,
    department: Uuid,
    model_name: &str,
    max_input: u64,
    max_output: u64,
    parameters: &Value,
    capabilities: &[String],
) -> ApiResult<()> {
    let capabilities = validate_capabilities(capabilities)?;
    sqlx::query("INSERT INTO model_deployments(id,tenant_id,connection_name,provider_type,endpoint,credential_id,owner_department_id,model_name,max_input_tokens,max_output_tokens,default_parameters,capabilities_json,revision_number,supersedes_deployment_id) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(required_name(connection_name)?).bind(provider).bind(endpoint).bind(credential).bind(department).bind(required_name(model_name)?).bind(max_input).bind(max_output).bind(parameters).bind(serde_json::to_value(&capabilities).map_err(ApiError::internal)?).bind(revision).bind(supersedes).execute(&mut **tx).await?;
    Ok(())
}
async fn insert_history(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    alias: Uuid,
    previous: Option<Uuid>,
    deployment: Uuid,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO model_alias_deployment_history(id,tenant_id,alias_id,previous_deployment_id,deployment_id,changed_by) VALUES(?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(alias).bind(previous).bind(deployment).bind(actor.user_id).execute(&mut **tx).await?;
    Ok(())
}
async fn insert_price(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    deployment: Uuid,
    version: u64,
    input: &PriceRequest,
) -> ApiResult<Uuid> {
    validate_price(input)?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO model_price_versions(id,tenant_id,deployment_id,version_number,currency,input_per_million,output_per_million,created_by) VALUES(?,?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(deployment).bind(version).bind(input.currency.to_ascii_uppercase()).bind(&input.input_per_million).bind(&input.output_per_million).bind(actor.user_id).execute(&mut **tx).await?;
    Ok(id)
}
async fn record_health(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
    status: &str,
    latency: u64,
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> ApiResult<()> {
    let next:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(check_sequence),0)+1 AS UNSIGNED) FROM resource_health_checks WHERE tenant_id=? AND resource_type='model' AND resource_id=?").bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    sqlx::query("INSERT INTO resource_health_checks(id,tenant_id,resource_type,resource_id,check_sequence,status,latency_ms,error_code,error_message,checked_by) VALUES(?,?,'model',?,?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(next).bind(status).bind(latency).bind(error_code).bind(error_message).bind(actor.user_id).execute(&state.pool).await?;
    Ok(())
}

pub(crate) async fn execute_runtime_resource_check(
    state: &ControlApiState,
    tenant_id: Uuid,
    endpoint: String,
    credential_id: Option<Uuid>,
    probe: RuntimeResourceProbeV1,
) -> ApiResult<RuntimeResourceCheckResponseV1> {
    let credential = credential_reference(state, tenant_id, credential_id).await?;
    execute_runtime_resource_check_with_reference(state, tenant_id, endpoint, credential, probe)
        .await
}

pub(crate) async fn execute_runtime_resource_check_with_reference(
    state: &ControlApiState,
    tenant_id: Uuid,
    endpoint: String,
    credential: Option<VaultSecretReferenceV1>,
    probe: RuntimeResourceProbeV1,
) -> ApiResult<RuntimeResourceCheckResponseV1> {
    let now = now_unix();
    let token = issue_service_token(
        &state.runtime_command_kid,
        state.runtime_command_key.expose_secret().as_bytes(),
        &ServiceClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: "platform-control-resource-check".into(),
            role: ControlRole::Publisher,
            scope: BTreeSet::from(["runtime.resources.check".into()]),
            iat: now,
            exp: now + 60,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/resource-checks:execute",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .timeout(RUNTIME_RESOURCE_CHECK_TIMEOUT)
        .json(&RuntimeResourceCheckRequestV1 {
            schema_version: 1,
            tenant_id,
            endpoint,
            credential,
            probe,
        })
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Runtime resource check is unavailable");
            ApiError::unavailable(
                "RUNTIME_UNAVAILABLE",
                "Runtime resource check is unavailable",
            )
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(%status, response_body=%body, "Runtime rejected resource check");
        return Err(ApiError::unavailable(
            "RUNTIME_RESOURCE_CHECK_REJECTED",
            "Runtime rejected the resource check",
        ));
    }
    response.json().await.map_err(ApiError::internal)
}

const RUNTIME_RESOURCE_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_runtime_resource_operation(
    state: &ControlApiState,
    tenant_id: Uuid,
    server_version_id: Uuid,
    transport: RuntimeMcpTransportV2,
    credential: Option<VaultSecretReferenceV1>,
    runtime_sandbox_profile: Option<RuntimeResourceBindingV1>,
    timeout_seconds: u32,
    operation: RuntimeResourceOperationV1,
) -> ApiResult<RuntimeResourceOperationResponseV1> {
    let now = now_unix();
    let token = issue_service_token(
        &state.runtime_command_kid,
        state.runtime_command_key.expose_secret().as_bytes(),
        &ServiceClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: "platform-control-resource-operation".into(),
            role: ControlRole::Publisher,
            scope: BTreeSet::from(["runtime.resources.execute".into()]),
            iat: now,
            exp: now + 60,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/resource-operations:execute",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(
            u64::from(timeout_seconds) + 5,
        ))
        .json(&RuntimeResourceOperationRequestV1 {
            schema_version: 1,
            operation_id: Uuid::now_v7(),
            tenant_id,
            server_version_id,
            transport,
            credential,
            runtime_sandbox_profile,
            timeout_seconds,
            operation,
        })
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Runtime resource operation is unavailable");
            ApiError::unavailable(
                "RUNTIME_UNAVAILABLE",
                "Runtime resource operation is unavailable",
            )
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(%status, response_body=%body, "Runtime rejected resource operation");
        return Err(ApiError::unavailable(
            "RUNTIME_RESOURCE_OPERATION_REJECTED",
            "Runtime rejected the resource operation",
        ));
    }
    response.json().await.map_err(ApiError::internal)
}

async fn credential_reference(
    state: &ControlApiState,
    tenant_id: Uuid,
    credential_id: Option<Uuid>,
) -> ApiResult<Option<VaultSecretReferenceV1>> {
    let Some(credential_id) = credential_id else {
        return Ok(None);
    };
    let row = sqlx::query("SELECT s.secret_ref,s.provider_version FROM credentials c JOIN credential_secret_versions s ON s.tenant_id=c.tenant_id AND s.credential_id=c.id AND s.version_number=c.current_secret_version WHERE c.tenant_id=? AND c.id=? AND c.status='active' AND s.provider='vault_kv_v2'")
        .bind(tenant_id)
        .bind(credential_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::unprocessable("CREDENTIAL_UNAVAILABLE", "Credential is disabled or unavailable"))?;
    let version = row
        .try_get::<String, _>("provider_version")?
        .parse()
        .map_err(ApiError::internal)?;
    Ok(Some(VaultSecretReferenceV1 {
        mount: state.vault_mount.clone(),
        path: row.try_get("secret_ref")?,
        key: "value".into(),
        version,
    }))
}
async fn require_visible(state: &ControlApiState, actor: &Actor, id: Uuid) -> ApiResult<()> {
    let visible: bool = if actor.roles.iter().any(|role| role == "company_admin") {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM model_aliases WHERE tenant_id=? AND id=?)")
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_one(&state.pool)
            .await?
    } else {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id JOIN department_closure dc ON dc.tenant_id=a.tenant_id AND dc.ancestor_id=? AND dc.descendant_id=d.owner_department_id WHERE a.tenant_id=? AND a.id=?)").bind(actor.department_id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?
    };
    if visible {
        Ok(())
    } else {
        Err(ApiError::not_found("Model"))
    }
}
async fn require_deployment_visible(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<()> {
    let model:Option<Uuid>=sqlx::query_scalar("SELECT a.id FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id WHERE a.tenant_id=? AND d.id=? AND (a.deployment_id=d.id OR EXISTS(SELECT 1 FROM model_alias_deployment_history h WHERE h.tenant_id=a.tenant_id AND h.alias_id=a.id AND h.deployment_id=d.id)) LIMIT 1").bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?;
    require_visible(
        state,
        actor,
        model.ok_or_else(|| ApiError::not_found("Model deployment"))?,
    )
    .await
}
async fn require_department_scope(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<()> {
    if actor.roles.iter().any(|role| role == "company_admin") {
        return Ok(());
    }
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM department_closure WHERE tenant_id=? AND ancestor_id=? AND descendant_id=?)").bind(actor.tenant_id).bind(actor.department_id).bind(id).fetch_one(&state.pool).await?;
    if allowed {
        Ok(())
    } else {
        Err(ApiError::forbidden("Department is outside the actor scope"))
    }
}
async fn require_credential(
    state: &ControlApiState,
    tenant: Uuid,
    id: Option<Uuid>,
) -> ApiResult<()> {
    let Some(id) = id else { return Ok(()) };
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM credentials WHERE tenant_id=? AND id=? AND status='active')",
    )
    .bind(tenant)
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    if active {
        Ok(())
    } else {
        Err(ApiError::unprocessable(
            "CREDENTIAL_UNAVAILABLE",
            "Credential is disabled or unavailable",
        ))
    }
}
async fn load_model(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<ModelResponse> {
    let row=sqlx::query("SELECT a.id,a.alias,a.status,a.version alias_version,a.updated_at,d.id deployment_id,d.connection_name,d.provider_type,d.endpoint,d.credential_id,d.owner_department_id,d.model_name,d.max_input_tokens,d.max_output_tokens,d.default_parameters,d.capabilities_json,d.revision_number,COALESCE((SELECT h.status FROM resource_health_checks h WHERE h.tenant_id=a.tenant_id AND h.resource_type='model' AND h.resource_id=a.id AND h.checked_at>=GREATEST(a.updated_at,d.updated_at) ORDER BY h.check_sequence DESC LIMIT 1),'untested') connection_status,(SELECT h.checked_at FROM resource_health_checks h WHERE h.tenant_id=a.tenant_id AND h.resource_type='model' AND h.resource_id=a.id AND h.checked_at>=GREATEST(a.updated_at,d.updated_at) ORDER BY h.check_sequence DESC LIMIT 1) connection_checked_at FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND a.id=?").bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Model"))?;
    Ok(model_from_row(row)?)
}
fn model_from_row(row: MySqlRow) -> Result<ModelResponse, sqlx::Error> {
    Ok(ModelResponse {
        id: row.try_get("id")?,
        alias: row.try_get("alias")?,
        status: row.try_get("status")?,
        alias_version: row.try_get("alias_version")?,
        deployment_id: row.try_get("deployment_id")?,
        connection_name: row.try_get("connection_name")?,
        provider_type: row.try_get("provider_type")?,
        endpoint: row.try_get("endpoint")?,
        credential_id: row.try_get("credential_id")?,
        owner_department_id: row.try_get("owner_department_id")?,
        model_name: row.try_get("model_name")?,
        max_input_tokens: row.try_get("max_input_tokens")?,
        max_output_tokens: row.try_get("max_output_tokens")?,
        default_parameters: row.try_get("default_parameters")?,
        capabilities: row
            .try_get::<Option<Value>, _>("capabilities_json")?
            .and_then(|value| value.as_array().cloned())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        revision_number: row.try_get("revision_number")?,
        connection_status: row.try_get("connection_status")?,
        connection_checked_at: row.try_get("connection_checked_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
fn price_from_row(row: MySqlRow) -> Result<PriceResponse, sqlx::Error> {
    Ok(PriceResponse {
        id: row.try_get("id")?,
        deployment_id: row.try_get("deployment_id")?,
        version_number: row.try_get("version_number")?,
        currency: row.try_get("currency")?,
        input_per_million: row.try_get("input_per_million")?,
        output_per_million: row.try_get("output_per_million")?,
        created_at: row.try_get("created_at")?,
    })
}
fn validate_model_input(
    provider: &str,
    endpoint: &str,
    max_input: u64,
    max_output: u64,
    price: &PriceRequest,
) -> ApiResult<()> {
    if !matches!(provider, "openai_compatible" | "custom_http") {
        return Err(ApiError::bad_request(
            "INVALID_MODEL_PROVIDER",
            "API format is unsupported",
        ));
    }
    let url = Url::parse(endpoint)
        .map_err(|_| ApiError::bad_request("INVALID_ENDPOINT", "Endpoint is invalid"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(
            "INVALID_ENDPOINT",
            "Endpoint must use HTTP or HTTPS",
        ));
    }
    if max_input == 0 || max_output == 0 {
        return Err(ApiError::bad_request(
            "INVALID_MODEL_TOKEN_LIMITS",
            "Model token limits must be positive",
        ));
    }
    validate_price(price)?;
    Ok(())
}
fn validate_price(input: &PriceRequest) -> ApiResult<()> {
    let input_price = input.input_per_million.parse::<Decimal>();
    let output_price = input.output_per_million.parse::<Decimal>();
    let valid_decimal = |value: &Decimal| {
        !value.is_sign_negative()
            && value.scale() <= 8
            && value.mantissa().unsigned_abs() < 10_u128.pow(20)
    };
    if input.currency.trim().len() != 3
        || !input
            .currency
            .chars()
            .all(|value| value.is_ascii_alphabetic())
        || !input_price.as_ref().is_ok_and(valid_decimal)
        || !output_price.as_ref().is_ok_and(valid_decimal)
    {
        Err(ApiError::bad_request(
            "INVALID_MODEL_PRICE",
            "Currency or price is invalid",
        ))
    } else {
        Ok(())
    }
}
fn validate_alias(value: &str) -> ApiResult<String> {
    let alias = value.trim().to_ascii_lowercase();
    if alias.is_empty() || alias.len() > 128 {
        Err(ApiError::bad_request(
            "INVALID_MODEL_ALIAS",
            "Model name is invalid",
        ))
    } else {
        Ok(alias)
    }
}
fn map_alias_error(error: sqlx::Error) -> ApiError {
    match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            ApiError::conflict("MODEL_NAME_EXISTS", "A model with this name already exists")
                .with_field_error(
                    "alias",
                    "MODEL_NAME_EXISTS",
                    "A model with this name already exists",
                )
        }
        _ => ApiError::from(error),
    }
}
fn default_model_name() -> String {
    "gpt-5.6-sol".into()
}
const fn default_max_input_tokens() -> u64 {
    1_050_000
}
const fn default_max_output_tokens() -> u64 {
    128_000
}
fn validate_capabilities(capabilities: &[String]) -> ApiResult<Vec<String>> {
    for capability in capabilities {
        if !matches!(capability.as_str(), "vision" | "audio") {
            return Err(ApiError::bad_request(
                "INVALID_MODEL_CAPABILITY",
                "Model capabilities must be vision or audio",
            ));
        }
    }
    Ok(capabilities.to_vec())
}

fn default_parameters() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_model_contract() {
        assert!(
            validate_model_input(
                "openai_compatible",
                "http://provider.test/v1",
                100,
                10,
                &PriceRequest {
                    currency: "USD".into(),
                    input_per_million: "1.25".into(),
                    output_per_million: "2.5".into(),
                }
            )
            .is_ok()
        );
        let price = PriceRequest {
            currency: "USD".into(),
            input_per_million: "1.25".into(),
            output_per_million: "2.5".into(),
        };
        assert!(validate_model_input("unknown", "http://provider.test", 100, 10, &price).is_err());
        assert!(validate_alias(" GPT-5 ").is_ok());
    }

    #[test]
    fn create_and_update_contracts_require_a_complete_price() {
        let base = json!({
            "connectionName":"fixture",
            "providerType":"openai_compatible",
            "endpoint":"https://provider.test/v1",
            "credentialId":null,
            "ownerDepartmentId":Uuid::now_v7(),
            "alias":"fixture",
            "modelName":"fixture",
            "maxInputTokens":100,
            "maxOutputTokens":10,
            "defaultParameters":{}
        });
        assert!(serde_json::from_value::<CreateRequest>(base.clone()).is_err());
        let mut create = base.clone();
        create["price"] = json!({"currency":"USD","inputPerMillion":"1","outputPerMillion":"2"});
        assert!(serde_json::from_value::<CreateRequest>(create).is_ok());

        let mut update = base;
        update["status"] = json!("active");
        update["expectedAliasVersion"] = json!(1);
        assert!(serde_json::from_value::<UpdateRequest>(update).is_err());
    }

    #[test]
    fn runtime_resource_check_timeout_covers_the_provider_probe_budget() {
        assert_eq!(RUNTIME_RESOURCE_CHECK_TIMEOUT.as_secs(), 20);
    }
}
