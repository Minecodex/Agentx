use super::dispatch::{BatchCommandClaim, command_tx, finish_tx};
use super::*;
use bytes::Bytes;
use object_store::{ObjectStore, PutMode, PutOptions, path::Path as ObjectPath};
use sha2::{Digest, Sha256};

const MAX_EXPORT: usize = 32 * 1024 * 1024;

pub async fn parse_table(
    state: &RuntimeState,
    caller: &BatchCaller,
    request: ReadTableV1,
) -> RuntimeResult<TableDataV1> {
    let bytes = match (request.artifact_id, request.text) {
        (Some(id), None) => {
            let mut tx = state.pool.begin().await?;
            crate::artifact_access::require_input(
                &mut tx,
                caller.tenant_id,
                caller.kind()?,
                caller.caller_id,
                id,
            )
            .await?;
            tx.commit().await?;
            let row=sqlx::query("SELECT content_type,size_bytes,storage_key,sha256 FROM artifacts WHERE tenant_id=? AND id=? AND deleted_at IS NULL").bind(caller.tenant_id).bind(id).fetch_one(&state.pool).await?;
            if row.try_get::<u64, _>("size_bytes")? > APPLICATION_BATCH_MAX_BYTES as u64
                || !matches!(
                    row.try_get::<String, _>("content_type")?.split(';').next(),
                    Some(
                        "text/csv" | "application/csv" | "text/tab-separated-values" | "text/plain"
                    )
                )
            {
                return Err(invalid("Import a UTF-8 CSV/TSV file no larger than 1 MiB"));
            }
            read_bytes(state, &row, APPLICATION_BATCH_MAX_BYTES)
                .await?
                .to_vec()
        }
        (None, Some(text)) => text.into_bytes(),
        _ => {
            return Err(invalid(
                "Supply exactly one owned artifactId or pasted text",
            ));
        }
    };
    parse(&bytes, &request.delimiter)
}

pub(super) fn parse(bytes: &[u8], delimiter: &str) -> RuntimeResult<TableDataV1> {
    if bytes.len() > APPLICATION_BATCH_MAX_BYTES {
        return Err(invalid("The table exceeds 1 MiB"));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid("Only UTF-8 text is accepted"))?
        .trim_start_matches('\u{feff}');
    let delimiter = match delimiter {
        "," => b',',
        "\t" => b'\t',
        _ => return Err(invalid("The delimiter must be comma or tab")),
    };
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(true)
        .flexible(false)
        .from_reader(text.as_bytes());
    let header = reader
        .headers()
        .map_err(|e| invalid(format!("Invalid CSV header: {e}")))?
        .clone();
    let mut names = std::collections::BTreeSet::new();
    if header.is_empty()
        || header.len() > 256
        || header
            .iter()
            .any(|v| v.trim().is_empty() || !names.insert(v.to_owned()))
    {
        return Err(invalid("Use 1–256 nonempty, unique column names"));
    }
    let mut result = TableDataV1 {
        headers: header.iter().map(str::to_owned).collect(),
        rows: Vec::new(),
        lines: Vec::new(),
    };
    for record in reader.records() {
        let record = record.map_err(|e| invalid(format!("Invalid CSV record: {e}")))?;
        if result.rows.len() == APPLICATION_BATCH_MAX_ITEMS {
            return Err(invalid("The table exceeds 100 data rows"));
        }
        result.lines.push(
            record
                .position()
                .map_or(result.rows.len() as u64 + 2, |p| p.line()),
        );
        result.rows.push(record.iter().map(str::to_owned).collect());
    }
    if result.rows.is_empty() {
        return Err(invalid("The table contains no data rows"));
    }
    if serde_json::to_vec(&result)
        .map_err(|e| RuntimeError::Internal(e.into()))?
        .len()
        > APPLICATION_BATCH_MAX_BYTES
    {
        return Err(invalid(
            "The parsed table exceeds the Bridge response limit",
        ));
    }
    Ok(result)
}

