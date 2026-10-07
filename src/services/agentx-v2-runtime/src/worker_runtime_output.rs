use std::str::FromStr;

use agentx_runtime_contracts::{
    RuntimeModelPriceV1, RuntimeResourceConfigurationV1, WorkerResultStatusV1,
};
use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive as _};
use serde_json::{Value, json};

use super::{ClaimedWorkerAttempt, WorkerExecution, mcp_tool_binding, successful_value};

pub(super) fn openai_chat_request(
    claim: &ClaimedWorkerAttempt,
    model: &str,
    price: &RuntimeModelPriceV1,
    input: &Value,
) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = system_prompt(&claim.node_parameters, &claim.node_type) {
        messages.push(json!({"role":"system","content":system}));
    }
    let content = claim
        .node_parameters
        .get("userQuestion")
        .cloned()
        .or_else(|| input.get("question").cloned())
        .unwrap_or_else(|| input.clone());
    messages.push(json!({"role":"user","content":json_text(&content)}));
    if let Some(tool) = input.get("tool") {
        messages.push(json!({
            "role":"tool",
            "tool_call_id":"agentx-runtime-tool",
            "content":json_text(tool),
        }));
    }
    let mut request = json!({
        "model":model,
        "messages":messages,
        "stream":false,
        "metadata":{"priceVersion":price.version_id},
    });
    if let Some(tool) = mcp_tool_binding(&claim.resources)
        && let RuntimeResourceConfigurationV1::Mcp { tool_name, .. } = &tool.configuration
    {
        request["tools"] = json!([{
            "type":"function",
            "function":{
                "name":tool_name,
                "description":"Runtime-pinned MCP tool",
                "parameters":{"type":"object","additionalProperties":true},
            }
        }]);
    }
    if claim.node_type == "model"
        && claim
            .node_parameters
            .get("responseMode")
            .and_then(Value::as_str)
            == Some("json_schema")
        && let Some(schema) = claim.node_parameters.get("structuredSchema")
    {
        request["response_format"] = json!({
            "type":"json_schema",
            "json_schema":{"name":"agentx_response","strict":true,"schema":schema}
        });
    }
    request
}

pub(super) fn system_prompt<'a>(parameters: &'a Value, node_type: &str) -> Option<&'a str> {
    parameters
        .get(if node_type == "model" {
            "prompt"
        } else {
            "systemPrompt"
        })
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

pub(super) fn openai_execution_output(
    execution: WorkerExecution,
    parameters: &Value,
) -> WorkerExecution {
    if execution.status != WorkerResultStatusV1::Succeeded {
        return execution;
    }
    let Some(response) = successful_value(&execution) else {
        return WorkerExecution::failed(
            "PROVIDER_RESPONSE_INVALID",
            "OpenAI-compatible response is empty",
            false,
        );
    };
    let Some(message) = response.pointer("/choices/0/message") else {
        return WorkerExecution::failed(
            "PROVIDER_RESPONSE_INVALID",
            "OpenAI-compatible response has no assistant message",
            false,
        );
    };
    let usage = response.get("usage").cloned().unwrap_or_else(|| json!({}));
    let normalized_usage = json!({
        "inputTokens":usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
        "outputTokens":usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
        "totalTokens":usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        "costMicros":usage.get("costMicros").and_then(Value::as_u64).unwrap_or(0),
    });
    if let Some(arguments) = message
        .pointer("/tool_calls/0/function/arguments")
        .and_then(Value::as_str)
    {
        let arguments = serde_json::from_str(arguments).unwrap_or_else(|_| {
            json!({
                "value":arguments,
            })
        });
        return WorkerExecution::succeeded(json!({
            "toolCall":arguments,
            "usage":normalized_usage,
        }));
    }
    let content = message.get("content").cloned().unwrap_or(Value::Null);
    let structured_output =
        if parameters.get("responseMode").and_then(Value::as_str) == Some("json_schema") {
            let Some(text) = content.as_str() else {
                return WorkerExecution::failed(
                    "MODEL_STRUCTURED_OUTPUT_INVALID",
                    "Structured model response is not text JSON",
                    false,
                );
            };
            let Ok(value) = serde_json::from_str::<Value>(text) else {
                return WorkerExecution::failed(
                    "MODEL_STRUCTURED_OUTPUT_INVALID",
                    "Structured model response is not valid JSON",
                    false,
                );
            };
            let Some(schema) = parameters.get("structuredSchema") else {
                return WorkerExecution::failed(
                    "MODEL_STRUCTURED_SCHEMA_REQUIRED",
                    "structuredSchema is required for json_schema mode",
                    false,
                );
            };
            let Ok(validator) = jsonschema::validator_for(schema) else {
                return WorkerExecution::failed(
                    "MODEL_STRUCTURED_SCHEMA_INVALID",
                    "structuredSchema is not a valid JSON Schema",
                    false,
                );
            };
            if let Err(error) = validator.validate(&value) {
                return WorkerExecution::failed(
                    "MODEL_STRUCTURED_OUTPUT_INVALID",
                    error.to_string(),
                    false,
                );
            }
            value
        } else {
            Value::Null
        };
    // Keep the adapter payload identical to the Model manifest.  Downstream
    // selectors are validated against that contract, so aliases such as
    // `answer` and `finalAnswer` turn a successful provider call into an
    // unresolvable End value.
    WorkerExecution::succeeded(json!({
        "text":json_text(&content),
        "reasoningContent":Value::Null,
        "structuredOutput":structured_output,
        "citations":[],
        "files":[],
        "usage":normalized_usage,
        "finishReason":response.get("choices").and_then(|choices| choices.get(0)).and_then(|choice| choice.get("finish_reason")).cloned().unwrap_or(Value::Null),
        "partial":false,
    }))
}

