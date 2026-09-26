use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    future::Future,
    time::Duration,
};

use agentx_bundle_builder::{
    BundleBuildSource, build_bundle_with_resolved, compile_workflow_version_with_resolved_plugins,
    composite_ir_object_id,
};
use agentx_control_infrastructure::{
    ControlInfrastructureSettings, connect_control_mysql, control_object_store,
};
use agentx_domain::WorkflowDefinition;
use agentx_mysql_lease::{DEFAULT_BATCH_SIZE, LeaseOwner};
use agentx_runtime_contracts::{
    ActivateDeploymentRequestV1, ActivationManifestV1, AdmissionStatusV1, AdmissionTargetV1,
    ApiKeyAdmissionV1, ApplicationRouteAdmissionV1, ApplyChatMappingReceiptV1,
    ApplyChatMappingRequestV1, ChatMappingV1, CommandEnvelopeV1, ContentHash, ControlRole,
    DisableDeploymentRequestV1, Plane, PrepareBundleRequestV1, RollbackDeploymentRequestV1,
    RuntimeAdmissionCommandV1, RuntimeAuthorizationSnapshotV1, RuntimeGrantStateV1,
    RuntimePolicyV1, RuntimeResourceBindingV1, RuntimeResourceConfigurationV1,
    RuntimeResourceKindV1, ServiceClaimsV1, ServiceIdentityAdmissionV1, issue_service_token,
    now_unix,
};
use anyhow::{Context, Result};
use axum::Router;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use ed25519_dalek::{SigningKey, pkcs8::DecodePrivateKey};
use object_store::{ObjectStore, path::Path as ObjectPath};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, Row};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

mod api_error;
mod application_catalog_api;
mod bootstrap_api;
mod canvas_plugin_api;
mod canvas_plugin_resolution;
mod catalog_api;
mod control_api;
mod control_helpers;
mod credential_api;
mod dataset_api;
mod deletion_api;
mod external_resource_api;
mod governance_api;
mod iam_api;
mod knowledge_document_api;
mod mcp_api;
mod model_api;
mod openapi_contract;
mod operations_api;
mod projector;
mod resource_api;
mod retention;
mod role_health;
mod runtime_admission;
mod runtime_bff;
mod runtime_grants;
mod runtime_resource_binding;
mod sandbox_profile_api;
mod skill_api;
mod work_packages;
mod workflow_api;
mod workflow_operations;
mod workflow_resources;

use control_helpers::{
    control_roles as roles, payload_strings, payload_uuid, payload_uuids, required_env as required,
};
#[cfg(test)]
use runtime_resource_binding::runtime_resource_kind_from_control;

#[cfg(test)]
mod api_first_tests;

struct Publisher {
    pool: MySqlPool,
    control_objects: std::sync::Arc<dyn ObjectStore>,
    runtime_url: String,
    http: reqwest::Client,
    jwt_kid: String,
    jwt_key: SecretString,
    bundle_kid: String,
    bundle_key: SigningKey,
    owner: LeaseOwner,
}

struct Attempt {
    id: Uuid,
    tenant_id: Uuid,
    application_id: Uuid,
    deployment_id: Uuid,
    bundle_id: Option<Uuid>,
    requested_action: String,
    state: String,
    next_action: String,
    activation_sequence: u64,
    minimum_admission_epoch: u64,
    expected_head_version: Option<u64>,
    fencing_token: u64,
}

struct AdmissionEvent {
    id: Uuid,
    tenant_id: Uuid,
    aggregate_type: String,
    aggregate_id: Uuid,
    event_type: String,
    payload: Value,
    occurred_at: OffsetDateTime,
    fencing_token: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(path) = openapi_contract::requested_output()? {
        return openapi_contract::write(&path);
    }
    let settings = ControlInfrastructureSettings::from_env()?;
    let publisher = Publisher {
        pool: connect_control_mysql(&settings.mysql).await?,
        control_objects: control_object_store(&settings.object_storage)?,
        runtime_url: env::var("AGENTX_RUNTIME_INTERNAL_URL")
            .unwrap_or_else(|_| "http://runtime-gateway.agentx-runtime.svc:8080".into()),
        http: reqwest::Client::new(),
        jwt_kid: required("AGENTX_CONTROL_PUBLISHER_JWT_KID")?,
        jwt_key: SecretString::from(required("AGENTX_CONTROL_PUBLISHER_JWT_PRIVATE_KEY_PEM")?),
        bundle_kid: required("AGENTX_CONTROL_BUNDLE_KEY_ID")?,
        bundle_key: SigningKey::from_pkcs8_pem(&required(
            "AGENTX_CONTROL_BUNDLE_ED25519_PRIVATE_KEY_PEM",
        )?)?,
        owner: LeaseOwner::for_process()?,
    };
    let roles = roles()?;
    let lifecycle = agentx_service_kit::ServiceLifecycle::default();
    let metrics = agentx_service_kit::MetricsRegistry::default();
    let health = agentx_service_kit::HealthRegistry::default();
    health.register("control_mysql", true).await;
    health.register("control_object_storage", true).await;
    health.set_status("control_mysql", "ready").await;
    health.set_status("control_object_storage", "ready").await;
    let publisher = std::sync::Arc::new(publisher);
    let mut tasks = tokio::task::JoinSet::new();
    if roles.contains("publisher") {
        let publish_worker = publisher.clone();
        let publish_lifecycle = lifecycle.clone();
        let publish_progress =
            role_health::watchdog("publisher", &health, &lifecycle, &metrics).await;
        tasks.spawn(async move {
            publish_worker
                .publish_loop(publish_lifecycle, publish_progress)
                .await
        });
        let admission_worker = publisher.clone();
        let admission_lifecycle = lifecycle.clone();
        let admission_progress =
            role_health::watchdog("admission-outbox", &health, &lifecycle, &metrics).await;
        tasks.spawn(async move {
            admission_worker
                .admission_loop(admission_lifecycle, admission_progress)
                .await
        });
    }
    if roles.contains("retention") {
        let retention_publisher = publisher.clone();
        let retention_lifecycle = lifecycle.clone();
        let retention_progress =
            role_health::watchdog("retention", &health, &lifecycle, &metrics).await;
        tasks.spawn(async move {
            retention::run_loop(retention_publisher, retention_lifecycle, retention_progress).await
        });
    }
    if roles.contains("projector") {
        let projector = projector::Projector::from_env(publisher.pool.clone())?;
        let projector_lifecycle = lifecycle.clone();
        let projector_progress =
            role_health::watchdog("projector", &health, &lifecycle, &metrics).await;
        tasks.spawn(async move {
            projector
                .run_loop(projector_lifecycle, projector_progress)
                .await
        });
    }
    anyhow::ensure!(
        !tasks.is_empty() || roles.contains("api"),
        "AGENTX_CONTROL_ROLES selected no implemented role"
    );
    let router = if roles.contains("api") {
        control_api::router(control_api::ControlApiState::from_env(
            publisher.pool.clone(),
            publisher.control_objects.clone(),
        )?)
    } else {
        Router::new()
    };
    let service_lifecycle = lifecycle.clone();
    let service_metrics = metrics.clone();
    let metrics_health = health.clone();
    tasks.spawn(async move {
        agentx_service_kit::serve_with_lifecycle(
            "platform-control",
            router,
            health,
            service_lifecycle,
            service_metrics,
        )
        .await
    });
    let metrics_pool = publisher.pool.clone();
    let metrics_lifecycle = lifecycle.clone();
    tasks.spawn(async move {
        role_health::collect_control_metrics(
            metrics_pool,
            metrics,
            metrics_health,
            metrics_lifecycle,
        )
        .await
    });
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

impl Publisher {
    async fn publish_loop(
        &self,
        lifecycle: agentx_service_kit::ServiceLifecycle,
        progress: agentx_service_kit::RoleProgressWatchdog,
    ) -> Result<()> {
        loop {
            if lifecycle.is_draining() {
                return Ok(());
            }
            let started = std::time::Instant::now();
            for attempt in self.claim().await? {
                if let Err(error) = self.advance(&attempt).await {
                    tracing::warn!(attempt_id=%attempt.id, %error, "Publish Attempt failed");
                    if let Err(record_error) = self.record_failure(&attempt, &error).await {
                        tracing::warn!(attempt_id=%attempt.id, %record_error, "Failed to persist Publish Attempt failure");
                    }
                }
            }
            progress.processed_since(started).await;
            tokio::time::sleep(Duration::from_millis(250)).await
        }
    }

