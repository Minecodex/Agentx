use std::{collections::BTreeSet, sync::Arc, time::Duration};

use agentx_runtime_contracts::{
    ActivateDeploymentRequestV1, ActivationManifestV1, ApplyReceiptV1, PublishReceiptStatusV1,
    PublishReceiptV1, RuntimeResourceKindV1,
};
use axum::{Json, Router, extract::State, routing::post};
use ed25519_dalek::SigningKey;
use object_store::memory::InMemory;
use rand::rngs::OsRng;
use secrecy::SecretString;
use sqlx::mysql::MySqlPoolOptions;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use time::OffsetDateTime;
use tokio::{net::TcpListener, sync::Mutex};
use uuid::Uuid;

use super::{Publisher, runtime_object_idempotency_key, runtime_resource_kind_from_control};

const PRIVATE_KEY: &str =
    include_str!("../../../crates/agentx-runtime-contracts/tests/fixtures/service-private.pem");

#[test]
fn control_grant_resource_types_map_to_runtime_contract_kinds() {
    assert_eq!(
        runtime_resource_kind_from_control("mcp_tool").unwrap(),
        RuntimeResourceKindV1::Mcp
    );
    assert_eq!(
        runtime_resource_kind_from_control("mcp_server").unwrap(),
        RuntimeResourceKindV1::Mcp
    );
    assert_eq!(
        runtime_resource_kind_from_control("sandbox_profile").unwrap(),
        RuntimeResourceKindV1::SandboxProfile
    );
    assert!(runtime_resource_kind_from_control("unknown").is_err());
}

