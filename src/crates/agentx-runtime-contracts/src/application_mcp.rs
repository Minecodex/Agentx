//! An application MCP revision publishes only the application contract, never internal tools.
use crate::ContentHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
pub const APPLICATION_MCP_PROTOCOL: &str = "2025-11-25";
pub const APPLICATION_MCP_HELPERS: [&str; 3] = ["get_task", "submit_task_input", "cancel_task"];

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationMcpRemovalV1 {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub publication_id: Uuid,
}

/// Object-form schemas preserve the same JSON Schema semantics and are accepted
/// by MCP client validators that require property schemas to be JSON objects.
pub fn application_mcp_schema(mut schema: schemars::Schema) -> schemars::Schema {
    use schemars::transform::{ReplaceBoolSchemas, Transform};
    let mut transform = ReplaceBoolSchemas::default();
    transform.skip_additional_properties = true;
    transform.transform(&mut schema);
    schema
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationMcpPublicationV1 {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub publication_id: Uuid,
    pub deployment_id: Uuid,
    pub bundle_id: Uuid,
    pub revision: u64,
    pub expected_revision: u64,
    pub slug: String,
    pub tool_name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub wait_input_schema: Value,
    pub chat_mapping: Option<crate::ChatMappingV1>,
    pub contract_hash: ContentHash,
    pub enabled: bool,
    pub protocol_version: String,
}
impl ApplicationMcpPublicationV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != APPLICATION_MCP_PROTOCOL
            || Some(self.revision) != self.expected_revision.checked_add(1)
            || self.tool_name.is_empty()
            || self.tool_name.len() > 64
            || !self
                .tool_name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || APPLICATION_MCP_HELPERS.contains(&self.tool_name.as_str())
            || self.description.trim().is_empty()
            || self.description.len() > 4096
        {
            return Err("Choose a business tool name, description and the fixed MCP protocol; helper tool names are reserved".into());
        }
        let expected=crate::content_hash(&serde_json::json!({"input":self.input_schema,"output":self.output_schema,"waitInputSchema":self.wait_input_schema,"chatMapping":self.chat_mapping})).map_err(|e|e.to_string())?;
        if expected != self.contract_hash {
            return Err("The immutable MCP input/output contract hash does not match".into());
        }
        Ok(())
    }
}
