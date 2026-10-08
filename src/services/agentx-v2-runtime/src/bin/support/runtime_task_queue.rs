#![allow(dead_code)]

use agentx_runtime_contracts::WorkerTaskV1;
use anyhow::{Context, Result};
use redis::aio::ConnectionManager;
use redis::streams::{
    StreamAutoClaimOptions, StreamAutoClaimReply, StreamId, StreamReadOptions, StreamReadReply,
};
use redis::{AsyncCommands, FromRedisValue};

pub const TASK_GROUP: &str = "agentx:v2:workers:v1";

// Recovery can redispatch an unclaimed MySQL Attempt while its original
// message is still queued. Keep one live stream entry per Attempt; Redis
// loss removes this accelerator and the MySQL recovery path rebuilds it.
const PUBLISH_TASK: &str = r#"
local previous = redis.call('HGET', KEYS[2], ARGV[1])
if previous and #redis.call('XRANGE', KEYS[1], previous, previous, 'COUNT', 1) > 0 then
  return previous
end
local id = redis.call('XADD', KEYS[1], '*', 'task', ARGV[2])
redis.call('HSET', KEYS[2], ARGV[1], id)
return id
"#;

const ACK_TASK: &str = r#"
redis.call('XACK', KEYS[1], ARGV[1], ARGV[2])
redis.call('XDEL', KEYS[1], ARGV[2])
if redis.call('HGET', KEYS[2], ARGV[3]) == ARGV[2] then
  redis.call('HDEL', KEYS[2], ARGV[3])
end
return 1
"#;

fn task_index(capability: &str) -> String {
    format!("agentx:v2:tasks:dedup:v1:{capability}")
}

fn attempt_key(task: &WorkerTaskV1) -> String {
    format!("{}:{}", task.tenant_id, task.attempt_id)
}

#[derive(Debug)]
pub struct TaskQueueItem {
    pub stream: String,
    pub stream_id: String,
    pub task: WorkerTaskV1,
}

pub fn stream_name(capability: &str) -> Result<String> {
    anyhow::ensure!(
        agentx_node_protocol::ALL_RUNTIME_CAPABILITIES.contains(&capability),
        "unsupported Runtime capability {capability}"
    );
    Ok(format!("agentx:v2:tasks:v1:{capability}"))
}

pub async fn ensure_groups(redis: &mut ConnectionManager) -> Result<()> {
    for capability in agentx_node_protocol::ALL_RUNTIME_CAPABILITIES {
        ensure_group(redis, capability).await?;
    }
    Ok(())
}

async fn ensure_group(redis: &mut ConnectionManager, capability: &str) -> Result<()> {
    let stream = stream_name(capability)?;
    let result = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(stream)
        .arg(TASK_GROUP)
        .arg("0-0")
        .arg("MKSTREAM")
        .query_async::<String>(redis)
        .await;
    if let Err(error) = result
        && error.code() != Some("BUSYGROUP")
    {
        return Err(error).context("failed to create Runtime task consumer group");
    }
    Ok(())
}

pub async fn publish(redis: &mut ConnectionManager, task: &WorkerTaskV1) -> Result<String> {
    let stream = stream_name(task.capability.as_str())?;
    let payload = serde_json::to_string(task)?;
    redis::Script::new(PUBLISH_TASK)
        .key(&stream)
        .key(task_index(task.capability.as_str()))
        .arg(attempt_key(task))
        .arg(payload)
        .invoke_async(redis)
        .await
        .context("failed to publish Runtime Worker Task")
}

pub async fn read(
    redis: &mut ConnectionManager,
    capability: &str,
    consumer: &str,
    block_ms: usize,
) -> Result<Vec<TaskQueueItem>> {
    let stream = stream_name(capability)?;
    let claimed: StreamAutoClaimReply = match redis
        .xautoclaim_options(
            &stream,
            TASK_GROUP,
            consumer,
            30_000,
            "0-0",
            StreamAutoClaimOptions::default().count(1),
        )
        .await
    {
        Ok(claimed) => claimed,
        Err(error) if error.code() == Some("NOGROUP") => {
            ensure_group(redis, capability).await?;
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
    };
    if !claimed.claimed.is_empty() {
        return decode(&stream, claimed.claimed);
    }
    let options = StreamReadOptions::default()
        .group(TASK_GROUP, consumer)
        .count(1)
        .block(block_ms);
    let reply: StreamReadReply = match redis
        .xread_options(&[stream.as_str()], &[">"], &options)
        .await
    {
        Ok(reply) => reply,
        Err(error) if error.code() == Some("NOGROUP") => {
            ensure_group(redis, capability).await?;
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
    };
    let mut items = Vec::new();
    for key in reply.keys {
        items.extend(decode(&key.key, key.ids)?);
    }
    Ok(items)
}

pub async fn ack(redis: &mut ConnectionManager, item: &TaskQueueItem) -> Result<()> {
    let _: u64 = redis::Script::new(ACK_TASK)
        .key(&item.stream)
        .key(task_index(item.task.capability.as_str()))
        .arg(TASK_GROUP)
        .arg(&item.stream_id)
        .arg(attempt_key(&item.task))
        .invoke_async(redis)
        .await?;
    Ok(())
}

fn decode(stream: &str, ids: Vec<StreamId>) -> Result<Vec<TaskQueueItem>> {
    ids.into_iter()
        .map(|id| {
            let payload = id
                .map
                .get("task")
                .context("Runtime task stream entry has no task field")?;
            let payload = String::from_redis_value(payload)?;
            Ok(TaskQueueItem {
                stream: stream.to_owned(),
                stream_id: id.id,
                task: serde_json::from_str(&payload)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::stream_name;

    #[test]
    fn stream_is_versioned_and_partitioned_by_capability() {
        assert_eq!(
            stream_name("builtin").unwrap(),
            "agentx:v2:tasks:v1:builtin"
        );
        assert!(stream_name("unknown").is_err());
    }
}
