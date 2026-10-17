use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// The end user a call acts for. API identities are declarations, not authentication.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationUserIdentityV1 {
    pub source: ApplicationUserSourceV1,
    pub source_id: String,
    pub external_user_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationUserSourceV1 {
    Platform,
    Channel,
    ApiParameter,
}

impl ApplicationUserIdentityV1 {
    pub fn platform(user_id: Uuid) -> Self {
        Self {
            source: ApplicationUserSourceV1::Platform,
            source_id: "platform".into(),
            external_user_id: user_id.to_string(),
        }
    }

    pub fn api(input: &Value, field: Option<&str>) -> Option<Self> {
        let id = input.get(field?)?.as_str()?;
        (!id.trim().is_empty()).then(|| Self {
            source: ApplicationUserSourceV1::ApiParameter,
            source_id: "api".into(),
            external_user_id: id.into(),
        })
    }

    pub fn channel(provider: &str, connection: &str, sender: &str) -> Option<Self> {
        (!sender.trim().is_empty() && sender != "unknown").then(|| Self {
            source: ApplicationUserSourceV1::Channel,
            source_id: format!("{provider}:{connection}"),
            external_user_id: sender.into(),
        })
    }

    pub fn subject_id(&self) -> Uuid {
        crate::deterministic_uuid(
            Uuid::nil(),
            &crate::canonical_bytes(self).expect("serializable user identity"),
        )
    }
}

/// Stable scope shared by all Mem0 call sites. Runtime data never uses display names.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryScopeV1 {
    pub tenant_id: Uuid,
    pub environment_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub workflow_id: Uuid,
    pub stable_node_id: String,
    pub user: ApplicationUserIdentityV1,
}

impl MemoryScopeV1 {
    pub fn scope_hash(&self) -> String {
        crate::content_hash(self)
            .expect("serializable memory scope")
            .as_str()
            .trim_start_matches("sha256:")
            .to_owned()
    }

    pub fn user_id(&self) -> String {
        format!(
            "agentx:{}",
            crate::content_hash(self)
                .expect("serializable memory scope")
                .as_str()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn declarations_do_not_need_platform_accounts_or_coercion() {
        let user =
            ApplicationUserIdentityV1::api(&json!({"customer":"external-42"}), Some("customer"))
                .unwrap();
        assert_eq!(user.external_user_id, "external-42");
        for value in [
            json!({}),
            json!({"customer":null}),
            json!({"customer":42}),
            json!({"customer":" "}),
        ] {
            assert!(ApplicationUserIdentityV1::api(&value, Some("customer")).is_none());
        }
        assert!(ApplicationUserIdentityV1::api(&json!({"customer":"42"}), None).is_none());
        assert!(ApplicationUserIdentityV1::channel("feishu", "bot", "unknown").is_none());
    }

    #[test]
    fn scope_separates_tenants_environments_apps_workflows_and_channel_accounts() {
        let base = MemoryScopeV1 {
            tenant_id: Uuid::from_u128(1),
            environment_id: Some(Uuid::from_u128(2)),
            application_id: Some(Uuid::from_u128(3)),
            workflow_id: Uuid::from_u128(4),
            stable_node_id: "stable-agent".into(),
            user: ApplicationUserIdentityV1::channel("feishu", "account-a", "42").unwrap(),
        };
        let original = base.user_id();
        let mut different = base.clone();
        different.tenant_id = Uuid::from_u128(9);
        assert_ne!(original, different.user_id());
        let mut different = base.clone();
        different.environment_id = Some(Uuid::from_u128(9));
        assert_ne!(original, different.user_id());
        let mut different = base.clone();
        different.application_id = Some(Uuid::from_u128(9));
        assert_ne!(original, different.user_id());
        let mut different = base.clone();
        different.workflow_id = Uuid::from_u128(9);
        assert_ne!(original, different.user_id());
        let mut different = base.clone();
        different.user.source_id = "feishu:account-b".into();
        assert_ne!(original, different.user_id());
        let serialized = serde_json::to_value(base).unwrap();
        for excluded in ["workflowVersionId", "deploymentId", "apiKeyId", "sessionId"] {
            assert!(serialized.get(excluded).is_none());
        }
    }

    #[test]
    fn sources_and_nodes_have_independent_stable_memory() {
        let api = ApplicationUserIdentityV1::api(&json!({"id":"42"}), Some("id")).unwrap();
        let mut scope = MemoryScopeV1 {
            tenant_id: Uuid::from_u128(1),
            environment_id: Some(Uuid::from_u128(2)),
            application_id: Some(Uuid::from_u128(3)),
            workflow_id: Uuid::from_u128(4),
            stable_node_id: "agent-a".into(),
            user: api,
        };
        let first = scope.user_id();
        assert_eq!(first, scope.clone().user_id());
        scope.stable_node_id = "agent-b".into();
        assert_ne!(first, scope.user_id());
        scope.stable_node_id = "agent-a".into();
        scope.user = ApplicationUserIdentityV1::channel("feishu", "bot", "42").unwrap();
        assert_ne!(first, scope.user_id());
        scope.application_id = None;
        assert_ne!(first, scope.user_id());
    }
}
