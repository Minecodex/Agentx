use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use agentx_runtime_contracts::WebhookChannelModeV1;
use agentx_runtime_contracts::WebhookInputMappingV1;

use super::webhook_provider_templates;
use crate::api_error::{ApiError, ApiResult};
use crate::control_api::{Actor, ControlApiState};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebhookResponse {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) public_id: String,
    pub(crate) path: String,
    pub(crate) status: String,
    pub(crate) secret: Option<String>,
    pub(crate) version: u64,
    pub(crate) provider_type: String,
    pub(crate) channel_mode: String,
    pub(crate) config_fields: Value,
    pub(crate) reply: Option<agentx_runtime_contracts::WebhookReplyConfigV1>,
    pub(crate) connection_status: Option<String>,
    pub(crate) connection_error: Option<String>,
    pub(crate) last_connected_at: Option<String>,
    pub(crate) input_mappings: Vec<WebhookInputMappingV1>,
    pub(crate) fixed_inputs: Value,
    pub(crate) configuration_revision: u64,
    pub(crate) mapping_count: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateWebhookRequest {
    pub(crate) name: String,
    #[serde(default = "default_webhook_provider")]
    pub(crate) provider_type: String,
    #[serde(default = "default_channel_mode")]
    pub(crate) channel_mode: String,
    #[serde(default)]
    pub(crate) channel_config: Option<Value>,
    #[serde(default)]
    pub(crate) reply: Option<agentx_runtime_contracts::WebhookReplyConfigV1>,
    #[serde(default)]
    pub(crate) input_mappings: Vec<WebhookInputMappingV1>,
    #[serde(default)]
    pub(crate) fixed_inputs: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateWebhookRequest {
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) version: u64,
    #[serde(default = "default_webhook_provider")]
    pub(crate) provider_type: String,
    #[serde(default = "default_channel_mode")]
    pub(crate) channel_mode: String,
    #[serde(default)]
    pub(crate) channel_config: Option<Value>,
    #[serde(default)]
    pub(crate) reply: Option<agentx_runtime_contracts::WebhookReplyConfigV1>,
    #[serde(default)]
    pub(crate) input_mappings: Vec<WebhookInputMappingV1>,
    #[serde(default)]
    pub(crate) fixed_inputs: Value,
}

pub(crate) fn default_webhook_provider() -> String {
    "agentx".into()
}

pub(crate) fn default_channel_mode() -> String {
    "callback".into()
}

pub(crate) fn workflow_schemas(definition: &agentx_domain::WorkflowDefinition) -> (Value, Value) {
    let all_complete = definition.end.completion == agentx_domain::WorkflowCompletion::AllComplete;
    let properties = definition.end.outputs.iter().map(|(name, value)| {
        let mut schema = value.schema.clone();
        if let Some(object) = schema.as_object_mut() {
            object.insert("x-agentx-sensitive".into(), json!(value.sensitive));
        }
        if all_complete {
            schema = json!({"type":"array", "items": if value.required { schema } else { json!({"anyOf":[schema,{"type":"null"}]}) }});
        }
        (name.clone(), schema)
    }).collect::<serde_json::Map<_, _>>();
    let required = definition
        .end
        .outputs
        .iter()
        .filter(|(_, value)| all_complete || value.required)
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    (
        definition.start.inputs.clone(),
        json!({"type":"object", "properties":properties, "required":required, "additionalProperties":false}),
    )
}

async fn configuration_schemas(
    state: &ControlApiState,
    tenant_id: Uuid,
    application_id: Uuid,
) -> ApiResult<(Value, Value)> {
    let deployment = sqlx::query("SELECT d.input_schema_json,d.output_schema_json FROM application_deployment_heads h JOIN application_deployments d ON d.tenant_id=h.tenant_id AND d.id=h.deployment_id WHERE h.tenant_id=? AND h.application_id=? AND d.status='active'")
        .bind(tenant_id).bind(application_id).fetch_optional(&state.pool).await?;
    if let Some(deployment) = deployment {
        return Ok((
            deployment.try_get("input_schema_json")?,
            deployment.try_get("output_schema_json")?,
        ));
    }
    let definition = sqlx::query_scalar::<_, Value>("SELECT v.definition_json FROM applications a JOIN workflow_versions v ON v.tenant_id=a.tenant_id AND v.workflow_id=a.workflow_id WHERE a.tenant_id=? AND a.id=? ORDER BY v.version_number DESC LIMIT 1")
        .bind(tenant_id).bind(application_id).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::unprocessable("APPLICATION_WORKFLOW_VERSION_REQUIRED", "Create a Workflow version before configuring channels"))?;
    let definition = serde_json::from_value(definition).map_err(ApiError::internal)?;
    Ok(workflow_schemas(&definition))
}

