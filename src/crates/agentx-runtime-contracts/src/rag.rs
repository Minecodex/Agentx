//! Shared RAG provider protocol (plan7 P7-E): request construction and
//! response normalization for LightRAG and RAGFlow, used by the runtime
//! worker, Agent attachment tools and Control retrieval tests
//! so the protocol cannot drift between surfaces.

use serde_json::{Value, json};

pub const RAG_PROVIDER_LIGHT_RAG: &str = "lightrag";
pub const RAG_PROVIDER_RAGFLOW: &str = "ragflow";

/// A query freezes an external content scope, never an Agentx index.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RagScope {
    Ragflow {
        #[serde(rename = "datasetIds")]
        dataset_ids: Vec<String>,
    },
    Lightrag {
        workspace: String,
    },
}

impl RagScope {
    pub fn provider(&self) -> &'static str {
        match self {
            Self::Ragflow { .. } => RAG_PROVIDER_RAGFLOW,
            Self::Lightrag { .. } => RAG_PROVIDER_LIGHT_RAG,
        }
    }

    pub fn validate(&self, provider: &str) -> Result<(), RagProtocolError> {
        if self.provider() != provider {
            return Err(RagProtocolError::new(
                "RAG_SCOPE_INVALID",
                "Scope does not match the connection provider",
            ));
        }
        if let Self::Ragflow { dataset_ids } = self {
            if dataset_ids.is_empty()
                || dataset_ids
                    .iter()
                    .any(|id| id.trim().is_empty() || id.len() > 512)
                || dataset_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != dataset_ids.len()
            {
                return Err(RagProtocolError::new(
                    "RAG_SCOPE_INVALID",
                    "Select distinct, nonempty dataset IDs",
                ));
            }
        }
        // Empty is the real default workspace. Discovery, not a fabricated
        // naming rule or request header, establishes which instance owns it.
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RagProtocolError {
    pub code: &'static str,
    pub message: String,
}
impl RagProtocolError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// One physical source per request: RAGFlow datasets may use incompatible
/// embedding models. LightRAG's workspace is fixed by the server instance.
pub fn rag_query_request(
    scope: &RagScope,
    query: &str,
) -> Result<(String, Value, &'static str), RagProtocolError> {
    scope.validate(scope.provider())?;
    match scope {
        RagScope::Ragflow { dataset_ids } if dataset_ids.len() == 1 => Ok((
            "api/v1/retrieval".into(),
            json!({"question":query,"dataset_ids":dataset_ids,"top_k":1024,"page":1,"page_size":20}),
            "authorization",
        )),
        RagScope::Ragflow { .. } => Err(RagProtocolError::new(
            "RAG_SCOPE_INVALID",
            "Resolve each dataset before retrieval",
        )),
        RagScope::Lightrag { .. } => Ok((
            "query/data".into(),
            json!({"query":query,"mode":"naive","top_k":20,"chunk_top_k":20}),
            "x-api-key",
        )),
    }
}

pub fn validate_retrieval_input(query: &str, top_k: u64) -> Result<(), RagProtocolError> {
    if query.trim().is_empty() || query.chars().count() > 2000 || !(1..=20).contains(&top_k) {
        return Err(RagProtocolError::new(
            "INVALID_RETRIEVAL_QUERY",
            "Enter a query up to 2000 characters and Top K between 1 and 20",
        ));
    }
    Ok(())
}

/// Verify the instance identity before using its fixed-workspace endpoints.
pub fn validate_instance_scope(scope: &RagScope, health: &Value) -> Result<(), RagProtocolError> {
    if let RagScope::Lightrag { workspace } = scope {
        if health
            .pointer("/configuration/workspace")
            .and_then(Value::as_str)
            != Some(workspace)
        {
            return Err(RagProtocolError::new(
                "RAG_WORKSPACE_CHANGED",
                "The instance no longer exposes the bound workspace",
            ));
        }
    }
    Ok(())
}

fn json_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

/// Structured retrieval does not generate an answer or invent hit scores.
pub fn finalize_retrieval_value(provider: &str, value: Value) -> Result<Value, RagProtocolError> {
    if provider == RAG_PROVIDER_RAGFLOW {
        if value.get("code").and_then(Value::as_i64) != Some(0) {
            return Err(RagProtocolError::new(
                "PROVIDER_REJECTED",
                "RAGFlow rejected retrieval",
            ));
        }
        let has_chunks = value.pointer("/data/chunks").is_some_and(Value::is_array);
        let normalized = finalize_rag_value(provider, value)?;
        if !has_chunks {
            return Err(RagProtocolError::new(
                "PROVIDER_REJECTED",
                "RAGFlow retrieval omitted chunks",
            ));
        }
        return Ok(normalized);
    }
    // LightRAG reports a valid search with zero hits as HTTP 200 / failure,
    // distinguished from provider failures by this machine-readable reason.
    if value["status"] == "failure"
        && value
            .pointer("/metadata/failure_reason")
            .and_then(Value::as_str)
            == Some("no_results")
    {
        return Ok(json!({"text":"","documents":[],"citations":[],"recordIds":[]}));
    }
    if value["status"] != "success" {
        return Err(RagProtocolError::new(
            "PROVIDER_REJECTED",
            "LightRAG retrieval failed",
        ));
    }
    let chunks = value
        .pointer("/data/chunks")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RagProtocolError::new("PROVIDER_REJECTED", "LightRAG retrieval omitted chunks")
        })?;
    let documents = chunks.iter().map(|chunk| json!({"documentId":chunk.get("doc_id").or_else(||chunk.get("file_path")),"chunkId":chunk.get("chunk_id"),"content":chunk.get("content"),"score":chunk.get("score"),"title":chunk.get("title").or_else(||chunk.get("file_path")),"uri":chunk.get("uri"),"metadata":chunk.get("metadata"),"files":chunk.get("files").or_else(||chunk.get("attachments"))})).collect::<Vec<_>>();
    Ok(
        json!({"text":"","documents":documents,"citations":value.pointer("/data/references"),"recordIds":chunks.iter().filter_map(|chunk|chunk["chunk_id"].as_str()).collect::<Vec<_>>() }),
    )
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
    fn scope_is_structured_and_lightrag_never_claims_to_route_workspaces() {
        let scope = RagScope::Lightrag {
            workspace: String::new(),
        };
        let (path, body, header) = rag_query_request(&scope, "hello").unwrap();
        assert_eq!(path, "query/data");
        assert_eq!(header, "x-api-key");
        assert!(body.get("workspace").is_none());
        assert!(body.get("indexVersion").is_none());
        assert!(
            serde_json::from_value::<RagScope>(
                json!({"kind":"lightrag","workspace":"","namespace":"fake"})
            )
            .is_err()
        );
        let scope = RagScope::Ragflow {
            dataset_ids: vec!["a".into(), "b".into()],
        };
        assert!(rag_query_request(&scope, "hello").is_err());
        let (_, body, _) = rag_query_request(
            &RagScope::Ragflow {
                dataset_ids: vec!["a".into()],
            },
            "hello",
        )
        .unwrap();
        assert_eq!(body["page_size"], 20);
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

    #[test]
    fn retrieval_no_hits_succeeds_for_real_provider_envelopes() {
        for (provider, response) in [
            (
                "lightrag",
                json!({"status":"failure","data":{},"message":"No relevant document chunks found.","metadata":{"mode":"naive","failure_reason":"no_results"}}),
            ),
            (
                "lightrag",
                json!({"status":"success","data":{"chunks":[],"references":[]}}),
            ),
            ("ragflow", json!({"code":0,"data":{"chunks":[]}})),
        ] {
            let result = finalize_retrieval_value(provider, response).unwrap();
            assert_eq!(result["documents"], json!([]));
            assert_eq!(result["recordIds"], json!([]));
        }
    }

    #[test]
    fn retrieval_does_not_hide_provider_or_protocol_failures() {
        for (provider, response) in [
            (
                "lightrag",
                json!({"status":"failure","metadata":{"failure_reason":"embedding_failed"}}),
            ),
            (
                "lightrag",
                json!({"status":"failure","message":"No relevant document chunks found."}),
            ),
            ("lightrag", json!({"status":"success","data":{}})),
            ("ragflow", json!({"code":102,"message":"no such dataset"})),
            ("ragflow", json!({"code":0,"data":{}})),
        ] {
            assert!(finalize_retrieval_value(provider, response).is_err());
        }
    }
}
