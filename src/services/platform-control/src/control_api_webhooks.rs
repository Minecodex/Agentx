use super::*;

pub(super) async fn list_webhooks(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application_id): Path<Uuid>,
) -> ApiResult<Json<Vec<WebhookResponse>>> {
    actor.require("application:view")?;
    require_application(&state, &actor, application_id).await?;
    let rows = sqlx::query("SELECT id,name,public_id,status,version,provider_type,channel_mode,channel_config_json,reply_config_json,input_mapping_json,fixed_inputs_json,configuration_revision FROM application_webhooks WHERE tenant_id=? AND application_id=? ORDER BY created_at DESC")
        .bind(actor.tenant_id).bind(application_id).fetch_all(&state.pool).await?;
    let mut responses = rows
        .into_iter()
        .map(application_webhooks::response_from_row)
        .collect::<ApiResult<Vec<_>>>()?;
    attach_channel_status(&state, &actor, application_id, &mut responses).await;
    Ok(Json(responses))
}

/// Best-effort stream connection status from the Runtime Gateway. Channel
/// listing must keep working when Runtime is unavailable, so failures degrade
/// to an absent status instead of an error.
async fn attach_channel_status(
    state: &ControlApiState,
    actor: &Actor,
    application_id: Uuid,
    responses: &mut [WebhookResponse],
) {
    if !responses
        .iter()
        .any(|response| response.channel_mode == "stream")
    {
        return;
    }
    let payload = json!({"operation":"channel_status","tenantId":actor.tenant_id,"applicationId":application_id});
    let Ok(request_hash) = agentx_runtime_contracts::content_hash(&payload) else {
        return;
    };
    let Ok(token) = delegation_token(
        state,
        actor,
        BTreeSet::from(["runtime.channels.status".into()]),
        BTreeSet::from([application_id]),
        BTreeSet::new(),
        request_hash,
    ) else {
        return;
    };
    let Ok(response) = state
        .http
        .get(format!(
            "{}/internal/runtime/v1/channel-status?tenantId={}&applicationId={}",
            state.runtime_query_url, actor.tenant_id, application_id
        ))
        .bearer_auth(token)
        .send()
        .await
    else {
        return;
    };
    let Ok(body) = response.json::<Value>().await else {
        return;
    };
    let Some(channels) = body.get("channels").and_then(Value::as_array) else {
        return;
    };
    for channel in channels {
        let Some(id) = channel
            .get("id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
        else {
            continue;
        };
        if let Some(response) = responses.iter_mut().find(|response| response.id == id) {
            response.connection_status = channel
                .get("connectionStatus")
                .and_then(Value::as_str)
                .map(str::to_owned);
            response.connection_error = channel
                .get("connectionError")
                .and_then(Value::as_str)
                .map(str::to_owned);
            response.last_connected_at = channel
                .get("lastConnectedAt")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
    }
}

pub(super) async fn create_webhook(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application_id): Path<Uuid>,
    Json(input): Json<CreateWebhookRequest>,
) -> ApiResult<(StatusCode, Json<WebhookResponse>)> {
    actor.require("application:manage")?;
    require_application(&state, &actor, application_id).await?;
    let name = required_name(&input.name)?;
    application_webhooks::validate(
        &state,
        &actor,
        application_id,
        &input.provider_type,
        &input.channel_mode,
        input.channel_config.as_ref(),
        input.reply.as_ref(),
        &input.input_mappings,
        &input.fixed_inputs,
    )
    .await?;
    let webhook_id = Uuid::now_v7();
    let public_id = random_url_token(18);
    let (secret, secret_reference, channel_config_json) = if input.provider_type == "agentx" {
        let secret = random_url_token(32);
        let reference = write_webhook_secret(&state, &actor, webhook_id, secret.as_bytes()).await?;
        (Some(secret), reference, json!({}))
    } else {
        let template = webhook_provider_templates::find(&input.provider_type, &input.channel_mode)
            .ok_or_else(|| {
                ApiError::bad_request(
                    "INVALID_WEBHOOK_CHANNEL_MODE",
                    "Webhook channel mode is not available for this provider",
                )
            })?;
        let (full, public) = webhook_provider_templates::merge_fields(
            &template.fields,
            input.channel_config.as_ref(),
            None,
        )?;
        let payload = serde_json::to_string(&Value::Object(full)).map_err(ApiError::internal)?;
        let reference =
            write_webhook_secret(&state, &actor, webhook_id, payload.as_bytes()).await?;
        (None, reference, Value::Object(public))
    };
    let mut tx = state.pool.begin().await?;
    let reply_json = input
        .reply
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO application_webhooks(id,tenant_id,application_id,name,public_id,secret_ref_json,provider_type,channel_mode,channel_config_json,reply_config_json,input_mapping_json,fixed_inputs_json,status,configuration_revision,configuration_hash,created_by) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,'active',1,?,?)")
        .bind(webhook_id).bind(actor.tenant_id).bind(application_id).bind(name).bind(&public_id)
        .bind(serde_json::to_value(&secret_reference).map_err(ApiError::internal)?)
        .bind(&input.provider_type).bind(&input.channel_mode).bind(&channel_config_json)
        .bind(&reply_json)
        .bind(serde_json::to_value(&input.input_mappings).map_err(ApiError::internal)?)
        .bind(if input.fixed_inputs.is_null() { json!({}) } else { input.fixed_inputs.clone() })
        .bind(agentx_runtime_contracts::content_hash(&json!({"publicId":public_id,"secret":secret_reference,"provider":input.provider_type,"mode":input.channel_mode,"reply":reply_json,"mapping":input.input_mappings,"fixed":input.fixed_inputs,"enabled":true})).map_err(ApiError::internal)?.as_str())
        .bind(actor.user_id).execute(&mut *tx).await?;
    rebuild_trigger_revision(&mut tx, &actor, application_id).await?;
    tx.commit().await?;
    let mut response =
        application_webhooks::load(&state, actor.tenant_id, application_id, webhook_id).await?;
    response.secret = secret;
    Ok((StatusCode::CREATED, Json(response)))
}

