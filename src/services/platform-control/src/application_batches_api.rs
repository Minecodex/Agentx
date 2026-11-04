//! Application call records are a scoped BFF over Runtime batch authority.
use super::*;
use agentx_runtime_contracts::{BatchQueryV1, BatchRerunV1, ManageBatchRequestV1};
use axum::http::HeaderMap;

pub(super) fn apply_openapi(contract: &mut Value) -> anyhow::Result<()> {
    use crate::openapi_contract::add_schema;
    use agentx_runtime_contracts::{BatchExportV1, BatchItemsPageV1, BatchViewV1, BatchesPageV1};
    add_schema::<BatchViewV1>(contract, "BatchViewV1")?;
    add_schema::<BatchesPageV1>(contract, "BatchesPageV1")?;
    add_schema::<BatchItemsPageV1>(contract, "BatchItemsPageV1")?;
    add_schema::<BatchExportV1>(contract, "BatchExportV1")?;
    add_schema::<BatchRerunV1>(contract, "BatchRerunV1")?;
    let base = "/api/v1/applications/{application}/batches";
    for (suffix, method, result, request) in [
        ("", "get", "BatchesPageV1", None),
        ("/{id}", "get", "BatchViewV1", None),
        ("/{id}/items", "get", "BatchItemsPageV1", None),
        ("/{id}/stop", "post", "BatchViewV1", None),
        ("/{id}/cancel", "post", "BatchViewV1", None),
        ("/{id}/rerun", "post", "BatchViewV1", Some("BatchRerunV1")),
        ("/{id}/exports", "post", "BatchExportV1", None),
        ("/{id}/exports/{export}", "get", "BatchExportV1", None),
    ] {
        let path = format!("{base}{suffix}");
        let mut operation = json!({"tags":["Application batches"],"responses":{"200":{"description":"Authorized Runtime view","content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{result}")}}}}},"security":[{"bearerAuth":[]}]});
        let mut params = vec![
            json!({"name":"application","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}),
        ];
        for name in ["id", "export"] {
            if suffix.contains(&format!("{{{name}}}")) {
                params.push(json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
        }
        if method == "post" {
            params.push(json!({"name":"Idempotency-Key","in":"header","required":true,"schema":{"type":"string","maxLength":192}}));
        }
        if suffix.is_empty() || suffix == "/{id}/items" {
            for (name, schema) in [
                ("limit", json!({"type":"integer","minimum":1,"maximum":50})),
                ("cursor", json!({"type":"string"})),
                ("state", json!({"type":"string"})),
                ("source", json!({"type":"string","enum":["web","api"]})),
                ("requestKey", json!({"type":"string","maxLength":192})),
            ] {
                params.push(json!({"name":name,"in":"query","schema":schema}));
            }
        }
        operation["parameters"] = json!(params);
        if let Some(request) = request {
            operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{request}")}}}});
        }
        contract["paths"][path][method] = operation;
    }
    let file_params = ["application","id","export"].map(|name|json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
    contract["paths"][format!("{base}/{{id}}/exports/{{export}}/file")]["get"] = json!({"tags":["Application batches"],"parameters":file_params,"responses":{"200":{"description":"Authorized CSV export","content":{"text/csv":{"schema":{"type":"string","format":"binary"}}}}},"security":[{"bearerAuth":[]}]});
    Ok(())
}

pub(super) fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/applications/{application}/batches", get(list))
        .route(
            "/api/v1/applications/{application}/batches/{id}",
            get(detail),
        )
        .route(
            "/api/v1/applications/{application}/batches/{id}/items",
            get(items),
        )
        .route(
            "/api/v1/applications/{application}/batches/{id}/stop",
            post(stop),
        )
        .route(
            "/api/v1/applications/{application}/batches/{id}/cancel",
            post(cancel),
        )
        .route(
            "/api/v1/applications/{application}/batches/{id}/rerun",
            post(rerun),
        )
        .route(
            "/api/v1/applications/{application}/batches/{id}/exports",
            post(export),
        )
        .route(
            "/api/v1/applications/{application}/batches/{id}/exports/{export}",
            get(export_status),
        )
        .route(
            "/api/v1/applications/{application}/batches/{id}/exports/{export}/file",
            get(export_file),
        )
}

fn request(actor: &Actor, app: Uuid, id: Option<Uuid>, op: &str) -> ManageBatchRequestV1 {
    ManageBatchRequestV1 {
        tenant_id: actor.tenant_id,
        application_id: app,
        batch_id: id,
        operation: op.into(),
        query: None,
        item_ids: None,
        export_id: None,
        request_key: None,
    }
}
fn key(headers: &HeaderMap) -> ApiResult<String> {
    headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .filter(|s| !s.is_empty() && s.len() <= 192)
        .map(str::to_owned)
        .ok_or_else(|| {
            ApiError::unprocessable(
                "IDEMPOTENCY_KEY_REQUIRED",
                "A stable Idempotency-Key is required",
            )
        })
}

async fn forward(
    state: &ControlApiState,
    actor: &Actor,
    request: ManageBatchRequestV1,
) -> ApiResult<reqwest::Response> {
    actor.require("execution:view")?;
    match request.operation.as_str() {
        "stop" | "cancel" => actor.require("execution:cancel")?,
        "rerun" => {
            actor.require("execution:run")?;
            crate::application_testing::authorize(
                state,
                actor,
                request.application_id,
                "application:invoke",
            )
            .await?;
        }
        _ => {}
    }
    let (tenant_wide, applications, workflows) = execution_query_scope(state, actor).await?;
    if !tenant_wide && !applications.contains(&request.application_id) {
        return Err(ApiError::forbidden(
            "The application is outside your execution scope",
        ));
    }
    let token = delegation_token(
        state,
        actor,
        "runtime.application_batches",
        BTreeSet::from([request.application_id]),
        workflows,
        BTreeSet::new(),
        tenant_wide,
        content_hash(&request).map_err(ApiError::internal)?,
    )?;
    state
        .http
        .post(format!(
            "{}/internal/runtime/v1/application-batches",
            state.runtime_query_url
        ))
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .map_err(runtime_unavailable)
}
async fn json(
    state: &ControlApiState,
    actor: &Actor,
    request: ManageBatchRequestV1,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        runtime_json(forward(state, actor, request).await?).await?,
    ))
}
async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(app): Path<Uuid>,
    Query(q): Query<BatchQueryV1>,
) -> ApiResult<Json<Value>> {
    let mut r = request(&actor, app, None, "list");
    r.query = Some(q);
    json(&state, &actor, r).await
}
async fn detail(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((app, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    json(&state, &actor, request(&actor, app, Some(id), "get")).await
}
async fn items(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((app, id)): Path<(Uuid, Uuid)>,
    Query(q): Query<BatchQueryV1>,
) -> ApiResult<Json<Value>> {
    let mut r = request(&actor, app, Some(id), "items");
    r.query = Some(q);
    json(&state, &actor, r).await
}
macro_rules! command {
    ($name:ident,$op:literal) => {
        async fn $name(
            State(state): State<ControlApiState>,
            actor: Actor,
            Path((app, id)): Path<(Uuid, Uuid)>,
            headers: HeaderMap,
        ) -> ApiResult<Json<Value>> {
            let mut r = request(&actor, app, Some(id), $op);
            r.request_key = Some(key(&headers)?);
            json(&state, &actor, r).await
        }
    };
}
command!(stop, "stop");
command!(cancel, "cancel");
command!(export, "export");
async fn rerun(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((app, id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<BatchRerunV1>,
) -> ApiResult<Json<Value>> {
    let mut r = request(&actor, app, Some(id), "rerun");
    r.item_ids = Some(input.item_ids);
    r.request_key = Some(key(&headers)?);
    json(&state, &actor, r).await
}
async fn export_status(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((app, id, export)): Path<(Uuid, Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    let mut r = request(&actor, app, Some(id), "export_status");
    r.export_id = Some(export);
    json(&state, &actor, r).await
}
async fn export_file(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((app, id, export)): Path<(Uuid, Uuid, Uuid)>,
) -> ApiResult<Response> {
    let mut r = request(&actor, app, Some(id), "export_file");
    r.export_id = Some(export);
    let response = forward(&state, &actor, r).await?;
    if !response.status().is_success() {
        let _: Value = runtime_json(response).await?;
        return Err(ApiError::internal(anyhow::anyhow!(
            "Invalid batch export response"
        )));
    }
    let bytes = response.bytes().await.map_err(runtime_unavailable)?;
    Response::builder()
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"batch.csv\"",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .map_err(ApiError::internal)
}
