use super::*;
use serde_json::json;
pub(super) fn apply(contract: &mut Value) -> Result<()> {
    use crate::application_testing::{
        CaseResponse, VersionContract, datasets, evaluations, profiles,
    };
    add_schema::<datasets::DatasetResponse>(contract, "DatasetResponse")?;
    add_schema::<datasets::CreateDatasetRequest>(contract, "CreateDatasetRequest")?;
    add_schema::<crate::application_testing::ValidateCase>(contract, "TestCaseValidationRequest")?;
    add_schema::<crate::application_testing::CaseValidation>(contract, "TestCaseValidation")?;
    add_schema::<crate::application_testing::InvocationSample>(contract, "InvocationTestSample")?;
    add_schema::<datasets::UpdateDataset>(contract, "UpdateDatasetRequest")?;
    add_schema::<datasets::Revision>(contract, "DatasetRevisionRequest")?;
    add_schema::<datasets::ImportCases>(contract, "DatasetImportRequest")?;
    add_schema::<datasets::CopyPreview>(contract, "DatasetCopyPreview")?;
    add_schema::<datasets::CopyDataset>(contract, "CopyDatasetRequest")?;
    add_schema::<datasets::ExportCases>(contract, "DatasetExportRequest")?;
    add_schema::<CaseResponse>(contract, "TestCaseResponse")?;
    add_schema::<datasets::SaveCase>(contract, "SaveTestCaseRequest")?;
    add_schema::<VersionContract>(contract, "ApplicationTestContract")?;
    add_schema::<profiles::ProfileDraft>(contract, "EvaluationProfileDraft")?;
    add_schema::<profiles::ProfileResponse>(contract, "EvaluationProfileResponse")?;
    add_schema::<evaluations::CreateEvaluation>(contract, "CreateEvaluationRequest")?;
    add_schema::<BatchReport>(contract, "EvaluationBatchReport")?;
    let base = "/api/v1/applications/{application}";
    for (suffix, method, request, response, array) in [
        (
            "test-contracts",
            "get",
            None,
            Some("ApplicationTestContract"),
            true,
        ),
        ("datasets", "get", None, Some("DatasetResponse"), true),
        (
            "test-case-validation",
            "post",
            Some("TestCaseValidationRequest"),
            Some("TestCaseValidation"),
            false,
        ),
        (
            "datasets",
            "post",
            Some("CreateDatasetRequest"),
            Some("DatasetResponse"),
            false,
        ),
        (
            "datasets/{dataset}",
            "get",
            None,
            Some("DatasetResponse"),
            false,
        ),
        (
            "datasets/{dataset}",
            "patch",
            Some("UpdateDatasetRequest"),
            Some("DatasetResponse"),
            false,
        ),
        ("datasets/{dataset}", "delete", None, None, false),
        (
            "datasets/{dataset}/cases",
            "get",
            None,
            Some("TestCaseResponse"),
            true,
        ),
        (
            "datasets/{dataset}/cases",
            "post",
            Some("SaveTestCaseRequest"),
            None,
            false,
        ),
        (
            "datasets/{dataset}/cases/{case}",
            "patch",
            Some("SaveTestCaseRequest"),
            None,
            false,
        ),
        (
            "datasets/{dataset}/cases/{case}",
            "delete",
            Some("DatasetRevisionRequest"),
            None,
            false,
        ),
        (
            "datasets/{dataset}/copy-preview",
            "post",
            Some("CopyDatasetRequest"),
            Some("DatasetCopyPreview"),
            false,
        ),
        (
            "datasets/{dataset}/copies",
            "post",
            Some("CopyDatasetRequest"),
            Some("DatasetResponse"),
            false,
        ),
        (
            "datasets/{dataset}/import",
            "post",
            Some("DatasetImportRequest"),
            None,
            false,
        ),
        (
            "datasets/{dataset}/export",
            "post",
            Some("DatasetExportRequest"),
            None,
            false,
        ),
        (
            "invocations/{invocation}/test-sample",
            "get",
            None,
            Some("InvocationTestSample"),
            false,
        ),
        (
            "evaluation-profiles",
            "get",
            None,
            Some("EvaluationProfileResponse"),
            true,
        ),
        (
            "evaluation-profiles",
            "post",
            Some("EvaluationProfileDraft"),
            Some("EvaluationProfileResponse"),
            false,
        ),
        (
            "evaluation-profiles/{profile}/versions",
            "get",
            None,
            Some("EvaluationProfileResponse"),
            true,
        ),
        (
            "evaluation-profiles/{profile}",
            "patch",
            Some("EvaluationProfileDraft"),
            Some("EvaluationProfileResponse"),
            false,
        ),
        ("evaluation-profiles/{profile}", "delete", None, None, false),
        (
            "evaluations",
            "get",
            None,
            Some("EvaluationBatchReport"),
            true,
        ),
        (
            "evaluations",
            "post",
            Some("CreateEvaluationRequest"),
            None,
            false,
        ),
        (
            "evaluations/{batch}/report",
            "get",
            None,
            Some("EvaluationBatchReport"),
            false,
        ),
        ("evaluations/{batch}/cancel", "post", None, None, false),
    ] {
        let path = format!("{base}/{suffix}");
        let mut parameters=path.split('/').filter_map(|part|part.strip_prefix('{').and_then(|p|p.strip_suffix('}'))).map(|name|json!({"in":"path","name":name,"required":true,"schema":{"type":"string","format":"uuid"}})).collect::<Vec<_>>();
        let schema = response
            .map(|name| json!({"$ref":format!("#/components/schemas/{name}")}))
            .unwrap_or_else(|| json!({"type":"object"}));
        let schema = if array {
            json!({"type":"array","items":schema})
        } else {
            schema
        };
        let status = if method == "delete" {
            "204"
        } else if method == "post" && (suffix == "evaluations" || suffix.ends_with("/cancel")) {
            "202"
        } else if method == "post"
            && (suffix == "datasets"
                || suffix.ends_with("/copies")
                || suffix == "evaluation-profiles")
        {
            "201"
        } else {
            "200"
        };
        if method == "post" && suffix == "evaluations" {
            parameters.push(json!({"in":"header","name":"Idempotency-Key","required":true,"schema":{"type":"string","minLength":1,"maxLength":128}}));
        }
        if method == "delete" && !suffix.ends_with("/{case}") {
            parameters.push(json!({"in":"query","name":"expectedVersion","required":true,"schema":{"type":"integer","minimum":1}}));
        }
        let mut operation = json!({"security":[{"bearerAuth":[]}],"parameters":parameters,"responses":{status:{"description":"Application test operation","content":{"application/json":{"schema":schema}}}}});
        if status == "204" {
            operation["responses"][status] = json!({"description":"Deleted"});
        }
        if suffix.ends_with("/export") {
            operation["responses"][status] = json!({"description":"JSON Lines dataset cases","content":{"text/plain":{"schema":{"type":"string"}}}});
        }
        if let Some(name) = request {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{name}")}}}})
        }
        if contract["paths"].get(&path).is_none() {
            contract["paths"][&path] = json!({});
        }
        contract["paths"][path][method] = operation;
    }
    Ok(())
}

