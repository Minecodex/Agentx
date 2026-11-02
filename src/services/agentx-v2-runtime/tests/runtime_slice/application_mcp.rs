#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn application_mcp_contract_is_real_http_and_uses_own_invocation_authority(){
    let mysql=GenericImage::new("mysql","8.4").with_exposed_port(3306.tcp()).with_wait_for(WaitFor::message_on_stdout("MySQL init process done. Ready for start up.")).with_wait_for(WaitFor::message_on_stderr("ready for connections")).with_env_var("MYSQL_DATABASE","agentx_runtime").with_env_var("MYSQL_USER","agentx").with_env_var("MYSQL_PASSWORD","agentx-test-password").with_env_var("MYSQL_ROOT_PASSWORD","agentx-root-password").with_mapped_port(0,3306.tcp()).start().await.unwrap();
    let port=mysql.get_host_port_ipv4(3306.tcp()).await.unwrap();
    let pool=connect_with_retry(port).await;
    agentx_runtime_infrastructure::migrate_runtime_mysql(&pool).await.unwrap();
    let observer=sqlx::mysql::MySqlPoolOptions::new().max_connections(1).connect(&format!("mysql://root:agentx-root-password@127.0.0.1:{port}/agentx_runtime")).await.unwrap();
    let redis=GenericImage::new("redis","7.4-alpine").with_exposed_port(6379.tcp()).with_wait_for(WaitFor::message_on_stdout("Ready to accept connections")).with_mapped_port(0,6379.tcp()).start().await.unwrap();
    let port=redis.get_host_port_ipv4(6379.tcp()).await.unwrap();
    let redis_client=redis::Client::open(format!("redis://127.0.0.1:{port}/")).unwrap().get_connection_manager().await.unwrap();
    let base=Fixture::new(pool,observer,redis_client);
    design_model_receipts_follow_the_frozen_deadline(&base).await;
    Box::pin(application_mcp_is_scoped_and_uses_formal_invocations(&base)).await;
}