    async fn claim(&self) -> Result<Vec<Attempt>> {
        let mut tx = self.pool.begin().await?;
        let rows=sqlx::query("SELECT id FROM publish_attempts WHERE state NOT IN ('active','rejected') AND available_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY created_at,id LIMIT ? FOR UPDATE SKIP LOCKED").bind(DEFAULT_BATCH_SIZE).fetch_all(&mut *tx).await?;
        for row in &rows {
            sqlx::query("UPDATE publish_attempts SET locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=? AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6))").bind(self.owner.0).bind(row.try_get::<Uuid,_>("id")?).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        let rows=sqlx::query("SELECT id,tenant_id,application_id,deployment_id,bundle_id,requested_action,state,next_action,activation_sequence,minimum_admission_epoch,expected_head_version,fencing_token FROM publish_attempts WHERE locked_by=? AND locked_until>UTC_TIMESTAMP(6) AND state NOT IN ('active','rejected') ORDER BY created_at,id").bind(self.owner.0).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| {
                Ok(Attempt {
                    id: r.try_get("id")?,
                    tenant_id: r.try_get("tenant_id")?,
                    application_id: r.try_get("application_id")?,
                    deployment_id: r.try_get("deployment_id")?,
                    bundle_id: r.try_get("bundle_id")?,
                    requested_action: r.try_get("requested_action")?,
                    state: r.try_get("state")?,
                    next_action: r.try_get("next_action")?,
                    activation_sequence: r.try_get("activation_sequence")?,
                    minimum_admission_epoch: r.try_get("minimum_admission_epoch")?,
                    expected_head_version: r.try_get("expected_head_version")?,
                    fencing_token: r.try_get("fencing_token")?,
                })
            })
            .collect()
    }

    async fn advance(&self, attempt: &Attempt) -> Result<()> {
        self.with_heartbeat(attempt, async {
            match attempt.next_action.as_str() {
                "build" => self.build(attempt).await,
                "copy_objects" => self.copy_objects(attempt).await,
                "apply_admission" => self.apply_admission(attempt).await,
                "prepare" => self.prepare(attempt).await,
                "activate" => self.activate(attempt).await,
                "update_head" => self.update_head(attempt).await,
                "none" => Ok(()),
                action => anyhow::bail!("unknown Publisher action {action}"),
            }
        })
        .await
    }

    async fn admission_loop(
        &self,
        lifecycle: agentx_service_kit::ServiceLifecycle,
        progress: agentx_service_kit::RoleProgressWatchdog,
    ) -> Result<()> {
        loop {
            if lifecycle.is_draining() {
                return Ok(());
            }
            let started = std::time::Instant::now();
            for event in self.claim_admission().await? {
                if let Err(error) = self.project_admission(&event).await {
                    let delay = retry_delay_seconds(event.fencing_token);
                    let message = sanitize_error(&error.to_string());
                    let changed = sqlx::query("UPDATE outbox SET status='failed',available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),locked_by=NULL,locked_until=NULL,last_error=? WHERE id=? AND locked_by=? AND fencing_token=? AND status='processing' AND locked_until>UTC_TIMESTAMP(6)")
                        .bind(delay).bind(message).bind(event.id).bind(self.owner.0).bind(event.fencing_token).execute(&self.pool).await?;
                    anyhow::ensure!(
                        changed.rows_affected() == 1,
                        "Admission Outbox Lease was lost"
                    );
                    if event.event_type == "ApplicationChatMappingChanged" {
                        sqlx::query("UPDATE application_playground_configs SET publish_status='failed',last_error_code='PLAYGROUND_MAPPING_PUBLISH_FAILED',last_error_message=? WHERE tenant_id=? AND deployment_id=? AND version=?")
                            .bind(sanitize_error(&error.to_string()))
                            .bind(event.tenant_id)
                            .bind(event.aggregate_id)
                            .bind(event.payload.get("version").and_then(Value::as_u64).unwrap_or_default())
                            .execute(&self.pool)
                            .await?;
                    }
                }
            }
            progress.processed_since(started).await;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn claim_admission(&self) -> Result<Vec<AdmissionEvent>> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query("SELECT id FROM outbox WHERE aggregate_type IN ('application_admission','runtime_user_admission','workflow_admission','application_chat_mapping') AND status IN ('pending','failed') AND available_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY occurred_at,id LIMIT ? FOR UPDATE SKIP LOCKED")
            .bind(DEFAULT_BATCH_SIZE).fetch_all(&mut *tx).await?;
        for row in rows {
            sqlx::query("UPDATE outbox SET status='processing',locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1,attempt_count=attempt_count+1 WHERE id=? AND status IN ('pending','failed')")
                .bind(self.owner.0).bind(row.try_get::<Uuid,_>("id")?).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        let rows = sqlx::query("SELECT id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,occurred_at,fencing_token FROM outbox WHERE aggregate_type IN ('application_admission','runtime_user_admission','workflow_admission','application_chat_mapping') AND status='processing' AND locked_by=? AND locked_until>UTC_TIMESTAMP(6) ORDER BY occurred_at,id")
            .bind(self.owner.0).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok(AdmissionEvent {
                    id: row.try_get("id")?,
                    tenant_id: row.try_get("tenant_id")?,
                    event_type: row.try_get("event_type")?,
                    aggregate_type: row.try_get("aggregate_type")?,
                    aggregate_id: Uuid::parse_str(&row.try_get::<String, _>("aggregate_id")?)?,
                    payload: row.try_get("payload_json")?,
                    occurred_at: row.try_get("occurred_at")?,
                    fencing_token: row.try_get("fencing_token")?,
                })
            })
            .collect()
    }

