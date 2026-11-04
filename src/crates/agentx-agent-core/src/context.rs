use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ports::ModelContentV1;
use crate::{AgentMessageV1, MessageRole};

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionKindV1 {
    Threshold,
    Overflow,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompactionSnapshotV1 {
    pub snapshot_id: String,
    pub kind: CompactionKindV1,
    pub summary: String,
    pub through_message_index: usize,
    pub retained_tail: Vec<AgentMessageV1>,
    pub tokens_before: u64,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSessionStateV1 {
    pub session_id: String,
    pub version: u64,
    pub messages: Vec<AgentMessageV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionSnapshotV1>,
    #[serde(default)]
    pub steering_queue: Vec<AgentMessageV1>,
    #[serde(default)]
    pub follow_up_queue: Vec<AgentMessageV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<crate::OperationRecordV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<crate::TerminalReasonV1>,
    /// Durable marker for the one overflow-triggered retry allowed for a
    /// Model Effect.  It is tied to the retry operation identity rather than
    /// the latest compaction kind, so a later Session turn can compact again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overflow_retry_operation_id: Option<String>,
}

pub struct ContextProjector;

impl ContextProjector {
    /// Projects the latest summary, its retained tail, and messages appended
    /// after the compaction cut. Stored history remains untouched.
    pub fn project(state: &AgentSessionStateV1) -> Vec<AgentMessageV1> {
        Self::project_with_system_prompt(state, None)
    }

    /// Projects the model-visible context without mutating durable history.
    /// The system prompt is supplied by the frozen Agent configuration and is
    /// therefore emitted before Bundle External Context and the compacted
    /// conversation projection on every turn.
    pub fn project_with_system_prompt(
        state: &AgentSessionStateV1,
        system_prompt: Option<&str>,
    ) -> Vec<AgentMessageV1> {
        let external_contexts = state
            .messages
            .iter()
            .filter(|message| {
                matches!(message.role, MessageRole::ExternalContext)
                    && message.message_id.starts_with("external-context:")
            })
            .cloned()
            .collect::<Vec<_>>();
        let system = system_prompt
            .filter(|value| !value.trim().is_empty())
            .map(|content| AgentMessageV1 {
                message_id: "system-prompt".into(),
                role: MessageRole::ExternalContext,
                content: ModelContentV1::Text(content.to_owned()),
                tool_calls: Vec::new(),
                tool_call_id: None,
                is_error: false,
            });
        let Some(compaction) = &state.compaction else {
            let mut projected = Vec::with_capacity(
                if system.is_some() { 1 } else { 0 }
                    + external_contexts.len()
                    + state.messages.len(),
            );
            if let Some(system) = system {
                projected.push(system);
            }
            projected.extend(external_contexts.clone());
            projected.extend(
                state
                    .messages
                    .iter()
                    .filter(|message| {
                        !(matches!(message.role, MessageRole::ExternalContext)
                            && message.message_id.starts_with("external-context:"))
                    })
                    .cloned(),
            );
            Self::bound_images(&mut projected);
            return projected;
        };
        let mut projected = Vec::with_capacity(
            (if system.is_some() { 1 } else { 0 })
                + external_contexts.len()
                + 1
                + compaction.retained_tail.len()
                + state.messages.len(),
        );
        if let Some(system) = system {
            projected.push(system);
        }
        projected.extend(external_contexts);
        projected.push(AgentMessageV1 {
            message_id: format!("compaction:{}", compaction.snapshot_id),
            role: MessageRole::ExternalContext,
            content: ModelContentV1::Text(compaction.summary.clone()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            is_error: false,
        });
        projected.extend(
            compaction
                .retained_tail
                .iter()
                .filter(|message| !message.message_id.starts_with("external-context:"))
                .cloned(),
        );
        projected.extend(
            state
                .messages
                .iter()
                // The retained tail is already emitted above.  Skip both the
                // summarized prefix and those retained entries to avoid
                // duplicating messages after a restart.
                .skip(
                    compaction
                        .through_message_index
                        .saturating_add(1)
                        .saturating_add(compaction.retained_tail.len()),
                )
                .filter(|message| !message.message_id.starts_with("external-context:"))
                .cloned(),
        );
        Self::bound_images(&mut projected);
        projected
    }

    // Retain recent visual observations in the model window, while full history stays intact.
    fn bound_images(projected: &mut [AgentMessageV1]) {
        let mut images = 0;
        for message in projected.iter_mut().rev() {
            if message.role != MessageRole::ToolObservation {
                continue;
            }
            if let crate::ModelContentV1::Parts(parts) = &mut message.content {
                for part in parts.iter_mut().rev() {
                    if matches!(part, crate::ModelContentPartV1::ImageUrl { .. }) {
                        images += 1;
                        if images > 4 {
                            *part = crate::ModelContentPartV1::Text { text: "An older screenshot is omitted from this model window. Request a fresh observation if needed.".into() };
                        }
                    }
                }
            }
        }
    }

    pub fn estimated_tokens(messages: &[AgentMessageV1]) -> u64 {
        messages
            .iter()
            .map(|message| {
                let content = match &message.content {
                    crate::ModelContentV1::Text(text) => (text.len() as u64).div_ceil(4),
                    crate::ModelContentV1::Parts(parts) => parts
                        .iter()
                        .map(|part| match part {
                            crate::ModelContentPartV1::Text { text } => {
                                (text.len() as u64).div_ceil(4)
                            }
                            // Bounded preview images reserve context conservatively; base64 is not text.
                            crate::ModelContentPartV1::ImageUrl { .. } => 8192,
                            crate::ModelContentPartV1::InputAudio { input_audio } => {
                                (input_audio.data.len() as u64).div_ceil(4)
                            }
                        })
                        .sum(),
                };
                let arguments = message
                    .tool_calls
                    .iter()
                    .map(|call| (call.arguments.to_string().len() as u64).div_ceil(4))
                    .sum::<u64>();
                content + arguments + 4
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_retention_does_not_remove_user_image_inputs() {
        let parts = (0..6)
            .map(|index| crate::ModelContentPartV1::ImageUrl {
                image_url: crate::ImageUrlV1 {
                    url: format!("data:image/webp;base64,user-{index}"),
                },
            })
            .collect::<Vec<_>>();
        let state = AgentSessionStateV1 {
            messages: vec![AgentMessageV1 {
                message_id: "user-images".into(),
                role: MessageRole::User,
                content: ModelContentV1::Parts(parts.clone()),
                tool_calls: vec![],
                tool_call_id: None,
                is_error: false,
            }],
            ..Default::default()
        };
        assert_eq!(
            ContextProjector::project(&state)[0].content,
            ModelContentV1::Parts(parts)
        );
    }

    #[test]
    fn visual_window_is_bounded_before_and_after_compaction_without_losing_history() {
        let images = (0..7)
            .map(|index| AgentMessageV1 {
                message_id: format!("visual-{index}"),
                role: MessageRole::ToolObservation,
                content: ModelContentV1::Parts(vec![crate::ModelContentPartV1::ImageUrl {
                    image_url: crate::ImageUrlV1 {
                        url: format!("agentx-artifact:{index}"),
                    },
                }]),
                tool_calls: vec![],
                tool_call_id: Some(format!("call-{index}")),
                is_error: false,
            })
            .collect::<Vec<_>>();
        for compacted in [false, true] {
            let state = AgentSessionStateV1 {
                messages: images.clone(),
                compaction: compacted.then(|| CompactionSnapshotV1 {
                    snapshot_id: "c".into(),
                    kind: CompactionKindV1::Threshold,
                    summary: "Earlier observations".into(),
                    through_message_index: 0,
                    retained_tail: images.iter().skip(1).cloned().collect(),
                    tokens_before: 60000,
                }),
                ..Default::default()
            };
            let projected = ContextProjector::project(&state);
            let visible = projected
                .iter()
                .flat_map(|message| match &message.content {
                    ModelContentV1::Parts(parts) => parts.as_slice(),
                    _ => &[],
                })
                .filter(|part| matches!(part, crate::ModelContentPartV1::ImageUrl { .. }))
                .count();
            assert_eq!(visible, 4);
            assert_eq!(state.messages, images);
            assert!(ContextProjector::estimated_tokens(&projected) >= 4 * 8192);
        }
    }

    #[test]
    fn external_context_is_first_and_never_accumulates_on_projection() {
        let state = AgentSessionStateV1 {
            session_id: "s".into(),
            messages: vec![
                AgentMessageV1 {
                    message_id: "external-context:skill-1".into(),
                    role: MessageRole::ExternalContext,
                    content: "instructions".into(),
                    tool_calls: vec![],
                    tool_call_id: None,
                    is_error: false,
                },
                AgentMessageV1::user("u", "hello"),
            ],
            ..AgentSessionStateV1::default()
        };
        let projected = ContextProjector::project(&state);
        assert_eq!(projected[0].message_id, "external-context:skill-1");
        assert_eq!(projected.len(), 2);
    }

    #[test]
    fn compaction_projection_does_not_duplicate_retained_tail() {
        let state = AgentSessionStateV1 {
            session_id: "s".into(),
            messages: vec![
                AgentMessageV1::user("u1", "old"),
                AgentMessageV1::assistant("a1", "tail", vec![]),
                AgentMessageV1::user("u2", "new"),
            ],
            compaction: Some(CompactionSnapshotV1 {
                snapshot_id: "c".into(),
                kind: CompactionKindV1::Threshold,
                summary: "summary".into(),
                through_message_index: 0,
                retained_tail: vec![AgentMessageV1::assistant("a1", "tail", vec![])],
                tokens_before: 3,
            }),
            ..AgentSessionStateV1::default()
        };
        let projected = ContextProjector::project(&state);
        let ids = projected
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["compaction:c", "a1", "u2"]);
    }

    #[test]
    fn system_prompt_precedes_external_context_and_history_without_persistence() {
        let state = AgentSessionStateV1 {
            session_id: "s".into(),
            messages: vec![
                AgentMessageV1 {
                    message_id: "external-context:skill".into(),
                    role: MessageRole::ExternalContext,
                    content: "skill".into(),
                    tool_calls: vec![],
                    tool_call_id: None,
                    is_error: false,
                },
                AgentMessageV1::user("u", "question"),
            ],
            ..AgentSessionStateV1::default()
        };
        let projected = ContextProjector::project_with_system_prompt(&state, Some("rules"));
        assert_eq!(projected[0].message_id, "system-prompt");
        assert_eq!(projected[1].message_id, "external-context:skill");
        assert_eq!(projected[2].message_id, "u");
        assert_eq!(state.messages.len(), 2);
    }
}
