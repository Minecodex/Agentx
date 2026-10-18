use std::{collections::BTreeMap, env};

use agentx_bundle_builder::{compile_workflow_version, compile_workflow_version_with_dependencies};
use agentx_control_infrastructure::{
    ControlInfrastructureSettings, connect_control_mysql, control_object_store,
};
use agentx_domain::WorkflowDefinition;
use agentx_runtime_contracts::RuntimeSkillProgramV2;
use anyhow::{Context, Result};
use bytes::Bytes;
use object_store::{ObjectStore, path::Path as ObjectPath};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySql, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

const TENANT: &str = "018f0000-0000-7000-8000-000000000001";
const USER: &str = "018f0000-0000-7000-8000-000000000002";
const DEPARTMENT: &str = "018f0000-0000-7000-8000-000000000005";
const ENVIRONMENT: &str = "018f0000-0000-7000-8000-000000000007";
const WORKFLOW: &str = "018f0000-0000-7000-8000-000000000401";
const IDENTITY: &str = "018f0000-0000-7000-8000-000000000402";
const VERSION: &str = "018f0000-0000-7000-8000-000000000403";
const WORKFLOW_DEPLOYMENT: &str = "018f0000-0000-7000-8000-000000000404";
const APPLICATION: &str = "018f0000-0000-7000-8000-000000000405";
const CHILD_WORKFLOW: &str = "018f0000-0000-7000-8000-000000000411";
const CHILD_VERSION: &str = "018f0000-0000-7000-8000-000000000412";
const GRANDCHILD_WORKFLOW: &str = "018f0000-0000-7000-8000-000000000413";
const GRANDCHILD_VERSION: &str = "018f0000-0000-7000-8000-000000000414";
const MODEL: &str = "018f0000-0000-7000-8000-000000000421";
const MODEL_VERSION: &str = "018f0000-0000-7000-8000-000000000422";
const MCP: &str = "018f0000-0000-7000-8000-000000000423";
const MCP_VERSION: &str = "018f0000-0000-7000-8000-000000000424";
const MCP_SERVER: &str = "018f0000-0000-7000-8000-00000000042d";
const MCP_SERVER_VERSION: &str = "018f0000-0000-7000-8000-00000000042e";
const MCP_CREDENTIAL: &str = "018f0000-0000-7000-8000-00000000042f";
const MCP_CREDENTIAL_VERSION: &str = "018f0000-0000-7000-8000-000000000430";
const RAG: &str = "018f0000-0000-7000-8000-000000000425";
const RAG_VERSION: &str = "018f0000-0000-7000-8000-000000000426";
const RAG_CREDENTIAL: &str = "018f0000-0000-7000-8000-000000000451";
const RAG_CREDENTIAL_VERSION: &str = "018f0000-0000-7000-8000-000000000452";
const MEMORY: &str = "018f0000-0000-7000-8000-000000000427";
const MEMORY_VERSION: &str = "018f0000-0000-7000-8000-000000000428";
const SKILL: &str = "018f0000-0000-7000-8000-000000000429";
const SKILL_VERSION: &str = "018f0000-0000-7000-8000-00000000042a";
const SANDBOX_PROFILE: &str = "018f0000-0000-7000-8000-00000000042b";
const APPROVAL_WORKFLOW: &str = "018f0000-0000-7000-8000-000000000433";
const APPROVAL_IDENTITY: &str = "018f0000-0000-7000-8000-000000000434";

struct ResourceFixture {
    node_id: &'static str,
    resource_type: &'static str,
    resource_id: Uuid,
    version_id: Uuid,
    operation: &'static str,
    snapshot: Value,
}

#[tokio::main]
async fn main() -> Result<()> {
    agentx_service_kit::install_tls_provider();
    let dependencies_namespace = env::var("AGENTX_V2_FIXTURE_DEPENDENCIES_NAMESPACE")
        .context("AGENTX_V2_FIXTURE_DEPENDENCIES_NAMESPACE is required")?;
    anyhow::ensure!(
        dependencies_namespace
            .chars()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == '-'),
        "dependencies Namespace is not a DNS label"
    );
    let settings = ControlInfrastructureSettings::from_env()?;
    let pool = connect_control_mysql(&settings.mysql).await?;
    let objects = control_object_store(&settings.object_storage)?;
    let tenant = id(TENANT)?;
    let workflow_version = id(VERSION)?;
    let child_version = id(CHILD_VERSION)?;
    let skill_version = id(SKILL_VERSION)?;

    let skill = RuntimeSkillProgramV2 {
        schema_version: 2,
        skill_version_id: skill_version,
        instructions: "Return the immutable V2-04 Skill result.".into(),
        assets: vec![],
        dependencies: vec![],
    };
    let skill_bytes = agentx_runtime_contracts::canonical_bytes(&skill)?;
    let skill_hash = agentx_runtime_contracts::content_hash(&skill)?;
    let skill_source_key = format!(
        "control/{tenant}/{skill_version}/{}",
        skill_hash
            .as_str()
            .strip_prefix("sha256:")
            .expect("ContentHash has sha256 prefix")
    );
    objects
        .put(
            &ObjectPath::from(skill_source_key.clone()),
            Bytes::from(skill_bytes.clone()).into(),
        )
        .await?;

    let grandchild_definition = grandchild_definition()?;
    let grandchild_version = id(GRANDCHILD_VERSION)?;
    compile_workflow_version(&grandchild_definition, grandchild_version)
        .context("V2-04 grandchild Workflow does not compile")?;
    let child_definition = child_definition()?;
    compile_workflow_version_with_dependencies(
        &child_definition,
        child_version,
        &BTreeMap::from([(grandchild_version, grandchild_definition.clone())]),
    )
    .context("V2-04 child Workflow does not compile")?;
    let definition = full_definition()?;
    compile_workflow_version(&definition, workflow_version)
        .context("V2-04 full Workflow does not compile")?;
    let approval_definition = suspension_definition()?;
    compile_workflow_version(&approval_definition, id(APPROVAL_WORKFLOW)?)
        .context("V2-04 Approval Debug Workflow does not compile")?;
    let resources = resource_fixtures(
        &dependencies_namespace,
        tenant,
        &skill_source_key,
        skill_hash.as_str(),
        skill_bytes.len() as u64,
    )?;
    seed_fixture_secrets(tenant).await?;

    let admission_epoch = fixture_admission_epoch();
    let mut tx = pool.begin().await?;
    seed_workflows(
        &mut tx,
        &definition,
        &child_definition,
        &grandchild_definition,
        &approval_definition,
        admission_epoch,
    )
    .await?;
    seed_resources(&mut tx, &resources).await?;
    seed_runtime_identity_admission(&mut tx, admission_epoch).await?;
    tx.commit().await?;

    println!(
        "{}",
        serde_json::to_string(&json!({
            "apiVersion":"agentx.io/v2-04-fixture/v1",
            "tenantId":tenant,
            "applicationId":APPLICATION,
            "applicationSlug":"v2-04-runtime-engine",
            "workflowId":WORKFLOW,
            "workflowVersionId":workflow_version,
            "environmentId":id(ENVIRONMENT)?,
            "serviceIdentityId":IDENTITY,
            "childWorkflowVersionId":CHILD_VERSION,
            "grandchildWorkflowVersionId":GRANDCHILD_VERSION,
            "approvalWorkflowId":APPROVAL_WORKFLOW,
            "skillObjectId":id(SKILL_VERSION)?,
            "resources":{
                "model":id(MODEL)?,
                "mcp":MCP,
                "rag":RAG,
                "memory":id(MEMORY)?,
                "skill":id(SKILL)?,
                "sandboxProfile":SANDBOX_PROFILE
            }
        }))?
    );
    Ok(())
}

