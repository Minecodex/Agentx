use agentx_agent_core::{EffectContextV1, ModelRequestV1, ModelResponseV1};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StudioBuilderTargetV1 {
    pub tenant_id: Uuid,
    pub workflow_id: Uuid,
    pub draft_id: Uuid,
    pub session_id: Uuid,
    pub run_id: Uuid,
    pub user_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesignModelEffectRequestV1 {
    pub protocol_version: u32,
    pub target: StudioBuilderTargetV1,
    pub binding: crate::RuntimeResourceBindingV1,
    pub parameters: Value,
    pub context: EffectContextV1,
    pub request: ModelRequestV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesignModelReceiptRequestV1 {
    pub target: StudioBuilderTargetV1,
    pub effect_id: Uuid,
    pub after: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesignModelDeltaV1 {
    pub sequence: u64,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesignModelReceiptV1 {
    pub effect_id: Uuid,
    pub status: String,
    pub response: Option<ModelResponseV1>,
    pub error_code: Option<String>,
    pub usage_known: bool,
    pub usage_estimated: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micros: u64,
    pub currency: Option<String>,
    pub deltas: Vec<DesignModelDeltaV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StudioBuilderRunSummaryV1 {
    pub id: Uuid,
    pub status: String,
    pub model_alias_id: Uuid,
    pub setting_version: u64,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StudioBuilderSessionV1 {
    pub id: Uuid,
    pub draft_id: Uuid,
    pub can_edit: bool,
    pub snapshot: crate::StudioEditorSnapshotV1,
    pub runs: Vec<StudioBuilderRunSummaryV1>,
}