    async fn project_admission(&self, event: &AdmissionEvent) -> Result<()> {
        if event.event_type == "ApplicationChatMappingChanged" {
            let mapping = event
                .payload
                .get("mapping")
                .cloned()
                .map(serde_json::from_value::<Option<ChatMappingV1>>)
                .transpose()?
                .flatten();
            let request = ApplyChatMappingRequestV1 {
                api_version: 1,
                idempotency_key: format!("{}:chat-mapping", event.id),
                tenant_id: event.tenant_id,
                application_id: payload_uuid(&event.payload, "applicationId")?,
                deployment_id: payload_uuid(&event.payload, "deploymentId")?,
                bundle_id: payload_uuid(&event.payload, "bundleId")?,
                version: event
                    .payload
                    .get("version")
                    .and_then(Value::as_u64)
                    .context("Chat Mapping version")?,
                mapping,
                content_hash: ContentHash::parse(
                    event
                        .payload
                        .get("contentHash")
                        .and_then(Value::as_str)
                        .context("Chat Mapping contentHash")?,
                )?,
            };
            let receipt: ApplyChatMappingReceiptV1 = self
                .post(
                    "runtime.chat_mappings.apply",
                    "/internal/runtime/v1/chat-mappings:apply",
                    &request,
                )
                .await?;
            anyhow::ensure!(
                receipt.version == request.version,
                "Runtime applied an unexpected Chat Mapping Version"
            );
            let mut tx = self.pool.begin().await?;
            sqlx::query("UPDATE application_playground_configs SET published_version=?,publish_status='active',last_error_code=NULL,last_error_message=NULL WHERE tenant_id=? AND deployment_id=? AND version=?")
                .bind(request.version).bind(event.tenant_id).bind(request.deployment_id).bind(request.version).execute(&mut *tx).await?;
            let changed = sqlx::query("UPDATE outbox SET status='published',published_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL,last_error=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND status='processing' AND locked_until>UTC_TIMESTAMP(6)")
                .bind(event.id).bind(self.owner.0).bind(event.fencing_token).execute(&mut *tx).await?;
            anyhow::ensure!(
                changed.rows_affected() == 1,
                "Admission Outbox Lease was lost"
            );
            tx.commit().await?;
            return Ok(());
        }
        let epoch = event
            .payload
            .get("admissionEpoch")
            .and_then(Value::as_u64)
            .context("Admission Outbox admissionEpoch")?;
        if event.event_type == "ApplicationAdmissionChanged" {
            let request = DisableDeploymentRequestV1 {
                api_version: 1,
                idempotency_key: format!("{}:disable", event.id),
                tenant_id: event.tenant_id,
                application_id: event.aggregate_id,
                admission_epoch: epoch,
            };
            let receipt: agentx_runtime_contracts::PublishReceiptV1 = self
                .post(
                    "runtime.deployments.disable",
                    "/internal/runtime/v1/deployments:disable",
                    &request,
                )
                .await?;
            ensure_publish_receipt(&receipt)?;
        } else if matches!(
            event.event_type.as_str(),
            "RuntimeUserAdmissionChanged"
                | "RuntimeUserApplicationGrantChanged"
                | "RuntimeUserWorkflowGrantChanged"
                | "ServiceIdentityAdmissionChanged"
                | "ResourceGrantAdmissionChanged"
        ) {
            let target = match event.event_type.as_str() {
                "RuntimeUserAdmissionChanged"
                | "RuntimeUserApplicationGrantChanged"
                | "RuntimeUserWorkflowGrantChanged" => {
                    let user_id = event
                        .payload
                        .get("userId")
                        .and_then(Value::as_str)
                        .and_then(|v| Uuid::parse_str(v).ok())
                        .context("Admission Outbox userId")?;
                    if event.event_type == "RuntimeUserAdmissionChanged" {
                        let user = sqlx::query("SELECT u.display_name,ud.department_id,d.name department_name FROM users u JOIN user_departments ud ON ud.tenant_id=u.tenant_id AND ud.user_id=u.id JOIN departments d ON d.tenant_id=ud.tenant_id AND d.id=ud.department_id WHERE u.tenant_id=? AND u.id=?")
                            .bind(event.tenant_id).bind(user_id).fetch_one(&self.pool).await?;
                        AdmissionTargetV1::RuntimeUser {
                            state: agentx_runtime_contracts::RuntimeUserAdmissionV1 {
                                tenant_id: event.tenant_id,
                                user_id,
                                user_name: user.try_get("display_name")?,
                                department_id: user.try_get("department_id")?,
                                department_name: user.try_get("department_name")?,
                                token_version: event
                                    .payload
                                    .get("tokenVersion")
                                    .and_then(Value::as_u64)
                                    .context("Admission tokenVersion")?,
                                enabled: event
                                    .payload
                                    .get("enabled")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(true),
                                tenant_query_enabled: event
                                    .payload
                                    .get("tenantQueryEnabled")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                                role_assignments: runtime_grants::user_role_assignments(
                                    &self.pool,
                                    event.tenant_id,
                                    user_id,
                                )
                                .await?,
                            },
                        }
                    } else if event.event_type == "RuntimeUserApplicationGrantChanged" {
                        AdmissionTargetV1::RuntimeUserApplicationGrant {
                            state: agentx_runtime_contracts::RuntimeUserApplicationGrantV1 {
                                tenant_id: event.tenant_id,
                                user_id,
                                application_id: event.aggregate_id,
                                grant_version: event
                                    .payload
                                    .get("grantVersion")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(epoch),
                                can_invoke: event
                                    .payload
                                    .get("canInvoke")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(true),
                                can_query: event
                                    .payload
                                    .get("canQuery")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                            },
                        }
                    } else {
                        AdmissionTargetV1::RuntimeUserWorkflowGrant {
                            state: agentx_runtime_contracts::RuntimeUserWorkflowGrantV1 {
                                tenant_id: event.tenant_id,
                                user_id,
                                workflow_id: event.aggregate_id,
                                grant_version: event
                                    .payload
                                    .get("grantVersion")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(epoch),
                                can_query: event
                                    .payload
                                    .get("canQuery")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                            },
                        }
                    }
                }
                "ServiceIdentityAdmissionChanged" => {
                    let policy_epoch = event
                        .payload
                        .get("policyEpoch")
                        .and_then(Value::as_u64)
                        .context("Service Identity policyEpoch")?;
                    anyhow::ensure!(policy_epoch == epoch, "Service Identity Epoch mismatch");
                    let status = match event.payload.get("status").and_then(Value::as_str) {
                        Some("active") => AdmissionStatusV1::Active,
                        Some("disabled") => AdmissionStatusV1::Disabled,
                        Some("revoked") => AdmissionStatusV1::Revoked,
                        _ => anyhow::bail!("Service Identity status is invalid"),
                    };
                    AdmissionTargetV1::ServiceIdentity {
                        state: ServiceIdentityAdmissionV1 {
                            tenant_id: event.tenant_id,
                            workflow_id: payload_uuid(&event.payload, "workflowId")?,
                            identity_id: payload_uuid(&event.payload, "identityId")?,
                            policy_epoch,
                            status,
                            capabilities: payload_strings(&event.payload, "capabilities")?,
                            grant_ids: payload_uuids(&event.payload, "grantIds")?,
                        },
                    }
                }
                "ResourceGrantAdmissionChanged" => {
                    let policy_epoch = event
                        .payload
                        .get("policyEpoch")
                        .and_then(Value::as_u64)
                        .context("Resource Grant policyEpoch")?;
                    anyhow::ensure!(policy_epoch == epoch, "Resource Grant Epoch mismatch");
                    AdmissionTargetV1::ResourceGrant {
                        state: RuntimeGrantStateV1 {
                            tenant_id: event.tenant_id,
                            identity_id: payload_uuid(&event.payload, "identityId")?,
                            grant_id: payload_uuid(&event.payload, "grantId")?,
                            resource_type: event
                                .payload
                                .get("resourceType")
                                .and_then(Value::as_str)
                                .context("Resource Grant resourceType")?
                                .to_owned(),
                            resource_id: payload_uuid(&event.payload, "resourceId")?,
                            operations: payload_strings(&event.payload, "operations")?
                                .into_iter()
                                .collect(),
                            policy_epoch,
                            enabled: event
                                .payload
                                .get("enabled")
                                .and_then(Value::as_bool)
                                .context("Resource Grant enabled")?,
                        },
                    }
                }
                _ => unreachable!(),
            };
            let command = RuntimeAdmissionCommandV1 {
                api_version: 1,
                command: CommandEnvelopeV1 {
                    schema_version: 1,
                    event_id: event.id,
                    source_plane: Plane::Control,
                    tenant_id: event.tenant_id,
                    aggregate_type: event.aggregate_type.clone(),
                    aggregate_id: event.aggregate_id.to_string(),
                    object_version: epoch,
                    occurred_at: event.occurred_at,
                    payload: json!({}),
                    content_hash: agentx_runtime_contracts::content_hash(&target)?,
                    correlation_id: event.id,
                    causation_id: None,
                    idempotency_key: format!("{}:admission", event.id),
                },
                admission_epoch: epoch,
                target,
            };
            let receipt: agentx_runtime_contracts::ApplyReceiptV1 = self
                .post(
                    "runtime.admission.apply",
                    "/internal/runtime/v1/admission-commands:apply",
                    &command,
                )
                .await?;
            anyhow::ensure!(receipt.applied, "Runtime rejected Admission event");
        } else if event.event_type == "ApiKeyAdmissionChanged" {
            let key_id = event
                .payload
                .get("keyId")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .context("Admission Outbox keyId")?;
            let status = event
                .payload
                .get("status")
                .and_then(Value::as_str)
                .context("Admission Outbox status")?;
            let row = sqlx::query("SELECT name,key_prefix,secret_hash FROM application_api_keys WHERE tenant_id=? AND application_id=? AND id=?")
                .bind(event.tenant_id).bind(event.aggregate_id).bind(key_id).fetch_optional(&self.pool).await?;
            let (key_name, key_prefix, secret_hash) = if let Some(row) = row {
                (
                    row.try_get("name")?,
                    row.try_get("key_prefix")?,
                    format!("sha256:{}", hex(row.try_get::<Vec<u8>, _>("secret_hash")?)),
                )
            } else {
                (
                    event
                        .payload
                        .get("keyName")
                        .and_then(Value::as_str)
                        .unwrap_or("Revoked API Key")
                        .to_owned(),
                    event
                        .payload
                        .get("keyPrefix")
                        .and_then(Value::as_str)
                        .unwrap_or("revoked")
                        .to_owned(),
                    event
                        .payload
                        .get("secretHash")
                        .and_then(Value::as_str)
                        .unwrap_or(&format!("sha256:{}", "0".repeat(64)))
                        .to_owned(),
                )
            };
            let target = AdmissionTargetV1::ApiKey {
                state: ApiKeyAdmissionV1 {
                    tenant_id: event.tenant_id,
                    application_id: event.aggregate_id,
                    key_id,
                    key_prefix,
                    key_name,
                    secret_hash,
                    status: if status == "active" {
                        AdmissionStatusV1::Active
                    } else {
                        AdmissionStatusV1::Revoked
                    },
                    expires_at: None,
                },
            };
            let command = RuntimeAdmissionCommandV1 {
                api_version: 1,
                command: CommandEnvelopeV1 {
                    schema_version: 1,
                    event_id: event.id,
                    source_plane: Plane::Control,
                    tenant_id: event.tenant_id,
                    aggregate_type: "application_admission".into(),
                    aggregate_id: event.aggregate_id.to_string(),
                    object_version: epoch,
                    occurred_at: event.occurred_at,
                    payload: json!({}),
                    content_hash: agentx_runtime_contracts::content_hash(&target)?,
                    correlation_id: event.id,
                    causation_id: None,
                    idempotency_key: format!("{}:admission", event.id),
                },
                admission_epoch: epoch,
                target,
            };
            let receipt: agentx_runtime_contracts::ApplyReceiptV1 = self
                .post(
                    "runtime.admission.apply",
                    "/internal/runtime/v1/admission-commands:apply",
                    &command,
                )
                .await?;
            anyhow::ensure!(receipt.applied, "Runtime rejected Admission Outbox event");
        } else {
            anyhow::bail!("unsupported Admission Outbox event {}", event.event_type);
        }
        let changed = sqlx::query("UPDATE outbox SET status='published',published_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL,last_error=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND status='processing' AND locked_until>UTC_TIMESTAMP(6)")
            .bind(event.id).bind(self.owner.0).bind(event.fencing_token).execute(&self.pool).await?;
        anyhow::ensure!(
            changed.rows_affected() == 1,
            "Admission Outbox Lease was lost"
        );
        Ok(())
    }

