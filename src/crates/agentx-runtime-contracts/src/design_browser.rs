//! Isolated authoring observations; never a business Invocation or arbitrary URL.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DesignBrowserDocumentV1 {
    Workflow {
        document: crate::StudioDocumentV1,
        manifests: std::collections::BTreeMap<String, Value>,
    },
    ApplicationPage {
        html: String,
        contract: crate::ApplicationPageContractV1,
        contract_hash: crate::ContentHash,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[schemars(inline)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DesignBrowserActionV1 {
    Click {
        selector: String,
    },
    Fill {
        selector: String,
        value: String,
    },
    Select {
        selector: String,
        value: String,
    },
    Press {
        selector: String,
        key: String,
    },
    Upload {
        selector: String,
        name: String,
        content: String,
    },
    Wait {
        milliseconds: u32,
    },
    Reload,
    Check,
    FocusNodes {
        node_ids: Vec<String>,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[schemars(inline)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesignBrowserOptionsV1 {
    pub actions: Vec<DesignBrowserActionV1>,
    pub screenshot: bool,
    pub locale: String,
    pub theme: String,
    /// Optional CSS region in the isolated preview, not a URL or executable script.
    pub region: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesignBrowserRequestV1 {
    pub target: crate::DesignModelTargetV1,
    pub effect_id: Uuid,
    pub document_hash: String,
    pub document: DesignBrowserDocumentV1,
    pub options: DesignBrowserOptionsV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesignBrowserImageV1 {
    pub artifact_id: Uuid,
    pub sha256: String,
    pub content_type: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesignBrowserReportV1 {
    pub effect_id: Uuid,
    pub document_hash: String,
    pub report: Value,
    pub image: Option<DesignBrowserImageV1>,
}

impl DesignBrowserRequestV1 {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.options.actions.len() > 24
            || !matches!(self.options.locale.as_str(), "zh-CN" | "en-US")
            || !matches!(self.options.theme.as_str(), "light" | "dark")
        {
            return Err("A preview allows 24 actions and desktop zh-CN/en-US light/dark settings");
        }
        if self.document_hash.len() > 80
            || self.options.region.as_ref().is_some_and(|r| r.len() > 512)
        {
            return Err("Invalid preview document hash or region");
        }
        let mut waits = 0;
        for action in &self.options.actions {
            let selector = match action {
                DesignBrowserActionV1::Click { selector }
                | DesignBrowserActionV1::Fill { selector, .. }
                | DesignBrowserActionV1::Select { selector, .. }
                | DesignBrowserActionV1::Press { selector, .. }
                | DesignBrowserActionV1::Upload { selector, .. } => Some(selector),
                _ => None,
            };
            if selector.is_some_and(|value| value.is_empty() || value.len() > 512) {
                return Err("A browser selector must contain 1–512 bytes");
            }
            match action {
                DesignBrowserActionV1::FocusNodes { node_ids }
                    if !matches!(self.document, DesignBrowserDocumentV1::Workflow { .. })
                        || node_ids.len() > 200
                        || node_ids.iter().any(|id| id.is_empty() || id.len() > 128) =>
                {
                    return Err(
                        "Focus accepts up to 200 Workflow node IDs; an empty list fits the complete canvas",
                    );
                }
                DesignBrowserActionV1::Wait { milliseconds } => {
                    waits += milliseconds;
                    if *milliseconds > 3000 {
                        return Err("A wait may not exceed 3000 milliseconds");
                    }
                }
                DesignBrowserActionV1::Fill { value, .. } if value.len() > 16384 => {
                    return Err("Preview input exceeds 16 KiB");
                }
                DesignBrowserActionV1::Upload { name, content, .. }
                    if name.len() > 128 || name.contains(['/', '\\']) || content.len() > 16384 =>
                {
                    return Err("Invalid synthetic preview file");
                }
                _ => {}
            }
        }
        if waits > 10000 {
            return Err("Preview waits exceed 10 seconds");
        }
        if matches!(
            (&self.target, &self.document),
            (
                crate::DesignModelTargetV1::WorkflowBuilder(_),
                DesignBrowserDocumentV1::ApplicationPage { .. }
            ) | (
                crate::DesignModelTargetV1::ApplicationUiBuilder(_),
                DesignBrowserDocumentV1::Workflow { .. }
            )
        ) {
            return Err("The preview document does not match its authoring target");
        }
        Ok(())
    }
}

pub fn design_image_owner(target: &crate::DesignModelTargetV1) -> String {
    format!("{}:{}", target.source_kind(), target.identity().session_id)
}
