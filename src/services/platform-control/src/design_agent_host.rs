use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    design_agent_runtime::{append_entry, inspect, submit},
};
use agentx_agent_core::{
    AgentCore, AgentRunInputV1, AgentSessionStateV1, BudgetDecision, BudgetPort, ClockPort,
    CoreEventV1, EffectContextV1, EffectObservationV1, EventPort, EventPortError, ModelPort,
    ModelPortError, ModelRequestV1, ModelResponseV1, StatePort, StatePortError, TerminalReasonV1,
    ToolPort,
};
use agentx_runtime_contracts::{
    DesignModelEffectRequestV1, DesignModelReceiptV1, DesignModelTargetV1, RuntimeResourceBindingV1,
};
use serde_json::{Value, json};
use sqlx::Row;
use std::time::Duration;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct RunContext {
    pub state: ControlApiState,
    pub target: DesignModelTargetV1,
    pub fence: u64,
    pub token_version: u64,
    pub binding: RuntimeResourceBindingV1,
    pub parameters: Value,
    pub deadline: OffsetDateTime,
}

impl RunContext {
    pub async fn authorize(&self) -> ApiResult<Actor> {
        let id = self.target.identity();
        let actor = crate::control_api::reload_actor(
            &self.state,
            id.tenant_id,
            id.user_id,
            self.token_version,
        )
        .await?;
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM design_agent_runs r JOIN design_agent_sessions s ON s.tenant_id=r.tenant_id AND s.id=r.session_id JOIN tenant_settings ts ON ts.tenant_id=r.tenant_id JOIN model_aliases a ON a.tenant_id=r.tenant_id AND a.id=r.model_alias_id JOIN model_deployments m ON m.tenant_id=a.tenant_id AND m.id=a.deployment_id JOIN model_deployments frozen ON frozen.tenant_id=r.tenant_id AND frozen.id=? LEFT JOIN credentials c ON c.tenant_id=r.tenant_id AND c.id=r.credential_id WHERE r.tenant_id=? AND r.id=? AND r.session_id=? AND s.user_id=? AND r.fencing_token=? AND r.status IN ('queued','running','waiting') AND r.deadline_at>UTC_TIMESTAMP(6) AND ts.platform_ai_model_alias_id IS NOT NULL AND a.status='active' AND m.status='active' AND frozen.status='active' AND m.credential_id <=> r.credential_id AND (r.credential_id IS NULL OR c.status='active'))")
            .bind(Uuid::parse_str(&self.binding.resource_version).map_err(ApiError::internal)?).bind(id.tenant_id).bind(id.run_id).bind(id.session_id).bind(id.user_id).bind(self.fence).fetch_one(&self.state.pool).await?;
        if !valid {
            return Err(ApiError::forbidden(
                "Design run, model or credential is no longer active",
            ));
        }
        match &self.target {
            DesignModelTargetV1::WorkflowBuilder(target) => {
                actor.require("workflow:edit")?;
                crate::workflow_api::require_workflow_access(
                    &self.state,
                    &actor,
                    target.workflow_id,
                    true,
                )
                .await?;
                let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM design_agent_sessions s JOIN workflow_drafts d ON d.tenant_id=s.tenant_id AND d.id=s.draft_id AND d.workflow_id=s.workflow_id JOIN workflows w ON w.tenant_id=d.tenant_id AND w.id=d.workflow_id JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.working_draft_id=d.id AND v.workflow_id=d.workflow_id WHERE s.tenant_id=? AND s.id=? AND s.target_kind='workflow_builder' AND s.workflow_id=? AND s.draft_id=? AND s.document_generation=d.generation AND v.archived=FALSE AND w.status='active')")
                    .bind(id.tenant_id).bind(id.session_id).bind(target.workflow_id).bind(target.draft_id).fetch_one(&self.state.pool).await?;
                if !active {
                    return Err(ApiError::forbidden(
                        "Workflow editor binding is no longer active",
                    ));
                }
            }
            DesignModelTargetV1::ApplicationUiBuilder(target) => {
                crate::application_ui_api::authorize_target(&self.state, &actor, target).await?;
            }
        }
        Ok(actor)
    }
}

