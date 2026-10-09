use std::collections::HashMap;

use aes::Aes256;
use aes::cipher::{BlockDecrypt, KeyInit as AesKeyInit, generic_array::GenericArray};
use agentx_runtime_contracts::{
    WebhookConversationV1, WebhookInputMappingV1, WebhookMessageV1, WebhookProviderV1,
    WebhookSenderV1, WebhookTriggerContextV1,
};
use axum::http::HeaderMap;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use hmac::{Hmac, Mac};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Map, Value, json};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::Sha256;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum WebhookDecode {
    Challenge(Value),
    Ignore,
    Event {
        context: WebhookTriggerContextV1,
        input: Value,
    },
}

#[allow(clippy::too_many_arguments)]
pub fn decode(
    provider: WebhookProviderV1,
    trigger_id: Uuid,
    connection_id: String,
    secret: &[u8],
    headers: &HeaderMap,
    query: &HashMap<String, String>,
    body: &Bytes,
    mappings: &[WebhookInputMappingV1],
    fixed_inputs: &Value,
) -> Result<WebhookDecode, &'static str> {
    let credentials = credentials(secret);
    let (payload, challenge) = decode_payload(provider, &credentials, query, body)?;
    if provider != WebhookProviderV1::Agentx {
        verify_provider_signature(provider, &credentials, headers, query, body, &payload)?;
    }
    if challenge {
        return Ok(WebhookDecode::Challenge(challenge_response(
            provider, &payload,
        )));
    }
    if !is_text_message(provider, &payload) {
        return Ok(WebhookDecode::Ignore);
    }
    let context = normalize(provider, trigger_id, connection_id, &payload)?;
    let input = map_input(&context, &payload, mappings, fixed_inputs)?;
    Ok(WebhookDecode::Event { context, input })
}

pub(crate) fn credentials(secret: &[u8]) -> Map<String, Value> {
    serde_json::from_slice::<Value>(secret)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_else(|| {
            let mut result = Map::new();
            result.insert(
                "token".into(),
                Value::String(String::from_utf8_lossy(secret).into_owned()),
            );
            result
        })
}
fn credential_string(credentials: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        credentials
            .get(*name)
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

fn decode_payload(
    provider: WebhookProviderV1,
    credentials: &Map<String, Value>,
    query: &HashMap<String, String>,
    body: &Bytes,
) -> Result<(Value, bool), &'static str> {
    if provider == WebhookProviderV1::Wecom && !body.trim_ascii_start().starts_with(b"{") {
        if let Some(echostr) = query.get("echostr") {
            let value = String::from_utf8(decrypt_wecom(echostr, credentials)?)
                .map_err(|_| "INVALID_ENCRYPTED_PAYLOAD")?;
            return Ok((json!({"echostr": value}), true));
        }
        let encrypt = xml_tag(body).ok_or("INVALID_ENCRYPTED_PAYLOAD")?;
        return parse_wecom_payload(&decrypt_wecom(&encrypt, credentials)?)
            .map(|value| (value, false));
    }
    let mut payload: Value = serde_json::from_slice(body).map_err(|_| "INVALID_JSON")?;
    if provider == WebhookProviderV1::Wecom {
        if let Some(encrypted) = payload.get("encrypt").and_then(Value::as_str) {
            payload = parse_wecom_payload(&decrypt_wecom(encrypted, credentials)?)?;
        }
    }
    if provider == WebhookProviderV1::Dingtalk {
        if let Some(encrypted) = payload.get("encrypt").and_then(Value::as_str) {
            payload = serde_json::from_slice(&decrypt_dingtalk(encrypted, credentials)?)
                .map_err(|_| "INVALID_JSON")?;
        }
    }
    if provider == WebhookProviderV1::Feishu {
        if let Some(encrypted) = payload.get("encrypt").and_then(Value::as_str) {
            payload = serde_json::from_slice(&decrypt_feishu(encrypted, credentials)?)
                .map_err(|_| "INVALID_JSON")?;
        }
    }
    let challenge = payload.get("challenge").is_some()
        || matches!(
            payload.get("type").and_then(Value::as_str),
            Some("url_verification" | "url_verification_v1")
        )
        || query.contains_key("echostr");
    Ok((payload, challenge))
}

