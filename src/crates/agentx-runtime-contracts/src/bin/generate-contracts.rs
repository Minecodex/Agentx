#![recursion_limit = "256"]

use std::{fs, path::Path};

use agentx_runtime_contracts::{
    ActivateDeploymentRequestV1, AgentKnowledgeSearchResultV1, AgentSessionClearReceiptV1,
    AgentSessionClearRequestV1, AgentSessionDiagnosticV1, AgentSessionEntryViewV1,
    AgentSessionSearchPageV1, AgentSessionSearchRequestV1, AgentSessionSummaryV1,
    AgentSubjectMemoryAuditViewV1, AgentSubjectMemoryClearReceiptV1,
    AgentSubjectMemoryClearRequestV1, AgentSubjectMemorySearchPageV1,
    AgentSubjectMemorySearchRequestV1, ApplyChatMappingReceiptV1, ApplyChatMappingRequestV1,
    ApplyReceiptV1, CancelWorkPackageRequestV1, ChatMappingV1, CommandEnvelopeV1,
    DelegationScopeV1, DisableDeploymentRequestV1, EgressConnectClaimsV1, EventEnvelopeV1,
    EventExportPageV1, EventExportRequestV1, ExecuteWorkPackageRequestV1, ExecutionArtifactV1,
    ExecutionCheckpointV1, ExecutionCollectionPageV1, ExecutionContextSnapshotV1,
    ExecutionDetailV1, ExecutionEventPageV1, ExecutionNodeV1, ExecutionRuntimeDetailsV1,
    ExecutionSearchPageV1, ExecutionSearchRequestV1, ExecutionSpecBundleV2, ExecutionTraceV1,
    GovernanceSnapshotPageV1, GovernanceSnapshotRequestV1, InvocationDetailV1,
    InvocationSearchPageV1, InvocationSearchRequestV1, ObservabilityAggregatePageV1,
    ObservabilityAggregateRequestV1, PluginDesignOperationRequestV1,
    PluginDesignOperationResponseV1, PrepareBundleRequestV1, PrepareWorkPackageRequestV1,
    ProcessSessionControlRequestV1, ProcessSessionControlResponseV1,
    ProcessSessionFramesResponseV1, ProcessSessionReadRequestV1, ProcessSessionStartRequestV1,
    ProcessSessionStartResponseV1, ProcessSessionWriteRequestV1, ProjectionStatusV1,
    PublishReceiptV1, ReferenceCheckReceiptV1, ReferenceCheckRequestV1, RetentionCommandRequestV1,
    RollbackDeploymentRequestV1, RuntimeAdmissionCommandV1, RuntimeAuthorizationSnapshotV1,
    RuntimeCommandApplyRequestV1, RuntimeDebugPlanV1, RuntimeIntegrationEventEnvelopeV1,
    RuntimeObjectUploadMetadataV1, RuntimeObjectUploadReceiptV1, RuntimePolicyV1,
    RuntimeResourceBindingV1, RuntimeResourceCheckRequestV1, RuntimeResourceCheckResponseV1,
    RuntimeResourceOperationRequestV1, RuntimeResourceOperationResponseV1, RuntimeSkillProgramV2,
    RuntimeTriggerSpecV1, RuntimeUserAdmissionV1, RuntimeUserApplicationGrantV1,
    RuntimeUserWorkflowGrantV1, RuntimeWorkPackageV1, SessionDetailV1, SessionSearchPageV1,
    SessionSearchRequestV1, SessionUpgradeCommandV1, SessionUpgradeReceiptV1, ToolEffectRequestV1,
    ToolEffectResponseV1, TraceEventEnvelopeV1, TraceSearchPageV1, TraceSearchRequestV1,
    TraceSpanDetailV1, WorkerAttemptLeaseV1, WorkerResultV1, WorkerTaskV1,
    WorkspaceAcquireRequestV1, WorkspaceAcquireResponseV1, WorkspaceReleaseRequestV1,
    WorkspaceReleaseResponseV1,
};
use anyhow::{Context, Result};
use schemars::{JsonSchema, schema_for};
use serde_json::{Map, Value, json};

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let schema_directory = arguments
        .next()
        .context("usage: generate-contracts <schema-directory> <openapi-file>")?;
    let openapi_file = arguments
        .next()
        .context("usage: generate-contracts <schema-directory> <runtime-openapi-file> [observability-openapi-file]")?;
    let observability_openapi_file = arguments.next();
    anyhow::ensure!(arguments.next().is_none(), "unexpected extra arguments");

    fs::create_dir_all(&schema_directory)
        .with_context(|| format!("create schema directory {schema_directory}"))?;
    if let Some(parent) = Path::new(&openapi_file).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create OpenAPI directory {}", parent.display()))?;
    }

    let schemas = contract_schemas()?;
    for (name, schema) in &schemas {
        write_json(
            &Path::new(&schema_directory).join(format!("{name}.schema.json")),
            schema,
        )?;
    }
    // Workflow definition / node manifest contracts live beside the runtime
    // schemas; they are shared platform-wide rather than internal-only, so
    // they are emitted as standalone files instead of OpenAPI components.
    write_json(
        &Path::new(&schema_directory).join("WorkflowDefinition.schema.json"),
        &serde_json::to_value(schema_for!(agentx_domain::WorkflowDefinition))?,
    )?;
    write_json(
        &Path::new(&schema_directory).join("NodeManifestVersion.schema.json"),
        &serde_json::to_value(schema_for!(agentx_node_protocol::NodeManifestVersion))?,
    )?;
    write_json(Path::new(&openapi_file), &internal_openapi(schemas.clone()))?;
    if let Some(path) = observability_openapi_file {
        if let Some(parent) = Path::new(&path).parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create OpenAPI directory {}", parent.display()))?;
        }
        write_json(Path::new(&path), &observability_openapi(schemas))?;
    }
    Ok(())
}

