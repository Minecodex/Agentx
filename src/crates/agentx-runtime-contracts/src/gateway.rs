use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionVersionPolicyV1 {
    Pinned,
    FollowDeployment,
    ManualUpgrade,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationRuntimePolicyV1 {
    pub session_version_policy: SessionVersionPolicyV1,
    pub synchronous_wait_seconds: u32,
    pub maximum_json_bytes: u64,
    pub maximum_multipart_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VaultSecretReferenceV1 {
    pub mount: String,
    pub path: String,
    pub key: String,
    pub version: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleMisfirePolicyV1 {
    Skip,
    FireOnce,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookProviderV1 {
    Agentx,
    Dingtalk,
    Wecom,
    Feishu,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookChannelModeV1 {
    Callback,
    Stream,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookInputMappingV1 {
    pub source: String,
    pub target: String,
    #[serde(default = "default_missing_policy")]
    pub missing_policy: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookTriggerContextV1 {
    pub provider: WebhookProviderV1,
    pub provider_connection_id: String,
    pub webhook_trigger_id: Uuid,
    pub provider_event_id: String,
    pub conversation: WebhookConversationV1,
    pub sender: WebhookSenderV1,
    pub message: WebhookMessageV1,
    #[serde(default)]
    pub session_webhook: Option<String>,
    #[serde(default)]
    pub session_webhook_expires_at: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookConversationV1 {
    pub id: String,
    pub name: Option<String>,
    pub conversation_type: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookSenderV1 {
    pub id: String,
    /// Sender display name (e.g. DingTalk senderNick); None when the
    /// platform message carries no nickname.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookMessageV1 {
    pub text: String,
}

/// Channel auto-reply configuration (plan7 P7-A L1): frozen into the trigger
/// revision alongside the rest of the channel configuration; the runtime
/// delivery loop enqueues an outbound reply when the execution reaches a
/// terminal state.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookReplyConfigV1 {
    pub enabled: bool,
    /// Name of the workflow output field whose value is replied (required
    /// when enabled; Control validates it against the deployment output
    /// schema before freezing).
    pub output_field: String,
    /// Optional template over the output; only output-field variables are
    /// allowed. When empty the raw output field value is replied as text.
    #[serde(default)]
    pub template: Option<String>,
}

fn default_missing_policy() -> String {
    "error".into()
}

fn default_agentx_provider() -> WebhookProviderV1 {
    WebhookProviderV1::Agentx
}

fn default_channel_mode() -> WebhookChannelModeV1 {
    WebhookChannelModeV1::Callback
}

fn default_fixed_inputs() -> Value {
    Value::Object(Default::default())
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeTriggerConfigurationV1 {
    Webhook {
        public_id: String,
        secret: VaultSecretReferenceV1,
        #[serde(default = "default_agentx_provider")]
        provider: WebhookProviderV1,
        #[serde(default = "default_channel_mode")]
        mode: WebhookChannelModeV1,
        #[serde(default)]
        input_mappings: Vec<WebhookInputMappingV1>,
        #[serde(default = "default_fixed_inputs")]
        fixed_inputs: Value,
        #[serde(default)]
        reply: Option<WebhookReplyConfigV1>,
    },
    Schedule {
        cron_expression: String,
        timezone: String,
        misfire_policy: ScheduleMisfirePolicyV1,
        grace_seconds: u32,
        input: Value,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeTriggerSpecV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub trigger_id: Uuid,
    pub trigger_name: String,
    pub application_id: Uuid,
    pub node_id: String,
    pub revision: u64,
    pub configuration_hash: crate::ContentHash,
    pub enabled: bool,
    pub configuration: RuntimeTriggerConfigurationV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeUserAdmissionV1 {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub user_name: String,
    pub department_id: Uuid,
    pub department_name: String,
    pub token_version: u64,
    pub enabled: bool,
    pub tenant_query_enabled: bool,
    pub role_assignments: Vec<crate::ExecutionRoleAssignmentV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeUserApplicationGrantV1 {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub application_id: Uuid,
    pub grant_version: u64,
    pub can_invoke: bool,
    pub can_query: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeUserWorkflowGrantV1 {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub workflow_id: Uuid,
    pub grant_version: u64,
    pub can_query: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionUpgradeCommandV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub idempotency_key: String,
    pub tenant_id: Uuid,
    pub session_id: Uuid,
    pub application_id: Uuid,
    pub expected_session_version: u64,
    pub target_bundle_id: Uuid,
    pub actor_user_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionUpgradeReceiptV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub session_id: Uuid,
    pub bundle_id: Uuid,
    pub session_version: u64,
    pub replayed: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSearchRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub application_id: Option<Uuid>,
    pub statuses: Vec<String>,
    pub after: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSummaryV1 {
    pub session_id: Uuid,
    pub application_id: Uuid,
    pub application_deployment_id: Uuid,
    pub workflow_version_id: Option<Uuid>,
    pub bundle_id: Option<Uuid>,
    pub version_policy: SessionVersionPolicyV1,
    pub external_user_id: Option<String>,
    pub title: Option<String>,
    pub status: String,
    pub version: u64,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSearchPageV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub items: Vec<SessionSummaryV1>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDetailV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub summary: SessionSummaryV1,
    pub external_user_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSessionRequestV1 {
    pub external_user_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionResponseV1 {
    pub id: Uuid,
    pub application_id: Uuid,
    pub application_deployment_id: Uuid,
    pub workflow_version_id: Option<Uuid>,
    pub bundle_id: Option<Uuid>,
    pub version_policy: SessionVersionPolicyV1,
    pub external_user_id: Option<String>,
    pub title: Option<String>,
    pub status: String,
    pub version: u64,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationRequestV1 {
    pub input: Value,
    pub session_id: Option<Uuid>,
    pub response_mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationResponseV1 {
    pub id: Uuid,
    pub application_id: Uuid,
    pub session_id: Option<Uuid>,
    pub execution_id: Option<Uuid>,
    pub bundle_id: Uuid,
    pub admission_epoch: u64,
    pub status: String,
    pub outputs: Option<Value>,
    pub error: Option<Value>,
    pub provider: Option<WebhookProviderV1>,
    pub provider_event_id: Option<String>,
    pub conversation_id: Option<String>,
    pub trigger_context: Option<Value>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatMappingV1 {
    pub question_input: String,
    pub file_input: Option<String>,
    pub answer_output: String,
    pub answer_files_output: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyChatMappingRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub idempotency_key: String,
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub deployment_id: Uuid,
    pub bundle_id: Uuid,
    pub version: u64,
    pub mapping: Option<ChatMappingV1>,
    pub content_hash: crate::ContentHash,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyChatMappingReceiptV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub deployment_id: Uuid,
    pub bundle_id: Uuid,
    pub version: u64,
    pub replayed: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessagePartInputV1 {
    pub part_type: String,
    pub content: Option<Value>,
    pub artifact_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageRequestV1 {
    pub parts: Vec<MessagePartInputV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageResponseV1 {
    pub id: Uuid,
    pub invocation_id: Option<Uuid>,
    pub sequence: u64,
    pub role: String,
    pub parts: Vec<MessagePartInputV1>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactUploadResponseV1 {
    pub artifact_id: Uuid,
    pub content_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandAcceptedV1 {
    pub accepted: bool,
    pub replayed: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayErrorV1 {
    pub code: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_trigger_configuration_defaults_mode_to_callback() {
        let legacy = serde_json::json!({"kind":"webhook","publicId":"pub","secret":{"mount":"secret","path":"tenants/t/webhooks/w","key":"value","version":1}});
        let RuntimeTriggerConfigurationV1::Webhook { mode, .. } =
            serde_json::from_value(legacy).unwrap()
        else {
            panic!("expected webhook variant")
        };
        assert_eq!(mode, WebhookChannelModeV1::Callback);
        let stream = serde_json::json!({"kind":"webhook","publicId":"pub","secret":{"mount":"secret","path":"tenants/t/webhooks/w","key":"value","version":1},"mode":"stream"});
        let RuntimeTriggerConfigurationV1::Webhook { mode, .. } =
            serde_json::from_value(stream).unwrap()
        else {
            panic!("expected webhook variant")
        };
        assert_eq!(mode, WebhookChannelModeV1::Stream);
    }

    #[test]
    fn webhook_trigger_context_tolerates_missing_session_webhook() {
        let context: WebhookTriggerContextV1 = serde_json::from_value(serde_json::json!({
            "provider":"dingtalk",
            "providerConnectionId":"binding",
            "webhookTriggerId":"00000000-0000-0000-0000-000000000000",
            "providerEventId":"event-1",
            "conversation":{"id":"chat","name":null,"conversationType":"group"},
            "sender":{"id":"user"},
            "message":{"text":"hello"}
        }))
        .unwrap();
        assert!(context.session_webhook.is_none());
        assert!(context.session_webhook_expires_at.is_none());
    }
}
