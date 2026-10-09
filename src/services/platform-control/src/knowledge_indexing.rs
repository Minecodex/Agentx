//! Durable Control-owned reconciliation of external LightRAG jobs. HTTP
//! acceptance is never treated as successful indexing. Claims survive API
//! restarts, use fencing, and reconcile an uncertain upload by file_source.

use std::time::Duration;

use agentx_application::ArtifactStore;
use agentx_control_infrastructure::artifact::MySqlControlArtifactStore;
use agentx_domain::{ArtifactId, TenantId};
use agentx_runtime_contracts::VaultSecretReferenceV1;
use anyhow::{Context, Result};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::ControlApiState,
};

#[derive(Deserialize, Serialize)]
struct IndexSnapshot {
    endpoint: String,
    workspace: String,
    index_version: u64,
    credential: Option<VaultSecretReferenceV1>,
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

pub(crate) async fn snapshot(
    state: &ControlApiState,
    tenant: Uuid,
    resource: &sqlx::mysql::MySqlRow,
    version: u64,
) -> ApiResult<Value> {
    let workspace: String = resource.try_get("external_resource_id")?;
    agentx_runtime_contracts::rag::validate_lightrag_workspace(&workspace)
        .map_err(|error| ApiError::unprocessable(error.code, error.message))?;
    serde_json::to_value(IndexSnapshot {
        endpoint: resource.try_get("endpoint")?,
        workspace,
        index_version: version,
        credential: credential_snapshot(state, tenant, resource.try_get("credential_id")?).await?,
    })
    .map_err(ApiError::internal)
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

struct Claim {
    id: Uuid,
    tenant: Uuid,
    resource: Uuid,
    artifact: Uuid,
    status: String,
    track: Option<String>,
    external: Option<String>,
    snapshot: IndexSnapshot,
    fencing: u64,
    age_seconds: i64,
}
struct Outcome {
    status: &'static str,
    track: Option<String>,
    external: Option<String>,
}
#[derive(Debug)]
struct IndexError {
    message: String,
    retryable: bool,
}
impl IndexError {
    fn protocol(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }
}

async fn claim(state: &ControlApiState, owner: Uuid) -> Result<Option<Claim>> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT id,tenant_id,rag_resource_id,artifact_id,status,track_id,external_document_id,index_snapshot_json,fencing_token,TIMESTAMPDIFF(SECOND,created_at,UTC_TIMESTAMP(6)) age_seconds FROM knowledge_documents WHERE status IN ('uploading','indexing') AND next_attempt_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY next_attempt_at,id LIMIT 1 FOR UPDATE SKIP LOCKED")
        .fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let id: Uuid = row.try_get("id")?;
    sqlx::query("UPDATE knowledge_documents SET locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1 WHERE id=?")
        .bind(owner).bind(id).execute(&mut *tx).await?;
    let result = Claim {
        id,
        tenant: row.try_get("tenant_id")?,
        resource: row.try_get("rag_resource_id")?,
        artifact: row.try_get("artifact_id")?,
        status: row.try_get("status")?,
        track: row.try_get("track_id")?,
        external: row.try_get("external_document_id")?,
        snapshot: serde_json::from_value(row.try_get("index_snapshot_json")?)?,
        fencing: row.try_get::<u64, _>("fencing_token")? + 1,
        age_seconds: row.try_get("age_seconds")?,
    };
    tx.commit().await?;
    Ok(Some(result))
}

pub(crate) async fn run_loop(
    state: ControlApiState,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> Result<()> {
    let owner = Uuid::now_v7();
    while !lifecycle.is_draining() {
        let started = std::time::Instant::now();
        if let Some(claim) = claim(&state, owner).await? {
            let future = reconcile(&state, &claim);
            tokio::pin!(future);
            let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
            heartbeat.tick().await;
            let result = loop {
                tokio::select! {
                    result = &mut future => break Some(result),
                    _ = heartbeat.tick() => {
                        let renewed = sqlx::query("UPDATE knowledge_documents SET locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND) WHERE tenant_id=? AND id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
                            .bind(claim.tenant).bind(claim.id).bind(owner).bind(claim.fencing).execute(&state.pool).await?;
                        if renewed.rows_affected() != 1 {
                            tracing::warn!(document_id = %claim.id, "Knowledge index lease was lost");
                            break None;
                        }
                    }
                }
            };
            if let Some(result) = result {
                settle(&state, &claim, owner, result).await?;
            }
        }
        progress.processed_since(started).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(())
}

async fn provider_request(
    state: &ControlApiState,
    snapshot: &IndexSnapshot,
    secret: Option<&str>,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, IndexError> {
    let url = reqwest::Url::parse(&format!(
        "{}/{}",
        snapshot.endpoint.trim_end_matches('/'),
        path
    ))
    .map_err(|_| IndexError::protocol("invalid knowledge endpoint"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(IndexError::protocol("invalid knowledge endpoint scheme"));
    }
    let mut request = state
        .http
        .request(method, url)
        .timeout(Duration::from_secs(15))
        .header("LIGHTRAG-WORKSPACE", &snapshot.workspace);
    if let Some(secret) = secret {
        request = request.header("x-api-key", secret);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| IndexError::unavailable("Knowledge provider unavailable"))?;
    let status = response.status();
    if status.as_u16() == 409 || status.as_u16() == 429 || status.is_server_error() {
        return Err(IndexError::unavailable(format!(
            "Knowledge provider returned HTTP {status}"
        )));
    }
    if !status.is_success() {
        return Err(IndexError::protocol(format!(
            "Knowledge provider returned HTTP {status}"
        )));
    }
    response
        .json()
        .await
        .map_err(|_| IndexError::protocol("Knowledge provider response is not JSON"))
}

fn document_outcome(document: &Value) -> Result<Outcome, IndexError> {
    let external = document["id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| IndexError::protocol("Knowledge provider omitted document ID"))?;
    let status = document["status"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    if status == "failed" {
        return Err(IndexError::protocol(
            "External document indexing failed; inspect the provider document for details",
        ));
    }
    let status = match status.as_str() {
        "processed" => "indexed",
        "pending" | "preprocessing" | "preprocessed" | "processing" | "parsing" | "parsed" => {
            "indexing"
        }
        _ => {
            return Err(IndexError::protocol(
                "Knowledge provider returned an unknown document status",
            ));
        }
    };
    Ok(Outcome {
        status,
        track: document["track_id"].as_str().map(str::to_owned),
        external: Some(external),
    })
}

async fn reconcile(state: &ControlApiState, claim: &Claim) -> Result<Outcome, IndexError> {
    if claim.age_seconds >= 7200 {
        return Err(IndexError::protocol(
            "External indexing did not complete within two hours",
        ));
    }
    let secret = read_secret(state, &claim.snapshot.credential)
        .await
        .map_err(|_| IndexError::unavailable("Frozen Vault credential unavailable"))?;
    let source = format!("agentx_{}.txt", claim.id.simple());
    if let Some(track) = &claim.track {
        let mut path =
            reqwest::Url::parse("http://provider.invalid/documents/track_status/").unwrap();
        path.path_segments_mut().unwrap().pop_if_empty().push(track);
        let path = path.path().trim_start_matches('/');
        let body = provider_request(
            state,
            &claim.snapshot,
            secret.as_deref(),
            reqwest::Method::GET,
            path,
            None,
        )
        .await?;
        let documents = body["documents"]
            .as_array()
            .ok_or_else(|| IndexError::protocol("Knowledge track response omitted documents"))?;
        if let Some(document) = documents
            .iter()
            .find(|document| document["file_path"].as_str() == Some(source.as_str()))
        {
            let mut outcome = document_outcome(document)?;
            outcome.track = Some(track.clone());
            return Ok(outcome);
        }
        return Ok(Outcome {
            status: "indexing",
            track: Some(track.clone()),
            external: claim.external.clone(),
        });
    }
    // The provider may have accepted an upload immediately before a Control
    // crash. Resolve the stable source before sending it again.
    for page in 1..=100 {
        let body = provider_request(state, &claim.snapshot, secret.as_deref(), reqwest::Method::POST, "documents/paginated", Some(json!({"page":page,"page_size":200,"sort_field":"file_path","sort_direction":"asc"}))).await?;
        let documents = body["documents"]
            .as_array()
            .ok_or_else(|| IndexError::protocol("Knowledge listing omitted documents"))?;
        if let Some(document) = documents
            .iter()
            .find(|document| document["file_path"].as_str() == Some(source.as_str()))
        {
            return document_outcome(document);
        }
        let more = body
            .pointer("/pagination/has_next")
            .and_then(Value::as_bool)
            .ok_or_else(|| IndexError::protocol("Knowledge listing omitted pagination"))?;
        if !more {
            break;
        }
        if page == 100 {
            return Err(IndexError::protocol(
                "External workspace exceeds the reconciliation query budget",
            ));
        }
    }
    let store = MySqlControlArtifactStore::new(state.pool.clone(), state.control_objects.clone());
    let source_content = store
        .get(
            TenantId::from_uuid(claim.tenant),
            ArtifactId::from_uuid(claim.artifact),
        )
        .await
        .map_err(|_| IndexError::unavailable("Knowledge source artifact unavailable"))?
        .ok_or_else(|| IndexError::protocol("Knowledge source artifact is missing"))?;
    let content = String::from_utf8(source_content.content)
        .map_err(|_| IndexError::protocol("Knowledge documents must be UTF-8 text"))?;
    let (_, mut body, _) = agentx_runtime_contracts::rag::rag_query_request(
        "lightrag",
        "insert",
        &claim.snapshot.workspace,
        &claim.snapshot.index_version.to_string(),
        &json!({}),
    )
    .map_err(|error| IndexError::protocol(error.message))?;
    body["text"] = json!(content);
    body["file_source"] = json!(source);
    let body = provider_request(
        state,
        &claim.snapshot,
        secret.as_deref(),
        reqwest::Method::POST,
        "documents/text",
        Some(body),
    )
    .await?;
    if body["status"] != "success" {
        return Err(IndexError::protocol(
            "Knowledge provider did not accept the document",
        ));
    }
    let track = body["track_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| IndexError::protocol("Knowledge upload acknowledgement omitted track_id"))?;
    Ok(Outcome {
        status: "indexing",
        track: Some(track),
        external: None,
    })
}

async fn settle(
    state: &ControlApiState,
    claim: &Claim,
    owner: Uuid,
    result: Result<Outcome, IndexError>,
) -> Result<()> {
    let (status, track, external, code, message, delay) = match result {
        Ok(outcome) => (
            outcome.status,
            outcome.track,
            outcome.external,
            None,
            None,
            3,
        ),
        Err(error) => (
            if error.retryable {
                claim.status.as_str()
            } else {
                "failed"
            },
            claim.track.clone(),
            claim.external.clone(),
            Some(if error.retryable {
                "KNOWLEDGE_INDEX_UNAVAILABLE"
            } else {
                "KNOWLEDGE_INDEX_FAILED"
            }),
            Some(error.message),
            30,
        ),
    };
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM rag_resources WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(claim.tenant)
        .bind(claim.resource)
        .fetch_one(&mut *tx)
        .await?;
    let changed = sqlx::query("UPDATE knowledge_documents SET status=?,track_id=?,external_document_id=?,error_code=?,error_message=?,indexed_at=IF(?='indexed',UTC_TIMESTAMP(6),indexed_at),version=version+1,next_attempt_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),locked_by=NULL,locked_until=NULL WHERE tenant_id=? AND id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(status).bind(track).bind(external).bind(code).bind(message).bind(status).bind(delay).bind(claim.tenant).bind(claim.id).bind(owner).bind(claim.fencing).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        tracing::warn!(document_id = %claim.id, "Knowledge index settlement lease was lost");
        return Ok(());
    }
    refresh_resource_status(&mut tx, claim.tenant, claim.resource).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn refresh_resource_status(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    resource: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE rag_resources SET sync_status=CASE WHEN EXISTS(SELECT 1 FROM knowledge_documents WHERE tenant_id=? AND rag_resource_id=? AND status IN ('uploading','indexing')) THEN 'syncing' WHEN EXISTS(SELECT 1 FROM knowledge_documents WHERE tenant_id=? AND rag_resource_id=? AND status='failed') THEN 'failed' ELSE 'synced' END WHERE tenant_id=? AND id=?")
        .bind(tenant).bind(resource).bind(tenant).bind(resource).bind(tenant).bind(resource).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acceptance_and_processing_are_not_indexed() {
        assert_eq!(
            document_outcome(&json!({"id":"doc-real","status":"processing","track_id":"track"}))
                .unwrap()
                .status,
            "indexing"
        );
        assert_eq!(
            document_outcome(&json!({"id":"doc-real","status":"processed"}))
                .unwrap()
                .external
                .as_deref(),
            Some("doc-real")
        );
        assert!(document_outcome(&json!({"id":"doc-real","status":"failed"})).is_err());
        assert!(document_outcome(&json!({"status":"processed"})).is_err());
    }
}