fn contract_schemas() -> Result<Map<String, Value>> {
    let mut schemas = Map::new();
    insert::<ExecutionSpecBundleV2>(&mut schemas, "ExecutionSpecBundleV2")?;
    insert::<RuntimeWorkPackageV1>(&mut schemas, "RuntimeWorkPackageV1")?;
    insert::<ExecutionContextSnapshotV1>(&mut schemas, "ExecutionContextSnapshotV1")?;
    insert::<RuntimeDebugPlanV1>(&mut schemas, "RuntimeDebugPlanV1")?;
    insert::<RuntimePolicyV1>(&mut schemas, "RuntimePolicyV1")?;
    insert::<RuntimeAuthorizationSnapshotV1>(&mut schemas, "RuntimeAuthorizationSnapshotV1")?;
    insert::<RuntimeResourceBindingV1>(&mut schemas, "RuntimeResourceBindingV1")?;
    insert::<EgressConnectClaimsV1>(&mut schemas, "EgressConnectClaimsV1")?;
    insert::<RuntimeResourceCheckRequestV1>(&mut schemas, "RuntimeResourceCheckRequestV1")?;
    insert::<RuntimeResourceCheckResponseV1>(&mut schemas, "RuntimeResourceCheckResponseV1")?;
    insert::<RuntimeResourceOperationRequestV1>(&mut schemas, "RuntimeResourceOperationRequestV1")?;
    insert::<RuntimeResourceOperationResponseV1>(
        &mut schemas,
        "RuntimeResourceOperationResponseV1",
    )?;
    insert::<PluginDesignOperationRequestV1>(&mut schemas, "PluginDesignOperationRequestV1")?;
    insert::<PluginDesignOperationResponseV1>(&mut schemas, "PluginDesignOperationResponseV1")?;
    insert::<RuntimeSkillProgramV2>(&mut schemas, "RuntimeSkillProgramV2")?;
    insert::<AgentKnowledgeSearchResultV1>(&mut schemas, "AgentKnowledgeSearchResultV1")?;
    insert::<ProcessSessionStartRequestV1>(&mut schemas, "ProcessSessionStartRequestV1")?;
    insert::<ProcessSessionStartResponseV1>(&mut schemas, "ProcessSessionStartResponseV1")?;
    insert::<ProcessSessionWriteRequestV1>(&mut schemas, "ProcessSessionWriteRequestV1")?;
    insert::<ProcessSessionReadRequestV1>(&mut schemas, "ProcessSessionReadRequestV1")?;
    insert::<ProcessSessionFramesResponseV1>(&mut schemas, "ProcessSessionFramesResponseV1")?;
    insert::<ProcessSessionControlRequestV1>(&mut schemas, "ProcessSessionControlRequestV1")?;
    insert::<ProcessSessionControlResponseV1>(&mut schemas, "ProcessSessionControlResponseV1")?;
    insert::<WorkerTaskV1>(&mut schemas, "WorkerTaskV1")?;
    insert::<WorkerAttemptLeaseV1>(&mut schemas, "WorkerAttemptLeaseV1")?;
    insert::<WorkerResultV1>(&mut schemas, "WorkerResultV1")?;
    insert::<WorkspaceAcquireRequestV1>(&mut schemas, "WorkspaceAcquireRequestV1")?;
    insert::<WorkspaceAcquireResponseV1>(&mut schemas, "WorkspaceAcquireResponseV1")?;
    insert::<ToolEffectRequestV1>(&mut schemas, "ToolEffectRequestV1")?;
    insert::<ToolEffectResponseV1>(&mut schemas, "ToolEffectResponseV1")?;
    insert::<WorkspaceReleaseRequestV1>(&mut schemas, "WorkspaceReleaseRequestV1")?;
    insert::<WorkspaceReleaseResponseV1>(&mut schemas, "WorkspaceReleaseResponseV1")?;
    insert::<CommandEnvelopeV1>(&mut schemas, "CommandEnvelopeV1")?;
    insert::<EventEnvelopeV1>(&mut schemas, "EventEnvelopeV1")?;
    insert::<RuntimeAdmissionCommandV1>(&mut schemas, "RuntimeAdmissionCommandV1")?;
    insert::<ChatMappingV1>(&mut schemas, "ChatMappingV1")?;
    insert::<ApplyChatMappingRequestV1>(&mut schemas, "ApplyChatMappingRequestV1")?;
    insert::<ApplyChatMappingReceiptV1>(&mut schemas, "ApplyChatMappingReceiptV1")?;
    insert::<RuntimeObjectUploadMetadataV1>(&mut schemas, "RuntimeObjectUploadMetadataV1")?;
    insert::<RuntimeObjectUploadReceiptV1>(&mut schemas, "RuntimeObjectUploadReceiptV1")?;
    insert::<PrepareBundleRequestV1>(&mut schemas, "PrepareBundleRequestV1")?;
    insert::<ActivateDeploymentRequestV1>(&mut schemas, "ActivateDeploymentRequestV1")?;
    insert::<RollbackDeploymentRequestV1>(&mut schemas, "RollbackDeploymentRequestV1")?;
    insert::<DisableDeploymentRequestV1>(&mut schemas, "DisableDeploymentRequestV1")?;
    insert::<PrepareWorkPackageRequestV1>(&mut schemas, "PrepareWorkPackageRequestV1")?;
    insert::<ExecuteWorkPackageRequestV1>(&mut schemas, "ExecuteWorkPackageRequestV1")?;
    insert::<CancelWorkPackageRequestV1>(&mut schemas, "CancelWorkPackageRequestV1")?;
    insert::<RuntimeCommandApplyRequestV1>(&mut schemas, "RuntimeCommandApplyRequestV1")?;
    insert::<ReferenceCheckRequestV1>(&mut schemas, "ReferenceCheckRequestV1")?;
    insert::<ReferenceCheckReceiptV1>(&mut schemas, "ReferenceCheckReceiptV1")?;
    insert::<RetentionCommandRequestV1>(&mut schemas, "RetentionCommandRequestV1")?;
    insert::<ApplyReceiptV1>(&mut schemas, "ApplyReceiptV1")?;
    insert::<PublishReceiptV1>(&mut schemas, "PublishReceiptV1")?;
    insert::<EventExportRequestV1>(&mut schemas, "EventExportRequestV1")?;
    insert::<EventExportPageV1>(&mut schemas, "EventExportPageV1")?;
    insert::<RuntimeIntegrationEventEnvelopeV1>(&mut schemas, "RuntimeIntegrationEventEnvelopeV1")?;
    insert::<GovernanceSnapshotRequestV1>(&mut schemas, "GovernanceSnapshotRequestV1")?;
    insert::<GovernanceSnapshotPageV1>(&mut schemas, "GovernanceSnapshotPageV1")?;
    insert::<DelegationScopeV1>(&mut schemas, "DelegationScopeV1")?;
    insert::<ExecutionSearchRequestV1>(&mut schemas, "ExecutionSearchRequestV1")?;
    insert::<ExecutionSearchPageV1>(&mut schemas, "ExecutionSearchPageV1")?;
    insert::<ExecutionDetailV1>(&mut schemas, "ExecutionDetailV1")?;
    insert::<InvocationSearchRequestV1>(&mut schemas, "InvocationSearchRequestV1")?;
    insert::<InvocationSearchPageV1>(&mut schemas, "InvocationSearchPageV1")?;
    insert::<InvocationDetailV1>(&mut schemas, "InvocationDetailV1")?;
    insert::<ExecutionNodeV1>(&mut schemas, "ExecutionNodeV1")?;
    insert::<ExecutionCheckpointV1>(&mut schemas, "ExecutionCheckpointV1")?;
    insert::<ExecutionRuntimeDetailsV1>(&mut schemas, "ExecutionRuntimeDetailsV1")?;
    insert::<ExecutionArtifactV1>(&mut schemas, "ExecutionArtifactV1")?;
    insert::<ExecutionCollectionPageV1<ExecutionNodeV1>>(&mut schemas, "ExecutionNodePageV1")?;
    insert::<ExecutionEventPageV1>(&mut schemas, "ExecutionEventPageV1")?;
    insert::<ExecutionCollectionPageV1<ExecutionCheckpointV1>>(
        &mut schemas,
        "ExecutionCheckpointPageV1",
    )?;
    insert::<ProjectionStatusV1>(&mut schemas, "ProjectionStatusV1")?;
    insert::<TraceEventEnvelopeV1>(&mut schemas, "TraceEventEnvelopeV1")?;
    insert::<TraceSearchRequestV1>(&mut schemas, "TraceSearchRequestV1")?;
    insert::<TraceSearchPageV1>(&mut schemas, "TraceSearchPageV1")?;
    insert::<ExecutionTraceV1>(&mut schemas, "ExecutionTraceV1")?;
    insert::<TraceSpanDetailV1>(&mut schemas, "TraceSpanDetailV1")?;
    insert::<ObservabilityAggregateRequestV1>(&mut schemas, "ObservabilityAggregateRequestV1")?;
    insert::<ObservabilityAggregatePageV1>(&mut schemas, "ObservabilityAggregatePageV1")?;
    insert::<RuntimeTriggerSpecV1>(&mut schemas, "RuntimeTriggerSpecV1")?;
    insert::<RuntimeUserAdmissionV1>(&mut schemas, "RuntimeUserAdmissionV1")?;
    insert::<RuntimeUserApplicationGrantV1>(&mut schemas, "RuntimeUserApplicationGrantV1")?;
    insert::<RuntimeUserWorkflowGrantV1>(&mut schemas, "RuntimeUserWorkflowGrantV1")?;
    insert::<SessionSearchRequestV1>(&mut schemas, "SessionSearchRequestV1")?;
    insert::<SessionSearchPageV1>(&mut schemas, "SessionSearchPageV1")?;
    insert::<SessionDetailV1>(&mut schemas, "SessionDetailV1")?;
    insert::<SessionUpgradeCommandV1>(&mut schemas, "SessionUpgradeCommandV1")?;
    insert::<SessionUpgradeReceiptV1>(&mut schemas, "SessionUpgradeReceiptV1")?;
    insert::<AgentSessionSearchRequestV1>(&mut schemas, "AgentSessionSearchRequestV1")?;
    insert::<AgentSessionSearchPageV1>(&mut schemas, "AgentSessionSearchPageV1")?;
    insert::<AgentSessionSummaryV1>(&mut schemas, "AgentSessionSummaryV1")?;
    insert::<AgentSessionEntryViewV1>(&mut schemas, "AgentSessionEntryViewV1")?;
    insert::<AgentSessionDiagnosticV1>(&mut schemas, "AgentSessionDiagnosticV1")?;
    insert::<AgentSessionClearRequestV1>(&mut schemas, "AgentSessionClearRequestV1")?;
    insert::<AgentSessionClearReceiptV1>(&mut schemas, "AgentSessionClearReceiptV1")?;
    insert::<AgentSubjectMemoryClearRequestV1>(&mut schemas, "AgentSubjectMemoryClearRequestV1")?;
    insert::<AgentSubjectMemoryClearReceiptV1>(&mut schemas, "AgentSubjectMemoryClearReceiptV1")?;
    insert::<AgentSubjectMemorySearchRequestV1>(&mut schemas, "AgentSubjectMemorySearchRequestV1")?;
    insert::<AgentSubjectMemorySearchPageV1>(&mut schemas, "AgentSubjectMemorySearchPageV1")?;
    insert::<AgentSubjectMemoryAuditViewV1>(&mut schemas, "AgentSubjectMemoryAuditViewV1")?;
    Ok(schemas)
}

