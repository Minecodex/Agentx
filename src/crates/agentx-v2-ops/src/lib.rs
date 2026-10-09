use std::{
    env,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use agentx_control_infrastructure::{
    ControlMySqlSettings, ControlObjectStorageSettings, MySqlTlsMode as ControlMySqlTlsMode,
    connect_control_mysql, control_object_store, migrate_control_mysql, ping_control_mysql,
};
use agentx_runtime_infrastructure::{
    MySqlTlsMode as RuntimeMySqlTlsMode, RuntimeMySqlSettings, RuntimeObjectStorageSettings,
    RuntimeRedisSettings, connect_runtime_mysql, connect_runtime_redis, migrate_runtime_mysql,
    ping_runtime_mysql, runtime_object_store,
};
use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use object_store::{ObjectStore, path::Path};
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Plane {
    Control,
    Runtime,
    Observability,
}

impl Plane {
    pub fn parse(value: Option<String>) -> Result<Self> {
        match value.as_deref() {
            Some("control") => Ok(Self::Control),
            Some("runtime") => Ok(Self::Runtime),
            Some("observability") => Ok(Self::Observability),
            _ => bail!("target must be one of: control, runtime, observability"),
        }
    }
}

#[derive(Clone)]
pub struct ObservabilitySettings {
    pub clickhouse_url: String,
    pub clickhouse_database: String,
    pub clickhouse_user: String,
    pub clickhouse_password: SecretString,
    pub clickhouse_query_user: String,
    pub clickhouse_consumer_user: String,
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_access_key: SecretString,
    pub s3_secret_key: SecretString,
    pub s3_allow_http: bool,
    pub s3_path_style: bool,
}

impl ObservabilitySettings {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            clickhouse_url: required("AGENTX_CLICKHOUSE_URL")?,
            clickhouse_database: value("AGENTX_CLICKHOUSE_DATABASE", "agentx_observability"),
            clickhouse_user: value("AGENTX_CLICKHOUSE_USER", "observability"),
            clickhouse_password: SecretString::from(required("AGENTX_CLICKHOUSE_PASSWORD")?),
            clickhouse_query_user: value("AGENTX_CLICKHOUSE_QUERY_USER", "observability_query"),
            clickhouse_consumer_user: value(
                "AGENTX_CLICKHOUSE_CONSUMER_USER",
                "observability_consumer",
            ),
            s3_endpoint: value("AGENTX_OBSERVABILITY_S3_ENDPOINT", "http://127.0.0.1:9000"),
            s3_bucket: value("AGENTX_OBSERVABILITY_S3_BUCKET", "agentx-observability"),
            s3_region: value("AGENTX_OBSERVABILITY_S3_REGION", "us-east-1"),
            s3_access_key: SecretString::from(value("AGENTX_OBSERVABILITY_S3_ACCESS_KEY", "")),
            s3_secret_key: SecretString::from(value("AGENTX_OBSERVABILITY_S3_SECRET_KEY", "")),
            s3_allow_http: boolean("AGENTX_OBSERVABILITY_S3_ALLOW_HTTP", true)?,
            s3_path_style: boolean("AGENTX_OBSERVABILITY_S3_PATH_STYLE", true)?,
        })
    }

    pub fn clickhouse(&self) -> clickhouse::Client {
        clickhouse::Client::default()
            .with_url(&self.clickhouse_url)
            .with_database(&self.clickhouse_database)
            .with_user(&self.clickhouse_user)
            .with_password(self.clickhouse_password.expose_secret())
    }

    pub fn object_store(&self) -> Result<Arc<dyn ObjectStore>> {
        use object_store::{Certificate, ClientOptions, aws::AmazonS3Builder};
        let mut options = ClientOptions::new().with_allow_http(self.s3_allow_http);
        if let Some(path) =
            env::var_os("AGENTX_OBSERVABILITY_S3_TLS_CA_PATH").filter(|path| !path.is_empty())
        {
            let pem = std::fs::read(path).context("failed reading Observability S3 CA")?;
            let certificates =
                Certificate::from_pem_bundle(&pem).context("invalid Observability S3 CA bundle")?;
            anyhow::ensure!(
                !certificates.is_empty(),
                "Observability S3 CA bundle contains no certificates"
            );
            for certificate in certificates {
                options = options.with_root_certificate(certificate);
            }
        }
        Ok(Arc::new(
            AmazonS3Builder::new()
                .with_bucket_name(&self.s3_bucket)
                .with_endpoint(&self.s3_endpoint)
                .with_region(&self.s3_region)
                .with_access_key_id(self.s3_access_key.expose_secret())
                .with_secret_access_key(self.s3_secret_key.expose_secret())
                .with_virtual_hosted_style_request(!self.s3_path_style)
                .with_client_options(options)
                .build()
                .context("failed to build Observability S3 client")?,
        ))
    }
}

