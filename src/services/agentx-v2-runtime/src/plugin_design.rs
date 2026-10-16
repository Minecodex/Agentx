use agentx_runtime_contracts::{
    PluginDesignOperationRequestV1, PluginDesignOperationResponseV1, PluginResolveSessionV1,
};
use axum::{Json, extract::State, http::HeaderMap};

use crate::{
    RuntimeState,
    error::{RuntimeError, RuntimeResult},
};

pub async fn execute(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<PluginDesignOperationRequestV1>,
) -> RuntimeResult<Json<PluginDesignOperationResponseV1>> {
    state.trust.publisher(&headers, "runtime.plugins.design")?;
    if request.resolve_session_id.is_some() && request.method != "node.resolveDefinition" {
        return Err(RuntimeError::InvalidRequest(
            "RESOLVE_ONLY_METHOD_INVALID",
            "Resolve sessions cannot invoke providers or business methods".into(),
        ));
    }
    if !matches!(
        request.method.as_str(),
        "node.resolveDefinition" | "node.invokeProvider"
    ) {
        return Err(RuntimeError::InvalidRequest(
            "PLUGIN_DESIGN_METHOD_INVALID",
            "Plugin design method is unsupported".into(),
        ));
    }
    if request.plugin.package_id.trim().is_empty()
        || request.plugin.runtime_source.trim().is_empty()
        || !request.plugin.bundle_digest.starts_with("sha256:")
    {
        return Err(RuntimeError::InvalidRequest(
            "PLUGIN_BINDING_INVALID",
            "Plugin design operation requires an immutable binding".into(),
        ));
    }
    if request.method == "node.resolveDefinition" {
        if !request.resources.is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "RESOLVE_ONLY_RESOURCES_DENIED",
                "Definition resolution cannot receive resource bindings".into(),
            ));
        }
        let endpoint = std::env::var("AGENTX_SANDBOX_MANAGER_ENDPOINT")
            .map_err(|_| RuntimeError::Unavailable)?;
        let client = crate::egress::ProviderHttpClient::from_env(
            agentx_runtime_contracts::EgressRole::RuntimeGateway,
        )
        .map_err(RuntimeError::Internal)?;
        let response = client
            .post_sandbox_manager(
                &format!("{endpoint}/internal/runtime/v1/sandboxes:resolve"),
                std::time::Duration::from_secs(55),
            )
            .map_err(RuntimeError::Internal)?
            .json(&request)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(operation_id=%request.operation_id, %error, "Resolve-only Sandbox Manager request failed");
                RuntimeError::Unavailable
            })?;
        if !response.status().is_success() {
            tracing::warn!(operation_id=%request.operation_id, status=%response.status(), "Resolve-only Sandbox Manager rejected request");
            return Err(RuntimeError::InvalidRequest(
                "RESOLVE_ONLY_UNAVAILABLE",
                "Isolated definition resolution was rejected or is unavailable".into(),
            ));
        }
        return response
            .json::<PluginDesignOperationResponseV1>()
            .await
            .map(Json)
            .map_err(|_| RuntimeError::Unavailable);
    }
    let worker =
        crate::worker_runtime::RuntimeWorker::new(state.pool.clone(), state.objects.clone())
            .map_err(RuntimeError::Internal)?;
    let result = crate::worker_runtime::plugin::invoke_design_operation(
        &worker,
        request.tenant_id,
        request.operation_id,
        &request.resources,
        &request.plugin,
        &request.method,
        request.parameters,
    )
    .await
    .map_err(|message| RuntimeError::InvalidRequest("PLUGIN_DESIGN_OPERATION_FAILED", message))?;
    Ok(Json(PluginDesignOperationResponseV1 {
        protocol_version: 1,
        operation_id: request.operation_id,
        result,
    }))
}

pub async fn close_session(
    State(state): State<RuntimeState>,
    headers: HeaderMap,
    Json(request): Json<PluginResolveSessionV1>,
) -> RuntimeResult<Json<PluginResolveSessionV1>> {
    state.trust.publisher(&headers, "runtime.plugins.design")?;
    let endpoint =
        std::env::var("AGENTX_SANDBOX_MANAGER_ENDPOINT").map_err(|_| RuntimeError::Unavailable)?;
    let client = crate::egress::ProviderHttpClient::from_env(
        agentx_runtime_contracts::EgressRole::RuntimeGateway,
    )
    .map_err(RuntimeError::Internal)?;
    let response = client
        .post_sandbox_manager(
            &format!("{endpoint}/internal/runtime/v1/sandboxes:close-resolve-session"),
            std::time::Duration::from_secs(15),
        )
        .map_err(RuntimeError::Internal)?
        .json(&request)
        .send()
        .await
        .map_err(|_| RuntimeError::Unavailable)?;
    if !response.status().is_success() {
        return Err(RuntimeError::Unavailable);
    }
    response
        .json()
        .await
        .map(Json)
        .map_err(|_| RuntimeError::Unavailable)
}
