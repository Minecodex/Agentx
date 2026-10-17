//! Durable Agent Session contracts.
//!
//! These types deliberately contain only domain data.  Runtime adapters map
//! them to SQL rows, object artifacts and authenticated identity evidence.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CompactionKindV1, ReplayPolicyV1, SessionPolicyModeV1};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionIdentityV1 {
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<String>,
    pub session_id: String,
    pub stable_agent_node_key: String,
    pub execution_id: Option<String>,
    pub attempt_id: Option<String>,
}

impl AgentSessionIdentityV1 {
    pub fn application_session(
        tenant_id: impl Into<String>,
        application_id: impl Into<String>,
        trusted_application_session_id: impl Into<String>,
        stable_agent_node_key: impl Into<String>,
    ) -> Self {
        let application_id = application_id.into();
        let session_id = trusted_application_session_id.into();
        Self {
            tenant_id: tenant_id.into(),
            application_id: Some(application_id),
            session_id,
            stable_agent_node_key: stable_agent_node_key.into(),
            execution_id: None,
            attempt_id: None,
        }
    }

    pub fn invocation(
        tenant_id: impl Into<String>,
        execution_id: impl Into<String>,
        attempt_id: impl Into<String>,
        stable_agent_node_key: impl Into<String>,
    ) -> Self {
        let execution_id = execution_id.into();
        let attempt_id = attempt_id.into();
        Self {
            tenant_id: tenant_id.into(),
            application_id: None,
            session_id: format!("invocation:{execution_id}:{attempt_id}"),
            stable_agent_node_key: stable_agent_node_key.into(),
            execution_id: Some(execution_id),
            attempt_id: Some(attempt_id),
        }
    }

    pub fn validate(&self, mode: SessionPolicyModeV1) -> Result<(), &'static str> {
        if self.tenant_id.trim().is_empty()
            || self.session_id.trim().is_empty()
            || self.stable_agent_node_key.trim().is_empty()
        {
            return Err("AGENT_SESSION_IDENTITY_INVALID");
        }
        match mode {
            SessionPolicyModeV1::ApplicationSession if self.application_id.is_none() => {
                Err("AGENT_SESSION_REQUIRED")
            }
            SessionPolicyModeV1::ApplicationSession
                if self.execution_id.is_some() || self.attempt_id.is_some() =>
            {
                Err("AGENT_SESSION_IDENTITY_INVALID")
            }
            SessionPolicyModeV1::Invocation
                if self.execution_id.is_none() || self.attempt_id.is_none() =>
            {
                Err("AGENT_SESSION_IDENTITY_INVALID")
            }
            SessionPolicyModeV1::Invocation if self.application_id.is_some() => {
                Err("AGENT_SESSION_IDENTITY_INVALID")
            }
            SessionPolicyModeV1::Invocation
                if self.session_id
                    != format!(
                        "invocation:{}:{}",
                        self.execution_id.as_deref().unwrap_or_default(),
                        self.attempt_id.as_deref().unwrap_or_default()
                    ) =>
            {
                Err("AGENT_SESSION_IDENTITY_INVALID")
            }
            _ => Ok(()),
        }
    }

    pub fn isolation_key(&self, mode: SessionPolicyModeV1) -> Result<String, &'static str> {
        self.validate(mode)?;
        Ok(match mode {
            SessionPolicyModeV1::ApplicationSession => format!(
                "application:{}:{}:{}:{}",
                self.tenant_id,
                self.application_id.as_deref().unwrap_or_default(),
                self.session_id,
                self.stable_agent_node_key
            ),
            SessionPolicyModeV1::Invocation => format!(
                "invocation:{}:{}:{}:{}",
                self.tenant_id,
                self.execution_id.as_deref().unwrap_or_default(),
                self.attempt_id.as_deref().unwrap_or_default(),
                self.stable_agent_node_key
            ),
        })
    }
}

/// The only lane implemented by P3-05.  Keeping this as a closed enum avoids
/// silently mixing a future branch/tree implementation with the main lane.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionLaneV1 {
    #[default]
    Main,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionEntryKindV1 {
    MessageUser,
    MessageAssistant,
    MessageToolCall,
    MessageToolResult,
    CustomExternalContext,
    Compaction,
    PendingSteering,
    PendingFollowUp,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(schema_with = "agent_session_entry_schema")]
pub struct AgentSessionEntryV1 {
    pub entry_id: String,
    pub session_id: String,
    pub lane: AgentSessionLaneV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_entry_id: Option<String>,
    pub entry_kind: AgentSessionEntryKindV1,
    /// Inline payload is bounded by the Runtime adapter.  Larger payloads are
    /// represented by `payload_artifact_id` and never copied into Core state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_json: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub created_at_millis: u64,
}

