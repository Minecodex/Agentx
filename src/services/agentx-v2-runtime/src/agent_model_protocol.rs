//! Shared OpenAI request/response projection for both Core hosts.
use agentx_agent_core::{MessageRole, ModelRequestV1, ModelResponseV1, ToolCallV1};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) fn request(request: &ModelRequestV1, model: &str, streaming: bool) -> Value {
    let messages = request.messages.iter().map(|message| {
        let role = match message.role { MessageRole::User => "user", MessageRole::Assistant => "assistant", MessageRole::ToolResult => "tool", MessageRole::ExternalContext => "system" };
        let mut value = json!({"role":role,"content":message.content});
        if let Some(id) = &message.tool_call_id { value["tool_call_id"] = json!(id); }
        if !message.tool_calls.is_empty() { value["tool_calls"] = json!(message.tool_calls.iter().map(|call| json!({"id":call.call_id,"type":"function","function":{"name":call.name,"arguments":call.arguments.to_string()}})).collect::<Vec<_>>()); }
        value
    }).collect::<Vec<_>>();
    let mut body = json!({"model":model,"messages":messages,"stream":streaming});
    if streaming {
        body["stream_options"] = json!({"include_usage":true});
    }
    if !request.tools.is_empty() {
        body["tools"] = json!(request.tools.iter().map(|tool| json!({"type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.input_schema}})).collect::<Vec<_>>());
    }
    body
}

pub(crate) fn apply_parameters(body: &mut Value, parameters: &Value, output_ceiling: u64) {
    for name in [
        "temperature",
        "top_p",
        "presence_penalty",
        "frequency_penalty",
        "seed",
        "stop",
        "thinking",
        "reasoning_effort",
    ] {
        if let Some(value) = parameters.get(name) {
            body[name] = value.clone();
        }
    }
    let limit = if parameters.get("max_completion_tokens").is_some() {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    let output = parameters
        .get(limit)
        .and_then(Value::as_u64)
        .unwrap_or(output_ceiling)
        .min(output_ceiling);
    body[limit] = json!(output);
}

pub(crate) fn response(payload: &Value, message_id: String) -> Result<ModelResponseV1, String> {
    if payload
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        == Some("length")
    {
        return Err(
            "MODEL_OUTPUT_LIMIT_REACHED: The provider stopped at its output token limit".into(),
        );
    }
    let message = payload
        .pointer("/choices/0/message")
        .ok_or("Provider response has no assistant message")?;
    let calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut tool_calls = Vec::with_capacity(calls.len());
    for call in calls {
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Tool call ID is missing")?;
        let function = call.get("function").ok_or("Tool function is missing")?;
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or("Tool name is missing")?;
        let raw = function
            .get("arguments")
            .and_then(Value::as_str)
            .ok_or("Tool arguments are missing")?;
        let arguments =
            serde_json::from_str(raw).map_err(|_| "Tool arguments must be valid JSON")?;
        let digest = format!(
            "{:x}",
            Sha256::digest(format!("{message_id}:{id}").as_bytes())
        );
        let call_id = format!("call_{}", &digest[..32]);
        tool_calls.push(ToolCallV1 {
            call_id,
            name: name.into(),
            arguments,
        });
    }
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if content.trim().is_empty() && tool_calls.is_empty() {
        return Err("MODEL_RESPONSE_EMPTY: No answer or tool call was returned".into());
    }
    Ok(ModelResponseV1 {
        message_id,
        content: content.into(),
        tool_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_model_parameters_preserve_identity_and_respect_the_output_budget() {
        let mut body = json!({"model":"pinned-model","messages":[{"role":"user","content":"business input"}],"stream":true});
        apply_parameters(
            &mut body,
            &json!({"model":"untrusted-override","messages":[],"stream":false,"max_tokens":16000,"reasoning_effort":"low","temperature":0.2}),
            4096,
        );
        assert_eq!(body["model"], "pinned-model");
        assert_eq!(body["messages"][0]["content"], "business input");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["temperature"], 0.2);
        let mut completion = json!({"model":"pinned-model"});
        apply_parameters(
            &mut completion,
            &json!({"max_completion_tokens":2048}),
            4096,
        );
        assert_eq!(completion["max_completion_tokens"], 2048);
        assert!(completion.get("max_tokens").is_none());
    }
    #[test]
    fn thinking_only_and_truncated_responses_cannot_complete_a_run_or_execute_tools() {
        assert!(response(&json!({"choices":[{"finish_reason":"stop","message":{"reasoning_content":"internal thought","content":""}}]}),"m".into()).unwrap_err().contains("MODEL_RESPONSE_EMPTY"));
        let call = json!({"id":"call-1","function":{"name":"page_edit","arguments":"{}"}});
        assert!(response(&json!({"choices":[{"finish_reason":"length","message":{"content":"partial","tool_calls":[call.clone()]}}]}),"m".into()).unwrap_err().contains("MODEL_OUTPUT_LIMIT_REACHED"));
        assert_eq!(response(&json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"","tool_calls":[call]}}]}),"m".into()).unwrap().tool_calls.len(),1);
    }
}
