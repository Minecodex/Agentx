#[path = "../src/bin/support/runtime_task_queue.rs"]
mod runtime_task_queue;

use agentx_node_protocol::NodeCapability;
use agentx_runtime_contracts::{WorkerTaskV1, content_hash};
use redis::AsyncCommands;
use testcontainers::{
    GenericImage,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use time::OffsetDateTime;
use uuid::Uuid;

#[tokio::test]
async fn concurrent_recovery_reuses_live_messages_and_redis_loss_rebuilds_them() {
    let container = GenericImage::new("redis", "7.4-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .unwrap();
    let port = container.get_host_port_ipv4(6379.tcp()).await.unwrap();
    let mut connection = redis::Client::open(format!("redis://127.0.0.1:{port}/"))
        .unwrap()
        .get_connection_manager()
        .await
        .unwrap();
    runtime_task_queue::ensure_groups(&mut connection)
        .await
        .unwrap();
    let task = WorkerTaskV1 {
        protocol_version: 1,
        task_id: Uuid::now_v7(),
        tenant_id: Uuid::now_v7(),
        execution_id: Uuid::now_v7(),
        node_execution_id: Uuid::now_v7(),
        attempt_id: Uuid::now_v7(),
        capability: NodeCapability::PluginNodejs,
        bundle_id: Uuid::now_v7(),
        work_package_id: None,
        state_version: 1,
        compatibility_hash: content_hash(&serde_json::json!({})).unwrap(),
        deadline_at: OffsetDateTime::now_utc() + time::Duration::minutes(5),
    };
    let stream = runtime_task_queue::stream_name("plugin_nodejs").unwrap();
    let index = "agentx:v2:tasks:dedup:v1:plugin_nodejs";
    let receipts = futures::future::join_all((0..32).map(|_| {
        let mut redis = connection.clone();
        let task = task.clone();
        async move {
            runtime_task_queue::publish(&mut redis, &task)
                .await
                .unwrap()
        }
    }))
    .await;
    assert!(receipts.iter().all(|id| id == &receipts[0]));
    let size: u64 = connection.xlen(&stream).await.unwrap();
    assert_eq!(
        size, 1,
        "concurrent recovery cannot multiply queued messages"
    );
    let original = runtime_task_queue::read(&mut connection, "plugin_nodejs", "first", 10)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(original.task.attempt_id, task.attempt_id);
    assert_eq!(
        runtime_task_queue::publish(&mut connection, &task)
            .await
            .unwrap(),
        original.stream_id,
        "pending delivery is still a live message"
    );

    // A missing stream must invalidate a retained index entry. An ACK from
    // the old stream must not delete the replacement receipt or message.
    let _: u64 = connection.del(&stream).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    runtime_task_queue::ensure_groups(&mut connection)
        .await
        .unwrap();
    let replacement_id = runtime_task_queue::publish(&mut connection, &task)
        .await
        .unwrap();
    assert_ne!(replacement_id, original.stream_id);
    let replacement = runtime_task_queue::read(&mut connection, "plugin_nodejs", "second", 10)
        .await
        .unwrap()
        .pop()
        .unwrap();
    runtime_task_queue::ack(&mut connection, &original)
        .await
        .unwrap();
    assert_eq!(
        runtime_task_queue::publish(&mut connection, &task)
            .await
            .unwrap(),
        replacement_id
    );
    runtime_task_queue::ack(&mut connection, &replacement)
        .await
        .unwrap();
    let size: u64 = connection.xlen(&stream).await.unwrap();
    let receipts: u64 = connection.hlen(index).await.unwrap();
    assert_eq!((size, receipts), (0, 0), "ACK also removes the index entry");

    runtime_task_queue::publish(&mut connection, &task)
        .await
        .unwrap();
    redis::cmd("FLUSHALL")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    runtime_task_queue::ensure_groups(&mut connection)
        .await
        .unwrap();
    runtime_task_queue::publish(&mut connection, &task)
        .await
        .unwrap();
    let mut retry = task.clone();
    retry.task_id = Uuid::now_v7();
    retry.attempt_id = Uuid::now_v7();
    runtime_task_queue::publish(&mut connection, &retry)
        .await
        .unwrap();
    let size: u64 = connection.xlen(&stream).await.unwrap();
    assert_eq!(size, 2, "new Attempts remain distinct after Redis rebuild");
    for consumer in ["rebuilt", "retry"] {
        let item = runtime_task_queue::read(&mut connection, "plugin_nodejs", consumer, 10)
            .await
            .unwrap()
            .pop()
            .unwrap();
        runtime_task_queue::ack(&mut connection, &item)
            .await
            .unwrap();
    }
    let receipts: u64 = connection.hlen(index).await.unwrap();
    assert_eq!(receipts, 0, "dedup metadata cannot grow after drain");
}
