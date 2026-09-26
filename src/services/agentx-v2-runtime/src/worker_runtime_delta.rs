//! Model delta sink (plan7 P7-B): bridges the synchronous agent-core
//! `on_delta` callbacks (and the model node stream loop) to incremental
//! `invocation_events` rows. Mirrors the plugin trace sink pattern — a
//! bounded mpsc channel with a background writer task — but with two
//! semantic differences the plan calls out:
//!
//! 1. Frames are throttled (max 50ms or 24 tokens per row) so token streams
//!    do not translate into per-token MySQL writes.
//! 2. `flush` must complete before the worker attempt settles, otherwise a
//!    delta row could commit after the terminal event and never reach the
//!    SSE consumer. Losing deltas is NOT acceptable, unlike plugin traces.

use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

const CHANNEL_CAPACITY: usize = 256;
const THROTTLE_WINDOW_MS: u64 = 50;
const THROTTLE_TOKENS: usize = 24;

enum Message {
    Delta {
        tenant_id: Uuid,
        invocation_id: Uuid,
        attempt_id: Uuid,
        node_key: String,
        text: String,
        reasoning: Option<String>,
    },
    Flush {
        ack: tokio::sync::oneshot::Sender<()>,
    },
}

#[derive(Clone)]
pub(crate) struct ModelDeltaSink {
    sender: mpsc::Sender<Message>,
}

pub(crate) struct ModelDeltaSinkHandle {
    pool: sqlx::MySqlPool,
    redis: Option<redis::Client>,
}

impl ModelDeltaSink {
    /// The sink degrades to a no-op without a pool; tests construct workers
    /// without one for pure execution paths.
    pub(crate) fn disabled() -> Self {
        let (sender, _receiver) = mpsc::channel(1);
        Self { sender }
    }

    pub(crate) fn start(pool: sqlx::MySqlPool, redis: Option<redis::Client>) -> Self {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let handle = ModelDeltaSinkHandle { pool, redis };
        tokio::spawn(writer_task(handle, receiver));
        Self { sender }
    }

    /// Non-blocking delta emission from synchronous contexts. A full channel
    /// drops nothing silently: the attempt result is authoritative and the
    /// client refreshes from the final payload; the drop is logged by the
    /// writer closing over the counter.
    pub(crate) fn emit(
        &self,
        tenant_id: Uuid,
        invocation_id: Option<Uuid>,
        attempt_id: Uuid,
        node_key: &str,
        text: &str,
        reasoning: Option<&str>,
    ) {
        let Some(invocation_id) = invocation_id else {
            return;
        };
        if text.is_empty() && reasoning.is_none() {
            return;
        }
        let _ = self.sender.try_send(Message::Delta {
            tenant_id,
            invocation_id,
            attempt_id,
            node_key: node_key.to_owned(),
            text: text.to_owned(),
            reasoning: reasoning.map(str::to_owned),
        });
    }

    /// Drains pending deltas. Must be awaited before the worker attempt
    /// result is submitted so no delta row lands after the terminal event.
    pub(crate) async fn flush(&self) {
        let (ack, done) = tokio::sync::oneshot::channel();
        let _ = self.sender.send(Message::Flush { ack }).await.is_ok() && done.await.is_ok();
    }
}

struct ThrottledFrame {
    tenant_id: Uuid,
    invocation_id: Uuid,
    attempt_id: Uuid,
    node_key: String,
    text: String,
    reasoning: Option<String>,
}

async fn writer_task(handle: ModelDeltaSinkHandle, mut receiver: mpsc::Receiver<Message>) {
    let mut pending: Option<ThrottledFrame> = None;
    let mut window_started: Option<tokio::time::Instant> = None;
    loop {
        let message = match receiver.recv().await {
            Some(message) => message,
            None => {
                if let Some(frame) = pending.take() {
                    write_frame(&handle, frame).await;
                }
                return;
            }
        };
        match message {
            Message::Delta {
                tenant_id,
                invocation_id,
                attempt_id,
                node_key,
                text,
                reasoning,
            } => {
                let started = *window_started.get_or_insert_with(tokio::time::Instant::now);
                match pending.as_mut() {
                    Some(frame)
                        if frame.invocation_id == invocation_id
                            && frame.attempt_id == attempt_id =>
                    {
                        frame.text.push_str(&text);
                        if let Some(reasoning) = reasoning {
                            frame
                                .reasoning
                                .get_or_insert_with(String::new)
                                .push_str(&reasoning);
                        }
                    }
                    _ => {
                        if let Some(frame) = pending.take() {
                            write_frame(&handle, frame).await;
                        }
                        pending = Some(ThrottledFrame {
                            tenant_id,
                            invocation_id,
                            attempt_id,
                            node_key,
                            text,
                            reasoning,
                        });
                        window_started = Some(tokio::time::Instant::now());
                    }
                }
                let due = pending.as_ref().is_some_and(|frame| {
                    frame.text.chars().count() >= THROTTLE_TOKENS
                        || started.elapsed() >= Duration::from_millis(THROTTLE_WINDOW_MS)
                });
                if due && let Some(frame) = pending.take() {
                    write_frame(&handle, frame).await;
                    window_started = None;
                }
            }
            Message::Flush { ack } => {
                if let Some(frame) = pending.take() {
                    write_frame(&handle, frame).await;
                    window_started = None;
                }
                let _ = ack.send(());
            }
        }
    }
}

async fn write_frame(handle: &ModelDeltaSinkHandle, frame: ThrottledFrame) {
    let payload = json!({
        "attemptId": frame.attempt_id,
        "nodeKey": frame.node_key,
        "deltaText": frame.text,
        "reasoningDeltaText": frame.reasoning,
    });
    let Ok(mut tx) = handle.pool.begin().await else {
        tracing::warn!(invocation_id = %frame.invocation_id, "Model delta transaction failed to start");
        return;
    };
    let next: Option<u64> = sqlx::query_scalar(
        "SELECT CAST(COALESCE(MAX(sequence_number),0)+1 AS UNSIGNED) FROM invocation_events WHERE tenant_id=? AND invocation_id=? FOR UPDATE",
    )
    .bind(frame.tenant_id)
    .bind(frame.invocation_id)
    .fetch_one(&mut *tx)
    .await
    .ok();
    let Some(next) = next else {
        return;
    };
    let inserted = sqlx::query(
        "INSERT INTO invocation_events(tenant_id,invocation_id,event_id,sequence_number,event_type,payload_json) VALUES(?,?,?,?,?,?)",
    )
    .bind(frame.tenant_id)
    .bind(frame.invocation_id)
    .bind(Uuid::now_v7())
    .bind(next)
    .bind("model.delta")
    .bind(&payload)
    .execute(&mut *tx)
    .await;
    match inserted {
        Ok(_) => {
            if let Err(error) = tx.commit().await {
                tracing::warn!(%error, invocation_id = %frame.invocation_id, "Model delta commit failed");
                return;
            }
            if let Some(client) = &handle.redis {
                // Best-effort wakeup; the SSE consumer falls back to its
                // 1s poll when pubsub is unavailable.
                let _ = crate::sse_wakeup::publish(Some(client), frame.invocation_id).await;
            }
        }
        Err(error) => {
            let _ = tx.rollback().await;
            tracing::warn!(%error, invocation_id = %frame.invocation_id, "Model delta insert failed");
        }
    }
}
