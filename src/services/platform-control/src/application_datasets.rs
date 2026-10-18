use super::*;
use crate::test_contract::project;
use axum::{extract::Query, http::StatusCode};

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DatasetResponse {
    pub id: Uuid,
    pub application_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub revision: u64,
    pub version: u64,
    pub source_workflow_version_id: Uuid,
    pub contract: TestContract,
    pub case_count: usize,
    pub incomplete_count: usize,
    pub compatible_versions: Vec<Uuid>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateDataset {
    pub name: String,
    pub description: Option<String>,
    pub workflow_version_id: Uuid,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateDatasetRequest {
    #[serde(flatten)]
    pub dataset: CreateDataset,
    pub initial_case: Option<CaseDraft>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateDataset {
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub version: u64,
}

pub(crate) async fn load(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    dataset: Uuid,
) -> ApiResult<DatasetResponse> {
    let row = sqlx::query("SELECT * FROM datasets WHERE tenant_id=? AND application_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(application)
        .bind(dataset)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Dataset"))?;
    let contract: TestContract =
        serde_json::from_value(row.try_get("contract_json")?).map_err(ApiError::internal)?;
    let mut cases = load_cases(state, actor.tenant_id, dataset, &contract).await?;
    crate::runtime_bff::test_artifacts(state, actor, application, &contract, &mut cases).await?;
    let compatible_versions = version_contracts(state, actor, application)
        .await?
        .into_iter()
        .filter(|version| version.contract.fingerprint == contract.fingerprint)
        .map(|version| version.id)
        .collect();
    Ok(DatasetResponse {
        id: dataset,
        application_id: application,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
        revision: row.try_get("revision")?,
        version: row.try_get("version")?,
        source_workflow_version_id: row.try_get("source_workflow_version_id")?,
        contract,
        case_count: cases.len(),
        incomplete_count: cases
            .iter()
            .filter(|case| !case.validation_errors.is_empty())
            .count(),
        compatible_versions,
    })
}

pub(crate) async fn load_cases(
    state: &ControlApiState,
    tenant: Uuid,
    dataset: Uuid,
    contract: &TestContract,
) -> ApiResult<Vec<CaseResponse>> {
    sqlx::query(
        "SELECT * FROM dataset_cases WHERE tenant_id=? AND dataset_id=? ORDER BY sort_order,id",
    )
    .bind(tenant)
    .bind(dataset)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|row| case_from_row(row, contract))
    .collect()
}

pub(crate) async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
) -> ApiResult<Json<Vec<DatasetResponse>>> {
    authorize(&state, &actor, application, "dataset:view").await?;
    let ids=sqlx::query_scalar("SELECT id FROM datasets WHERE tenant_id=? AND application_id=? ORDER BY updated_at DESC,id").bind(actor.tenant_id).bind(application).fetch_all(&state.pool).await?;
    let mut values = Vec::new();
    for id in ids {
        values.push(load(&state, &actor, application, id).await?);
    }
    Ok(Json(values))
}

pub(crate) async fn get(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<DatasetResponse>> {
    authorize(&state, &actor, application, "dataset:view").await?;
    Ok(Json(load(&state, &actor, application, dataset).await?))
}

async fn insert_dataset(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    application: Uuid,
    input: &CreateDataset,
    contract: &TestContract,
    source: Option<Uuid>,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    let name = crate::control_helpers::required_name(&input.name)?;
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM datasets WHERE tenant_id=? AND application_id=? AND name=?)",
    )
    .bind(actor.tenant_id)
    .bind(application)
    .bind(&name)
    .fetch_one(&mut **tx)
    .await?;
    if duplicate {
        return Err(ApiError::conflict(
            "DATASET_NAME_EXISTS",
            "A dataset with this name already exists",
        )
        .with_field_error("name", "NOT_UNIQUE", "Choose another name"));
    }
    sqlx::query("INSERT INTO datasets(id,tenant_id,application_id,source_workflow_version_id,contract_json,contract_fingerprint,source_dataset_id,name,description,owner_user_id,owner_department_id) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
        .bind(id).bind(actor.tenant_id).bind(application).bind(input.workflow_version_id).bind(json!(contract)).bind(&contract.fingerprint).bind(source).bind(name).bind(&input.description).bind(actor.user_id).bind(actor.department_id).execute(&mut **tx).await.map_err(|error|name_conflict(error,"DATASET_NAME_EXISTS"))?;
    Ok(id)
}

pub(crate) async fn create(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
    Json(mut input): Json<CreateDatasetRequest>,
) -> ApiResult<(StatusCode, Json<DatasetResponse>)> {
    authorize(&state, &actor, application, "dataset:manage").await?;
    let contract = selected_contract(
        &state,
        &actor,
        application,
        input.dataset.workflow_version_id,
    )
    .await?;
    if let Some(draft) = &mut input.initial_case {
        check_invocation_contract(&state, &actor, application, &contract, draft).await?;
        draft.pending_fields = remaining_fields(draft, &contract);
    }
    let mut tx = state.pool.begin().await?;
    let id = insert_dataset(
        &mut tx,
        &actor,
        application,
        &input.dataset,
        &contract,
        None,
    )
    .await?;
    if let Some(draft) = &input.initial_case {
        insert_case(&mut tx, actor.tenant_id, id, None, 1, draft).await?;
        bump_revision(&mut tx, actor.tenant_id, id).await?;
    }
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load(&state, &actor, application, id).await?),
    ))
}

