use futures_util::{StreamExt, stream::BoxStream};
use reqwest::header::HeaderMap;
use serde_json::Value;
use tokio::sync::mpsc;

use super::{WorkerProvider, WorkerProviderError, WorkerProviderResponse};
use crate::egress::{
    EgressRequestContext, LegacySseExchange, LegacySseExchangeError, LegacySseExchangeResponse,
    ProviderHttpClient,
};

#[async_trait::async_trait]
impl WorkerProvider for ProviderHttpClient {
    async fn post_json(
        &self,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: std::time::Duration,
        headers: HeaderMap,
        body: &Value,
    ) -> Result<WorkerProviderResponse, WorkerProviderError> {
        let mut request = self
            .post(endpoint, context, timeout)
            .map_err(|error| WorkerProviderError::Denied(error.to_string()))?;
        for (name, value) in headers {
            if let Some(name) = name {
                request = request.header(name, value);
            }
        }
        response(request.json(body).send().await).await
    }

    async fn post_json_stream(
        &self,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: std::time::Duration,
        headers: HeaderMap,
        body: &Value,
    ) -> Result<super::WorkerStreamResponse, WorkerProviderError> {
        let mut request = self
            .post(endpoint, context, timeout)
            .map_err(|error| WorkerProviderError::Denied(error.to_string()))?;
        for (name, value) in headers {
            if let Some(name) = name {
                request = request.header(name, value);
            }
        }
        let response =
            request
                .json(body)
                .send()
                .await
                .map_err(|error| WorkerProviderError::Request {
                    message: error.to_string(),
                    is_connect: error.is_connect(),
                })?;
        let status = response.status();
        Ok(super::WorkerStreamResponse { status, response })
    }

    async fn post_sandbox_manager_json(
        &self,
        endpoint: &str,
        _context: EgressRequestContext,
        timeout: std::time::Duration,
        headers: HeaderMap,
        body: &Value,
    ) -> Result<WorkerProviderResponse, WorkerProviderError> {
        let mut request = self
            .post_sandbox_manager(endpoint, timeout)
            .map_err(|error| WorkerProviderError::Denied(error.to_string()))?;
        for (name, value) in headers {
            if let Some(name) = name {
                request = request.header(name, value);
            }
        }
        reqwest_response(request.json(body).send().await).await
    }

    async fn request_json(
        &self,
        method: &str,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: std::time::Duration,
        headers: HeaderMap,
        body: Option<&Value>,
    ) -> Result<WorkerProviderResponse, WorkerProviderError> {
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|error| WorkerProviderError::Denied(error.to_string()))?;
        let mut request = self
            .request(method, endpoint, context, timeout)
            .map_err(|error| WorkerProviderError::Denied(error.to_string()))?;
        for (name, value) in headers {
            if let Some(name) = name {
                request = request.header(name, value);
            }
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        response(request.send().await).await
    }

    async fn legacy_sse_rpc(
        &self,
        session_key: &str,
        endpoint: &str,
        context: EgressRequestContext,
        timeout: std::time::Duration,
        headers: HeaderMap,
        body: &Value,
    ) -> Result<WorkerProviderResponse, WorkerProviderError> {
        let response = legacy_sse_exchange(
            self,
            session_key,
            endpoint,
            context,
            timeout,
            headers,
            body.clone(),
        )
        .await
        .map_err(|error| WorkerProviderError::Request {
            message: error.message,
            is_connect: error.is_connect,
        })?;
        Ok(WorkerProviderResponse {
            status: response.status,
            headers: response.headers,
            body: response.body,
        })
    }

    async fn close_legacy_sse_session(&self, session_key: &str) {
        self.legacy_sse_sessions.lock().await.remove(session_key);
    }
}

