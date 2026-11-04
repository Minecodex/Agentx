//! Fresh, network-denied previews using the existing Sandbox Manager lifecycle.
use super::*;
use agentx_runtime_contracts::{
    DesignBrowserImageV1, DesignBrowserReportV1, DesignBrowserRequestV1, design_image_owner,
};
use object_store::path::Path as ObjectPath;

// Pin the OCI index so both supported Linux architectures use the same release.
const IMAGE: &str = "opensandbox/playwright@sha256:eaa19eb83afdb52b5216b699acdc23946cddffc29f2baefc5c52d7ebd952ed2d";
const RUNNER: &str = include_str!("design_browser_runner.py");

pub(super) async fn observe(
    State(state): State<SandboxManagerState>,
    Json(request): Json<DesignBrowserRequestV1>,
) -> RuntimeResult<Json<DesignBrowserReportV1>> {
    request.validate().map_err(bad_request)?;
    let tenant = request.target.identity().tenant_id;
    let fingerprint = content_hash(&serde_json::to_value(&request)?)
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    if let Some(row) = sqlx::query("SELECT fingerprint,status,result_json FROM sandbox_design_leases WHERE tenant_id=? AND operation_id=?")
        .bind(tenant).bind(request.effect_id).fetch_optional(&state.pool).await? {
        if row.try_get::<String,_>("fingerprint")? != fingerprint.as_str() { return Err(conflict("Preview effect identity conflict")); }
        if row.try_get::<String,_>("status")? == "completed" {
            return Ok(Json(serde_json::from_value(row.try_get("result_json")?)?));
        }
        return Err(RuntimeError::Unavailable);
    }
    sqlx::query("INSERT INTO sandbox_design_leases(operation_id,tenant_id,fingerprint,owner,status,lease_until) VALUES(?,?,?,?,'creating',UTC_TIMESTAMP(6)+INTERVAL 90 SECOND)")
        .bind(request.effect_id).bind(tenant).bind(fingerprint.as_str()).bind(state.owner).execute(&state.pool).await?;
    let result = execute_preview(&state, &request).await;
    let sandbox: Option<String> = sqlx::query_scalar(
        "SELECT sandbox_id FROM sandbox_design_leases WHERE tenant_id=? AND operation_id=?",
    )
    .bind(tenant)
    .bind(request.effect_id)
    .fetch_one(&state.pool)
    .await?;
    let sandbox = match sandbox {
        Some(id) => Some(id),
        None => find_provider_sandbox(&state, request.effect_id).await,
    };
    let cleanup = if let Some(id) = sandbox {
        terminate_provider(
            &state,
            &id,
            &format!("design-preview:{}", request.effect_id),
        )
        .await
        .is_ok()
    } else {
        true
    };
    if !cleanup {
        sqlx::query("UPDATE sandbox_design_leases SET status='cleanup_pending',lease_until=UTC_TIMESTAMP(6) WHERE operation_id=? AND owner=?")
            .bind(request.effect_id).bind(state.owner).execute(&state.pool).await?;
        return Err(RuntimeError::Unavailable);
    }
    match result {
        Ok(report) => {
            sqlx::query("UPDATE sandbox_design_leases SET status='completed',result_json=?,lease_until=UTC_TIMESTAMP(6) WHERE operation_id=? AND owner=?")
                .bind(serde_json::to_value(&report)?).bind(request.effect_id).bind(state.owner).execute(&state.pool).await?;
            Ok(Json(report))
        }
        Err(error) => {
            tracing::warn!(effect_id=%request.effect_id,error=%error,"Authoring browser observation failed");
            sqlx::query("UPDATE sandbox_design_leases SET status='failed',lease_until=UTC_TIMESTAMP(6) WHERE operation_id=? AND owner=?")
                .bind(request.effect_id).bind(state.owner).execute(&state.pool).await?;
            Err(error)
        }
    }
}

