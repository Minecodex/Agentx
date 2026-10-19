use super::*;
use agentx_runtime_contracts::{InvocationSearchPageV1, InvocationSearchRequestV1};

pub(crate) async fn test_sample(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    invocation: Uuid,
) -> ApiResult<Value> {
    actor.require("execution:view")?;
    let workflow =
        crate::application_testing::authorize(state, actor, application, "dataset:manage").await?;
    let hash = content_hash(&json!({"operation":"get_invocation","invocationId":invocation}))
        .map_err(ApiError::internal)?;
    let token = delegation_token(
        state,
        actor,
        "runtime.query.invocation",
        BTreeSet::from([application]),
        BTreeSet::from([workflow]),
        BTreeSet::new(),
        false,
        hash,
    )?;
    let response = state
        .http
        .get(format!(
            "{}/internal/runtime/v1/query/invocations/{invocation}",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    let detail: agentx_runtime_contracts::InvocationDetailV1 = runtime_json(response).await?;
    if detail.summary.application_id != application {
        return Err(ApiError::forbidden(
            "Invocation belongs to another application",
        ));
    }
    let execution = detail.summary.execution_id.ok_or_else(|| {
        ApiError::unprocessable(
            "INVOCATION_NOT_EXECUTED",
            "The invocation has no execution sample",
        )
    })?;
    let runtime_details: ExecutionRuntimeDetailsV1 = runtime_execution_get(
        state,
        actor,
        execution,
        "execution_runtime_details",
        &format!("/internal/runtime/v1/query/executions/{execution}/runtime-details"),
    )
    .await?;
    let mut wait_inputs = agentx_runtime_contracts::WaitInputsV1::new();
    for wait in runtime_details
        .input_waits
        .into_iter()
        .filter(|wait| wait.status == "received")
    {
        if let Some(content) = wait.content {
            wait_inputs.entry(wait.node_path).or_default().push(content);
        }
    }
    let execution = runtime_execution_detail(state, actor, execution).await?;
    let version = execution.summary.workflow_version_id.ok_or_else(|| {
        ApiError::unprocessable(
            "INVALID_TEST_VERSION",
            "The invocation has no published version",
        )
    })?;
    let contract =
        crate::application_testing::selected_contract(state, actor, application, version).await?;
    let draft = crate::application_testing::CaseDraft {
        wait_inputs,
        name: format!("Invocation {invocation}"),
        input: execution.input.unwrap_or(Value::Null),
        expected_output: execution.output.unwrap_or(Value::Null),
        tags: Vec::new(),
        pending_fields: Vec::new(),
        source_invocation_id: Some(invocation),
        reference_confirmed: false,
    };
    let validation_errors = crate::application_testing::validate_draft(
        state,
        actor,
        application,
        &contract,
        draft.clone(),
    )
    .await?;
    serde_json::to_value(crate::application_testing::InvocationSample {
        wait_inputs: draft.wait_inputs,
        invocation_id: invocation,
        workflow_version_id: version,
        contract,
        input: draft.input,
        expected_output: draft.expected_output,
        name: draft.name,
        reference_confirmed: false,
        validation_errors,
    })
    .map_err(ApiError::internal)
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InvocationFilter {
    external_user_id: Option<String>,
    user_source: Option<String>,
    cursor: Option<String>,
}

pub(super) async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
    Query(filter): Query<InvocationFilter>,
) -> ApiResult<Json<InvocationSearchPageV1>> {
    actor.require("execution:view")?;
    let (tenant_wide, applications, workflows) = execution_query_scope(&state, &actor).await?;
    if !tenant_wide && !applications.contains(&application) {
        return Err(ApiError::forbidden(
            "The application is outside your execution scope",
        ));
    }
    let request = InvocationSearchRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        application_ids: vec![application],
        external_user_id: filter.external_user_id,
        user_source: filter.user_source,
        statuses: Vec::new(),
        created_after: None,
        created_before: None,
        cursor: filter.cursor,
        limit: 20,
    };
    let hash = content_hash(&json!({"operation":"invocation-search","request":request}))
        .map_err(ApiError::internal)?;
    let token = delegation_token(
        &state,
        &actor,
        "runtime.query.invocations",
        applications,
        workflows,
        BTreeSet::new(),
        tenant_wide,
        hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/query/invocations:search",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    runtime_json(response).await.map(Json)
}

pub(crate) async fn test_artifacts(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    contract: &crate::test_contract::TestContract,
    cases: &mut [crate::application_testing::CaseResponse],
) -> ApiResult<()> {
    let encoded = serde_json::to_string(contract).map_err(ApiError::internal)?;
    if !encoded.contains("x-agentx-artifact") {
        return Ok(());
    }
    let mut schemas = Vec::new();
    let mut values = Vec::new();
    for case in cases.iter() {
        schemas.extend([
            contract.input_schema.clone(),
            contract.output_schema.clone(),
        ]);
        values.extend([case.draft.input.clone(), case.draft.expected_output.clone()]);
    }
    let request = agentx_runtime_contracts::TestArtifactCheckV1 {
        tenant_id: actor.tenant_id,
        application_id: application,
        schemas,
        values,
    };
    let hash = content_hash(&json!({"operation":"test-artifacts","request":request}))
        .map_err(ApiError::internal)?;
    let token = delegation_token(
        state,
        actor,
        "runtime.query.test_artifacts",
        BTreeSet::from([application]),
        BTreeSet::new(),
        BTreeSet::new(),
        false,
        hash,
    )?;
    let response = state
        .http
        .post(format!(
            "{}/internal/runtime/v1/query/test-artifacts",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_unavailable)?;
    let issues: Vec<agentx_runtime_contracts::TestArtifactIssueV1> = runtime_json(response).await?;
    for issue in issues {
        if let Some(case) = cases.get_mut(issue.index / 2) {
            case.validation_errors
                .push(crate::test_contract::CaseFieldError {
                    code: crate::test_contract::CaseErrorCode::ArtifactUnavailable,
                    field: format!(
                        "{}{}",
                        if issue.index % 2 == 0 {
                            "input"
                        } else {
                            "expectedOutput"
                        },
                        issue.field
                    ),
                    message: issue.message,
                });
        }
    }
    Ok(())
}