pub(crate) fn target_from_row(
    row: &sqlx::mysql::MySqlRow,
    tenant: Uuid,
    user: Uuid,
    session: Uuid,
    run: Uuid,
) -> ApiResult<DesignModelTargetV1> {
    match row.try_get::<String, _>("target_kind")?.as_str() {
        "workflow_builder" => Ok(agentx_runtime_contracts::StudioBuilderTargetV1 {
            tenant_id: tenant,
            user_id: user,
            session_id: session,
            run_id: run,
            workflow_id: row.try_get("workflow_id")?,
            draft_id: row.try_get("draft_id")?,
        }
        .into()),
        "application_ui_builder" => Ok(agentx_runtime_contracts::ApplicationUiBuilderTargetV1 {
            tenant_id: tenant,
            user_id: user,
            session_id: session,
            run_id: run,
            application_id: row.try_get("application_id")?,
            ui_draft_id: row.try_get("ui_draft_id")?,
            bundle_id: row.try_get("bundle_id")?,
            contract_hash: serde_json::from_value(json!(
                row.try_get::<String, _>("contract_hash")?
            ))
            .map_err(ApiError::internal)?,
        }
        .into()),
        _ => Err(ApiError::internal("Design profile is invalid")),
    }
}

fn block<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}
struct HostState {
    context: RunContext,
}
impl StatePort for HostState {
    fn load(&mut self, _session: &str) -> Result<Option<AgentSessionStateV1>, StatePortError> {
        block(async {
            let value: Value = sqlx::query_scalar(
                "SELECT state_json FROM design_agent_sessions WHERE tenant_id=? AND id=?",
            )
            .bind(self.context.target.identity().tenant_id)
            .bind(self.context.target.identity().session_id)
            .fetch_one(&self.context.state.pool)
            .await
            .map_err(|_| StatePortError::Persistence("Builder state is unavailable".into()))?;
            let mut state: AgentSessionStateV1 = serde_json::from_value(value)
                .map_err(|_| StatePortError::Persistence("Builder state is invalid".into()))?;
            if state.operation.as_ref().is_some_and(|op| {
                !op.operation_id
                    .starts_with(&self.context.target.identity().run_id.to_string())
            }) {
                let calls = state
                    .messages
                    .iter()
                    .filter(|message| message.role == agentx_agent_core::MessageRole::Assistant)
                    .flat_map(|message| message.tool_calls.clone())
                    .collect::<Vec<_>>();
                for call in calls {
                    if !state
                        .messages
                        .iter()
                        .any(|message| message.tool_call_id.as_deref() == Some(&call.call_id))
                    {
                        state.messages.push(agentx_agent_core::AgentMessageV1 {message_id:format!("tool-result:{}",call.call_id),role:agentx_agent_core::MessageRole::ToolResult,content:"Previous run stopped before this effect settled. No successful edit or execution is asserted.".into(),tool_calls:vec![],tool_call_id:Some(call.call_id),is_error:true});
                    }
                }
                state.operation = None;
                state.terminal = None;
            }
            crate::design_agent_inputs::hydrate(
                &self.context.state,
                &self.context.target,
                &mut state,
            )
            .await
            .map_err(|_| StatePortError::Persistence("Design guidance is unavailable".into()))?;
            Ok(Some(state))
        })
    }
    fn commit(
        &mut self,
        expected: u64,
        state: &AgentSessionStateV1,
        fence: u64,
    ) -> Result<u64, StatePortError> {
        block(async {
            let mut committed = state.clone();
            committed.version = expected + 1;
            let value = serde_json::to_value(committed).map_err(|_| {
                StatePortError::Persistence("Builder state cannot be serialized".into())
            })?;
            let mut tx = self.context.state.pool.begin().await.map_err(|_| {
                StatePortError::Persistence("Builder transaction is unavailable".into())
            })?;
            // ACK and cancellation lock Run before Session. Match that order;
            // a multi-table UPDATE may let MySQL choose the reverse order.
            let leased=sqlx::query("SELECT id FROM design_agent_runs WHERE tenant_id=? AND id=? AND session_id=? AND fencing_token=? AND status IN ('queued','running','waiting') AND lease_until>UTC_TIMESTAMP(6) FOR UPDATE").bind(self.context.target.identity().tenant_id).bind(self.context.target.identity().run_id).bind(self.context.target.identity().session_id).bind(fence).fetch_optional(&mut *tx).await.map_err(|_|StatePortError::Persistence("Builder lease is unavailable".into()))?;
            if leased.is_none() {
                return Err(StatePortError::LeaseLost);
            }
            let changed=sqlx::query("UPDATE design_agent_sessions SET state_json=?,state_version=state_version+1 WHERE tenant_id=? AND id=? AND state_version=?")
                .bind(value).bind(self.context.target.identity().tenant_id).bind(self.context.target.identity().session_id).bind(expected).execute(&mut *tx).await.map_err(|error| {
                    tracing::warn!(run_id=%self.context.target.identity().run_id, database_code=?error.as_database_error().and_then(|error|error.code()), "Builder state commit failed");
                    StatePortError::Persistence("Builder state commit failed".into())
                })?;
            if changed.rows_affected() == 0 {
                return Err(StatePortError::LeaseLost);
            }
            sqlx::query("UPDATE design_agent_runs SET lease_until=UTC_TIMESTAMP(6)+INTERVAL 60 SECOND WHERE tenant_id=? AND id=? AND fencing_token=?").bind(self.context.target.identity().tenant_id).bind(self.context.target.identity().run_id).bind(fence).execute(&mut *tx).await.map_err(|_|StatePortError::Persistence("Builder lease renewal failed".into()))?;
            tx.commit().await.map_err(|_| {
                StatePortError::Persistence("Builder state transaction failed".into())
            })?;
            Ok(expected + 1)
        })
    }
}

