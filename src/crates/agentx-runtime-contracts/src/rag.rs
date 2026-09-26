//! Shared RAG provider protocol (plan7 P7-E): request construction and
//! response normalization for LightRAG and RAGFlow, used by the runtime
//! worker, the agent attachment slots and the control-plane document indexer
//! so the protocol cannot drift between surfaces.

use serde_json::{Value, json};

pub const RAG_PROVIDER_LIGHT_RAG: &str = "lightrag";
pub const RAG_PROVIDER_RAGFLOW: &str = "ragflow";

/// Pure protocol failure; surfaces carry it in their own error shapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RagProtocolError {
    pub code: &'static str,
    pub message: String,
}

impl RagProtocolError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn rag_query_text(payload: &Value) -> String {
    payload
        .get("query")
        .or_else(|| payload.get("question"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| payload.to_string())
}

fn rag_top_k(payload: &Value) -> u64 {
    payload
        .get("topK")
        .or_else(|| payload.get("top_k"))
        .and_then(Value::as_u64)
        .unwrap_or(5)
}

fn json_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

/// Builds the provider-specific HTTP request. Returns the URL path, the JSON
/// body and the header name carrying the secret. Unknown operations are
/// rejected explicitly: LightRAG historically folded them into `/query`,
/// which silently mis-signals delete-style intents (plan7 05 §3.2).
pub fn rag_query_request(
    provider: &str,
    operation: &str,
    namespace: &str,
    index_version: &str,
    input: &Value,
) -> Result<(String, Value, &'static str), RagProtocolError> {
    if provider == RAG_PROVIDER_RAGFLOW {
        if operation != "query" {
            return Err(RagProtocolError::new(
                "RAG_OPERATION_UNSUPPORTED",
                "RAGFlow knowledge connections support query only",
            ));
        }
        let dataset_ids: Vec<String> = namespace
            .split(',')
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
            .map(str::to_owned)
            .collect();
        if dataset_ids.is_empty() {
            return Err(RagProtocolError::new(
                "RAG_DATASET_REQUIRED",
                "RAGFlow knowledge resource must reference at least one dataset id",
            ));
        }
        return Ok((
            "api/v1/retrieval".into(),
            json!({
                "question": rag_query_text(input),
                "dataset_ids": dataset_ids,
                "top_k": rag_top_k(input),
            }),
            "authorization",
        ));
    }
    if !matches!(operation, "query" | "insert") {
        return Err(RagProtocolError::new(
            "RAG_OPERATION_UNSUPPORTED",
            format!("LightRAG protocol does not support the {operation} operation here"),
        ));
    }
    let mut body = if input.is_object() {
        input.clone()
    } else {
        json!({"query": input, "mode": "naive"})
    };
    if let Some(object) = body.as_object_mut() {
        if let Some(top_k) = object.remove("topK") {
            object.insert("top_k".into(), top_k);
        }
        object
            .entry("workspace".to_owned())
            .or_insert_with(|| json!(namespace));
        object
            .entry("indexVersion".to_owned())
            .or_insert_with(|| json!(index_version));
    }
    let path = if operation == "insert" {
        "documents/text"
    } else {
        "query"
    };
    Ok((path.into(), body, "x-api-key"))
}

/// Normalizes a successful provider response value into the canonical rag
/// payload (`text`, `documents`, `citations`, `recordIds`). LightRAG values
/// pass through unchanged; RAGFlow `{code, data}` envelopes are mapped and
/// non-zero codes surface the provider message.
pub fn finalize_rag_value(provider: &str, value: Value) -> Result<Value, RagProtocolError> {
    if provider != RAG_PROVIDER_RAGFLOW {
        return Ok(value);
    }
    match value.get("code").and_then(Value::as_i64) {
        None | Some(0) => {}
        Some(_) => {
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("RAGFlow rejected the query");
            return Err(RagProtocolError::new("PROVIDER_REJECTED", message));
        }
    }
    let documents = value
        .pointer("/data/chunks")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let record_ids = value
        .pointer("/data/chunks")
        .and_then(Value::as_array)
        .map(|chunks| {
            Value::Array(
                chunks
                    .iter()
                    .filter_map(|chunk| chunk.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .map(Value::String)
                    .collect(),
            )
        })
        .unwrap_or_else(|| json!([]));
    Ok(json!({
        "text": json_text(&documents),
        "documents": documents,
        "citations": [],
        "recordIds": record_ids,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lightrag_requests_keep_the_workspace_contract() {
        let (path, body, header) = rag_query_request(
            RAG_PROVIDER_LIGHT_RAG,
            "query",
            "ns-1",
            "v3",
            &json!({"query": "hello"}),
        )
        .unwrap();
        assert_eq!(path, "query");
        assert_eq!(header, "x-api-key");
        assert_eq!(body["workspace"], json!("ns-1"));
        assert_eq!(body["indexVersion"], json!("v3"));
    }

    #[test]
    fn lightrag_delete_is_rejected_instead_of_becoming_a_query() {
        let error = rag_query_request(RAG_PROVIDER_LIGHT_RAG, "delete", "ns", "v1", &json!({}))
            .unwrap_err();
        assert_eq!(error.code, "RAG_OPERATION_UNSUPPORTED");
    }

    #[test]
    fn ragflow_error_envelopes_surface_the_provider_message() {
        let error = finalize_rag_value(
            RAG_PROVIDER_RAGFLOW,
            json!({"code": 100, "message": "no such dataset"}),
        )
        .unwrap_err();
        assert_eq!(error.code, "PROVIDER_REJECTED");
        assert_eq!(error.message, "no such dataset");
    }

    #[test]
    fn ragflow_chunks_map_to_documents_and_record_ids() {
        let normalized = finalize_rag_value(
            RAG_PROVIDER_RAGFLOW,
            json!({"code": 0, "data": {"chunks": [{"id": "c1", "content": "alpha"}]}}),
        )
        .unwrap();
        assert_eq!(normalized["documents"][0]["id"], json!("c1"));
        assert_eq!(normalized["recordIds"], json!(["c1"]));
    }
}
