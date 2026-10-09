//! OpenAI chat stream framing and completion validation. The same SSE
//! framing parser is used by the existing provider adapter.

use std::{collections::BTreeMap, future::Future};

use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};

use super::provider::{legacy_sse_boundary, parse_legacy_sse_event};

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOOL_CALLS: u64 = 256;

pub(super) async fn aggregate_openai_sse_stream<F, Fut>(
    response: reqwest::Response,
    request: &Value,
    on_delta: &mut F,
) -> Result<Value, String>
where
    F: FnMut(String, Option<String>) -> Fut + Send,
    Fut: Future<Output = Result<(), String>> + Send,
{
    aggregate_chunks(response.bytes_stream(), request, on_delta).await
}

#[derive(Default)]
struct ToolCallFragment {
    id: String,
    name: String,
    arguments: String,
}

async fn aggregate_chunks<S, E, F, Fut>(
    stream: S,
    request: &Value,
    on_delta: &mut F,
) -> Result<Value, String>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::fmt::Display,
    F: FnMut(String, Option<String>) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    futures_util::pin_mut!(stream);
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = BTreeMap::<u64, ToolCallFragment>::new();
    let mut finish_reason: Option<String> = None;
    let mut usage = json!({});
    let mut buffer = BytesMut::new();
    let mut received = 0usize;
    let mut done = false;
    while !done {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let chunk = chunk.map_err(|error| format!("model stream interrupted: {error}"))?;
        received += chunk.len();
        if received > MAX_RESPONSE_BYTES {
            return Err("model stream exceeds 16 MiB".into());
        }
        buffer.extend_from_slice(&chunk);
        while let Some((position, separator)) = legacy_sse_boundary(&buffer) {
            if position > MAX_FRAME_BYTES {
                return Err("model stream frame exceeds 1 MiB".into());
            }
            let raw = buffer.split_to(position + separator);
            let raw = std::str::from_utf8(&raw[..position])
                .map_err(|error| format!("model stream is not UTF-8: {error}"))?;
            let (kind, payload) = parse_legacy_sse_event(raw);
            let payload = payload.trim();
            if kind == "error" {
                return Err("model provider sent an error event".into());
            }
            if payload.is_empty() {
                continue;
            }
            if payload == "[DONE]" {
                done = true;
                break;
            }
            let frame: Value = serde_json::from_str(payload)
                .map_err(|error| format!("model stream frame is not JSON: {error}"))?;
            if frame.get("error").is_some_and(|error| !error.is_null()) {
                return Err("model provider sent an error frame".into());
            }
            if let Some(value) = frame.get("usage").filter(|value| value.is_object()) {
                usage = value.clone();
            }
            let choice = frame
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| {
                    choices.iter().find(|choice| {
                        choice.get("index").and_then(Value::as_u64).unwrap_or(0) == 0
                    })
                });
            let Some(choice) = choice else { continue };
            if let Some(delta) = choice.get("delta") {
                let content = delta.get("content").and_then(Value::as_str).unwrap_or("");
                let thought = delta
                    .get("reasoning_content")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let tool_deltas = delta.get("tool_calls").and_then(Value::as_array);
                if finish_reason.is_some()
                    && (!content.is_empty()
                        || !thought.is_empty()
                        || tool_deltas.is_some_and(|calls| !calls.is_empty()))
                {
                    return Err("model stream contains deltas after completion".into());
                }
                if !content.is_empty() || !thought.is_empty() {
                    text.push_str(content);
                    reasoning.push_str(thought);
                    on_delta(
                        content.to_owned(),
                        (!thought.is_empty()).then(|| thought.to_owned()),
                    )
                    .await?;
                }
                for call in tool_deltas.into_iter().flatten() {
                    let index = call
                        .get("index")
                        .and_then(Value::as_u64)
                        .filter(|index| *index < MAX_TOOL_CALLS)
                        .ok_or("model stream has an invalid tool call index")?;
                    let fragment = calls.entry(index).or_default();
                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        if !fragment.id.is_empty() && fragment.id != id {
                            return Err("model stream changed a tool call ID".into());
                        }
                        fragment.id = id.to_owned();
                    }
                    if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                        fragment.name.push_str(name);
                    }
                    if let Some(arguments) =
                        call.pointer("/function/arguments").and_then(Value::as_str)
                    {
                        fragment.arguments.push_str(arguments);
                    }
                }
            }
            if let Some(reason) = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
            {
                if finish_reason
                    .as_deref()
                    .is_some_and(|previous| previous != reason)
                {
                    return Err("model stream changed its finish reason".into());
                }
                finish_reason = Some(reason.to_owned());
            }
        }
        if buffer.len() > MAX_FRAME_BYTES {
            return Err("model stream frame exceeds 1 MiB".into());
        }
    }
    if !done || finish_reason.is_none() {
        return Err("model stream ended before finish_reason and [DONE]".into());
    }
    if text.is_empty() && reasoning.is_empty() && calls.is_empty() {
        return Err("model stream produced no content".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for call in calls.values() {
        if call.id.is_empty() || call.name.is_empty() || !ids.insert(call.id.clone()) {
            return Err("model stream has missing or duplicate tool call IDs/names".into());
        }
        serde_json::from_str::<Value>(&call.arguments)
            .map_err(|_| "model stream has incomplete tool arguments")?;
    }
    let prompt_estimate = ["messages", "tools"]
        .iter()
        .filter_map(|key| request.get(*key))
        .map(|value| value.to_string().chars().count() as u64)
        .sum::<u64>()
        .div_ceil(4);
    let completion_estimate = (text.chars().count() as u64
        + reasoning.chars().count() as u64
        + calls
            .values()
            .map(|call| (call.name.chars().count() + call.arguments.chars().count()) as u64)
            .sum::<u64>())
    .div_ceil(4);
    let estimated = usage.get("prompt_tokens").and_then(Value::as_u64).is_none()
        || usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .is_none();
    let prompt = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(prompt_estimate);
    let completion = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(completion_estimate);
    usage["prompt_tokens"] = json!(prompt);
    usage["completion_tokens"] = json!(completion);
    if usage.get("total_tokens").and_then(Value::as_u64).is_none() {
        usage["total_tokens"] = json!(prompt.saturating_add(completion));
    }
    let mut message = json!({"role":"assistant", "content":text});
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    if !calls.is_empty() {
        message["tool_calls"] = json!(calls.values().map(|call| json!({
            "id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}
        })).collect::<Vec<_>>());
    }
    Ok(
        json!({"choices":[{"message":message,"finish_reason":finish_reason}],"usage":usage,"usage_estimated":estimated}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parse(value: &str) -> Result<Value, String> {
        let chunks = value
            .as_bytes()
            .chunks(3)
            .map(|chunk| Ok::<_, String>(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>();
        aggregate_chunks(
            futures_util::stream::iter(chunks),
            &json!({"messages":[{"role":"user","content":"hello"}]}),
            &mut |_, _| async { Ok(()) },
        )
        .await
    }

    #[tokio::test]
    async fn preserves_tool_identity_and_fragmented_arguments() {
        let value = parse("data:{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_original\",\"function\":{\"name\":\"search\",\"arguments\":\"{\"}}]}}]}\r\n\r\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"}\\n\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n").await.unwrap();
        assert_eq!(
            value.pointer("/choices/0/message/tool_calls/0/id"),
            Some(&json!("call_original"))
        );
        assert_eq!(value["usage_estimated"], true);
        assert!(value["usage"]["prompt_tokens"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn rejects_truncation_error_and_unfinished_calls() {
        let partial = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
        assert!(parse(partial).await.is_err());
        assert!(parse(&format!("{partial}data: [DONE]\n\n")).await.is_err());
        assert!(
            parse(&format!("{partial}event: error\ndata: failed\n\n"))
                .await
                .is_err()
        );
        assert!(
            parse(&format!(
                "{partial}data: {{\"error\":{{\"message\":\"failed\"}}}}\n\n"
            ))
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn preserves_actual_usage_and_accepts_multiline_sse() {
        let value = parse("data: {\"choices\":[{\"delta\":{\"content\":\"中文\"},\ndata: \"finish_reason\":\"stop\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":7,\"total_tokens\":19}}\n\ndata: [DONE]\n\n").await.unwrap();
        assert_eq!(value["usage_estimated"], false);
        assert_eq!(value["usage"]["total_tokens"], 19);
    }

    #[tokio::test]
    async fn sink_failure_aborts_stream() {
        let stream = futures_util::stream::iter([Ok::<_, String>(Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"text\"}}]}\n\n",
        ))]);
        let error = aggregate_chunks(stream, &json!({}), &mut |_, _| async {
            Err("delta persistence failed".into())
        })
        .await
        .unwrap_err();
        assert_eq!(error, "delta persistence failed");
    }
}
