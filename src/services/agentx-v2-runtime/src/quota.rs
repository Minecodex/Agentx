use std::collections::BTreeMap;

use agentx_node_protocol::Item;
use serde_json::Value;
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

#[derive(Clone, Debug)]
pub struct QuotaCounterProjection {
    pub tenant_id: Uuid,
    pub dimension: String,
    pub hard_limit: u64,
    pub active: u64,
    pub committed: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct QuotaProjectionLease {
    pub owner_id: Uuid,
    pub fencing_token: u64,
}

pub async fn claim_projection(
    pool: &sqlx::MySqlPool,
    owner_id: Uuid,
) -> RuntimeResult<Option<QuotaProjectionLease>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT fencing_token FROM runtime_role_leases WHERE role_key='quota_projection' AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let fencing_token = row.try_get::<u64, _>("fencing_token")? + 1;
    let changed = sqlx::query(
        "UPDATE runtime_role_leases SET locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),fencing_token=?,heartbeat_at=UTC_TIMESTAMP(6) WHERE role_key='quota_projection' AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6))",
    )
    .bind(owner_id)
    .bind(fencing_token)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        tx.rollback().await?;
        return Ok(None);
    }
    tx.commit().await?;
    Ok(Some(QuotaProjectionLease {
        owner_id,
        fencing_token,
    }))
}

pub async fn heartbeat_projection(
    pool: &sqlx::MySqlPool,
    lease: QuotaProjectionLease,
) -> RuntimeResult<()> {
    let changed = sqlx::query(
        "UPDATE runtime_role_leases SET locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),heartbeat_at=UTC_TIMESTAMP(6) WHERE role_key='quota_projection' AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)",
    )
    .bind(lease.owner_id)
    .bind(lease.fencing_token)
    .execute(pool)
    .await?;
    if changed.rows_affected() != 1 {
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Quota projection Lease was lost".into(),
        ));
    }
    Ok(())
}

pub async fn counter_projection(
    pool: &sqlx::MySqlPool,
) -> RuntimeResult<Vec<QuotaCounterProjection>> {
    let policies = sqlx::query(
        "SELECT tenant_id,dimension_key,hard_limit,period_seconds FROM quota_policy_projection ORDER BY tenant_id,dimension_key",
    )
    .fetch_all(pool)
    .await?;
    let mut projection = Vec::with_capacity(policies.len());
    for policy in policies {
        let tenant_id: Uuid = policy.try_get("tenant_id")?;
        let dimension: String = policy.try_get("dimension_key")?;
        let active: rust_decimal::Decimal = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount),0) FROM quota_reservations WHERE tenant_id=? AND dimension_key=? AND status='active' AND expires_at>UTC_TIMESTAMP(6)",
        )
        .bind(tenant_id)
        .bind(&dimension)
        .fetch_one(pool)
        .await?;
        let committed: rust_decimal::Decimal = if dimension == "artifact_bytes" {
            sqlx::query_scalar(
                "SELECT COALESCE(SUM(size_bytes),0) FROM artifacts WHERE tenant_id=? AND deleted_at IS NULL",
            )
            .bind(tenant_id)
            .fetch_one(pool)
            .await?
        } else if let Some(period_seconds) = policy.try_get::<Option<u64>, _>("period_seconds")? {
            sqlx::query_scalar(
                "SELECT COALESCE(SUM(amount),0) FROM quota_usage_ledger WHERE tenant_id=? AND dimension_key=? AND occurred_at>=TIMESTAMPADD(SECOND,-CAST(? AS SIGNED),UTC_TIMESTAMP(6))",
            )
            .bind(tenant_id)
            .bind(&dimension)
            .bind(period_seconds)
            .fetch_one(pool)
            .await?
        } else {
            rust_decimal::Decimal::ZERO
        };
        projection.push(QuotaCounterProjection {
            tenant_id,
            dimension,
            hard_limit: decimal_u64(policy.try_get("hard_limit")?)?,
            active: decimal_u64(active)?,
            committed: decimal_u64(committed)?,
        });
    }
    Ok(projection)
}

