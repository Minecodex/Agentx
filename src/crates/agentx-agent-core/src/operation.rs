use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ReplayPolicyV1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKindV1 {
    Model,
    Tool,
    Compaction,
    StateTransition,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum OperationPhaseV1 {
    IntentPersisted,
    EffectStarted,
    Pending { token: String },
    EffectCompleted { result: Value },
    SettledSuccess { result: Value },
    SettledFailure { error: String },
    UnknownOutcome { reason: String },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationRecordV1 {
    pub operation_id: String,
    pub effect_id: String,
    pub operation_kind: OperationKindV1,
    pub attempt: u32,
    pub intent: Value,
    pub effect_identity: String,
    pub replay_policy: ReplayPolicyV1,
    pub phase: OperationPhaseV1,
    pub fencing_token: u64,
    pub created_at_millis: u64,
    pub updated_at_millis: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "status", content = "result", rename_all = "snake_case")]
pub enum EffectObservationV1 {
    NotStarted,
    Pending(String),
    Confirmed(Value),
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDecisionV1 {
    CreateNewOperation,
    ExecuteWithSameId,
    SettleObservedEffect,
    RetryWithSameId,
    ResumeAfterSettlement,
    PauseUnknownOutcome,
    ResumePending,
}

pub fn recovery_decision_for(
    operation: Option<&OperationRecordV1>,
    observation: &EffectObservationV1,
) -> RecoveryDecisionV1 {
    operation.map_or(RecoveryDecisionV1::CreateNewOperation, |operation| {
        recovery_decision(operation, observation)
    })
}

pub fn recovery_decision(
    operation: &OperationRecordV1,
    observation: &EffectObservationV1,
) -> RecoveryDecisionV1 {
    match &operation.phase {
        OperationPhaseV1::SettledSuccess { .. } | OperationPhaseV1::SettledFailure { .. } => {
            RecoveryDecisionV1::ResumeAfterSettlement
        }
        OperationPhaseV1::Pending { .. } => RecoveryDecisionV1::ResumePending,
        OperationPhaseV1::UnknownOutcome { .. } => RecoveryDecisionV1::PauseUnknownOutcome,
        OperationPhaseV1::IntentPersisted => match observation {
            EffectObservationV1::Pending(_) => RecoveryDecisionV1::ResumePending,
            EffectObservationV1::NotStarted => RecoveryDecisionV1::ExecuteWithSameId,
            EffectObservationV1::Confirmed(_) => RecoveryDecisionV1::SettleObservedEffect,
            EffectObservationV1::Unknown => RecoveryDecisionV1::PauseUnknownOutcome,
        },
        OperationPhaseV1::EffectStarted | OperationPhaseV1::EffectCompleted { .. } => {
            match observation {
                EffectObservationV1::Pending(_) => RecoveryDecisionV1::ResumePending,
                EffectObservationV1::Confirmed(_) => RecoveryDecisionV1::SettleObservedEffect,
                EffectObservationV1::NotStarted
                    if matches!(operation.replay_policy, ReplayPolicyV1::Safe) =>
                {
                    RecoveryDecisionV1::RetryWithSameId
                }
                EffectObservationV1::NotStarted | EffectObservationV1::Unknown => {
                    RecoveryDecisionV1::PauseUnknownOutcome
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn operation(phase: OperationPhaseV1, replay_policy: ReplayPolicyV1) -> OperationRecordV1 {
        OperationRecordV1 {
            operation_id: "op-1".into(),
            effect_id: "effect-1".into(),
            operation_kind: OperationKindV1::Tool,
            attempt: 1,
            intent: json!({}),
            effect_identity: "stable-key".into(),
            replay_policy,
            phase,
            fencing_token: 7,
            created_at_millis: 1,
            updated_at_millis: 1,
        }
    }

    #[test]
    fn crash_matrix_never_blindly_replays_unsafe_effects() {
        let started = operation(OperationPhaseV1::EffectStarted, ReplayPolicyV1::Never);
        assert_eq!(
            recovery_decision(&started, &EffectObservationV1::Unknown),
            RecoveryDecisionV1::PauseUnknownOutcome
        );
        assert_eq!(
            recovery_decision(&started, &EffectObservationV1::NotStarted),
            RecoveryDecisionV1::PauseUnknownOutcome
        );
    }

    #[test]
    fn crash_matrix_reconciles_or_retries_only_when_proven() {
        let intent = operation(OperationPhaseV1::IntentPersisted, ReplayPolicyV1::Never);
        assert_eq!(
            recovery_decision(&intent, &EffectObservationV1::NotStarted),
            RecoveryDecisionV1::ExecuteWithSameId
        );
        assert_eq!(
            recovery_decision(&intent, &EffectObservationV1::Confirmed(json!({"ok":true}))),
            RecoveryDecisionV1::SettleObservedEffect
        );
        let safe = operation(OperationPhaseV1::EffectStarted, ReplayPolicyV1::Safe);
        assert_eq!(
            recovery_decision(&safe, &EffectObservationV1::NotStarted),
            RecoveryDecisionV1::RetryWithSameId
        );
    }

    #[test]
    fn settled_effect_resumes_without_execution() {
        let settled = operation(
            OperationPhaseV1::SettledSuccess { result: json!({}) },
            ReplayPolicyV1::Never,
        );
        assert_eq!(
            recovery_decision(&settled, &EffectObservationV1::Unknown),
            RecoveryDecisionV1::ResumeAfterSettlement
        );
    }

    #[test]
    fn every_persist_effect_settlement_crash_boundary_converges() {
        assert_eq!(
            recovery_decision_for(None, &EffectObservationV1::NotStarted),
            RecoveryDecisionV1::CreateNewOperation
        );
        let intent = operation(OperationPhaseV1::IntentPersisted, ReplayPolicyV1::Never);
        assert_eq!(
            recovery_decision_for(Some(&intent), &EffectObservationV1::NotStarted),
            RecoveryDecisionV1::ExecuteWithSameId
        );
        let started = operation(OperationPhaseV1::EffectStarted, ReplayPolicyV1::Never);
        assert_eq!(
            recovery_decision_for(
                Some(&started),
                &EffectObservationV1::Confirmed(json!({"result":"durable"}))
            ),
            RecoveryDecisionV1::SettleObservedEffect
        );
        let completed = operation(
            OperationPhaseV1::EffectCompleted {
                result: json!({"result":"durable"}),
            },
            ReplayPolicyV1::Never,
        );
        assert_eq!(
            recovery_decision_for(
                Some(&completed),
                &EffectObservationV1::Confirmed(json!({"result":"durable"}))
            ),
            RecoveryDecisionV1::SettleObservedEffect
        );
        let settled = operation(
            OperationPhaseV1::SettledSuccess {
                result: json!({"result":"durable"}),
            },
            ReplayPolicyV1::Never,
        );
        assert_eq!(
            recovery_decision_for(Some(&settled), &EffectObservationV1::Unknown),
            RecoveryDecisionV1::ResumeAfterSettlement
        );
        assert_eq!(
            recovery_decision_for(Some(&settled), &EffectObservationV1::Unknown),
            RecoveryDecisionV1::ResumeAfterSettlement,
            "a second crash during recovery must make the same decision"
        );
    }
}
