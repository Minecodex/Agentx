#[path = "work_package_models.rs"]
mod models;
use models::{load_model_binding, require_model_grant};

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
};

#[cfg(test)]
use agentx_bundle_builder::compile_workflow_version_with_dependencies;
use agentx_bundle_builder::{
    WorkPackageBuildSource, build_work_package_with_resolved,
    compile_workflow_version_with_resolved_plugins, composite_ir_object_id,
    node_registry_with_plugins,
};
use agentx_domain::WorkflowDefinition;
use agentx_runtime_contracts::{
    CancelWorkPackageRequestV1, ContentHash, ControlRole, ExecuteWorkPackageRequestV1,
    ExecutionDepartmentSnapshotV1, ExecutionWorkflowSnapshotV1, PartialExecutionModeV1,
    PrepareWorkPackageRequestV1, PublishReceiptStatusV1, RuntimeAuthorizationSnapshotV1,
    RuntimeCallPurposeV1, RuntimeDebugInputSourceV1, RuntimeDebugPlanV1, RuntimeEvaluationCaseV1,
    RuntimeEvaluatorV1, RuntimeObjectReferenceV1, RuntimeObjectUploadMetadataV1,
    RuntimeObjectUploadReceiptV1, RuntimePolicyV1, RuntimeResourceBindingV1,
    RuntimeResourceConfigurationV1, RuntimeResourceKindV1, RuntimeWorkPackageOverlayV1,
    RuntimeWorkPackageSpecV1, ServiceClaimsV1, SideEffectResolutionV1, StorageDomain,
    VaultSecretReferenceV1, WorkPackagePurpose, issue_service_token, now_unix,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ed25519_dalek::{SigningKey, pkcs8::DecodePrivateKey};
use object_store::path::Path as ObjectPath;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState, execution_origin},
};

fn require_complete_plugin_definitions(
    resolved: &crate::canvas_plugin_api::ResolvedWorkflowPlugins,
) -> ApiResult<()> {
    if resolved.incomplete.is_empty() && resolved.invalid.is_empty() {
        return Ok(());
    }
    let message = resolved
        .incomplete
        .iter()
        .chain(&resolved.invalid)
        .map(|issue| format!("{}: {}", issue.path, issue.message))
        .collect::<Vec<_>>()
        .join("; ");
    Err(ApiError::unprocessable(
        "PLUGIN_DEFINITION_NOT_READY",
        message,
    ))
}

async fn workflow_snapshot(
    pool: &MySqlPool,
    tenant_id: Uuid,
    workflow_id: Uuid,
    version_id: Uuid,
    version_number: u64,
) -> ApiResult<ExecutionWorkflowSnapshotV1> {
    let row = sqlx::query("SELECT w.name,w.owner_department_id,d.name owner_department_name FROM workflows w LEFT JOIN departments d ON d.tenant_id=w.tenant_id AND d.id=w.owner_department_id WHERE w.tenant_id=? AND w.id=?")
        .bind(tenant_id)
        .bind(workflow_id)
        .fetch_one(pool)
        .await?;
    Ok(ExecutionWorkflowSnapshotV1 {
        id: workflow_id,
        name: row.try_get("name")?,
        version_id,
        version_number,
        owner_department: row
            .try_get::<Option<Uuid>, _>("owner_department_id")?
            .map(|id| ExecutionDepartmentSnapshotV1 {
                id,
                name: row.try_get("owner_department_name").unwrap_or_default(),
            }),
    })
}

#[derive(Clone)]
pub(crate) struct WorkPackageClient {
    runtime_url: String,
    http: reqwest::Client,
    jwt_kid: String,
    jwt_key: SecretString,
    signing_kid: String,
    signing_key: SigningKey,
}

impl WorkPackageClient {
    pub(crate) fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            runtime_url: env::var("AGENTX_RUNTIME_INTERNAL_URL").unwrap_or_else(|_| {
                "http://runtime-gateway-internal.agentx-runtime.svc:8080".into()
            }),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            jwt_kid: env::var("AGENTX_CONTROL_PUBLISHER_JWT_KID")?,
            jwt_key: SecretString::from(env::var("AGENTX_CONTROL_PUBLISHER_JWT_PRIVATE_KEY_PEM")?),
            signing_kid: env::var("AGENTX_CONTROL_WORK_PACKAGE_KEY_ID")?,
            signing_key: SigningKey::from_pkcs8_pem(&env::var(
                "AGENTX_CONTROL_WORK_PACKAGE_ED25519_PRIVATE_KEY_PEM",
            )?)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(runtime_url: String) -> Self {
        Self {
            runtime_url,
            http: reqwest::Client::new(),
            jwt_kid: "test-service".into(),
            jwt_key: SecretString::from(include_str!(
                "../../../crates/agentx-runtime-contracts/tests/fixtures/service-private.pem"
            )),
            signing_kid: "test-work-package".into(),
            signing_key: SigningKey::from_bytes(&[7_u8; 32]),
        }
    }

    async fn post<T: DeserializeOwned, B: Serialize>(
        &self,
        scope: &str,
        path: &str,
        body: &B,
    ) -> ApiResult<T> {
        let token = self.token(scope)?;
        let response = self
            .http
            .post(format!("{}{}", self.runtime_url, path))
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, %path, "Runtime Work Package API is unavailable");
                ApiError::unavailable("RUNTIME_UNAVAILABLE", "Runtime service is unavailable")
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(%status, %path, response_body=%body, "Runtime rejected Work Package request");
            return Err(ApiError::conflict(
                "RUNTIME_WORK_PACKAGE_REJECTED",
                "Runtime rejected the Work Package request",
            ));
        }
        response.json().await.map_err(ApiError::internal)
    }

    fn token(&self, scope: &str) -> ApiResult<String> {
        let now = now_unix();
        issue_service_token(
            &self.jwt_kid,
            self.jwt_key.expose_secret().as_bytes(),
            &ServiceClaimsV1 {
                iss: "agentx-control".into(),
                aud: "agentx-runtime-internal".into(),
                sub: "debug-orchestrator".into(),
                role: ControlRole::Publisher,
                scope: BTreeSet::from([scope.into()]),
                iat: now,
                exp: now + 300,
                jti: Uuid::now_v7(),
            },
        )
        .map_err(ApiError::internal)
    }

    async fn upload_object(
        &self,
        metadata: &RuntimeObjectUploadMetadataV1,
        content: Vec<u8>,
    ) -> ApiResult<RuntimeObjectUploadReceiptV1> {
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
                        reqwest::multipart::Part::text(
                            serde_json::to_string(metadata).map_err(ApiError::internal)?,
                        )
                        .mime_str("application/json")
                        .map_err(ApiError::internal)?,
                    )
                    .part(
                        "content",
                        reqwest::multipart::Part::bytes(content)
                            .mime_str(&metadata.media_type)
                            .map_err(ApiError::internal)?,
                    ),
            )
            .send()
            .await
            .map_err(|_| {
                ApiError::unavailable(
                    "RUNTIME_UNAVAILABLE",
                    "Runtime object upload is unavailable",
                )
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(%status, response_body=%body, "Runtime rejected Work Package object");
            return Err(ApiError::conflict(
                "RUNTIME_OBJECT_REJECTED",
                "Runtime rejected the immutable evaluator object",
            ));
        }
        response.json().await.map_err(ApiError::internal)
    }

    pub(crate) fn signing_key_id(&self) -> &str {
        &self.signing_kid
    }

    pub(crate) fn sign_content_hash(&self, value: &str) -> String {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use ed25519_dalek::Signer as _;
        URL_SAFE_NO_PAD.encode(self.signing_key.sign(value.as_bytes()).to_bytes())
    }

    pub(crate) fn verify_content_hash(&self, value: &str, signature: &str) -> bool {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use ed25519_dalek::{Signature, Verifier as _};
        URL_SAFE_NO_PAD
            .decode(signature)
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
            .is_some_and(|signature| {
                self.signing_key
                    .verifying_key()
                    .verify(value.as_bytes(), &signature)
                    .is_ok()
            })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartDebugRunRequest {
    pub(crate) idempotency_key: String,
    #[serde(default)]
    pub(crate) input: Value,
    #[serde(default)]
    pub(crate) context: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) node_parameters: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) mode: Option<PartialExecutionModeV1>,
    pub(crate) target_node_id: Option<String>,
    pub(crate) input_source: Option<RuntimeDebugInputSourceV1>,
    #[serde(default)]
    pub(crate) side_effect_decisions: BTreeMap<String, SideEffectResolutionV1>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartDebugRunResponse {
    debug_run_id: Uuid,
    work_package_id: Uuid,
    pub(crate) execution_id: Option<Uuid>,
    pub(crate) status: String,
    pub(crate) replayed: bool,
}