fn fixture_id(seed: &str, purpose: &str) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(seed.as_bytes());
    digest.update([0]);
    digest.update(purpose.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn id(value: &str) -> Result<Uuid> {
    let override_name = match value {
        TENANT => Some("AGENTX_V2_FIXTURE_TENANT_ID"),
        USER => Some("AGENTX_V2_FIXTURE_USER_ID"),
        DEPARTMENT => Some("AGENTX_V2_FIXTURE_DEPARTMENT_ID"),
        ENVIRONMENT => Some("AGENTX_V2_FIXTURE_ENVIRONMENT_ID"),
        _ => None,
    };
    if value == VERSION {
        let policy =
            env::var("AGENTX_V2_FIXTURE_SESSION_POLICY").unwrap_or_else(|_| "invocation".into());
        anyhow::ensure!(
            matches!(policy.as_str(), "invocation" | "application_session"),
            "AGENTX_V2_FIXTURE_SESSION_POLICY must be invocation or application_session"
        );
        let seed = env::var("AGENTX_V2_FIXTURE_ID_SEED").unwrap_or_default();
        if !seed.is_empty() {
            return Ok(fixture_id(&format!("{seed}:{policy}"), "workflow-version"));
        }
    }
    // Model, Memory, and Skill snapshots vary by fixture scenario. Scope both
    // resource identity and immutable version so exact grants are never
    // rewritten underneath an earlier published Workflow Version.
    if matches!(
        value,
        MODEL | MEMORY | SKILL | MODEL_VERSION | MEMORY_VERSION | SKILL_VERSION
    ) {
        let seed = env::var("AGENTX_V2_FIXTURE_ID_SEED").unwrap_or_default();
        if !seed.is_empty() {
            let policy = env::var("AGENTX_V2_FIXTURE_SESSION_POLICY")
                .unwrap_or_else(|_| "invocation".into());
            let purpose = match value {
                MODEL => "model",
                MEMORY => "memory",
                SKILL => "skill",
                MODEL_VERSION => "model-version",
                MEMORY_VERSION => "memory-version",
                SKILL_VERSION => "skill-version",
                _ => unreachable!(),
            };
            return Ok(fixture_id(&format!("{seed}:{policy}"), purpose));
        }
    }
    let value = override_name
        .and_then(|name| env::var(name).ok())
        .unwrap_or_else(|| value.to_owned());
    Uuid::parse_str(&value).map_err(Into::into)
}

async fn seed_fixture_secrets(tenant: Uuid) -> Result<()> {
    let endpoint = env::var("AGENTX_CONTROL_VAULT_ENDPOINT")?;
    let mount = env::var("AGENTX_CONTROL_VAULT_MOUNT").unwrap_or_else(|_| "secret".into());
    let token = env::var("AGENTX_CONTROL_VAULT_TOKEN")?;
    let client =
        agentx_service_kit::reqwest_client_builder_with_ca("AGENTX_CONTROL_VAULT_TLS_CA_PATH")?
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
    for (name, value) in [
        ("model", "m5-model-secret"),
        ("rag", "agentx-v2-04-rag-key"),
    ] {
        let response = client
            .post(format!(
                "{}/v1/{mount}/data/tenants/{tenant}/runtime-credentials/{name}",
                endpoint.trim_end_matches('/')
            ))
            .header("X-Vault-Token", &token)
            .json(&json!({"data":{"value":value}}))
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success(),
            "fixture Vault write for {name} failed with {}",
            response.status()
        );
    }
    Ok(())
}

fn reference(
    binding_role: Option<&str>,
    resource_type: &str,
    resource_id: &str,
    resource_version_id: &str,
    operation: &str,
) -> Value {
    let mut value = json!({
        "resourceType":resource_type,
        "resourceId":resource_id,
        "resourceVersionId":resource_version_id,
        "operation":operation
    });
    if let Some(role) = binding_role {
        value["bindingRole"] = json!(role);
    }
    value
}

