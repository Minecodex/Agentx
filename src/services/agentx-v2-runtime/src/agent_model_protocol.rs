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

pub(crate) fn response(payload: &Value, message_id: String) -> Result<ModelResponseV1, String> {
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
    Ok(ModelResponseV1 {
        message_id,
        content: message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        tool_calls,
    })
}
