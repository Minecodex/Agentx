use agentx_runtime_contracts::{
    ActivateDeploymentRequestV1, AdmissionStatusV1, AdmissionTargetV1, ApplyChatMappingReceiptV1,
    ApplyChatMappingRequestV1, ApplyReceiptV1, DisableDeploymentRequestV1, PrepareBundleRequestV1,
    PublishReceiptStatusV1, PublishReceiptV1, RollbackDeploymentRequestV1,
    RuntimeAdmissionCommandV1, RuntimePublishErrorCodeV1,
};
use axum::{Json, extract::State, http::HeaderMap};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

pub async fn apply_chat_mapping(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<ApplyChatMappingRequestV1>,
) -> RuntimeResult<Json<ApplyChatMappingReceiptV1>> {
    state
        .trust
        .publisher(&headers, "runtime.chat_mappings.apply")?;
    let expected_hash = agentx_runtime_contracts::content_hash(&request.mapping)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    if expected_hash != request.content_hash {
        return Err(RuntimeError::BadRequest(
            RuntimePublishErrorCodeV1::ContentHashMismatch,
            "Chat Mapping Content Hash does not match".into(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    let bundle = sqlx::query("SELECT application_id,deployment_id FROM deployment_bundles WHERE tenant_id=? AND id=? AND status IN ('active','superseded','retained') FOR SHARE")
        .bind(request.tenant_id)
        .bind(request.bundle_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RuntimeError::NotFound)?;
    if bundle.try_get::<Uuid, _>("application_id")? != request.application_id
        || bundle.try_get::<Uuid, _>("deployment_id")? != request.deployment_id
    {
        return Err(RuntimeError::Unauthorized);
    }
    let existing = sqlx::query("SELECT version,content_hash FROM application_chat_mappings WHERE tenant_id=? AND deployment_id=? FOR UPDATE")
        .bind(request.tenant_id)
        .bind(request.deployment_id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(existing) = existing {
        let version: u64 = existing.try_get("version")?;
        let content_hash: String = existing.try_get("content_hash")?;
        if version > request.version {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::HeadVersionConflict,
                "A newer Chat Mapping is already active".into(),
            ));
        }
        if version == request.version {
            if content_hash != request.content_hash.as_str() {
                return Err(RuntimeError::Conflict(
                    RuntimePublishErrorCodeV1::IdempotencyConflict,
                    "Chat Mapping Version was reused with different content".into(),
                ));
            }
            tx.commit().await?;
            return Ok(Json(ApplyChatMappingReceiptV1 {
                api_version: 1,
                deployment_id: request.deployment_id,
                bundle_id: request.bundle_id,
                version: request.version,
                replayed: true,
            }));
        }
    }
    sqlx::query("INSERT INTO application_chat_mappings(tenant_id,application_id,deployment_id,bundle_id,version,mapping_json,content_hash) VALUES(?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE application_id=VALUES(application_id),bundle_id=VALUES(bundle_id),version=VALUES(version),mapping_json=VALUES(mapping_json),content_hash=VALUES(content_hash)")
        .bind(request.tenant_id)
        .bind(request.application_id)
        .bind(request.deployment_id)
        .bind(request.bundle_id)
        .bind(request.version)
        .bind(serde_json::to_value(&request.mapping).map_err(|error| RuntimeError::Internal(error.into()))?)
        .bind(request.content_hash.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(ApplyChatMappingReceiptV1 {
        api_version: 1,
        deployment_id: request.deployment_id,
        bundle_id: request.bundle_id,
        version: request.version,
        replayed: false,
    }))
}

pub async fn prepare_bundle(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<PrepareBundleRequestV1>,
) -> RuntimeResult<Json<PublishReceiptV1>> {
    state.trust.publisher(&headers, "runtime.bundles.prepare")?;
    if let Some(receipt) = replay::<_, PublishReceiptV1>(
        &state,
        request.bundle.payload.tenant_id,
        "prepare",
        &request.idempotency_key,
        &request,
    )
    .await?
    {
        return Ok(Json(receipt));
    }
    let tenant_id = request.bundle.payload.tenant_id;
    let bundle_id = request.bundle.payload.bundle_id;
    let result = prepare_bundle_inner(&state, &request).await;
    match result {
        Ok(receipt) => Ok(Json(receipt)),
        Err(RuntimeError::BadRequest(code, message) | RuntimeError::Conflict(code, message)) => {
            let receipt = rejected_receipt(bundle_id, code, &message);
            persist_receipt(
                &state,
                tenant_id,
                "prepare",
                &request.idempotency_key,
                &request,
                Some(bundle_id),
                Some(request.bundle.payload.application_id),
                Some(request.bundle.payload.deployment_id),
                "rejected",
                Some(code_string(code)),
                &receipt,
            )
            .await?;
            Ok(Json(receipt))
        }
        Err(error) => Err(error),
    }
}

async fn prepare_bundle_inner(
    state: &RuntimeState,
    request: &PrepareBundleRequestV1,
) -> RuntimeResult<PublishReceiptV1> {
    let bundle = &request.bundle;
    let key = state.trust.bundle_key(&bundle.signature.key_id)?;
    bundle.verify(key).map_err(|error| {
        let code = if matches!(
            error,
            agentx_runtime_contracts::ContractError::ContentHashMismatch
        ) {
            RuntimePublishErrorCodeV1::ContentHashMismatch
        } else {
            RuntimePublishErrorCodeV1::InvalidSignature
        };
        RuntimeError::BadRequest(code, error.to_string())
    })?;
    if bundle.payload.compiled_ir.contract_version != 1 {
        return Err(RuntimeError::BadRequest(
            RuntimePublishErrorCodeV1::UnsupportedIrVersion,
            "only IR v1 is supported".into(),
        ));
    }
    if bundle
        .payload
        .worker_compatibility
        .capabilities
        .iter()
        .any(|value| !agentx_node_protocol::ALL_RUNTIME_CAPABILITIES.contains(&value.as_str()))
    {
        return Err(RuntimeError::BadRequest(
            RuntimePublishErrorCodeV1::UnsupportedCapability,
            "Runtime Bundle requires an unsupported Worker or Trigger capability".into(),
        ));
    }
    for object in &bundle.payload.objects {
        if object.tenant_id != bundle.payload.tenant_id || !object.has_canonical_key() {
            return Err(RuntimeError::BadRequest(
                RuntimePublishErrorCodeV1::TenantMismatch,
                "Bundle object key or tenant is not canonical".into(),
            ));
        }
        let row = sqlx::query("SELECT CAST(object_key AS CHAR CHARACTER SET utf8mb4) AS object_key,content_hash,size_bytes,media_type FROM runtime_objects WHERE tenant_id=? AND object_id=? AND status='ready'")
            .bind(object.tenant_id).bind(object.object_id).fetch_optional(&state.pool).await?
            .ok_or_else(|| RuntimeError::BadRequest(RuntimePublishErrorCodeV1::ObjectMissing, format!("Runtime object {} is missing", object.object_id)))?;
        if row.try_get::<String, _>("object_key")? != object.object_key
            || row.try_get::<String, _>("content_hash")? != object.content_hash.as_str()
            || row.try_get::<u64, _>("size_bytes")? != object.size_bytes
            || row.try_get::<String, _>("media_type")? != object.media_type
        {
            return Err(RuntimeError::BadRequest(
                RuntimePublishErrorCodeV1::ObjectHashMismatch,
                format!(
                    "Runtime object {} does not match its manifest",
                    object.object_id
                ),
            ));
        }
    }
    let composites = crate::composite::materialize(
        state,
        bundle.payload.tenant_id,
        &bundle.payload.resources,
        &bundle.payload.objects,
    )
    .await?;
    let payload = serde_json::to_value(&bundle.payload)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let manifest = serde_json::to_value(&bundle.payload.objects)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let signature = STANDARD
        .decode(&bundle.signature.signature_base64)
        .map_err(|_| {
            RuntimeError::BadRequest(
                RuntimePublishErrorCodeV1::InvalidSignature,
                "signature is not base64".into(),
            )
        })?;
    let mut tx = state.pool.begin().await?;
    let existing = sqlx::query("SELECT tenant_id,application_id,deployment_id,content_hash FROM deployment_bundles WHERE id=? FOR UPDATE")
        .bind(bundle.payload.bundle_id).fetch_optional(&mut *tx).await?;
    if let Some(existing) = existing {
        if existing.try_get::<Uuid, _>("tenant_id")? != bundle.payload.tenant_id
            || existing.try_get::<Uuid, _>("application_id")? != bundle.payload.application_id
            || existing.try_get::<Uuid, _>("deployment_id")? != bundle.payload.deployment_id
            || existing.try_get::<String, _>("content_hash")? != bundle.content_hash.as_str()
        {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::IdempotencyConflict,
                "Bundle ID is already bound to different immutable content".into(),
            ));
        }
    } else {
        sqlx::query("INSERT INTO deployment_bundles(id,tenant_id,application_id,deployment_id,workflow_id,workflow_version_id,sequence_number,schema_version,content_hash,signature_key_id,signature,payload_json,object_manifest_json,status,prepared_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,'prepared',UTC_TIMESTAMP(6))")
        .bind(bundle.payload.bundle_id).bind(bundle.payload.tenant_id).bind(bundle.payload.application_id)
        .bind(bundle.payload.deployment_id).bind(bundle.payload.workflow_id).bind(bundle.payload.workflow_version_id)
        .bind(bundle.payload.bundle_sequence).bind(bundle.payload.schema_version).bind(bundle.content_hash.as_str())
        .bind(&bundle.signature.key_id).bind(signature).bind(payload).bind(manifest).execute(&mut *tx).await?;
    }
    for (ordinal, object) in bundle.payload.objects.iter().enumerate() {
        let locked = sqlx::query("SELECT content_hash,size_bytes,media_type FROM runtime_objects WHERE tenant_id=? AND object_id=? AND status='ready' FOR UPDATE")
            .bind(object.tenant_id)
            .bind(object.object_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::ObjectMissing,
                format!("Runtime object {} became unavailable during Prepare", object.object_id),
            ))?;
        if locked.try_get::<String, _>("content_hash")? != object.content_hash.as_str()
            || locked.try_get::<u64, _>("size_bytes")? != object.size_bytes
            || locked.try_get::<String, _>("media_type")? != object.media_type
        {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::ObjectHashMismatch,
                format!("Runtime object {} changed during Prepare", object.object_id),
            ));
        }
        sqlx::query("INSERT INTO bundle_objects(tenant_id,bundle_id,object_id,content_hash,size_bytes,media_type,ordinal) VALUES(?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE object_id=object_id")
            .bind(bundle.payload.tenant_id).bind(bundle.payload.bundle_id).bind(object.object_id)
            .bind(object.content_hash.as_str()).bind(object.size_bytes).bind(&object.media_type).bind(ordinal as u32)
            .execute(&mut *tx).await?;
    }
    for resource in &bundle.payload.resources {
        let resource_kind = serde_json::to_value(resource.resource_kind)
            .map_err(|error| RuntimeError::Internal(error.into()))?
            .as_str()
            .ok_or_else(|| {
                RuntimeError::Internal(anyhow::anyhow!("resource kind is not a string"))
            })?
            .to_owned();
        let binding_id = stable_binding_id(
            bundle.payload.bundle_id,
            &resource_kind,
            resource.resource_id,
            &resource.resource_version,
        );
        sqlx::query(
            "INSERT INTO runtime_resource_bindings(tenant_id,binding_id,bundle_id,work_package_id,resource_kind,resource_id,resource_version,state_epoch,content_hash,configuration_json,object_ids_json) VALUES(?,?,?,NULL,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE binding_id=binding_id",
        )
        .bind(bundle.payload.tenant_id)
        .bind(binding_id)
        .bind(bundle.payload.bundle_id)
        .bind(&resource_kind)
        .bind(resource.resource_id)
        .bind(&resource.resource_version)
        .bind(resource.state_epoch)
        .bind(resource.content_hash.as_str())
        .bind(serde_json::to_value(&resource.configuration).map_err(|error| RuntimeError::Internal(error.into()))?)
        .bind(serde_json::to_value(&resource.object_ids).map_err(|error| RuntimeError::Internal(error.into()))?)
        .execute(&mut *tx)
        .await?;
        if let agentx_runtime_contracts::RuntimeResourceConfigurationV1::Composite {
            workflow, ..
        } = &resource.configuration
            && let Some(snapshot) = composites.get(&workflow.version_id)
        {
            crate::composite::persist(
                &mut tx,
                bundle.payload.tenant_id,
                Some(bundle.payload.bundle_id),
                None,
                binding_id,
                snapshot,
            )
            .await?;
        }
        sqlx::query(
            "INSERT IGNORE INTO runtime_resource_states(tenant_id,resource_kind,resource_id,resource_version,state_epoch,status,content_hash) VALUES(?,?,?,?,?,'active',?)",
        )
        .bind(bundle.payload.tenant_id)
        .bind(&resource_kind)
        .bind(resource.resource_id)
        .bind(&resource.resource_version)
        .bind(resource.state_epoch)
        .bind(resource.content_hash.as_str())
        .execute(&mut *tx)
        .await?;
    }
    let receipt = accepted_receipt(bundle.payload.bundle_id, None, None, false);
    persist_receipt_tx(
        &mut tx,
        bundle.payload.tenant_id,
        "prepare",
        &request.idempotency_key,
        request,
        Some(bundle.payload.bundle_id),
        Some(bundle.payload.application_id),
        Some(bundle.payload.deployment_id),
        "accepted",
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(receipt)
}