pub async fn migrate(plane: Plane) -> Result<()> {
    assert_plane_environment(plane)?;
    let timeout = migration_timeout()?;
    match plane {
        Plane::Control => {
            let settings = ControlMySqlSettings::from_env()?;
            let pool = connect_control_mysql(&settings).await?;
            tokio::time::timeout(timeout, migrate_control_mysql(&pool))
                .await
                .context("timed out waiting for the Control migration lock")??;
            Ok(())
        }
        Plane::Runtime => {
            let settings = RuntimeMySqlSettings::from_env()?;
            let pool = connect_runtime_mysql(&settings).await?;
            tokio::time::timeout(timeout, migrate_runtime_mysql(&pool))
                .await
                .context("timed out waiting for the Runtime migration lock")??;
            Ok(())
        }
        Plane::Observability => migrate_observability(&ObservabilitySettings::from_env()?).await,
    }
}

fn migration_timeout() -> Result<Duration> {
    let seconds = env::var("AGENTX_MIGRATION_LOCK_TIMEOUT_SECONDS")
        .ok()
        .map(|value| value.parse::<u64>())
        .transpose()
        .context("AGENTX_MIGRATION_LOCK_TIMEOUT_SECONDS must be an integer")?
        .unwrap_or(60);
    ensure!(
        (1..=600).contains(&seconds),
        "AGENTX_MIGRATION_LOCK_TIMEOUT_SECONDS must be between 1 and 600"
    );
    Ok(Duration::from_secs(seconds))
}

async fn migrate_observability(settings: &ObservabilitySettings) -> Result<()> {
    let client = settings.clickhouse();
    client
        .query("CREATE TABLE IF NOT EXISTS observability_schema_migrations (version UInt64, applied_at DateTime64(6, 'UTC') DEFAULT now64(6)) ENGINE = TinyLog")
        .execute()
        .await?;
    acquire_observability_migration_lock(&client).await?;
    let result = migrate_observability_locked(&client, settings).await;
    let unlock = client
        .query("DROP TABLE observability_schema_migration_lock")
        .execute()
        .await
        .context("failed to release Observability migration lock");
    result.and(unlock)
}

