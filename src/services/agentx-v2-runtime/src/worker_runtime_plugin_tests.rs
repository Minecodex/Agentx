use super::*;
use agentx_node_protocol::{
    NodeCapability, PluginNodeBinding, PluginRuntimeArtifact, plugin_runtime_object_id,
};
use agentx_runtime_contracts::{ContentHash, WorkerAttemptLeaseV1, WorkerTaskV1};
use object_store::memory::InMemory;
use sqlx::mysql::MySqlPoolOptions;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use time::OffsetDateTime;
use uuid::Uuid;

fn plugin_binding(package_id: &str, digest: String, source: String) -> PluginNodeBinding {
    let content_hash = format!("sha256:{:x}", Sha256::digest(source.as_bytes()));
    PluginNodeBinding {
        package_id: package_id.into(),
        package_version: "1.0.0".into(),
        bundle_digest: digest.clone(),
        runtime_entry: "runtime/entry.js".into(),
        runtime_artifact: Some(PluginRuntimeArtifact {
            object_id: plugin_runtime_object_id(&digest),
            content_hash,
            size_bytes: source.len() as u64,
            media_type: "text/javascript".into(),
        }),
        runtime_source: source,
        ui_entry: None,
        ui_source: None,
        ui_styles: None,
        ui_assets: BTreeMap::new(),
        trace_renderers: vec![],
    }
}

async fn install_runtime_artifact(
    worker: &RuntimeWorker,
    tenant_id: Uuid,
    binding: &PluginNodeBinding,
) {
    let artifact = binding.runtime_artifact.as_ref().unwrap();
    let content_hash = ContentHash::parse(artifact.content_hash.clone()).unwrap();
    let key = RuntimeObjectReferenceV1::canonical_key(tenant_id, artifact.object_id, &content_hash);
    worker
        .objects
        .put(
            &object_store::path::Path::from(key),
            bytes::Bytes::copy_from_slice(binding.runtime_source.as_bytes()).into(),
        )
        .await
        .unwrap();
}

pub(super) struct NoProvider;
#[async_trait::async_trait]
impl super::super::WorkerProvider for NoProvider {
    async fn post_json(
        &self,
        _: &str,
        _: crate::egress::EgressRequestContext,
        _: std::time::Duration,
        _: reqwest::header::HeaderMap,
        _: &Value,
    ) -> Result<super::super::WorkerProviderResponse, super::super::WorkerProviderError> {
        Err(super::super::WorkerProviderError::Denied("unused".into()))
    }

    async fn request_json(
        &self,
        method: &str,
        endpoint: &str,
        _: crate::egress::EgressRequestContext,
        _: std::time::Duration,
        _: reqwest::header::HeaderMap,
        _: Option<&Value>,
    ) -> Result<super::super::WorkerProviderResponse, super::super::WorkerProviderError> {
        Ok(super::super::WorkerProviderResponse {
            status: reqwest::StatusCode::OK,
            headers: reqwest::header::HeaderMap::new(),
            body: bytes::Bytes::from(
                json!({"method":method,"endpoint":endpoint,"name":"remote-option"}).to_string(),
            ),
        })
    }
}