pub async fn request_export(
    state: &RuntimeState,
    scope: &BatchScope,
    id: Uuid,
    key: &str,
) -> RuntimeResult<BatchExportV1> {
    let mut tx = crate::task_view::snapshot(&state.pool).await?;
    let batch = row(&mut tx, scope, id, false).await?;
    let export_id = deterministic_uuid(
        scope.caller.tenant_id,
        format!("batch:export:{id}:{}:{key}", scope.caller.caller_id).as_bytes(),
    );
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_commands WHERE tenant_id=? AND id=? AND aggregate_type='batch' AND command_type='export_batch')").bind(scope.caller.tenant_id).bind(export_id).fetch_one(&mut *tx).await?;
    if !exists {
        let snapshot = super::read::views_tx(&mut tx, &batch).await?;
        let bundle: Value = sqlx::query_scalar(
            "SELECT payload_json FROM deployment_bundles WHERE tenant_id=? AND id=?",
        )
        .bind(scope.caller.tenant_id)
        .bind(batch.try_get::<Uuid, _>("bundle_id")?)
        .fetch_one(&mut *tx)
        .await?;
        let spec: ExecutionSpecPayloadV2 =
            serde_json::from_value(bundle).map_err(|e| RuntimeError::Internal(e.into()))?;
        let payload = json!({"batchId":id,"requestedBy":scope.caller,"snapshot":snapshot,"outputSchema":spec.output_contract,"snapshotAt":time::OffsetDateTime::now_utc().unix_timestamp()});
        if serde_json::to_vec(&payload)
            .map_err(|e| RuntimeError::Internal(e.into()))?
            .len()
            > MAX_EXPORT
        {
            return Err(invalid("The export snapshot exceeds 32 MiB"));
        }
        sqlx::query("INSERT IGNORE INTO runtime_commands(id,tenant_id,command_type,aggregate_type,aggregate_id,idempotency_key,payload_json) VALUES(?,?,'export_batch','batch',?,?,?)")
            .bind(export_id).bind(scope.caller.tenant_id).bind(id.to_string()).bind(format!("batch:export:{export_id}")).bind(payload).execute(&mut *tx).await?;
        audit(
            &mut tx,
            scope,
            id,
            "application.batch.export.requested",
            json!({"exportId":export_id}),
        )
        .await?;
    }
    tx.commit().await?;
    get_export(state, scope, id, export_id).await
}

pub async fn get_export(
    state: &RuntimeState,
    scope: &BatchScope,
    id: Uuid,
    export_id: Uuid,
) -> RuntimeResult<BatchExportV1> {
    let mut tx = state.pool.begin().await?;
    row(&mut tx, scope, id, false).await?;
    let job=sqlx::query("SELECT status,result_json,error_message,payload_json FROM runtime_commands WHERE tenant_id=? AND id=? AND command_type='export_batch' AND aggregate_type='batch' AND aggregate_id=?").bind(scope.caller.tenant_id).bind(export_id).bind(id.to_string()).fetch_optional(&mut *tx).await?.ok_or(RuntimeError::NotFound)?;
    let payload: Value = job.try_get("payload_json")?;
    if !scope.management && payload["requestedBy"]["callerId"] != json!(scope.caller.caller_id) {
        return Err(RuntimeError::NotFound);
    }
    let file = job
        .try_get::<Option<Value>, _>("result_json")?
        .and_then(|v| v.get("file").cloned())
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| RuntimeError::Internal(e.into()))?;
    let view = BatchExportV1 {
        export_id,
        batch_id: id,
        status: job.try_get("status")?,
        file,
        error: job.try_get("error_message")?,
    };
    tx.commit().await?;
    Ok(view)
}

