use std::{collections::BTreeSet, str::FromStr};

use agentx_runtime_contracts::{
    ContentHash, DELEGATION_TOKEN_TTL_SECONDS, DelegationClaimsV1, InvocationSearchPageV1,
    InvocationSearchRequestV1, SessionSearchPageV1, SessionSearchRequestV1, content_hash,
    issue_delegation_token, now_unix,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{MySqlPool, Row};
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
};

pub fn routes() -> Router<ControlApiState> {
    Router::new().route(
        "/api/v1/deletion-impact/{entity_type}/{id}",
        get(deletion_impact),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntityType {
    Workflow,
    Environment,
    Application,
    Credential,
    Model,
    McpServer,
    Skill,
    Knowledge,
    Memory,
    SandboxProfile,
    Dataset,
    EvaluationProfile,
    Department,
    Role,
    ApplicationWebhook,
    ApplicationSchedule,
}

impl EntityType {
    fn key(self) -> &'static str {
        match self {
            Self::Workflow => "workflow",
            Self::Environment => "environment",
            Self::Application => "application",
            Self::Credential => "credential",
            Self::Model => "model",
            Self::McpServer => "mcp_server",
            Self::Skill => "skill",
            Self::Knowledge => "knowledge",
            Self::Memory => "memory",
            Self::SandboxProfile => "sandbox_profile",
            Self::Dataset => "dataset",
            Self::EvaluationProfile => "evaluation_profile",
            Self::Department => "department",
            Self::Role => "role",
            Self::ApplicationWebhook => "application_webhook",
            Self::ApplicationSchedule => "application_schedule",
        }
    }

    fn permission(self) -> &'static str {
        match self {
            Self::Workflow | Self::Environment => "workflow:delete",
            Self::Application | Self::ApplicationWebhook | Self::ApplicationSchedule => {
                "application:delete"
            }
            Self::Credential => "credential:delete",
            Self::Model => "model:delete",
            Self::McpServer => "mcp:delete",
            Self::Skill => "skill:delete",
            Self::Knowledge => "knowledge:delete",
            Self::Memory => "memory:delete",
            Self::SandboxProfile => "sandbox:delete",
            Self::Dataset => "dataset:delete",
            Self::EvaluationProfile => "evaluation_profile:delete",
            Self::Department => "department:delete",
            Self::Role => "role:delete",
        }
    }

    fn resource_type(self) -> Option<&'static str> {
        match self {
            Self::Credential => Some("credential"),
            Self::Model => Some("model"),
            Self::McpServer => Some("mcp_server"),
            Self::Skill => Some("skill"),
            Self::Knowledge => Some("rag"),
            Self::Memory => Some("memory"),
            Self::SandboxProfile => Some("sandbox_profile"),
            Self::Workflow => Some("workflow"),
            _ => None,
        }
    }
}

