//! Restricted design-time model effects. No Workflow execution or attempt is created.
use std::time::Duration;

use agentx_agent_core::{ModelPurposeV1, ToolOriginV1};
use agentx_runtime_contracts::{
    DesignModelDeltaV1, DesignModelEffectRequestV1, DesignModelReceiptRequestV1,
    DesignModelReceiptV1, EgressRole, RuntimeResourceConfigurationV1, RuntimeResourceKindV1,
    content_hash, deterministic_uuid,
};
use axum::{Json, extract::State, http::HeaderMap};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    RuntimeState,
    egress::{EgressRequestContext, ProviderHttpClient},
    error::{RuntimeError, RuntimeResult},
};

const TOOLS: [&str; 7] = [
    "workflow_read",
    "node_catalog",
    "resource_query",
    "workflow_edit",
    "workflow_validate",
    "execution_inspect",
    "ask_user",
];

fn authorize(
    state: &RuntimeState,
    headers: &HeaderMap,
    scope: &str,
    body: &Value,
    target: &agentx_runtime_contracts::StudioBuilderTargetV1,
) -> RuntimeResult<()> {
    let claims = state.trust.publisher(headers, scope)?;
    let hash = content_hash(body).map_err(|e| RuntimeError::Internal(e.into()))?;
    if claims.sub != format!("studio-builder:{}:{}", target.tenant_id, target.run_id)
        || !claims
            .scope
            .contains(&format!("studio-body:{}", hash.as_str()))
    {
        return Err(RuntimeError::Unauthorized);
    }
    Ok(())
}

