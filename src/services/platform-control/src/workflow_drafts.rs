use crate::{api_error::ApiResult, control_api::Actor};
use sqlx::{MySql, Transaction};
use uuid::Uuid;

pub(crate) async fn record_initial_revision(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    workflow: Uuid,
    draft: Uuid,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO workflow_draft_revisions(id,tenant_id,workflow_id,draft_id,revision,schema_version,definition_json,editor_json,content_hash,editor_hash,created_by) SELECT ?,tenant_id,workflow_id,id,0,schema_version,definition_json,editor_json,content_hash,editor_hash,? FROM workflow_drafts WHERE tenant_id=? AND workflow_id=? AND id=?")
        .bind(Uuid::now_v7()).bind(actor.user_id).bind(actor.tenant_id).bind(workflow).bind(draft).execute(&mut **tx).await?;
    Ok(())
}
