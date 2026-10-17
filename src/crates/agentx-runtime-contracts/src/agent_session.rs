use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionSearchRequestV1 {
    pub api_version: u32,
    pub tenant_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_agent_node_key: Option<String>,
    #[serde(default)]
    pub limit: u32,
    #[serde(default)]
    pub after: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionSummaryV1 {
    pub session_key: String,
    pub session_id: String,
    pub stable_agent_node_key: String,
    pub application_id: Option<Uuid>,
    pub state_version: u64,
    pub fencing_token: u64,
    pub leaf_entry_id: Option<String>,
    pub open_operation_id: Option<String>,
    pub terminal_state: Option<String>,
    pub bundle_hash: String,
    pub model_version: String,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionSearchPageV1 {
    pub api_version: u32,
    pub items: Vec<AgentSessionSummaryV1>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionEntryViewV1 {
    pub entry_id: String,
    pub session_id: String,
    pub lane: String,
    pub parent_entry_id: Option<String>,
    pub entry_kind: String,
    pub payload: Option<Value>,
    pub operation_id: Option<String>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionDiagnosticV1 {
    pub api_version: u32,
    pub summary: AgentSessionSummaryV1,
    pub entries: Vec<AgentSessionEntryViewV1>,
    pub usages: Vec<Value>,
    pub compaction: Option<Value>,
    pub recovery: Option<Value>,
    pub memory_targets: Vec<AgentMemoryTargetV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentMemoryTargetV1 {
    pub scope: crate::MemoryScopeV1,
    pub memory_resource_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionClearRequestV1 {
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub session_key: String,
    pub stable_agent_node_key: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionClearReceiptV1 {
    pub api_version: u32,
    pub session_key: String,
    pub stable_agent_node_key: String,
    pub cleared_entries: u64,
    pub cleared_pending: u64,
    pub audit_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSubjectMemoryClearRequestV1 {
    pub scope: crate::MemoryScopeV1,
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub memory_resource_id: Uuid,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSubjectMemoryClearReceiptV1 {
    pub api_version: u32,
    pub clear_id: Uuid,
    pub audit_id: Uuid,
    pub scope_hash: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSubjectMemorySearchRequestV1 {
    pub scope: crate::MemoryScopeV1,
    pub api_version: u32,
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub memory_resource_id: Uuid,
    #[serde(default)]
    pub limit: u32,
    #[serde(default)]
    pub after: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSubjectMemoryAuditViewV1 {
    pub audit_id: Uuid,
    pub operation: String,
    pub scope_hash: String,
    pub operation_id: Option<String>,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSubjectMemorySearchPageV1 {
    pub api_version: u32,
    pub items: Vec<AgentSubjectMemoryAuditViewV1>,
    pub next: Option<String>,
}