fn insert<T: JsonSchema>(schemas: &mut Map<String, Value>, name: &str) -> Result<()> {
    schemas.insert(name.to_owned(), serde_json::to_value(schema_for!(T))?);
    Ok(())
}

fn internal_openapi(schemas: Map<String, Value>) -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Agentx Runtime Internal API",
            "version": "1",
            "description": "Agentx V2 control-to-runtime contract. JSON endpoints use application/json; object upload uses multipart metadata plus streaming content."
        },
        "paths": {
            "/internal/runtime/v1/objects:upload": multipart_post("Upload Runtime Object", "RuntimeObjectUploadMetadataV1", "RuntimeObjectUploadReceiptV1"),
            "/internal/runtime/v1/bundles:prepare": post("Prepare Bundle", "PrepareBundleRequestV1", "PublishReceiptV1"),
            "/internal/runtime/v1/deployments:activate": post("Activate Deployment", "ActivateDeploymentRequestV1", "PublishReceiptV1"),
            "/internal/runtime/v1/deployments:rollback": post("Rollback Deployment", "RollbackDeploymentRequestV1", "PublishReceiptV1"),
            "/internal/runtime/v1/deployments:disable": post("Disable Deployment", "DisableDeploymentRequestV1", "PublishReceiptV1"),
            "/internal/runtime/v1/admission-commands:apply": post("Apply Admission Command", "RuntimeAdmissionCommandV1", "ApplyReceiptV1"),
            "/internal/runtime/v1/work-packages:prepare": post("Prepare Work Package", "PrepareWorkPackageRequestV1", "PublishReceiptV1"),
            "/internal/runtime/v1/work-packages/{id}:execute": post_with_id("Execute Work Package", "ExecuteWorkPackageRequestV1", "ApplyReceiptV1"),
            "/internal/runtime/v1/work-packages/{id}:cancel": post_with_id("Cancel Work Package", "CancelWorkPackageRequestV1", "ApplyReceiptV1"),
            "/internal/runtime/v1/runtime-commands:apply": post("Apply Runtime Command", "RuntimeCommandApplyRequestV1", "ApplyReceiptV1"),
            "/internal/runtime/v1/references:check": post("Check Runtime References", "ReferenceCheckRequestV1", "ReferenceCheckReceiptV1"),
            "/internal/runtime/v1/retention-commands:apply": post("Apply Retention Command", "RetentionCommandRequestV1", "ApplyReceiptV1"),
            "/internal/runtime/v1/resource-checks:execute": post("Execute Resource Check", "RuntimeResourceCheckRequestV1", "RuntimeResourceCheckResponseV1"),
            "/internal/runtime/v1/resource-operations:execute": post("Execute Resource Operation", "RuntimeResourceOperationRequestV1", "RuntimeResourceOperationResponseV1"),
            "/internal/runtime/v1/plugin-design-operations:execute": post("Execute Plugin Design Operation", "PluginDesignOperationRequestV1", "PluginDesignOperationResponseV1"),
            "/internal/runtime/v1/sandboxes:acquire": post("Acquire Agent Workspace", "WorkspaceAcquireRequestV1", "WorkspaceAcquireResponseV1"),
            "/internal/runtime/v1/sandboxes:tool": post("Execute Agent Workspace Tool", "ToolEffectRequestV1", "ToolEffectResponseV1"),
            "/internal/runtime/v1/sandboxes:release": post("Release Agent Workspace", "WorkspaceReleaseRequestV1", "WorkspaceReleaseResponseV1"),
            "/internal/runtime/v1/events:export": {
                "get": {
                    "operationId": "exportRuntimeEventsV1",
                    "parameters": [
                        query_parameter("afterCursor", "integer", true),
                        query_parameter("limit", "integer", true),
                        query_parameter("waitSeconds", "integer", true)
                    ],
                    "responses": response("EventExportPageV1"),
                    "security": [{"serviceJwt": ["events:export"]}]
                }
            },
            "/internal/runtime/v1/governance-snapshots:export": post("Export Governance Snapshot", "GovernanceSnapshotRequestV1", "GovernanceSnapshotPageV1"),
            "/internal/runtime/v1/query/executions:search": post("Search Executions", "ExecutionSearchRequestV1", "ExecutionSearchPageV1"),
            "/internal/runtime/v1/chat-mappings:apply": post("Apply Chat Mapping", "ApplyChatMappingRequestV1", "ApplyChatMappingReceiptV1"),
            "/internal/runtime/v1/query/invocations:search": post("Search Invocations", "InvocationSearchRequestV1", "InvocationSearchPageV1"),
            "/internal/runtime/v1/query/invocations/{id}": get_with_id("Get Invocation", "InvocationDetailV1", "runtime.query.invocation"),
            "/internal/runtime/v1/query/executions/{id}": {
                "get": {
                    "operationId": "getExecutionV1",
                    "parameters": [{
                        "name": "id", "in": "path", "required": true,
                        "schema": {"type": "string", "format": "uuid"}
                    }],
                    "responses": response("ExecutionDetailV1"),
                    "security": [{"serviceJwt": ["query:execution"]}]
                }
            },
            "/internal/runtime/v1/query/sessions:search": post("Search Sessions", "SessionSearchRequestV1", "SessionSearchPageV1"),
            "/internal/runtime/v1/query/executions/{id}/nodes": get_with_id("List Execution Nodes", "ExecutionNodePageV1", "runtime.query.execution"),
            "/internal/runtime/v1/query/executions/{id}/nodes/{node_execution_id}": get_execution_node(),
            "/internal/runtime/v1/query/executions/{id}/events": get_execution_events(),
            "/internal/runtime/v1/query/executions/{id}/checkpoints": get_with_id("List Execution Checkpoints", "ExecutionCheckpointPageV1", "runtime.query.execution"),
            "/internal/runtime/v1/query/executions/{id}/runtime-details": get_with_id("Get Execution Runtime Details", "ExecutionRuntimeDetailsV1", "runtime.query.execution"),
            "/internal/runtime/v1/query/executions/{id}/artifacts/{artifact_id}": get_execution_artifact(),
            "/internal/runtime/v1/query/sessions/{id}": {
                "get": {
                    "operationId": "getSessionV1",
                    "parameters": [{
                        "name": "id", "in": "path", "required": true,
                        "schema": {"type": "string", "format": "uuid"}
                    }],
                    "responses": response("SessionDetailV1"),
                    "security": [{"serviceJwt": ["runtime.query.sessions"]}]
                }
            },
            "/internal/runtime/v1/session-commands:apply": post("Apply Session Command", "SessionUpgradeCommandV1", "SessionUpgradeReceiptV1")
            ,"/internal/runtime/v1/query/agent-sessions:search": post("Search Agent Sessions", "AgentSessionSearchRequestV1", "AgentSessionSearchPageV1")
            ,"/internal/runtime/v1/query/agent-sessions/{session_key}/{node_key}": {
                "get": {
                    "summary": "Get Agent Session Diagnostics",
                    "parameters": [
                        {"name":"session_key","in":"path","required":true,"schema":{"type":"string"}},
                        {"name":"node_key","in":"path","required":true,"schema":{"type":"string"}}
                    ],
                    "responses": response("AgentSessionDiagnosticV1"),
                    "security": [{"serviceJwt":["runtime.query.agent_sessions"]}]
                }
            }
            ,"/internal/runtime/v1/agent-sessions:clear": post("Clear Agent Session", "AgentSessionClearRequestV1", "AgentSessionClearReceiptV1")
            ,"/internal/runtime/v1/agent-subject-memory:clear": post("Clear Agent Subject Memory", "AgentSubjectMemoryClearRequestV1", "AgentSubjectMemoryClearReceiptV1")
            ,"/internal/runtime/v1/query/agent-subject-memory:search": post("Search Agent Subject Memory Audit", "AgentSubjectMemorySearchRequestV1", "AgentSubjectMemorySearchPageV1")
        },
        "components": {
            "securitySchemes": {
                "serviceJwt": {"type": "http", "scheme": "bearer", "bearerFormat": "RS256 JWT"}
            },
            "schemas": schemas
        }
    })
}

