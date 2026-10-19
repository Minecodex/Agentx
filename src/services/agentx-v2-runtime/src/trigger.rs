//! Runtime-local trigger Claim/Lease implementation.
//!
//! Trigger configuration is immutable inside an activated Bundle. MySQL owns
//! every cursor and idempotency fact; provider I/O is always performed after
//! the claim transaction commits.

use std::time::Duration;

use agentx_runtime_contracts::{
    RuntimeTriggerConfigurationV1, RuntimeTriggerSpecV1, ScheduleMisfirePolicyV1,
};
use chrono::{DateTime, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    error::{RuntimeError, RuntimeResult},
    execution::{InvocationCaller, create_runtime_invocation_tx},
};

pub struct TriggerClaim {
    pub binding_id: Uuid,
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub bundle_id: Uuid,
    pub revision: u64,
    pub kind: String,
    pub configuration: RuntimeTriggerSpecV1,
    pub cursor: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub owner: Uuid,
    pub fencing_token: u64,
}

pub async fn claim(
    pool: &sqlx::MySqlPool,
    owner: Uuid,
    limit: u32,
) -> RuntimeResult<Vec<TriggerClaim>> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query("SELECT id FROM trigger_bindings WHERE status='active' AND trigger_kind IN ('schedule') AND next_poll_at IS NOT NULL AND next_poll_at<=UTC_TIMESTAMP(6) AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6)) ORDER BY next_poll_at,id LIMIT ? FOR UPDATE SKIP LOCKED")
        .bind(limit.clamp(1,100)).fetch_all(&mut *tx).await?;
    let mut ids = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let updated = sqlx::query("UPDATE trigger_bindings SET locked_by=?,locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),heartbeat_at=UTC_TIMESTAMP(6),fencing_token=fencing_token+1 WHERE id=? AND status='active' AND (locked_until IS NULL OR locked_until<=UTC_TIMESTAMP(6))")
            .bind(owner).bind(id).execute(&mut *tx).await?;
        if updated.rows_affected() == 1 {
            ids.push(id);
        }
    }
    tx.commit().await?;
    let mut claims = Vec::new();
    for id in ids {
        let row=sqlx::query("SELECT id,tenant_id,application_id,bundle_id,configuration_revision,trigger_kind,configuration_json,cursor_value,next_poll_at,fencing_token FROM trigger_bindings WHERE id=? AND locked_by=? AND locked_until>UTC_TIMESTAMP(6)").bind(id).bind(owner).fetch_one(pool).await?;
        claims.push(TriggerClaim {
            binding_id: id,
            tenant_id: row.try_get("tenant_id")?,
            application_id: row.try_get("application_id")?,
            bundle_id: row.try_get("bundle_id")?,
            revision: row.try_get("configuration_revision")?,
            kind: row.try_get("trigger_kind")?,
            configuration: serde_json::from_value(row.try_get("configuration_json")?)
                .map_err(|e| RuntimeError::Internal(e.into()))?,
            cursor: row.try_get("cursor_value")?,
            due_at: row
                .try_get::<Option<time::OffsetDateTime>, _>("next_poll_at")?
                .map(|value| {
                    DateTime::from_timestamp(value.unix_timestamp(), value.nanosecond())
                        .expect("valid timestamp")
                }),
            owner,
            fencing_token: row.try_get("fencing_token")?,
        });
    }
    Ok(claims)
}

pub async fn heartbeat(pool: &sqlx::MySqlPool, claim: &TriggerClaim) -> RuntimeResult<()> {
    let changed=sqlx::query("UPDATE trigger_bindings SET locked_until=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 30 SECOND),heartbeat_at=UTC_TIMESTAMP(6) WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(claim.binding_id).bind(claim.owner).bind(claim.fencing_token).execute(pool).await?;
    if changed.rows_affected() != 1 {
        return Err(lease_lost());
    }
    Ok(())
}

pub async fn execute_with_heartbeat(
    pool: &sqlx::MySqlPool,
    claim: &TriggerClaim,
) -> RuntimeResult<()> {
    let execution = execute(pool, claim);
    tokio::pin!(execution);
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(10));
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first interval tick is immediate; consume it so heartbeats remain
    // ten seconds apart and do not extend a claim before work starts.
    heartbeat_interval.tick().await;
    loop {
        tokio::select! {
            result = &mut execution => return result,
            _ = heartbeat_interval.tick() => heartbeat(pool, claim).await?,
        }
    }
}