fn stable_binding_id(
    bundle_id: Uuid,
    resource_kind: &str,
    resource_id: Uuid,
    resource_version: &str,
) -> Uuid {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(bundle_id.as_bytes());
    hasher.update(resource_kind.as_bytes());
    hasher.update(resource_id.as_bytes());
    hasher.update(resource_version.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

pub async fn apply_admission(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<RuntimeAdmissionCommandV1>,
) -> RuntimeResult<Json<ApplyReceiptV1>> {
    state.trust.publisher(&headers, "runtime.admission.apply")?;
    let tenant_id = request.command.tenant_id;
    if let Some(mut receipt) = replay::<_, ApplyReceiptV1>(
        &state,
        tenant_id,
        "admission",
        &request.command.idempotency_key,
        &request,
    )
    .await?
    {
        receipt.replayed = true;
        return Ok(Json(receipt));
    }
    let expected_hash = agentx_runtime_contracts::content_hash(&request.target)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    if request.command.content_hash != expected_hash {
        return rejected_admission(
            &state,
            &request,
            RuntimePublishErrorCodeV1::ContentHashMismatch,
            "Admission target Content Hash does not match",
        )
        .await
        .map(Json);
    }
    let result = apply_admission_inner(&state, &request).await;
    match result {
        Ok(receipt) => Ok(Json(receipt)),
        Err(RuntimeError::BadRequest(code, message) | RuntimeError::Conflict(code, message)) => {
            rejected_admission(&state, &request, code, &message)
                .await
                .map(Json)
        }
        Err(error) => Err(error),
    }
}

async fn apply_admission_inner(
    state: &RuntimeState,
    request: &RuntimeAdmissionCommandV1,
) -> RuntimeResult<ApplyReceiptV1> {
    let tenant_id = request.command.tenant_id;
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO tenant_admission(tenant_id,status,admission_epoch,policy_version) VALUES(?,'active',?,1) ON DUPLICATE KEY UPDATE admission_epoch=GREATEST(admission_epoch,VALUES(admission_epoch))")
        .bind(tenant_id).bind(request.admission_epoch).execute(&mut *tx).await?;
    // The tenant row serializes synchronous publish barriers with the outbox
    // publisher. Recheck after taking that lock so concurrent delivery of the
    // same command converges to the committed receipt instead of a duplicate-key
    // database error.
    if let Some(mut receipt) = replay_tx::<_, ApplyReceiptV1>(
        &mut tx,
        tenant_id,
        "admission",
        &request.command.idempotency_key,
        request,
    )
    .await?
    {
        receipt.replayed = true;
        tx.commit().await?;
        return Ok(receipt);
    }
    match &request.target {
        AdmissionTargetV1::Tenant { enabled } => {
            sqlx::query("UPDATE tenant_admission SET status=?,admission_epoch=? WHERE tenant_id=? AND admission_epoch<=?")
                .bind(if *enabled { "active" } else { "disabled" }).bind(request.admission_epoch).bind(tenant_id).bind(request.admission_epoch).execute(&mut *tx).await?;
        }
        AdmissionTargetV1::ApplicationRoute { state: route } => {
            ensure_tenant(tenant_id, route.tenant_id)?;
            sqlx::query("INSERT INTO application_routes(application_id,tenant_id,route_key,active_bundle_id,admission_epoch,status) VALUES(?,?,?,NULL,?,?) ON DUPLICATE KEY UPDATE route_key=IF(admission_epoch<=VALUES(admission_epoch),VALUES(route_key),route_key),status=IF(admission_epoch<=VALUES(admission_epoch),VALUES(status),status),admission_epoch=GREATEST(admission_epoch,VALUES(admission_epoch))")
                .bind(route.application_id).bind(route.tenant_id).bind(&route.route_key).bind(request.admission_epoch).bind(route_status(route.status)).execute(&mut *tx).await?;
        }
        AdmissionTargetV1::ApiKey { state: key } => {
            ensure_tenant(tenant_id, key.tenant_id)?;
            let hash = parse_sha256(&key.secret_hash)?;
            sqlx::query("INSERT INTO api_key_admission(key_id,tenant_id,application_id,key_prefix,key_name,secret_hash,admission_epoch,status,expires_at) VALUES(?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE key_prefix=IF(admission_epoch<=VALUES(admission_epoch),VALUES(key_prefix),key_prefix),key_name=IF(admission_epoch<=VALUES(admission_epoch),VALUES(key_name),key_name),secret_hash=IF(admission_epoch<=VALUES(admission_epoch),VALUES(secret_hash),secret_hash),status=IF(admission_epoch<=VALUES(admission_epoch),VALUES(status),status),expires_at=IF(admission_epoch<=VALUES(admission_epoch),VALUES(expires_at),expires_at),admission_epoch=GREATEST(admission_epoch,VALUES(admission_epoch))")
                .bind(key.key_id).bind(key.tenant_id).bind(key.application_id).bind(&key.key_prefix).bind(&key.key_name).bind(hash).bind(request.admission_epoch).bind(if key.status == AdmissionStatusV1::Active { "active" } else { "revoked" }).bind(key.expires_at).execute(&mut *tx).await?;
        }
        AdmissionTargetV1::ServiceIdentity { state: identity } => {
            ensure_tenant(tenant_id, identity.tenant_id)?;
            sqlx::query("INSERT INTO service_identity_projection(identity_id,tenant_id,workflow_id,policy_epoch,status,capabilities_json) VALUES(?,?,?,?,?,?) ON DUPLICATE KEY UPDATE policy_epoch=GREATEST(policy_epoch,VALUES(policy_epoch)),status=IF(policy_epoch<=VALUES(policy_epoch),VALUES(status),status),capabilities_json=IF(policy_epoch<=VALUES(policy_epoch),VALUES(capabilities_json),capabilities_json)")
                .bind(identity.identity_id).bind(identity.tenant_id).bind(identity.workflow_id).bind(identity.policy_epoch)
                .bind(if identity.status == AdmissionStatusV1::Active { "active" } else { "disabled" }).bind(json!(identity.capabilities)).execute(&mut *tx).await?;
            for grant_id in &identity.grant_ids {
                // Service identity events carry only grant IDs.  Keep a
                // placeholder for a grant that has not arrived yet, but do
                // not overwrite the detailed ResourceGrant projection when
                // events are delivered in the opposite order.
                sqlx::query("INSERT INTO resource_grant_projection(grant_id,tenant_id,subject_id,resource_type,resource_id,operations_json,policy_epoch,status) VALUES(?,?,?,'bundle',?,JSON_ARRAY('use'),?,'active') ON DUPLICATE KEY UPDATE subject_id=IF(resource_type='bundle' AND policy_epoch<=VALUES(policy_epoch),VALUES(subject_id),subject_id),policy_epoch=GREATEST(policy_epoch,VALUES(policy_epoch)),status=IF(resource_type='bundle' AND policy_epoch<=VALUES(policy_epoch),'active',status)")
                    .bind(grant_id).bind(identity.tenant_id).bind(identity.identity_id).bind(grant_id).bind(identity.policy_epoch).execute(&mut *tx).await?;
            }
        }
        AdmissionTargetV1::RuntimeUser { state: user } => {
            ensure_tenant(tenant_id, user.tenant_id)?;
            let role_assignments = serde_json::to_value(&user.role_assignments)
                .map_err(|error| RuntimeError::Internal(error.into()))?;
            sqlx::query("INSERT INTO runtime_user_admission(tenant_id,user_id,user_name,department_id,department_name,token_version,status,tenant_query_enabled,role_assignments_json,admission_epoch) VALUES(?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE user_name=IF(admission_epoch<=VALUES(admission_epoch),VALUES(user_name),user_name),department_id=IF(admission_epoch<=VALUES(admission_epoch),VALUES(department_id),department_id),department_name=IF(admission_epoch<=VALUES(admission_epoch),VALUES(department_name),department_name),token_version=IF(admission_epoch<=VALUES(admission_epoch),VALUES(token_version),token_version),status=IF(admission_epoch<=VALUES(admission_epoch),VALUES(status),status),tenant_query_enabled=IF(admission_epoch<=VALUES(admission_epoch),VALUES(tenant_query_enabled),tenant_query_enabled),role_assignments_json=IF(admission_epoch<=VALUES(admission_epoch),VALUES(role_assignments_json),role_assignments_json),admission_epoch=GREATEST(admission_epoch,VALUES(admission_epoch))")
                .bind(user.tenant_id).bind(user.user_id).bind(&user.user_name).bind(user.department_id).bind(&user.department_name).bind(user.token_version).bind(if user.enabled{"active"}else{"disabled"}).bind(user.tenant_query_enabled).bind(role_assignments).bind(request.admission_epoch).execute(&mut *tx).await?;
        }
        AdmissionTargetV1::RuntimeUserApplicationGrant { state: grant } => {
            ensure_tenant(tenant_id, grant.tenant_id)?;
            sqlx::query("INSERT INTO runtime_user_application_grants(tenant_id,user_id,application_id,grant_version,status,can_invoke,can_query,admission_epoch) VALUES(?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE grant_version=IF(admission_epoch<=VALUES(admission_epoch),VALUES(grant_version),grant_version),status=IF(admission_epoch<=VALUES(admission_epoch),VALUES(status),status),can_invoke=IF(admission_epoch<=VALUES(admission_epoch),VALUES(can_invoke),can_invoke),can_query=IF(admission_epoch<=VALUES(admission_epoch),VALUES(can_query),can_query),admission_epoch=GREATEST(admission_epoch,VALUES(admission_epoch))")
                .bind(grant.tenant_id).bind(grant.user_id).bind(grant.application_id).bind(grant.grant_version).bind(if grant.can_invoke || grant.can_query{"active"}else{"revoked"}).bind(grant.can_invoke).bind(grant.can_query).bind(request.admission_epoch).execute(&mut *tx).await?;
        }
        AdmissionTargetV1::RuntimeUserWorkflowGrant { state: grant } => {
            ensure_tenant(tenant_id, grant.tenant_id)?;
            sqlx::query("INSERT INTO runtime_user_workflow_grants(tenant_id,user_id,workflow_id,grant_version,status,admission_epoch) VALUES(?,?,?,?,?,?) ON DUPLICATE KEY UPDATE grant_version=IF(admission_epoch<=VALUES(admission_epoch),VALUES(grant_version),grant_version),status=IF(admission_epoch<=VALUES(admission_epoch),VALUES(status),status),admission_epoch=GREATEST(admission_epoch,VALUES(admission_epoch))")
                .bind(grant.tenant_id).bind(grant.user_id).bind(grant.workflow_id).bind(grant.grant_version).bind(if grant.can_query{"active"}else{"revoked"}).bind(request.admission_epoch).execute(&mut *tx).await?;
        }
        AdmissionTargetV1::ResourceGrant { state: grant } => {
            ensure_tenant(tenant_id, grant.tenant_id)?;
            sqlx::query("INSERT INTO resource_grant_projection(grant_id,tenant_id,subject_id,resource_type,resource_id,operations_json,policy_epoch,status) VALUES(?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE subject_id=IF(policy_epoch<=VALUES(policy_epoch),VALUES(subject_id),subject_id),resource_type=IF(policy_epoch<=VALUES(policy_epoch),VALUES(resource_type),resource_type),resource_id=IF(policy_epoch<=VALUES(policy_epoch),VALUES(resource_id),resource_id),operations_json=IF(policy_epoch<=VALUES(policy_epoch),VALUES(operations_json),operations_json),status=IF(policy_epoch<=VALUES(policy_epoch),VALUES(status),status),policy_epoch=GREATEST(policy_epoch,VALUES(policy_epoch))")
                .bind(grant.grant_id).bind(grant.tenant_id).bind(grant.identity_id).bind(&grant.resource_type).bind(grant.resource_id).bind(json!(grant.operations)).bind(grant.policy_epoch).bind(if grant.enabled{"active"}else{"revoked"}).execute(&mut *tx).await?;
        }
        AdmissionTargetV1::ResourceState { state: resource } => {
            ensure_tenant(tenant_id, resource.tenant_id)?;
            let resource_kind = serde_json::to_value(resource.resource_kind)
                .map_err(|error| RuntimeError::Internal(error.into()))?
                .as_str()
                .ok_or_else(|| {
                    RuntimeError::Internal(anyhow::anyhow!("resource kind is not a string"))
                })?
                .to_owned();
            let status = match resource.status {
                agentx_runtime_contracts::RuntimeResourceStateStatusV1::Active => "active",
                agentx_runtime_contracts::RuntimeResourceStateStatusV1::Disabled => "disabled",
                agentx_runtime_contracts::RuntimeResourceStateStatusV1::Revoked => "revoked",
            };
            sqlx::query("INSERT INTO runtime_resource_states(tenant_id,resource_kind,resource_id,resource_version,state_epoch,status,content_hash) VALUES(?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE resource_version=IF(state_epoch<=VALUES(state_epoch),VALUES(resource_version),resource_version),status=IF(state_epoch<=VALUES(state_epoch),VALUES(status),status),content_hash=IF(state_epoch<=VALUES(state_epoch),VALUES(content_hash),content_hash),state_epoch=GREATEST(state_epoch,VALUES(state_epoch)),applied_at=IF(state_epoch<=VALUES(state_epoch),UTC_TIMESTAMP(6),applied_at)")
                .bind(resource.tenant_id).bind(resource_kind).bind(resource.resource_id).bind(&resource.resource_version).bind(resource.state_epoch).bind(status).bind(resource.content_hash.as_str()).execute(&mut *tx).await?;
        }
        AdmissionTargetV1::QuotaPolicy { state: policy } => {
            ensure_tenant(tenant_id, policy.tenant_id)?;
            for (dimension, limit) in &policy.limits {
                sqlx::query("INSERT INTO quota_policy_projection(tenant_id,dimension_key,hard_limit,period_seconds,version,updated_by) VALUES(?,?,?,NULL,?,?) ON DUPLICATE KEY UPDATE hard_limit=IF(version<=VALUES(version),VALUES(hard_limit),hard_limit),version=GREATEST(version,VALUES(version)),updated_by=IF(version<=VALUES(version),VALUES(updated_by),updated_by)")
                    .bind(policy.tenant_id).bind(quota_dimension(*dimension)).bind(if policy.enabled{*limit}else{0}).bind(policy.policy_version).bind(request.command.event_id).execute(&mut *tx).await?;
            }
        }
        AdmissionTargetV1::ApprovalAction { state: action } => {
            apply_approval_action(
                &mut tx,
                tenant_id,
                request.command.event_id,
                &request.command.idempotency_key,
                action,
            )
            .await?;
        }
        AdmissionTargetV1::RetentionHold { state: hold } => {
            if hold.held {
                sqlx::query("INSERT INTO runtime_retention_holds(id,tenant_id,aggregate_type,aggregate_id,reason,version,expires_at) VALUES(?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE reason=IF(version<=VALUES(version),VALUES(reason),reason),expires_at=IF(version<=VALUES(version),VALUES(expires_at),expires_at),released_at=IF(version<=VALUES(version),NULL,released_at),version=GREATEST(version,VALUES(version))")
                    .bind(hold.hold_id).bind(tenant_id).bind(&hold.aggregate_type).bind(hold.aggregate_id).bind(&hold.reason).bind(request.admission_epoch).bind(hold.expires_at).execute(&mut *tx).await?;
            } else {
                sqlx::query("UPDATE runtime_retention_holds SET released_at=UTC_TIMESTAMP(6),version=? WHERE id=? AND tenant_id=? AND version<=?")
                    .bind(request.admission_epoch).bind(hold.hold_id).bind(tenant_id).bind(request.admission_epoch).execute(&mut *tx).await?;
            }
        }
        AdmissionTargetV1::RetentionPolicy { state: policy } => {
            ensure_tenant(tenant_id, policy.tenant_id)?;
            for (data_type, retention_days) in &policy.retention_days {
                let data_type = match data_type {
                    agentx_runtime_contracts::RuntimeRetentionDataTypeV1::Artifact => "artifact",
                    agentx_runtime_contracts::RuntimeRetentionDataTypeV1::Execution => "execution",
                    agentx_runtime_contracts::RuntimeRetentionDataTypeV1::ApplicationMessage => {
                        "application_message"
                    }
                    agentx_runtime_contracts::RuntimeRetentionDataTypeV1::EvaluationReport => {
                        "evaluation_report"
                    }
                    agentx_runtime_contracts::RuntimeRetentionDataTypeV1::Trace => "trace",
                };
                sqlx::query("INSERT INTO retention_policy_projection(tenant_id,data_type,retention_days,enabled,version,updated_by) VALUES(?,?,?,?,?,?) ON DUPLICATE KEY UPDATE retention_days=IF(version<=VALUES(version),VALUES(retention_days),retention_days),enabled=IF(version<=VALUES(version),VALUES(enabled),enabled),version=GREATEST(version,VALUES(version)),updated_by=IF(version<=VALUES(version),VALUES(updated_by),updated_by)")
                    .bind(policy.tenant_id).bind(data_type).bind(*retention_days).bind(policy.enabled).bind(policy.policy_version).bind(request.command.event_id).execute(&mut *tx).await?;
            }
        }
    }
    let receipt = ApplyReceiptV1 {
        api_version: 1,
        event_id: request.command.event_id,
        applied: true,
        replayed: false,
        object_version: request.admission_epoch,
        result: json!({"admissionEpoch":request.admission_epoch}),
    };
    persist_receipt_tx(
        &mut tx,
        tenant_id,
        "admission",
        &request.command.idempotency_key,
        &request,
        None,
        None,
        None,
        "accepted",
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(receipt)
}

async fn apply_approval_action(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    command_id: Uuid,
    idempotency_key: &str,
    action: &agentx_runtime_contracts::RuntimeApprovalActionV1,
) -> RuntimeResult<()> {
    use agentx_runtime_contracts::ApprovalActionValueV1;

    let task = sqlx::query("SELECT a.execution_id,a.node_execution_id,a.status,a.claimed_by,a.deadline_at,a.version,a.request_payload_json,a.buttons_json,e.status execution_status FROM approval_tasks a JOIN workflow_executions e ON e.tenant_id=a.tenant_id AND e.id=a.execution_id WHERE a.tenant_id=? AND a.id=? FOR UPDATE")
        .bind(tenant_id)
        .bind(action.task_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RuntimeError::NotFound)?;
    if task.try_get::<u64, _>("version")? != action.task_version {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::HeadVersionConflict,
            "Approval Task Version changed".into(),
        ));
    }
    let status: String = task.try_get("status")?;
    let claimed_by: Option<Uuid> = task.try_get("claimed_by")?;
    let execution_id: Uuid = task.try_get("execution_id")?;
    let node_execution_id: Uuid = task.try_get("node_execution_id")?;
    let execution_active = !matches!(
        task.try_get::<String, _>("execution_status")?.as_str(),
        "succeeded" | "failed" | "cancelled" | "timed_out"
    );
    let actor_is_candidate: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM approval_candidates WHERE tenant_id=? AND approval_task_id=? AND candidate_type='user' AND candidate_id=?)",
    )
    .bind(tenant_id)
    .bind(action.task_id)
    .bind(action.actor_id)
    .fetch_one(&mut **tx)
    .await?;
    let target_status = match &action.action {
        ApprovalActionValueV1::Claim | ApprovalActionValueV1::Reassign => "claimed",
        ApprovalActionValueV1::Release => "pending",
        ApprovalActionValueV1::Decide { .. } => "decided",
        ApprovalActionValueV1::Cancel => "cancelled",
        ApprovalActionValueV1::Timeout => "timed_out",
    };
    let valid = match &action.action {
        ApprovalActionValueV1::Claim => {
            execution_active && actor_is_candidate && status == "pending"
        }
        ApprovalActionValueV1::Release => {
            execution_active && status == "claimed" && claimed_by == Some(action.actor_id)
        }
        ApprovalActionValueV1::Reassign => {
            let target_is_candidate = if let Some(target_user_id) = action.target_user_id {
                sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM approval_candidates WHERE tenant_id=? AND approval_task_id=? AND candidate_type='user' AND candidate_id=?)",
                )
                .bind(tenant_id)
                .bind(action.task_id)
                .bind(target_user_id)
                .fetch_one(&mut **tx)
                .await?
            } else {
                false
            };
            execution_active
                && matches!(status.as_str(), "pending" | "claimed")
                && target_is_candidate
        }
        ApprovalActionValueV1::Decide { decision_id } => {
            let button_exists = task
                .try_get::<Value, _>("buttons_json")?
                .as_array()
                .is_some_and(|buttons| {
                    buttons.iter().any(|button| {
                        button.get("id").and_then(Value::as_str) == Some(decision_id.as_str())
                    })
                });
            execution_active
                && actor_is_candidate
                && status == "claimed"
                && claimed_by == Some(action.actor_id)
                && button_exists
        }
        ApprovalActionValueV1::Cancel => matches!(status.as_str(), "pending" | "claimed"),
        ApprovalActionValueV1::Timeout => {
            matches!(status.as_str(), "pending" | "claimed")
                && task
                    .try_get::<Option<OffsetDateTime>, _>("deadline_at")?
                    .is_some_and(|deadline| deadline <= OffsetDateTime::now_utc())
        }
    };
    if !valid {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::HeadVersionConflict,
            "Approval action does not match the current Runtime Task state".into(),
        ));
    }
    let next_claimed_by = match &action.action {
        ApprovalActionValueV1::Claim => Some(action.actor_id),
        ApprovalActionValueV1::Reassign => action.target_user_id,
        ApprovalActionValueV1::Release => None,
        _ => claimed_by,
    };
    let original_input = task
        .try_get::<Option<Value>, _>("request_payload_json")?
        .unwrap_or(Value::Null);
    let decision = approval_action_result(action, &original_input);
    let decision_id = match &action.action {
        ApprovalActionValueV1::Decide { decision_id } => Some(decision_id.as_str()),
        _ => None,
    };
    let changed = sqlx::query("UPDATE approval_tasks SET status=?,claimed_by=?,claimed_at=IF(?='claimed',UTC_TIMESTAMP(6),claimed_at),version=version+1,decision_idempotency_key=IF(?='decided',?,decision_idempotency_key),decision_receipt_json=IF(?='decided',?,decision_receipt_json),decision_id=IF(?='decided',?,decision_id),decided_by=IF(?='decided',?,decided_by),decision_reason=IF(?='decided',?,decision_reason),decided_at=IF(?='decided',UTC_TIMESTAMP(6),decided_at),resume_status=IF(?='decided','pending',resume_status) WHERE tenant_id=? AND id=? AND version=?")
        .bind(target_status)
        .bind(next_claimed_by)
        .bind(target_status)
        .bind(target_status)
        .bind(idempotency_key)
        .bind(target_status)
        .bind(&decision)
        .bind(target_status)
        .bind(decision_id)
        .bind(target_status)
        .bind(action.actor_id)
        .bind(target_status)
        .bind(action.input.as_ref().and_then(|input| input.get("reason")).and_then(Value::as_str))
        .bind(target_status)
        .bind(target_status)
        .bind(tenant_id)
        .bind(action.task_id)
        .bind(action.task_version)
        .execute(&mut **tx)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::HeadVersionConflict,
            "Approval Task changed while applying the action".into(),
        ));
    }
    if matches!(&action.action, ApprovalActionValueV1::Decide { .. }) {
        sqlx::query("INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json,status) VALUES(?,?,'resume_execution','execution',?,?,?,'pending')")
            .bind(command_id)
            .bind(tenant_id)
            .bind(execution_id.to_string())
            .bind(format!("approval:{}:{}", action.task_id, action.task_version))
            .bind(json!({
                "nodeExecutionId": node_execution_id,
                "outputPort": match &action.action {
                    ApprovalActionValueV1::Decide { decision_id } => format!("decision:{decision_id}"),
                    _ => unreachable!("only Decide resumes an Approval"),
                },
                "payload": decision,
            }))
            .execute(&mut **tx)
            .await?;
    }
    crate::event_export::enqueue_approval_event_from_task(tx, tenant_id, action.task_id).await?;
    Ok(())
}

