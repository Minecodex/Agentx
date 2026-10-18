use super::*;
use axum::{extract::Query, http::StatusCode};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProfileDraft {
    pub name: String,
    pub description: Option<String>,
    pub criteria: String,
    pub pass_threshold: f64,
    pub version: Option<u64>,
}
#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileResponse {
    pub id: Uuid,
    pub application_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub version: u64,
    pub version_id: Uuid,
    pub version_number: u64,
    pub criteria: String,
    pub pass_threshold: f64,
}
fn validate(draft: &ProfileDraft) -> ApiResult<()> {
    crate::control_helpers::required_name(&draft.name)?;
    if draft.criteria.trim().is_empty() || draft.criteria.len() > 64 * 1024 {
        return Err(ApiError::bad_request(
            "INVALID_EVALUATION_CRITERIA",
            "Provide evaluation criteria of at most 64 KiB",
        ));
    }
    if !draft.pass_threshold.is_finite() || !(0.0..=1.0).contains(&draft.pass_threshold) {
        return Err(ApiError::bad_request(
            "INVALID_PASS_THRESHOLD",
            "Pass threshold must be between 0 and 1",
        ));
    }
    Ok(())
}
pub(crate) async fn load(
    state: &ControlApiState,
    actor: &Actor,
    application: Uuid,
    id: Uuid,
) -> ApiResult<ProfileResponse> {
    let row=sqlx::query("SELECT p.*,v.id version_id,v.version_number,v.criteria,CAST(v.pass_threshold AS DOUBLE) pass_threshold FROM evaluation_profiles p JOIN evaluation_profile_versions v ON v.tenant_id=p.tenant_id AND v.profile_id=p.id WHERE p.tenant_id=? AND p.application_id=? AND p.id=? ORDER BY v.version_number DESC LIMIT 1").bind(actor.tenant_id).bind(application).bind(id).fetch_optional(&state.pool).await?.ok_or_else(||ApiError::not_found("Evaluation Profile"))?;
    Ok(ProfileResponse {
        id,
        application_id: application,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        version: row.try_get("version")?,
        version_id: row.try_get("version_id")?,
        version_number: row.try_get("version_number")?,
        criteria: row.try_get("criteria")?,
        pass_threshold: row.try_get("pass_threshold")?,
    })
}
pub(crate) async fn list(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
) -> ApiResult<Json<Vec<ProfileResponse>>> {
    authorize(&state, &actor, application, "evaluation_profile:view").await?;
    let ids=sqlx::query_scalar("SELECT id FROM evaluation_profiles WHERE tenant_id=? AND application_id=? ORDER BY updated_at DESC,id").bind(actor.tenant_id).bind(application).fetch_all(&state.pool).await?;
    let mut items = Vec::new();
    for id in ids {
        items.push(load(&state, &actor, application, id).await?)
    }
    Ok(Json(items))
}
async fn version(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    profile: Uuid,
    number: u64,
    draft: &ProfileDraft,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO evaluation_profile_versions(id,tenant_id,profile_id,version_number,criteria,pass_threshold,content_hash,created_by) VALUES(?,?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(profile).bind(number).bind(draft.criteria.trim()).bind(draft.pass_threshold).bind(hash(&json!({"criteria":draft.criteria.trim(),"passThreshold":draft.pass_threshold}))?).bind(actor.user_id).execute(&mut **tx).await?;
    Ok(id)
}
pub(crate) async fn create(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path(application): Path<Uuid>,
    Json(input): Json<ProfileDraft>,
) -> ApiResult<(StatusCode, Json<ProfileResponse>)> {
    authorize(&state, &actor, application, "evaluation_profile:manage").await?;
    validate(&input)?;
    let id = Uuid::now_v7();
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO evaluation_profiles(id,tenant_id,application_id,name,description,owner_user_id,owner_department_id) VALUES(?,?,?,?,?,?,?)").bind(id).bind(actor.tenant_id).bind(application).bind(input.name.trim()).bind(&input.description).bind(actor.user_id).bind(actor.department_id).execute(&mut *tx).await.map_err(|error|name_conflict(error,"EVALUATION_PROFILE_NAME_EXISTS"))?;
    version(&mut tx, &actor, id, 1, &input).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(load(&state, &actor, application, id).await?),
    ))
}
pub(crate) async fn update(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, profile)): Path<(Uuid, Uuid)>,
    Json(input): Json<ProfileDraft>,
) -> ApiResult<Json<ProfileResponse>> {
    authorize(&state, &actor, application, "evaluation_profile:manage").await?;
    validate(&input)?;
    let mut tx = state.pool.begin().await?;
    let current=sqlx::query("SELECT p.version,v.version_number,v.criteria,CAST(v.pass_threshold AS DOUBLE) pass_threshold FROM evaluation_profiles p JOIN evaluation_profile_versions v ON v.tenant_id=p.tenant_id AND v.profile_id=p.id WHERE p.tenant_id=? AND p.application_id=? AND p.id=? ORDER BY v.version_number DESC LIMIT 1 FOR UPDATE").bind(actor.tenant_id).bind(application).bind(profile).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Evaluation Profile"))?;
    if Some(current.try_get::<u64, _>("version")?) != input.version {
        return Err(ApiError::conflict(
            "PROFILE_VERSION_CONFLICT",
            "Evaluation profile changed; reload before saving",
        ));
    }
    if current.try_get::<String, _>("criteria")? != input.criteria.trim()
        || current.try_get::<f64, _>("pass_threshold")? != input.pass_threshold
    {
        version(
            &mut tx,
            &actor,
            profile,
            current.try_get::<u64, _>("version_number")? + 1,
            &input,
        )
        .await?;
    }
    sqlx::query("UPDATE evaluation_profiles SET name=?,description=?,version=version+1 WHERE tenant_id=? AND id=?").bind(input.name.trim()).bind(input.description).bind(actor.tenant_id).bind(profile).execute(&mut *tx).await.map_err(|error|name_conflict(error,"EVALUATION_PROFILE_NAME_EXISTS"))?;
    tx.commit().await?;
    Ok(Json(load(&state, &actor, application, profile).await?))
}
pub(crate) async fn delete(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, profile)): Path<(Uuid, Uuid)>,
    Query(input): Query<super::datasets::DeleteVersion>,
) -> ApiResult<StatusCode> {
    authorize(&state, &actor, application, "evaluation_profile:delete").await?;
    let current = load(&state, &actor, application, profile).await?;
    if current.version != input.expected_version {
        return Err(ApiError::conflict(
            "PROFILE_VERSION_CONFLICT",
            "Evaluation profile changed",
        ));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM applications WHERE tenant_id=? AND id=? FOR UPDATE")
        .bind(actor.tenant_id)
        .bind(application)
        .fetch_one(&mut *tx)
        .await?;
    let revision:u64=sqlx::query_scalar("SELECT version FROM evaluation_profiles WHERE tenant_id=? AND application_id=? AND id=? FOR UPDATE").bind(actor.tenant_id).bind(application).bind(profile).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Evaluation Profile"))?;
    if revision != input.expected_version {
        return Err(ApiError::conflict(
            "PROFILE_VERSION_CONFLICT",
            "Evaluation profile changed",
        ));
    }
    let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM evaluation_batches b JOIN evaluation_profile_versions v ON v.tenant_id=b.tenant_id AND v.id=b.profile_version_id WHERE b.tenant_id=? AND v.profile_id=?)").bind(actor.tenant_id).bind(profile).fetch_one(&mut *tx).await?;
    if used {
        return Err(ApiError::conflict(
            "ENTITY_IN_USE",
            "Profile is referenced by evaluation history",
        ));
    }
    sqlx::query("DELETE FROM evaluation_profile_versions WHERE tenant_id=? AND profile_id=?")
        .bind(actor.tenant_id)
        .bind(profile)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM evaluation_profiles WHERE tenant_id=? AND application_id=? AND id=? AND version=?").bind(actor.tenant_id).bind(application).bind(profile).bind(input.expected_version).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn versions(
    State(state): State<ControlApiState>,
    actor: Actor,
    Path((application, profile)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Vec<ProfileResponse>>> {
    authorize(&state, &actor, application, "evaluation_profile:view").await?;
    let current = load(&state, &actor, application, profile).await?;
    let rows=sqlx::query("SELECT id,version_number,criteria,CAST(pass_threshold AS DOUBLE) pass_threshold FROM evaluation_profile_versions WHERE tenant_id=? AND profile_id=? ORDER BY version_number DESC").bind(actor.tenant_id).bind(profile).fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                Ok(ProfileResponse {
                    id: profile,
                    application_id: application,
                    name: current.name.clone(),
                    description: current.description.clone(),
                    version: current.version,
                    version_id: row.try_get("id")?,
                    version_number: row.try_get("version_number")?,
                    criteria: row.try_get("criteria")?,
                    pass_threshold: row.try_get("pass_threshold")?,
                })
            })
            .collect::<ApiResult<Vec<_>>>()?,
    ))
}
