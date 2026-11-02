use super::*;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_execution_keeps_its_prompt_out_of_another_attempt() {
    let container = GenericImage::new("mysql", "8.4")
        .with_exposed_port(3306.tcp())
        .with_wait_for(WaitFor::message_on_stdout(
            "MySQL init process done. Ready for start up.",
        ))
        .with_wait_for(WaitFor::message_on_stderr("ready for connections"))
        .with_env_var("MYSQL_DATABASE", "agentx_queue_test")
        .with_env_var("MYSQL_ROOT_PASSWORD", "queue-test-password")
        .with_env_var("MYSQL_ROOT_HOST", "%")
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(3306.tcp()).await.unwrap();
    let options = sqlx::mysql::MySqlConnectOptions::new()
        .host("127.0.0.1")
        .port(port)
        .username("root")
        .password("queue-test-password")
        .database("agentx_queue_test")
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled);
    let pool = connect_with_retry(options).await;
    agentx_runtime_infrastructure::migrate_runtime_mysql(&pool)
        .await
        .unwrap();
    let tenant = Uuid::now_v7();
    let attempt = Uuid::now_v7();
    let other = Uuid::now_v7();
    let stale = AgentMessageV1::user("stale-register-copy", "already consumed");
    sqlx::query("INSERT INTO agent_session_registers(tenant_id,session_key,stable_agent_node_key,session_id,bundle_hash,definition_hash,model_version,core_contract_version,register_json) VALUES(?,'session','agent','session','bundle','definition','model','1',?)")
        .bind(tenant).bind(json!({"followUpQueue":[stale]})).execute(&pool).await.unwrap();
    for (sequence, kind, owner, id) in [
        (1_u64, "retry", Some(attempt), "own-execution"),
        (2, "retry", Some(other), "other-execution"),
        (3, "steering", None, "current-guidance"),
        (4, "follow_up", None, "current-follow-up"),
    ] {
        sqlx::query("INSERT INTO agent_session_pending_entries(entry_id,tenant_id,session_key,stable_agent_node_key,attempt_id,queue_kind,idempotency_key,payload_json,sequence_number) VALUES(?,?,'session','agent',?,?,?,?,?)")
            .bind(id).bind(tenant).bind(owner).bind(kind).bind(id)
            .bind(json!(AgentMessageV1::user(id,id))).bind(sequence).execute(&pool).await.unwrap();
    }
    let mut state = DurableStatePort::new(
        pool.clone(),
        Arc::new(object_store::memory::InMemory::new()),
        tenant,
        None,
        Uuid::now_v7(),
        Uuid::now_v7(),
        attempt,
        "session".into(),
        "agent".into(),
        "bundle".into(),
        "definition".into(),
        "model".into(),
        "1".into(),
        u64::MAX,
    );
    let loaded = state.load("session").unwrap().unwrap();
    let ids = loaded
        .follow_up_queue
        .iter()
        .map(|value| value.message_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec!["own-execution", "current-follow-up"],
        "another Workflow Execution must retain its own queued input"
    );
    assert_eq!(loaded.steering_queue.len(), 1);
    let mut next = state.clone();
    next.attempt_id = other;
    let loaded = next.load("session").unwrap().unwrap();
    assert_eq!(
        loaded
            .follow_up_queue
            .iter()
            .map(|value| value.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["other-execution", "current-follow-up"]
    );
    sqlx::query("UPDATE agent_session_pending_entries SET status='consumed' WHERE tenant_id=? AND (attempt_id=? OR queue_kind<>'retry')")
        .bind(tenant).bind(attempt).execute(&pool).await.unwrap();
    let loaded = state.load("session").unwrap().unwrap();
    assert!(
        loaded.follow_up_queue.is_empty() && loaded.steering_queue.is_empty(),
        "the authoritative pending table must not resurrect a stale Register queue copy"
    );
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_session_pending_entries WHERE tenant_id=? AND status='pending'",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        remaining, 1,
        "the other execution's input must remain pending"
    );
    pool.close().await;
}

// Match the existing Control API fixture's bounded MySQL readiness check;
// the image's temporary initialization server can emit an earlier ready log.
async fn connect_with_retry(options: sqlx::mysql::MySqlConnectOptions) -> sqlx::MySqlPool {
    let mut last_error = None;
    for _ in 0..40 {
        match sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(3)
            .connect_with(options.clone())
            .await
        {
            Ok(pool) => return pool,
            Err(error) => last_error = Some(error),
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("connect to queue fixture MySQL: {last_error:?}");
}