async fn application_mcp_is_scoped_and_uses_formal_invocations(base:&Fixture){
    use agentx_runtime_contracts::{ApplicationMcpPublicationV1,APPLICATION_MCP_PROTOCOL,content_hash};
    let mut fixture=Fixture::new(base.state.pool.clone(),base.observer.clone(),base.state.admission_redis.clone().unwrap());
    fixture.api_key=format!("axk_{}",Uuid::new_v4().simple());
    let bundle=fixture.bundle(1).await;
    fixture.upload_bundle_object(&bundle,"mcp-fixture:bundle").await;
    prepare(&fixture,&bundle,"mcp-fixture:prepare").await;
    apply_initial_admission(&fixture,1).await;
    activate(&fixture,&bundle,None,1,1).await;
    let mut publication=ApplicationMcpPublicationV1{tenant_id:fixture.tenant_id,application_id:fixture.application_id,publication_id:Uuid::now_v7(),deployment_id:bundle.payload.deployment_id,bundle_id:bundle.payload.bundle_id,revision:1,expected_revision:0,slug:format!("mcp-{}",fixture.application_id.simple()),tool_name:"run_business".into(),title:"Whole application".into(),description:"Submit the complete business application".into(),input_schema:bundle.payload.input_contract.clone(),output_schema:bundle.payload.output_contract.clone(),wait_input_schema:json!({"type":"string"}),chat_mapping:None,contract_hash:content_hash(&json!({"input":bundle.payload.input_contract,"output":bundle.payload.output_contract,"waitInputSchema":{"type":"string"},"chatMapping":null})).unwrap(),enabled:true,protocol_version:APPLICATION_MCP_PROTOCOL.into()};
    let request=admission_request(&fixture,1,AdmissionTargetV1::ApplicationMcp{state:Box::new(publication.clone())},"mcp-fixture:publication");
    let receipt=apply_admission(State(fixture.state.clone()),publisher_headers("runtime.admission.apply"),Json(request)).await.unwrap().0;
    assert!(receipt.applied,"MCP publication must apply through the existing Publisher boundary");
    let old_origin=std::env::var("AGENTX_RUNTIME_PUBLIC_ORIGIN").ok();
    // This is the only test in this integration process; no task reads this setting concurrently.
    unsafe{std::env::set_var("AGENTX_RUNTIME_PUBLIC_ORIGIN","http://127.0.0.1");}
    let router=agentx_v2_runtime::gateway::router().with_state(fixture.state.clone());
    let initialize=mcp_request(&router,&fixture,&publication.slug,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":APPLICATION_MCP_PROTOCOL,"capabilities":{},"clientInfo":{"name":"independent-protocol-fixture","version":"1"}}})).await;
    assert_eq!(initialize["result"]["protocolVersion"],APPLICATION_MCP_PROTOCOL,"{initialize}");
    let older=mcp_request(&router,&fixture,&publication.slug,json!({"jsonrpc":"2.0","id":11,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"older-client","version":"1"}}})).await;
    assert_eq!(older["result"]["protocolVersion"],APPLICATION_MCP_PROTOCOL,"The application server must negotiate only its explicitly implemented protocol");
    let tools=mcp_request(&router,&fixture,&publication.slug,json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})).await;
    let names=tools["result"]["tools"].as_array().unwrap().iter().map(|tool|tool["name"].as_str().unwrap()).collect::<BTreeSet<_>>();
    assert_eq!(names,BTreeSet::from(["run_business","get_task","submit_task_input","cancel_task"]),"Internal workflow tools must never be published");
    let request_id=Uuid::now_v7();
    let submit=json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"run_business","arguments":{"requestId":request_id,"publicationRevision":1,"input":{"message":"MCP formal business input"}}}});
    let created=mcp_request(&router,&fixture,&publication.slug,submit.clone()).await;
    let task=&created["result"]["structuredContent"];
    let id=task["taskId"].as_str().unwrap_or_else(||panic!("MCP did not create a formal task: {created}"));
    assert_eq!(task["state"],"accepted");assert_eq!(task["accessBinding"]["kind"],"mcp");assert!(task["taskUrl"].is_null());
    let replay=mcp_request(&router,&fixture,&publication.slug,submit.clone()).await;
    assert_eq!(replay["result"]["structuredContent"]["taskId"],id);
    let invocations:i64=sqlx::query_scalar("SELECT COUNT(*) FROM application_invocations WHERE tenant_id=? AND idempotency_key=?").bind(fixture.tenant_id).bind(request_id.to_string()).fetch_one(&fixture.state.pool).await.unwrap();assert_eq!(invocations,1);
    let first_publication=publication.clone();
    publication.publication_id=Uuid::now_v7();publication.expected_revision=1;publication.revision=2;publication.enabled=false;
    publication.wait_input_schema=json!({"type":"object","required":["confirmed"],"properties":{"confirmed":{"type":"boolean"}},"additionalProperties":false});
    publication.contract_hash=content_hash(&json!({"input":publication.input_schema,"output":publication.output_schema,"waitInputSchema":publication.wait_input_schema,"chatMapping":publication.chat_mapping})).unwrap();
    let request=admission_request(&fixture,2,AdmissionTargetV1::ApplicationMcp{state:Box::new(publication.clone())},"mcp-fixture:disable");
    assert!(apply_admission(State(fixture.state.clone()),publisher_headers("runtime.admission.apply"),Json(request)).await.unwrap().0.applied);
    let changed_tools=mcp_request(&router,&fixture,&publication.slug,json!({"jsonrpc":"2.0","id":21,"method":"tools/list","params":{}})).await;
    let helper=changed_tools["result"]["tools"].as_array().unwrap().iter().find(|tool|tool["name"]=="submit_task_input").unwrap();
    let content=&helper["inputSchema"]["properties"]["content"];
    assert!(jsonschema::is_valid(content,&json!("original task response")),"The real route must retain earlier published wait types");
    assert!(jsonschema::is_valid(content,&json!({"confirmed":true})));
    assert!(!jsonschema::is_valid(content,&json!(12)));
    let replay=mcp_request(&router,&fixture,&publication.slug,submit).await;
    assert_eq!(replay["result"]["structuredContent"]["taskId"],id,"Publication changes must not duplicate the original request");
    let rejected=mcp_request(&router,&fixture,&publication.slug,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"run_business","arguments":{"requestId":Uuid::now_v7(),"publicationRevision":2,"input":{"message":"disabled new input"}}}})).await;
    assert_eq!(rejected["result"]["isError"],true,"{rejected}");
    let queried=mcp_request(&router,&fixture,&publication.slug,json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_task","arguments":{"taskId":id}}})).await;
    assert_eq!(queried["result"]["structuredContent"]["taskId"],id,"Disabling an entrance must preserve owned tasks");
    let arbitrary=mcp_request(&router,&fixture,&publication.slug,json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"get_task","arguments":{"taskId":Uuid::now_v7()}}})).await;
    assert_eq!(arbitrary["result"]["isError"],true);
    assert!(arbitrary["result"].get("structuredContent").is_none(),"Tool failures must not violate the successful TaskView output schema");
    let error:Value=serde_json::from_str(arbitrary["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["httpStatus"],404);
    let mut retained=publication.clone();retained.publication_id=Uuid::now_v7();retained.revision=3;retained.expected_revision=2;retained.enabled=true;retained.tool_name="run_revised".into();
    mcp_publish_revision(&fixture,&retained,"mcp-fixture:third").await;
    let shared=mcp_request(&router,&fixture,&retained.slug,json!({"jsonrpc":"2.0","id":31,"method":"tools/list"})).await;
    let names=shared["result"]["tools"].as_array().unwrap().iter().map(|v|v["name"].as_str().unwrap()).collect::<BTreeSet<_>>();
    assert_eq!(names,BTreeSet::from(["run_business","run_revised","get_task","submit_task_input","cancel_task"]));
    let mut latest=retained.clone();latest.publication_id=Uuid::now_v7();latest.revision=4;latest.expected_revision=3;latest.slug=format!("{}-new",latest.slug);latest.tool_name="run_latest".into();
    mcp_publish_revision(&fixture,&latest,"mcp-fixture:fourth").await;
    let mut created_ids=vec![id.to_string()];
    for published in [&first_publication,&retained,&latest] {
        let result=mcp_request(&router,&fixture,&published.slug,json!({"jsonrpc":"2.0","id":32,"method":"tools/call","params":{"name":published.tool_name,"arguments":{"requestId":Uuid::now_v7(),"publicationRevision":published.revision,"input":{"message":"retained publication"}}}})).await;
        assert_eq!(result["result"]["structuredContent"]["accessBinding"]["publicationId"],published.publication_id.to_string(),"{result}");
        created_ids.push(result["result"]["structuredContent"]["taskId"].as_str().unwrap().to_owned());
    }
    let wrong_address=mcp_request(&router,&fixture,&latest.slug,json!({"jsonrpc":"2.0","id":33,"method":"tools/call","params":{"name":first_publication.tool_name,"arguments":{"requestId":Uuid::now_v7(),"publicationRevision":1,"input":{"message":"cross address"}}}})).await;
    assert_eq!(wrong_address["result"]["isError"],true,"A revision cannot bypass the selected MCP address");
    mcp_remove_revision(&fixture,&retained,"mcp-fixture:remove-third").await;
    mcp_remove_revision(&fixture,&retained,"mcp-fixture:remove-third-again").await;
    mcp_publish_revision(&fixture,&retained,"mcp-fixture:old-delivery-again").await;
    let tools=mcp_request(&router,&fixture,&first_publication.slug,json!({"jsonrpc":"2.0","id":34,"method":"tools/list"})).await;
    assert!(!tools["result"]["tools"].as_array().unwrap().iter().any(|tool|tool["name"]=="run_revised"),"A delayed delivery must not resurrect a removed publication");
    let deleted=mcp_request(&router,&fixture,&first_publication.slug,json!({"jsonrpc":"2.0","id":35,"method":"tools/call","params":{"name":retained.tool_name,"arguments":{"requestId":Uuid::now_v7(),"publicationRevision":3,"input":{"message":"removed revision"}}}})).await;
    assert_eq!(deleted["result"]["isError"],true);
    mcp_remove_revision(&fixture,&first_publication,"mcp-fixture:remove-first").await;
    mcp_remove_revision(&fixture,&publication,"mcp-fixture:remove-disabled").await;
    mcp_address_missing(&router,&fixture,&first_publication.slug).await;
    let old_task=mcp_request(&router,&fixture,&latest.slug,json!({"jsonrpc":"2.0","id":36,"method":"tools/call","params":{"name":"get_task","arguments":{"taskId":id}}})).await;
    assert_eq!(old_task["result"]["structuredContent"]["taskId"],id,"Deleting a publication must preserve its existing task");
    let old_reference:i64=sqlx::query_scalar("SELECT COUNT(*) FROM bundle_references WHERE tenant_id=? AND reference_kind='application_mcp' AND owner_id=?").bind(fixture.tenant_id).bind(first_publication.publication_id).fetch_one(&fixture.state.pool).await.unwrap();
    assert_eq!(old_reference,0);
    for task_id in created_ids.iter().skip(1) {
        let cancelled=mcp_request(&router,&fixture,&latest.slug,json!({"jsonrpc":"2.0","id":37,"method":"tools/call","params":{"name":"cancel_task","arguments":{"taskId":task_id,"requestId":Uuid::now_v7()}}})).await;
        assert_eq!(cancelled["result"]["structuredContent"]["taskId"],*task_id);
    }
    let cancelled=mcp_request(&router,&fixture,&latest.slug,json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"cancel_task","arguments":{"taskId":id,"requestId":Uuid::now_v7()}}})).await;
    assert_eq!(cancelled["result"]["structuredContent"]["taskId"],id);
    let execution:Uuid=sqlx::query_scalar("SELECT execution_id FROM application_invocations WHERE id=?").bind(Uuid::parse_str(id).unwrap()).fetch_one(&fixture.state.pool).await.unwrap();
    let commands=claim_commands(&fixture.state.pool,Uuid::now_v7(),100).await.unwrap();
    assert!(commands.iter().any(|command|command.execution_id==execution));
    for command in commands {process_command(&fixture.state.pool,&command).await.unwrap();}
    let final_task=mcp_request(&router,&fixture,&latest.slug,json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"get_task","arguments":{"taskId":id}}})).await;
    assert_eq!(final_task["result"]["structuredContent"]["state"],"cancelled");
    mcp_remove_revision(&fixture,&latest,"mcp-fixture:remove-last").await;
    mcp_address_missing(&router,&fixture,&latest.slug).await;
    let mut reopened=first_publication.clone();reopened.publication_id=Uuid::now_v7();reopened.revision=5;reopened.expected_revision=4;
    mcp_publish_revision(&fixture,&reopened,"mcp-fixture:reopen").await;
    let tools=mcp_request(&router,&fixture,&reopened.slug,json!({"jsonrpc":"2.0","id":38,"method":"tools/list"})).await;
    assert_eq!(tools["result"]["tools"][0]["inputSchema"]["properties"]["publicationRevision"]["default"],5,"Deleting every publication must not reset revision numbering");
    unsafe{if let Some(value)=old_origin{std::env::set_var("AGENTX_RUNTIME_PUBLIC_ORIGIN",value);}else{std::env::remove_var("AGENTX_RUNTIME_PUBLIC_ORIGIN");}}
}
async fn mcp_request(router:&Router,fixture:&Fixture,slug:&str,body:Value)->Value{
    let mut request=Request::post(format!("/mcp/applications/{slug}")).header("Host","127.0.0.1").header("Authorization",format!("Bearer {}",fixture.api_key)).header("Accept","application/json, text/event-stream").header("Content-Type","application/json");
    if body["method"]!="initialize" {request=request.header("MCP-Protocol-Version","2025-11-25");}
    let response=router.clone().oneshot(request.body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap()).await.unwrap();
    let status=response.status();
    let bytes=to_bytes(response.into_body(),1024*1024).await.unwrap();
    let value=serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_|panic!("MCP response was not JSON: {status} {}",String::from_utf8_lossy(&bytes)));
    assert_eq!(status,200,"{value}");value
}

