use agentx_runtime_contracts::MemoryScopeV1;
use sqlx::MySqlPool;
use uuid::Uuid;

pub(crate) async fn is_cleared(
    pool: &MySqlPool,
    scope: &MemoryScopeV1,
    connection: Uuid,
) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_subject_memory_clears WHERE tenant_id=? AND scope_hash=? AND memory_connection_id=? AND status IN ('requested','applied')")
        .bind(scope.tenant_id).bind(scope.scope_hash()).bind(connection).fetch_one(pool).await?;
    Ok(count > 0)
}

pub(crate) async fn record(
    pool: &MySqlPool,
    scope: &MemoryScopeV1,
    connection: Uuid,
    operation: &str,
    operation_id: &str,
) -> Result<(), sqlx::Error> {
    let Some(application) = scope.application_id else {
        return Ok(());
    };
    sqlx::query("INSERT INTO agent_subject_memory_audit(audit_id,tenant_id,application_id,application_user_id,memory_connection_id,operation,scope_hash,operation_id) VALUES(?,?,?,?,?,?,?,?)")
        .bind(Uuid::now_v7()).bind(scope.tenant_id).bind(application).bind(scope.user.subject_id())
        .bind(connection).bind(operation).bind(scope.scope_hash()).bind(operation_id).execute(pool).await?;
    Ok(())
}
