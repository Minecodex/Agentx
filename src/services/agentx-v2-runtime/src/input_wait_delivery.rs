use std::time::Duration;

use reqwest::{Method, Url};

use crate::{
    delivery::DeliveryClaim,
    delivery_send::DeliverySendError,
    egress::{EgressRequestContext, ProviderHttpClient},
    error::{RuntimeError, RuntimeResult},
};

pub(crate) fn validate_url(endpoint: &str) -> RuntimeResult<()> {
    let url = Url::parse(endpoint).map_err(|_| invalid())?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || (url.scheme() != "https" && !crate::egress::is_managed_cluster_fixture(&url))
    {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> RuntimeError {
    RuntimeError::InvalidRequest(
        "WAIT_HOOK_URL_INVALID",
        "Hook URL must be an HTTPS URL without embedded credentials or fragment".into(),
    )
}

pub(crate) async fn send(
    http: &ProviderHttpClient,
    claim: &DeliveryClaim,
) -> Result<Option<String>, DeliverySendError> {
    let url = claim
        .target
        .get("url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    validate_url(url).map_err(|error| DeliverySendError {
        code: "WAIT_HOOK_URL_INVALID",
        message: error.to_string(),
        retryable: false,
    })?;
    let request = http
        .request(
            Method::POST,
            url,
            EgressRequestContext::execution(claim.tenant_id, claim.execution_id),
            Duration::from_secs(15),
        )
        .map_err(|_| DeliverySendError {
            code: "WAIT_HOOK_URL_INVALID",
            message: "Hook endpoint is invalid".into(),
            retryable: false,
        })?;
    let response = request
        .json(&claim.payload)
        .header("Idempotency-Key", claim.id.to_string())
        .send()
        .await
        .map_err(|error| DeliverySendError {
            code: "WAIT_HOOK_UNAVAILABLE",
            message: "Hook request failed or was denied".into(),
            retryable: error.is_connect() || error.is_timeout(),
        })?;
    let status = response.status();
    if status.is_success() {
        return Ok(None);
    }
    Err(DeliverySendError {
        code: "WAIT_HOOK_REJECTED",
        message: format!("Hook returned HTTP {}", status.as_u16()),
        retryable: status.is_server_error() || status.as_u16() == 429 || status.as_u16() == 408,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hook_url_is_one_https_address() {
        assert!(validate_url("https://customer.example/workflow-hook?token=abc").is_ok());
        for url in [
            "http://customer.example/hook",
            "file:///etc/passwd",
            "https://user:secret@customer.example/hook",
            "https://customer.example/#fragment",
            "",
        ] {
            assert!(validate_url(url).is_err());
        }
    }
}
