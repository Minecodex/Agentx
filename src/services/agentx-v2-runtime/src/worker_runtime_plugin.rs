use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::LazyLock,
};

use agentx_node_protocol::{Item, NodeCapability, PluginNodeBinding};
use agentx_runtime_contracts::{
    ContentHash, RuntimeObjectReferenceV1, RuntimeResourceConfigurationV1, RuntimeResourceKindV1,
    TraceContentKindV1, TraceEventKindV1, TraceSpanKindV1, WorkerResultStatusV1,
    deterministic_uuid,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
    time::{Duration, timeout},
};

use super::{ClaimedWorkerAttempt, RuntimeWorker, WorkerExecution};

#[path = "worker_runtime_plugin_files.rs"]
mod files;

static DESIGN_SLOTS: LazyLock<std::sync::Arc<Semaphore>> = LazyLock::new(|| {
    let maximum = std::env::var("AGENTX_PLUGIN_DESIGN_MAX_PROCESSES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 16);
    std::sync::Arc::new(Semaphore::new(maximum))
});

#[derive(Deserialize)]
struct RpcResponse {
    id: Value,
    #[serde(default)]
    result: Option<PluginResult>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PluginResult {
    Completed {
        outputs: std::collections::BTreeMap<String, Vec<Item>>,
        #[serde(default, rename = "traceDiagnostics")]
        diagnostics: Value,
    },
    Failed {
        code: String,
        message: String,
        #[serde(default)]
        retryable: bool,
        #[serde(default)]
        details: Value,
        #[serde(default, rename = "traceDiagnostics")]
        diagnostics: Value,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginTrace {
    name: String,
    status: String,
    #[serde(default)]
    attributes: Value,
}

#[derive(Deserialize)]
struct PluginContent {
    #[serde(rename = "type")]
    content_type: String,
    version: u32,
    data: Value,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveTraceEvent {
    invocation_id: String,
    index: usize,
    #[serde(default)]
    parent_index: Option<usize>,
    phase: String,
    span: PluginTrace,
    #[serde(default)]
    content: Option<PluginContent>,
    #[serde(default)]
    event: Option<Value>,
}

#[derive(Clone)]
pub(crate) struct PluginTraceSink {
    events: tokio::sync::mpsc::Sender<(std::sync::Arc<ClaimedWorkerAttempt>, LiveTraceEvent)>,
    diagnostics: tokio::sync::mpsc::Sender<(std::sync::Arc<ClaimedWorkerAttempt>, u64)>,
}

impl PluginTraceSink {
    pub(crate) fn new(
        pool: sqlx::MySqlPool,
        objects: std::sync::Arc<dyn object_store::ObjectStore>,
    ) -> Self {
        let (events, mut event_rx) = tokio::sync::mpsc::channel::<(
            std::sync::Arc<ClaimedWorkerAttempt>,
            LiveTraceEvent,
        )>(128);
        let (diagnostics, mut diagnostic_rx) =
            tokio::sync::mpsc::channel::<(std::sync::Arc<ClaimedWorkerAttempt>, u64)>(16);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    Some((claim, dropped)) = diagnostic_rx.recv() => emit_trace_diagnostics(&pool, &claim, dropped).await,
                    Some((claim, event)) = event_rx.recv() => {
                        if !emit_live_trace_event(&pool, &objects, &claim, &event).await {
                            tracing::warn!(attempt_id=%claim.task.attempt_id, "Plugin Trace delivery failed");
                        }
                    }
                    else => break,
                }
            }
        });
        Self {
            events,
            diagnostics,
        }
    }

    fn event(&self, claim: std::sync::Arc<ClaimedWorkerAttempt>, event: LiveTraceEvent) -> bool {
        self.events.try_send((claim, event)).is_ok()
    }

    fn diagnostics(&self, claim: std::sync::Arc<ClaimedWorkerAttempt>, dropped: u64) {
        if dropped > 0 && self.diagnostics.try_send((claim.clone(), dropped)).is_err() {
            tracing::warn!(attempt_id=%claim.task.attempt_id, dropped, "Plugin Trace diagnostics queue is full");
        }
    }
}

struct PluginProcess {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    process_tree: ProcessTreeGuard,
    _capacity: OwnedSemaphorePermit,
}

pub(crate) struct PluginArtifactCache {
    root: PathBuf,
    maximum_entries: usize,
    io: Mutex<()>,
}

impl PluginArtifactCache {
    pub(crate) fn from_env() -> Self {
        let root = std::env::var_os("AGENTX_PLUGIN_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("agentx-plugin-runtime-cache-v1"));
        let maximum_entries = std::env::var("AGENTX_PLUGIN_CACHE_MAX_ENTRIES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(128)
            .clamp(8, 4_096);
        Self {
            root,
            maximum_entries,
            io: Mutex::new(()),
        }
    }

    pub(crate) fn for_tests() -> Self {
        Self {
            root: std::env::temp_dir().join(format!(
                "agentx-plugin-runtime-cache-test-{}",
                uuid::Uuid::now_v7()
            )),
            maximum_entries: 16,
            io: Mutex::new(()),
        }
    }

    fn path(&self, content_hash: &str) -> PathBuf {
        self.root.join(format!(
            "{}.mjs",
            content_hash.strip_prefix("sha256:").unwrap_or("invalid")
        ))
    }

    async fn load(
        &self,
        worker: &RuntimeWorker,
        claim: &ClaimedWorkerAttempt,
        plugin: &PluginNodeBinding,
    ) -> Result<String, WorkerExecution> {
        let artifact = plugin.runtime_artifact.as_ref().ok_or_else(|| {
            WorkerExecution::failed(
                "PLUGIN_ARTIFACT_INVALID",
                "Plugin execution is missing its frozen Runtime artifact reference",
                false,
            )
        })?;
        let expected_hash = ContentHash::parse(artifact.content_hash.clone()).map_err(|error| {
            WorkerExecution::failed("PLUGIN_ARTIFACT_INVALID", error.to_string(), false)
        })?;
        let _guard = self.io.lock().await;
        let cache_path = self.path(expected_hash.as_str());
        if let Ok(bytes) = tokio::fs::read(&cache_path).await
            && artifact_bytes_match(&bytes, artifact.size_bytes, expected_hash.as_str())
        {
            return String::from_utf8(bytes).map_err(|error| {
                WorkerExecution::failed("PLUGIN_ARTIFACT_INVALID", error.to_string(), false)
            });
        }
        let _ = tokio::fs::remove_file(&cache_path).await;
        let object_key = RuntimeObjectReferenceV1::canonical_key(
            claim.task.tenant_id,
            artifact.object_id,
            &expected_hash,
        );
        let bytes = worker
            .objects
            .get(&object_store::path::Path::from(object_key))
            .await
            .map_err(|error| {
                WorkerExecution::failed(
                    "PLUGIN_ARTIFACT_MISSING",
                    format!("Frozen plugin Runtime artifact is unavailable: {error}"),
                    false,
                )
            })?
            .bytes()
            .await
            .map_err(|error| {
                WorkerExecution::failed(
                    "PLUGIN_ARTIFACT_MISSING",
                    format!("Frozen plugin Runtime artifact could not be read: {error}"),
                    false,
                )
            })?;
        if !artifact_bytes_match(&bytes, artifact.size_bytes, expected_hash.as_str()) {
            return Err(WorkerExecution::failed(
                "PLUGIN_ARTIFACT_CORRUPT",
                "Frozen plugin Runtime artifact failed its size or digest check",
                false,
            ));
        }
        let source = String::from_utf8(bytes.to_vec()).map_err(|error| {
            WorkerExecution::failed("PLUGIN_ARTIFACT_INVALID", error.to_string(), false)
        })?;
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|error| {
                WorkerExecution::failed("PLUGIN_CACHE_WRITE_FAILED", error.to_string(), false)
            })?;
        let temporary = cache_path.with_extension(format!("{}.tmp", std::process::id()));
        tokio::fs::write(&temporary, source.as_bytes())
            .await
            .map_err(|error| {
                WorkerExecution::failed("PLUGIN_CACHE_WRITE_FAILED", error.to_string(), false)
            })?;
        if let Err(error) = tokio::fs::rename(&temporary, &cache_path).await {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(WorkerExecution::failed(
                "PLUGIN_CACHE_WRITE_FAILED",
                error.to_string(),
                false,
            ));
        }
        self.prune().await;
        Ok(source)
    }

    async fn prune(&self) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.root).await else {
            return;
        };
        let mut files = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Ok(metadata) = entry.metadata().await
                && metadata.is_file()
            {
                files.push((metadata.modified().ok(), entry.path()));
            }
        }
        files.sort_by_key(|(modified, _)| *modified);
        let remove_count = files.len().saturating_sub(self.maximum_entries);
        for (_, path) in files.into_iter().take(remove_count) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

fn artifact_bytes_match(bytes: &[u8], expected_size: u64, expected_hash: &str) -> bool {
    bytes.len() as u64 == expected_size
        && format!("sha256:{:x}", Sha256::digest(bytes)) == expected_hash
}

async fn spawn_plugin_process(
    node: &str,
    runner: &str,
    directory: &Path,
    capacity: OwnedSemaphorePermit,
) -> std::io::Result<PluginProcess> {
    let mut command = Command::new(node);
    let memory_mb = std::env::var("AGENTX_PLUGIN_MEMORY_MB")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(96)
        .clamp(32, 1_024);
    command
        .arg(format!("--max-old-space-size={memory_mb}"))
        .arg(std::path::absolute(runner)?)
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let process_tree = ProcessTreeGuard::attach(&child)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("Runner stdin is unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("Runner stdout is unavailable"))?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let message = line.chars().take(4_096).collect::<String>();
                tracing::info!(target:"agentx_plugin", %message);
            }
        });
    }
    let mut lines = BufReader::new(stdout).lines();
    let initialize_id = uuid::Uuid::now_v7().to_string();
    let initialize = json!({"jsonrpc":"2.0","id":initialize_id,"method":"runner.initialize","params":{"protocolVersion":2,"sdkApiVersion":2}});
    stdin
        .write_all(format!("{initialize}\n").as_bytes())
        .await?;
    let response = timeout(Duration::from_secs(5), lines.next_line())
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "Runner handshake timed out")
        })??
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Runner returned no handshake",
            )
        })?;
    let response: Value = serde_json::from_str(&response)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if response.get("id").and_then(Value::as_str) != Some(initialize_id.as_str())
        || response
            .get("result")
            .and_then(|value| value.get("protocolVersion"))
            .and_then(Value::as_u64)
            != Some(2)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Plugin Runner handshake failed",
        ));
    }
    Ok(PluginProcess {
        child,
        stdin,
        lines,
        process_tree,
        _capacity: capacity,
    })
}

