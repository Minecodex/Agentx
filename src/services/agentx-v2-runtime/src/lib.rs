pub mod admission;
pub mod agent_session_queue;
pub mod artifact;
mod assistant_message;
pub mod auth;
mod composite;
pub mod composite_execution;
mod debug_overlay;
pub mod delivery;
pub mod delivery_query;
pub mod delivery_send;
pub mod egress;
pub mod engine;
mod engine_names;
mod engine_persistence;
mod engine_protocol;
mod engine_trace;
pub mod error;
pub mod event_export;
pub mod execution;
mod execution_context;
mod fork_runtime;
pub mod gateway;
pub mod gc;
pub mod internal_engine;
pub mod object_upload;
mod output_contract;
pub mod plugin_design;
pub mod provider_breaker;
pub mod publish;
pub mod query;
mod query_authority;
pub mod quota;
pub mod rate_limit;
pub mod redis_admission;
pub mod resource_check;
pub mod retention;
pub mod sandbox;
pub mod sse_wakeup;
pub mod stream;
mod suspension;
mod trace_artifact;
pub use suspension::enqueue_due as enqueue_due_approval_timeouts;
pub mod trace_delivery;
pub mod trigger;
pub mod vault;
pub mod webhook;
mod work_package_execution;
mod worker_registry;
pub mod worker_runtime;
mod worker_runtime_calls;
pub mod worker_runtime_delta;
pub mod worker_support;

use std::sync::Arc;

use agentx_runtime_infrastructure::{
    RuntimeMySqlSettings, RuntimeObjectStorageSettings, connect_runtime_mysql, runtime_object_store,
};
use anyhow::Result;
use object_store::ObjectStore;
use sqlx::MySqlPool;

use crate::auth::RuntimeTrust;
use crate::vault::RuntimeVault;

#[derive(Clone)]
pub struct RuntimeState {
    pub pool: MySqlPool,
    pub objects: Arc<dyn ObjectStore>,
    pub trust: Arc<RuntimeTrust>,
    pub wakeups: crate::sse_wakeup::SseWakeup,
    pub vault: Option<RuntimeVault>,
    pub admission_redis: Option<redis::aio::ConnectionManager>,
}

impl RuntimeState {
    pub async fn from_env() -> Result<Self> {
        Self::gateway_from_env().await
    }

    pub async fn gateway_from_env() -> Result<Self> {
        let mysql = RuntimeMySqlSettings::from_env()?;
        let object_storage = RuntimeObjectStorageSettings::from_env()?;
        Ok(Self {
            pool: connect_runtime_mysql(&mysql).await?,
            objects: runtime_object_store(&object_storage)?,
            trust: Arc::new(RuntimeTrust::from_env()?),
            wakeups: crate::sse_wakeup::SseWakeup::from_env()?,
            vault: Some(RuntimeVault::from_env()?),
            admission_redis: Some(
                agentx_runtime_infrastructure::connect_runtime_redis(
                    &agentx_runtime_infrastructure::RuntimeRedisSettings::from_env()?,
                )
                .await?,
            ),
        })
    }

    pub async fn maintenance_from_env() -> Result<Self> {
        let mysql = RuntimeMySqlSettings::from_env()?;
        let object_storage = RuntimeObjectStorageSettings::from_env()?;
        Ok(Self {
            pool: connect_runtime_mysql(&mysql).await?,
            objects: runtime_object_store(&object_storage)?,
            trust: Arc::new(RuntimeTrust::from_env_without_user_keys()?),
            wakeups: crate::sse_wakeup::SseWakeup::disabled(),
            // The delivery role rides the maintenance state and reads frozen
            // channel credentials; a missing Vault fails its startup check
            // explicitly instead of degrading to a crash loop.
            vault: RuntimeVault::from_env().ok(),
            admission_redis: None,
        })
    }
}