#[tokio::test]
async fn rust_worker_executes_a_node_module() {
    let pool = MySqlPoolOptions::new()
        .connect_lazy("mysql://user:password@127.0.0.1/unused")
        .unwrap();
    let worker =
        RuntimeWorker::new_with_provider(pool, Arc::new(InMemory::new()), Arc::new(NoProvider));
    let attempt_id = Uuid::now_v7();
    let mut claim = ClaimedWorkerAttempt {
            lease: WorkerAttemptLeaseV1 { protocol_version: 1, attempt_id, worker_id: Uuid::now_v7(), fencing_token: 1, locked_until: OffsetDateTime::now_utc() + time::Duration::seconds(30) },
            task: WorkerTaskV1 { protocol_version: 1, task_id: Uuid::now_v7(), tenant_id: Uuid::now_v7(), execution_id: Uuid::now_v7(), node_execution_id: Uuid::now_v7(), attempt_id, capability: NodeCapability::PluginNodejs, bundle_id: Uuid::now_v7(), work_package_id: None, state_version: 1, compatibility_hash: ContentHash::parse(format!("sha256:{}", "a".repeat(64))).unwrap(), deadline_at: OffsetDateTime::now_utc() + time::Duration::seconds(10) },
            node_type: "acme.test".into(), node_version: 1, run_index: 0, iteration_index: 0, timeout_ms: 5_000,
            invocation_id: None,
            node_parameters: json!({"answer":42}), per_item_parameters: vec![], string_conversions: json!([]), inputs: BTreeMap::new(), resources: vec![], context: json!({}),
            plugin: Some(plugin_binding("acme/test", format!("sha256:{}", "b".repeat(64)), "export async function execute(ctx){return {status:'completed',outputs:{main:[{json:{answer:ctx.parameters.answer,pid:process.pid}}]}}}".into())),
            trace_parent_span_entity_id: None,
        };
    install_runtime_artifact(
        &worker,
        claim.task.tenant_id,
        claim.plugin.as_ref().unwrap(),
    )
    .await;
    let result = execute(&worker, &claim).await;
    assert_eq!(
        result.status,
        WorkerResultStatusV1::Succeeded,
        "{:?}: {:?}",
        result.error_code,
        result.error_message
    );
    assert_eq!(result.outputs["main"][0].json["answer"], json!(42));
    let first_pid = result.outputs["main"][0].json["pid"].clone();
    assert_eq!(worker.plugin_slots.available_permits(), 8);
    let artifact = claim
        .plugin
        .as_ref()
        .unwrap()
        .runtime_artifact
        .as_ref()
        .unwrap();
    let cache_path = worker.plugin_artifacts.path(&artifact.content_hash);
    tokio::fs::write(&cache_path, b"corrupted cache")
        .await
        .unwrap();
    let next_attempt = Uuid::now_v7();
    claim.task.attempt_id = next_attempt;
    claim.task.task_id = Uuid::now_v7();
    claim.task.deadline_at = OffsetDateTime::now_utc() + time::Duration::seconds(10);
    claim.lease.attempt_id = next_attempt;
    let second = execute(&worker, &claim).await;
    assert_eq!(second.status, WorkerResultStatusV1::Succeeded);
    assert_eq!(
        tokio::fs::read_to_string(&cache_path).await.unwrap(),
        claim.plugin.as_ref().unwrap().runtime_source,
        "a corrupt local cache entry must be replaced from immutable Runtime storage"
    );
    assert_ne!(
        second.outputs["main"][0].json["pid"], first_pid,
        "each invocation owns a fresh process"
    );
    assert_eq!(worker.plugin_slots.available_permits(), 8);
}

#[tokio::test]
async fn timeout_reclaims_the_plugin_process_tree() {
    let pool = MySqlPoolOptions::new()
        .connect_lazy("mysql://user:password@127.0.0.1/unused")
        .unwrap();
    let worker =
        RuntimeWorker::new_with_provider(pool, Arc::new(InMemory::new()), Arc::new(NoProvider));
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("descendant.pid");
    let pid_path = serde_json::to_string(&pid_file.to_string_lossy()).unwrap();
    let source = "import {spawn} from 'node:child_process';import {writeFileSync} from 'node:fs';export async function execute(){const child=spawn(process.execPath,['-e','setInterval(()=>{},10000)'],{stdio:'ignore'});writeFileSync(__PID_PATH__,String(child.pid));await new Promise(()=>{})}"
            .replace("__PID_PATH__", &pid_path);
    let attempt_id = Uuid::now_v7();
    let claim = ClaimedWorkerAttempt {
        lease: WorkerAttemptLeaseV1 {
            protocol_version: 1,
            attempt_id,
            worker_id: Uuid::now_v7(),
            fencing_token: 1,
            locked_until: OffsetDateTime::now_utc() + time::Duration::seconds(30),
        },
        task: WorkerTaskV1 {
            protocol_version: 1,
            task_id: Uuid::now_v7(),
            tenant_id: Uuid::now_v7(),
            execution_id: Uuid::now_v7(),
            node_execution_id: Uuid::now_v7(),
            attempt_id,
            capability: NodeCapability::PluginNodejs,
            bundle_id: Uuid::now_v7(),
            work_package_id: None,
            state_version: 1,
            compatibility_hash: ContentHash::parse(format!("sha256:{}", "a".repeat(64))).unwrap(),
            deadline_at: OffsetDateTime::now_utc() + time::Duration::seconds(5),
        },
        node_type: "acme.timeout".into(),
        invocation_id: None,
        node_version: 1,
        run_index: 0,
        iteration_index: 0,
        timeout_ms: 500,
        node_parameters: json!({}),
        per_item_parameters: vec![],
        string_conversions: json!([]),
        inputs: BTreeMap::new(),
        resources: vec![],
        context: json!({}),
        plugin: Some(plugin_binding(
            "acme/timeout",
            format!("sha256:{}", "c".repeat(64)),
            source,
        )),
        trace_parent_span_entity_id: None,
    };
    install_runtime_artifact(
        &worker,
        claim.task.tenant_id,
        claim.plugin.as_ref().unwrap(),
    )
    .await;
    let result = execute(&worker, &claim).await;
    assert_eq!(result.error_code.as_deref(), Some("PLUGIN_TIMED_OUT"));
    let pid: u32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !process_is_alive(pid),
        "plugin descendant {pid} is still alive"
    );
}