pub async fn download_export(
    state: &RuntimeState,
    scope: &BatchScope,
    id: Uuid,
    export: Uuid,
) -> RuntimeResult<Bytes> {
    let job = get_export(state, scope, id, export).await?;
    let file = job
        .file
        .ok_or_else(|| conflict("The export has not completed"))?;
    let row=sqlx::query("SELECT size_bytes,storage_key,sha256 FROM artifacts WHERE tenant_id=? AND id=? AND deleted_at IS NULL").bind(scope.caller.tenant_id).bind(file.artifact_id).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    let bytes = read_bytes(state, &row, MAX_EXPORT).await?;
    let mut tx = state.pool.begin().await?;
    audit(
        &mut tx,
        scope,
        id,
        "application.batch.export.downloaded",
        json!({"exportId":export}),
    )
    .await?;
    tx.commit().await?;
    Ok(bytes)
}

async fn read_bytes(
    state: &RuntimeState,
    row: &sqlx::mysql::MySqlRow,
    limit: usize,
) -> RuntimeResult<Bytes> {
    let size: u64 = row.try_get("size_bytes")?;
    if size > limit as u64 {
        return Err(invalid("File exceeds the operation limit"));
    }
    let bytes = state
        .objects
        .get(&ObjectPath::from(row.try_get::<String, _>("storage_key")?))
        .await
        .map_err(|e| RuntimeError::Internal(e.into()))?
        .bytes()
        .await
        .map_err(|e| RuntimeError::Internal(e.into()))?;
    if bytes.len() as u64 != size
        || format!("{:x}", Sha256::digest(&bytes)) != row.try_get::<String, _>("sha256")?
    {
        return Err(RuntimeError::Unavailable);
    }
    Ok(bytes)
}

pub(super) async fn process_export(
    state: &RuntimeState,
    claim: &BatchCommandClaim,
) -> RuntimeResult<()> {
    let snapshot: Vec<BatchItemViewV1> = serde_json::from_value(claim.payload["snapshot"].clone())
        .map_err(|e| RuntimeError::Internal(e.into()))?;
    let bytes = export_csv(&snapshot, &claim.payload["outputSchema"])?;
    let artifact = deterministic_uuid(claim.id, b"csv");
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let key = format!(
        "{}/batch-exports/{}/{}.csv",
        claim.tenant, claim.batch, claim.id
    );
    let mut tx = state.pool.begin().await?;
    command_tx(&mut tx, claim).await?;
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT sha256 FROM artifacts WHERE tenant_id=? AND id=? AND deleted_at IS NULL",
    )
    .bind(claim.tenant)
    .bind(artifact)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(existing) = existing {
        if existing != sha {
            return Err(conflict("The frozen export changed"));
        }
    } else {
        // Reserve quota before writing bytes; metadata, reference and command completion commit together.
        crate::artifact::commit_upload_tx(
            &mut tx,
            claim.tenant,
            artifact,
            "text/csv",
            bytes.len() as u64,
            &sha,
            &key,
            &format!("batch-export:{}", claim.batch),
            &format!("batch-export:{}", claim.id),
            &sha,
        )
        .await?;
        let path = ObjectPath::from(key.clone());
        match state
            .objects
            .put_opts(
                &path,
                bytes.clone().into(),
                PutOptions {
                    mode: PutMode::Create,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(_) => {}
            Err(object_store::Error::AlreadyExists { .. }) => {
                let stored = state
                    .objects
                    .get(&path)
                    .await
                    .map_err(|e| RuntimeError::Internal(e.into()))?
                    .bytes()
                    .await
                    .map_err(|e| RuntimeError::Internal(e.into()))?;
                if format!("{:x}", Sha256::digest(&stored)) != sha {
                    return Err(conflict("The export object hash changed"));
                }
            }
            Err(e) => return Err(RuntimeError::Internal(e.into())),
        }
    }
    let file = ArtifactUploadResponseV1 {
        artifact_id: artifact,
        content_type: "text/csv".into(),
        size_bytes: bytes.len() as u64,
        sha256: sha,
    };
    sqlx::query("INSERT IGNORE INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'batch_export',?,'result')").bind(claim.tenant).bind(artifact).bind(claim.batch.to_string()).execute(&mut *tx).await?;
    finish_tx(&mut tx, claim, json!({"file":file}), false).await?;
    tx.commit().await?;
    Ok(())
}

