use std::{env, net::SocketAddr};

use axum::{
    Json as AxumJson, Router,
    body::Body,
    extract::OriginalUri,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bytes::Bytes;
use futures_util::StreamExt;
use rmcp::{
    Json, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::{
        StreamableHttpServerConfig,
        streamable_http_server::{
            session::local::LocalSessionManager, tower::StreamableHttpService,
        },
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EchoRequest {
    #[schemars(description = "Text returned by the echo tool")]
    text: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct EchoResponse {
    text: String,
}

#[derive(Debug, Clone)]
struct EchoService {
    tool_router: ToolRouter<Self>,
}

impl EchoService {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl EchoService {
    #[tool(description = "Return the provided text unchanged")]
    fn echo(
        &self,
        Parameters(EchoRequest { text }): Parameters<EchoRequest>,
    ) -> Json<EchoResponse> {
        Json(EchoResponse { text })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EchoService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Agentx local Echo MCP test server".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "echo_mcp=info".into()),
        )
        .json()
        .init();

    let bind_addr: SocketAddr = env::var("AGENTX_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8090".to_owned())
        .parse()?;
    let cancellation = CancellationToken::new();
    let service: StreamableHttpService<EchoService, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(EchoService::new()),
            Default::default(),
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: None,
                cancellation_token: cancellation.child_token(),
            },
        );
    let app = Router::new()
        .route(
            "/health",
            get(|| async { AxumJson(serde_json::json!({ "status": "ready" })) }),
        )
        .route(
            "/models",
            get(|| async {
                AxumJson(serde_json::json!({
                    "object": "list",
                    "data": [{ "id": "echo-model", "object": "model" }]
                }))
            }),
        )
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/plan5/items", post(plan5_items))
        .route("/v1/plan5/delay", get(plan5_delay))
        .route("/v1/plan5/binary", get(plan5_binary))
        .route("/v1/plan5/request", get(plan5_request))
        .route("/v1/plan5/maybe-fail", get(plan5_maybe_fail))
        .route("/v2/runtime/model", post(v2_runtime_model))
        .route("/v2/runtime/mcp", post(v2_runtime_mcp))
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn(require_mcp_bearer));
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(%bind_addr, "Echo MCP is listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancellation.cancel();
        })
        .await?;
    Ok(())
}

async fn require_mcp_bearer(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request.uri().path().starts_with("/mcp")
        && let Ok(token) = env::var("AGENTX_FIXTURE_MCP_AUTH_TOKEN")
        && request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(format!("Bearer {token}").as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

async fn plan5_items(headers: HeaderMap, AxumJson(body): AxumJson<Value>) -> AxumJson<Value> {
    AxumJson(json!({
        "headers": {
            "x-plan5-secret": headers
                .get("x-plan5-secret")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
        },
        "json": body
    }))
}

async fn plan5_delay() -> AxumJson<Value> {
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    AxumJson(json!({"completed": true}))
}

async fn plan5_binary() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=plan5-fixture.bin",
            ),
        ],
        vec![0_u8, 1, 2, 3, 0x7f, 0x80, 0xfe, 0xff],
    )
        .into_response()
}

async fn plan5_request(OriginalUri(uri): OriginalUri, headers: HeaderMap) -> AxumJson<Value> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    AxumJson(json!({
        "authorization": header("authorization"),
        "x-api-key": header("x-api-key"),
        "x-custom-auth": header("x-custom-auth"),
        "query": uri.query().unwrap_or_default(),
    }))
}

async fn plan5_maybe_fail(OriginalUri(uri): OriginalUri) -> Response {
    let query = uri.query().unwrap_or_default();
    if query.split('&').any(|pair| pair == "fail=true") {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            AxumJson(json!({"failed": true})),
        )
            .into_response();
    }
    AxumJson(json!({"failed": false})).into_response()
}

