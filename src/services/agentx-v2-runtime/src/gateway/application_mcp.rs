//! Standard MCP transport over the exact same formal Invocation services as Web/API.
use super::*;
use agentx_runtime_contracts::{APPLICATION_MCP_PROTOCOL, ApplicationMcpPublicationV1};
use rmcp::transport::{
    StreamableHttpServerConfig,
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde::Deserialize;
use std::sync::Arc;
use tower::ServiceExt;

pub(super) async fn metadata(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
) -> RuntimeResult<Json<Value>> {
    crate::application_pages::validate_slug(&slug)?;
    let app: Uuid = sqlx::query_scalar(
        "SELECT r.application_id FROM application_mcp_routes r WHERE r.slug=? AND EXISTS(SELECT 1 FROM application_mcp_publications p WHERE p.tenant_id=r.tenant_id AND p.application_id=r.application_id AND p.slug=r.slug AND p.status='active' AND JSON_EXTRACT(p.manifest_json,'$.enabled')=TRUE)",
    )
    .bind(&slug)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(RuntimeError::NotFound)?;
    Ok(Json(
        json!({"resource":oauth::resource(&slug)?,"authorization_servers":[oauth::issuer()?],"scopes_supported":[format!("agentx:application:{app}"),"offline_access"],"bearer_methods_supported":["header"]}),
    ))
}
pub(super) async fn serve(
    State(state): State<RuntimeState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Response {
    match Box::pin(prepare(state, slug.clone(), headers, request)).await {
        Ok(response) => response,
        Err(error) => {
            let mut response = error.into_response();
            if response.status() == StatusCode::UNAUTHORIZED
                && let Ok(origin) = oauth::public_origin()
                && let Ok(uri) = origin.join(&format!(
                    "gateway/v1/.well-known/oauth-protected-resource/mcp/applications/{slug}"
                ))
                && let Ok(value) = format!("Bearer resource_metadata=\"{uri}\"").parse()
            {
                response
                    .headers_mut()
                    .insert(axum::http::header::WWW_AUTHENTICATE, value);
            }
            response
        }
    }
}
async fn prepare(
    state: RuntimeState,
    slug: String,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> RuntimeResult<Response> {
    crate::application_pages::validate_slug(&slug)?;
    // An ordinary browser bearer is not an OAuth grant to an external AI client.
    let token = bearer_token(&headers)?;
    if !token.starts_with("axk_") && !token.starts_with("ory_at_") {
        return Err(RuntimeError::Unauthorized);
    }
    let caller = if token.starts_with("ory_at_") {
        oauth::authenticate(&state, token, None, Some(&slug)).await?
    } else {
        authenticate(&state, &headers, None).await?
    };
    let rows=sqlx::query("SELECT p.manifest_json,r.route_key FROM application_mcp_publications p JOIN application_routes r ON r.tenant_id=p.tenant_id AND r.application_id=p.application_id WHERE p.tenant_id=? AND p.application_id=? AND p.slug=? AND p.status='active' AND r.status='active' ORDER BY p.revision DESC").bind(caller.tenant_id).bind(caller.application_id).bind(&slug).fetch_all(&state.pool).await?;
    let route_key = rows
        .first()
        .ok_or(RuntimeError::NotFound)?
        .try_get("route_key")?;
    let publications = rows
        .into_iter()
        .map(|row| {
            serde_json::from_value::<ApplicationMcpPublicationV1>(row.try_get("manifest_json")?)
                .map_err(|e| RuntimeError::Internal(e.into()))
        })
        .collect::<RuntimeResult<Vec<_>>>()?;
    let mut waiting_schemas = sqlx::query_scalar::<_, Value>(
        "SELECT JSON_EXTRACT(manifest_json,'$.waitInputSchema') FROM application_mcp_publications WHERE tenant_id=? AND application_id=? ORDER BY revision DESC",
    )
    .bind(caller.tenant_id)
    .bind(caller.application_id)
    .fetch_all(&state.pool)
    .await?;
    let page_waits = sqlx::query_scalar::<_, Value>(
        "SELECT JSON_EXTRACT(manifest_json,'$.contract.waitInputNodes') FROM application_page_publications WHERE tenant_id=? AND application_id=? AND status IN ('active','superseded') ORDER BY revision DESC",
    )
    .bind(caller.tenant_id)
    .bind(caller.application_id)
    .fetch_all(&state.pool)
    .await?;
    for waits in page_waits {
        let waits: Vec<agentx_runtime_contracts::WaitInputNodeV1> =
            serde_json::from_value(waits).map_err(|e| RuntimeError::Internal(e.into()))?;
        waiting_schemas.extend(
            waits
                .into_iter()
                .map(|wait| wait.response_schema.unwrap_or_else(|| json!({}))),
        );
    }
    if headers
        .get("mcp-protocol-version")
        .is_some_and(|value| value != APPLICATION_MCP_PROTOCOL)
    {
        return Err(bad_request("MCP_PROTOCOL_VERSION_UNSUPPORTED"));
    }
    let origin = oauth::public_origin()?;
    let host = origin[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_allowed_hosts([host])
        .enforce_origin_validation();
    let handler = ApplicationService {
        state,
        caller,
        publications,
        slug,
        headers,
        route_key,
        wait_content_schema: wait_content_schema(waiting_schemas),
    };
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let response = match Box::pin(service.oneshot(request)).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    Ok(response.into_response())
}
#[derive(Clone)]
struct ApplicationService {
    state: RuntimeState,
    caller: Caller,
    publications: Vec<ApplicationMcpPublicationV1>,
    slug: String,
    headers: HeaderMap,
    route_key: String,
    wait_content_schema: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Business {
    request_id: Uuid,
    publication_revision: u64,
    input: Value,
    session_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Task {
    task_id: Uuid,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    task_id: Uuid,
    wait_id: Uuid,
    request_id: Uuid,
    content: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cancel {
    task_id: Uuid,
    request_id: Uuid,
}

fn wait_content_schema(history: Vec<Value>) -> Value {
    // Helpers also serve old tasks. Keep the actual published schemas rather
    // than guessing every possible JSON type or narrowing to the current one.
    let mut schemas = Vec::new();
    for schema in history {
        if schema != json!({"not":{}}) && !schemas.contains(&schema) {
            schemas.push(schema);
        }
    }
    let mut schema = match schemas.len() {
        0 => json!({"not":{}}),
        1 => schemas.remove(0),
        _ => json!({"anyOf":schemas}),
    };
    let description = "Use the native JSON type required by this task's pendingInputs[].responseSchema. Objects and arrays must not be JSON-encoded strings. These are the application's published waiting schemas; each original task still validates its own exact waitId and responseSchema.";
    // Explicit, proven types help model providers interpret anyOf correctly.
    // An untyped branch must remain open rather than narrowing old tasks.
    let types = schema_types(&schema);
    if let Some(object) = schema.as_object_mut() {
        if let Some(types) = types
            && !types.is_empty()
        {
            object.insert(
                "type".into(),
                if types.len() == 1 {
                    json!(types.first())
                } else {
                    json!(types)
                },
            );
        }
        object.insert("description".into(), json!(description));
    } else {
        schema = json!({"allOf":[schema],"description":description});
    }
    schema
}

fn schema_types(schema: &Value) -> Option<std::collections::BTreeSet<String>> {
    if let Some(value) = schema.get("type") {
        return if let Some(value) = value.as_str() {
            Some(std::iter::once(value.to_owned()).collect())
        } else {
            value
                .as_array()?
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect()
        };
    }
    let branches = schema.get("anyOf").or_else(|| schema.get("oneOf"))?;
    let mut types = std::collections::BTreeSet::new();
    for branch in branches.as_array()? {
        types.extend(schema_types(branch)?);
    }
    Some(types)
}

impl ApplicationService {
    fn tools(&self) -> Vec<Tool> {
        let uuid = json!({"type":"string","format":"uuid"});
        let task_schema = serde_json::to_value(agentx_runtime_contracts::application_mcp_schema(
            schemars::schema_for!(agentx_runtime_contracts::TaskViewV1),
        ))
        .expect("Task schema");
        let mut names = std::collections::BTreeSet::new();
        let mut definitions = Vec::new();
        for publication in &self.publications {
            if !names.insert(&publication.tool_name) {
                continue;
            }
            definitions.push(
(publication.tool_name.clone(),format!("{} Submit this application's business task. A waiting task is not completed. {}",publication.title,publication.description),json!({"type":"object","additionalProperties":false,"required":["requestId","publicationRevision","input"],"properties":{"requestId":uuid,"publicationRevision":{"type":"integer","minimum":1,"default":publication.revision,"description":"Use the revision returned by tools/list. Retained revisions remain callable until an administrator deletes that publication. Use the revision for this tool and address."},"input":publication.input_schema,"sessionId":{"type":["string","null"],"format":"uuid","description":"For conversational applications, reuse the sessionId returned by the previous task; omit to start a new conversation."}}}))
            );
        }
        definitions.extend([
            ("get_task".into(),"Query an owned task, its actual waits, approvals, output and authorized file references. Never approve business operations through MCP.".into(),json!({"type":"object","additionalProperties":false,"required":["taskId"],"properties":{"taskId":uuid}})),
            ("submit_task_input".into(),"Provide native JSON content matching the requested task's current wait responseSchema, then query the original task. Objects and arrays must remain JSON values, never JSON-encoded strings. Use the current waitId.".into(),json!({"type":"object","additionalProperties":false,"required":["taskId","waitId","requestId","content"],"properties":{"taskId":uuid,"waitId":uuid,"requestId":uuid,"content":self.wait_content_schema}})),
            ("cancel_task".into(),"Request cancellation of an owned task. Query it again for the final authoritative state.".into(),json!({"type":"object","additionalProperties":false,"required":["taskId","requestId"],"properties":{"taskId":uuid,"requestId":uuid}})),
        ]);
        definitions
            .into_iter()
            .map(|(name, description, schema)| {
                let schema = agentx_runtime_contracts::application_mcp_schema(
                    schemars::Schema::try_from(schema).expect("input object schema"),
                );
                let mut tool = Tool::new(
                    name,
                    description,
                    schema.as_object().expect("object schema").clone(),
                );
                tool.output_schema = Some(Arc::new(
                    task_schema.as_object().expect("Task object schema").clone(),
                ));
                tool
            })
            .collect()
    }
    async fn execute(&self, name: &str, args: Value) -> RuntimeResult<Value> {
        let mut headers = self.headers.clone();
        let id = |headers: &mut HeaderMap, key: Uuid| -> RuntimeResult<()> {
            headers.insert(
                "idempotency-key",
                key.to_string()
                    .parse()
                    .map_err(|_| bad_request("MCP_REQUEST_ID_INVALID"))?,
            );
            Ok(())
        };
        let task = match name {
            name if !agentx_runtime_contracts::APPLICATION_MCP_HELPERS.contains(&name) => {
                let request: Business = serde_json::from_value(args)
                    .map_err(|_| bad_request("MCP_ARGUMENTS_INVALID"))?;
                let manifest:Value=sqlx::query_scalar("SELECT manifest_json FROM application_mcp_publications WHERE tenant_id=? AND application_id=? AND revision=? AND slug=? AND status='active'").bind(self.caller.tenant_id).bind(self.caller.application_id).bind(request.publication_revision).bind(&self.slug).fetch_optional(&self.state.pool).await?.ok_or_else(||bad_request("MCP_SCHEMA_REVISION_CHANGED"))?;
                let original: ApplicationMcpPublicationV1 = serde_json::from_value(manifest)
                    .map_err(|e| RuntimeError::Internal(e.into()))?;
                if name != original.tool_name {
                    return Err(bad_request("MCP_TOOL_NOT_PUBLISHED"));
                }
                id(&mut headers, request.request_id)?;
                headers.insert(
                    "x-agentx-mcp-publication",
                    original.publication_id.to_string().parse().expect("UUID"),
                );
                headers.insert(
                    "x-agentx-mcp-revision",
                    original.revision.to_string().parse().expect("revision"),
                );
                let invocation = if let Some(mapping) = original.chat_mapping {
                    let question = request
                        .input
                        .get(&mapping.question_input)
                        .and_then(Value::as_str)
                        .ok_or_else(|| bad_request("MCP_CONVERSATION_QUESTION_REQUIRED"))?;
                    let session = if let Some(session) = request.session_id {
                        let _ = load_session(&self.state, &self.caller, session).await?;
                        let initial:Value=sqlx::query_scalar("SELECT initial_input_json FROM application_sessions WHERE tenant_id=? AND id=?").bind(self.caller.tenant_id).bind(session).fetch_one(&self.state.pool).await?;
                        let mut initial = initial;
                        let mut supplied = request.input.clone();
                        for field in std::iter::once(&mapping.question_input)
                            .chain(mapping.file_input.iter())
                        {
                            if let Some(fields) = initial.as_object_mut() {
                                fields.remove(field);
                            }
                            if let Some(fields) = supplied.as_object_mut() {
                                fields.remove(field);
                            }
                        }
                        if initial != supplied {
                            return Err(bad_request("SESSION_INITIAL_INPUT_CONFLICT"));
                        }
                        session
                    } else {
                        let (_, Json(session)) = create_session(
                            State(self.state.clone()),
                            Path(self.route_key.clone()),
                            headers.clone(),
                            Json(CreateSessionRequestV1 {
                                input: request.input.clone(),
                                title: None,
                            }),
                        )
                        .await?;
                        session.id
                    };
                    let mut parts = vec![MessagePartInputV1 {
                        part_type: "text".into(),
                        content: Some(json!(question)),
                        artifact_id: None,
                    }];
                    if let Some(field) = mapping.file_input
                        && let Some(files) = request.input.get(&field).and_then(Value::as_array)
                    {
                        for file in files {
                            let id = file
                                .get("artifactId")
                                .and_then(Value::as_str)
                                .and_then(|id| Uuid::parse_str(id).ok())
                                .ok_or_else(|| bad_request("ARTIFACT_REFERENCE_INVALID"))?;
                            parts.push(MessagePartInputV1 {
                                part_type: "file".into(),
                                artifact_id: Some(id),
                                content: None,
                            });
                        }
                    }
                    let (_, Json(invocation)) = send_message(
                        State(self.state.clone()),
                        Path(session),
                        headers,
                        Json(MessageRequestV1 {
                            parts,
                            wait_inputs: None,
                        }),
                    )
                    .await?;
                    invocation
                } else {
                    if request.session_id.is_some() {
                        return Err(bad_request("MCP_CONVERSATION_UNAVAILABLE"));
                    }
                    let request: InvocationRequestV1 = serde_json::from_value(
                        json!({"input":request.input,"responseMode":"async"}),
                    )
                    .map_err(|e| RuntimeError::Internal(e.into()))?;
                    let (_, Json(invocation)) = create_invocation(
                        State(self.state.clone()),
                        Path(self.route_key.clone()),
                        headers,
                        Json(request),
                    )
                    .await?;
                    invocation
                };
                invocation.id
            }
            "get_task" => {
                serde_json::from_value::<Task>(args)
                    .map_err(|_| bad_request("MCP_ARGUMENTS_INVALID"))?
                    .task_id
            }
            "submit_task_input" => {
                let request: Input = serde_json::from_value(args)
                    .map_err(|_| bad_request("MCP_ARGUMENTS_INVALID"))?;
                id(&mut headers, request.request_id)?;
                let _receipt = submit_wait_input(
                    State(self.state.clone()),
                    Path((request.task_id, request.wait_id)),
                    headers,
                    Json(agentx_runtime_contracts::SubmitWaitInputV1 {
                        content: request.content,
                    }),
                )
                .await?;
                request.task_id
            }
            "cancel_task" => {
                let request: Cancel = serde_json::from_value(args)
                    .map_err(|_| bad_request("MCP_ARGUMENTS_INVALID"))?;
                id(&mut headers, request.request_id)?;
                let _receipt =
                    cancel_invocation(State(self.state.clone()), Path(request.task_id), headers)
                        .await?;
                request.task_id
            }
            _ => return Err(bad_request("MCP_TOOL_NOT_PUBLISHED")),
        };
        serde_json::to_value(tasks::load(&self.state, &self.caller, task).await?)
            .map_err(|e| RuntimeError::Internal(e.into()))
    }
}
impl ServerHandler for ApplicationService {
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Owned(vec![ProtocolVersion::V_2025_11_25])
    }
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_protocol_version(ProtocolVersion::V_2025_11_25).with_instructions("Use only this published application. Poll tasks until terminal. Business approvals must be performed by authorized people in Agentx. Files are authorized Artifact references, never arbitrary URLs or local paths.")
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools().into_iter().find(|tool| tool.name == name)
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.tools(),
            ..Default::default()
        })
    }
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResponse, ErrorData>> + Send + '_ {
        Box::pin(async move {
            let call_id = Uuid::now_v7();
            let arguments = Value::Object(request.arguments.unwrap_or_default());
            let arguments_hash = agentx_runtime_contracts::content_hash(&arguments)
                .map(|hash| hash.to_string())
                .unwrap_or_default();
            let publication_id = self
                .publications
                .iter()
                .find(|publication| {
                    arguments["publicationRevision"].as_u64() == Some(publication.revision)
                        && request.name == publication.tool_name
                })
                .map(|publication| publication.publication_id);
            let result = match Box::pin(self.execute(&request.name, arguments)).await {
                Ok(task) => {
                    tracing::info!(event="application_mcp_call",%call_id,tenant_id=%self.caller.tenant_id,application_id=%self.caller.application_id,caller_type=self.caller.caller_type,caller_id=%self.caller.subject_id,publication_id=?publication_id,tool=%request.name,%arguments_hash,task_id=task["taskId"].as_str().unwrap_or_default(),outcome="succeeded","Application MCP operation settled");
                    CallToolResult::structured(task)
                }
                Err(error) => {
                    let response = error.into_response();
                    let status = response.status().as_u16();
                    let body = axum::body::to_bytes(response.into_body(), 65536)
                        .await
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                        .unwrap_or_else(|| json!({"code":"APPLICATION_OPERATION_FAILED"}));
                    tracing::info!(event="application_mcp_call",%call_id,tenant_id=%self.caller.tenant_id,application_id=%self.caller.application_id,caller_type=self.caller.caller_type,caller_id=%self.caller.subject_id,publication_id=?publication_id,tool=%request.name,%arguments_hash,http_status=status,error_code=body["code"].as_str().unwrap_or("APPLICATION_OPERATION_FAILED"),outcome="failed","Application MCP operation settled");
                    CallToolResult::error(vec![ContentBlock::text(
                        json!({"httpStatus":status,"error":body}).to_string(),
                    )])
                }
            };
            Ok(result.into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_helper_accepts_original_task_types_after_publication_changes() {
        let current = json!({"type":"object","additionalProperties":false,"required":["confirmed"],"properties":{"confirmed":{"type":"boolean"}}});
        let originals = [
            json!({"type":"string"}),
            json!({"type":"array","items":{"type":"integer"}}),
            json!({"type":"object","additionalProperties":false,"required":["contact"],"properties":{"contact":{"type":"string"}}}),
        ];
        let schema = wait_content_schema(
            std::iter::once(current.clone())
                .chain(originals.iter().cloned())
                .collect(),
        );
        assert_eq!(schema["type"], json!(["array", "object", "string"]));
        let advertised = jsonschema::validator_for(&schema).unwrap();
        for (original, response) in [
            (json!({"type":"string"}), json!("original explanation")),
            (
                json!({"type":"array","items":{"type":"integer"}}),
                json!([1, 2]),
            ),
            (
                json!({"type":"object","additionalProperties":false,"required":["contact"],"properties":{"contact":{"type":"string"}}}),
                json!({"contact":"Original contact"}),
            ),
        ] {
            assert!(
                agentx_runtime::validate_input_wait_content(&original, &response)
                    .unwrap()
                    .is_empty()
            );
            assert!(!jsonschema::is_valid(&current, &response));
            assert!(
                advertised.is_valid(&response),
                "Client discovery must not reject valid input for an earlier task"
            );
            assert!(
                !agentx_runtime::validate_input_wait_content(&original, &json!({"confirmed":true}))
                    .unwrap()
                    .is_empty(),
                "The original wait still performs strict validation"
            );
        }
    }

    #[test]
    fn input_helper_keeps_concrete_types_and_deduplicates_history() {
        let original = json!({"type":"object","required":["contact"],"properties":{"contact":{"type":"string"}},"additionalProperties":false});
        let schema = wait_content_schema(vec![original.clone(), original, json!({"not":{}})]);
        assert_eq!(schema["type"], "object");
        assert!(schema.get("anyOf").is_none());
        assert!(!jsonschema::is_valid(
            &schema,
            &json!("{\"contact\":\"Name\"}")
        ));
        assert!(jsonschema::is_valid(&schema, &json!({"contact":"Name"})));
        assert!(!jsonschema::is_valid(
            &wait_content_schema(vec![]),
            &json!({})
        ));
        let open = wait_content_schema(vec![json!({}), json!({"type":"string"})]);
        assert!(open.get("type").is_none());
        assert!(jsonschema::is_valid(&open, &json!({"original":true})));
    }
}
