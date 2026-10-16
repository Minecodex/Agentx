use agentx_api_types::{ApiErrorResponse, FieldError};
use axum::{
    Json,
    body::to_bytes,
    extract::Request,
    http::{
        HeaderValue, StatusCode,
        header::{CONTENT_TYPE, HeaderName},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    field_errors: Vec<FieldError>,
    retry_after_seconds: Option<u64>,
}

pub fn invalid_credentials() -> ApiError {
    ApiError::unauthorized("INVALID_CREDENTIALS", "Username or password is incorrect")
}

impl ApiError {
    pub(crate) fn design_feedback(&self) -> serde_json::Value {
        serde_json::json!({"code":self.code,"message":self.message,"fieldErrors":self.field_errors})
    }
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn unauthorized(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "FORBIDDEN", message)
    }

    pub fn not_found(resource: &'static str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("{resource} was not found"),
        )
    }

    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    pub fn unavailable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, message)
    }

    pub fn accepted(code: &'static str, message: impl Into<String>, retry_after: u64) -> Self {
        let mut error = Self::new(StatusCode::ACCEPTED, code, message);
        error.retry_after_seconds = Some(retry_after);
        error
    }

    pub fn with_field_error(
        mut self,
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        self.field_errors.push(FieldError {
            field: field.into(),
            code: code.into(),
            message: message.into(),
        });
        self
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "Control API request failed");
        #[cfg(test)]
        eprintln!("Control API request failed: {error}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "The request could not be completed",
        )
    }

    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            field_errors: Vec::new(),
            retry_after_seconds: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let request_id = Uuid::now_v7();
        let mut response = (
            self.status,
            Json(ApiErrorResponse {
                code: self.code.to_owned(),
                message: self.message,
                request_id,
                field_errors: self.field_errors,
                details: None,
            }),
        )
            .into_response();
        response.headers_mut().insert(
            HeaderName::from_static("x-request-id"),
            HeaderValue::from_str(&request_id.to_string()).expect("UUID header is valid"),
        );
        if let Some(seconds) = self.retry_after_seconds {
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_str(&seconds.to_string()).expect("retry seconds are valid"),
            );
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(value: sqlx::Error) -> Self {
        if value
            .as_database_error()
            .is_some_and(|db| db.is_foreign_key_violation())
        {
            return Self::conflict(
                "RESOURCE_REFERENCED",
                "The resource is referenced and cannot be deleted",
            );
        }
        Self::internal(value)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Axum's built-in JSON extractor returns a plain-text 422 before a handler is
/// entered. Normalize that rejection to the public API error envelope so web
/// forms never have to fall back to the HTTP reason phrase.
pub async fn normalize_json_rejection(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let plain_unprocessable = response.status() == StatusCode::UNPROCESSABLE_ENTITY
        && !response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"));
    if !plain_unprocessable {
        return response;
    }

    let (parts, body) = response.into_parts();
    let body = to_bytes(body, 64 * 1024).await.unwrap_or_default();
    let detail = String::from_utf8_lossy(&body);
    let mut error = ApiError::unprocessable(
        "INVALID_REQUEST_BODY",
        "The submitted form contains invalid or missing values",
    );
    if let Some((field, missing)) = rejection_field(&detail) {
        error = error.with_field_error(
            field,
            if missing {
                "REQUIRED_FIELD"
            } else {
                "INVALID_FIELD"
            },
            if missing {
                "This field is required"
            } else {
                "This field has an invalid value"
            },
        );
    }
    drop(parts);
    error.into_response()
}

fn rejection_field(detail: &str) -> Option<(String, bool)> {
    if let Some(value) = detail
        .split("missing field `")
        .nth(1)
        .and_then(|value| value.split('`').next())
    {
        return Some((value.to_owned(), true));
    }
    let value = detail
        .split("target type: ")
        .nth(1)?
        .split(':')
        .next()?
        .trim();
    (!value.is_empty()
        && value
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '_' | '.' | '[' | ']')))
    .then(|| (value.to_owned(), false))
}

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header::CONTENT_TYPE},
        middleware,
        routing::post,
    };
    use serde::Deserialize;
    use tower::ServiceExt;

    use super::{normalize_json_rejection, rejection_field};

    #[derive(Deserialize)]
    struct RequiredPayload {
        name: String,
    }

    async fn required_payload(Json(payload): Json<RequiredPayload>) {
        drop(payload.name);
    }

    #[test]
    fn extracts_missing_and_invalid_json_fields() {
        assert_eq!(
            rejection_field(
                "Failed to deserialize the JSON body into the target type: missing field `ownerDepartmentId` at line 1 column 2"
            ),
            Some(("ownerDepartmentId".into(), true)),
        );
        assert_eq!(
            rejection_field(
                "Failed to deserialize the JSON body into the target type: price.inputPerMillion: invalid type: null, expected a string"
            ),
            Some(("price.inputPerMillion".into(), false)),
        );
        assert_eq!(
            rejection_field("Failed to parse the request body as JSON"),
            None
        );
    }

    #[tokio::test]
    async fn normalizes_plain_json_extractor_rejections() {
        let app = Router::new()
            .route("/", post(required_payload))
            .layer(middleware::from_fn(normalize_json_rejection));
        let response = app
            .oneshot(
                Request::post("/")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["code"], "INVALID_REQUEST_BODY");
        assert_eq!(body["fieldErrors"][0]["field"], "name");
        assert_eq!(body["fieldErrors"][0]["code"], "REQUIRED_FIELD");
    }
}