struct PendingObject {
    metadata: RuntimeObjectUploadMetadataV1,
    reference: RuntimeObjectReferenceV1,
    content: Vec<u8>,
}

pub(crate) async fn start_version_execution(
    state: &ControlApiState,
    actor: &Actor,
    version_id: Uuid,
    input: Value,
    idempotency_key: String,
) -> ApiResult<StartDebugRunResponse> {
    validate_idempotency_key(&idempotency_key)?;
    let version = sqlx::query("SELECT v.workflow_id,v.version_number,v.source_revision,v.content_hash,v.definition_json,i.id service_identity_id,i.version identity_version FROM workflow_versions v JOIN workflows w ON w.tenant_id=v.tenant_id AND w.id=v.workflow_id JOIN workflow_service_identities i ON i.tenant_id=v.tenant_id AND i.workflow_id=v.workflow_id WHERE v.tenant_id=? AND v.id=? AND w.status='active' AND i.status='active'")
        .bind(actor.tenant_id)
        .bind(version_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Workflow Version"))?;
    let workflow_id: Uuid = version.try_get("workflow_id")?;
    let version_number: u64 = version.try_get("version_number")?;
    let source_revision: u64 = version.try_get("source_revision")?;
    let version_hash: String = version.try_get("content_hash")?;
    let request_hash = agentx_runtime_contracts::content_hash(&json!({
        "workflowVersionId": version_id,
        "versionHash": version_hash,
        "input": input,
    }))
    .map_err(ApiError::internal)?;
    if let Some(response) = replay(
        &state.pool,
        actor.tenant_id,
        &idempotency_key,
        request_hash.as_str(),
    )
    .await?
    {
        return Ok(response);
    }

    let definition: WorkflowDefinition =
        serde_json::from_value(version.try_get("definition_json")?).map_err(|error| {
            ApiError::unprocessable("INVALID_WORKFLOW_DEFINITION", error.to_string())
        })?;
    let (dependencies, resources, pending_objects) =
        load_published_version_closure(state, actor.tenant_id, version_id, &definition).await?;
    crate::canvas_plugin_api::require_definition_plugin_closure_enabled(
        state,
        actor.tenant_id,
        &definition,
        &dependencies,
    )
    .await?;
    let plugin_manifests =
        crate::canvas_plugin_api::plugin_manifests_for_tenant(state, actor.tenant_id).await?;
    let plugin_registry =
        node_registry_with_plugins(&dependencies, &plugin_manifests).map_err(ApiError::internal)?;
    let resolved = crate::canvas_plugin_api::resolve_workflow_plugin_closure(
        state,
        actor.tenant_id,
        &definition,
        &dependencies,
        &plugin_registry,
    )
    .await?;
    require_complete_plugin_definitions(&resolved)?;
    let mut resources = resources;
    let mut pending_objects = pending_objects;
    add_composite_closure(
        &state.pool,
        actor.tenant_id,
        &dependencies,
        &plugin_manifests,
        &resolved.dependency_manifests,
        &mut resources,
        &mut pending_objects,
    )
    .await?;
    add_plugin_runtime_objects(actor.tenant_id, &resolved, &mut pending_objects)?;
    let service_identity_id: Uuid = version.try_get("service_identity_id")?;
    let (grant_ids, grant_bindings) = crate::runtime_resource_binding::authorization_grants(
        &state.pool,
        actor.tenant_id,
        service_identity_id,
    )
    .await?;
    let package_id = Uuid::now_v7();
    let debug_run_id = Uuid::now_v7();
    let created_at = OffsetDateTime::now_utc();
    let expires_at = created_at + time::Duration::hours(1);
    let compiled = compile_workflow_version_with_resolved_plugins(
        &definition,
        package_id,
        &dependencies,
        &plugin_manifests,
        &resolved.manifests,
    )
    .map_err(|error| {
        ApiError::unprocessable(
            "WORK_PACKAGE_BUILD_FAILED",
            format!("Workflow Version could not be compiled: {error}"),
        )
    })?;
    let debug_plan = build_whole_execution_plan(&compiled)?;
    let package = build_work_package_with_resolved(
        WorkPackageBuildSource {
            package_id,
            tenant_id: actor.tenant_id,
            workflow: workflow_snapshot(
                &state.pool,
                actor.tenant_id,
                workflow_id,
                version_id,
                version_number,
            )
            .await?,
            origin: execution_origin(state, actor).await?,
            purpose: WorkPackagePurpose::Debug,
            call_purpose: RuntimeCallPurposeV1::Debug,
            spec: RuntimeWorkPackageSpecV1::Debug {
                draft_revision: version_number,
                debug_plan,
            },
            source_revision: format!("version:{version_id}:{version_number}"),
            definition,
            dependency_versions: dependencies,
            plugin_manifests,
            supported_capabilities: agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
                .iter()
                .map(ToString::to_string)
                .collect(),
            overlay: RuntimeWorkPackageOverlayV1 {
                input,
                ..Default::default()
            },
            resources,
            authorization: RuntimeAuthorizationSnapshotV1 {
                schema_version: 1,
                tenant_id: actor.tenant_id,
                service_identity_id,
                workflow_id,
                policy_epoch: version.try_get("identity_version")?,
                grant_ids,
                grant_bindings,
                capabilities: agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                maximum_policy_staleness_seconds: 72 * 60 * 60,
                captured_at: created_at,
            },
            objects: pending_objects
                .iter()
                .map(|object| object.reference.clone())
                .collect(),
            runtime_policy: RuntimePolicyV1::default(),
            created_at,
            expires_at,
        },
        &resolved.manifests,
        &resolved.dependency_manifests,
        &state.work_packages.signing_kid,
        &state.work_packages.signing_key,
    )
    .map_err(|error| ApiError::unprocessable("WORK_PACKAGE_BUILD_FAILED", error.to_string()))?;
    persist_version_building(
        &state.pool,
        actor,
        workflow_id,
        version_id,
        source_revision,
        debug_run_id,
        &idempotency_key,
        request_hash.as_str(),
        &package,
    )
    .await?;
    for object in pending_objects {
        let receipt = state
            .work_packages
            .upload_object(&object.metadata, object.content)
            .await;
        match receipt {
            Ok(receipt)
                if receipt.object.object_id == object.reference.object_id
                    && receipt.object.content_hash == object.reference.content_hash => {}
            Ok(_) => {
                mark_failed(&state.pool, actor.tenant_id, package_id).await?;
                return Err(ApiError::conflict(
                    "RUNTIME_OBJECT_RECEIPT_MISMATCH",
                    "Runtime returned a different immutable Resource object",
                ));
            }
            Err(error) => {
                mark_failed(&state.pool, actor.tenant_id, package_id).await?;
                return Err(error);
            }
        }
    }
    prepare_and_execute(state, actor.tenant_id, debug_run_id, package).await
}

pub(crate) async fn start_debug_run(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(workflow_id): Path<Uuid>,
    Json(request): Json<StartDebugRunRequest>,
) -> ApiResult<(StatusCode, Json<StartDebugRunResponse>)> {
    actor.require("workflow:edit")?;
    validate_idempotency_key(&request.idempotency_key)?;
    let draft = sqlx::query("SELECT d.revision,d.definition_json,i.id service_identity_id,i.version identity_version FROM workflow_drafts d JOIN workflows w ON w.id=d.workflow_id AND w.tenant_id=d.tenant_id JOIN workflow_service_identities i ON i.workflow_id=d.workflow_id AND i.tenant_id=d.tenant_id WHERE d.tenant_id=? AND d.workflow_id=? AND w.status='active' AND i.status='active'")
        .bind(actor.tenant_id)
        .bind(workflow_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Workflow draft"))?;
    let revision: u64 = draft.try_get("revision")?;
    let request_hash = agentx_runtime_contracts::content_hash(&json!({
        "workflowId": workflow_id,
        "revision": revision,
        "input": request.input,
        "context": request.context,
        "nodeParameters": request.node_parameters,
        "mode": request.mode,
        "targetNodeId": request.target_node_id,
        "inputSource": request.input_source,
        "sideEffectDecisions": request.side_effect_decisions,
    }))
    .map_err(ApiError::internal)?;
    if let Some(response) = replay(
        &state.pool,
        actor.tenant_id,
        &request.idempotency_key,
        request_hash.as_str(),
    )
    .await?
    {
        return Ok((StatusCode::ACCEPTED, Json(response)));
    }

    let definition: WorkflowDefinition = serde_json::from_value(draft.try_get("definition_json")?)
        .map_err(|error| {
            ApiError::unprocessable(
                "INVALID_WORKFLOW_DEFINITION",
                format!("Workflow draft cannot be compiled: {error}"),
            )
        })?;
    let dependencies = load_version_dependencies(&state.pool, actor.tenant_id, &definition).await?;
    crate::canvas_plugin_api::require_definition_plugin_closure_enabled(
        &state,
        actor.tenant_id,
        &definition,
        &dependencies,
    )
    .await?;
    let snapshots = crate::workflow_resources::build_version_snapshots(
        &state,
        actor.tenant_id,
        workflow_id,
        &definition,
    )
    .await?;
    let mut resources = Vec::new();
    for snapshot in &snapshots {
        let binding =
            crate::runtime_resource_binding::from_snapshot(snapshot).map_err(|error| {
                ApiError::unprocessable("WORKFLOW_RESOURCE_SNAPSHOT_INVALID", error.to_string())
            })?;
        if let Some(existing) = resources
            .iter()
            .find(|existing: &&RuntimeResourceBindingV1| {
                existing.resource_kind == binding.resource_kind
                    && existing.resource_id == binding.resource_id
                    && existing.resource_version == binding.resource_version
            })
        {
            if existing.content_hash != binding.content_hash {
                return Err(ApiError::conflict(
                    "WORKFLOW_RESOURCE_SNAPSHOT_CONFLICT",
                    "Workflow Draft contains conflicting immutable Resource snapshots",
                ));
            }
        } else {
            resources.push(binding);
        }
    }
    let snapshot_values = snapshots
        .iter()
        .map(|snapshot| snapshot.snapshot.clone())
        .collect::<Vec<_>>();
    let mut pending_objects =
        load_snapshot_values(&state, actor.tenant_id, &snapshot_values).await?;
    let plugin_manifests =
        crate::canvas_plugin_api::plugin_manifests_for_tenant(&state, actor.tenant_id).await?;
    let plugin_registry =
        node_registry_with_plugins(&dependencies, &plugin_manifests).map_err(ApiError::internal)?;
    let resolved = crate::canvas_plugin_api::resolve_workflow_plugin_closure(
        &state,
        actor.tenant_id,
        &definition,
        &dependencies,
        &plugin_registry,
    )
    .await?;
    require_complete_plugin_definitions(&resolved)?;
    add_composite_closure(
        &state.pool,
        actor.tenant_id,
        &dependencies,
        &plugin_manifests,
        &resolved.dependency_manifests,
        &mut resources,
        &mut pending_objects,
    )
    .await?;
    add_plugin_runtime_objects(actor.tenant_id, &resolved, &mut pending_objects)?;
    let service_identity_id: Uuid = draft.try_get("service_identity_id")?;
    let (grant_ids, grant_bindings) = crate::runtime_resource_binding::authorization_grants(
        &state.pool,
        actor.tenant_id,
        service_identity_id,
    )
    .await?;
    let package_id = Uuid::now_v7();
    let debug_run_id = Uuid::now_v7();
    let created_at = OffsetDateTime::now_utc();
    let expires_at = created_at + time::Duration::hours(1);
    let mode = request.mode.unwrap_or(PartialExecutionModeV1::Whole);
    let compiled = compile_workflow_version_with_resolved_plugins(
        &definition,
        package_id,
        &dependencies,
        &plugin_manifests,
        &resolved.manifests,
    )
    .map_err(|error| {
        ApiError::unprocessable(
            "WORK_PACKAGE_BUILD_FAILED",
            format!("Workflow Draft could not be compiled: {error}"),
        )
    })?;
    let debug_plan = build_debug_plan(
        &compiled,
        mode,
        request.target_node_id.clone(),
        request.input_source.clone(),
        request.side_effect_decisions.clone(),
    )?;
    let package = build_work_package_with_resolved(
        WorkPackageBuildSource {
            package_id,
            tenant_id: actor.tenant_id,
            workflow: workflow_snapshot(
                &state.pool,
                actor.tenant_id,
                workflow_id,
                package_id,
                revision,
            )
            .await?,
            origin: execution_origin(&state, &actor).await?,
            purpose: WorkPackagePurpose::Debug,
            call_purpose: RuntimeCallPurposeV1::Debug,
            spec: agentx_runtime_contracts::RuntimeWorkPackageSpecV1::Debug {
                draft_revision: revision,
                debug_plan,
            },
            source_revision: format!("draft:{revision}"),
            definition,
            dependency_versions: dependencies,
            plugin_manifests,
            supported_capabilities: agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
                .iter()
                .map(ToString::to_string)
                .collect(),
            overlay: RuntimeWorkPackageOverlayV1 {
                input: request.input,
                context: request.context,
                node_parameters: request.node_parameters,
            },
            resources,
            authorization: RuntimeAuthorizationSnapshotV1 {
                schema_version: 1,
                tenant_id: actor.tenant_id,
                service_identity_id,
                workflow_id,
                policy_epoch: draft.try_get("identity_version")?,
                grant_ids,
                grant_bindings,
                capabilities: agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                maximum_policy_staleness_seconds: 72 * 60 * 60,
                captured_at: created_at,
            },
            objects: pending_objects
                .iter()
                .map(|object| object.reference.clone())
                .collect(),
            runtime_policy: RuntimePolicyV1::default(),
            created_at,
            expires_at,
        },
        &resolved.manifests,
        &resolved.dependency_manifests,
        &state.work_packages.signing_kid,
        &state.work_packages.signing_key,
    )
    .map_err(|error| {
        ApiError::unprocessable(
            "WORK_PACKAGE_BUILD_FAILED",
            format!("Debug Work Package could not be built: {error}"),
        )
    })?;
    persist_building(
        &state.pool,
        &actor,
        workflow_id,
        revision,
        debug_run_id,
        &request.idempotency_key,
        request_hash.as_str(),
        &package,
    )
    .await?;

    for object in pending_objects {
        let receipt = state
            .work_packages
            .upload_object(&object.metadata, object.content)
            .await;
        match receipt {
            Ok(receipt)
                if receipt.object.object_id == object.reference.object_id
                    && receipt.object.content_hash == object.reference.content_hash => {}
            Ok(_) => {
                mark_failed(&state.pool, actor.tenant_id, package_id).await?;
                return Err(ApiError::conflict(
                    "RUNTIME_OBJECT_RECEIPT_MISMATCH",
                    "Runtime returned a different immutable Resource object",
                ));
            }
            Err(error) => {
                mark_failed(&state.pool, actor.tenant_id, package_id).await?;
                return Err(error);
            }
        }
    }

    let response = prepare_and_execute(&state, actor.tenant_id, debug_run_id, package).await?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn prepare_and_execute(
    state: &ControlApiState,
    tenant_id: Uuid,
    debug_run_id: Uuid,
    package: agentx_runtime_contracts::RuntimeWorkPackageV1,
) -> ApiResult<StartDebugRunResponse> {
    let package_id = package.payload.package_id;
    let prepare: agentx_runtime_contracts::PublishReceiptV1 = match state
        .work_packages
        .post(
            "runtime.work-packages.prepare",
            "/internal/runtime/v1/work-packages:prepare",
            &PrepareWorkPackageRequestV1 {
                api_version: 1,
                idempotency_key: format!("debug:prepare:{package_id}"),
                work_package: package,
            },
        )
        .await
    {
        Ok(receipt) => receipt,
        Err(error) => {
            mark_failed(&state.pool, tenant_id, package_id).await?;
            return Err(error);
        }
    };
    if prepare.status != PublishReceiptStatusV1::Accepted {
        mark_failed(&state.pool, tenant_id, package_id).await?;
        return Err(ApiError::conflict(
            "WORK_PACKAGE_PREPARE_REJECTED",
            "Runtime rejected the Debug Work Package",
        ));
    }
    sqlx::query("UPDATE runtime_work_package_publications SET status='prepared',runtime_version=?,prepare_receipt_json=? WHERE tenant_id=? AND package_id=? AND status='building'")
        .bind(prepare.receipt.object_version)
        .bind(serde_json::to_value(&prepare).map_err(ApiError::internal)?)
        .bind(tenant_id)
        .bind(package_id)
        .execute(&state.pool)
        .await?;
    sqlx::query("UPDATE workflow_debug_runs SET status='prepared' WHERE tenant_id=? AND id=? AND status='building'")
        .bind(tenant_id)
        .bind(debug_run_id)
        .execute(&state.pool)
        .await?;
    let execute: agentx_runtime_contracts::ApplyReceiptV1 = match state
        .work_packages
        .post(
            "runtime.work-packages.execute",
            &format!("/internal/runtime/v1/work-packages/{package_id}:execute"),
            &ExecuteWorkPackageRequestV1 {
                api_version: 1,
                idempotency_key: format!("debug:execute:{package_id}"),
                package_id,
                input: Value::Null,
            },
        )
        .await
    {
        Ok(receipt) => receipt,
        Err(error) => {
            mark_failed(&state.pool, tenant_id, package_id).await?;
            return Err(error);
        }
    };
    let execution_id = execute
        .result
        .get("executionId")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    if execution_id.is_none() {
        mark_failed(&state.pool, tenant_id, package_id).await?;
        return Err(ApiError::conflict(
            "WORK_PACKAGE_EXECUTION_RECEIPT_INVALID",
            "Runtime did not return an Execution identifier",
        ));
    }
    let receipt_json = serde_json::to_value(&execute).map_err(ApiError::internal)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE runtime_work_package_publications SET status='running',runtime_version=? WHERE tenant_id=? AND package_id=? AND status='prepared'")
        .bind(execute.object_version)
        .bind(tenant_id)
        .bind(package_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE workflow_debug_runs SET status='running',command_id=?,command_version=?,result_receipt_json=? WHERE tenant_id=? AND id=? AND status='prepared'")
        .bind(execute.event_id)
        .bind(execute.object_version)
        .bind(receipt_json)
        .bind(tenant_id)
        .bind(debug_run_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StartDebugRunResponse {
        debug_run_id,
        work_package_id: package_id,
        execution_id,
        status: "running".into(),
        replayed: execute.replayed,
    })
}

fn validate_idempotency_key(value: &str) -> ApiResult<()> {
    if value.trim().is_empty() || value.len() > 192 {
        return Err(ApiError::bad_request(
            "INVALID_IDEMPOTENCY_KEY",
            "Idempotency key must contain 1 to 192 characters",
        ));
    }
    Ok(())
}

async fn load_version_dependencies(
    pool: &MySqlPool,
    tenant_id: Uuid,
    definition: &WorkflowDefinition,
) -> ApiResult<BTreeMap<Uuid, WorkflowDefinition>> {
    let mut dependencies = BTreeMap::new();
    let mut pending = dependency_ids(definition)?;
    while let Some(version_id) = pending.pop() {
        if dependencies.contains_key(&version_id) {
            continue;
        }
        let value: Value = sqlx::query_scalar(
            "SELECT definition_json FROM workflow_versions WHERE tenant_id=? AND id=?",
        )
        .bind(tenant_id)
        .bind(version_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| {
            ApiError::unprocessable(
                "COMPOSITE_VERSION_MISSING",
                format!("Fixed Workflow Version {version_id} is missing"),
            )
        })?;
        let child: WorkflowDefinition = serde_json::from_value(value).map_err(|error| {
            ApiError::unprocessable("INVALID_COMPOSITE_DEFINITION", error.to_string())
        })?;
        pending.extend(dependency_ids(&child)?);
        dependencies.insert(version_id, child);
    }
    Ok(dependencies)
}

async fn load_published_version_closure(
    state: &ControlApiState,
    tenant_id: Uuid,
    version_id: Uuid,
    definition: &WorkflowDefinition,
) -> ApiResult<(
    BTreeMap<Uuid, WorkflowDefinition>,
    Vec<RuntimeResourceBindingV1>,
    Vec<PendingObject>,
)> {
    let dependencies = load_version_dependencies(&state.pool, tenant_id, definition).await?;
    let resource_rows = sqlx::query("SELECT resource_type,resource_id,resource_version_id,snapshot_json,snapshot_hash FROM workflow_version_resources WHERE tenant_id=? AND workflow_version_id=? ORDER BY resource_type,resource_id,node_id")
        .bind(tenant_id)
        .bind(version_id)
        .fetch_all(&state.pool)
        .await?;
    let mut resources = Vec::new();
    for row in &resource_rows {
        let binding = crate::runtime_resource_binding::from_row(row).map_err(|error| {
            ApiError::unprocessable("WORKFLOW_RESOURCE_SNAPSHOT_INVALID", error.to_string())
        })?;
        if let Some(existing) = resources
            .iter()
            .find(|existing: &&RuntimeResourceBindingV1| {
                existing.resource_kind == binding.resource_kind
                    && existing.resource_id == binding.resource_id
                    && existing.resource_version == binding.resource_version
            })
        {
            if existing.content_hash != binding.content_hash {
                return Err(ApiError::conflict(
                    "WORKFLOW_RESOURCE_SNAPSHOT_CONFLICT",
                    "Workflow Version contains conflicting immutable Resource snapshots",
                ));
            }
        } else {
            resources.push(binding);
        }
    }
    let pending_objects = load_snapshot_objects(state, tenant_id, &resource_rows).await?;
    Ok((dependencies, resources, pending_objects))
}

fn dependency_ids(definition: &WorkflowDefinition) -> ApiResult<Vec<Uuid>> {
    definition
        .nodes
        .iter()
        .filter(|node| node.node_type == "sub_workflow")
        .map(|node| {
            node.parameters
                .get("workflowVersionId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ApiError::unprocessable(
                        "COMPOSITE_VERSION_REQUIRED",
                        "Composite nodes require a fixed Workflow Version",
                    )
                })
                .and_then(|value| {
                    Uuid::parse_str(value).map_err(|_| {
                        ApiError::unprocessable(
                            "COMPOSITE_VERSION_INVALID",
                            "Composite Workflow Version is invalid",
                        )
                    })
                })
        })
        .collect()
}

async fn load_snapshot_objects(
    state: &ControlApiState,
    tenant_id: Uuid,
    rows: &[sqlx::mysql::MySqlRow],
) -> ApiResult<Vec<PendingObject>> {
    let snapshots = rows
        .iter()
        .map(|row| row.try_get("snapshot_json"))
        .collect::<Result<Vec<Value>, _>>()?;
    load_snapshot_values(state, tenant_id, &snapshots).await
}

async fn load_snapshot_values(
    state: &ControlApiState,
    tenant_id: Uuid,
    snapshots: &[Value],
) -> ApiResult<Vec<PendingObject>> {
    let mut objects: Vec<PendingObject> = Vec::new();
    for snapshot in snapshots {
        let Some(entries) = snapshot.get("runtimeObjects").and_then(Value::as_array) else {
            continue;
        };
        for entry in entries {
            let object_id = json_uuid(entry, "objectId")?;
            let content_hash = ContentHash::parse(json_string(entry, "contentHash")?)
                .map_err(ApiError::internal)?;
            if let Some(existing) = objects
                .iter()
                .find(|object| object.reference.object_id == object_id)
            {
                if existing.reference.content_hash != content_hash {
                    return Err(ApiError::conflict(
                        "WORKFLOW_RESOURCE_OBJECT_CONFLICT",
                        "Workflow Version contains conflicting immutable Resource objects",
                    ));
                }
                continue;
            }
            let source_key = json_string(entry, "sourceKey")?;
            let content = state
                .control_objects
                .get(&ObjectPath::from(source_key))
                .await
                .map_err(ApiError::internal)?
                .bytes()
                .await
                .map_err(ApiError::internal)?
                .to_vec();
            let size_bytes = entry
                .get("sizeBytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    ApiError::unprocessable(
                        "WORKFLOW_RESOURCE_OBJECT_INVALID",
                        "Runtime Resource object size is missing",
                    )
                })?;
            let actual_hash = ContentHash::parse(format!("sha256:{:x}", Sha256::digest(&content)))
                .map_err(ApiError::internal)?;
            if content.len() as u64 != size_bytes || actual_hash != content_hash {
                return Err(ApiError::conflict(
                    "WORKFLOW_RESOURCE_OBJECT_CHANGED",
                    "Immutable Runtime Resource object no longer matches its Version snapshot",
                ));
            }
            let media_type = json_string(entry, "mediaType")?;
            objects.push(PendingObject {
                metadata: RuntimeObjectUploadMetadataV1 {
                    api_version: 1,
                    idempotency_key: runtime_object_upload_key(object_id, &content_hash),
                    tenant_id,
                    object_id,
                    content_hash: content_hash.clone(),
                    size_bytes,
                    media_type: media_type.clone(),
                },
                reference: RuntimeObjectReferenceV1 {
                    tenant_id,
                    storage_domain: StorageDomain::Runtime,
                    object_id,
                    object_key: RuntimeObjectReferenceV1::canonical_key(
                        tenant_id,
                        object_id,
                        &content_hash,
                    ),
                    content_hash,
                    size_bytes,
                    media_type,
                },
                content,
            });
        }
    }
    Ok(objects)
}