async fn check_invocation_contract(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    contract: &TestContract,
    draft: &CaseDraft,
) -> ApiResult<()> {
    if let Some(id) = draft.source_invocation_id {
        let sample = crate::runtime_bff::test_sample(state, actor, application, id).await?;
        if sample
            .pointer("/contract/fingerprint")
            .and_then(Value::as_str)
            != Some(contract.fingerprint.as_str())
        {
            return Err(ApiError::unprocessable(
                "TEST_CONTRACT_MISMATCH",
                "Invocation and dataset contracts differ",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn update(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset)): Path<(Uuid, Uuid)>,
    Json(input): Json<UpdateDataset>,
) -> ApiResult<Json<DatasetResponse>> {
    authorize(&state, &actor, application, "dataset:manage").await?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_STATUS",
            "Invalid dataset status",
        ));
    }
    let result=sqlx::query("UPDATE datasets SET name=?,description=?,status=?,version=version+1 WHERE tenant_id=? AND application_id=? AND id=? AND version=?")
        .bind(crate::control_helpers::required_name(&input.name)?).bind(&input.description).bind(&input.status).bind(actor.tenant_id).bind(application).bind(dataset).bind(input.version).execute(&state.pool).await.map_err(|error|name_conflict(error,"DATASET_NAME_EXISTS"))?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "DATASET_VERSION_CONFLICT",
            "Dataset changed; reload before editing",
        ));
    }
    Ok(Json(load(&state, &actor, application, dataset).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteVersion {
    pub expected_version: u64,
}
pub(crate) async fn delete(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset)): Path<(Uuid, Uuid)>,
    Query(input): Query<DeleteVersion>,
) -> ApiResult<StatusCode> {
    authorize(&state, &actor, application, "dataset:delete").await?;
    load(&state, &actor, application, dataset).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM applications WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id)
        .bind(application)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "SELECT id FROM datasets WHERE tenant_id=? AND application_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(application)
    .bind(dataset)
    .fetch_one(&mut *tx)
    .await?;
    let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM evaluation_runs r JOIN dataset_versions v ON v.tenant_id=r.tenant_id AND v.id=r.dataset_version_id WHERE r.tenant_id=? AND v.dataset_id=?)").bind(actor.tenant_id).bind(dataset).fetch_one(&mut *tx).await?;
    if used {
        return Err(ApiError::conflict(
            "ENTITY_IN_USE",
            "Dataset is referenced by immutable evaluation history",
        ));
    }
    let changed = sqlx::query(
        "DELETE FROM datasets WHERE tenant_id=? AND application_id=? AND id=? AND version=?",
    )
    .bind(actor.tenant_id)
    .bind(application)
    .bind(dataset)
    .bind(input.expected_version)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "DATASET_VERSION_CONFLICT",
            "Dataset changed",
        ));
    }
    sqlx::query("DELETE c FROM dataset_version_cases c JOIN dataset_versions v ON v.tenant_id=c.tenant_id AND v.id=c.dataset_version_id WHERE v.tenant_id=? AND v.dataset_id=?").bind(actor.tenant_id).bind(dataset).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM dataset_versions WHERE tenant_id=? AND dataset_id=?")
        .bind(actor.tenant_id)
        .bind(dataset)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM dataset_cases WHERE tenant_id=? AND dataset_id=?")
        .bind(actor.tenant_id)
        .bind(dataset)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn cases(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<CaseResponse>>> {
    authorize(&state, &actor, application, "dataset:view").await?;
    let data = load(&state, &actor, application, dataset).await?;
    let mut cases = load_cases(&state, actor.tenant_id, dataset, &data.contract).await?;
    crate::runtime_bff::test_artifacts(&state, &actor, application, &data.contract, &mut cases)
        .await?;
    Ok(Json(cases))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveCase {
    pub expected_revision: u64,
    pub version: Option<u64>,
    #[serde(flatten)]
    pub draft: CaseDraft,
}

pub(crate) async fn save_case(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset)): Path<(Uuid, Uuid)>,
    Json(input): Json<SaveCase>,
) -> ApiResult<Json<Value>> {
    save(&state, &actor, application, dataset, None, input)
        .await
        .map(Json)
}
pub(crate) async fn edit_case(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset, case)): Path<(Uuid, Uuid, Uuid)>,
    Json(input): Json<SaveCase>,
) -> ApiResult<Json<Value>> {
    save(&state, &actor, application, dataset, Some(case), input)
        .await
        .map(Json)
}
async fn save(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    dataset: Uuid,
    case: Option<Uuid>,
    mut input: SaveCase,
) -> ApiResult<Value> {
    authorize(state, actor, application, "dataset:manage").await?;
    let source_contract =
        if let Some(id) = input.draft.source_invocation_id.filter(|_| case.is_none()) {
            Some(crate::runtime_bff::test_sample(state, actor, application, id).await?)
        } else {
            None
        };
    let mut tx = state.pool.begin().await?;
    let contract = lock_revision(
        &mut tx,
        actor.tenant_id,
        application,
        dataset,
        input.expected_revision,
    )
    .await?;
    if source_contract.as_ref().is_some_and(|sample| {
        sample
            .pointer("/contract/fingerprint")
            .and_then(Value::as_str)
            != Some(contract.fingerprint.as_str())
    }) {
        return Err(ApiError::unprocessable(
            "TEST_CONTRACT_MISMATCH",
            "Invocation and dataset contracts differ",
        ));
    }
    input.draft.pending_fields = remaining_fields(&input.draft, &contract);
    let id = if let Some(id) = case {
        let provenance:Option<Uuid>=sqlx::query_scalar("SELECT source_invocation_id FROM dataset_cases WHERE tenant_id=? AND dataset_id=? AND id=? FOR UPDATE").bind(actor.tenant_id).bind(dataset).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Test case"))?;
        if input.draft.source_invocation_id != provenance {
            return Err(ApiError::bad_request(
                "IMMUTABLE_CASE_PROVENANCE",
                "An existing case's invocation provenance cannot be changed",
            ));
        }

        let changed=sqlx::query("UPDATE dataset_cases SET name=?,input_json=?,expected_output_json=?,tags_json=?,pending_fields_json=?,source_invocation_id=?,reference_confirmed=?,version=version+1 WHERE tenant_id=? AND dataset_id=? AND id=? AND version=?")
            .bind(crate::control_helpers::required_name(&input.draft.name)?).bind(&input.draft.input).bind(&input.draft.expected_output).bind(json!(input.draft.tags)).bind(json!(input.draft.pending_fields)).bind(input.draft.source_invocation_id).bind(input.draft.reference_confirmed).bind(actor.tenant_id).bind(dataset).bind(id).bind(input.version).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::conflict(
                "CASE_VERSION_CONFLICT",
                "The case changed; reload it",
            ));
        }
        id
    } else {
        let order:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(sort_order),0)+1 AS UNSIGNED) FROM dataset_cases WHERE tenant_id=? AND dataset_id=?").bind(actor.tenant_id).bind(dataset).fetch_one(&mut *tx).await?;
        insert_case(&mut tx, actor.tenant_id, dataset, None, order, &input.draft).await?
    };
    bump_revision(&mut tx, actor.tenant_id, dataset).await?;
    tx.commit().await?;
    Ok(
        json!({"id":id,"validationErrors":contract.errors(&input.draft.input,&input.draft.expected_output)}),
    )
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Revision {
    pub expected_revision: u64,
}
pub(crate) async fn delete_case(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset, case)): Path<(Uuid, Uuid, Uuid)>,
    Json(input): Json<Revision>,
) -> ApiResult<StatusCode> {
    authorize(&state, &actor, application, "dataset:manage").await?;
    let mut tx = state.pool.begin().await?;
    lock_revision(
        &mut tx,
        actor.tenant_id,
        application,
        dataset,
        input.expected_revision,
    )
    .await?;
    sqlx::query("DELETE FROM dataset_cases WHERE tenant_id=? AND dataset_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(dataset)
        .bind(case)
        .execute(&mut *tx)
        .await?;
    bump_revision(&mut tx, actor.tenant_id, dataset).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CopyDataset {
    pub expected_revision: u64,
    #[serde(flatten)]
    pub target: CreateDataset,
}
fn copied_draft(case: CaseDraft, old: &TestContract, target: &TestContract) -> CaseDraft {
    let input = project(&case.input, &old.input_schema, &target.input_schema);
    let expected_output = project(
        &case.expected_output,
        &old.output_schema,
        &target.output_schema,
    );
    let mut pending_fields = case
        .pending_fields
        .iter()
        .filter(|field| contract_field(target, field).is_some())
        .cloned()
        .collect::<Vec<_>>();
    unmapped_fields(
        &case.input,
        &old.input_schema,
        &target.input_schema,
        &input,
        "input",
        &mut pending_fields,
    );
    unmapped_fields(
        &case.expected_output,
        &old.output_schema,
        &target.output_schema,
        &expected_output,
        "expectedOutput",
        &mut pending_fields,
    );
    pending_fields.sort();
    pending_fields.dedup();
    CaseDraft {
        input,
        expected_output,
        pending_fields,
        reference_confirmed: case.reference_confirmed && old.fingerprint == target.fingerprint,
        ..case
    }
}
fn unmapped_fields(
    source: &Value,
    old: &Value,
    target: &Value,
    projected: &Value,
    prefix: &str,
    missing: &mut Vec<String>,
) {
    for (name, schema) in target
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let field = format!("{prefix}/{}", name.replace('~', "~0").replace('/', "~1"));
        let previous = old.get("properties").and_then(|p| p.get(name));
        if previous.is_none() || source.get(name).is_some() && projected.get(name).is_none() {
            missing.push(field);
            continue;
        }
        if schema.get("properties").is_some() && source.get(name).is_some() {
            unmapped_fields(
                &source[name],
                previous.unwrap(),
                schema,
                &projected[name],
                &field,
                missing,
            );
        }
    }
}
fn contract_field<'a>(contract: &'a TestContract, field: &str) -> Option<&'a Value> {
    let (root, path) = field.split_once('/')?;
    let mut schema = match root {
        "input" => &contract.input_schema,
        "expectedOutput" => &contract.output_schema,
        _ => return None,
    };
    for segment in path.split('/') {
        schema = schema
            .get("properties")?
            .get(segment.replace("~1", "/").replace("~0", "~"))?;
    }
    Some(schema)
}
pub(super) fn remaining_fields(draft: &CaseDraft, contract: &TestContract) -> Vec<String> {
    draft
        .pending_fields
        .iter()
        .filter(|field| {
            let Some((root, path)) = field.split_once('/') else {
                return false;
            };
            let value = if root == "input" {
                &draft.input
            } else {
                &draft.expected_output
            };
            let schema = contract_field(contract, field);
            schema.is_some_and(|schema| {
                value.pointer(&format!("/{path}")).is_none_or(|value| {
                    !crate::test_contract::validate(schema, value, field).is_empty()
                })
            })
        })
        .cloned()
        .collect()
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopyPreview {
    pub case_count: usize,
    pub incomplete_count: usize,
    pub contract_changed: bool,
    pub cases: Vec<CaseResponse>,
}
pub(crate) async fn copy_preview(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset)): Path<(Uuid, Uuid)>,
    Json(input): Json<CopyDataset>,
) -> ApiResult<Json<CopyPreview>> {
    authorize(&state, &actor, application, "dataset:manage").await?;
    let target = selected_contract(
        &state,
        &actor,
        application,
        input.target.workflow_version_id,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let old = lock_revision(
        &mut tx,
        actor.tenant_id,
        application,
        dataset,
        input.expected_revision,
    )
    .await?;
    let rows = sqlx::query(
        "SELECT * FROM dataset_cases WHERE tenant_id=? AND dataset_id=? ORDER BY sort_order,id",
    )
    .bind(actor.tenant_id)
    .bind(dataset)
    .fetch_all(&mut *tx)
    .await?;
    let mut cases = Vec::new();
    for row in rows {
        let mut case = case_from_row(row, &old)?;
        case.draft = copied_draft(case.draft, &old, &target);
        case.validation_errors = target.errors(&case.draft.input, &case.draft.expected_output);
        case.validation_errors
            .extend(
                case.draft
                    .pending_fields
                    .iter()
                    .map(|field| CaseFieldError {
                        field: field.clone(),
                        code: CaseErrorCode::PendingField,
                        message: "Fill or confirm this field".into(),
                    }),
            );
        if !case.draft.reference_confirmed {
            case.validation_errors.push(CaseFieldError {
                field: "expectedOutput".into(),
                code: CaseErrorCode::ReferenceUnconfirmed,
                message: "Confirm the reference answer".into(),
            });
        }
        cases.push(case);
    }
    tx.commit().await?;
    Ok(Json(CopyPreview {
        contract_changed: old.fingerprint != target.fingerprint,
        case_count: cases.len(),
        incomplete_count: cases
            .iter()
            .filter(|case| !case.validation_errors.is_empty())
            .count(),
        cases,
    }))
}
pub(crate) async fn copy(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset)): Path<(Uuid, Uuid)>,
    Json(input): Json<CopyDataset>,
) -> ApiResult<(StatusCode, Json<DatasetResponse>)> {
    authorize(&state, &actor, application, "dataset:manage").await?;
    let target = selected_contract(
        &state,
        &actor,
        application,
        input.target.workflow_version_id,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let old = lock_revision(
        &mut tx,
        actor.tenant_id,
        application,
        dataset,
        input.expected_revision,
    )
    .await?;
    let source = sqlx::query(
        "SELECT * FROM dataset_cases WHERE tenant_id=? AND dataset_id=? ORDER BY sort_order,id",
    )
    .bind(actor.tenant_id)
    .bind(dataset)
    .fetch_all(&mut *tx)
    .await?;
    let id = insert_dataset(
        &mut tx,
        &actor,
        application,
        &input.target,
        &target,
        Some(dataset),
    )
    .await?;
    for row in source {
        let case = case_from_row(row, &old)?;
        let draft = copied_draft(case.draft, &old, &target);
        insert_case(
            &mut tx,
            actor.tenant_id,
            id,
            Some(case.lineage_id),
            case.sort_order,
            &draft,
        )
        .await?;
    }
    bump_revision(&mut tx, actor.tenant_id, id).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load(&state, &actor, application, id).await?),
    ))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportCases {
    pub expected_revision: u64,
    pub content: String,
    pub format: String,
}
pub(crate) async fn import(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset)): Path<(Uuid, Uuid)>,
    Json(input): Json<ImportCases>,
) -> ApiResult<Json<Value>> {
    authorize(&state, &actor, application, "dataset:manage").await?;
    let cases = parse_import(&input.format, &input.content)?;
    let mut tx = state.pool.begin().await?;
    lock_revision(
        &mut tx,
        actor.tenant_id,
        application,
        dataset,
        input.expected_revision,
    )
    .await?;
    let order:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(sort_order),0) AS UNSIGNED) FROM dataset_cases WHERE tenant_id=? AND dataset_id=?").bind(actor.tenant_id).bind(dataset).fetch_one(&mut *tx).await?;
    for (index, draft) in cases.iter().enumerate() {
        if draft.source_invocation_id.is_some() {
            return Err(ApiError::bad_request(
                "INVALID_DATASET_IMPORT",
                "Import cannot assert invocation provenance",
            ));
        }
        insert_case(
            &mut tx,
            actor.tenant_id,
            dataset,
            None,
            order + index as u64 + 1,
            draft,
        )
        .await?;
    }
    bump_revision(&mut tx, actor.tenant_id, dataset).await?;
    tx.commit().await?;
    Ok(Json(json!({"importedCount":cases.len()})))
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExportCases {
    pub expected_revision: u64,
    pub case_ids: Vec<Uuid>,
}

