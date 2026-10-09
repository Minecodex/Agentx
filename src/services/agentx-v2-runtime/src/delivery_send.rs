//! Provider send clients for outbound delivery (plan7 P7-A). Each platform
//! gets the minimal single-message-send API surface; credentials come from the
//! frozen Vault snapshot on the delivery row and all traffic goes through the
//! egress provider client. Domain control lives here because the egress
//! gateway has no hostname allowlist: every destination must match the
//! provider's allowed suffixes.

use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::delivery::DeliveryClaim;
use crate::egress::{EgressRequestContext, ProviderHttpClient};
use crate::vault::RuntimeVault;

pub struct DeliverySendError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl DeliverySendError {
    fn rejected(message: String) -> Self {
        Self {
            code: "DELIVERY_PROVIDER_REJECTED",
            message,
            retryable: false,
        }
    }

    fn unavailable(message: String) -> Self {
        Self {
            code: "PROVIDER_UNAVAILABLE",
            message,
            retryable: true,
        }
    }

    fn rate_limited(message: String) -> Self {
        Self {
            code: "PROVIDER_RATE_LIMITED",
            message,
            retryable: true,
        }
    }
}

fn allowed_host(url: &str, suffixes: &[&str]) -> Result<(), DeliverySendError> {
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .ok_or_else(|| DeliverySendError::rejected("invalid delivery url".into()))?;
    if suffixes
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
    {
        return Ok(());
    }
    // E2E fixtures and in-cluster relays point sessionWebhook at services
    // like `im-mock.<namespace>.svc`; extra entries match the leading DNS
    // label exactly and extend (never replace) the per-provider suffixes.
    if let Ok(extra) = std::env::var("AGENTX_DELIVERY_DOMAIN_EXTRA_ALLOWLIST") {
        if extra
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .any(|label| host == label || host.starts_with(&format!("{label}.")))
        {
            return Ok(());
        }
    }
    Err(DeliverySendError::rejected(format!(
        "delivery host {host} is outside the provider allowlist"
    )))
}

fn text_of(payload: &Value) -> Result<String, DeliverySendError> {
    payload
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| DeliverySendError::rejected("delivery payload has no text".into()))
}

