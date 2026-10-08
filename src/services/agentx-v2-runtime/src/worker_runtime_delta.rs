//! Bounded, lossless model delta delivery. A per-attempt flush acknowledges
//! durable writes before settlement. Redis is only a wakeup optimization.

use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

const CHANNEL_CAPACITY: usize = 256;
const THROTTLE_WINDOW: Duration = Duration::from_millis(50);
const THROTTLE_CHARACTERS: usize = 24;

enum Message {
    Delta(ThrottledFrame),
    Flush {
        attempt_id: Uuid,
        ack: oneshot::Sender<Result<(), String>>,
    },
}

#[derive(Clone)]
pub(crate) struct ModelDeltaSink {
    sender: Option<mpsc::Sender<Message>>,
}

struct ModelDeltaSinkHandle {
    pool: sqlx::MySqlPool,
    wakeup: crate::sse_wakeup::SseWakeup,
}

impl ModelDeltaSink {
    pub(crate) fn disabled() -> Self {
        Self { sender: None }
    }

    pub(crate) fn start(pool: sqlx::MySqlPool, wakeup: crate::sse_wakeup::SseWakeup) -> Self {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let handle = Arc::new(ModelDeltaSinkHandle { pool, wakeup });
        tokio::spawn(writer_task(receiver, move |frame| {
            let handle = Arc::clone(&handle);
            async move { write_frame(&handle, frame).await }
        }));
        Self {
            sender: Some(sender),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn emit(
        &self,
        tenant_id: Uuid,
        execution_id: Uuid,
        invocation_id: Option<Uuid>,
        attempt_id: Uuid,
        node_key: &str,
        text: &str,
        reasoning: Option<&str>,
    ) -> Result<(), String> {
        let Some(sender) = &self.sender else {
            return Ok(());
        };
        if text.is_empty() && reasoning.is_none_or(str::is_empty) {
            return Ok(());
        }
        sender
            .send(Message::Delta(ThrottledFrame {
                tenant_id,
                execution_id,
                invocation_id,
                attempt_id,
                node_key: node_key.to_owned(),
                text: text.to_owned(),
                reasoning: reasoning.map(str::to_owned),
            }))
            .await
            .map_err(|_| "model delta writer stopped".to_owned())
    }

    pub(crate) async fn flush(&self, attempt_id: Uuid) -> Result<(), String> {
        let Some(sender) = &self.sender else {
            return Ok(());
        };
        let (ack, done) = oneshot::channel();
        sender
            .send(Message::Flush { attempt_id, ack })
            .await
            .map_err(|_| "model delta writer stopped")?;
        done.await
            .map_err(|_| "model delta flush was not acknowledged")?
    }
}

struct ThrottledFrame {
    tenant_id: Uuid,
    execution_id: Uuid,
    invocation_id: Option<Uuid>,
    attempt_id: Uuid,
    node_key: String,
    text: String,
    reasoning: Option<String>,
}

async fn persist<F, Fut>(frame: ThrottledFrame, failed: &mut HashMap<Uuid, String>, write: &mut F)
where
    F: FnMut(ThrottledFrame) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let attempt_id = frame.attempt_id;
    if failed.contains_key(&attempt_id) {
        return;
    }
    if let Err(error) = write(frame).await {
        failed.insert(attempt_id, error);
    }
}

async fn writer_task<F, Fut>(mut receiver: mpsc::Receiver<Message>, mut write: F)
where
    F: FnMut(ThrottledFrame) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let mut pending: Option<ThrottledFrame> = None;
    let mut deadline = tokio::time::Instant::now() + THROTTLE_WINDOW;
    let mut failed = HashMap::<Uuid, String>::new();
    loop {
        let message = tokio::select! {
            _ = tokio::time::sleep_until(deadline), if pending.is_some() => {
                persist(pending.take().unwrap(), &mut failed, &mut write).await;
                continue;
            }
            message = receiver.recv() => message,
        };
        let Some(message) = message else {
            if let Some(frame) = pending.take() {
                persist(frame, &mut failed, &mut write).await;
            }
            return;
        };
        match message {
            Message::Delta(frame) => {
                if failed.contains_key(&frame.attempt_id) {
                    continue;
                }
                if let Some(current) = pending.as_mut().filter(|current| {
                    current.execution_id == frame.execution_id
                        && current.attempt_id == frame.attempt_id
                }) {
                    current.text.push_str(&frame.text);
                    if let Some(reasoning) = frame.reasoning {
                        current
                            .reasoning
                            .get_or_insert_with(String::new)
                            .push_str(&reasoning);
                    }
                } else {
                    if let Some(current) = pending.take() {
                        persist(current, &mut failed, &mut write).await;
                    }
                    pending = Some(frame);
                    deadline = tokio::time::Instant::now() + THROTTLE_WINDOW;
                }
                if pending.as_ref().is_some_and(|frame| {
                    frame.text.chars().count()
                        + frame.reasoning.as_deref().unwrap_or("").chars().count()
                        >= THROTTLE_CHARACTERS
                }) {
                    persist(pending.take().unwrap(), &mut failed, &mut write).await;
                }
            }
            Message::Flush { attempt_id, ack } => {
                if pending
                    .as_ref()
                    .is_some_and(|frame| frame.attempt_id == attempt_id)
                {
                    persist(pending.take().unwrap(), &mut failed, &mut write).await;
                }
                let _ = ack.send(failed.remove(&attempt_id).map_or(Ok(()), Err));
            }
        }
    }
}

