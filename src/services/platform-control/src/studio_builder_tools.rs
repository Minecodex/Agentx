use std::collections::BTreeMap;

use agentx_agent_core::{
    EffectContextV1, EffectObservationV1, ReplayPolicyV1, ToolCallV1, ToolDefinitionV1,
    ToolOriginV1, ToolPort, ToolPortError, ToolResultV1,
};
use agentx_runtime_contracts::{StudioEditIntentV1, StudioEditorCommandV1, StudioEditorSnapshotV1};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    studio_builder_host::RunContext,
};

#[derive(Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ReadInput {
    Summary,
    Document {
        cursor: Option<usize>,
        limit: Option<usize>,
    },
    Nodes {
        node_ids: Vec<String>,
    },
    Bindings {
        node_ids: Vec<String>,
    },
    Field {
        node_id: String,
        parameter_path: String,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    Artifact {
        artifact_id: Uuid,
        offset: Option<usize>,
        limit: Option<usize>,
    },
}
#[derive(Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum CatalogInput {
    Search {
        query: String,
        cursor: Option<usize>,
    },
    Describe {
        node_type: String,
        version: u32,
    },
    Resolve {
        node_type: String,
        version: u32,
        configuration: Value,
        upstream_contracts: Value,
    },
}
#[derive(Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ResourceInput {
    Search {
        resource_type: String,
        operation: String,
        search: Option<String>,
        page: Option<u32>,
    },
    Inspect {
        resource_type: String,
        operation: String,
        resource_id: Uuid,
    },
    Options {
        resource_type: String,
        operation: String,
        resource_id: Uuid,
    },
}
#[derive(Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum EditInput {
    Batch {
        expected_local_document_revision: u64,
        expected_document_hash: String,
        intents: Vec<StudioEditIntentV1>,
    },
}
#[derive(Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ValidateInput {
    Draft { expected_document_hash: String },
    Complete { expected_document_hash: String },
}
#[derive(Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ExecutionInput {
    Summary,
    Node { node_execution_id: Uuid },
    Trace,
    Artifact { artifact_id: Uuid },
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AskInput {
    question: String,
    options: Vec<String>,
}

fn tool<T: JsonSchema>(name: &str, description: &str) -> ToolDefinitionV1 {
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    expose_discriminated_properties(&mut schema);
    ToolDefinitionV1 {
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        origin: ToolOriginV1::Platform,
        replay_policy: ReplayPolicyV1::IdempotencyRequired,
    }
}