fn decimal_u64(value: rust_decimal::Decimal) -> RuntimeResult<u64> {
    use rust_decimal::prelude::ToPrimitive as _;
    value.to_u64().ok_or_else(|| {
        RuntimeError::Internal(anyhow::anyhow!(
            "quota counter cannot be represented as an unsigned integer"
        ))
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn reserve(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    dimension: &str,
    scope_type: &str,
    scope_id: &str,
    idempotency_key: &str,
    amount: u64,
    ttl_seconds: u32,
) -> RuntimeResult<()> {
    if let Some(existing) = sqlx::query(
        "SELECT dimension_key,scope_type,scope_id,amount FROM quota_reservations WHERE tenant_id=? AND idempotency_key=? FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await?
    {
        let requested = rust_decimal::Decimal::from(amount);
        if existing.try_get::<String, _>("dimension_key")? == dimension
            && existing.try_get::<String, _>("scope_type")? == scope_type
            && existing.try_get::<String, _>("scope_id")? == scope_id
            && existing.try_get::<rust_decimal::Decimal, _>("amount")? == requested
        {
            return Ok(());
        }
        return Err(RuntimeError::Conflict(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
            "Quota idempotency key was reused with different reservation input".into(),
        ));
    }
    if let Some(policy) = sqlx::query(
        "SELECT hard_limit,period_seconds FROM quota_policy_projection WHERE tenant_id=? AND dimension_key=? FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(dimension)
    .fetch_optional(&mut **tx)
    .await?
    {
        let limit: rust_decimal::Decimal = policy.try_get("hard_limit")?;
        // One statement gives active reservations and committed usage the
        // same read view, even if a concurrent settlement changes both tables.
        let (active, committed): (rust_decimal::Decimal, rust_decimal::Decimal) =
            if dimension == "artifact_bytes" {
            sqlx::query_as(
                "SELECT (SELECT COALESCE(SUM(amount),0) FROM quota_reservations WHERE tenant_id=? AND dimension_key=? AND status='active' AND expires_at>UTC_TIMESTAMP(6)),(SELECT COALESCE(SUM(size_bytes),0) FROM artifacts WHERE tenant_id=? AND deleted_at IS NULL)",
            )
            .bind(tenant_id)
            .bind(dimension)
            .bind(tenant_id)
            .fetch_one(&mut **tx)
            .await?
        } else {
            let period_seconds = policy.try_get::<Option<u64>, _>("period_seconds")?;
            sqlx::query_as(
                "SELECT (SELECT COALESCE(SUM(amount),0) FROM quota_reservations WHERE tenant_id=? AND dimension_key=? AND status='active' AND expires_at>UTC_TIMESTAMP(6)),(SELECT COALESCE(SUM(amount),0) FROM quota_usage_ledger WHERE tenant_id=? AND dimension_key=? AND ? IS NOT NULL AND occurred_at>=TIMESTAMPADD(SECOND,-CAST(? AS SIGNED),UTC_TIMESTAMP(6)))",
            )
            .bind(tenant_id)
            .bind(dimension)
            .bind(tenant_id)
            .bind(dimension)
            .bind(period_seconds)
            .bind(period_seconds)
            .fetch_one(&mut **tx)
            .await?
        };
        if active + committed + rust_decimal::Decimal::from(amount) > limit {
            return Err(RuntimeError::BadRequest(
                agentx_runtime_contracts::RuntimePublishErrorCodeV1::AdmissionPrerequisiteMissing,
                format!("RUNTIME_QUOTA_EXCEEDED: Runtime quota {dimension} is exhausted"),
            ));
        }
    }
    sqlx::query(
        "INSERT INTO quota_reservations(id,tenant_id,dimension_key,scope_type,scope_id,idempotency_key,amount,status,expires_at) VALUES(?,?,?,?,?,?,?,'active',DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND)) ON DUPLICATE KEY UPDATE id=id",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(dimension)
    .bind(scope_type)
    .bind(scope_id)
    .bind(idempotency_key)
    .bind(rust_decimal::Decimal::from(amount))
    .bind(ttl_seconds.max(1))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn settle_attempt_usage(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    attempt_id: Uuid,
    outputs: &BTreeMap<String, Vec<Item>>,
) -> RuntimeResult<()> {
    let mut usage = BTreeMap::<&'static str, u64>::new();
    for items in outputs.values() {
        for item in items {
            collect_usage(&item.json, &mut usage);
        }
    }
    if let Some(row) = sqlx::query(
        "SELECT iteration_count,input_tokens+output_tokens tokens,cost_micros FROM agent_runs WHERE tenant_id=? AND node_execution_id=(SELECT node_execution_id FROM node_attempts WHERE id=?)",
    )
    .bind(tenant_id)
    .bind(attempt_id)
    .fetch_optional(&mut **tx)
    .await?
    {
        usage.insert("agent_iterations", row.try_get::<u64, _>("iteration_count")?);
        usage.insert("tokens", row.try_get::<u64, _>("tokens")?);
        usage.insert("cost_micros", row.try_get::<u64, _>("cost_micros")?);
    }
    for (dimension, amount) in usage {
        if amount > 0 {
            settle(
                tx,
                tenant_id,
                dimension,
                "attempt",
                &attempt_id.to_string(),
                amount,
                &format!("attempt:{attempt_id}:usage:{dimension}"),
            )
            .await?;
        }
    }
    Ok(())
}

pub(crate) async fn reserve_attempt_budget(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    ttl_seconds: u32,
    node: &agentx_runtime::CompiledNode,
    attempt_id: Uuid,
    resources: &[agentx_runtime_contracts::RuntimeResourceBindingV1],
) -> RuntimeResult<()> {
    let budget = node.parameters.get("budget").unwrap_or(&node.parameters);
    let mut dimensions = BTreeMap::from([
        (
            "agent_iterations",
            budget.get("maxIterations").and_then(Value::as_u64),
        ),
        ("tokens", budget.get("maxTokens").and_then(Value::as_u64)),
        (
            "cost_micros",
            budget.get("maxCostMicros").and_then(Value::as_u64),
        ),
        (
            "artifact_bytes",
            node.parameters
                .get("maxArtifactBytes")
                .and_then(Value::as_u64),
        ),
        (
            "cpu_millis",
            node.parameters.get("cpuMillis").and_then(Value::as_u64),
        ),
        (
            "memory_bytes",
            node.parameters.get("memoryBytes").and_then(Value::as_u64),
        ),
        (
            "pids",
            node.parameters.get("pidLimit").and_then(Value::as_u64),
        ),
        (
            "disk_bytes",
            node.parameters.get("diskBytes").and_then(Value::as_u64),
        ),
        (
            "ttl_seconds",
            node.parameters.get("ttlSeconds").and_then(Value::as_u64),
        ),
    ]);
    if node.capability == agentx_node_protocol::NodeCapability::Sandbox
        && let Some(agentx_runtime_contracts::RuntimeResourceConfigurationV1::SandboxProfile {
            cpu_millis,
            memory_bytes,
            disk_bytes,
            pid_limit,
            maximum_ttl_seconds,
            ..
        }) = resources
            .iter()
            .find(|resource| {
                resource.resource_kind
                    == agentx_runtime_contracts::RuntimeResourceKindV1::SandboxProfile
            })
            .map(|resource| &resource.configuration)
    {
        dimensions.insert("cpu_millis", Some(u64::from(*cpu_millis)));
        dimensions.insert("memory_bytes", Some(*memory_bytes));
        dimensions.insert("disk_bytes", Some(*disk_bytes));
        dimensions.insert("pids", Some(u64::from(*pid_limit)));
        dimensions.insert("ttl_seconds", Some(u64::from(*maximum_ttl_seconds)));
    }
    for (dimension, amount) in dimensions {
        let Some(amount) = amount.filter(|amount| *amount > 0) else {
            continue;
        };
        reserve(
            tx,
            tenant_id,
            dimension,
            "attempt",
            &attempt_id.to_string(),
            &format!("attempt:{attempt_id}:{dimension}"),
            amount,
            ttl_seconds,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn settle(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    dimension: &str,
    scope_type: &str,
    scope_id: &str,
    amount: u64,
    idempotency_key: &str,
) -> RuntimeResult<()> {
    sqlx::query(
        "INSERT INTO quota_usage_ledger(id,tenant_id,dimension_key,scope_type,scope_id,amount,idempotency_key) VALUES(?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE id=id",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(dimension)
    .bind(scope_type)
    .bind(scope_id)
    .bind(rust_decimal::Decimal::from(amount))
    .bind(idempotency_key)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE quota_reservations SET status='settled',settled_amount=?,release_reason='usage_settled',settled_at=UTC_TIMESTAMP(6) WHERE tenant_id=? AND dimension_key=? AND scope_type=? AND scope_id=? AND status='active'",
    )
    .bind(rust_decimal::Decimal::from(amount))
    .bind(tenant_id)
    .bind(dimension)
    .bind(scope_type)
    .bind(scope_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn collect_usage(value: &Value, usage: &mut BTreeMap<&'static str, u64>) {
    let mappings = [
        ("tokens", ["tokens", "totalTokens"]),
        ("cost_micros", ["costMicros", "costMicros"]),
        ("artifact_bytes", ["artifactBytes", "artifactBytes"]),
        ("cpu_millis", ["cpuMillis", "cpuMillis"]),
        ("memory_bytes", ["memoryBytes", "memoryBytes"]),
        ("pids", ["pids", "pidCount"]),
        ("disk_bytes", ["diskBytes", "diskBytes"]),
        ("ttl_seconds", ["ttlSeconds", "ttlSeconds"]),
    ];
    for (dimension, keys) in mappings {
        if let Some(amount) = keys
            .iter()
            .find_map(|key| value.get(key).and_then(Value::as_u64))
        {
            usage
                .entry(dimension)
                .and_modify(|current| *current = current.saturating_add(amount))
                .or_insert(amount);
        }
    }
    if let Some(usage_value) = value.get("usage") {
        collect_usage(usage_value, usage);
        let tokens = usage_value
            .get("inputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(
                usage_value
                    .get("outputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            );
        if tokens > 0
            && usage_value.get("tokens").is_none()
            && usage_value.get("totalTokens").is_none()
        {
            usage
                .entry("tokens")
                .and_modify(|current| *current = current.saturating_add(tokens))
                .or_insert(tokens);
        }
    }
}

pub(crate) async fn release_attempt(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    attempt_id: Uuid,
    reason: &str,
) -> RuntimeResult<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM quota_reservations WHERE tenant_id=? AND scope_type='attempt' AND scope_id=? AND status='active' ORDER BY id",
    )
    .bind(tenant_id)
    .bind(attempt_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    release_reservations(tx, &ids, reason).await?;
    Ok(())
}

pub(crate) async fn release_execution(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
    reason: &str,
) -> RuntimeResult<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM quota_reservations WHERE tenant_id=? AND ((scope_type='execution' AND scope_id=?) OR (scope_type='attempt' AND scope_id IN (SELECT CAST(BIN_TO_UUID(id) AS CHAR) COLLATE utf8mb4_0900_ai_ci FROM node_attempts WHERE tenant_id=? AND execution_id=?))) AND status='active' ORDER BY id",
    )
    .bind(tenant_id)
    .bind(execution_id.to_string())
    .bind(tenant_id)
    .bind(execution_id)
    .fetch_all(&mut **tx)
    .await?;
    release_reservations(tx, &ids, reason).await?;
    Ok(())
}

async fn release_reservations(
    tx: &mut Transaction<'_, MySql>,
    ids: &[Uuid],
    reason: &str,
) -> RuntimeResult<()> {
    // Lock primary keys in a stable order, as expiry recovery does. In
    // particular, an attempt with no budget must never lock another scope.
    for id in ids {
        sqlx::query("UPDATE quota_reservations SET status='released',settled_amount=0,release_reason=?,settled_at=UTC_TIMESTAMP(6) WHERE id=? AND status='active'")
            .bind(reason).bind(id).execute(&mut **tx).await?;
    }
    Ok(())
}
