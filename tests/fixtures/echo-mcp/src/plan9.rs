//! A deliberately non-idempotent CRM fixture: every real tools/call creates one record.
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub(crate) struct CrmLedger(pub Arc<Mutex<Vec<Quote>>>);

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateQuote {
    pub customer: String,
    pub gross_amount: f64,
    pub discount_percent: f64,
    pub reference: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Quote {
    pub receipt_id: String,
    pub customer: String,
    pub total: f64,
    pub discount_percent: f64,
    pub reference: String,
}

impl CrmLedger {
    pub fn create(&self, input: CreateQuote) -> Quote {
        let mut records = self.0.lock().expect("CRM fixture lock");
        let quote = Quote {
            receipt_id: format!("CRM-{:06}", records.len() + 1),
            customer: input.customer,
            total: (input.gross_amount * (1.0 - input.discount_percent / 100.0) * 100.0).round()
                / 100.0,
            discount_percent: input.discount_percent,
            reference: input.reference,
        };
        records.push(quote.clone());
        quote
    }
}

#[derive(Deserialize)]
pub(crate) struct ReportQuery {
    summary: String,
}

pub(crate) async fn report(
    axum::extract::Query(query): axum::extract::Query<ReportQuery>,
) -> impl axum::response::IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/octet-stream"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=business-report.txt",
            ),
        ],
        format!("Agentx business report\n{}\n", query.summary),
    )
}

/// Drain the real CRM response (and therefore the write), then break the reply
/// stream. Only the owned plan9 fixture and the explicit fault reference use it.
pub(crate) async fn lose_quote_response(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::body::{Body, to_bytes};
    if std::env::var("AGENTX_FIXTURE_PLAN9").as_deref() != Ok("1")
        || request.method() != axum::http::Method::POST
        || request.uri().path() != "/mcp"
    {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return axum::response::Response::builder()
                .status(413)
                .body(Body::empty())
                .expect("response");
        }
    };
    let lose = serde_json::from_slice::<serde_json::Value>(&bytes).is_ok_and(|value| {
        value["method"] == "tools/call"
            && value["params"]["name"] == "crm_create_quote"
            && value
                .pointer("/params/arguments/reference")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reference| reference.ends_with("-lost-response"))
    });
    let response = next
        .run(axum::extract::Request::from_parts(parts, Body::from(bytes)))
        .await;
    if !lose {
        return response;
    }
    let (parts, body) = response.into_parts();
    let _ = to_bytes(body, 1024 * 1024).await;
    axum::response::Response::from_parts(
        parts,
        Body::from_stream(futures_util::stream::once(async {
            Err::<bytes::Bytes, _>(std::io::Error::other(
                "controlled reply loss after CRM write",
            ))
        })),
    )
}