async fn legacy_sse_exchange(
    client: &ProviderHttpClient,
    session_key: &str,
    endpoint: &str,
    context: EgressRequestContext,
    timeout: std::time::Duration,
    headers: HeaderMap,
    body: Value,
) -> Result<LegacySseExchangeResponse, LegacySseExchangeError> {
    for _ in 0..2 {
        let sender = {
            let mut sessions = client.legacy_sse_sessions.lock().await;
            if let Some(sender) = sessions.get(session_key) {
                sender.clone()
            } else {
                let (sender, receiver) = mpsc::channel(8);
                sessions.insert(session_key.to_owned(), sender.clone());
                tokio::spawn(run_legacy_sse_session(
                    client.clone(),
                    endpoint.to_owned(),
                    context,
                    receiver,
                ));
                sender
            }
        };
        let (response, result) = tokio::sync::oneshot::channel();
        if sender
            .send(LegacySseExchange {
                headers: headers.clone(),
                body: body.clone(),
                timeout,
                response,
            })
            .await
            .is_err()
        {
            client.legacy_sse_sessions.lock().await.remove(session_key);
            continue;
        }
        return result.await.map_err(|_| LegacySseExchangeError {
            message: "Legacy SSE session ended before responding".into(),
            is_connect: false,
        })?;
    }
    Err(LegacySseExchangeError {
        message: "Legacy SSE session could not be established".into(),
        is_connect: true,
    })
}

struct LegacySseConnection {
    message_endpoint: reqwest::Url,
    stream: BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>,
    buffer: Vec<u8>,
}

async fn run_legacy_sse_session(
    client: ProviderHttpClient,
    endpoint: String,
    context: EgressRequestContext,
    mut requests: mpsc::Receiver<LegacySseExchange>,
) {
    let Some(first) = requests.recv().await else {
        return;
    };
    let mut connection = match tokio::time::timeout(
        first.timeout,
        connect_legacy_sse(&client, &endpoint, context, &first.headers),
    )
    .await
    {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => {
            let _ = first.response.send(Err(error));
            return;
        }
        Err(_) => {
            let _ = first.response.send(Err(LegacySseExchangeError {
                message: "Legacy SSE connect timed out".into(),
                is_connect: true,
            }));
            return;
        }
    };
    let mut current = Some(first);
    loop {
        let exchange = if let Some(exchange) = current.take() {
            exchange
        } else {
            let Some(exchange) = requests.recv().await else {
                return;
            };
            exchange
        };
        let result = tokio::time::timeout(
            exchange.timeout,
            execute_legacy_sse_exchange(&client, context, &mut connection, &exchange),
        )
        .await
        .unwrap_or_else(|_| {
            Err(LegacySseExchangeError {
                message: "Legacy SSE RPC timed out".into(),
                is_connect: false,
            })
        });
        let failed = result.is_err();
        let _ = exchange.response.send(result);
        if failed {
            return;
        }
    }
}

async fn connect_legacy_sse(
    client: &ProviderHttpClient,
    endpoint: &str,
    context: EgressRequestContext,
    headers: &HeaderMap,
) -> Result<LegacySseConnection, LegacySseExchangeError> {
    let mut connect = client
        .get(endpoint, context, std::time::Duration::from_secs(300))
        .map_err(|error| LegacySseExchangeError {
            message: error.to_string(),
            is_connect: true,
        })?;
    for (name, value) in headers {
        connect = connect.header(name, value);
    }
    let response = connect
        .header("accept", "text/event-stream")
        .send()
        .await
        .map_err(|error| LegacySseExchangeError {
            message: error.to_string(),
            is_connect: error.is_connect(),
        })?;
    if !response.status().is_success() {
        return Err(LegacySseExchangeError {
            message: format!("Legacy SSE connect returned HTTP {}", response.status()),
            is_connect: false,
        });
    }
    let base = response.url().clone();
    let mut connection = LegacySseConnection {
        message_endpoint: base.clone(),
        stream: response.bytes_stream().boxed(),
        buffer: Vec::new(),
    };
    loop {
        let (kind, data) = next_legacy_sse_event(&mut connection).await?;
        if kind == "endpoint" {
            connection.message_endpoint =
                base.join(data.trim())
                    .map_err(|error| LegacySseExchangeError {
                        message: error.to_string(),
                        is_connect: false,
                    })?;
            return Ok(connection);
        }
    }
}

