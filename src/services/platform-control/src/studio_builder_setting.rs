use axum::{Json, Router, extract::State, routing::get};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuilderModelSetting {
    pub version: u64,
    pub model_alias_id: Option<Uuid>,
    pub model_alias: Option<String>,
    pub status: String,
    pub reason: Option<String>,
    pub can_manage: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetBuilderModel {
    pub expected_version: u64,
    #[serde(deserialize_with = "nullable_model")]
    #[schemars(required)]
    pub model_alias_id: Option<Uuid>,
}

pub(crate) fn routes() -> Router<ControlApiState> {
    Router::new().route(
        "/api/v1/studio-builder/model-setting",
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
) -> ApiResult<Json<BuilderModelSetting>> {
    if !actor
        .permissions
        .iter()
        .any(|p| p == "workflow:edit" || p == "model:view")
    {
        return Err(ApiError::forbidden(
            "Model settings require workflow edit or model view permission",
        ));
    }
    Ok(Json(load(&state, &actor).await?))
}

pub(crate) async fn load(state: &ControlApiState, actor: &Actor) -> ApiResult<BuilderModelSetting> {
    let row = sqlx::query("SELECT s.version,s.studio_builder_model_alias_id model_id,s.studio_builder_credential_id authorized_credential,a.alias,a.status model_status,d.credential_id,d.status deployment_status,c.status credential_status FROM tenant_settings s LEFT JOIN model_aliases a ON a.tenant_id=s.tenant_id AND a.id=s.studio_builder_model_alias_id LEFT JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id LEFT JOIN credentials c ON c.tenant_id=d.tenant_id AND c.id=d.credential_id WHERE s.tenant_id=?")
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
    Ok(BuilderModelSetting {
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
    Json(input): Json<SetBuilderModel>,
) -> ApiResult<Json<BuilderModelSetting>> {
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
            "BUILDER_MODEL_SETTING_CONFLICT",
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
                "BUILDER_MODEL_UNAVAILABLE",
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
    sqlx::query("UPDATE tenant_settings SET studio_builder_model_alias_id=?,studio_builder_credential_id=?,version=version+1 WHERE tenant_id=? AND version=?")
        .bind(input.model_alias_id).bind(credential).bind(actor.tenant_id).bind(current).execute(&mut *tx).await?;
    crate::workflow_api::audit(
        &mut tx,
        &actor,
        "studio_builder.model_setting",
        "tenant",
        actor.tenant_id,
        json!({"modelAliasId":input.model_alias_id,"version":current+1}),
    )
    .await?;
    tx.commit().await?;
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
    use super::SetBuilderModel;
    use serde_json::json;

    #[test]
    fn clearing_a_model_requires_an_explicit_null() {
        assert!(serde_json::from_value::<SetBuilderModel>(json!({"expectedVersion":1})).is_err());
        let cleared: SetBuilderModel =
            serde_json::from_value(json!({"expectedVersion":1,"modelAliasId":null})).unwrap();
        assert!(cleared.model_alias_id.is_none());
        let schema = serde_json::to_value(schemars::schema_for!(SetBuilderModel)).unwrap();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("modelAliasId"))
        );
    }
}
