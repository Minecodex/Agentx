use std::io::{Cursor, Read, Write};

use agentx_api_types::PageResponse;
use agentx_application::{ArtifactStore, ArtifactWrite};
use agentx_control_infrastructure::artifact::MySqlControlArtifactStore;
use agentx_domain::{ArtifactId, TenantId};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{HeaderValue, Response, StatusCode, header},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{MySql, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::{Actor, ControlApiState},
    control_helpers::required_name,
};

const MAX_FILE_SIZE: usize = 20 * 1024 * 1024;
const MAX_WORKSPACE_SIZE: usize = 100 * 1024 * 1024;
const MAX_ENTRIES: usize = 1000;
const MAX_DEPTH: usize = 20;

pub fn routes() -> Router<ControlApiState> {
    Router::new()
        .route("/api/v1/skills", get(list_skills).post(create_skill))
        .route(
            "/api/v1/skills/{id}",
            get(get_skill).patch(update_skill).delete(delete_skill),
        )
        .route("/api/v1/skills/{id}/workspace", get(get_workspace))
        .route(
            "/api/v1/skills/{id}/workspace/export",
            get(export_workspace),
        )
        .route(
            "/api/v1/skills/{id}/workspace/import",
            post(import_workspace),
        )
        .route("/api/v1/skills/{id}/entries", post(create_entry))
        .route(
            "/api/v1/skills/{id}/entries/{entry_id}",
            axum::routing::patch(move_entry).delete(delete_entry),
        )
        .route(
            "/api/v1/skills/{id}/files/{entry_id}",
            get(get_file).put(update_markdown),
        )
        .route("/api/v1/skills/{id}/uploads", post(upload_file))
        .route(
            "/api/v1/skills/{id}/versions",
            get(list_versions).post(create_version),
        )
        .layer(DefaultBodyLimit::max(MAX_FILE_SIZE + 1024 * 1024))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    search: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillResponse {
    id: Uuid,
    name: String,
    alias: String,
    description: Option<String>,
    owner_department_id: Uuid,
    status: String,
    draft_revision: u64,
    latest_version: Option<u64>,
    grant_count: u64,
    version: u64,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateSkillRequest {
    name: String,
    alias: String,
    description: String,
    owner_department_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateSkillRequest {
    name: String,
    alias: String,
    description: Option<String>,
    status: String,
    version: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceEntry {
    id: Uuid,
    parent_id: Option<Uuid>,
    name: String,
    path: String,
    entry_type: String,
    mime_type: Option<String>,
    artifact_id: Option<Uuid>,
    content_hash: Option<String>,
    size_bytes: u64,
    editable: bool,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceResponse {
    revision: u64,
    entries: Vec<WorkspaceEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateEntryRequest {
    parent_id: Option<Uuid>,
    name: String,
    entry_type: String,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MoveEntryRequest {
    parent_id: Option<Uuid>,
    name: String,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateMarkdownRequest {
    content: String,
    description: Option<String>,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteQuery {
    expected_version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteEntryQuery {
    expected_revision: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileResponse {
    entry_id: Uuid,
    content: String,
    content_hash: String,
    revision: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DependencyInput {
    resource_type: String,
    resource_id: Uuid,
    resource_version_id: Option<Uuid>,
    operation: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishRequest {
    expected_revision: u64,
    #[serde(default)]
    dependencies: Vec<DependencyInput>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionResponse {
    id: Uuid,
    skill_id: Uuid,
    version_number: u64,
    source_revision: u64,
    manifest: Value,
    content_hash: String,
    file_count: u64,
    dependencies: Vec<DependencyInput>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadResponse {
    entry: WorkspaceEntry,
    revision: u64,
}

#[derive(Deserialize, Serialize)]
struct SkillMetadata {
    name: String,
    description: String,
}

struct StoredArtifact {
    id: Uuid,
    bytes: Vec<u8>,
    hash: String,
}

async fn list_skills(
    State(state): State<ControlApiState>,
    actor: Actor,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<PageResponse<SkillResponse>>> {
    actor.require("skill:view")?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let search = format!("%{}%", query.search.unwrap_or_default().trim());
    let status = query.status.unwrap_or_default();
    let rows = sqlx::query("SELECT s.id,s.name,s.alias,s.description,s.owner_department_id,s.status,s.draft_revision,s.version,s.updated_at,(SELECT MAX(version_number) FROM skill_versions v WHERE v.tenant_id=s.tenant_id AND v.skill_id=s.id) latest_version,(SELECT COUNT(*) FROM resource_grants g WHERE g.tenant_id=s.tenant_id AND g.resource_type='skill' AND g.resource_id=s.id) grant_count,COUNT(*) OVER() total_count FROM skills s WHERE s.tenant_id=? AND (?='' OR s.status=?) AND (?='%%' OR s.name LIKE ? OR s.alias LIKE ?) ORDER BY s.updated_at DESC,s.id DESC LIMIT ? OFFSET ?")
        .bind(actor.tenant_id).bind(&status).bind(&status).bind(&search).bind(&search).bind(&search)
        .bind(page_size).bind(u64::from((page - 1) * page_size)).fetch_all(&state.pool).await?;
    let total = rows
        .first()
        .map_or(Ok(0_i64), |row| row.try_get("total_count"))? as u64;
    let items = rows
        .into_iter()
        .map(skill_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(PageResponse {
        items,
        page,
        page_size,
        total,
    }))
}

async fn create_skill(
    State(state): State<ControlApiState>,
    actor: Actor,
    Json(input): Json<CreateSkillRequest>,
) -> ApiResult<(StatusCode, Json<SkillResponse>)> {
    actor.require("skill:manage")?;
    let name = required_name(&input.name)?;
    let alias = validate_alias(&input.alias)?;
    let description = validate_description(Some(&input.description))?;
    require_department(&state, &actor, input.owner_department_id).await?;
    ensure_identity(&state, actor.tenant_id, &name, &alias, None).await?;
    let content = render_skill_document(&name, &description, &format!("# {name}\n"))?;
    let artifact = put_artifact(
        &state,
        actor.tenant_id,
        "text/markdown; charset=utf-8",
        content.into_bytes(),
    )
    .await?;
    let id = Uuid::now_v7();
    let entry_id = Uuid::now_v7();
    let result: ApiResult<()> = async {
        let mut tx = state.pool.begin().await?;
        sqlx::query("INSERT INTO skills(id,tenant_id,name,alias,description,owner_department_id,created_by) VALUES(?,?,?,?,?,?,?)")
            .bind(id).bind(actor.tenant_id).bind(&name).bind(&alias).bind(&description).bind(input.owner_department_id).bind(actor.user_id).execute(&mut *tx).await.map_err(map_unique)?;
        sqlx::query("INSERT INTO skill_workspace_entries(id,tenant_id,skill_id,name,path,path_hash,entry_type,mime_type,artifact_id,content_hash,size_bytes,editable) VALUES(?,?,?,'SKILL.md','SKILL.md',?,'file','text/markdown; charset=utf-8',?,?,?,TRUE)")
            .bind(entry_id).bind(actor.tenant_id).bind(id).bind(path_hash("SKILL.md")).bind(artifact.id).bind(&artifact.hash).bind(artifact.bytes.len() as u64).execute(&mut *tx).await?;
        insert_revision(&mut tx, &actor, id, entry_id, 1, &artifact).await?;
        tx.commit().await?;
        Ok(())
    }.await;
    if let Err(error) = result {
        delete_artifact(&state, actor.tenant_id, artifact.id).await;
        return Err(error);
    }
    Ok((
        StatusCode::CREATED,
        Json(load_skill(&state, actor.tenant_id, id).await?),
    ))
}

async fn get_skill(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SkillResponse>> {
    actor.require("skill:view")?;
    Ok(Json(load_skill(&state, actor.tenant_id, id).await?))
}

async fn update_skill(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateSkillRequest>,
) -> ApiResult<Json<SkillResponse>> {
    actor.require("skill:manage")?;
    let name = required_name(&input.name)?;
    let alias = validate_alias(&input.alias)?;
    let description = validate_description(input.description.as_deref())?;
    if !matches!(input.status.as_str(), "draft" | "active" | "disabled") {
        return Err(ApiError::bad_request(
            "INVALID_STATUS",
            "Skill status is invalid",
        ));
    }
    ensure_identity(&state, actor.tenant_id, &name, &alias, Some(id)).await?;
    if input.status == "active" {
        let published: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM skill_versions WHERE tenant_id=? AND skill_id=?)",
        )
        .bind(actor.tenant_id)
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
        if !published {
            return Err(ApiError::unprocessable(
                "SKILL_VERSION_REQUIRED",
                "Publish a Skill version before activation",
            ));
        }
    }
    let result = sqlx::query("UPDATE skills SET name=?,alias=?,description=?,status=?,version=version+1 WHERE tenant_id=? AND id=? AND version=?")
        .bind(name).bind(alias).bind(description).bind(input.status).bind(actor.tenant_id).bind(id).bind(input.version).execute(&state.pool).await.map_err(map_unique)?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "SKILL_VERSION_CONFLICT",
            "Skill changed",
        ));
    }
    Ok(Json(load_skill(&state, actor.tenant_id, id).await?))
}

async fn delete_skill(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    actor.require("skill:manage")?;
    let referenced: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_grants WHERE tenant_id=? AND resource_type='skill' AND resource_id=? UNION ALL SELECT 1 FROM skill_dependencies WHERE tenant_id=? AND resource_type='skill' AND resource_id=?)")
        .bind(actor.tenant_id).bind(id).bind(actor.tenant_id).bind(id).fetch_one(&state.pool).await?;
    if referenced {
        return Err(ApiError::conflict(
            "RESOURCE_REFERENCED",
            "Skill is referenced and cannot be deleted",
        ));
    }
    let artifacts = sqlx::query_scalar::<_, Uuid>("SELECT DISTINCT artifact_id FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=? AND artifact_id IS NOT NULL")
        .bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    let mut tx = state.pool.begin().await?;
    let deleted = sqlx::query("DELETE FROM skills WHERE tenant_id=? AND id=? AND version=?")
        .bind(actor.tenant_id)
        .bind(id)
        .bind(query.expected_version)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "SKILL_VERSION_CONFLICT",
            "Skill changed or was not found",
        ));
    }
    sqlx::query("DELETE FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    for artifact in artifacts {
        delete_artifact(&state, actor.tenant_id, artifact).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn get_workspace(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<WorkspaceResponse>> {
    actor.require("skill:view")?;
    Ok(Json(load_workspace(&state, actor.tenant_id, id).await?))
}

async fn create_entry(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<CreateEntryRequest>,
) -> ApiResult<(StatusCode, Json<WorkspaceResponse>)> {
    actor.require("skill:manage")?;
    let name = validate_entry_name(&input.name, &input.entry_type)?;
    let parent = parent_path(&state, actor.tenant_id, id, input.parent_id).await?;
    let path = join_path(&parent, &name)?;
    ensure_capacity(&state, actor.tenant_id, id, 1, 0).await?;
    let artifact = if input.entry_type == "file" {
        if !name.to_ascii_lowercase().ends_with(".md") {
            return Err(ApiError::bad_request(
                "SKILL_MARKDOWN_REQUIRED",
                "Online-created files must use the .md extension",
            ));
        }
        Some(
            put_artifact(
                &state,
                actor.tenant_id,
                "text/markdown; charset=utf-8",
                Vec::new(),
            )
            .await?,
        )
    } else {
        None
    };
    let mut tx = state.pool.begin().await?;
    let revision = bump_revision(&mut tx, actor.tenant_id, id, input.expected_revision).await?;
    let result = sqlx::query("INSERT INTO skill_workspace_entries(id,tenant_id,skill_id,parent_id,name,path,path_hash,entry_type,mime_type,artifact_id,content_hash,size_bytes,editable) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(id).bind(input.parent_id).bind(name).bind(&path).bind(path_hash(&path)).bind(&input.entry_type)
        .bind(artifact.as_ref().map(|_| "text/markdown; charset=utf-8")).bind(artifact.as_ref().map(|a| a.id)).bind(artifact.as_ref().map(|a| a.hash.as_str())).bind(0_u64).bind(input.entry_type == "file").execute(&mut *tx).await;
    if let Err(error) = result {
        tx.rollback().await?;
        if let Some(artifact) = artifact {
            delete_artifact(&state, actor.tenant_id, artifact.id).await;
        }
        return Err(map_unique(error));
    }
    if let Some(artifact) = &artifact {
        let entry_id: Uuid = sqlx::query_scalar("SELECT id FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=? AND path_hash=?").bind(actor.tenant_id).bind(id).bind(path_hash(&path)).fetch_one(&mut *tx).await?;
        insert_revision(&mut tx, &actor, id, entry_id, revision, artifact).await?;
    }
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load_workspace(&state, actor.tenant_id, id).await?),
    ))
}

async fn move_entry(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, entry_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<MoveEntryRequest>,
) -> ApiResult<Json<WorkspaceResponse>> {
    actor.require("skill:manage")?;
    let row = entry_row(&state, actor.tenant_id, id, entry_id).await?;
    let old: String = row.try_get("path")?;
    if old == "SKILL.md" {
        return Err(ApiError::bad_request(
            "SKILL_ROOT_IMMUTABLE",
            "SKILL.md cannot be moved",
        ));
    }
    let kind: String = row.try_get("entry_type")?;
    let name = validate_entry_name(&input.name, &kind)?;
    let parent = parent_path(&state, actor.tenant_id, id, input.parent_id).await?;
    let new = join_path(&parent, &name)?;
    if kind == "directory" && (new == old || new.starts_with(&format!("{old}/"))) {
        return Err(ApiError::bad_request(
            "SKILL_DIRECTORY_CYCLE",
            "Directory cannot be moved into itself",
        ));
    }
    let mut tx = state.pool.begin().await?;
    bump_revision(&mut tx, actor.tenant_id, id, input.expected_revision).await?;
    sqlx::query("UPDATE skill_workspace_entries SET parent_id=?,name=?,path=?,path_hash=? WHERE tenant_id=? AND skill_id=? AND id=?")
        .bind(input.parent_id).bind(name).bind(&new).bind(path_hash(&new)).bind(actor.tenant_id).bind(id).bind(entry_id).execute(&mut *tx).await.map_err(map_unique)?;
    if kind == "directory" {
        sqlx::query("UPDATE skill_workspace_entries SET path_hash=SHA2(CONCAT(?,SUBSTRING(path,?)),256),path=CONCAT(?,SUBSTRING(path,?)) WHERE tenant_id=? AND skill_id=? AND path LIKE ?")
            .bind(&new).bind((old.len()+1) as u64).bind(&new).bind((old.len()+1) as u64).bind(actor.tenant_id).bind(id).bind(format!("{old}/%")).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(load_workspace(&state, actor.tenant_id, id).await?))
}

async fn delete_entry(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, entry_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<DeleteEntryQuery>,
) -> ApiResult<StatusCode> {
    actor.require("skill:manage")?;
    let row = entry_row(&state, actor.tenant_id, id, entry_id).await?;
    let path: String = row.try_get("path")?;
    if path == "SKILL.md" {
        return Err(ApiError::bad_request(
            "SKILL_ROOT_IMMUTABLE",
            "SKILL.md cannot be deleted",
        ));
    }
    let children: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=? AND parent_id=?").bind(actor.tenant_id).bind(id).bind(entry_id).fetch_one(&state.pool).await?;
    if children != 0 {
        return Err(ApiError::unprocessable(
            "SKILL_DIRECTORY_NOT_EMPTY",
            "Directory is not empty",
        ));
    }
    let artifact: Option<Uuid> = row.try_get("artifact_id")?;
    let mut tx = state.pool.begin().await?;
    bump_revision(&mut tx, actor.tenant_id, id, query.expected_revision).await?;
    sqlx::query("DELETE FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=? AND id=?")
        .bind(actor.tenant_id)
        .bind(id)
        .bind(entry_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if let Some(artifact) = artifact {
        delete_artifact(&state, actor.tenant_id, artifact).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn get_file(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, entry_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Response<Body>> {
    actor.require("skill:view")?;
    let row = entry_row(&state, actor.tenant_id, id, entry_id).await?;
    let artifact: Option<Uuid> = row.try_get("artifact_id")?;
    let value = get_artifact(
        &state,
        actor.tenant_id,
        artifact
            .ok_or_else(|| ApiError::bad_request("SKILL_ENTRY_NOT_FILE", "Entry is not a file"))?,
    )
    .await?;
    let mut response = Response::new(Body::from(value.bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(
            row.try_get::<Option<String>, _>("mime_type")?
                .as_deref()
                .unwrap_or("application/octet-stream"),
        )
        .map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'",
        ),
    );
    Ok(response)
}

async fn update_markdown(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((id, entry_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<UpdateMarkdownRequest>,
) -> ApiResult<Json<FileResponse>> {
    actor.require("skill:manage")?;
    let row = entry_row(&state, actor.tenant_id, id, entry_id).await?;
    if !row.try_get::<bool, _>("editable")? {
        return Err(ApiError::bad_request(
            "SKILL_FILE_READ_ONLY",
            "Only Markdown files can be edited",
        ));
    }
    let path: String = row.try_get("path")?;
    let root_description = if path == "SKILL.md" {
        let skill = load_skill(&state, actor.tenant_id, id).await?;
        Some(validate_description(
            input
                .description
                .as_deref()
                .or(skill.description.as_deref()),
        )?)
    } else {
        None
    };
    let content = if let Some(description) = root_description.as_deref() {
        let skill = load_skill(&state, actor.tenant_id, id).await?;
        render_skill_document(&skill.name, description, strip_frontmatter(&input.content))?
    } else {
        input.content
    };
    if content.len() > MAX_FILE_SIZE {
        return Err(ApiError::bad_request(
            "SKILL_FILE_TOO_LARGE",
            "Skill file exceeds 20 MiB",
        ));
    }
    ensure_capacity(
        &state,
        actor.tenant_id,
        id,
        0,
        content.len() as i64 - row.try_get::<u64, _>("size_bytes")? as i64,
    )
    .await?;
    let artifact = put_artifact(
        &state,
        actor.tenant_id,
        "text/markdown; charset=utf-8",
        content.into_bytes(),
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let revision = bump_revision(&mut tx, actor.tenant_id, id, input.expected_revision).await?;
    sqlx::query("UPDATE skill_workspace_entries SET artifact_id=?,content_hash=?,size_bytes=? WHERE tenant_id=? AND skill_id=? AND id=?")
        .bind(artifact.id).bind(&artifact.hash).bind(artifact.bytes.len() as u64).bind(actor.tenant_id).bind(id).bind(entry_id).execute(&mut *tx).await?;
    if let Some(description) = root_description {
        sqlx::query("UPDATE skills SET description=? WHERE tenant_id=? AND id=?")
            .bind(description)
            .bind(actor.tenant_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    insert_revision(&mut tx, &actor, id, entry_id, revision, &artifact).await?;
    tx.commit().await?;
    Ok(Json(FileResponse {
        entry_id,
        content: String::from_utf8(artifact.bytes).map_err(ApiError::internal)?,
        content_hash: artifact.hash,
        revision,
    }))
}

async fn upload_file(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<UploadResponse>)> {
    actor.require("skill:manage")?;
    let mut parent_id = None;
    let mut expected = None;
    let mut upload = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad_request("INVALID_MULTIPART", "Upload is invalid"))?
    {
        match field.name() {
            Some("parentId") => {
                let value = field.text().await.map_err(ApiError::internal)?;
                if !value.is_empty() {
                    parent_id = Some(Uuid::parse_str(&value).map_err(|_| {
                        ApiError::bad_request("INVALID_PARENT", "Parent is invalid")
                    })?);
                }
            }
            Some("expectedRevision") => {
                expected = Some(
                    field
                        .text()
                        .await
                        .map_err(ApiError::internal)?
                        .parse::<u64>()
                        .map_err(|_| {
                            ApiError::bad_request("INVALID_REVISION", "Revision is invalid")
                        })?,
                )
            }
            Some("file") if upload.is_none() => {
                let name = validate_entry_name(field.file_name().unwrap_or("file"), "file")?;
                let mime = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_owned();
                let bytes = field.bytes().await.map_err(ApiError::internal)?.to_vec();
                upload = Some((name, mime, bytes));
            }
            _ => {}
        }
    }
    let (name, mime, bytes) = upload
        .ok_or_else(|| ApiError::bad_request("FILE_REQUIRED", "Multipart file is required"))?;
    if bytes.len() > MAX_FILE_SIZE {
        return Err(ApiError::bad_request(
            "SKILL_FILE_TOO_LARGE",
            "Skill file exceeds 20 MiB",
        ));
    }
    ensure_capacity(&state, actor.tenant_id, id, 1, bytes.len() as i64).await?;
    let parent = parent_path(&state, actor.tenant_id, id, parent_id).await?;
    let path = join_path(&parent, &name)?;
    let artifact = put_artifact(&state, actor.tenant_id, &mime, bytes).await?;
    let entry_id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    let revision = bump_revision(
        &mut tx,
        actor.tenant_id,
        id,
        expected.ok_or_else(|| {
            ApiError::bad_request("REVISION_REQUIRED", "Expected revision is required")
        })?,
    )
    .await?;
    sqlx::query("INSERT INTO skill_workspace_entries(id,tenant_id,skill_id,parent_id,name,path,path_hash,entry_type,mime_type,artifact_id,content_hash,size_bytes,editable) VALUES(?,?,?,?,?,?,?,'file',?,?,?,?,?)")
        .bind(entry_id).bind(actor.tenant_id).bind(id).bind(parent_id).bind(name).bind(&path).bind(path_hash(&path)).bind(mime).bind(artifact.id).bind(&artifact.hash).bind(artifact.bytes.len() as u64).bind(false).execute(&mut *tx).await.map_err(map_unique)?;
    insert_revision(&mut tx, &actor, id, entry_id, revision, &artifact).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(UploadResponse {
            entry: load_entry(&state, actor.tenant_id, id, entry_id).await?,
            revision,
        }),
    ))
}

async fn export_workspace(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Response<Body>> {
    actor.require("skill:view")?;
    let workspace = load_workspace(&state, actor.tenant_id, id).await?;
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for entry in workspace
        .entries
        .iter()
        .filter(|entry| entry.entry_type == "file")
    {
        let artifact = get_artifact(
            &state,
            actor.tenant_id,
            entry
                .artifact_id
                .ok_or_else(|| ApiError::internal("file has no artifact"))?,
        )
        .await?;
        zip.start_file(&entry.path, SimpleFileOptions::default())
            .map_err(ApiError::internal)?;
        zip.write_all(&artifact.bytes).map_err(ApiError::internal)?;
    }
    let bytes = zip.finish().map_err(ApiError::internal)?.into_inner();
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=skill-workspace.zip"),
    );
    Ok(response)
}

async fn import_workspace(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    mut multipart: Multipart,
) -> ApiResult<Json<WorkspaceResponse>> {
    actor.require("skill:manage")?;
    let mut expected = None;
    let mut archive = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad_request("INVALID_MULTIPART", "Import is invalid"))?
    {
        match field.name() {
            Some("expectedRevision") => {
                expected = Some(
                    field
                        .text()
                        .await
                        .map_err(ApiError::internal)?
                        .parse::<u64>()
                        .map_err(|_| {
                            ApiError::bad_request("INVALID_REVISION", "Revision is invalid")
                        })?,
                )
            }
            Some("file") => {
                archive = Some(field.bytes().await.map_err(ApiError::internal)?.to_vec())
            }
            _ => {}
        }
    }
    let files = read_zip(
        archive.ok_or_else(|| ApiError::bad_request("FILE_REQUIRED", "ZIP file is required"))?,
    )?;
    if !files.iter().any(|(path, _)| path == "SKILL.md") {
        return Err(ApiError::unprocessable(
            "SKILL_ROOT_REQUIRED",
            "Root SKILL.md is required",
        ));
    }
    ensure_capacity(
        &state,
        actor.tenant_id,
        id,
        files.len(),
        files.iter().map(|(_, b)| b.len() as i64).sum(),
    )
    .await?;
    let mut stored = Vec::new();
    for (path, bytes) in files {
        let mime = if path.to_ascii_lowercase().ends_with(".md") {
            "text/markdown; charset=utf-8"
        } else {
            "application/octet-stream"
        };
        stored.push((
            path,
            mime.to_owned(),
            put_artifact(&state, actor.tenant_id, mime, bytes).await?,
        ));
    }
    let result: ApiResult<()> = async {
        let mut tx = state.pool.begin().await?;
        let revision = bump_revision(&mut tx, actor.tenant_id, id, expected.ok_or_else(|| ApiError::bad_request("REVISION_REQUIRED", "Expected revision is required"))?).await?;
        sqlx::query("DELETE FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=?").bind(actor.tenant_id).bind(id).execute(&mut *tx).await?;
        for (path, mime, artifact) in &stored {
            let entry_id = Uuid::now_v7();
            let name = path.rsplit('/').next().unwrap_or(path);
            sqlx::query("INSERT INTO skill_workspace_entries(id,tenant_id,skill_id,name,path,path_hash,entry_type,mime_type,artifact_id,content_hash,size_bytes,editable) VALUES(?,?,?,?,?,?,'file',?,?,?,?,?)")
                .bind(entry_id).bind(actor.tenant_id).bind(id).bind(name).bind(path).bind(path_hash(path)).bind(mime).bind(artifact.id).bind(&artifact.hash).bind(artifact.bytes.len() as u64).bind(path.to_ascii_lowercase().ends_with(".md")).execute(&mut *tx).await?;
            insert_revision(&mut tx, &actor, id, entry_id, revision, artifact).await?;
        }
        tx.commit().await?;
        Ok(())
    }.await;
    if let Err(error) = result {
        for (_, _, artifact) in stored {
            delete_artifact(&state, actor.tenant_id, artifact.id).await;
        }
        return Err(error);
    }
    Ok(Json(load_workspace(&state, actor.tenant_id, id).await?))
}

async fn list_versions(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<VersionResponse>>> {
    actor.require("skill:view")?;
    let rows = sqlx::query("SELECT id,skill_id,version_number,source_revision,manifest_json,content_hash,created_at,(SELECT COUNT(*) FROM skill_version_files f WHERE f.skill_version_id=v.id) file_count FROM skill_versions v WHERE tenant_id=? AND skill_id=? ORDER BY version_number DESC")
        .bind(actor.tenant_id).bind(id).fetch_all(&state.pool).await?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(version_from_row(&state, row).await?);
    }
    Ok(Json(versions))
}

async fn create_version(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(input): Json<PublishRequest>,
) -> ApiResult<(StatusCode, Json<VersionResponse>)> {
    actor.require("skill:manage")?;
    let skill = load_skill(&state, actor.tenant_id, id).await?;
    if skill.draft_revision != input.expected_revision {
        return Err(ApiError::conflict(
            "SKILL_REVISION_CONFLICT",
            "Skill workspace changed",
        ));
    }
    validate_dependencies(
        &state,
        &actor,
        skill.owner_department_id,
        &input.dependencies,
    )
    .await?;
    let workspace = load_workspace(&state, actor.tenant_id, id).await?;
    let files = workspace
        .entries
        .into_iter()
        .filter(|entry| entry.entry_type == "file")
        .collect::<Vec<_>>();
    if !files.iter().any(|entry| entry.path == "SKILL.md") {
        return Err(ApiError::unprocessable(
            "SKILL_ROOT_REQUIRED",
            "Root SKILL.md is required",
        ));
    }
    let manifest = json!({"schemaVersion":1,"skillId":id,"name":skill.name,"alias":skill.alias,"sourceRevision":input.expected_revision,"files":files.iter().map(|entry| json!({"path":entry.path,"artifactId":entry.artifact_id,"contentHash":entry.content_hash,"sizeBytes":entry.size_bytes,"mimeType":entry.mime_type})).collect::<Vec<_>>(),"dependencies":input.dependencies});
    let hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&manifest).map_err(ApiError::internal)?)
    );
    let version_id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    let number: u64 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(version_number),0)+1 AS UNSIGNED) FROM skill_versions WHERE tenant_id=? AND skill_id=? FOR UPDATE").bind(actor.tenant_id).bind(id).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO skill_versions(id,tenant_id,skill_id,version_number,source_revision,manifest_json,content_hash,created_by) VALUES(?,?,?,?,?,?,?,?)")
        .bind(version_id).bind(actor.tenant_id).bind(id).bind(number).bind(input.expected_revision).bind(&manifest).bind(&hash).bind(actor.user_id).execute(&mut *tx).await?;
    for entry in &files {
        sqlx::query("INSERT INTO skill_version_files(id,tenant_id,skill_version_id,path,path_hash,mime_type,artifact_id,content_hash,size_bytes) VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(version_id).bind(&entry.path).bind(path_hash(&entry.path)).bind(entry.mime_type.as_deref().unwrap_or("application/octet-stream")).bind(entry.artifact_id).bind(entry.content_hash.as_deref().unwrap_or_default()).bind(entry.size_bytes).execute(&mut *tx).await?;
    }
    for dependency in &input.dependencies {
        sqlx::query("INSERT INTO skill_dependencies(id,tenant_id,skill_version_id,resource_type,resource_id,resource_version_id,operation_key) VALUES(?,?,?,?,?,?,?)")
            .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(version_id).bind(&dependency.resource_type).bind(dependency.resource_id).bind(dependency.resource_version_id).bind(&dependency.operation).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let row = sqlx::query("SELECT id,skill_id,version_number,source_revision,manifest_json,content_hash,created_at,(SELECT COUNT(*) FROM skill_version_files f WHERE f.skill_version_id=v.id) file_count FROM skill_versions v WHERE tenant_id=? AND id=?")
        .bind(actor.tenant_id).bind(version_id).fetch_one(&state.pool).await?;
    Ok((
        StatusCode::CREATED,
        Json(version_from_row(&state, row).await?),
    ))
}

fn skill_from_row(row: sqlx::mysql::MySqlRow) -> Result<SkillResponse, sqlx::Error> {
    Ok(SkillResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        alias: row.try_get("alias")?,
        description: row.try_get("description")?,
        owner_department_id: row.try_get("owner_department_id")?,
        status: row.try_get("status")?,
        draft_revision: row.try_get("draft_revision")?,
        latest_version: row.try_get("latest_version")?,
        grant_count: row.try_get::<i64, _>("grant_count")? as u64,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_skill(state: &ControlApiState, tenant: Uuid, id: Uuid) -> ApiResult<SkillResponse> {
    let row = sqlx::query("SELECT s.id,s.name,s.alias,s.description,s.owner_department_id,s.status,s.draft_revision,s.version,s.updated_at,(SELECT MAX(version_number) FROM skill_versions v WHERE v.tenant_id=s.tenant_id AND v.skill_id=s.id) latest_version,(SELECT COUNT(*) FROM resource_grants g WHERE g.tenant_id=s.tenant_id AND g.resource_type='skill' AND g.resource_id=s.id) grant_count FROM skills s WHERE s.tenant_id=? AND s.id=?")
        .bind(tenant).bind(id).fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::not_found("Skill"))?;
    Ok(skill_from_row(row)?)
}

async fn load_workspace(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<WorkspaceResponse> {
    let revision: u64 =
        sqlx::query_scalar("SELECT draft_revision FROM skills WHERE tenant_id=? AND id=?")
            .bind(tenant)
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::not_found("Skill"))?;
    let rows = sqlx::query("SELECT id,parent_id,name,path,entry_type,mime_type,artifact_id,content_hash,size_bytes,editable,updated_at FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=? ORDER BY path")
        .bind(tenant).bind(id).fetch_all(&state.pool).await?;
    Ok(WorkspaceResponse {
        revision,
        entries: rows
            .into_iter()
            .map(entry_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn entry_from_row(row: sqlx::mysql::MySqlRow) -> Result<WorkspaceEntry, sqlx::Error> {
    Ok(WorkspaceEntry {
        id: row.try_get("id")?,
        parent_id: row.try_get("parent_id")?,
        name: row.try_get("name")?,
        path: row.try_get("path")?,
        entry_type: row.try_get("entry_type")?,
        mime_type: row.try_get("mime_type")?,
        artifact_id: row.try_get("artifact_id")?,
        content_hash: row.try_get("content_hash")?,
        size_bytes: row.try_get("size_bytes")?,
        editable: row.try_get("editable")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_entry(
    state: &ControlApiState,
    tenant: Uuid,
    skill: Uuid,
    id: Uuid,
) -> ApiResult<WorkspaceEntry> {
    entry_from_row(entry_row(state, tenant, skill, id).await?).map_err(ApiError::from)
}

async fn entry_row(
    state: &ControlApiState,
    tenant: Uuid,
    skill: Uuid,
    id: Uuid,
) -> ApiResult<sqlx::mysql::MySqlRow> {
    sqlx::query("SELECT id,parent_id,name,path,entry_type,mime_type,artifact_id,content_hash,size_bytes,editable,updated_at FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=? AND id=?")
        .bind(tenant).bind(skill).bind(id).fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::not_found("Skill entry"))
}

async fn version_from_row(
    state: &ControlApiState,
    row: sqlx::mysql::MySqlRow,
) -> ApiResult<VersionResponse> {
    let id: Uuid = row.try_get("id")?;
    let dependencies = sqlx::query("SELECT resource_type,resource_id,resource_version_id,operation_key FROM skill_dependencies WHERE skill_version_id=? ORDER BY resource_type,resource_id")
        .bind(id).fetch_all(&state.pool).await?.into_iter().map(|row| Ok(DependencyInput { resource_type: row.try_get("resource_type")?, resource_id: row.try_get("resource_id")?, resource_version_id: row.try_get("resource_version_id")?, operation: row.try_get("operation_key")? })).collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(VersionResponse {
        id,
        skill_id: row.try_get("skill_id")?,
        version_number: row.try_get("version_number")?,
        source_revision: row.try_get("source_revision")?,
        manifest: row.try_get("manifest_json")?,
        content_hash: row.try_get("content_hash")?,
        file_count: row.try_get::<i64, _>("file_count")? as u64,
        dependencies,
        created_at: row.try_get("created_at")?,
    })
}

async fn put_artifact(
    state: &ControlApiState,
    tenant: Uuid,
    content_type: &str,
    bytes: Vec<u8>,
) -> ApiResult<StoredArtifact> {
    let store = MySqlControlArtifactStore::new(state.pool.clone(), state.control_objects.clone());
    let value = store
        .put(ArtifactWrite {
            tenant_id: TenantId::from_uuid(tenant),
            content_type: content_type.to_owned(),
            content: bytes,
        })
        .await
        .map_err(ApiError::internal)?;
    Ok(StoredArtifact {
        id: value.id.as_uuid(),
        bytes: value.content,
        hash: value.sha256,
    })
}

async fn get_artifact(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<StoredArtifact> {
    let store = MySqlControlArtifactStore::new(state.pool.clone(), state.control_objects.clone());
    let value = store
        .get(TenantId::from_uuid(tenant), ArtifactId::from_uuid(id))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("Artifact"))?;
    Ok(StoredArtifact {
        id,
        bytes: value.content,
        hash: value.sha256,
    })
}

async fn delete_artifact(state: &ControlApiState, tenant: Uuid, id: Uuid) {
    let store = MySqlControlArtifactStore::new(state.pool.clone(), state.control_objects.clone());
    if let Err(error) = store
        .delete(TenantId::from_uuid(tenant), ArtifactId::from_uuid(id))
        .await
    {
        tracing::warn!(%id, %error, "failed to compensate Skill artifact");
    }
}

async fn insert_revision(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    skill: Uuid,
    entry: Uuid,
    revision: u64,
    artifact: &StoredArtifact,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO skill_file_revisions(id,tenant_id,skill_id,entry_id,workspace_revision,artifact_id,content_hash,size_bytes,created_by) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(skill).bind(entry).bind(revision).bind(artifact.id).bind(&artifact.hash).bind(artifact.bytes.len() as u64).bind(actor.user_id).execute(&mut **tx).await?;
    Ok(())
}

async fn bump_revision(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    skill: Uuid,
    expected: u64,
) -> ApiResult<u64> {
    let result = sqlx::query("UPDATE skills SET draft_revision=draft_revision+1,version=version+1 WHERE tenant_id=? AND id=? AND draft_revision=?").bind(tenant).bind(skill).bind(expected).execute(&mut **tx).await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "SKILL_REVISION_CONFLICT",
            "Skill workspace changed",
        ));
    }
    Ok(expected + 1)
}

async fn parent_path(
    state: &ControlApiState,
    tenant: Uuid,
    skill: Uuid,
    parent: Option<Uuid>,
) -> ApiResult<String> {
    let Some(parent) = parent else {
        return Ok(String::new());
    };
    sqlx::query_scalar("SELECT path FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=? AND id=? AND entry_type='directory'").bind(tenant).bind(skill).bind(parent).fetch_optional(&state.pool).await?.ok_or_else(|| ApiError::bad_request("INVALID_PARENT", "Parent directory is invalid"))
}

async fn ensure_capacity(
    state: &ControlApiState,
    tenant: Uuid,
    skill: Uuid,
    entries: usize,
    size_delta: i64,
) -> ApiResult<()> {
    let row = sqlx::query("SELECT COUNT(*) entry_count,CAST(COALESCE(SUM(size_bytes),0) AS UNSIGNED) total_size FROM skill_workspace_entries WHERE tenant_id=? AND skill_id=?").bind(tenant).bind(skill).fetch_one(&state.pool).await?;
    let count: i64 = row.try_get("entry_count")?;
    let size: u64 = row.try_get("total_size")?;
    if count + entries as i64 > MAX_ENTRIES as i64 {
        return Err(ApiError::unprocessable(
            "SKILL_ENTRY_LIMIT",
            "Skill workspace exceeds 1000 entries",
        ));
    }
    let size = i64::try_from(size).map_err(ApiError::internal)? + size_delta;
    if size < 0 || size as usize > MAX_WORKSPACE_SIZE {
        return Err(ApiError::unprocessable(
            "SKILL_WORKSPACE_TOO_LARGE",
            "Skill workspace exceeds 100 MiB",
        ));
    }
    Ok(())
}

async fn require_department(
    state: &ControlApiState,
    actor: &Actor,
    department: Uuid,
) -> ApiResult<()> {
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM departments WHERE tenant_id=? AND id=? AND status='active')",
    )
    .bind(actor.tenant_id)
    .bind(department)
    .fetch_one(&state.pool)
    .await?;
    if !allowed {
        return Err(ApiError::forbidden("Owner department is unavailable"));
    }
    Ok(())
}

async fn ensure_identity(
    state: &ControlApiState,
    tenant: Uuid,
    name: &str,
    alias: &str,
    except: Option<Uuid>,
) -> ApiResult<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM skills WHERE tenant_id=? AND (name=? OR alias=?) AND (? IS NULL OR id<>?))").bind(tenant).bind(name).bind(alias).bind(except).bind(except).fetch_one(&state.pool).await?;
    if exists {
        return Err(ApiError::conflict(
            "SKILL_IDENTITY_EXISTS",
            "Skill name or alias already exists",
        ));
    }
    Ok(())
}

async fn validate_dependencies(
    state: &ControlApiState,
    actor: &Actor,
    owner_department: Uuid,
    dependencies: &[DependencyInput],
) -> ApiResult<()> {
    for dependency in dependencies {
        crate::resource_api::validate_skill_dependency(
            state, actor.tenant_id, owner_department,
            &serde_json::from_value(json!({
                "resourceType": dependency.resource_type, "resourceId": dependency.resource_id,
                "resourceVersionId": dependency.resource_version_id, "operation": dependency.operation
            })).map_err(ApiError::internal)?
        ).await?;
    }
    Ok(())
}

fn validate_alias(value: &str) -> ApiResult<String> {
    let value = required_name(value)?
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if !value
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ApiError::bad_request(
            "INVALID_SKILL_ALIAS",
            "Skill alias must contain letters, digits, dash, or underscore",
        )
        .with_field_error(
            "alias",
            "INVALID_SKILL_ALIAS",
            "Skill alias must contain letters, digits, dash, or underscore",
        ));
    }
    Ok(value)
}

fn validate_description(value: Option<&str>) -> ApiResult<String> {
    let value = value.unwrap_or_default().trim();
    if value.is_empty() || value.chars().count() > 1000 {
        return Err(ApiError::bad_request(
            "SKILL_DESCRIPTION_REQUIRED",
            "Skill description is required and must not exceed 1000 characters",
        ));
    }
    Ok(value.to_owned())
}

fn validate_entry_name(value: &str, kind: &str) -> ApiResult<String> {
    let value = value.trim();
    if !matches!(kind, "directory" | "file")
        || value.is_empty()
        || value.len() > 255
        || matches!(value, "." | "..")
        || value.contains(['/', '\\'])
    {
        return Err(ApiError::bad_request(
            "INVALID_SKILL_ENTRY_NAME",
            "Skill entry name is invalid",
        ));
    }
    Ok(value.to_owned())
}

fn join_path(parent: &str, name: &str) -> ApiResult<String> {
    let value = if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    };
    if value.len() > 2048 || value.split('/').count() > MAX_DEPTH {
        return Err(ApiError::bad_request(
            "INVALID_SKILL_PATH",
            "Skill path is too long or deep",
        ));
    }
    Ok(value)
}

fn path_hash(path: &str) -> String {
    format!("{:x}", Sha256::digest(path.as_bytes()))
}

fn render_skill_document(name: &str, description: &str, body: &str) -> ApiResult<String> {
    let metadata = serde_yaml::to_string(&SkillMetadata {
        name: name.to_owned(),
        description: description.to_owned(),
    })
    .map_err(ApiError::internal)?;
    Ok(format!(
        "---\n{}---\n\n{}",
        metadata.trim_start_matches("---\n"),
        strip_frontmatter(body).trim_start()
    ))
}

fn strip_frontmatter(value: &str) -> &str {
    let normalized = value.trim_start();
    normalized
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n").map(|(_, body)| body))
        .unwrap_or(value)
}

fn read_zip(bytes: Vec<u8>) -> ApiResult<Vec<(String, Vec<u8>)>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| ApiError::bad_request("INVALID_SKILL_ARCHIVE", "Skill archive is invalid"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(ApiError::unprocessable(
            "SKILL_ENTRY_LIMIT",
            "Skill archive exceeds 1000 entries",
        ));
    }
    let mut result = Vec::new();
    let mut total = 0_usize;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(ApiError::internal)?;
        if file.is_dir() {
            continue;
        }
        let path = file
            .enclosed_name()
            .and_then(|path| path.to_str().map(|value| value.replace('\\', "/")))
            .ok_or_else(|| {
                ApiError::bad_request("INVALID_SKILL_PATH", "Archive contains an unsafe path")
            })?;
        if path.split('/').count() > MAX_DEPTH || path.len() > 2048 {
            return Err(ApiError::bad_request(
                "INVALID_SKILL_PATH",
                "Archive path is too long or deep",
            ));
        }
        let mut content = Vec::new();
        file.by_ref()
            .take((MAX_FILE_SIZE + 1) as u64)
            .read_to_end(&mut content)
            .map_err(ApiError::internal)?;
        if content.len() > MAX_FILE_SIZE {
            return Err(ApiError::bad_request(
                "SKILL_FILE_TOO_LARGE",
                "Skill file exceeds 20 MiB",
            ));
        }
        total += content.len();
        if total > MAX_WORKSPACE_SIZE {
            return Err(ApiError::unprocessable(
                "SKILL_WORKSPACE_TOO_LARGE",
                "Skill archive exceeds 100 MiB",
            ));
        }
        result.push((path, content));
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    if result.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(ApiError::conflict(
            "SKILL_PATH_EXISTS",
            "Archive contains duplicate paths",
        ));
    }
    Ok(result)
}

fn map_unique(error: sqlx::Error) -> ApiError {
    if error
        .as_database_error()
        .is_some_and(|value| value.is_unique_violation())
    {
        ApiError::conflict(
            "SKILL_CONFLICT",
            "Skill name, alias, or workspace path already exists",
        )
    } else {
        ApiError::from(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_paths_and_archives() {
        assert!(validate_entry_name("../secret", "file").is_err());
        assert!(join_path(&"a/".repeat(20), "file.md").is_err());
        assert_eq!(validate_alias("My-Skill").unwrap(), "my-skill");
        assert_eq!(
            validate_alias("数据分析-Agent_2").unwrap(),
            "数据分析-agent_2"
        );
        assert!(validate_alias("数据 分析").is_err());
        assert!(validate_alias("数据/分析").is_err());
    }

    #[test]
    fn renders_required_skill_frontmatter() {
        let value = render_skill_document("Example", "Description", "# Body").unwrap();
        assert!(value.starts_with("---\n"));
        assert!(value.contains("name: Example"));
        assert!(value.ends_with("# Body"));
    }
}