fn approval_action_result(
    action: &agentx_runtime_contracts::RuntimeApprovalActionV1,
    original_input: &Value,
) -> Option<Value> {
    use agentx_runtime_contracts::ApprovalActionValueV1;

    let decision = match &action.action {
        ApprovalActionValueV1::Decide { decision_id } => decision_id,
        _ => return None,
    };
    Some(json!({
        "taskId": action.task_id,
        "decision": decision,
        "decidedBy": action.actor_id,
        "reason": action.input.as_ref().and_then(|input| input.get("reason").or_else(|| input.get("comment"))).cloned().unwrap_or(Value::Null),
        "input": original_input,
    }))
}

const fn quota_dimension(dimension: agentx_runtime_contracts::QuotaDimensionV1) -> &'static str {
    use agentx_runtime_contracts::QuotaDimensionV1;
    match dimension {
        QuotaDimensionV1::ExecutionConcurrency => "execution_concurrency",
        QuotaDimensionV1::NodeConcurrency => "node_concurrency",
        QuotaDimensionV1::SandboxConcurrency => "sandbox_concurrency",
        QuotaDimensionV1::AgentIterations => "agent_iterations",
        QuotaDimensionV1::Tokens => "tokens",
        QuotaDimensionV1::CostMicros => "cost_micros",
        QuotaDimensionV1::ArtifactBytes => "artifact_bytes",
        QuotaDimensionV1::CpuMillis => "cpu_millis",
        QuotaDimensionV1::MemoryBytes => "memory_bytes",
        QuotaDimensionV1::Pids => "pids",
        QuotaDimensionV1::DiskBytes => "disk_bytes",
        QuotaDimensionV1::TtlSeconds => "ttl_seconds",
    }
}