fn verify_provider_signature(
    provider: WebhookProviderV1,
    credentials: &Map<String, Value>,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
    body: &[u8],
    payload: &Value,
) -> Result<(), &'static str> {
    match provider {
        WebhookProviderV1::Dingtalk => {
            let timestamp = header_or_query(headers, query, &["timestamp", "x-dingtalk-timestamp"])
                .ok_or("UNAUTHORIZED")?;
            validate_timestamp(&timestamp)?;
            let supplied = header_or_query(headers, query, &["sign", "x-dingtalk-signature"])
                .ok_or("UNAUTHORIZED")?;
            let secret = credential_string(credentials, &["secret", "appSecret", "token"])
                .ok_or("UNAUTHORIZED")?;
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
                .map_err(|_| "UNAUTHORIZED")?;
            mac.update(timestamp.as_bytes());
            mac.update(b"\n");
            mac.update(secret.as_bytes());
            let expected = STANDARD.encode(mac.finalize().into_bytes());
            if supplied != expected && !hmac_body_match(&secret, &timestamp, body, &supplied) {
                return Err("UNAUTHORIZED");
            }
        }
        WebhookProviderV1::Wecom => {
            let token = credential_string(credentials, &["token", "verificationToken"])
                .ok_or("UNAUTHORIZED")?;
            let timestamp =
                header_or_query(headers, query, &["timestamp"]).ok_or("UNAUTHORIZED")?;
            validate_timestamp(&timestamp)?;
            let nonce = header_or_query(headers, query, &["nonce"]).ok_or("UNAUTHORIZED")?;
            let supplied = header_or_query(headers, query, &["msg_signature", "signature"])
                .ok_or("UNAUTHORIZED")?;
            let encrypted = xml_tag(body)
                .or_else(|| {
                    serde_json::from_slice::<Value>(body)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("encrypt")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                })
                .or_else(|| query.get("echostr").cloned())
                .unwrap_or_default();
            let mut values = [
                token.as_str(),
                timestamp.as_str(),
                nonce.as_str(),
                encrypted.as_str(),
            ];
            values.sort_unstable();
            let mut digest = Sha1::new();
            for value in values {
                digest.update(value.as_bytes());
            }
            let valid = hex_lower(&digest.finalize()) == supplied || {
                let mut values = [
                    token.as_str(),
                    timestamp.as_str(),
                    nonce.as_str(),
                    std::str::from_utf8(body).unwrap_or(""),
                ];
                values.sort_unstable();
                let mut digest = Sha1::new();
                for value in values {
                    digest.update(value.as_bytes());
                }
                hex_lower(&digest.finalize()) == supplied
            };
            if !valid {
                return Err("UNAUTHORIZED");
            }
        }
        WebhookProviderV1::Feishu => {
            if let Some(token) = credential_string(credentials, &["verificationToken", "token"]) {
                let received = payload
                    .get("token")
                    .and_then(Value::as_str)
                    .or_else(|| payload.pointer("/header/token").and_then(Value::as_str));
                if received != Some(token.as_str()) {
                    return Err("UNAUTHORIZED");
                }
            }
            if payload.get("challenge").is_some() && payload.get("token").is_some() {
                return Ok(());
            }
            let timestamp =
                header_or_query(headers, query, &["x-lark-request-timestamp", "timestamp"])
                    .ok_or("UNAUTHORIZED")?;
            validate_timestamp(&timestamp)?;
            let nonce = header_or_query(headers, query, &["x-lark-request-nonce", "nonce"])
                .ok_or("UNAUTHORIZED")?;
            let supplied =
                header_or_query(headers, query, &["x-lark-signature", "x-feishu-signature"])
                    .ok_or("UNAUTHORIZED")?;
            let secret =
                credential_string(credentials, &["encryptKey", "verificationToken", "token"])
                    .ok_or("UNAUTHORIZED")?;
            let mut digest = Sha256::new();
            digest.update(timestamp.as_bytes());
            digest.update(nonce.as_bytes());
            digest.update(secret.as_bytes());
            digest.update(body);
            if hex_lower(&digest.finalize()) != supplied {
                return Err("UNAUTHORIZED");
            }
        }
        WebhookProviderV1::Agentx => {}
    }
    Ok(())
}
fn hmac_body_match(secret: &str, timestamp: &str, body: &[u8], supplied: &str) -> bool {
    let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    STANDARD
        .decode(supplied)
        .map(|bytes| bytes == mac.finalize().into_bytes().as_slice())
        .unwrap_or(false)
}

pub(crate) fn normalize(
    provider: WebhookProviderV1,
    trigger_id: Uuid,
    connection_id: String,
    payload: &Value,
) -> Result<WebhookTriggerContextV1, &'static str> {
    let event_id =
        first_string(payload, event_paths(provider)).ok_or("PROVIDER_EVENT_ID_REQUIRED")?;
    let conversation_id = first_string(payload, conversation_paths(provider))
        .or_else(|| first_string(payload, sender_paths(provider)).map(|id| format!("direct:{id}")))
        .ok_or("PROVIDER_CONVERSATION_ID_REQUIRED")?;
    let sender_id =
        first_string(payload, sender_paths(provider)).unwrap_or_else(|| "unknown".into());
    let text = message_text(provider, payload).unwrap_or_default();
    let conversation_type = explicit_conversation_type(provider, payload).unwrap_or_else(|| {
        if conversation_id.starts_with("direct:") {
            "direct"
        } else {
            "group"
        }
    });
    let session_webhook = if provider == WebhookProviderV1::Dingtalk {
        first_string(payload, &["sessionWebhook"])
    } else {
        None
    };
    let session_webhook_expires_at = if provider == WebhookProviderV1::Dingtalk {
        payload
            .get("sessionWebhookExpiredTime")
            .and_then(Value::as_i64)
    } else {
        None
    };
    Ok(WebhookTriggerContextV1 {
        provider,
        provider_connection_id: connection_id,
        webhook_trigger_id: trigger_id,
        provider_event_id: event_id,
        conversation: WebhookConversationV1 {
            id: conversation_id,
            name: first_string(
                payload,
                &[
                    "conversationName",
                    "conversationTitle",
                    "chat_name",
                    "event.message.chat_name",
                ],
            ),
            conversation_type: conversation_type.into(),
        },
        sender: WebhookSenderV1 {
            id: sender_id,
            name: first_string(payload, &["senderNick"]),
        },
        message: WebhookMessageV1 { text },
        session_webhook,
        session_webhook_expires_at,
    })
}

