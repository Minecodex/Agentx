use super::*;
use agentx_runtime_contracts::{InvocationSearchPageV1, InvocationSearchRequestV1};

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