// Providers inspect top-level properties to guide tool generation. Retain the
// closed union for validation while exposing every discriminator and field.
fn expose_discriminated_properties(schema: &mut Value) {
    match schema {
        Value::Array(items) => items.iter_mut().for_each(expose_discriminated_properties),
        Value::Object(object) => {
            object
                .values_mut()
                .for_each(expose_discriminated_properties);
            let Some(branches) = object.get("oneOf").and_then(Value::as_array) else {
                return;
            };
            let Some(discriminator) = ["action", "kind"].into_iter().find(|key| {
                branches.iter().all(|branch| {
                    branch["type"] == "object" && branch["properties"][key]["const"].is_string()
                })
            }) else {
                return;
            };
            let mut properties = serde_json::Map::new();
            for branch in branches {
                for (key, value) in branch["properties"].as_object().unwrap() {
                    properties
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
            }
            properties.insert(discriminator.into(),json!({"type":"string","enum":branches.iter().map(|branch|branch["properties"][discriminator]["const"].clone()).collect::<Vec<_>>()}));
            object.insert("type".into(), json!("object"));
            object.insert("properties".into(), Value::Object(properties));
            object.insert("required".into(), json!([discriminator]));
            object.insert("additionalProperties".into(), json!(false));
        }
        _ => {}
    }
}
pub(crate) fn registry() -> Vec<ToolDefinitionV1> {
    vec![
        tool::<ReadInput>(
            "workflow_read",
            "Read the CURRENT local canvas. Read summary first. document returns start/end/settings and a page of nodes plus connections; nodes reads exact IDs. Hash and document sequence are authoritative. Never infer current state from old chat.",
        ),
        tool::<CatalogInput>(
            "node_catalog",
            "Search supported node types or describe the EXACT manifest, parameters, Binding/Selector schema, dynamic ports and defaults before editing. resolve is STATIC ONLY and has no resource effects.",
        ),
        tool::<ResourceInput>(
            "resource_query",
            "Find configured resources by name and inspect their real IDs, versions, dependency readiness and Workflow authorization. Search returns nameMatch, bindingAvailable, binding and uniqueExactMatchId only when the complete result is unique. Follow pagination; never invent IDs. It cannot grant resources, test connections or invoke Providers.",
        ),
        tool::<EditInput>(
            "workflow_edit",
            "Edit ONLY this canvas. batch applies ordered intents atomically in ONE Undo transaction after editor ACK. Use latest workflow_read hash and sequence. Keep protected Exit. No save/run/test/version/publish tools exist. Configure resource references using discovered IDs and exact schema.",
        ),
        tool::<ValidateInput>(
            "workflow_validate",
            "STATIC validation of the ACTUAL current canvas, never runs any Workflow or node. The report is valid only for its document hash. Fix reported issues through workflow_edit, then validate again.",
        ),
        tool::<ExecutionInput>(
            "execution_inspect",
            "Read ONLY the execution explicitly selected by the user in the UI. Preserve actual source/Mock/Pin evidence. Cannot run, retry, cancel or publish.",
        ),
        tool::<AskInput>(
            "ask_user",
            "Ask a persistent design clarification. Answers supply design information only and cannot authorize running/tests/publishing.",
        ),
    ]
}

pub(crate) struct BuilderTools {
    pub context: RunContext,
}

fn result(value: Value, error: bool) -> ToolResultV1 {
    let encoded = value.to_string();
    if encoded.len() > 128 * 1024 {
        return ToolResultV1{content:json!({"metadata":value.get("metadata"),"hint":"The full result is stored as a design artifact. Use workflow_read artifact with its ID, offset and limit, or narrow nodes/document/field."}).to_string(),structured_result:Some(value),artifact_refs:vec![],truncated:true,is_error:error,terminate:false};
    }
    ToolResultV1 {
        content: encoded,
        structured_result: Some(value),
        artifact_refs: vec![],
        truncated: false,
        is_error: error,
        terminate: false,
    }
}
fn parse<T: for<'de> Deserialize<'de>>(call: &ToolCallV1) -> ApiResult<T> {
    serde_json::from_value(call.arguments.clone()).map_err(|error| {
        let detail = error.to_string();
        let message = if detail.starts_with("missing field") || detail.starts_with("unknown field")
        {
            format!("Tool arguments do not match the registered schema: {detail}")
        } else {
            "Tool action and field types must match the registered schema".into()
        };
        ApiError::bad_request("BUILDER_TOOL_ARGUMENT_INVALID", message)
    })
}

fn block<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

impl ToolPort for BuilderTools {
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
                    "Builder permission or model authorization was revoked".into(),
                )
            })?;
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
                        "Tool identity was reused".into(),
                    ));
                }
                if let Some(value) = row
                    .try_get::<Option<Value>, _>("result_json")
                    .ok()
                    .flatten()
                {
                    return serde_json::from_value(value).map_err(|_| {
                        ToolPortError::OutcomeUnknown("Stored tool result is invalid".into())
                    });
                }
                let id = row
                    .try_get::<Uuid, _>("id")
                    .map_err(|_| ToolPortError::Effect("Tool identity is invalid".into()))?;
                let event = if call.name == "workflow_edit" {
                    json!({"type":"command","command":row.try_get::<Value,_>("command_json").unwrap()})
                } else {
                    json!({"type":"question","questionId":id,"question":call.arguments["question"],"options":call.arguments["options"]})
                };
                crate::studio_builder_runtime::append_entry(
                    &self.context.state,
                    &self.context.target,
                    &format!("pending:{id}"),
                    event,
                )
                .await
                .map_err(|_| {
                    ToolPortError::Effect("Pending editor event could not be persisted".into())
                })?;
                return Err(ToolPortError::Pending(id.to_string()));
            }
            let mut value = match self.dispatch(call, context).await {
                Ok(value) => value,
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
                                ToolPortError::OutcomeUnknown("Invalid tool receipt".into())
                            });
                        }
                        return Err(ToolPortError::Pending(
                            row.try_get::<Uuid, _>("id").unwrap().to_string(),
                        ));
                    }
                    result(
                        json!({"error":"BUILDER_TOOL_REJECTED","detail":error.design_feedback()}),
                        true,
                    )
                }
            };
            let id = agentx_runtime_contracts::deterministic_uuid(
                self.context.target.run_id,
                context.effect_id.as_bytes(),
            );
            if value.truncated && value.structured_result.is_some() {
                value.artifact_refs = vec![format!("studio-tool:{id}")];
                let mut preview: Value = serde_json::from_str(&value.content).unwrap();
                preview["artifactId"] = json!(id);
                value.content = preview.to_string();
            }
            sqlx::query("INSERT INTO studio_builder_effects(id,tenant_id,session_id,run_id,effect_key,tool_name,request_hash,input_json,result_json,status) VALUES(?,?,?,?,?,?,?,?,?,'settled') ON DUPLICATE KEY UPDATE id=id")
                .bind(id).bind(self.context.target.tenant_id).bind(self.context.target.session_id).bind(self.context.target.run_id).bind(&context.effect_id).bind(&call.name).bind(agentx_domain::canonical_content_hash(&serde_json::to_value(call).unwrap()).unwrap()).bind(&call.arguments).bind(serde_json::to_value(&value).unwrap()).execute(&self.context.state.pool).await.map_err(|_|ToolPortError::Effect("Tool receipt could not be persisted".into()))?;
            Ok(value)
        })
    }
}

