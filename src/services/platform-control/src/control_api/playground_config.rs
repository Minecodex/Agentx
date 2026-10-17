use agentx_runtime_contracts::ChatMappingV1;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use super::{Actor, ControlApiState, require_application};
use crate::api_error::{ApiError, ApiResult};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PlaygroundConfigResponse {
    application_id: Uuid,
    deployment_id: Uuid,
    deployment_version: u64,
    version: u64,
    mapping: Option<ChatMappingV1>,
    published_version: Option<u64>,
    publish_status: String,
    error_code: Option<String>,
    error_message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PutPlaygroundConfigRequest {
    expected_version: u64,
    mapping: Option<ChatMappingV1>,
}

pub(super) async fn get(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application_id, deployment_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<PlaygroundConfigResponse>> {
    if !actor.permissions.iter().any(|permission| {
        matches!(
            permission.as_str(),
            "application:invoke" | "application:manage"
        )
    }) {
        return Err(ApiError::forbidden("Missing permission application:invoke"));
    }
    require_application(&state, &actor, application_id).await?;
    let deployment_version: u64 = sqlx::query_scalar(
        "SELECT sequence_number FROM application_deployments WHERE tenant_id=? AND application_id=? AND id=?",
    )
    .bind(actor.tenant_id)
    .bind(application_id)
    .bind(deployment_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Application Deployment"))?;
    let row = sqlx::query("SELECT version,mapping_json,published_version,publish_status,last_error_code,last_error_message FROM application_playground_configs WHERE tenant_id=? AND deployment_id=?")
        .bind(actor.tenant_id)
        .bind(deployment_id)
        .fetch_optional(&state.pool)
        .await?;
    let response = if let Some(row) = row {
        PlaygroundConfigResponse {
            application_id,
            deployment_id,
            deployment_version,
            version: row.try_get("version")?,
            mapping: row
                .try_get::<Option<Value>, _>("mapping_json")?
                .map(serde_json::from_value)
                .transpose()
                .map_err(ApiError::internal)?,
            published_version: row.try_get("published_version")?,
            publish_status: row.try_get("publish_status")?,
            error_code: row.try_get("last_error_code")?,
            error_message: row.try_get("last_error_message")?,
        }
    } else {
        PlaygroundConfigResponse {
            application_id,
            deployment_id,
            deployment_version,
            version: 0,
            mapping: None,
            published_version: Some(0),
            publish_status: "active".into(),
            error_code: None,
            error_message: None,
        }
    };
    Ok(Json(response))
}

pub(super) async fn put(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application_id, deployment_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<PutPlaygroundConfigRequest>,
) -> ApiResult<(StatusCode, Json<PlaygroundConfigResponse>)> {
    actor.require("application:manage")?;
    require_application(&state, &actor, application_id).await?;
    let mut tx = state.pool.begin().await?;
    let deployment = sqlx::query("SELECT ad.sequence_number,ad.input_schema_json,ad.output_schema_json,b.id bundle_id FROM application_deployments ad JOIN execution_spec_bundles b ON b.tenant_id=ad.tenant_id AND b.deployment_id=ad.id AND b.status='published' WHERE ad.tenant_id=? AND ad.application_id=? AND ad.id=? AND ad.status='active' FOR SHARE")
        .bind(actor.tenant_id)
        .bind(application_id)
        .bind(deployment_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::unprocessable("PLAYGROUND_DEPLOYMENT_NOT_ACTIVE", "Playground configuration can only target the active published Deployment"))?;
    let input_schema: Value = deployment.try_get("input_schema_json")?;
    let output_schema: Value = deployment.try_get("output_schema_json")?;
    if let Some(mapping) = &input.mapping {
        validate_chat_mapping(mapping, &input_schema, &output_schema)?;
    }
    let current = sqlx::query("SELECT version,published_version FROM application_playground_configs WHERE tenant_id=? AND deployment_id=? FOR UPDATE")
        .bind(actor.tenant_id)
        .bind(deployment_id)
        .fetch_optional(&mut *tx)
        .await?;
    let current_version = current
        .as_ref()
        .map(|row| row.try_get::<u64, _>("version"))
        .transpose()?
        .unwrap_or(0);
    let published_version = current
        .as_ref()
        .map(|row| row.try_get::<Option<u64>, _>("published_version"))
        .transpose()?
        .flatten();
    if current_version != input.expected_version {
        return Err(ApiError::conflict(
            "PLAYGROUND_CONFIG_VERSION_CONFLICT",
            "Playground configuration changed; reload it before saving",
        ));
    }
    let version = current_version + 1;
    let mapping_json = input
        .mapping
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(ApiError::internal)?;
    let mapping_hash =
        agentx_runtime_contracts::content_hash(&mapping_json).map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO application_playground_configs(tenant_id,application_id,deployment_id,version,mapping_json,mapping_hash,publish_status,updated_by) VALUES(?,?,?,?,?,?,'publishing',?) ON DUPLICATE KEY UPDATE version=VALUES(version),mapping_json=VALUES(mapping_json),mapping_hash=VALUES(mapping_hash),publish_status='publishing',last_error_code=NULL,last_error_message=NULL,updated_by=VALUES(updated_by)")
        .bind(actor.tenant_id)
        .bind(application_id)
        .bind(deployment_id)
        .bind(version)
        .bind(&mapping_json)
        .bind(mapping_hash.as_str())
        .bind(actor.user_id)
        .execute(&mut *tx)
        .await?;
    let outbox_id = Uuid::now_v7();
    let bundle_id: Uuid = deployment.try_get("bundle_id")?;
    let payload = json!({
        "applicationId": application_id,
        "deploymentId": deployment_id,
        "bundleId": bundle_id,
        "version": version,
        "mapping": mapping_json,
        "contentHash": mapping_hash,
    });
    sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,'ApplicationChatMappingChanged','application_chat_mapping',?,?,'pending',?,?)")
        .bind(outbox_id)
        .bind(actor.tenant_id)
        .bind(deployment_id.to_string())
        .bind(&payload)
        .bind(mapping_hash.as_str())
        .bind(format!("playground-config:{deployment_id}:{version}"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(PlaygroundConfigResponse {
            application_id,
            deployment_id,
            deployment_version: deployment.try_get("sequence_number")?,
            version,
            mapping: input.mapping,
            published_version,
            publish_status: "publishing".into(),
            error_code: None,
            error_message: None,
        }),
    ))
}

fn validate_chat_mapping(
    mapping: &ChatMappingV1,
    input_schema: &Value,
    output_schema: &Value,
) -> ApiResult<()> {
    if mapping.question_input.trim().is_empty() || mapping.answer_output.trim().is_empty() {
        return Err(ApiError::unprocessable(
            "PLAYGROUND_MAPPING_FIELD_REQUIRED",
            "Question input and answer output are required",
        ));
    }
    let input_properties = input_schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            ApiError::unprocessable(
                "PLAYGROUND_INPUT_SCHEMA_INCOMPATIBLE",
                "Deployment Input Schema must be a top-level object",
            )
        })?;
    let output_properties = output_schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            ApiError::unprocessable(
                "PLAYGROUND_OUTPUT_SCHEMA_INCOMPATIBLE",
                "Deployment Output Schema must be a top-level object",
            )
        })?;
    require_mapping_field(
        input_properties,
        &mapping.question_input,
        MappingFieldKind::String,
        "questionInput",
    )?;
    if let Some(field) = &mapping.file_input {
        require_mapping_field(
            input_properties,
            field,
            MappingFieldKind::Artifact,
            "fileInput",
        )?;
        if field == &mapping.question_input {
            return Err(ApiError::unprocessable(
                "PLAYGROUND_MAPPING_FIELDS_CONFLICT",
                "Question and file inputs must be different fields",
            ));
        }
    }
    require_mapping_field(
        output_properties,
        &mapping.answer_output,
        MappingFieldKind::String,
        "answerOutput",
    )?;
    if output_properties[&mapping.answer_output]
        .get("x-agentx-sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(ApiError::unprocessable(
            "PLAYGROUND_ANSWER_OUTPUT_SENSITIVE",
            "Sensitive outputs cannot be exposed as the Assistant answer",
        ));
    }
    if let Some(field) = &mapping.answer_files_output {
        require_mapping_field(
            output_properties,
            field,
            MappingFieldKind::Artifact,
            "answerFilesOutput",
        )?;
        if field == &mapping.answer_output {
            return Err(ApiError::unprocessable(
                "PLAYGROUND_MAPPING_FIELDS_CONFLICT",
                "Answer text and file outputs must be different fields",
            ));
        }
    }
    // Other required inputs are supplied once when creating the Session.
    // Runtime validates the merged initial inputs and each message projection.
    Ok(())
}

