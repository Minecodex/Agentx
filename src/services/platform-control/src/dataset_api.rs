use agentx_api_types::PageResponse;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, Response, StatusCode, header},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    control_helpers::required_name,
};

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/datasets", get(list_datasets).post(create_dataset))
        .route(
            "/api/v1/datasets/{id}",
            get(get_dataset)
                .patch(update_dataset)
                .delete(delete_dataset),
        )
        .route(
            "/api/v1/datasets/{id}/cases",
            get(list_cases).post(create_case),
        )
        .route(
            "/api/v1/datasets/{id}/cases/{case_id}",
            axum::routing::patch(update_case).delete(delete_case),
        )
        .route("/api/v1/datasets/{id}/import", post(import_cases))
        .route("/api/v1/datasets/{id}/export", get(export_cases))
        .route(
            "/api/v1/datasets/{id}/versions",
            get(list_versions).post(publish_version),
        )
        .route(
            "/api/v1/evaluation-profiles",
            get(list_profiles).post(create_profile),
        )
        .route(
            "/api/v1/evaluation-profiles/{id}",
            get(get_profile).delete(delete_profile),
        )
        .route("/api/v1/evaluations", post(create_evaluation))
        .route("/api/v1/evaluations/{id}/start", post(start_evaluation))
        .route("/api/v1/evaluations/{id}/cancel", post(cancel_evaluation))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    search: Option<String>,
    status: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DatasetResponse {
    id: Uuid,
    name: String,
    description: Option<String>,
    visibility: String,
    status: String,
    owner_department_id: Uuid,
    revision: u64,
    case_count: u64,
    latest_version: Option<u64>,
    version: u64,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateDatasetRequest {
    name: String,
    description: Option<String>,
    visibility: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateDatasetRequest {
    name: String,
    description: Option<String>,
    visibility: String,
    status: String,
    version: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteQuery {
    expected_version: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CaseInput {
    case_key: String,
    name: String,
    input: Value,
    expected_output: Option<Value>,
    context: Option<Value>,
    #[serde(default)]
    tags: Vec<String>,
    evaluator_override: Option<Value>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaseResponse {
    id: Uuid,
    case_key: String,
    name: String,
    input: Value,
    expected_output: Option<Value>,
    context: Option<Value>,
    tags: Vec<String>,
    evaluator_override: Option<Value>,
    sort_order: u64,
    version: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateCaseRequest {
    expected_revision: u64,
    #[serde(flatten)]
    case: CaseInput,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateCaseRequest {
    expected_revision: u64,
    version: u64,
    #[serde(flatten)]
    case: CaseInput,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteCaseRequest {
    expected_revision: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportRequest {
    expected_revision: u64,
    format: String,
    content: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionResponse {
    id: Uuid,
    dataset_id: Uuid,
    version_number: u64,
    source_revision: u64,
    content_hash: String,
    case_count: u64,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuleInput {
    key: String,
    name: String,
    evaluator_type: String,
    configuration: Value,
    #[serde(default = "default_weight")]
    weight: String,
    #[serde(default = "default_required")]
    required: bool,
}
fn default_weight() -> String {
    "1".into()
}
fn default_required() -> bool {
    true
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileInput {
    name: String,
    description: Option<String>,
    #[serde(default = "default_visibility")]
    visibility: String,
    #[serde(default = "default_aggregation")]
    aggregation: String,
    #[serde(default = "default_threshold")]
    pass_threshold: String,
    rules: Vec<RuleInput>,
}
fn default_visibility() -> String {
    "department".into()
}
fn default_aggregation() -> String {
    "all".into()
}
fn default_threshold() -> String {
    "1".into()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvaluationInput {
    name: String,
    workflow_version_id: Uuid,
    dataset_version_id: Uuid,
    evaluation_profile_version_id: Uuid,
    #[serde(default = "default_visibility")]
    visibility: String,
    parameters: Value,
}

async fn list_datasets(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<DatasetResponse>>> {
    actor.require("dataset:view")?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let search = format!("%{}%", query.search.unwrap_or_default());
    let status = query.status.unwrap_or_default();
    let rows=sqlx::query("SELECT d.id,d.name,d.description,d.visibility,d.status,d.owner_department_id,d.revision,d.version,d.updated_at,(SELECT COUNT(*) FROM dataset_cases c WHERE c.tenant_id=d.tenant_id AND c.dataset_id=d.id) case_count,(SELECT MAX(version_number) FROM dataset_versions v WHERE v.tenant_id=d.tenant_id AND v.dataset_id=d.id) latest_version,COUNT(*) OVER() total_count FROM datasets d WHERE d.tenant_id=? AND (?='' OR d.status=?) AND (?='%%' OR d.name LIKE ?) ORDER BY d.updated_at DESC,d.id DESC LIMIT ? OFFSET ?").bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(page_size).bind(u64::from((page-1)*page_size)).fetch_all(&state.pool).await?;
    let total = rows
        .first()
        .map_or(Ok(0_i64), |r| r.try_get("total_count"))? as u64;
    let items = rows
        .into_iter()
        .map(dataset_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(PageResponse {
        items,
        page,
        page_size,
        total,
    }))
}

async fn create_dataset(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateDatasetRequest>,
) -> ApiResult<(StatusCode, Json<DatasetResponse>)> {
    actor.require("dataset:manage")?;
    validate_visibility(&input.visibility)?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO datasets(id,tenant_id,name,description,visibility,owner_user_id,owner_department_id) VALUES(?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(required_name(&input.name)?).bind(input.description).bind(input.visibility).bind(actor.user_id).bind(actor.department_id).execute(&state.pool).await?;
    Ok((
        StatusCode::CREATED,
        Json(load_dataset(&state, actor.tenant_id, id).await?),
    ))
}
async fn get_dataset(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<DatasetResponse>> {
    actor.require("dataset:view")?;
    Ok(Json(load_dataset(&state, actor.tenant_id, id).await?))
}
async fn update_dataset(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateDatasetRequest>,
) -> ApiResult<Json<DatasetResponse>> {
    actor.require("dataset:manage")?;
    validate_visibility(&input.visibility)?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_STATUS",
            "Dataset status is invalid",
        ));
    }
    let result=sqlx::query("UPDATE datasets SET name=?,description=?,visibility=?,status=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?").bind(required_name(&input.name)?).bind(input.description).bind(input.visibility).bind(input.status).bind(actor.tenant_id).bind(id).bind(input.version).execute(&state.pool).await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "DATASET_VERSION_CONFLICT",
            "Dataset changed",
        ));
    }
    Ok(Json(load_dataset(&state, actor.tenant_id, id).await?))
}
async fn delete_dataset(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    actor.require("dataset:manage")?;
    let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM evaluation_runs r JOIN dataset_versions v ON v.id=r.dataset_version_id WHERE r.tenant_id=? AND v.dataset_id=?)").bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if used {
        return Err(ApiError::conflict(
            "RESOURCE_REFERENCED",
            "Dataset is referenced by Evaluation runs",
        ));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM dataset_cases WHERE tenant_id=? AND dataset_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query("DELETE FROM datasets WHERE tenant_id=? AND id=? AND version=?")
        .bind(actor.tenant_id)
        .bind(id)
        .bind(query.expected_version)
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "DATASET_VERSION_CONFLICT",
            "Dataset changed",
        ));
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_cases(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<CaseResponse>>> {
    actor.require("dataset:view")?;
    load_dataset(&state, actor.tenant_id, id).await?;
    let rows=sqlx::query("SELECT id,case_key,name,input_json,expected_output_json,context_json,tags_json,evaluator_override_json,sort_order,version FROM dataset_cases WHERE tenant_id=? AND dataset_id=? ORDER BY sort_order,id").bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(case_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}
async fn create_case(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<CreateCaseRequest>,
) -> ApiResult<(StatusCode, Json<CaseResponse>)> {
    actor.require("dataset:manage")?;
    validate_case(&input.case)?;
    let mut tx = state.pool.begin().await?;
    bump_revision(&mut tx, actor.tenant_id, id, input.expected_revision).await?;
    let order:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(sort_order),0)+1 AS UNSIGNED) FROM dataset_cases WHERE tenant_id=? AND dataset_id=?").bind(actor.tenant_id).bind(id).fetch_one(&mut *tx).await?;
    let case_id = Uuid::now_v7();
    insert_case(&mut tx, actor.tenant_id, id, case_id, order, &input.case).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_case(&state, actor.tenant_id, id, case_id).await?),
    ))
}
async fn update_case(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, case_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<UpdateCaseRequest>,
) -> ApiResult<Json<CaseResponse>> {
    actor.require("dataset:manage")?;
    validate_case(&input.case)?;
    let mut tx = state.pool.begin().await?;
    bump_revision(&mut tx, actor.tenant_id, id, input.expected_revision).await?;
    let result=sqlx::query("UPDATE dataset_cases SET case_key=?,name=?,input_json=?,expected_output_json=?,context_json=?,tags_json=?,evaluator_override_json=?,version=version+1 WHERE tenant_id=? AND dataset_id=? AND id=? AND version=?").bind(input.case.case_key.trim()).bind(input.case.name.trim()).bind(input.case.input).bind(input.case.expected_output).bind(input.case.context).bind(json!(input.case.tags)).bind(input.case.evaluator_override).bind(actor.tenant_id).bind(id).bind(case_id).bind(input.version).execute(&mut *tx).await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "DATASET_CASE_VERSION_CONFLICT",
            "Test Case changed",
        ));
    }
    tx.commit().await?;
    Ok(Json(load_case(&state, actor.tenant_id, id, case_id).await?))
}
async fn delete_case(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, case_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<DeleteCaseRequest>,
) -> ApiResult<StatusCode> {
    actor.require("dataset:manage")?;
    let mut tx = state.pool.begin().await?;
    bump_revision(&mut tx, actor.tenant_id, id, input.expected_revision).await?;
    let result =
        sqlx::query("DELETE FROM dataset_cases WHERE tenant_id=? AND dataset_id=? AND id=?")
            .bind(actor.tenant_id)
            .bind(id)
            .bind(case_id)
            .execute(&mut *tx)
            .await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::not_found("Test Case"));
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn import_cases(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<ImportRequest>,
) -> ApiResult<Json<Vec<CaseResponse>>> {
    actor.require("dataset:manage")?;
    let cases = parse_import(&input.format, &input.content)?;
    if cases.is_empty() || cases.len() > 10_000 {
        return Err(ApiError::bad_request(
            "INVALID_DATASET_IMPORT",
            "Import requires 1 to 10000 cases",
        ));
    }
    let mut tx = state.pool.begin().await?;
    bump_revision(&mut tx, actor.tenant_id, id, input.expected_revision).await?;
    let start: u64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(MAX(sort_order),0) AS UNSIGNED) FROM dataset_cases WHERE tenant_id=? AND dataset_id=?",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    for (index, case) in cases.iter().enumerate() {
        validate_case(case)?;
        insert_case(
            &mut tx,
            actor.tenant_id,
            id,
            Uuid::now_v7(),
            start + index as u64 + 1,
            case,
        )
        .await?;
    }
    tx.commit().await?;
    list_cases(State(state), actor, Path(id)).await
}
async fn export_cases(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Response<Body>> {
    let cases = list_cases(State(state), actor, Path(id)).await?.0;
    let body=cases.iter().map(|case|serde_json::to_string(&json!({"caseKey":case.case_key,"name":case.name,"input":case.input,"expectedOutput":case.expected_output,"context":case.context,"tags":case.tags,"evaluatorOverride":case.evaluator_override})).map_err(ApiError::internal)).collect::<ApiResult<Vec<_>>>()?.join("\n");
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=dataset.jsonl"),
    );
    Ok(response)
}

