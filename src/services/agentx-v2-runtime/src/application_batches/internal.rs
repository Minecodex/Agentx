//! Management entry points use signed delegation, never employee impersonation.
use super::*;
use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};

pub fn routes() -> Router<RuntimeState> {
    Router::new().route("/internal/runtime/v1/application-batches", post(apply))
}
async fn apply(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<ManageBatchRequestV1>,
) -> RuntimeResult<axum::response::Response> {
    use axum::response::IntoResponse;
    let expected = content_hash(&request).map_err(|e| RuntimeError::Internal(e.into()))?;
    let claims = crate::query_authority::authorize_delegation(
        &state,
        &headers,
        request.tenant_id,
        "runtime.application_batches",
        &expected,
    )
    .await?;
    if !claims.tenant_wide && !claims.application_ids.contains(&request.application_id) {
        return Err(RuntimeError::Unauthorized);
    }
    crate::query_authority::verify_application_grant(&state, &claims, request.application_id)
        .await?;
    let scope = BatchScope {
        caller: BatchCaller {
            tenant_id: request.tenant_id,
            application_id: request.application_id,
            caller_type: "user".into(),
            caller_id: claims.sub,
            token_version: Some(claims.token_version),
            origin: ExecutionOriginV1::system(None),
        },
        management: true,
    };
    let id = || {
        request
            .batch_id
            .ok_or_else(|| invalid("batchId is required"))
    };
    let key = || {
        request
            .request_key
            .as_deref()
            .filter(|s| !s.is_empty() && s.len() <= 192)
            .ok_or_else(|| invalid("requestKey is required"))
    };
    let query = request.query.clone().unwrap_or_default();
    let value = match request.operation.as_str() {
        "list" => json!(list(&state, &scope, &query).await?),
        "get" => json!(get(&state, &scope, id()?).await?),
        "items" => json!(items(&state, &scope, id()?, &query).await?),
        "stop" => {
            key()?;
            json!(stop(&state, &scope, id()?).await?)
        }
        "cancel" => json!(cancel(&state, &scope, id()?, key()?).await?),
        "rerun" => json!(
            rerun(
                &state,
                &scope,
                id()?,
                BatchRerunV1 {
                    item_ids: request
                        .item_ids
                        .clone()
                        .ok_or_else(|| invalid("itemIds are required"))?
                },
                key()?
            )
            .await?
        ),
        "export" => json!(request_export(&state, &scope, id()?, key()?).await?),
        "export_status" => json!(
            get_export(
                &state,
                &scope,
                id()?,
                request
                    .export_id
                    .ok_or_else(|| invalid("exportId is required"))?
            )
            .await?
        ),
        "export_file" => {
            let bytes = download_export(
                &state,
                &scope,
                id()?,
                request
                    .export_id
                    .ok_or_else(|| invalid("exportId is required"))?,
            )
            .await?;
            crate::query_authority::complete_query_receipt(&state, claims.jti, "OK").await?;
            return Ok((
                [
                    (axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8"),
                    (
                        axum::http::header::CONTENT_DISPOSITION,
                        "attachment; filename=\"batch.csv\"",
                    ),
                    (axum::http::header::CACHE_CONTROL, "no-store"),
                ],
                bytes,
            )
                .into_response());
        }
        _ => return Err(invalid("Unsupported batch operation")),
    };
    crate::query_authority::complete_query_receipt(&state, claims.jti, "OK").await?;
    Ok(Json(value).into_response())
}
