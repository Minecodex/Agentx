use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    ClaimedWorkerAttempt, RuntimeResourceKindV1, RuntimeWorker, WorkerExecution, output,
    select_binding, successful_value,
};

impl RuntimeWorker {
    pub(super) async fn execute_knowledge_retrieval(
        &self,
        claim: &ClaimedWorkerAttempt,
    ) -> WorkerExecution {
        let Some(binding) = select_binding(claim, RuntimeResourceKindV1::Rag) else {
            return WorkerExecution::failed(
                "RUNTIME_RESOURCE_BINDING_MISSING",
                "Select a knowledge resource",
                false,
            );
        };
        let Some(query) = claim.node_parameters["query"]
            .as_str()
            .filter(|query| !query.trim().is_empty())
        else {
            return WorkerExecution::failed(
                "KNOWLEDGE_QUERY_REQUIRED",
                "Knowledge retrieval requires a nonempty query",
                false,
            );
        };
        let top_k = claim
            .node_parameters
            .get("topK")
            .map_or(Some(5), Value::as_u64);
        let Some(top_k @ 1..=20) = top_k else {
            return WorkerExecution::failed(
                "KNOWLEDGE_TOP_K_INVALID",
                "Top K must be between 1 and 20",
                false,
            );
        };
        let mut retrieval = claim.clone();
        retrieval.node_parameters =
            json!({"operation":"retrieve","input":{"query":query,"topK":top_k}});
        let execution = self
            .execute_provider_call(&retrieval, binding, Value::Null, 0)
            .await;
        let Some(value) = successful_value(&execution) else {
            return execution;
        };
        WorkerExecution::succeeded(retrieval_output(
            &value,
            binding.resource_id,
            top_k as usize,
        ))
    }
}

// Source references remain external metadata. They are never accepted as
// platform Artifacts, fetched, or given object-store download permissions.
fn retrieval_output(value: &Value, resource_id: Uuid, top_k: usize) -> Value {
    let mut files = Vec::new();
    let documents = value["documents"].as_array().into_iter().flatten().take(top_k).map(|source| {
        let metadata = source.get("metadata").filter(|value| value.is_object()).or_else(|| source.get("document_metadata").filter(|value| value.is_object())).cloned().unwrap_or_else(|| json!({}));
        let field = |names: &[&str]| string_field(source, &metadata, names);
        let document_id = field(&["documentId", "document_id", "doc_id", "document", "file_path"]);
        for attachment in source.get("files").or_else(|| source.get("attachments")).and_then(Value::as_array).into_iter().flatten().take(20) {
            let empty = Value::Null;
            let reference_id = string_field(attachment, &empty, &["referenceId", "id", "image_id"]);
            let uri = safe_uri(string_field(attachment, &empty, &["uri", "url"]));
            if reference_id.is_none() && uri.is_none() { continue; }
            let file = json!({"knowledgeResourceId":resource_id,"documentId":document_id,"referenceId":reference_id,"uri":uri,"name":string_field(attachment,&empty,&["name","fileName"]),"contentType":string_field(attachment,&empty,&["contentType","mime_type"])});
            if !files.contains(&file) { files.push(file); }
        }
        // RAGFlow retrieval carries an existing image reference on the chunk
        // itself. Keep the opaque ID; it does not authorize a download URL.
        if let Some(image_id) = source.get("image_id").and_then(Value::as_str).filter(|id| !id.trim().is_empty())
            && !files.iter().any(|file| file["referenceId"] == image_id) {
            files.push(json!({"knowledgeResourceId":resource_id,"documentId":document_id,"referenceId":image_id,"uri":null,"name":null,"contentType":null}));
        }
        json!({
            "knowledgeResourceId":resource_id,"documentId":document_id,
            "chunkId":field(&["chunkId","chunk_id","id"]),"title":field(&["title","document_name","document_keyword","docnm_kwd","file_name","name"]),
            "uri":safe_uri(field(&["uri","url","source"])),
            "content":field(&["content","content_with_weight","text","pageContent"]).unwrap_or_default(),
            "score":(["score","similarity"].iter().find_map(|key| source.get(key).and_then(Value::as_f64).or_else(||metadata.get(key).and_then(Value::as_f64)))),
            "metadata":metadata,
        })
    }).collect::<Vec<_>>();
    let text = documents
        .iter()
        .filter_map(|document| document["content"].as_str())
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    json!({"text":text,"documents":documents,"files":files})
}

