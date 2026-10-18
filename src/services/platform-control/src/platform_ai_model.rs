use axum::{Json, Router, extract::State, routing::get};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use agentx_domain::{ResourceOperation, ResourceReference, ResourceType, ResourceVersionSnapshot};
use agentx_runtime_contracts::RuntimeResourceBindingV1;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlatformAiModelSetting {
    pub version: u64,
    pub model_alias_id: Option<Uuid>,
    pub model_alias: Option<String>,
    pub status: String,
    pub reason: Option<String>,
    pub can_manage: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetPlatformAiModel {
    pub expected_version: u64,
    #[serde(deserialize_with = "nullable_model")]
    #[schemars(required)]
    pub model_alias_id: Option<Uuid>,
}

pub(crate) fn routes() -> Router<ControlApiState> {
    Router::new().route(
        "/api/v1/platform-ai/model-setting",
        get(get_setting).put(set_setting),
    )
}

pub(crate) fn can_manage(actor: &Actor) -> bool {
    ["model:manage", "company:manage", "resource:grant"]
        .iter()
        .all(|required| actor.permissions.iter().any(|value| value == required))
}

async fn get_setting(
    State(state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<Json<PlatformAiModelSetting>> {
    if !actor.permissions.iter().any(|p| {
        matches!(
            p.as_str(),
            "workflow:edit" | "model:view" | "evaluation:view" | "evaluation:manage"
        )
    }) {
        return Err(ApiError::forbidden(
            "Model settings require workflow, model, or evaluation permission",
        ));
    }
    Ok(Json(load(&state, &actor).await?))
}

pub(crate) async fn load(
    state: &ControlApiState,
    actor: &Actor,
) -> ApiResult<PlatformAiModelSetting> {
    let row = sqlx::query("SELECT s.version,s.platform_ai_model_alias_id model_id,s.platform_ai_credential_id authorized_credential,a.alias,a.status model_status,d.credential_id,d.status deployment_status,c.status credential_status FROM tenant_settings s LEFT JOIN model_aliases a ON a.tenant_id=s.tenant_id AND a.id=s.platform_ai_model_alias_id LEFT JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id LEFT JOIN credentials c ON c.tenant_id=d.tenant_id AND c.id=d.credential_id WHERE s.tenant_id=?")
        .bind(actor.tenant_id).fetch_one(&state.pool).await?;
    let model_id: Option<Uuid> = row.try_get("model_id")?;
    let credential: Option<Uuid> = row.try_get("credential_id")?;
    let authorized: Option<Uuid> = row.try_get("authorized_credential")?;
    let ready = row.try_get::<Option<String>, _>("model_status")?.as_deref() == Some("active")
        && row
            .try_get::<Option<String>, _>("deployment_status")?
            .as_deref()
            == Some("active")
        && credential == authorized
        && (credential.is_none()
            || row
                .try_get::<Option<String>, _>("credential_status")?
                .as_deref()
                == Some("active"));
    Ok(PlatformAiModelSetting {
        version: row.try_get("version")?,
        model_alias_id: model_id,
        model_alias: row.try_get("alias")?,
        status: if model_id.is_none() {
            "unconfigured"
        } else if ready {
            "ready"
        } else {
            "unavailable"
        }
        .into(),
        reason: if model_id.is_none() {
            Some("unconfigured")
        } else if row.try_get::<Option<String>, _>("model_status")?.as_deref() != Some("active") {
            Some("model_disabled")
        } else if row
            .try_get::<Option<String>, _>("deployment_status")?
            .as_deref()
            != Some("active")
        {
            Some("deployment_disabled")
        } else if credential != authorized {
            Some("credential_changed")
        } else if !ready {
            Some("credential_disabled")
        } else {
            None
        }
        .map(str::to_owned),
        can_manage: can_manage(actor),
    })
}

async fn set_setting(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<SetPlatformAiModel>,
) -> ApiResult<Json<PlatformAiModelSetting>> {
    for permission in ["model:manage", "company:manage", "resource:grant"] {
        actor.require(permission)?;
    }
    let mut tx = state.pool.begin().await?;
    let current: u64 =
        sqlx::query_scalar("SELECT version FROM tenant_settings WHERE tenant_id=? FOR UPDATE")
            .bind(actor.tenant_id)
            .fetch_one(&mut *tx)
            .await?;
    if current != input.expected_version {
        return Err(ApiError::conflict(
            "PLATFORM_AI_SETTING_CONFLICT",
            "The tenant model setting changed; reload before setting it",
        ));
    }
    let credential = if let Some(model_id) = input.model_alias_id {
        let model = sqlx::query("SELECT a.status,d.status deployment_status,d.credential_id FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND a.id=? FOR UPDATE")
            .bind(actor.tenant_id).bind(model_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| ApiError::not_found("Model"))?;
        if model.try_get::<String, _>("status")? != "active"
            || model.try_get::<String, _>("deployment_status")? != "active"
        {
            return Err(ApiError::unprocessable(
                "PLATFORM_AI_MODEL_UNAVAILABLE",
                "The model is disabled",
            ));
        }
        let credential: Option<Uuid> = model.try_get("credential_id")?;
        if let Some(id) = credential {
            let available: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM credentials WHERE tenant_id=? AND id=? AND status='active')")
                .bind(actor.tenant_id).bind(id).fetch_one(&mut *tx).await?;
            if !available {
                return Err(ApiError::unprocessable(
                    "CREDENTIAL_UNAVAILABLE",
                    "The model credential is unavailable",
                ));
            }
        }
        credential
    } else {
        None
    };
    sqlx::query("UPDATE tenant_settings SET platform_ai_model_alias_id=?,platform_ai_credential_id=?,version=version+1 WHERE tenant_id=? AND version=?")
        .bind(input.model_alias_id).bind(credential).bind(actor.tenant_id).bind(current).execute(&mut *tx).await?;
    crate::workflow_api::audit(
        &mut tx,
        &actor,
        "platform_ai.model_setting",
        "tenant",
        actor.tenant_id,
        json!({"modelAliasId":input.model_alias_id,"version":current+1}),
    )
    .await?;
    let revoked = if input.model_alias_id.is_none() {
        enqueue_evaluation_revocations(
            &mut tx,
            actor.tenant_id,
            crate::studio_builder_api::DesignRevocation::Setting(current),
        )
        .await?
    } else {
        Vec::new()
    };
    tx.commit().await?;
    publish_evaluation_revocations(&state, revoked).await?;
    if input.model_alias_id.is_none() {
        crate::studio_builder_api::revoke_runs(
            &state,
            actor.tenant_id,
            crate::studio_builder_api::DesignRevocation::Setting(current),
        )
        .await?;
    }
    Ok(Json(load(&state, &actor).await?))
}

fn nullable_model<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Uuid>, D::Error> {
    Option::<Uuid>::deserialize(deserializer)
}

#[cfg(test)]
mod tests {
    use super::SetPlatformAiModel;
    use serde_json::json;

    #[test]
    fn clearing_a_model_requires_an_explicit_null() {
        assert!(
            serde_json::from_value::<SetPlatformAiModel>(json!({"expectedVersion":1})).is_err()
        );
        let cleared: SetPlatformAiModel =
            serde_json::from_value(json!({"expectedVersion":1,"modelAliasId":null})).unwrap();
        assert!(cleared.model_alias_id.is_none());
        let schema = serde_json::to_value(schemars::schema_for!(SetPlatformAiModel)).unwrap();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("modelAliasId"))
        );
    }
}

