use crate::{
    api_error::{ApiError, ApiResult},
    design_agent_host::RunContext,
    design_agent_tools::tool,
};
use agentx_agent_core::{
    EffectContextV1, EffectObservationV1, ToolCallV1, ToolDefinitionV1, ToolPort, ToolPortError,
    ToolResultV1,
};
use agentx_runtime_contracts::{
    ContentHash, DesignModelTargetV1, PageEditCommandV1, PageEditorSnapshotV1, TaskViewV1,
    content_hash,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContractInput {
    action: ContractAction,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ContractAction {
    Read,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadInput {
    offset: Option<usize>,
    limit: Option<usize>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExactEdit {
    old_text: String,
    new_text: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum EditInput {
    Replace {
        base_revision: u64,
        html_hash: ContentHash,
        html: String,
        summary: String,
    },
    Edits {
        base_revision: u64,
        html_hash: ContentHash,
        edits: Vec<ExactEdit>,
        summary: String,
    },
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum CheckMode {
    Static,
    Preview,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckInput {
    mode: CheckMode,
    html_hash: ContentHash,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AskInput {
    question: String,
    options: Vec<String>,
}

pub(crate) fn registry() -> Vec<ToolDefinitionV1> {
    vec![
        tool::<ContractInput>(
            "application_contract",
            "Read the frozen application input/output/wait/conversation contract, supported SDK and shared styles. This is design information only.",
        ),
        tool::<ReadInput>(
            "page_read",
            "Read the current acknowledged HTML and exact local/server revisions and hash; use character pagination for large sources.",
        ),
        tool::<EditInput>(
            "page_edit",
            "Replace the page or apply exact unique text edits atomically against the current local revision/hash. In action=edits, each item contains only oldText and newText; htmlHash belongs to the command, never an edit. The browser must apply and save the command before it settles. No business execution or publication.",
        ),
        tool::<CheckInput>(
            "page_check",
            "Ask the isolated browser to check the exact current HTML hash: static DOM/JavaScript or synthetic SDK preview. No real API, file upload, workflow execution or approval is permitted.",
        ),
        tool::<AskInput>(
            "ask_user",
            "Ask a design-only question when essential requirements are missing. The response cannot expand this agent's capabilities.",
        ),
    ]
}

pub(crate) struct PageTools {
    pub context: RunContext,
}
fn block<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}
fn result(value: Value, error: bool) -> ToolResultV1 {
    ToolResultV1 {
        content: value.to_string(),
        structured_result: Some(value),
        artifact_refs: vec![],
        truncated: false,
        is_error: error,
        terminate: false,
    }
}
fn parse<T: for<'de> Deserialize<'de>>(call: &ToolCallV1) -> ApiResult<T> {
    serde_json::from_value(call.arguments.clone()).map_err(|error| {
        ApiError::bad_request(
            "BUILDER_TOOL_ARGUMENT_INVALID",
            format!("Use the registered tool schema: {error}"),
        )
    })
}

impl ToolPort for PageTools {
    fn observe(&mut self, context: &EffectContextV1) -> EffectObservationV1 {
        match block(self.stored(context)) {
            Ok(Some(row)) => match row.try_get::<Option<Value>, _>("result_json") {
                Ok(Some(value)) => EffectObservationV1::Confirmed(value),
                _ => EffectObservationV1::Pending(
                    row.try_get::<Uuid, _>("id")
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                ),
            },
            Ok(None) => EffectObservationV1::NotStarted,
            Err(_) => EffectObservationV1::Unknown,
        }
    }
    fn execute(
        &mut self,
        call: &ToolCallV1,
        context: &EffectContextV1,
    ) -> Result<ToolResultV1, ToolPortError> {
        block(async {
            self.context.authorize().await.map_err(|_| {
                ToolPortError::Unauthorized(
                    "Page authoring permission or model authorization was revoked".into(),
                )
            })?;
            if !self
                .context
                .target
                .model_tools()
                .contains(&call.name.as_str())
            {
                return Err(ToolPortError::Unauthorized(
                    "Tool is outside this design profile".into(),
                ));
            }
            if let Some(row) = self
                .stored(context)
                .await
                .map_err(|_| ToolPortError::Effect("Tool receipt is unavailable".into()))?
            {
                if row.try_get::<String, _>("request_hash").ok().as_deref()
                    != agentx_domain::canonical_content_hash(&serde_json::to_value(call).unwrap())
                        .ok()
                        .as_deref()
                {
                    return Err(ToolPortError::OutcomeUnknown(
                        "Tool identity was reused with different arguments".into(),
                    ));
                }
                if let Some(value) = row
                    .try_get::<Option<Value>, _>("result_json")
                    .ok()
                    .flatten()
                {
                    return serde_json::from_value(value).map_err(|_| {
                        ToolPortError::OutcomeUnknown("Stored tool receipt is invalid".into())
                    });
                }
                let id: Uuid = row
                    .try_get("id")
                    .map_err(|_| ToolPortError::Effect("Tool identity is invalid".into()))?;
                self.pending_event(call, id, row.try_get("command_json").ok())
                    .await
                    .map_err(|_| {
                        ToolPortError::Effect("Pending editor event is unavailable".into())
                    })?;
                return Err(ToolPortError::Pending(id.to_string()));
            }
            let dispatched = self.dispatch(call, context).await;
            match dispatched {
                Ok(Some(value)) => {
                    self.persist(call, context, None, Some(&value))
                        .await
                        .map_err(|_| {
                            ToolPortError::Effect("Tool receipt could not be persisted".into())
                        })?;
                    Ok(value)
                }
                Ok(None) => Err(ToolPortError::Pending(self.effect_id(context).to_string())),
                Err(error) => {
                    if let Some(row) = self
                        .stored(context)
                        .await
                        .map_err(|_| ToolPortError::Effect("Tool receipt is unavailable".into()))?
                    {
                        if let Some(value) = row
                            .try_get::<Option<Value>, _>("result_json")
                            .ok()
                            .flatten()
                        {
                            return serde_json::from_value(value).map_err(|_| {
                                ToolPortError::OutcomeUnknown("Stored receipt is invalid".into())
                            });
                        }
                        return Err(ToolPortError::Pending(self.effect_id(context).to_string()));
                    }
                    let value = result(
                        json!({"error":"PAGE_TOOL_REJECTED","detail":error.design_feedback()}),
                        true,
                    );
                    self.persist(call, context, None, Some(&value))
                        .await
                        .map_err(|_| {
                            ToolPortError::Effect(
                                "Tool failure receipt could not be persisted".into(),
                            )
                        })?;
                    Ok(value)
                }
            }
        })
    }
}

impl PageTools {
    fn effect_id(&self, context: &EffectContextV1) -> Uuid {
        agentx_runtime_contracts::deterministic_uuid(
            self.context.target.identity().run_id,
            context.effect_id.as_bytes(),
        )
    }
    async fn stored(&self, context: &EffectContextV1) -> ApiResult<Option<sqlx::mysql::MySqlRow>> {
        let id = self.context.target.identity();
        Ok(sqlx::query(
            "SELECT * FROM design_agent_effects WHERE tenant_id=? AND run_id=? AND effect_key=?",
        )
        .bind(id.tenant_id)
        .bind(id.run_id)
        .bind(&context.effect_id)
        .fetch_optional(&self.context.state.pool)
        .await?)
    }
    async fn snapshot(&self) -> ApiResult<PageEditorSnapshotV1> {
        let id = self.context.target.identity();
        let value:Value=sqlx::query_scalar("SELECT snapshot_json FROM design_agent_sessions WHERE tenant_id=? AND id=? AND target_kind='application_ui_builder'").bind(id.tenant_id).bind(id.session_id).fetch_one(&self.context.state.pool).await?;
        serde_json::from_value(value).map_err(ApiError::internal)
    }
    async fn pending_event(
        &self,
        call: &ToolCallV1,
        id: Uuid,
        command: Option<Value>,
    ) -> ApiResult<()> {
        let payload = if call.name == "ask_user" {
            json!({"type":"question","questionId":id,"question":call.arguments["question"],"options":call.arguments["options"]})
        } else {
            json!({"type":"page_command","commandId":id,"tool":call.name,"command":command})
        };
        crate::design_agent_runtime::append_entry(
            &self.context.state,
            &self.context.target,
            &format!("pending:{id}"),
            payload,
        )
        .await
    }
    async fn persist(
        &self,
        call: &ToolCallV1,
        context: &EffectContextV1,
        command: Option<&Value>,
        result: Option<&ToolResultV1>,
    ) -> ApiResult<()> {
        let id = self.context.target.identity();
        let mut tx = self.context.state.pool.begin().await?;
        sqlx::query("SELECT id FROM design_agent_runs WHERE tenant_id=? AND session_id=? AND id=? AND fencing_token=? AND status='running' AND lease_until>UTC_TIMESTAMP(6) FOR UPDATE").bind(id.tenant_id).bind(id.session_id).bind(id.run_id).bind(self.context.fence).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::conflict("BUILDER_LEASE_LOST","Run lease was lost"))?;
        if let Some(command) = command {
            let snapshot:Value=sqlx::query_scalar("SELECT snapshot_json FROM design_agent_sessions WHERE tenant_id=? AND id=? FOR UPDATE").bind(id.tenant_id).bind(id.session_id).fetch_one(&mut *tx).await?;
            let expected = if call.name == "page_edit" {
                &command["baseHtmlHash"]
            } else {
                &command["htmlHash"]
            };
            if call.name != "ask_user" && snapshot["htmlHash"] != *expected {
                return Err(ApiError::conflict(
                    "PAGE_REVISION_CONFLICT",
                    "The source changed while this command was being prepared",
                ));
            }
        }
        sqlx::query("INSERT INTO design_agent_effects(id,tenant_id,session_id,run_id,effect_key,tool_name,request_hash,input_json,command_json,result_json,status) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id")
            .bind(self.effect_id(context)).bind(id.tenant_id).bind(id.session_id).bind(id.run_id).bind(&context.effect_id).bind(&call.name).bind(agentx_domain::canonical_content_hash(&serde_json::to_value(call).unwrap()).map_err(ApiError::internal)?).bind(&call.arguments).bind(command).bind(result.map(serde_json::to_value).transpose().map_err(ApiError::internal)?).bind(if result.is_some(){"settled"}else{"pending"}).execute(&mut *tx).await?;
        tx.commit().await?;
        if result.is_none() {
            self.pending_event(call, self.effect_id(context), command.cloned())
                .await?;
        }
        Ok(())
    }
    async fn dispatch(
        &self,
        call: &ToolCallV1,
        context: &EffectContextV1,
    ) -> ApiResult<Option<ToolResultV1>> {
        let DesignModelTargetV1::ApplicationUiBuilder(target) = &self.context.target else {
            return Err(ApiError::forbidden("Page tools require a page target"));
        };
        let snapshot = self.snapshot().await?;
        let metadata = json!({"uiDraftId":target.ui_draft_id,"htmlHash":snapshot.html_hash,"localRevision":snapshot.local_revision,"serverRevision":snapshot.server_revision,"saveStatus":snapshot.save_status});
        match call.name.as_str() {
            "application_contract" => {
                let ContractInput {
                    action: ContractAction::Read,
                } = parse(call)?;
                let page = crate::application_ui_api::load(
                    &self.context.state,
                    target.tenant_id,
                    target.application_id,
                    target.ui_draft_id,
                )
                .await?;
                Ok(Some(result(
                    json!({"contract":page.contract,"contractHash":page.contract_hash,"taskViewSchema":schemars::schema_for!(TaskViewV1),"sdk":include_str!("../../../../contracts/sdk/application-page-v1.md"),"metadata":metadata}),
                    false,
                )))
            }
            "page_read" => {
                let input: ReadInput = parse(call)?;
                let offset = input.offset.unwrap_or(0);
                let limit = input.limit.unwrap_or(32000).clamp(1, 32000);
                let length = snapshot.html.chars().count();
                let html: String = snapshot.html.chars().skip(offset).take(limit).collect();
                Ok(Some(result(
                    json!({"metadata":metadata,"html":html,"offset":offset,"nextOffset":(offset+limit<length).then_some(offset+limit),"totalCharacters":length}),
                    false,
                )))
            }
            "page_edit" => {
                let input: EditInput = parse(call)?;
                let (base_revision, hash, html, summary) = match input {
                    EditInput::Replace {
                        base_revision,
                        html_hash,
                        html,
                        summary,
                    } => (base_revision, html_hash, html, summary),
                    EditInput::Edits {
                        base_revision,
                        html_hash,
                        edits,
                        summary,
                    } => {
                        if edits.is_empty() || edits.len() > 100 {
                            return Err(ApiError::bad_request(
                                "PAGE_EDIT_INVALID",
                                "Supply 1–100 exact text edits",
                            ));
                        }
                        let mut html = snapshot.html.clone();
                        for edit in edits {
                            if edit.old_text.is_empty() || html.matches(&edit.old_text).count() != 1
                            {
                                return Err(ApiError::conflict(
                                    "PAGE_EDIT_AMBIGUOUS",
                                    "Each oldText must occur exactly once in the current source",
                                ));
                            }
                            html = html.replacen(&edit.old_text, &edit.new_text, 1);
                        }
                        (base_revision, html_hash, html, summary)
                    }
                };
                if base_revision != snapshot.local_revision || hash != snapshot.html_hash {
                    return Err(ApiError::conflict(
                        "PAGE_REVISION_CONFLICT",
                        "Read the current page before editing",
                    ));
                }
                crate::application_ui_api::validate_html_size(&html)?;
                let command = PageEditCommandV1 {
                    command_id: self.effect_id(context),
                    run_id: target.run_id,
                    ui_draft_id: target.ui_draft_id,
                    editor_instance_id: snapshot.editor_instance_id,
                    base_local_revision: base_revision,
                    base_server_revision: snapshot.server_revision,
                    base_html_hash: hash,
                    html_hash: content_hash(&html).map_err(ApiError::internal)?,
                    html,
                    summary,
                };
                self.persist(
                    call,
                    context,
                    Some(&serde_json::to_value(command).map_err(ApiError::internal)?),
                    None,
                )
                .await?;
                Ok(None)
            }
            "page_check" => {
                let input: CheckInput = parse(call)?;
                if input.html_hash != snapshot.html_hash {
                    return Err(ApiError::conflict(
                        "PAGE_CHECK_STALE",
                        "Read the current page before checking",
                    ));
                }
                let command = json!({"commandId":self.effect_id(context),"htmlHash":input.html_hash,"localRevision":snapshot.local_revision,"mode":match input.mode {CheckMode::Static=>"static",CheckMode::Preview=>"preview"}});
                self.persist(call, context, Some(&command), None).await?;
                Ok(None)
            }
            "ask_user" => {
                let input: AskInput = parse(call)?;
                if input.question.trim().is_empty()
                    || input.question.len() > 4096
                    || input.options.len() > 6
                    || input.options.iter().any(|option| option.len() > 512)
                {
                    return Err(ApiError::bad_request(
                        "BUILDER_QUESTION_INVALID",
                        "Provide a short design question with up to six options",
                    ));
                }
                self.persist(call, context, None, None).await?;
                Ok(None)
            }
            _ => Err(ApiError::forbidden("Tool is outside this design profile")),
        }
    }
}
