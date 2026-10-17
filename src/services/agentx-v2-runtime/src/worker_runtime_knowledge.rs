use super::{
    ClaimedWorkerAttempt, RuntimeResourceKindV1, RuntimeWorker, WorkerExecution, successful_value,
};
use agentx_runtime_contracts::{
    RuntimeResourceBindingV1, RuntimeResourceConfigurationV1,
    rag::{
        RagProtocolError, RagScope, rag_query_request, validate_instance_scope,
        validate_retrieval_input,
    },
    retrieval::{QueryScope, resolve_sources, retrieve},
};
use serde_json::{Value, json};

impl RuntimeWorker {
    pub(super) async fn execute_knowledge_retrieval(
        &self,
        claim: &ClaimedWorkerAttempt,
    ) -> WorkerExecution {
        let bindings = claim
            .resources
            .iter()
            .filter(|binding| binding.resource_kind == RuntimeResourceKindV1::Rag)
            .collect::<Vec<_>>();
        self.execute_knowledge_queries(claim, &bindings, &claim.node_parameters, 0)
            .await
    }

    pub(super) async fn execute_knowledge_queries(
        &self,
        claim: &ClaimedWorkerAttempt,
        bindings: &[&RuntimeResourceBindingV1],
        input: &Value,
        call_index: u32,
    ) -> WorkerExecution {
        let query = input["query"].as_str().unwrap_or_default();
        let top_k = input
            .get("topK")
            .map_or(Some(5), Value::as_u64)
            .unwrap_or(0);
        if let Err(error) = validate_retrieval_input(query, top_k) {
            return WorkerExecution::failed(error.code, error.message, false);
        }
        // Preflight the complete frozen grant and credential package. An
        // authorization failure must happen before contacting any source.
        let authorization = async {
            let mut tx = self.pool.begin().await?;
            let snapshot: Value = sqlx::query_scalar("SELECT authorization_snapshot_json FROM execution_snapshots WHERE tenant_id=? AND execution_id=?").bind(claim.task.tenant_id).bind(claim.task.execution_id).fetch_one(&mut *tx).await?;
            let snapshot = serde_json::from_value(snapshot)?;
            crate::engine_persistence::authorize_snapshot(&mut tx, &snapshot).await.map_err(|e|anyhow::anyhow!("{e}"))?;
            crate::engine_persistence::authorize_resources(&mut tx, claim.task.tenant_id, &claim.resources).await.map_err(|e|anyhow::anyhow!("{e}"))?;
            Ok::<_, anyhow::Error>(())
        }.await;
        if let Err(error) = authorization {
            return WorkerExecution::failed(
                "KNOWLEDGE_AUTHORIZATION_FAILED",
                error.to_string(),
                false,
            );
        }
        let queries = bindings
            .iter()
            .filter_map(|binding| match &binding.configuration {
                RuntimeResourceConfigurationV1::Rag {
                    connection_id,
                    connection_version,
                    scope,
                    ..
                } => Some(QueryScope {
                    query_id: binding.resource_id,
                    connection_id: *connection_id,
                    connection_version: *connection_version,
                    scope: scope.clone(),
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        let sources = match resolve_sources(&queries) {
            Ok(sources) => sources,
            Err(e) => return WorkerExecution::failed(e.code, e.message, false),
        };
        let result = retrieve(sources, top_k as usize, |index, source| async move {
            let binding = bindings
                .iter()
                .find(|b| source.query_ids.contains(&b.resource_id))
                .expect("resolved binding");
            let RuntimeResourceConfigurationV1::Rag {
                endpoint,
                credential,
                provider,
                ..
            } = &binding.configuration
            else {
                unreachable!()
            };
            source.scope.validate(provider)?;
            let index = call_index + index as u32 * 2;
            if let RagScope::Lightrag { .. } = &source.scope {
                let response = self
                    .call_http(
                        claim,
                        "rag_health",
                        &format!("{}/health", endpoint.trim_end_matches('/')),
                        json!({"method":"GET"}),
                        index,
                        credential.as_ref(),
                        "x-api-key",
                        Some(binding),
                    )
                    .await;
                let health = native_response(response)?;
                validate_instance_scope(&source.scope, &health)?;
            }
            let (path, body, header) = rag_query_request(&source.scope, query)?;
            native_response(
                self.call_http(
                    claim,
                    "rag",
                    &format!("{}/{}", endpoint.trim_end_matches('/'), path),
                    body,
                    index + 1,
                    credential.as_ref(),
                    header,
                    Some(binding),
                )
                .await,
            )
        })
        .await;
        match result {
            Ok(value) => WorkerExecution::succeeded(value),
            Err(error) => {
                let mut failure = WorkerExecution::failed(error.code, error.message, false);
                failure.outputs.insert(
                    "error".into(),
                    vec![agentx_node_protocol::Item {
                        json: json!({"sources":error.sources}),
                        ..Default::default()
                    }],
                );
                failure
            }
        }
    }
}
fn native_response(execution: WorkerExecution) -> Result<Value, RagProtocolError> {
    successful_value(&execution).ok_or_else(|| {
        RagProtocolError::new(
            "PROVIDER_REQUEST_FAILED",
            format!(
                "{}: {}",
                execution.error_code.unwrap_or_default(),
                execution
                    .error_message
                    .unwrap_or_else(|| "Provider returned no result".into())
            ),
        )
    })
}
