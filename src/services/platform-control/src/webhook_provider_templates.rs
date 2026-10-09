use serde::Serialize;
use serde_json::{Map, Value};

use crate::api_error::{ApiError, ApiResult};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebhookProviderTemplateFieldV1 {
    pub(crate) key: String,
    pub(crate) sensitive: bool,
    pub(crate) required: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebhookProviderTemplateV1 {
    pub(crate) provider: String,
    pub(crate) mode: String,
    pub(crate) fields: Vec<WebhookProviderTemplateFieldV1>,
    /// Raw payload paths (`raw.<dotted.path>`) this provider's messages carry
    /// beyond the standardized Trigger Context fields, offered as mapping
    /// source suggestions. Identical for callback and stream modes.
    pub(crate) mapping_sources: Vec<String>,
}

fn field(key: &str, sensitive: bool, required: bool) -> WebhookProviderTemplateFieldV1 {
    WebhookProviderTemplateFieldV1 {
        key: key.into(),
        sensitive,
        required,
    }
}

fn mapping_source(path: &str) -> String {
    format!("raw.{path}")
}

/// Field keys must stay aligned with the Runtime provider adapters
/// (`credential_string` lookups in agentx-v2-runtime webhook/stream decoding).
pub(crate) fn templates() -> Vec<WebhookProviderTemplateV1> {
    vec![
        WebhookProviderTemplateV1 {
            provider: "agentx".into(),
            mode: "callback".into(),
            fields: vec![],
            mapping_sources: vec![],
        },
        WebhookProviderTemplateV1 {
            provider: "dingtalk".into(),
            mode: "callback".into(),
            fields: vec![
                field("secret", true, true),
                field("aesKey", true, false),
                field("robotCode", false, false),
            ],
            mapping_sources: dingtalk_mapping_sources(),
        },
        WebhookProviderTemplateV1 {
            provider: "dingtalk".into(),
            mode: "stream".into(),
            fields: vec![
                field("clientId", false, true),
                field("clientSecret", true, true),
                field("robotCode", false, false),
            ],
            mapping_sources: dingtalk_mapping_sources(),
        },
        WebhookProviderTemplateV1 {
            provider: "wecom".into(),
            mode: "callback".into(),
            fields: vec![
                field("token", true, true),
                field("encodingAESKey", true, true),
                field("corpId", false, false),
                field("corpSecret", true, false),
                field("agentId", false, false),
            ],
            mapping_sources: ["msgtype", "createTime", "agentId", "toUserName"]
                .iter()
                .map(|path| mapping_source(path))
                .collect(),
        },
        WebhookProviderTemplateV1 {
            provider: "feishu".into(),
            mode: "callback".into(),
            fields: vec![
                field("verificationToken", true, true),
                field("encryptKey", true, false),
            ],
            mapping_sources: feishu_mapping_sources(),
        },
        WebhookProviderTemplateV1 {
            provider: "feishu".into(),
            mode: "stream".into(),
            fields: vec![field("appId", false, true), field("appSecret", true, true)],
            mapping_sources: feishu_mapping_sources(),
        },
    ]
}

/// DingTalk robot message fields not covered by the standardized context
/// (conversationTitle/senderNick are available as conversation.name/sender.name).
fn dingtalk_mapping_sources() -> Vec<String> {
    ["senderCorpId", "senderId", "createAt", "msgtype", "atUsers"]
        .iter()
        .map(|path| mapping_source(path))
        .collect()
}

/// Feishu im.message.receive_v1 fields not covered by the standardized context.
fn feishu_mapping_sources() -> Vec<String> {
    [
        "event.message.message_id",
        "event.message.message_type",
        "event.message.create_time",
        "event.message.root_id",
        "event.message.parent_id",
        "event.message.thread_id",
        "event.sender.sender_type",
        "event.sender.sender_id.union_id",
        "event.sender.sender_id.user_id",
        "header.event_type",
        "header.tenant_key",
        "header.app_id",
    ]
    .iter()
    .map(|path| mapping_source(path))
    .collect()
}

pub(crate) fn find(provider: &str, mode: &str) -> Option<WebhookProviderTemplateV1> {
    templates()
        .into_iter()
        .find(|template| template.provider == provider && template.mode == mode)
}

/// Merge submitted channel fields with the previously stored secret values.
/// Returns the full field map (written to Vault) and the non-sensitive subset (stored in DB).
pub(crate) fn merge_fields(
    fields: &[WebhookProviderTemplateFieldV1],
    submitted: Option<&Value>,
    existing: Option<&Map<String, Value>>,
) -> ApiResult<(Map<String, Value>, Map<String, Value>)> {
    let submitted = submitted
        .filter(|value| !value.is_null())
        .and_then(Value::as_object)
        .ok_or_else(|| {
            ApiError::bad_request(
                "INVALID_WEBHOOK_CHANNEL_CONFIG",
                "Channel config must be an object",
            )
        })?
        .clone();
    for key in submitted.keys() {
        if !fields.iter().any(|field| &field.key == key) {
            return Err(ApiError::bad_request(
                "WEBHOOK_CHANNEL_FIELD_UNKNOWN",
                "Channel config contains an unknown field",
            ));
        }
    }
    let mut full = Map::new();
    let mut public = Map::new();
    for field in fields {
        let submitted_value = submitted
            .get(&field.key)
            .and_then(Value::as_str)
            .unwrap_or("");
        let value = if submitted_value.is_empty() {
            existing
                .and_then(|existing| existing.get(&field.key))
                .cloned()
        } else {
            Some(Value::String(submitted_value.to_owned()))
        };
        if field.required
            && value
                .as_ref()
                .and_then(Value::as_str)
                .map(str::is_empty)
                .unwrap_or(true)
        {
            return Err(ApiError::unprocessable(
                "WEBHOOK_CHANNEL_FIELD_REQUIRED",
                "Channel config is missing a required field",
            ));
        }
        if let Some(value) = value {
            if !field.sensitive {
                public.insert(field.key.clone(), value.clone());
            }
            full.insert(field.key.clone(), value);
        }
    }
    Ok((full, public))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_mode_matrix_matches_platform_capabilities() {
        assert!(find("agentx", "callback").is_some());
        assert!(find("wecom", "callback").is_some());
        assert!(find("dingtalk", "callback").is_some());
        assert!(find("dingtalk", "stream").is_some());
        assert!(find("feishu", "callback").is_some());
        assert!(find("feishu", "stream").is_some());
        assert!(
            find("agentx", "stream").is_none(),
            "agentx has no platform connection to dial"
        );
        assert!(
            find("wecom", "stream").is_none(),
            "WeCom has no reverse connection mode"
        );
    }

    #[test]
    fn mapping_sources_are_raw_paths_consistent_across_modes() {
        for template in templates() {
            for source in &template.mapping_sources {
                assert!(
                    source.starts_with("raw.") && source.len() > 4,
                    "catalog entries must be raw.* paths: {source}"
                );
            }
        }
        assert_eq!(
            find("dingtalk", "callback").unwrap().mapping_sources,
            find("dingtalk", "stream").unwrap().mapping_sources
        );
        assert_eq!(
            find("feishu", "callback").unwrap().mapping_sources,
            find("feishu", "stream").unwrap().mapping_sources
        );
    }

    #[test]
    fn blank_sensitive_input_keeps_the_stored_secret() {
        let fields = find("dingtalk", "callback").unwrap().fields;
        let existing = serde_json::json!({"secret": "stored-secret"})
            .as_object()
            .unwrap()
            .clone();
        let (full, public) = merge_fields(
            &fields,
            Some(&serde_json::json!({"secret":"", "aesKey":"new-aes"})),
            Some(&existing),
        )
        .unwrap();
        assert_eq!(full["secret"], "stored-secret");
        assert_eq!(full["aesKey"], "new-aes");
        assert!(
            public.is_empty(),
            "callback fields are all sensitive and must not echo back"
        );
    }

    #[test]
    fn merge_rejects_unknown_keys_and_missing_required_fields() {
        let fields = find("dingtalk", "stream").unwrap().fields;
        assert!(
            merge_fields(&fields, Some(&serde_json::json!({"unexpected":"1"})), None).is_err(),
            "unknown keys are rejected"
        );
        assert!(
            merge_fields(&fields, Some(&serde_json::json!({"clientId":"id"})), None).is_err(),
            "clientSecret is required"
        );
        let (full, public) = merge_fields(
            &fields,
            Some(&serde_json::json!({"clientId":"id", "clientSecret":"secret"})),
            None,
        )
        .unwrap();
        assert_eq!(public["clientId"], "id");
        assert!(
            !public.contains_key("clientSecret"),
            "sensitive values never reach the stored config echo"
        );
        assert_eq!(full["clientSecret"], "secret");
    }
}
