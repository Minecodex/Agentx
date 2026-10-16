use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{EffectContextV1, ToolCallV1};

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOriginV1 {
    Core,
    Platform,
    Mcp,
    Skill,
    Knowledge,
    Memory,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPolicyV1 {
    Safe,
    Never,
    IdempotencyRequired,
    LedgerDependent,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolDefinitionV1 {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub origin: ToolOriginV1,
    pub replay_policy: ReplayPolicyV1,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolResultV1 {
    pub content: String,
    #[serde(default)]
    pub structured_result: Option<Value>,
    #[serde(default)]
    pub artifact_refs: Vec<String>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub terminate: bool,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ToolPortError {
    #[error("tool effect is pending: {0}")]
    Pending(String),
    #[error("tool is not authorized: {0}")]
    Unauthorized(String),
    #[error("tool effect failed: {0}")]
    Effect(String),
    #[error("tool effect outcome is unknown: {0}")]
    OutcomeUnknown(String),
}

pub trait ToolPort {
    fn observe(&mut self, _context: &EffectContextV1) -> crate::EffectObservationV1 {
        crate::EffectObservationV1::Unknown
    }

    fn execute(
        &mut self,
        call: &ToolCallV1,
        context: &EffectContextV1,
    ) -> Result<ToolResultV1, ToolPortError>;
}

pub const CORE_TOOL_NAMES: [&str; 4] = ["read", "write", "edit", "bash"];

pub fn core_tool_registry(workspace_sandbox_selected: bool) -> Vec<ToolDefinitionV1> {
    if !workspace_sandbox_selected {
        return Vec::new();
    }
    vec![
        core_tool(
            "read",
            "Read a file inside the Agent workspace sandbox.",
            ReplayPolicyV1::Safe,
        ),
        core_tool(
            "write",
            "Write a file inside the Agent workspace sandbox.",
            ReplayPolicyV1::Never,
        ),
        core_tool(
            "edit",
            "Edit a file inside the Agent workspace sandbox.",
            ReplayPolicyV1::Never,
        ),
        core_tool(
            "bash",
            "Run a command inside the Agent workspace sandbox.",
            ReplayPolicyV1::Never,
        ),
    ]
}

fn core_tool(name: &str, description: &str, replay_policy: ReplayPolicyV1) -> ToolDefinitionV1 {
    ToolDefinitionV1 {
        name: name.into(),
        description: description.into(),
        input_schema: core_tool_schema(name),
        origin: ToolOriginV1::Core,
        replay_policy,
    }
}

fn core_tool_schema(name: &str) -> Value {
    match name {
        "read" => json!({
            "type":"object","additionalProperties":false,
            "required":["path"],
            "properties":{"path":{"type":"string","minLength":1},"startLine":{"type":"integer","minimum":1},"endLine":{"type":"integer","minimum":1},"startByte":{"type":"integer","minimum":0},"maxBytes":{"type":"integer","minimum":1,"maximum":8388608}}
        }),
        "write" => json!({
            "type":"object","additionalProperties":false,
            "required":["path","content","encoding","mode"],
            "properties":{"path":{"type":"string","minLength":1},"content":{"type":"string","maxLength":8388608},"encoding":{"enum":["utf8","base64"]},"mode":{"enum":["create","overwrite"]},"createParents":{"type":"boolean"}}
        }),
        "edit" => json!({
            "type":"object","additionalProperties":false,
            "required":["path","expectedHash","edits","encoding"],
            "properties":{"path":{"type":"string","minLength":1},"expectedHash":{"type":"string","pattern":"^sha256:[0-9a-f]{64}$"},"encoding":{"enum":["utf8"]},"edits":{"type":"array","minItems":1,"maxItems":1024,"items":{"type":"object","additionalProperties":false,"required":["startByte","endByte","replacement"],"properties":{"startByte":{"type":"integer","minimum":0},"endByte":{"type":"integer","minimum":0},"replacement":{"type":"string","maxLength":8388608}}}}}
        }),
        "bash" => json!({
            "type":"object","additionalProperties":false,
            "required":["argv"],
            "properties":{"argv":{"type":"array","minItems":1,"maxItems":128,"items":{"type":"string","maxLength":4096}},"cwd":{"type":"string"},"env":{"type":"object","propertyNames":{"enum":["PATH","HOME","LANG","LC_ALL"]},"additionalProperties":{"type":"string","maxLength":4096}},"timeoutMs":{"type":"integer","minimum":1,"maximum":300000},"maxOutputBytes":{"type":"integer","minimum":1,"maximum":8388608}}
        }),
        _ => json!({"type":"object"}),
    }
}

pub fn validate_core_tool_call(call: &ToolCallV1) -> Result<(), String> {
    let args = call
        .arguments
        .as_object()
        .ok_or_else(|| "arguments must be an object".to_owned())?;
    let path = |key: &str| {
        let value = args
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{key} is required"))?;
        validate_workspace_path(value)
    };
    match call.name.as_str() {
        "read" => {
            ensure_allowed_keys(
                args,
                &["path", "startLine", "endLine", "startByte", "maxBytes"],
            )?;
            path("path")?;
            let start_line = optional_u64(args, "startLine", 1, u64::MAX)?;
            let end_line = optional_u64(args, "endLine", 1, u64::MAX)?;
            if start_line
                .zip(end_line)
                .is_some_and(|(start, end)| start > end)
            {
                return Err("startLine must not exceed endLine".into());
            }
            optional_u64(args, "startByte", 0, u64::MAX)?;
            optional_u64(args, "maxBytes", 1, 8_388_608)?;
        }
        "write" => {
            ensure_allowed_keys(
                args,
                &["path", "content", "encoding", "mode", "createParents"],
            )?;
            path("path")?;
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| "content is required".to_owned())?;
            if content.len() > 8_388_608 {
                return Err("content exceeds 8 MiB".into());
            }
            require_enum(args, "encoding", &["utf8", "base64"])?;
            require_enum(args, "mode", &["create", "overwrite"])?;
            if args
                .get("createParents")
                .is_some_and(|value| !value.is_boolean())
            {
                return Err("createParents must be a boolean".into());
            }
        }
        "edit" => {
            ensure_allowed_keys(args, &["path", "expectedHash", "edits", "encoding"])?;
            path("path")?;
            let hash = args
                .get("expectedHash")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if hash.len() != 71
                || !hash.starts_with("sha256:")
                || !hash[7..]
                    .bytes()
                    .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
            {
                return Err("expectedHash must be sha256".into());
            }
            let edits = args
                .get("edits")
                .and_then(Value::as_array)
                .ok_or_else(|| "edits is required".to_owned())?;
            if edits.is_empty() || edits.len() > 1_024 {
                return Err("edits is required".into());
            }
            for edit in edits {
                let edit = edit
                    .as_object()
                    .ok_or_else(|| "each edit must be an object".to_owned())?;
                ensure_allowed_keys(edit, &["startByte", "endByte", "replacement"])?;
                let start = required_u64(edit, "startByte")?;
                let end = required_u64(edit, "endByte")?;
                if start > end {
                    return Err("edit startByte must not exceed endByte".into());
                }
                let replacement = edit
                    .get("replacement")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "edit replacement is required".to_owned())?;
                if replacement.len() > 8_388_608 {
                    return Err("edit replacement exceeds 8 MiB".into());
                }
            }
            require_enum(args, "encoding", &["utf8"])?;
        }
        "bash" => {
            ensure_allowed_keys(args, &["argv", "cwd", "env", "timeoutMs", "maxOutputBytes"])?;
            let argv = args
                .get("argv")
                .and_then(Value::as_array)
                .ok_or_else(|| "argv is required".to_owned())?;
            if argv.is_empty()
                || argv.len() > 128
                || argv
                    .iter()
                    .any(|value| value.as_str().is_none_or(|argument| argument.len() > 4_096))
            {
                return Err("argv must contain strings".into());
            }
            if let Some(cwd) = args.get("cwd") {
                validate_workspace_path(
                    cwd.as_str()
                        .ok_or_else(|| "cwd must be a string".to_owned())?,
                )?;
            }
            if let Some(environment) = args.get("env") {
                let environment = environment
                    .as_object()
                    .ok_or_else(|| "env must be an object".to_owned())?;
                if environment.iter().any(|(name, value)| {
                    !matches!(name.as_str(), "PATH" | "HOME" | "LANG" | "LC_ALL")
                        || value
                            .as_str()
                            .is_none_or(|environment_value| environment_value.len() > 4_096)
                }) {
                    return Err("env contains a variable that is not allowed".into());
                }
            }
            optional_u64(args, "timeoutMs", 1, 300_000)?;
            optional_u64(args, "maxOutputBytes", 1, 8_388_608)?;
        }
        _ => {}
    }
    Ok(())
}

fn ensure_allowed_keys(
    args: &serde_json::Map<String, Value>,
    allowed: &[&str],
) -> Result<(), String> {
    args.keys()
        .all(|key| allowed.contains(&key.as_str()))
        .then_some(())
        .ok_or_else(|| "arguments contain an unknown field".into())
}

fn required_u64(args: &serde_json::Map<String, Value>, key: &str) -> Result<u64, String> {
    args.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{key} must be a non-negative integer"))
}

fn optional_u64(
    args: &serde_json::Map<String, Value>,
    key: &str,
    minimum: u64,
    maximum: u64,
) -> Result<Option<u64>, String> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_u64()
        .ok_or_else(|| format!("{key} must be an integer"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{key} is outside the allowed range"));
    }
    Ok(Some(value))
}

fn require_enum(
    args: &serde_json::Map<String, Value>,
    key: &str,
    allowed: &[&str],
) -> Result<(), String> {
    let value = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is required"))?;
    allowed
        .contains(&value)
        .then_some(())
        .ok_or_else(|| format!("invalid {key}"))
}

pub fn validate_workspace_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path
            .split('/')
            .next()
            .is_some_and(|part| part.contains(':'))
        || path
            .split('/')
            .any(|part| part == ".." || part.is_empty() && !path.is_empty())
    {
        return Err("workspace path must be relative and cannot escape the workspace".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_selection_controls_exact_core_tool_registry() {
        assert!(core_tool_registry(false).is_empty());
        let tools = core_tool_registry(true);
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            CORE_TOOL_NAMES
        );
        assert_eq!(tools[0].replay_policy, ReplayPolicyV1::Safe);
        assert!(
            tools[1..]
                .iter()
                .all(|tool| tool.replay_policy == ReplayPolicyV1::Never)
        );
    }

    #[test]
    fn workspace_paths_reject_absolute_escape_and_windows_forms() {
        for invalid in [
            "",
            "/etc/passwd",
            "../secret",
            "a/../secret",
            "a\\b",
            "C:/Windows",
        ] {
            assert!(validate_workspace_path(invalid).is_err(), "{invalid}");
        }
        assert!(validate_workspace_path("relative/path.txt").is_ok());
    }

    #[test]
    fn bash_requires_argv_and_edit_requires_exact_hash() {
        assert!(
            validate_core_tool_call(&ToolCallV1 {
                call_id: "1".into(),
                name: "bash".into(),
                arguments: json!({"command":"echo unsafe"})
            })
            .is_err()
        );
        assert!(validate_core_tool_call(&ToolCallV1 { call_id:"2".into(), name:"edit".into(), arguments:json!({"path":"a","expectedHash":"sha256:bad","edits":[{"startByte":0,"endByte":0,"replacement":"x"}],"encoding":"utf8"}) }).is_err());
        assert!(
            validate_core_tool_call(&ToolCallV1 {
                call_id: "3".into(),
                name: "bash".into(),
                arguments: json!({"argv":["env"],"env":{"SECRET":"value"}})
            })
            .is_err()
        );
    }
}
