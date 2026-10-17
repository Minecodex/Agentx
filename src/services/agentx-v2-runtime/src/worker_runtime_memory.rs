use super::*;

impl RuntimeWorker {
    pub(super) async fn call_memory(
        &self,
        claim: &ClaimedWorkerAttempt,
        binding: &RuntimeResourceBindingV1,
        input: Value,
        call_index: u32,
    ) -> WorkerExecution {
        let RuntimeResourceConfigurationV1::Memory {
            endpoint,
            credential,
        } = &binding.configuration
        else {
            unreachable!("Memory binding")
        };
        let secret_header = match memory_secret_header(&claim.resources, credential.as_ref()) {
            Ok(header) => header,
            Err(code) => {
                return WorkerExecution::failed(
                    code,
                    "Memory requires a frozen api_key or bearer Credential",
                    false,
                );
            }
        };
        let memory_scope = match crate::user_identity::memory_scope(
            &self.pool,
            claim.task.tenant_id,
            claim.task.execution_id,
            &claim.node_key,
        )
        .await
        {
            Ok(Some(scope)) => scope,
            Ok(None) => {
                return WorkerExecution::failed(
                    "MEMORY_USER_ID_UNAVAILABLE",
                    "Personal memory requires a supplied user ID",
                    false,
                );
            }
            Err(error) => {
                return WorkerExecution::failed(
                    "MEMORY_SCOPE_UNAVAILABLE",
                    error.to_string(),
                    false,
                );
            }
        };
        let namespace = memory_scope.user_id();
        let operation = claim
            .node_parameters
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or("search");
        let input = claim.node_parameters.get("input").cloned().unwrap_or(input);
        let (path, request) = if operation == "add" || operation == "write" {
            let content = input.get("text").cloned().unwrap_or_else(|| input.clone());
            (
                "memories",
                json!({
                    "messages":[{"role":"user","content":content}],
                    "user_id":namespace
                }),
            )
        } else {
            let query = input.get("query").cloned().unwrap_or_else(|| input.clone());
            let top_k = input.get("top_k").and_then(Value::as_u64).unwrap_or(5);
            (
                "search",
                json!({"query":query,"filters":{"user_id":namespace},"top_k":top_k}),
            )
        };
        let cleared =
            match crate::memory_audit::is_cleared(&self.pool, &memory_scope, binding.resource_id)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    return WorkerExecution::failed(
                        "MEMORY_AUDIT_UNAVAILABLE",
                        error.to_string(),
                        true,
                    );
                }
            };
        if cleared {
            return if path == "memories" {
                WorkerExecution::failed(
                    "AGENT_LONG_TERM_MEMORY_CLEARED",
                    "This personal memory scope has been cleared",
                    false,
                )
            } else {
                memory_execution_output(WorkerExecution::succeeded(
                    json!({"results":[],"cleared":true}),
                ))
            };
        }
        let execution = self
            .call_http(
                claim,
                "memory",
                &format!("{}/{}", endpoint.trim_end_matches('/'), path),
                request,
                call_index,
                credential.as_ref(),
                secret_header,
                Some(binding),
            )
            .await;
        if let Err(error) = crate::memory_audit::record(
            &self.pool,
            &memory_scope,
            binding.resource_id,
            if path == "memories" {
                "write"
            } else {
                "recall"
            },
            &format!("memory:{}:{call_index}", claim.task.attempt_id),
        )
        .await
        {
            return WorkerExecution::failed("MEMORY_AUDIT_UNAVAILABLE", error.to_string(), false);
        }
        memory_execution_output(execution)
    }
}
