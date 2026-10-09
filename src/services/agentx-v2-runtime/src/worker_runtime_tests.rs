use super::output::effective_agent_budget;
use super::output::runtime_call_is_replayable;
use super::{
    WorkerExecution, apply_http_credential, declarative_http_request, mcp_tool_binding,
    openai_chat_completions_endpoint, openai_execution_output, provider_secret_header,
    provider_usage_detail, redact_secret_bytes, runtime_call_fingerprint, sandbox_execution_output,
    secret_fragments, system_prompt,
};
use crate::worker_runtime_calls::runtime_call_side_effect;
use agentx_runtime_contracts::{
    ContentHash, RuntimeResourceBindingV1, RuntimeResourceConfigurationV1, RuntimeResourceKindV1,
    WorkerResultStatusV1,
};
use reqwest::header::HeaderMap;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[tokio::test]
async fn worker_operation_deadline_cancels_slow_provider_work() {
    let result = crate::worker_support::with_operation_deadline(
        time::OffsetDateTime::now_utc() + time::Duration::milliseconds(5),
        async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            "late"
        },
    )
    .await;
    assert!(result.is_err());
}

fn mcp_binding(resource_id: Uuid, tool_name: &str) -> RuntimeResourceBindingV1 {
    RuntimeResourceBindingV1 {
        resource_kind: RuntimeResourceKindV1::Mcp,
        resource_id,
        resource_version: "1".into(),
        state_epoch: 1,
        content_hash: ContentHash::parse(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap(),
        configuration: RuntimeResourceConfigurationV1::Mcp {
            server_id: Uuid::from_u128(1),
            server_version_id: Uuid::from_u128(2),
            transport: agentx_runtime_contracts::RuntimeMcpTransportV2::StreamableHttp {
                endpoint: "http://mcp.example/mcp".into(),
            },
            tool_name: tool_name.into(),
            tool_version: "1".into(),
            input_schema_hash: ContentHash::parse(
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            )
            .unwrap(),
            input_schema: json!({"type":"object"}),
            output_schema: None,
            side_effect: "read_only".into(),
            timeout_seconds: 30,
            credential: None,
        },
        object_ids: Vec::new(),
    }
}

#[test]
fn mcp_server_closure_never_shadows_the_executable_tool_binding() {
    let server = mcp_binding(Uuid::now_v7(), "__server__");
    let tool_id = Uuid::now_v7();
    let resources = [server, mcp_binding(tool_id, "echo")];
    let selected = mcp_tool_binding(&resources).unwrap();
    assert_eq!(selected.resource_id, tool_id);
    assert!(
        matches!(&selected.configuration, RuntimeResourceConfigurationV1::Mcp { tool_name, .. } if tool_name == "echo")
    );
}

#[test]
fn declarative_http_parameters_build_the_actual_request() {
    assert_eq!(
        declarative_http_request(
            &json!({"method":"PATCH","url":"https://example.test/tickets","query":[{"name":"page","value":2}],"headers":[{"name":"x-agentx","value":"contract"}],"body":{"enabled":true}}),
            Some(json!({"ignored":"input"})),
        ).unwrap(),
        ("https://example.test/tickets?page=2".into(), json!({"method":"PATCH","headers":{"x-agentx":"contract"},"body":{"enabled":true},"apiKeyPlacement":null}))
    );
}

#[test]
fn http_credentials_inject_only_the_declared_header_or_query_fields() {
    let mut bearer_endpoint = "https://example.test/items".to_owned();
    let mut bearer_headers = HeaderMap::new();
    apply_http_credential(
        &mut bearer_endpoint,
        &mut bearer_headers,
        "bearer",
        b"bearer-secret",
        None,
    )
    .unwrap();
    assert_eq!(bearer_endpoint, "https://example.test/items");
    assert_eq!(
        bearer_headers["authorization"].to_str().unwrap(),
        "Bearer bearer-secret"
    );

    let mut basic_endpoint = "https://example.test/items".to_owned();
    let mut basic_headers = HeaderMap::new();
    apply_http_credential(
        &mut basic_endpoint,
        &mut basic_headers,
        "basic",
        br#"{"username":"agentx-user","password":"basic-secret"}"#,
        None,
    )
    .unwrap();
    assert_eq!(basic_endpoint, "https://example.test/items");
    assert_eq!(
        basic_headers["authorization"].to_str().unwrap(),
        "Basic YWdlbnR4LXVzZXI6YmFzaWMtc2VjcmV0"
    );

    let mut header_endpoint = "https://example.test/items".to_owned();
    let mut header_headers = HeaderMap::new();
    apply_http_credential(
        &mut header_endpoint,
        &mut header_headers,
        "api_key",
        b"header-secret",
        Some(&json!({"in":"header","name":"x-api-key"})),
    )
    .unwrap();
    assert_eq!(header_endpoint, "https://example.test/items");
    assert_eq!(header_headers["x-api-key"], "header-secret");

    let mut endpoint = "https://example.test/items".to_owned();
    let mut headers = HeaderMap::new();
    apply_http_credential(
        &mut endpoint,
        &mut headers,
        "api_key",
        b"secret-value",
        Some(&json!({"in":"query","name":"token"})),
    )
    .unwrap();
    assert_eq!(endpoint, "https://example.test/items?token=secret-value");
    assert!(headers.is_empty());

    let mut custom_endpoint = "https://example.test/items?existing=1".to_owned();
    let mut custom_headers = HeaderMap::new();
    apply_http_credential(
        &mut custom_endpoint,
        &mut custom_headers,
        "custom_json",
        br#"{"headers":{"x-custom-auth":"header-value"},"query":{"custom_token":"query-value"}}"#,
        None,
    )
    .unwrap();
    assert_eq!(
        custom_endpoint,
        "https://example.test/items?existing=1&custom_token=query-value"
    );
    assert_eq!(custom_headers["x-custom-auth"], "header-value");

    let invalid = apply_http_credential(
        &mut endpoint,
        &mut headers,
        "custom_json",
        br#"{"headers":"not-an-object"}"#,
        None,
    )
    .unwrap_err();
    assert_eq!(invalid, "Custom HTTP credential headers must be an object");

    let unknown = apply_http_credential(
        &mut endpoint,
        &mut headers,
        "custom_json",
        br#"{"headers":{},"body":{"forbidden":true}}"#,
        None,
    )
    .unwrap_err();
    assert_eq!(
        unknown,
        "Custom HTTP credential only accepts headers and query"
    );
}

#[test]
fn http_responses_redact_raw_and_structured_credential_values() {
    let secrets = secret_fragments(br#"{"username":"agentx-user","password":"top-secret"}"#);
    let redacted = redact_secret_bytes(
        br#"prefix top-secret {"password":"top-secret"} suffix"#,
        &secrets,
    );
    let text = String::from_utf8(redacted).unwrap();
    assert!(!text.contains("top-secret"));
    assert!(text.contains("[REDACTED]"));
}

#[test]
fn current_agent_manifest_budget_fields_override_defaults() {
    assert_eq!(
        effective_agent_budget(
            &json!({"maxIterations":3,"maxTotalTokens":1000,"maxCost":0.001,"maxDurationSeconds":120})
        ),
        json!({"maxIterations":3,"maxModelCalls":12,"maxToolCalls":32,"maxTokens":1000,"maxOutputTokens":4096,"maxCostMicros":1000,"maxDurationMs":120000,"limitAction":"error_output"})
    );
}

#[test]
fn explicit_nested_agent_budget_takes_precedence() {
    assert_eq!(
        effective_agent_budget(
            &json!({"budget":{"maxIterations":4,"maxTokens":2000,"maxCost":0.003},"maxIterations":2,"maxTotalTokens":500,"maxCost":0.0007})
        ),
        json!({"maxIterations":4,"maxModelCalls":12,"maxToolCalls":32,"maxTokens":2000,"maxOutputTokens":4096,"maxCostMicros":3000,"maxDurationMs":300000,"limitAction":"error_output"})
    );
}

#[test]
fn sandbox_runtime_call_fingerprint_ignores_attempt_lease_identity() {
    let first = json!({"apiVersion":1,"attemptId":"018f0000-0000-7000-8000-000000000001","workerId":"018f0000-0000-7000-8000-000000000002","fencingToken":1,"idempotencyKey":"sandbox:execute:attempt","input":{"message":"stable"}});
    let replacement = json!({"apiVersion":1,"attemptId":"018f0000-0000-7000-8000-000000000001","workerId":"018f0000-0000-7000-8000-000000000003","fencingToken":2,"idempotencyKey":"sandbox:execute:attempt","input":{"message":"stable"}});
    assert_eq!(
        runtime_call_fingerprint("sandbox", &first),
        runtime_call_fingerprint("sandbox", &replacement)
    );
    let changed = json!({"apiVersion":1,"attemptId":"018f0000-0000-7000-8000-000000000001","workerId":"018f0000-0000-7000-8000-000000000003","fencingToken":2,"idempotencyKey":"sandbox:execute:attempt","input":{"message":"changed"}});
    assert_ne!(
        runtime_call_fingerprint("sandbox", &first),
        runtime_call_fingerprint("sandbox", &changed)
    );
}

#[test]
fn only_uncommitted_or_idempotent_sent_runtime_calls_are_replayable() {
    assert!(runtime_call_is_replayable("reserved", "irreversible"));
    assert!(runtime_call_is_replayable("sent", "none"));
    assert!(runtime_call_is_replayable("sent", "idempotent"));
    assert!(!runtime_call_is_replayable("sent", "irreversible"));
    assert!(!runtime_call_is_replayable("outcome_unknown", "none"));
    assert!(!runtime_call_is_replayable("failed", "none"));
    for kind in ["model", "compaction"] {
        let side_effect = runtime_call_side_effect(kind, &json!({}));
        assert_eq!(side_effect, "irreversible");
        assert!(!runtime_call_is_replayable("sent", side_effect));
    }
}

#[test]
fn unsafe_workspace_tools_are_never_replayed_after_send() {
    assert_eq!(
        runtime_call_side_effect("sandbox", &json!({"toolName":"read"})),
        "idempotent"
    );
    for tool in ["write", "edit", "bash"] {
        let request = json!({"toolName":tool});
        let side_effect = runtime_call_side_effect("sandbox", &request);
        assert_eq!(side_effect, "irreversible");
        assert!(!runtime_call_is_replayable("sent", side_effect));
    }
}

#[test]
fn stdio_frames_preserve_frozen_replay_policy_in_the_ledger() {
    for (policy, expected) in [
        ("safe", "none"),
        ("idempotency_required", "idempotent"),
        ("never", "irreversible"),
    ] {
        let request = json!({
            "frame":{"jsonrpc":"2.0","method":"tools/call"},
            "replayPolicy":policy,
        });
        assert_eq!(runtime_call_side_effect("sandbox", &request), expected);
    }
    let unknown_mcp = json!({"tool":"unknown"});
    let side_effect = runtime_call_side_effect("mcp_tool", &unknown_mcp);
    assert_eq!(side_effect, "irreversible");
    assert!(!runtime_call_is_replayable("sent", side_effect));
    let memory_write = json!({"messages":[{"role":"user","content":"remember"}]});
    let side_effect = runtime_call_side_effect("memory", &memory_write);
    assert_eq!(side_effect, "irreversible");
    assert!(!runtime_call_is_replayable("sent", side_effect));
}

#[test]
fn mcp_replay_uses_the_frozen_side_effect_annotation() {
    for side_effect in ["none", "read_only"] {
        let request = json!({"sideEffect":side_effect});
        assert_eq!(runtime_call_side_effect("mcp_tool", &request), "none");
        assert!(runtime_call_is_replayable("sent", "none"));
    }
    let idempotent = json!({"sideEffect":"idempotent"});
    assert_eq!(
        runtime_call_side_effect("mcp_tool", &idempotent),
        "idempotent"
    );
    assert!(runtime_call_is_replayable("sent", "idempotent"));
    for side_effect in ["unknown", "non_idempotent", "irreversible"] {
        let request = json!({"sideEffect":side_effect});
        assert_eq!(
            runtime_call_side_effect("mcp_tool", &request),
            "irreversible"
        );
        assert!(!runtime_call_is_replayable("sent", "irreversible"));
    }
}

#[test]
fn sandbox_manager_envelope_is_not_exposed_as_node_output() {
    let execution = sandbox_execution_output(
        WorkerExecution::succeeded(
            json!({"apiVersion":1,"leaseId":"018f0000-0000-7000-8000-000000000001","sandboxId":"sandbox-v2","replayed":true,"output":{"stdout":"agentx-v2-04","stderr":"","exitCode":0,"structuredOutput":{"ok":true},"files":[],"partial":false}}),
        ),
        &json!({"outputSchema":{"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}}}}),
    );
    assert_eq!(execution.status, WorkerResultStatusV1::Succeeded);
    assert_eq!(
        execution.outputs["main"][0].json,
        json!({"stdout":"agentx-v2-04","stderr":"","exitCode":0,"structuredOutput":{"ok":true},"files":[],"partial":false})
    );
}

#[test]
fn code_rejects_non_object_structured_results() {
    let execution = sandbox_execution_output(
        WorkerExecution::succeeded(
            json!({"output":{"stdout":"","stderr":"","exitCode":0,"structuredOutput":"text","files":[],"partial":false}}),
        ),
        &json!({"outputSchema":{"type":"object"}}),
    );
    assert_eq!(execution.status, WorkerResultStatusV1::Failed);
    assert_eq!(
        execution.error_code.as_deref(),
        Some("CODE_OUTPUT_OBJECT_REQUIRED")
    );
}

#[test]
fn openai_compatible_endpoint_targets_chat_completions_once() {
    assert_eq!(
        openai_chat_completions_endpoint("https://provider.example/v1"),
        "https://provider.example/v1/chat/completions"
    );
    assert_eq!(
        openai_chat_completions_endpoint("https://provider.example/v1/chat/completions/"),
        "https://provider.example/v1/chat/completions"
    );
}

#[test]
fn model_prompt_and_agent_system_prompt_use_their_manifest_names() {
    assert_eq!(
        system_prompt(&json!({"prompt":"你叫 kakj"}), "model"),
        Some("你叫 kakj")
    );
    assert_eq!(
        system_prompt(&json!({"systemPrompt":"agent rules"}), "agent"),
        Some("agent rules")
    );
    assert_eq!(
        system_prompt(&json!({"prompt":"wrong field"}), "agent"),
        None
    );
}

#[test]
fn provider_authorization_secret_adds_bearer_only_when_needed() {
    assert_eq!(
        provider_secret_header(b"raw-secret", "authorization"),
        b"Bearer raw-secret"
    );
    assert_eq!(
        provider_secret_header(b"Bearer token", "authorization"),
        b"Bearer token"
    );
    assert_eq!(
        provider_secret_header(b"Basic token", "authorization"),
        b"Basic token"
    );
    assert_eq!(
        provider_secret_header(b"raw-secret", "x-api-key"),
        b"raw-secret"
    );
}

#[test]
fn openai_tool_call_is_normalized_for_the_agent_loop() {
    let execution = openai_execution_output(
        WorkerExecution::succeeded(
            json!({"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-1","type":"function","function":{"name":"echo","arguments":"{\"text\":\"hello\"}"}}]}}],"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}),
        ),
        &json!({"responseMode":"text"}),
    );
    assert_eq!(
        execution.outputs["main"][0].json,
        json!({"toolCall":{"text":"hello"},"usage":{"inputTokens":11,"outputTokens":7,"totalTokens":18,"costMicros":0}})
    );
}

#[test]
fn openai_model_output_matches_the_manifest_contract() {
    let execution = openai_execution_output(
        WorkerExecution::succeeded(
            json!({"choices":[{"message":{"role":"assistant","content":"complete"}}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}),
        ),
        &json!({"responseMode":"text"}),
    );
    assert_eq!(
        execution.outputs["main"][0].json,
        json!({"text":"complete","reasoningContent":null,"structuredOutput":null,"citations":[],"files":[],"usage":{"inputTokens":5,"outputTokens":3,"totalTokens":8,"costMicros":0},"finishReason":null,"partial":false})
    );
}

#[test]
fn provider_usage_detail_accepts_raw_and_normalized_token_names() {
    assert_eq!(
        provider_usage_detail(
            &json!({"usage":{"prompt_tokens":5,"completion_tokens":3,"cost_micros":7}})
        ),
        (5, 3, 7)
    );
    assert_eq!(
        provider_usage_detail(&json!({"usage":{"inputTokens":11,"outputTokens":4,"costMicros":9}})),
        (11, 4, 9)
    );
}

#[test]
fn every_studio_manifest_parameter_has_an_explicit_runtime_consumer() {
    let runtime_consumers: BTreeMap<&str, &[&str]> = BTreeMap::from([
        ("set", &["values", "keepOnlySet"][..]),
        ("if", &["cases"]),
        ("list", &["input", "filter", "sort", "takeN"]),
        (
            "merge",
            &[
                "mode",
                "leftField",
                "rightField",
                "joinType",
                "conflictStrategy",
            ],
        ),
        (
            "loop_over_items",
            &["input", "outputSelector", "errorMode", "parallelism"],
        ),
        (
            "approval",
            &[
                "title",
                "description",
                "candidateUserId",
                "buttons",
                "timeoutMs",
            ],
        ),
        ("sub_workflow", &["workflowVersionId", "inputs"]),
        (
            "declarative_http",
            &[
                "method",
                "url",
                "query",
                "headers",
                "body",
                "apiKeyPlacement",
            ],
        ),
        (
            "model",
            &[
                "prompt",
                "userQuestion",
                "responseMode",
                "stream",
                "structuredSchema",
            ],
        ),
        (
            "agent",
            &[
                "systemPrompt",
                "userQuestion",
                "sessionPolicy",
                "maxIterations",
                "maxModelCalls",
                "maxToolCalls",
                "maxTotalTokens",
                "maxOutputTokens",
                "maxCost",
                "maxDurationSeconds",
                "limitAction",
            ],
        ),
        (
            "code",
            &[
                "runner",
                "inputs",
                "source",
                "outputExample",
                "networkPolicy",
            ],
        ),
        ("reply_message", &["content"]),
        (
            "send_message",
            &["content", "channelId", "targetConversationId", "senderId"],
        ),
    ]);
    let registry = agentx_runtime::NodeRegistry::m5_defaults();
    assert_eq!(registry.studio_manifests().count(), runtime_consumers.len());
    for manifest in registry.studio_manifests() {
        let declared = manifest.parameter_schema["properties"]
            .as_object()
            .map(|properties| properties.keys().map(String::as_str).collect())
            .unwrap_or_else(BTreeSet::new);
        let consumed = runtime_consumers
            .get(manifest.node_type.as_str())
            .unwrap_or_else(|| panic!("{} has no runtime consumer ownership", manifest.node_type))
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            declared, consumed,
            "{} parameter ownership drifted",
            manifest.node_type
        );
    }
}