enum DeliveryAuth<'a> {
    Bearer(&'a str),
    DingTalk(&'a str),
}

async fn send_json(
    http: &ProviderHttpClient,
    url: &str,
    suffixes: &[&str],
    tenant_id: Uuid,
    request_id: Uuid,
    auth: Option<DeliveryAuth<'_>>,
    body: Value,
) -> Result<(u16, Value), DeliverySendError> {
    send_request(
        http,
        reqwest::Method::POST,
        url,
        suffixes,
        tenant_id,
        request_id,
        auth,
        Some(body),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn send_request(
    http: &ProviderHttpClient,
    method: reqwest::Method,
    url: &str,
    suffixes: &[&str],
    tenant_id: Uuid,
    request_id: Uuid,
    auth: Option<DeliveryAuth<'_>>,
    body: Option<Value>,
) -> Result<(u16, Value), DeliverySendError> {
    allowed_host(url, suffixes)?;
    let mut builder = http
        .request(
            method,
            url,
            EgressRequestContext::request(tenant_id, request_id),
            Duration::from_secs(15),
        )
        .map_err(|_| DeliverySendError::rejected("invalid delivery endpoint".into()))?;
    if let Some(body) = body {
        builder = builder.json(&body);
    }
    match auth {
        Some(DeliveryAuth::Bearer(token)) => builder = builder.bearer_auth(token),
        Some(DeliveryAuth::DingTalk(token)) => {
            builder = builder.header("x-acs-dingtalk-access-token", token)
        }
        None => {}
    }
    let response = builder.send().await.map_err(|error| {
        if error.is_connect() || error.is_timeout() {
            DeliverySendError::unavailable("delivery connection unavailable or timed out".into())
        } else {
            DeliverySendError::rejected("delivery request failed or was denied".into())
        }
    })?;
    let status = response.status().as_u16();
    let parsed = response.json::<Value>().await.map_err(|error| {
        DeliverySendError::unavailable(format!(
            "unreadable provider response: {}",
            error.without_url()
        ))
    })?;
    Ok((status, parsed))
}

/// Reads the credential JSON from the frozen Vault snapshot on the claim.
pub async fn credential_json(
    vault: &RuntimeVault,
    claim: &DeliveryClaim,
) -> Result<Value, DeliverySendError> {
    let reference = claim
        .credential_ref
        .clone()
        .ok_or_else(|| DeliverySendError::rejected("delivery has no credential snapshot".into()))?;
    let parsed: agentx_runtime_contracts::VaultSecretReferenceV1 =
        serde_json::from_value(reference).map_err(|error| {
            DeliverySendError::rejected(format!("invalid credential reference: {error}"))
        })?;
    let raw = vault
        .read(&parsed)
        .await
        .map_err(|error| DeliverySendError::unavailable(format!("vault read failed: {error}")))?;
    serde_json::from_slice(&raw).map_err(|error| {
        DeliverySendError::rejected(format!("channel credential is not JSON: {error}"))
    })
}

/// Sends one delivery and returns the provider message id when the platform
/// reports one.
pub async fn send(
    vault: &RuntimeVault,
    http: &ProviderHttpClient,
    claim: &DeliveryClaim,
) -> Result<Option<String>, DeliverySendError> {
    let text = text_of(&claim.payload)?;
    match claim.provider.as_str() {
        "dingtalk" => send_dingtalk(vault, http, claim, &text).await,
        "feishu" => send_feishu(vault, http, claim, &text).await,
        "wecom" => send_wecom(vault, http, claim, &text).await,
        other => Err(DeliverySendError::rejected(format!(
            "unsupported delivery provider: {other}"
        ))),
    }
}

const DINGTALK_SUFFIXES: &[&str] = &["oapi.dingtalk.com", "api.dingtalk.com"];
const FEISHU_SUFFIXES: &[&str] = &["open.feishu.cn", "open.larksuite.com"];
const WECOM_SUFFIXES: &[&str] = &["qyapi.weixin.qq.com"];

/// Base for the official DingTalk robot API; overridable so isolated E2E
/// environments can point the token/send calls at the in-cluster mock.
fn dingtalk_api_base() -> String {
    api_base(
        "AGENTX_DELIVERY_DINGTALK_API_BASE",
        "https://api.dingtalk.com",
    )
}

fn api_base(variable: &str, default: &str) -> String {
    std::env::var(variable)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.into())
}

async fn send_dingtalk(
    vault: &RuntimeVault,
    http: &ProviderHttpClient,
    claim: &DeliveryClaim,
    text: &str,
) -> Result<Option<String>, DeliverySendError> {
    let credential = credential_json(vault, claim).await?;
    let session_webhook = claim.target.get("sessionWebhook").and_then(Value::as_str);
    let expires_at_ms = claim
        .target
        .get("sessionWebhookExpiresAt")
        .and_then(Value::as_i64);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default();
    let session_usable = session_webhook.is_some_and(|_| {
        expires_at_ms
            .map(|expires| expires > now_ms + 60_000)
            .unwrap_or(true)
    });
    if session_usable {
        let url = session_webhook.unwrap_or_default();
        let (status, body) = send_json(
            http,
            url,
            DINGTALK_SUFFIXES,
            claim.tenant_id,
            claim.id,
            None,
            json!({"msgtype": "text", "text": {"content": text}}),
        )
        .await?;
        return classify_platform_response(status, &body, "errcode", &["0"]);
    }
    // Official robot API fallback: access token then batch (direct) or group
    // send keyed by the conversation type from the trigger context.
    let app_key = credential.get("clientId").and_then(Value::as_str);
    let app_secret = credential.get("clientSecret").and_then(Value::as_str);
    let robot_code = credential.get("robotCode").and_then(Value::as_str);
    let (app_key, app_secret, robot_code) = match (app_key, app_secret, robot_code) {
        (Some(a), Some(s), Some(r)) => (a, s, r),
        _ => {
            return Err(DeliverySendError::rejected(
                "dingtalk channel lacks robot API credentials".into(),
            ));
        }
    };
    let (status, body) = send_json(
        http,
        &format!("{}/v1.0/oauth2/accessToken", dingtalk_api_base()),
        DINGTALK_SUFFIXES,
        claim.tenant_id,
        claim.id,
        None,
        json!({"appKey": app_key, "appSecret": app_secret}),
    )
    .await?;
    if status != 200 {
        return classify_platform_response(status, &body, "code", &["0"]);
    }
    let token = body
        .get("accessToken")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DeliverySendError::rejected("dingtalk token response missed accessToken".into())
        })?
        .to_owned();
    let conversation_type = claim
        .target
        .get("conversationType")
        .and_then(Value::as_str)
        .unwrap_or("direct");
    let request = if conversation_type == "group" {
        let conversation_id = claim
            .target
            .get("conversationId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DeliverySendError::rejected("group delivery has no conversationId".into())
            })?;
        (
            format!("{}/v1.0/robot/groupMessages/send", dingtalk_api_base()),
            json!({"robotCode": robot_code, "openConversationId": conversation_id, "msgKey": "sampleText", "msgParam": json!({"content": text}).to_string()}),
        )
    } else {
        let sender_id = claim
            .target
            .get("senderId")
            .and_then(Value::as_str)
            .ok_or_else(|| DeliverySendError::rejected("direct delivery has no senderId".into()))?;
        (
            format!("{}/v1.0/robot/oToMessages/batchSend", dingtalk_api_base()),
            json!({"robotCode": robot_code, "userIds": [sender_id], "msgKey": "sampleText", "msgParam": json!({"content": text}).to_string()}),
        )
    };
    let (status, body) = send_json(
        http,
        &request.0,
        DINGTALK_SUFFIXES,
        claim.tenant_id,
        claim.id,
        Some(DeliveryAuth::DingTalk(&token)),
        request.1,
    )
    .await?;
    let message_id = body
        .get("messageId")
        .or_else(|| body.get("processQueryKey"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    classify_platform_response(status, &body, "code", &["0"])?;
    Ok(message_id)
}

async fn send_feishu(
    vault: &RuntimeVault,
    http: &ProviderHttpClient,
    claim: &DeliveryClaim,
    text: &str,
) -> Result<Option<String>, DeliverySendError> {
    let credential = credential_json(vault, claim).await?;
    let app_id = credential.get("appId").and_then(Value::as_str);
    let app_secret = credential.get("appSecret").and_then(Value::as_str);
    let (app_id, app_secret) = match (app_id, app_secret) {
        (Some(a), Some(s)) => (a, s),
        _ => {
            return Err(DeliverySendError::rejected(
                "feishu channel lacks app credentials".into(),
            ));
        }
    };
    let (status, body) = send_json(
        http,
        &format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            api_base("AGENTX_DELIVERY_FEISHU_API_BASE", "https://open.feishu.cn")
        ),
        FEISHU_SUFFIXES,
        claim.tenant_id,
        claim.id,
        None,
        json!({"app_id": app_id, "app_secret": app_secret}),
    )
    .await?;
    if status != 200 || body.get("code").and_then(Value::as_i64) != Some(0) {
        return classify_platform_response(status, &body, "code", &["0"]);
    }
    let token = body
        .get("tenant_access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DeliverySendError::rejected("feishu token response missed tenant_access_token".into())
        })?
        .to_owned();
    let conversation_id = claim
        .target
        .get("conversationId")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DeliverySendError::rejected("feishu delivery has no conversationId".into())
        })?;
    let (status, body) = send_json(
        http,
        &format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", api_base("AGENTX_DELIVERY_FEISHU_API_BASE", "https://open.feishu.cn")),
        FEISHU_SUFFIXES,
        claim.tenant_id,
        claim.id,
        Some(DeliveryAuth::Bearer(&token)),
        json!({"receive_id": conversation_id, "msg_type": "text", "content": json!({"text": text}).to_string()}),
    )
    .await?;
    let message_id = body
        .pointer("/data/message_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    classify_platform_response(status, &body, "code", &["0"])?;
    Ok(message_id)
}