/// Platform-explicit conversation type, normalized to direct/group:
/// DingTalk conversationType ("1"/"2"), Feishu chat_type ("p2p"/"group"),
/// WeCom chattype ("single"/"group"). Agentx has no explicit marker.
fn explicit_conversation_type(
    provider: WebhookProviderV1,
    payload: &Value,
) -> Option<&'static str> {
    let raw = match provider {
        WebhookProviderV1::Dingtalk => first_string(payload, &["conversationType"])?,
        WebhookProviderV1::Feishu => {
            first_string(payload, &["event.message.chat_type", "chat_type"])?
        }
        WebhookProviderV1::Wecom => first_string(payload, &["chattype", "chatType"])?,
        WebhookProviderV1::Agentx => return None,
    };
    match (provider, raw.as_str()) {
        (WebhookProviderV1::Dingtalk, "1")
        | (WebhookProviderV1::Feishu, "p2p")
        | (WebhookProviderV1::Wecom, "single") => Some("direct"),
        (WebhookProviderV1::Dingtalk, "2")
        | (WebhookProviderV1::Feishu, "group")
        | (WebhookProviderV1::Wecom, "group") => Some("group"),
        _ => None,
    }
}
fn event_paths(provider: WebhookProviderV1) -> &'static [&'static str] {
    match provider {
        WebhookProviderV1::Dingtalk => &["msgId", "eventId"],
        WebhookProviderV1::Wecom => &["msgid", "event_id", "eventId"],
        WebhookProviderV1::Feishu => &["header.event_id", "event_id", "eventId"],
        WebhookProviderV1::Agentx => &["eventId", "event_id"],
    }
}
fn conversation_paths(provider: WebhookProviderV1) -> &'static [&'static str] {
    match provider {
        WebhookProviderV1::Dingtalk => &["conversationId"],
        WebhookProviderV1::Wecom => &["chatid", "conversation_id", "conversationId"],
        WebhookProviderV1::Feishu => {
            &["event.message.chat_id", "conversation_id", "conversationId"]
        }
        WebhookProviderV1::Agentx => &["conversationId", "conversation_id"],
    }
}
fn sender_paths(provider: WebhookProviderV1) -> &'static [&'static str] {
    match provider {
        WebhookProviderV1::Dingtalk => &["senderStaffId", "senderId"],
        WebhookProviderV1::Wecom => &["from.userid", "sender_id", "senderId"],
        WebhookProviderV1::Feishu => &[
            "event.sender.sender_id.open_id",
            "event.sender.sender_id.user_id",
            "sender_id",
        ],
        WebhookProviderV1::Agentx => &["sender_id", "senderId"],
    }
}
fn message_text(provider: WebhookProviderV1, payload: &Value) -> Option<String> {
    if provider == WebhookProviderV1::Dingtalk {
        // Regular robot callbacks use the `text.content` object; session
        // webhook callbacks carry a JSON-encoded `content` string instead.
        return first_string(payload, &["text.content", "text"])
            .or_else(|| dingtalk_session_content_text(payload));
    }
    let value = first_string(
        payload,
        match provider {
            WebhookProviderV1::Dingtalk => &["text.content", "text"],
            WebhookProviderV1::Wecom => &["text.content", "content", "message.text"],
            WebhookProviderV1::Feishu => &["event.message.content", "message.text", "text"],
            WebhookProviderV1::Agentx => &["message.text", "text", "content"],
        },
    )?;
    if provider == WebhookProviderV1::Feishu && value.trim_start().starts_with('{') {
        serde_json::from_str::<Value>(&value)
            .ok()
            .and_then(|v| v.get("text").and_then(Value::as_str).map(str::to_owned))
    } else {
        Some(value)
    }
}

/// DingTalk session-webhook robot callbacks put the message body in a
/// JSON-encoded `content` field (`"{\"content\":\"hi\"}"`).
fn dingtalk_session_content_text(payload: &Value) -> Option<String> {
    let value = first_string(payload, &["content"])?;
    if !value.trim_start().starts_with('{') {
        return None;
    }
    serde_json::from_str::<Value>(&value)
        .ok()
        .and_then(|v| v.get("content").and_then(Value::as_str).map(str::to_owned))
}
pub(crate) fn is_text_message(provider: WebhookProviderV1, payload: &Value) -> bool {
    if provider == WebhookProviderV1::Dingtalk {
        return payload
            .get("msgtype")
            .and_then(Value::as_str)
            .map(|v| v == "text")
            .unwrap_or(payload.get("text").is_some());
    }
    if provider == WebhookProviderV1::Feishu {
        return payload
            .get("event")
            .and_then(|v| v.get("message"))
            .and_then(|v| v.get("message_type"))
            .and_then(Value::as_str)
            .map(|v| v == "text")
            .unwrap_or(message_text(provider, payload).is_some());
    }
    payload
        .get("msgtype")
        .and_then(Value::as_str)
        .map(|v| v == "text")
        .unwrap_or(message_text(provider, payload).is_some())
}