/// The storage payload is deliberately represented as a tagged choice at the
/// wire boundary.  `Option<T>` by itself only communicates that both fields
/// are optional, which would allow an entry with no payload (or both storage
/// locations) through generated JSON Schema validation.  Runtime adapters
/// still repeat this check before writing SQL/object rows.
fn agent_session_entry_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "entryId": {"type": "string", "minLength": 1},
            "sessionId": {"type": "string", "minLength": 1},
            "lane": {"const": "main"},
            "parentEntryId": {"type": ["string", "null"]},
            "entryKind": {
                "enum": [
                    "message_user",
                    "message_assistant",
                    "message_tool_call",
                    "message_tool_result",
                    "custom_external_context",
                    "compaction",
                    "pending_steering",
                    "pending_follow_up"
                ]
            },
            "payloadJson": {},
            "payloadArtifactId": {"type": "string", "minLength": 1},
            "operationId": {"type": ["string", "null"]},
            "createdAtMillis": {"type": "integer", "minimum": 0}
        },
        "required": ["entryId", "sessionId", "lane", "entryKind", "createdAtMillis"],
        "oneOf": [
            {
                "required": ["payloadJson"],
                "not": {"required": ["payloadArtifactId"]}
            },
            {
                "required": ["payloadArtifactId"],
                "not": {"required": ["payloadJson"]}
            }
        ]
    })
}

