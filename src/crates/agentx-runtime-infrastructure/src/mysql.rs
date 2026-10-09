use anyhow::{Context, Result};
use secrecy::ExposeSecret;
use sqlx::{
    MySqlPool,
    mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode},
};

use crate::{MySqlTlsMode, RuntimeMySqlSettings};

pub async fn connect_runtime_mysql(settings: &RuntimeMySqlSettings) -> Result<MySqlPool> {
    let mut options = MySqlConnectOptions::new()
        .host(&settings.host)
        .port(settings.port)
        .database(&settings.database)
        .username(&settings.username)
        .password(settings.password.expose_secret())
        .ssl_mode(match settings.tls_mode {
            MySqlTlsMode::Disabled => MySqlSslMode::Disabled,
            MySqlTlsMode::Preferred => MySqlSslMode::Preferred,
            MySqlTlsMode::Required => MySqlSslMode::Required,
            MySqlTlsMode::VerifyCa => MySqlSslMode::VerifyCa,
            MySqlTlsMode::VerifyIdentity => MySqlSslMode::VerifyIdentity,
        });
    if let Some(path) = &settings.tls_ca_path {
        options = options.ssl_ca(path);
    }
    if let Some(path) = &settings.tls_client_cert_path {
        options = options.ssl_client_cert(path);
    }
    if let Some(path) = &settings.tls_client_key_path {
        options = options.ssl_client_key(path);
    }
    MySqlPoolOptions::new()
        .max_connections(settings.max_connections)
        .after_connect(|connection, _| {
            Box::pin(async move {
                // Aggregate authority is guarded by explicit parent-row locks,
                // leases and CAS. Missing idempotency keys must not take gap locks
                // that deadlock independent executions during quota reservation.
                sqlx::query("SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .context("failed to connect to Runtime MySQL")
}

pub async fn ping_runtime_mysql(pool: &MySqlPool) -> Result<()> {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .context("Runtime MySQL health check failed")?;
    Ok(())
}

pub async fn migrate_runtime_mysql(pool: &MySqlPool) -> Result<()> {
    reject_populated_v2_03_runtime(pool).await?;
    reject_populated_v2_04_runtime(pool).await?;
    sqlx::migrate!("../../../deploy/migrations/runtime")
        .run(pool)
        .await
        .context("failed to run Runtime MySQL migrations")
}

async fn reject_populated_v2_04_runtime(pool: &MySqlPool) -> Result<()> {
    let migration_table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if !migration_table_exists {
        return Ok(());
    }
    let v2_05_applied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=5 AND success=TRUE)",
    )
    .fetch_one(pool)
    .await?;
    let v2_04_applied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=4 AND success=TRUE)",
    )
    .fetch_one(pool)
    .await?;
    if v2_05_applied || !v2_04_applied {
        return Ok(());
    }
    let business_rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM deployment_bundles) + (SELECT COUNT(*) FROM application_sessions) + (SELECT COUNT(*) FROM application_invocations) + (SELECT COUNT(*) FROM workflow_executions) + (SELECT COUNT(*) FROM runtime_work_packages)",
    )
    .fetch_one(pool)
    .await?;
    anyhow::ensure!(
        business_rows == 0,
        "V2-05 refuses to rewrite a populated V2-04 Runtime domain; rerun deployment with explicit -RecreateV2Data"
    );
    Ok(())
}

