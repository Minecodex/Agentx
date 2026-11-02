//! An immutable page and its business bundle form one activation unit.
use crate::{ApplicationPageContractV1, ContentHash, Ed25519Signature, RuntimeObjectReferenceV1};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const APPLICATION_PAGE_STYLE_VERSION: u32 = 1;
pub const APPLICATION_PAGE_SDK_SOURCE: &str =
    include_str!("../../../../contracts/sdk/application-page-v1.js");
pub const APPLICATION_PAGE_STYLE_SOURCE: &str =
    include_str!("../../../../contracts/sdk/application-page-v1.css");

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationPagePublicationV1 {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub publication_id: Uuid,
    pub ui_version_id: Uuid,
    pub revision: u64,
    pub expected_revision: u64,
    pub slug: String,
    pub enabled: bool,
    pub contract: ApplicationPageContractV1,
    pub contract_hash: ContentHash,
    pub html: RuntimeObjectReferenceV1,
    pub html_hash: ContentHash,
    pub style_version: u32,
    pub sdk: RuntimeObjectReferenceV1,
    pub stylesheet: RuntimeObjectReferenceV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparePagePublicationV1 {
    pub publication: ApplicationPagePublicationV1,
    pub content_hash: ContentHash,
    pub signature: Ed25519Signature,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivatePagePublicationV1 {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub publication_id: Uuid,
    pub expected_revision: u64,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ApplicationAccessBindingV1 {
    Web { publication_id: Uuid, revision: u64 },
    Mcp { publication_id: Uuid, revision: u64 },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationPageViewV1 {
    pub publication: ApplicationPagePublicationV1,
    pub html: String,
    pub sdk_script: String,
    pub stylesheet: String,
    pub route_key: String,
    pub historical: bool,
    pub task_id: Option<Uuid>,
}