enum MappingFieldKind {
    String,
    Artifact,
}

fn require_mapping_field(
    properties: &serde_json::Map<String, Value>,
    field: &str,
    kind: MappingFieldKind,
    mapping_name: &str,
) -> ApiResult<()> {
    let schema = properties.get(field).ok_or_else(|| {
        ApiError::unprocessable(
            "PLAYGROUND_MAPPING_FIELD_NOT_FOUND",
            format!("{mapping_name} references unknown top-level field '{field}'"),
        )
    })?;
    let compatible = match kind {
        MappingFieldKind::String => {
            schema.get("type").and_then(Value::as_str) == Some("string")
                && !schema
                    .get("x-agentx-artifact")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        }
        MappingFieldKind::Artifact => schema
            .get("x-agentx-artifact")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    compatible.then_some(()).ok_or_else(|| {
        ApiError::unprocessable(
            "PLAYGROUND_MAPPING_TYPE_MISMATCH",
            format!("{mapping_name} field '{field}' has an incompatible type"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> ChatMappingV1 {
        ChatMappingV1 {
            question_input: "prompt".into(),
            file_input: Some("documents".into()),
            answer_output: "answer".into(),
            answer_files_output: Some("reports".into()),
        }
    }

    #[test]
    fn mapping_accepts_compatible_top_level_fields_and_defaulted_required_inputs() {
        let input = json!({"type":"object","properties":{
            "prompt":{"type":"string"},
            "documents":{"type":"array","x-agentx-artifact":true,"x-agentx-artifact-array":true},
            "temperature":{"type":"number","default":0.2}
        },"required":["prompt","temperature"]});
        let output = json!({"type":"object","properties":{
            "answer":{"type":"string","x-agentx-sensitive":false},
            "reports":{"type":"array","x-agentx-artifact":true,"x-agentx-artifact-array":true}
        }});
        assert!(validate_chat_mapping(&mapping(), &input, &output).is_ok());
    }

    #[test]
    fn mapping_accepts_session_business_inputs_and_rejects_sensitive_answers() {
        let input = json!({"type":"object","properties":{
            "prompt":{"type":"string"},
            "documents":{"type":"object","x-agentx-artifact":true},
            "tenant":{"type":"string"}
        },"required":["prompt","tenant"]});
        let sensitive = json!({"type":"object","properties":{
            "answer":{"type":"string","x-agentx-sensitive":true},
            "reports":{"type":"object","x-agentx-artifact":true}
        }});
        assert!(validate_chat_mapping(&mapping(), &input, &sensitive).is_err());
        let safe = json!({"type":"object","properties":{
            "answer":{"type":"string"},
            "reports":{"type":"object","x-agentx-artifact":true}
        }});
        assert!(validate_chat_mapping(&mapping(), &input, &safe).is_ok());
    }
}
