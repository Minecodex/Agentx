//! Page material preparation uses the same Publisher identity, object copier and signing key.
use super::*;
use agentx_runtime_contracts::{
    ActivatePagePublicationV1, ApplicationPagePublicationV1, PreparePagePublicationV1,
    RuntimeObjectUploadMetadataV1, RuntimeObjectUploadReceiptV1,
};

impl Publisher {
    pub(crate) async fn publish_pages(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let id:Option<Uuid>=sqlx::query_scalar("SELECT id FROM application_ui_publications WHERE status IN ('copying','preparing','activating') AND available_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
        let Some(id) = id else {
            tx.commit().await?;
            return Ok(());
        };
        sqlx::query("UPDATE application_ui_publications SET locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=fencing_token+1 WHERE id=?").bind(self.owner.0).bind(id).execute(&mut *tx).await?;
        let row = sqlx::query("SELECT * FROM application_ui_publications WHERE id=?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        let fence: u64 = row.try_get("fencing_token")?;
        tx.commit().await?;
        let result = tokio::time::timeout(Duration::from_secs(25), self.advance_page(&row))
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("Page publish step timed out")));
        if let Err(error) = result {
            let terminal = error
                .downcast_ref::<RuntimeCallError>()
                .is_some_and(RuntimeCallError::terminal);
            let code = error
                .downcast_ref::<RuntimeCallError>()
                .map(RuntimeCallError::code)
                .unwrap_or_else(|| "PAGE_PUBLISH_UNAVAILABLE".into());
            sqlx::query("UPDATE application_ui_publications SET status=IF(?,'rejected',status),last_error=?,locked_by=NULL,locked_until=NULL,available_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 5 SECOND) WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
                .bind(terminal).bind(&code).bind(id).bind(self.owner.0).bind(fence).execute(&self.pool).await?;
            tracing::warn!(publication_id=%id,error_code=%code,retryable=!terminal,"Application page publish step failed");
        }
        Ok(())
    }

    async fn copy_page_asset(
        &self,
        tenant: Uuid,
        version: Uuid,
        object: Uuid,
        bytes: &[u8],
        media_type: &str,
        role: &str,
    ) -> Result<agentx_runtime_contracts::RuntimeObjectReferenceV1> {
        let hash = ContentHash::parse(format!("sha256:{:x}", Sha256::digest(bytes)))?;
        let key = format!("artifacts/{tenant}/{object}");
        self.control_objects
            .put(&ObjectPath::from(key.clone()), bytes.to_vec().into())
            .await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO artifacts(id,tenant_id,content_type,size_bytes,sha256,storage_key) VALUES(?,?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id").bind(object).bind(tenant).bind(media_type).bind(bytes.len() as u64).bind(format!("{:x}",Sha256::digest(bytes))).bind(&key).execute(&mut *tx).await?;
        sqlx::query("INSERT IGNORE INTO artifact_references(tenant_id,artifact_id,owner_type,owner_id,reference_role) VALUES(?,?,'application_page',?,?)").bind(tenant).bind(object).bind(version.to_string()).bind(role).execute(&mut *tx).await?;
        tx.commit().await?;
        let metadata = RuntimeObjectUploadMetadataV1 {
            api_version: 1,
            idempotency_key: format!("page-asset:{object}"),
            tenant_id: tenant,
            object_id: object,
            content_hash: hash,
            size_bytes: bytes.len() as u64,
            media_type: media_type.into(),
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
                        reqwest::multipart::Part::bytes(bytes.to_vec()).mime_str(media_type)?,
                    ),
            )
            .send()
            .await
            .map_err(RuntimeCallError::transport)?;
        let receipt: RuntimeObjectUploadReceiptV1 =
            decode_runtime_response(response).await?.json().await?;
        anyhow::ensure!(
            receipt.object.content_hash == metadata.content_hash
                && receipt.object.object_id == object
                && receipt.object.size_bytes == metadata.size_bytes
                && receipt.object.media_type == media_type,
            "Runtime returned different page materials"
        );
        Ok(receipt.object)
    }

    async fn advance_page(&self, row: &sqlx::mysql::MySqlRow) -> Result<()> {
        let id: Uuid = row.try_get("id")?;
        let tenant: Uuid = row.try_get("tenant_id")?;
        let application: Uuid = row.try_get("application_id")?;
        let fence: u64 = row.try_get("fencing_token")?;
        let phase: String = row.try_get("status")?;
        let next = match phase.as_str() {
            "copying" => {
                let version: Uuid = row.try_get("ui_version_id")?;
                let source=sqlx::query("SELECT * FROM application_ui_versions WHERE tenant_id=? AND application_id=? AND id=?").bind(tenant).bind(application).bind(version).fetch_one(&self.pool).await?;
                let html: String = source.try_get("html")?;
                let object: Uuid = source.try_get("html_object_id")?;
                let html_object = self
                    .copy_page_asset(
                        tenant,
                        version,
                        object,
                        html.as_bytes(),
                        "text/html",
                        "html",
                    )
                    .await?;
                let sdk_source = agentx_runtime_contracts::APPLICATION_PAGE_SDK_SOURCE.as_bytes();
                let style_source =
                    agentx_runtime_contracts::APPLICATION_PAGE_STYLE_SOURCE.as_bytes();
                let sdk_id = agentx_runtime_contracts::deterministic_uuid(
                    tenant,
                    format!("page-sdk:{:x}", Sha256::digest(sdk_source)).as_bytes(),
                );
                let style_id = agentx_runtime_contracts::deterministic_uuid(
                    tenant,
                    format!("page-style:{:x}", Sha256::digest(style_source)).as_bytes(),
                );
                let sdk = self
                    .copy_page_asset(
                        tenant,
                        version,
                        sdk_id,
                        sdk_source,
                        "application/javascript",
                        "sdk",
                    )
                    .await?;
                let stylesheet = self
                    .copy_page_asset(tenant, version, style_id, style_source, "text/css", "style")
                    .await?;
                let initial: Value = row.try_get("manifest_json")?;
                let manifest = ApplicationPagePublicationV1 {
                    tenant_id: tenant,
                    application_id: application,
                    publication_id: id,
                    ui_version_id: version,
                    revision: row.try_get("revision")?,
                    expected_revision: row.try_get("expected_revision")?,
                    slug: row.try_get("slug")?,
                    enabled: initial["enabled"]
                        .as_bool()
                        .context("Publication enable setting is missing")?,
                    contract: serde_json::from_value(source.try_get("contract_json")?)?,
                    contract_hash: ContentHash::parse(
                        source.try_get::<String, _>("contract_hash")?,
                    )?,
                    html: html_object,
                    sdk,
                    stylesheet,
                    html_hash: ContentHash::parse(source.try_get::<String, _>("html_hash")?)?,
                    style_version: agentx_runtime_contracts::APPLICATION_PAGE_STYLE_VERSION,
                };
                let changed=sqlx::query("UPDATE application_ui_publications SET manifest_json=?,status='preparing',last_error=NULL,locked_by=NULL,locked_until=NULL WHERE id=? AND status='copying' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
                    .bind(serde_json::to_value(manifest)?).bind(id).bind(self.owner.0).bind(fence).execute(&self.pool).await?;
                anyhow::ensure!(
                    changed.rows_affected() == 1,
                    "Page publisher lease was lost"
                );
                return Ok(());
            }
            "preparing" => {
                let page: ApplicationPagePublicationV1 =
                    serde_json::from_value(row.try_get("manifest_json")?)?;
                let request = PreparePagePublicationV1 {
                    content_hash: agentx_runtime_contracts::content_hash(&page)?,
                    signature: agentx_runtime_contracts::sign_canonical(
                        &self.bundle_kid,
                        &self.bundle_key,
                        &page,
                    )?,
                    publication: page,
                };
                let receipt: Value = self
                    .post(
                        "runtime.pages.prepare",
                        "/internal/runtime/v1/pages:prepare",
                        &request,
                    )
                    .await?;
                anyhow::ensure!(
                    receipt["prepared"] == true,
                    "Runtime did not acknowledge page preparation"
                );
                "activating"
            }
            "activating" => {
                let page: ApplicationPagePublicationV1 =
                    serde_json::from_value(row.try_get("manifest_json")?)?;
                let request = ActivatePagePublicationV1 {
                    tenant_id: tenant,
                    application_id: application,
                    publication_id: id,
                    expected_revision: page.expected_revision,
                    enabled: page.enabled,
                };
                let receipt: Value = self
                    .post(
                        "runtime.pages.activate",
                        "/internal/runtime/v1/pages:activate",
                        &request,
                    )
                    .await?;
                anyhow::ensure!(
                    receipt["activated"] == true && receipt["revision"] == page.revision,
                    "Runtime did not acknowledge paired activation"
                );
                let mut tx = self.pool.begin().await?;
                let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM application_ui_publications WHERE id=? AND status='activating' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)) FOR UPDATE").bind(id).bind(self.owner.0).bind(fence).fetch_one(&mut *tx).await?;
                anyhow::ensure!(valid, "Page publisher lease was lost");
                let updated=sqlx::query("UPDATE application_web_heads SET publication_id=?,revision=?,slug=?,enabled=? WHERE tenant_id=? AND application_id=? AND revision=?")
                    .bind(id).bind(page.revision).bind(page.slug).bind(page.enabled).bind(tenant).bind(application).bind(page.expected_revision).execute(&mut *tx).await?;
                anyhow::ensure!(
                    updated.rows_affected() == 1,
                    "Control page head changed after Runtime activation"
                );
                sqlx::query("UPDATE application_ui_publications SET status='superseded' WHERE tenant_id=? AND application_id=? AND status='active'").bind(tenant).bind(application).execute(&mut *tx).await?;
                let updated=sqlx::query("UPDATE application_ui_publications SET status='active',last_error=NULL,locked_by=NULL,locked_until=NULL WHERE id=? AND status='activating' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)").bind(id).bind(self.owner.0).bind(fence).execute(&mut *tx).await?;
                anyhow::ensure!(
                    updated.rows_affected() == 1,
                    "Page publisher lease was lost"
                );
                tx.commit().await?;
                return Ok(());
            }
            _ => anyhow::bail!("Invalid page publisher state"),
        };
        let changed=sqlx::query("UPDATE application_ui_publications SET status=?,last_error=NULL,locked_by=NULL,locked_until=NULL WHERE id=? AND status=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)").bind(next).bind(id).bind(phase).bind(self.owner.0).bind(fence).execute(&self.pool).await?;
        anyhow::ensure!(
            changed.rows_affected() == 1,
            "Page publisher lease was lost"
        );
        Ok(())
    }
}