pub async fn execute(pool: &sqlx::MySqlPool, claim: &TriggerClaim) -> RuntimeResult<()> {
    let (input, idempotency, cursor, next) = match &claim.configuration.configuration {
        RuntimeTriggerConfigurationV1::Schedule {
            cron_expression,
            timezone,
            misfire_policy,
            grace_seconds,
            input,
            ..
        } => {
            let due = claim.due_at.ok_or_else(|| {
                RuntimeError::Internal(anyhow::anyhow!(
                    "claimed Schedule is missing its due instant"
                ))
            })?;
            let now = mysql_now(pool).await?;
            let previous = claim
                .cursor
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc));
            let (should_fire, cursor_instant, next_after) =
                schedule_plan(due, now, previous, misfire_policy, *grace_seconds);
            let next = next_schedule(cron_expression, timezone, next_after)?;
            (
                should_fire.then_some(input.clone()),
                format!("schedule:{}:{}", claim.binding_id, due.timestamp_micros()),
                Some(cursor_instant.to_rfc3339()),
                Some(next),
            )
        }
        RuntimeTriggerConfigurationV1::Webhook { .. } => {
            return complete(pool, claim, None, None).await;
        }
    };
    let mut tx = pool.begin().await?;
    let current_revision: Option<u64> = sqlx::query_scalar(
        "SELECT configuration_revision FROM trigger_bindings WHERE id=? FOR UPDATE",
    )
    .bind(claim.binding_id)
    .fetch_optional(&mut *tx)
    .await?;
    if current_revision != Some(claim.revision) {
        return Err(lease_lost());
    }
    if let Some(input) = input {
        create_runtime_invocation_tx(
            &mut tx,
            claim.tenant_id,
            claim.application_id,
            InvocationCaller {
                wait_inputs: match &claim.configuration.configuration {
                    RuntimeTriggerConfigurationV1::Schedule { wait_inputs, .. } => {
                        Some(wait_inputs.clone())
                    }
                    _ => None,
                },
                trigger_context: None,
                end_user: None,
                caller_type: match claim.kind.as_str() {
                    "schedule" => "schedule",
                    _ => "webhook",
                },
                caller_id: claim.binding_id,
                token_version: None,
                origin: agentx_runtime_contracts::ExecutionOriginV1 {
                    trigger_source_id: Some(claim.binding_id),
                    trigger_name: Some(claim.configuration.trigger_name.clone()),
                    ..agentx_runtime_contracts::ExecutionOriginV1::system(None)
                },
            },
            None,
            &input,
            &idempotency,
        )
        .await?;
    }
    complete_tx(&mut tx, claim, cursor, next).await?;
    tx.commit().await?;
    Ok(())
}

async fn complete(
    pool: &sqlx::MySqlPool,
    claim: &TriggerClaim,
    cursor: Option<String>,
    next: Option<DateTime<Utc>>,
) -> RuntimeResult<()> {
    let mut tx = pool.begin().await?;
    complete_tx(&mut tx, claim, cursor, next).await?;
    tx.commit().await?;
    Ok(())
}

