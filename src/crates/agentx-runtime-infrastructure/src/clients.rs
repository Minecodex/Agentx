use std::{str::FromStr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use object_store::{Certificate, ClientOptions, ObjectStore, aws::AmazonS3Builder};
use redis::aio::ConnectionManager;
use secrecy::ExposeSecret;

use crate::{RuntimeObjectStorageSettings, RuntimeRedisSettings};

pub async fn connect_runtime_redis(settings: &RuntimeRedisSettings) -> Result<ConnectionManager> {
    let client = runtime_redis_client(settings)?;
    tokio::time::timeout(Duration::from_secs(5), ConnectionManager::new(client))
        .await
        .context("timed out connecting to Runtime Redis")?
        .context("failed to connect to Runtime Redis")
}

pub fn runtime_redis_client(settings: &RuntimeRedisSettings) -> Result<redis::Client> {
    let mut info = redis::ConnectionInfo::from_str(settings.url.expose_secret())
        .context("invalid Runtime Redis URL")?;
    if let Some(password) = &settings.password {
        info.redis.password = Some(password.expose_secret().to_owned());
    }
    // rediss:// against a private CA requires the explicit root bundle; the
    // default webpki roots reject it. Redis rejects TLS settings on redis://.
    let has_client_identity =
        settings.tls_client_cert_path.is_some() && settings.tls_client_key_path.is_some();
    if settings.tls_ca_path.is_some() || has_client_identity {
        let certificates = redis::TlsCertificates {
            root_cert: settings
                .tls_ca_path
                .as_ref()
                .map(|path| {
                    let pem = std::fs::read(path).with_context(|| {
                        format!(
                            "failed to read Runtime Redis CA bundle at {}",
                            path.display()
                        )
                    })?;
                    anyhow::ensure!(!pem.is_empty(), "Runtime Redis CA bundle is empty");
                    Ok(pem)
                })
                .transpose()?,
            client_tls: match (
                &settings.tls_client_cert_path,
                &settings.tls_client_key_path,
            ) {
                (Some(cert_path), Some(key_path)) => Some(redis::ClientTlsConfig {
                    client_cert: std::fs::read(cert_path).with_context(|| {
                        format!(
                            "failed to read Runtime Redis client certificate at {}",
                            cert_path.display()
                        )
                    })?,
                    client_key: std::fs::read(key_path).with_context(|| {
                        format!(
                            "failed to read Runtime Redis client key at {}",
                            key_path.display()
                        )
                    })?,
                }),
                _ => None,
            },
        };
        return redis::Client::build_with_tls(info, certificates)
            .context("invalid Runtime Redis TLS configuration");
    }
    redis::Client::open(info).context("invalid Runtime Redis URL")
}

pub fn runtime_object_store(
    settings: &RuntimeObjectStorageSettings,
) -> Result<Arc<dyn ObjectStore>> {
    let mut client_options = ClientOptions::new().with_allow_http(settings.allow_http);
    if let Some(path) = &settings.tls_ca_path {
        let pem = std::fs::read(path).context("failed to read Runtime S3 CA certificate")?;
        let certificates =
            Certificate::from_pem_bundle(&pem).context("invalid Runtime S3 CA bundle")?;
        anyhow::ensure!(
            !certificates.is_empty(),
            "Runtime S3 CA bundle contains no certificates"
        );
        for certificate in certificates {
            client_options = client_options.with_root_certificate(certificate);
        }
    }
    let mut builder = AmazonS3Builder::new()
        .with_bucket_name(&settings.bucket)
        .with_endpoint(&settings.endpoint)
        .with_region(&settings.region)
        .with_access_key_id(settings.access_key.expose_secret())
        .with_secret_access_key(settings.secret_key.expose_secret())
        .with_virtual_hosted_style_request(!settings.path_style)
        .with_client_options(client_options);
    if let Some(token) = &settings.session_token {
        builder = builder.with_token(token.expose_secret());
    }
    Ok(Arc::new(
        builder
            .build()
            .context("failed to build Runtime S3 object store")?,
    ))
}