async fn execute_legacy_sse_exchange(
    client: &ProviderHttpClient,
    context: EgressRequestContext,
    connection: &mut LegacySseConnection,
    exchange: &LegacySseExchange,
) -> Result<LegacySseExchangeResponse, LegacySseExchangeError> {
    let expected_id = exchange.body.get("id").cloned();
    let mut post = client
        .post(
            connection.message_endpoint.as_str(),
            context,
            std::time::Duration::from_secs(300),
        )
        .map_err(|error| LegacySseExchangeError {
            message: error.to_string(),
            is_connect: false,
        })?;
    for (name, value) in &exchange.headers {
        post = post.header(name, value);
    }
    let response =
        post.json(&exchange.body)
            .send()
            .await
            .map_err(|error| LegacySseExchangeError {
                message: error.to_string(),
                is_connect: error.is_connect(),
            })?;
    if !response.status().is_success() {
        return Err(LegacySseExchangeError {
            message: format!("Legacy SSE message returned HTTP {}", response.status()),
            is_connect: false,
        });
    }
    if expected_id.is_none() {
        return Ok(LegacySseExchangeResponse {
            status: reqwest::StatusCode::ACCEPTED,
            headers: HeaderMap::new(),
            body: bytes::Bytes::from_static(b"null"),
        });
    }
    loop {
        let (kind, data) = next_legacy_sse_event(connection).await?;
        if kind != "message" {
            continue;
        }
        let value: Value =
            serde_json::from_str(data.trim()).map_err(|error| LegacySseExchangeError {
                message: error.to_string(),
                is_connect: false,
            })?;
        if value.get("id") == expected_id.as_ref() {
            return Ok(LegacySseExchangeResponse {
                status: reqwest::StatusCode::OK,
                headers: HeaderMap::new(),
                body: bytes::Bytes::from(serde_json::to_vec(&value).unwrap_or_default()),
            });
        }
    }
}

async fn next_legacy_sse_event(
    connection: &mut LegacySseConnection,
) -> Result<(String, String), LegacySseExchangeError> {
    loop {
        if let Some((boundary, separator)) = legacy_sse_boundary(&connection.buffer) {
            let event = connection.buffer[..boundary].to_vec();
            connection.buffer.drain(..boundary + separator);
            return Ok(parse_legacy_sse_event(&String::from_utf8_lossy(&event)));
        }
        let chunk = connection
            .stream
            .next()
            .await
            .ok_or_else(|| LegacySseExchangeError {
                message: "Legacy SSE stream ended".into(),
                is_connect: false,
            })?
            .map_err(|error| LegacySseExchangeError {
                message: error.to_string(),
                is_connect: error.is_connect(),
            })?;
        connection.buffer.extend_from_slice(&chunk);
        if connection.buffer.len() > 1024 * 1024 {
            return Err(LegacySseExchangeError {
                message: "Legacy SSE event exceeds 1 MiB".into(),
                is_connect: false,
            });
        }
    }
}

fn legacy_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (position, 4))
        .or_else(|| {
            buffer
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| (position, 2))
        })
}

fn parse_legacy_sse_event(event: &str) -> (String, String) {
    let mut kind = "message".to_owned();
    let mut data = String::new();
    for line in event.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            kind = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
    (kind, data)
}

async fn response(
    result: Result<reqwest::Response, crate::egress::ProviderRequestError>,
) -> Result<WorkerProviderResponse, WorkerProviderError> {
    let response = result.map_err(|error| WorkerProviderError::Request {
        message: error.to_string(),
        is_connect: error.is_connect(),
    })?;
    Ok(WorkerProviderResponse {
        status: response.status(),
        headers: response.headers().clone(),
        body: response
            .bytes()
            .await
            .map_err(|error| WorkerProviderError::Request {
                message: error.to_string(),
                is_connect: error.is_connect(),
            })?,
    })
}

