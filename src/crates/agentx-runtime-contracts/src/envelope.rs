use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ContentHash, EVENT_SCHEMA_VERSION, INTERNAL_API_VERSION};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandEnvelopeV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub event_id: Uuid,
    pub source_plane: Plane,
    pub tenant_id: Uuid,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub object_version: u64,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub payload: Value,
    pub content_hash: ContentHash,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    pub idempotency_key: String,
}

impl CommandEnvelopeV1 {
    #[must_use]
    pub fn current_schema_version() -> u32 {
        EVENT_SCHEMA_VERSION
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventEnvelopeV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub schema_version: u32,
    pub event_id: Uuid,
    pub source_plane: Plane,
    pub tenant_id: Uuid,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub object_version: u64,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub payload: crate::RuntimeEventPayloadV1,
    pub content_hash: ContentHash,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Plane {
    Control,
    Runtime,
    Observability,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyReceiptV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub event_id: Uuid,
    pub applied: bool,
    pub replayed: bool,
    pub object_version: u64,
    pub result: Value,
}

impl ApplyReceiptV1 {
    #[must_use]
    pub fn current_api_version() -> u32 {
        INTERNAL_API_VERSION
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeAdmissionCommandV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub api_version: u32,
    pub command: CommandEnvelopeV1,
    pub admission_epoch: u64,
    pub target: AdmissionTargetV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdmissionTargetV1 {
    Tenant {
        enabled: bool,
    },
    ApplicationRoute {
        state: ApplicationRouteAdmissionV1,
    },
    ApiKey {
        state: ApiKeyAdmissionV1,
    },
    RuntimeUser {
        state: crate::RuntimeUserAdmissionV1,
    },
    RuntimeUserApplicationGrant {
        state: crate::RuntimeUserApplicationGrantV1,
    },
    RuntimeUserWorkflowGrant {
        state: crate::RuntimeUserWorkflowGrantV1,
    },
    ServiceIdentity {
        state: ServiceIdentityAdmissionV1,
    },
    ResourceGrant {
        state: crate::RuntimeGrantStateV1,
    },
    PlatformEvaluation {
        state: crate::RuntimePlatformEvaluationAuthorizationV1,
    },
    ResourceState {
        state: crate::RuntimeResourceStateV1,
    },
    QuotaPolicy {
        state: crate::RuntimeQuotaPolicyV1,
    },
    ApprovalAction {
        state: crate::RuntimeApprovalActionV1,
    },
    RetentionHold {
        state: crate::RuntimeRetentionHoldV1,
    },
    RetentionPolicy {
        state: crate::RuntimeRetentionPolicyV1,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionStatusV1 {
    Active,
    Disabled,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationRouteAdmissionV1 {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub route_key: String,
    pub status: AdmissionStatusV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiKeyAdmissionV1 {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub key_id: Uuid,
    pub key_prefix: String,
    pub key_name: String,
    pub secret_hash: String,
    pub status: AdmissionStatusV1,
    #[schemars(with = "Option<String>")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceIdentityAdmissionV1 {
    pub tenant_id: Uuid,
    pub workflow_id: Uuid,
    pub identity_id: Uuid,
    pub policy_epoch: u64,
    pub status: AdmissionStatusV1,
    pub capabilities: Vec<String>,
    pub grant_ids: Vec<Uuid>,
}
