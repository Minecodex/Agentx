//! Trusted Runtime caller context, independent of HTTP transport.
use crate::error::{RuntimeError, RuntimeResult};
use sqlx::{MySql, Transaction};
use uuid::Uuid;
#[derive(Clone)]
pub(crate) struct ApplicationCaller {
    pub(crate) tenant_id: Uuid,
    pub(crate) application_id: Uuid,
    pub(crate) subject_id: Uuid,
    pub(crate) caller_type: &'static str,
    pub(crate) token_version: Option<u64>,
    pub(crate) origin: agentx_runtime_contracts::ExecutionOriginV1,
}

pub(crate) async fn admission_epoch(
    tx: &mut Transaction<'_, MySql>,
    tenant: Uuid,
    application: Uuid,
    kind: &str,
    id: Uuid,
    version: Option<u64>,
    head_epoch: u64,
) -> RuntimeResult<u64> {
    match kind {
        "api_key"=>sqlx::query_scalar("SELECT admission_epoch FROM api_key_admission WHERE key_id=? AND tenant_id=? AND application_id=? AND status='active' AND (expires_at IS NULL OR expires_at>UTC_TIMESTAMP(6)) FOR SHARE").bind(id).bind(tenant).bind(application).fetch_optional(&mut **tx).await?.ok_or(RuntimeError::Unauthorized),
        "user"=>sqlx::query_scalar("SELECT GREATEST(u.admission_epoch,g.admission_epoch) FROM runtime_user_admission u JOIN runtime_user_application_grants g ON g.tenant_id=u.tenant_id AND g.user_id=u.user_id WHERE u.tenant_id=? AND u.user_id=? AND u.status='active' AND u.token_version=? AND g.application_id=? AND g.status='active' AND g.can_invoke=TRUE FOR SHARE").bind(tenant).bind(id).bind(version.ok_or(RuntimeError::Unauthorized)?).bind(application).fetch_optional(&mut **tx).await?.ok_or(RuntimeError::Unauthorized),
        _=>Ok(head_epoch)
    }
}