fn json_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

pub(super) fn provider_usage_detail(value: &Value) -> (u64, u64, u64) {
    let usage = value.get("usage").unwrap_or(value);
    let input = usage
        .get("inputTokens")
        .or_else(|| usage.get("input_tokens"))
        .or_else(|| usage.get("promptTokens"))
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = usage
        .get("outputTokens")
        .or_else(|| usage.get("output_tokens"))
        .or_else(|| usage.get("completionTokens"))
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cost = usage
        .get("costMicros")
        .or_else(|| usage.get("cost_micros"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    (input, output, cost)
}

pub(super) fn apply_model_price(
    value: &mut Value,
    price: &RuntimeModelPriceV1,
) -> Result<(u64, u64, u64), String> {
    let (input_tokens, output_tokens, _) = provider_usage_detail(value);
    let input_price = Decimal::from_str(&price.input_per_million)
        .map_err(|error| format!("Invalid frozen input price: {error}"))?;
    let output_price = Decimal::from_str(&price.output_per_million)
        .map_err(|error| format!("Invalid frozen output price: {error}"))?;
    if input_price.is_sign_negative() || output_price.is_sign_negative() {
        return Err("Frozen Model price cannot be negative".into());
    }
    // A per-million-token price expressed in currency units has the same
    // numeric multiplier as micro-currency per token. Round once after both
    // token classes are summed so the persisted result is deterministic.
    let cost = (Decimal::from(input_tokens) * input_price
        + Decimal::from(output_tokens) * output_price)
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .to_u64()
        .ok_or_else(|| "Calculated Model cost exceeds u64 micro-units".to_owned())?;
    let usage = value
        .as_object_mut()
        .ok_or_else(|| "Provider response must be a JSON object".to_owned())?
        .entry("usage")
        .or_insert_with(|| json!({}));
    let usage = usage
        .as_object_mut()
        .ok_or_else(|| "Provider usage must be a JSON object".to_owned())?;
    usage.insert("costMicros".into(), json!(cost));
    Ok((input_tokens, output_tokens, cost))
}

#[cfg(test)]
pub(super) fn effective_agent_budget(parameters: &Value) -> Value {
    let budget = parameters.get("budget").unwrap_or(parameters);
    let maximum_iterations = budget
        .get("maxIterations")
        .and_then(Value::as_u64)
        .unwrap_or(12)
        .clamp(1, 12);
    let maximum_model_calls = budget
        .get("maxModelCalls")
        .and_then(Value::as_u64)
        .unwrap_or(12)
        .clamp(1, 12);
    let maximum_tool_calls = budget
        .get("maxToolCalls")
        .and_then(Value::as_u64)
        .unwrap_or(32)
        .clamp(0, 32);
    let maximum_tokens = budget
        .get("maxTokens")
        .or_else(|| budget.get("maxTotalTokens"))
        .and_then(Value::as_u64)
        .unwrap_or(64_000);
    let maximum_output_tokens = budget
        .get("maxOutputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(4_096);
    let maximum_cost = budget
        .get("maxCost")
        .and_then(Value::as_f64)
        .map(|cost| (cost * 1_000_000.0) as u64)
        .unwrap_or(1_000_000);
    json!({
        "maxIterations": maximum_iterations,
        "maxModelCalls": maximum_model_calls,
        "maxToolCalls": maximum_tool_calls,
        "maxTokens": maximum_tokens,
        "maxOutputTokens": maximum_output_tokens,
        "maxCostMicros": maximum_cost,
        "maxDurationMs":budget.get("maxDurationSeconds").and_then(Value::as_u64).map(|seconds|seconds.saturating_mul(1_000)).unwrap_or(300_000),
        "limitAction":budget.get("limitAction").and_then(Value::as_str).unwrap_or("error_output"),
    })
}

pub(super) fn runtime_call_is_replayable(status: &str, side_effect: &str) -> bool {
    status == "reserved" || (status == "sent" && matches!(side_effect, "none" | "idempotent"))
}

pub(super) fn runtime_call_side_effect(kind: &str, request: &Value) -> &'static str {
    match (kind, request.get("toolName").and_then(Value::as_str)) {
        ("model" | "compaction", _) => "irreversible",
        ("sandbox", Some("write" | "edit" | "bash")) => "irreversible",
        ("sandbox", _) if request.get("frame").is_some() => {
            match request.get("replayPolicy").and_then(Value::as_str) {
                Some("safe") => "none",
                Some("idempotency_required") => "idempotent",
                _ => "irreversible",
            }
        }
        ("sandbox", _) => "idempotent",
        ("mcp_tool", _) => match request.get("sideEffect").and_then(Value::as_str) {
            Some("none" | "read_only") => "none",
            Some("idempotent") => "idempotent",
            _ => "irreversible",
        },
        ("memory", _) if request.get("messages").is_some() => "irreversible",
        _ => "none",
    }
}

pub(super) fn sandbox_execution_output(
    execution: WorkerExecution,
    parameters: &Value,
) -> WorkerExecution {
    if execution.status != WorkerResultStatusV1::Succeeded {
        return execution;
    }
    let Some(response) = successful_value(&execution) else {
        return WorkerExecution::failed(
            "PROVIDER_RESPONSE_INVALID",
            "Sandbox Manager returned no response payload",
            false,
        );
    };
    let Some(output) = response.get("output").cloned() else {
        return WorkerExecution::failed(
            "PROVIDER_RESPONSE_INVALID",
            "Sandbox Manager response has no output",
            false,
        );
    };
    let structured_output = output
        .get("structuredOutput")
        .cloned()
        .unwrap_or(Value::Null);
    if !structured_output.is_object() {
        return WorkerExecution::failed(
            "CODE_OUTPUT_OBJECT_REQUIRED",
            "Code must write a JSON object as its structured output",
            false,
        );
    }
    let Some(schema) = parameters.get("outputSchema") else {
        return WorkerExecution::failed(
            "CODE_OUTPUT_SCHEMA_REQUIRED",
            "Code outputSchema is required",
            false,
        );
    };
    let Ok(validator) = jsonschema::validator_for(schema) else {
        return WorkerExecution::failed(
            "CODE_OUTPUT_SCHEMA_INVALID",
            "Code outputSchema is not valid JSON Schema",
            false,
        );
    };
    if let Err(error) = validator.validate(&structured_output) {
        return WorkerExecution::failed(
            "CODE_OUTPUT_SCHEMA_VALIDATION_FAILED",
            error.to_string(),
            false,
        );
    }
    WorkerExecution::succeeded(json!({
        "stdout":output.get("stdout").and_then(Value::as_str).unwrap_or_default(),
        "stderr":output.get("stderr").and_then(Value::as_str).unwrap_or_default(),
        "exitCode":output.get("exitCode").and_then(Value::as_i64).unwrap_or_default(),
        "structuredOutput":structured_output,
        "files":output.get("files").cloned().unwrap_or_else(|| json!([])),
        "partial":output.get("partial").and_then(Value::as_bool).unwrap_or(false),
    }))
}

pub(super) fn tool_execution_output(execution: WorkerExecution) -> WorkerExecution {
    normalize_semantic_output(execution, "structuredContent")
}

pub(super) fn rag_execution_output(execution: WorkerExecution) -> WorkerExecution {
    if execution.status != WorkerResultStatusV1::Succeeded {
        return execution;
    }
    let Some(value) = successful_value(&execution) else {
        return invalid_empty();
    };
    let documents = value
        .get("documents")
        .or_else(|| value.get("chunks"))
        .or_else(|| value.get("data"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| json_text(&documents));
    let record_ids = string_ids(value.get("recordIds").or_else(|| value.get("record_ids")));
    WorkerExecution::succeeded(
        json!({"text":text,"documents":documents,"citations":value.get("citations").cloned().unwrap_or_else(|| json!([])),"recordIds":record_ids}),
    )
}

pub(super) fn memory_execution_output(execution: WorkerExecution) -> WorkerExecution {
    if execution.status != WorkerResultStatusV1::Succeeded {
        return execution;
    }
    let Some(value) = successful_value(&execution) else {
        return invalid_empty();
    };
    let records = value
        .get("records")
        .or_else(|| value.get("results"))
        .or_else(|| value.get("memories"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| json_text(&records));
    let record_ids = string_ids(value.get("recordIds").or_else(|| value.get("record_ids")));
    WorkerExecution::succeeded(json!({"text":text,"records":records,"recordIds":record_ids}))
}

fn normalize_semantic_output(execution: WorkerExecution, structured_key: &str) -> WorkerExecution {
    if execution.status != WorkerResultStatusV1::Succeeded {
        return execution;
    }
    let Some(value) = successful_value(&execution) else {
        return invalid_empty();
    };
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            value
                .get("content")
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter_map(|item| item.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
        })
        .unwrap_or_default();
    let structured = value
        .get(structured_key)
        .or_else(|| value.get("structuredOutput"))
        .cloned()
        .filter(Value::is_object)
        .unwrap_or(Value::Null);
    WorkerExecution::succeeded(
        json!({"text":text,"structuredOutput":structured,"files":value.get("files").cloned().unwrap_or_else(|| json!([]))}),
    )
}

fn invalid_empty() -> WorkerExecution {
    WorkerExecution::failed(
        "PROVIDER_RESPONSE_INVALID",
        "Provider response is empty",
        false,
    )
}

fn string_ids(value: Option<&Value>) -> Value {
    Value::Array(
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|value| {
                Value::String(
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| json_text(value)),
                )
            })
            .collect(),
    )
}

pub(crate) const RAG_PROVIDER_LIGHT_RAG: &str = "lightrag";
pub(crate) const RAG_PROVIDER_RAGFLOW: &str = "ragflow";

fn rag_query_text(payload: &Value) -> String {
    payload
        .get("query")
        .or_else(|| payload.get("question"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| json_text(payload))
}

fn rag_top_k(payload: &Value) -> u64 {
    payload
        .get("topK")
        .or_else(|| payload.get("top_k"))
        .and_then(Value::as_u64)
        .unwrap_or(5)
}

/// Build the provider-specific query request from the caller payload.
/// Returns the URL path, the JSON body and the header carrying the secret.
pub(super) fn rag_query_request(
    provider: &str,
    operation: &str,
    namespace: &str,
    index_version: &str,
    input: &Value,
) -> Result<(String, Value, &'static str), WorkerExecution> {
    if provider == RAG_PROVIDER_RAGFLOW {
        if operation != "query" {
            return Err(WorkerExecution::failed(
                "RAG_OPERATION_UNSUPPORTED",
                "RAGFlow knowledge connections support query only",
                false,
            ));
        }
        let dataset_ids: Vec<String> = namespace
            .split(',')
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
            .map(str::to_owned)
            .collect();
        if dataset_ids.is_empty() {
            return Err(WorkerExecution::failed(
                "RAG_DATASET_REQUIRED",
                "RAGFlow knowledge resource must reference at least one dataset id",
                false,
            ));
        }
        return Ok((
            "api/v1/retrieval".into(),
            json!({
                "question": rag_query_text(input),
                "dataset_ids": dataset_ids,
                "top_k": rag_top_k(input),
            }),
            "authorization",
        ));
    }
    let mut body = if input.is_object() {
        input.clone()
    } else {
        json!({"query": input, "mode": "naive"})
    };
    if let Some(object) = body.as_object_mut() {
        if let Some(top_k) = object.remove("topK") {
            object.insert("top_k".into(), top_k);
        }
        object
            .entry("workspace".to_owned())
            .or_insert_with(|| json!(namespace));
        object
            .entry("indexVersion".to_owned())
            .or_insert_with(|| json!(index_version));
    }
    let path = if operation == "insert" {
        "documents/text"
    } else {
        "query"
    };
    Ok((path.into(), body, "x-api-key"))
}

/// Normalize the provider envelope into the canonical rag payload consumed by
/// rag_execution_output / knowledge_result_from_execution. LightRAG responses
/// pass through unchanged; RAGFlow `{code, data}` envelopes are mapped and
/// non-zero codes surface the provider message as a failure.
pub(super) fn finalize_rag_response(provider: &str, execution: WorkerExecution) -> WorkerExecution {
    if provider != RAG_PROVIDER_RAGFLOW || execution.status != WorkerResultStatusV1::Succeeded {
        return execution;
    }
    let Some(value) = successful_value(&execution) else {
        return invalid_empty();
    };
    match value.get("code").and_then(Value::as_i64) {
        None | Some(0) => {}
        Some(_) => {
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("RAGFlow rejected the query");
            return WorkerExecution::failed("PROVIDER_REJECTED", message, false);
        }
    }
    let documents = value
        .pointer("/data/chunks")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let record_ids = value
        .pointer("/data/chunks")
        .and_then(Value::as_array)
        .map(|chunks| {
            Value::Array(
                chunks
                    .iter()
                    .filter_map(|chunk| chunk.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .map(Value::String)
                    .collect(),
            )
        })
        .unwrap_or_else(|| json!([]));
    WorkerExecution::succeeded(json!({
        "text": json_text(&documents),
        "documents": documents,
        "citations": [],
        "recordIds": record_ids,
    }))
}

#[cfg(test)]
mod pricing_tests {
    use agentx_runtime_contracts::RuntimeModelPriceV1;
    use serde_json::json;

    use super::apply_model_price;

    #[test]
    fn calculates_micro_cost_from_the_frozen_price_snapshot() {
        let mut response =
            json!({"usage":{"prompt_tokens":4784,"completion_tokens":10,"total_tokens":4794}});
        let usage = apply_model_price(
            &mut response,
            &RuntimeModelPriceV1 {
                version_id: "price-1".into(),
                currency: "USD".into(),
                input_per_million: "5".into(),
                output_per_million: "30".into(),
            },
        )
        .unwrap();
        assert_eq!(usage, (4784, 10, 24_220));
        assert_eq!(response["usage"]["costMicros"], 24_220);
    }

    #[test]
    fn rounds_fractional_micro_units_once() {
        let mut response = json!({"usage":{"input_tokens":3,"output_tokens":1}});
        let usage = apply_model_price(
            &mut response,
            &RuntimeModelPriceV1 {
                version_id: "price-2".into(),
                currency: "USD".into(),
                input_per_million: "0.15".into(),
                output_per_million: "0.25".into(),
            },
        )
        .unwrap();
        assert_eq!(usage.2, 1);
    }
}

#[cfg(test)]
mod rag_protocol_tests {
    use agentx_runtime_contracts::WorkerResultStatusV1;
    use serde_json::{Value, json};

    use super::{finalize_rag_response, rag_query_request};

    #[test]
    fn lightrag_requests_keep_the_workspace_contract() {
        let (path, body, header) = match rag_query_request(
            "lightrag",
            "query",
            "kb-1",
            "v3",
            &json!({"query": "hello", "topK": 4}),
        ) {
            Ok(built) => built,
            Err(_) => panic!("lightrag request must build"),
        };
        assert_eq!(path, "query");
        assert_eq!(header, "x-api-key");
        assert_eq!(
            body,
            json!({"query": "hello", "top_k": 4, "workspace": "kb-1", "indexVersion": "v3"})
        );
    }

    #[test]
    fn ragflow_requests_map_to_the_retrieval_contract() {
        let (path, body, header) = match rag_query_request(
            "ragflow",
            "query",
            "ds-1, ds-2",
            "v3",
            &json!({"query": "hello", "topK": 6}),
        ) {
            Ok(built) => built,
            Err(_) => panic!("ragflow request must build"),
        };
        assert_eq!(path, "api/v1/retrieval");
        assert_eq!(header, "authorization");
        assert_eq!(
            body,
            json!({"question": "hello", "dataset_ids": ["ds-1", "ds-2"], "top_k": 6})
        );
    }

    #[test]
    fn ragflow_rejects_non_query_operations_and_empty_datasets() {
        let err = match rag_query_request("ragflow", "insert", "ds-1", "v3", &json!({})) {
            Err(failed) => failed,
            Ok(_) => panic!("insert must be unsupported"),
        };
        assert_eq!(err.error_code.as_deref(), Some("RAG_OPERATION_UNSUPPORTED"));
        let err = match rag_query_request("ragflow", "query", " , ", "v3", &json!({})) {
            Err(failed) => failed,
            Ok(_) => panic!("dataset must be required"),
        };
        assert_eq!(err.error_code.as_deref(), Some("RAG_DATASET_REQUIRED"));
    }

    #[test]
    fn ragflow_envelopes_map_to_canonical_documents() {
        let execution = super::super::WorkerExecution::succeeded(json!({
            "code": 0,
            "data": {"chunks": [
                {"id": "c1", "content": "alpha", "similarity": 0.9},
                {"id": "c2", "content": "beta", "similarity": 0.8},
            ], "total": 2},
            "message": ""
        }));
        let normalized = finalize_rag_response("ragflow", execution);
        assert_eq!(normalized.status, WorkerResultStatusV1::Succeeded);
        let payload = normalized
            .outputs
            .get("main")
            .and_then(|items| items.first())
            .map(|item| item.json.clone())
            .expect("payload");
        assert_eq!(
            payload.get("documents"),
            Some(&json!([
                {"id": "c1", "content": "alpha", "similarity": 0.9},
                {"id": "c2", "content": "beta", "similarity": 0.8},
            ]))
        );
        assert_eq!(payload.get("recordIds"), Some(&json!(["c1", "c2"])));
    }

    #[test]
    fn ragflow_error_envelopes_surface_the_provider_message() {
        let execution = super::super::WorkerExecution::succeeded(json!({
            "code": 109,
            "data": false,
            "message": "Authentication error: API key is invalid!"
        }));
        let normalized = finalize_rag_response("ragflow", execution);
        assert_eq!(normalized.status, WorkerResultStatusV1::Failed);
        assert_eq!(normalized.error_code.as_deref(), Some("PROVIDER_REJECTED"));
        assert_eq!(
            normalized.error_message.as_deref(),
            Some("Authentication error: API key is invalid!")
        );
    }

    #[test]
    fn lightrag_responses_pass_through_unchanged() {
        let payload: Value = json!({"data": {"documents": ["a"]}});
        let execution = super::super::WorkerExecution::succeeded(payload.clone());
        let normalized = finalize_rag_response("lightrag", execution);
        let kept = normalized
            .outputs
            .get("main")
            .and_then(|items| items.first())
            .map(|item| item.json.clone())
            .expect("payload");
        assert_eq!(kept, payload);
    }
}