pub(crate) async fn export(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, dataset)): Path<(Uuid, Uuid)>,
    Json(input): Json<ExportCases>,
) -> ApiResult<String> {
    authorize(&state, &actor, application, "dataset:view").await?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT revision,contract_json FROM datasets WHERE tenant_id=? AND application_id=? AND id=? FOR SHARE")
        .bind(actor.tenant_id).bind(application).bind(dataset).fetch_optional(&mut *tx).await?
        .ok_or_else(||ApiError::not_found("Dataset"))?;
    if row.try_get::<u64, _>("revision")? != input.expected_revision {
        return Err(ApiError::conflict(
            "DATASET_REVISION_CONFLICT",
            "The dataset changed; reload its current revision",
        ));
    }
    let contract =
        serde_json::from_value(row.try_get("contract_json")?).map_err(ApiError::internal)?;
    let rows = sqlx::query(
        "SELECT * FROM dataset_cases WHERE tenant_id=? AND dataset_id=? ORDER BY sort_order,id",
    )
    .bind(actor.tenant_id)
    .bind(dataset)
    .fetch_all(&mut *tx)
    .await?;
    let cases = rows
        .into_iter()
        .map(|row| case_from_row(row, &contract))
        .collect::<ApiResult<Vec<_>>>()?;
    let content = export_selected(cases, &input.case_ids)?;
    tx.commit().await?;
    Ok(content)
}