pub async fn activate_deployment(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<ActivateDeploymentRequestV1>,
) -> RuntimeResult<Json<PublishReceiptV1>> {
    state
        .trust
        .publisher(&headers, "runtime.deployments.activate")?;
    activate_with_receipt(
        &state,
        "activate",
        &request.idempotency_key,
        &request.manifest,
        &request,
        false,
    )
    .await
    .map(Json)
}

pub async fn rollback_deployment(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<RollbackDeploymentRequestV1>,
) -> RuntimeResult<Json<PublishReceiptV1>> {
    state
        .trust
        .publisher(&headers, "runtime.deployments.rollback")?;
    activate_with_receipt(
        &state,
        "rollback",
        &request.idempotency_key,
        &request.manifest,
        &request,
        true,
    )
    .await
    .map(Json)
}

async fn activate_with_receipt<T: Serialize>(
    state: &RuntimeState,
    operation: &str,
    idempotency_key: &str,
    manifest: &agentx_runtime_contracts::ActivationManifestV1,
    request: &T,
    rollback: bool,
) -> RuntimeResult<PublishReceiptV1> {
    if let Some(receipt) = replay::<_, PublishReceiptV1>(
        state,
        manifest.tenant_id,
        operation,
        idempotency_key,
        request,
    )
    .await?
    {
        return Ok(receipt);
    }
    let result = activate_inner(
        state,
        operation,
        idempotency_key,
        manifest,
        request,
        rollback,
    )
    .await;
    match result {
        Ok(receipt) => Ok(receipt),
        Err(RuntimeError::BadRequest(code, message) | RuntimeError::Conflict(code, message)) => {
            let receipt = rejected_receipt(manifest.bundle_id, code, &message);
            persist_receipt(
                state,
                manifest.tenant_id,
                operation,
                idempotency_key,
                request,
                Some(manifest.bundle_id),
                Some(manifest.application_id),
                Some(manifest.deployment_id),
                "rejected",
                Some(code_string(code)),
                &receipt,
            )
            .await?;
            Ok(receipt)
        }
        Err(error) => Err(error),
    }
}

