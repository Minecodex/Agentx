async fn trace_batches_are_disjoint_fenced_and_published(pool: &MySqlPool) {
    use agentx_v2_runtime::trace_delivery::{
        TRACE_STREAM, TraceDraft, claim, complete, enqueue, fail, publish,
    };
    let container = GenericImage::new("redis", "7.4-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379.tcp()).await.unwrap();
    let mut redis = redis::Client::open(format!("redis://127.0.0.1:{port}/"))
        .unwrap()
        .get_connection_manager()
        .await
        .unwrap();
    let tenant = Uuid::now_v7();
    let execution = Uuid::now_v7();
    sqlx::query("INSERT INTO workflow_executions(id,tenant_id,workflow_id,workflow_version_id,trace_id,trigger_type,status,started_at) VALUES(?,?,?,?,?,'debug','running',UTC_TIMESTAMP(6))")
        .bind(execution).bind(tenant).bind(Uuid::now_v7()).bind(Uuid::now_v7()).bind(Uuid::now_v7())
        .execute(pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    for index in 0..151 {
        let mut draft = TraceDraft::execution(
            tenant,
            execution,
            format!("execution.batch_{index}"),
            "running",
        );
        draft.attributes = json!({"index": index, "body": "b".repeat(12_000)});
        enqueue(&mut tx, draft).await.unwrap();
    }
    tx.commit().await.unwrap();
    let (first, second) = tokio::join!(claim(pool, Uuid::now_v7()), claim(pool, Uuid::now_v7()));
    let mut first = first.unwrap();
    let mut second = second.unwrap();
    // SKIP LOCKED may return an empty batch while the other transaction is
    // still committing its selection. Claim the remaining rows after both
    // commits; the live first leases must stay exclusive.
    if first.is_empty() {
        first = claim(pool, Uuid::now_v7()).await.unwrap();
    }
    if second.is_empty() {
        second = claim(pool, Uuid::now_v7()).await.unwrap();
    }
    assert_eq!(first.len() + second.len(), 151);
    assert!(first.len() <= 100 && second.len() <= 100 && !first.is_empty() && !second.is_empty());
    let ids: BTreeSet<_> = first
        .iter()
        .chain(&second)
        .map(|row| row.event_id)
        .collect();
    assert_eq!(ids.len(), 151, "relay owners cannot claim the same event");
    fail(pool, &second, "transient transport failure")
        .await
        .unwrap();
    assert!(
        claim(pool, Uuid::now_v7()).await.unwrap().is_empty(),
        "failed rows respect backoff and live leases remain exclusive"
    );
    sqlx::query("UPDATE trace_outbox SET available_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE execution_id=? AND status='failed'")
        .bind(execution).execute(pool).await.unwrap();
    let replacement = claim(pool, Uuid::now_v7()).await.unwrap();
    assert_eq!(replacement.len(), second.len());
    assert!(replacement.iter().all(|row| row.fencing_token == 2));
    let stale_ids = vec!["0-1".to_owned(); second.len()];
    assert!(
        matches!(
            complete(pool, &second, &stale_ids).await,
            Err(RuntimeError::Conflict(_, _))
        ),
        "an old batch cannot complete its replacement leases"
    );
    for claims in [&first, &replacement] {
        let stream_ids = publish(&mut redis, claims).await.unwrap();
        assert_eq!(stream_ids.len(), claims.len());
        assert!(matches!(
            complete(pool, claims, &stream_ids[..stream_ids.len() - 1]).await,
            Err(RuntimeError::Internal(_))
        ));
        complete(pool, claims, &stream_ids).await.unwrap();
    }
    let streamed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trace_outbox WHERE execution_id=? AND status='streamed' AND stream_id IS NOT NULL AND locked_by IS NULL")
        .bind(execution).fetch_one(pool).await.unwrap();
    assert_eq!(streamed, 151);
    let events: redis::streams::StreamRangeReply = redis::cmd("XRANGE")
        .arg(TRACE_STREAM)
        .arg("-")
        .arg("+")
        .query_async(&mut redis)
        .await
        .unwrap();
    assert_eq!(events.ids.len(), 151);
    let receipts: BTreeMap<String, Uuid> = events
        .ids
        .iter()
        .map(|item| {
            let payload: String = redis::from_redis_value(&item.map["payload"]).unwrap();
            (
                item.id.clone(),
                serde_json::from_str::<agentx_runtime_contracts::TraceEventEnvelopeV1>(&payload)
                    .unwrap()
                    .event_id,
            )
        })
        .collect();
    assert_eq!(
        receipts.values().copied().collect::<BTreeSet<_>>(),
        ids,
        "pipeline receipts correspond to actual durable Redis events"
    );
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT event_id,stream_id FROM trace_outbox WHERE execution_id=?")
            .bind(execution)
            .fetch_all(pool)
            .await
            .unwrap();
    for (event_id, stream_id) in rows {
        assert_eq!(
            receipts[&stream_id], event_id,
            "each outbox receipt must point to its own Redis event"
        );
    }
    sqlx::query("DELETE FROM trace_outbox WHERE execution_id=?")
        .bind(execution)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM execution_events WHERE execution_id=?")
        .bind(execution)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM workflow_executions WHERE id=?")
        .bind(execution)
        .execute(pool)
        .await
        .unwrap();
}
