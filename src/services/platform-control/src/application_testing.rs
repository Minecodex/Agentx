//! Application-owned test authoring and evaluation orchestration.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    test_contract::{CaseErrorCode, CaseFieldError, TestContract},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

#[path = "application_datasets.rs"]
pub(crate) mod datasets;
#[path = "application_evaluations.rs"]
pub(crate) mod evaluations;
#[path = "application_evaluation_profiles.rs"]
pub(crate) mod profiles;
#[path = "application_evaluation_reports.rs"]
pub(crate) mod reports;

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route(
            "/api/v1/applications/{application}/test-contracts",
            get(contracts),
        )
        .route(
            "/api/v1/applications/{application}/test-case-validation",
            post(validate_case),
        )
        .route(
            "/api/v1/applications/{application}/datasets",
            get(datasets::list).post(datasets::create),
        )
        .route(
            "/api/v1/applications/{application}/datasets/{dataset}",
            get(datasets::get)
                .patch(datasets::update)
                .delete(datasets::delete),
        )
        .route(
            "/api/v1/applications/{application}/datasets/{dataset}/cases",
            get(datasets::cases).post(datasets::save_case),
        )
        .route(
            "/api/v1/applications/{application}/datasets/{dataset}/cases/{case}",
            axum::routing::patch(datasets::edit_case).delete(datasets::delete_case),
        )
        .route(
            "/api/v1/applications/{application}/datasets/{dataset}/copies",
            post(datasets::copy),
        )
        .route(
            "/api/v1/applications/{application}/datasets/{dataset}/copy-preview",
            post(datasets::copy_preview),
        )
        .route(
            "/api/v1/applications/{application}/datasets/{dataset}/import",
            post(datasets::import),
        )
        .route(
            "/api/v1/applications/{application}/datasets/{dataset}/export",
            post(datasets::export),
        )
        .route(
            "/api/v1/applications/{application}/invocations/{invocation}/test-sample",
            get(datasets::invocation_sample),
        )
        .route(
            "/api/v1/applications/{application}/evaluation-profiles",
            get(profiles::list).post(profiles::create),
        )
        .route(
            "/api/v1/applications/{application}/evaluation-profiles/{profile}/versions",
            get(profiles::versions),
        )
        .route(
            "/api/v1/applications/{application}/evaluation-profiles/{profile}",
            axum::routing::patch(profiles::update).delete(profiles::delete),
        )
        .route(
            "/api/v1/applications/{application}/evaluations",
            get(reports::list).post(evaluations::create),
        )
        .route(
            "/api/v1/applications/{application}/evaluations/{batch}/report",
            get(reports::get),
        )
        .route(
            "/api/v1/applications/{application}/evaluations/{batch}/cancel",
            post(evaluations::cancel),
        )
}

pub(crate) async fn authorize(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    permission: &str,
) -> ApiResult<Uuid> {
    actor.require("application:view")?;
    actor.require(permission)?;
    let row = sqlx::query("SELECT workflow_id,visibility,owner_user_id,owner_department_id FROM applications WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id).bind(application).fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::not_found("Application"))?;
    let visible = actor.roles.iter().any(|role| role == "company_admin")
        || row.try_get::<String,_>("visibility")? == "company"
        || row.try_get::<Uuid,_>("owner_user_id")? == actor.user_id
        || (row.try_get::<String,_>("visibility")? == "department" && sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM department_closure WHERE tenant_id=? AND ancestor_id=? AND descendant_id=?)")
            .bind(actor.tenant_id).bind(row.try_get::<Uuid,_>("owner_department_id")?).bind(actor.department_id).fetch_one(&state.pool).await?);
    if !visible {
        return Err(ApiError::forbidden(
            "Application is outside your data scope",
        ));
    }
    Ok(row.try_get("workflow_id")?)
}

#[derive(Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VersionContract {
    pub id: Uuid,
    pub version_number: u64,
    pub archived: bool,
    pub contract: TestContract,
}