struct HostEvents {
    context: RunContext,
}
impl EventPort for HostEvents {
    fn publish(&mut self, event: CoreEventV1) -> Result<(), EventPortError> {
        block(async {
            let mut payload = serde_json::to_value(&event)
                .map_err(|_| EventPortError("Core event cannot be serialized".into()))?;
            let key = match &event {
                CoreEventV1::MessageAdded { message_id } => {
                    let value: Value = sqlx::query_scalar(
                        "SELECT state_json FROM design_agent_sessions WHERE tenant_id=? AND id=?",
                    )
                    .bind(self.context.target.identity().tenant_id)
                    .bind(self.context.target.identity().session_id)
                    .fetch_one(&self.context.state.pool)
                    .await
                    .map_err(|_| EventPortError("Core entry is unavailable".into()))?;
                    let core: AgentSessionStateV1 = serde_json::from_value(value)
                        .map_err(|_| EventPortError("Core entry is invalid".into()))?;
                    if let Some(message) = core
                        .messages
                        .iter()
                        .find(|message| &message.message_id == message_id)
                    {
                        payload["message"] = serde_json::to_value(message).unwrap();
                    }
                    format!("message:{message_id}")
                }
                CoreEventV1::ModelIntent { operation_id } => format!("model-intent:{operation_id}"),
                CoreEventV1::ModelSettled { operation_id, .. } => {
                    format!("model-settled:{operation_id}")
                }
                CoreEventV1::ToolIntent { operation_id, .. } => {
                    format!("tool-intent:{operation_id}")
                }
                CoreEventV1::ToolSettled { operation_id, .. } => {
                    format!("tool-settled:{operation_id}")
                }
                CoreEventV1::CompactionCompleted { .. } => {
                    let snapshot = crate::design_agent_runtime::compaction_snapshot_id(&self.context.state, &self.context.target).await
                        .map_err(|error| {
                            tracing::warn!(run_id=%self.context.target.identity().run_id,error=?error,"Compaction event could not read its snapshot");
                            EventPortError("Compaction snapshot is unavailable".into())
                        })?;
                    payload["snapshotId"] = Value::String(snapshot.clone());
                    format!(
                        "compaction:{}:{snapshot}",
                        self.context.target.identity().run_id
                    )
                }
                _ => format!(
                    "core:{}:{}",
                    self.context.target.identity().run_id,
                    agentx_domain::canonical_content_hash(&payload).unwrap()
                ),
            };
            append_entry(&self.context.state, &self.context.target, &key, payload)
                .await
                .map_err(|error| {
                    tracing::warn!(run_id=%self.context.target.identity().run_id,error=?error,"Core event persistence failed");
                    EventPortError("Core event persistence failed".into())
                })
        })
    }
}
struct HostClock;
impl ClockPort for HostClock {
    fn now_millis(&self) -> u64 {
        (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).max(0) as u64
    }
}
struct HostBudget {
    context: RunContext,
}
impl BudgetPort for HostBudget {
    fn admit_turn(&mut self, _turn: u32, _tokens: u64) -> BudgetDecision {
        block(async {
            if self.context.authorize().await.is_err() {
                return BudgetDecision::Cancel;
            }
            let count:Result<i64,_>=sqlx::query_scalar("SELECT COUNT(*) FROM design_agent_entries WHERE tenant_id=? AND run_id=? AND event_key LIKE 'model-intent:%'").bind(self.context.target.identity().tenant_id).bind(self.context.target.identity().run_id).fetch_one(&self.context.state.pool).await;
            if count.is_err() || count.unwrap_or(33) > 32 {
                BudgetDecision::Exhausted
            } else {
                let recent=sqlx::query("SELECT tool_name,input_json,result_json FROM design_agent_effects FORCE INDEX(idx_builder_effect_order) WHERE tenant_id=? AND run_id=? AND status='settled' ORDER BY created_at DESC,id DESC LIMIT 6").bind(self.context.target.identity().tenant_id).bind(self.context.target.identity().run_id).fetch_all(&self.context.state.pool).await;
                if let Ok(rows) = recent {
                    let stalled=rows.iter().map_while(|row|{
                        let result:Value=row.try_get("result_json").ok()?;
                        let value=result.get("structuredResult")?;
                        let is_edit=row.try_get::<String,_>("tool_name").ok()?=="workflow_edit";
                        (result["isError"]==true || (is_edit && value["status"]=="no_change")).then(||json!({"tool":row.try_get::<String,_>("tool_name").ok(),"input":row.try_get::<Value,_>("input_json").ok(),"error":value}))
                    }).take(3).collect::<Vec<_>>();
                    if stalled.len() == 3 && stalled.windows(2).all(|pair| pair[0] == pair[1]) {
                        let _=append_entry(&self.context.state,&self.context.target,&format!("no-progress:{}",self.context.target.identity().run_id),json!({"type":"progress","text":"Repeated unchanged tool failures. Supply missing design information and start a new run."})).await;
                        return BudgetDecision::Exhausted;
                    }
                }
                BudgetDecision::Continue
            }
        })
    }
}