async fn acquire_observability_migration_lock(client: &clickhouse::Client) -> Result<()> {
    let deadline = Instant::now() + migration_timeout()?;
    loop {
        let exists = client
            .query("EXISTS TABLE observability_schema_migration_lock")
            .fetch_one::<u8>()
            .await?
            == 1;
        if !exists {
            match client
                .query("CREATE TABLE observability_schema_migration_lock (acquired_at DateTime64(6, 'UTC') DEFAULT now64(6)) ENGINE = Memory")
                .execute()
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let raced = client
                        .query("EXISTS TABLE observability_schema_migration_lock")
                        .fetch_one::<u8>()
                        .await?
                        == 1;
                    if !raced {
                        return Err(error).context("failed to acquire Observability migration lock");
                    }
                }
            }
        }
        ensure!(
            Instant::now() < deadline,
            "timed out waiting for Observability migration lock"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn migrate_observability_locked(
    client: &clickhouse::Client,
    settings: &ObservabilitySettings,
) -> Result<()> {
    let applied = client
        .query("SELECT count() FROM observability_schema_migrations WHERE version = 1")
        .fetch_one::<u64>()
        .await?;
    let schema_ready = client
        .query("EXISTS TABLE workflow_trace_events")
        .fetch_one::<u8>()
        .await?
        == 1;
    if applied == 0 || !schema_ready {
        let migration =
            include_str!("../../../../deploy/migrations/observability/0001_initial.sql")
                .lines()
                .filter(|line| !line.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
        for statement in migration
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
        {
            client.query(statement).execute().await?;
        }
        if applied == 0 {
            client
                .query("INSERT INTO observability_schema_migrations (version) VALUES (1)")
                .execute()
                .await?;
        }
    }
    let v2_05_applied = client
        .query("SELECT count() FROM observability_schema_migrations WHERE version = 2")
        .fetch_one::<u64>()
        .await?;
    if v2_05_applied == 0 {
        let rows = client
            .query("SELECT count() FROM workflow_trace_events")
            .fetch_one::<u64>()
            .await?;
        ensure!(
            rows == 0,
            "V2-05 refuses to rewrite populated V2-04 Trace data; rerun deployment with explicit -RecreateV2Data"
        );
        execute_clickhouse_migration(
            client,
            include_str!(
                "../../../../deploy/migrations/observability/0002_query_and_observability.sql"
            ),
        )
        .await?;
        client
            .query("INSERT INTO observability_schema_migrations (version) VALUES (2)")
            .execute()
            .await?;
    }
    grant_observability_privileges(client, settings).await?;
    Ok(())
}

async fn grant_observability_privileges(
    client: &clickhouse::Client,
    settings: &ObservabilitySettings,
) -> Result<()> {
    for (label, identifier) in [
        ("ClickHouse database", settings.clickhouse_database.as_str()),
        (
            "ClickHouse query user",
            settings.clickhouse_query_user.as_str(),
        ),
        (
            "ClickHouse consumer user",
            settings.clickhouse_consumer_user.as_str(),
        ),
    ] {
        ensure!(
            valid_clickhouse_identifier(identifier),
            "{label} is not a safe identifier"
        );
    }
    let database = &settings.clickhouse_database;
    let query_user = &settings.clickhouse_query_user;
    let consumer_user = &settings.clickhouse_consumer_user;
    for statement in [
        format!("GRANT SELECT ON {database}.* TO {query_user}"),
        format!("GRANT SELECT, INSERT ON {database}.workflow_trace_events TO {consumer_user}"),
        format!("GRANT INSERT ON {database}.trace_ingest_conflicts TO {consumer_user}"),
        format!("GRANT INSERT ON {database}.observability_consumer_health TO {consumer_user}"),
    ] {
        client.query(&statement).execute().await?;
    }
    Ok(())
}

fn valid_clickhouse_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

async fn execute_clickhouse_migration(client: &clickhouse::Client, migration: &str) -> Result<()> {
    let migration = migration
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    for statement in migration
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
    {
        client.query(statement).execute().await?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DoctorReport {
    plane: &'static str,
    database: &'static str,
    database_version: String,
    schema_ready: bool,
    least_privilege_ready: bool,
    database_tls_mode: &'static str,
    database_tls_encrypted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection_pool: Option<ConnectionPoolReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redis_ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redis_tls_encrypted: Option<bool>,
    object_storage_ready: bool,
    object_storage_tls_encrypted: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionPoolReport {
    configured_max: u32,
    server_max: u64,
    allowed_max: u64,
    budget_percent: u64,
}

pub async fn doctor(plane: Plane) -> Result<()> {
    assert_plane_environment(plane)?;
    let report = match plane {
        Plane::Control => doctor_control().await?,
        Plane::Runtime => doctor_runtime().await?,
        Plane::Observability => doctor_observability().await?,
    };
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

async fn doctor_control() -> Result<DoctorReport> {
    let mysql = ControlMySqlSettings::from_env()?;
    let pool = connect_control_mysql(&mysql).await?;
    ping_control_mysql(&pool).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name IN ('tenants','workflows','outbox','runtime_projection_status')",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(count == 4, "Control schema is incomplete");
    let migration: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE version=6 AND success=1")
            .fetch_one(&pool)
            .await?;
    ensure!(migration == 1, "Control migration history is incomplete");
    assert_mysql_system_catalog_denied(&pool, "Control").await?;
    let tls_mode = control_tls_mode(&mysql.tls_mode);
    let (version, tls_encrypted, connection_pool) = mysql_runtime_facts(
        &pool,
        mysql.max_connections,
        tls_mode,
        matches!(mysql.tls_mode, ControlMySqlTlsMode::Disabled),
        matches!(
            mysql.tls_mode,
            ControlMySqlTlsMode::Required
                | ControlMySqlTlsMode::VerifyCa
                | ControlMySqlTlsMode::VerifyIdentity
        ),
    )
    .await?;
    let object_settings = ControlObjectStorageSettings::from_env()?;
    let object_storage_tls_encrypted = endpoint_tls(&object_settings.endpoint);
    ensure!(
        object_settings.allow_http || object_storage_tls_encrypted,
        "Control S3 endpoint must use HTTPS when HTTP is disabled"
    );
    let objects = control_object_store(&object_settings)?;
    object_probe(objects, "control").await?;
    Ok(DoctorReport {
        plane: "control",
        database: "mysql",
        database_version: version,
        schema_ready: true,
        least_privilege_ready: true,
        database_tls_mode: tls_mode,
        database_tls_encrypted: tls_encrypted,
        connection_pool: Some(connection_pool),
        redis_ready: None,
        redis_tls_encrypted: None,
        object_storage_ready: true,
        object_storage_tls_encrypted,
    })
}

async fn doctor_runtime() -> Result<DoctorReport> {
    let mysql = RuntimeMySqlSettings::from_env()?;
    let pool = connect_runtime_mysql(&mysql).await?;
    ping_runtime_mysql(&pool).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name IN ('tenant_admission','workflow_executions','integration_event_log','runtime_query_snapshots','runtime_query_receipts')",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(count == 5, "Runtime schema is incomplete");
    let migration: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE version=6 AND success=1")
            .fetch_one(&pool)
            .await?;
    ensure!(migration == 1, "Runtime migration history is incomplete");
    assert_mysql_system_catalog_denied(&pool, "Runtime").await?;
    let tls_mode = runtime_tls_mode(&mysql.tls_mode);
    let (version, tls_encrypted, connection_pool) = mysql_runtime_facts(
        &pool,
        mysql.max_connections,
        tls_mode,
        matches!(mysql.tls_mode, RuntimeMySqlTlsMode::Disabled),
        matches!(
            mysql.tls_mode,
            RuntimeMySqlTlsMode::Required
                | RuntimeMySqlTlsMode::VerifyCa
                | RuntimeMySqlTlsMode::VerifyIdentity
        ),
    )
    .await?;
    let redis_settings = RuntimeRedisSettings::from_env()?;
    let redis_tls_encrypted = redis_settings.url.expose_secret().starts_with("rediss://");
    let mut redis = connect_runtime_redis(&redis_settings).await?;
    let pong = redis::cmd("PING")
        .query_async::<String>(&mut redis)
        .await
        .context("Runtime Redis health check failed")?;
    ensure!(
        pong == "PONG",
        "Runtime Redis returned an invalid PING response"
    );
    let object_settings = RuntimeObjectStorageSettings::from_env()?;
    let object_storage_tls_encrypted = endpoint_tls(&object_settings.endpoint);
    ensure!(
        object_settings.allow_http || object_storage_tls_encrypted,
        "Runtime S3 endpoint must use HTTPS when HTTP is disabled"
    );
    let objects = runtime_object_store(&object_settings)?;
    object_probe(objects, "runtime").await?;
    Ok(DoctorReport {
        plane: "runtime",
        database: "mysql",
        database_version: version,
        schema_ready: true,
        least_privilege_ready: true,
        database_tls_mode: tls_mode,
        database_tls_encrypted: tls_encrypted,
        connection_pool: Some(connection_pool),
        redis_ready: Some(true),
        redis_tls_encrypted: Some(redis_tls_encrypted),
        object_storage_ready: true,
        object_storage_tls_encrypted,
    })
}

async fn doctor_observability() -> Result<DoctorReport> {
    let settings = ObservabilitySettings::from_env()?;
    let client = settings.clickhouse();
    let exists = client
        .query("EXISTS TABLE workflow_trace_events")
        .fetch_one::<u8>()
        .await?;
    ensure!(exists == 1, "Observability schema is incomplete");
    let migration = client
        .query("SELECT count() FROM observability_schema_migrations WHERE version=2")
        .fetch_one::<u64>()
        .await?;
    ensure!(
        migration >= 1,
        "Observability migration history is incomplete"
    );
    let version = client
        .query("SELECT version()")
        .fetch_one::<String>()
        .await?;
    let database_tls_encrypted = endpoint_tls(&settings.clickhouse_url);
    let object_storage_tls_encrypted = endpoint_tls(&settings.s3_endpoint);
    ensure!(
        settings.s3_allow_http || object_storage_tls_encrypted,
        "Observability S3 endpoint must use HTTPS when HTTP is disabled"
    );
    object_probe(settings.object_store()?, "observability").await?;
    Ok(DoctorReport {
        plane: "observability",
        database: "clickhouse",
        database_version: version,
        schema_ready: true,
        least_privilege_ready: true,
        database_tls_mode: if database_tls_encrypted {
            "https"
        } else {
            "http"
        },
        database_tls_encrypted,
        connection_pool: None,
        redis_ready: None,
        redis_tls_encrypted: None,
        object_storage_ready: true,
        object_storage_tls_encrypted,
    })
}

async fn mysql_runtime_facts(
    pool: &sqlx::MySqlPool,
    configured_max: u32,
    tls_mode: &'static str,
    tls_disabled: bool,
    tls_required: bool,
) -> Result<(String, bool, ConnectionPoolReport)> {
    let version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(pool)
        .await?;
    let status = sqlx::query("SHOW SESSION STATUS LIKE 'Ssl_cipher'")
        .fetch_one(pool)
        .await?;
    let cipher: String = status.try_get(1)?;
    let tls_encrypted = !cipher.is_empty();
    ensure!(
        !tls_disabled || !tls_encrypted,
        "MySQL TLS mode {tls_mode} unexpectedly negotiated encryption"
    );
    ensure!(
        !tls_required || tls_encrypted,
        "MySQL TLS mode {tls_mode} did not negotiate encryption"
    );
    let server_max: u64 = sqlx::query_scalar("SELECT @@GLOBAL.max_connections")
        .fetch_one(pool)
        .await?;
    let allowed_max = server_max.saturating_mul(70) / 100;
    ensure!(
        u64::from(configured_max) <= allowed_max,
        "configured MySQL pool {configured_max} exceeds 70% budget {allowed_max} of server max_connections {server_max}"
    );
    let budget_percent = u64::from(configured_max).saturating_mul(100) / server_max.max(1);
    Ok((
        version,
        tls_encrypted,
        ConnectionPoolReport {
            configured_max,
            server_max,
            allowed_max,
            budget_percent,
        },
    ))
}

fn control_tls_mode(mode: &ControlMySqlTlsMode) -> &'static str {
    match mode {
        ControlMySqlTlsMode::Disabled => "disabled",
        ControlMySqlTlsMode::Preferred => "preferred",
        ControlMySqlTlsMode::Required => "required",
        ControlMySqlTlsMode::VerifyCa => "verify_ca",
        ControlMySqlTlsMode::VerifyIdentity => "verify_identity",
    }
}

fn runtime_tls_mode(mode: &RuntimeMySqlTlsMode) -> &'static str {
    match mode {
        RuntimeMySqlTlsMode::Disabled => "disabled",
        RuntimeMySqlTlsMode::Preferred => "preferred",
        RuntimeMySqlTlsMode::Required => "required",
        RuntimeMySqlTlsMode::VerifyCa => "verify_ca",
        RuntimeMySqlTlsMode::VerifyIdentity => "verify_identity",
    }
}

fn endpoint_tls(endpoint: &str) -> bool {
    endpoint.starts_with("https://")
}

async fn assert_mysql_system_catalog_denied(pool: &sqlx::MySqlPool, plane: &str) -> Result<()> {
    match sqlx::query("SELECT Host FROM mysql.user LIMIT 1")
        .execute(pool)
        .await
    {
        Ok(_) => bail!("{plane} application account can read mysql.user"),
        Err(sqlx::Error::Database(_)) => Ok(()),
        Err(error) => Err(error).context("system catalog denial probe failed unexpectedly"),
    }
}

async fn object_probe(store: Arc<dyn ObjectStore>, plane: &str) -> Result<()> {
    let path = Path::from(format!("doctor/{plane}/{}.probe", Uuid::now_v7()));
    store
        .put(&path, Bytes::from_static(b"agentx-v2-doctor").into())
        .await?;
    let bytes = store.get(&path).await?.bytes().await?;
    let delete = store.delete(&path).await;
    ensure!(
        bytes.as_ref() == b"agentx-v2-doctor",
        "S3 probe returned unexpected bytes"
    );
    delete.context("failed to clean up S3 doctor object")
}

pub async fn bootstrap(plane: Plane) -> Result<()> {
    assert_plane_environment(plane)?;
    match plane {
        Plane::Control => {
            let pool = connect_control_mysql(&ControlMySqlSettings::from_env()?).await?;
            let schema_ready: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='bootstrap_state'",
            )
            .fetch_one(&pool)
            .await?;
            ensure!(schema_ready == 1, "Control bootstrap schema is incomplete");
        }
        Plane::Runtime => {
            let pool = connect_runtime_mysql(&RuntimeMySqlSettings::from_env()?).await?;
            let schema_ready: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='tenant_admission'",
            )
            .fetch_one(&pool)
            .await?;
            ensure!(schema_ready == 1, "Runtime bootstrap schema is incomplete");
        }
        Plane::Observability => {
            let client = ObservabilitySettings::from_env()?.clickhouse();
            let row = client
                .query(
                    "SELECT version FROM observability_schema_migrations WHERE version=2 LIMIT 1",
                )
                .fetch_optional::<u64>()
                .await?;
            ensure!(
                row == Some(2),
                "Observability migration version 2 is not applied"
            );
        }
    }
    Ok(())
}

pub fn assert_plane_environment(plane: Plane) -> Result<()> {
    let present = forbidden_environment_variables(plane, env::vars().map(|(name, _)| name));
    ensure!(
        present.is_empty(),
        "forbidden cross-plane environment variables: {}",
        present.join(", ")
    );
    Ok(())
}

fn forbidden_environment_variables(
    plane: Plane,
    names: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let forbidden: &[&str] = match plane {
        Plane::Control => &[
            "AGENTX_RUNTIME_MYSQL_",
            "AGENTX_RUNTIME_REDIS_",
            "AGENTX_RUNTIME_S3_",
            "AGENTX_OBSERVABILITY_S3_",
            "AGENTX_CLICKHOUSE_",
            "AGENTX_REDIS_",
        ],
        Plane::Runtime => &[
            "AGENTX_CONTROL_MYSQL_",
            "AGENTX_CONTROL_S3_",
            "AGENTX_OBSERVABILITY_S3_",
            "AGENTX_CLICKHOUSE_",
            "AGENTX_REDIS_",
        ],
        Plane::Observability => &[
            "AGENTX_CONTROL_MYSQL_",
            "AGENTX_RUNTIME_MYSQL_",
            "AGENTX_RUNTIME_REDIS_",
            "AGENTX_CONTROL_S3_",
            "AGENTX_RUNTIME_S3_",
            "AGENTX_REDIS_",
        ],
    };
    names
        .into_iter()
        .filter(|name| forbidden.iter().any(|prefix| name.starts_with(prefix)))
        .collect()
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("required environment variable {name} is missing"))
}

fn value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn boolean(name: &str, default: bool) -> Result<bool> {
    match env::var(name) {
        Ok(value) if matches!(value.as_str(), "true" | "1") => Ok(true),
        Ok(value) if matches!(value.as_str(), "false" | "0") => Ok(false),
        Ok(_) => bail!("{name} must be true or false"),
        Err(_) => Ok(default),
    }
}

pub fn certificate_path(name: &str) -> Option<PathBuf> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use secrecy::SecretString;
    use testcontainers::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    };

    use super::{
        ObservabilitySettings, Plane, assert_plane_environment, endpoint_tls,
        forbidden_environment_variables, migrate_observability,
    };

    #[test]
    fn plane_parser_is_strict() {
        assert_eq!(
            Plane::parse(Some("control".into())).unwrap(),
            Plane::Control
        );
        assert!(Plane::parse(Some("legacy".into())).is_err());
    }

    #[test]
    fn normal_test_environment_has_no_cross_plane_credentials() {
        assert_plane_environment(Plane::Observability).unwrap();
    }

    #[test]
    fn every_plane_rejects_cross_plane_secrets_and_legacy_redis() {
        assert_eq!(
            forbidden_environment_variables(
                Plane::Control,
                ["AGENTX_RUNTIME_S3_SECRET_KEY".to_owned()]
            ),
            ["AGENTX_RUNTIME_S3_SECRET_KEY"]
        );
        assert_eq!(
            forbidden_environment_variables(
                Plane::Runtime,
                ["AGENTX_CONTROL_MYSQL_PASSWORD".to_owned()]
            ),
            ["AGENTX_CONTROL_MYSQL_PASSWORD"]
        );
        assert_eq!(
            forbidden_environment_variables(
                Plane::Observability,
                ["AGENTX_RUNTIME_REDIS_PASSWORD".to_owned()]
            ),
            ["AGENTX_RUNTIME_REDIS_PASSWORD"]
        );
        assert_eq!(
            forbidden_environment_variables(Plane::Control, ["AGENTX_REDIS_URL".to_owned()]),
            ["AGENTX_REDIS_URL"]
        );
    }

    #[test]
    fn endpoint_tls_only_accepts_https() {
        assert!(endpoint_tls("https://storage.example"));
        assert!(!endpoint_tls("http://storage.example"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn observability_migration_grants_users_only_after_tables_exist() {
        let container = GenericImage::new("clickhouse/clickhouse-server", "25.3")
            .with_exposed_port(8123.tcp())
            .with_wait_for(WaitFor::seconds(3))
            .with_env_var("CLICKHOUSE_DB", "agentx_observability")
            .with_env_var("CLICKHOUSE_USER", "observability_migrate")
            .with_env_var("CLICKHOUSE_PASSWORD", "agentxtestpassword")
            .with_env_var("CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT", "1")
            .start()
            .await
            .expect("ClickHouse container should start");
        let port = container.get_host_port_ipv4(8123.tcp()).await.unwrap();
        let settings = ObservabilitySettings {
            clickhouse_url: format!("http://127.0.0.1:{port}"),
            clickhouse_database: "agentx_observability".into(),
            clickhouse_user: "observability_migrate".into(),
            clickhouse_password: SecretString::from("agentxtestpassword"),
            clickhouse_query_user: "observability_query".into(),
            clickhouse_consumer_user: "observability_consumer".into(),
            s3_endpoint: "http://127.0.0.1:1".into(),
            s3_bucket: "unused".into(),
            s3_region: "unused".into(),
            s3_access_key: SecretString::from("unused"),
            s3_secret_key: SecretString::from("unused"),
            s3_allow_http: true,
            s3_path_style: true,
        };
        let admin = wait_for_clickhouse(&settings).await;
        admin
            .query("CREATE USER observability_query IDENTIFIED WITH sha256_password BY 'querypassword'")
            .execute()
            .await
            .unwrap();
        admin
            .query("CREATE USER observability_consumer IDENTIFIED WITH sha256_password BY 'consumerpassword'")
            .execute()
            .await
            .unwrap();
        let (first, second) = tokio::join!(
            migrate_observability(&settings),
            migrate_observability(&settings)
        );
        first.unwrap();
        second.unwrap();
        migrate_observability(&settings).await.unwrap();

        let query = clickhouse::Client::default()
            .with_url(&settings.clickhouse_url)
            .with_database(&settings.clickhouse_database)
            .with_user("observability_query")
            .with_password("querypassword");
        assert_eq!(
            query
                .query("SELECT count() FROM workflow_trace_events")
                .fetch_one::<u64>()
                .await
                .unwrap(),
            0
        );
        assert!(
            query
                .query("CREATE TABLE forbidden_query(id UInt64) ENGINE=Memory")
                .execute()
                .await
                .is_err()
        );

        let consumer = clickhouse::Client::default()
            .with_url(&settings.clickhouse_url)
            .with_database(&settings.clickhouse_database)
            .with_user("observability_consumer")
            .with_password("consumerpassword");
        consumer
            .query("INSERT INTO workflow_trace_events(event_id,tenant_id,execution_id,execution_sequence,trace_id,span_id,event_type,status,cost_micros,attributes_json,content_hash,occurred_at) VALUES(generateUUIDv4(),generateUUIDv4(),generateUUIDv4(),1,generateUUIDv4(),generateUUIDv4(),'test','succeeded',0,'{}','sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',now64(6))")
            .execute()
            .await
            .unwrap();
        assert!(
            consumer
                .query("INSERT INTO observability_schema_migrations(version) VALUES(99)")
                .execute()
                .await
                .is_err()
        );
    }

    async fn wait_for_clickhouse(settings: &ObservabilitySettings) -> clickhouse::Client {
        let client = settings.clickhouse();
        let mut last_error = None;
        for _ in 0..60 {
            match client.query("SELECT 1").fetch_one::<u8>().await {
                Ok(1) => return client,
                Ok(_) => {}
                Err(error) => last_error = Some(error),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("ClickHouse did not become ready: {last_error:?}");
    }
}