async fn v2_runtime_model(headers: HeaderMap, AxumJson(request): AxumJson<Value>) -> Response {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some("Bearer m5-model-secret")
    {
        return (
            StatusCode::UNAUTHORIZED,
            AxumJson(json!({"error":{"message":"invalid fixture credential"}})),
        )
            .into_response();
    }
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    let agent_call = request
        .pointer("/parameters/maxIterations")
        .and_then(Value::as_u64)
        .is_some();
    let has_tool_result = input.get("tool").is_some();
    if agent_call && !has_tool_result {
        return AxumJson(json!({
            "toolCall":{"text":"agentx-v2-tool"},
            "usage":{"inputTokens":12,"outputTokens":8,"tokens":20,"costMicros":5}
        }))
        .into_response();
    }
    AxumJson(json!({
        "done":true,
        "answer":"agentx-v2-model",
        "input":input,
        "usage":{"inputTokens":12,"outputTokens":8,"tokens":20,"costMicros":5}
    }))
    .into_response()
}

async fn v2_runtime_mcp(headers: HeaderMap, AxumJson(request): AxumJson<Value>) -> Response {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some("Bearer m5-model-secret")
    {
        return (
            StatusCode::UNAUTHORIZED,
            AxumJson(json!({"error":{"message":"invalid fixture credential"}})),
        )
            .into_response();
    }
    AxumJson(json!({
        "content":{
            "text":"agentx-v2-mcp",
            "arguments":request.pointer("/params/arguments").cloned().unwrap_or(Value::Null)
        }
    }))
    .into_response()
}

async fn embeddings(headers: HeaderMap, AxumJson(request): AxumJson<Value>) -> Response {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some("Bearer m5-model-secret")
    {
        return (
            StatusCode::UNAUTHORIZED,
            AxumJson(json!({"error":{"message":"invalid fixture credential"}})),
        )
            .into_response();
    }
    let inputs = match request.get("input") {
        Some(Value::Array(values)) => values.clone(),
        Some(value) => vec![value.clone()],
        None => Vec::new(),
    };
    let default_dimensions =
        if request.get("model").and_then(Value::as_str) == Some("echo-embedding-1536") {
            1536
        } else {
            8
        };
    let dimensions = request
        .get("dimensions")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| (1..=3072).contains(value))
        .unwrap_or(default_dimensions);
    let mut token_count = 0_u64;
    let data = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| {
            let text = input
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| input.to_string());
            token_count += text.len().div_ceil(4) as u64;
            let mut embedding = vec![0_f64; dimensions];
            for (offset, byte) in text.bytes().enumerate() {
                embedding[offset % dimensions] += f64::from(byte) / 255.0;
            }
            json!({"object":"embedding","embedding":embedding,"index":index})
        })
        .collect::<Vec<_>>();
    AxumJson(json!({
        "object":"list",
        "data":data,
        "model":request.get("model").cloned().unwrap_or_else(||json!("text-embedding-3-small")),
        "usage":{"prompt_tokens":token_count,"total_tokens":token_count}
    }))
    .into_response()
}

