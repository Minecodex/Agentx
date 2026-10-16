use agentx_node_protocol::PluginNodeBinding;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDesignOperationRequestV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub tenant_id: Uuid,
    #[serde(default)]
    pub resolve_session_id: Option<Uuid>,
    pub method: String,
    pub plugin: PluginNodeBinding,
    #[serde(default)]
    pub resources: Vec<crate::RuntimeResourceBindingV1>,
    #[serde(default)]
    pub parameters: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDesignOperationResponseV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub result: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginResolveSessionV1 {
    #[serde(deserialize_with = "crate::deserialize_v1")]
    pub protocol_version: u32,
    pub tenant_id: Uuid,
    pub session_id: Uuid,
}