pub(crate) async fn invoke_design_operation(
    worker: &RuntimeWorker,
    tenant_id: uuid::Uuid,
    operation_id: uuid::Uuid,
    resources: &[agentx_runtime_contracts::RuntimeResourceBindingV1],
    binding: &PluginNodeBinding,
    method: &str,
    mut parameters: Value,
) -> Result<Value, String> {
    if !matches!(method, "node.resolveDefinition" | "node.invokeProvider") {
        return Err(format!("Unsupported plugin design method '{method}'"));
    }
    let node = std::env::var("AGENTX_NODE_EXECUTABLE").unwrap_or_else(|_| "node".into());
    let runner = std::env::var("AGENTX_PLUGIN_RUNNER_PATH").unwrap_or_else(|_| {
        let local = Path::new("src/plugins/packages/plugin-runner/runner.mjs");
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/packages/plugin-runner/runner.mjs");
        if local.is_file() {
            local.to_string_lossy().into_owned()
        } else if workspace.is_file() {
            workspace.to_string_lossy().into_owned()
        } else {
            "/opt/agentx/plugin-runner/runner.mjs".into()
        }
    });
    let capacity = timeout(Duration::from_secs(5), DESIGN_SLOTS.clone().acquire_owned())
        .await
        .map_err(|_| "Plugin design operation expired while waiting for capacity".to_owned())?
        .map_err(|error| error.to_string())?;
    let mut files = files::InvocationFiles::new().map_err(|error| error.to_string())?;
    let mut process = spawn_plugin_process(&node, &runner, files.path(), capacity)
        .await
        .map_err(|error| error.to_string())?;
    parameters["runtimeSource"] = Value::String(binding.runtime_source.clone());
    parameters["runtimeEntry"] = Value::String(binding.runtime_entry.clone());
    parameters["deadlineMs"] = Value::from(10_000);
    let request_id = uuid::Uuid::now_v7().to_string();
    let request = json!({"jsonrpc":"2.0","id":request_id,"method":method,"params":parameters});
    if let Err(error) = process
        .stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
    {
        terminate_process_tree(&process.process_tree, &mut process.child).await;
        return Err(error.to_string());
    }
    let wait = async {
        let mut call_index = 0_u32;
        loop {
            let line = process
                .lines
                .next_line()
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Plugin Runner returned no response".to_owned())?;
            let message: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
            if let Some(host_method) = message.get("method").and_then(Value::as_str) {
                call_index = call_index.saturating_add(1);
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let response = match handle_design_host_call(
                    worker,
                    tenant_id,
                    operation_id,
                    resources,
                    host_method,
                    message.get("params").cloned().unwrap_or(Value::Null),
                    call_index,
                    &mut files,
                )
                .await
                {
                    Ok(result) => {
                        files.observe(&result);
                        json!({"jsonrpc":"2.0","id":id,"result":result})
                    }
                    Err((code, message)) => {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
                    }
                };
                process
                    .stdin
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .map_err(|error| error.to_string())?;
                continue;
            }
            return Ok::<Value, String>(message);
        }
    };
    let response = match timeout(Duration::from_secs(10), wait).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            terminate_process_tree(&process.process_tree, &mut process.child).await;
            return Err(error);
        }
        Err(_) => {
            let cancel = json!({"jsonrpc":"2.0","id":format!("cancel:{request_id}"),"method":"invocation.cancel","params":{"invocationId":format!("design:{request_id}")}});
            let _ = process
                .stdin
                .write_all(format!("{cancel}\n").as_bytes())
                .await;
            terminate_process_tree(&process.process_tree, &mut process.child).await;
            return Err("Plugin design operation timed out".into());
        }
    };
    terminate_process_tree(&process.process_tree, &mut process.child).await;
    if response.get("id").and_then(Value::as_str) != Some(request_id.as_str()) {
        return Err("Plugin Runner response ID mismatch".into());
    }
    if let Some(error) = response.get("error") {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Plugin design operation failed")
            .into());
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

