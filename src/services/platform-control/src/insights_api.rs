//! Insights aggregate BFF (plan7 P7-C C3): proxies the observability
//! aggregates:query surface. The request is built with the shared contract
//! type so the delegation request hash matches the observability side byte
//! for byte; budget and degraded states map to explicit API codes.

use axum::{Json, extract::State, http::HeaderMap};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use agentx_runtime_contracts::{
    ObservabilityAggregateRequestV1, ObservabilityDimensionV1, ObservabilityMetricV1, content_hash,
};

use crate::api_error::{ApiError, ApiResult};
use crate::control_api::{Actor, ControlApiState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InsightsAggregateInput {
    #[serde(with = "time::serde::rfc3339")]
    pub from: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub to: time::OffsetDateTime,
    pub metrics: Vec<ObservabilityMetricV1>,
    pub dimensions: Vec<ObservabilityDimensionV1>,
    #[serde(default)]
    pub filters: Value,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    100
}

pub async fn aggregate(
    State(state): State<ControlApiState>,
    actor: Actor,
    headers: HeaderMap,
    Json(input): Json<InsightsAggregateInput>,
) -> ApiResult<Json<Value>> {
    actor.require("runtime:view")?;
    let request = ObservabilityAggregateRequestV1 {
        api_version: 1,
        tenant_id: actor.tenant_id,
        from: input.from,
        to: input.to,
        metrics: input.metrics,
        dimensions: input.dimensions,
        filters: input.filters,
        limit: input.limit.clamp(1, 1000),
    };
    let request_hash = content_hash(&json!({"operation":"aggregate-query","request":request}))
        .map_err(ApiError::internal)?;
    let hash_text = request_hash.as_str().to_owned();
    let token = super::runtime_bff::mint_observability_token(
        &state,
        &actor,
        "observability.aggregate.read",
        request_hash,
    )
    .await?;
    let response = state
        .http
        .post(format!(
            "{}/internal/observability/v1/aggregates:query",
            state.observability_query_url
        ))
        .header("x-agentx-request-hash", hash_text)
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(|_| {
            ApiError::unavailable(
                "INSIGHTS_DEGRADED",
                "Observability aggregates are unavailable",
            )
        })?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|_| ApiError::unavailable("INSIGHTS_DEGRADED", "Unreadable aggregate response"))?;
    if status.as_u16() == 422 {
        return Err(ApiError::unprocessable(
            "OBSERVABILITY_QUERY_BUDGET_EXCEEDED",
            body.get("code")
                .and_then(Value::as_str)
                .unwrap_or("Aggregate query budget exceeded"),
        ));
    }
    if !status.is_success() {
        return Err(ApiError::unavailable(
            "INSIGHTS_DEGRADED",
            format!("Observability returned HTTP {status}"),
        ));
    }
    let _ = headers;
    Ok(Json(body))
}

#[allow(dead_code)]
pub fn routes() -> axum::Router<ControlApiState> {
    axum::Router::new().route(
        "/api/v1/insights/aggregates",
        axum::routing::post(aggregate),
    )
}