pub async fn submit(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<DesignModelEffectRequestV1>,
) -> RuntimeResult<Json<DesignModelReceiptV1>> {
    let value = serde_json::to_value(&request).map_err(RuntimeError::from)?;
    authorize(
        &state,
        &headers,
        "runtime.studio-model.submit",
        &value,
        &request.target,
    )?;
    if request.protocol_version != 1
        || request.binding.resource_kind != RuntimeResourceKindV1::Model
        || request.context.run_id != request.target.run_id.to_string()
        || request.context.deadline_at_millis <= now_millis()
        || request.request.tools.iter().any(|tool| {
            tool.origin != ToolOriginV1::Platform || !TOOLS.contains(&tool.name.as_str())
        })
        || (request.request.purpose == ModelPurposeV1::Compaction
            && !request.request.tools.is_empty())
    {
        return Err(RuntimeError::InvalidRequest(
            "DESIGN_MODEL_CAPABILITY_DENIED",
            "Invalid design-time model request".into(),
        ));
    }
    if serde_json::to_vec(&value)
        .map_err(RuntimeError::from)?
        .len()
        > 8 * 1024 * 1024
    {
        return Err(RuntimeError::InvalidRequest(
            "DESIGN_MODEL_CONTEXT_TOO_LARGE",
            "Model context exceeds the design request budget".into(),
        ));
    }
    let effect = deterministic_uuid(request.target.run_id, request.context.effect_id.as_bytes());
    // Fencing changes on host takeover without changing effect identity or its immutable request.
    let mut fingerprint_value = value.clone();
    fingerprint_value["context"]["fencingToken"] = json!(0);
    let fingerprint =
        content_hash(&fingerprint_value).map_err(|e| RuntimeError::Internal(e.into()))?;
    let mut tx = state.pool.begin().await?;
    if let Some(row) = sqlx::query("SELECT request_fingerprint,status FROM runtime_calls WHERE tenant_id=? AND id=? FOR UPDATE").bind(request.target.tenant_id).bind(effect).fetch_optional(&mut *tx).await? {
        if row.try_get::<String,_>("status")? != "cancelled" && row.try_get::<String,_>("request_fingerprint")? != fingerprint.as_str().trim_start_matches("sha256:") {
            return Err(RuntimeError::InvalidRequest("DESIGN_MODEL_IDEMPOTENCY_CONFLICT", "Effect identity belongs to a different request".into()));
        }
    } else {
        let output=request.parameters.get("max_tokens").or_else(||request.parameters.get("max_completion_tokens")).and_then(Value::as_u64).ok_or_else(||RuntimeError::InvalidRequest("DESIGN_MODEL_BUDGET_INVALID","A frozen output token limit is required".into()))?;
        let input=agentx_agent_core::ContextProjector::estimated_tokens(&request.request.messages).saturating_add(request.request.tools.iter().map(|tool|(tool.input_schema.to_string().len() as u64).div_ceil(4)).sum::<u64>());
        let mut reservation=json!({"usage":{"prompt_tokens":input,"completion_tokens":output}});
        let price=match &request.binding.configuration {RuntimeResourceConfigurationV1::Model{price,..}=>price,_=>return Err(RuntimeError::Unauthorized)};
        let (_,_,cost)=crate::worker_runtime::output::apply_model_price(&mut reservation,price).map_err(|_|RuntimeError::InvalidRequest("DESIGN_MODEL_PRICE_INVALID","The frozen price is invalid".into()))?;
        for (dimension,amount) in [("tokens",input.saturating_add(output)),("cost_micros",cost)] {
            crate::quota::reserve(&mut tx,request.target.tenant_id,dimension,"studio_builder_model",&effect.to_string(),&format!("studio:{effect}:{dimension}"),amount,150).await?;
        }
        sqlx::query("INSERT INTO runtime_calls(id,tenant_id,source_kind,studio_target_json,studio_model_binding_json,call_index,call_kind,idempotency_key,request_fingerprint,resource_type,resource_id,resource_version_id,status,request_json) VALUES(?,?,'studio_builder',?,?,0,?,?,?,'model',?,?,'reserved',?)")
            .bind(effect).bind(request.target.tenant_id).bind(serde_json::to_value(&request.target).map_err(RuntimeError::from)?)
            .bind(serde_json::to_value(&request.binding).map_err(RuntimeError::from)?)
            .bind(if request.request.purpose == ModelPurposeV1::Compaction {"compaction"} else {"model"}).bind(format!("studio-model:{effect}")).bind(fingerprint.as_str().trim_start_matches("sha256:"))
            .bind(request.binding.resource_id).bind(&request.binding.resource_version).bind(&value).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let task_state = state.clone();
    let task_request = request.clone();
    tokio::spawn(async move {
        if let Err(error) = execute(&task_state, effect, &task_request).await {
            tracing::warn!(effect_id=%effect, error=%error, "design model effect failed");
            let code = match &error {
                RuntimeError::InvalidRequest(code, _)
                | RuntimeError::Deterministic { code, .. } => *code,
                RuntimeError::SecretUnavailable => "DESIGN_MODEL_SECRET_UNAVAILABLE",
                _ => "DESIGN_MODEL_OUTCOME_UNKNOWN",
            };
            let status = if code == "DESIGN_MODEL_OUTCOME_UNKNOWN"
                || code == "DESIGN_MODEL_STREAM_INVALID"
            {
                "outcome_unknown"
            } else {
                "failed"
            };
            let _ = sqlx::query("UPDATE runtime_calls SET status=?,error_code=?,ended_at=UTC_TIMESTAMP(6) WHERE id=? AND source_kind='studio_builder' AND status IN ('reserved','sent')")
                .bind(status).bind(code).bind(effect).execute(&task_state.pool).await;
        }
    });
    receipt(
        &state,
        &DesignModelReceiptRequestV1 {
            target: request.target,
            effect_id: effect,
            after: 0,
        },
    )
    .await
    .map(Json)
}

pub async fn inspect(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<DesignModelReceiptRequestV1>,
) -> RuntimeResult<Json<DesignModelReceiptV1>> {
    authorize(
        &state,
        &headers,
        "runtime.studio-model.inspect",
        &serde_json::to_value(&request).map_err(RuntimeError::from)?,
        &request.target,
    )?;
    receipt(&state, &request).await.map(Json)
}

pub async fn cancel(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<DesignModelReceiptRequestV1>,
) -> RuntimeResult<Json<DesignModelReceiptV1>> {
    authorize(
        &state,
        &headers,
        "runtime.studio-model.cancel",
        &serde_json::to_value(&request).map_err(RuntimeError::from)?,
        &request.target,
    )?;
    let existing =
        sqlx::query("SELECT studio_target_json FROM runtime_calls WHERE tenant_id=? AND id=?")
            .bind(request.target.tenant_id)
            .bind(request.effect_id)
            .fetch_optional(&state.pool)
            .await?;
    if let Some(row) = existing {
        if serde_json::from_value::<agentx_runtime_contracts::StudioBuilderTargetV1>(
            row.try_get("studio_target_json")?,
        )
        .map_err(RuntimeError::from)?
            != request.target
        {
            return Err(RuntimeError::Unauthorized);
        }
    } else {
        // Cancellation can win the race with submission; the tombstone prevents a late send.
        sqlx::query("INSERT INTO runtime_calls(id,tenant_id,source_kind,studio_target_json,call_index,call_kind,idempotency_key,request_fingerprint,status,error_code,ended_at) VALUES(?,?,'studio_builder',?,0,'model',?,?,'cancelled','DESIGN_MODEL_CANCELLED',UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE id=id")
            .bind(request.effect_id).bind(request.target.tenant_id).bind(serde_json::to_value(&request.target).map_err(RuntimeError::from)?).bind(format!("studio-model:{}",request.effect_id)).bind("0".repeat(64)).execute(&state.pool).await?;
    }
    sqlx::query("UPDATE runtime_calls SET status='cancelled',error_code='DESIGN_MODEL_CANCELLED',ended_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND source_kind='studio_builder' AND status IN ('reserved','sent')")
        .bind(request.target.tenant_id).bind(request.effect_id).execute(&state.pool).await?;
    receipt(&state, &request).await.map(Json)
}

async fn receipt(
    state: &RuntimeState,
    request: &DesignModelReceiptRequestV1,
) -> RuntimeResult<DesignModelReceiptV1> {
    // A sent call never gets replayed after process loss. Its bounded unknown outcome is explicit.
    sqlx::query("UPDATE runtime_calls SET status='outcome_unknown',error_code='DESIGN_MODEL_OUTCOME_UNKNOWN',ended_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND source_kind='studio_builder' AND status='sent' AND started_at < UTC_TIMESTAMP(6) - INTERVAL 150 SECOND")
        .bind(request.target.tenant_id).bind(request.effect_id).execute(&state.pool).await?;
    let row = sqlx::query("SELECT studio_target_json,status,response_json,error_code,input_tokens,output_tokens,cost_micros,cost_currency,usage_estimated FROM runtime_calls WHERE tenant_id=? AND id=? AND source_kind='studio_builder'")
        .bind(request.target.tenant_id).bind(request.effect_id).fetch_optional(&state.pool).await?.ok_or(RuntimeError::NotFound)?;
    let target: agentx_runtime_contracts::StudioBuilderTargetV1 =
        serde_json::from_value(row.try_get("studio_target_json")?).map_err(RuntimeError::from)?;
    if target != request.target {
        return Err(RuntimeError::Unauthorized);
    }
    let deltas = sqlx::query("SELECT id,text FROM studio_model_deltas WHERE tenant_id=? AND call_id=? AND id>? ORDER BY id LIMIT 200")
        .bind(target.tenant_id).bind(request.effect_id).bind(request.after).fetch_all(&state.pool).await?.into_iter().map(|row| Ok(DesignModelDeltaV1 {sequence:row.try_get("id")?,text:row.try_get("text")?})).collect::<Result<Vec<_>,sqlx::Error>>()?;
    Ok(DesignModelReceiptV1 {
        effect_id: request.effect_id,
        status: row.try_get("status")?,
        response: row
            .try_get::<Option<Value>, _>("response_json")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(RuntimeError::from)?,
        error_code: row.try_get("error_code")?,
        usage_known: row
            .try_get::<u64, _>("input_tokens")?
            .saturating_add(row.try_get("output_tokens")?)
            > 0
            || row.try_get::<String, _>("status")? == "succeeded",
        usage_estimated: row.try_get("usage_estimated")?,
        input_tokens: row.try_get("input_tokens")?,
        output_tokens: row.try_get("output_tokens")?,
        cost_micros: row.try_get("cost_micros")?,
        currency: row.try_get("cost_currency")?,
        deltas,
    })
}

async fn execute(
    state: &RuntimeState,
    effect: Uuid,
    request: &DesignModelEffectRequestV1,
) -> RuntimeResult<()> {
    let changed = sqlx::query("UPDATE runtime_calls SET status='sent',started_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND source_kind='studio_builder' AND status='reserved'")
        .bind(request.target.tenant_id).bind(effect).execute(&state.pool).await?;
    if changed.rows_affected() == 0 {
        return Ok(());
    }
    let RuntimeResourceConfigurationV1::Model {
        provider,
        endpoint,
        model,
        credential,
        price,
        ..
    } = &request.binding.configuration
    else {
        return Err(RuntimeError::Unauthorized);
    };
    if provider != "openai_compatible" {
        return Err(RuntimeError::Unauthorized);
    }
    let client =
        ProviderHttpClient::from_env(EgressRole::RuntimeGateway).map_err(RuntimeError::Internal)?;
    let mut body = crate::agent_model_protocol::request(&request.request, model, true);
    if let Some(parameters) = request.parameters.as_object() {
        for name in [
            "temperature",
            "top_p",
            "max_tokens",
            "max_completion_tokens",
            "thinking",
            "reasoning_effort",
        ] {
            if let Some(value) = parameters.get(name) {
                body[name] = value.clone();
            }
        }
    }
    let mut builder = client
        .post(
            &crate::worker_support::openai_chat_completions_endpoint(endpoint),
            EgressRequestContext::request(request.target.tenant_id, effect),
            Duration::from_secs(120),
        )
        .map_err(RuntimeError::Internal)?;
    if let Some(secret) = credential {
        let bytes = state
            .vault
            .as_ref()
            .ok_or(RuntimeError::SecretUnavailable)?
            .read(secret)
            .await?;
        let header = reqwest::header::HeaderValue::from_bytes(
            &crate::worker_support::provider_secret_header(&bytes, "authorization"),
        )
        .map_err(|_| RuntimeError::SecretUnavailable)?;
        builder = builder.header("authorization", header);
    }
    let provider = async {
        let response = builder
            .json(&body)
            .send()
            .await
            .map_err(|error| if error.is_connect() { RuntimeError::InvalidRequest("DESIGN_MODEL_CONNECTION_DENIED", "The model connection could not be established through the configured egress gateway".into()) } else { RuntimeError::Unavailable })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .bytes()
                .await
                .map_err(|_| RuntimeError::Unavailable)?;
            let code = if status == reqwest::StatusCode::BAD_REQUEST
                && body.len() <= 65536
                && serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|value| {
                        value
                            .pointer("/error/code")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .is_some_and(|code| {
                        matches!(
                            code.as_str(),
                            "context_length_exceeded"
                                | "max_context_length_exceeded"
                                | "context_window_exceeded"
                        )
                    }) {
                "DESIGN_MODEL_CONTEXT_OVERFLOW"
            } else {
                "DESIGN_MODEL_PROVIDER_REJECTED"
            };
            return Err(RuntimeError::InvalidRequest(
                code,
                format!("Model provider returned HTTP {status}"),
            ));
        }
        let mut delta = |text: String, _reasoning: Option<String>| async move {
            if !text.is_empty() {
                sqlx::query(
                    "INSERT INTO studio_model_deltas(tenant_id,call_id,text) VALUES(?,?,?)",
                )
                .bind(request.target.tenant_id)
                .bind(effect)
                .bind(text)
                .execute(&state.pool)
                .await
                .map_err(|_| "Design stream persistence failed".to_owned())?;
            }
            Ok(())
        };
        let mut payload =
            crate::worker_runtime::stream::aggregate_openai_sse_stream(response, &body, &mut delta)
                .await
                .map_err(|_| {
                    RuntimeError::InvalidRequest(
                        "DESIGN_MODEL_STREAM_INVALID",
                        "Model stream did not complete".into(),
                    )
                })?;
        let (input, output, cost) =
            crate::worker_runtime::output::apply_model_price(&mut payload, price).map_err(
                |_| {
                    RuntimeError::InvalidRequest(
                        "DESIGN_MODEL_PRICE_INVALID",
                        "Model price or usage is invalid".into(),
                    )
                },
            )?;
        let estimated = payload
            .get("usage_estimated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut usage_tx = state.pool.begin().await?;
        sqlx::query("UPDATE runtime_calls SET input_tokens=?,output_tokens=?,cost_micros=?,cost_currency=?,usage_estimated=? WHERE tenant_id=? AND id=? AND status IN ('sent','cancelled')").bind(input).bind(output).bind(cost).bind(&price.currency).bind(estimated).bind(request.target.tenant_id).bind(effect).execute(&mut *usage_tx).await?;
        for (dimension, amount) in [
            ("tokens", input.saturating_add(output)),
            ("cost_micros", cost),
        ] {
            crate::quota::settle(
                &mut usage_tx,
                request.target.tenant_id,
                dimension,
                "studio_builder_model",
                &effect.to_string(),
                amount,
                &format!("studio:{effect}:usage:{dimension}"),
            )
            .await?;
        }
        usage_tx.commit().await?;
        let result = crate::agent_model_protocol::response(&payload, format!("assistant:{effect}"))
            .map_err(|_| {
                RuntimeError::InvalidRequest(
                    "DESIGN_MODEL_RESPONSE_INVALID",
                    "Model response is invalid".into(),
                )
            })?;
        sqlx::query("UPDATE runtime_calls SET status='succeeded',response_json=?,input_tokens=?,output_tokens=?,cost_micros=?,cost_currency=?,ended_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND id=? AND source_kind='studio_builder' AND status='sent'")
            .bind(serde_json::to_value(result).map_err(RuntimeError::from)?).bind(input).bind(output).bind(cost).bind(&price.currency).bind(request.target.tenant_id).bind(effect).execute(&state.pool).await?;
        Ok::<_, RuntimeError>(())
    };
    let cancelled = async {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let status: String =
                sqlx::query_scalar("SELECT status FROM runtime_calls WHERE tenant_id=? AND id=?")
                    .bind(request.target.tenant_id)
                    .bind(effect)
                    .fetch_one(&state.pool)
                    .await?;
            if status != "sent" {
                return Ok::<_, RuntimeError>(());
            }
        }
    };
    tokio::select! { result = provider => result, result = cancelled => result }
}

fn now_millis() -> u64 {
    time::OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .max(0) as u64
        / 1_000_000
}
