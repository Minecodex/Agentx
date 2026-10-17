use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub async fn with_operation_deadline<F>(
    deadline_at: time::OffsetDateTime,
    future: F,
) -> Result<F::Output, tokio::time::error::Elapsed>
where
    F: std::future::Future,
{
    let remaining_ms = (deadline_at - time::OffsetDateTime::now_utc())
        .whole_milliseconds()
        .max(0);
    tokio::time::timeout(
        std::time::Duration::from_millis(u64::try_from(remaining_ms).unwrap_or(u64::MAX)),
        future,
    )
    .await
}

pub(crate) fn runtime_call_span_name(kind: &str) -> String {
    match kind {
        "model" => "Model call",
        "compaction" => "Compaction model call",
        "mcp_tool" => "MCP tool call",
        "rag" => "RAG query",
        "memory" => "Memory operation",
        "sandbox" => "Sandbox call",
        value => value,
    }
    .into()
}

pub(crate) fn openai_chat_completions_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint.ends_with("/chat/completions") {
        endpoint.to_owned()
    } else {
        format!("{endpoint}/chat/completions")
    }
}

pub(crate) fn provider_secret_header(value: &[u8], header: &str) -> Vec<u8> {
    if header != "authorization" || value.starts_with(b"Bearer ") || value.starts_with(b"Basic ") {
        return value.to_vec();
    }
    let mut header_value = b"Bearer ".to_vec();
    header_value.extend_from_slice(value);
    header_value
}

pub(crate) fn memory_secret_header(
    resources: &[agentx_runtime_contracts::RuntimeResourceBindingV1],
    credential: Option<&agentx_runtime_contracts::VaultSecretReferenceV1>,
) -> Result<&'static str, &'static str> {
    let Some(reference) = credential else {
        return Ok("authorization");
    };
    let credential_type = resources.iter().find_map(|binding| {
        if let agentx_runtime_contracts::RuntimeResourceConfigurationV1::Credential {
            credential_type,
            secret,
            ..
        } = &binding.configuration
            && secret.mount == reference.mount
            && secret.path == reference.path
            && secret.key == reference.key
            && secret.version == reference.version
        {
            Some(credential_type.as_str())
        } else {
            None
        }
    });
    match credential_type {
        Some("api_key") => Ok("x-api-key"),
        Some("bearer") => Ok("authorization"),
        Some(_) => Err("MEMORY_CREDENTIAL_TYPE_UNSUPPORTED"),
        None => Err("MEMORY_CREDENTIAL_BINDING_MISSING"),
    }
}

pub(crate) fn agent_turn_entity_id(run_id: Uuid, turn: u32) -> Uuid {
    stable_id(run_id, format!("turn-{turn}").as_bytes())
}

pub(crate) fn stable_id(namespace: Uuid, label: &[u8]) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update(label);
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

pub(crate) fn runtime_call_fingerprint(kind: &str, request: &Value) -> String {
    if kind != "sandbox" {
        return raw_hash(request);
    }
    let mut stable_request = request.clone();
    if let Some(object) = stable_request.as_object_mut() {
        // Lease proof changes after Worker takeover, but is not part of the
        // immutable provider operation or its replay identity.
        object.remove("workerId");
        object.remove("fencingToken");
    }
    raw_hash(&stable_request)
}

pub(crate) fn raw_hash(value: &Value) -> String {
    let bytes = agentx_runtime_contracts::canonical_bytes(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}