async fn send_wecom(
    vault: &RuntimeVault,
    http: &ProviderHttpClient,
    claim: &DeliveryClaim,
    text: &str,
) -> Result<Option<String>, DeliverySendError> {
    let credential = credential_json(vault, claim).await?;
    let corp_id = credential.get("corpId").and_then(Value::as_str);
    let corp_secret = credential.get("corpSecret").and_then(Value::as_str);
    let agent_id = credential
        .get("agentId")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0);
    let (corp_id, corp_secret, agent_id) = match (corp_id, corp_secret, agent_id) {
        (Some(c), Some(s), Some(a)) => (c, s, a),
        _ => {
            return Err(DeliverySendError::rejected(
                "wecom channel lacks corp credentials".into(),
            ));
        }
    };
    let base = api_base(
        "AGENTX_DELIVERY_WECOM_API_BASE",
        "https://qyapi.weixin.qq.com",
    );
    let mut token_url = reqwest::Url::parse(&format!("{base}/cgi-bin/gettoken"))
        .map_err(|_| DeliverySendError::rejected("invalid wecom API base".into()))?;
    token_url
        .query_pairs_mut()
        .append_pair("corpid", corp_id)
        .append_pair("corpsecret", corp_secret);
    let (status, body) = send_request(
        http,
        reqwest::Method::GET,
        token_url.as_str(),
        WECOM_SUFFIXES,
        claim.tenant_id,
        claim.id,
        None,
        None,
    )
    .await?;
    if status != 200 || body.get("errcode").and_then(Value::as_i64) != Some(0) {
        return classify_platform_response(status, &body, "errcode", &["0"]);
    }
    let token = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DeliverySendError::rejected("wecom token response missed access_token".into())
        })?
        .to_owned();
    let to_user = claim
        .target
        .get("senderId")
        .and_then(Value::as_str)
        .ok_or_else(|| DeliverySendError::rejected("wecom delivery has no senderId".into()))?;
    let mut send_url = reqwest::Url::parse(&format!("{base}/cgi-bin/message/send"))
        .map_err(|_| DeliverySendError::rejected("invalid wecom API base".into()))?;
    send_url
        .query_pairs_mut()
        .append_pair("access_token", &token);
    let (status, body) = send_json(
        http,
        send_url.as_str(),
        WECOM_SUFFIXES,
        claim.tenant_id,
        claim.id,
        None,
        json!({"touser": to_user, "msgtype": "text", "agentid": agent_id, "text": {"content": text}}),
    )
    .await?;
    classify_platform_response(status, &body, "errcode", &["0"])?;
    Ok(body.get("msgid").and_then(Value::as_str).map(str::to_owned))
}