async fn list_versions(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<VersionResponse>>> {
    actor.require("dataset:view")?;
    let rows=sqlx::query("SELECT id,dataset_id,version_number,source_revision,content_hash,case_count,created_at FROM dataset_versions WHERE tenant_id=? AND dataset_id=? ORDER BY version_number DESC").bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(version_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}
async fn publish_version(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<(StatusCode, Json<VersionResponse>)> {
    actor.require("dataset:manage")?;
    let mut tx = state.pool.begin().await?;
    let revision: u64 = sqlx::query_scalar(
        "SELECT revision FROM datasets WHERE tenant_id=? AND id=? AND status='active' FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Active Dataset"))?;
    let rows=sqlx::query("SELECT id,case_key,name,input_json,expected_output_json,context_json,tags_json,evaluator_override_json,sort_order,version FROM dataset_cases WHERE tenant_id=? AND dataset_id=? ORDER BY sort_order,id").bind(actor.tenant_id).bind(id).fetch_all(&mut *tx).await?;
    if rows.is_empty() {
        return Err(ApiError::unprocessable(
            "DATASET_EMPTY",
            "Dataset requires a Test Case",
        ));
    }
    let cases = rows
        .into_iter()
        .map(case_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&cases).map_err(ApiError::internal)?)
    );
    if let Some(row)=sqlx::query("SELECT id,dataset_id,version_number,source_revision,content_hash,case_count,created_at FROM dataset_versions WHERE tenant_id=? AND dataset_id=? AND source_revision=? AND content_hash=?").bind(actor.tenant_id).bind(id).bind(revision).bind(&hash).fetch_optional(&mut *tx).await?{tx.commit().await?;return Ok((StatusCode::OK,Json(version_from_row(row)?)));}
    let version_id = Uuid::now_v7();
    let number:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(version_number),0)+1 AS UNSIGNED) FROM dataset_versions WHERE tenant_id=? AND dataset_id=?").bind(actor.tenant_id).bind(id).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO dataset_versions(id,tenant_id,dataset_id,version_number,source_revision,content_hash,case_count,created_by) VALUES(?,?,?,?,?,?,?,?)").bind(version_id).bind(actor.tenant_id).bind(id).bind(number).bind(revision).bind(&hash).bind(cases.len() as u64).bind(actor.user_id).execute(&mut *tx).await?;
    for case in &cases {
        let case_hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(case).map_err(ApiError::internal)?)
        );
        sqlx::query("INSERT INTO dataset_version_cases(tenant_id,dataset_version_id,source_case_id,case_key,name,input_json,expected_output_json,context_json,tags_json,evaluator_override_json,sort_order,content_hash) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)").bind(actor.tenant_id).bind(version_id).bind(case.id).bind(&case.case_key).bind(&case.name).bind(&case.input).bind(&case.expected_output).bind(&case.context).bind(json!(case.tags)).bind(&case.evaluator_override).bind(case.sort_order).bind(case_hash).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let row=sqlx::query("SELECT id,dataset_id,version_number,source_revision,content_hash,case_count,created_at FROM dataset_versions WHERE tenant_id=? AND id=?").bind(actor.tenant_id).bind(version_id).fetch_one(&state.pool).await?;
    Ok((StatusCode::CREATED, Json(version_from_row(row)?)))
}

