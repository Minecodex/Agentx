use std::collections::{BTreeSet, HashMap};

use agentx_runtime_contracts::{ApplicationUserIdentityV1, ApplicationUserSourceV1};
use schemars::JsonSchema;
use serde::Serialize;
use sqlx::{MySql, QueryBuilder, Row};
use uuid::Uuid;

use crate::{
    api_error::{ApiError, ApiResult},
    control_api::ControlApiState,
};

pub(crate) type UserNames = HashMap<Uuid, String>;

/// Control-owned presentation data; never persisted as a Runtime or memory identity.
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EndUserResponse {
    source: ApplicationUserSourceV1,
    source_id: String,
    external_user_id: String,
    display_name: Option<String>,
}

pub(crate) async fn user_names<'a>(
    state: &ControlApiState,
    tenant: Uuid,
    users: impl IntoIterator<Item = &'a ApplicationUserIdentityV1>,
) -> ApiResult<UserNames> {
    let ids: BTreeSet<_> = users.into_iter().filter_map(platform_user_id).collect();
    if ids.is_empty() {
        return Ok(UserNames::new());
    }
    let mut query =
        QueryBuilder::<MySql>::new("SELECT id,display_name FROM users WHERE tenant_id=");
    query.push_bind(tenant).push(" AND id IN (");
    let mut separated = query.separated(",");
    for id in ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(")");
    query
        .build()
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(|row| Ok((row.try_get("id")?, row.try_get("display_name")?)))
        .collect()
}

pub(crate) fn user_response(user: ApplicationUserIdentityV1, names: &UserNames) -> EndUserResponse {
    let display_name = platform_user_id(&user).and_then(|id| names.get(&id).cloned());
    EndUserResponse {
        source: user.source,
        source_id: user.source_id,
        external_user_id: user.external_user_id,
        display_name,
    }
}

pub(crate) async fn memory_target_names(
    state: &ControlApiState,
    tenant: Uuid,
    mut response: serde_json::Value,
) -> ApiResult<serde_json::Value> {
    if let Some(targets) = response
        .get_mut("memoryTargets")
        .and_then(serde_json::Value::as_array_mut)
    {
        let users = targets
            .iter()
            .map(|target| {
                serde_json::from_value::<ApplicationUserIdentityV1>(target["scope"]["user"].clone())
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(ApiError::internal)?;
        let names = user_names(state, tenant, &users).await?;
        for (target, user) in targets.iter_mut().zip(users) {
            target["endUser"] =
                serde_json::to_value(user_response(user, &names)).map_err(ApiError::internal)?;
        }
    }
    Ok(response)
}

fn platform_user_id(user: &ApplicationUserIdentityV1) -> Option<Uuid> {
    (user.source == ApplicationUserSourceV1::Platform)
        .then(|| Uuid::parse_str(&user.external_user_id).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn platform_names_do_not_change_identity_or_name_external_declarations() {
        let id = Uuid::from_u128(42);
        let names = UserNames::from([(id, "调用人".into())]);
        let platform = ApplicationUserIdentityV1::platform(id);
        let original = serde_json::to_value(&platform).unwrap();
        let view = serde_json::to_value(user_response(platform, &names)).unwrap();
        assert_eq!(view["displayName"], "调用人");
        for key in ["source", "sourceId", "externalUserId"] {
            assert_eq!(view[key], original[key]);
        }
        let api = ApplicationUserIdentityV1::api(&json!({"user":id}), Some("user")).unwrap();
        let channel = ApplicationUserIdentityV1::channel("feishu", "bot", &id.to_string()).unwrap();
        for user in [api, channel] {
            assert!(
                serde_json::to_value(user_response(user, &names)).unwrap()["displayName"].is_null()
            );
        }
        let missing = user_response(ApplicationUserIdentityV1::platform(id), &UserNames::new());
        assert!(serde_json::to_value(missing).unwrap()["displayName"].is_null());
    }
}