async fn reqwest_response(
    result: Result<reqwest::Response, reqwest::Error>,
) -> Result<WorkerProviderResponse, WorkerProviderError> {
    let response = result.map_err(|error| WorkerProviderError::Request {
        message: error.to_string(),
        is_connect: error.is_connect(),
    })?;
    Ok(WorkerProviderResponse {
        status: response.status(),
        headers: response.headers().clone(),
        body: response
            .bytes()
            .await
            .map_err(|error| WorkerProviderError::Request {
                message: error.to_string(),
                is_connect: error.is_connect(),
            })?,
    })
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{
        Json, Router,
        body::Body,
        extract::State,
        http::{Response, header::CONTENT_TYPE},
        routing::{get, post},
    };
    use uuid::Uuid;

    use super::*;

    #[derive(Clone)]
    struct LegacySseFixture {
        connections: Arc<AtomicUsize>,
        sender: Arc<tokio::sync::Mutex<Option<mpsc::Sender<bytes::Bytes>>>>,
    }

    #[test]
    fn legacy_sse_parser_preserves_multiline_data_and_fragment_boundaries() {
        let (kind, data) =
            parse_legacy_sse_event("event: message\ndata: {\"jsonrpc\":\"2.0\",\ndata: \"id\":1}");
        assert_eq!(kind, "message");
        assert_eq!(data, "{\"jsonrpc\":\"2.0\",\n\"id\":1}");
        assert_eq!(
            legacy_sse_boundary(b"event: endpoint\r\ndata: /rpc\r\n\r\nrest"),
            Some((27, 4))
        );
        assert_eq!(
            legacy_sse_boundary(b"data: first\n\ndata: second"),
            Some((11, 2))
        );
        assert_eq!(legacy_sse_boundary(b"data: incomplete"), None);
    }

    #[tokio::test]
    async fn legacy_sse_reuses_one_stream_and_correlates_fragmented_responses() {
        let fixture = LegacySseFixture {
            connections: Arc::new(AtomicUsize::new(0)),
            sender: Arc::new(tokio::sync::Mutex::new(None)),
        };
        let router = Router::new()
            .route(
                "/sse",
                get(
                    |State(fixture): State<LegacySseFixture>| async move {
                        fixture.connections.fetch_add(1, Ordering::SeqCst);
                        let (sender, mut receiver) = mpsc::channel(8);
                        *fixture.sender.lock().await = Some(sender.clone());
                        sender
                            .send(bytes::Bytes::from_static(
                                b"event: endpoint\ndata: /rpc\n\n",
                            ))
                            .await
                            .unwrap();
                        let stream = async_stream::stream! {
                            while let Some(chunk) = receiver.recv().await {
                                yield Ok::<_, Infallible>(chunk);
                            }
                        };
                        Response::builder()
                            .status(reqwest::StatusCode::OK)
                            .header(CONTENT_TYPE, "text/event-stream")
                            .body(Body::from_stream(stream))
                            .unwrap()
                    },
                ),
            )
            .route(
                "/rpc",
                post(
                    |State(fixture): State<LegacySseFixture>, Json(body): Json<Value>| async move {
                        let sender = fixture
                            .sender
                            .lock()
                            .await
                            .clone()
                            .expect("SSE stream must be connected before POST");
                        sender
                            .send(bytes::Bytes::from_static(
                                b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n",
                            ))
                            .await
                            .unwrap();
                        let response = format!(
                            "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"ok\":true}}}}\n\n",
                            body["id"]
                        );
                        let boundary = response.len() / 2;
                        sender
                            .send(bytes::Bytes::copy_from_slice(
                                &response.as_bytes()[..boundary],
                            ))
                            .await
                            .unwrap();
                        sender
                            .send(bytes::Bytes::copy_from_slice(
                                &response.as_bytes()[boundary..],
                            ))
                            .await
                            .unwrap();
                        reqwest::StatusCode::ACCEPTED
                    },
                ),
            )
            .with_state(fixture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = crate::egress::ProviderHttpClient::direct_for_test(address);
        let endpoint = format!("http://echo-mcp:{}/sse", address.port());
        let context = EgressRequestContext::execution(Uuid::now_v7(), Uuid::now_v7());
        for id in [1, 2] {
            let response = client
                .legacy_sse_rpc(
                    "agent-run:fixture:mcp-server:fixture",
                    &endpoint,
                    context,
                    std::time::Duration::from_secs(5),
                    HeaderMap::new(),
                    &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/list"}),
                )
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(body["id"], id);
            assert_eq!(body["result"]["ok"], true);
        }
        assert_eq!(fixture.connections.load(Ordering::SeqCst), 1);
        client
            .close_legacy_sse_session("agent-run:fixture:mcp-server:fixture")
            .await;
        assert!(client.legacy_sse_sessions.lock().await.is_empty());
        server.abort();
    }
}
