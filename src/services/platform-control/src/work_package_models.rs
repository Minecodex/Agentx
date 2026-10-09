//! Model evaluator bindings and their complete authorization prerequisites.

use super::*;

pub(super) async fn require_model_grant(
    pool: &MySqlPool,
    tenant_id: Uuid,
    service_identity_id: Uuid,
    model_id: Uuid,
) -> ApiResult<()> {
    let granted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grants WHERE tenant_id=? AND subject_type='workflow_service_identity' AND subject_id=? AND resource_type='model' AND resource_id=? AND operation_key IN ('use','manage'))")
        .bind(tenant_id)
        .bind(service_identity_id)
        .bind(model_id)
        .fetch_one(pool)
        .await?;
    if granted {
        let credential: Option<Uuid> = sqlx::query_scalar("SELECT d.credential_id FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id WHERE a.tenant_id=? AND a.id=?")
            .bind(tenant_id).bind(model_id).fetch_optional(pool).await?.flatten();
        if let Some(credential) = credential {
            let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grants WHERE tenant_id=? AND subject_type='workflow_service_identity' AND subject_id=? AND resource_type='credential' AND resource_id=? AND operation_key IN ('use','manage'))")
                .bind(tenant_id).bind(service_identity_id).bind(credential).fetch_one(pool).await?;
            if !allowed {
                return Err(ApiError::unprocessable(
                    "MODEL_EVALUATOR_CREDENTIAL_GRANT_REQUIRED",
                    "Evaluation Workflow identity is not authorized to use the Model evaluator Credential",
                ));
            }
        }
        Ok(())
    } else {
        Err(ApiError::unprocessable(
            "MODEL_EVALUATOR_GRANT_REQUIRED",
            "Evaluation Workflow identity is not authorized to use the Model evaluator",
        ))
    }
}

pub(super) async fn load_model_binding(
    state: &ControlApiState,
    tenant_id: Uuid,
    model_id: Uuid,
) -> ApiResult<RuntimeResourceBindingV1> {
    let row = sqlx::query("SELECT a.version alias_version,d.id deployment_id,d.version deployment_version,d.provider_type,d.endpoint,d.model_name,d.credential_id,d.capabilities_json,p.id price_version_id,p.currency,CAST(p.input_per_million AS CHAR) input_per_million,CAST(p.output_per_million AS CHAR) output_per_million FROM model_aliases a JOIN model_deployments d ON d.tenant_id=a.tenant_id AND d.id=a.deployment_id JOIN model_price_versions p ON p.tenant_id=d.tenant_id AND p.deployment_id=d.id AND p.id=(SELECT latest.id FROM model_price_versions latest WHERE latest.tenant_id=d.tenant_id AND latest.deployment_id=d.id ORDER BY latest.version_number DESC LIMIT 1) WHERE a.tenant_id=? AND a.id=? AND a.status='active' AND d.status='active'")
        .bind(tenant_id)
        .bind(model_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Active Model evaluator"))?;
    let credential = if let Some(credential_id) = row.try_get::<Option<Uuid>, _>("credential_id")? {
        let secret = sqlx::query("SELECT s.secret_ref,s.provider_version FROM credentials c JOIN credential_secret_versions s ON s.tenant_id=c.tenant_id AND s.credential_id=c.id AND s.version_number=c.current_secret_version WHERE c.tenant_id=? AND c.id=? AND c.status='active' AND s.provider='vault_kv_v2'")
            .bind(tenant_id)
            .bind(credential_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::unprocessable("MODEL_CREDENTIAL_UNAVAILABLE", "Model evaluator Credential has no active Vault version"))?;
        Some(VaultSecretReferenceV1 {
            mount: state.vault_mount.clone(),
            path: secret.try_get("secret_ref")?,
            key: "value".into(),
            version: secret
                .try_get::<String, _>("provider_version")?
                .parse()
                .map_err(|_| ApiError::internal("Vault provider version is invalid"))?,
        })
    } else {
        None
    };
    let deployment_id: Uuid = row.try_get("deployment_id")?;
    let configuration = RuntimeResourceConfigurationV1::Model {
        provider: row.try_get("provider_type")?,
        endpoint: row.try_get("endpoint")?,
        model: row.try_get("model_name")?,
        context_window: 128_000,
        price: agentx_runtime_contracts::RuntimeModelPriceV1 {
            version_id: row.try_get::<Uuid, _>("price_version_id")?.to_string(),
            currency: row.try_get("currency")?,
            input_per_million: row.try_get("input_per_million")?,
            output_per_million: row.try_get("output_per_million")?,
        },
        credential,
        capabilities: row
            .try_get::<Option<Value>, _>("capabilities_json")?
            .and_then(|value| value.as_array().cloned())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    };
    Ok(RuntimeResourceBindingV1 {
        resource_kind: RuntimeResourceKindV1::Model,
        resource_id: model_id,
        resource_version: deployment_id.to_string(),
        state_epoch: row
            .try_get::<u64, _>("alias_version")?
            .max(row.try_get("deployment_version")?),
        content_hash: agentx_runtime_contracts::content_hash(&configuration)
            .map_err(ApiError::internal)?,
        configuration,
        object_ids: Vec::new(),
    })
}