async fn chat_completions(headers: HeaderMap, AxumJson(request): AxumJson<Value>) -> Response {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some("Bearer m5-model-secret")
    {
        return (
            StatusCode::UNAUTHORIZED,
            AxumJson(json!({"error":{"message":"invalid fixture credential"}})),
        )
            .into_response();
    }
    if request.get("model").and_then(Value::as_str) == Some("echo-unavailable") {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            AxumJson(json!({"error":{"message":"fixture provider unavailable"}})),
        )
            .into_response();
    }
    if request.get("model").and_then(Value::as_str) == Some("echo-timeout") {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    // plan7 P7-B B6 scenario 5: the vision fixture reports whether the
    // request carried native image_url content parts.
    if request.get("model").and_then(Value::as_str) == Some("echo-vision") {
        let messages = request
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let image_parts = messages
            .iter()
            .filter_map(|message| message.get("content").and_then(Value::as_array))
            .flatten()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("image_url"))
            .count();
        let text = if image_parts > 0 {
            format!("vision-ok:{image_parts}")
        } else {
            "vision-missing".into()
        };
        return (
            StatusCode::OK,
            AxumJson(json!({
                "id":"chatcmpl-vision","object":"chat.completion","model":"echo-vision",
                "choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}
            })),
        )
            .into_response();
    }
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if request
        .pointer("/response_format/type")
        .and_then(Value::as_str)
        == Some("json_object")
        && messages.iter().any(|message| {
            message.get("role").and_then(Value::as_str) == Some("system")
                && message
                    .get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("Memory Extractor"))
        })
    {
        let extraction = messages
            .iter()
            .rev()
            .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .and_then(|message| message.get("content").and_then(Value::as_str))
            .and_then(|text| text.split_once("## New Messages\n"))
            .and_then(|(_, text)| text.split_once("\n## Observation Date"))
            .map(|(text, _)| text);
        let Some(extraction) = extraction else {
            return (
                StatusCode::BAD_REQUEST,
                AxumJson(json!({"error":{"message":"Mem0 fixture omitted New Messages section"}})),
            )
                .into_response();
        };
        let memory: Vec<Value> = extraction
            .lines()
            .filter_map(|line| line.strip_prefix("user: "))
            .filter(|text| !text.trim().is_empty())
            .enumerate()
            .map(|(index, text)| json!({"id":index.to_string(),"text":text,"attributed_to":"user"}))
            .collect();
        return AxumJson(json!({
            "id":"mem0-extraction-fixture", "object":"chat.completion",
            "model":request.get("model").cloned().unwrap_or_else(|| json!("echo-model")),
            "choices":[{"index":0,"message":{"role":"assistant","content":json!({"memory":memory}).to_string()},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":24,"completion_tokens":9,"total_tokens":33}
        })).into_response();
    }
    let agent_purpose = request
        .pointer("/metadata/agentPurpose")
        .and_then(Value::as_str)
        .unwrap_or("agent_turn");
    let p3_overflow_requested = messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.contains("P3_CONTEXT_OVERFLOW"))
    });
    let p3_overflow_compacted = messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.contains("P3_OVERFLOW_COMPACTED"))
    });
    if agent_purpose == "agent_turn" && p3_overflow_requested && !p3_overflow_compacted {
        return (
            StatusCode::BAD_REQUEST,
            AxumJson(json!({"error":{"code":"context_length_exceeded","message":"maximum context length exceeded by P3 fixture"}})),
        )
            .into_response();
    }
    let tool_messages = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
        .count();
    let requested_loop = messages
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .any(|content| content.contains("m5-loop"));
    let kakj_identity = messages.iter().any(|message| {
        message.get("role").and_then(Value::as_str) == Some("system")
            && message
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|content| content.contains("你叫 kakj"))
    });
    let large_trace_response = messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.contains("TRACE_LARGE_RESPONSE"))
    });
    let p3_skill_context = messages.iter().any(|message| {
        message.get("role").and_then(Value::as_str) == Some("system")
            && message
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|content| content.contains("immutable V2-04 Skill result"))
    });
    let tools = request.get("tools").and_then(Value::as_array);
    let find_tool = |name: &str| {
        tools.and_then(|items| {
            items.iter().find_map(|candidate| {
                let candidate_name = candidate
                    .pointer("/function/name")
                    .and_then(Value::as_str)?;
                (candidate_name == name).then_some(candidate_name)
            })
        })
    };
    let tool = tools
        .and_then(|items| items.first())
        .and_then(|candidate| candidate.pointer("/function/name"))
        .and_then(Value::as_str);
    let latest_user_content = messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let requested_memory_write = latest_user_content.contains("P3_MEMORY_WRITE");
    let requested_memory_recall = latest_user_content.contains("P3_MEMORY_RECALL");
    let selected_tool = if requested_memory_write {
        request
            .get("tools")
            .and_then(Value::as_array)
            .and_then(|tools| {
                tools.iter().find_map(|candidate| {
                    (candidate.pointer("/function/name").and_then(Value::as_str)
                        == Some("memory_write"))
                    .then_some("memory_write")
                })
            })
            .or(tool)
    } else if requested_memory_recall {
        request
            .get("tools")
            .and_then(Value::as_array)
            .and_then(|tools| {
                tools.iter().find_map(|candidate| {
                    (candidate.pointer("/function/name").and_then(Value::as_str)
                        == Some("memory_recall"))
                    .then_some("memory_recall")
                })
            })
            .or(tool)
    } else {
        find_tool("echo").or(tool)
    };
    let tool_call = selected_tool
        .filter(|_| requested_loop || tool_messages == 0)
        .map(|name| {
            let arguments = match name {
                "memory_write" => json!({
                    "text":"p3-subject-memory",
                    "metadata":{"fixture":true}
                }),
                "memory_recall" => json!({"query":"p3-subject-memory","topK":5}),
                _ => tools
                    .and_then(|tools| {
                        tools.iter().find(|candidate| {
                            candidate.pointer("/function/name").and_then(Value::as_str)
                                == Some(name)
                        })
                    })
                    .and_then(|candidate| candidate.pointer("/function/parameters"))
                    .map(schema_example)
                    .unwrap_or_else(|| json!({"text":"m5-tool-result"})),
            };
            json!({
                "id": format!("m5-call-{tool_messages}"),
                "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()}
            })
        });
    let finish_reason = if tool_call.is_some() {
        "tool_calls"
    } else {
        "stop"
    };
    let structured_content = request
        .pointer("/response_format/json_schema/schema")
        .map(schema_example)
        .and_then(|value| serde_json::to_string(&value).ok());
    let content = tool_call.is_none().then(|| {
        if request.get("model").and_then(Value::as_str) == Some("echo-invalid-judge") {
            return "{\"passed\":\"invalid\",\"score\":2,\"reason\":42}".into();
        }
        structured_content.unwrap_or_else(|| {
            if agent_purpose == "compaction" && p3_overflow_requested {
                "P3_OVERFLOW_COMPACTED".into()
            } else if agent_purpose == "compaction" {
                "P3_THRESHOLD_COMPACTED".into()
            } else if kakj_identity {
                "你好，我叫 kakj。".into()
            } else if p3_skill_context {
                "P3-04 Agent attachment completed; skill_context=true".into()
            } else {
                "M5 Agent completed after the MCP tool result".into()
            }
        })
    });
    let usage = json!({"prompt_tokens": 24 + tool_messages, "completion_tokens": if tool_call.is_some() { 12 } else { 9 }, "total_tokens": 45 + tool_messages});
    if request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let model_name = request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("echo-model")
            .to_owned();
        let behavior_stream = matches!(
            model_name.as_str(),
            "echo-slow-stream" | "echo-abort-stream" | "echo-no-usage"
        ) && tool_call.is_none();
        let delta = if let Some(call) = tool_call {
            json!({"tool_calls":[{"index":0,"id":call["id"],"type":"function","function":{"name":call["function"]["name"],"arguments":call["function"]["arguments"]}}]})
        } else {
            let content = if large_trace_response {
                content
                    .as_ref()
                    .map(|text| format!("{text}\n{}", "trace-artifact-marker|".repeat(1_024)))
            } else {
                content.clone()
            };
            json!({"content":content})
        };
        // plan7 P7-B B6: behavior fixtures switch on the model name so the
        // streaming E2E can exercise slow streams, mid-stream aborts and
        // usage-less streams against the real worker transport.
        if behavior_stream {
            let tokens: Vec<String> = match &content {
                Some(text) => text.split_inclusive(' ').map(str::to_owned).collect(),
                None => vec!["echo".into()],
            };
            let abort = model_name.as_str() == "echo-abort-stream";
            let include_usage = model_name.as_str() != "echo-no-usage";
            let slow = model_name.as_str() == "echo-slow-stream";
            let usage_frame = usage.clone();
            let model_for_stream = model_name.clone();
            let last_index = tokens.len().saturating_sub(1);
            let stream = futures_util::stream::iter(tokens.into_iter().enumerate()).then(
                move |(index, token)| {
                    let usage_for_frame = usage_frame.clone();
                    let model_in_frame = model_for_stream.clone();
                    async move {
                        if slow {
                            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                        }
                        let frame = json!({
                            "id":"m5-stream","model":model_in_frame,
                            "choices":[{"index":0,"delta":{"content":token},"finish_reason":null}]
                        });
                        let mut frame_text = format!("data: {frame}\n\n");
                        if index == last_index && !abort {
                            let mut finished = json!({"id":"m5-stream","model":model_in_frame,"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
                            if include_usage {
                                finished["usage"] = usage_for_frame;
                            }
                            frame_text.push_str(&format!("data: {finished}\n\ndata: [DONE]\n\n"));
                        }
                        Ok::<Bytes, std::io::Error>(Bytes::from(frame_text))
                    }
                },
            )
            .take(if abort { 3 } else { usize::MAX });
            let body = Body::from_stream(stream);
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(body)
                .expect("fixture response");
        }
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({"id":"m5-stream","choices":[{"index":0,"delta":delta,"finish_reason":finish_reason}]}),
            json!({"id":"m5-stream","choices":[],"usage":usage})
        );
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .expect("fixture response");
    }
    let mut message = json!({"role":"assistant","content":content});
    if let Some(call) = tool_call {
        message["tool_calls"] = json!([call]);
    }
    let mut response = json!({
        "id":"m5-completion",
        "object":"chat.completion",
        "model":request.get("model").cloned().unwrap_or_else(||json!("echo-model")),
        "choices":[{"index":0,"message":message,"finish_reason":finish_reason}],
        "usage":usage
    });
    if large_trace_response {
        response["fixture_trace_payload"] = Value::String("trace-artifact-marker|".repeat(1_024));
    }
    AxumJson(response).into_response()
}