#[allow(clippy::too_many_arguments)]
async fn handle_design_host_call(
    worker: &RuntimeWorker,
    tenant_id: uuid::Uuid,
    operation_id: uuid::Uuid,
    resources: &[agentx_runtime_contracts::RuntimeResourceBindingV1],
    method: &str,
    params: Value,
    call_index: u32,
    files: &mut files::InvocationFiles,
) -> Result<Value, (i64, String)> {
    let input = params.get("input").cloned().unwrap_or(Value::Null);
    match method {
        "host.credentials.list" => Ok(credential_descriptors(resources)),
        "host.http" => {
            let mut normalized = input;
            if let Some(headers) = normalized
                .get("headers")
                .and_then(Value::as_object)
                .cloned()
            {
                normalized["headers"] = Value::Array(
                    headers
                        .into_iter()
                        .map(|(name, value)| json!({"name":name,"value":value}))
                        .collect(),
                );
            }
            let (mut endpoint, request) = super::declarative_http_request(&normalized, None)
                .map_err(|message| (-32602, message))?;
            let method_name = request
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .to_ascii_uppercase();
            if !matches!(method_name.as_str(), "GET" | "HEAD" | "OPTIONS") {
                return Err((
                    -32602,
                    "Design-time HTTP providers may only perform read operations".into(),
                ));
            }
            let mut headers = request_headers(&request)?;
            if let Some(credential_index) = normalized
                .get("credentialIndex")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
            {
                let binding = resources
                    .iter()
                    .filter(|binding| binding.resource_kind == RuntimeResourceKindV1::Credential)
                    .nth(credential_index)
                    .ok_or_else(|| (-32602, "Credential resource is not bound".into()))?;
                apply_design_credential(worker, binding, &request, &mut endpoint, &mut headers)
                    .await?;
            }
            let response = worker
                .provider
                .request_json(
                    &method_name,
                    &endpoint,
                    crate::egress::EgressRequestContext::request(tenant_id, operation_id),
                    Duration::from_secs(8),
                    headers,
                    request.get("body").filter(|body| !body.is_null()),
                )
                .await
                .map_err(design_provider_error)?;
            Ok(design_http_response(response))
        }
        "host.model" => {
            let resource_index = input
                .get("resourceIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let binding = resources
                .iter()
                .filter(|binding| binding.resource_kind == RuntimeResourceKindV1::Model)
                .nth(resource_index)
                .ok_or_else(|| (-32602, "Model resource is not bound".into()))?;
            let RuntimeResourceConfigurationV1::Model {
                endpoint,
                model,
                credential,
                ..
            } = &binding.configuration
            else {
                return Err((-32602, "Model resource configuration is invalid".into()));
            };
            let endpoint = crate::worker_support::openai_chat_completions_endpoint(endpoint);
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/json"),
            );
            if let Some(secret) = credential {
                let vault = worker
                    .vault
                    .as_ref()
                    .ok_or_else(|| (-32020, "Runtime Vault is unavailable".into()))?;
                let value = vault
                    .read(secret)
                    .await
                    .map_err(|error| (-32020, error.to_string()))?;
                let value = crate::worker_support::provider_secret_header(&value, "authorization");
                headers.insert(
                    reqwest::header::AUTHORIZATION,
                    reqwest::header::HeaderValue::from_bytes(&value)
                        .map_err(|error| (-32602, error.to_string()))?,
                );
            }
            let prompt = input
                .get("prompt")
                .or_else(|| input.get("userQuestion"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let body = json!({"model":model,"messages":[{"role":"user","content":prompt}]});
            let response = worker
                .provider
                .post_json(
                    &endpoint,
                    crate::egress::EgressRequestContext::request(tenant_id, operation_id),
                    Duration::from_secs(8),
                    headers,
                    &body,
                )
                .await
                .map_err(design_provider_error)?;
            Ok(design_http_response(response)["body"].clone())
        }
        "host.artifacts.read" | "host.artifacts.put" => {
            files
                .handle(
                    worker,
                    tenant_id,
                    operation_id,
                    None,
                    method,
                    input,
                    call_index,
                )
                .await
        }
        _ => Err((-32601, format!("Host method '{method}' is not supported"))),
    }
}

fn credential_descriptors(
    resources: &[agentx_runtime_contracts::RuntimeResourceBindingV1],
) -> Value {
    Value::Array(
        resources
            .iter()
            .filter_map(|binding| {
                let RuntimeResourceConfigurationV1::Credential {
                    credential_type,
                    allowed_operations,
                    ..
                } = &binding.configuration
                else {
                    return None;
                };
                Some((binding, credential_type, allowed_operations))
            })
            .enumerate()
            .map(|(index, (binding, credential_type, allowed_operations))| {
                json!({
                    "index":index,
                    "resourceId":binding.resource_id,
                    "resourceVersion":binding.resource_version,
                    "credentialType":credential_type,
                    "allowedOperations":allowed_operations,
                })
            })
            .collect(),
    )
}

fn request_headers(request: &Value) -> Result<reqwest::header::HeaderMap, (i64, String)> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in request
        .get("headers")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| (-32602, error.to_string()))?;
        let value = value
            .as_str()
            .ok_or_else(|| (-32602, "HTTP header value must be a string".into()))?;
        headers.insert(
            name,
            reqwest::header::HeaderValue::from_str(value)
                .map_err(|error| (-32602, error.to_string()))?,
        );
    }
    Ok(headers)
}

