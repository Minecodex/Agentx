use agentx_runtime_contracts::{
    CompiledWorkflowV1, ExecutionWorkflowSnapshotV1, RuntimeObjectReferenceV1,
    RuntimePublishErrorCodeV1, RuntimeResourceBindingV1, RuntimeResourceConfigurationV1,
};
use object_store::path::Path as ObjectPath;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{MySql, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

#[derive(Clone)]
pub(crate) struct MaterializedComposite {
    pub workflow: ExecutionWorkflowSnapshotV1,
    pub definition_object_id: Uuid,
    pub ir_object_id: Uuid,
    pub definition: Value,
    pub compiled_ir: CompiledWorkflowV1,
    pub definition_hash: String,
    pub ir_hash: String,
}

pub(crate) async fn materialize(
    state: &RuntimeState,
    tenant_id: Uuid,
    resources: &[RuntimeResourceBindingV1],
    objects: &[RuntimeObjectReferenceV1],
) -> RuntimeResult<BTreeMap<Uuid, MaterializedComposite>> {
    let object_map = objects
        .iter()
        .map(|object| (object.object_id, object))
        .collect::<BTreeMap<_, _>>();
    let mut result = BTreeMap::new();
    for resource in resources {
        let RuntimeResourceConfigurationV1::Composite {
            workflow,
            definition_object_id,
            ir_object_id,
        } = &resource.configuration
        else {
            continue;
        };
        let workflow_version_id = workflow.version_id.ok_or_else(|| {
            invalid_object("Composite source must be an immutable Workflow Version")
        })?;
        if !resource.object_ids.contains(definition_object_id)
            || !resource.object_ids.contains(ir_object_id)
        {
            return Err(invalid_object(
                "Composite Binding does not contain its Definition and IR objects",
            ));
        }
        let definition_object = object_map
            .get(definition_object_id)
            .ok_or_else(|| invalid_object("Composite Definition object is absent from closure"))?;
        let ir_object = object_map
            .get(ir_object_id)
            .ok_or_else(|| invalid_object("Composite IR object is absent from closure"))?;
        if definition_object.tenant_id != tenant_id || ir_object.tenant_id != tenant_id {
            return Err(invalid_object("Composite object belongs to another Tenant"));
        }
        let definition_bytes = load_verified(state, definition_object).await?;
        let ir_bytes = load_verified(state, ir_object).await?;
        let definition: agentx_domain::WorkflowDefinition =
            serde_json::from_slice(&definition_bytes).map_err(|error| {
                RuntimeError::BadRequest(
                    RuntimePublishErrorCodeV1::UnsupportedBundleVersion,
                    format!("Composite Definition is invalid: {error}"),
                )
            })?;
        let compiled_ir: CompiledWorkflowV1 =
            serde_json::from_slice(&ir_bytes).map_err(|error| {
                RuntimeError::BadRequest(
                    RuntimePublishErrorCodeV1::UnsupportedIrVersion,
                    format!("Composite IR is invalid: {error}"),
                )
            })?;
        if compiled_ir.contract_version != 1 {
            return Err(RuntimeError::BadRequest(
                RuntimePublishErrorCodeV1::UnsupportedIrVersion,
                "Composite IR must use contract version 1".into(),
            ));
        }
        let definition_hash = format!("sha256:{:x}", Sha256::digest(&definition_bytes));
        let ir_hash = format!("sha256:{:x}", Sha256::digest(&ir_bytes));
        if result
            .insert(
                workflow_version_id,
                MaterializedComposite {
                    workflow: workflow.clone(),
                    definition_object_id: *definition_object_id,
                    ir_object_id: *ir_object_id,
                    definition: serde_json::to_value(definition)
                        .map_err(|error| RuntimeError::Internal(error.into()))?,
                    compiled_ir,
                    definition_hash,
                    ir_hash,
                },
            )
            .is_some()
        {
            return Err(invalid_object(
                "Composite Workflow Version has multiple immutable Bindings",
            ));
        }
    }
    Ok(result)
}

async fn load_verified(
    state: &RuntimeState,
    object: &RuntimeObjectReferenceV1,
) -> RuntimeResult<bytes::Bytes> {
    let bytes = state
        .objects
        .get(&ObjectPath::from(object.object_key.clone()))
        .await
        .map_err(|_| invalid_object("Composite Runtime object is unavailable"))?
        .bytes()
        .await
        .map_err(|_| invalid_object("Composite Runtime object cannot be read"))?;
    let hash = format!("sha256:{:x}", Sha256::digest(&bytes));
    if bytes.len() as u64 != object.size_bytes || hash != object.content_hash.as_str() {
        return Err(invalid_object(
            "Composite Runtime object size or hash does not match its manifest",
        ));
    }
    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    bundle_id: Option<Uuid>,
    work_package_id: Option<Uuid>,
    binding_id: Uuid,
    snapshot: &MaterializedComposite,
) -> RuntimeResult<()> {
    sqlx::query(
        "INSERT INTO runtime_composite_snapshots(tenant_id,bundle_id,work_package_id,binding_id,workflow_version_id,workflow_json,definition_object_id,ir_object_id,definition_hash,ir_hash,definition_json,compiled_ir_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE binding_id=binding_id",
    )
    .bind(tenant_id)
    .bind(bundle_id)
    .bind(work_package_id)
    .bind(binding_id)
    .bind(snapshot.workflow.version_id)
    .bind(serde_json::to_value(&snapshot.workflow).map_err(|error| RuntimeError::Internal(error.into()))?)
    .bind(snapshot.definition_object_id)
    .bind(snapshot.ir_object_id)
    .bind(&snapshot.definition_hash)
    .bind(&snapshot.ir_hash)
    .bind(&snapshot.definition)
    .bind(
        serde_json::to_value(&snapshot.compiled_ir)
            .map_err(|error| RuntimeError::Internal(error.into()))?,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn invalid_object(message: &str) -> RuntimeError {
    RuntimeError::BadRequest(
        RuntimePublishErrorCodeV1::ObjectHashMismatch,
        message.into(),
    )
}