async fn activate_inner<T: Serialize>(
    state: &RuntimeState,
    operation: &str,
    idempotency_key: &str,
    manifest: &agentx_runtime_contracts::ActivationManifestV1,
    request: &T,
    rollback: bool,
) -> RuntimeResult<PublishReceiptV1> {
    let mut tx = state.pool.begin().await?;
    let bundle = sqlx::query("SELECT application_id,status,payload_json FROM deployment_bundles WHERE id=? AND tenant_id=? FOR UPDATE")
        .bind(manifest.bundle_id).bind(manifest.tenant_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    let bundle_status = bundle.try_get::<String, _>("status")?;
    let lifecycle_valid = if rollback {
        matches!(bundle_status.as_str(), "superseded" | "retained")
    } else {
        bundle_status == "prepared"
    };
    if bundle.try_get::<Uuid, _>("application_id")? != manifest.application_id || !lifecycle_valid {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::BundleReferenceConflict,
            "Bundle cannot be activated for this application".into(),
        ));
    }
    let head = sqlx::query("SELECT bundle_id,version,sequence_number,admission_epoch FROM deployment_heads WHERE tenant_id=? AND application_id=? FOR UPDATE")
        .bind(manifest.tenant_id).bind(manifest.application_id).fetch_optional(&mut *tx).await?;
    let current_version = head
        .as_ref()
        .map(|row| row.try_get::<u64, _>("version"))
        .transpose()?;
    if current_version != manifest.expected_head_version {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::HeadVersionConflict,
            "expected Head Version does not match".into(),
        ));
    }
    let current_sequence = head
        .as_ref()
        .map(|row| row.try_get::<u64, _>("sequence_number"))
        .transpose()?
        .unwrap_or(0);
    if manifest.activation_sequence <= current_sequence {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::ActivationSequenceConflict,
            "activation sequence must increase".into(),
        ));
    }
    let tenant_ok: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenant_admission WHERE tenant_id=? AND status='active' AND admission_epoch>=?)")
        .bind(manifest.tenant_id).bind(manifest.minimum_admission_epoch).fetch_one(&mut *tx).await?;
    let route_ok: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM application_routes WHERE tenant_id=? AND application_id=? AND status='active' AND admission_epoch>=?)")
        .bind(manifest.tenant_id).bind(manifest.application_id).bind(manifest.minimum_admission_epoch).fetch_one(&mut *tx).await?;
    let key_ok: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM api_key_admission WHERE tenant_id=? AND application_id=?) OR EXISTS(SELECT 1 FROM api_key_admission WHERE tenant_id=? AND application_id=? AND status='active' AND admission_epoch>=? AND (expires_at IS NULL OR expires_at>UTC_TIMESTAMP(6)))")
        .bind(manifest.tenant_id).bind(manifest.application_id)
        .bind(manifest.tenant_id).bind(manifest.application_id).bind(manifest.minimum_admission_epoch)
        .fetch_one(&mut *tx).await?;
    let payload: Value = bundle.try_get("payload_json")?;
    let specification: agentx_runtime_contracts::ExecutionSpecPayloadV2 =
        serde_json::from_value(payload.clone())
            .map_err(|error| RuntimeError::Internal(error.into()))?;
    let identity_id = specification.authorization.service_identity_id;
    let identity_ok: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM service_identity_projection WHERE identity_id=? AND tenant_id=? AND status='active')")
        .bind(identity_id).bind(manifest.tenant_id).fetch_one(&mut *tx).await?;
    let mut grants_ok = true;
    for grant in &specification.authorization.grant_ids {
        let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grant_projection WHERE grant_id=? AND tenant_id=? AND subject_id=? AND status='active' AND policy_epoch>=?)")
            .bind(grant).bind(manifest.tenant_id).bind(identity_id).bind(specification.authorization.policy_epoch).fetch_one(&mut *tx).await?;
        grants_ok &= found;
    }
    let mut resources_ok = true;
    for resource in &specification.resources {
        let resource_kind = serde_json::to_value(resource.resource_kind)
            .map_err(|error| RuntimeError::Internal(error.into()))?
            .as_str()
            .ok_or_else(|| {
                RuntimeError::Internal(anyhow::anyhow!("resource kind is not a string"))
            })?
            .to_owned();
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_resource_states WHERE tenant_id=? AND resource_kind=? AND resource_id=? AND resource_version=? AND state_epoch>=? AND status='active' AND content_hash=?)",
        )
        .bind(manifest.tenant_id)
        .bind(resource_kind)
        .bind(resource.resource_id)
        .bind(&resource.resource_version)
        .bind(resource.state_epoch)
        .bind(resource.content_hash.as_str())
        .fetch_one(&mut *tx)
        .await?;
        resources_ok &= found;
    }
    if !(tenant_ok && route_ok && key_ok && identity_ok && grants_ok && resources_ok) {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::AdmissionPrerequisiteMissing,
            "Runtime Admission prerequisites are incomplete".into(),
        ));
    }
    if let Some(old) = &head {
        let old_bundle: Uuid = old.try_get("bundle_id")?;
        if old_bundle != manifest.bundle_id {
            sqlx::query("UPDATE deployment_bundles SET status='superseded',superseded_at=UTC_TIMESTAMP(6),retained_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 14 DAY) WHERE id=? AND status='active'").bind(old_bundle).execute(&mut *tx).await?;
        }
        let updated = sqlx::query("UPDATE deployment_heads SET bundle_id=?,sequence_number=?,admission_epoch=GREATEST(admission_epoch,?),version=version+1 WHERE tenant_id=? AND application_id=? AND version=? AND sequence_number<?")
            .bind(manifest.bundle_id).bind(manifest.activation_sequence).bind(manifest.minimum_admission_epoch).bind(manifest.tenant_id).bind(manifest.application_id).bind(current_version).bind(manifest.activation_sequence).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(RuntimeError::Conflict(
                RuntimePublishErrorCodeV1::HeadVersionConflict,
                "deployment Head changed during activation".into(),
            ));
        }
    } else {
        sqlx::query("INSERT INTO deployment_heads(tenant_id,application_id,bundle_id,sequence_number,admission_epoch,version) VALUES(?,?,?,?,?,1)")
            .bind(manifest.tenant_id).bind(manifest.application_id).bind(manifest.bundle_id).bind(manifest.activation_sequence).bind(manifest.minimum_admission_epoch).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE deployment_bundles SET status='active',activated_at=UTC_TIMESTAMP(6),disabled_at=NULL WHERE id=?").bind(manifest.bundle_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE application_routes SET active_bundle_id=?,status='active',admission_epoch=GREATEST(admission_epoch,?),runtime_config_revision=?,runtime_policy_json=? WHERE tenant_id=? AND application_id=?").bind(manifest.bundle_id).bind(manifest.minimum_admission_epoch).bind(manifest.runtime_config_revision).bind(serde_json::to_value(&manifest.runtime_policy).map_err(|e|RuntimeError::Internal(e.into()))?).bind(manifest.tenant_id).bind(manifest.application_id).execute(&mut *tx).await?;
    replace_trigger_bindings(
        &mut tx,
        manifest,
        &payload,
        head.as_ref()
            .map(|row| row.try_get::<Uuid, _>("bundle_id"))
            .transpose()?,
    )
    .await?;
    let head_version = current_version.unwrap_or(0) + 1;
    let receipt = accepted_receipt(
        manifest.bundle_id,
        Some(head_version),
        Some(manifest.activation_sequence),
        false,
    );
    persist_receipt_tx(
        &mut tx,
        manifest.tenant_id,
        operation,
        idempotency_key,
        request,
        Some(manifest.bundle_id),
        Some(manifest.application_id),
        Some(manifest.deployment_id),
        "accepted",
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(receipt)
}