async fn list_profiles(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<Json<Vec<Value>>> {
    actor.require("evaluation_profile:view")?;
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM evaluation_profiles WHERE tenant_id=? ORDER BY updated_at DESC",
    )
    .bind(actor.tenant_id)
    .fetch_all(&state.pool)
    .await?;
    let mut values = Vec::new();
    for id in ids {
        values.push(load_profile(&state, actor.tenant_id, id).await?);
    }
    Ok(Json(values))
}
async fn get_profile(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    actor.require("evaluation_profile:view")?;
    Ok(Json(load_profile(&state, actor.tenant_id, id).await?))
}

async fn delete_profile(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    actor.require("evaluation_profile:manage")?;
    let referenced: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM evaluation_runs r JOIN evaluation_profile_versions v ON v.tenant_id=r.tenant_id AND v.id=r.evaluation_profile_version_id WHERE r.tenant_id=? AND v.profile_id=?)")
        .bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if referenced {
        return Err(ApiError::conflict(
            "RESOURCE_REFERENCED",
            "Evaluation Profile is referenced by Evaluation runs",
        ));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE r FROM evaluation_profile_rules r JOIN evaluation_profile_versions v ON v.tenant_id=r.tenant_id AND v.id=r.profile_version_id WHERE v.tenant_id=? AND v.profile_id=?").bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM evaluation_profile_versions WHERE tenant_id=? AND profile_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let result =
        sqlx::query("DELETE FROM evaluation_profiles WHERE tenant_id=? AND id=? AND version=?")
            .bind(actor.tenant_id)
            .bind(id)
            .bind(query.expected_version)
            .execute(&mut *tx)
            .await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "EVALUATION_PROFILE_VERSION_CONFLICT",
            "Evaluation Profile changed",
        ));
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn create_profile(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<ProfileInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("evaluation_profile:manage")?;
    validate_profile(&input)?;
    let id = Uuid::now_v7();
    let version = Uuid::now_v7();
    let hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&input).map_err(ApiError::internal)?)
    );
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO evaluation_profiles(id,tenant_id,name,description,owner_user_id,owner_department_id,visibility) VALUES(?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(required_name(&input.name)?).bind(input.description).bind(actor.user_id).bind(actor.department_id).bind(input.visibility).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO evaluation_profile_versions(id,tenant_id,profile_id,version_number,aggregation,pass_threshold,content_hash,created_by) VALUES(?,?,?,1,?,?,?,?)").bind(version).bind(actor.tenant_id).bind(id).bind(input.aggregation).bind(input.pass_threshold).bind(hash).bind(actor.user_id).execute(&mut *tx).await?;
    for (index, rule) in input.rules.iter().enumerate() {
        sqlx::query("INSERT INTO evaluation_profile_rules(id,tenant_id,profile_version_id,rule_key,name,evaluator_type,configuration_json,weight,required,sort_order) VALUES(?,?,?,?,?,?,?,?,?,?)").bind(Uuid::now_v7()).bind(actor.tenant_id).bind(version).bind(rule.key.trim()).bind(required_name(&rule.name)?).bind(&rule.evaluator_type).bind(&rule.configuration).bind(&rule.weight).bind(rule.required).bind(index as u32).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_profile(&state, actor.tenant_id, id).await?),
    ))
}