async fn add_composite_closure(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    dependencies: &BTreeMap<Uuid, WorkflowDefinition>,
    plugin_manifests: &[agentx_node_protocol::NodeManifestVersion],
    resolved_dependencies: &agentx_bundle_builder::ResolvedPluginManifestDependencies,
    resources: &mut Vec<RuntimeResourceBindingV1>,
    objects: &mut Vec<PendingObject>,
) -> ApiResult<()> {
    for (version_id, definition) in dependencies {
        let definition_bytes =
            agentx_runtime_contracts::canonical_bytes(definition).map_err(ApiError::internal)?;
        push_pending_object(
            tenant_id,
            *version_id,
            "application/vnd.agentx.workflow-definition+json",
            definition_bytes,
            objects,
        )?;
        let resolved_manifests = resolved_dependencies
            .get(version_id)
            .cloned()
            .unwrap_or_default();
        let compiled = compile_workflow_version_with_resolved_plugins(
            definition,
            *version_id,
            dependencies,
            plugin_manifests,
            &resolved_manifests,
        )
        .map_err(|error| ApiError::unprocessable("WORKFLOW_COMPILE_FAILED", error.to_string()))?;
        let ir_object_id = composite_ir_object_id(*version_id);
        let ir_bytes =
            agentx_runtime_contracts::canonical_bytes(&compiled).map_err(ApiError::internal)?;
        push_pending_object(
            tenant_id,
            ir_object_id,
            "application/vnd.agentx.compiled-workflow.v1+json",
            ir_bytes,
            objects,
        )?;
        if resources.iter().any(|resource| {
            matches!(
                &resource.configuration,
                RuntimeResourceConfigurationV1::Composite {
                    workflow,
                    ..
                } if workflow.version_id == *version_id
            )
        }) {
            continue;
        }
        let configuration = RuntimeResourceConfigurationV1::Composite {
            workflow: crate::runtime_resource_binding::workflow_snapshot(
                pool,
                tenant_id,
                *version_id,
            )
            .await
            .map_err(ApiError::internal)?,
            definition_object_id: *version_id,
            ir_object_id,
        };
        resources.push(RuntimeResourceBindingV1 {
            resource_kind: RuntimeResourceKindV1::Composite,
            resource_id: *version_id,
            resource_version: version_id.to_string(),
            state_epoch: 1,
            content_hash: agentx_runtime_contracts::content_hash(&configuration)
                .map_err(ApiError::internal)?,
            configuration,
            object_ids: vec![*version_id, ir_object_id],
        });
    }
    Ok(())
}