async fn replace_trigger_bindings(
    tx: &mut Transaction<'_, MySql>,
    manifest: &agentx_runtime_contracts::ActivationManifestV1,
    payload: &Value,
    _previous_bundle_id: Option<Uuid>,
) -> RuntimeResult<()> {
    let triggers: Vec<agentx_runtime_contracts::RuntimeTriggerSpecV1> = serde_json::from_value(
        payload
            .get("triggers")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|error| {
        RuntimeError::BadRequest(
            RuntimePublishErrorCodeV1::UnsupportedBundleVersion,
            error.to_string(),
        )
    })?;
    sqlx::query("UPDATE trigger_bindings SET status='disabled',next_poll_at=NULL,locked_by=NULL,locked_until=NULL,heartbeat_at=NULL WHERE tenant_id=? AND application_id=? AND status<>'disabled'")
        .bind(manifest.tenant_id).bind(manifest.application_id).execute(&mut **tx).await?;
    apply_trigger_bindings(
        tx,
        manifest.tenant_id,
        manifest.application_id,
        manifest.deployment_id,
        manifest.bundle_id,
        &triggers,
    )
    .await
}

/// Disables the application's schedule/webhook bindings and re-creates them
/// from the given trigger specs against the given bundle. Shared by Deployment
/// activation and the trigger-only sync path.
async fn apply_trigger_bindings(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    application_id: Uuid,
    deployment_id: Uuid,
    bundle_id: Uuid,
    triggers: &[agentx_runtime_contracts::RuntimeTriggerSpecV1],
) -> RuntimeResult<()> {
    sqlx::query("UPDATE webhook_bindings SET status='disabled',locked_by=NULL,locked_until=NULL,heartbeat_at=NULL,connection_status='disconnected' WHERE tenant_id=? AND application_id=? AND status='active'")
        .bind(tenant_id).bind(application_id).execute(&mut **tx).await?;
    for trigger in triggers {
        if trigger.application_id != application_id {
            return Err(RuntimeError::BadRequest(
                RuntimePublishErrorCodeV1::TenantMismatch,
                "Trigger Application does not match activation".into(),
            ));
        }
        let kind = match &trigger.configuration {
            agentx_runtime_contracts::RuntimeTriggerConfigurationV1::Webhook { .. } => "webhook",
            agentx_runtime_contracts::RuntimeTriggerConfigurationV1::Schedule { .. } => "schedule",
        };
        let active = trigger.enabled;
        let next_poll_at = initial_trigger_due(tx, trigger).await?;
        sqlx::query("INSERT INTO trigger_bindings(id,tenant_id,application_id,application_deployment_id,bundle_id,workflow_version_id,node_id,configuration_revision,configuration_hash,trigger_kind,configuration_json,status,next_poll_at,activated_at) SELECT ?,?,?,?,?,workflow_version_id,?,?,?,?,?,?,?,UTC_TIMESTAMP(6) FROM deployment_bundles WHERE tenant_id=? AND id=? ON DUPLICATE KEY UPDATE application_deployment_id=VALUES(application_deployment_id),bundle_id=VALUES(bundle_id),configuration_revision=VALUES(configuration_revision),cursor_value=IF(configuration_hash=VALUES(configuration_hash),cursor_value,NULL),next_poll_at=IF(VALUES(status)='active' AND configuration_hash=VALUES(configuration_hash),next_poll_at,VALUES(next_poll_at)),configuration_hash=VALUES(configuration_hash),configuration_json=VALUES(configuration_json),status=VALUES(status),locked_by=NULL,locked_until=NULL,heartbeat_at=NULL,activated_at=UTC_TIMESTAMP(6)")
            .bind(trigger.trigger_id).bind(tenant_id).bind(application_id).bind(deployment_id).bind(bundle_id).bind(&trigger.node_id).bind(trigger.revision).bind(trigger.configuration_hash.as_str()).bind(kind).bind(serde_json::to_value(trigger).map_err(|e|RuntimeError::Internal(e.into()))?).bind(if active {"active"} else {"disabled"}).bind(next_poll_at).bind(tenant_id).bind(bundle_id).execute(&mut **tx).await?;
        if let agentx_runtime_contracts::RuntimeTriggerConfigurationV1::Webhook {
            public_id,
            secret,
            provider,
            mode,
            input_mappings,
            fixed_inputs,
            reply,
        } = &trigger.configuration
        {
            let provider_type =
                serde_json::to_value(provider).map_err(|e| RuntimeError::Internal(e.into()))?;
            let channel_mode =
                serde_json::to_value(mode).map_err(|e| RuntimeError::Internal(e.into()))?;
            let reply_config = reply
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|e| RuntimeError::Internal(e.into()))?;
            sqlx::query("INSERT INTO webhook_bindings(id,tenant_id,application_id,bundle_id,configuration_revision,configuration_hash,public_id,provider_type,channel_mode,input_mapping_json,fixed_inputs_json,reply_config_json,secret_ref_json,status,activated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE bundle_id=VALUES(bundle_id),configuration_revision=VALUES(configuration_revision),configuration_hash=VALUES(configuration_hash),provider_type=VALUES(provider_type),channel_mode=VALUES(channel_mode),input_mapping_json=VALUES(input_mapping_json),fixed_inputs_json=VALUES(fixed_inputs_json),reply_config_json=VALUES(reply_config_json),secret_ref_json=VALUES(secret_ref_json),status=VALUES(status),activated_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL,heartbeat_at=NULL,connection_status=NULL")
                .bind(trigger.trigger_id).bind(tenant_id).bind(application_id).bind(bundle_id).bind(trigger.revision).bind(trigger.configuration_hash.as_str()).bind(public_id.clone())
                .bind(provider_type.as_str().unwrap_or("agentx"))
                .bind(channel_mode.as_str().unwrap_or("callback"))
                .bind(serde_json::to_value(input_mappings).map_err(|e|RuntimeError::Internal(e.into()))?)
                .bind(fixed_inputs.clone())
                .bind(&reply_config)
                .bind(serde_json::to_value(secret).map_err(|e|RuntimeError::Internal(e.into()))?).bind(if trigger.enabled{"active"}else{"disabled"}).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

async fn initial_trigger_due(
    tx: &mut Transaction<'_, MySql>,
    trigger: &agentx_runtime_contracts::RuntimeTriggerSpecV1,
) -> RuntimeResult<Option<OffsetDateTime>> {
    if !trigger.enabled {
        return Ok(None);
    }
    let now: OffsetDateTime = sqlx::query_scalar("SELECT UTC_TIMESTAMP(6)")
        .fetch_one(&mut **tx)
        .await?;
    match &trigger.configuration {
        agentx_runtime_contracts::RuntimeTriggerConfigurationV1::Schedule {
            cron_expression,
            timezone,
            ..
        } => {
            let anchor =
                chrono::DateTime::from_timestamp(now.unix_timestamp(), now.nanosecond())
                    .ok_or_else(|| RuntimeError::Internal(anyhow::anyhow!("invalid MySQL time")))?;
            let next = crate::trigger::next_schedule(cron_expression, timezone, anchor)?;
            Ok(time::OffsetDateTime::from_unix_timestamp(next.timestamp()).ok())
        }
        agentx_runtime_contracts::RuntimeTriggerConfigurationV1::Webhook { .. } => Ok(None),
    }
}

pub async fn disable_deployment(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<DisableDeploymentRequestV1>,
) -> RuntimeResult<Json<PublishReceiptV1>> {
    state
        .trust
        .publisher(&headers, "runtime.deployments.disable")?;
    if let Some(receipt) = replay::<_, PublishReceiptV1>(
        &state,
        request.tenant_id,
        "disable",
        &request.idempotency_key,
        &request,
    )
    .await?
    {
        return Ok(Json(receipt));
    }
    let result = disable_deployment_inner(&state, &request).await;
    match result {
        Ok(receipt) => Ok(Json(receipt)),
        Err(RuntimeError::BadRequest(code, message) | RuntimeError::Conflict(code, message)) => {
            let bundle_id = sqlx::query_scalar(
                "SELECT bundle_id FROM deployment_heads WHERE tenant_id=? AND application_id=?",
            )
            .bind(request.tenant_id)
            .bind(request.application_id)
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or(Uuid::nil());
            let receipt = rejected_receipt(bundle_id, code, &message);
            persist_receipt(
                &state,
                request.tenant_id,
                "disable",
                &request.idempotency_key,
                &request,
                (bundle_id != Uuid::nil()).then_some(bundle_id),
                Some(request.application_id),
                None,
                "rejected",
                Some(code_string(code)),
                &receipt,
            )
            .await?;
            Ok(Json(receipt))
        }
        Err(error) => Err(error),
    }
}

async fn disable_deployment_inner(
    state: &RuntimeState,
    request: &DisableDeploymentRequestV1,
) -> RuntimeResult<PublishReceiptV1> {
    let mut tx = state.pool.begin().await?;
    let head = sqlx::query("SELECT bundle_id,version,sequence_number FROM deployment_heads WHERE tenant_id=? AND application_id=? FOR UPDATE").bind(request.tenant_id).bind(request.application_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    let bundle_id: Uuid = head.try_get("bundle_id")?;
    let current_epoch: u64 = sqlx::query_scalar("SELECT admission_epoch FROM application_routes WHERE tenant_id=? AND application_id=? FOR UPDATE").bind(request.tenant_id).bind(request.application_id).fetch_one(&mut *tx).await?;
    if request.admission_epoch <= current_epoch {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::ActivationSequenceConflict,
            "disable Admission Epoch must increase".into(),
        ));
    }
    sqlx::query("UPDATE application_routes SET status='disabled',admission_epoch=? WHERE tenant_id=? AND application_id=?").bind(request.admission_epoch).bind(request.tenant_id).bind(request.application_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE deployment_bundles SET status='disabled',disabled_at=UTC_TIMESTAMP(6),retained_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 14 DAY) WHERE id=?").bind(bundle_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE trigger_bindings SET status='disabled',next_poll_at=NULL,locked_by=NULL,locked_until=NULL,heartbeat_at=NULL WHERE tenant_id=? AND application_id=?")
        .bind(request.tenant_id)
        .bind(request.application_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE webhook_bindings SET status='disabled' WHERE tenant_id=? AND application_id=?",
    )
    .bind(request.tenant_id)
    .bind(request.application_id)
    .execute(&mut *tx)
    .await?;
    let receipt = accepted_receipt(
        bundle_id,
        Some(head.try_get("version")?),
        Some(head.try_get("sequence_number")?),
        false,
    );
    persist_receipt_tx(
        &mut tx,
        request.tenant_id,
        "disable",
        &request.idempotency_key,
        &request,
        Some(bundle_id),
        Some(request.application_id),
        None,
        "accepted",
        None,
        &receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(receipt)
}

fn accepted_receipt(
    bundle_id: Uuid,
    head_version: Option<u64>,
    activation_sequence: Option<u64>,
    replayed: bool,
) -> PublishReceiptV1 {
    PublishReceiptV1 {
        api_version: 1,
        receipt: ApplyReceiptV1 {
            api_version: 1,
            event_id: Uuid::now_v7(),
            applied: true,
            replayed,
            object_version: head_version.unwrap_or(1),
            result: json!({}),
        },
        bundle_id,
        head_version,
        activation_sequence,
        status: PublishReceiptStatusV1::Accepted,
        rejection: None,
        accepted_at: OffsetDateTime::now_utc(),
    }
}

fn rejected_receipt(
    bundle_id: Uuid,
    code: RuntimePublishErrorCodeV1,
    message: &str,
) -> PublishReceiptV1 {
    PublishReceiptV1 {
        api_version: 1,
        receipt: ApplyReceiptV1 {
            api_version: 1,
            event_id: Uuid::now_v7(),
            applied: false,
            replayed: false,
            object_version: 0,
            result: json!({}),
        },
        bundle_id,
        head_version: None,
        activation_sequence: None,
        status: PublishReceiptStatusV1::Rejected,
        rejection: Some(agentx_runtime_contracts::PublishRejectionV1 {
            code,
            message: message.into(),
            details: json!({}),
        }),
        accepted_at: OffsetDateTime::now_utc(),
    }
}

async fn rejected_admission(
    state: &RuntimeState,
    request: &RuntimeAdmissionCommandV1,
    code: RuntimePublishErrorCodeV1,
    message: &str,
) -> RuntimeResult<ApplyReceiptV1> {
    let receipt = ApplyReceiptV1 {
        api_version: 1,
        event_id: request.command.event_id,
        applied: false,
        replayed: false,
        object_version: request.admission_epoch,
        result: json!({
            "rejection": {
                "code": code_string(code),
                "message": message,
            }
        }),
    };
    persist_receipt(
        state,
        request.command.tenant_id,
        "admission",
        &request.command.idempotency_key,
        request,
        None,
        None,
        None,
        "rejected",
        Some(code_string(code)),
        &receipt,
    )
    .await?;
    Ok(receipt)
}

async fn replay<T: Serialize, R: serde::de::DeserializeOwned>(
    state: &RuntimeState,
    tenant_id: Uuid,
    operation: &str,
    idempotency_key: &str,
    request: &T,
) -> RuntimeResult<Option<R>> {
    let request_hash = agentx_runtime_contracts::content_hash(request)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let row = sqlx::query("SELECT request_hash,response_json FROM publish_receipts WHERE tenant_id=? AND operation=? AND idempotency_key=?")
        .bind(tenant_id).bind(operation).bind(idempotency_key).fetch_optional(&state.pool).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<String, _>("request_hash")? != request_hash.as_str() {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::IdempotencyConflict,
            "idempotency key was reused for a different request".into(),
        ));
    }
    Ok(Some(
        serde_json::from_value(row.try_get("response_json")?)
            .map_err(|error| RuntimeError::Internal(error.into()))?,
    ))
}

