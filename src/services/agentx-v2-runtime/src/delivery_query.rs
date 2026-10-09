//! Delivery query API (plan7 P7-A): serves the delivery outbox and dead
//! letters to the control BFF over the delegated internal query surface.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliverySearchRequestV1 {
    pub tenant_id: Uuid,
    #[serde(default)]
    pub application_id: Option<Uuid>,
    #[serde(default)]
    pub invocation_id: Option<Uuid>,
    #[serde(default)]
    pub execution_id: Option<Uuid>,
    /// pending / delivering / delivered / failed / dead
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    50
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverySummaryV1 {
    pub id: Uuid,
    pub application_id: Option<Uuid>,
    pub invocation_id: Option<Uuid>,
    pub execution_id: Uuid,
    pub channel_binding_id: Uuid,
    pub provider: String,
    pub origin: String,
    pub status: String,
    pub attempt_count: u32,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub provider_message_id: Option<String>,
    #[serde(rename = "payload")]
    pub payload: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverySearchPageV1 {
    pub api_version: u32,
    pub items: Vec<DeliverySummaryV1>,
}

pub async fn search_deliveries(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<DeliverySearchRequestV1>,
) -> RuntimeResult<Json<DeliverySearchPageV1>> {
    let claims = state
        .trust
        .delegation(&headers, request.tenant_id, "runtime.query.deliveries")?;
    if let Some(application_id) = request.application_id {
        if !claims.tenant_wide && !claims.application_ids.contains(&application_id) {
            return Err(RuntimeError::Unauthorized);
        }
    } else if !claims.tenant_wide {
        return Err(RuntimeError::Unauthorized);
    }
    let limit = request.limit.clamp(1, 100);
    let mut items: Vec<DeliverySummaryV1> = Vec::new();
    for (index, sql) in [
        "SELECT 'outbox' source,id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,attempt_count,COALESCE(status,'pending') status,last_error_code,last_error_message,provider_message_id,payload_json,DATE_FORMAT(created_at,'%Y-%m-%dT%H:%i:%s.%fZ') created_at,DATE_FORMAT(updated_at,'%Y-%m-%dT%H:%i:%s.%fZ') updated_at FROM delivery_outbox WHERE tenant_id=?",
        "SELECT 'dead' source,id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,attempt_count,'dead' status,last_error_code,last_error_message,provider_message_id,payload_json,DATE_FORMAT(created_at,'%Y-%m-%dT%H:%i:%s.%fZ') created_at,DATE_FORMAT(dead_at,'%Y-%m-%dT%H:%i:%s.%fZ') updated_at FROM delivery_dead_letters WHERE tenant_id=?",
    ]
    .into_iter()
    .enumerate()
    {
        let status_column = if index == 0 { "status" } else { "'dead'" };
        let statement = format!("{sql} AND (? IS NULL OR application_id=?) AND (? IS NULL OR invocation_id=?) AND (? IS NULL OR execution_id=?) AND (? IS NULL OR {status_column}=?) ORDER BY created_at DESC LIMIT ?");
        let query = sqlx::query(&statement)
            .bind(request.tenant_id)
            .bind(request.application_id).bind(request.application_id)
            .bind(request.invocation_id).bind(request.invocation_id)
            .bind(request.execution_id).bind(request.execution_id)
            .bind(&request.status).bind(&request.status)
            .bind(limit);
        let rows = query.fetch_all(&state.pool).await?;
        for row in rows {
            items.push(delivery_summary(row)?);
        }
        if index == 0 && request.status.as_deref() == Some("dead") {
            // outbox rows never carry 'dead'; skip the empty result noise.
        }
    }
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items.truncate(limit as usize);
    Ok(Json(DeliverySearchPageV1 {
        api_version: 1,
        items,
    }))
}

pub async fn get_delivery(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(delivery_id): Path<Uuid>,
) -> RuntimeResult<Json<Value>> {
    let row = sqlx::query(
        "SELECT BIN_TO_UUID(tenant_id) tenant_id FROM delivery_outbox WHERE id=? UNION ALL SELECT BIN_TO_UUID(tenant_id) FROM delivery_dead_letters WHERE id=? LIMIT 1",
    )
    .bind(delivery_id)
    .bind(delivery_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(RuntimeError::NotFound);
    };
    let tenant_id: String = row.try_get("tenant_id")?;
    let tenant_id =
        Uuid::parse_str(&tenant_id).map_err(|error| RuntimeError::Internal(error.into()))?;
    state
        .trust
        .delegation(&headers, tenant_id, "runtime.query.deliveries")?;
    for sql in [
        "SELECT 'outbox' source,id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,attempt_count,COALESCE(status,'pending') status,last_error_code,last_error_message,provider_message_id,payload_json,target_json,credential_ref_json,DATE_FORMAT(created_at,'%Y-%m-%dT%H:%i:%s.%fZ') created_at,DATE_FORMAT(updated_at,'%Y-%m-%dT%H:%i:%s.%fZ') updated_at FROM delivery_outbox WHERE tenant_id=? AND id=?",
        "SELECT 'dead' source,id,tenant_id,application_id,invocation_id,execution_id,channel_binding_id,provider,origin,attempt_count,'dead' status,last_error_code,last_error_message,provider_message_id,payload_json,target_json,credential_ref_json,DATE_FORMAT(created_at,'%Y-%m-%dT%H:%i:%s.%fZ') created_at,DATE_FORMAT(dead_at,'%Y-%m-%dT%H:%i:%s.%fZ') updated_at FROM delivery_dead_letters WHERE tenant_id=? AND id=?",
    ] {
        if let Some(row) = sqlx::query(sql)
            .bind(tenant_id)
            .bind(delivery_id)
            .fetch_optional(&state.pool)
            .await?
        {
            let credential_ref: Option<Value> = row.try_get("credential_ref_json")?;
            return Ok(Json(json!({
                "apiVersion": 1,
                "id": row.try_get::<Uuid, _>("id")?,
                "applicationId": row.try_get::<Option<Uuid>, _>("application_id")?,
                "invocationId": row.try_get::<Option<Uuid>, _>("invocation_id")?,
                "executionId": row.try_get::<Uuid, _>("execution_id")?,
                "channelBindingId": row.try_get::<Uuid, _>("channel_binding_id")?,
                "provider": row.try_get::<String, _>("provider")?,
                "origin": row.try_get::<String, _>("origin")?,
                "status": row.try_get::<String, _>("status")?,
                "attemptCount": row.try_get::<u32, _>("attempt_count")?,
                "lastErrorCode": row.try_get::<Option<String>, _>("last_error_code")?,
                "lastErrorMessage": row.try_get::<Option<String>, _>("last_error_message")?,
                "providerMessageId": row.try_get::<Option<String>, _>("provider_message_id")?,
                "payload": row.try_get::<Value, _>("payload_json")?,
                // The target snapshot carries conversation addresses; the
                // credential reference is a Vault pointer only and never a
                // secret, but it stays out of the API surface anyway.
                "target": redact_target(&row.try_get::<Value, _>("target_json")?),
                "hasCredentialSnapshot": credential_ref.is_some(),
                "createdAt": row.try_get::<String, _>("created_at")?,
                "updatedAt": row.try_get::<String, _>("updated_at")?,
            })));
        }
    }
    Err(RuntimeError::NotFound)
}

pub async fn retry_delivery(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Path(delivery_id): Path<Uuid>,
) -> RuntimeResult<Json<Value>> {
    let row = sqlx::query(
        "SELECT BIN_TO_UUID(tenant_id) tenant_id FROM delivery_dead_letters WHERE id=?",
    )
    .bind(delivery_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Err(RuntimeError::NotFound);
    };
    let tenant_id: String = row.try_get("tenant_id")?;
    let tenant_id =
        Uuid::parse_str(&tenant_id).map_err(|error| RuntimeError::Internal(error.into()))?;
    state
        .trust
        .delegation(&headers, tenant_id, "runtime.delivery.retry")?;
    let replayed = crate::delivery::replay_dead_letter(&state.pool, tenant_id, delivery_id).await?;
    Ok(Json(
        json!({"apiVersion": 1, "deliveryId": replayed, "status": "pending"}),
    ))
}

fn redact_target(target: &Value) -> Value {
    // sessionWebhook URLs are short-lived credentials; drop them from the API
    // echo while keeping addressing fields visible.
    let mut redacted = target.clone();
    if let Some(object) = redacted.as_object_mut() {
        object.remove("sessionWebhook");
    }
    redacted
}

fn delivery_summary(row: sqlx::mysql::MySqlRow) -> RuntimeResult<DeliverySummaryV1> {
    Ok(DeliverySummaryV1 {
        id: row.try_get("id")?,
        application_id: row.try_get("application_id")?,
        invocation_id: row.try_get("invocation_id")?,
        execution_id: row.try_get("execution_id")?,
        channel_binding_id: row.try_get("channel_binding_id")?,
        provider: row.try_get("provider")?,
        origin: row.try_get("origin")?,
        status: row.try_get("status")?,
        attempt_count: row.try_get("attempt_count")?,
        last_error_code: row.try_get("last_error_code")?,
        last_error_message: row.try_get("last_error_message")?,
        provider_message_id: row.try_get("provider_message_id")?,
        payload: row.try_get("payload_json")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
