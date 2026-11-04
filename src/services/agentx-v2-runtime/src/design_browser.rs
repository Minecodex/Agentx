//! Authenticated design observations and their model-only image projection.
use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};
use agentx_agent_core::{ModelContentPartV1, ModelContentV1, ModelRequestV1};
use agentx_runtime_contracts::{
    DesignBrowserReportV1, DesignBrowserRequestV1, DesignModelTargetV1, RuntimeResourceBindingV1,
    RuntimeResourceConfigurationV1, design_image_owner,
};
use axum::{Json, extract::State, http::HeaderMap};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use object_store::path::Path;
use sha2::{Digest, Sha256};
use sqlx::Row;

pub async fn observe(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<DesignBrowserRequestV1>,
) -> RuntimeResult<Json<DesignBrowserReportV1>> {
    let value = serde_json::to_value(&request)?;
    crate::builder_model::authorize(
        &state,
        &headers,
        "runtime.design-browser.observe",
        &value,
        &request.target,
    )?;
    request.validate().map_err(|message| {
        RuntimeError::InvalidRequest("DESIGN_BROWSER_ARGUMENT_INVALID", message.into())
    })?;
    if serde_json::to_vec(&request)?.len() > 2 * 1024 * 1024 {
        return Err(RuntimeError::InvalidRequest(
            "DESIGN_BROWSER_TOO_LARGE",
            "Preview exceeds 2 MiB".into(),
        ));
    }
    let endpoint =
        std::env::var("AGENTX_SANDBOX_MANAGER_ENDPOINT").map_err(|_| RuntimeError::Unavailable)?;
    let client = crate::egress::ProviderHttpClient::from_env(
        agentx_runtime_contracts::EgressRole::RuntimeGateway,
    )
    .map_err(RuntimeError::Internal)?;
    let response = client
        .post_sandbox_manager(
            &format!("{endpoint}/internal/runtime/v1/sandboxes:preview"),
            std::time::Duration::from_secs(55),
        )
        .map_err(RuntimeError::Internal)?
        .json(&request)
        .send()
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    if !response.status().is_success() {
        let status = response.status();
        let diagnostic = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(|text| text.chars().take(2400).collect::<String>())
            });
        tracing::warn!(effect_id=%request.effect_id,%status,diagnostic,"Isolated authoring preview was rejected");
        if status == reqwest::StatusCode::BAD_REQUEST {
            return Err(RuntimeError::InvalidRequest(
                "DESIGN_BROWSER_FAILED",
                diagnostic.unwrap_or_else(|| "The preview could not be rendered".into()),
            ));
        }
        return Err(RuntimeError::Unavailable);
    }
    response
        .json()
        .await
        .map(Json)
        .map_err(|_| RuntimeError::Unavailable)
}

pub(crate) async fn resolve_images(
    state: &RuntimeState,
    target: &DesignModelTargetV1,
    binding: &RuntimeResourceBindingV1,
    request: &mut ModelRequestV1,
) -> RuntimeResult<()> {
    let vision = matches!(&binding.configuration,RuntimeResourceConfigurationV1::Model{capabilities,..} if capabilities.iter().any(|c|c=="vision"));
    let image_count = request
        .messages
        .iter()
        .map(|message| match &message.content {
            ModelContentV1::Parts(parts) => parts
                .iter()
                .filter(|part| matches!(part, ModelContentPartV1::ImageUrl { .. }))
                .count(),
            _ => 0,
        })
        .sum::<usize>();
    if image_count > 4 {
        return Err(RuntimeError::InvalidRequest(
            "DESIGN_IMAGE_LIMIT",
            "At most four recent image observations may enter a model request".into(),
        ));
    }
    for message in &mut request.messages {
        let observation = message.role == agentx_agent_core::MessageRole::ToolObservation;
        let ModelContentV1::Parts(parts) = &mut message.content else {
            continue;
        };
        for part in parts {
            let ModelContentPartV1::ImageUrl { image_url } = part else {
                continue;
            };
            let id = image_url
                .url
                .strip_prefix("agentx-artifact:")
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
                .ok_or_else(|| {
                    RuntimeError::InvalidRequest(
                        "DESIGN_IMAGE_SOURCE_INVALID",
                        "Design images must be authorized preview artifacts".into(),
                    )
                })?;
            if !vision {
                if observation {
                    *part=ModelContentPartV1::Text{text:"An earlier screenshot is not visible to the selected text-only model. Use DOM observations; do not claim a visual check.".into()};
                    continue;
                }
                return Err(RuntimeError::InvalidRequest(
                    "DESIGN_MODEL_VISION_REQUIRED",
                    "The selected model does not declare image input support".into(),
                ));
            }
            let row=sqlx::query("SELECT a.storage_key,a.content_type,a.size_bytes,a.sha256,(r.retention_until IS NULL OR r.retention_until>UTC_TIMESTAMP(6)) AS retained FROM artifacts a JOIN artifact_references r ON r.tenant_id=a.tenant_id AND r.artifact_id=a.id WHERE a.tenant_id=? AND a.id=? AND a.deleted_at IS NULL AND r.owner_type='design_preview' AND r.owner_id=?")
                .bind(target.identity().tenant_id).bind(id).bind(design_image_owner(target)).fetch_optional(&state.pool).await?;
            let Some(row) = row.filter(|row| row.try_get::<bool, _>("retained").unwrap_or(false))
            else {
                if observation {
                    *part=ModelContentPartV1::Text{text:"An earlier screenshot is unavailable or outside its retention period. Request a fresh observation of the current draft before making visual claims.".into()};
                    continue;
                }
                return Err(RuntimeError::Unauthorized);
            };
            if row.try_get::<String, _>("content_type")? != "image/webp"
                || row.try_get::<u64, _>("size_bytes")? > 576 * 1024
            {
                return Err(RuntimeError::Unauthorized);
            }
            let key: String = row.try_get("storage_key")?;
            let object = match state.objects.get(&Path::from(key)).await {
                Ok(object) => object,
                Err(object_store::Error::NotFound { .. }) if observation => {
                    *part=ModelContentPartV1::Text{text:"An earlier screenshot is no longer stored. Request a fresh observation; its historical text is not current visual evidence.".into()};
                    continue;
                }
                Err(_) => return Err(RuntimeError::Unavailable),
            };
            let bytes = object
                .bytes()
                .await
                .map_err(|_| RuntimeError::Unavailable)?;
            if bytes.len() as u64 != row.try_get::<u64, _>("size_bytes")?
                || format!("{:x}", Sha256::digest(&bytes)) != row.try_get::<String, _>("sha256")?
            {
                return Err(RuntimeError::ProviderRejected);
            }
            sqlx::query("UPDATE artifact_references SET retention_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL COALESCE((SELECT retention_days FROM retention_policy_projection WHERE tenant_id=? AND data_type='trace'),30) DAY) WHERE tenant_id=? AND artifact_id=? AND owner_type='design_preview' AND owner_id=?")
                .bind(target.identity().tenant_id).bind(target.identity().tenant_id).bind(id).bind(design_image_owner(target)).execute(&state.pool).await?;
            image_url.url = format!("data:image/webp;base64,{}", STANDARD.encode(bytes));
        }
    }
    Ok(())
}
