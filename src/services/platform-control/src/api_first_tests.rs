use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{Multipart, Path},
    http::Request,
    routing::post,
};
use http_body_util::BodyExt;
use object_store::memory::InMemory;
use serde_json::{Value, json};
use sqlx::mysql::MySqlPoolOptions;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::net::TcpListener;
use tower::ServiceExt;
use uuid::Uuid;

use crate::control_api::{ControlApiState, router};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn api_first_control_closure_uses_empty_schema_and_public_routes() {
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

    let vault = Router::new().route("/v1/secret/data/{*path}", post(mock_vault_write));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vault_url = format!("http://{}", listener.local_addr().unwrap());
    let vault_server = tokio::spawn(async move { axum::serve(listener, vault).await.unwrap() });
    let runtime = Router::new()
        .route(
            "/internal/runtime/v1/admission-commands:apply",
            post(mock_apply_admission),
        )
        .route(
            "/internal/runtime/v1/work-packages:prepare",
            post(mock_prepare_work_package),
        )
        .route(
            "/internal/runtime/v1/work-packages/{package_action}",
            post(mock_execute_work_package),
        )
        .route(
            "/internal/runtime/v1/plugin-design-operations:execute",
            post(mock_plugin_design_operation),
        )
        .route(
            "/internal/runtime/v1/objects:upload",
            post(mock_upload_object),
        );
    let runtime_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime_url = format!("http://{}", runtime_listener.local_addr().unwrap());
    let runtime_server =
        tokio::spawn(async move { axum::serve(runtime_listener, runtime).await.unwrap() });
    let app = router(ControlApiState::for_test(
        pool.clone(),
        Arc::new(InMemory::new()),
        runtime_url,
        vault_url,
    ));

    let (status, bootstrap) = request(
        &app,
        "POST",
        "/api/v1/bootstrap",
        None,
        Some(json!({
            "companyName":"API First Company",
            "adminUsername":"api-admin",
            "adminDisplayName":"API Admin",
            "password":"api-first-password",
            "locale":"zh-CN",
            "timezone":"Asia/Shanghai"
        })),
    )
    .await;
    assert_eq!(status, 201, "bootstrap response: {bootstrap}");
    let token = bootstrap["accessToken"].as_str().unwrap().to_owned();
    let department_id = bootstrap["user"]["departmentId"].as_str().unwrap();
    let department_review_permissions: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT p.permission_key) FROM roles r JOIN role_permissions rp ON rp.tenant_id=r.tenant_id AND rp.role_id=r.id JOIN permissions p ON p.id=rp.permission_id WHERE r.code='department_admin' AND p.permission_key IN ('approval:view','approval:act','resource:grant','notification:view')")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(department_review_permissions, 4);
    sqlx::query(
        "INSERT INTO runtime_projection_status(projection_name,partition_key,state,current_cursor,active_generation,last_success_at) VALUES('runtime_governance_v1','global','ready',0,1,UTC_TIMESTAMP(6))",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (status, login) = request(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"username":"api-admin","password":"api-first-password"})),
    )
    .await;
    assert_eq!(status, 200, "login response: {login}");

    let (status, roles) = request(&app, "GET", "/api/v1/roles", Some(&token), None).await;
    assert_eq!(status, 200, "role list response: {roles}");
    let company_role_id = roles["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|role| role["code"] == "company_admin")
        .and_then(|role| role["id"].as_str())
        .unwrap();
    let (status, user) = request(
        &app,
        "POST",
        "/api/v1/users",
        Some(&token),
        Some(json!({
            "username":"api-operator","displayName":"API Operator",
            "departmentId":department_id,"roleId":company_role_id
        })),
    )
    .await;
    assert_eq!(status, 201, "user response: {user}");
    let (status, projection_department) = request(
        &app,
        "POST",
        "/api/v1/departments",
        Some(&token),
        Some(json!({"parentId":department_id,"name":"Projection Department"})),
    )
    .await;
    assert_eq!(
        status, 201,
        "projection department: {projection_department}"
    );
    let (status, projection_role) = request(
        &app,
        "POST",
        "/api/v1/roles",
        Some(&token),
        Some(json!({
            "code":"workflow_operator_fixture",
            "name":"Workflow Operator Fixture",
            "description":"Execution context projection fixture",
            "dataScope":"company",
            "permissions":["application:invoke","execution:view"]
        })),
    )
    .await;
    assert_eq!(status, 201, "projection role: {projection_role}");
    let (status, projection_user) = request(
        &app,
        "POST",
        "/api/v1/users",
        Some(&token),
        Some(json!({
            "username":"projection-user",
            "displayName":"Projection User",
            "departmentId":projection_department["id"],
            "roleId":projection_role["id"]
        })),
    )
    .await;
    assert_eq!(status, 201, "projection user: {projection_user}");
    let (status, renamed_role) = request(
        &app,
        "PATCH",
        &format!("/api/v1/roles/{}", projection_role["id"].as_str().unwrap()),
        Some(&token),
        Some(json!({
            "name":"Renamed Workflow Operator",
            "description":"Renamed execution context projection fixture",
            "dataScope":"company",
            "permissions":["application:invoke","execution:view"],
            "version":1
        })),
    )
    .await;
    assert_eq!(status, 200, "renamed role: {renamed_role}");
    let (status, renamed_department) = request(
        &app,
        "PATCH",
        &format!(
            "/api/v1/departments/{}",
            projection_department["id"].as_str().unwrap()
        ),
        Some(&token),
        Some(json!({
            "name":"Renamed Projection Department",
            "parentId":department_id,
            "version":1
        })),
    )
    .await;
    assert_eq!(status, 200, "renamed department: {renamed_department}");
    let projection_user_id = Uuid::parse_str(projection_user["id"].as_str().unwrap()).unwrap();
    let projection_tenant_id =
        Uuid::parse_str(bootstrap["user"]["companyId"].as_str().unwrap()).unwrap();
    let projected_roles = crate::runtime_grants::user_role_assignments(
        &pool,
        projection_tenant_id,
        projection_user_id,
    )
    .await
    .unwrap();
    assert_eq!(projected_roles.len(), 1);
    assert_eq!(projected_roles[0].code, "workflow_operator_fixture");
    assert_eq!(projected_roles[0].name, "Renamed Workflow Operator");
    let latest_projection_admission: Value = sqlx::query_scalar(
        "SELECT payload_json FROM outbox WHERE event_type='RuntimeUserAdmissionChanged' AND aggregate_id=? ORDER BY occurred_at DESC,id DESC LIMIT 1",
    )
    .bind(projection_user["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(latest_projection_admission["tokenVersion"], 3);
    let projected_department_name: String = sqlx::query_scalar(
        "SELECT d.name FROM user_departments ud JOIN departments d ON d.tenant_id=ud.tenant_id AND d.id=ud.department_id WHERE ud.tenant_id=? AND ud.user_id=?",
    )
    .bind(projection_tenant_id)
    .bind(projection_user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(projected_department_name, "Renamed Projection Department");
    let (status, password_login) = request(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"username":"api-operator","password":"123456"})),
    )
    .await;
    assert_eq!(status, 200, "password login response: {password_login}");
    assert_eq!(password_login["passwordChangeRequired"], true);
    let (status, password_changed) = request(
        &app,
        "POST",
        "/api/v1/auth/change-password",
        None,
        Some(json!({
            "token":password_login["changePasswordToken"],
            "password":"api-operator-password"
        })),
    )
    .await;
    assert_eq!(status, 200, "password change response: {password_changed}");
    assert_eq!(password_changed["passwordChangeRequired"], false);
    let changed_admission: Value = sqlx::query_scalar(
        "SELECT payload_json FROM outbox WHERE event_type='RuntimeUserAdmissionChanged' AND aggregate_id=? ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(user["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(changed_admission["tokenVersion"], 2);
    assert_eq!(changed_admission["tenantQueryEnabled"], true);

    let (status, workflow) = request(
        &app,
        "POST",
        "/api/v1/workflows",
        Some(&token),
        Some(json!({"name":"API First Workflow","description":"closure","visibility":"company"})),
    )
    .await;
    assert_eq!(status, 201, "workflow response: {workflow}");
    let workflow_id = workflow["id"].as_str().unwrap();
    let service_identity_id = workflow["serviceIdentityId"].as_str().unwrap();
    let identity_admission: Value = sqlx::query_scalar(
        "SELECT payload_json FROM outbox WHERE event_type='ServiceIdentityAdmissionChanged' AND aggregate_id=?",
    )
    .bind(service_identity_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identity_admission["workflowId"], workflow["id"]);
    assert_eq!(
        identity_admission["identityId"],
        workflow["serviceIdentityId"]
    );
    assert_eq!(identity_admission["policyEpoch"], 1);
    assert_eq!(identity_admission["grantIds"], json!([]));
    let user_admission: Value = sqlx::query_scalar(
        "SELECT payload_json FROM outbox WHERE event_type='RuntimeUserAdmissionChanged' AND aggregate_id=? ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(bootstrap["user"]["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(user_admission["userId"], bootstrap["user"]["id"]);
    assert_eq!(user_admission["enabled"], true);
    assert_eq!(user_admission["tenantQueryEnabled"], true);
    let workflow_grant: Value = sqlx::query_scalar(
        "SELECT payload_json FROM outbox WHERE event_type='RuntimeUserWorkflowGrantChanged' AND aggregate_id=?",
    )
    .bind(workflow_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(workflow_grant["userId"], bootstrap["user"]["id"]);
    assert_eq!(workflow_grant["workflowId"], workflow["id"]);
    assert_eq!(workflow_grant["canQuery"], true);
    let (status, draft) = request(
        &app,
        "GET",
        &format!("/api/v1/workflows/{workflow_id}/draft"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "workflow draft response: {draft}");
    let definition = json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","additionalProperties":true},"contexts":{}},
        "nodes":[{
            "id":"root","key":"root","type":"set","typeVersion":1,"name":"Root",
            "disabled":false,"parameters":{},"contextWrites":[],
            "resourceReferences":[],"settings":{}
        },{
            "id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End",
            "disabled":false,"protected":true,"parameters":{"outputs":{},"errorOutputs":{}},
            "contextWrites":[],"resourceReferences":[],"settings":{}
        }],
        "connections":[
            {"id":"start-root","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"root","targetHandle":"main","order":0},
            {"id":"root-end","sourceNodeId":"root","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}},
        "settings":{"activationBudget":8,"executionOrder":"deterministic"}
    });
    let (status, saved_draft) = request(
        &app,
        "PUT",
        &format!("/api/v1/workflows/{workflow_id}/draft"),
        Some(&token),
        Some(json!({
            "expectedRevision":draft["revision"],
            "definition":definition,
            "editorDocument":draft["editorDocument"]
        })),
    )
    .await;
    assert_eq!(status, 200, "workflow draft save response: {saved_draft}");
    let (status, version) = request(
        &app,
        "POST",
        &format!("/api/v1/workflows/{workflow_id}/versions"),
        Some(&token),
        Some(json!({"draftRevision":saved_draft["revision"]})),
    )
    .await;
    assert_eq!(status, 201, "workflow version response: {version}");
    let version_run_key = Uuid::now_v7().to_string();
    let version_run_path = format!(
        "/api/v1/workflow-versions/{}/executions",
        version["id"].as_str().unwrap()
    );
    let version_run_body = json!({"input":{"source":"api-first"},"idempotencyKey":version_run_key});
    let (status, first_version_run) = request(
        &app,
        "POST",
        &version_run_path,
        Some(&token),
        Some(version_run_body.clone()),
    )
    .await;
    assert_eq!(status, 202, "version run response: {first_version_run}");
    assert!(first_version_run["executionId"].as_str().is_some());
    assert_eq!(first_version_run["replayed"], false);
    let (status, replayed_version_run) = request(
        &app,
        "POST",
        &version_run_path,
        Some(&token),
        Some(version_run_body),
    )
    .await;
    assert_eq!(status, 202, "version run replay: {replayed_version_run}");
    assert_eq!(
        replayed_version_run["executionId"],
        first_version_run["executionId"]
    );
    assert_eq!(replayed_version_run["replayed"], true);
    let publication: (String, Uuid, Value) = sqlx::query_as(
        "SELECT source_type,source_id,package_json FROM runtime_work_package_publications WHERE prepare_idempotency_key=?",
    )
    .bind(&version_run_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(publication.0, "workflow_version");
    assert_eq!(publication.1.to_string(), version["id"].as_str().unwrap());
    let package_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_work_package_publications WHERE prepare_idempotency_key=?",
    )
    .bind(&version_run_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(package_count, 1);
    assert_eq!(
        publication.2["payload"]["spec"]["debugPlan"]["mode"],
        "whole"
    );

    let (status, credential) = request(
        &app,
        "POST",
        "/api/v1/credentials",
        Some(&token),
        Some(json!({
            "name":"Provider Token","credentialType":"bearer","secret":"secret-value",
            "ownerDepartmentId":department_id
        })),
    )
    .await;
    assert_eq!(status, 201, "credential response: {credential}");
    assert_eq!(credential["storageMode"], "external_reference");
    let credential_id = credential["id"].as_str().unwrap();
    let mcp_owner_department_id = projection_department["id"].as_str().unwrap();
    let (status, dependency_options) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/departments/{mcp_owner_department_id}/resource-options?resourceType=credential&operation=use&page=1&pageSize=100"
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "MCP dependency options: {dependency_options}");
    let credential_option = dependency_options["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == credential_id)
        .unwrap();
    assert_eq!(credential_option["accessState"], "grantable");
    let (status, dependency_grant) = request(
        &app,
        "POST",
        &format!("/api/v1/departments/{mcp_owner_department_id}/resource-authorizations"),
        Some(&token),
        Some(json!({
            "resourceType":"credential","resourceId":credential_id,
            "resourceVersionId":null,"operation":"use"
        })),
    )
    .await;
    assert_eq!(status, 200, "MCP dependency grant: {dependency_grant}");
    assert_eq!(dependency_grant["grantedCount"], 1);
    let department_runtime_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM outbox WHERE event_type='ResourceGrantAdmissionChanged' AND JSON_UNQUOTE(JSON_EXTRACT(payload_json,'$.identityId'))=?",
    )
    .bind(mcp_owner_department_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        department_runtime_events, 0,
        "configuration grants must not enter Runtime admission"
    );
    let (status, cross_department_mcp) = request(
        &app,
        "POST",
        "/api/v1/mcp/servers",
        Some(&token),
        Some(json!({
            "name":"Department MCP","description":"Department configuration grant",
            "ownerDepartmentId":mcp_owner_department_id,
            "transport":{
                "kind":"streamable_http",
                "endpoint":"http://echo-mcp.test.svc.cluster.local:8090/mcp",
                "bearerCredentialId":credential_id
            },
            "configuration":{}
        })),
    )
    .await;
    assert_eq!(status, 201, "cross-department MCP: {cross_department_mcp}");

    let (status, rag_connection) = request(
        &app,
        "POST",
        "/api/v1/knowledge/connections",
        Some(&token),
        Some(json!({
            "name":"API Knowledge Connection","endpoint":"http://lightrag:9621",
            "healthPath":"/health","credentialId":null,
            "ownerDepartmentId":department_id,"configuration":{}
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "knowledge connection response: {rag_connection}"
    );
    let (status, knowledge) = request(
        &app,
        "POST",
        "/api/v1/knowledge/resources",
        Some(&token),
        Some(json!({
            "connectionId":rag_connection["id"],"name":"API Knowledge",
            "externalResourceId":"api_knowledge","ownerDepartmentId":department_id
        })),
    )
    .await;
    assert_eq!(status, 201, "knowledge resource response: {knowledge}");
    assert_eq!(knowledge["grantCount"], 0);

    let (status, mcp_server) = request(
        &app,
        "POST",
        "/api/v1/mcp/servers",
        Some(&token),
        Some(json!({
            "name":"API MCP","description":"API-first MCP","ownerDepartmentId":department_id,
            "transport":{
                "kind":"streamable_http",
                "endpoint":"http://echo-mcp.test.svc.cluster.local:8090/mcp",
                "bearerCredentialId":credential_id
            },
            "configuration":{}
        })),
    )
    .await;
    assert_eq!(status, 201, "MCP server response: {mcp_server}");
    let (status, mcp_servers) = request(
        &app,
        "GET",
        "/api/v1/mcp/servers?pageSize=100",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "MCP server list response: {mcp_servers}");
    assert_eq!(mcp_servers["items"][0]["name"], "API MCP");

    let (status, model) = request(
        &app,
        "POST",
        "/api/v1/models/aliases",
        Some(&token),
        Some(json!({
            "connectionName":"API Model Connection","providerType":"openai_compatible",
            "endpoint":"http://model.test.svc.cluster.local:8090/v1","credentialId":credential_id,
            "ownerDepartmentId":department_id,"alias":"API Model","modelName":"api-model",
            "maxInputTokens":4096,"maxOutputTokens":1024,"defaultParameters":{},
            "price":{"currency":"USD","inputPerMillion":"1","outputPerMillion":"2"}
        })),
    )
    .await;
    assert_eq!(status, 201, "model response: {model}");

    let (status, resource_workflow) = request(
        &app,
        "POST",
        "/api/v1/workflows",
        Some(&token),
        Some(json!({"name":"Credential Workflow","description":"resource closure","visibility":"company"})),
    )
    .await;
    assert_eq!(
        status, 201,
        "resource workflow response: {resource_workflow}"
    );
    let resource_workflow_id = resource_workflow["id"].as_str().unwrap();
    let (status, model_options) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/workflows/{resource_workflow_id}/resource-options?resourceType=model&operation=use&page=1&pageSize=100"
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "model options response: {model_options}");
    assert_eq!(
        model_options["items"][0]["metadata"],
        json!({"maxInputTokens":4096,"maxOutputTokens":1024,"currency":"USD"}),
        "model options must carry budget metadata for studio defaults"
    );
    assert_eq!(
        model_options["items"][0]["requirements"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        model_options["items"][0]["requirements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|requirement| requirement["name"] == "Provider Token")
    );
    let (status, grant_request) = request(
        &app,
        "POST",
        &format!("/api/v1/workflows/{resource_workflow_id}/resource-grant-requests"),
        Some(&token),
        Some(json!({
            "resourceType":"model","resourceId":model["id"],"resourceVersionId":null,
            "operation":"use","sourceNodeId":"model-node","sourceRevision":0,
            "message":"API-first resource closure"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "resource grant request response: {grant_request}"
    );
    assert_eq!(grant_request["items"].as_array().unwrap().len(), 2);
    let (status, notifications) =
        request(&app, "GET", "/api/v1/notifications", Some(&token), None).await;
    assert_eq!(status, 200, "notification response: {notifications}");
    assert_eq!(notifications["unreadCount"], 1);
    assert_eq!(
        notifications["items"][0]["notificationType"],
        "resource_grant_request_created"
    );
    let (status, grant) = request(
        &app,
        "POST",
        &format!("/api/v1/resources/credential/{credential_id}/grants"),
        Some(&token),
        Some(json!({
            "subjectType":"workflow_service_identity",
            "subjectId":resource_workflow["serviceIdentityId"],
            "resourceVersionId":null,
            "operation":"use"
        })),
    )
    .await;
    assert_eq!(status, 201, "credential grant response: {grant}");
    let resource_identity_epoch: u64 = sqlx::query_scalar(
        "SELECT version FROM workflow_service_identities WHERE tenant_id=(SELECT tenant_id FROM workflows WHERE id=?) AND id=?",
    )
    .bind(Uuid::parse_str(resource_workflow_id).unwrap())
    .bind(Uuid::parse_str(resource_workflow["serviceIdentityId"].as_str().unwrap()).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(resource_identity_epoch, 2);
    let grant_admission: Value = sqlx::query_scalar(
        "SELECT payload_json FROM outbox WHERE event_type='ResourceGrantAdmissionChanged' AND aggregate_id=? ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(grant["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        grant_admission["identityId"],
        resource_workflow["serviceIdentityId"]
    );
    assert_eq!(grant_admission["policyEpoch"], 2);
    assert_eq!(grant_admission["enabled"], true);
    let (status, resource_options) = request(
        &app,
        "GET",
        &format!(
            "/api/v1/workflows/{resource_workflow_id}/resource-options?resourceType=credential&operation=use&page=1&pageSize=100"
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "resource options response: {resource_options}");
    assert_eq!(resource_options["items"][0]["accessState"], "authorized");
    let (status, resource_draft) = request(
        &app,
        "GET",
        &format!("/api/v1/workflows/{resource_workflow_id}/draft"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "resource draft response: {resource_draft}");
    let resource_definition = json!({
        "schemaVersion":"8.0",
        "start":{"inputs":{"type":"object","additionalProperties":true},"contexts":{}},
        "nodes":[{
            "id":"credential-node","key":"credential_node","type":"set","typeVersion":1,
            "name":"Credential Node","disabled":false,"parameters":{},
            "contextWrites":[],"settings":{},"resourceReferences":[{
                "resourceType":"credential","resourceId":credential_id,"operation":"use"
            }]
        },{
            "id":"exit","key":"exit","type":"exit","typeVersion":1,"name":"End",
            "disabled":false,"protected":true,"parameters":{"outputs":{},"errorOutputs":{}},
            "contextWrites":[],"resourceReferences":[],"settings":{}
        }],
        "connections":[
            {"id":"start-node","sourceNodeId":"__start__","sourceHandle":"main","targetNodeId":"credential-node","targetHandle":"main","order":0},
            {"id":"node-end","sourceNodeId":"credential-node","sourceHandle":"main","targetNodeId":"exit","targetHandle":"main","order":0}
        ],
        "end":{"outputs":{}},"settings":{"activationBudget":8,"executionOrder":"deterministic"}
    });
    let (status, resource_saved) = request(
        &app,
        "PUT",
        &format!("/api/v1/workflows/{resource_workflow_id}/draft"),
        Some(&token),
        Some(json!({
            "expectedRevision":resource_draft["revision"],"definition":resource_definition,
            "editorDocument":resource_draft["editorDocument"]
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "resource draft save response: {resource_saved}"
    );
    let (status, resource_version) = request(
        &app,
        "POST",
        &format!("/api/v1/workflows/{resource_workflow_id}/versions"),
        Some(&token),
        Some(json!({"draftRevision":resource_saved["revision"]})),
    )
    .await;
    assert_eq!(status, 201, "resource version response: {resource_version}");

    let (status, skill) = request(
        &app,
        "POST",
        "/api/v1/skills",
        Some(&token),
        Some(json!({
            "name":"API Skill","alias":"api-skill","description":"API-first skill",
            "ownerDepartmentId":department_id
        })),
    )
    .await;
    assert_eq!(status, 201, "skill response: {skill}");
    let skill_id = skill["id"].as_str().unwrap();
    let (status, workspace) = request(
        &app,
        "GET",
        &format!("/api/v1/skills/{skill_id}/workspace"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "skill workspace response: {workspace}");
    let entry_id = workspace["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == "SKILL.md")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let (status, file) = request(
        &app,
        "PUT",
        &format!("/api/v1/skills/{skill_id}/files/{entry_id}"),
        Some(&token),
        Some(json!({
            "content":"# API Skill\n\nRuns through public APIs.",
            "description":"API-first skill workspace",
            "expectedRevision":workspace["revision"]
        })),
    )
    .await;
    assert_eq!(status, 200, "skill file response: {file}");
    let (status, refreshed_skill) = request(
        &app,
        "GET",
        &format!("/api/v1/skills/{skill_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "skill detail response: {refreshed_skill}");
    assert_eq!(refreshed_skill["description"], "API-first skill workspace");
    let (status, skill_version) = request(
        &app,
        "POST",
        &format!("/api/v1/skills/{skill_id}/versions"),
        Some(&token),
        Some(json!({"expectedRevision":file["revision"],"dependencies":[]})),
    )
    .await;
    assert_eq!(status, 201, "skill version response: {skill_version}");

    let (status, dataset) = request(
        &app,
        "POST",
        "/api/v1/datasets",
        Some(&token),
        Some(json!({"name":"API Dataset","description":"closure","visibility":"company"})),
    )
    .await;
    assert_eq!(status, 201, "dataset response: {dataset}");
    let dataset_id = dataset["id"].as_str().unwrap();
    let (status, case) = request(
        &app,
        "POST",
        &format!("/api/v1/datasets/{dataset_id}/cases"),
        Some(&token),
        Some(json!({
            "expectedRevision":0,"caseKey":"case-1","name":"Case 1","input":{"value":1},
            "expectedOutput":{"value":1},"context":null,"tags":["api"],"evaluatorOverride":null
        })),
    )
    .await;
    assert_eq!(status, 201, "dataset case response: {case}");
    let (status, dataset_version) = request(
        &app,
        "POST",
        &format!("/api/v1/datasets/{dataset_id}/versions"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 201, "dataset version response: {dataset_version}");

    let (status, profile) = request(
        &app,
        "POST",
        "/api/v1/evaluation-profiles",
        Some(&token),
        Some(json!({
            "name":"Exact Profile","description":"closure","visibility":"company",
            "aggregation":"all","passThreshold":"1","rules":[{
                "key":"exact","name":"Exact","evaluatorType":"exact","configuration":{},
                "weight":"1","required":true
            }]
        })),
    )
    .await;
    assert_eq!(status, 201, "evaluation profile response: {profile}");
    let (status, evaluation) = request(
        &app,
        "POST",
        "/api/v1/evaluations",
        Some(&token),
        Some(json!({
            "name":"API Evaluation","workflowVersionId":resource_version["id"],
            "datasetVersionId":dataset_version["id"],"evaluationProfileVersionId":profile["versionId"],
            "visibility":"company","parameters":{}
        })),
    )
    .await;
    assert_eq!(status, 201, "evaluation response: {evaluation}");
    let evaluation_id = evaluation["id"].as_str().unwrap();
    let (status, started) = request(
        &app,
        "POST",
        &format!("/api/v1/evaluations/{evaluation_id}/start"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 202, "evaluation start response: {started}");
    let evaluation_package: Value = sqlx::query_scalar(
        "SELECT package_json FROM runtime_work_package_publications WHERE source_type='evaluation_run' AND source_id=?",
    )
    .bind(Uuid::parse_str(evaluation_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        evaluation_package["payload"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|binding| {
                binding["resourceKind"] == "credential" && binding["resourceId"] == credential_id
            }),
        "Evaluation Work Package must carry the immutable Workflow Version Resource closure"
    );
    let (status, evaluations) =
        request(&app, "GET", "/api/v1/evaluations", Some(&token), None).await;
    assert_eq!(status, 200, "evaluation list response: {evaluations}");
    assert_eq!(evaluations[0]["workflowName"], "Credential Workflow");
    assert_eq!(evaluations[0]["datasetName"], "API Dataset");
    let (status, report) = request(
        &app,
        "GET",
        &format!("/api/v1/evaluations/{evaluation_id}/report"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "evaluation report response: {report}");
    assert_eq!(report["run"]["workflowName"], "Credential Workflow");
    assert_eq!(report["run"]["datasetName"], "API Dataset");
    assert!(
        report["metrics"].is_array(),
        "the public Evaluation Report contract keeps the existing metric array: {report}"
    );
    assert!(report["results"].is_array());

    let credential_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM credential_secret_versions WHERE provider='vault_kv_v2'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(credential_rows, 1);
    let business_counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM workflows),(SELECT COUNT(*) FROM skill_versions),(SELECT COUNT(*) FROM dataset_versions),(SELECT COUNT(*) FROM evaluation_runs)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(business_counts, (2, 1, 1, 1));
    let (snapshot, snapshot_hash): (Value, String) = sqlx::query_as(
        "SELECT snapshot_json,snapshot_hash FROM workflow_version_resources WHERE workflow_version_id=? AND resource_type='credential'",
    )
    .bind(Uuid::parse_str(resource_version["id"].as_str().unwrap()).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(snapshot["vaultSecretRef"]["mount"], "secret");
    assert_eq!(snapshot["vaultSecretRef"]["version"], 1);
    assert_eq!(
        agentx_runtime_contracts::ContentHash::parse(&snapshot_hash)
            .unwrap()
            .as_str(),
        snapshot_hash
    );
    let (status, deleted) = request(
        &app,
        "DELETE",
        &format!(
            "/api/v1/resources/credential/{credential_id}/grants/{}",
            grant["id"].as_str().unwrap()
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 204, "credential grant delete response: {deleted}");
    let revoked_admission: Value = sqlx::query_scalar(
        "SELECT payload_json FROM outbox WHERE event_type='ResourceGrantAdmissionChanged' AND aggregate_id=? ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(grant["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(revoked_admission["enabled"], false);
    assert_eq!(revoked_admission["policyEpoch"], 3);
    let (status, restored_grant) = request(
        &app,
        "POST",
        &format!("/api/v1/resources/credential/{credential_id}/grants"),
        Some(&token),
        Some(json!({
            "subjectType":"workflow_service_identity",
            "subjectId":resource_workflow["serviceIdentityId"],
            "resourceVersionId":null,
            "operation":"use"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "credential grant restore response: {restored_grant}"
    );
    assert_eq!(
        restored_grant["id"], grant["id"],
        "regrant must restore the immutable Grant referenced by a published Bundle"
    );
    let restored_admission: Value = sqlx::query_scalar(
        "SELECT payload_json FROM outbox WHERE event_type='ResourceGrantAdmissionChanged' AND aggregate_id=? ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(grant["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(restored_admission["enabled"], true);
    assert_eq!(restored_admission["policyEpoch"], 4);
    assert!(!service_identity_id.is_empty());
    runtime_server.abort();
    vault_server.abort();
}

async fn request(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = builder
        .body(Body::from(
            body.map_or_else(String::new, |value| value.to_string()),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, body)
}

async fn mock_vault_write() -> Json<Value> {
    Json(json!({"data":{"version":1}}))
}

async fn mock_apply_admission(Json(request): Json<Value>) -> Json<Value> {
    Json(json!({
        "apiVersion":1,
        "eventId":request["command"]["eventId"],
        "applied":true,
        "replayed":false,
        "objectVersion":request["admissionEpoch"],
        "result":{"admissionEpoch":request["admissionEpoch"]}
    }))
}

async fn mock_prepare_work_package(Json(request): Json<Value>) -> Json<Value> {
    let package_id = request["workPackage"]["payload"]["packageId"]
        .as_str()
        .unwrap();
    Json(json!({
        "apiVersion":1,
        "receipt":{
            "apiVersion":1,"eventId":Uuid::now_v7(),"applied":true,"replayed":false,
            "objectVersion":1,"result":{}
        },
        "bundleId":package_id,"headVersion":null,"activationSequence":null,
        "status":"accepted","rejection":null,"acceptedAt":"2026-08-17T00:00:00Z"
    }))
}

async fn mock_execute_work_package(Path(package_action): Path<String>) -> Json<Value> {
    assert!(package_action.ends_with(":execute"));
    Json(json!({
        "apiVersion":1,"eventId":Uuid::now_v7(),"applied":true,"replayed":false,
        "objectVersion":2,"result":{"executionId":Uuid::now_v7()}
    }))
}

async fn mock_plugin_design_operation(Json(request): Json<Value>) -> Json<Value> {
    Json(json!({
        "protocolVersion":1,
        "operationId":request["operationId"],
        "result":{"status":"complete"}
    }))
}

async fn mock_upload_object(
    mut multipart: Multipart,
) -> Json<agentx_runtime_contracts::RuntimeObjectUploadReceiptV1> {
    let mut metadata = None;
    let mut content = None;
    while let Some(field) = multipart.next_field().await.unwrap() {
        let name = field.name().unwrap_or_default().to_owned();
        let bytes = field.bytes().await.unwrap();
        match name.as_str() {
            "metadata" => {
                metadata =
                    Some(
                        serde_json::from_slice::<
                            agentx_runtime_contracts::RuntimeObjectUploadMetadataV1,
                        >(&bytes)
                        .unwrap(),
                    );
            }
            "content" => content = Some(bytes),
            _ => panic!("unexpected Runtime object multipart field {name}"),
        }
    }
    let metadata = metadata.expect("Runtime object metadata is required");
    let content = content.expect("Runtime object content is required");
    assert_eq!(content.len() as u64, metadata.size_bytes);
    let object_key = agentx_runtime_contracts::RuntimeObjectReferenceV1::canonical_key(
        metadata.tenant_id,
        metadata.object_id,
        &metadata.content_hash,
    );
    Json(agentx_runtime_contracts::RuntimeObjectUploadReceiptV1 {
        api_version: 1,
        object: agentx_runtime_contracts::RuntimeObjectReferenceV1 {
            tenant_id: metadata.tenant_id,
            storage_domain: agentx_runtime_contracts::StorageDomain::Runtime,
            object_id: metadata.object_id,
            object_key,
            content_hash: metadata.content_hash,
            size_bytes: metadata.size_bytes,
            media_type: metadata.media_type,
        },
        replayed: false,
        accepted_at: time::OffsetDateTime::now_utc(),
    })
}

async fn connect_with_retry(port: u16) -> sqlx::MySqlPool {
    let url = format!("mysql://agentx:agentx-test-password@127.0.0.1:{port}/agentx_control");
    let mut last_error = None;
    for _ in 0..40 {
        match MySqlPoolOptions::new()
            .max_connections(10)
            .connect(&url)
            .await
        {
            Ok(pool) => return pool,
            Err(error) => last_error = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("connect to Control MySQL: {last_error:?}");
}
