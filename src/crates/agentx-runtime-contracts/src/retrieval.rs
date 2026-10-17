//! Shared source expansion, bounded execution, provenance and reciprocal rank
//! fusion. Transports and authorization belong to the caller; authorization
//! must complete for the entire selection before entering this executor.
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    time::Instant,
};

use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::rag::{RagProtocolError, RagScope, finalize_retrieval_value};

#[derive(Clone, Debug)]
pub struct QueryScope {
    pub query_id: Uuid,
    pub connection_id: Uuid,
    pub connection_version: u64,
    pub scope: RagScope,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetrievalSource {
    pub query_ids: Vec<Uuid>,
    pub connection_id: Uuid,
    pub connection_version: u64,
    pub scope: RagScope,
}

pub fn resolve_sources(queries: &[QueryScope]) -> Result<Vec<RetrievalSource>, RagProtocolError> {
    if queries.is_empty() {
        return Err(RagProtocolError::new(
            "RAG_SCOPE_REQUIRED",
            "Select at least one query",
        ));
    }
    let mut sources = BTreeMap::new();
    for query in queries {
        query.scope.validate(query.scope.provider())?;
        let scopes = match &query.scope {
            RagScope::Ragflow { dataset_ids } => dataset_ids
                .iter()
                .map(|id| RagScope::Ragflow {
                    dataset_ids: vec![id.clone()],
                })
                .collect(),
            scope => vec![scope.clone()],
        };
        for scope in scopes {
            let source = sources
                .entry((query.connection_id, query.connection_version, scope.clone()))
                .or_insert_with(|| RetrievalSource {
                    query_ids: vec![],
                    connection_id: query.connection_id,
                    connection_version: query.connection_version,
                    scope,
                });
            if !source.query_ids.contains(&query.query_id) {
                source.query_ids.push(query.query_id);
            }
        }
    }
    Ok(sources.into_values().collect())
}

#[derive(Debug)]
pub struct RetrievalFailure {
    pub code: &'static str,
    pub message: String,
    pub sources: Vec<Value>,
}

/// Each callback returns the native provider response. Protocol errors,
/// timings, empty success and fusion are identical on every product surface.
pub async fn retrieve<F, Fut>(
    sources: Vec<RetrievalSource>,
    top_k: usize,
    fetch: F,
) -> Result<Value, RetrievalFailure>
where
    F: Fn(usize, RetrievalSource) -> Fut,
    Fut: Future<Output = Result<Value, RagProtocolError>>,
{
    if !(1..=20).contains(&top_k) || sources.is_empty() {
        return Err(RetrievalFailure {
            code: "RAG_QUERY_INVALID",
            message: "Select a source and a Top K between 1 and 20".into(),
            sources: vec![],
        });
    }
    let mut results = stream::iter(sources.into_iter().enumerate().map(|(index, source)| {
        let pending = fetch(index, source.clone());
        async move {
            let started = Instant::now();
            let result = pending
                .await
                .and_then(|value| finalize_retrieval_value(source.scope.provider(), value));
            (index, source, started.elapsed().as_millis() as u64, result)
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    // Network completion order must not affect tie-breaking or provenance.
    results.sort_by_key(|(index, ..)| *index);
    let mut statuses = Vec::new();
    let mut lists = Vec::new();
    for (_, source, elapsed, result) in results {
        let mut status = serde_json::to_value(&source).expect("source serialization");
        status["elapsedMs"] = json!(elapsed);
        match result {
            Ok(value) => {
                status["status"] = json!("succeeded");
                status["error"] = Value::Null;
                lists.push((source, value));
            }
            Err(error) => {
                status["status"] = json!("failed");
                status["error"] = json!({"code":error.code,"message":error.message});
            }
        }
        statuses.push(status);
    }
    if lists.is_empty() {
        return Err(RetrievalFailure {
            code: "KNOWLEDGE_ALL_SOURCES_FAILED",
            message: "All knowledge sources failed".into(),
            sources: statuses,
        });
    }
    let partial = lists.len() != statuses.len();
    let mut output = fuse(lists, top_k);
    output["partial"] = json!(partial);
    output["sources"] = json!(statuses);
    Ok(output)
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
        url::Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
        })
    })
}

fn fuse(lists: Vec<(RetrievalSource, Value)>, top_k: usize) -> Value {
    let mut hits: BTreeMap<String, Value> = BTreeMap::new();
    for (source, value) in lists {
        let mut seen = BTreeSet::new();
        for (rank, chunk) in value["documents"]
            .as_array()
            .into_iter()
            .flatten()
            .take(20)
            .enumerate()
        {
            let metadata = chunk
                .get("metadata")
                .or_else(|| chunk.get("document_metadata"))
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let field = |names: &[&str]| string_field(chunk, &metadata, names);
            let document_id = field(&[
                "documentId",
                "document_id",
                "doc_id",
                "document",
                "file_path",
            ]);
            let chunk_id = field(&["chunkId", "chunk_id", "id"]);
            let content = field(&["content", "content_with_weight", "text", "pageContent"])
                .unwrap_or_default();
            let identity = json!([
                source.connection_id,
                source.connection_version,
                document_id,
                chunk_id.as_deref().unwrap_or(&content)
            ])
            .to_string();
            if !seen.insert(identity.clone()) {
                continue;
            }
            let raw_score = ["score", "similarity"]
                .iter()
                .find_map(|key| chunk.get(key).and_then(Value::as_f64));
            let mut provenance = serde_json::to_value(&source).expect("source serialization");
            provenance["rawScore"] = json!(raw_score);
            provenance["rank"] = json!(rank + 1);
            let mut files = vec![];
            for file in chunk
                .get("files")
                .or_else(|| chunk.get("attachments"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(20)
            {
                let id = string_field(file, &Value::Null, &["referenceId", "id", "image_id"]);
                let uri = safe_uri(string_field(file, &Value::Null, &["uri", "url"]));
                if id.is_some() || uri.is_some() {
                    files.push(json!({"documentId":document_id,"referenceId":id,"uri":uri,"name":string_field(file,&Value::Null,&["name","fileName"]),"contentType":string_field(file,&Value::Null,&["contentType","mime_type"]),"sources":[provenance.clone()]}));
                }
            }
            if let Some(id) = chunk["image_id"].as_str().filter(|id| !id.is_empty()) {
                files.push(json!({"documentId":document_id,"referenceId":id,"uri":null,"name":null,"contentType":null,"sources":[provenance.clone()]}));
            }
            let hit = hits.entry(identity).or_insert_with(|| json!({
                "queryIds":[],"documentId":document_id,"chunkId":chunk_id,"content":content,
                "title":field(&["title","document_name","document_keyword","docnm_kwd","file_name","name"]),
                "uri":safe_uri(field(&["uri","url","source"])),"metadata":metadata,
                "sources":[],"fusionScore":0.0,"files":[],
            }));
            hit["fusionScore"] =
                json!(hit["fusionScore"].as_f64().unwrap() + 1.0 / (60.0 + rank as f64 + 1.0));
            hit["sources"].as_array_mut().unwrap().push(provenance);
            for id in &source.query_ids {
                let ids = hit["queryIds"].as_array_mut().unwrap();
                if !ids.contains(&json!(id)) {
                    ids.push(json!(id));
                }
            }
            hit["files"].as_array_mut().unwrap().extend(files);
        }
    }
    let mut documents = hits.into_values().collect::<Vec<_>>();
    documents.sort_by(|a, b| {
        b["fusionScore"]
            .as_f64()
            .unwrap()
            .total_cmp(&a["fusionScore"].as_f64().unwrap())
    });
    documents.truncate(top_k);
    let mut files: BTreeMap<String, Value> = BTreeMap::new();
    for document in &mut documents {
        for file in document
            .as_object_mut()
            .unwrap()
            .remove("files")
            .unwrap()
            .as_array()
            .unwrap()
        {
            let key = json!([
                file["sources"][0]["connectionId"],
                file["sources"][0]["connectionVersion"],
                file["documentId"],
                file["referenceId"],
                file["uri"]
            ])
            .to_string();
            let merged = files.entry(key).or_insert_with(|| {
                let mut value = file.clone();
                value["sources"] = json!([]);
                value
            });
            let sources = merged["sources"].as_array_mut().unwrap();
            for source in file["sources"].as_array().unwrap() {
                if !sources.contains(source) {
                    sources.push(source.clone());
                }
            }
        }
    }
    let text = documents
        .iter()
        .filter_map(|d| d["content"].as_str())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    json!({"text":text,"documents":documents,"files":files.into_values().collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn query(id: u128, datasets: &[&str]) -> QueryScope {
        QueryScope {
            query_id: Uuid::from_u128(id),
            connection_id: Uuid::nil(),
            connection_version: 1,
            scope: RagScope::Ragflow {
                dataset_ids: datasets.iter().map(|s| s.to_string()).collect(),
            },
        }
    }
    #[test]
    fn repeated_sources_are_coalesced_and_rrf_does_not_compare_raw_scores() {
        let sources = resolve_sources(&[query(1, &["a", "b"]), query(2, &["a"])]).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].query_ids.len(), 2);
        let value = futures::executor::block_on(retrieve(sources, 5, |index, _| async move {
            Ok(json!({"code":0,"data":{"chunks":[{"id":"shared","doc_id":"doc","content":"common","similarity":if index == 0 {0.01} else {1000.0}},{"id":format!("unique{index}"),"content":"unique","similarity":99999}]}}))
        })).unwrap();
        assert_eq!(value["documents"].as_array().unwrap().len(), 3);
        assert_eq!(value["documents"][0]["chunkId"], "shared");
        assert_eq!(
            value["documents"][0]["queryIds"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            value["documents"][0]["sources"].as_array().unwrap().len(),
            2
        );
        assert_eq!(value["documents"][0]["fusionScore"], 2.0 / 61.0);
        assert_eq!(value["partial"], false);
    }
    #[test]
    fn empty_success_is_success_and_partial_is_explicit() {
        for failed in 0..=2 {
            let result = futures::executor::block_on(retrieve(
                resolve_sources(&[query(1, &["a", "b"])]).unwrap(),
                5,
                |index, _| async move {
                    if index < failed {
                        Err(RagProtocolError::new("PROVIDER_UNAVAILABLE", "unavailable"))
                    } else {
                        Ok(json!({"code":0,"data":{"chunks":[]}}))
                    }
                },
            ));
            if failed == 2 {
                assert_eq!(result.unwrap_err().sources.len(), 2);
            } else {
                let value = result.unwrap();
                assert_eq!(value["partial"], failed == 1);
                assert_eq!(value["documents"], json!([]));
            }
        }
    }
    #[test]
    fn concurrency_is_bounded_and_connection_versions_stay_distinct() {
        use std::cell::Cell;
        use std::task::Poll;
        let queries = (1..=9)
            .map(|id| {
                let mut q = query(id, &["a"]);
                q.connection_version = id as u64;
                q
            })
            .collect::<Vec<_>>();
        let sources = resolve_sources(&queries).unwrap();
        assert_eq!(sources.len(), 9);
        let active = Cell::new(0usize);
        let peak = Cell::new(0usize);
        let calls = Cell::new(0usize);
        let result = futures::executor::block_on(retrieve(sources, 20, |_, _| {
            let mut started = false;
            let (active, peak, calls) = (&active, &peak, &calls);
            futures::future::poll_fn(move |cx| {
                if !started {
                    started = true;
                    calls.set(calls.get() + 1);
                    active.set(active.get() + 1);
                    peak.set(peak.get().max(active.get()));
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    active.set(active.get() - 1);
                    Poll::Ready(Ok(json!({"code":0,"data":{"chunks":[]}})))
                }
            })
        }))
        .unwrap();
        assert_eq!(calls.get(), 9);
        assert_eq!(peak.get(), 4);
        assert_eq!(active.get(), 0);
        assert_eq!(result["sources"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn candidate_limit_and_attachment_dedup_keep_complete_provenance() {
        let result = futures::executor::block_on(retrieve(resolve_sources(&[query(1, &["a"]), query(2, &["a"])]).unwrap(), 20, |_, _| async {
            Ok(json!({"code":0,"data":{"chunks":(0..30).map(|id| json!({"id":id.to_string(),"doc_id":"guide","content":"text","image_id":"diagram"})).collect::<Vec<_>>()}}))
        })).unwrap();
        assert_eq!(result["documents"].as_array().unwrap().len(), 20);
        assert_eq!(result["files"].as_array().unwrap().len(), 1);
        assert_eq!(result["files"][0]["sources"].as_array().unwrap().len(), 20);
        assert_eq!(
            result["files"][0]["sources"][0]["queryIds"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(result["documents"][0].get("score").is_none());
        assert_eq!(result["documents"][0]["fusionScore"], 1.0 / 61.0);
    }
}