struct HostModel {
    context: RunContext,
}

fn model_transport_error(error: ApiError, effect: Uuid) -> ModelPortError {
    match error.status() {
        axum::http::StatusCode::SERVICE_UNAVAILABLE => ModelPortError::Pending(effect.to_string()),
        axum::http::StatusCode::UNAUTHORIZED | axum::http::StatusCode::FORBIDDEN => {
            ModelPortError::Cancelled
        }
        status if status.is_server_error() => {
            ModelPortError::OutcomeUnknown("Model receipt could not be verified".into())
        }
        _ => ModelPortError::Effect("Model effect was rejected".into()),
    }
}
impl HostModel {
    async fn receipt(
        &self,
        context: &EffectContextV1,
        request: Option<&ModelRequestV1>,
    ) -> Result<Option<DesignModelReceiptV1>, ModelPortError> {
        self.context
            .authorize()
            .await
            .map_err(|_| ModelPortError::Cancelled)?;
        let effect = agentx_runtime_contracts::deterministic_uuid(
            self.context.target.identity().run_id,
            context.effect_id.as_bytes(),
        );
        let cursors: Value = sqlx::query_scalar(
            "SELECT delta_cursors_json FROM design_agent_runs WHERE tenant_id=? AND id=?",
        )
        .bind(self.context.target.identity().tenant_id)
        .bind(self.context.target.identity().run_id)
        .fetch_one(&self.context.state.pool)
        .await
        .map_err(|_| ModelPortError::Effect("Model cursor is unavailable".into()))?;
        let after = cursors
            .get(effect.to_string())
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let existing = inspect(&self.context.state, &self.context.target, effect, after)
            .await
            .map_err(|error| model_transport_error(error, effect))?;
        let receipt = if let Some(request) = request
            && existing
                .as_ref()
                .is_none_or(|receipt| receipt.status == "reserved")
        {
            Some(
                submit(
                    &self.context.state,
                    &DesignModelEffectRequestV1 {
                        protocol_version: 1,
                        target: self.context.target.clone(),
                        binding: self.context.binding.clone(),
                        parameters: self.context.parameters.clone(),
                        context: context.clone(),
                        request: request.clone(),
                    },
                )
                .await
                .map_err(|error| model_transport_error(error, effect))?,
            )
        } else {
            existing
        };
        if let Some(receipt) = &receipt {
            let deltas = receipt
                .deltas
                .iter()
                .filter(|delta| delta.sequence > after)
                .map(|delta| {
                    (
                        format!("delta:{effect}:{}", delta.sequence),
                        json!({"type":"model_delta","effectId":effect,"text":delta.text}),
                    )
                })
                .collect();
            crate::design_agent_runtime::append_entries(
                &self.context.state,
                &self.context.target,
                deltas,
            )
            .await
            .map_err(|_| ModelPortError::Effect("Model delta persistence failed".into()))?;
            if let Some(last) = receipt.deltas.last() {
                sqlx::query("UPDATE design_agent_runs SET delta_cursors_json=JSON_SET(delta_cursors_json,?,?) WHERE tenant_id=? AND id=? AND fencing_token=?").bind(format!("$.\"{effect}\"")).bind(last.sequence).bind(self.context.target.identity().tenant_id).bind(self.context.target.identity().run_id).bind(self.context.fence).execute(&self.context.state.pool).await.map_err(|_|ModelPortError::Effect("Model cursor persistence failed".into()))?;
            }
            if receipt.usage_known {
                append_entry(&self.context.state,&self.context.target,&format!("usage:{effect}"),json!({"type":"usage","effectId":effect,"inputTokens":receipt.input_tokens,"outputTokens":receipt.output_tokens,"costMicros":receipt.cost_micros,"currency":receipt.currency,"estimated":receipt.usage_estimated})).await.map_err(|_|ModelPortError::Effect("Model usage persistence failed".into()))?;
            }
            if receipt.status == "failed" {
                append_entry(
                    &self.context.state,
                    &self.context.target,
                    &format!("model-failure:{effect}"),
                    json!({"type":"model_failed","code":receipt.error_code}),
                )
                .await
                .map_err(|_| ModelPortError::Effect("Model failure persistence failed".into()))?;
            }
        }
        Ok(receipt)
    }
}
impl ModelPort for HostModel {
    fn classify_failure(&self, reason: String) -> ModelPortError {
        model_failure(reason)
    }
    fn observe(&mut self, context: &EffectContextV1) -> EffectObservationV1 {
        match block(self.receipt(context, None)) {
            Ok(Some(receipt)) => match receipt.status.as_str() {
                "succeeded" => receipt
                    .response
                    .and_then(|value| serde_json::to_value(value).ok())
                    .map(EffectObservationV1::Confirmed)
                    .unwrap_or(EffectObservationV1::Unknown),
                "reserved" | "sent" => EffectObservationV1::Pending(receipt.effect_id.to_string()),
                "failed" => EffectObservationV1::Failed(
                    receipt
                        .error_code
                        .unwrap_or_else(|| "DESIGN_MODEL_EFFECT_FAILED".into()),
                ),
                _ => EffectObservationV1::Unknown,
            },
            Ok(None) => EffectObservationV1::NotStarted,
            // An unavailable receipt query says nothing about dispatch. Keep
            // the original Effect pending; inspect it again before any submit.
            Err(ModelPortError::Pending(token)) => EffectObservationV1::Pending(token),
            Err(ModelPortError::Effect(reason)) => EffectObservationV1::Failed(reason),
            Err(_) => EffectObservationV1::Unknown,
        }
    }
    fn invoke(
        &mut self,
        request: &ModelRequestV1,
        context: &EffectContextV1,
    ) -> Result<ModelResponseV1, ModelPortError> {
        let receipt = block(self.receipt(context, Some(request)))?
            .ok_or_else(|| ModelPortError::OutcomeUnknown("Model receipt is missing".into()))?;
        match receipt.status.as_str() {
            "succeeded" => receipt
                .response
                .ok_or_else(|| ModelPortError::OutcomeUnknown("Model result is missing".into())),
            "reserved" | "sent" => Err(ModelPortError::Pending(receipt.effect_id.to_string())),
            "cancelled" => Err(ModelPortError::Cancelled),
            "outcome_unknown" => Err(ModelPortError::OutcomeUnknown(
                "Provider call has an unknown outcome".into(),
            )),
            _ => Err(model_failure(
                receipt
                    .error_code
                    .unwrap_or_else(|| "DESIGN_MODEL_EFFECT_FAILED".into()),
            )),
        }
    }
}