fn add_plugin_runtime_objects(
    tenant_id: Uuid,
    resolved: &crate::canvas_plugin_api::ResolvedWorkflowPlugins,
    objects: &mut Vec<PendingObject>,
) -> ApiResult<()> {
    for binding in crate::canvas_plugin_resolution::resolved_runtime_plugin_bindings(resolved) {
        let artifact = binding.runtime_artifact.as_ref().ok_or_else(|| {
            ApiError::unprocessable(
                "PLUGIN_ARTIFACT_INVALID",
                "Plugin binding has no immutable Runtime artifact",
            )
        })?;
        let content = binding.runtime_source.as_bytes().to_vec();
        let actual_hash = format!("sha256:{:x}", Sha256::digest(&content));
        if actual_hash != artifact.content_hash
            || content.len() as u64 != artifact.size_bytes
            || artifact.media_type != "text/javascript"
        {
            return Err(ApiError::conflict(
                "PLUGIN_ARTIFACT_CHANGED",
                "Plugin Runtime source no longer matches its immutable artifact reference",
            ));
        }
        push_pending_object(
            tenant_id,
            artifact.object_id,
            &artifact.media_type,
            content,
            objects,
        )?;
    }
    Ok(())
}

fn push_pending_object(
    tenant_id: Uuid,
    object_id: Uuid,
    media_type: &str,
    content: Vec<u8>,
    objects: &mut Vec<PendingObject>,
) -> ApiResult<()> {
    let content_hash = ContentHash::parse(format!("sha256:{:x}", Sha256::digest(&content)))
        .map_err(ApiError::internal)?;
    if let Some(existing) = objects
        .iter()
        .find(|object| object.reference.object_id == object_id)
    {
        if existing.reference.content_hash != content_hash {
            return Err(ApiError::conflict(
                "WORKFLOW_RESOURCE_OBJECT_CONFLICT",
                "Workflow Version contains conflicting immutable Resource objects",
            ));
        }
        return Ok(());
    }
    let size_bytes = content.len() as u64;
    let media_type = media_type.to_owned();
    objects.push(PendingObject {
        metadata: RuntimeObjectUploadMetadataV1 {
            api_version: 1,
            idempotency_key: runtime_object_upload_key(object_id, &content_hash),
            tenant_id,
            object_id,
            content_hash: content_hash.clone(),
            size_bytes,
            media_type: media_type.clone(),
        },
        reference: RuntimeObjectReferenceV1 {
            tenant_id,
            storage_domain: StorageDomain::Runtime,
            object_id,
            object_key: RuntimeObjectReferenceV1::canonical_key(
                tenant_id,
                object_id,
                &content_hash,
            ),
            content_hash,
            size_bytes,
            media_type,
        },
        content,
    });
    Ok(())
}