fn export_csv(snapshot: &[BatchItemViewV1], schema: &Value) -> RuntimeResult<Vec<u8>> {
    let mut columns = std::collections::BTreeSet::new();
    fn collect(schema: &Value, path: &str, out: &mut std::collections::BTreeSet<String>) {
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (name, child) in properties {
                collect(
                    child,
                    &format!("{path}/{}", name.replace('~', "~0").replace('/', "~1")),
                    out,
                )
            }
        } else if !path.is_empty() {
            out.insert(path.to_owned());
        }
        for key in ["anyOf", "oneOf", "allOf"] {
            if let Some(variants) = schema.get(key).and_then(Value::as_array) {
                for variant in variants {
                    collect(variant, path, out)
                }
            }
        }
    }
    collect(schema, "", &mut columns);
    let mut writer = csv::Writer::from_writer(Vec::new());
    let mut headers = [
        "itemId",
        "ordinal",
        "clientItemId",
        "state",
        "input",
        "output",
        "error",
        "sourceItemId",
        "sourceInvocationId",
    ]
    .map(str::to_owned)
    .to_vec();
    headers.extend(columns.iter().map(|p| format!("output{p}")));
    writer
        .write_record(headers)
        .map_err(|e| RuntimeError::Internal(e.into()))?;
    for item in snapshot {
        let output = item.task.as_ref().and_then(|t| t.outputs.as_ref());
        let mut fields = vec![
            item.item_id.to_string(),
            item.ordinal.to_string(),
            formula_safe(&item.client_item_id),
            item.state.clone(),
            item.input.to_string(),
            output.map(Value::to_string).unwrap_or_default(),
            item.error
                .as_ref()
                .or_else(|| item.task.as_ref().and_then(|t| t.error.as_ref()))
                .map(Value::to_string)
                .unwrap_or_default(),
            item.source_item_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            item.source_invocation_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
        ];
        fields.extend(
            columns
                .iter()
                .map(|path| csv_value(output.and_then(|v| v.pointer(path)))),
        );
        writer
            .write_record(fields)
            .map_err(|e| RuntimeError::Internal(e.into()))?;
    }
    let bytes = writer
        .into_inner()
        .map_err(|e| RuntimeError::Internal(e.into()))?;
    if bytes.len() > MAX_EXPORT {
        return Err(invalid("The CSV export exceeds 32 MiB"));
    }
    Ok(bytes)
}
fn csv_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => formula_safe(value),
        Some(value) => value.to_string(),
    }
}

fn formula_safe(value: &str) -> String {
    if value
        .trim_start_matches([' ', '\t', '\r', '\n'])
        .starts_with(['=', '+', '-', '@'])
        || value.starts_with(['\t', '\r'])
    {
        format!("'{value}")
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_multiline_and_duplicate_rows_are_preserved() {
        let data = parse(b"name,n\n\"first\nsecond\",1\na,2\na,2\n", ",").unwrap();
        assert_eq!(data.rows.len(), 3);
        assert_eq!(data.rows[0][0], "first\nsecond");
        assert_eq!(data.lines, vec![2, 4, 5]);
    }
    #[test]
    fn rejects_overflow_and_malformed_columns() {
        assert!(parse(b"a,b\n1\n", ",").is_err());
        let text = format!("a\n{}", "x\n".repeat(101));
        assert!(parse(text.as_bytes(), ",").is_err());
        assert_eq!(formula_safe(" =SUM(A1:A2)"), "' =SUM(A1:A2)");
        assert_eq!(formula_safe("normal"), "normal");
    }
}