async fn replay_tx<T: Serialize, R: serde::de::DeserializeOwned>(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    operation: &str,
    idempotency_key: &str,
    request: &T,
) -> RuntimeResult<Option<R>> {
    let request_hash = agentx_runtime_contracts::content_hash(request)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let row = sqlx::query("SELECT request_hash,response_json FROM publish_receipts WHERE tenant_id=? AND operation=? AND idempotency_key=?")
        .bind(tenant_id)
        .bind(operation)
        .bind(idempotency_key)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<String, _>("request_hash")? != request_hash.as_str() {
        return Err(RuntimeError::Conflict(
            RuntimePublishErrorCodeV1::IdempotencyConflict,
            "idempotency key was reused for a different request".into(),
        ));
    }
    Ok(Some(
        serde_json::from_value(row.try_get("response_json")?)
            .map_err(|error| RuntimeError::Internal(error.into()))?,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn persist_receipt<T: Serialize, R: Serialize>(
    state: &RuntimeState,
    tenant_id: Uuid,
    operation: &str,
    idempotency_key: &str,
    request: &T,
    bundle_id: Option<Uuid>,
    application_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    status: &str,
    error_code: Option<String>,
    receipt: &R,
) -> RuntimeResult<()> {
    let mut tx = state.pool.begin().await?;
    persist_receipt_tx(
        &mut tx,
        tenant_id,
        operation,
        idempotency_key,
        request,
        bundle_id,
        application_id,
        deployment_id,
        status,
        error_code,
        receipt,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn persist_receipt_tx<T: Serialize, R: Serialize>(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    operation: &str,
    idempotency_key: &str,
    request: &T,
    bundle_id: Option<Uuid>,
    application_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    status: &str,
    error_code: Option<String>,
    receipt: &R,
) -> RuntimeResult<()> {
    let request_hash = agentx_runtime_contracts::content_hash(request)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    sqlx::query("INSERT INTO publish_receipts(id,tenant_id,operation,idempotency_key,request_hash,bundle_id,application_id,deployment_id,status,error_code,response_json) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(tenant_id).bind(operation).bind(idempotency_key).bind(request_hash.as_str()).bind(bundle_id).bind(application_id).bind(deployment_id).bind(status).bind(error_code).bind(serde_json::to_value(receipt).map_err(|error| RuntimeError::Internal(error.into()))?).execute(&mut **tx).await?;
    Ok(())
}

fn ensure_tenant(expected: Uuid, actual: Uuid) -> RuntimeResult<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(RuntimeError::BadRequest(
            RuntimePublishErrorCodeV1::TenantMismatch,
            "Admission tenant mismatch".into(),
        ))
    }
}
fn route_status(status: AdmissionStatusV1) -> &'static str {
    if status == AdmissionStatusV1::Active {
        "active"
    } else {
        "disabled"
    }
}
fn parse_sha256(value: &str) -> RuntimeResult<Vec<u8>> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(RuntimeError::BadRequest(
            RuntimePublishErrorCodeV1::ContentHashMismatch,
            "Admission secret hash must be SHA-256".into(),
        ));
    }
    (0..64)
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&value[i..i + 2], 16).map_err(|_| {
                RuntimeError::BadRequest(
                    RuntimePublishErrorCodeV1::ContentHashMismatch,
                    "Admission secret hash must be SHA-256".into(),
                )
            })
        })
        .collect()
}
fn code_string(code: RuntimePublishErrorCodeV1) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "UNKNOWN".into())
}