fn string_field(source: &Value, metadata: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| {
            source
                .get(name)
                .and_then(Value::as_str)
                .or_else(|| metadata.get(name).and_then(Value::as_str))
        })
        .map(str::to_owned)
}

fn safe_uri(value: Option<String>) -> Option<String> {
    value.filter(|value| {
        reqwest::Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
        })
    })
}

pub(super) fn finalize(provider: &str, execution: WorkerExecution) -> WorkerExecution {
    output::finalize_retrieval_response(provider, execution)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieval_keeps_text_provenance_and_existing_attachments_without_fabricating_files() {
        let result = retrieval_output(
            &json!({"documents":[
                {"doc_id":"doc1","id":"chunk1","content_with_weight":"First","docnm_kwd":"Guide","similarity":0.9,"files":[{"id":"image1","url":"https://kb.example/img.png","name":"image.png"}]},
            {"documentId":"doc2","chunkId":"chunk2","content":"Second","title":null,"metadata":{"title":"Notes"},"files":[{"url":"javascript:alert(1)"},{"url":"https://secret:password@kb.example/file"}]},
                {"content":"outside top K"}
            ]}),
            Uuid::nil(),
            2,
        );
        assert_eq!(result["text"], "First\n\nSecond");
        assert_eq!(result["documents"][0]["documentId"], "doc1");
        assert_eq!(result["documents"][1]["title"], "Notes");
        assert!(result["documents"][1]["score"].is_null());
        assert_eq!(result["files"].as_array().unwrap().len(), 1);
        assert_eq!(result["files"][0]["referenceId"], "image1");
        assert!(result["files"][0].get("artifactId").is_none());
        let manifest: Value = serde_json::from_str(include_str!(
            "../../../plugins/builtin/core/nodes/knowledge_retrieval.json"
        ))
        .unwrap();
        jsonschema::validator_for(&manifest["outputSchema"])
            .unwrap()
            .validate(&result)
            .unwrap();
    }

    #[test]
    fn empty_retrieval_is_a_successful_empty_output() {
        assert_eq!(
            retrieval_output(&json!({"documents":[]}), Uuid::nil(), 5),
            json!({"text":"","documents":[],"files":[]})
        );
    }

    #[test]
    fn ragflow_native_chunk_keeps_document_metadata_and_opaque_image_reference() {
        let result = retrieval_output(
            &json!({"documents":[
                {"document_id":"doc1","id":"chunk1","content":"Native chunk","document_keyword":"guide.pdf","document_metadata":{"author":"Support"},"image_id":"dataset-image-1"},
                {"document_id":"doc1","id":"chunk2","content":"Second","image_id":"dataset-image-1"},
                {"document_id":"doc2","id":"chunk3","content":"Third","image_id":""}
            ]}),
            Uuid::nil(),
            3,
        );
        assert_eq!(result["documents"][0]["documentId"], "doc1");
        assert_eq!(result["documents"][0]["title"], "guide.pdf");
        assert_eq!(result["documents"][0]["metadata"]["author"], "Support");
        assert_eq!(result["files"].as_array().unwrap().len(), 1);
        assert_eq!(result["files"][0]["referenceId"], "dataset-image-1");
        assert!(result["files"][0]["uri"].is_null());
        assert!(result["files"][0].get("artifactId").is_none());
    }
}