fn export_selected(cases: Vec<CaseResponse>, requested: &[Uuid]) -> ApiResult<String> {
    let ids = requested
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if ids.is_empty() {
        return Err(ApiError::unprocessable(
            "INVALID_DATASET_EXPORT",
            "Select at least one case",
        ));
    }
    let cases = cases
        .into_iter()
        .filter(|case| ids.contains(&case.id))
        .collect::<Vec<_>>();
    if cases.len() != ids.len() {
        return Err(ApiError::unprocessable(
            "INVALID_DATASET_EXPORT",
            "Some selected cases do not belong to this dataset",
        ));
    }
    cases
        .into_iter()
        .map(|case| {
            let mut draft = case.draft;
            draft.source_invocation_id = None;
            serde_json::to_string(&draft).map_err(ApiError::internal)
        })
        .collect::<ApiResult<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

pub(crate) async fn invocation_sample(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, invocation)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    authorize(&state, &actor, application, "dataset:manage").await?;
    Ok(Json(
        crate::runtime_bff::test_sample(&state, &actor, application, invocation).await?,
    ))
}

pub(crate) async fn snapshot(
    state: &ControlApiState,
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    application: Uuid,
    dataset: Uuid,
    revision: u64,
) -> ApiResult<Uuid> {
    let contract = lock_revision(tx, actor.tenant_id, application, dataset, revision).await?;
    let rows = sqlx::query(
        "SELECT * FROM dataset_cases WHERE tenant_id=? AND dataset_id=? ORDER BY sort_order,id",
    )
    .bind(actor.tenant_id)
    .bind(dataset)
    .fetch_all(&mut **tx)
    .await?;
    let mut cases = rows
        .into_iter()
        .map(|row| case_from_row(row, &contract))
        .collect::<ApiResult<Vec<_>>>()?;
    crate::runtime_bff::test_artifacts(state, actor, application, &contract, &mut cases).await?;
    if cases.is_empty() {
        return Err(ApiError::unprocessable(
            "DATASET_EMPTY",
            "Add at least one test case",
        ));
    }
    let incomplete = cases
        .iter()
        .filter(|case| !case.validation_errors.is_empty())
        .collect::<Vec<_>>();
    if !incomplete.is_empty() {
        return Err(ApiError::unprocessable(
            "DATASET_INCOMPLETE",
            "Complete all cases before evaluation",
        )
        .with_details(json!({"datasetId":dataset,"cases":incomplete})));
    }
    if let Some(id) = sqlx::query_scalar(
        "SELECT id FROM dataset_versions WHERE tenant_id=? AND dataset_id=? AND source_revision=?",
    )
    .bind(actor.tenant_id)
    .bind(dataset)
    .bind(revision)
    .fetch_optional(&mut **tx)
    .await?
    {
        return Ok(id);
    }
    let id = Uuid::now_v7();
    let number:u64=sqlx::query_scalar("SELECT CAST(COALESCE(MAX(version_number),0)+1 AS UNSIGNED) FROM dataset_versions WHERE tenant_id=? AND dataset_id=?").bind(actor.tenant_id).bind(dataset).fetch_one(&mut **tx).await?;
    sqlx::query("INSERT INTO dataset_versions(id,tenant_id,dataset_id,version_number,source_revision,content_hash,case_count,created_by) VALUES(?,?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(dataset).bind(number).bind(revision).bind(hash(&cases)?).bind(cases.len() as u64).bind(actor.user_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO dataset_version_cases(tenant_id,dataset_version_id,source_case_id,case_key,name,input_json,expected_output_json,context_json,tags_json,pending_fields_json,lineage_id,source_invocation_id,reference_confirmed,sort_order,content_hash) SELECT tenant_id,?,id,case_key,name,input_json,expected_output_json,context_json,tags_json,pending_fields_json,lineage_id,source_invocation_id,reference_confirmed,sort_order,SHA2(CONCAT(CAST(input_json AS CHAR),CAST(expected_output_json AS CHAR)),256) FROM dataset_cases WHERE tenant_id=? AND dataset_id=?").bind(id).bind(actor.tenant_id).bind(dataset).execute(&mut **tx).await?;
    Ok(id)
}