async fn mcp_publish_revision(fixture:&Fixture, publication:&agentx_runtime_contracts::ApplicationMcpPublicationV1, key:&str) {
    let request=admission_request(fixture,publication.revision,AdmissionTargetV1::ApplicationMcp{state:Box::new(publication.clone())},key);
    assert!(apply_admission(State(fixture.state.clone()),publisher_headers("runtime.admission.apply"),Json(request)).await.unwrap().0.applied);
}
async fn mcp_remove_revision(fixture:&Fixture, publication:&agentx_runtime_contracts::ApplicationMcpPublicationV1, key:&str) {
    let removal=agentx_runtime_contracts::ApplicationMcpRemovalV1 {tenant_id:fixture.tenant_id,application_id:fixture.application_id,publication_id:publication.publication_id};
    let request=admission_request(fixture,publication.revision,AdmissionTargetV1::ApplicationMcpRemoved{state:removal},key);
    assert!(apply_admission(State(fixture.state.clone()),publisher_headers("runtime.admission.apply"),Json(request)).await.unwrap().0.applied);
}
async fn mcp_address_missing(router:&Router,fixture:&Fixture,slug:&str) {
    for (path,method) in [(format!("/mcp/applications/{slug}"),"POST"),(format!("/.well-known/oauth-protected-resource/mcp/applications/{slug}"),"GET")] {
        let response=router.clone().oneshot(Request::builder().method(method).uri(path).header("Authorization",format!("Bearer {}",fixture.api_key)).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(),404,"Removing the last publication must remove the address and its OAuth resource discovery");
    }
}