/// Maps a platform response to success/retryable/permanent using the HTTP
/// status and the platform's own code field. Rate limits are retryable, 5xx
/// are retryable, other non-success codes are permanent.
fn classify_platform_response(
    status: u16,
    body: &Value,
    code_field: &str,
    success_codes: &[&str],
) -> Result<Option<String>, DeliverySendError> {
    if status == 429 {
        return Err(DeliverySendError::rate_limited(format!(
            "provider rate limited: {status}"
        )));
    }
    if status >= 500 {
        return Err(DeliverySendError::unavailable(format!(
            "provider server error: {status}"
        )));
    }
    let code = body.get(code_field).map(|value| match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        _ => String::new(),
    });
    let code = code.unwrap_or_default();
    let official_dingtalk_success = code_field == "code"
        && code.is_empty()
        && body
            .get("processQueryKey")
            .and_then(Value::as_str)
            .is_some();
    if (200..300).contains(&status)
        && (success_codes.contains(&code.as_str()) || official_dingtalk_success)
    {
        return Ok(body
            .get("message_id")
            .or_else(|| body.pointer("/data/message_id"))
            .and_then(Value::as_str)
            .map(str::to_owned));
    }
    if matches!(code.as_str(), "429" | "45009" | "99991400") {
        return Err(DeliverySendError::rate_limited(format!(
            "provider rate limited: {body}"
        )));
    }
    Err(DeliverySendError::rejected(format!(
        "provider rejected delivery: {status} {body}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_allowlist_rejects_unknown_hosts() {
        assert!(allowed_host("https://oapi.dingtalk.com/x", DINGTALK_SUFFIXES).is_ok());
        assert!(allowed_host("https://evil.example.com/x", DINGTALK_SUFFIXES).is_err());
        assert!(
            allowed_host(
                "https://api.dingtalk.com.attacker.example/x",
                DINGTALK_SUFFIXES
            )
            .is_err()
        );
    }

    #[test]
    fn platform_classification_buckets() {
        assert!(classify_platform_response(200, &json!({"errcode": 0}), "errcode", &["0"]).is_ok());
        assert!(
            classify_platform_response(429, &json!({}), "errcode", &["0"])
                .unwrap_err()
                .retryable
        );
        assert!(
            classify_platform_response(503, &json!({}), "errcode", &["0"])
                .unwrap_err()
                .retryable
        );
        assert!(
            !classify_platform_response(401, &json!({}), "errcode", &["0"])
                .unwrap_err()
                .retryable
        );
    }
}