#[cfg(test)]
mod tests {
    use agentx_runtime_contracts::{ApprovalActionValueV1, RuntimeApprovalActionV1};
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::approval_action_result;

    #[test]
    fn approval_action_result_uses_the_frozen_decision_output_shape() {
        let task_id = Uuid::now_v7();
        let actor_id = Uuid::now_v7();
        let original_input = json!({"request":"deploy"});
        let approved = approval_action_result(
            &RuntimeApprovalActionV1 {
                task_id,
                task_version: 2,
                action: ApprovalActionValueV1::Decide {
                    decision_id: "escalate".into(),
                },
                actor_id,
                target_user_id: None,
                input: Some(json!({"comment":"ship it"})),
            },
            &original_input,
        )
        .expect("canvas button is a decision");

        assert_eq!(approved["taskId"], json!(task_id));
        assert_eq!(approved["decision"], "escalate");
        assert_eq!(approved["decidedBy"], json!(actor_id));
        assert_eq!(approved["reason"], "ship it");
        assert_eq!(approved["input"], original_input);
    }

    #[test]
    fn non_terminal_approval_actions_have_no_decision_output() {
        let action = RuntimeApprovalActionV1 {
            task_id: Uuid::now_v7(),
            task_version: 1,
            action: ApprovalActionValueV1::Claim,
            actor_id: Uuid::now_v7(),
            target_user_id: None,
            input: None,
        };
        assert!(approval_action_result(&action, &Value::Null).is_none());
    }
}