pub(crate) async fn freeze_model(
    state: &ControlApiState,
    actor: &Actor,
) -> ApiResult<(
    Uuid,
    Option<Uuid>,
    u64,
    RuntimeResourceBindingV1,
    Value,
    u64,
    Option<u64>,
)> {
    let setting = crate::platform_ai_model::load(state, actor).await?;
    if setting.status != "ready" {
        return Err(ApiError::conflict(
            "PLATFORM_AI_MODEL_UNAVAILABLE",
            "The tenant platform AI model is unconfigured or unavailable",
        ));
    }
    let alias = setting.model_alias_id.ok_or_else(|| {
        ApiError::conflict(
            "PLATFORM_AI_MODEL_UNAVAILABLE",
            "No platform AI model is configured",
        )
    })?;
    let credential: Option<Uuid> = sqlx::query_scalar(
        "SELECT platform_ai_credential_id FROM tenant_settings WHERE tenant_id=?",
    )
    .bind(actor.tenant_id)
    .fetch_one(&state.pool)
    .await?;
    let mut reference = ResourceReference {
        binding_role: None,
        resource_type: ResourceType::Model,
        resource_id: alias,
        resource_version_id: None,
        operation: ResourceOperation::Use,
    };
    let snapshot =
        crate::workflow_resources::resource_snapshot(state, actor.tenant_id, &mut reference)
            .await?;
    let hash = agentx_runtime_contracts::content_hash(&snapshot)
        .map_err(ApiError::internal)?
        .to_string();
    let binding = crate::runtime_resource_binding::from_snapshot(&ResourceVersionSnapshot {
        node_id: "platform_ai".into(),
        reference,
        snapshot: snapshot.clone(),
        snapshot_hash: hash,
    })
    .map_err(ApiError::internal)?;
    let versions=sqlx::query("SELECT a.version model_version,c.version credential_version FROM model_aliases a LEFT JOIN credentials c ON c.tenant_id=a.tenant_id AND c.id=? WHERE a.tenant_id=? AND a.id=?").bind(credential).bind(actor.tenant_id).bind(alias).fetch_one(&state.pool).await?;
    Ok((
        alias,
        credential,
        setting.version,
        binding,
        {
            let mut parameters = snapshot
                .get("defaultParameters")
                .cloned()
                .ok_or_else(|| ApiError::internal("Model defaults are missing"))?;
            let output = snapshot
                .get("maxOutputTokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| ApiError::internal("Model output limit is missing"))?
                .min(16_384);
            let limit = if parameters.get("max_completion_tokens").is_some() {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            parameters[limit] = json!(
                parameters
                    .get(limit)
                    .and_then(Value::as_u64)
                    .unwrap_or(output)
                    .min(output)
            );
            parameters
        },
        versions.try_get("model_version")?,
        versions.try_get("credential_version")?,
    ))
}

pub(crate) async fn enqueue_evaluation_revocations(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    tenant: Uuid,
    revocation: crate::studio_builder_api::DesignRevocation,
) -> ApiResult<Vec<agentx_runtime_contracts::RuntimeAdmissionCommandV1>> {
    use agentx_runtime_contracts::{
        AdmissionTargetV1, CommandEnvelopeV1, Plane, RuntimeAdmissionCommandV1,
        RuntimePlatformEvaluationAuthorizationV1, RuntimePlatformEvaluationModelV1, content_hash,
    };
    let rows=sqlx::query("SELECT id,model_snapshot_json FROM evaluation_batches WHERE tenant_id=? AND status IN ('queued','running')").bind(tenant).fetch_all(&mut **tx).await?;
    let mut commands = Vec::new();
    for row in rows {
        let model: RuntimePlatformEvaluationModelV1 =
            serde_json::from_value(row.try_get("model_snapshot_json")?)
                .map_err(ApiError::internal)?;
        let matches = match revocation {
            crate::studio_builder_api::DesignRevocation::Setting(version) => {
                model.setting_version <= version
            }
            crate::studio_builder_api::DesignRevocation::Model(id, version) => {
                model.binding.resource_id == id && model.model_alias_version <= version
            }
            crate::studio_builder_api::DesignRevocation::Credential(id, version) => {
                model.credential_id == Some(id)
                    && model.credential_version.is_some_and(|v| v <= version)
            }
        };
        if !matches {
            continue;
        }
        let id = Uuid::now_v7();
        let target = AdmissionTargetV1::PlatformEvaluation {
            state: RuntimePlatformEvaluationAuthorizationV1 {
                tenant_id: tenant,
                application_id: model.application_id,
                batch_id: model.batch_id,
                model_hash: content_hash(&model).map_err(ApiError::internal)?,
                epoch: model.setting_version + 1,
                enabled: false,
            },
        };
        let command = RuntimeAdmissionCommandV1 {
            api_version: 1,
            admission_epoch: model.setting_version + 1,
            command: CommandEnvelopeV1 {
                schema_version: 1,
                event_id: id,
                source_plane: Plane::Control,
                tenant_id: tenant,
                aggregate_type: "platform_evaluation".into(),
                aggregate_id: model.batch_id.to_string(),
                object_version: model.setting_version + 1,
                occurred_at: time::OffsetDateTime::now_utc(),
                payload: json!({}),
                content_hash: content_hash(&target).map_err(ApiError::internal)?,
                correlation_id: id,
                causation_id: None,
                idempotency_key: format!("platform-evaluation-revoke:{id}"),
            },
            target,
        };
        let payload = json!(command);
        sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,'PlatformEvaluationModelRevoked','evaluation_model_admission',?,?,'pending',?,?)")
            .bind(id).bind(tenant).bind(model.batch_id.to_string()).bind(&payload).bind(content_hash(&payload).map_err(ApiError::internal)?.as_str()).bind(&command.command.idempotency_key).execute(&mut **tx).await?;
        commands.push(command);
    }
    Ok(commands)
}

pub(crate) async fn publish_evaluation_revocations(
    state: &ControlApiState,
    commands: Vec<agentx_runtime_contracts::RuntimeAdmissionCommandV1>,
) -> ApiResult<()> {
    for command in commands {
        match crate::governance_api::post_runtime_admission(state, &command).await {
            Ok(receipt) if receipt.applied || receipt.replayed => {
                sqlx::query(
                    "UPDATE outbox SET status='published',published_at=UTC_TIMESTAMP(6) WHERE id=? AND status IN ('pending','failed')",
                )
                .bind(command.command.event_id)
                .execute(&state.pool)
                .await?;
            }
            _ => {
                tracing::warn!(batch=%command.command.aggregate_id,"Platform model revocation queued for durable delivery");
            }
        }
    }
    Ok(())
}