pub(crate) fn map_input(
    context: &WebhookTriggerContextV1,
    payload: &Value,
    mappings: &[WebhookInputMappingV1],
    fixed_inputs: &Value,
) -> Result<Value, &'static str> {
    let mut output = Map::new();
    for mapping in mappings {
        let value = match mapping.source.as_str() {
            "message.text" => json!(context.message.text),
            "sender.id" => json!(context.sender.id),
            "sender.name" => context
                .sender
                .name
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
            "conversation.id" => json!(context.conversation.id),
            "conversation.name" => context
                .conversation
                .name
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
            "conversation.type" => json!(context.conversation.conversation_type),
            "provider" => {
                serde_json::to_value(context.provider).map_err(|_| "INVALID_WEBHOOK_MAPPING")?
            }
            "provider_event_id" => json!(context.provider_event_id),
            // raw.<dotted.path> passes any decrypted platform payload field
            // through as-is, so providers can expose fields beyond the
            // standardized Trigger Context without a contract change.
            source if source.starts_with("raw.") && source.len() > 4 => {
                raw_payload_value(payload, &source["raw.".len()..])
            }
            _ => return Err("INVALID_WEBHOOK_SOURCE"),
        };
        if value.is_null() && mapping.missing_policy == "error" {
            return Err("WEBHOOK_REQUIRED_SOURCE_MISSING");
        }
        if output.insert(mapping.target.clone(), value).is_some() {
            return Err("WEBHOOK_MAPPING_CONFLICT");
        }
    }
    if let Some(fixed) = fixed_inputs.as_object() {
        for (key, value) in fixed {
            if output.contains_key(key)
                || ["conversation_id", "sender_id", "provider_event_id"].contains(&key.as_str())
            {
                return Err("WEBHOOK_MAPPING_CONFLICT");
            }
            output.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(output))
}

fn raw_payload_value(payload: &Value, dotted_path: &str) -> Value {
    let pointer = format!("/{}", dotted_path.replace('.', "/"));
    payload.pointer(&pointer).cloned().unwrap_or(Value::Null)
}
/// Shared stream-side dispatch context: normalizes a provider event payload,
/// applies input mappings and creates the Invocation. Text-message gating and
/// failures are logged, never propagated to the connection loop.
pub(crate) struct DispatchState {
    pub(crate) pool: sqlx::MySqlPool,
    pub(crate) tenant_id: Uuid,
    pub(crate) application_id: Uuid,
    pub(crate) binding_id: Uuid,
    pub(crate) connection_id: String,
    pub(crate) trigger_name: String,
    pub(crate) configuration_revision: u64,
    pub(crate) input_mappings: Vec<WebhookInputMappingV1>,
    pub(crate) fixed_inputs: Value,
    pub(crate) provider: WebhookProviderV1,
}

