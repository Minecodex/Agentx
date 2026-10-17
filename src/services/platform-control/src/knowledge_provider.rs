//! Read-only external knowledge provider access. Documents and indexing are provider-owned.
use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};
use agentx_runtime_contracts::{VaultSecretReferenceV1, rag::RagScope};
use anyhow::{Context, Result};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

pub(crate) struct Provider {
    pub kind: String,
    pub endpoint: String,
    secret: Option<String>,
}

impl Provider {
    pub async fn new(
        state: &ControlApiState,
        tenant: Uuid,
        kind: &str,
        endpoint: &str,
        credential: Option<Uuid>,
    ) -> ApiResult<Self> {
        let reference = credential_snapshot(state, tenant, credential).await?;
        let secret = read_secret(state, &reference).await.map_err(|_| {
            ApiError::unavailable("VAULT_UNAVAILABLE", "Credential could not be read")
        })?;
        Ok(Self {
            kind: kind.into(),
            endpoint: endpoint.trim_end_matches('/').into(),
            secret,
        })
    }

    pub async fn request(
        &self,
        state: &ControlApiState,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, agentx_runtime_contracts::rag::RagProtocolError> {
        use agentx_runtime_contracts::rag::RagProtocolError;
        let mut request = state
            .http
            .request(
                method,
                format!("{}/{}", self.endpoint, path.trim_start_matches('/')),
            )
            .timeout(Duration::from_secs(20));
        if let Some(secret) = &self.secret {
            request = if self.kind == "ragflow" {
                request.bearer_auth(secret)
            } else {
                request.header("x-api-key", secret)
            };
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|_| {
            RagProtocolError::new("PROVIDER_UNAVAILABLE", "Knowledge provider is unreachable")
        })?;
        if !response.status().is_success() {
            return Err(RagProtocolError::new(
                "PROVIDER_REJECTED",
                format!("Knowledge provider returned HTTP {}", response.status()),
            ));
        }
        let value: Value = response.json().await.map_err(|_| {
            RagProtocolError::new(
                "PROVIDER_REJECTED",
                "Knowledge provider returned invalid JSON",
            )
        })?;
        if self.kind == "ragflow" && value.get("code").and_then(Value::as_i64) != Some(0) {
            return Err(RagProtocolError::new(
                "PROVIDER_REJECTED",
                value["message"]
                    .as_str()
                    .unwrap_or("RAGFlow rejected the request"),
            ));
        }
        Ok(value)
    }

    pub async fn catalog(&self, state: &ControlApiState) -> ApiResult<Vec<Value>> {
        if self.kind == "lightrag" {
            let value = self
                .request(state, reqwest::Method::GET, "health", None)
                .await
                .map_err(protocol_error)?;
            let workspace = value.pointer("/configuration/workspace").and_then(Value::as_str).ok_or_else(|| ApiError::unprocessable("RAG_WORKSPACE_UNAVAILABLE", "The authenticated server health response must expose configuration.workspace"))?;
            return Ok(vec![
                json!({"id":workspace,"name":workspace,"scope":{"kind":"lightrag","workspace":workspace}}),
            ]);
        }
        let mut result = Vec::new();
        let mut page = 1;
        loop {
            let value = self
                .request(
                    state,
                    reqwest::Method::GET,
                    &format!("api/v1/datasets?page={page}&page_size=100"),
                    None,
                )
                .await
                .map_err(protocol_error)?;
            let items = value["data"].as_array().ok_or_else(|| {
                ApiError::unprocessable("PROVIDER_REJECTED", "RAGFlow omitted datasets")
            })?;
            for item in items {
                let id = item["id"].as_str().ok_or_else(|| {
                    ApiError::unprocessable("PROVIDER_REJECTED", "Dataset ID is missing")
                })?;
                result.push(json!({"id":id,"name":item["name"],"scope":{"kind":"ragflow","datasetIds":[id]}}));
            }
            if items.len() < 100 {
                break;
            }
            page += 1;
        }
        Ok(result)
    }
}

pub(crate) fn protocol_error(error: agentx_runtime_contracts::rag::RagProtocolError) -> ApiError {
    ApiError::unprocessable(error.code, error.message)
}

pub(crate) async fn connection_provider(
    state: &ControlApiState,
    actor: &Actor,
    id: Uuid,
) -> ApiResult<Provider> {
    crate::external_resource_api::require_connection(state, actor, "rag_connections", id).await?;
    let row = sqlx::query("SELECT provider,endpoint,credential_id FROM rag_connections WHERE tenant_id=? AND id=? AND status='active'").bind(actor.tenant_id).bind(id).fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::unprocessable("CONNECTION_DISABLED", "Connection is disabled"))?;
    crate::resource_api::require_knowledge_credential(state, actor, row.try_get("credential_id")?)
        .await?;
    Provider::new(
        state,
        actor.tenant_id,
        row.try_get("provider")?,
        row.try_get("endpoint")?,
        row.try_get("credential_id")?,
    )
    .await
}