fn parse_import(format: &str, content: &str) -> ApiResult<Vec<CaseDraft>> {
    let invalid = |error: String| ApiError::bad_request("INVALID_DATASET_IMPORT", error);
    match format {
        "jsonl" => content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .enumerate()
            .map(|(i, line)| {
                serde_json::from_str(line)
                    .map_err(|error| invalid(format!("Line {}: {error}", i + 1)))
            })
            .collect(),
        "csv" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct CsvCase {
                name: String,
                input: String,
                expected_output: String,
                #[serde(default)]
                tags: String,
            }
            csv::Reader::from_reader(content.as_bytes())
                .deserialize::<CsvCase>()
                .enumerate()
                .map(|(i, row)| {
                    let row = row.map_err(|error| invalid(format!("Row {}: {error}", i + 2)))?;
                    Ok(CaseDraft {
                        name: row.name,
                        input: serde_json::from_str(&row.input)
                            .map_err(|error| invalid(format!("Row {} input: {error}", i + 2)))?,
                        expected_output: serde_json::from_str(&row.expected_output).map_err(
                            |error| invalid(format!("Row {} expectedOutput: {error}", i + 2)),
                        )?,
                        tags: row
                            .tags
                            .split(',')
                            .map(str::trim)
                            .filter(|tag| !tag.is_empty())
                            .map(str::to_owned)
                            .collect(),
                        pending_fields: Vec::new(),
                        source_invocation_id: None,
                        reference_confirmed: true,
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
    fn dataset_creation_accepts_an_atomic_initial_case() {
        let version = Uuid::now_v7();
        let plain: CreateDatasetRequest = serde_json::from_value(json!({
            "name":"Empty", "workflowVersionId":version,
        }))
        .unwrap();
        assert!(plain.initial_case.is_none());
        let seeded: CreateDatasetRequest = serde_json::from_value(json!({
            "name":"Seeded", "workflowVersionId":version,
            "initialCase":{"name":"Hello", "input":{"message":"hello"},
                "expectedOutput":{"answer":"hello"},"referenceConfirmed":true},
        }))
        .unwrap();
        assert_eq!(seeded.dataset.workflow_version_id, version);
        assert!(seeded.initial_case.unwrap().reference_confirmed);
        let validation: ValidateCase = serde_json::from_value(json!({
            "workflowVersionId":version,"name":"Draft","input":{},"expectedOutput":{},
        }))
        .unwrap();
        assert_eq!(validation.workflow_version_id, version);
    }

    #[test]
    fn copying_optional_additions_requires_explicit_completion() {
        let old = TestContract {
            input_schema: json!({"type":"object","properties":{"question":{"type":"string"}}}),
            output_schema: json!({}),
            fingerprint: "old".into(),
        };
        let target = TestContract {
            input_schema: json!({"type":"object","properties":{"question":{"type":"string"},"optional":{"type":"string"}}}),
            output_schema: json!({}),
            fingerprint: "new".into(),
        };
        let draft = CaseDraft {
            name: "Case".into(),
            input: json!({"question":"hello"}),
            expected_output: json!({}),
            tags: vec![],
            pending_fields: vec![],
            source_invocation_id: None,
            reference_confirmed: true,
        };
        let copied = copied_draft(draft, &old, &target);
        assert!(!copied.reference_confirmed);
        assert_eq!(copied.input, json!({"question":"hello"}));
        assert_eq!(copied.pending_fields, vec!["input/optional"]);
        assert_eq!(remaining_fields(&copied, &target), vec!["input/optional"]);
        let mut completed = copied;
        completed.input["optional"] = json!("");
        assert!(remaining_fields(&completed, &target).is_empty());
    }

    #[test]
    fn changed_contract_requires_review_even_when_all_copied_values_are_valid() {
        let old = TestContract {
            input_schema: json!({"type":"object","properties":{"question":{"type":"string","minLength":3}},"required":["question"]}),
            output_schema: json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}),
            fingerprint: "old".into(),
        };
        let mut target = old.clone();
        target.input_schema["properties"]["question"]["minLength"] = json!(1);
        target.fingerprint = "new".into();
        let draft = CaseDraft {
            name: "Reviewed source".into(),
            input: json!({"question":"hello"}),
            expected_output: json!({"answer":"hello"}),
            tags: vec![],
            pending_fields: vec![],
            source_invocation_id: None,
            reference_confirmed: true,
        };
        let copied = copied_draft(draft.clone(), &old, &target);
        assert!(!copied.reference_confirmed);
        assert!(
            target
                .errors(&copied.input, &copied.expected_output)
                .is_empty()
        );
        assert!(copied.pending_fields.is_empty());
        assert_eq!(copied.input, draft.input);
        assert!(draft.reference_confirmed);
        assert!(copied_draft(draft.clone(), &old, &old).reference_confirmed);
        assert!(
            !copied_draft(
                CaseDraft {
                    reference_confirmed: false,
                    ..draft
                },
                &old,
                &old
            )
            .reference_confirmed
        );
    }

    #[test]
    fn export_requires_selection_and_never_exports_unselected_or_foreign_cases() {
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let cases = || {
            [first, second]
                .into_iter()
                .enumerate()
                .map(|(index, id)| CaseResponse {
                    id,
                    case_key: format!("case-{index}"),
                    lineage_id: Uuid::now_v7(),
                    version: 1,
                    sort_order: index as u64,
                    draft: CaseDraft {
                        name: format!("Case {index}"),
                        input: json!({"question":index}),
                        expected_output: json!({}),
                        tags: vec![],
                        pending_fields: vec![],
                        source_invocation_id: Some(Uuid::now_v7()),
                        reference_confirmed: true,
                    },
                    validation_errors: vec![],
                })
                .collect()
        };
        let selected: Value =
            serde_json::from_str(&export_selected(cases(), &[second, second]).unwrap()).unwrap();
        assert_eq!(selected["name"], "Case 1");
        assert!(selected["sourceInvocationId"].is_null());
        let all = export_selected(cases(), &[second, first]).unwrap();
        let lines = all
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["name"], "Case 0");
        assert!(export_selected(cases(), &[]).is_err());
        assert!(export_selected(cases(), &[first, Uuid::now_v7()]).is_err());
    }

    #[test]
    fn import_accepts_structured_csv_and_keeps_incomplete_jsonl() {
        let cases = parse_import(
            "csv",
            r#"name,input,expectedOutput,tags
Case,"{""q"":""hello""}",{},tag
"#,
        )
        .unwrap();
        assert_eq!(cases[0].input, json!({"q":"hello"}));
        assert_eq!(
            parse_import(
                "jsonl",
                r#"{"name":"Draft","input":{},"expectedOutput":{}}"#
            )
            .unwrap()
            .len(),
            1
        );
        assert!(parse_import("csv", "name,input,expectedOutput\nx,broken,{}").is_err());
        assert!(parse_import("yaml", "").is_err());
    }
}