pub(super) async fn get_webhook(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application_id, webhook_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<WebhookResponse>> {
    actor.require("application:view")?;
    require_application(&state, &actor, application_id).await?;
    let mut response =
        application_webhooks::load(&state, actor.tenant_id, application_id, webhook_id).await?;
    attach_channel_status(
        &state,
        &actor,
        application_id,
        std::slice::from_mut(&mut response),
    )
    .await;
    Ok(Json(response))
}

pub(super) async fn update_webhook(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application_id, webhook_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<UpdateWebhookRequest>,
) -> ApiResult<Json<WebhookResponse>> {
    actor.require("application:manage")?;
    require_application(&state, &actor, application_id).await?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_WEBHOOK_STATUS",
            "Webhook status is invalid",
        ));
    }
    application_webhooks::validate(
        &state,
        &actor,
        application_id,
        &input.provider_type,
        &input.channel_mode,
        input.channel_config.as_ref(),
        input.reply.as_ref(),
        &input.input_mappings,
        &input.fixed_inputs,
    )
    .await?;
    let existing_row = sqlx::query("SELECT provider_type,channel_mode,secret_ref_json FROM application_webhooks WHERE tenant_id=? AND application_id=? AND id=?")
        .bind(actor.tenant_id).bind(application_id).bind(webhook_id).fetch_optional(&state.pool).await?
        .ok_or_else(|| ApiError::not_found("Webhook"))?;
    let (secret, secret_reference, channel_config_json) = if input.provider_type == "agentx" {
        let secret = random_url_token(32);
        let reference = write_webhook_secret(&state, &actor, webhook_id, secret.as_bytes()).await?;
        (Some(secret), reference, json!({}))
    } else {
        let template = webhook_provider_templates::find(&input.provider_type, &input.channel_mode)
            .ok_or_else(|| {
                ApiError::bad_request(
                    "INVALID_WEBHOOK_CHANNEL_MODE",
                    "Webhook channel mode is not available for this provider",
                )
            })?;
        let same_channel = existing_row.try_get::<String, _>("provider_type")?
            == input.provider_type
            && existing_row
                .try_get::<Option<String>, _>("channel_mode")?
                .as_deref()
                .unwrap_or("callback")
                == input.channel_mode;
        let existing_fields = if same_channel {
            let reference: VaultSecretReferenceV1 =
                serde_json::from_value(existing_row.try_get("secret_ref_json")?)
                    .map_err(ApiError::internal)?;
            read_webhook_secret_fields(&state, &reference).await?
        } else {
            None
        };
        let (full, public) = webhook_provider_templates::merge_fields(
            &template.fields,
            input.channel_config.as_ref(),
            existing_fields.as_ref(),
        )?;
        let payload = serde_json::to_string(&Value::Object(full)).map_err(ApiError::internal)?;
        let reference =
            write_webhook_secret(&state, &actor, webhook_id, payload.as_bytes()).await?;
        (None, reference, Value::Object(public))
    };
    let mut tx = state.pool.begin().await?;
    let reply_json = input
        .reply
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(ApiError::internal)?;
    let changed = sqlx::query("UPDATE application_webhooks SET name=?,status=?,provider_type=?,channel_mode=?,channel_config_json=?,reply_config_json=?,input_mapping_json=?,fixed_inputs_json=?,secret_ref_json=?,version=version+1,configuration_revision=configuration_revision+1 WHERE tenant_id=? AND application_id=? AND id=? AND version=?")
        .bind(required_name(&input.name)?).bind(&input.status).bind(&input.provider_type).bind(&input.channel_mode).bind(&channel_config_json).bind(&reply_json).bind(serde_json::to_value(&input.input_mappings).map_err(ApiError::internal)?)
        .bind(if input.fixed_inputs.is_null() { json!({}) } else { input.fixed_inputs.clone() }).bind(serde_json::to_value(&secret_reference).map_err(ApiError::internal)?).bind(actor.tenant_id).bind(application_id).bind(webhook_id).bind(input.version).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "WEBHOOK_VERSION_CONFLICT",
            "Webhook changed on the server",
        ));
    }
    let row = sqlx::query("SELECT public_id,secret_ref_json,configuration_revision,status,provider_type,channel_mode,input_mapping_json,fixed_inputs_json,reply_config_json FROM application_webhooks WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id).bind(webhook_id).fetch_one(&mut *tx).await?;
    let hash = agentx_runtime_contracts::content_hash(&json!({"publicId":row.try_get::<String,_>("public_id")?,"secret":row.try_get::<Value,_>("secret_ref_json")?,"provider":row.try_get::<String,_>("provider_type")?,"mode":row.try_get::<Option<String>,_>("channel_mode")?,"mapping":row.try_get::<Option<Value>,_>("input_mapping_json")?,"fixed":row.try_get::<Option<Value>,_>("fixed_inputs_json")?,"reply":row.try_get::<Option<Value>,_>("reply_config_json")?,"revision":row.try_get::<u64,_>("configuration_revision")?,"enabled":row.try_get::<String,_>("status")? == "active"})).map_err(ApiError::internal)?;
    sqlx::query("UPDATE application_webhooks SET configuration_hash=? WHERE tenant_id=? AND id=?")
        .bind(hash.as_str())
        .bind(actor.tenant_id)
        .bind(webhook_id)
        .execute(&mut *tx)
        .await?;
    rebuild_trigger_revision(&mut tx, &actor, application_id).await?;
    tx.commit().await?;
    let mut response =
        application_webhooks::load(&state, actor.tenant_id, application_id, webhook_id).await?;
    response.secret = secret;
    Ok(Json(response))
}

