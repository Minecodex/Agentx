use agentx_runtime_contracts::{
    ExecutionApplicationSnapshotV1, ExecutionContextSnapshotV1, ExecutionInitiatorSnapshotV1,
    ExecutionInvocationSnapshotV1, ExecutionOriginV1, ExecutionRolesSnapshotV1,
    ExecutionSessionSnapshotV1, ExecutionTriggerSnapshotV1, ExecutionUserSnapshotV1,
    ExecutionWorkflowSnapshotV1,
};
use serde_json::Value;
use sqlx::{MySql, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{RuntimeError, RuntimeResult};

pub(crate) struct NewExecutionContext<'a> {
    pub execution_id: Uuid,
    pub started_at: OffsetDateTime,
    pub parent_execution_id: Option<Uuid>,
    pub workflow: ExecutionWorkflowSnapshotV1,
    pub trigger_type: &'a str,
    pub origin: &'a ExecutionOriginV1,
    pub application_id: Option<Uuid>,
    pub invocation_id: Option<Uuid>,
    pub session: Option<(Uuid, Option<String>)>,
}

pub(crate) fn create_snapshot(input: NewExecutionContext<'_>) -> ExecutionContextSnapshotV1 {
    let user = input
        .origin
        .initiator_user_id
        .zip(input.origin.initiator_user_name.clone())
        .map(|(id, name)| ExecutionUserSnapshotV1 { id, name });
    let department = input
        .origin
        .initiator_department_id
        .zip(input.origin.initiator_department_name.clone())
        .map(|(id, name)| agentx_runtime_contracts::ExecutionDepartmentSnapshotV1 { id, name });
    let roles = user
        .as_ref()
        .map(|_| ExecutionRolesSnapshotV1::from_assignments(input.origin.role_assignments.clone()));
    ExecutionContextSnapshotV1 {
        end_user: input
            .origin
            .initiator_user_id
            .map(agentx_runtime_contracts::ApplicationUserIdentityV1::platform),
        environment_id: None,
        id: input.execution_id,
        started_at: input.started_at,
        parent_execution_id: input.parent_execution_id,
        workflow: input.workflow,
        trigger: ExecutionTriggerSnapshotV1 {
            kind: input.trigger_type.into(),
            source_id: input.origin.trigger_source_id,
            name: input.origin.trigger_name.clone(),
        },
        initiator: ExecutionInitiatorSnapshotV1 {
            kind: if user.is_some() {
                "user"
            } else {
                input.trigger_type
            }
            .into(),
            user,
            department,
            roles,
        },
        application: input
            .application_id
            .map(|id| ExecutionApplicationSnapshotV1 { id }),
        invocation: input
            .invocation_id
            .map(|id| ExecutionInvocationSnapshotV1 { id }),
        session: input
            .session
            .map(|(id, external_user_id)| ExecutionSessionSnapshotV1 {
                id,
                external_user_id,
            }),
        node: None,
    }
}

pub(crate) async fn runtime_user_origin(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    user_id: Uuid,
) -> RuntimeResult<ExecutionOriginV1> {
    let row = sqlx::query("SELECT user_name,department_id,department_name,role_assignments_json FROM runtime_user_admission WHERE tenant_id=? AND user_id=? AND status='active'")
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RuntimeError::Unauthorized)?;
    Ok(ExecutionOriginV1 {
        initiator_user_id: Some(user_id),
        initiator_user_name: Some(row.try_get("user_name")?),
        initiator_department_id: Some(row.try_get("department_id")?),
        initiator_department_name: Some(row.try_get("department_name")?),
        trigger_source_id: None,
        trigger_name: None,
        role_assignments: serde_json::from_value(row.try_get("role_assignments_json")?)
            .map_err(|error| RuntimeError::Internal(error.into()))?,
    })
}

pub(crate) async fn load(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    execution_id: Uuid,
) -> RuntimeResult<Value> {
    sqlx::query_scalar(
        "SELECT execution_context_json FROM execution_snapshots WHERE tenant_id=? AND execution_id=?",
    )
    .bind(tenant_id)
    .bind(execution_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

pub(crate) fn with_node(
    mut execution: Value,
    node_id: &str,
    node_execution_id: Uuid,
    run_index: u32,
    item_index: Option<usize>,
    loop_iteration_index: Option<u32>,
) -> Value {
    let Some(root) = execution.as_object_mut() else {
        return execution;
    };
    root.insert(
        "node".into(),
        serde_json::json!({
            "id": node_id,
            "executionId": node_execution_id,
            "runIndex": run_index,
            "itemIndex": item_index,
            "loopIterationIndex": loop_iteration_index,
        }),
    );
    execution
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_metadata_uses_one_stable_shape() {
        let value = with_node(
            serde_json::json!({"id":Uuid::nil()}),
            "http",
            Uuid::from_u128(7),
            2,
            Some(3),
            Some(4),
        );
        assert_eq!(value["node"]["id"], "http");
        assert_eq!(value["node"]["runIndex"], 2);
        assert_eq!(value["node"]["itemIndex"], 3);
        assert!(value.get("executionId").is_none());
    }

    #[test]
    fn human_roles_are_derived_but_system_triggers_omit_human_fields() {
        let user_id = Uuid::from_u128(10);
        let role_id = Uuid::from_u128(11);
        let human = create_snapshot(NewExecutionContext {
            execution_id: Uuid::from_u128(1),
            started_at: OffsetDateTime::UNIX_EPOCH,
            parent_execution_id: None,
            workflow: ExecutionWorkflowSnapshotV1 {
                id: Uuid::from_u128(2),
                name: "Workflow".into(),
                source: agentx_runtime_contracts::WorkflowDesignSourceV1::Version {
                    version_id: Uuid::from_u128(3),
                    version_number: 1,
                },
                version_id: Some(Uuid::from_u128(3)),
                version_number: Some(1),
                owner_department: None,
            },
            trigger_type: "debug",
            origin: &ExecutionOriginV1 {
                initiator_user_id: Some(user_id),
                initiator_user_name: Some("Operator".into()),
                initiator_department_id: None,
                initiator_department_name: None,
                trigger_source_id: None,
                trigger_name: None,
                role_assignments: vec![agentx_runtime_contracts::ExecutionRoleAssignmentV1 {
                    id: role_id,
                    code: "operator".into(),
                    name: "Operator".into(),
                    data_scope: "company".into(),
                    scope_department: None,
                }],
            },
            application_id: None,
            invocation_id: None,
            session: None,
        });
        let human = serde_json::to_value(human).unwrap();
        assert_eq!(human["initiator"]["type"], "user");
        assert_eq!(
            human["initiator"]["roles"]["codes"],
            serde_json::json!(["operator"])
        );

        let system = create_snapshot(NewExecutionContext {
            execution_id: Uuid::from_u128(4),
            started_at: OffsetDateTime::UNIX_EPOCH,
            parent_execution_id: None,
            workflow: ExecutionWorkflowSnapshotV1 {
                id: Uuid::from_u128(2),
                name: "Workflow".into(),
                source: agentx_runtime_contracts::WorkflowDesignSourceV1::Version {
                    version_id: Uuid::from_u128(3),
                    version_number: 1,
                },
                version_id: Some(Uuid::from_u128(3)),
                version_number: Some(1),
                owner_department: None,
            },
            trigger_type: "schedule",
            origin: &ExecutionOriginV1::system(None),
            application_id: None,
            invocation_id: None,
            session: None,
        });
        let system = serde_json::to_value(system).unwrap();
        assert!(system["initiator"].get("user").is_none());
        assert!(system["initiator"].get("roles").is_none());
    }
}