#[test]
fn object_upload_idempotency_is_stable_across_bundles() {
    let object = agentx_runtime_contracts::RuntimeObjectReferenceV1 {
        tenant_id: Uuid::now_v7(),
        storage_domain: agentx_runtime_contracts::StorageDomain::Runtime,
        object_id: Uuid::now_v7(),
        object_key: "runtime/tenant/object/hash".into(),
        content_hash: agentx_runtime_contracts::ContentHash::parse(format!(
            "sha256:{}",
            "a".repeat(64)
        ))
        .unwrap(),
        size_bytes: 42,
        media_type: "application/json".into(),
    };

    let key = runtime_object_idempotency_key(&object);
    assert_eq!(key, runtime_object_idempotency_key(&object));
    assert!(!key.contains("bundle-object"));
    assert!(key.len() <= 192);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn publisher_claim_takeover_and_transition_are_fenced() {
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

    let first = publisher(pool.clone());
    let second = publisher(pool.clone());
    let attempt_id = Uuid::now_v7();
    sqlx::query("INSERT INTO publish_attempts(id,tenant_id,application_id,deployment_id,requested_action,state,next_action,idempotency_key,activation_sequence,minimum_admission_epoch,created_by) VALUES(?,?,?,?,'publish','building','build',?,1,1,?)")
            .bind(attempt_id)
            .bind(Uuid::now_v7())
            .bind(Uuid::now_v7())
            .bind(Uuid::now_v7())
            .bind(format!("publish:{attempt_id}"))
            .bind(Uuid::now_v7())
            .execute(&pool)
            .await
            .unwrap();

    let (first_claims, second_claims) = tokio::join!(first.claim(), second.claim());
    let mut claims = first_claims.unwrap();
    claims.extend(second_claims.unwrap());
    assert_eq!(claims.len(), 1, "two Publishers must not claim one Attempt");
    let stale = claims.pop().unwrap();
    assert_eq!(stale.fencing_token, 1);

    sqlx::query("UPDATE publish_attempts SET locked_until=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE id=?")
            .bind(attempt_id)
            .execute(&pool)
            .await
            .unwrap();
    let replacement = if stale.fencing_token == 1 && stale.id == attempt_id {
        if stale_owner(&first, &stale).await {
            &second
        } else {
            &first
        }
    } else {
        unreachable!()
    };
    let current = replacement.claim().await.unwrap().pop().unwrap();
    assert_eq!(current.fencing_token, 2);

    let stale_publisher = if replacement.owner == first.owner {
        &second
    } else {
        &first
    };
    let mut stale_tx = pool.begin().await.unwrap();
    assert!(
        stale_publisher
            .transition(
                &mut stale_tx,
                &stale,
                "building",
                "copying",
                "copy_objects",
                None,
            )
            .await
            .is_err()
    );
    stale_tx.rollback().await.unwrap();

    let mut current_tx = pool.begin().await.unwrap();
    replacement
        .transition(
            &mut current_tx,
            &current,
            "building",
            "copying",
            "copy_objects",
            None,
        )
        .await
        .unwrap();
    current_tx.commit().await.unwrap();

    runtime_success_before_local_transition_is_replayed_after_takeover(&pool).await;
}

async fn runtime_success_before_local_transition_is_replayed_after_takeover(
    pool: &sqlx::MySqlPool,
) {
    let runtime_facts = Arc::new(Mutex::new(BTreeSet::<String>::new()));
    let app = Router::new()
        .route(
            "/internal/runtime/v1/deployments:activate",
            post(mock_activate),
        )
        .with_state(runtime_facts.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let first = publisher_with_url(pool.clone(), format!("http://{address}"));
    let second = publisher_with_url(pool.clone(), format!("http://{address}"));
    let attempt_id = Uuid::now_v7();
    let tenant_id = Uuid::now_v7();
    let application_id = Uuid::now_v7();
    let deployment_id = Uuid::now_v7();
    let bundle_id = Uuid::now_v7();
    sqlx::query("INSERT INTO application_deployments(id,tenant_id,application_id,workflow_version_id,environment_id,sequence_number,input_schema_json,output_schema_json,session_version_policy,status,created_by) VALUES(?,?,?,?,?,2,JSON_OBJECT(),JSON_OBJECT(),'pinned','prepared',?)")
            .bind(deployment_id)
            .bind(tenant_id)
            .bind(application_id)
            .bind(Uuid::now_v7())
            .bind(Uuid::now_v7())
            .bind(Uuid::now_v7())
            .execute(pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO publish_attempts(id,tenant_id,application_id,deployment_id,bundle_id,requested_action,state,next_action,idempotency_key,activation_sequence,minimum_admission_epoch,created_by) VALUES(?,?,?,?,?,'publish','prepared','activate',?,2,1,?)")
            .bind(attempt_id)
            .bind(tenant_id)
            .bind(application_id)
            .bind(deployment_id)
            .bind(bundle_id)
            .bind(format!("publish:{attempt_id}"))
            .bind(Uuid::now_v7())
            .execute(pool)
            .await
            .unwrap();

    let stale = first.claim().await.unwrap().pop().unwrap();
    let request = ActivateDeploymentRequestV1 {
        api_version: 1,
        idempotency_key: format!("{}:activate", stale.id),
        manifest: ActivationManifestV1 {
            api_version: 1,
            tenant_id,
            application_id,
            deployment_id,
            bundle_id,
            expected_head_version: None,
            activation_sequence: 2,
            minimum_admission_epoch: 1,
            runtime_config_revision: 1,
            runtime_policy: agentx_runtime_contracts::ApplicationRuntimePolicyV1 {
                api_user_id_input: None,
                environment_id: None,
                session_version_policy: agentx_runtime_contracts::SessionVersionPolicyV1::Pinned,
                synchronous_wait_seconds: 30,
                maximum_json_bytes: 1_048_576,
                maximum_multipart_bytes: 52_428_800,
            },
        },
    };
    let receipt: PublishReceiptV1 = first
        .post(
            "runtime.deployments.activate",
            "/internal/runtime/v1/deployments:activate",
            &request,
        )
        .await
        .unwrap();
    assert_eq!(receipt.status, PublishReceiptStatusV1::Accepted);
    let still_prepared: String =
        sqlx::query_scalar("SELECT state FROM publish_attempts WHERE id=?")
            .bind(attempt_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(still_prepared, "prepared");

    sqlx::query("UPDATE publish_attempts SET locked_until=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 1 SECOND) WHERE id=?")
            .bind(attempt_id)
            .execute(pool)
            .await
            .unwrap();
    let current = second.claim().await.unwrap().pop().unwrap();
    assert!(current.fencing_token > stale.fencing_token);
    second.activate(&current).await.unwrap();
    assert_eq!(runtime_facts.lock().await.len(), 1);
    let local: String = sqlx::query_scalar("SELECT state FROM publish_attempts WHERE id=?")
        .bind(attempt_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(local, "activating");

    let mut stale_tx = pool.begin().await.unwrap();
    assert!(
        first
            .transition(
                &mut stale_tx,
                &stale,
                "prepared",
                "activating",
                "update_head",
                Some(bundle_id),
            )
            .await
            .is_err()
    );
    stale_tx.rollback().await.unwrap();
    server.abort();
}

async fn mock_activate(
    State(facts): State<Arc<Mutex<BTreeSet<String>>>>,
    Json(request): Json<ActivateDeploymentRequestV1>,
) -> Json<PublishReceiptV1> {
    let replayed = !facts.lock().await.insert(request.idempotency_key);
    Json(PublishReceiptV1 {
        api_version: 1,
        receipt: ApplyReceiptV1 {
            api_version: 1,
            event_id: Uuid::now_v7(),
            applied: true,
            replayed,
            object_version: 1,
            result: serde_json::json!({}),
        },
        bundle_id: request.manifest.bundle_id,
        head_version: Some(1),
        activation_sequence: Some(request.manifest.activation_sequence),
        status: PublishReceiptStatusV1::Accepted,
        rejection: None,
        accepted_at: OffsetDateTime::now_utc(),
    })
}

async fn stale_owner(publisher: &Publisher, attempt: &super::Attempt) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT locked_by=? FROM publish_attempts WHERE id=?")
        .bind(publisher.owner.0)
        .bind(attempt.id)
        .fetch_one(&publisher.pool)
        .await
        .unwrap()
}

fn publisher(pool: sqlx::MySqlPool) -> Publisher {
    publisher_with_url(pool, "http://127.0.0.1:1".into())
}

fn publisher_with_url(pool: sqlx::MySqlPool, runtime_url: String) -> Publisher {
    Publisher {
        pool,
        control_objects: Arc::new(InMemory::new()),
        runtime_url,
        http: reqwest::Client::new(),
        jwt_kid: "test".into(),
        jwt_key: SecretString::from(PRIVATE_KEY),
        bundle_kid: "bundle-test".into(),
        bundle_key: SigningKey::generate(&mut OsRng),
        owner: agentx_mysql_lease::LeaseOwner::new(),
    }
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