fn observability_openapi(schemas: Map<String, Value>) -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Agentx Observability Internal API",
            "version": "1",
            "description": "Cluster-internal, tenant-scoped trace and aggregate queries."
        },
        "paths": {
            "/internal/observability/v1/executions/{id}/trace": observability_trace_get(),
            "/internal/observability/v1/executions/{id}/trace/spans/{span_id}": observability_span_get(),
            "/internal/observability/v1/traces:search": observability_post("Search Traces", "TraceSearchRequestV1", "TraceSearchPageV1", "observability.trace.read"),
            "/internal/observability/v1/aggregates:query": observability_post("Query Aggregates", "ObservabilityAggregateRequestV1", "ObservabilityAggregatePageV1", "observability.aggregate.read")
        },
        "components": {
            "securitySchemes": {
                "delegationJwt": {"type":"http","scheme":"bearer","bearerFormat":"RS256 JWT"}
            },
            "schemas": schemas
        }
    })
}

fn observability_span_get() -> Value {
    json!({"get": {
        "summary": "Get Trace Span",
        "parameters": [
            {"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},
            {"name":"span_id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}
        ],
        "responses": response("TraceSpanDetailV1"),
        "security": [{"delegationJwt": ["observability.trace.read"]}]
    }})
}

fn observability_trace_get() -> Value {
    json!({"get": {
        "summary": "Get Execution Trace",
        "parameters": [
            {"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},
            {"name":"expectedWatermark","in":"query","required":true,"schema":{"type":"integer","format":"uint64"}},
            {"name":"expectedEventCount","in":"query","required":true,"schema":{"type":"integer","format":"uint64"}},
            {"name":"limit","in":"query","required":false,"schema":{"type":"integer","minimum":1,"maximum":1000}},
            {"name":"cursor","in":"query","required":false,"schema":{"type":"string"}},
            {"name":"nodeExecutionId","in":"query","required":false,"schema":{"type":"string","format":"uuid"}}
        ],
        "responses": response("ExecutionTraceV1"),
        "security": [{"delegationJwt": ["observability.trace.read"]}]
    }})
}

fn observability_post(summary: &str, request: &str, response_schema: &str, scope: &str) -> Value {
    json!({"post": {
        "summary": summary,
        "requestBody": body(request),
        "responses": response(response_schema),
        "security": [{"delegationJwt": [scope]}]
    }})
}

fn multipart_post(summary: &str, metadata: &str, response_schema: &str) -> Value {
    json!({
        "post": {
            "summary": summary,
            "requestBody": {
                "required": true,
                "content": {
                    "multipart/form-data": {
                        "schema": {
                            "type": "object",
                            "required": ["metadata", "content"],
                            "properties": {
                                "metadata": schema_reference(metadata),
                                "content": {"type": "string", "format": "binary"}
                            }
                        }
                    }
                }
            },
            "responses": response(response_schema),
            "security": [{"serviceJwt": ["runtime.objects.write"]}]
        }
    })
}

fn post(summary: &str, request: &str, response_schema: &str) -> Value {
    json!({
        "post": {
            "summary": summary,
            "requestBody": body(request),
            "responses": response(response_schema),
            "security": [{"serviceJwt": []}]
        }
    })
}

fn post_with_id(summary: &str, request: &str, response_schema: &str) -> Value {
    let mut value = post(summary, request, response_schema);
    value["post"]["parameters"] = json!([{
        "name": "id", "in": "path", "required": true,
        "schema": {"type": "string", "format": "uuid"}
    }]);
    value
}

fn get_with_id(summary: &str, response_schema: &str, scope: &str) -> Value {
    json!({"get": {
        "summary": summary,
        "parameters": [{"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}],
        "responses": response(response_schema),
        "security": [{"serviceJwt": [scope]}]
    }})
}

fn get_execution_artifact() -> Value {
    json!({"get": {
        "summary": "Get Execution Artifact",
        "parameters": [
            {"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},
            {"name":"artifact_id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}
        ],
        "responses": {
            "200": {
                "description": "Authorized artifact content",
                "headers": {
                    "Content-Disposition": {"schema":{"type":"string"}},
                    "ETag": {"schema":{"type":"string"}}
                },
                "content": {"application/octet-stream":{"schema":{"type":"string","format":"binary"}}}
            }
        },
        "security": [{"serviceJwt": ["runtime.query.execution"]}]
    }})
}

fn get_execution_node() -> Value {
    json!({"get": {
        "summary": "Get Execution Node",
        "parameters": [
            {"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},
            {"name":"node_execution_id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}
        ],
        "responses": response("ExecutionNodeV1"),
        "security": [{"serviceJwt": ["runtime.query.execution"]}]
    }})
}

fn get_execution_events() -> Value {
    json!({"get": {
        "summary": "List Execution Events",
        "parameters": [
            {"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},
            query_parameter("after", "integer", false),
            query_parameter("limit", "integer", false)
        ],
        "responses": response("ExecutionEventPageV1"),
        "security": [{"serviceJwt": ["runtime.query.execution"]}]
    }})
}

fn body(schema: &str) -> Value {
    json!({
        "required": true,
        "content": {"application/json": {"schema": schema_reference(schema)}}
    })
}

fn response(schema: &str) -> Value {
    json!({
        "200": {
            "description": "Applied",
            "content": {"application/json": {"schema": schema_reference(schema)}}
        }
    })
}

fn schema_reference(schema: &str) -> Value {
    json!({"$ref": format!("#/components/schemas/{schema}")})
}

fn query_parameter(name: &str, value_type: &str, required: bool) -> Value {
    json!({"name": name, "in": "query", "required": required, "schema": {"type": value_type}})
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::internal_openapi;

    #[test]
    fn v2_04_internal_routes_are_frozen() {
        let api = internal_openapi(Default::default());
        let paths = api["paths"].as_object().unwrap();
        assert!(paths.contains_key("/internal/runtime/v1/objects:upload"));
        assert!(paths.contains_key("/internal/runtime/v1/deployments:disable"));
        assert_eq!(
            paths["/internal/runtime/v1/objects:upload"]["post"]["requestBody"]["content"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert!(paths.contains_key("/internal/runtime/v1/query/sessions:search"));
        assert!(paths.contains_key("/internal/runtime/v1/query/sessions/{id}"));
        assert!(paths.contains_key("/internal/runtime/v1/session-commands:apply"));
        assert!(paths.contains_key("/internal/runtime/v1/work-packages/{id}:cancel"));
        assert!(paths.contains_key("/internal/runtime/v1/runtime-commands:apply"));
        assert!(paths.contains_key("/internal/runtime/v1/references:check"));
        assert!(paths.contains_key("/internal/runtime/v1/retention-commands:apply"));
        let event_parameters =
            paths["/internal/runtime/v1/query/executions/{id}/events"]["get"]["parameters"]
                .as_array()
                .unwrap();
        assert!(
            event_parameters
                .iter()
                .any(|parameter| { parameter["name"] == "after" && parameter["in"] == "query" })
        );
        assert!(
            event_parameters
                .iter()
                .any(|parameter| { parameter["name"] == "limit" && parameter["in"] == "query" })
        );
    }
}