#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BatchReport {
    pub id: uuid::Uuid,
    pub application_id: uuid::Uuid,
    pub name: String,
    pub status: String,
    pub profile_name: String,
    pub profile_version_id: uuid::Uuid,
    pub criteria: String,
    pub pass_threshold: f64,
    pub model_id: uuid::Uuid,
    pub model_version: String,
    pub model_name: String,
    pub model_parameters: Value,
    pub created_at: String,
    pub duration_ms: u64,
    pub runs: Vec<RunReport>,
}
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunReport {
    pub id: uuid::Uuid,
    pub dataset_id: uuid::Uuid,
    pub dataset_name: String,
    pub contract_fingerprint: String,
    pub dataset_version_id: uuid::Uuid,
    pub source_revision: u64,
    pub workflow_version_id: uuid::Uuid,
    pub workflow_version_number: u64,
    pub status: String,
    pub total_cases: u64,
    pub passed_count: u64,
    pub failed_count: u64,
    pub error_count: u64,
    pub scored_count: u64,
    pub average_score: Option<f64>,
    pub pass_rate: Option<f64>,
    pub business_cost_micros: u64,
    pub judge_cost_micros: u64,
    pub business_cost_by_currency: std::collections::BTreeMap<String, u64>,
    pub judge_cost_by_currency: std::collections::BTreeMap<String, u64>,
    pub dispatch_error: Option<Value>,
    pub cases: Vec<CaseReport>,
}
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaseReport {
    pub id: uuid::Uuid,
    pub name: String,
    pub lineage_id: uuid::Uuid,
    pub input: Value,
    pub expected_output: Option<Value>,
    pub actual_output: Option<Value>,
    pub status: String,
    pub score: Option<f64>,
    pub reason: Option<String>,
    pub error: Option<Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub execution_id: Option<uuid::Uuid>,
    pub judge_execution_id: Option<uuid::Uuid>,
    pub duration_ms: Option<u64>,
    pub judge_duration_ms: Option<u64>,
    pub business_cost_micros: u64,
    pub judge_cost_micros: u64,
    pub business_cost_by_currency: std::collections::BTreeMap<String, u64>,
    pub judge_cost_by_currency: std::collections::BTreeMap<String, u64>,
}
