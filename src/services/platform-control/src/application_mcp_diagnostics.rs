//! Read-only entrance diagnostics. No diagnostic API key or business invocation is created.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};
use agentx_runtime_contracts::ApplicationMcpPublicationV1;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticCheck {
    name: String,
    passed: bool,
}
#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpDiagnostic {
    publication_id: Uuid,
    revision: u64,
    passed: bool,
    checks: Vec<DiagnosticCheck>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticQuery {
    publication_id: Option<Uuid>,
}

pub(crate) async fn inspect(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
    Query(query): Query<DiagnosticQuery>,
) -> ApiResult<Json<McpDiagnostic>> {
    crate::application_ui_api::authorize(&state, &actor, application).await?;
    let row=sqlx::query("SELECT p.manifest_json,o.status FROM application_mcp_publications p JOIN outbox o ON o.id=p.outbox_id WHERE p.tenant_id=? AND p.application_id=? AND p.deleted_at IS NULL AND (? IS NULL OR p.id=?) ORDER BY p.revision DESC LIMIT 1")
        .bind(actor.tenant_id).bind(application).bind(query.publication_id).bind(query.publication_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("MCP publication"))?;
    let manifest: ApplicationMcpPublicationV1 =
        serde_json::from_value(row.try_get("manifest_json")?).map_err(ApiError::internal)?;
    let origin = crate::oauth_api::origin()?;
    let resource = crate::oauth_api::resource(&manifest.slug)?;
    let resource_metadata = origin
        .join(&format!(
            "gateway/v1/.well-known/oauth-protected-resource/mcp/applications/{}",
            manifest.slug
        ))
        .map_err(ApiError::internal)?
        .to_string();
    let base = state.runtime_query_url.trim_end_matches('/');
    let denied = state
        .http
        .post(format!(
            "{base}/gateway/v1/mcp/applications/{}",
            manifest.slug
        ))
        .header("Accept", "application/json, text/event-stream")
        .timeout(Duration::from_secs(8))
        .send();
    let metadata = get_json(
        &state,
        format!(
            "{base}/gateway/v1/.well-known/oauth-protected-resource/mcp/applications/{}",
            manifest.slug
        ),
    );
    let oauth = get_json(
        &state,
        format!("{base}/.well-known/oauth-authorization-server"),
    );
    let (denied, metadata, oauth) = tokio::join!(denied, metadata, oauth);
    let authorization = denied.is_ok_and(|response| {
        response.status() == reqwest::StatusCode::UNAUTHORIZED
            && response
                .headers()
                .get("www-authenticate")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value == format!("Bearer resource_metadata=\"{resource_metadata}\"")
                })
    });
    let resource_ok = metadata.is_some_and(|value| {
        value["resource"] == resource
            && value["scopes_supported"].as_array().is_some_and(|scopes| {
                scopes.contains(&Value::String(format!("agentx:application:{application}")))
            })
    });
    let oauth_ok = oauth.is_some_and(|value| {
        value
            .get("issuer")
            .and_then(Value::as_str)
            .and_then(|issuer| reqwest::Url::parse(issuer).ok())
            .as_ref()
            == Some(&origin)
            && [
                ("authorization_endpoint", "oauth2/auth"),
                ("token_endpoint", "oauth2/token"),
                ("revocation_endpoint", "oauth2/revoke"),
            ]
            .into_iter()
            .all(|(key, path)| {
                origin
                    .join(path)
                    .is_ok_and(|url| value[key] == url.to_string())
            })
    });
    let checks = vec![
        DiagnosticCheck {
            name: "publication".into(),
            passed: row.try_get::<String, _>("status")? == "published"
                && manifest.enabled
                && manifest.validate().is_ok(),
        },
        DiagnosticCheck {
            name: "authorization".into(),
            passed: authorization,
        },
        DiagnosticCheck {
            name: "resource".into(),
            passed: resource_ok,
        },
        DiagnosticCheck {
            name: "oauth".into(),
            passed: oauth_ok,
        },
    ];
    Ok(Json(McpDiagnostic {
        publication_id: manifest.publication_id,
        revision: manifest.revision,
        passed: checks.iter().all(|check| check.passed),
        checks,
    }))
}
async fn get_json(state: &ControlApiState, url: String) -> Option<Value> {
    let response = state
        .http
        .get(url)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}