async fn apply_design_credential(
    worker: &RuntimeWorker,
    binding: &agentx_runtime_contracts::RuntimeResourceBindingV1,
    request: &Value,
    endpoint: &mut String,
    headers: &mut reqwest::header::HeaderMap,
) -> Result<(), (i64, String)> {
    let RuntimeResourceConfigurationV1::Credential {
        credential_type,
        secret,
        ..
    } = &binding.configuration
    else {
        return Err((
            -32602,
            "Credential resource configuration is invalid".into(),
        ));
    };
    let vault = worker
        .vault
        .as_ref()
        .ok_or_else(|| (-32020, "Runtime Vault is unavailable".into()))?;
    let value = vault
        .read(secret)
        .await
        .map_err(|error| (-32020, error.to_string()))?;
    super::apply_http_credential(
        endpoint,
        headers,
        credential_type,
        &value,
        request.get("apiKeyPlacement"),
    )
    .map_err(|message| (-32602, message))
}

fn design_provider_error(error: super::WorkerProviderError) -> (i64, String) {
    match error {
        super::WorkerProviderError::Protocol(message) => {
            (0, format!("model stream protocol error: {message}"))
        }
        super::WorkerProviderError::Denied(message) => (-32020, message),
        super::WorkerProviderError::Request { message, .. } => (-32020, message),
    }
}