impl DispatchState {
    pub(crate) async fn dispatch(&self, payload: Value) {
        if !is_text_message(self.provider, &payload) {
            return;
        }
        let result = normalize(
            self.provider,
            self.binding_id,
            self.connection_id.clone(),
            &payload,
        )
        .and_then(|context| {
            map_input(&context, &payload, &self.input_mappings, &self.fixed_inputs)
                .map(|input| (context, input))
        });
        match result {
            Ok((context, input)) => {
                if let Err(error) = dispatch_event(
                    &self.pool,
                    self.tenant_id,
                    self.application_id,
                    self.binding_id,
                    self.trigger_name.clone(),
                    self.configuration_revision,
                    context,
                    input,
                )
                .await
                {
                    tracing::warn!(%error, binding = %self.binding_id, "provider stream event dispatch failed");
                }
            }
            Err(code) => {
                tracing::warn!(code, binding = %self.binding_id, "provider stream event was rejected")
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(provider: WebhookProviderV1) -> Self {
        Self {
            pool: sqlx::MySqlPool::connect_lazy("mysql://agentx:agentx@127.0.0.1:1/agentx")
                .unwrap(),
            tenant_id: Uuid::nil(),
            application_id: Uuid::nil(),
            binding_id: Uuid::nil(),
            connection_id: "test".into(),
            trigger_name: "test".into(),
            configuration_revision: 1,
            input_mappings: Vec::new(),
            fixed_inputs: json!({}),
            provider,
        }
    }
}

fn challenge_response(provider: WebhookProviderV1, payload: &Value) -> Value {
    if provider == WebhookProviderV1::Feishu {
        json!({"challenge": payload.get("challenge").cloned().unwrap_or(Value::Null)})
    } else if provider == WebhookProviderV1::Wecom {
        payload.get("echostr").cloned().unwrap_or(Value::Null)
    } else if let Some(challenge) = payload.get("challenge") {
        json!({"challenge":challenge})
    } else {
        json!({"success":true})
    }
}

/// Shared provider-event dispatch used by both the HTTP callback gateway and
/// the stream connectors: idempotent Invocation creation plus Trigger Context snapshot.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_event(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    application_id: Uuid,
    binding_id: Uuid,
    trigger_name: String,
    configuration_revision: u64,
    context: WebhookTriggerContextV1,
    input: Value,
) -> Result<Uuid, crate::error::RuntimeError> {
    let raw_event_key = format!("provider:{binding_id}:{}", context.provider_event_id);
    let event_key = if raw_event_key.len() <= 120 {
        raw_event_key
    } else {
        format!("provider:{:x}", Sha256::digest(raw_event_key.as_bytes()))
    };
    let origin = agentx_runtime_contracts::ExecutionOriginV1 {
        trigger_source_id: Some(binding_id),
        trigger_name: Some(trigger_name),
        ..agentx_runtime_contracts::ExecutionOriginV1::system(None)
    };
    let accepted = crate::execution::create_runtime_invocation(
        pool,
        tenant_id,
        application_id,
        crate::execution::InvocationCaller {
            caller_type: "webhook",
            caller_id: binding_id,
            token_version: None,
            origin: origin.clone(),
        },
        None,
        &input,
        &event_key,
    )
    .await?;
    let mut context_value = serde_json::to_value(&context)
        .map_err(|e| crate::error::RuntimeError::Internal(e.into()))?;
    if let Some(object) = context_value.as_object_mut() {
        object.insert(
            "configurationRevision".into(),
            json!(configuration_revision),
        );
    }
    sqlx::query("UPDATE application_invocations SET provider_event_id=?,conversation_id=?,trigger_context_json=? WHERE tenant_id=? AND id=?")
        .bind(&context.provider_event_id).bind(&context.conversation.id).bind(context_value).bind(tenant_id).bind(accepted.invocation_id).execute(pool).await?;
    Ok(accepted.invocation_id)
}

fn decrypt_wecom(encoded: &str, credentials: &Map<String, Value>) -> Result<Vec<u8>, &'static str> {
    let key = credential_string(credentials, &["aesKey", "encodingAESKey"])
        .ok_or("INVALID_ENCRYPTION_KEY")?;
    let mut key_bytes = STANDARD
        .decode(format!("{}=", key.trim_end_matches('=')))
        .map_err(|_| "INVALID_ENCRYPTION_KEY")?;
    if key_bytes.len() != 32 {
        return Err("INVALID_ENCRYPTION_KEY");
    }
    let iv = key_bytes[..16].to_vec();
    let bytes = decrypt_cbc(&mut key_bytes, &iv, encoded)?;
    if bytes.len() >= 20 {
        let length = u32::from_be_bytes(bytes[16..20].try_into().unwrap()) as usize;
        if 20 + length <= bytes.len() {
            return Ok(bytes[20..20 + length].to_vec());
        }
    }
    Ok(bytes)
}
fn decrypt_feishu(
    encoded: &str,
    credentials: &Map<String, Value>,
) -> Result<Vec<u8>, &'static str> {
    let key = credential_string(credentials, &["encryptKey"]).ok_or("INVALID_ENCRYPTION_KEY")?;
    let digest = Sha256::digest(key.as_bytes());
    decrypt_cbc(&mut digest.to_vec(), &digest[..16], encoded)
}
fn decrypt_dingtalk(
    encoded: &str,
    credentials: &Map<String, Value>,
) -> Result<Vec<u8>, &'static str> {
    let key = credential_string(credentials, &["aesKey", "encodingAESKey", "encryptKey"])
        .ok_or("INVALID_ENCRYPTION_KEY")?;
    let mut key_bytes = STANDARD
        .decode(format!("{}=", key.trim_end_matches('=')).as_bytes())
        .map_err(|_| "INVALID_ENCRYPTION_KEY")?;
    if key_bytes.len() != 32 {
        key_bytes = Sha256::digest(key.as_bytes()).to_vec();
    }
    let iv = key_bytes[..16].to_vec();
    decrypt_cbc(&mut key_bytes, &iv, encoded)
}
fn parse_wecom_payload(bytes: &[u8]) -> Result<Value, &'static str> {
    if let Ok(value) = serde_json::from_slice(bytes) {
        return Ok(value);
    }
    let event_id = xml_tag_named(bytes, "MsgId")
        .or_else(|| xml_tag_named(bytes, "EventKey"))
        .ok_or("INVALID_JSON")?;
    let conversation_id = xml_tag_named(bytes, "ChatId").ok_or("INVALID_JSON")?;
    let sender_id = xml_tag_named(bytes, "FromUserName").unwrap_or_else(|| "unknown".into());
    let msg_type = xml_tag_named(bytes, "MsgType").unwrap_or_else(|| "text".into());
    let content = xml_tag_named(bytes, "Content").unwrap_or_default();
    let chat_type = xml_tag_named(bytes, "ChatType");
    let create_time = xml_tag_named(bytes, "CreateTime");
    let agent_id = xml_tag_named(bytes, "AgentID");
    let to_user = xml_tag_named(bytes, "ToUserName");
    Ok(
        json!({"msgid": event_id, "chatid": conversation_id, "from": {"userid": sender_id}, "msgtype": msg_type, "text": {"content": content}, "chattype": chat_type, "createTime": create_time, "agentId": agent_id, "toUserName": to_user}),
    )
}
fn decrypt_cbc(key: &mut [u8], iv: &[u8], encoded: &str) -> Result<Vec<u8>, &'static str> {
    let mut bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "INVALID_ENCRYPTED_PAYLOAD")?;
    if bytes.len() % 16 != 0 || key.len() != 32 {
        return Err("INVALID_ENCRYPTED_PAYLOAD");
    }
    let cipher = Aes256::new_from_slice(key).map_err(|_| "INVALID_ENCRYPTED_PAYLOAD")?;
    let mut previous = iv.to_vec();
    for chunk in bytes.chunks_mut(16) {
        let original = chunk.to_vec();
        cipher.decrypt_block(GenericArray::from_mut_slice(chunk));
        for (index, byte) in chunk.iter_mut().enumerate() {
            *byte ^= previous[index];
        }
        previous = original;
    }
    let padding = *bytes.last().ok_or("INVALID_ENCRYPTED_PAYLOAD")? as usize;
    if padding == 0
        || padding > 32
        || bytes.len() < padding
        || !bytes[bytes.len() - padding..]
            .iter()
            .all(|byte| usize::from(*byte) == padding)
    {
        return Err("INVALID_ENCRYPTED_PAYLOAD");
    }
    bytes.truncate(bytes.len() - padding);
    Ok(bytes)
}
fn xml_tag(body: &[u8]) -> Option<String> {
    xml_tag_named(body, "Encrypt")
}
fn xml_tag_named(body: &[u8], tag: &str) -> Option<String> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut active = false;
    loop {
        match reader.read_event_into(&mut buffer).ok()? {
            Event::Start(event) if event.name().as_ref() == tag.as_bytes() => active = true,
            Event::Text(text) if active => {
                return text.unescape().ok().map(|value| value.into_owned());
            }
            Event::End(event) if event.name().as_ref() == tag.as_bytes() => active = false,
            Event::Eof => return None,
            _ => {}
        }
        buffer.clear();
    }
}
fn first_string(payload: &Value, paths: &[&str]) -> Option<String> {
    paths.iter().find_map(|path| {
        payload
            .pointer(&format!("/{path}").replace('.', "/"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                payload
                    .get(*path)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
    })
}
fn header_or_query(
    headers: &HeaderMap,
    query: &HashMap<String, String>,
    names: &[&str],
) -> Option<String> {
    names.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .or_else(|| query.get(*name).cloned())
    })
}
fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn validate_timestamp(value: &str) -> Result<(), &'static str> {
    let timestamp = value.parse::<i64>().map_err(|_| "UNAUTHORIZED")?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let timestamp = if timestamp.abs() > 100_000_000_000 {
        timestamp / 1_000
    } else {
        timestamp
    };
    if (now - timestamp).abs() > 300 {
        return Err("UNAUTHORIZED");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    fn mapping(source: &str, target: &str) -> WebhookInputMappingV1 {
        WebhookInputMappingV1 {
            source: source.into(),
            target: target.into(),
            missing_policy: "error".into(),
        }
    }
    #[test]
    fn fixed_input_cannot_override_source() {
        let context = WebhookTriggerContextV1 {
            provider: WebhookProviderV1::Feishu,
            provider_connection_id: "c".into(),
            webhook_trigger_id: Uuid::nil(),
            provider_event_id: "e".into(),
            conversation: WebhookConversationV1 {
                id: "chat".into(),
                name: None,
                conversation_type: "group".into(),
            },
            sender: WebhookSenderV1 {
                id: "u".into(),
                name: None,
            },
            message: WebhookMessageV1 { text: "x".into() },
            session_webhook: None,
            session_webhook_expires_at: None,
        };
        assert_eq!(
            map_input(
                &context,
                &json!({}),
                &[mapping("conversation.id", "conversation_id")],
                &json!({"conversation_id":"bad"})
            ),
            Err("WEBHOOK_MAPPING_CONFLICT")
        );
    }
    #[test]
    fn dingtalk_session_webhook_content_is_decoded_to_text() {
        let payload = json!({"msgId":"event-s","conversationId":"chat-s","conversationType":"1","senderStaffId":"user-s","msgtype":"text","content":"{\"content\":\"hello delivery\"}","sessionWebhook":"https://oapi.dingtalk.com/robot/sendBySession?session=x"});
        let context = normalize(
            WebhookProviderV1::Dingtalk,
            Uuid::nil(),
            "c".into(),
            &payload,
        )
        .unwrap();
        assert_eq!(context.message.text, "hello delivery");
        assert_eq!(
            context.session_webhook.as_deref(),
            Some("https://oapi.dingtalk.com/robot/sendBySession?session=x")
        );
    }
    #[test]
    fn dingtalk_group_name_and_sender_nick_are_normalized() {
        let payload = json!({"msgId":"event-n","conversationId":"chat-n","conversationType":"2","conversationTitle":"客服一群","senderStaffId":"user-n","senderNick":"张三","msgtype":"text","text":{"content":"hi"}});
        let context = normalize(
            WebhookProviderV1::Dingtalk,
            Uuid::nil(),
            "c".into(),
            &payload,
        )
        .unwrap();
        assert_eq!(context.conversation.name.as_deref(), Some("客服一群"));
        assert_eq!(context.conversation.conversation_type, "group");
        assert_eq!(context.sender.name.as_deref(), Some("张三"));
    }
    #[test]
    fn dingtalk_direct_chat_type_is_detected_from_explicit_marker() {
        let payload = json!({"msgId":"event-d","conversationId":"chat-d","conversationType":"1","senderStaffId":"user-d","msgtype":"text","text":{"content":"hi"}});
        let context = normalize(
            WebhookProviderV1::Dingtalk,
            Uuid::nil(),
            "c".into(),
            &payload,
        )
        .unwrap();
        assert_eq!(context.conversation.conversation_type, "direct");
    }
    #[test]
    fn feishu_and_wecom_explicit_chat_types_are_normalized() {
        let feishu = json!({"header":{"event_id":"e-f"},"event":{"sender":{"sender_id":{"open_id":"u"}},"message":{"chat_id":"c","chat_type":"p2p","message_type":"text","content":"{\"text\":\"hi\"}"}}});
        assert_eq!(
            normalize(WebhookProviderV1::Feishu, Uuid::nil(), "c".into(), &feishu)
                .unwrap()
                .conversation
                .conversation_type,
            "direct"
        );
        let wecom = json!({"msgid":"e-w","chatid":"chat-w","chattype":"group","from":{"userid":"u"},"msgtype":"text","text":{"content":"hi"}});
        assert_eq!(
            normalize(WebhookProviderV1::Wecom, Uuid::nil(), "c".into(), &wecom)
                .unwrap()
                .conversation
                .conversation_type,
            "group"
        );
    }
    #[test]
    fn raw_source_paths_pass_platform_payload_fields_through() {
        let context = WebhookTriggerContextV1 {
            provider: WebhookProviderV1::Dingtalk,
            provider_connection_id: "c".into(),
            webhook_trigger_id: Uuid::nil(),
            provider_event_id: "e".into(),
            conversation: WebhookConversationV1 {
                id: "chat".into(),
                name: None,
                conversation_type: "group".into(),
            },
            sender: WebhookSenderV1 {
                id: "u".into(),
                name: None,
            },
            message: WebhookMessageV1 { text: "x".into() },
            session_webhook: None,
            session_webhook_expires_at: None,
        };
        let payload = json!({"msgId":"e","conversationId":"chat","createAt":1780000000000i64,"atUsers":[{"dingtalkId":"u1","nick":"@甲"}]});
        let input = map_input(
            &context,
            &payload,
            &[
                mapping("raw.createAt", "sent_at"),
                mapping("raw.atUsers.0.nick", "mentioned"),
                WebhookInputMappingV1 {
                    source: "raw.conversationTitle".into(),
                    target: "group_name".into(),
                    missing_policy: "null".into(),
                },
            ],
            &json!({}),
        )
        .unwrap();
        assert_eq!(
            input,
            json!({"sent_at":1780000000000i64,"mentioned":"@甲","group_name":null})
        );
        assert_eq!(
            map_input(
                &context,
                &payload,
                &[mapping("raw.missing", "x")],
                &json!({})
            ),
            Err("WEBHOOK_REQUIRED_SOURCE_MISSING")
        );
        assert_eq!(
            map_input(&context, &payload, &[mapping("raw.", "x")], &json!({})),
            Err("INVALID_WEBHOOK_SOURCE")
        );
    }
    #[test]
    fn dingtalk_session_webhook_is_captured_for_outbound_delivery() {
        let payload = json!({"msgId":"event-sw","conversationId":"chat-sw","senderStaffId":"user-sw","msgtype":"text","text":{"content":"hi"},"sessionWebhook":"https://oapi.dingtalk.com/robot/sendBySession/abc","sessionWebhookExpiredTime":1780000000000i64});
        let context = normalize(
            WebhookProviderV1::Dingtalk,
            Uuid::nil(),
            "c".into(),
            &payload,
        )
        .unwrap();
        assert_eq!(
            context.session_webhook.as_deref(),
            Some("https://oapi.dingtalk.com/robot/sendBySession/abc")
        );
        assert_eq!(context.session_webhook_expires_at, Some(1780000000000));
    }
    #[test]
    fn challenge_is_returned_without_execution() {
        let body = Bytes::from(r#"{"type":"url_verification","challenge":"c","token":"t"}"#);
        let result = decode(
            WebhookProviderV1::Feishu,
            Uuid::nil(),
            "c".into(),
            br#"{"verificationToken":"t","token":"t"}"#,
            &HeaderMap::new(),
            &HashMap::new(),
            &body,
            &[],
            &json!({}),
        );
        assert!(matches!(result, Ok(WebhookDecode::Challenge(_))));
    }
    #[test]
    fn dingtalk_event_is_verified_and_normalized() {
        let secret = "secret";
        let timestamp = OffsetDateTime::now_utc().unix_timestamp().to_string();
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(b"\n");
        mac.update(secret.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        let body = Bytes::from(
            r#"{"msgId":"event-1","conversationId":"chat-1","senderStaffId":"user-1","msgtype":"text","text":{"content":"hello"}}"#,
        );
        let mut query = HashMap::new();
        query.insert("timestamp".into(), timestamp);
        query.insert("sign".into(), signature);
        let result = decode(
            WebhookProviderV1::Dingtalk,
            Uuid::nil(),
            "credential".into(),
            br#"{"secret":"secret"}"#,
            &HeaderMap::new(),
            &query,
            &body,
            &[mapping("message.text", "question")],
            &json!({}),
        );
        match result {
            Ok(WebhookDecode::Event { context, input }) => {
                assert_eq!(context.provider_event_id, "event-1");
                assert_eq!(context.conversation.id, "chat-1");
                assert_eq!(input, json!({"question":"hello"}));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn wecom_event_is_verified_and_normalized() {
        let token = "wecom-token";
        let timestamp = OffsetDateTime::now_utc().unix_timestamp().to_string();
        let nonce = "nonce-1";
        let body = Bytes::from(
            r#"{"msgid":"event-2","chatid":"chat-2","from":{"userid":"user-2"},"msgtype":"text","text":{"content":"hello wecom"}}"#,
        );
        let mut values = [token, timestamp.as_str(), nonce, ""];
        values.sort_unstable();
        let mut digest = Sha1::new();
        for value in values {
            digest.update(value.as_bytes());
        }
        let mut query = HashMap::new();
        query.insert("timestamp".into(), timestamp);
        query.insert("nonce".into(), nonce.into());
        query.insert("msg_signature".into(), hex_lower(&digest.finalize()));
        let result = decode(
            WebhookProviderV1::Wecom,
            Uuid::nil(),
            "credential".into(),
            br#"{"token":"wecom-token"}"#,
            &HeaderMap::new(),
            &query,
            &body,
            &[
                mapping("message.text", "question"),
                mapping("conversation.id", "conversation_id"),
            ],
            &json!({}),
        );
        match result {
            Ok(WebhookDecode::Event { context, input }) => {
                assert_eq!(context.provider_event_id, "event-2");
                assert_eq!(context.conversation.id, "chat-2");
                assert_eq!(
                    input,
                    json!({"question":"hello wecom","conversation_id":"chat-2"})
                );
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn feishu_event_uses_header_signature_and_json_message_content() {
        let timestamp = OffsetDateTime::now_utc().unix_timestamp().to_string();
        let nonce = "nonce-2";
        let encrypt_key = "encrypt-key";
        let body = Bytes::from(
            r#"{"header":{"event_id":"event-3","token":"verify"},"event":{"sender":{"sender_id":{"open_id":"user-3"}},"message":{"chat_id":"chat-3","message_type":"text","content":"{\"text\":\"hello feishu\"}"}}}"#,
        );
        let mut digest = Sha256::new();
        digest.update(timestamp.as_bytes());
        digest.update(nonce.as_bytes());
        digest.update(encrypt_key.as_bytes());
        digest.update(&body);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-lark-request-timestamp",
            HeaderValue::from_str(&timestamp).unwrap(),
        );
        headers.insert("x-lark-request-nonce", HeaderValue::from_static("nonce-2"));
        headers.insert(
            "x-lark-signature",
            HeaderValue::from_str(&hex_lower(&digest.finalize())).unwrap(),
        );
        let result = decode(
            WebhookProviderV1::Feishu,
            Uuid::nil(),
            "credential".into(),
            br#"{"encryptKey":"encrypt-key","verificationToken":"verify"}"#,
            &headers,
            &HashMap::new(),
            &body,
            &[mapping("message.text", "question")],
            &json!({}),
        );
        match result {
            Ok(WebhookDecode::Event { context, input }) => {
                assert_eq!(context.provider_event_id, "event-3");
                assert_eq!(context.sender.id, "user-3");
                assert_eq!(input, json!({"question":"hello feishu"}));
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn invalid_provider_signature_is_rejected() {
        let body = Bytes::from(
            r#"{"msgId":"event-4","conversationId":"chat-4","senderStaffId":"user-4","msgtype":"text","text":{"content":"hello"}}"#,
        );
        let mut query = HashMap::new();
        query.insert(
            "timestamp".into(),
            OffsetDateTime::now_utc().unix_timestamp().to_string(),
        );
        query.insert("sign".into(), "invalid".into());
        assert!(matches!(
            decode(
                WebhookProviderV1::Dingtalk,
                Uuid::nil(),
                "credential".into(),
                br#"{"secret":"secret"}"#,
                &HeaderMap::new(),
                &query,
                &body,
                &[],
                &json!({})
            ),
            Err("UNAUTHORIZED")
        ));
    }
}
