use agentx_runtime_contracts::{
    AdmissionTargetV1, ExecutionDepartmentSnapshotV1, ExecutionRoleAssignmentV1,
    RuntimeUserAdmissionV1, RuntimeUserApplicationGrantV1,
};
use anyhow::Result;
use sqlx::{MySql, Row, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

const APPLICATION_USER_QUERY: &str = "SELECT u.id,u.display_name,ud.department_id,d.name department_name,u.token_version,u.status,EXISTS(SELECT 1 FROM user_roles ur JOIN roles r ON r.id=ur.role_id AND r.tenant_id=ur.tenant_id AND r.status='active' JOIN role_permissions rp ON rp.role_id=r.id AND rp.tenant_id=r.tenant_id JOIN permissions p ON p.id=rp.permission_id WHERE ur.tenant_id=u.tenant_id AND ur.user_id=u.id AND p.permission_key='application:invoke' AND (a.owner_user_id=u.id OR r.data_scope='company' OR (r.data_scope='department_tree' AND EXISTS(SELECT 1 FROM department_closure scoped WHERE scoped.tenant_id=a.tenant_id AND scoped.ancestor_id=COALESCE(ur.scope_department_id,ud.department_id) AND scoped.descendant_id=a.owner_department_id)) OR a.visibility='company' OR (a.visibility='department' AND EXISTS(SELECT 1 FROM department_closure visible WHERE visible.tenant_id=a.tenant_id AND ((visible.ancestor_id=a.owner_department_id AND visible.descendant_id=ud.department_id) OR (visible.ancestor_id=ud.department_id AND visible.descendant_id=a.owner_department_id)))))) can_invoke,EXISTS(SELECT 1 FROM user_roles qur JOIN roles qr ON qr.id=qur.role_id AND qr.tenant_id=qur.tenant_id AND qr.status='active' AND qr.data_scope='company' JOIN role_permissions qrp ON qrp.role_id=qr.id AND qrp.tenant_id=qr.tenant_id JOIN permissions qp ON qp.id=qrp.permission_id AND qp.permission_key='execution:view' WHERE qur.tenant_id=u.tenant_id AND qur.user_id=u.id) tenant_query_enabled FROM users u JOIN user_departments ud ON ud.tenant_id=u.tenant_id AND ud.user_id=u.id JOIN departments d ON d.tenant_id=ud.tenant_id AND d.id=ud.department_id JOIN applications a ON a.tenant_id=u.tenant_id AND a.id=? WHERE u.tenant_id=? AND (? IS NULL OR u.id=?) ORDER BY u.id";

/// Materializes the smallest user admission projection needed by Runtime.
///
/// Roles, departments and visibility remain Control facts. Runtime receives an
/// immutable-ready display projection plus one exact Application grant per
/// Control user; resource authorization still uses Service Identity grants.
pub async fn application_user_targets(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    application_id: Uuid,
    grant_version: u64,
) -> Result<Vec<AdmissionTargetV1>> {
    let rows = sqlx::query(APPLICATION_USER_QUERY)
        .bind(application_id)
        .bind(tenant_id)
        .bind(Option::<Uuid>::None)
        .bind(Option::<Uuid>::None)
        .fetch_all(pool)
        .await?;
    let mut roles_by_user = role_assignments_by_user(pool, tenant_id, None).await?;
    let mut targets = Vec::with_capacity(rows.len() * 2);
    for row in rows {
        let user_id: Uuid = row.try_get("id")?;
        let enabled = row.try_get::<String, _>("status")? == "active";
        targets.push(AdmissionTargetV1::RuntimeUser {
            state: RuntimeUserAdmissionV1 {
                tenant_id,
                user_id,
                user_name: row.try_get("display_name")?,
                department_id: row.try_get("department_id")?,
                department_name: row.try_get("department_name")?,
                token_version: row.try_get("token_version")?,
                enabled,
                tenant_query_enabled: enabled && row.try_get::<bool, _>("tenant_query_enabled")?,
                role_assignments: roles_by_user.remove(&user_id).unwrap_or_default(),
            },
        });
        targets.push(AdmissionTargetV1::RuntimeUserApplicationGrant {
            state: RuntimeUserApplicationGrantV1 {
                tenant_id,
                user_id,
                application_id,
                grant_version,
                can_invoke: enabled && row.try_get::<bool, _>("can_invoke")?,
                can_query: enabled
                    && (row.try_get::<bool, _>("can_invoke")?
                        || row.try_get::<bool, _>("tenant_query_enabled")?),
            },
        });
    }
    Ok(targets)
}

pub async fn user_role_assignments(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<ExecutionRoleAssignmentV1>, sqlx::Error> {
    Ok(role_assignments_by_user(pool, tenant_id, Some(user_id))
        .await?
        .remove(&user_id)
        .unwrap_or_default())
}

/// Refreshes existing Application grants in the same transaction as IAM facts.
/// Each grant uses its Application admission epoch, not the User token version.
pub(crate) async fn emit_user_application_grants(
    tx: &mut Transaction<'_, MySql>,
    tenant_id: Uuid,
    user_id: Uuid,
) -> crate::api_error::ApiResult<()> {
    let applications =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM applications WHERE tenant_id=? ORDER BY id")
            .bind(tenant_id)
            .fetch_all(&mut **tx)
            .await?;
    for application_id in applications {
        let row = sqlx::query(APPLICATION_USER_QUERY)
            .bind(application_id)
            .bind(tenant_id)
            .bind(user_id)
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await?;
        let enabled = row.try_get::<String, _>("status")? == "active";
        let can_invoke = enabled && row.try_get::<bool, _>("can_invoke")?;
        let can_query =
            enabled && (can_invoke || row.try_get::<bool, _>("tenant_query_enabled")?);
        crate::control_api::admission_outbox(
            tx,
            tenant_id,
            application_id,
            "RuntimeUserApplicationGrantChanged",
            serde_json::json!({"userId":user_id,"canInvoke":can_invoke,"canQuery":can_query}),
        )
        .await?;
    }
    Ok(())
}

async fn role_assignments_by_user(
    pool: &sqlx::MySqlPool,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<BTreeMap<Uuid, Vec<ExecutionRoleAssignmentV1>>, sqlx::Error> {
    let query = "SELECT ur.user_id,r.id,r.code,r.name,r.data_scope,sd.id scope_department_id,sd.name scope_department_name FROM user_roles ur JOIN roles r ON r.tenant_id=ur.tenant_id AND r.id=ur.role_id AND r.status='active' LEFT JOIN departments sd ON sd.tenant_id=ur.tenant_id AND sd.id=ur.scope_department_id WHERE ur.tenant_id=? AND (? IS NULL OR ur.user_id=?) ORDER BY ur.user_id,r.code,r.id";
    let rows = sqlx::query(query)
        .bind(tenant_id)
        .bind(user_id)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    let mut roles_by_user = BTreeMap::<Uuid, Vec<ExecutionRoleAssignmentV1>>::new();
    for row in rows {
        let scope_department_id: Option<Uuid> = row.try_get("scope_department_id")?;
        roles_by_user
            .entry(row.try_get("user_id")?)
            .or_default()
            .push(ExecutionRoleAssignmentV1 {
                id: row.try_get("id")?,
                code: row.try_get("code")?,
                name: row.try_get("name")?,
                data_scope: row.try_get("data_scope")?,
                scope_department: scope_department_id.map(|id| ExecutionDepartmentSnapshotV1 {
                    id,
                    name: row.try_get("scope_department_name").unwrap_or_default(),
                }),
            });
    }
    Ok(roles_by_user)
}

#[cfg(test)]
mod tests {
    use sqlx::mysql::MySqlPoolOptions;
    use testcontainers::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    };
    use uuid::Uuid;

    use super::application_user_targets;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn projects_company_department_owner_and_revoked_grants_without_iam_rows() {
        let container = GenericImage::new("mysql", "8.4")
            .with_exposed_port(3306.tcp())
            .with_wait_for(WaitFor::message_on_stdout(
                "MySQL init process done. Ready for start up.",
            ))
            .with_wait_for(WaitFor::message_on_stderr("ready for connections"))
            .with_env_var("MYSQL_DATABASE", "agentx_control")
            .with_env_var("MYSQL_USER", "agentx")
            .with_env_var("MYSQL_PASSWORD", "agentx-test-password")
            .with_env_var("MYSQL_ROOT_PASSWORD", "agentx-root-password")
            .start()
            .await
            .expect("Control MySQL container should start");
        let port = container.get_host_port_ipv4(3306.tcp()).await.unwrap();
        let pool = connect_with_retry(port).await;
        agentx_control_infrastructure::migrate_control_mysql(&pool)
            .await
            .unwrap();

        let tenant = Uuid::now_v7();
        let root = Uuid::now_v7();
        let child = Uuid::now_v7();
        let owner = Uuid::now_v7();
        let company = Uuid::now_v7();
        let department = Uuid::now_v7();
        let forbidden = Uuid::now_v7();
        let disabled = Uuid::now_v7();
        let workflow = Uuid::now_v7();
        let application = Uuid::now_v7();
        sqlx::query("INSERT INTO tenants(id,name,normalized_name) VALUES(?,'Tenant','tenant')")
            .bind(tenant)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO departments(id,tenant_id,name,normalized_name,is_root) VALUES(?,?,'Root','root',TRUE),(?,?,'Child','child',FALSE)")
            .bind(root).bind(tenant).bind(child).bind(tenant).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO department_closure(tenant_id,ancestor_id,descendant_id,depth) VALUES(?,?,?,0),(?,?,?,0),(?,?,?,1)")
            .bind(tenant).bind(root).bind(root)
            .bind(tenant).bind(child).bind(child)
            .bind(tenant).bind(root).bind(child)
            .execute(&pool).await.unwrap();
        for (user, name, status, department_id) in [
            (owner, "owner", "active", child),
            (company, "company", "active", root),
            (department, "department", "active", root),
            (forbidden, "forbidden", "active", root),
            (disabled, "disabled", "disabled", root),
        ] {
            sqlx::query("INSERT INTO users(id,tenant_id,username,username_normalized,display_name,status) VALUES(?,?,?,?,?,?)")
                .bind(user).bind(tenant).bind(name).bind(name).bind(name).bind(status).execute(&pool).await.unwrap();
            sqlx::query(
                "INSERT INTO user_departments(tenant_id,user_id,department_id) VALUES(?,?,?)",
            )
            .bind(tenant)
            .bind(user)
            .bind(department_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO workflows(id,tenant_id,name,owner_user_id,owner_department_id) VALUES(?,?,'Workflow',?,?)")
            .bind(workflow).bind(tenant).bind(owner).bind(child).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO applications(id,tenant_id,workflow_id,name,slug,visibility,owner_user_id,owner_department_id) VALUES(?, ?, ?, 'App', 'app', 'private', ?, ?)")
            .bind(application).bind(tenant).bind(workflow).bind(owner).bind(child).execute(&pool).await.unwrap();
        let permission = Uuid::now_v7();
        sqlx::query("INSERT INTO permissions(id,permission_key,name) VALUES(?,'application:invoke','Invoke')")
            .bind(permission).execute(&pool).await.unwrap();
        for (user, scope, scope_department) in [
            (owner, "own", None),
            (company, "company", None),
            (department, "department_tree", Some(root)),
            (disabled, "company", None),
        ] {
            let role = Uuid::now_v7();
            sqlx::query("INSERT INTO roles(id,tenant_id,code,name,data_scope) VALUES(?,?,?,?,?)")
                .bind(role)
                .bind(tenant)
                .bind(role.to_string())
                .bind(role.to_string())
                .bind(scope)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO role_permissions(tenant_id,role_id,permission_id) VALUES(?,?,?)",
            )
            .bind(tenant)
            .bind(role)
            .bind(permission)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO user_roles(id,tenant_id,user_id,role_id,scope_department_id) VALUES(?,?,?,?,?)")
                .bind(Uuid::now_v7()).bind(tenant).bind(user).bind(role).bind(scope_department).execute(&pool).await.unwrap();
        }

        let targets = application_user_targets(&pool, tenant, application, 7)
            .await
            .unwrap();
        let department_projection = targets
            .iter()
            .find_map(|target| match target {
                agentx_runtime_contracts::AdmissionTargetV1::RuntimeUser { state }
                    if state.user_id == department =>
                {
                    Some(state)
                }
                _ => None,
            })
            .expect("department user projection exists");
        assert_eq!(department_projection.role_assignments.len(), 1);
        assert_eq!(
            department_projection.role_assignments[0].data_scope,
            "department_tree"
        );
        assert_eq!(
            department_projection.role_assignments[0]
                .scope_department
                .as_ref()
                .map(|department| (department.id, department.name.as_str())),
            Some((root, "Root"))
        );
        let grants = targets
            .into_iter()
            .filter_map(|target| match target {
                agentx_runtime_contracts::AdmissionTargetV1::RuntimeUserApplicationGrant {
                    state,
                } => Some((state.user_id, state.can_invoke)),
                _ => None,
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert!(grants[&owner]);
        assert!(grants[&company]);
        assert!(grants[&department]);
        assert!(!grants[&forbidden]);
        assert!(!grants[&disabled]);
    }

    async fn connect_with_retry(port: u16) -> sqlx::MySqlPool {
        let url = format!("mysql://agentx:agentx-test-password@127.0.0.1:{port}/agentx_control");
        let mut last_error = None;
        for _ in 0..40 {
            match MySqlPoolOptions::new()
                .max_connections(5)
                .connect(&url)
                .await
            {
                Ok(pool) => return pool,
                Err(error) => last_error = Some(error),
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        panic!("Control MySQL did not become ready: {last_error:?}");
    }
}