pub(crate) async fn catalog(
    axum::extract::State(state): axum::extract::State<ControlApiState>,
    actor: Actor,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> ApiResult<axum::Json<Vec<Value>>> {
    actor.require("knowledge:view")?;
    Ok(axum::Json(
        connection_provider(&state, &actor, id)
            .await?
            .catalog(&state)
            .await?,
    ))
}

pub(crate) fn scope_in_catalog(
    scope: &RagScope,
    provider: &str,
    catalog: &[Value],
) -> ApiResult<()> {
    scope.validate(provider).map_err(protocol_error)?;
    let ids = match scope {
        RagScope::Ragflow { dataset_ids } => dataset_ids.clone(),
        RagScope::Lightrag { workspace } => vec![workspace.clone()],
    };
    if ids
        .iter()
        .any(|id| !catalog.iter().any(|item| item["id"].as_str() == Some(id)))
    {
        return Err(ApiError::unprocessable(
            "RAG_SCOPE_UNAVAILABLE",
            "Select content returned by this connection",
        ));
    }
    Ok(())
}

pub(crate) async fn validate_scope(
    state: &ControlApiState,
    actor: &Actor,
    connection: Uuid,
    scope: &RagScope,
) -> ApiResult<()> {
    let provider = connection_provider(state, actor, connection).await?;
    scope_in_catalog(scope, &provider.kind, &provider.catalog(state).await?)
}

pub(crate) async fn credential_snapshot(
    state: &ControlApiState,
    tenant: Uuid,
    id: Option<Uuid>,
) -> ApiResult<Option<VaultSecretReferenceV1>> {
    let Some(id) = id else { return Ok(None) };
    let row = sqlx::query("SELECT v.secret_ref,v.provider_version FROM credentials c JOIN credential_secret_versions v ON v.tenant_id=c.tenant_id AND v.credential_id=c.id AND v.version_number=c.current_secret_version WHERE c.tenant_id=? AND c.id=? AND c.status='active' AND v.provider='vault_kv_v2'")
        .bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::unprocessable("KNOWLEDGE_CREDENTIAL_UNAVAILABLE", "Knowledge credential has no active Vault version"))?;
    Ok(Some(VaultSecretReferenceV1 {
        mount: state.vault_mount.clone(),
        path: row.try_get("secret_ref")?,
        key: "value".into(),
        version: row
            .try_get::<String, _>("provider_version")?
            .parse()
            .map_err(ApiError::internal)?,
    }))
}

pub(crate) async fn read_secret(
    state: &ControlApiState,
    reference: &Option<VaultSecretReferenceV1>,
) -> Result<Option<String>> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    let response = state
        .http
        .get(format!(
            "{}/v1/{}/data/{}",
            state.vault_endpoint.trim_end_matches('/'),
            reference.mount.trim_matches('/'),
            reference.path.trim_start_matches('/')
        ))
        .query(&[("version", reference.version)])
        .timeout(Duration::from_secs(15))
        .header("X-Vault-Token", state.vault_token.expose_secret())
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Vault request unavailable"))?;
    anyhow::ensure!(
        response.status().is_success(),
        "Vault credential snapshot unavailable"
    );
    let body: Value = response.json().await.context("unreadable Vault response")?;
    body.pointer(&format!("/data/data/{}", reference.key))
        .and_then(Value::as_str)
        .map(|value| Some(value.to_owned()))
        .context("Vault credential value is missing or not text")
}