async fn execute_preview(
    state: &SandboxManagerState,
    request: &DesignBrowserRequestV1,
) -> RuntimeResult<DesignBrowserReportV1> {
    let assets = std::env::var("AGENTX_DESIGN_BROWSER_ASSETS_DIR")
        .unwrap_or_else(|_| "/opt/agentx/design-preview".into());
    let script = tokio::fs::read(format!("{assets}/preview.js"))
        .await
        .map_err(|_| bad_request("The deployed design preview assets are unavailable"))?;
    let styles = tokio::fs::read(format!("{assets}/preview.css"))
        .await
        .map_err(|_| bad_request("The deployed design preview styles are unavailable"))?;
    let metadata = BTreeMap::from([
        ("agentxLeaseId".into(), request.effect_id.to_string()),
        (
            "agentxTenantId".into(),
            request.target.identity().tenant_id.to_string(),
        ),
        ("agentxDesignOperation".into(), "browser-preview".into()),
    ]);
    let created = create_provider_sandbox(
        state,
        IMAGE,
        90,
        1000,
        1024 * 1024 * 1024,
        256 * 1024 * 1024,
        SandboxEgressModeV1::None,
        metadata,
        &format!("design-preview:{}", request.effect_id),
    )
    .await
    .map_err(|_| RuntimeError::Unavailable)?;
    let sandbox = created
        .0
        .get("id")
        .and_then(Value::as_str)
        .ok_or(RuntimeError::Unavailable)?;
    sqlx::query("UPDATE sandbox_design_leases SET sandbox_id=?,status='executing' WHERE operation_id=? AND owner=? AND status='creating'")
        .bind(sandbox).bind(request.effect_id).bind(state.owner).execute(&state.pool).await?;
    let (endpoint, headers) = process::provider_execd_endpoint(state, sandbox)
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    wait_execd_ready(state, &endpoint, &headers)
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    for (name, bytes) in [
        ("preview.js", script),
        ("preview.css", styles),
        ("runner.py", RUNNER.as_bytes().to_vec()),
        ("input.json", serde_json::to_vec(request)?),
    ] {
        upload_sandbox_file(
            state,
            &endpoint,
            headers.clone(),
            &format!("/tmp/agentx-preview-{name}"),
            bytes,
        )
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    }
    let response=state.client.post(endpoint.join("command").map_err(|_|RuntimeError::Unavailable)?).headers(headers).json(&json!({"command":"/venv/bin/python /tmp/agentx-preview-runner.py","cwd":"/tmp","background":false,"timeout":45000,"envs":{}})).send().await.map_err(|_|RuntimeError::Unavailable)?;
    if !response.status().is_success() {
        return Err(RuntimeError::ProviderRejected);
    }
    let (stdout, stderr, exit) = resolve::read_resolve_command_result(response).await?;
    if exit != 0 {
        return Err(RuntimeError::InvalidRequest(
            "DESIGN_BROWSER_FAILED",
            format!(
                "The isolated browser could not render this preview: {}",
                stderr.chars().take(2400).collect::<String>()
            ),
        ));
    }
    let mut output: Value =
        serde_json::from_str(stdout.trim()).map_err(|_| RuntimeError::ProviderRejected)?;
    let image = if let Some(image) = output.get_mut("image").filter(|v| !v.is_null()) {
        let encoded = image
            .get("base64")
            .and_then(Value::as_str)
            .ok_or(RuntimeError::ProviderRejected)?;
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| RuntimeError::ProviderRejected)?;
        let width = image.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
        let height = image.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
        if !request.options.screenshot
            || bytes.len() > 576 * 1024
            || bytes.len() < 12
            || &bytes[..4] != b"RIFF"
            || &bytes[8..12] != b"WEBP"
            || width == 0
            || height == 0
            || width > 1440
            || height > 1440
        {
            return Err(RuntimeError::ProviderRejected);
        }
        Some(store_image(state, request, bytes, width, height).await?)
    } else {
        None
    };
    Ok(DesignBrowserReportV1 {
        effect_id: request.effect_id,
        document_hash: request.document_hash.clone(),
        report: output
            .get("report")
            .cloned()
            .ok_or(RuntimeError::ProviderRejected)?,
        image,
    })
}

async fn store_image(
    state: &SandboxManagerState,
    request: &DesignBrowserRequestV1,
    bytes: Vec<u8>,
    width: u32,
    height: u32,
) -> RuntimeResult<DesignBrowserImageV1> {
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let id = agentx_runtime_contracts::deterministic_uuid(request.effect_id, b"screenshot");
    let tenant = request.target.identity().tenant_id;
    let key = format!(
        "design-preview/{tenant}/{}/{id}.webp",
        request.target.identity().session_id
    );
    let size = bytes.len() as u64;
    state
        .objects
        .put(&ObjectPath::from(key.clone()), bytes.into())
        .await
        .map_err(|error| RuntimeError::Internal(error.into()))?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO artifacts(id,tenant_id,content_type,size_bytes,sha256,storage_key) VALUES(?,?,'image/webp',?,?,?)")
        .bind(id).bind(tenant).bind(size).bind(&sha256).bind(&key).execute(&mut *tx).await?;
    // Observations follow the existing artifact retention policy, renewed when used by a model.
    sqlx::query("INSERT INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role,retention_until) VALUES(?,?,'design_preview',?,'visual_observation',DATE_ADD(UTC_TIMESTAMP(6),INTERVAL COALESCE((SELECT retention_days FROM retention_policy_projection WHERE tenant_id=? AND data_type='trace'),30) DAY))")
        .bind(tenant).bind(id).bind(design_image_owner(&request.target)).bind(tenant).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(DesignBrowserImageV1 {
        artifact_id: id,
        sha256,
        content_type: "image/webp".into(),
        width,
        height,
        size_bytes: size,
    })
}