    async fn with_heartbeat<T, F>(&self, attempt: &Attempt, operation: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        tokio::pin!(operation);
        let start = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut heartbeat = tokio::time::interval_at(start, Duration::from_secs(10));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                result = &mut operation => return result,
                _ = heartbeat.tick() => {
                    let changed = sqlx::query("UPDATE publish_attempts SET locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND) WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
                        .bind(attempt.id).bind(self.owner.0).bind(attempt.fencing_token)
                        .execute(&self.pool).await?;
                    anyhow::ensure!(changed.rows_affected() == 1, "Publish Attempt Lease was lost");
                }
            }
        }
    }

    async fn record_failure(&self, attempt: &Attempt, error: &anyhow::Error) -> Result<()> {
        let runtime = error.downcast_ref::<RuntimeCallError>();
        let terminal = runtime.is_some_and(RuntimeCallError::terminal)
            || (attempt.next_action == "build" && runtime.is_none());
        let code = runtime
            .map(RuntimeCallError::code)
            .unwrap_or_else(|| "PUBLISH_STEP_FAILED".to_owned());
        let message = sanitize_error(
            runtime
                .map(RuntimeCallError::message)
                .as_deref()
                .unwrap_or("Publish step failed"),
        );
        let details = json!({
            "action": attempt.next_action,
            "retryable": !terminal,
        });
        let result = if terminal {
            sqlx::query("UPDATE publish_attempts SET state='rejected',next_action='none',last_error_code=?,last_error_message=?,last_error_json=?,locked_by=NULL,locked_until=NULL,completed_at=UTC_TIMESTAMP(6),updated_at=UTC_TIMESTAMP(6) WHERE id=? AND state=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
                .bind(&code).bind(&message).bind(details).bind(attempt.id).bind(&attempt.state)
                .bind(self.owner.0).bind(attempt.fencing_token).execute(&self.pool).await?
        } else {
            let delay = retry_delay_seconds(attempt.fencing_token);
            sqlx::query("UPDATE publish_attempts SET available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),last_error_code=?,last_error_message=?,last_error_json=?,locked_by=NULL,locked_until=NULL,updated_at=UTC_TIMESTAMP(6) WHERE id=? AND state=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
                .bind(delay).bind(&code).bind(&message).bind(details).bind(attempt.id).bind(&attempt.state)
                .bind(self.owner.0).bind(attempt.fencing_token).execute(&self.pool).await?
        };
        anyhow::ensure!(
            result.rows_affected() == 1,
            "Publish Attempt Lease was lost while recording failure"
        );
        if terminal {
            sqlx::query("UPDATE application_deployments SET status='rejected' WHERE id=? AND tenant_id=? AND status<>'active'")
                .bind(attempt.deployment_id).bind(attempt.tenant_id).execute(&self.pool).await?;
            sqlx::query("UPDATE outbox SET status='published',published_at=UTC_TIMESTAMP(6),last_error=? WHERE tenant_id=? AND aggregate_type='bundle_publish' AND aggregate_id=? AND status IN ('pending','failed')")
                .bind(&message).bind(attempt.tenant_id).bind(attempt.id.to_string()).execute(&self.pool).await?;
        }
        Ok(())
    }

    async fn build(&self, a: &Attempt) -> Result<()> {
        let row=sqlx::query("SELECT d.workflow_version_id,d.sequence_number,d.input_schema_json,d.output_schema_json,d.session_version_policy,d.trigger_revision,a.workflow_id,a.slug,w.name workflow_name,w.owner_department_id,od.name owner_department_name,wv.version_number,wv.definition_json,wsi.id identity_id,wsi.version identity_version FROM application_deployments d JOIN applications a ON a.id=d.application_id AND a.tenant_id=d.tenant_id JOIN workflows w ON w.id=a.workflow_id AND w.tenant_id=a.tenant_id LEFT JOIN departments od ON od.id=w.owner_department_id AND od.tenant_id=w.tenant_id JOIN workflow_versions wv ON wv.id=d.workflow_version_id AND wv.tenant_id=d.tenant_id JOIN workflow_service_identities wsi ON wsi.workflow_id=a.workflow_id AND wsi.tenant_id=a.tenant_id WHERE d.id=? AND d.tenant_id=?").bind(a.deployment_id).bind(a.tenant_id).fetch_one(&self.pool).await?;
        let definition: WorkflowDefinition =
            serde_json::from_value(row.try_get("definition_json")?)?;
        let workflow_version_id: Uuid = row.try_get("workflow_version_id")?;
        let workflow_id: Uuid = row.try_get("workflow_id")?;
        let object_bytes = agentx_runtime_contracts::canonical_bytes(&definition)?;
        let object_hash = agentx_runtime_contracts::content_hash(&definition)?;
        let object_id = workflow_version_id;
        let object_source_key = format!(
            "control/{}/{}/{}",
            a.tenant_id,
            object_id,
            object_hash
                .as_str()
                .strip_prefix("sha256:")
                .expect("ContentHash carries the sha256 prefix")
        );
        self.control_objects
            .put(
                &ObjectPath::from(object_source_key.clone()),
                Bytes::from(object_bytes.clone()).into(),
            )
            .await?;
        let object = agentx_runtime_contracts::RuntimeObjectReferenceV1 {
            tenant_id: a.tenant_id,
            storage_domain: agentx_runtime_contracts::StorageDomain::Runtime,
            object_id,
            object_key: agentx_runtime_contracts::RuntimeObjectReferenceV1::canonical_key(
                a.tenant_id,
                object_id,
                &object_hash,
            ),
            content_hash: object_hash.clone(),
            size_bytes: object_bytes.len() as u64,
            media_type: "application/vnd.agentx.workflow-definition+json".into(),
        };
        let dependencies = load_dependencies(&self.pool, a.tenant_id, &definition).await?;
        let plugin_manifests =
            canvas_plugin_api::plugin_manifests_for_pool(&self.pool, a.tenant_id, false).await?;
        let resolved = canvas_plugin_resolution::resolve_for_publisher(
            self.pool.clone(),
            self.control_objects.clone(),
            a.tenant_id,
            &definition,
            &dependencies,
            &plugin_manifests,
        )
        .await?;
        anyhow::ensure!(
            resolved.incomplete.is_empty() && resolved.invalid.is_empty(),
            "Workflow contains an incomplete or invalid plugin definition"
        );
        let mut bundle_objects = vec![(object, object_source_key)];
        for binding in canvas_plugin_resolution::resolved_runtime_plugin_bindings(&resolved) {
            let artifact = binding
                .runtime_artifact
                .as_ref()
                .context("Plugin binding has no immutable Runtime artifact")?;
            if bundle_objects
                .iter()
                .any(|(object, _)| object.object_id == artifact.object_id)
            {
                continue;
            }
            let bytes = binding.runtime_source.as_bytes().to_vec();
            anyhow::ensure!(
                bytes.len() as u64 == artifact.size_bytes
                    && format!("sha256:{:x}", Sha256::digest(&bytes)) == artifact.content_hash
                    && artifact.media_type == "text/javascript",
                "Plugin Runtime source no longer matches its immutable artifact reference"
            );
            let content_hash = ContentHash::parse(artifact.content_hash.clone())?;
            let source_key = format!(
                "control/{}/{}/{}",
                a.tenant_id,
                artifact.object_id,
                content_hash
                    .as_str()
                    .strip_prefix("sha256:")
                    .expect("ContentHash carries the sha256 prefix")
            );
            self.control_objects
                .put(
                    &ObjectPath::from(source_key.clone()),
                    Bytes::from(bytes).into(),
                )
                .await?;
            bundle_objects.push((
                agentx_runtime_contracts::RuntimeObjectReferenceV1 {
                    tenant_id: a.tenant_id,
                    storage_domain: agentx_runtime_contracts::StorageDomain::Runtime,
                    object_id: artifact.object_id,
                    object_key: agentx_runtime_contracts::RuntimeObjectReferenceV1::canonical_key(
                        a.tenant_id,
                        artifact.object_id,
                        &content_hash,
                    ),
                    content_hash,
                    size_bytes: artifact.size_bytes,
                    media_type: artifact.media_type.clone(),
                },
                source_key,
            ));
        }
        for (dependency_id, dependency) in &dependencies {
            let bytes = agentx_runtime_contracts::canonical_bytes(dependency)?;
            let hash = agentx_runtime_contracts::content_hash(dependency)?;
            let source_key = format!(
                "control/{}/{}/{}",
                a.tenant_id,
                dependency_id,
                hash.as_str()
                    .strip_prefix("sha256:")
                    .expect("ContentHash carries the sha256 prefix")
            );
            self.control_objects
                .put(
                    &ObjectPath::from(source_key.clone()),
                    Bytes::from(bytes.clone()).into(),
                )
                .await?;
            bundle_objects.push((
                agentx_runtime_contracts::RuntimeObjectReferenceV1 {
                    tenant_id: a.tenant_id,
                    storage_domain: agentx_runtime_contracts::StorageDomain::Runtime,
                    object_id: *dependency_id,
                    object_key: agentx_runtime_contracts::RuntimeObjectReferenceV1::canonical_key(
                        a.tenant_id,
                        *dependency_id,
                        &hash,
                    ),
                    content_hash: hash,
                    size_bytes: bytes.len() as u64,
                    media_type: "application/vnd.agentx.workflow-definition+json".into(),
                },
                source_key,
            ));
            let dependency_manifests = resolved
                .dependency_manifests
                .get(dependency_id)
                .cloned()
                .unwrap_or_default();
            let compiled = compile_workflow_version_with_resolved_plugins(
                dependency,
                *dependency_id,
                &dependencies,
                &plugin_manifests,
                &dependency_manifests,
            )?;
            let ir_bytes = agentx_runtime_contracts::canonical_bytes(&compiled)?;
            let ir_hash = agentx_runtime_contracts::content_hash(&compiled)?;
            let ir_object_id = composite_ir_object_id(*dependency_id);
            let ir_source_key = format!(
                "control/{}/{}/{}",
                a.tenant_id,
                ir_object_id,
                ir_hash
                    .as_str()
                    .strip_prefix("sha256:")
                    .expect("ContentHash carries the sha256 prefix")
            );
            self.control_objects
                .put(
                    &ObjectPath::from(ir_source_key.clone()),
                    Bytes::from(ir_bytes.clone()).into(),
                )
                .await?;
            bundle_objects.push((
                agentx_runtime_contracts::RuntimeObjectReferenceV1 {
                    tenant_id: a.tenant_id,
                    storage_domain: agentx_runtime_contracts::StorageDomain::Runtime,
                    object_id: ir_object_id,
                    object_key: agentx_runtime_contracts::RuntimeObjectReferenceV1::canonical_key(
                        a.tenant_id,
                        ir_object_id,
                        &ir_hash,
                    ),
                    content_hash: ir_hash,
                    size_bytes: ir_bytes.len() as u64,
                    media_type: "application/vnd.agentx.compiled-workflow.v1+json".into(),
                },
                ir_source_key,
            ));
        }
        let resource_rows = sqlx::query("SELECT resource_type,resource_id,resource_version_id,snapshot_json,snapshot_hash FROM workflow_version_resources WHERE tenant_id=? AND workflow_version_id=? ORDER BY resource_type,resource_id,node_id")
            .bind(a.tenant_id)
            .bind(workflow_version_id)
            .fetch_all(&self.pool)
            .await?;
        let mut resources: Vec<RuntimeResourceBindingV1> = Vec::new();
        for resource_row in &resource_rows {
            let binding = runtime_resource_binding::from_row(resource_row)?;
            if let Some(existing) = resources.iter().find(|existing| {
                existing.resource_kind == binding.resource_kind
                    && existing.resource_id == binding.resource_id
                    && existing.resource_version == binding.resource_version
            }) {
                anyhow::ensure!(
                    existing.content_hash == binding.content_hash,
                    "Workflow Version contains conflicting snapshots for Runtime Resource {}",
                    binding.resource_id
                );
            } else {
                resources.push(binding);
            }
        }
        for row in &resource_rows {
            let snapshot: Value = row.try_get("snapshot_json")?;
            let Some(objects) = snapshot.get("runtimeObjects").and_then(Value::as_array) else {
                continue;
            };
            for object in objects {
                let object_id = runtime_resource_binding::json_uuid(object, "objectId")
                    .context("Runtime resource object requires objectId")?;
                if bundle_objects
                    .iter()
                    .any(|(existing, _)| existing.object_id == object_id)
                {
                    continue;
                }
                let content_hash = ContentHash::parse(
                    runtime_resource_binding::required_json_string(object, "contentHash")?,
                )?;
                bundle_objects.push((
                    agentx_runtime_contracts::RuntimeObjectReferenceV1 {
                        tenant_id: a.tenant_id,
                        storage_domain: agentx_runtime_contracts::StorageDomain::Runtime,
                        object_id,
                        object_key:
                            agentx_runtime_contracts::RuntimeObjectReferenceV1::canonical_key(
                                a.tenant_id,
                                object_id,
                                &content_hash,
                            ),
                        content_hash,
                        size_bytes: runtime_resource_binding::required_json_u64(
                            object,
                            "sizeBytes",
                        )?,
                        media_type: runtime_resource_binding::required_json_string(
                            object,
                            "mediaType",
                        )?,
                    },
                    runtime_resource_binding::required_json_string(object, "sourceKey")?,
                ));
            }
        }
        for dependency_id in dependencies.keys().copied() {
            if resources.iter().any(|binding| {
                matches!(
                    &binding.configuration,
                    RuntimeResourceConfigurationV1::Composite {
                        workflow,
                        ..
                    } if workflow.version_id == dependency_id
                )
            }) {
                continue;
            }
            let configuration = RuntimeResourceConfigurationV1::Composite {
                workflow: runtime_resource_binding::workflow_snapshot(
                    &self.pool,
                    a.tenant_id,
                    dependency_id,
                )
                .await?,
                definition_object_id: dependency_id,
                ir_object_id: composite_ir_object_id(dependency_id),
            };
            resources.push(RuntimeResourceBindingV1 {
                resource_kind: RuntimeResourceKindV1::Composite,
                resource_id: dependency_id,
                resource_version: dependency_id.to_string(),
                state_epoch: 1,
                content_hash: agentx_runtime_contracts::content_hash(&configuration)?,
                configuration,
                object_ids: vec![dependency_id, composite_ir_object_id(dependency_id)],
            });
        }
        let (grants, grant_bindings) = runtime_resource_binding::authorization_grants(
            &self.pool,
            a.tenant_id,
            row.try_get::<Uuid, _>("identity_id")?,
        )
        .await?;
        let trigger_revision: u64 = row.try_get("trigger_revision")?;
        let trigger_rows = sqlx::query("SELECT manifest_json FROM application_runtime_trigger_revisions WHERE tenant_id=? AND application_id=? AND revision=?")
            .bind(a.tenant_id).bind(a.application_id).bind(trigger_revision).fetch_optional(&self.pool).await?;
        let triggers: Vec<agentx_runtime_contracts::RuntimeTriggerSpecV1> =
            if let Some(trigger) = trigger_rows {
                serde_json::from_value(trigger.try_get("manifest_json")?)?
            } else {
                vec![]
            };
        let required_capabilities = compile_workflow_version_with_resolved_plugins(
            &definition,
            workflow_version_id,
            &dependencies,
            &plugin_manifests,
            &resolved.manifests,
        )?
        .nodes
        .iter()
        .map(|node| node.capability.as_str().to_owned())
        .collect::<BTreeSet<_>>();
        let bundle_id = a.bundle_id.unwrap_or_else(Uuid::now_v7);
        let bundle = build_bundle_with_resolved(
            BundleBuildSource {
                bundle_id,
                tenant_id: a.tenant_id,
                application_id: a.application_id,
                deployment_id: a.deployment_id,
                workflow_id,
                workflow_version_id,
                workflow_name: row.try_get("workflow_name")?,
                workflow_version_number: row.try_get("version_number")?,
                workflow_owner_department: row
                    .try_get::<Option<Uuid>, _>("owner_department_id")?
                    .map(
                        |id| agentx_runtime_contracts::ExecutionDepartmentSnapshotV1 {
                            id,
                            name: row.try_get("owner_department_name").unwrap_or_default(),
                        },
                    ),
                sequence: row.try_get("sequence_number")?,
                definition,
                dependency_versions: dependencies,
                plugin_manifests,
                supported_capabilities: agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
                input_contract: row.try_get("input_schema_json")?,
                output_contract: row.try_get("output_schema_json")?,
                resources,
                authorization: RuntimeAuthorizationSnapshotV1 {
                    schema_version: 1,
                    tenant_id: a.tenant_id,
                    service_identity_id: row.try_get("identity_id")?,
                    workflow_id,
                    policy_epoch: row.try_get("identity_version")?,
                    capabilities: required_capabilities,
                    grant_ids: grants,
                    grant_bindings,
                    maximum_policy_staleness_seconds: 72 * 60 * 60,
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
                triggers,
                runtime_policy: RuntimePolicyV1::default(),
                objects: bundle_objects
                    .iter()
                    .map(|(object, _)| object.clone())
                    .collect(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
            &resolved.manifests,
            &resolved.dependency_manifests,
            &self.bundle_kid,
            &self.bundle_key,
        )?;
        let signature = STANDARD.decode(&bundle.signature.signature_base64)?;
        let payload = serde_json::to_value(&bundle.payload)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO execution_spec_bundles(id,tenant_id,application_id,deployment_id,workflow_id,workflow_version_id,sequence_number,schema_version,compiler_version,content_hash,signature_key_id,signature,payload_json,object_manifest_json,status,built_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,'built',UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE id=id").bind(bundle_id).bind(a.tenant_id).bind(a.application_id).bind(a.deployment_id).bind(workflow_id).bind(workflow_version_id).bind(bundle.payload.bundle_sequence).bind(1_u32).bind(agentx_runtime::COMPILER_VERSION).bind(bundle.content_hash.as_str()).bind(&self.bundle_kid).bind(signature).bind(payload).bind(serde_json::to_value(&bundle.payload.objects)?).execute(&mut *tx).await?;
        for (object, source_key) in bundle_objects {
            sqlx::query("INSERT INTO bundle_object_copies(id,tenant_id,bundle_id,object_id,source_key,content_hash,size_bytes,media_type,idempotency_key,status) VALUES(?,?,?,?,?,?,?,?,?,'pending') ON DUPLICATE KEY UPDATE source_key=VALUES(source_key)")
                .bind(Uuid::now_v7()).bind(a.tenant_id).bind(bundle_id).bind(object.object_id)
                .bind(source_key).bind(object.content_hash.as_str()).bind(object.size_bytes)
                .bind(&object.media_type).bind(runtime_object_idempotency_key(&object))
                .execute(&mut *tx).await?;
        }
        self.transition(
            &mut tx,
            a,
            "building",
            "copying",
            "copy_objects",
            Some(bundle_id),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn copy_objects(&self, a: &Attempt) -> Result<()> {
        let rows=sqlx::query("SELECT id,source_key,object_id,content_hash,size_bytes,media_type,idempotency_key FROM bundle_object_copies WHERE tenant_id=? AND bundle_id=? AND status<>'copied' ORDER BY id").bind(a.tenant_id).bind(a.bundle_id).fetch_all(&self.pool).await?;
        for row in rows {
            let source: String = row.try_get("source_key")?;
            let content = self.control_objects.get(&ObjectPath::from(source)).await?;
            let metadata = agentx_runtime_contracts::RuntimeObjectUploadMetadataV1 {
                api_version: 1,
                idempotency_key: row.try_get("idempotency_key")?,
                tenant_id: a.tenant_id,
                object_id: row.try_get("object_id")?,
                content_hash: ContentHash::parse(row.try_get::<String, _>("content_hash")?)?,
                size_bytes: row.try_get("size_bytes")?,
                media_type: row.try_get("media_type")?,
            };
            let response = self
                .http
                .post(format!(
                    "{}/internal/runtime/v1/objects:upload",
                    self.runtime_url
                ))
                .bearer_auth(self.token("runtime.objects.write")?)
                .multipart(
                    reqwest::multipart::Form::new()
                        .part(
                            "metadata",
                            reqwest::multipart::Part::text(serde_json::to_string(&metadata)?)
                                .mime_str("application/json")?,
                        )
                        .part(
                            "content",
                            reqwest::multipart::Part::stream_with_length(
                                reqwest::Body::wrap_stream(content.into_stream()),
                                metadata.size_bytes,
                            )
                            .mime_str(&metadata.media_type)?,
                        ),
                )
                .send()
                .await
                .map_err(RuntimeCallError::transport)?;
            let response = decode_runtime_response(response).await?;
            let receipt: agentx_runtime_contracts::RuntimeObjectUploadReceiptV1 =
                response.json().await?;
            sqlx::query("UPDATE bundle_object_copies SET status='copied',runtime_object_key=?,receipt_json=?,copied_at=UTC_TIMESTAMP(6) WHERE id=?").bind(&receipt.object.object_key).bind(serde_json::to_value(&receipt)?).bind(row.try_get::<Uuid,_>("id")?).execute(&self.pool).await?;
        }
        let mut tx = self.pool.begin().await?;
        self.transition(
            &mut tx,
            a,
            "copying",
            "preparing",
            "apply_admission",
            a.bundle_id,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn apply_admission(&self, a: &Attempt) -> Result<()> {
        let row=sqlx::query("SELECT app.slug,b.payload_json FROM applications app JOIN execution_spec_bundles b ON b.application_id=app.id AND b.tenant_id=app.tenant_id WHERE b.id=?").bind(a.bundle_id).fetch_one(&self.pool).await?;
        let payload: agentx_runtime_contracts::ExecutionSpecPayloadV2 =
            serde_json::from_value(row.try_get("payload_json")?)?;
        let identity_id = payload.authorization.service_identity_id;
        let workflow_id = payload.workflow_id;
        let required_grants = payload.authorization.grant_ids.clone();
        let required_grant_ids = required_grants.iter().copied().collect::<BTreeSet<_>>();
        let grant_rows = sqlx::query("SELECT id,resource_type,resource_id,operation_key FROM resource_grants WHERE tenant_id=? AND subject_type='workflow_service_identity' AND subject_id=? ORDER BY id")
            .bind(a.tenant_id)
            .bind(identity_id)
            .fetch_all(&self.pool)
            .await?;
        let mut grant_targets = Vec::new();
        let mut found_grants = BTreeSet::new();
        for grant in grant_rows {
            let grant_id: Uuid = grant.try_get("id")?;
            if !required_grant_ids.contains(&grant_id) {
                continue;
            }
            found_grants.insert(grant_id);
            grant_targets.push(AdmissionTargetV1::ResourceGrant {
                state: RuntimeGrantStateV1 {
                    tenant_id: a.tenant_id,
                    identity_id,
                    grant_id,
                    resource_type: grant.try_get("resource_type")?,
                    resource_id: grant.try_get("resource_id")?,
                    operations: BTreeSet::from([grant.try_get("operation_key")?]),
                    policy_epoch: a.minimum_admission_epoch,
                    enabled: true,
                },
            });
        }
        anyhow::ensure!(
            found_grants == required_grant_ids,
            "Bundle authorization references a missing Control Resource Grant"
        );
        let key=sqlx::query("SELECT id,name,key_prefix,secret_hash FROM application_api_keys WHERE tenant_id=? AND application_id=? AND status='active' ORDER BY created_at DESC LIMIT 1").bind(a.tenant_id).bind(a.application_id).fetch_optional(&self.pool).await?;
        let mut commands = vec![
            AdmissionTargetV1::Tenant { enabled: true },
            AdmissionTargetV1::ApplicationRoute {
                state: ApplicationRouteAdmissionV1 {
                    tenant_id: a.tenant_id,
                    application_id: a.application_id,
                    route_key: row.try_get("slug")?,
                    status: AdmissionStatusV1::Active,
                },
            },
        ];
        if let Some(key) = key {
            commands.push(AdmissionTargetV1::ApiKey {
                state: ApiKeyAdmissionV1 {
                    tenant_id: a.tenant_id,
                    application_id: a.application_id,
                    key_id: key.try_get("id")?,
                    key_prefix: key.try_get("key_prefix")?,
                    key_name: key.try_get("name")?,
                    secret_hash: format!(
                        "sha256:{}",
                        hex(key.try_get::<Vec<u8>, _>("secret_hash")?)
                    ),
                    status: AdmissionStatusV1::Active,
                    expires_at: None,
                },
            });
        }
        commands.push(AdmissionTargetV1::ServiceIdentity {
            state: ServiceIdentityAdmissionV1 {
                tenant_id: a.tenant_id,
                workflow_id,
                identity_id,
                policy_epoch: a.minimum_admission_epoch,
                status: AdmissionStatusV1::Active,
                capabilities: payload.authorization.capabilities.iter().cloned().collect(),
                grant_ids: required_grants,
            },
        });
        commands.extend(grant_targets);
        commands.extend(
            runtime_grants::application_user_targets(
                &self.pool,
                a.tenant_id,
                a.application_id,
                a.activation_sequence,
            )
            .await?,
        );
        for (index, target) in commands.into_iter().enumerate() {
            let event = Uuid::now_v7();
            let target_hash = agentx_runtime_contracts::content_hash(&target)?;
            let command = RuntimeAdmissionCommandV1 {
                api_version: 1,
                command: CommandEnvelopeV1 {
                    schema_version: 1,
                    event_id: event,
                    source_plane: Plane::Control,
                    tenant_id: a.tenant_id,
                    aggregate_type: "admission".into(),
                    aggregate_id: a.application_id.to_string(),
                    object_version: a.minimum_admission_epoch,
                    occurred_at: OffsetDateTime::now_utc(),
                    payload: json!({}),
                    content_hash: target_hash,
                    correlation_id: a.id,
                    causation_id: None,
                    idempotency_key: format!("{}:admission:{index}", a.id),
                },
                admission_epoch: a.minimum_admission_epoch,
                target,
            };
            let receipt: agentx_runtime_contracts::ApplyReceiptV1 = self
                .post(
                    "runtime.admission.apply",
                    "/internal/runtime/v1/admission-commands:apply",
                    &command,
                )
                .await?;
            if !receipt.applied {
                return Err(RuntimeCallError::Response {
                    status: reqwest::StatusCode::UNPROCESSABLE_ENTITY,
                    code: receipt
                        .result
                        .pointer("/rejection/code")
                        .and_then(Value::as_str)
                        .unwrap_or("ADMISSION_REJECTED")
                        .to_owned(),
                    message: receipt
                        .result
                        .pointer("/rejection/message")
                        .and_then(Value::as_str)
                        .unwrap_or("Runtime rejected Admission")
                        .to_owned(),
                }
                .into());
            }
        }
        let mut tx = self.pool.begin().await?;
        self.transition(&mut tx, a, "preparing", "preparing", "prepare", a.bundle_id)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn prepare(&self, a: &Attempt) -> Result<()> {
        let row=sqlx::query("SELECT payload_json,content_hash,signature_key_id,signature FROM execution_spec_bundles WHERE id=?").bind(a.bundle_id).fetch_one(&self.pool).await?;
        let payload = serde_json::from_value(row.try_get("payload_json")?)?;
        let bundle = agentx_runtime_contracts::ExecutionSpecBundleV2 {
            payload,
            content_hash: ContentHash::parse(row.try_get::<String, _>("content_hash")?)?,
            signature: agentx_runtime_contracts::Ed25519Signature {
                key_id: row.try_get("signature_key_id")?,
                algorithm: agentx_runtime_contracts::SignatureAlgorithm::Ed25519,
                signature_base64: STANDARD.encode(row.try_get::<Vec<u8>, _>("signature")?),
            },
        };
        let request = PrepareBundleRequestV1 {
            api_version: 1,
            idempotency_key: format!("{}:prepare", a.id),
            bundle,
        };
        let receipt: agentx_runtime_contracts::PublishReceiptV1 = self
            .post(
                "runtime.bundles.prepare",
                "/internal/runtime/v1/bundles:prepare",
                &request,
            )
            .await?;
        ensure_publish_receipt(&receipt)?;
        let mut tx = self.pool.begin().await?;
        self.transition(&mut tx, a, "preparing", "prepared", "activate", a.bundle_id)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn activate(&self, a: &Attempt) -> Result<()> {
        let policy: String = sqlx::query_scalar(
            "SELECT session_version_policy FROM application_deployments WHERE tenant_id=? AND id=?",
        )
        .bind(a.tenant_id)
        .bind(a.deployment_id)
        .fetch_one(&self.pool)
        .await?;
        let runtime_config_revision: u64 = sqlx::query_scalar(
            "SELECT trigger_revision FROM application_deployments WHERE tenant_id=? AND id=?",
        )
        .bind(a.tenant_id)
        .bind(a.deployment_id)
        .fetch_one(&self.pool)
        .await?;
        let manifest = ActivationManifestV1 {
            api_version: 1,
            tenant_id: a.tenant_id,
            application_id: a.application_id,
            deployment_id: a.deployment_id,
            bundle_id: a.bundle_id.context("Attempt Bundle")?,
            expected_head_version: a.expected_head_version,
            activation_sequence: a.activation_sequence,
            minimum_admission_epoch: a.minimum_admission_epoch,
            runtime_config_revision,
            runtime_policy: agentx_runtime_contracts::ApplicationRuntimePolicyV1 {
                session_version_policy: match policy.as_str() {
                    "pinned" => agentx_runtime_contracts::SessionVersionPolicyV1::Pinned,
                    "follow_deployment" => {
                        agentx_runtime_contracts::SessionVersionPolicyV1::FollowDeployment
                    }
                    "manual_upgrade" => {
                        agentx_runtime_contracts::SessionVersionPolicyV1::ManualUpgrade
                    }
                    _ => anyhow::bail!("invalid Session policy"),
                },
                synchronous_wait_seconds: 30,
                maximum_json_bytes: 1_048_576,
                maximum_multipart_bytes: 52_428_800,
            },
        };
        let receipt: agentx_runtime_contracts::PublishReceiptV1 =
            if a.requested_action == "rollback" {
                let request = RollbackDeploymentRequestV1 {
                    api_version: 1,
                    idempotency_key: format!("{}:rollback", a.id),
                    manifest,
                };
                self.post(
                    "runtime.deployments.rollback",
                    "/internal/runtime/v1/deployments:rollback",
                    &request,
                )
                .await?
            } else {
                let request = ActivateDeploymentRequestV1 {
                    api_version: 1,
                    idempotency_key: format!("{}:activate", a.id),
                    manifest,
                };
                self.post(
                    "runtime.deployments.activate",
                    "/internal/runtime/v1/deployments:activate",
                    &request,
                )
                .await?
            };
        ensure_publish_receipt(&receipt)?;
        let mut tx = self.pool.begin().await?;
        self.transition(
            &mut tx,
            a,
            "prepared",
            "activating",
            "update_head",
            a.bundle_id,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn update_head(&self, a: &Attempt) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE application_deployments SET status='superseded' WHERE tenant_id=? AND application_id=? AND id<>? AND status='active'").bind(a.tenant_id).bind(a.application_id).bind(a.deployment_id).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE application_deployments SET status='active' WHERE id=? AND tenant_id=?",
        )
        .bind(a.deployment_id)
        .bind(a.tenant_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE applications a JOIN application_deployments d ON d.tenant_id=a.tenant_id AND d.id=? SET a.published_runtime_config_revision=GREATEST(a.published_runtime_config_revision,d.trigger_revision) WHERE a.id=? AND a.tenant_id=?")
            .bind(a.deployment_id).bind(a.application_id).bind(a.tenant_id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO application_deployment_heads(tenant_id,application_id,deployment_id) VALUES(?,?,?) ON DUPLICATE KEY UPDATE deployment_id=VALUES(deployment_id),version=version+1").bind(a.tenant_id).bind(a.application_id).bind(a.deployment_id).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE applications SET status='active',version=version+1 WHERE id=? AND tenant_id=?",
        )
        .bind(a.application_id)
        .bind(a.tenant_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE execution_spec_bundles SET status='published',published_at=COALESCE(published_at,UTC_TIMESTAMP(6)) WHERE id=? AND tenant_id=?")
            .bind(a.bundle_id).bind(a.tenant_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE outbox SET status='published',published_at=UTC_TIMESTAMP(6),last_error=NULL WHERE tenant_id=? AND aggregate_type='bundle_publish' AND aggregate_id=? AND status IN ('pending','failed')")
            .bind(a.tenant_id).bind(a.id.to_string()).execute(&mut *tx).await?;
        self.transition(&mut tx, a, "activating", "active", "none", a.bundle_id)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn transition(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
        a: &Attempt,
        expected: &str,
        state: &str,
        next: &str,
        bundle: Option<Uuid>,
    ) -> Result<()> {
        let changed=sqlx::query("UPDATE publish_attempts SET state=?,next_action=?,bundle_id=?,locked_by=NULL,locked_until=NULL,updated_at=UTC_TIMESTAMP(6),completed_at=IF(?='active',UTC_TIMESTAMP(6),NULL) WHERE id=? AND state=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)").bind(state).bind(next).bind(bundle).bind(state).bind(a.id).bind(expected).bind(self.owner.0).bind(a.fencing_token).execute(&mut **tx).await?;
        anyhow::ensure!(
            changed.rows_affected() == 1,
            "Publish Attempt Lease was lost"
        );
        Ok(())
    }
    fn token(&self, scope: &str) -> Result<String> {
        let now = now_unix();
        Ok(issue_service_token(
            &self.jwt_kid,
            self.jwt_key.expose_secret().as_bytes(),
            &ServiceClaimsV1 {
                iss: "agentx-control".into(),
                aud: "agentx-runtime-internal".into(),
                sub: "publisher".into(),
                role: ControlRole::Publisher,
                scope: BTreeSet::from([scope.into()]),
                iat: now,
                exp: now + 300,
                jti: Uuid::now_v7(),
            },
        )?)
    }
    async fn post<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        scope: &str,
        path: &str,
        body: &T,
    ) -> Result<R> {
        let response = self
            .http
            .post(format!("{}{path}", self.runtime_url))
            .bearer_auth(self.token(scope)?)
            .json(body)
            .send()
            .await
            .map_err(RuntimeCallError::transport)?;
        Ok(decode_runtime_response(response).await?.json().await?)
    }
}

#[derive(Debug, Error)]
enum RuntimeCallError {
    #[error("Runtime request transport failed")]
    Transport(#[source] reqwest::Error),
    #[error("Runtime rejected the request with {status}: {code}")]
    Response {
        status: reqwest::StatusCode,
        code: String,
        message: String,
    },
}

impl RuntimeCallError {
    fn transport(error: reqwest::Error) -> Self {
        Self::Transport(error)
    }

    fn terminal(&self) -> bool {
        matches!(self, Self::Response { status, .. } if status.is_client_error())
    }

    fn code(&self) -> String {
        match self {
            Self::Transport(_) => "RUNTIME_UNAVAILABLE".into(),
            Self::Response { code, .. } => code.clone(),
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Transport(_) => "Runtime Internal API is temporarily unavailable".into(),
            Self::Response { message, .. } => message.clone(),
        }
    }
}

async fn decode_runtime_response(
    response: reqwest::Response,
) -> std::result::Result<reqwest::Response, RuntimeCallError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.json::<Value>().await.unwrap_or_else(|_| json!({}));
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("RUNTIME_REQUEST_FAILED")
        .to_owned();
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Runtime rejected the publish request")
        .to_owned();
    Err(RuntimeCallError::Response {
        status,
        code,
        message,
    })
}

fn retry_delay_seconds(fencing_token: u64) -> u64 {
    1_u64 << fencing_token.saturating_sub(1).min(6)
}

fn sanitize_error(value: &str) -> String {
    let value = value.replace(['\r', '\n'], " ");
    value.chars().take(1000).collect()
}

fn ensure_publish_receipt(receipt: &agentx_runtime_contracts::PublishReceiptV1) -> Result<()> {
    if receipt.status == agentx_runtime_contracts::PublishReceiptStatusV1::Accepted {
        return Ok(());
    }
    let rejection = receipt.rejection.as_ref();
    Err(RuntimeCallError::Response {
        status: reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        code: rejection
            .map(|value| format!("{:?}", value.code).to_ascii_uppercase())
            .unwrap_or_else(|| "PUBLISH_REJECTED".into()),
        message: rejection
            .map(|value| value.message.clone())
            .unwrap_or_else(|| "Runtime rejected publish operation".into()),
    }
    .into())
}

async fn load_dependencies(
    pool: &MySqlPool,
    tenant: Uuid,
    definition: &WorkflowDefinition,
) -> Result<BTreeMap<Uuid, WorkflowDefinition>> {
    let mut result = BTreeMap::new();
    let mut pending = dependency_ids(definition)?;
    while let Some(id) = pending.pop() {
        if result.contains_key(&id) {
            continue;
        }
        let value: Value = sqlx::query_scalar(
            "SELECT definition_json FROM workflow_versions WHERE tenant_id=? AND id=?",
        )
        .bind(tenant)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .with_context(|| format!("fixed Workflow Version {id} is missing"))?;
        let child: WorkflowDefinition = serde_json::from_value(value)?;
        pending.extend(dependency_ids(&child)?);
        result.insert(id, child);
    }
    Ok(result)
}

fn dependency_ids(definition: &WorkflowDefinition) -> Result<Vec<Uuid>> {
    definition
        .nodes
        .iter()
        .filter(|node| node.node_type == "sub_workflow")
        .map(|node| {
            node.parameters
                .get("workflowVersionId")
                .and_then(Value::as_str)
                .context("Sub-workflow must use a fixed Workflow Version")
                .and_then(|value| Uuid::parse_str(value).map_err(Into::into))
        })
        .collect()
}
fn hex(bytes: Vec<u8>) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn runtime_object_idempotency_key(
    object: &agentx_runtime_contracts::RuntimeObjectReferenceV1,
) -> String {
    format!(
        "runtime-object:{}:{}",
        object.object_id,
        object.content_hash.as_str()
    )
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
