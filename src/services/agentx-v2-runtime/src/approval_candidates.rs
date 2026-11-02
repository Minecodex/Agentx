use crate::error::RuntimeResult;
use sqlx::{MySql, Transaction};
use uuid::Uuid;

/// Current projected membership, never the model's claimed role or a stale decision snapshot.
pub(crate) async fn current(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    task: Uuid,
    user: Uuid,
) -> RuntimeResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_user_admission u WHERE u.tenant_id=? AND u.user_id=? AND u.status='active' AND u.can_approve=TRUE AND EXISTS(SELECT 1 FROM approval_candidates c WHERE c.tenant_id=u.tenant_id AND c.approval_task_id=? AND ((c.candidate_type='user' AND c.candidate_id=u.user_id) OR (c.candidate_type='department' AND c.candidate_id=u.department_id) OR (c.candidate_type='role' AND EXISTS(SELECT 1 FROM JSON_TABLE(u.role_assignments_json,'$[*]' COLUMNS(role_id CHAR(36) PATH '$.id')) r WHERE c.candidate_id=UUID_TO_BIN(r.role_id))))))")
        .bind(tenant).bind(user).bind(task).fetch_one(&mut **tx).await?)
}