pub(crate) fn parse_provider(
    value: Option<String>,
) -> ApiResult<agentx_runtime_contracts::WebhookProviderV1> {
    match value.as_deref().unwrap_or("agentx") {
        "agentx" => Ok(agentx_runtime_contracts::WebhookProviderV1::Agentx),
        "dingtalk" => Ok(agentx_runtime_contracts::WebhookProviderV1::Dingtalk),
        "wecom" => Ok(agentx_runtime_contracts::WebhookProviderV1::Wecom),
        "feishu" => Ok(agentx_runtime_contracts::WebhookProviderV1::Feishu),
        _ => Err(ApiError::bad_request(
            "INVALID_WEBHOOK_PROVIDER",
            "Webhook provider is invalid",
        )),
    }
}

pub(crate) fn parse_mode(value: &str) -> ApiResult<WebhookChannelModeV1> {
    match value {
        "callback" => Ok(WebhookChannelModeV1::Callback),
        "stream" => Ok(WebhookChannelModeV1::Stream),
        _ => Err(ApiError::bad_request(
            "INVALID_WEBHOOK_CHANNEL_MODE",
            "Webhook channel mode is invalid",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn validate(
    state: &ControlApiState,
    actor: &Actor,
    application_id: Uuid,
    provider_type: &str,
    channel_mode: &str,
    channel_config: Option<&Value>,
    reply: Option<&agentx_runtime_contracts::WebhookReplyConfigV1>,
    mappings: &[WebhookInputMappingV1],
    fixed_inputs: &Value,
) -> ApiResult<()> {
    if !matches!(provider_type, "agentx" | "dingtalk" | "wecom" | "feishu") {
        return Err(ApiError::bad_request(
            "INVALID_WEBHOOK_PROVIDER",
            "Webhook provider is invalid",
        ));
    }
    if webhook_provider_templates::find(provider_type, channel_mode).is_none() {
        return Err(ApiError::bad_request(
            "INVALID_WEBHOOK_CHANNEL_MODE",
            "Webhook channel mode is not available for this provider",
        ));
    }
    if provider_type == "agentx"
        && channel_config.is_some_and(|value| {
            !value.is_null() && value.as_object().is_some_and(|object| !object.is_empty())
        })
    {
        return Err(ApiError::bad_request(
            "INVALID_WEBHOOK_CHANNEL_CONFIG",
            "Agentx webhooks do not accept channel config",
        ));
    }
    if !fixed_inputs.is_null() && !fixed_inputs.is_object() {
        return Err(ApiError::bad_request(
            "INVALID_WEBHOOK_FIXED_INPUTS",
            "Fixed inputs must be an object",
        ));
    }
    if let Some(reply) = reply {
        if reply.enabled {
            if reply.output_field.trim().is_empty() {
                return Err(ApiError::bad_request(
                    "INVALID_WEBHOOK_REPLY_OUTPUT_FIELD",
                    "Reply output field is required when the reply is enabled",
                ));
            }
            if let Some(template) = reply.template.as_deref() {
                if template.len() > 1000 {
                    return Err(ApiError::bad_request(
                        "INVALID_WEBHOOK_REPLY_TEMPLATE",
                        "Reply template must be at most 1000 characters",
                    ));
                }
            }
        }
    }
    let (input_schema, output_schema) =
        configuration_schemas(state, actor.tenant_id, application_id).await?;
    validate_bindings(
        provider_type,
        &input_schema,
        &output_schema,
        reply,
        mappings,
        fixed_inputs,
    )
}

fn validate_bindings(
    provider_type: &str,
    input_schema: &Value,
    output_schema: &Value,
    reply: Option<&agentx_runtime_contracts::WebhookReplyConfigV1>,
    mappings: &[WebhookInputMappingV1],
    fixed_inputs: &Value,
) -> ApiResult<()> {
    let empty = serde_json::Map::new();
    let properties = input_schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut targets = std::collections::HashSet::new();
    for mapping in mappings {
        // Standardized Trigger Context fields, or raw.<dotted.path> passthrough
        // of any decrypted platform payload field (resolved by the Runtime).
        let standard_source = matches!(
            mapping.source.as_str(),
            "message.text"
                | "sender.id"
                | "sender.name"
                | "conversation.id"
                | "conversation.name"
                | "conversation.type"
                | "provider"
                | "provider_event_id"
        );
        let raw_source = mapping.source.starts_with("raw.") && mapping.source.len() > 4;
        if !standard_source && !raw_source {
            return Err(ApiError::bad_request(
                "INVALID_WEBHOOK_SOURCE",
                "Webhook source field is invalid",
            ));
        }
        if mapping.target.trim().is_empty() || !targets.insert(mapping.target.clone()) {
            return Err(ApiError::bad_request(
                "INVALID_WEBHOOK_MAPPING",
                "Webhook mapping targets must be unique and non-empty",
            ));
        }
        if !matches!(
            mapping.missing_policy.as_str(),
            "error" | "null" | "default" | "omit"
        ) {
            return Err(ApiError::bad_request(
                "INVALID_WEBHOOK_MISSING_POLICY",
                "Webhook missing policy is invalid",
            ));
        }
        {
            let Some(property) = properties.get(&mapping.target) else {
                return Err(ApiError::unprocessable(
                    "WEBHOOK_MAPPING_TARGET_UNKNOWN",
                    "Webhook mapping target is not in the active Deployment schema",
                ));
            };
            // Raw payload values have no statically known type, so only the
            // standardized string sources can be type-checked here.
            if standard_source && !schema_accepts_string(property) {
                return Err(ApiError::unprocessable(
                    "WEBHOOK_MAPPING_TYPE_MISMATCH",
                    "Webhook source values are strings and require a string Workflow input",
                ));
            }
        }
    }
    if let Some(reply) = reply.filter(|reply| reply.enabled) {
        if !output_schema
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| properties.contains_key(&reply.output_field))
        {
            return Err(ApiError::unprocessable(
                "WEBHOOK_REPLY_OUTPUT_FIELD_UNKNOWN",
                "Reply output field is not in the Workflow output schema",
            )
            .with_field_error(
                "reply.outputField",
                "WEBHOOK_REPLY_OUTPUT_FIELD_UNKNOWN",
                "Select an existing Workflow output field",
            ));
        }
    }
    if let Some(fixed) = fixed_inputs.as_object() {
        for (target, value) in fixed {
            if ["conversation_id", "sender_id", "provider_event_id"].contains(&target.as_str()) {
                return Err(ApiError::bad_request(
                    "WEBHOOK_MAPPING_CONFLICT",
                    "Fixed inputs cannot override provider source fields",
                ));
            }
            if !targets.insert(target.clone()) {
                return Err(ApiError::bad_request(
                    "WEBHOOK_MAPPING_CONFLICT",
                    "Fixed input conflicts with a mapped source",
                ));
            }
            {
                let Some(property) = properties.get(target) else {
                    return Err(ApiError::unprocessable(
                        "WEBHOOK_FIXED_INPUT_TARGET_UNKNOWN",
                        "Fixed input target is not in the active Deployment schema",
                    ));
                };
                if !schema_accepts_value(property, value) {
                    return Err(ApiError::unprocessable(
                        "WEBHOOK_FIXED_INPUT_TYPE_MISMATCH",
                        "Fixed input value is incompatible with the Workflow Start Input schema",
                    ));
                }
            }
        }
    }
    if provider_type != "agentx" {
        if let Some(required) = input_schema.get("required").and_then(Value::as_array) {
            for field in required.iter().filter_map(Value::as_str) {
                if !targets.contains(field) {
                    return Err(ApiError::unprocessable(
                        "WEBHOOK_REQUIRED_INPUT_UNMAPPED",
                        "Every required Workflow Start Input must be mapped or fixed",
                    )
                    .with_field_error(
                        "inputMappings",
                        "WEBHOOK_REQUIRED_INPUT_UNMAPPED",
                        format!("Required Workflow input '{field}' must be mapped or fixed"),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn schema_accepts_string(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(value)) => value == "string",
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some("string")),
        _ => true,
    }
}

fn schema_accepts_value(schema: &Value, value: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(kind)) => match kind.as_str() {
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "null" => value.is_null(),
            _ => true,
        },
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| {
            let mut schema = schema.clone();
            schema["type"] = kind.clone();
            schema_accepts_value(&schema, value)
        }),
        _ => true,
    }
}

pub(crate) fn response_from_row(row: sqlx::mysql::MySqlRow) -> ApiResult<WebhookResponse> {
    let public_id: String = row.try_get("public_id")?;
    let input_mappings: Vec<WebhookInputMappingV1> = row
        .try_get::<Option<Value>, _>("input_mapping_json")?
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    Ok(WebhookResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        path: format!("/gateway/v1/webhooks/{public_id}"),
        public_id,
        status: row.try_get("status")?,
        secret: None,
        version: row.try_get("version")?,
        provider_type: row
            .try_get::<Option<String>, _>("provider_type")?
            .unwrap_or_else(default_webhook_provider),
        channel_mode: row
            .try_get::<Option<String>, _>("channel_mode")?
            .unwrap_or_else(default_channel_mode),
        config_fields: row
            .try_get::<Option<Value>, _>("channel_config_json")?
            .unwrap_or_else(|| json!({})),
        reply: row
            .try_get::<Option<Value>, _>("reply_config_json")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?,
        connection_status: None,
        connection_error: None,
        last_connected_at: None,
        mapping_count: input_mappings.len(),
        input_mappings,
        fixed_inputs: row
            .try_get::<Option<Value>, _>("fixed_inputs_json")?
            .unwrap_or_else(|| json!({})),
        configuration_revision: row
            .try_get::<Option<u64>, _>("configuration_revision")?
            .unwrap_or(1),
    })
}

pub(crate) async fn load(
    state: &ControlApiState,
    tenant_id: Uuid,
    application_id: Uuid,
    id: Uuid,
) -> ApiResult<WebhookResponse> {
    let row = sqlx::query("SELECT id,name,public_id,status,version,provider_type,channel_mode,channel_config_json,reply_config_json,input_mapping_json,fixed_inputs_json,configuration_revision FROM application_webhooks WHERE tenant_id=? AND application_id=? AND id=?")
        .bind(tenant_id).bind(application_id).bind(id).fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::not_found("Webhook"))?;
    response_from_row(row)
}