fn model_failure(code: String) -> ModelPortError {
    match code.as_str() {
        "DESIGN_MODEL_CONTEXT_OVERFLOW" => ModelPortError::ContextOverflow,
        "DESIGN_MODEL_OUTPUT_LIMIT" => ModelPortError::Recoverable("The model reached its output limit. Keep the same limit and split the work into small edits.".into()),
        "DESIGN_MODEL_RESPONSE_INVALID" => ModelPortError::Recoverable("The completed response had empty content or invalid tool arguments. Return complete JSON for each tool call.".into()),
        "DESIGN_MODEL_PROVIDER_UNAVAILABLE" => ModelPortError::Recoverable("The model service temporarily rejected this generation. Retry within the existing task budget.".into()),
        _ => ModelPortError::Effect(code),
    }
}

pub(crate) async fn drive<T: ToolPort>(
    context: RunContext,
    input: AgentRunInputV1,
    mut tools: T,
) -> ApiResult<()> {
    let state = context.state.clone();
    let target = context.target.clone();
    let run = target.identity().run_id;
    let fence = context.fence;
    let result = tokio::task::block_in_place(|| {
        AgentCore::run(
            &input,
            &mut HostModel {
                context: context.clone(),
            },
            &mut tools,
            &mut HostState {
                context: context.clone(),
            },
            &mut HostEvents {
                context: context.clone(),
            },
            &HostClock,
            &mut HostBudget {
                context: context.clone(),
            },
        )
    });
    let status = match &result {
        Ok(result) => match result.terminal_reason {
            TerminalReasonV1::Completed => "completed",
            TerminalReasonV1::Suspended => "waiting",
            TerminalReasonV1::Cancelled => "cancelled",
            TerminalReasonV1::OutcomeUnknown => "outcome_unknown",
            _ => "failed",
        },
        Err(_) => "failed",
    };
    let status = crate::design_agent_inputs::finish_run(&state, &target, fence, status).await?;
    if let Some(status) = status.filter(|status| *status != "waiting") {
        sqlx::query("UPDATE design_agent_effects SET status='cancelled' WHERE tenant_id=? AND run_id=? AND status='pending'").bind(target.identity().tenant_id).bind(run).execute(&state.pool).await?;
        if let Some(operation) = result
            .as_ref()
            .ok()
            .and_then(|result| result.state.operation.as_ref())
            && matches!(
                operation.operation_kind,
                agentx_agent_core::OperationKindV1::Model
                    | agentx_agent_core::OperationKindV1::Compaction
            )
            && status != "completed"
        {
            let effect =
                agentx_runtime_contracts::deterministic_uuid(run, operation.effect_id.as_bytes());
            let _ = crate::design_agent_runtime::cancel(&state, &target, effect).await;
        }
        let reason = match &result {
            Ok(value) => serde_json::to_value(&value.terminal_reason).unwrap(),
            Err(error) => Value::String(error.to_string()),
        };
        append_entry(
            &state,
            &target,
            &format!("terminal:{run}"),
            json!({"type":"terminal","status":status,"reason":reason}),
        )
        .await?;
    }
    if let Err(error) = result {
        tracing::warn!(run_id=%run, error=%error,"Studio Builder Core stopped");
    }
    Ok(())
}