pub(super) async fn delete_webhook(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application_id, webhook_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    actor.require("application:delete")?;
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("DELETE FROM application_webhooks WHERE tenant_id=? AND application_id=? AND id=? AND version=?")
        .bind(actor.tenant_id).bind(application_id).bind(webhook_id).bind(query.expected_version).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "VERSION_CONFLICT",
            "Webhook changed before deletion",
        ));
    }
    rebuild_trigger_revision(&mut tx, &actor, application_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct VaultWriteResponse {
    data: VaultWriteData,
}

#[derive(Deserialize)]
struct VaultWriteData {
    version: u64,
}

async fn write_webhook_secret(
    state: &ControlApiState,
    actor: &Actor,
    webhook_id: Uuid,
    secret: &[u8],
) -> ApiResult<VaultSecretReferenceV1> {
    let path = format!("tenants/{}/webhooks/{webhook_id}", actor.tenant_id);
    let value = std::str::from_utf8(secret).map_err(ApiError::internal)?;
    let response = state
        .http
        .post(format!(
            "{}/v1/{}/data/{}",
            state.vault_endpoint,
            state.vault_mount.trim_matches('/'),
            path
        ))
        .header("X-Vault-Token", state.vault_token.expose_secret())
        .json(&json!({"data":{"value":value}}))
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Control Vault write failed");
            ApiError::unavailable("VAULT_UNAVAILABLE", "Webhook Secret could not be stored")
        })?;
    if !response.status().is_success() {
        tracing::warn!(status=%response.status(), "Control Vault rejected Webhook Secret write");
        return Err(ApiError::unavailable(
            "VAULT_UNAVAILABLE",
            "Webhook Secret could not be stored",
        ));
    }
    let response: VaultWriteResponse = response.json().await.map_err(ApiError::internal)?;
    Ok(VaultSecretReferenceV1 {
        mount: state.vault_mount.clone(),
        path,
        key: "value".into(),
        version: response.data.version,
    })
}