impl FromStr for EntityType {
    type Err = ApiError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.replace('-', "_").as_str() {
            "workflow" | "workflows" => Ok(Self::Workflow),
            "environment" | "environments" => Ok(Self::Environment),
            "application" | "applications" => Ok(Self::Application),
            "credential" | "credentials" => Ok(Self::Credential),
            "model" | "model_alias" | "model_aliases" => Ok(Self::Model),
            "mcp" | "mcp_server" | "mcp_servers" => Ok(Self::McpServer),
            "skill" | "skills" => Ok(Self::Skill),
            "knowledge" | "knowledge_resource" | "knowledge_resources" | "rag" => {
                Ok(Self::Knowledge)
            }
            "memory" | "memory_connection" | "memory_connections" => Ok(Self::Memory),
            "sandbox" | "sandbox_profile" | "sandbox_profiles" => Ok(Self::SandboxProfile),
            "dataset" | "datasets" => Ok(Self::Dataset),
            "evaluation_profile" | "evaluation_profiles" => Ok(Self::EvaluationProfile),
            "department" | "departments" => Ok(Self::Department),
            "role" | "roles" => Ok(Self::Role),
            "application_webhook" | "application_webhooks" | "webhook" => {
                Ok(Self::ApplicationWebhook)
            }
            "application_schedule" | "application_schedules" | "schedule" => {
                Ok(Self::ApplicationSchedule)
            }
            _ => Err(ApiError::bad_request(
                "INVALID_ENTITY_TYPE",
                "Entity type does not support deletion",
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeletionReference {
    source_module: String,
    source_type: String,
    source_id: Uuid,
    source_name: String,
    relation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_name: Option<String>,
    immutable: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeletionImpactResponse {
    deletable: bool,
    target_version: u64,
    total: u64,
    page: u32,
    page_size: u32,
    references: Vec<DeletionReference>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImpactQuery {
    page: Option<u32>,
    page_size: Option<u32>,
}

struct Target {
    name: String,
    version: u64,
    immutable: bool,
}

async fn deletion_impact(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((entity_type, id)): Path<(String, Uuid)>,
    Query(query): Query<ImpactQuery>,
) -> ApiResult<Json<DeletionImpactResponse>> {
    let entity = EntityType::from_str(&entity_type)?;
    actor.require(entity.permission())?;
    if matches!(entity, EntityType::Dataset | EntityType::EvaluationProfile) {
        let query = if entity == EntityType::Dataset {
            "SELECT application_id FROM datasets WHERE tenant_id=? AND id=?"
        } else {
            "SELECT application_id FROM evaluation_profiles WHERE tenant_id=? AND id=?"
        };
        let application: Uuid = sqlx::query_scalar(query)
            .bind(actor.tenant_id)
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("Deletion target"))?;
        crate::application_testing::authorize(&state, &actor, application, entity.permission())
            .await?;
    }
    let target = load_target(&state.pool, actor.tenant_id, entity, id).await?;
    let mut references = if target.immutable {
        vec![DeletionReference {
            source_module: "system".into(),
            source_type: entity.key().into(),
            source_id: id,
            source_name: target.name,
            relation: "immutable".into(),
            parent_id: None,
            node_id: None,
            node_name: None,
            immutable: true,
        }]
    } else {
        control_references(&state.pool, actor.tenant_id, entity, id).await?
    };
    if entity == EntityType::Application {
        references.extend(runtime_application_references(&state, &actor, id).await?);
    }
    references.sort_by(|left, right| {
        (
            &left.source_module,
            &left.source_type,
            &left.source_name,
            left.source_id,
        )
            .cmp(&(
                &right.source_module,
                &right.source_type,
                &right.source_name,
                right.source_id,
            ))
    });
    references.dedup_by(|left, right| {
        left.source_type == right.source_type
            && left.source_id == right.source_id
            && left.relation == right.relation
            && left.node_id == right.node_id
    });
    let total = references.len() as u64;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let offset = ((page - 1) * page_size) as usize;
    Ok(Json(DeletionImpactResponse {
        deletable: total == 0,
        target_version: target.version,
        total,
        page,
        page_size,
        references: references
            .into_iter()
            .skip(offset)
            .take(page_size as usize)
            .collect(),
    }))
}

async fn load_target(
    pool: &MySqlPool,
    tenant_id: Uuid,
    entity: EntityType,
    id: Uuid,
) -> ApiResult<Target> {
    let sql = match entity {
        EntityType::Workflow => {
            "SELECT name,version,FALSE immutable FROM workflows WHERE tenant_id=? AND id=?"
        }
        EntityType::Environment => {
            "SELECT name,version,is_builtin immutable FROM workflow_environments WHERE tenant_id=? AND id=?"
        }
        EntityType::Application => {
            "SELECT name,version,FALSE immutable FROM applications WHERE tenant_id=? AND id=?"
        }
        EntityType::Credential => {
            "SELECT name,version,FALSE immutable FROM credentials WHERE tenant_id=? AND id=?"
        }
        EntityType::Model => {
            "SELECT alias name,version,FALSE immutable FROM model_aliases WHERE tenant_id=? AND id=?"
        }
        EntityType::McpServer => {
            "SELECT name,version,FALSE immutable FROM mcp_servers WHERE tenant_id=? AND id=?"
        }
        EntityType::Skill => {
            "SELECT name,version,FALSE immutable FROM skills WHERE tenant_id=? AND id=?"
        }
        EntityType::Knowledge => {
            "SELECT name,version,FALSE immutable FROM rag_resources WHERE tenant_id=? AND id=?"
        }
        EntityType::Memory => {
            "SELECT name,version,FALSE immutable FROM memory_connections WHERE tenant_id=? AND id=?"
        }
        EntityType::SandboxProfile => {
            "SELECT name,version,FALSE immutable FROM sandbox_profiles WHERE tenant_id=? AND id=?"
        }
        EntityType::Dataset => {
            "SELECT name,version,FALSE immutable FROM datasets WHERE tenant_id=? AND id=?"
        }
        EntityType::EvaluationProfile => {
            "SELECT name,version,FALSE immutable FROM evaluation_profiles WHERE tenant_id=? AND id=?"
        }
        EntityType::Department => {
            "SELECT name,version,is_root immutable FROM departments WHERE tenant_id=? AND id=?"
        }
        EntityType::Role => {
            "SELECT name,version,is_builtin immutable FROM roles WHERE tenant_id=? AND id=?"
        }
        EntityType::ApplicationWebhook => {
            "SELECT name,version,FALSE immutable FROM application_webhooks WHERE tenant_id=? AND id=?"
        }
        EntityType::ApplicationSchedule => {
            "SELECT name,version,FALSE immutable FROM application_schedules WHERE tenant_id=? AND id=?"
        }
    };
    let row = sqlx::query(sql)
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found("Deletion target"))?;
    Ok(Target {
        name: row.try_get("name")?,
        version: row.try_get("version")?,
        immutable: row.try_get("immutable")?,
    })
}

async fn control_references(
    pool: &MySqlPool,
    tenant_id: Uuid,
    entity: EntityType,
    id: Uuid,
) -> ApiResult<Vec<DeletionReference>> {
    let mut output = Vec::new();
    if let Some(resource_type) = entity.resource_type() {
        push_resource_references(pool, tenant_id, id, resource_type, entity, &mut output).await?;
    }
    for sql in direct_reference_queries(entity) {
        push_rows(pool, tenant_id, id, sql, &mut output).await?;
    }
    Ok(output)
}

async fn push_resource_references(
    pool: &MySqlPool,
    tenant_id: Uuid,
    id: Uuid,
    resource_type: &str,
    entity: EntityType,
    output: &mut Vec<DeletionReference>,
) -> ApiResult<()> {
    let include_mcp_tools = entity == EntityType::McpServer;
    for sql in RESOURCE_REFERENCE_QUERIES {
        let rows = sqlx::query(sql)
            .bind(tenant_id)
            .bind(resource_type)
            .bind(id)
            .bind(include_mcp_tools)
            .bind(id)
            .fetch_all(pool)
            .await?;
        push_reference_rows(rows, output)?;
    }
    Ok(())
}

const RESOURCE_REFERENCE_QUERIES: &[&str] = &[
    "SELECT 'workflows' source_module,'workflow_draft_revision' source_type,r.id source_id,CONCAT(w.name,' / ',CONCAT('v',v.version_number),' r',r.revision) source_name,'revision_resource' relation,w.id parent_id,j.node_id,NULL node_name,TRUE immutable FROM workflow_draft_revisions r JOIN workflows w ON w.tenant_id=r.tenant_id AND w.id=r.workflow_id JOIN workflow_drafts d ON d.tenant_id=r.tenant_id AND d.id=r.draft_id JOIN JSON_TABLE(r.resource_snapshot_json,'$[*]' COLUMNS(node_id VARCHAR(128) PATH '$.nodeId',resource_type VARCHAR(32) PATH '$.reference.resourceType',resource_id VARCHAR(36) PATH '$.reference.resourceId')) j JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.working_draft_id=d.id WHERE r.tenant_id=? AND ((j.resource_type=? AND j.resource_id=BIN_TO_UUID(?)) OR (? AND j.resource_type='mcp_tool' AND j.resource_id IN (SELECT BIN_TO_UUID(id) FROM mcp_tools WHERE tenant_id=r.tenant_id AND server_id=?)))",
    "SELECT 'workflows' source_module,'workflow_draft' source_type,d.id source_id,CONCAT(w.name,' / ',CONCAT('v',v.version_number)) source_name,'draft_resource' relation,w.id parent_id,r.node_id,r.node_name,FALSE immutable FROM workflow_draft_resources r JOIN workflows w ON w.tenant_id=r.tenant_id AND w.id=r.workflow_id JOIN workflow_drafts d ON d.tenant_id=r.tenant_id AND d.id=r.draft_id JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.working_draft_id=d.id WHERE r.tenant_id=? AND ((r.resource_type=? AND r.resource_id=?) OR (? AND r.resource_type='mcp_tool' AND r.resource_id IN (SELECT id FROM mcp_tools WHERE tenant_id=r.tenant_id AND server_id=?)))",
    "SELECT 'workflows' source_module,'workflow_version' source_type,v.id source_id,CONCAT(w.name,' v',v.version_number) source_name,'version_resource' relation,w.id parent_id,r.node_id,NULL node_name,TRUE immutable FROM workflow_version_resources r JOIN workflow_versions v ON v.tenant_id=r.tenant_id AND v.id=r.workflow_version_id JOIN workflows w ON w.tenant_id=v.tenant_id AND w.id=v.workflow_id WHERE r.tenant_id=? AND ((r.resource_type=? AND r.resource_id=?) OR (? AND r.resource_type='mcp_tool' AND r.resource_id IN (SELECT id FROM mcp_tools WHERE tenant_id=r.tenant_id AND server_id=?)))",
    "SELECT 'skills' source_module,'skill_version' source_type,v.id source_id,CONCAT(s.name,' v',v.version_number) source_name,'skill_dependency' relation,s.id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM skill_dependencies r JOIN skill_versions v ON v.tenant_id=r.tenant_id AND v.id=r.skill_version_id JOIN skills s ON s.tenant_id=v.tenant_id AND s.id=v.skill_id WHERE r.tenant_id=? AND ((r.resource_type=? AND r.resource_id=?) OR (? AND r.resource_type='mcp_tool' AND r.resource_id IN (SELECT id FROM mcp_tools WHERE tenant_id=r.tenant_id AND server_id=?)))",
    "SELECT 'resourceGrants' source_module,'resource_grant' source_type,r.id source_id,COALESCE(d.name,w.name,r.subject_type) source_name,'resource_grant' relation,r.subject_id parent_id,NULL node_id,NULL node_name,FALSE immutable FROM resource_grants r LEFT JOIN departments d ON r.subject_type='department' AND d.tenant_id=r.tenant_id AND d.id=r.subject_id LEFT JOIN workflow_service_identities i ON r.subject_type='workflow_service_identity' AND i.tenant_id=r.tenant_id AND i.id=r.subject_id LEFT JOIN workflows w ON w.tenant_id=i.tenant_id AND w.id=i.workflow_id WHERE r.tenant_id=? AND ((r.resource_type=? AND r.resource_id=?) OR (? AND r.resource_type='mcp_tool' AND r.resource_id IN (SELECT id FROM mcp_tools WHERE tenant_id=r.tenant_id AND server_id=?)))",
];

fn direct_reference_queries(entity: EntityType) -> &'static [&'static str] {
    match entity {
        EntityType::Workflow => &[
            "SELECT 'applications' source_module,'application' source_type,a.id source_id,a.name source_name,'workflow' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM applications a WHERE a.tenant_id=? AND a.workflow_id=?",
            "SELECT 'applications' source_module,'evaluation_run' source_type,e.batch_id source_id,e.name source_name,'workflow_version' relation,e.application_id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM evaluation_runs e JOIN workflow_versions v ON v.tenant_id=e.tenant_id AND v.id=e.workflow_version_id WHERE e.tenant_id=? AND v.workflow_id=?",
            "SELECT 'workflows' source_module,'workflow_draft' source_type,w.id source_id,w.name source_name,'subworkflow_draft' relation,NULL parent_id,r.node_id,r.node_name,FALSE immutable FROM workflow_draft_resources r JOIN workflows w ON w.tenant_id=r.tenant_id AND w.id=r.workflow_id WHERE r.tenant_id=? AND r.resource_type='workflow' AND r.resource_id=?",
            "SELECT 'workflows' source_module,'workflow_version' source_type,v.id source_id,CONCAT(w.name,' v',v.version_number) source_name,'subworkflow_version' relation,w.id parent_id,r.node_id,NULL node_name,TRUE immutable FROM workflow_version_resources r JOIN workflow_versions v ON v.tenant_id=r.tenant_id AND v.id=r.workflow_version_id JOIN workflows w ON w.tenant_id=v.tenant_id AND w.id=v.workflow_id WHERE r.tenant_id=? AND r.resource_type='workflow' AND r.resource_id=?",
        ],
        EntityType::Environment => &[
            "SELECT 'workflows' source_module,'workflow_deployment' source_type,d.id source_id,CONCAT(w.name,' #',d.sequence_number) source_name,'environment' relation,w.id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM workflow_deployments d JOIN workflows w ON w.tenant_id=d.tenant_id AND w.id=d.workflow_id WHERE d.tenant_id=? AND d.environment_id=?",
            "SELECT 'applications' source_module,'application_deployment' source_type,d.id source_id,CONCAT(a.name,' #',d.sequence_number) source_name,'environment' relation,a.id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM application_deployments d JOIN applications a ON a.tenant_id=d.tenant_id AND a.id=d.application_id WHERE d.tenant_id=? AND d.environment_id=?",
        ],
        EntityType::Application => &[
            "SELECT 'applications' source_module,'dataset' source_type,d.id source_id,d.name source_name,'application' relation,d.application_id parent_id,NULL node_id,NULL node_name,FALSE immutable FROM datasets d WHERE d.tenant_id=? AND d.application_id=?",
            "SELECT 'applications' source_module,'evaluation_profile' source_type,p.id source_id,p.name source_name,'application' relation,p.application_id parent_id,NULL node_id,NULL node_name,FALSE immutable FROM evaluation_profiles p WHERE p.tenant_id=? AND p.application_id=?",
            "SELECT 'applications' source_module,'evaluation_run' source_type,b.id source_id,b.name source_name,'evaluation_batch' relation,b.application_id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM evaluation_batches b WHERE b.tenant_id=? AND b.application_id=?",
        ],
        EntityType::Model => &[
            "SELECT 'applications' source_module,'evaluation_run' source_type,b.id source_id,b.name source_name,'platform_evaluation_model' relation,b.application_id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM evaluation_batches b WHERE b.tenant_id=? AND JSON_UNQUOTE(JSON_EXTRACT(b.model_snapshot_json,'$.binding.resourceId'))=BIN_TO_UUID(?)",
            "SELECT 'models' source_module,'platform_ai_model' source_type,s.tenant_id source_id,'Platform AI model setting' source_name,'studio_builder_model' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM tenant_settings s WHERE s.tenant_id=? AND s.platform_ai_model_alias_id=?",
            "SELECT 'workflows' source_module,'studio_builder_run' source_type,r.id source_id,CONCAT(w.name,' / ',CONCAT('v',v.version_number)) source_name,'studio_builder_model' relation,w.id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM studio_builder_runs r JOIN studio_builder_sessions s ON s.tenant_id=r.tenant_id AND s.id=r.session_id JOIN workflows w ON w.tenant_id=s.tenant_id AND w.id=s.workflow_id JOIN workflow_drafts d ON d.tenant_id=s.tenant_id AND d.id=s.draft_id JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.working_draft_id=d.id WHERE r.tenant_id=? AND r.model_alias_id=? AND r.status IN ('queued','running','waiting','outcome_unknown')",
        ],
        EntityType::Credential => &[
            "SELECT 'applications' source_module,'evaluation_run' source_type,b.id source_id,b.name source_name,'platform_evaluation_credential' relation,b.application_id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM evaluation_batches b WHERE b.tenant_id=? AND JSON_UNQUOTE(JSON_EXTRACT(b.model_snapshot_json,'$.credentialId'))=BIN_TO_UUID(?)",
            "SELECT 'models' source_module,'platform_ai_model' source_type,s.tenant_id source_id,'Platform AI model setting' source_name,'studio_builder_credential' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM tenant_settings s WHERE s.tenant_id=? AND s.platform_ai_credential_id=?",
            "SELECT 'workflows' source_module,'studio_builder_run' source_type,r.id source_id,CONCAT(w.name,' / ',CONCAT('v',v.version_number)) source_name,'studio_builder_credential' relation,w.id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM studio_builder_runs r JOIN studio_builder_sessions s ON s.tenant_id=r.tenant_id AND s.id=r.session_id JOIN workflows w ON w.tenant_id=s.tenant_id AND w.id=s.workflow_id JOIN workflow_drafts d ON d.tenant_id=s.tenant_id AND d.id=s.draft_id JOIN workflow_versions v ON v.tenant_id=d.tenant_id AND v.working_draft_id=d.id WHERE r.tenant_id=? AND r.credential_id=? AND r.status IN ('queued','running','waiting','outcome_unknown')",
            "SELECT 'models' source_module,'model_deployment' source_type,d.id source_id,CONCAT(d.connection_name,' / ',d.model_name) source_name,'credential' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM model_deployments d WHERE d.tenant_id=? AND d.credential_id=?",
            "SELECT 'mcp' source_module,'mcp_server_version' source_type,v.id source_id,CONCAT(s.name,' v',v.version_number) source_name,'credential' relation,s.id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM mcp_server_versions v JOIN mcp_servers s ON s.tenant_id=v.tenant_id AND s.id=v.server_id WHERE v.tenant_id=? AND v.credential_id=?",
            "SELECT 'knowledge' source_module,'knowledge_connection' source_type,c.id source_id,c.name source_name,'credential' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM rag_connections c WHERE c.tenant_id=? AND c.credential_id=?",
            "SELECT 'memory' source_module,'memory_connection' source_type,c.id source_id,c.name source_name,'credential' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM memory_connections c WHERE c.tenant_id=? AND c.credential_id=?",
        ],
        EntityType::Dataset => &[
            "SELECT 'applications' source_module,'evaluation_run' source_type,e.batch_id source_id,e.name source_name,'dataset_version' relation,e.application_id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM evaluation_runs e JOIN dataset_versions v ON v.tenant_id=e.tenant_id AND v.id=e.dataset_version_id WHERE e.tenant_id=? AND v.dataset_id=?",
        ],
        EntityType::EvaluationProfile => &[
            "SELECT 'applications' source_module,'evaluation_run' source_type,e.batch_id source_id,e.name source_name,'evaluation_profile_version' relation,e.application_id parent_id,NULL node_id,NULL node_name,TRUE immutable FROM evaluation_runs e JOIN evaluation_profile_versions v ON v.tenant_id=e.tenant_id AND v.id=e.evaluation_profile_version_id WHERE e.tenant_id=? AND v.profile_id=?",
        ],
        EntityType::Department => &[
            "SELECT 'organization' source_module,'department' source_type,d.id source_id,d.name source_name,'parent_department' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM departments d WHERE d.tenant_id=? AND d.parent_id=?",
            "SELECT 'organization' source_module,'user' source_type,u.id source_id,u.display_name source_name,'department_membership' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM user_departments ud JOIN users u ON u.tenant_id=ud.tenant_id AND u.id=ud.user_id WHERE ud.tenant_id=? AND ud.department_id=?",
            "SELECT 'workflows' source_module,'workflow' source_type,w.id source_id,w.name source_name,'owner_department' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM workflows w WHERE w.tenant_id=? AND w.owner_department_id=?",
            "SELECT 'applications' source_module,'application' source_type,a.id source_id,a.name source_name,'owner_department' relation,NULL parent_id,NULL node_id,NULL node_name,FALSE immutable FROM applications a WHERE a.tenant_id=? AND a.owner_department_id=?",
        ],
        EntityType::Role => &[
            "SELECT 'organization' source_module,'user_role' source_type,ur.id source_id,u.display_name source_name,'role_assignment' relation,u.id parent_id,NULL node_id,NULL node_name,FALSE immutable FROM user_roles ur JOIN users u ON u.tenant_id=ur.tenant_id AND u.id=ur.user_id WHERE ur.tenant_id=? AND ur.role_id=?",
        ],
        _ => &[],
    }
}

async fn push_rows(
    pool: &MySqlPool,
    tenant_id: Uuid,
    id: Uuid,
    sql: &str,
    output: &mut Vec<DeletionReference>,
) -> ApiResult<()> {
    let placeholders = sql.as_bytes().iter().filter(|byte| **byte == b'?').count();
    let mut query = sqlx::query(sql).bind(tenant_id).bind(id);
    if placeholders == 3 {
        query = query.bind(id);
    }
    push_reference_rows(query.fetch_all(pool).await?, output)
}

fn push_reference_rows(
    rows: Vec<sqlx::mysql::MySqlRow>,
    output: &mut Vec<DeletionReference>,
) -> ApiResult<()> {
    for row in rows {
        output.push(DeletionReference {
            source_module: row.try_get("source_module")?,
            source_type: row.try_get("source_type")?,
            source_id: row.try_get("source_id")?,
            source_name: row.try_get("source_name")?,
            relation: row.try_get("relation")?,
            parent_id: row.try_get("parent_id")?,
            node_id: row.try_get("node_id")?,
            node_name: row.try_get("node_name")?,
            immutable: row.try_get("immutable")?,
        });
    }
    Ok(())
}

async fn runtime_application_references(
    state: &ControlApiState,
    actor: &Actor,
    application_id: Uuid,
) -> ApiResult<Vec<DeletionReference>> {
    let mut references = Vec::new();
    let mut after = None;
    loop {
        let request = SessionSearchRequestV1 {
            user_source: None,
            external_user_id: None,
            api_version: 1,
            tenant_id: actor.tenant_id,
            application_id: Some(application_id),
            statuses: Vec::new(),
            after,
            limit: 100,
        };
        let hash = content_hash(&json!({"operation":"session-search","request":request}))
            .map_err(ApiError::internal)?;
        let token =
            runtime_delegation(state, actor, "runtime.query.sessions", application_id, hash)?;
        let response = state
            .http
            .post(format!(
                "{}/internal/runtime/v1/query/sessions:search",
                state.runtime_query_url
            ))
            .bearer_auth(token)
            .json(&request)
            .send()
            .await
            .map_err(runtime_unavailable)?;
        let page: SessionSearchPageV1 = runtime_json(response).await?;
        references.extend(page.items.into_iter().map(|item| {
            DeletionReference {
                source_module: "applications".into(),
                source_type: "session".into(),
                source_id: item.session_id,
                source_name: item
                    .title
                    .unwrap_or_else(|| format!("Session {}", item.session_id)),
                relation: "application".into(),
                parent_id: Some(application_id),
                node_id: None,
                node_name: None,
                immutable: true,
            }
        }));
        after = page.next;
        if after.is_none() {
            break;
        }
    }

    let mut cursor = None;
    loop {
        let request = InvocationSearchRequestV1 {
            user_source: None,
            external_user_id: None,
            api_version: 1,
            tenant_id: actor.tenant_id,
            application_ids: vec![application_id],
            statuses: Vec::new(),
            created_after: None,
            created_before: None,
            cursor,
            limit: 100,
        };
        let hash = content_hash(&json!({"operation":"invocation-search","request":request}))
            .map_err(ApiError::internal)?;
        let token = runtime_delegation(
            state,
            actor,
            "runtime.query.invocations",
            application_id,
            hash,
        )?;
        let response = state
            .http
            .post(format!(
                "{}/internal/runtime/v1/query/invocations:search",
                state.runtime_query_url
            ))
            .bearer_auth(token)
            .json(&request)
            .send()
            .await
            .map_err(runtime_unavailable)?;
        let page: InvocationSearchPageV1 = runtime_json(response).await?;
        references.extend(page.items.into_iter().map(|item| DeletionReference {
            source_module: "applications".into(),
            source_type: "invocation".into(),
            source_id: item.invocation_id,
            source_name: format!("{} invocation", item.caller_type),
            relation: "application".into(),
            parent_id: item.session_id.or(Some(application_id)),
            node_id: None,
            node_name: None,
            immutable: true,
        }));
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    Ok(references)
}

fn runtime_delegation(
    state: &ControlApiState,
    actor: &Actor,
    scope: &str,
    application_id: Uuid,
    request_hash: ContentHash,
) -> ApiResult<String> {
    let now = now_unix();
    issue_delegation_token(
        &state.delegation_kid,
        state.delegation_key.expose_secret().as_bytes(),
        &DelegationClaimsV1 {
            iss: "agentx-control".into(),
            aud: "agentx-runtime-internal".into(),
            sub: actor.user_id,
            tenant_id: actor.tenant_id,
            token_version: actor.token_version,
            tenant_wide: false,
            scope: BTreeSet::from([scope.to_owned()]),
            application_ids: BTreeSet::from([application_id]),
            workflow_ids: BTreeSet::new(),
            execution_ids: BTreeSet::new(),
            session_ids: BTreeSet::new(),
            request_hash,
            iat: now,
            exp: now + DELEGATION_TOKEN_TTL_SECONDS,
            jti: Uuid::now_v7(),
        },
    )
    .map_err(ApiError::internal)
}

fn runtime_unavailable(error: reqwest::Error) -> ApiError {
    tracing::warn!(%error, "Runtime reference query is unavailable");
    ApiError::unavailable(
        "RUNTIME_REFERENCE_CHECK_UNAVAILABLE",
        "Runtime references could not be checked",
    )
}

async fn runtime_json<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> ApiResult<T> {
    if !response.status().is_success() {
        tracing::warn!(status=%response.status(), "Runtime rejected reference query");
        return Err(ApiError::unavailable(
            "RUNTIME_REFERENCE_CHECK_UNAVAILABLE",
            "Runtime references could not be checked",
        ));
    }
    response.json().await.map_err(ApiError::internal)
}

#[cfg(test)]
mod tests {
    use super::EntityType;
    use std::str::FromStr;

    #[test]
    fn supports_all_publicly_deletable_entity_types() {
        for value in [
            "workflow",
            "environment",
            "application",
            "credential",
            "model",
            "mcp_server",
            "skill",
            "knowledge",
            "memory",
            "sandbox_profile",
            "dataset",
            "evaluation_profile",
            "department",
            "role",
            "application_webhook",
            "application_schedule",
        ] {
            assert!(EntityType::from_str(value).is_ok(), "{value}");
        }
        assert!(EntityType::from_str("execution").is_err());
    }
}