fn schema_example(schema: &Value) -> Value {
    if let Some(value) = schema.get("const") {
        return value.clone();
    }
    if let Some(value) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return value.clone();
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => Value::Object(
            schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(name, property)| (name.clone(), schema_example(property)))
                .collect(),
        ),
        Some("array") => Value::Array(vec![]),
        Some("number" | "integer") => json!(1),
        Some("boolean") => json!(true),
        Some("null") => Value::Null,
        _ => Value::String("structured-value".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{chat_completions, embeddings, schema_example};
    use axum::{
        Json,
        http::{HeaderMap, HeaderValue, StatusCode, header},
        response::IntoResponse,
    };
    use bytes::Bytes;
    use futures_util::StreamExt;
    use serde_json::{Value, json};

    #[test]
    fn structured_response_fixture_materializes_the_declared_object_shape() {
        assert_eq!(
            schema_example(
                &json!({"type":"object","properties":{"answer":{"type":"string"},"count":{"type":"integer"}}})
            ),
            json!({"answer":"structured-value","count":1})
        );
    }

    #[tokio::test]
    async fn usage_less_stream_emits_finish_and_done_after_all_content() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer m5-model-secret"),
        );
        let response = chat_completions(headers, Json(json!({"model":"echo-no-usage","stream":true,"messages":[{"role":"user","content":"hello"}]}))).await;
        let mut stream = response.into_body().into_data_stream();
        let mut frames: Vec<Bytes> = Vec::new();
        while let Some(frame) = stream.next().await {
            frames.push(frame.unwrap());
        }
        let text = frames
            .iter()
            .map(|frame| std::str::from_utf8(frame).unwrap())
            .collect::<String>();
        assert_eq!(text.matches("[DONE]").count(), 1);
        assert_eq!(text.matches("\"finish_reason\":\"stop\"").count(), 1);
        assert!(!text.contains("\"usage\""));
        assert!(text.ends_with("data: [DONE]\n\n"));
    }

    #[tokio::test]
    async fn model_fixture_requires_auth_and_completes_after_tool_output() {
        let unauthorized = chat_completions(HeaderMap::new(), Json(json!({}))).await;
        assert_eq!(unauthorized.status(), 401);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer m5-model-secret"),
        );
        let response = chat_completions(headers, Json(json!({"messages":[{"role":"tool","content":"ok"}],"tools":[{"function":{"name":"echo"}}]}))).await.into_response();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn model_fixture_prefers_echo_and_generates_arguments_from_its_schema() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer m5-model-secret"),
        );
        let response = chat_completions(
            headers,
            Json(json!({
                "messages":[{"role":"user","content":"use the attachment"}],
                "tools":[
                    {"function":{"name":"read","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}},
                    {"function":{"name":"echo","parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}}}
                ]
            })),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value.pointer("/choices/0/message/tool_calls/0/function/name"),
            Some(&json!("echo"))
        );
        let arguments = value
            .pointer("/choices/0/message/tool_calls/0/function/arguments")
            .and_then(Value::as_str)
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
            .unwrap();
        assert_eq!(arguments, json!({"text":"structured-value"}));
    }

    #[tokio::test]
    async fn model_fixture_applies_the_system_identity_prompt() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer m5-model-secret"),
        );
        let response = chat_completions(
            headers,
            Json(json!({"messages":[
                {"role":"system","content":"你叫 kakj"},
                {"role":"user","content":"你是谁？"}
            ]})),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value.pointer("/choices/0/message/content"),
            Some(&json!("你好，我叫 kakj。"))
        );
    }

    #[tokio::test]
    async fn model_fixture_reports_p3_skill_external_context() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer m5-model-secret"),
        );
        let response = chat_completions(
            headers,
            Json(json!({"messages":[
                {"role":"system","content":"Return the immutable V2-04 Skill result."},
                {"role":"tool","content":"mcp complete"}
            ],"tools":[{"function":{"name":"echo"}}]})),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value.pointer("/choices/0/message/content"),
            Some(&json!(
                "P3-04 Agent attachment completed; skill_context=true"
            ))
        );
    }

    #[tokio::test]
    async fn model_fixture_can_emit_a_large_trace_only_provider_response() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer m5-model-secret"),
        );
        let response = chat_completions(
            headers,
            Json(json!({"messages":[{
                "role":"system",
                "content":"你叫 kakj\nTRACE_LARGE_RESPONSE"
            }]})),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.len() > 16 * 1024);
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            value["fixture_trace_payload"]
                .as_str()
                .is_some_and(|payload| payload.contains("trace-artifact-marker"))
        );
        assert_eq!(
            value.pointer("/choices/0/message/content"),
            Some(&json!("你好，我叫 kakj。"))
        );
    }

    #[tokio::test]
    async fn streaming_model_fixture_emits_large_assistant_content_for_trace() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer m5-model-secret"),
        );
        let response = chat_completions(
            headers,
            Json(json!({"stream":true,"messages":[{
                "role":"system",
                "content":"你叫 kakj\nTRACE_LARGE_RESPONSE"
            }]})),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body = std::str::from_utf8(&bytes).unwrap();
        let frame: serde_json::Value = serde_json::from_str(
            body.lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap(),
        )
        .unwrap();
        let content = frame
            .pointer("/choices/0/delta/content")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(content.starts_with("你好，我叫 kakj。"));
        assert!(content.len() > 16 * 1024);
        assert!(body.ends_with("data: [DONE]\n\n"));
    }

    #[tokio::test]
    async fn mem0_extraction_fixture_keeps_the_actual_new_user_facts() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer m5-model-secret"),
        );
        let response = chat_completions(headers, Json(json!({
            "response_format":{"type":"json_object"},
            "messages":[
                {"role":"system","content":"You are a Memory Extractor."},
                {"role":"user","content":"## Existing Memories\n[{\"text\":\"do not copy existing data\"}]\n## New Messages\nuser: p3-subject-memory\nuser: second-scoped-fact\n## Observation Date\n2026-10-10"}
            ]
        }))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let extraction: Value = serde_json::from_str(
            value
                .pointer("/choices/0/message/content")
                .unwrap()
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            extraction,
            json!({"memory":[
                {"id":"0","text":"p3-subject-memory","attributed_to":"user"},
                {"id":"1","text":"second-scoped-fact","attributed_to":"user"}
            ]})
        );
    }

    #[tokio::test]
    async fn embedding_fixture_is_authenticated_and_deterministic() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer m5-model-secret"),
        );
        let request = json!({"model":"text-embedding-3-small","input":["alpha","beta"]});
        let first = embeddings(headers.clone(), Json(request.clone())).await;
        let second = embeddings(headers, Json(request)).await;
        assert_eq!(first.status(), 200);
        assert_eq!(second.status(), 200);
        assert_eq!(
            axum::body::to_bytes(first.into_body(), usize::MAX)
                .await
                .unwrap(),
            axum::body::to_bytes(second.into_body(), usize::MAX)
                .await
                .unwrap()
        );
    }
}