impl BuilderTools {
    async fn stored(&self, context: &EffectContextV1) -> ApiResult<Option<sqlx::mysql::MySqlRow>> {
        Ok(sqlx::query(
            "SELECT * FROM studio_builder_effects WHERE tenant_id=? AND run_id=? AND effect_key=?",
        )
        .bind(self.context.target.tenant_id)
        .bind(self.context.target.run_id)
        .bind(&context.effect_id)
        .fetch_optional(&self.context.state.pool)
        .await?)
    }
    async fn snapshot(&self) -> ApiResult<StudioEditorSnapshotV1> {
        let value: Value = sqlx::query_scalar(
            "SELECT snapshot_json FROM studio_builder_sessions WHERE tenant_id=? AND id=?",
        )
        .bind(self.context.target.tenant_id)
        .bind(self.context.target.session_id)
        .fetch_one(&self.context.state.pool)
        .await?;
        serde_json::from_value(value).map_err(ApiError::internal)
    }
    async fn dispatch(
        &self,
        call: &ToolCallV1,
        context: &EffectContextV1,
    ) -> Result<ToolResultV1, ApiError> {
        let snapshot = self.snapshot().await?;
        let actor = self.context.authorize().await?;
        let state = &self.context.state;
        let target = &self.context.target;
        let metadata = json!({"draftId":target.draft_id,"localDocumentRevision":snapshot.local_document_revision,"documentHash":snapshot.document_hash,"serverRevision":snapshot.server_revision,"saveStatus":snapshot.save_status});
        let value = match call.name.as_str() {
            "workflow_read" => match parse::<ReadInput>(call)? {
                ReadInput::Artifact {
                    artifact_id,
                    offset,
                    limit,
                } => {
                    let stored: Value=sqlx::query_scalar("SELECT result_json FROM studio_builder_effects WHERE tenant_id=? AND session_id=? AND id=? AND status='settled'").bind(target.tenant_id).bind(target.session_id).bind(artifact_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Design artifact"))?;
                    if stored["truncated"] != true || stored["structuredResult"].is_null() {
                        return Err(ApiError::not_found("Design artifact"));
                    }
                    let text = stored["structuredResult"].to_string();
                    let offset = offset.unwrap_or(0);
                    let limit = limit.unwrap_or(8000).clamp(1, 16000);
                    let total = text.chars().count();
                    json!({"artifactId":artifact_id,"offset":offset,"totalCharacters":total,"content":text.chars().skip(offset).take(limit).collect::<String>(),"nextOffset":(offset+limit<total).then_some(offset+limit)})
                }
                ReadInput::Summary => {
                    json!({"metadata":metadata,"start":snapshot.document.definition.start,"end":snapshot.document.definition.end,"settings":snapshot.document.definition.settings,"selectedNodeIds":snapshot.selected_node_ids,"nodes":snapshot.document.definition.nodes.iter().map(|node|json!({"id":node.id,"key":node.key,"type":node.node_type,"typeVersion":node.type_version,"name":node.name})).collect::<Vec<_>>(),"connections":snapshot.document.definition.connections,"selectedExecutionId":snapshot.selected_execution_id})
                }
                ReadInput::Document { cursor, limit } => {
                    let offset = cursor.unwrap_or(0);
                    let limit = limit.unwrap_or(30).clamp(1, 100);
                    let nodes = snapshot
                        .document
                        .definition
                        .nodes
                        .iter()
                        .skip(offset)
                        .take(limit)
                        .collect::<Vec<_>>();
                    json!({"metadata":metadata,"start":snapshot.document.definition.start,"end":snapshot.document.definition.end,"settings":snapshot.document.definition.settings,"nodes":nodes,"connections":snapshot.document.definition.connections,"editorDocument":snapshot.document.editor_document,"nextCursor":(offset+limit<snapshot.document.definition.nodes.len()).then_some(offset+limit)})
                }
                ReadInput::Nodes { node_ids } => {
                    json!({"metadata":metadata,"nodes":snapshot.document.definition.nodes.iter().filter(|node|node_ids.contains(&node.id)).collect::<Vec<_>>()})
                }
                ReadInput::Field {
                    node_id,
                    parameter_path,
                    offset,
                    limit,
                } => {
                    let node = snapshot
                        .document
                        .definition
                        .nodes
                        .iter()
                        .find(|node| node.id == node_id)
                        .ok_or_else(|| ApiError::not_found("Node"))?;
                    let value = node
                        .parameters
                        .pointer(&parameter_path)
                        .ok_or_else(|| ApiError::not_found("Node parameter"))?;
                    let text = value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string());
                    let offset = offset.unwrap_or(0);
                    let limit = limit.unwrap_or(4096).clamp(1, 16384);
                    let chunk = text.chars().skip(offset).take(limit).collect::<String>();
                    json!({"metadata":metadata,"nodeId":node_id,"parameterPath":parameter_path,"text":chunk,"offset":offset,"nextOffset":(offset+limit<text.chars().count()).then_some(offset+limit)})
                }
                ReadInput::Bindings { node_ids } => {
                    json!({"metadata":metadata,"bindings":snapshot.document.definition.nodes.iter().filter(|node|node_ids.is_empty()||node_ids.contains(&node.id)).map(|node|json!({"nodeId":node.id,"resourceReferences":node.resource_references})).collect::<Vec<_>>()})
                }
            },
            "node_catalog" => {
                let manifests =
                    crate::catalog_api::manifests_for_tenant(state, target.tenant_id).await?;
                match parse::<CatalogInput>(call)? {
                    CatalogInput::Search { query, cursor } => {
                        crate::studio_builder_catalog::search(
                            &manifests,
                            &query,
                            cursor.unwrap_or(0),
                        )
                    }
                    CatalogInput::Describe { node_type, version }
                        if node_type == "exit" && version == 1 =>
                    {
                        crate::studio_builder_catalog::describe_exit()
                    }
                    CatalogInput::Describe { node_type, version } => {
                        let (manifest, hash) = manifests
                            .into_iter()
                            .find(|(m, _)| m.node_type == node_type && m.version == version)
                            .ok_or_else(|| ApiError::not_found("Node manifest"))?;
                        let mut public =
                            serde_json::to_value(manifest).map_err(ApiError::internal)?;
                        // Runtime/UI bundles are data-plane code, not design instructions.
                        public.as_object_mut().unwrap().remove("plugin");
                        json!({"manifest":public,"manifestHash":hash,"inputBindingSchema":schemars::schema_for!(agentx_domain::InputBinding)})
                    }
                    CatalogInput::Resolve {
                        node_type,
                        version,
                        configuration,
                        upstream_contracts,
                    } => {
                        let (manifest, _) = manifests
                            .into_iter()
                            .find(|(m, _)| m.node_type == node_type && m.version == version)
                            .ok_or_else(|| ApiError::not_found("Node manifest"))?;
                        if let Some(plugin) = manifest.plugin.as_ref().filter(|_| {
                            manifest.capability
                                == agentx_node_protocol::NodeCapability::PluginNodejs
                        }) {
                            crate::canvas_plugin_api::invoke_plugin_method(state,target.tenant_id,plugin,"node.resolveDefinition",json!({"nodeType":node_type,"configuration":configuration,"upstreamContracts":upstream_contracts,"inputPorts":manifest.input_ports,"outputPorts":manifest.output_ports,"outputSchema":manifest.output_schema,"outputPortSchemas":manifest.output_port_schemas})).await?
                        } else {
                            let mut definition = snapshot.document.definition.clone();
                            let mut node = definition.nodes.first().cloned().ok_or_else(|| {
                                ApiError::bad_request(
                                    "BUILDER_DOCUMENT_INVALID",
                                    "The document needs an Exit node",
                                )
                            })?;
                            node.id = "__builder_contract__".into();
                            node.key = "builder_contract".into();
                            node.node_type = node_type;
                            node.type_version = version;
                            node.parameters = configuration;
                            node.protected = false;
                            node.disabled = false;
                            node.resource_references.clear();
                            if let Some(version_id) = node
                                .parameters
                                .get("workflowVersionId")
                                .and_then(Value::as_str)
                            {
                                let version_id = Uuid::parse_str(version_id).map_err(|_| {
                                    ApiError::bad_request(
                                        "WORKFLOW_VERSION_INVALID",
                                        "Select an immutable Workflow version",
                                    )
                                })?;
                                let workflow:Uuid = sqlx::query_scalar("SELECT workflow_id FROM workflow_versions WHERE tenant_id=? AND id=?").bind(target.tenant_id).bind(version_id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Workflow version"))?;
                                crate::workflow_api::require_workflow_access(
                                    state, &actor, workflow, false,
                                )
                                .await?;
                            }
                            definition.nodes.push(node.clone());
                            let dependencies = crate::workflow_api::load_composite_definitions(
                                state,
                                target.tenant_id,
                                target.workflow_id,
                                &definition,
                            )
                            .await?;
                            let registry = crate::canvas_plugin_api::registry_for_tenant(
                                state,
                                target.tenant_id,
                                &dependencies,
                                true,
                            )
                            .await?;
                            let effective = registry
                                .resolve_definition_manifest(
                                    &node.node_type,
                                    node.type_version,
                                    &node.parameters,
                                )
                                .ok_or_else(|| ApiError::not_found("Node manifest"))?;
                            let output = agentx_runtime::WorkflowCompiler::new(&registry)
                                .design_output_contracts(&definition)
                                .remove(&node.id);
                            json!({"status":"complete","inputPorts":effective.input_ports,"outputContract":output,"executionPerformed":false})
                        }
                    }
                }
            }
            "resource_query" => match parse::<ResourceInput>(call)? {
                ResourceInput::Search {
                    resource_type,
                    operation,
                    search,
                    page,
                } => {
                    let mut value = crate::resource_api::builder_resource_options(
                        state,
                        &actor,
                        (target.workflow_id, target.draft_id),
                        resource_type,
                        operation.clone(),
                        search.clone(),
                        page.unwrap_or(1),
                    )
                    .await?;
                    crate::studio_builder_configuration::annotate_search(
                        &mut value,
                        search.as_deref(),
                        &operation,
                    );
                    value
                }
                ResourceInput::Inspect {
                    resource_type,
                    operation,
                    resource_id,
                }
                | ResourceInput::Options {
                    resource_type,
                    operation,
                    resource_id,
                } => {
                    let mut page = 1;
                    loop {
                        let value = crate::resource_api::builder_resource_options(
                            state,
                            &actor,
                            (target.workflow_id, target.draft_id),
                            resource_type.clone(),
                            operation.clone(),
                            None,
                            page,
                        )
                        .await?;
                        let found = value["items"].as_array().and_then(|items| {
                            items.iter().find(|item| {
                                item.get("id").and_then(Value::as_str)
                                    == Some(&resource_id.to_string())
                            })
                        });
                        if let Some(found) = found {
                            break json!({"metadata":found,"optionsSource":"platform_metadata","providerInvoked":false});
                        }
                        if value["items"]
                            .as_array()
                            .is_none_or(|items| items.len() < 50)
                        {
                            return Err(ApiError::not_found("Resource metadata"));
                        }
                        page += 1;
                        if page > 20 {
                            return Err(ApiError::bad_request(
                                "BUILDER_QUERY_BUDGET",
                                "Use resource search to narrow the query",
                            ));
                        }
                    }
                }
            },
            "workflow_edit" => {
                let EditInput::Batch {
                    expected_local_document_revision,
                    expected_document_hash,
                    intents,
                } = parse(call)?;
                if expected_local_document_revision != snapshot.local_document_revision
                    || expected_document_hash != snapshot.document_hash
                {
                    return Err(ApiError::conflict(
                        "EDITOR_DOCUMENT_STALE",
                        "Read the current document before editing",
                    ));
                }
                if intents.is_empty() || intents.len() > 200 {
                    return Err(ApiError::bad_request(
                        "BUILDER_EDIT_BUDGET",
                        "A batch requires 1 to 200 intents",
                    ));
                }
                let mut document = snapshot.document.clone();
                let requested_layout = intents
                    .iter()
                    .any(|intent| matches!(intent, StudioEditIntentV1::AutoLayout));
                for intent in intents {
                    apply_intent(&mut document, intent)?;
                }
                normalize_connection_orders(&mut document.definition.connections);
                let layouts = document.editor_document.node_layouts.clone();
                document.editor_document.node_layouts = document
                    .definition
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(index, node)| {
                        layouts
                            .iter()
                            .find(|layout| layout.node_id == node.id)
                            .cloned()
                            .unwrap_or(agentx_domain::NodeLayout {
                                node_id: node.id.clone(),
                                x: 120.0 + index as f64 * 260.0,
                                y: 180.0,
                                width: None,
                                height: None,
                                collapsed: false,
                            })
                    })
                    .collect();
                let edges = document.editor_document.edges.clone();
                document.editor_document.edges = document
                    .definition
                    .connections
                    .iter()
                    .map(|edge| {
                        edges
                            .iter()
                            .find(|item| item.edge_id == edge.id)
                            .cloned()
                            .unwrap_or(agentx_domain::EditorEdge {
                                edge_id: edge.id.clone(),
                                label_position: None,
                            })
                    })
                    .collect();
                for node in snapshot
                    .document
                    .definition
                    .nodes
                    .iter()
                    .filter(|node| node.protected)
                {
                    if !document.definition.nodes.iter().any(|candidate| {
                        candidate.id == node.id
                            && candidate.protected
                            && candidate.node_type == node.node_type
                    }) {
                        return Err(ApiError::bad_request(
                            "PROTECTED_NODE",
                            "Protected nodes cannot be removed or converted",
                        ));
                    }
                }
                crate::workflow_api::validate_documents(
                    serde_json::to_value(&document.definition).map_err(ApiError::internal)?,
                    serde_json::to_value(&document.editor_document).map_err(ApiError::internal)?,
                )?;
                let report = crate::workflow_api::builder_validate_document(
                    state,
                    &actor,
                    target.workflow_id,
                    target.draft_id,
                    &document,
                    false,
                )
                .await?;
                if report["issues"]
                    .as_array()
                    .is_some_and(|issues| !issues.is_empty())
                {
                    return Ok(result(
                        json!({"status":"rejected","issues":report["issues"]}),
                        true,
                    ));
                }
                let changed_node_ids =
                    changed_nodes(&snapshot.document.definition, &document.definition);
                crate::studio_builder_scope::check(
                    &self.context.edit_scope,
                    &snapshot.document,
                    &document,
                    &changed_node_ids,
                )?;
                let hash = agentx_domain::canonical_content_hash(
                    &serde_json::to_value(&document).map_err(ApiError::internal)?,
                )
                .map_err(ApiError::internal)?;
                let auto_layout = crate::studio_builder_layout::needs_layout(
                    &snapshot.document,
                    &document,
                    &self.context.edit_scope,
                    requested_layout,
                )?;
                if hash == snapshot.document_hash && !auto_layout {
                    return Ok(result(
                        json!({"status":"no_change","documentHash":hash}),
                        false,
                    ));
                }
                let id = agentx_runtime_contracts::deterministic_uuid(
                    target.run_id,
                    context.effect_id.as_bytes(),
                );
                let command = StudioEditorCommandV1 {
                    command_id: id,
                    run_id: target.run_id,
                    draft_id: target.draft_id,
                    editor_instance_id: snapshot.editor_instance_id,
                    expected_local_document_revision,
                    expected_document_hash,
                    result_document_hash: hash,
                    auto_layout,
                    document,
                    changed_node_ids,
                };
                self.prepare_pending(
                    call,
                    context,
                    id,
                    Some(&command),
                    json!({"type":"command","command":command}),
                )
                .await?;
                // Dispatcher observes this durable pending receipt on the next pass.
                return Err(ApiError::accepted(
                    "BUILDER_EDITOR_PENDING",
                    "Waiting for editor acknowledgement",
                    1,
                ));
            }
            "workflow_validate" => {
                let (hash, complete) = match parse::<ValidateInput>(call)? {
                    ValidateInput::Draft {
                        expected_document_hash,
                    } => (expected_document_hash, false),
                    ValidateInput::Complete {
                        expected_document_hash,
                    } => (expected_document_hash, true),
                };
                if hash != snapshot.document_hash {
                    return Err(ApiError::conflict(
                        "EDITOR_DOCUMENT_STALE",
                        "The validation target is stale",
                    ));
                }
                let mut report = crate::workflow_api::builder_validate_document(
                    state,
                    &actor,
                    target.workflow_id,
                    target.draft_id,
                    &snapshot.document,
                    complete,
                )
                .await?;
                let configuration = crate::studio_builder_configuration::review(
                    state,
                    &actor,
                    target.workflow_id,
                    target.draft_id,
                    &snapshot.document.definition,
                )
                .await?;
                report["configurationIssues"] = json!(configuration);
                if let Some(issues) = report["issues"].as_array_mut() {
                    issues.extend(configuration);
                }
                report["documentHash"] = json!(hash);
                report["executionPerformed"] = json!(false);
                crate::studio_builder_runtime::append_entry(
                    state,
                    target,
                    &format!("validation:{}", context.effect_id),
                    json!({"type":"validation","report":report}),
                )
                .await?;
                report
            }
            "execution_inspect" => {
                let execution = snapshot.selected_execution_id.ok_or_else(|| {
                    ApiError::bad_request(
                        "BUILDER_EXECUTION_NOT_SELECTED",
                        "Select an execution in the UI first",
                    )
                })?;
                actor.require("execution:view")?;
                let detail =
                    crate::runtime_bff::runtime_execution_detail(state, &actor, execution).await?;
                if detail.summary.workflow_id != target.workflow_id {
                    return Err(ApiError::not_found("Selected Workflow execution"));
                }
                match parse::<ExecutionInput>(call)? {
                    ExecutionInput::Summary=>serde_json::to_value(detail).map_err(ApiError::internal)?,
                    ExecutionInput::Node {node_execution_id}=>crate::runtime_bff::runtime_execution_get::<Value>(state,&actor,execution,"runtime.query.nodes",&format!("/internal/runtime/v1/query/executions/{execution}/nodes/{node_execution_id}")).await?,
                    ExecutionInput::Trace=>crate::runtime_bff::builder_trace(state,&actor,execution).await?,
                    ExecutionInput::Artifact {artifact_id}=>crate::runtime_bff::builder_artifact(state,&actor,execution,artifact_id).await?,
                }
            }
            "ask_user" => {
                let input: AskInput = parse(call)?;
                if input.question.trim().is_empty()
                    || input.question.len() > 8192
                    || input.options.len() > 8
                {
                    return Err(ApiError::bad_request(
                        "BUILDER_QUESTION_INVALID",
                        "The design question is invalid",
                    ));
                }
                let id = agentx_runtime_contracts::deterministic_uuid(
                    target.run_id,
                    context.effect_id.as_bytes(),
                );
                self.prepare_pending(call,context,id,None,json!({"type":"question","questionId":id,"question":input.question,"options":input.options})).await?;
                return Err(ApiError::accepted(
                    "BUILDER_USER_PENDING",
                    "Waiting for a design answer",
                    1,
                ));
            }
            _ => {
                return Err(ApiError::forbidden(
                    "The requested tool is not in the design registry",
                ));
            }
        };
        Ok(result(value, false))
    }
    async fn prepare_pending(
        &self,
        call: &ToolCallV1,
        context: &EffectContextV1,
        id: Uuid,
        command: Option<&StudioEditorCommandV1>,
        event: Value,
    ) -> ApiResult<()> {
        let mut tx = self.context.state.pool.begin().await?;
        // Serialize command preparation with cancellation and lease takeover.
        // Run precedes Session, matching Core commits and acknowledgements.
        let active:Option<Uuid>=sqlx::query_scalar("SELECT id FROM studio_builder_runs WHERE tenant_id=? AND id=? AND session_id=? AND fencing_token=? AND status IN ('queued','running','waiting') AND lease_until>UTC_TIMESTAMP(6) FOR UPDATE")
            .bind(self.context.target.tenant_id).bind(self.context.target.run_id).bind(self.context.target.session_id).bind(self.context.fence).fetch_optional(&mut *tx).await?;
        if active.is_none() {
            return Err(ApiError::conflict(
                "BUILDER_RUN_CLOSED",
                "The run stopped before command preparation",
            ));
        }
        let current:Value=sqlx::query_scalar("SELECT snapshot_json FROM studio_builder_sessions WHERE tenant_id=? AND id=? FOR UPDATE").bind(self.context.target.tenant_id).bind(self.context.target.session_id).fetch_one(&mut *tx).await?;
        if let Some(command) = command {
            if current["documentHash"].as_str() != Some(&command.expected_document_hash)
                || current["localDocumentRevision"].as_u64()
                    != Some(command.expected_local_document_revision)
            {
                return Err(ApiError::conflict(
                    "EDITOR_DOCUMENT_STALE",
                    "Canvas changed while the command was prepared",
                ));
            }
        }
        sqlx::query("INSERT INTO studio_builder_effects(id,tenant_id,session_id,run_id,effect_key,tool_name,request_hash,input_json,command_json) VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(id).bind(self.context.target.tenant_id).bind(self.context.target.session_id).bind(self.context.target.run_id).bind(&context.effect_id).bind(&call.name).bind(agentx_domain::canonical_content_hash(&serde_json::to_value(call).unwrap()).unwrap()).bind(&call.arguments).bind(command.map(|command|serde_json::to_value(command).unwrap())).execute(&mut *tx).await?;
        tx.commit().await?;
        crate::studio_builder_runtime::append_entry(
            &self.context.state,
            &self.context.target,
            &format!("pending:{id}"),
            event,
        )
        .await?;
        Ok(())
    }
}

fn normalize_connection_orders(connections: &mut [agentx_domain::WorkflowConnection]) {
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (index, connection) in connections.iter().enumerate() {
        groups
            .entry((
                connection.source_node_id.clone(),
                connection.source_handle.clone(),
            ))
            .or_default()
            .push(index);
    }
    for indexes in groups.values_mut() {
        indexes.sort_by(|left, right| {
            connections[*left]
                .order
                .cmp(&connections[*right].order)
                .then_with(|| connections[*left].id.cmp(&connections[*right].id))
        });
        for (order, index) in indexes.iter().enumerate() {
            connections[*index].order = order as u32;
        }
    }
}

fn apply_intent(
    document: &mut agentx_runtime_contracts::StudioDocumentV1,
    intent: StudioEditIntentV1,
) -> ApiResult<()> {
    match intent {
        StudioEditIntentV1::AutoLayout => {}
        StudioEditIntentV1::ReplaceDocument { document: next } => *document = next,
        StudioEditIntentV1::UpsertNode { node, layout } => {
            if let Some(existing) = document
                .definition
                .nodes
                .iter_mut()
                .find(|candidate| candidate.id == node.id)
            {
                *existing = node;
            } else {
                document.definition.nodes.push(node);
            }
            if let Some(layout) = layout {
                document
                    .editor_document
                    .node_layouts
                    .retain(|item| item.node_id != layout.node_id);
                document.editor_document.node_layouts.push(layout);
            }
        }
        StudioEditIntentV1::RemoveNode { node_id } => {
            document.definition.nodes.retain(|node| node.id != node_id);
            document
                .definition
                .connections
                .retain(|edge| edge.source_node_id != node_id && edge.target_node_id != node_id);
            document
                .editor_document
                .node_layouts
                .retain(|layout| layout.node_id != node_id);
            let ids = document
                .definition
                .connections
                .iter()
                .map(|edge| edge.id.as_str())
                .collect::<Vec<_>>();
            document
                .editor_document
                .edges
                .retain(|edge| ids.contains(&edge.edge_id.as_str()));
            for group in &mut document.editor_document.groups {
                group.node_ids.retain(|id| id != &node_id);
            }
        }
        StudioEditIntentV1::ReplaceConnections { connections } => {
            document.definition.connections = connections
        }
        StudioEditIntentV1::ConfigureWorkflow {
            start,
            end,
            settings,
        } => {
            if let Some(start) = start {
                document.definition.start = start;
            }
            if let Some(end) = end {
                document.definition.end = end;
            }
            if let Some(settings) = settings {
                document.definition.settings = settings;
            }
        }
        StudioEditIntentV1::SetEditorDocument { editor_document } => {
            document.editor_document = editor_document
        }
    }
    Ok(())
}
fn changed_nodes(
    before: &agentx_domain::WorkflowDefinition,
    after: &agentx_domain::WorkflowDefinition,
) -> Vec<String> {
    let mut old: BTreeMap<String, Value> = before
        .nodes
        .iter()
        .map(|node| (node.id.clone(), serde_json::to_value(node).unwrap()))
        .collect();
    let mut changed = Vec::new();
    for node in &after.nodes {
        if old.remove(&node.id).as_ref() != Some(&serde_json::to_value(node).unwrap()) {
            changed.push(node.id.clone());
        }
    }
    changed.extend(old.into_keys());
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_orders_are_local_to_each_output_port() {
        let edge = |id: &str, port: &str, order| agentx_domain::WorkflowConnection {
            id: id.into(),
            source_node_id: "if".into(),
            source_handle: port.into(),
            target_node_id: "exit".into(),
            target_handle: "main".into(),
            order,
        };
        let mut edges = vec![
            edge("case", "case:yes", 0),
            edge("else", "else", 1),
            edge("later", "else", 7),
        ];
        normalize_connection_orders(&mut edges);
        assert_eq!(
            edges.iter().map(|edge| edge.order).collect::<Vec<_>>(),
            vec![0, 0, 1]
        );
        assert_eq!(edges[1].source_handle, "else");
    }
    #[test]
    fn model_schemas_expose_actions_and_enforce_closed_branches() {
        let registry = registry();
        let describe = &registry
            .iter()
            .find(|tool| tool.name == "node_catalog")
            .unwrap()
            .input_schema;
        assert_eq!(describe["properties"]["nodeType"]["type"], "string");
        assert!(
            describe["properties"]["action"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!("describe"))
        );
        let validator = jsonschema::validator_for(describe).unwrap();
        let valid = json!({"action":"describe","nodeType":"if","version":1});
        assert!(validator.is_valid(&valid));
        assert!(serde_json::from_value::<CatalogInput>(valid).is_ok());
        assert!(!validator.is_valid(&json!({"action":"describe","node_type":"if","version":1})));
        assert!(!validator.is_valid(&json!({"action":"describe","nodeType":"if"})));
        assert!(!validator.is_valid(&json!({"action":"execute","nodeType":"if","version":1})));
        assert_eq!(registry.len(), 7);
        assert!(
            registry
                .iter()
                .all(|tool| tool.origin == ToolOriginV1::Platform)
        );
    }
}