pub(crate) async fn version_contracts(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
) -> ApiResult<Vec<VersionContract>> {
    let rows = sqlx::query("SELECT v.id,v.version_number,v.archived,v.definition_json FROM workflow_versions v JOIN applications a ON a.tenant_id=v.tenant_id AND a.workflow_id=v.workflow_id WHERE a.tenant_id=? AND a.id=? AND v.publication_status='published' ORDER BY v.version_number DESC")
        .bind(actor.tenant_id).bind(application).fetch_all(&state.pool).await?;
    let mut versions = Vec::new();
    for row in rows {
        let definition =
            serde_json::from_value(row.try_get("definition_json")?).map_err(ApiError::internal)?;
        let nodes =
            crate::workflow_wait_inputs::discover(&state.pool, actor.tenant_id, &definition)
                .await?;
        versions.push(VersionContract {
            id: row.try_get("id")?,
            version_number: row.try_get("version_number")?,
            archived: row.try_get("archived")?,
            contract: TestContract::from_definition(&definition).with_wait_nodes(nodes),
        });
    }
    Ok(versions)
}

async fn contracts(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
) -> ApiResult<Json<Vec<VersionContract>>> {
    authorize(&state, &actor, application, "dataset:view").await?;
    Ok(Json(version_contracts(&state, &actor, application).await?))
}

pub(crate) async fn selected_contract(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    version: Uuid,
) -> ApiResult<TestContract> {
    version_contracts(state, actor, application)
        .await?
        .into_iter()
        .find(|v| v.id == version)
        .map(|v| v.contract)
        .ok_or_else(|| {
            ApiError::unprocessable(
                "INVALID_TEST_VERSION",
                "Select a published version of this application's Workflow",
            )
        })
}

pub(crate) async fn lock_revision(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    application: Uuid,
    dataset: Uuid,
    expected: u64,
) -> ApiResult<TestContract> {
    let row = sqlx::query("SELECT revision,contract_json,status FROM datasets WHERE tenant_id=? AND application_id=? AND id=? FOR UPDATE")
        .bind(tenant).bind(application).bind(dataset).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::not_found("Dataset"))?;
    if row.try_get::<u64, _>("revision")? != expected {
        return Err(ApiError::conflict(
            "DATASET_REVISION_CONFLICT",
            "The dataset changed; reload its current revision",
        ));
    }
    if row.try_get::<String, _>("status")? != "active" {
        return Err(ApiError::unprocessable(
            "DATASET_DISABLED",
            "The dataset is disabled",
        ));
    }
    serde_json::from_value(row.try_get("contract_json")?).map_err(ApiError::internal)
}

pub(crate) async fn bump_revision(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    dataset: Uuid,
) -> ApiResult<()> {
    sqlx::query("UPDATE datasets SET revision=revision+1,updated_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=?").bind(tenant).bind(dataset).execute(&mut **tx).await?;
    Ok(())
}

pub(crate) fn hash(value: &impl Serialize) -> ApiResult<String> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(ApiError::internal)?)
    ))
}