fn design_http_response(response: super::WorkerProviderResponse) -> Value {
    let headers = response
        .headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), Value::String(value.to_owned())))
        })
        .collect::<serde_json::Map<_, _>>();
    let body = serde_json::from_slice(&response.body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&response.body).into_owned()));
    json!({"status":response.status.as_u16(),"headers":headers,"body":body,"files":[]})
}

pub(super) async fn execute(
    worker: &RuntimeWorker,
    claim: &ClaimedWorkerAttempt,
) -> WorkerExecution {
    let Some(plugin) = &claim.plugin else {
        return WorkerExecution::failed(
            "PLUGIN_BINDING_MISSING",
            "Execution snapshot has no plugin binding",
            false,
        );
    };
    let wait_ms = (claim.task.deadline_at - time::OffsetDateTime::now_utc())
        .whole_milliseconds()
        .max(1) as u64;
    let runtime_source = match worker.plugin_artifacts.load(worker, claim, plugin).await {
        Ok(source) => source,
        Err(execution) => return execution,
    };
    let node = std::env::var("AGENTX_NODE_EXECUTABLE").unwrap_or_else(|_| "node".into());
    let runner = std::env::var("AGENTX_PLUGIN_RUNNER_PATH").unwrap_or_else(|_| {
        let local = Path::new("src/plugins/packages/plugin-runner/runner.mjs");
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/packages/plugin-runner/runner.mjs");
        if local.is_file() {
            local.to_string_lossy().into_owned()
        } else if workspace.is_file() {
            workspace.to_string_lossy().into_owned()
        } else {
            "/opt/agentx/plugin-runner/runner.mjs".into()
        }
    });
    let capacity = match timeout(
        Duration::from_millis(wait_ms),
        worker.plugin_slots.clone().acquire_owned(),
    )
    .await
    {
        Ok(Ok(capacity)) => capacity,
        _ => {
            return WorkerExecution::failed(
                "PLUGIN_TIMED_OUT",
                "Plugin expired while waiting for process capacity",
                false,
            );
        }
    };
    let mut files = match files::InvocationFiles::new() {
        Ok(files) => files,
        Err(error) => {
            return WorkerExecution::failed("PLUGIN_WORKSPACE_FAILED", error.to_string(), false);
        }
    };
    let mut process = match spawn_plugin_process(&node, &runner, files.path(), capacity).await {
        Ok(process) => process,
        Err(error) => {
            return WorkerExecution::failed(
                "PLUGIN_PROCESS_START_FAILED",
                error.to_string(),
                false,
            );
        }
    };
    files.observe(
        &json!({"inputs":claim.inputs,"parameters":claim.node_parameters,
        "perItemParameters":claim.per_item_parameters,"context":claim.context}),
    );
    let execution = json!({
        "nodeType":claim.node_type,
        "nodeVersion":claim.node_version,
        "packageId":plugin.package_id,
        "packageVersion":plugin.package_version,
        "bundleDigest":plugin.bundle_digest,
        "executionId":claim.task.execution_id,
        "nodeExecutionId":claim.task.node_execution_id,
        "attemptId":claim.task.attempt_id,
        "runIndex":claim.run_index,
        "iterationIndex":claim.iteration_index,
        "idempotencyKey":format!("{}:{}",claim.task.attempt_id,claim.lease.fencing_token),
        "deadline":claim.task.deadline_at.format(&time::format_description::well_known::Rfc3339).expect("OffsetDateTime formats as RFC3339"),
    });
    let request = json!({"jsonrpc":"2.0","id":claim.task.attempt_id.to_string(),"method":"node.execute","params":{
        "invocationId":claim.task.attempt_id,
        "runtimeSource":runtime_source,
        "runtimeEntry":plugin.runtime_entry,
        "inputs":claim.inputs,
        "parameters":claim.node_parameters,
        "perItemParameters":claim.per_item_parameters,
        "stringConversions":claim.string_conversions,
        "context":claim.context,
        "execution":execution,
    }});
    if let Err(error) = process
        .stdin
        .write_all(format!("{}\n", request).as_bytes())
        .await
    {
        terminate_process_tree(&process.process_tree, &mut process.child).await;
        return WorkerExecution::failed("PLUGIN_PROTOCOL_ERROR", error.to_string(), false);
    }
    let trace_claim = std::sync::Arc::new(claim.clone());
    let wait = async {
        let mut host_call_index = 0_u32;
        let mut dropped_trace = 0_u64;
        loop {
            let line = process.lines.next_line().await?.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Runner exited without a response",
                )
            })?;
            if line.len() > 8 * 1024 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Runner message exceeds 8 MiB",
                ));
            }
            let message: Value = serde_json::from_str(&line)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            if message.get("method").and_then(Value::as_str) == Some("trace.event") {
                if let Some(params) = message.get("params")
                    && let Ok(event) = serde_json::from_value::<LiveTraceEvent>(params.clone())
                    && event.invocation_id == claim.task.attempt_id.to_string()
                    && !worker.plugin_trace.event(trace_claim.clone(), event)
                {
                    dropped_trace += 1;
                }
                continue;
            }
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                if !id
                    .as_str()
                    .is_some_and(|id| id.starts_with(&format!("host:{}:", claim.task.attempt_id)))
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Host request belongs to another invocation",
                    ));
                }
                host_call_index = host_call_index.saturating_add(1);
                let response = match handle_host_call(
                    worker,
                    claim,
                    method,
                    message.get("params").cloned().unwrap_or(Value::Null),
                    host_call_index,
                    &mut files,
                )
                .await
                {
                    Ok(result) => {
                        files.observe(&result);
                        json!({"jsonrpc":"2.0","id":id,"result":result})
                    }
                    Err((code, message)) => {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
                    }
                };
                process
                    .stdin
                    .write_all(format!("{response}\n").as_bytes())
                    .await?;
                continue;
            }
            let response = serde_json::from_value::<RpcResponse>(message)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            return Ok::<_, std::io::Error>((response, dropped_trace));
        }
    };
    let response = match timeout(Duration::from_millis(claim.timeout_ms.max(1)), wait).await {
        Err(_) => {
            let cancel = json!({"jsonrpc":"2.0","id":format!("cancel:{}",claim.task.attempt_id),"method":"invocation.cancel","params":{"invocationId":claim.task.attempt_id}});
            let _ = process
                .stdin
                .write_all(format!("{cancel}\n").as_bytes())
                .await;
            terminate_process_tree(&process.process_tree, &mut process.child).await;
            return WorkerExecution::failed(
                "PLUGIN_TIMED_OUT",
                "Plugin execution exceeded its deadline",
                false,
            );
        }
        Ok(Err(error)) => {
            terminate_process_tree(&process.process_tree, &mut process.child).await;
            return WorkerExecution::failed("PLUGIN_PROTOCOL_ERROR", error.to_string(), false);
        }
        Ok(Ok(response)) => response,
    };
    let (response, dropped_trace) = response;
    if response.id != Value::String(claim.task.attempt_id.to_string()) {
        terminate_process_tree(&process.process_tree, &mut process.child).await;
        return WorkerExecution::failed(
            "PLUGIN_PROTOCOL_ERROR",
            "Runner response ID does not match the Attempt",
            false,
        );
    }
    terminate_process_tree(&process.process_tree, &mut process.child).await;
    drop(process);
    if dropped_trace > 0 {
        worker
            .plugin_trace
            .diagnostics(trace_claim.clone(), dropped_trace);
    }
    if let Some(error) = response.error {
        return WorkerExecution::failed(
            if error.code == -32021 {
                "PLUGIN_HOST_OUTCOME_UNKNOWN"
            } else {
                "PLUGIN_EXECUTION_ERROR"
            },
            format!("{} ({})", error.message, error.code),
            error.code == -32021,
        );
    }
    match response.result {
        Some(PluginResult::Completed {
            outputs,
            diagnostics,
        }) => {
            worker.plugin_trace.diagnostics(
                trace_claim.clone(),
                diagnostics
                    .get("dropped")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            );
            WorkerExecution {
                status: WorkerResultStatusV1::Succeeded,
                outputs,
                error_code: None,
                error_message: None,
                retryable: None,
            }
        }
        Some(PluginResult::Failed {
            code,
            message,
            retryable,
            details,
            diagnostics,
        }) => {
            worker.plugin_trace.diagnostics(
                trace_claim.clone(),
                diagnostics
                    .get("dropped")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            );
            let message = if details.is_null() {
                message
            } else {
                format!("{message}; details={details}")
            };
            let mut execution = WorkerExecution::failed(&code, message, false);
            execution.retryable = Some(retryable);
            execution
        }
        None => WorkerExecution::failed(
            "PLUGIN_PROTOCOL_ERROR",
            "Runner response has no result",
            false,
        ),
    }
}

