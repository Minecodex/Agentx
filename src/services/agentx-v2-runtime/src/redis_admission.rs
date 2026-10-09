//! Bounded, atomic Redis task backlog checks. MySQL remains authoritative;
//! Redis pressure prevents accepting work faster than the dispatch cache drains.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use redis::aio::ConnectionManager;

use crate::error::{RuntimeError, RuntimeResult};

const SNAPSHOT: &str = r#"
local unread, pending = 0, 0
for _, key in ipairs(KEYS) do
  if redis.call('EXISTS', key) == 1 then
    local cursor = '0-0'
    for _, group in ipairs(redis.call('XINFO', 'GROUPS', key)) do
      local name, count, delivered = nil, 0, '0-0'
      for i = 1, #group, 2 do
        if group[i] == 'name' then name = group[i+1] end
        if group[i] == 'pending' then count = group[i+1] end
        if group[i] == 'last-delivered-id' then delivered = group[i+1] end
      end
      if name == ARGV[1] then pending = pending + count; cursor = delivered end
    end
    unread = unread + #redis.call('XRANGE', key, '(' .. cursor, '+', 'COUNT', ARGV[2])
  end
end
return {unread, pending}
"#;

static UNREAD: AtomicI64 = AtomicI64::new(0);
static PENDING: AtomicI64 = AtomicI64::new(0);
static AVAILABLE: AtomicI64 = AtomicI64::new(0);

pub fn metrics() -> (i64, i64, i64) {
    (
        UNREAD.load(Ordering::Relaxed),
        PENDING.load(Ordering::Relaxed),
        AVAILABLE.load(Ordering::Relaxed),
    )
}

pub async fn snapshot(connection: &ConnectionManager, limit: i64) -> RuntimeResult<(i64, i64)> {
    let mut redis = connection.clone();
    let script = redis::Script::new(SNAPSHOT);
    let mut invocation = script.prepare_invoke();
    for capability in agentx_node_protocol::ALL_RUNTIME_CAPABILITIES {
        invocation.key(format!("agentx:v2:tasks:v1:{capability}"));
    }
    invocation
        .arg("agentx:v2:workers:v1")
        .arg(limit.saturating_add(1));
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        invocation.invoke_async::<(i64, i64)>(&mut redis),
    )
    .await;
    match result {
        Ok(Ok((unread, pending))) => {
            UNREAD.store(unread, Ordering::Relaxed);
            PENDING.store(pending, Ordering::Relaxed);
            AVAILABLE.store(1, Ordering::Relaxed);
            Ok((unread, pending))
        }
        _ => {
            AVAILABLE.store(0, Ordering::Relaxed);
            tracing::warn!("Redis admission snapshot unavailable; refusing new intake");
            Err(RuntimeError::Unavailable)
        }
    }
}