async fn write_frame(handle: &ModelDeltaSinkHandle, frame: ThrottledFrame) -> Result<(), String> {
    let result = async {
        let mut tx = handle.pool.begin().await?;
        let payload = json!({"attemptId":frame.attempt_id,"nodeKey":frame.node_key,"deltaText":frame.text,"reasoningDeltaText":frame.reasoning});
        if let Some(invocation_id) = frame.invocation_id {
            sqlx::query("SELECT id FROM application_invocations WHERE tenant_id=? AND id=? FOR UPDATE")
                .bind(frame.tenant_id).bind(invocation_id).fetch_one(&mut *tx).await?;
            let next: u64 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(sequence_number),0)+1 AS UNSIGNED) FROM invocation_events WHERE tenant_id=? AND invocation_id=?")
                .bind(frame.tenant_id).bind(invocation_id).fetch_one(&mut *tx).await?;
            sqlx::query("INSERT INTO invocation_events(tenant_id,invocation_id,event_id,sequence_number,event_type,payload_json) VALUES(?,?,?,?,?,?)")
                .bind(frame.tenant_id).bind(invocation_id).bind(Uuid::now_v7()).bind(next).bind("model.delta").bind(&payload)
                .execute(&mut *tx).await?;
        } else {
            sqlx::query("INSERT INTO execution_model_deltas(tenant_id,execution_id,payload_json) VALUES(?,?,?)")
                .bind(frame.tenant_id).bind(frame.execution_id).bind(&payload).execute(&mut *tx).await?;
        }
        tx.commit().await
    }.await;
    result.map_err(|error: sqlx::Error| {
        tracing::warn!(%error, execution_id=%frame.execution_id, "Model delta persistence failed");
        "model delta persistence failed".to_owned()
    })?;
    if let Some(invocation_id) = frame.invocation_id {
        handle.wakeup.publish(invocation_id).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saturation_preserves_all_deltas_and_timer_flushes_without_new_input() {
        let (sender, receiver) = mpsc::channel(2);
        let sink = ModelDeltaSink {
            sender: Some(sender),
        };
        let written = Arc::new(tokio::sync::Mutex::new(String::new()));
        let output = Arc::clone(&written);
        let task = tokio::spawn(writer_task(receiver, move |frame| {
            let output = Arc::clone(&output);
            async move {
                tokio::time::sleep(Duration::from_millis(2)).await;
                output.lock().await.push_str(&frame.text);
                Ok(())
            }
        }));
        let attempt = Uuid::now_v7();
        let invocation = Uuid::now_v7();
        for _ in 0..600 {
            sink.emit(
                Uuid::nil(),
                Uuid::nil(),
                Some(invocation),
                attempt,
                "model",
                "x",
                None,
            )
            .await
            .unwrap();
        }
        sink.flush(attempt).await.unwrap();
        assert_eq!(written.lock().await.len(), 600);
        sink.emit(
            Uuid::nil(),
            Uuid::nil(),
            Some(invocation),
            attempt,
            "model",
            "tail",
            None,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !written.lock().await.ends_with("tail") {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        drop(sink);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn write_failure_is_acknowledged_only_to_its_attempt() {
        let (sender, receiver) = mpsc::channel(2);
        let sink = ModelDeltaSink {
            sender: Some(sender),
        };
        let failed = Uuid::now_v7();
        let healthy = Uuid::now_v7();
        let task = tokio::spawn(writer_task(receiver, move |frame| async move {
            if frame.attempt_id == failed {
                Err("database unavailable".into())
            } else {
                Ok(())
            }
        }));
        sink.emit(
            Uuid::nil(),
            Uuid::nil(),
            Some(Uuid::now_v7()),
            failed,
            "model",
            "lost",
            None,
        )
        .await
        .unwrap();
        sink.emit(
            Uuid::nil(),
            Uuid::nil(),
            Some(Uuid::now_v7()),
            healthy,
            "model",
            "saved",
            None,
        )
        .await
        .unwrap();
        assert!(sink.flush(healthy).await.is_ok());
        assert_eq!(
            sink.flush(failed).await.unwrap_err(),
            "database unavailable"
        );
        drop(sink);
        task.await.unwrap();
    }
}
