//! Durable collection coordination. Execution remains owned by formal Invocations.
use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
    execution::InvocationCaller,
};
use agentx_runtime_contracts::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{MySql, Row, Transaction};
use uuid::Uuid;

pub(crate) mod capacity;
mod create;
mod dispatch;
mod files;
pub mod internal;
mod operations;
mod read;
pub(crate) mod retention;
pub use create::create;
pub use dispatch::process;
pub use dispatch::run_once;
pub use files::{download_export, get_export, parse_table, request_export};
pub use operations::{cancel, rerun, stop};
pub use read::{get, items, list};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchCaller {
    pub tenant_id: Uuid,
    pub application_id: Uuid,
    pub caller_type: String,
    pub caller_id: Uuid,
    pub token_version: Option<u64>,
    pub origin: ExecutionOriginV1,
}
impl BatchCaller {
    fn kind(&self) -> RuntimeResult<&'static str> {
        match self.caller_type.as_str() {
            "user" => Ok("user"),
            "api_key" => Ok("api_key"),
            _ => Err(RuntimeError::Unauthorized),
        }
    }
    fn invocation(
        &self,
        access: Option<ApplicationAccessBindingV1>,
        identity: Option<Value>,
    ) -> RuntimeResult<InvocationCaller> {
        Ok(InvocationCaller {
            caller_type: self.kind()?,
            caller_id: self.caller_id,
            token_version: self.token_version,
            origin: self.origin.clone(),
            access,
            end_user: identity
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| RuntimeError::Internal(e.into()))?,
            wait_inputs: None,
            trigger_context: None,
        })
    }
    fn task_caller(&self) -> RuntimeResult<crate::application_caller::ApplicationCaller> {
        Ok(crate::application_caller::ApplicationCaller {
            tenant_id: self.tenant_id,
            application_id: self.application_id,
            subject_id: self.caller_id,
            caller_type: self.kind()?,
            token_version: self.token_version,
            origin: self.origin.clone(),
        })
    }
}

/// Management scopes are constructed only after signed object-scope delegation checks.
#[derive(Clone)]
pub struct BatchScope {
    pub caller: BatchCaller,
    pub management: bool,
}
impl BatchScope {
    pub fn owner(caller: BatchCaller) -> Self {
        Self {
            caller,
            management: false,
        }
    }
    fn authorize(&self, row: &sqlx::mysql::MySqlRow) -> RuntimeResult<()> {
        if row.try_get::<Uuid, _>("tenant_id")? != self.caller.tenant_id
            || row.try_get::<Uuid, _>("application_id")? != self.caller.application_id
            || (!self.management
                && (row.try_get::<String, _>("caller_type")? != self.caller.caller_type
                    || row.try_get::<Uuid, _>("caller_id")? != self.caller.caller_id))
        {
            return Err(RuntimeError::NotFound);
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidRequest("BATCH_REQUEST_INVALID", message.into())
}
fn conflict(message: impl Into<String>) -> RuntimeError {
    RuntimeError::StateConflict("BATCH_CONFLICT", message.into())
}
fn hash(value: &impl Serialize) -> RuntimeResult<String> {
    Ok(content_hash(value)
        .map_err(|e| RuntimeError::Internal(e.into()))?
        .as_str()
        .to_owned())
}
fn binding(row: &sqlx::mysql::MySqlRow) -> RuntimeResult<Option<ApplicationAccessBindingV1>> {
    row.try_get::<Option<Value>, _>("access_binding_json")?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| RuntimeError::Internal(e.into()))
}
fn owner(row: &sqlx::mysql::MySqlRow) -> RuntimeResult<BatchCaller> {
    serde_json::from_value(row.try_get("caller_json")?)
        .map_err(|e| RuntimeError::Internal(e.into()))
}
async fn row(
    tx: &mut Transaction<'_, MySql>,
    scope: &BatchScope,
    id: Uuid,
    lock: bool,
) -> RuntimeResult<sqlx::mysql::MySqlRow> {
    let sql = if lock {
        "SELECT * FROM application_batches WHERE tenant_id=? AND application_id=? AND id=? AND retention_deleted_at IS NULL FOR UPDATE"
    } else {
        "SELECT * FROM application_batches WHERE tenant_id=? AND application_id=? AND id=? AND retention_deleted_at IS NULL"
    };
    let row = sqlx::query(sql)
        .bind(scope.caller.tenant_id)
        .bind(scope.caller.application_id)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RuntimeError::NotFound)?;
    scope.authorize(&row)?;
    Ok(row)
}
async fn audit(
    tx: &mut Transaction<'_, MySql>,
    scope: &BatchScope,
    id: Uuid,
    action: &str,
    detail: Value,
) -> RuntimeResult<()> {
    sqlx::query("INSERT INTO audit_events(id,tenant_id,actor_user_id,action,target_type,target_id,request_id,detail_json) VALUES(?,?,?,?,'application_batch',?,?,?)")
        .bind(Uuid::now_v7()).bind(scope.caller.tenant_id).bind((scope.caller.caller_type=="user").then_some(scope.caller.caller_id)).bind(action).bind(id.to_string()).bind(Uuid::now_v7()).bind(detail).execute(&mut **tx).await?;
    Ok(())
}
