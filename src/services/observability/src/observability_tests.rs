use std::time::Duration;

use agentx_runtime_contracts::{
    ObservabilityAggregateRequestV1, ObservabilityDimensionV1, ObservabilityMetricV1,
    TraceContentKindV1, TraceEventEnvelopeV1, TraceEventKindV1, TraceSpanKindV1, content_hash,
};
use clickhouse::Client;
use serde_json::json;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    AggregateRow, TraceConflictRow, TraceRow, TraceSpanKeyRow, aggregate_query_sql,
    aggregate_spans, existing_trace_hash, ingest_trace_batch, parse_span_cursor, span_cursor,
    span_page_sql, timestamp_micros, trace_contents, trace_ingestion_snapshot,
};

#[test]
fn span_aggregation_handles_out_of_order_and_incomplete_lifecycles() {
    let execution_id = Uuid::now_v7();
    let span_id = Uuid::now_v7();
    let started = OffsetDateTime::from_unix_timestamp(100).unwrap();
    let finished = started + time::Duration::seconds(2);
    let mut finished_event = trace_event(
        execution_id,
        span_id,
        2,
        TraceEventKindV1::Finished,
        "failed",
        finished,
    );
    finished_event.span_name = "stale technical key".into();
    let events = vec![
        finished_event,
        trace_event(
            execution_id,
            span_id,
            1,
            TraceEventKindV1::Started,
            "running",
            started,
        ),
    ];
    let span = aggregate_spans(&events).pop().unwrap();
    assert_eq!(span.started_at, started);
    assert_eq!(span.ended_at, Some(finished));
    assert_eq!(span.duration_ms, Some(2_000));
    assert_eq!(span.status, "failed");
    assert_eq!(span.span_name, "Model call");

    let mut usage_event = trace_event(
        execution_id,
        span_id,
        3,
        TraceEventKindV1::Updated,
        "running",
        started + time::Duration::seconds(1),
    );
    usage_event.input_tokens = Some(12);
    usage_event.output_tokens = Some(8);
    usage_event.cost_micros = 42;
    let mut completed_without_usage = trace_event(
        execution_id,
        span_id,
        4,
        TraceEventKindV1::Finished,
        "succeeded",
        finished,
    );
    completed_without_usage.span_name = "stale technical key".into();
    let usage_span = aggregate_spans(&[completed_without_usage, usage_event])
        .pop()
        .unwrap();
    assert_eq!(usage_span.input_tokens, Some(12));
    assert_eq!(usage_span.output_tokens, Some(8));
    assert_eq!(usage_span.cost_micros, 42);

    let running_id = Uuid::now_v7();
    let running = aggregate_spans(&[trace_event(
        execution_id,
        running_id,
        3,
        TraceEventKindV1::Started,
        "running",
        started,
    )])
    .pop()
    .unwrap();
    assert_eq!(running.ended_at, None);
    assert_eq!(running.duration_ms, None);
}

#[test]
fn span_cursor_round_trips_the_stable_sort_tuple() {
    let event = trace_event(
        Uuid::now_v7(),
        Uuid::now_v7(),
        1,
        TraceEventKindV1::Started,
        "running",
        OffsetDateTime::from_unix_timestamp(123).unwrap() + time::Duration::microseconds(456),
    );
    let span = aggregate_spans(&[event]).pop().unwrap();
    assert_eq!(
        parse_span_cursor(&span_cursor(&span)).unwrap(),
        (span.started_at, span.span_id)
    );
    assert!(parse_span_cursor("broken").is_err());
}

#[test]
fn span_detail_keeps_every_semantic_content_in_event_order() {
    let execution_id = Uuid::now_v7();
    let span_id = Uuid::now_v7();
    let started = OffsetDateTime::from_unix_timestamp(200).unwrap();
    let mut request = trace_event(
        execution_id,
        span_id,
        1,
        TraceEventKindV1::Started,
        "running",
        started,
    );
    request.content_kind = Some(TraceContentKindV1::RuntimeRequest);
    request.content_preview = Some(json!({"messages":[{"role":"system","content":"你叫 kakj"}]}));
    let request_id = request.event_id;
    let mut response = trace_event(
        execution_id,
        span_id,
        2,
        TraceEventKindV1::Finished,
        "succeeded",
        started + time::Duration::seconds(1),
    );
    response.content_kind = Some(TraceContentKindV1::RuntimeResponse);
    response.content_preview = Some(json!({"text":"你好，我叫 kakj。"}));
    let response_id = response.event_id;

    let contents = trace_contents(&[request, response]);
    assert_eq!(contents.len(), 2);
    assert_eq!(contents[0].event_id, request_id);
    assert_eq!(contents[0].kind, TraceContentKindV1::RuntimeRequest);
    assert_eq!(
        contents[0].preview.as_ref().unwrap()["messages"][0]["role"],
        "system"
    );
    assert_eq!(contents[1].event_id, response_id);
    assert_eq!(contents[1].kind, TraceContentKindV1::RuntimeResponse);
    assert_eq!(
        contents[1].preview.as_ref().unwrap()["text"],
        "你好，我叫 kakj。"
    );
}