async fn emit_trace_diagnostics(
    pool: &sqlx::MySqlPool,
    claim: &ClaimedWorkerAttempt,
    dropped: u64,
) {
    if dropped == 0 {
        return;
    }
    let _ = timeout(Duration::from_millis(500), async {
        let Ok(mut tx) = pool.begin().await else { return; };
        let mut draft = crate::trace_delivery::TraceDraft::span(
            claim.task.tenant_id, claim.task.execution_id, claim.task.attempt_id,
            Some((claim.task.node_execution_id, TraceSpanKindV1::Node)), TraceSpanKindV1::Attempt,
            "Plugin diagnostics", TraceEventKindV1::Updated, "plugin.trace.incomplete", "running",
        );
        draft.node_execution_id = Some(claim.task.node_execution_id);
        draft.attempt_id = Some(claim.task.attempt_id);
        draft.attributes = json!({"diagnosticWarning":"Plugin diagnostics were dropped", "traceIncomplete":true,"droppedDiagnostics":dropped});
        crate::trace_delivery::enqueue_best_effort(&mut tx, draft).await;
        let _ = tx.commit().await;
    }).await;
}

#[allow(clippy::too_many_arguments)]
async fn handle_host_call(
    worker: &RuntimeWorker,
    claim: &ClaimedWorkerAttempt,
    method: &str,
    params: Value,
    call_index: u32,
    files: &mut files::InvocationFiles,
) -> Result<Value, (i64, String)> {
    let input = params.get("input").cloned().unwrap_or(Value::Null);
    let mut nested = claim.clone();
    nested.trace_parent_span_entity_id = params
        .get("parentIndex")
        .and_then(Value::as_u64)
        .map(|index| trace_entity(claim.task.attempt_id, index as usize));
    match method {
        "host.http" => {
            let mut normalized = input;
            if let Some(headers) = normalized
                .get("headers")
                .and_then(Value::as_object)
                .cloned()
            {
                normalized["headers"] = Value::Array(
                    headers
                        .into_iter()
                        .map(|(name, value)| json!({"name":name,"value":value}))
                        .collect(),
                );
            }
            let (endpoint, request) = super::declarative_http_request(&normalized, None)
                .map_err(|message| (-32602, message))?;
            let credential_index = normalized
                .get("credentialIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let credential = claim
                .resources
                .iter()
                .filter(|binding| binding.resource_kind == RuntimeResourceKindV1::Credential)
                .nth(credential_index);
            let method_name = normalized
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .to_ascii_uppercase();
            let idempotency_key = normalized.get("idempotencyKey").and_then(Value::as_str);
            if !matches!(method_name.as_str(), "GET" | "HEAD" | "OPTIONS" | "DELETE")
                && idempotency_key.is_none()
            {
                return Err((
                    -32602,
                    "Mutating plugin HTTP calls require idempotencyKey".into(),
                ));
            }
            let execution = if let Some(key) = idempotency_key {
                worker
                    .call_http_effect(
                        &nested,
                        "http",
                        &endpoint,
                        request,
                        call_index,
                        key,
                        None,
                        "authorization",
                        credential,
                    )
                    .await
            } else {
                worker
                    .call_http(
                        &nested,
                        "http",
                        &endpoint,
                        request,
                        call_index,
                        None,
                        "authorization",
                        credential,
                    )
                    .await
            };
            host_execution_value(execution)
        }
        "host.model" => {
            let resource_index = input
                .get("resourceIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let binding = claim
                .resources
                .iter()
                .filter(|binding| binding.resource_kind == RuntimeResourceKindV1::Model)
                .nth(resource_index)
                .ok_or_else(|| (-32602, "Model resource is not bound".into()))?;
            nested.node_type = "model".into();
            nested.task.capability = NodeCapability::Model;
            nested.node_parameters = input;
            let execution = worker
                .execute_provider_call(&nested, binding, Value::Null, call_index)
                .await;
            host_execution_value(execution)
        }
        "host.credentials.list" => Ok(Value::Array(
            claim
                .resources
                .iter()
                .filter_map(|binding| {
                    let RuntimeResourceConfigurationV1::Credential {
                        credential_type,
                        allowed_operations,
                        ..
                    } = &binding.configuration
                    else {
                        return None;
                    };
                    Some(json!({
                        "index":0,
                        "resourceId":binding.resource_id,
                        "resourceVersion":binding.resource_version,
                        "credentialType":credential_type,
                        "allowedOperations":allowed_operations,
                    }))
                })
                .enumerate()
                .map(|(index, mut value)| {
                    value["index"] = Value::from(index);
                    value
                })
                .collect(),
        )),
        "host.artifacts.read" | "host.artifacts.put" => {
            files
                .handle(
                    worker,
                    claim.task.tenant_id,
                    claim.task.attempt_id,
                    Some(claim),
                    method,
                    input,
                    call_index,
                )
                .await
        }
        _ => Err((-32601, format!("Host method '{method}' is not supported"))),
    }
}

fn host_execution_value(execution: WorkerExecution) -> Result<Value, (i64, String)> {
    if execution.status != WorkerResultStatusV1::Succeeded {
        return Err((
            if execution.status == WorkerResultStatusV1::OutcomeUnknown {
                -32021
            } else {
                -32020
            },
            format!(
                "{}: {}",
                execution
                    .error_code
                    .as_deref()
                    .unwrap_or("HOST_CALL_FAILED"),
                execution
                    .error_message
                    .as_deref()
                    .unwrap_or("Host call failed")
            ),
        ));
    }
    Ok(execution
        .outputs
        .get("main")
        .and_then(|items| items.first())
        .map(|item| item.json.clone())
        .unwrap_or(Value::Null))
}

async fn terminate_process_tree(
    process_tree: &ProcessTreeGuard,
    child: &mut tokio::process::Child,
) {
    process_tree.terminate();
    let _ = child.kill().await;
    let _ = timeout(Duration::from_secs(2), child.wait()).await;
}

struct ProcessTreeGuard {
    #[cfg(unix)]
    process_group: i32,
    #[cfg(windows)]
    job: usize,
}

impl ProcessTreeGuard {
    fn attach(child: &tokio::process::Child) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            let pid = child
                .id()
                .ok_or_else(|| std::io::Error::other("child PID is unavailable"))?;
            Ok(Self {
                process_group: pid as i32,
            })
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::CloseHandle,
                System::JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                    SetInformationJobObject,
                },
            };
            let process = child
                .raw_handle()
                .ok_or_else(|| std::io::Error::other("child process handle is unavailable"))?;
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut limits = unsafe { std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            };
            let assigned =
                configured != 0 && unsafe { AssignProcessToJobObject(job, process.cast()) } != 0;
            if !assigned {
                let error = std::io::Error::last_os_error();
                unsafe { CloseHandle(job) };
                return Err(error);
            }
            Ok(Self { job: job as usize })
        }
    }

    fn terminate(&self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.process_group, libc::SIGKILL);
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(
                self.job as windows_sys::Win32::Foundation::HANDLE,
                1,
            );
        }
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        self.terminate();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(
                self.job as windows_sys::Win32::Foundation::HANDLE,
            );
        }
    }
}

