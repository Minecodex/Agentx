use agentx_runtime_contracts::RuntimePublishErrorCodeV1;
use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("{message}")]
    Deterministic { code: &'static str, message: String },
    #[error("{1}")]
    InvalidRequest(&'static str, String),
    #[error("{1}")]
    BadRequest(RuntimePublishErrorCodeV1, String),
    #[error("{1}")]
    Conflict(RuntimePublishErrorCodeV1, String),
    #[error("authentication failed")]
    Unauthorized,
    #[error("resource not found")]
    NotFound,
    #[error("runtime storage is unavailable")]
    Unavailable,
    #[error("sandbox provider rejected the operation")]
    ProviderRejected,
    #[error("runtime database is unavailable")]
    DatabaseUnavailable,
    #[error("runtime admission rejected the overloaded request")]
    AdmissionRejected { retry_after_seconds: u32 },
    #[error("runtime query budget was exceeded")]
    QueryBudgetExceeded,
    #[error("runtime query cursor expired")]
    QueryCursorExpired,
    #[error("runtime event cursor expired")]
    EventCursorExpired,
    #[error("runtime secret provider is unavailable")]
    SecretUnavailable,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    code: Value,
    message: String,
}

impl IntoResponse for RuntimeError {
    fn into_response(self) -> axum::response::Response {
        let retry_after = match &self {
            Self::AdmissionRejected {
                retry_after_seconds,
            } => Some(*retry_after_seconds),
            _ => None,
        };
        let (status, code, message) = match self {
            Self::Deterministic { code, message } => {
                (StatusCode::UNPROCESSABLE_ENTITY, json!(code), message)
            }
            Self::InvalidRequest(code, message) => (StatusCode::BAD_REQUEST, json!(code), message),
            Self::BadRequest(code, message) => {
                (StatusCode::UNPROCESSABLE_ENTITY, json!(code), message)
            }
            Self::Conflict(code, message) => (StatusCode::CONFLICT, json!(code), message),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                json!("UNAUTHORIZED"),
                "authentication failed".into(),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                json!("NOT_FOUND"),
                "resource not found".into(),
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                json!("RUNTIME_STORAGE_UNAVAILABLE"),
                "runtime storage is unavailable".into(),
            ),
            Self::ProviderRejected => (
                StatusCode::BAD_GATEWAY,
                json!("SANDBOX_PROVIDER_REJECTED"),
                "sandbox provider rejected the operation".into(),
            ),
            Self::DatabaseUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                json!("RUNTIME_DATABASE_UNAVAILABLE"),
                "runtime database is unavailable".into(),
            ),
            Self::AdmissionRejected {
                retry_after_seconds,
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                json!("RUNTIME_ADMISSION_REJECTED"),
                format!("runtime is overloaded; retry after {retry_after_seconds}s"),
            ),
            Self::QueryBudgetExceeded => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!("QUERY_BUDGET_EXCEEDED"),
                "the query must be narrowed to at most 10000 rows".into(),
            ),
            Self::QueryCursorExpired => (
                StatusCode::GONE,
                json!("QUERY_CURSOR_EXPIRED"),
                "the query snapshot cursor expired".into(),
            ),
            Self::EventCursorExpired => (
                StatusCode::GONE,
                json!("CURSOR_EXPIRED"),
                "the event cursor is below the retention floor".into(),
            ),
            Self::SecretUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                json!("RUNTIME_SECRET_UNAVAILABLE"),
                "runtime secret provider is unavailable".into(),
            ),
            Self::Internal(error) => {
                tracing::error!(%error, "runtime request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!("INTERNAL"),
                    "internal runtime error".into(),
                )
            }
        };
        let mut response = (status, Json(ErrorBody { code, message })).into_response();
        if let Some(seconds) = retry_after
            && let Ok(value) = seconds.to_string().parse()
        {
            response.headers_mut().insert("retry-after", value);
        }
        response
    }
}

impl From<sqlx::Error> for RuntimeError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(%error, "Runtime MySQL request failed");
        Self::DatabaseUnavailable
    }
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;