fn inspector_reference(resource_type: &str, resource_id: &str, resource_version_id: &str) -> Value {
    json!({
        "bindingRole":resource_type,
        "resourceType":resource_type,
        "resourceId":resource_id,
        "resourceVersionId":resource_version_id,
        "operation":"use"
    })
}

fn full_definition() -> Result<WorkflowDefinition> {
    let session_policy =
        env::var("AGENTX_V2_FIXTURE_SESSION_POLICY").unwrap_or_else(|_| "invocation".into());
    anyhow::ensure!(
        matches!(
            session_policy.as_str(),
            "invocation" | "application_session"
        ),
        "AGENTX_V2_FIXTURE_SESSION_POLICY must be invocation or application_session"
    );
    let system_prompt = env::var("AGENTX_V2_FIXTURE_SYSTEM_PROMPT").unwrap_or_else(|_| {
        "P3_ATTACHMENT_MATRIX: use an attached tool once, then return the result.".into()
    });
    let user_question = env::var("AGENTX_V2_FIXTURE_USER_QUESTION")
        .map(|text| json!({"kind":"template","segments":[{"kind":"text","text":text}]}))
        .unwrap_or_else(|_| {
            json!({
                "kind":"template",
                "segments":[{
                    "kind":"reference",
                    "selector":{
                        "namespace":"inputs",
                        "run":{"kind":"current"},
                        "item":{"kind":"current"},
                        "path":["message"]
                    },
                    "missingPolicy":{"kind":"error"}
                }]
            })
        });
    let max_total_tokens = env::var("AGENTX_V2_FIXTURE_MAX_TOTAL_TOKENS")
        .ok()
        .map(|value| value.parse::<u64>().context("invalid max total tokens"))
        .transpose()?
        .unwrap_or(2_000);
    let memory_operation = "use".to_owned();
    let skill_version = id(SKILL_VERSION)?.to_string();
    let model_version = id(MODEL_VERSION)?.to_string();
    let memory_version = id(MEMORY_VERSION)?.to_string();
    let skill_id = id(SKILL)?.to_string();
    let model_id = id(MODEL)?.to_string();
    let memory_id = id(MEMORY)?.to_string();
    let parameters = json!({
        "systemPrompt":{"kind":"template","segments":[{"kind":"text","text":system_prompt}]},"userQuestion":user_question,
        "sessionPolicy":{"mode":session_policy},"maxIterations":4,"maxModelCalls":4,
        "maxToolCalls":4,"maxTotalTokens":max_total_tokens,"maxOutputTokens":512,"maxCost":0.01,
        "maxDurationSeconds":60,"limitAction":"fail"
    });
    let nodes = vec![json!({
        "id":"agent","key":"agent","type":"agent","typeVersion":2,"name":"Agent",
        "parameters":parameters,"contextWrites":[],"resourceReferences":[
            inspector_reference("model",&model_id,&model_version),
            reference(Some("mcp_tools"),"mcp_tool",MCP,MCP_VERSION,"use"),
            reference(Some("knowledge"),"rag",RAG,RAG_VERSION,"read"),
            reference(Some("long_term_memory"),"memory",&memory_id,&memory_version,&memory_operation),
            reference(Some("skills"),"skill",&skill_id,&skill_version,"use")
        ]
    })];
    let nodes = [nodes, vec![json!({
        "id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","disabled":false,"protected":true,
        "parameters":{"outputs":{},"errorOutputs":{}},"contextWrites":[],
        "resourceReferences":[],"settings":{}
    })]].concat();
    let connections = vec![
        json!({"id":"start-agent","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"agent","targetHandle":"main","order":0}),
        json!({"id":"agent-end","sourceNodeId":"agent","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}),
    ];
    serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"],"additionalProperties":false},"contexts":{}},
        "nodes":nodes,
        "connections":connections,
        "end":{"outputs":{}},
        "settings":{"activationBudget":64,"executionOrder":"deterministic"}
    }))
    .map_err(Into::into)
}

fn child_definition() -> Result<WorkflowDefinition> {
    serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","additionalProperties":true},"contexts":{}},
        "nodes":[{"id":"grandchild","key":"grandchild","type":"sub_workflow","typeVersion":1,"name":"Fixed Grandchild","parameters":{"workflowVersionId":GRANDCHILD_VERSION,"inputs":{"kind":"object","fields":{}}},"contextWrites":[],"resourceReferences":[]},{"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","disabled":false,"protected":true,"parameters":{"outputs":{},"errorOutputs":{}} ,"contextWrites":[],"resourceReferences":[],"settings":{}}],
        "connections":[
            {"id":"child-start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"grandchild","targetHandle":"main","order":0},
            {"id":"child-end","sourceNodeId":"grandchild","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}},
        "settings":{"activationBudget":8,"executionOrder":"deterministic"}
    }))
    .map_err(Into::into)
}

fn grandchild_definition() -> Result<WorkflowDefinition> {
    serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","additionalProperties":true},"contexts":{}},
        "nodes":[{"id":"grandchild-pass","key":"grandchild_pass","type":"set","typeVersion":1,"name":"Grandchild Pass","parameters":{},"contextWrites":[],"resourceReferences":[]},{"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","disabled":false,"protected":true,"parameters":{"outputs":{},"errorOutputs":{}},"contextWrites":[],"resourceReferences":[],"settings":{}}],
        "connections":[
            {"id":"grandchild-start","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"grandchild-pass","targetHandle":"main","order":0},
            {"id":"grandchild-end","sourceNodeId":"grandchild-pass","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}},
        "settings":{"activationBudget":8,"executionOrder":"deterministic"}
    }))
    .map_err(Into::into)
}

fn suspension_definition() -> Result<WorkflowDefinition> {
    let parameters = json!({
        "title":{"kind":"template","segments":[{"kind":"text","text":"V2-04 Runtime approval"}]},
        "description":{"kind":"template","segments":[{"kind":"text","text":"Kubernetes E2E decision"}]},
        "candidateUserId":USER,
        "timeoutMs":300000
    });
    let connections = vec![
        json!({"id":"start-suspend","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"suspend","targetHandle":"main","order":0}),
        json!({"id":"approved-end","sourceNodeId":"suspend","sourceHandle":"decision:approved","targetNodeId":"exit","targetHandle":"main","order":0}),
        json!({"id":"rejected-end","sourceNodeId":"suspend","sourceHandle":"decision:rejected","targetNodeId":"exit","targetHandle":"main","order":1}),
        json!({"id":"timed-out-end","sourceNodeId":"suspend","sourceHandle":"timed_out","targetNodeId":"exit","targetHandle":"main","order":2}),
    ];
    serde_json::from_value(json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","additionalProperties":true},"contexts":{}},
        "nodes":[{"id":"suspend","key":"suspend","type":"approval","typeVersion":1,"name":"Approval","parameters":parameters,"contextWrites":[],"resourceReferences":[]},{"id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End","disabled":false,"protected":true,"parameters":{"outputs":{},"errorOutputs":{}},"contextWrites":[],"resourceReferences":[],"settings":{}}],
        "connections":connections,
        "end":{"outputs":{}},
        "settings":{"activationBudget":20,"executionOrder":"deterministic"}
    }))
    .map_err(Into::into)
}

fn vault_reference(tenant: Uuid, credential: &str) -> Value {
    json!({
        "mount":"secret",
        "path":format!("tenants/{tenant}/runtime-credentials/{credential}"),
        "key":"value",
        "version":1
    })
}

fn resource_fixtures(
    dependencies_namespace: &str,
    tenant: Uuid,
    skill_source_key: &str,
    skill_hash: &str,
    skill_size: u64,
) -> Result<Vec<ResourceFixture>> {
    let echo = format!("http://echo-mcp.{dependencies_namespace}.svc:8090");
    let rag = format!("http://lightrag.{dependencies_namespace}.svc:9621");
    let memory = format!("http://mem0.{dependencies_namespace}.svc:8000");
    let skill_version = id(SKILL_VERSION)?;
    let mcp_input_schema = json!({
        "type":"object",
        "properties":{"text":{"type":"string"}},
        "required":["text"],
        "additionalProperties":false
    });
    let mcp_schema_hash = agentx_runtime_contracts::content_hash(&mcp_input_schema)?;
    let skill_object = json!({
        "objectId":skill_version,
        "sourceKey":skill_source_key,
        "contentHash":skill_hash,
        "sizeBytes":skill_size,
        "mediaType":"application/vnd.agentx.skill-program.v2+json"
    });
    let mcp_transport = json!({"kind":"streamable_http","endpoint":format!("{echo}/mcp")});
    let mcp_credential = vault_reference(tenant, "model");
    let rag_credential = vault_reference(tenant, "rag");
    let memory_operation_key = "use";
    let model_context_window = if let Ok(value) = env::var("AGENTX_V2_FIXTURE_COMPACTION_THRESHOLD")
    {
        let threshold = value
            .parse::<u64>()
            .context("invalid compaction threshold")?;
        (threshold.saturating_mul(100).saturating_add(71) / 72).max(2)
    } else {
        128_000
    };
    Ok(vec![
        resource(
            "agent",
            "model",
            MODEL,
            MODEL_VERSION,
            "use",
            json!({
                "providerType":"openai_compatible",
                "endpoint":format!("{echo}/v1"),
                "modelName":"echo-model",
                "contextWindow":model_context_window,
                "price":{"versionId":"fixture-v1","currency":"USD","inputPerMillion":"0","outputPerMillion":"0"},
                "resourceVersion":1,
                "vaultSecretRef":mcp_credential
            }),
        )?,
        resource(
            "agent",
            "mcp_server",
            MCP_SERVER,
            MCP_SERVER_VERSION,
            "use",
            json!({
                "serverId":MCP_SERVER,
                "serverVersionId":MCP_SERVER_VERSION,
                "transport":mcp_transport,
                "toolName":"__server__",
                "configurationHash":mcp_schema_hash,
                "resourceVersion":1,
                "vaultSecretRef":mcp_credential
            }),
        )?,
        resource(
            "agent",
            "credential",
            MCP_CREDENTIAL,
            MCP_CREDENTIAL_VERSION,
            "use",
            json!({"credentialType":"bearer","resourceVersion":1,"vaultSecretRef":mcp_credential}),
        )?,
        resource(
            "agent",
            "mcp_tool",
            MCP,
            MCP_VERSION,
            "use",
            json!({
                "serverId":MCP_SERVER,
                "serverVersionId":MCP_SERVER_VERSION,
                "transport":mcp_transport,
                "toolName":"echo",
                "schemaHash":mcp_schema_hash,
                "inputSchema":mcp_input_schema,
                "sideEffect":"read_only",
                "resourceVersion":1,
                "vaultSecretRef":mcp_credential
            }),
        )?,
        resource(
            "agent",
            "credential",
            RAG_CREDENTIAL,
            RAG_CREDENTIAL_VERSION,
            "use",
            json!({"credentialType":"bearer","resourceVersion":1,"vaultSecretRef":rag_credential}),
        )?,
        resource(
            "agent",
            "rag",
            RAG,
            RAG_VERSION,
            "read",
            json!({"provider":"lightrag","endpoint":rag,"connectionId":RAG,"connectionVersion":1,"scope":{"kind":"lightrag","workspace":""},"name":"Knowledge","description":"Search external documentation","resourceVersion":1,"vaultSecretRef":rag_credential}),
        )?,
        resource(
            "agent",
            "memory",
            MEMORY,
            MEMORY_VERSION,
            memory_operation_key,
            json!({"endpoint":memory,"resourceVersion":1}),
        )?,
        resource(
            "agent",
            "skill",
            SKILL,
            SKILL_VERSION,
            "use",
            json!({
                "resourceVersion":1,
                "entrypointObjectId":skill_version,
                "entrypointContentHash":skill_hash,
                "dependencyObjectIds":[],
                "dependencies":[],
                "runtimeObjects":[skill_object]
            }),
        )?,
    ])
}

fn resource(
    node_id: &'static str,
    resource_type: &'static str,
    resource_id: &str,
    version_id: &str,
    operation: &'static str,
    snapshot: Value,
) -> Result<ResourceFixture> {
    Ok(ResourceFixture {
        node_id,
        resource_type,
        resource_id: id(resource_id)?,
        version_id: id(version_id)?,
        operation,
        snapshot,
    })
}

async fn seed_workflows(
    tx: &mut Transaction<'_, MySql>,
    definition: &WorkflowDefinition,
    child_definition: &WorkflowDefinition,
    grandchild_definition: &WorkflowDefinition,
    approval_definition: &WorkflowDefinition,
    admission_epoch: u64,
) -> Result<()> {
    let tenant = id(TENANT)?;
    let user = id(USER)?;
    let department = id(DEPARTMENT)?;
    let workflow = id(WORKFLOW)?;
    let child_workflow = id(CHILD_WORKFLOW)?;
    let grandchild_workflow = id(GRANDCHILD_WORKFLOW)?;
    let version = id(VERSION)?;
    let child_version = id(CHILD_VERSION)?;
    let grandchild_version = id(GRANDCHILD_VERSION)?;
    let environment = id(ENVIRONMENT)?;
    let deployment = workflow_deployment_id(version)?;
    let application = id(APPLICATION)?;
    let fixture_version_number = sqlx::query_scalar::<_, u64>(
        "SELECT CAST(COALESCE((SELECT version_number FROM workflow_versions WHERE tenant_id=? AND id=?),(SELECT COALESCE(MAX(version_number),0)+1 FROM workflow_versions WHERE tenant_id=? AND workflow_id=?)) AS UNSIGNED)",
    )
    .bind(tenant)
    .bind(version)
    .bind(tenant)
    .bind(workflow)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("INSERT INTO workflows(id,tenant_id,name,status,visibility,owner_user_id,owner_department_id) VALUES(?,?,'V2-04 Runtime Engine','active','private',?,?) ON DUPLICATE KEY UPDATE name=VALUES(name)")
        .bind(workflow).bind(tenant).bind(user).bind(department).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO workflows(id,tenant_id,name,status,visibility,owner_user_id,owner_department_id) VALUES(?,?,'V2-04 Composite Child','active','private',?,?) ON DUPLICATE KEY UPDATE name=VALUES(name)")
        .bind(child_workflow).bind(tenant).bind(user).bind(department).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO workflows(id,tenant_id,name,status,visibility,owner_user_id,owner_department_id) VALUES(?,?,'V2-04 Composite Grandchild','active','private',?,?) ON DUPLICATE KEY UPDATE name=VALUES(name)")
        .bind(grandchild_workflow).bind(tenant).bind(user).bind(department).execute(&mut **tx).await?;
    for (workflow_id, name, identity_id, draft) in [(
        id(APPROVAL_WORKFLOW)?,
        "V2-04 Approval Debug",
        id(APPROVAL_IDENTITY)?,
        approval_definition,
    )] {
        sqlx::query("INSERT INTO workflows(id,tenant_id,name,status,visibility,owner_user_id,owner_department_id) VALUES(?,?,?,'active','private',?,?) ON DUPLICATE KEY UPDATE name=VALUES(name)")
            .bind(workflow_id).bind(tenant).bind(name).bind(user).bind(department).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO workflow_service_identities(id,tenant_id,workflow_id,status,version) VALUES(?,?,?,'active',1) ON DUPLICATE KEY UPDATE status='active',version=1")
            .bind(identity_id).bind(tenant).bind(workflow_id).execute(&mut **tx).await?;
        seed_draft(tx, tenant, workflow_id, draft, user).await?;
    }
    sqlx::query("INSERT INTO workflow_service_identities(id,tenant_id,workflow_id,status,version) VALUES(?,?,?,'active',?) ON DUPLICATE KEY UPDATE status='active',version=VALUES(version)")
        .bind(id(IDENTITY)?).bind(tenant).bind(workflow).bind(admission_epoch).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO workflow_members(tenant_id,workflow_id,user_id,member_role,created_by) VALUES(?,?,?,'manager',?) ON DUPLICATE KEY UPDATE member_role='manager'")
        .bind(tenant).bind(workflow).bind(user).bind(user).execute(&mut **tx).await?;
    let workflow_grant = json!({
        "userId":user,
        "workflowId":workflow,
        "grantVersion":admission_epoch,
        "canQuery":true,
        "admissionEpoch":admission_epoch
    });
    let workflow_grant_hash = agentx_runtime_contracts::content_hash(&workflow_grant)?;
    sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,'RuntimeUserWorkflowGrantChanged','workflow_admission',?,?,'pending',?,?) ON DUPLICATE KEY UPDATE payload_json=VALUES(payload_json),status='pending',request_hash=VALUES(request_hash)")
        .bind(Uuid::now_v7()).bind(tenant).bind(workflow.to_string()).bind(workflow_grant)
        .bind(workflow_grant_hash.as_str()).bind(format!("workflow-query-grant:{workflow}:{user}:{admission_epoch}"))
        .execute(&mut **tx).await?;
    for (index, (version_id, workflow_id, value, hash)) in [
        (
            version,
            workflow,
            serde_json::to_value(definition)?,
            agentx_runtime_contracts::content_hash(definition)?.to_string(),
        ),
        (
            child_version,
            child_workflow,
            serde_json::to_value(child_definition)?,
            agentx_runtime_contracts::content_hash(child_definition)?.to_string(),
        ),
        (
            grandchild_version,
            grandchild_workflow,
            serde_json::to_value(grandchild_definition)?,
            agentx_runtime_contracts::content_hash(grandchild_definition)?.to_string(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let version_number = if index == 0 {
            fixture_version_number
        } else {
            1_u64
        };
        let parsed = serde_json::from_value(value.clone())?;
        let (draft_id, revision_id, revision) =
            seed_draft(tx, tenant, workflow_id, &parsed, user).await?;
        let editor = serde_json::to_value(agentx_domain::EditorDocument::default())?;
        let editor_hash = agentx_domain::canonical_content_hash(&editor)?;
        let registry = agentx_runtime::NodeRegistry::m5_defaults();
        let manifests = parsed
            .nodes
            .iter()
            .filter_map(|node| {
                registry
                    .resolve_definition_manifest(
                        &node.node_type,
                        node.type_version,
                        &node.parameters,
                    )
                    .map(|manifest| (node.id.clone(), manifest.clone()))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        sqlx::query("INSERT INTO workflow_versions(id,tenant_id,workflow_id,version_number,source_draft_id,source_revision_id,source_revision,schema_version,definition_json,node_manifests_json,editor_json,content_hash,editor_hash,created_by,updated_by,publication_status,published_at) VALUES(?,?,?,?,?,?,?,'8.0',?,?,?,?,?,?,?,'published',UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE id=id")
            .bind(version_id).bind(tenant).bind(workflow_id).bind(version_number).bind(draft_id).bind(revision_id).bind(revision).bind(value).bind(serde_json::to_value(manifests)?).bind(editor).bind(hash).bind(editor_hash).bind(user).bind(user).execute(&mut **tx).await?;
    }
    let deployment_sequence = sqlx::query_scalar::<_, u64>(
        "SELECT CAST(COALESCE((SELECT sequence_number FROM workflow_deployments WHERE id=?),(SELECT COALESCE(MAX(sequence_number),0)+1 FROM workflow_deployments WHERE tenant_id=? AND workflow_id=? AND environment_id=?)) AS UNSIGNED)",
    )
    .bind(deployment)
    .bind(tenant)
    .bind(workflow)
    .bind(environment)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("UPDATE workflow_deployments SET status='superseded' WHERE tenant_id=? AND workflow_id=? AND environment_id=? AND id<>? AND status='active'")
        .bind(tenant).bind(workflow).bind(environment).bind(deployment).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO workflow_deployments(id,tenant_id,workflow_id,environment_id,workflow_version_id,sequence_number,status,source,created_by) VALUES(?,?,?,?,?,?,'active','publish',?) ON DUPLICATE KEY UPDATE workflow_id=VALUES(workflow_id),environment_id=VALUES(environment_id),workflow_version_id=VALUES(workflow_version_id),sequence_number=VALUES(sequence_number),status='active'")
        .bind(deployment).bind(tenant).bind(workflow).bind(environment).bind(version).bind(deployment_sequence).bind(user).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO workflow_deployment_heads(tenant_id,workflow_id,environment_id,active_deployment_id,version) VALUES(?,?,?,?,1) ON DUPLICATE KEY UPDATE active_deployment_id=VALUES(active_deployment_id),version=version+1")
        .bind(tenant).bind(workflow).bind(environment).bind(deployment).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO applications(id,tenant_id,workflow_id,name,slug,visibility,owner_user_id,owner_department_id,status) VALUES(?,?,?,'V2-04 Runtime Engine','v2-04-runtime-engine','private',?,?,'draft') ON DUPLICATE KEY UPDATE status='draft'")
        .bind(application).bind(tenant).bind(workflow).bind(user).bind(department).execute(&mut **tx).await?;
    Ok(())
}

fn workflow_deployment_id(workflow_version_id: Uuid) -> Result<Uuid> {
    if workflow_version_id == Uuid::parse_str(VERSION)? {
        id(WORKFLOW_DEPLOYMENT)
    } else {
        Ok(fixture_id(
            &workflow_version_id.to_string(),
            "workflow-deployment",
        ))
    }
}

fn fixture_admission_epoch() -> u64 {
    // Millisecond precision keeps sequential fixture Jobs distinct even when
    // they run within one Unix second, while remaining a normal u64 epoch.
    (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as u64
}

async fn seed_resources(
    tx: &mut Transaction<'_, MySql>,
    resources: &[ResourceFixture],
) -> Result<()> {
    let tenant = id(TENANT)?;
    let identity = id(IDENTITY)?;
    let user = id(USER)?;
    let version = id(VERSION)?;
    for (index, resource) in resources.iter().enumerate() {
        let snapshot_hash = agentx_runtime_contracts::content_hash(&resource.snapshot)?;
        sqlx::query("INSERT INTO workflow_version_resources(id,tenant_id,workflow_version_id,node_id,resource_type,resource_id,resource_version_id,operation_key,snapshot_json,snapshot_hash) VALUES(?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE snapshot_json=VALUES(snapshot_json),snapshot_hash=VALUES(snapshot_hash)")
            .bind(Uuid::now_v7()).bind(tenant).bind(version).bind(resource.node_id)
            .bind(resource.resource_type).bind(resource.resource_id).bind(resource.version_id)
            .bind(resource.operation).bind(&resource.snapshot).bind(snapshot_hash.as_str())
            .execute(&mut **tx).await?;
        let grant_operation = if resource.operation == "manage" {
            "manage"
        } else if resource.operation == "write" {
            "write"
        } else if resource.operation == "read" {
            "read"
        } else {
            "use"
        };
        sqlx::query("INSERT INTO resource_grants(id,tenant_id,subject_type,subject_id,resource_type,resource_id,resource_version_id,operation_key,created_by) VALUES(?,?,'workflow_service_identity',?,?,?,?,?,?) ON DUPLICATE KEY UPDATE resource_version_id=VALUES(resource_version_id)")
            .bind(Uuid::now_v7()).bind(tenant).bind(identity).bind(resource.resource_type)
            .bind(resource.resource_id).bind(resource.version_id).bind(grant_operation).bind(user)
            .execute(&mut **tx).await?;
        if index == 0 {
            anyhow::ensure!(snapshot_hash.as_str().starts_with("sha256:"));
        }
    }
    Ok(())
}

async fn seed_runtime_identity_admission(
    tx: &mut Transaction<'_, MySql>,
    admission_epoch: u64,
) -> Result<()> {
    let tenant = id(TENANT)?;
    let identity = id(IDENTITY)?;
    let workflow = id(WORKFLOW)?;
    // Every fixture invocation may replace immutable resource versions while
    // retaining the stable service identity.  Use a fresh monotonic epoch and
    // include it in outbox idempotency keys so Runtime accepts the new
    // authorization snapshot instead of treating it as a conflicting replay.
    let grants = sqlx::query("SELECT id,resource_type,resource_id,operation_key FROM resource_grants WHERE tenant_id=? AND subject_type='workflow_service_identity' AND subject_id=? ORDER BY id")
        .bind(tenant)
        .bind(identity)
        .fetch_all(&mut **tx)
        .await?;
    let mut grant_ids = Vec::with_capacity(grants.len());
    for grant in grants {
        let grant_id: Uuid = grant.try_get("id")?;
        let resource_type: String = grant.try_get("resource_type")?;
        let resource_id: Uuid = grant.try_get("resource_id")?;
        let operation: String = grant.try_get("operation_key")?;
        grant_ids.push(grant_id);
        let payload = json!({
            "identityId":identity,
            "grantId":grant_id,
            "resourceType":resource_type,
            "resourceId":resource_id,
            "operations":[operation],
            "enabled":true,
            "policyEpoch":admission_epoch,
            "admissionEpoch":admission_epoch
        });
        let request_hash = agentx_runtime_contracts::content_hash(&payload)?;
        sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,'ResourceGrantAdmissionChanged','workflow_admission',?,?,'pending',?,?) ON DUPLICATE KEY UPDATE payload_json=VALUES(payload_json),status='pending',request_hash=VALUES(request_hash)")
            .bind(Uuid::now_v7()).bind(tenant).bind(grant_id.to_string()).bind(payload)
            .bind(request_hash.as_str()).bind(format!("resource-grant-admission:{grant_id}:{admission_epoch}:active"))
            .execute(&mut **tx).await?;
    }
    let payload = json!({
        "workflowId":workflow,
        "identityId":identity,
        "status":"active",
        "policyEpoch":admission_epoch,
        "capabilities":agentx_node_protocol::ALL_RUNTIME_CAPABILITIES,
        "grantIds":grant_ids,
        "admissionEpoch":admission_epoch
    });
    let request_hash = agentx_runtime_contracts::content_hash(&payload)?;
    sqlx::query("INSERT INTO outbox(id,tenant_id,event_type,aggregate_type,aggregate_id,payload_json,status,request_hash,idempotency_key) VALUES(?,?,'ServiceIdentityAdmissionChanged','workflow_admission',?,?,'pending',?,?) ON DUPLICATE KEY UPDATE payload_json=VALUES(payload_json),status='pending',request_hash=VALUES(request_hash)")
        .bind(Uuid::now_v7()).bind(tenant).bind(identity.to_string()).bind(payload)
        .bind(request_hash.as_str()).bind(format!("service-identity-admission:{identity}:{admission_epoch}"))
        .execute(&mut **tx).await?;
    Ok(())
}

async fn seed_draft(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    workflow: Uuid,
    definition: &WorkflowDefinition,
    user: Uuid,
) -> Result<(Uuid, Uuid, u64)> {
    let draft = agentx_runtime_contracts::deterministic_uuid(workflow, b"fixture-draft");
    let editor = serde_json::to_value(agentx_domain::EditorDocument::default())?;
    let editor_hash = agentx_domain::canonical_content_hash(&editor)?;
    let empty = serde_json::to_value(WorkflowDefinition::empty())?;
    let initial = agentx_domain::canonical_content_hash(&empty)?;
    sqlx::query("INSERT INTO workflow_drafts(id,tenant_id,workflow_id,schema_version,revision,definition_json,editor_json,content_hash,editor_hash,created_by,updated_by) VALUES(?,?,?,'8.0',0,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id").bind(draft).bind(tenant).bind(workflow).bind(&empty).bind(&editor).bind(&initial).bind(&editor_hash).bind(user).bind(user).execute(&mut **tx).await?;
    let zero = agentx_runtime_contracts::deterministic_uuid(draft, b"revision:0");
    sqlx::query("INSERT INTO workflow_draft_revisions(id,tenant_id,workflow_id,draft_id,revision,schema_version,definition_json,editor_json,content_hash,editor_hash,created_by) VALUES(?,?,?,?,0,'8.0',?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id").bind(zero).bind(tenant).bind(workflow).bind(draft).bind(empty).bind(&editor).bind(initial).bind(&editor_hash).bind(user).execute(&mut **tx).await?;
    let value = serde_json::to_value(definition)?;
    let hash = agentx_domain::canonical_content_hash(&value)?;
    let row = sqlx::query(
        "SELECT revision,content_hash FROM workflow_drafts WHERE tenant_id=? AND id=? FOR UPDATE",
    )
    .bind(tenant)
    .bind(draft)
    .fetch_one(&mut **tx)
    .await?;
    let current: u64 = row.try_get("revision")?;
    let revision = if row.try_get::<String, _>("content_hash")? == hash {
        current
    } else {
        current + 1
    };
    let revision_id = agentx_runtime_contracts::deterministic_uuid(
        draft,
        format!("revision:{revision}").as_bytes(),
    );
    sqlx::query("INSERT INTO workflow_draft_revisions(id,tenant_id,workflow_id,draft_id,revision,schema_version,definition_json,editor_json,content_hash,editor_hash,created_by) VALUES(?,?,?,?,?,'8.0',?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id").bind(revision_id).bind(tenant).bind(workflow).bind(draft).bind(revision).bind(&value).bind(&editor).bind(&hash).bind(&editor_hash).bind(user).execute(&mut **tx).await?;
    sqlx::query("UPDATE workflow_drafts SET revision=?,definition_json=?,content_hash=?,updated_by=? WHERE tenant_id=? AND id=?").bind(revision).bind(value).bind(hash).bind(user).bind(tenant).bind(draft).execute(&mut **tx).await?;
    Ok((draft, revision_id, revision))
}

#[cfg(test)]
mod tests {
    use agentx_runtime::{CompileContext, NodeRegistry, WorkflowCompiler};

    use super::*;

    #[test]
    fn suspension_fixtures_follow_the_current_node_manifests() {
        let definition = suspension_definition().unwrap();
        compile_workflow_version(&definition, id(APPROVAL_WORKFLOW).unwrap()).unwrap();
    }

    #[test]
    fn child_fixture_uses_the_fixed_grandchild_manifest() {
        let grandchild = grandchild_definition().unwrap();
        let grandchild_version = id(GRANDCHILD_VERSION).unwrap();
        compile_workflow_version_with_dependencies(
            &child_definition().unwrap(),
            id(CHILD_VERSION).unwrap(),
            &BTreeMap::from([(grandchild_version, grandchild)]),
        )
        .unwrap();
    }

    #[test]
    fn full_fixture_follows_the_current_agent_manifest() {
        let definition = full_definition().unwrap();
        let result = WorkflowCompiler::new(&NodeRegistry::m5_defaults())
            .compile(&definition, &CompileContext::default());
        assert!(result.is_ok(), "{:#?}", result.unwrap_err().issues);
    }

    #[test]
    fn seeded_fixture_ids_are_deterministic_and_purpose_scoped() {
        assert_eq!(fixture_id("run", "success"), fixture_id("run", "success"));
        assert_ne!(fixture_id("run", "success"), fixture_id("run", "cancelled"));
    }

    #[test]
    fn session_fixture_version_seed_is_stable_and_policy_scoped() {
        let seed = "e2e-run";
        assert_ne!(
            fixture_id(&format!("{seed}:invocation"), "workflow-version"),
            fixture_id(&format!("{seed}:application_session"), "workflow-version")
        );
        assert_eq!(
            fixture_id(&format!("{seed}:application_session"), "workflow-version"),
            fixture_id(&format!("{seed}:application_session"), "workflow-version")
        );
    }

    #[test]
    fn session_fixture_deployment_is_immutable_and_version_scoped() {
        let invocation = fixture_id("e2e-run:invocation", "workflow-version");
        let application = fixture_id("e2e-run:application_session", "workflow-version");
        assert_ne!(
            workflow_deployment_id(invocation).unwrap(),
            workflow_deployment_id(application).unwrap()
        );
        assert_eq!(
            workflow_deployment_id(Uuid::parse_str(VERSION).unwrap()).unwrap(),
            Uuid::parse_str(WORKFLOW_DEPLOYMENT).unwrap()
        );
    }

    #[test]
    fn agent_attachment_resources_use_the_frozen_runtime_snapshots() {
        let resources = resource_fixtures(
            "agentx-e2e-deps-fixture",
            id(TENANT).unwrap(),
            "control/fixture/skill",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            128,
        )
        .unwrap();
        let model = resources
            .iter()
            .find(|resource| resource.resource_type == "model")
            .unwrap();
        assert_eq!(model.snapshot["providerType"], "openai_compatible");
        assert!(model.snapshot["price"]["currency"].is_string());
        for credential in resources
            .iter()
            .filter(|resource| resource.resource_type == "credential")
        {
            assert_eq!(credential.snapshot["credentialType"], "bearer");
        }
        let server = resources
            .iter()
            .find(|resource| resource.resource_type == "mcp_server")
            .unwrap();
        assert_eq!(server.snapshot["transport"]["kind"], "streamable_http");
        assert_eq!(server.snapshot["serverVersionId"], MCP_SERVER_VERSION);
        let tool = resources
            .iter()
            .find(|resource| resource.resource_type == "mcp_tool")
            .unwrap();
        assert_eq!(tool.snapshot["serverId"], MCP_SERVER);
        assert_eq!(tool.snapshot["serverVersionId"], MCP_SERVER_VERSION);
        assert_eq!(tool.snapshot["inputSchema"]["required"], json!(["text"]));
        assert_eq!(
            tool.snapshot["inputSchema"]["properties"]["text"]["type"],
            "string"
        );
        let skill = resources
            .iter()
            .find(|resource| resource.resource_type == "skill")
            .unwrap();
        assert_eq!(skill.snapshot["entrypointObjectId"], SKILL_VERSION);
        assert_eq!(skill.snapshot["dependencies"], json!([]));
        assert_eq!(resources.len(), 8);
    }
}