async fn create_evaluation(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<EvaluationInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    actor.require("evaluation:manage")?;
    validate_visibility(&input.visibility)?;
    for (label, sql, id) in [
        (
            "workflow_versions",
            "SELECT EXISTS(SELECT 1 FROM workflow_versions WHERE tenant_id=? AND id=?)",
            input.workflow_version_id,
        ),
        (
            "dataset_versions",
            "SELECT EXISTS(SELECT 1 FROM dataset_versions WHERE tenant_id=? AND id=?)",
            input.dataset_version_id,
        ),
        (
            "evaluation_profile_versions",
            "SELECT EXISTS(SELECT 1 FROM evaluation_profile_versions WHERE tenant_id=? AND id=?)",
            input.evaluation_profile_version_id,
        ),
    ] {
        let exists: bool = sqlx::query_scalar(sql)
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
        if !exists {
            return Err(ApiError::unprocessable(
                "INVALID_EVALUATION_SNAPSHOT",
                format!("{label} snapshot was not found"),
            ));
        }
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO evaluation_runs(id,tenant_id,name,workflow_version_id,dataset_version_id,evaluation_profile_version_id,parameters_json,created_by,owner_department_id,visibility) VALUES(?,?,?,?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(required_name(&input.name)?).bind(input.workflow_version_id).bind(input.dataset_version_id).bind(input.evaluation_profile_version_id).bind(input.parameters).bind(actor.user_id).bind(actor.department_id).bind(input.visibility).execute(&state.pool).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id":id,"status":"created"})),
    ))
}
async fn start_evaluation(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("evaluation:manage")?;
    crate::work_packages::start_evaluation(&state, &actor, id).await?;
    Ok(StatusCode::ACCEPTED)
}
async fn cancel_evaluation(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    actor.require("evaluation:manage")?;
    crate::work_packages::cancel_evaluation(&state, &actor, id).await?;
    Ok(StatusCode::ACCEPTED)
}