#[tokio::test]
async fn runtime_executes_design_operations_without_control_spawning_node() {
    let pool = MySqlPoolOptions::new()
        .connect_lazy("mysql://user:password@127.0.0.1/unused")
        .unwrap();
    let worker =
        RuntimeWorker::new_with_provider(pool, Arc::new(InMemory::new()), Arc::new(NoProvider));
    let binding = plugin_binding(
            "acme/dynamic",
            format!("sha256:{}", "d".repeat(64)),
            "export function resolveDefinition(configuration){return {status:configuration.ready?'complete':'incomplete',outputSchema:{type:'object',properties:{value:{type:'string'}}}}}".into(),
        );
    let result = invoke_design_operation(
        &worker,
        Uuid::now_v7(),
        Uuid::now_v7(),
        &[],
        &binding,
        "node.resolveDefinition",
        json!({"configuration":{"ready":true}}),
    )
    .await
    .unwrap();
    assert_eq!(result["status"], "complete");
    assert_eq!(
        result["outputSchema"]["properties"]["value"]["type"],
        "string"
    );
}

#[tokio::test]
async fn design_provider_uses_host_http_and_reports_missing_model_resources() {
    let pool = MySqlPoolOptions::new()
        .connect_lazy("mysql://user:password@127.0.0.1/unused")
        .unwrap();
    let worker =
        RuntimeWorker::new_with_provider(pool, Arc::new(InMemory::new()), Arc::new(NoProvider));
    let tenant_id = Uuid::now_v7();
    let operation_id = Uuid::now_v7();
    let credential_id = Uuid::now_v7();
    let credential = agentx_runtime_contracts::RuntimeResourceBindingV1 {
        resource_kind: RuntimeResourceKindV1::Credential,
        resource_id: credential_id,
        resource_version: "7".into(),
        state_epoch: 1,
        content_hash: ContentHash::parse(format!("sha256:{}", "3".repeat(64))).unwrap(),
        configuration: RuntimeResourceConfigurationV1::Credential {
            credential_type: "api_key".into(),
            secret: agentx_runtime_contracts::VaultSecretReferenceV1 {
                mount: "secret".into(),
                path: "credentials/provider".into(),
                key: "value".into(),
                version: 7,
            },
            allowed_operations: BTreeSet::from(["use".into()]),
        },
        object_ids: vec![],
    };
    let binding = plugin_binding(
            "acme/provider",
            format!("sha256:{}", "7".repeat(64)),
            "export const providers={options:async(input,ctx)=>{const credentials=await ctx.credentials.list();const response=await ctx.http({url:'https://provider.example/options'});return [{value:String(credentials.length),label:response.body.name}]},missingModel:async(input,ctx)=>{await ctx.model({prompt:'discover'});return []}}".into(),
        );
    let result = invoke_design_operation(
        &worker,
        tenant_id,
        operation_id,
        std::slice::from_ref(&credential),
        &binding,
        "node.invokeProvider",
        json!({"provider":"options","input":{}}),
    )
    .await
    .unwrap();
    assert_eq!(result, json!([{"value":"1","label":"remote-option"}]));

    let error = invoke_design_operation(
        &worker,
        tenant_id,
        Uuid::now_v7(),
        &[credential],
        &binding,
        "node.invokeProvider",
        json!({"provider":"missingModel","input":{}}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("Model resource is not bound"), "{error}");
}

#[tokio::test]
async fn runner_crash_is_contained_and_the_pool_recovers() {
    let pool = MySqlPoolOptions::new()
        .connect_lazy("mysql://user:password@127.0.0.1/unused")
        .unwrap();
    let worker =
        RuntimeWorker::new_with_provider(pool, Arc::new(InMemory::new()), Arc::new(NoProvider));
    let attempt_id = Uuid::now_v7();
    let mut claim = ClaimedWorkerAttempt {
        lease: WorkerAttemptLeaseV1 {
            protocol_version: 1,
            attempt_id,
            worker_id: Uuid::now_v7(),
            fencing_token: 1,
            locked_until: OffsetDateTime::now_utc() + time::Duration::seconds(30),
        },
        task: WorkerTaskV1 {
            protocol_version: 1,
            task_id: Uuid::now_v7(),
            tenant_id: Uuid::now_v7(),
            execution_id: Uuid::now_v7(),
            node_execution_id: Uuid::now_v7(),
            attempt_id,
            capability: NodeCapability::PluginNodejs,
            bundle_id: Uuid::now_v7(),
            work_package_id: None,
            state_version: 1,
            compatibility_hash: ContentHash::parse(format!("sha256:{}", "a".repeat(64))).unwrap(),
            deadline_at: OffsetDateTime::now_utc() + time::Duration::seconds(10),
        },
        node_type: "acme.crash".into(),
        invocation_id: None,
        node_version: 1,
        run_index: 0,
        iteration_index: 0,
        timeout_ms: 5_000,
        node_parameters: json!({}),
        per_item_parameters: vec![],
        string_conversions: json!([]),
        inputs: BTreeMap::new(),
        resources: vec![],
        context: json!({}),
        plugin: Some(plugin_binding(
            "acme/crash",
            format!("sha256:{}", "e".repeat(64)),
            "export async function execute(){process.exit(17)}".into(),
        )),
        trace_parent_span_entity_id: None,
    };
    install_runtime_artifact(
        &worker,
        claim.task.tenant_id,
        claim.plugin.as_ref().unwrap(),
    )
    .await;
    let crashed = execute(&worker, &claim).await;
    assert_eq!(crashed.error_code.as_deref(), Some("PLUGIN_PROTOCOL_ERROR"));
    assert_eq!(worker.plugin_slots.available_permits(), 8);
    claim.plugin = Some(plugin_binding(
        "acme/crash",
        format!("sha256:{}", "f".repeat(64)),
        "export async function execute(){return {status:'completed',outputs:{main:[]}}}".into(),
    ));
    install_runtime_artifact(
        &worker,
        claim.task.tenant_id,
        claim.plugin.as_ref().unwrap(),
    )
    .await;
    let recovered = execute(&worker, &claim).await;
    assert_eq!(recovered.status, WorkerResultStatusV1::Succeeded);
}

#[tokio::test]
async fn plugin_failure_preserves_its_retry_decision() {
    let pool = MySqlPoolOptions::new()
        .connect_lazy("mysql://user:password@127.0.0.1/unused")
        .unwrap();
    let worker =
        RuntimeWorker::new_with_provider(pool, Arc::new(InMemory::new()), Arc::new(NoProvider));
    let attempt_id = Uuid::now_v7();
    let claim = ClaimedWorkerAttempt {
            lease: WorkerAttemptLeaseV1 { protocol_version: 1, attempt_id, worker_id: Uuid::now_v7(), fencing_token: 1, locked_until: OffsetDateTime::now_utc() + time::Duration::seconds(30) },
            task: WorkerTaskV1 { protocol_version: 1, task_id: Uuid::now_v7(), tenant_id: Uuid::now_v7(), execution_id: Uuid::now_v7(), node_execution_id: Uuid::now_v7(), attempt_id, capability: NodeCapability::PluginNodejs, bundle_id: Uuid::now_v7(), work_package_id: None, state_version: 1, compatibility_hash: ContentHash::parse(format!("sha256:{}", "a".repeat(64))).unwrap(), deadline_at: OffsetDateTime::now_utc() + time::Duration::seconds(10) },
            node_type: "acme.retry".into(), node_version: 1, run_index: 0, iteration_index: 0, timeout_ms: 5_000,
            invocation_id: None,
            node_parameters: json!({}), per_item_parameters: vec![], string_conversions: json!([]), inputs: BTreeMap::new(), resources: vec![], context: json!({}),
            plugin: Some(plugin_binding("acme/retry", format!("sha256:{}", "9".repeat(64)), "export async function execute(){return {status:'failed',code:'TRY_AGAIN',message:'transient',retryable:true,details:{phase:'remote'}}}".into())),
            trace_parent_span_entity_id: None,
        };
    install_runtime_artifact(
        &worker,
        claim.task.tenant_id,
        claim.plugin.as_ref().unwrap(),
    )
    .await;
    let result = execute(&worker, &claim).await;
    assert_eq!(result.retryable, Some(true));
    assert!(result.error_message.as_deref().unwrap().contains("phase"));
}

#[tokio::test]
async fn bounded_invocations_wake_waiters_and_keep_fast_work_independent() {
    let pool = MySqlPoolOptions::new()
        .connect_lazy("mysql://user:password@127.0.0.1/unused")
        .unwrap();
    let mut worker =
        RuntimeWorker::new_with_provider(pool, Arc::new(InMemory::new()), Arc::new(NoProvider));
    worker.plugin_slots = Arc::new(Semaphore::new(2));
    let tenant = Uuid::now_v7();
    let binding = plugin_binding("acme/concurrent", format!("sha256:{}", "9".repeat(64)),
        "export async function execute(ctx){const started=Date.now();await new Promise(resolve=>setTimeout(resolve,ctx.parameters.delay));return {status:'completed',outputs:{main:[{json:{started,finished:Date.now()}}]}}}".into());
    install_runtime_artifact(&worker, tenant, &binding).await;
    let claims = [800, 50, 50].map(|delay| {
        let attempt = Uuid::now_v7();
        ClaimedWorkerAttempt {
            lease: WorkerAttemptLeaseV1 {
                protocol_version: 1,
                attempt_id: attempt,
                worker_id: Uuid::now_v7(),
                fencing_token: 1,
                locked_until: OffsetDateTime::now_utc() + time::Duration::seconds(20),
            },
            task: WorkerTaskV1 {
                protocol_version: 1,
                task_id: Uuid::now_v7(),
                tenant_id: tenant,
                execution_id: Uuid::now_v7(),
                node_execution_id: Uuid::now_v7(),
                attempt_id: attempt,
                capability: NodeCapability::PluginNodejs,
                bundle_id: Uuid::now_v7(),
                work_package_id: None,
                state_version: 1,
                compatibility_hash: ContentHash::parse(format!("sha256:{}", "a".repeat(64)))
                    .unwrap(),
                deadline_at: OffsetDateTime::now_utc() + time::Duration::seconds(10),
            },
            node_type: "acme.concurrent".into(),
            invocation_id: None,
            node_version: 1,
            run_index: 0,
            iteration_index: 0,
            timeout_ms: 5000,
            node_parameters: json!({"delay":delay}),
            per_item_parameters: vec![],
            string_conversions: json!([]),
            inputs: BTreeMap::new(),
            resources: vec![],
            context: json!({}),
            plugin: Some(binding.clone()),
            trace_parent_span_entity_id: None,
        }
    });
    let results =
        futures::future::join_all(claims.iter().map(|claim| execute(&worker, claim))).await;
    for result in &results {
        assert_eq!(
            result.status,
            WorkerResultStatusV1::Succeeded,
            "{:?}",
            result.error_message
        );
    }
    let outputs = results
        .iter()
        .map(|result| &result.outputs["main"][0].json)
        .collect::<Vec<_>>();
    let slow_end = outputs[0]["finished"].as_i64().unwrap();
    assert!(outputs[1]["finished"].as_i64().unwrap() < slow_end);
    assert!(
        outputs[2]["finished"].as_i64().unwrap() < slow_end,
        "queued invocation must wake as soon as one process exits"
    );
    assert_eq!(worker.plugin_slots.available_permits(), 2);
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut code = 0_u32;
    let alive =
        unsafe { GetExitCodeProcess(handle, &mut code) } != 0 && code == STILL_ACTIVE as u32;
    unsafe { CloseHandle(handle) };
    alive
}