fn runtime_object_upload_key(object_id: Uuid, content_hash: &ContentHash) -> String {
    format!("runtime-object:{object_id}:{}", content_hash.as_str())
}

fn json_string(value: &Value, key: &str) -> ApiResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ApiError::unprocessable(
                "WORKFLOW_RESOURCE_OBJECT_INVALID",
                format!("Runtime Resource object requires {key}"),
            )
        })
}

fn json_uuid(value: &Value, key: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(&json_string(value, key)?).map_err(|_| {
        ApiError::unprocessable(
            "WORKFLOW_RESOURCE_OBJECT_INVALID",
            format!("Runtime Resource object {key} is invalid"),
        )
    })
}

async fn replay(
    pool: &MySqlPool,
    tenant_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
) -> ApiResult<Option<StartDebugRunResponse>> {
    let row = sqlx::query("SELECT p.package_id,p.status,p.request_hash,d.id debug_run_id,d.result_receipt_json FROM runtime_work_package_publications p JOIN workflow_debug_runs d ON d.tenant_id=p.tenant_id AND d.work_package_id=p.package_id WHERE p.tenant_id=? AND p.prepare_idempotency_key=?")
        .bind(tenant_id)
        .bind(idempotency_key)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<String, _>("request_hash")? != request_hash {
        return Err(ApiError::conflict(
            "IDEMPOTENCY_CONFLICT",
            "Idempotency key is already bound to a different Debug request",
        ));
    }
    let receipt: Option<Value> = row.try_get("result_receipt_json")?;
    let execution_id = receipt
        .as_ref()
        .and_then(|value| value.get("result"))
        .and_then(|value| value.get("executionId"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    Ok(Some(StartDebugRunResponse {
        debug_run_id: row.try_get("debug_run_id")?,
        work_package_id: row.try_get("package_id")?,
        execution_id,
        status: row.try_get("status")?,
        replayed: true,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn persist_building(
    pool: &MySqlPool,
    actor: &Actor,
    workflow_id: Uuid,
    revision: u64,
    debug_run_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
    package: &agentx_runtime_contracts::RuntimeWorkPackageV1,
) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO runtime_work_package_publications(id,tenant_id,package_id,purpose,source_type,source_id,source_revision,content_hash,signature_key_id,package_json,object_manifest_json,status,prepare_idempotency_key,request_hash,expires_at,created_by) VALUES(?,?,?,'debug','workflow_draft',?,?,?,?,?,?,'building',?,?,?,?)")
        .bind(Uuid::now_v7())
        .bind(actor.tenant_id)
        .bind(package.payload.package_id)
        .bind(workflow_id)
        .bind(&package.payload.source_revision)
        .bind(package.content_hash.as_str())
        .bind(&package.signature.key_id)
        .bind(serde_json::to_value(package).map_err(ApiError::internal)?)
        .bind(serde_json::to_value(&package.payload.objects).map_err(ApiError::internal)?)
        .bind(idempotency_key)
        .bind(request_hash)
        .bind(package.payload.expires_at)
        .bind(actor.user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO workflow_debug_runs(id,tenant_id,workflow_id,draft_revision,work_package_id,status,expires_at,created_by) VALUES(?,?,?,?,?,'building',?,?)")
        .bind(debug_run_id)
        .bind(actor.tenant_id)
        .bind(workflow_id)
        .bind(revision)
        .bind(package.payload.package_id)
        .bind(package.payload.expires_at)
        .bind(actor.user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn persist_version_building(
    pool: &MySqlPool,
    actor: &Actor,
    workflow_id: Uuid,
    version_id: Uuid,
    source_revision: u64,
    debug_run_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
    package: &agentx_runtime_contracts::RuntimeWorkPackageV1,
) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO runtime_work_package_publications(id,tenant_id,package_id,purpose,source_type,source_id,source_revision,content_hash,signature_key_id,package_json,object_manifest_json,status,prepare_idempotency_key,request_hash,expires_at,created_by) VALUES(?,?,?,'debug','workflow_version',?,?,?,?,?,?,'building',?,?,?,?)")
        .bind(Uuid::now_v7())
        .bind(actor.tenant_id)
        .bind(package.payload.package_id)
        .bind(version_id)
        .bind(&package.payload.source_revision)
        .bind(package.content_hash.as_str())
        .bind(&package.signature.key_id)
        .bind(serde_json::to_value(package).map_err(ApiError::internal)?)
        .bind(serde_json::to_value(&package.payload.objects).map_err(ApiError::internal)?)
        .bind(idempotency_key)
        .bind(request_hash)
        .bind(package.payload.expires_at)
        .bind(actor.user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO workflow_debug_runs(id,tenant_id,workflow_id,draft_revision,work_package_id,status,expires_at,created_by) VALUES(?,?,?,?,?,'building',?,?)")
        .bind(debug_run_id)
        .bind(actor.tenant_id)
        .bind(workflow_id)
        .bind(source_revision)
        .bind(package.payload.package_id)
        .bind(package.payload.expires_at)
        .bind(actor.user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn mark_failed(pool: &MySqlPool, tenant_id: Uuid, package_id: Uuid) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE runtime_work_package_publications SET status='failed' WHERE tenant_id=? AND package_id=? AND status IN ('building','prepared')")
        .bind(tenant_id)
        .bind(package_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE workflow_debug_runs SET status='failed',completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND work_package_id=? AND status IN ('building','prepared')")
        .bind(tenant_id)
        .bind(package_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE evaluation_runs SET status='failed',completed_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND work_package_id=? AND status IN ('queued','running')")
        .bind(tenant_id)
        .bind(package_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn start_evaluation(
    state: &ControlApiState,
    actor: &Actor,
    run_id: Uuid,
) -> ApiResult<()> {
    let row = sqlx::query("SELECT r.workflow_version_id,r.dataset_version_id,r.evaluation_profile_version_id,r.parameters_json,r.status,v.workflow_id,v.version_number workflow_version_number,v.definition_json,i.id service_identity_id,i.version identity_version FROM evaluation_runs r JOIN workflow_versions v ON v.tenant_id=r.tenant_id AND v.id=r.workflow_version_id JOIN workflow_service_identities i ON i.tenant_id=v.tenant_id AND i.workflow_id=v.workflow_id WHERE r.tenant_id=? AND r.id=?")
        .bind(actor.tenant_id).bind(run_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Evaluation Run"))?;
    if row.try_get::<String, _>("status")? != "created" {
        return Err(ApiError::conflict(
            "EVALUATION_ALREADY_STARTED",
            "Evaluation Run has already been started",
        ));
    }
    let workflow_id: Uuid = row.try_get("workflow_id")?;
    let workflow_version_id: Uuid = row.try_get("workflow_version_id")?;
    let dataset_version_id: Uuid = row.try_get("dataset_version_id")?;
    let profile_version_id: Uuid = row.try_get("evaluation_profile_version_id")?;
    let case_rows=sqlx::query("SELECT source_case_id,input_json,expected_output_json FROM dataset_version_cases WHERE tenant_id=? AND dataset_version_id=? ORDER BY sort_order,source_case_id")
        .bind(actor.tenant_id).bind(dataset_version_id).fetch_all(&state.pool).await?;
    if case_rows.is_empty() {
        return Err(ApiError::unprocessable(
            "EVALUATION_DATASET_EMPTY",
            "Dataset Version has no Cases",
        ));
    }
    let cases = case_rows
        .into_iter()
        .map(|case| {
            Ok(RuntimeEvaluationCaseV1 {
                case_id: case.try_get("source_case_id")?,
                input: case.try_get("input_json")?,
                expected_output: case.try_get("expected_output_json")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let rule_rows=sqlx::query("SELECT id,evaluator_type,configuration_json FROM evaluation_profile_rules WHERE tenant_id=? AND profile_version_id=? ORDER BY sort_order,id")
        .bind(actor.tenant_id).bind(profile_version_id).fetch_all(&state.pool).await?;
    let service_identity_id: Uuid = row.try_get("service_identity_id")?;
    let definition: WorkflowDefinition = serde_json::from_value(row.try_get("definition_json")?)
        .map_err(|error| {
            ApiError::unprocessable("INVALID_WORKFLOW_DEFINITION", error.to_string())
        })?;
    let (dependencies, mut resources, mut pending_objects) =
        load_published_version_closure(state, actor.tenant_id, workflow_version_id, &definition)
            .await?;
    crate::canvas_plugin_api::require_definition_plugin_closure_enabled(
        state,
        actor.tenant_id,
        &definition,
        &dependencies,
    )
    .await?;
    let mut evaluators = Vec::new();
    for rule in rule_rows {
        let kind: String = rule.try_get("evaluator_type")?;
        let configuration: Value = rule.try_get("configuration_json")?;
        let evaluator_id: Uuid = rule.try_get("id")?;
        match kind.as_str() {
            "llm_judge" => {
                let model_id = configuration
                    .get("modelId")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or_else(|| {
                        ApiError::unprocessable(
                            "INVALID_MODEL_EVALUATOR",
                            "Model evaluator requires modelId",
                        )
                    })?;
                let prompt = configuration
                    .get("prompt")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty() && value.len() <= 64 * 1024)
                    .ok_or_else(|| {
                        ApiError::unprocessable(
                            "INVALID_MODEL_EVALUATOR",
                            "Model evaluator requires a prompt of at most 64 KiB",
                        )
                    })?;
                require_model_grant(&state.pool, actor.tenant_id, service_identity_id, model_id)
                    .await?;
                let binding = load_model_binding(state, actor.tenant_id, model_id).await?;
                if !resources.iter().any(|item: &RuntimeResourceBindingV1| {
                    item.resource_kind == RuntimeResourceKindV1::Model
                        && item.resource_id == model_id
                }) {
                    resources.push(binding);
                }
                let content =
                    serde_json::to_vec(&json!({"prompt":prompt})).map_err(ApiError::internal)?;
                let content_hash =
                    ContentHash::parse(format!("sha256:{:x}", Sha256::digest(&content)))
                        .map_err(ApiError::internal)?;
                let reference = RuntimeObjectReferenceV1 {
                    tenant_id: actor.tenant_id,
                    storage_domain: StorageDomain::Runtime,
                    object_id: evaluator_id,
                    object_key: RuntimeObjectReferenceV1::canonical_key(
                        actor.tenant_id,
                        evaluator_id,
                        &content_hash,
                    ),
                    content_hash: content_hash.clone(),
                    size_bytes: content.len() as u64,
                    media_type: "application/json".into(),
                };
                pending_objects.push(PendingObject {
                    metadata: RuntimeObjectUploadMetadataV1 {
                        api_version: 1,
                        idempotency_key: runtime_object_upload_key(evaluator_id, &content_hash),
                        tenant_id: actor.tenant_id,
                        object_id: evaluator_id,
                        content_hash,
                        size_bytes: content.len() as u64,
                        media_type: "application/json".into(),
                    },
                    reference,
                    content,
                });
                evaluators.push(RuntimeEvaluatorV1::Model {
                    evaluator_id,
                    resource_id: model_id,
                    prompt_object_id: evaluator_id,
                });
            }
            "custom_code" => {
                return Err(ApiError::unprocessable(
                    "CUSTOM_EVALUATOR_UNSUPPORTED",
                    "V2 Evaluation supports deterministic rules and Model evaluators",
                ));
            }
            _ => evaluators.push(RuntimeEvaluatorV1::DeterministicRule {
                evaluator_id,
                expression: json!({"type":kind,"configuration":configuration}).to_string(),
            }),
        }
    }
    if evaluators.is_empty() {
        return Err(ApiError::unprocessable(
            "EVALUATOR_REQUIRED",
            "Evaluation Profile has no evaluators",
        ));
    }
    let (grant_ids, grant_bindings) = crate::runtime_resource_binding::authorization_grants(
        &state.pool,
        actor.tenant_id,
        service_identity_id,
    )
    .await?;
    let package_id = Uuid::now_v7();
    let created_at = OffsetDateTime::now_utc();
    let expires_at = created_at + time::Duration::hours(24);
    let plugin_manifests =
        crate::canvas_plugin_api::plugin_manifests_for_tenant(state, actor.tenant_id).await?;
    let plugin_registry =
        node_registry_with_plugins(&dependencies, &plugin_manifests).map_err(ApiError::internal)?;
    let resolved = crate::canvas_plugin_api::resolve_workflow_plugin_closure(
        state,
        actor.tenant_id,
        &definition,
        &dependencies,
        &plugin_registry,
    )
    .await?;
    require_complete_plugin_definitions(&resolved)?;
    add_composite_closure(
        &state.pool,
        actor.tenant_id,
        &dependencies,
        &plugin_manifests,
        &resolved.dependency_manifests,
        &mut resources,
        &mut pending_objects,
    )
    .await?;
    add_plugin_runtime_objects(actor.tenant_id, &resolved, &mut pending_objects)?;
    let package = build_work_package_with_resolved(
        WorkPackageBuildSource {
            package_id,
            tenant_id: actor.tenant_id,
            workflow: workflow_snapshot(
                &state.pool,
                actor.tenant_id,
                workflow_id,
                row.try_get("workflow_version_id")?,
                row.try_get("workflow_version_number")?,
            )
            .await?,
            origin: execution_origin(state, actor).await?,
            purpose: WorkPackagePurpose::Evaluation,
            call_purpose: RuntimeCallPurposeV1::Evaluation,
            spec: RuntimeWorkPackageSpecV1::Evaluation {
                dataset_version_id,
                profile_version_id,
                cases,
                evaluators,
            },
            source_revision: format!(
                "evaluation:{run_id}:{}",
                row.try_get::<Uuid, _>("workflow_version_id")?
            ),
            definition,
            dependency_versions: dependencies,
            plugin_manifests,
            supported_capabilities: agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
                .iter()
                .map(ToString::to_string)
                .collect(),
            overlay: RuntimeWorkPackageOverlayV1 {
                input: row.try_get("parameters_json")?,
                ..Default::default()
            },
            resources,
            authorization: RuntimeAuthorizationSnapshotV1 {
                schema_version: 1,
                tenant_id: actor.tenant_id,
                service_identity_id,
                workflow_id,
                policy_epoch: row.try_get("identity_version")?,
                grant_ids,
                grant_bindings,
                capabilities: agentx_node_protocol::ALL_RUNTIME_CAPABILITIES
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                maximum_policy_staleness_seconds: 72 * 60 * 60,
                captured_at: created_at,
            },
            objects: pending_objects
                .iter()
                .map(|object| object.reference.clone())
                .collect(),
            runtime_policy: RuntimePolicyV1::default(),
            created_at,
            expires_at,
        },
        &resolved.manifests,
        &resolved.dependency_manifests,
        &state.work_packages.signing_kid,
        &state.work_packages.signing_key,
    )
    .map_err(|error| ApiError::unprocessable("WORK_PACKAGE_BUILD_FAILED", error.to_string()))?;
    let request_hash =
        agentx_runtime_contracts::content_hash(&package).map_err(ApiError::internal)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO runtime_work_package_publications(id,tenant_id,package_id,purpose,source_type,source_id,source_revision,content_hash,signature_key_id,package_json,object_manifest_json,status,prepare_idempotency_key,request_hash,expires_at,created_by) VALUES(?,?,?,'evaluation','evaluation_run',?,?,?,?,?,?,'building',?,?,?,?)")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(package_id).bind(run_id).bind(&package.payload.source_revision).bind(package.content_hash.as_str()).bind(&package.signature.key_id).bind(serde_json::to_value(&package).map_err(ApiError::internal)?).bind(serde_json::to_value(&package.payload.objects).map_err(ApiError::internal)?).bind(format!("evaluation:prepare:{run_id}")).bind(request_hash.as_str()).bind(expires_at).bind(actor.user_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE evaluation_runs SET work_package_id=?,status='queued',started_at=UTC_TIMESTAMP(6),total_cases=? WHERE tenant_id=? AND id=? AND status='created'").bind(package_id).bind(package.payload.spec.evaluation_case_count()).bind(actor.tenant_id).bind(run_id).execute(&mut *tx).await?;
    tx.commit().await?;
    for object in pending_objects {
        let receipt = state
            .work_packages
            .upload_object(&object.metadata, object.content)
            .await;
        match receipt {
            Ok(receipt)
                if receipt.object.object_id == object.reference.object_id
                    && receipt.object.content_hash == object.reference.content_hash => {}
            Ok(_) => {
                mark_failed(&state.pool, actor.tenant_id, package_id).await?;
                return Err(ApiError::conflict(
                    "RUNTIME_OBJECT_RECEIPT_MISMATCH",
                    "Runtime returned a different immutable evaluator object",
                ));
            }
            Err(error) => {
                mark_failed(&state.pool, actor.tenant_id, package_id).await?;
                return Err(error);
            }
        }
    }
    let prepare: agentx_runtime_contracts::PublishReceiptV1 = state
        .work_packages
        .post(
            "runtime.work-packages.prepare",
            "/internal/runtime/v1/work-packages:prepare",
            &PrepareWorkPackageRequestV1 {
                api_version: 1,
                idempotency_key: format!("evaluation:prepare:{run_id}"),
                work_package: package,
            },
        )
        .await?;
    if prepare.status != PublishReceiptStatusV1::Accepted {
        mark_failed(&state.pool, actor.tenant_id, package_id).await?;
        return Err(ApiError::conflict(
            "WORK_PACKAGE_PREPARE_REJECTED",
            "Runtime rejected the Evaluation Work Package",
        ));
    }
    let execute: agentx_runtime_contracts::ApplyReceiptV1 = state
        .work_packages
        .post(
            "runtime.work-packages.execute",
            &format!("/internal/runtime/v1/work-packages/{package_id}:execute"),
            &ExecuteWorkPackageRequestV1 {
                api_version: 1,
                idempotency_key: format!("evaluation:execute:{run_id}"),
                package_id,
                input: Value::Null,
            },
        )
        .await?;
    sqlx::query("UPDATE runtime_work_package_publications SET status='running',runtime_version=?,prepare_receipt_json=? WHERE tenant_id=? AND package_id=?").bind(execute.object_version).bind(serde_json::to_value(&prepare).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(package_id).execute(&state.pool).await?;
    sqlx::query("UPDATE evaluation_runs SET runtime_command_id=?,runtime_receipt_json=?,status='running' WHERE tenant_id=? AND id=?").bind(execute.event_id).bind(serde_json::to_value(execute).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(run_id).execute(&state.pool).await?;
    Ok(())
}

pub(crate) async fn cancel_evaluation(
    state: &ControlApiState,
    actor: &Actor,
    run_id: Uuid,
) -> ApiResult<()> {
    let row=sqlx::query("SELECT work_package_id,intent_version,status FROM evaluation_runs WHERE tenant_id=? AND id=?").bind(actor.tenant_id).bind(run_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Evaluation Run"))?;
    let package_id: Uuid = row
        .try_get::<Option<Uuid>, _>("work_package_id")?
        .ok_or_else(|| {
            ApiError::conflict(
                "EVALUATION_NOT_STARTED",
                "Evaluation has no Runtime Work Package",
            )
        })?;
    if matches!(
        row.try_get::<String, _>("status")?.as_str(),
        "completed" | "failed" | "cancelled"
    ) {
        return Ok(());
    }
    let expected:u64=sqlx::query_scalar("SELECT runtime_version FROM runtime_work_package_publications WHERE tenant_id=? AND package_id=?").bind(actor.tenant_id).bind(package_id).fetch_one(&state.pool).await?;
    let receipt: agentx_runtime_contracts::ApplyReceiptV1 = state
        .work_packages
        .post(
            "runtime.work-packages.cancel",
            &format!("/internal/runtime/v1/work-packages/{package_id}:cancel"),
            &CancelWorkPackageRequestV1 {
                api_version: 1,
                tenant_id: actor.tenant_id,
                package_id,
                expected_version: expected,
                idempotency_key: format!("evaluation:cancel:{run_id}"),
            },
        )
        .await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE runtime_work_package_publications SET status='cancelled',runtime_version=?,cancel_idempotency_key=?,cancel_receipt_json=? WHERE tenant_id=? AND package_id=?").bind(receipt.object_version).bind(format!("evaluation:cancel:{run_id}")).bind(serde_json::to_value(&receipt).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(package_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE evaluation_runs SET status='cancelled',completed_at=UTC_TIMESTAMP(6),intent_version=intent_version+1,runtime_receipt_json=? WHERE tenant_id=? AND id=?").bind(serde_json::to_value(receipt).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(run_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

trait EvaluationSpecCount {
    fn evaluation_case_count(&self) -> u64;
}
impl EvaluationSpecCount for RuntimeWorkPackageSpecV1 {
    fn evaluation_case_count(&self) -> u64 {
        match self {
            RuntimeWorkPackageSpecV1::Evaluation { cases, .. } => cases.len() as u64,
            RuntimeWorkPackageSpecV1::Debug { .. } => 0,
        }
    }
}

fn build_debug_plan(
    compiled: &agentx_runtime_contracts::CompiledWorkflowV1,
    mode: PartialExecutionModeV1,
    target_node_id: Option<String>,
    input_source: Option<RuntimeDebugInputSourceV1>,
    side_effect_decisions: BTreeMap<String, SideEffectResolutionV1>,
) -> ApiResult<RuntimeDebugPlanV1> {
    let included_indexes = match (mode, target_node_id.as_deref()) {
        (PartialExecutionModeV1::Whole, None) => (0..compiled.nodes.len()).collect(),
        (PartialExecutionModeV1::Whole, Some(_)) | (_, None) => {
            return Err(ApiError::unprocessable(
                "DEBUG_PLAN_INVALID",
                "Partial Debug execution requires exactly one target node",
            ));
        }
        (mode, Some(target)) => {
            let selected = compiled
                .nodes
                .iter()
                .position(|node| node.id == target)
                .ok_or_else(|| {
                    ApiError::unprocessable(
                        "DEBUG_TARGET_NOT_FOUND",
                        "Debug target is not an enabled Workflow node",
                    )
                })?;
            let mut included = BTreeSet::from([selected]);
            let mut frontier = std::collections::VecDeque::from([selected]);
            while let Some(node) = frontier.pop_front() {
                let connections = match mode {
                    PartialExecutionModeV1::Node => continue,
                    PartialExecutionModeV1::ToNode => &compiled.nodes[node].incoming_connections,
                    PartialExecutionModeV1::FromNode => &compiled.nodes[node].outgoing_connections,
                    PartialExecutionModeV1::Whole => unreachable!(),
                };
                for connection in connections {
                    let connection = &compiled.connections[*connection];
                    let next = if mode == PartialExecutionModeV1::ToNode {
                        connection.source_node
                    } else {
                        connection.target_node
                    };
                    if included.insert(next) {
                        frontier.push_back(next);
                    }
                }
            }
            included
        }
    };
    let requires_source = matches!(
        mode,
        PartialExecutionModeV1::Node | PartialExecutionModeV1::FromNode
    );
    if requires_source != input_source.is_some() {
        return Err(ApiError::unprocessable(
            "DEBUG_INPUT_SOURCE_INVALID",
            "Only node and from_node Debug modes require an explicit input source",
        ));
    }
    if side_effect_decisions
        .keys()
        .any(|node_id| !compiled.nodes.iter().any(|node| &node.id == node_id))
    {
        return Err(ApiError::unprocessable(
            "DEBUG_SIDE_EFFECT_NODE_INVALID",
            "Debug side-effect decisions must reference an enabled Workflow node",
        ));
    }
    for index in &included_indexes {
        let node = &compiled.nodes[*index];
        if node.side_effect_level == agentx_node_protocol::SideEffectLevel::Irreversible
            && !side_effect_decisions.contains_key(&node.id)
        {
            return Err(ApiError::unprocessable(
                "DEBUG_SIDE_EFFECT_DECISION_REQUIRED",
                format!(
                    "Irreversible Debug node {} requires an explicit decision",
                    node.id
                ),
            ));
        }
    }
    let included_node_ids = compiled
        .nodes
        .iter()
        .enumerate()
        .filter(|(index, _)| included_indexes.contains(index))
        .map(|(_, node)| node.id.clone())
        .collect();
    let skipped_node_ids = compiled
        .nodes
        .iter()
        .enumerate()
        .filter(|(index, _)| !included_indexes.contains(index))
        .map(|(_, node)| node.id.clone())
        .collect();
    Ok(RuntimeDebugPlanV1 {
        mode,
        target_node_id,
        included_node_ids,
        skipped_node_ids,
        input_source,
        side_effect_decisions,
    })
}

fn build_whole_execution_plan(
    compiled: &agentx_runtime_contracts::CompiledWorkflowV1,
) -> ApiResult<RuntimeDebugPlanV1> {
    let side_effect_decisions = compiled
        .nodes
        .iter()
        .filter(|node| {
            node.side_effect_level == agentx_node_protocol::SideEffectLevel::Irreversible
        })
        .map(|node| (node.id.clone(), SideEffectResolutionV1::Execute))
        .collect();
    build_debug_plan(
        compiled,
        PartialExecutionModeV1::Whole,
        None,
        None,
        side_effect_decisions,
    )
}

#[cfg(test)]
#[path = "work_packages_tests.rs"]
mod debug_plan_tests;