fn dataset_from_row(row: sqlx::mysql::MySqlRow) -> Result<DatasetResponse, sqlx::Error> {
    Ok(DatasetResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        visibility: row.try_get("visibility")?,
        status: row.try_get("status")?,
        owner_department_id: row.try_get("owner_department_id")?,
        revision: row.try_get("revision")?,
        case_count: row.try_get::<i64, _>("case_count")? as u64,
        latest_version: row.try_get("latest_version")?,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}
async fn load_dataset(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<DatasetResponse> {
    let row=sqlx::query("SELECT d.id,d.name,d.description,d.visibility,d.status,d.owner_department_id,d.revision,d.version,d.updated_at,(SELECT COUNT(*) FROM dataset_cases c WHERE c.tenant_id=d.tenant_id AND c.dataset_id=d.id) case_count,(SELECT MAX(version_number) FROM dataset_versions v WHERE v.tenant_id=d.tenant_id AND v.dataset_id=d.id) latest_version FROM datasets d WHERE d.tenant_id=? AND d.id=?").bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Dataset"))?;
    Ok(dataset_from_row(row)?)
}
fn case_from_row(row: sqlx::mysql::MySqlRow) -> Result<CaseResponse, sqlx::Error> {
    Ok(CaseResponse {
        id: row.try_get("id")?,
        case_key: row.try_get("case_key")?,
        name: row.try_get("name")?,
        input: row.try_get("input_json")?,
        expected_output: row.try_get("expected_output_json")?,
        context: row.try_get("context_json")?,
        tags: serde_json::from_value(row.try_get("tags_json")?).unwrap_or_default(),
        evaluator_override: row.try_get("evaluator_override_json")?,
        sort_order: row.try_get("sort_order")?,
        version: row.try_get("version")?,
    })
}
async fn load_case(
    state: &ControlApiState,
    tenant: Uuid,
    dataset: Uuid,
    id: Uuid,
) -> ApiResult<CaseResponse> {
    let row=sqlx::query("SELECT id,case_key,name,input_json,expected_output_json,context_json,tags_json,evaluator_override_json,sort_order,version FROM dataset_cases WHERE tenant_id=? AND dataset_id=? AND id=?").bind(tenant).bind(dataset).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Test Case"))?;
    Ok(case_from_row(row)?)
}
async fn bump_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    id: Uuid,
    expected: u64,
) -> ApiResult<()> {
    let result=sqlx::query("UPDATE datasets SET revision=revision+1,version=version+1 WHERE tenant_id=? AND id=? AND revision=? AND status='active'").bind(tenant).bind(id).bind(expected).execute(&mut **tx).await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "DATASET_REVISION_CONFLICT",
            "Dataset changed",
        ));
    }
    Ok(())
}
async fn insert_case(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    dataset: Uuid,
    id: Uuid,
    order: u64,
    case: &CaseInput,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO dataset_cases(id,tenant_id,dataset_id,case_key,name,input_json,expected_output_json,context_json,tags_json,evaluator_override_json,sort_order) VALUES(?,?,?,?,?,?,?,?,?,?,?)").bind(id).bind(tenant).bind(dataset).bind(case.case_key.trim()).bind(case.name.trim()).bind(&case.input).bind(&case.expected_output).bind(&case.context).bind(json!(case.tags)).bind(&case.evaluator_override).bind(order).execute(&mut **tx).await.map_err(|error| match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => ApiError::conflict("DATASET_CASE_KEY_EXISTS", "A Test Case with this key already exists in the Dataset").with_field_error("caseKey", "DATASET_CASE_KEY_EXISTS", "A Test Case with this key already exists in the Dataset"),
        _ => ApiError::from(error),
    })?;
    Ok(())
}
fn version_from_row(row: sqlx::mysql::MySqlRow) -> Result<VersionResponse, sqlx::Error> {
    Ok(VersionResponse {
        id: row.try_get("id")?,
        dataset_id: row.try_get("dataset_id")?,
        version_number: row.try_get("version_number")?,
        source_revision: row.try_get("source_revision")?,
        content_hash: row.try_get("content_hash")?,
        case_count: row.try_get("case_count")?,
        created_at: row.try_get("created_at")?,
    })
}
async fn load_profile(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<Value> {
    let row=sqlx::query("SELECT p.id,p.name,p.description,p.visibility,p.owner_department_id,p.status,p.version,v.id version_id,v.version_number,v.aggregation,CAST(v.pass_threshold AS CHAR) pass_threshold FROM evaluation_profiles p JOIN evaluation_profile_versions v ON v.tenant_id=p.tenant_id AND v.profile_id=p.id WHERE p.tenant_id=? AND p.id=? ORDER BY v.version_number DESC LIMIT 1").bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Evaluation Profile"))?;
    let version: Uuid = row.try_get("version_id")?;
    let rules=sqlx::query("SELECT id,rule_key,name,evaluator_type,configuration_json,CAST(weight AS CHAR) weight,required,sort_order FROM evaluation_profile_rules WHERE tenant_id=? AND profile_version_id=? ORDER BY sort_order").bind(tenant).bind(version).fetch_all(&state.pool).await?.into_iter().map(|r|json!({"id":r.try_get::<Uuid,_>("id").unwrap(),"key":r.try_get::<String,_>("rule_key").unwrap(),"name":r.try_get::<String,_>("name").unwrap(),"evaluatorType":r.try_get::<String,_>("evaluator_type").unwrap(),"configuration":r.try_get::<Value,_>("configuration_json").unwrap(),"weight":r.try_get::<String,_>("weight").unwrap(),"required":r.try_get::<bool,_>("required").unwrap(),"sortOrder":r.try_get::<u32,_>("sort_order").unwrap()})).collect::<Vec<_>>();
    Ok(
        json!({"id":id,"versionId":version,"versionNumber":row.try_get::<u64,_>("version_number")?,"name":row.try_get::<String,_>("name")?,"description":row.try_get::<Option<String>,_>("description")?,"visibility":row.try_get::<String,_>("visibility")?,"ownerDepartmentId":row.try_get::<Uuid,_>("owner_department_id")?,"status":row.try_get::<String,_>("status")?,"aggregation":row.try_get::<String,_>("aggregation")?,"passThreshold":row.try_get::<String,_>("pass_threshold")?,"rules":rules,"version":row.try_get::<u64,_>("version")?}),
    )
}
fn validate_visibility(value: &str) -> ApiResult<()> {
    if !matches!(value, "private" | "department" | "company") {
        return Err(ApiError::bad_request(
            "INVALID_VISIBILITY",
            "Visibility is invalid",
        ));
    }
    Ok(())
}
fn validate_case(case: &CaseInput) -> ApiResult<()> {
    if case.case_key.trim().is_empty() || case.case_key.len() > 128 {
        return Err(ApiError::bad_request(
            "INVALID_CASE_KEY",
            "Case key is required and limited to 128 characters",
        ));
    }
    required_name(&case.name)?;
    Ok(())
}
fn validate_profile(input: &ProfileInput) -> ApiResult<()> {
    validate_visibility(&input.visibility)?;
    if !matches!(input.aggregation.as_str(), "all" | "any" | "weighted") {
        return Err(ApiError::bad_request(
            "INVALID_PROFILE_AGGREGATION",
            "Aggregation is invalid",
        ));
    }
    let threshold = input.pass_threshold.parse::<f64>().map_err(|_| {
        ApiError::bad_request("INVALID_PASS_THRESHOLD", "Pass threshold must be a number")
    })?;
    if !(0.0..=1.0).contains(&threshold) || input.rules.is_empty() || input.rules.len() > 50 {
        return Err(ApiError::bad_request(
            "INVALID_EVALUATION_RULES",
            "Profile rules or threshold are invalid",
        ));
    }
    let mut keys = std::collections::BTreeSet::new();
    for rule in &input.rules {
        if !keys.insert(rule.key.trim()) || rule.key.trim().is_empty() {
            return Err(ApiError::bad_request(
                "INVALID_EVALUATION_RULE",
                "Evaluation rule is invalid",
            ));
        }
        validate_evaluator(rule)?;
    }
    Ok(())
}

fn validate_evaluator(rule: &RuleInput) -> ApiResult<()> {
    match rule.evaluator_type.as_str() {
        "exact" | "contains" => {
            if !rule.configuration.is_object() {
                return Err(ApiError::bad_request(
                    "INVALID_EVALUATION_RULE",
                    "Deterministic evaluator configuration must be an object",
                ));
            }
        }
        "regex" => {
            let pattern = rule
                .configuration
                .get("pattern")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= 4096)
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "INVALID_EVALUATION_RULE",
                        "Regex evaluator requires a pattern of at most 4096 bytes",
                    )
                })?;
            regex::Regex::new(pattern).map_err(|_| {
                ApiError::bad_request(
                    "INVALID_EVALUATION_RULE",
                    "Regex evaluator pattern is invalid",
                )
            })?;
        }
        "json_schema" => {
            let schema = rule.configuration.get("schema").ok_or_else(|| {
                ApiError::bad_request(
                    "INVALID_EVALUATION_RULE",
                    "JSON Schema evaluator requires schema",
                )
            })?;
            jsonschema::validator_for(schema).map_err(|_| {
                ApiError::bad_request(
                    "INVALID_EVALUATION_RULE",
                    "JSON Schema evaluator schema is invalid",
                )
            })?;
        }
        "llm_judge" => {
            let model_id = rule
                .configuration
                .get("modelId")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok());
            let prompt = rule
                .configuration
                .get("prompt")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty() && value.len() <= 64 * 1024);
            if model_id.is_none() || prompt.is_none() {
                return Err(ApiError::bad_request(
                    "INVALID_EVALUATION_RULE",
                    "Model evaluator requires a valid modelId and a non-empty prompt of at most 64 KiB",
                ));
            }
        }
        "custom_code" => {
            return Err(ApiError::unprocessable(
                "EVALUATOR_UNSUPPORTED",
                "Custom code evaluators are not supported by Runtime Contracts v1",
            ));
        }
        _ => {
            return Err(ApiError::bad_request(
                "INVALID_EVALUATION_RULE",
                "Evaluation rule type is invalid",
            ));
        }
    }
    Ok(())
}
fn parse_import(format: &str, content: &str) -> ApiResult<Vec<CaseInput>> {
    match format {
        "jsonl" => content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .enumerate()
            .map(|(index, line)| {
                serde_json::from_str(line).map_err(|error| {
                    ApiError::bad_request(
                        "INVALID_DATASET_IMPORT",
                        format!("Line {}: {error}", index + 1),
                    )
                })
            })
            .collect(),
        "csv" => {
            let mut reader = csv::Reader::from_reader(content.as_bytes());
            reader
                .deserialize()
                .map(|row| {
                    row.map_err(|error| {
                        ApiError::bad_request("INVALID_DATASET_IMPORT", error.to_string())
                    })
                })
                .collect()
        }
        _ => Err(ApiError::bad_request(
            "INVALID_IMPORT_FORMAT",
            "Format must be jsonl or csv",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_errors_are_explicit() {
        assert!(parse_import("yaml", "").is_err());
        assert!(parse_import("jsonl", "bad").is_err());
    }
    #[test]
    fn profiles_require_rules() {
        let input = ProfileInput {
            name: "p".into(),
            description: None,
            visibility: "department".into(),
            aggregation: "all".into(),
            pass_threshold: "1".into(),
            rules: vec![],
        };
        assert!(validate_profile(&input).is_err());
    }

    #[test]
    fn evaluator_configuration_is_validated_before_persistence() {
        let mut input = ProfileInput {
            name: "p".into(),
            description: None,
            visibility: "department".into(),
            aggregation: "all".into(),
            pass_threshold: "1".into(),
            rules: vec![RuleInput {
                key: "regex".into(),
                name: "Regex".into(),
                evaluator_type: "regex".into(),
                configuration: json!({"pattern":"["}),
                weight: "1".into(),
                required: true,
            }],
        };
        assert!(validate_profile(&input).is_err());
        input.rules[0].evaluator_type = "json_schema".into();
        input.rules[0].configuration = json!({"schema":{"type":"unknown"}});
        assert!(validate_profile(&input).is_err());
        input.rules[0].evaluator_type = "custom_code".into();
        input.rules[0].configuration = json!({});
        assert!(validate_profile(&input).is_err());
    }
}