#[derive(Deserialize)]
struct VaultReadResponse {
    data: VaultReadValue,
}

#[derive(Deserialize)]
struct VaultReadValue {
    data: serde_json::Map<String, Value>,
}

async fn read_webhook_secret_fields(
    state: &ControlApiState,
    reference: &VaultSecretReferenceV1,
) -> ApiResult<Option<serde_json::Map<String, Value>>> {
    let response = state
        .http
        .get(format!(
            "{}/v1/{}/data/{}?version={}",
            state.vault_endpoint,
            reference.mount.trim_matches('/'),
            reference.path,
            reference.version
        ))
        .header("X-Vault-Token", state.vault_token.expose_secret())
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Control Vault read failed");
            ApiError::unavailable("VAULT_UNAVAILABLE", "Webhook Secret could not be read")
        })?;
    if !response.status().is_success() {
        tracing::warn!(status=%response.status(), "Control Vault rejected Webhook Secret read");
        return Err(ApiError::unavailable(
            "VAULT_UNAVAILABLE",
            "Webhook Secret could not be read",
        ));
    }
    let response: VaultReadResponse = response.json().await.map_err(ApiError::internal)?;
    let payload = response
        .data
        .data
        .get(&reference.key)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ApiError::unavailable("VAULT_UNAVAILABLE", "Webhook Secret could not be read")
        })?;
    Ok(Some(
        serde_json::from_str(payload).map_err(ApiError::internal)?,
    ))
}

pub(super) async fn list_webhook_provider_templates(
    State(_state): State<ControlApiState>,
    actor: Actor,
) -> ApiResult<Json<Vec<webhook_provider_templates::WebhookProviderTemplateV1>>> {
    actor.require("application:view")?;
    Ok(Json(webhook_provider_templates::templates()))
}

