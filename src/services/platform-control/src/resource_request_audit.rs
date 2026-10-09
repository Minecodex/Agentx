//! Persist resource request actions in the existing audit store.

use super::*;

pub(super) fn timestamp(value: OffsetDateTime) -> ApiResult<String> {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(ApiError::internal)
}

pub(super) async fn record(
    tx: &mut Transaction<'_, MySql>,
    actor: &Actor,
    request: Uuid,
    action: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO audit_events(id,tenant_id,actor_user_id,action,target_type,target_id,request_id,detail_json) VALUES(?,?,?,?, 'resource_grant_request',?,?,JSON_OBJECT())")
        .bind(Uuid::now_v7()).bind(actor.tenant_id).bind(actor.user_id).bind(action)
        .bind(request.to_string()).bind(Uuid::now_v7()).execute(&mut **tx).await?;
    Ok(())
}

pub(super) async fn history(
    state: &ControlApiState,
    tenant: Uuid,
    id: Uuid,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query("SELECT a.id,a.action,a.occurred_at,u.display_name actor_name FROM audit_events a LEFT JOIN users u ON u.tenant_id=a.tenant_id AND u.id=a.actor_user_id WHERE a.tenant_id=? AND a.target_type='resource_grant_request' AND a.target_id=? ORDER BY a.occurred_at,a.id")
        .bind(tenant).bind(id.to_string()).fetch_all(&state.pool).await?;
    rows.into_iter()
        .map(|row| {
            Ok(json!({
                "id":row.try_get::<Uuid,_>("id")?, "action":row.try_get::<String,_>("action")?,
                "actorName":row.try_get::<Option<String>,_>("actor_name")?,
                "occurredAt":timestamp(row.try_get("occurred_at")?)?
            }))
        })
        .collect()
}