#[test]
fn boundary_span_and_node_filter_are_part_of_the_strong_contract() {
    assert_eq!(
        super::parse_trace_span_kind("boundary").unwrap(),
        TraceSpanKindV1::Boundary
    );
    assert_eq!(
        super::trace_span_kind_name(TraceSpanKindV1::Boundary),
        "boundary"
    );
    let filtered = span_page_sql(false, true);
    assert!(filtered.contains("node_execution_id=?"));
    assert!(!span_page_sql(false, false).contains("node_execution_id=?"));
}

fn trace_event(
    execution_id: Uuid,
    span_id: Uuid,
    sequence: u64,
    event_kind: TraceEventKindV1,
    status: &str,
    occurred_at: OffsetDateTime,
) -> TraceEventEnvelopeV1 {
    TraceEventEnvelopeV1 {
        schema_version: 1,
        event_id: Uuid::now_v7(),
        tenant_id: Uuid::now_v7(),
        execution_id,
        execution_sequence: sequence,
        trace_id: Uuid::now_v7(),
        workflow_id: None,
        application_id: None,
        span_id,
        parent_span_id: None,
        event_kind,
        span_kind: TraceSpanKindV1::RuntimeCall,
        span_name: "Model call".into(),
        node_execution_id: None,
        attempt_id: None,
        agent_run_id: None,
        agent_iteration_id: None,
        runtime_call_id: Some(span_id),
        sandbox_lease_id: None,
        wait_id: None,
        resource_type: Some("model".into()),
        resource_id: None,
        resource_version: None,
        event_type: "runtime_call.lifecycle".into(),
        status: status.into(),
        duration_ms: None,
        input_tokens: None,
        output_tokens: None,
        cost_micros: 0,
        error_code: (status == "failed").then(|| "MODEL_FAILED".into()),
        error_message: (status == "failed").then(|| "Model request failed".into()),
        attributes: json!({}),
        content_ref: None,
        content_kind: None,
        content_preview: None,
        occurred_at,
        content_hash: content_hash(&json!({"sequence":sequence})).unwrap(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clickhouse_trace_queries_decode_aggregate_and_exclude_conflicts() {
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
    let admin = clickhouse_with_retry(port).await;
    execute_migration(
        &admin,
        include_str!("../../../../deploy/migrations/observability/0001_initial.sql"),
    )
    .await;
    execute_migration(
        &admin,
        include_str!(
            "../../../../deploy/migrations/observability/0002_query_and_observability.sql"
        ),
    )
    .await;

    let tenant_id = Uuid::now_v7();
    let execution_id = Uuid::now_v7();
    let conflict_event_id = Uuid::now_v7();
    let clean_event_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    batch_ingestion_replays_and_concurrent_conflicts_are_durable(&admin).await;
    assert_eq!(
        existing_trace_hash(&admin, clean_event_id).await.unwrap(),
        None,
        "an empty ClickHouse result must not be interpreted as an existing hash"
    );
    let mut insert = admin.insert("workflow_trace_events").unwrap();
    insert
        .write(&trace_row(
            conflict_event_id,
            tenant_id,
            execution_id,
            1,
            now,
            3,
            9,
        ))
        .await
        .unwrap();
    insert
        .write(&trace_row(
            clean_event_id,
            tenant_id,
            execution_id,
            2,
            now,
            5,
            11,
        ))
        .await
        .unwrap();
    insert.end().await.unwrap();
    assert_eq!(
        existing_trace_hash(&admin, clean_event_id).await.unwrap(),
        Some(hash('a'))
    );
    let mut conflicts = admin.insert("trace_ingest_conflicts").unwrap();
    conflicts
        .write(&TraceConflictRow {
            event_id: conflict_event_id,
            tenant_id,
            execution_id,
            existing_hash: hash('a'),
            conflicting_hash: hash('b'),
            stream_id: "1-0".into(),
        })
        .await
        .unwrap();
    conflicts.end().await.unwrap();

    let request = ObservabilityAggregateRequestV1 {
        api_version: 1,
        tenant_id,
        from: now - time::Duration::hours(1),
        to: now + time::Duration::hours(1),
        dimensions: vec![ObservabilityDimensionV1::Status],
        metrics: vec![
            ObservabilityMetricV1::Count,
            ObservabilityMetricV1::CostMicros,
            ObservabilityMetricV1::InputTokens,
        ],
        filters: json!({"spanKind":"runtime_call"}),
        limit: 100,
    };
    let sql = aggregate_query_sql(&request).unwrap();
    let rows = admin
        .query(&sql)
        .bind(tenant_id)
        .bind(timestamp_micros(request.from).unwrap())
        .bind(timestamp_micros(request.to).unwrap())
        .bind(tenant_id)
        .bind(request.limit)
        .fetch_all::<AggregateRow>()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let dimensions: serde_json::Value = serde_json::from_str(&rows[0].dimensions).unwrap();
    let metrics: serde_json::Value = serde_json::from_str(&rows[0].metrics).unwrap();
    assert_eq!(dimensions["status"], "succeeded");
    assert_eq!(metrics["count"], 1);
    assert_eq!(metrics["costMicros"], 11);
    assert_eq!(metrics["inputTokens"], 5);

    let watermark = admin
        .query("SELECT max(execution_sequence) FROM workflow_trace_events FINAL WHERE tenant_id=? AND execution_id=? AND event_id NOT IN (SELECT event_id FROM trace_ingest_conflicts WHERE tenant_id=?)")
        .bind(tenant_id)
        .bind(execution_id)
        .bind(tenant_id)
        .fetch_one::<u64>()
        .await
        .unwrap();
    assert_eq!(watermark, 2);

    let mut extra = admin.insert("workflow_trace_events").unwrap();
    for sequence in 3..=5 {
        extra
            .write(&trace_row(
                Uuid::now_v7(),
                tenant_id,
                execution_id,
                sequence,
                now + time::Duration::microseconds(sequence as i64),
                1,
                1,
            ))
            .await
            .unwrap();
    }
    extra.end().await.unwrap();

    let first_page = admin
        .query(span_page_sql(false, false))
        .bind(tenant_id)
        .bind(execution_id)
        .bind(tenant_id)
        .bind(2_u64)
        .fetch_all::<TraceSpanKeyRow>()
        .await
        .unwrap();
    assert_eq!(first_page.len(), 2);
    let first_cursor = first_page.last().unwrap();
    let second_page = admin
        .query(span_page_sql(true, false))
        .bind(tenant_id)
        .bind(execution_id)
        .bind(tenant_id)
        .bind(timestamp_micros(first_cursor.started_at).unwrap())
        .bind(first_cursor.span_id)
        .bind(2_u64)
        .fetch_all::<TraceSpanKeyRow>()
        .await
        .unwrap();
    assert_eq!(second_page.len(), 2);
    let first_ids = first_page
        .iter()
        .map(|row| row.span_id)
        .collect::<std::collections::HashSet<_>>();
    assert!(
        second_page
            .iter()
            .all(|row| !first_ids.contains(&row.span_id))
    );
    let second_cursor = second_page.last().unwrap();
    let exhausted = admin
        .query(span_page_sql(true, false))
        .bind(tenant_id)
        .bind(execution_id)
        .bind(tenant_id)
        .bind(timestamp_micros(second_cursor.started_at).unwrap())
        .bind(second_cursor.span_id)
        .bind(2_u64)
        .fetch_all::<TraceSpanKeyRow>()
        .await
        .unwrap();
    assert!(
        exhausted.is_empty(),
        "Cursor pagination must not repeat or skip a clean Span"
    );
}

async fn batch_ingestion_replays_and_concurrent_conflicts_are_durable(admin: &Client) {
    let tenant_id = Uuid::now_v7();
    let partial_execution = Uuid::now_v7();
    let mut partial = Vec::new();
    for sequence in [10, 30, 40] {
        let mut event = trace_event(
            partial_execution,
            Uuid::now_v7(),
            sequence,
            TraceEventKindV1::Finished,
            "succeeded",
            OffsetDateTime::now_utc(),
        );
        event.tenant_id = tenant_id;
        partial.push((format!("snapshot-{sequence}"), event));
    }
    ingest_trace_batch(admin, &partial).await.unwrap();
    let snapshot =
        trace_ingestion_snapshot(admin, tenant_id, partial_execution, 30, "snapshot-missing")
            .await
            .unwrap();
    assert_eq!(
        snapshot,
        (40, 2),
        "later events cannot hide a missing event below the snapshot watermark"
    );
    let mut middle = trace_event(
        partial_execution,
        Uuid::now_v7(),
        20,
        TraceEventKindV1::Finished,
        "succeeded",
        OffsetDateTime::now_utc(),
    );
    middle.tenant_id = tenant_id;
    let middle = vec![("snapshot-20".into(), middle)];
    ingest_trace_batch(admin, &middle).await.unwrap();
    ingest_trace_batch(admin, &middle).await.unwrap();
    let snapshot =
        trace_ingestion_snapshot(admin, tenant_id, partial_execution, 30, "snapshot-complete")
            .await
            .unwrap();
    assert_eq!(
        snapshot,
        (40, 3),
        "replay must not inflate the snapshot event count"
    );
    let execution_id = Uuid::now_v7();
    let events: Vec<_> = (1..=100)
        .map(|sequence| {
            (
                format!("{sequence}-0"),
                trace_event(
                    execution_id,
                    Uuid::now_v7(),
                    sequence,
                    TraceEventKindV1::Finished,
                    "succeeded",
                    OffsetDateTime::now_utc(),
                ),
            )
        })
        .collect();
    assert_eq!(ingest_trace_batch(admin, &events).await.unwrap(), 0);
    assert_eq!(ingest_trace_batch(admin, &events).await.unwrap(), 0);
    let ids: Vec<_> = events.iter().map(|(_, event)| event.event_id).collect();
    let count = admin
        .query("SELECT count() FROM workflow_trace_events FINAL WHERE event_id IN ?")
        .bind(&ids)
        .fetch_one::<u64>()
        .await
        .unwrap();
    assert_eq!(
        count, 100,
        "replayed batches must not inflate authoritative trace rows"
    );

    let mut other = events[0].clone();
    other.1.content_hash = content_hash(&json!({"conflicting":true})).unwrap();
    let mut third = events[0].clone();
    third.1.content_hash = content_hash(&json!({"conflicting":2})).unwrap();
    let first = vec![other];
    let second = vec![third];
    let (left, right) = tokio::join!(
        ingest_trace_batch(admin, &first),
        ingest_trace_batch(admin, &second)
    );
    assert!(left.unwrap() > 0);
    assert!(right.unwrap() > 0);
    let recorded = admin
        .query("SELECT count() FROM trace_ingest_conflicts WHERE event_id=?")
        .bind(events[0].1.event_id)
        .fetch_one::<u64>()
        .await
        .unwrap();
    assert!(
        recorded >= 2,
        "conflicts must be durable before acknowledging either consumer"
    );
}

fn trace_row(
    event_id: Uuid,
    tenant_id: Uuid,
    execution_id: Uuid,
    sequence: u64,
    occurred_at: OffsetDateTime,
    input_tokens: u64,
    cost_micros: u64,
) -> TraceRow {
    TraceRow {
        event_id,
        tenant_id,
        execution_id,
        execution_sequence: sequence,
        trace_id: Uuid::now_v7(),
        span_id: Uuid::now_v7(),
        parent_span_id: None,
        event_kind: "finished".into(),
        span_kind: "runtime_call".into(),
        span_name: "Model call".into(),
        node_execution_id: None,
        attempt_id: None,
        agent_run_id: None,
        agent_iteration_id: None,
        runtime_call_id: None,
        sandbox_lease_id: None,
        wait_id: None,
        workflow_id: None,
        application_id: None,
        resource_type: Some("model".into()),
        resource_id: None,
        resource_version: Some("v1".into()),
        event_type: "runtime_call.completed".into(),
        status: "succeeded".into(),
        error_code: None,
        error_message: None,
        duration_ms: Some(4),
        input_tokens: Some(input_tokens),
        output_tokens: Some(2),
        cost_micros,
        attributes_json: json!({"provider":"fixture"}).to_string(),
        content_ref: None,
        content_kind: None,
        content_preview_json: None,
        content_hash: hash('a'),
        occurred_at,
    }
}

fn hash(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}

async fn clickhouse_with_retry(port: u16) -> Client {
    let client = Client::default()
        .with_url(format!("http://127.0.0.1:{port}"))
        .with_database("agentx_observability")
        .with_user("observability_migrate")
        .with_password("agentxtestpassword");
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

async fn execute_migration(client: &Client, migration: &str) {
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
        client.query(statement).execute().await.unwrap();
    }
}