async fn complete_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    claim: &TriggerClaim,
    cursor: Option<String>,
    next: Option<DateTime<Utc>>,
) -> RuntimeResult<()> {
    let next =
        next.and_then(|value| time::OffsetDateTime::from_unix_timestamp(value.timestamp()).ok());
    let changed=sqlx::query("UPDATE trigger_bindings SET cursor_value=COALESCE(?,cursor_value),last_poll_at=UTC_TIMESTAMP(6),next_poll_at=?,locked_by=NULL,locked_until=NULL,heartbeat_at=NULL,last_error=NULL WHERE id=? AND locked_by=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6)")
        .bind(cursor).bind(next).bind(claim.binding_id).bind(claim.owner).bind(claim.fencing_token).execute(&mut **tx).await?;
    if changed.rows_affected() != 1 {
        return Err(lease_lost());
    }
    Ok(())
}
fn lease_lost() -> RuntimeError {
    RuntimeError::Conflict(
        agentx_runtime_contracts::RuntimePublishErrorCodeV1::IdempotencyConflict,
        "LeaseLost".into(),
    )
}
pub(crate) fn next_schedule(
    expression: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> RuntimeResult<DateTime<Utc>> {
    let timezone: Tz = timezone.parse().map_err(|_| {
        RuntimeError::BadRequest(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::UnsupportedCapability,
            "Invalid IANA timezone".into(),
        )
    })?;
    let normalized = normalize_cron(expression)?;
    let schedule: Schedule = normalized.parse().map_err(|_| {
        RuntimeError::BadRequest(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::UnsupportedCapability,
            "Invalid cron expression".into(),
        )
    })?;
    let local = timezone
        .timestamp_opt(after.timestamp(), 0)
        .single()
        .ok_or_else(|| RuntimeError::Internal(anyhow::anyhow!("invalid timestamp")))?;
    for candidate in schedule.after(&local).take(4096) {
        let selected = match timezone.from_local_datetime(&candidate.naive_local()) {
            LocalResult::Single(value) => Some(value),
            // During a DST fall-back, only the first physical instant for the
            // repeated wall-clock time is a valid Agentx schedule instant.
            LocalResult::Ambiguous(first, second) => Some(first.min(second)),
            // A nonexistent wall-clock time during a DST spring-forward is
            // skipped instead of being shifted to a different local time.
            LocalResult::None => None,
        };
        if selected.is_some_and(|value| value == candidate) {
            return Ok(candidate.with_timezone(&Utc));
        }
    }
    Err(RuntimeError::Internal(anyhow::anyhow!(
        "Schedule has no next supported instant"
    )))
}

fn normalize_cron(expression: &str) -> RuntimeResult<String> {
    match expression.split_whitespace().count() {
        5 => Ok(format!("0 {expression}")),
        6 | 7 => Ok(expression.to_owned()),
        _ => Err(RuntimeError::BadRequest(
            agentx_runtime_contracts::RuntimePublishErrorCodeV1::UnsupportedCapability,
            "Cron expression must contain five, six or seven fields".into(),
        )),
    }
}

fn schedule_plan(
    due: DateTime<Utc>,
    now: DateTime<Utc>,
    previous: Option<DateTime<Utc>>,
    policy: &ScheduleMisfirePolicyV1,
    grace_seconds: u32,
) -> (bool, DateTime<Utc>, DateTime<Utc>) {
    let is_new_instant = previous.is_none_or(|previous| due > previous);
    let lateness = now.signed_duration_since(due).num_seconds().max(0);
    let should_fire = is_new_instant
        && (lateness <= i64::from(grace_seconds) || *policy == ScheduleMisfirePolicyV1::FireOnce);
    let cursor = previous.map_or(due, |previous| previous.max(due));
    (should_fire, cursor, now.max(cursor))
}

async fn mysql_now(pool: &sqlx::MySqlPool) -> RuntimeResult<DateTime<Utc>> {
    let value: time::OffsetDateTime = sqlx::query_scalar("SELECT UTC_TIMESTAMP(6)")
        .fetch_one(pool)
        .await?;
    DateTime::from_timestamp(value.unix_timestamp(), value.nanosecond())
        .ok_or_else(|| RuntimeError::Internal(anyhow::anyhow!("MySQL returned an invalid time")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule_uses_iana_timezone() {
        assert!(next_schedule("0 0 * * * * *", "Asia/Shanghai", Utc::now()).is_ok());
    }
    #[test]
    fn five_field_cron_is_normalized() {
        let after = "2026-08-13T00:00:00Z".parse().unwrap();
        assert_eq!(
            next_schedule("0 9 * * 1", "Asia/Shanghai", after).unwrap(),
            "2026-08-16T01:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }
    #[test]
    fn nonexistent_dst_instant_is_skipped() {
        let after = "2026-03-08T05:00:00Z".parse().unwrap();
        assert_eq!(
            next_schedule("30 2 * * *", "America/New_York", after).unwrap(),
            "2026-03-09T06:30:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }
    #[test]
    fn repeated_dst_instant_only_uses_the_first_occurrence() {
        let before = "2026-11-01T04:00:00Z".parse().unwrap();
        let after_first = "2026-11-01T05:30:00Z".parse().unwrap();
        assert_eq!(
            next_schedule("30 1 * * *", "America/New_York", before).unwrap(),
            "2026-11-01T05:30:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(
            next_schedule("30 1 * * *", "America/New_York", after_first).unwrap(),
            "2026-11-02T06:30:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }
    #[test]
    fn schedule_never_moves_behind_its_cursor_when_the_clock_moves_back() {
        let cursor = "2026-08-13T09:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let wall_clock_after_rollback = "2026-08-13T08:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let anchor = cursor.max(wall_clock_after_rollback);
        assert!(next_schedule("0 * * * *", "UTC", anchor).unwrap() > cursor);
    }
    #[test]
    fn skip_drops_a_misfire_beyond_grace_and_advances_the_cursor() {
        let due = "2026-08-13T09:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let now = "2026-08-13T09:01:01Z".parse::<DateTime<Utc>>().unwrap();
        let (fire, cursor, next_after) =
            schedule_plan(due, now, None, &ScheduleMisfirePolicyV1::Skip, 60);
        assert!(!fire);
        assert_eq!(cursor, due);
        assert_eq!(next_after, now);
    }
    #[test]
    fn fire_once_collapses_any_number_of_missed_instants() {
        let due = "2026-08-12T09:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let now = "2026-08-13T09:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let (fire, cursor, next_after) =
            schedule_plan(due, now, None, &ScheduleMisfirePolicyV1::FireOnce, 60);
        assert!(fire);
        assert_eq!(cursor, due);
        assert_eq!(next_after, now);
    }
    #[test]
    fn a_clock_rollback_cannot_replay_the_cursor_instant() {
        let cursor = "2026-08-13T09:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let rolled_back_now = "2026-08-13T08:59:00Z".parse::<DateTime<Utc>>().unwrap();
        let (fire, retained_cursor, next_after) = schedule_plan(
            cursor,
            rolled_back_now,
            Some(cursor),
            &ScheduleMisfirePolicyV1::FireOnce,
            60,
        );
        assert!(!fire);
        assert_eq!(retained_cursor, cursor);
        assert_eq!(next_after, cursor);
    }
    #[test]
    fn invalid_timezone_is_rejected() {
        assert!(next_schedule("0 0 * * * * *", "not-a-zone", Utc::now()).is_err());
    }
}