#[derive(Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CaseDraft {
    #[serde(default)]
    pub wait_inputs: agentx_runtime_contracts::WaitInputsV1,
    pub name: String,
    pub input: Value,
    pub expected_output: Value,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pending_fields: Vec<String>,
    pub source_invocation_id: Option<Uuid>,
    #[serde(default = "confirmed")]
    pub reference_confirmed: bool,
}
fn confirmed() -> bool {
    true
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ValidateCase {
    pub workflow_version_id: Uuid,
    #[serde(flatten)]
    pub draft: CaseDraft,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaseValidation {
    pub validation_errors: Vec<CaseFieldError>,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InvocationSample {
    pub wait_inputs: agentx_runtime_contracts::WaitInputsV1,
    pub invocation_id: Uuid,
    pub workflow_version_id: Uuid,
    pub contract: TestContract,
    pub input: Value,
    pub expected_output: Value,
    pub name: String,
    pub reference_confirmed: bool,
    pub validation_errors: Vec<CaseFieldError>,
}

async fn validate_case(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
    Json(input): Json<ValidateCase>,
) -> ApiResult<Json<CaseValidation>> {
    authorize(&state, &actor, application, "dataset:manage").await?;
    let contract =
        selected_contract(&state, &actor, application, input.workflow_version_id).await?;
    Ok(Json(CaseValidation {
        validation_errors: validate_draft(&state, &actor, application, &contract, input.draft)
            .await?,
    }))
}

/// Authoring validates current data independently from the user's final approval.
pub(crate) async fn validate_draft(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    contract: &TestContract,
    mut draft: CaseDraft,
) -> ApiResult<Vec<CaseFieldError>> {
    crate::workflow_wait_inputs::validate_plan(&contract.wait_input_nodes, &draft.wait_inputs)?;
    draft.pending_fields = datasets::remaining_fields(&draft, contract);
    let mut errors = contract.errors(&draft.input, &draft.expected_output);
    errors.extend(draft.pending_fields.iter().map(|field| CaseFieldError {
        field: field.clone(),
        code: CaseErrorCode::PendingField,
        message: "Fill this field or explicitly confirm leaving it unused".into(),
    }));
    let mut cases = [CaseResponse {
        id: Uuid::nil(),
        case_key: String::new(),
        lineage_id: Uuid::nil(),
        version: 0,
        sort_order: 0,
        draft,
        validation_errors: errors,
    }];
    crate::runtime_bff::test_artifacts(state, actor, application, contract, &mut cases).await?;
    Ok(std::mem::take(&mut cases[0].validation_errors))
}

#[derive(Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaseResponse {
    pub id: Uuid,
    pub case_key: String,
    pub lineage_id: Uuid,
    pub version: u64,
    pub sort_order: u64,
    #[serde(flatten)]
    pub draft: CaseDraft,
    pub validation_errors: Vec<CaseFieldError>,
}

pub(crate) fn case_from_row(
    row: sqlx::mysql::MySqlRow,
    contract: &TestContract,
) -> ApiResult<CaseResponse> {
    let input = row.try_get("input_json")?;
    let expected = row
        .try_get::<Option<Value>, _>("expected_output_json")?
        .unwrap_or(Value::Null);
    let mut errors = contract.errors(&input, &expected);
    let pending: Vec<String> =
        serde_json::from_value(row.try_get("pending_fields_json")?).map_err(ApiError::internal)?;
    errors.extend(pending.iter().map(|field| CaseFieldError {
        field: field.clone(),
        code: CaseErrorCode::PendingField,
        message: "Fill this field or explicitly confirm leaving it unused".into(),
    }));
    let confirmed: bool = row.try_get("reference_confirmed")?;
    if !confirmed {
        errors.push(CaseFieldError {
            field: "expectedOutput".into(),
            code: CaseErrorCode::ReferenceUnconfirmed,
            message: "Confirm the reference answer before evaluation".into(),
        });
    }
    Ok(CaseResponse {
        id: row.try_get("id")?,
        case_key: row.try_get("case_key")?,
        lineage_id: row.try_get("lineage_id")?,
        version: row.try_get("version")?,
        sort_order: row.try_get("sort_order")?,
        draft: CaseDraft {
            wait_inputs: serde_json::from_value(row.try_get("wait_inputs_json")?)
                .map_err(ApiError::internal)?,
            name: row.try_get("name")?,
            input,
            expected_output: expected,
            pending_fields: pending,
            tags: serde_json::from_value(row.try_get("tags_json")?).map_err(ApiError::internal)?,
            source_invocation_id: row.try_get("source_invocation_id")?,
            reference_confirmed: confirmed,
        },
        validation_errors: errors,
    })
}

pub(crate) async fn insert_case(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    dataset: Uuid,
    lineage: Option<Uuid>,
    order: u64,
    draft: &CaseDraft,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO dataset_cases(id,tenant_id,dataset_id,case_key,name,input_json,expected_output_json,wait_inputs_json,context_json,tags_json,pending_fields_json,lineage_id,source_invocation_id,reference_confirmed,sort_order) VALUES(?,?,?,?,?,?,?,?,NULL,?,?,?,?,?,?)")
        .bind(id).bind(tenant).bind(dataset).bind(id.to_string()).bind(crate::control_helpers::required_name(&draft.name)?).bind(&draft.input).bind(&draft.expected_output).bind(json!(draft.wait_inputs)).bind(json!(draft.tags)).bind(json!(draft.pending_fields)).bind(lineage.unwrap_or(id)).bind(draft.source_invocation_id).bind(draft.reference_confirmed).bind(order).execute(&mut **tx).await?;
    Ok(id)
}

/// Map the database constraint as well as a racing create to the same form error.
pub(crate) fn name_conflict(error: sqlx::Error, kind: &'static str) -> ApiError {
    if error
        .as_database_error()
        .is_some_and(|error| error.is_unique_violation())
    {
        ApiError::conflict(kind, "This name already exists in the application").with_field_error(
            "name",
            "NOT_UNIQUE",
            "Choose another name",
        )
    } else {
        error.into()
    }
}
