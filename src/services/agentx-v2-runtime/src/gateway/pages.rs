//! Employee page entrance. All business operations use the existing formal services.
use super::*;
use agentx_runtime_contracts::{
    ApplicationAccessBindingV1, ApplicationPagePublicationV1, ApplicationPageViewV1,
};
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PageQuery {
    task_id: Option<Uuid>,
    batch_id: Option<Uuid>,
}

pub(super) async fn get(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> RuntimeResult<(HeaderMap, Json<ApplicationPageViewV1>)> {
    let claims = state.trust.user(bearer_token(&headers)?)?;
    let row=sqlx::query("SELECT h.application_id,h.publication_id,h.enabled,r.route_key FROM application_web_heads h JOIN application_routes r ON r.tenant_id=h.tenant_id AND r.application_id=h.application_id WHERE h.tenant_id=? AND h.slug=?")
        .bind(claims.tenant_id).bind(&slug).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    let application = row.try_get("application_id")?;
    let caller = authorize_user_application(&state, claims, application).await?;
    if query.task_id.is_some() && query.batch_id.is_some() {
        return Err(RuntimeError::InvalidRequest(
            "PAGE_TARGET_CONFLICT",
            "Specify either taskId or batchId".into(),
        ));
    }
    let publication = if let Some(batch_id) = query.batch_id {
        let scope = crate::application_batches::BatchScope::owner(
            crate::application_batches::BatchCaller {
                tenant_id: caller.tenant_id,
                application_id: caller.application_id,
                caller_type: caller.caller_type.into(),
                caller_id: caller.subject_id,
                token_version: caller.token_version,
                origin: caller.origin.clone(),
            },
        );
        let batch = crate::application_batches::get(&state, &scope, batch_id).await?;
        batch.page_publication_id.ok_or_else(||crate::application_pages::conflict("No matching business page was available when this batch was accepted; use its API records"))?
    } else if let Some(task_id) = query.task_id {
        let task = tasks::load(&state, &caller, task_id).await?;
        if let Some(ApplicationAccessBindingV1::Web { publication_id, .. }) = task.access_binding {
            publication_id
        } else {
            sqlx::query_scalar("SELECT id FROM application_page_publications WHERE tenant_id=? AND application_id=? AND bundle_id=? AND status IN ('active','superseded') ORDER BY revision DESC LIMIT 1")
                .bind(caller.tenant_id).bind(application).bind(task.bundle_id).fetch_optional(&state.pool).await?.ok_or_else(||crate::application_pages::conflict("This task has no published page for its exact business Bundle"))?
        }
    } else {
        if !row.try_get::<bool, _>("enabled")? {
            return Err(crate::application_pages::conflict(
                "This application page is disabled",
            ));
        }
        row.try_get("publication_id")?
    };
    let value:Value=sqlx::query_scalar("SELECT manifest_json FROM application_page_publications WHERE tenant_id=? AND application_id=? AND id=? AND status IN ('active','superseded')")
        .bind(caller.tenant_id).bind(application).bind(publication).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    let page: ApplicationPagePublicationV1 =
        serde_json::from_value(value).map_err(|e| RuntimeError::Internal(e.into()))?;
    let historical = (query.task_id.is_some() || query.batch_id.is_some())
        && (!row.try_get::<bool, _>("enabled")?
            || page.publication_id != row.try_get::<Uuid, _>("publication_id")?);
    let html = crate::application_pages::read_asset(&state, &page.html).await?;
    if agentx_runtime_contracts::content_hash(&html)
        .map_err(|e| RuntimeError::Internal(e.into()))?
        != page.html_hash
    {
        return Err(RuntimeError::Unavailable);
    }
    let sdk_script = crate::application_pages::read_asset(&state, &page.sdk).await?;
    let stylesheet = crate::application_pages::read_asset(&state, &page.stylesheet).await?;
    let response = ApplicationPageViewV1 {
        publication: page,
        html,
        sdk_script,
        stylesheet,
        route_key: row.try_get("route_key")?,
        historical,
        task_id: query.task_id,
        batch_id: query.batch_id,
    };
    Ok((
        HeaderMap::from_iter([(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        )]),
        Json(response),
    ))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubmitPage {
    pub publication_id: Uuid,
    pub revision: u64,
    pub input: Value,
}

pub(super) async fn invoke(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    mut headers: HeaderMap,
    Json(request): Json<SubmitPage>,
) -> RuntimeResult<(StatusCode, Json<agentx_runtime_contracts::TaskViewV1>)> {
    let claims = state.trust.user(bearer_token(&headers)?)?;
    let application = sqlx::query_scalar(
        "SELECT application_id FROM application_web_heads WHERE tenant_id=? AND slug=?",
    )
    .bind(claims.tenant_id)
    .bind(slug)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(RuntimeError::NotFound)?;
    let caller = authorize_user_application(&state, claims, application).await?;
    headers.insert(
        "x-agentx-page-publication",
        request
            .publication_id
            .to_string()
            .parse()
            .expect("UUID header"),
    );
    headers.insert(
        "x-agentx-page-revision",
        request
            .revision
            .to_string()
            .parse()
            .expect("numeric header"),
    );
    let accepted = create_runtime_invocation(
        &state.pool,
        caller.tenant_id,
        caller.application_id,
        InvocationCaller {
            access: binding(&headers)?,
            wait_inputs: None,
            trigger_context: None,
            end_user: None,
            caller_type: caller.caller_type,
            caller_id: caller.subject_id,
            token_version: caller.token_version,
            origin: caller.origin.clone(),
        },
        None,
        &request.input,
        idempotency_key(&headers)?,
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(tasks::load(&state, &caller, accepted.invocation_id).await?),
    ))
}

pub(super) fn binding(headers: &HeaderMap) -> RuntimeResult<Option<ApplicationAccessBindingV1>> {
    let web = headers.contains_key("x-agentx-page-publication")
        || headers.contains_key("x-agentx-page-revision");
    let mcp = headers.contains_key("x-agentx-mcp-publication")
        || headers.contains_key("x-agentx-mcp-revision");
    if web && mcp {
        return Err(bad_request("ACCESS_BINDING_CONFLICT"));
    }
    let (id, revision) = if mcp {
        ("x-agentx-mcp-publication", "x-agentx-mcp-revision")
    } else {
        ("x-agentx-page-publication", "x-agentx-page-revision")
    };
    match (headers.get(id), headers.get(revision)) {
        (None, None) => Ok(None),
        (Some(id), Some(revision)) => {
            let publication_id = id
                .to_str()
                .ok()
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or_else(|| bad_request("ACCESS_BINDING_INVALID"))?;
            let revision = revision
                .to_str()
                .ok()
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| bad_request("ACCESS_BINDING_INVALID"))?;
            Ok(Some(if mcp {
                ApplicationAccessBindingV1::Mcp {
                    publication_id,
                    revision,
                }
            } else {
                ApplicationAccessBindingV1::Web {
                    publication_id,
                    revision,
                }
            }))
        }
        _ => Err(bad_request("ACCESS_BINDING_INVALID")),
    }
}