fn trace_entity(attempt_id: uuid::Uuid, index: usize) -> uuid::Uuid {
    deterministic_uuid(
        attempt_id,
        format!("agentx-plugin-operation-v1:{index}").as_bytes(),
    )
}

fn trace_parent(
    claim: &ClaimedWorkerAttempt,
    parent_index: Option<usize>,
) -> (uuid::Uuid, TraceSpanKindV1) {
    parent_index.map_or((claim.task.attempt_id, TraceSpanKindV1::Attempt), |index| {
        (
            trace_entity(claim.task.attempt_id, index),
            TraceSpanKindV1::PluginOperation,
        )
    })
}

fn valid_plugin_content(binding: &PluginNodeBinding, content: &PluginContent) -> bool {
    binding.trace_renderers.iter().any(|renderer| {
        renderer.content_type == content.content_type
            && renderer.content_version == content.version
            && jsonschema::validator_for(&renderer.schema)
                .is_ok_and(|validator| validator.is_valid(&content.data))
    })
}

async fn emit_live_trace_event(
    pool: &sqlx::MySqlPool,
    objects: &std::sync::Arc<dyn object_store::ObjectStore>,
    claim: &ClaimedWorkerAttempt,
    event: &LiveTraceEvent,
) -> bool {
    if event.index >= 64 {
        return false;
    }
    let Some(binding) = &claim.plugin else {
        return false;
    };
    let entity_id = trace_entity(claim.task.attempt_id, event.index);
    let content_envelope = event.content.as_ref().and_then(|content| {
        valid_plugin_content(binding, content).then(|| {
            json!({
                "packageId":binding.package_id,
                "packageVersion":binding.package_version,
                "bundleDigest":binding.bundle_digest,
                "nodeType":claim.node_type,
                "typeVersion":claim.node_version,
                "contentType":content.content_type,
                "contentVersion":content.version,
                "label":content.label,
                "data":content.data,
            })
        })
    });
    let content_ref = if let Some(value) = content_envelope.as_ref() {
        timeout(
            Duration::from_secs(2),
            crate::trace_artifact::externalize_plugin_content(
                pool, objects, claim, entity_id, value,
            ),
        )
        .await
        .ok()
        .flatten()
    } else {
        None
    };
    let Ok(Ok(mut tx)) = timeout(Duration::from_millis(250), pool.begin()).await else {
        return false;
    };
    let (event_kind, event_type, status) = match event.phase.as_str() {
        "started" => (
            TraceEventKindV1::Started,
            "plugin.operation.started",
            "running",
        ),
        "finished" => (
            TraceEventKindV1::Finished,
            "plugin.operation.finished",
            if event.span.status == "failed" {
                "failed"
            } else {
                "succeeded"
            },
        ),
        "content" => (TraceEventKindV1::Updated, "plugin.content", "running"),
        "event" => (TraceEventKindV1::Updated, "plugin.event", "running"),
        _ => return false,
    };
    let mut draft = crate::trace_delivery::TraceDraft::span(
        claim.task.tenant_id,
        claim.task.execution_id,
        entity_id,
        Some(trace_parent(claim, event.parent_index)),
        TraceSpanKindV1::PluginOperation,
        event.span.name.clone(),
        event_kind,
        event_type,
        status,
    );
    draft.node_execution_id = Some(claim.task.node_execution_id);
    draft.attempt_id = Some(claim.task.attempt_id);
    draft.attributes = json!({
        "packageId":binding.package_id,
        "packageVersion":binding.package_version,
        "bundleDigest":binding.bundle_digest,
        "nodeType":claim.node_type,
        "meteringSource":"plugin_diagnostic",
        "pluginAttributes":event.span.attributes,
        "pluginEvent":event.event,
    });
    if let Some(content) = &event.content {
        if valid_plugin_content(binding, content) {
            draft.content_kind = Some(TraceContentKindV1::PluginContent);
            draft.content_preview = content_envelope
                .as_ref()
                .and_then(crate::trace_delivery::bounded_preview);
            draft.content_ref = content_ref;
        } else {
            draft.event_type = "plugin.content.invalid".into();
            draft.attributes["diagnosticWarning"] =
                Value::String("Plugin content does not match its declared renderer Schema".into());
        }
    }
    crate::trace_delivery::enqueue_best_effort(&mut tx, draft).await;
    if let Err(error) = tx.commit().await {
        tracing::warn!(%error, "Live plugin Trace commit failed");
        return false;
    }
    true
}

#[cfg(test)]
#[path = "worker_runtime_plugin_tests.rs"]
mod tests;