pub(crate) async fn run_loop(
    state: ControlApiState,
    lifecycle: agentx_service_kit::ServiceLifecycle,
    progress: agentx_service_kit::RoleProgressWatchdog,
) -> anyhow::Result<()> {
    let owner = Uuid::now_v7();
    loop {
        if lifecycle.is_draining() {
            break;
        }
        let mut tx = state.pool.begin().await?;
        let row=sqlx::query("SELECT r.id,r.fencing_token,s.target_kind FROM design_agent_runs r JOIN design_agent_sessions s ON s.tenant_id=r.tenant_id AND s.id=r.session_id WHERE r.status IN ('queued','running','waiting') AND r.available_at<=UTC_TIMESTAMP(6) AND (r.lease_until IS NULL OR r.lease_until<=UTC_TIMESTAMP(6)) ORDER BY r.available_at,r.id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
        if let Some(row) = row {
            let run: Uuid = row.try_get("id")?;
            let fence = row.try_get::<u64, _>("fencing_token")? + 1;
            sqlx::query("UPDATE design_agent_runs SET status='running',lease_owner=?,lease_until=UTC_TIMESTAMP(6)+INTERVAL 60 SECOND,fencing_token=? WHERE id=?").bind(owner).bind(fence).bind(run).execute(&mut *tx).await?;
            tx.commit().await?;
            let result = match row.try_get::<String, _>("target_kind")?.as_str() {
                "workflow_builder" => {
                    crate::studio_builder_host::step(state.clone(), run, fence).await
                }
                "application_ui_builder" => {
                    crate::application_ui_host::step(state.clone(), run, fence).await
                }
                _ => Err(ApiError::internal("Design profile is invalid")),
            };
            if let Err(error) = result {
                tracing::warn!(run_id=%run,error=?error,"Builder step failed");
            }
        } else {
            tx.commit().await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        progress.processed_since(std::time::Instant::now()).await;
    }
    Ok(())
}
