use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

pub async fn lightrag(Json(input): Json<Value>) -> Response {
    if input["query"] == "fixture-error" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status":"failure","message":"retrieval rejected"})),
        )
            .into_response();
    }
    if input["query"] == "fixture-empty" {
        return Json(json!({"status":"failure","data":{},"message":"No relevant document chunks found.","metadata":{"mode":"naive","failure_reason":"no_results"}})).into_response();
    }
    let chunks = {
        vec![
            json!({"doc_id":"guide-1","chunk_id":"chunk-1","content":"The launch code is sapphire-orbit.","score":0.91,"title":"Launch guide","files":[{"id":"diagram-1","url":"https://knowledge.example/diagram.png","name":"diagram.png","contentType":"image/png"}]}),
            json!({"doc_id":"guide-2","chunk_id":"chunk-2","content":"Contact the support team.","title":"Support guide"}),
        ]
    };
    Json(json!({"status":"success","data":{"chunks":chunks,"references":[]}})).into_response()
}

pub async fn ragflow(Json(input): Json<Value>) -> Response {
    if input["question"] == "fixture-error"
        || (input["question"] == "fixture-partial" && input["dataset_ids"][0] == "dataset-b")
    {
        return Json(json!({"code":102,"message":"retrieval rejected"})).into_response();
    }
    let chunks = if input["question"] == "fixture-empty" {
        vec![]
    } else {
        vec![
            json!({"document_id":"guide-1","id":"chunk-1","content":"The launch code is sapphire-orbit.","similarity":0.91,"document_keyword":"Launch guide","document_metadata":{"author":"Support"},"image_id":"diagram-1"}),
        ]
    };
    Json(json!({"code":0,"data":{"chunks":chunks}})).into_response()
}

pub async fn datasets() -> Json<Value> {
    Json(
        json!({"code":0,"data":[{"id":"dataset-a","name":"Product documentation"},{"id":"dataset-b","name":"Support handbook"}]}),
    )
}
pub async fn documents() -> Json<Value> {
    Json(
        json!({"code":0,"data":{"docs":[{"id":"guide-1","name":"Launch guide","run":"3"}],"total":1},"documents":[{"id":"guide-1","file_path":"Launch guide","status":"processed"}],"pagination":{"total_count":1}}),
    )
}
