//! Strict design target union. A page never borrows a fake workflow or business execution identity.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationUiBuilderTargetV1 {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub ui_draft_id: Uuid,
    pub bundle_id: Uuid,
    pub contract_hash: crate::ContentHash,
    pub session_id: Uuid,
    pub run_id: Uuid,
    pub user_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    content = "target",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DesignModelTargetV1 {
    WorkflowBuilder(crate::StudioBuilderTargetV1),
    ApplicationUiBuilder(ApplicationUiBuilderTargetV1),
}

#[derive(Clone, Copy, Debug)]
pub struct DesignAgentIdentityV1 {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub run_id: Uuid,
}

impl DesignModelTargetV1 {
    pub fn identity(&self) -> DesignAgentIdentityV1 {
        match self {
            Self::WorkflowBuilder(target) => DesignAgentIdentityV1 {
                tenant_id: target.tenant_id,
                user_id: target.user_id,
                session_id: target.session_id,
                run_id: target.run_id,
            },
            Self::ApplicationUiBuilder(target) => DesignAgentIdentityV1 {
                tenant_id: target.tenant_id,
                user_id: target.user_id,
                session_id: target.session_id,
                run_id: target.run_id,
            },
        }
    }

    pub fn source_kind(&self) -> &'static str {
        match self {
            Self::WorkflowBuilder(_) => "studio_builder",
            Self::ApplicationUiBuilder(_) => "application_ui_builder",
        }
    }

    pub fn model_tools(&self) -> &'static [&'static str] {
        match self {
            Self::WorkflowBuilder(_) => &[
                "workflow_read",
                "node_catalog",
                "resource_query",
                "workflow_edit",
                "workflow_validate",
                "execution_inspect",
                "ask_user",
            ],
            Self::ApplicationUiBuilder(_) => &[
                "application_contract",
                "page_read",
                "page_edit",
                "page_check",
                "ask_user",
            ],
        }
    }
}

impl From<crate::StudioBuilderTargetV1> for DesignModelTargetV1 {
    fn from(target: crate::StudioBuilderTargetV1) -> Self {
        Self::WorkflowBuilder(target)
    }
}

impl From<ApplicationUiBuilderTargetV1> for DesignModelTargetV1 {
    fn from(target: ApplicationUiBuilderTargetV1) -> Self {
        Self::ApplicationUiBuilder(target)
    }
}

impl From<&crate::StudioBuilderTargetV1> for DesignModelTargetV1 {
    fn from(target: &crate::StudioBuilderTargetV1) -> Self {
        Self::WorkflowBuilder(target.clone())
    }
}
impl From<&ApplicationUiBuilderTargetV1> for DesignModelTargetV1 {
    fn from(target: &ApplicationUiBuilderTargetV1) -> Self {
        Self::ApplicationUiBuilder(target.clone())
    }
}
impl From<&DesignModelTargetV1> for DesignModelTargetV1 {
    fn from(target: &DesignModelTargetV1) -> Self {
        target.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profiles_have_disjoint_targets_and_narrow_tool_capabilities() {
        let id = Uuid::now_v7();
        let value = json!({"kind":"application_ui_builder","target":{"tenantId":id,"applicationId":id,"uiDraftId":id,"bundleId":id,"contractHash":crate::content_hash(&json!({})).unwrap(),"sessionId":id,"runId":id,"userId":id}});
        let target: DesignModelTargetV1 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(target.source_kind(), "application_ui_builder");
        assert!(!target.model_tools().contains(&"workflow_edit"));
        assert!(!target.model_tools().contains(&"bash"));
        let mut mixed = value.clone();
        mixed["target"]["workflowId"] = json!(id);
        assert!(serde_json::from_value::<DesignModelTargetV1>(mixed).is_err());
        let mut missing = value;
        missing["target"]
            .as_object_mut()
            .unwrap()
            .remove("bundleId");
        assert!(serde_json::from_value::<DesignModelTargetV1>(missing).is_err());
    }
}