pub(super) async fn rebuild_trigger_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    actor: &Actor,
    application_id: Uuid,
) -> ApiResult<u64> {
    let current: u64 = sqlx::query_scalar(
        "SELECT runtime_config_revision FROM applications WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(actor.tenant_id)
    .bind(application_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Application"))?;
    let revision = current + 1;
    let mut triggers = Vec::new();
    let webhooks = sqlx::query("SELECT id,name,public_id,secret_ref_json,status,configuration_revision,provider_type,channel_mode,input_mapping_json,fixed_inputs_json,reply_config_json FROM application_webhooks WHERE tenant_id=? AND application_id=? ORDER BY id")
        .bind(actor.tenant_id).bind(application_id).fetch_all(&mut **tx).await?;
    for row in webhooks {
        let provider_type: Option<String> = row.try_get("provider_type")?;
        let channel_mode: Option<String> = row.try_get("channel_mode")?;
        let input_mappings = row
            .try_get::<Option<Value>, _>("input_mapping_json")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?
            .unwrap_or_default();
        let reply = row
            .try_get::<Option<Value>, _>("reply_config_json")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?;
        let configuration = RuntimeTriggerConfigurationV1::Webhook {
            public_id: row.try_get("public_id")?,
            secret: serde_json::from_value(row.try_get("secret_ref_json")?)
                .map_err(ApiError::internal)?,
            provider: application_webhooks::parse_provider(provider_type)?,
            mode: application_webhooks::parse_mode(channel_mode.as_deref().unwrap_or("callback"))?,
            input_mappings,
            fixed_inputs: row
                .try_get::<Option<Value>, _>("fixed_inputs_json")?
                .unwrap_or_else(|| json!({})),
            reply,
        };
        let hash =
            agentx_runtime_contracts::content_hash(&configuration).map_err(ApiError::internal)?;
        let trigger_id: Uuid = row.try_get("id")?;
        sqlx::query(
            "UPDATE application_webhooks SET configuration_hash=? WHERE tenant_id=? AND id=?",
        )
        .bind(hash.as_str())
        .bind(actor.tenant_id)
        .bind(trigger_id)
        .execute(&mut **tx)
        .await?;
        triggers.push(RuntimeTriggerSpecV1 {
            schema_version: 1,
            trigger_id,
            trigger_name: row.try_get("name")?,
            application_id,
            node_id: format!("webhook:{trigger_id}"),
            revision: row.try_get("configuration_revision")?,
            configuration_hash: hash,
            enabled: row.try_get::<String, _>("status")? == "active",
            configuration,
        });
    }
    let schedules = sqlx::query("SELECT id,name,cron_expression,timezone,input_json,misfire_policy,status,configuration_revision FROM application_schedules WHERE tenant_id=? AND application_id=? ORDER BY id")
        .bind(actor.tenant_id).bind(application_id).fetch_all(&mut **tx).await?;
    for row in schedules {
        let policy = match row.try_get::<String, _>("misfire_policy")?.as_str() {
            "skip" => ScheduleMisfirePolicyV1::Skip,
            "fire_once" => ScheduleMisfirePolicyV1::FireOnce,
            _ => return Err(ApiError::internal("invalid stored Schedule Misfire Policy")),
        };
        let configuration = RuntimeTriggerConfigurationV1::Schedule {
            cron_expression: row.try_get("cron_expression")?,
            timezone: row.try_get("timezone")?,
            misfire_policy: policy,
            grace_seconds: 60,
            input: row.try_get("input_json")?,
        };
        let hash =
            agentx_runtime_contracts::content_hash(&configuration).map_err(ApiError::internal)?;
        let trigger_id: Uuid = row.try_get("id")?;
        sqlx::query(
            "UPDATE application_schedules SET configuration_hash=? WHERE tenant_id=? AND id=?",
        )
        .bind(hash.as_str())
        .bind(actor.tenant_id)
        .bind(trigger_id)
        .execute(&mut **tx)
        .await?;
        triggers.push(RuntimeTriggerSpecV1 {
            schema_version: 1,
            trigger_id,
            trigger_name: row.try_get("name")?,
            application_id,
            node_id: format!("schedule:{trigger_id}"),
            revision: row.try_get("configuration_revision")?,
            configuration_hash: hash,
            enabled: row.try_get::<String, _>("status")? == "active",
            configuration,
        });
    }
    triggers.sort_by_key(|trigger| trigger.trigger_id);
    let manifest_hash =
        agentx_runtime_contracts::content_hash(&triggers).map_err(ApiError::internal)?;
    sqlx::query("INSERT INTO application_runtime_trigger_revisions(tenant_id,application_id,revision,manifest_hash,manifest_json,created_by) VALUES(?,?,?,?,?,?)")
        .bind(actor.tenant_id).bind(application_id).bind(revision).bind(manifest_hash.as_str())
        .bind(serde_json::to_value(&triggers).map_err(ApiError::internal)?).bind(actor.user_id)
        .execute(&mut **tx).await?;
    sqlx::query("UPDATE applications SET runtime_config_revision=? WHERE tenant_id=? AND id=?")
        .bind(revision)
        .bind(actor.tenant_id)
        .bind(application_id)
        .execute(&mut **tx)
        .await?;
    Ok(revision)
}