impl AgentSessionEntryV1 {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.entry_id.trim().is_empty() || self.session_id.trim().is_empty() {
            return Err("AGENT_SESSION_ENTRY_INVALID");
        }
        if self.payload_json.is_some() == self.payload_artifact_id.is_some() {
            return Err("AGENT_SESSION_ENTRY_PAYLOAD_INVALID");
        }
        if self
            .payload_artifact_id
            .as_deref()
            .is_some_and(str::is_empty)
        {
            return Err("AGENT_SESSION_ENTRY_PAYLOAD_INVALID");
        }
        if self.parent_entry_id.as_deref() == Some(self.entry_id.as_str()) {
            return Err("AGENT_SESSION_ENTRY_PARENT_INVALID");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionRegisterV1 {
    pub session_id: String,
    pub lane: AgentSessionLaneV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_operation_id: Option<String>,
    pub state_version: u64,
    pub fencing_token: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_contract_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_state: Option<DurableOperationStateV1>,
    #[serde(default)]
    pub pending_entries: Vec<AgentSessionEntryV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionRegisterV1>,
    pub updated_at_millis: u64,
}

impl AgentSessionRegisterV1 {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.session_id.trim().is_empty() {
            return Err("AGENT_SESSION_REGISTER_INVALID");
        }
        match (&self.open_operation_id, &self.operation_state) {
            (None, Some(operation))
                if !matches!(operation.phase, DurableOperationPhaseV1::Terminal)
                    || operation.operation_id.trim().is_empty() =>
            {
                return Err("AGENT_SESSION_OPERATION_REGISTER_INVALID");
            }
            (Some(operation_id), Some(operation)) if operation_id != &operation.operation_id => {
                return Err("AGENT_SESSION_OPERATION_REGISTER_INVALID");
            }
            (Some(_), None) => return Err("AGENT_SESSION_OPERATION_REGISTER_INVALID"),
            _ => {}
        }
        if let Some(operation) = &self.operation_state {
            operation.validate()?;
        }
        if let Some(compaction) = &self.compaction {
            compaction.validate()?;
        }
        for entry in &self.pending_entries {
            entry.validate()?;
        }
        if self.pending_entries.len() > 32 {
            return Err("AGENT_SESSION_BUSY");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKindV1 {
    Model,
    Compaction,
    Tool,
    Memory,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionUsageV1 {
    pub usage_id: String,
    pub session_id: String,
    pub operation_id: String,
    pub effect_id: String,
    pub usage_kind: UsageKindV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_reference: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_micros: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_currency: Option<String>,
    pub created_at_millis: u64,
}

impl AgentSessionUsageV1 {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.usage_id.trim().is_empty()
            || self.session_id.trim().is_empty()
            || self.operation_id.trim().is_empty()
            || self.effect_id.trim().is_empty()
        {
            return Err("AGENT_SESSION_USAGE_INVALID");
        }
        if self.cost_currency.as_deref().is_some_and(|currency| {
            currency.len() != 3
                || !currency
                    .chars()
                    .all(|character| character.is_ascii_uppercase())
        }) {
            return Err("AGENT_SESSION_USAGE_INVALID");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableOperationPhaseV1 {
    Accepted,
    Checkpoint,
    ModelReady,
    ModelEffectPending,
    ModelSettled,
    ToolBatchPlanned,
    ToolEffectPending,
    ToolSettled,
    CompactionPlanned,
    CompactionEffectPending,
    CompactionSettled,
    Terminal,
    Pending,
    Cancelled,
    UnknownOutcome,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DurableOperationStateV1 {
    pub operation_id: String,
    pub attempt_id: String,
    pub fencing_token: u64,
    pub expected_state_version: u64,
    pub phase: DurableOperationPhaseV1,
    pub input_hash: String,
    pub bundle_hash: String,
    pub model_version: String,
    pub registry_hash: String,
    pub replay_policy: ReplayPolicyV1,
    pub deadline_at_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_action: Option<String>,
    pub updated_at_millis: u64,
}

impl DurableOperationStateV1 {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.operation_id.trim().is_empty()
            || self.attempt_id.trim().is_empty()
            || self.input_hash.trim().is_empty()
            || self.bundle_hash.trim().is_empty()
            || self.model_version.trim().is_empty()
            || self.registry_hash.trim().is_empty()
        {
            return Err("AGENT_SESSION_OPERATION_INVALID");
        }
        if self.deadline_at_millis == 0 {
            return Err("AGENT_SESSION_OPERATION_INVALID");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompactionRegisterV1 {
    pub snapshot_id: String,
    pub kind: CompactionKindV1,
    pub summary_hash: String,
    pub cut_entry_id: Option<String>,
    pub retained_entry_ids: Vec<String>,
    pub tokens_before: u64,
    pub tokens_after: u64,
}

impl CompactionRegisterV1 {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.snapshot_id.trim().is_empty()
            || self.summary_hash.trim().is_empty()
            || self.tokens_after > self.tokens_before
            || self
                .retained_entry_ids
                .iter()
                .any(|id| id.trim().is_empty())
        {
            return Err("AGENT_COMPACTION_REGISTER_INVALID");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApplicationUserEvidenceV1 {
    pub user_identity_id: String,
    pub source: String,
    pub evidence_hash: String,
}

impl ApplicationUserEvidenceV1 {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.user_identity_id.trim().is_empty()
            || self.source.trim().is_empty()
            || self.evidence_hash.trim().is_empty()
        {
            return Err("AGENT_LONG_TERM_MEMORY_SUBJECT_REQUIRED");
        }
        if self.source != "application_user_identity" {
            return Err("AGENT_LONG_TERM_MEMORY_SUBJECT_UNTRUSTED");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_identity_has_explicit_policy_specific_isolation() {
        let application = AgentSessionIdentityV1::application_session("t", "a", "s", "node");
        assert!(
            application
                .isolation_key(SessionPolicyModeV1::ApplicationSession)
                .unwrap()
                .contains("t:a:s:node")
        );
        let invocation = AgentSessionIdentityV1::invocation("t", "e", "attempt", "node");
        assert!(
            invocation
                .isolation_key(SessionPolicyModeV1::Invocation)
                .unwrap()
                .contains("t:e:attempt:node")
        );
        assert_eq!(
            application.validate(SessionPolicyModeV1::Invocation),
            Err("AGENT_SESSION_IDENTITY_INVALID")
        );
    }

    #[test]
    fn inbound_application_user_evidence_accepts_declared_api_users() {
        let evidence = ApplicationUserEvidenceV1 {
            user_identity_id: "external-customer-42".into(),
            source: "application_user_identity".into(),
            evidence_hash: "sha256:abc".into(),
        };
        assert_eq!(evidence.validate(), Ok(()));
    }

    #[test]
    fn register_rejects_orphaned_open_operation() {
        let register = AgentSessionRegisterV1 {
            session_id: "s".into(),
            open_operation_id: Some("op-1".into()),
            operation_state: None,
            ..AgentSessionRegisterV1::default()
        };
        assert_eq!(
            register.validate(),
            Err("AGENT_SESSION_OPERATION_REGISTER_INVALID")
        );
    }

    #[test]
    fn register_enforces_the_durable_pending_queue_limit() {
        let pending = |index: usize| AgentSessionEntryV1 {
            entry_id: format!("pending-{index}"),
            session_id: "session-1".into(),
            lane: AgentSessionLaneV1::Main,
            parent_entry_id: None,
            entry_kind: AgentSessionEntryKindV1::PendingFollowUp,
            payload_json: Some(serde_json::json!({"index": index})),
            payload_artifact_id: None,
            operation_id: None,
            created_at_millis: index as u64,
        };
        let mut register = AgentSessionRegisterV1 {
            session_id: "session-1".into(),
            pending_entries: (0..32).map(pending).collect(),
            ..AgentSessionRegisterV1::default()
        };
        assert!(register.validate().is_ok());
        register.pending_entries.push(pending(32));
        assert_eq!(register.validate(), Err("AGENT_SESSION_BUSY"));
    }

    #[test]
    fn trusted_subject_source_is_closed() {
        let evidence = ApplicationUserEvidenceV1 {
            user_identity_id: "subject-1".into(),
            source: "client_claim".into(),
            evidence_hash: "sha256:abc".into(),
        };
        assert_eq!(
            evidence.validate(),
            Err("AGENT_LONG_TERM_MEMORY_SUBJECT_UNTRUSTED")
        );
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RetentionPolicyV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_expires_at_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_artifact_expires_at_millis: Option<u64>,
    #[serde(default)]
    pub protect_referenced_artifacts: bool,
}

/// Projection of the durable Register supplied to one Core invocation.  The
/// Worker remains authoritative for loading and CAS; Core treats this as an
/// immutable recovery hint and never derives identity from user input.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionProjectionV1 {
    pub mode: SessionPolicyModeV1,
    pub session_key: String,
    pub state_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_subject: Option<ApplicationUserEvidenceV1>,
    #[serde(default)]
    pub retention: RetentionPolicyV1,
}
