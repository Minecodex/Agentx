use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionDepartmentSnapshotV1 {
    pub id: Uuid,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionUserSnapshotV1 {
    pub id: Uuid,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionRoleAssignmentV1 {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub data_scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_department: Option<ExecutionDepartmentSnapshotV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionRolesSnapshotV1 {
    pub ids: Vec<Uuid>,
    pub codes: Vec<String>,
    pub names: Vec<String>,
    pub assignments: Vec<ExecutionRoleAssignmentV1>,
}

impl ExecutionRolesSnapshotV1 {
    #[must_use]
    pub fn from_assignments(assignments: Vec<ExecutionRoleAssignmentV1>) -> Self {
        Self {
            ids: assignments.iter().map(|role| role.id).collect(),
            codes: assignments.iter().map(|role| role.code.clone()).collect(),
            names: assignments.iter().map(|role| role.name.clone()).collect(),
            assignments,
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionWorkflowSnapshotV1 {
    pub id: Uuid,
    pub name: String,
    pub version_id: Option<Uuid>,
    pub version_number: Option<u64>,
    pub source: crate::WorkflowDesignSourceV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_department: Option<ExecutionDepartmentSnapshotV1>,
}

impl ExecutionWorkflowSnapshotV1 {
    pub fn bind_source(&mut self, source: crate::WorkflowDesignSourceV1) {
        match &source {
            crate::WorkflowDesignSourceV1::Version {
                version_id,
                version_number,
            } => {
                self.version_id = Some(*version_id);
                self.version_number = Some(*version_number);
            }
            crate::WorkflowDesignSourceV1::DraftRevision { .. } => {
                self.version_id = None;
                self.version_number = None;
            }
        }
        self.source = source;
    }
    pub fn source_is_valid(&self) -> bool {
        self.source.is_valid()
            && match &self.source {
                crate::WorkflowDesignSourceV1::Version {
                    version_id,
                    version_number,
                } => {
                    self.version_id == Some(*version_id)
                        && self.version_number == Some(*version_number)
                }
                crate::WorkflowDesignSourceV1::DraftRevision { .. } => {
                    self.version_id.is_none() && self.version_number.is_none()
                }
            }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionTriggerSnapshotV1 {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionInitiatorSnapshotV1 {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<ExecutionUserSnapshotV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<ExecutionDepartmentSnapshotV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<ExecutionRolesSnapshotV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionApplicationSnapshotV1 {
    pub id: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionInvocationSnapshotV1 {
    pub id: Uuid,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionSessionSnapshotV1 {
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_user_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionNodeSnapshotV1 {
    pub id: String,
    pub execution_id: Uuid,
    pub run_index: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_iteration_index: Option<u64>,
}

/// Immutable expression-visible metadata captured when an Execution is created.
/// Resource authorization deliberately does not consume this value.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionContextSnapshotV1 {
    pub end_user: Option<crate::ApplicationUserIdentityV1>,
    pub environment_id: Option<Uuid>,
    pub id: Uuid,
    #[schemars(with = "String")]
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_execution_id: Option<Uuid>,
    pub workflow: ExecutionWorkflowSnapshotV1,
    pub trigger: ExecutionTriggerSnapshotV1,
    pub initiator: ExecutionInitiatorSnapshotV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<ExecutionApplicationSnapshotV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invocation: Option<ExecutionInvocationSnapshotV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<ExecutionSessionSnapshotV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<ExecutionNodeSnapshotV1>,
}

/// Immutable audit context captured when an Execution is created.
///
/// Control remains authoritative for current IAM and resource names. Runtime
/// stores this snapshot so later renames do not rewrite execution history.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionOriginV1 {
    pub initiator_user_id: Option<Uuid>,
    pub initiator_user_name: Option<String>,
    pub initiator_department_id: Option<Uuid>,
    pub initiator_department_name: Option<String>,
    pub trigger_source_id: Option<Uuid>,
    pub trigger_name: Option<String>,
    pub role_assignments: Vec<ExecutionRoleAssignmentV1>,
}

impl ExecutionOriginV1 {
    #[must_use]
    pub const fn system(trigger_source_id: Option<Uuid>) -> Self {
        Self {
            initiator_user_id: None,
            initiator_user_name: None,
            initiator_department_id: None,
            initiator_department_name: None,
            trigger_source_id,
            trigger_name: None,
            role_assignments: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn roles_derive_stable_arrays_and_reject_unknown_fields() {
        let role = ExecutionRoleAssignmentV1 {
            id: Uuid::from_u128(1),
            code: "workflow_operator".into(),
            name: "Workflow Operator".into(),
            data_scope: "department_tree".into(),
            scope_department: Some(ExecutionDepartmentSnapshotV1 {
                id: Uuid::from_u128(2),
                name: "Platform".into(),
            }),
        };
        let roles = ExecutionRolesSnapshotV1::from_assignments(vec![role]);
        assert_eq!(roles.ids, vec![Uuid::from_u128(1)]);
        assert_eq!(roles.codes, vec!["workflow_operator"]);
        assert_eq!(roles.names, vec!["Workflow Operator"]);
        assert!(
            serde_json::from_value::<ExecutionRoleAssignmentV1>(json!({
                "id": Uuid::from_u128(1),
                "code": "workflow_operator",
                "name": "Workflow Operator",
                "dataScope": "company",
                "permissions": ["resource:admin"]
            }))
            .is_err()
        );
    }
}
