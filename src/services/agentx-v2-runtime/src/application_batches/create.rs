use super::*;
use std::collections::BTreeSet;

pub async fn create(
    state: &RuntimeState,
    caller: &BatchCaller,
    request: CreateBatchV1,
    key: &str,
    access: Option<ApplicationAccessBindingV1>,
    source_batch: Option<Uuid>,
    sources: &[(Uuid, Option<Uuid>)],
) -> RuntimeResult<BatchViewV1> {
    validate(&request, key)?;
    let request_hash = hash(
        &json!({"request":request,"access":access,"sourceBatchId":source_batch,"sources":sources}),
    )?;
    let id = deterministic_uuid(
        caller.tenant_id,
        format!(
            "batch:{}:{}:{}:{key}",
            caller.application_id, caller.caller_type, caller.caller_id
        )
        .as_bytes(),
    );
    let scope = BatchScope::owner(caller.clone());
    let mut tx = state.pool.begin().await?;
    if let Some(existing) =
        sqlx::query("SELECT request_hash FROM application_batches WHERE tenant_id=? AND id=?")
            .bind(caller.tenant_id)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
    {
        if existing.try_get::<String, _>("request_hash")? != request_hash {
            return Err(conflict(
                "The request key was reused with different batch data",
            ));
        }
        tx.commit().await?;
        return get(state, &scope, id).await;
    }
    let head=sqlx::query("SELECT h.bundle_id,h.admission_epoch,b.payload_json FROM deployment_heads h JOIN deployment_bundles b ON b.id=h.bundle_id AND b.tenant_id=h.tenant_id JOIN application_routes r ON r.tenant_id=h.tenant_id AND r.application_id=h.application_id JOIN tenant_admission t ON t.tenant_id=h.tenant_id WHERE h.tenant_id=? AND h.application_id=? AND b.status='active' AND r.status='active' AND r.active_bundle_id=h.bundle_id AND t.status='active' FOR SHARE")
        .bind(caller.tenant_id).bind(caller.application_id).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    crate::application_caller::admission_epoch(
        &mut tx,
        caller.tenant_id,
        caller.application_id,
        caller.kind()?,
        caller.caller_id,
        caller.token_version,
        head.try_get("admission_epoch")?,
    )
    .await?;
    let head_bundle: Uuid = head.try_get("bundle_id")?;
    let bundle = if let Some(access) = &access {
        crate::application_pages::resolve_binding(
            &mut tx,
            caller.tenant_id,
            caller.application_id,
            access,
        )
        .await?
    } else {
        head_bundle
    };
    if source_batch.is_some() && bundle != head_bundle {
        return Err(conflict(
            "Publish a page for the current business version before rerunning this batch",
        ));
    }
    let payload: Value = if bundle == head_bundle {
        head.try_get("payload_json")?
    } else {
        sqlx::query_scalar("SELECT payload_json FROM deployment_bundles WHERE tenant_id=? AND id=? AND status IN ('active','superseded','retained')").bind(caller.tenant_id).bind(bundle).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?
    };
    let spec: ExecutionSpecPayloadV2 =
        serde_json::from_value(payload).map_err(|e| RuntimeError::Internal(e.into()))?;
    let policy =
        crate::user_identity::api_policy(&mut tx, caller.tenant_id, caller.application_id).await?;
    let source = if access.is_some() { "web" } else { "api" };
    let page = match &access {
        Some(ApplicationAccessBindingV1::Web { publication_id, .. }) => Some(*publication_id),
        _ => sqlx::query_scalar("SELECT p.id FROM application_web_heads h JOIN application_page_publications p ON p.tenant_id=h.tenant_id AND p.id=h.publication_id WHERE h.tenant_id=? AND h.application_id=? AND h.enabled=TRUE AND p.bundle_id=? AND p.status='active' FOR SHARE")
            .bind(caller.tenant_id).bind(caller.application_id).bind(bundle).fetch_optional(&mut *tx).await?,
    };
    // Upsert takes the duplicate row's exclusive lock immediately. INSERT IGNORE
    // takes shared locks that concurrent replays would then deadlock upgrading.
    sqlx::query("INSERT INTO application_batches(id,tenant_id,application_id,name,caller_type,caller_id,caller_json,identity_policy_json,source,bundle_id,workflow_version_id,workflow_version_number,input_contract_hash,access_binding_json,history_page_id,source_batch_id,idempotency_key,request_hash) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id")
        .bind(id).bind(caller.tenant_id).bind(caller.application_id).bind(request.name.trim()).bind(&caller.caller_type).bind(caller.caller_id).bind(json!(caller)).bind(&policy).bind(source).bind(bundle).bind(spec.workflow_version_id).bind(spec.workflow_version_number).bind(hash(&spec.input_contract)?).bind(access.as_ref().map(|v|json!(v))).bind(page).bind(source_batch).bind(key).bind(&request_hash).execute(&mut *tx).await?;
    let existing = sqlx::query(
        "SELECT request_hash FROM application_batches WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(caller.tenant_id)
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if existing.try_get::<String, _>("request_hash")? != request_hash {
        return Err(conflict(
            "The request key was reused with different batch data",
        ));
    }
    // Every committed batch has all its items. Do not infer new/replayed from
    // affected_rows, whose no-op-upsert value depends on CLIENT_FOUND_ROWS.
    let replayed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM application_batch_items WHERE tenant_id=? AND batch_id=?)",
    )
    .bind(caller.tenant_id)
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if replayed {
        tx.commit().await?;
        return get(state, &scope, id).await;
    }
    let mut normalized_bytes = 0usize;
    for (index, item) in request.items.into_iter().enumerate() {
        let item_id = Uuid::now_v7();
        let mut input = item.input;
        if let Err(error) =
            agentx_runtime::materialize_and_validate_start_input(&mut input, &spec.input_contract)
        {
            let mut errors = Vec::new();
            if let Ok(validator) = jsonschema::validator_for(&spec.input_contract) {
                for error in validator.iter_errors(&input) {
                    errors.push(GatewayFieldErrorV1 {
                        field: format!("/items/{index}/input{}", error.instance_path),
                        code: "INPUT_SCHEMA_VALIDATION_FAILED".into(),
                        message: error.to_string(),
                    });
                }
            }
            if errors.is_empty() {
                errors.push(GatewayFieldErrorV1 {
                    field: format!("/items/{index}/input"),
                    code: "INPUT_SCHEMA_VALIDATION_FAILED".into(),
                    message: error.to_string(),
                });
            }
            return Err(RuntimeError::Validation {
                code: "BATCH_INPUT_INVALID",
                message: format!(
                    "Invalid input on row {} ({})",
                    index + 1,
                    item.client_item_id
                ),
                field_errors: errors,
            });
        }
        normalized_bytes += serde_json::to_vec(&input)
            .map_err(|e| RuntimeError::Internal(e.into()))?
            .len();
        if normalized_bytes > APPLICATION_BATCH_MAX_BYTES {
            return Err(invalid("Materialized batch inputs exceed 1 MiB"));
        }
        let artifacts = crate::execution::validate_input_artifacts(
            &mut tx,
            caller.tenant_id,
            &input,
            &spec.input_contract,
        )
        .await?;
        for artifact in artifacts {
            let access = crate::artifact_access::require_input(
                &mut tx,
                caller.tenant_id,
                caller.kind()?,
                caller.caller_id,
                artifact,
            )
            .await;
            if let Err(error) = access {
                if !matches!(
                    error,
                    RuntimeError::InvalidRequest("ARTIFACT_REFERENCE_INVALID", _)
                ) {
                    return Err(error);
                }
                let source = source_batch.zip(sources.get(index)).ok_or(error)?;
                require_source_artifact(&mut tx, caller, source.0, source.1.0, &input, artifact)
                    .await?;
            }
            sqlx::query("INSERT INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'batch_item',?,'input')")
                .bind(caller.tenant_id).bind(artifact).bind(item_id.to_string()).execute(&mut *tx).await?;
        }
        let identity =
            crate::user_identity::from_call(caller.kind()?, caller.caller_id, &input, &policy);
        let source = sources.get(index);
        sqlx::query("INSERT INTO application_batch_items(id,tenant_id,batch_id,client_item_id,ordinal,input_json,input_hash,user_identity_json,source_item_id,source_invocation_id) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(item_id).bind(caller.tenant_id).bind(id).bind(item.client_item_id).bind(index as u32+1).bind(&input).bind(hash(&input)?).bind(identity.map(|i|json!(i))).bind(source.map(|s|s.0)).bind(source.and_then(|s|s.1)).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'application_batch',?)").bind(Uuid::now_v7()).bind(caller.tenant_id).bind(bundle).bind(id).execute(&mut *tx).await?;
    if page.is_some() {
        // Existing publication ownership protects the HTML, SDK and style objects as one unit.
        sqlx::query("INSERT IGNORE INTO bundle_references(id,tenant_id,bundle_id,reference_kind,owner_id) VALUES(?,?,?,'batch_page',?)").bind(Uuid::now_v7()).bind(caller.tenant_id).bind(bundle).bind(id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json) VALUES(?,?,'dispatch_batch','batch',?,?,?)")
        .bind(Uuid::now_v7()).bind(caller.tenant_id).bind(id.to_string()).bind(format!("batch:dispatch:{id}")).bind(json!({"batchId":id})).execute(&mut *tx).await?;
    audit(
        &mut tx,
        &scope,
        id,
        "application.batch.created",
        json!({"source":source,"sourceBatchId":source_batch}),
    )
    .await?;
    tx.commit().await?;
    get(state, &scope, id).await
}

/// Only the internal rerun path supplies immutable source rows after object/run
/// authorization. The new actor receives a scoped input reference, not the old identity.
async fn require_source_artifact(
    tx: &mut Transaction<'_, MySql>,
    caller: &BatchCaller,
    batch: Uuid,
    item: Uuid,
    input: &Value,
    artifact: Uuid,
) -> RuntimeResult<()> {
    let source = sqlx::query("SELECT b.caller_json,i.input_hash FROM application_batch_items i JOIN application_batches b ON b.tenant_id=i.tenant_id AND b.id=i.batch_id WHERE i.tenant_id=? AND b.application_id=? AND b.id=? AND i.id=? AND b.retention_deleted_at IS NULL FOR SHARE")
        .bind(caller.tenant_id).bind(caller.application_id).bind(batch).bind(item).fetch_optional(&mut **tx).await?.ok_or(RuntimeError::NotFound)?;
    if source.try_get::<String, _>("input_hash")? != hash(input)? {
        return Err(conflict(
            "Derived input differs from the authorized original row",
        ));
    }
    let original = owner(&source)?;
    crate::artifact_access::require_input(
        tx,
        caller.tenant_id,
        original.kind()?,
        original.caller_id,
        artifact,
    )
    .await
}

pub(super) fn validate(request: &CreateBatchV1, key: &str) -> RuntimeResult<()> {
    if request.name.trim().is_empty()
        || request.name.chars().count() > 200
        || key.is_empty()
        || key.len() > 192
    {
        return Err(invalid(
            "A name of 1–200 characters and stable request key are required",
        ));
    }
    if !(1..=APPLICATION_BATCH_MAX_ITEMS).contains(&request.items.len()) {
        return Err(invalid("A batch must contain 1–100 items"));
    }
    if serde_json::to_vec(request)
        .map_err(|e| RuntimeError::Internal(e.into()))?
        .len()
        > APPLICATION_BATCH_MAX_BYTES
    {
        return Err(invalid("The batch request exceeds 1 MiB"));
    }
    let mut ids = BTreeSet::new();
    for item in &request.items {
        if item.client_item_id.is_empty()
            || item.client_item_id.chars().count() > 128
            || !ids.insert(&item.client_item_id)
        {
            return Err(invalid(
                "Every row needs a unique clientItemId of 1–128 characters",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn equal_contents_are_distinct_business_rows() {
        let request = CreateBatchV1 {
            name: "same".into(),
            items: vec![
                BatchInputV1 {
                    client_item_id: "a".into(),
                    input: json!({"n":1}),
                },
                BatchInputV1 {
                    client_item_id: "b".into(),
                    input: json!({"n":1}),
                },
            ],
        };
        assert!(validate(&request, "key").is_ok());
        let mut duplicate = request;
        duplicate.items[1].client_item_id = "a".into();
        assert!(validate(&duplicate, "key").is_err());
    }
}