async fn reject_populated_v2_03_runtime(pool: &MySqlPool) -> Result<()> {
    let migration_table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await?;
    if !migration_table_exists {
        return Ok(());
    }
    let v2_04_applied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=4 AND success=TRUE)",
    )
    .fetch_one(pool)
    .await?;
    let v2_03_applied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=3 AND success=TRUE)",
    )
    .fetch_one(pool)
    .await?;
    if v2_04_applied || !v2_03_applied {
        return Ok(());
    }
    let business_rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM deployment_bundles) + (SELECT COUNT(*) FROM application_sessions) + (SELECT COUNT(*) FROM application_invocations) + (SELECT COUNT(*) FROM workflow_executions)",
    )
    .fetch_one(pool)
    .await?;
    anyhow::ensure!(
        business_rows == 0,
        "V2-04 refuses to rewrite a populated V2-03 Runtime domain; rerun deployment with explicit -RecreateV2Data"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{migrate_runtime_mysql, reject_populated_v2_04_runtime};
    use sqlx::{MySqlPool, mysql::MySqlPoolOptions};
    use testcontainers::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    };

    #[tokio::test]
    async fn populated_v2_04_runtime_requires_an_explicit_recreate() {
        let container = GenericImage::new("mysql", "8.4")
            .with_exposed_port(3306.tcp())
            .with_wait_for(WaitFor::message_on_stdout(
                "MySQL init process done. Ready for start up.",
            ))
            .with_wait_for(WaitFor::message_on_stderr("ready for connections"))
            .with_env_var("MYSQL_DATABASE", "agentx_runtime")
            .with_env_var("MYSQL_USER", "agentx")
            .with_env_var("MYSQL_PASSWORD", "agentx-test-password")
            .with_env_var("MYSQL_ROOT_PASSWORD", "agentx-root-password")
            .start()
            .await
            .unwrap();
        let port = container.get_host_port_ipv4(3306.tcp()).await.unwrap();
        let pool = connect(port, "agentx_runtime").await;
        sqlx::raw_sql(
            "CREATE TABLE _sqlx_migrations(version BIGINT PRIMARY KEY,success BOOLEAN NOT NULL);\
             INSERT INTO _sqlx_migrations VALUES(4,TRUE);\
             CREATE TABLE deployment_bundles(id BIGINT PRIMARY KEY);\
             CREATE TABLE application_sessions(id BIGINT PRIMARY KEY);\
             CREATE TABLE application_invocations(id BIGINT PRIMARY KEY);\
             CREATE TABLE workflow_executions(id BIGINT PRIMARY KEY);\
             CREATE TABLE runtime_work_packages(id BIGINT PRIMARY KEY);\
             INSERT INTO workflow_executions VALUES(1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let error = reject_populated_v2_04_runtime(&pool)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("-RecreateV2Data"), "{error}");
        sqlx::query("DELETE FROM workflow_executions")
            .execute(&pool)
            .await
            .unwrap();
        reject_populated_v2_04_runtime(&pool).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn empty_runtime_migrations_are_concurrent_and_replayable() {
        let container = mysql_container("agentx_runtime_migrations").await;
        let port = container.get_host_port_ipv4(3306.tcp()).await.unwrap();
        let pool = connect(port, "agentx_runtime_migrations").await;

        let (first, second) =
            tokio::join!(migrate_runtime_mysql(&pool), migrate_runtime_mysql(&pool));
        first.unwrap();
        second.unwrap();
        migrate_runtime_mysql(&pool).await.unwrap();

        let applied: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _sqlx_migrations WHERE version=6 AND success=TRUE",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(applied, 1);
        let snapshot_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='runtime_query_snapshots')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(snapshot_exists);
        let index_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM information_schema.statistics WHERE table_schema=DATABASE() AND table_name='node_attempts' AND index_name='idx_v206_node_attempt_claim')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(index_exists);
    }

    async fn mysql_container(database: &str) -> testcontainers::ContainerAsync<GenericImage> {
        GenericImage::new("mysql", "8.4")
            .with_exposed_port(3306.tcp())
            .with_wait_for(WaitFor::message_on_stdout(
                "MySQL init process done. Ready for start up.",
            ))
            .with_wait_for(WaitFor::message_on_stderr("ready for connections"))
            .with_env_var("MYSQL_DATABASE", database)
            .with_env_var("MYSQL_USER", "agentx")
            .with_env_var("MYSQL_PASSWORD", "agentx-test-password")
            .with_env_var("MYSQL_ROOT_PASSWORD", "agentx-root-password")
            .start()
            .await
            .unwrap()
    }

    async fn connect(port: u16, database: &str) -> MySqlPool {
        let url = format!("mysql://agentx:agentx-test-password@127.0.0.1:{port}/{database}");
        for _ in 0..40 {
            if let Ok(pool) = MySqlPoolOptions::new().connect(&url).await {
                return pool;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        panic!("MySQL test container did not become ready")
    }
}
