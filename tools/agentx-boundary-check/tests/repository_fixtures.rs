use std::{fs, path::Path, process::Command};

use serde_json::json;
use tempfile::TempDir;

#[test]
fn positive_fixture_passes_the_production_checker() {
    let fixture = fixture_repository();
    assert_checker(&fixture, true, "positive");
}

#[test]
fn same_named_tables_have_independent_plane_ownership() {
    let fixture = fixture_repository();
    let root = fixture.path();
    let rows = ["control", "runtime"].map(|plane| {
        json!({
            "table": "paired_publications", "plane": plane,
            "authoritative_writer": format!("{plane} publisher"),
            "allowed_readers": [format!("{plane} API")],
            "cross_plane_contract": "immutable publication copy",
            "retention_and_delete_rule": "retain referenced publications",
            "owning_task": "P9-03",
            "schema_file": format!("deploy/migrations/{plane}/0002_publications.sql")
        })
    });
    write(
        root,
        "docs/planv2/contracts/v2-schema-table-ownership.json",
        &serde_json::to_string(&rows).unwrap(),
    );
    for plane in ["control", "runtime"] {
        write(
            root,
            &format!("deploy/migrations/{plane}/0002_publications.sql"),
            "CREATE TABLE paired_publications (id BINARY(16) PRIMARY KEY);\n",
        );
    }
    for source in [
        "src/crates/agentx-control-infrastructure/src/lib.rs",
        "src/services/workflow-worker/src/main.rs",
    ] {
        write(
            root,
            source,
            "fn access() { let _ = \"SELECT * FROM paired_publications\"; }\n",
        );
    }
    assert_checker(
        &fixture,
        true,
        "shared table name with independent ownership",
    );
    write(
        root,
        "src/services/workflow-worker/src/main.rs",
        "fn forbidden() { let _ = \"SELECT * FROM control_only_table\"; }\n",
    );
    assert_checker(
        &fixture,
        false,
        "Control-only data is still forbidden to Runtime",
    );
    let duplicate = vec![rows[0].clone(), rows[0].clone(), rows[1].clone()];
    write(
        root,
        "docs/planv2/contracts/v2-schema-table-ownership.json",
        &serde_json::to_string(&duplicate).unwrap(),
    );
    assert_checker(&fixture, false, "duplicate ownership in the same plane");
}

#[test]
fn all_five_negative_fixtures_fail_the_production_checker() {
    for category in ["cargo", "sql", "env", "secret", "network"] {
        let fixture = fixture_repository();
        introduce_violation(fixture.path(), category);
        assert_checker(&fixture, false, category);
    }
}

#[test]
fn unregistered_incremental_table_fails_the_production_checker() {
    let fixture = fixture_repository();
    introduce_violation(fixture.path(), "incremental_schema");
    assert_checker(&fixture, false, "incremental_schema");
}

#[test]
fn runtime_gateway_redis_outside_sse_wakeup_fails_the_production_checker() {
    let fixture = fixture_repository();
    introduce_violation(fixture.path(), "gateway_redis");
    assert_checker(&fixture, false, "gateway_redis");
}

#[test]
fn unmanaged_provider_http_client_fails_the_production_checker() {
    for category in [
        "provider_http",
        "provider_http_alias",
        "provider_http_builder_import",
        "provider_http_ca_builder",
        "provider_http_nested_module",
    ] {
        let fixture = fixture_repository();
        introduce_violation(fixture.path(), category);
        assert_checker(&fixture, false, category);
    }
}

fn assert_checker(fixture: &TempDir, expected_success: bool, category: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_agentx-boundary-check"))
        .arg("fixture")
        .arg(fixture.path())
        .output()
        .expect("run production boundary checker");
    assert_eq!(
        output.status.success(),
        expected_success,
        "{category} fixture returned {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn fixture_repository() -> TempDir {
    let directory = TempDir::new().expect("fixture directory");
    let root = directory.path();
    write(
        root,
        "Cargo.toml",
        r#"[workspace]
members = ["src/crates/agentx-control-infrastructure", "src/services/workflow-worker"]
resolver = "3"

[workspace.package]
edition = "2024"
license = "Apache-2.0"
rust-version = "1.85"
version = "0.1.0"
"#,
    );
    write(
        root,
        "src/crates/agentx-control-infrastructure/Cargo.toml",
        r#"[package]
name = "agentx-control-infrastructure"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
version.workspace = true
"#,
    );
    write(
        root,
        "src/crates/agentx-control-infrastructure/src/lib.rs",
        "pub fn control_only() {}\n",
    );
    write(
        root,
        "src/services/workflow-worker/Cargo.toml",
        r#"[package]
name = "workflow-worker"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
version.workspace = true
"#,
    );
    write(
        root,
        "src/services/workflow-worker/src/main.rs",
        "fn main() {}\n",
    );

    let names = std::iter::once("control_only_table".to_owned())
        .chain((1..133).map(|index| format!("fixture_table_{index:03}")))
        .collect::<Vec<_>>();
    let catalog = names
        .iter()
        .map(|name| format!("### {name}\n"))
        .collect::<String>();
    write(root, "docs/reference/mysql-schema-catalog.md", &catalog);
    let disposition = names
        .iter()
        .map(|name| {
            json!({
                "current_table": name,
                "decision": "control",
                "control_replacement": name,
                "runtime_replacement": null,
                "authoritative_writer": "platform-control",
                "allowed_readers": ["platform-control"],
                "cross_plane_contract": "none",
                "retention_and_delete_rule": "fixture only",
                "owning_task": "V2D-001"
            })
        })
        .collect::<Vec<_>>();
    write(
        root,
        "docs/planv2/contracts/table-disposition.json",
        &format!("{}\n", serde_json::to_string_pretty(&disposition).unwrap()),
    );
    write(
        root,
        "docs/planv2/contracts/v2-schema-table-ownership.json",
        "[]\n",
    );
    let policy = json!({
        "runtime_packages": ["workflow-worker"],
        "control_packages": ["agentx-control-infrastructure"],
        "observability_packages": [],
        "runtime_gateway_redis_allowed_modules": ["src/services/agentx-v2-runtime/src/sse_wakeup.rs"],
        "legacy_direct_dependencies": [],
        "legacy_rust_imports": [],
        "legacy_sql_references": [],
        "legacy_dynamic_sql": [],
        "legacy_boundary_tokens": [],
        "legacy_network_policy_files": []
    });
    write(
        root,
        "docs/planv2/contracts/boundary-policy.json",
        &format!("{}\n", serde_json::to_string_pretty(&policy).unwrap()),
    );
    directory
}

fn introduce_violation(root: &Path, category: &str) {
    match category {
        "cargo" => write(
            root,
            "src/services/workflow-worker/Cargo.toml",
            r#"[package]
name = "workflow-worker"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
version.workspace = true

[dependencies]
agentx-control-infrastructure = { path = "../../crates/agentx-control-infrastructure" }
"#,
        ),
        "sql" => write(
            root,
            "src/services/workflow-worker/src/main.rs",
            "fn main() { let _ = \"SELECT * FROM control_only_table\"; }\n",
        ),
        "env" => write(
            root,
            "src/services/workflow-worker/src/main.rs",
            "fn main() { let _ = std::env::var(\"AGENTX_CONTROL_MYSQL_PASSWORD\"); }\n",
        ),
        "secret" => write(
            root,
            "src/services/workflow-worker/deployment.yaml",
            "secretName: control-mysql-secret\n",
        ),
        "network" => write(
            root,
            "deploy/runtime/network-policy.yaml",
            r#"apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: forbidden-cross-plane
spec:
  from:
    agentx.io/plane: control
  to:
    agentx.io/plane: runtime
  ports:
    - port: 3306
"#,
        ),
        "incremental_schema" => write(
            root,
            "deploy/migrations/runtime/0002_unregistered.sql",
            "CREATE TABLE unregistered_runtime_fact (id BINARY(16) NOT NULL PRIMARY KEY);\n",
        ),
        "gateway_redis" => write(
            root,
            "src/services/agentx-v2-runtime/src/gateway.rs",
            "fn forbidden(client: redis::Client) { let _ = client; }\n",
        ),
        "provider_http" => write(
            root,
            "src/services/agentx-v2-runtime/src/resource_check.rs",
            "fn forbidden() { let _ = reqwest::Client::new(); }\n",
        ),
        "provider_http_alias" => write(
            root,
            "src/services/agentx-v2-runtime/src/resource_check.rs",
            "use reqwest as external_http; fn forbidden() { let _ = external_http::Client::new(); }\n",
        ),
        "provider_http_builder_import" => write(
            root,
            "src/services/agentx-v2-runtime/src/worker_runtime.rs",
            "use reqwest::{header::HeaderMap, ClientBuilder as ProviderBuilder}; fn forbidden() { let _ = ProviderBuilder::new(); }\n",
        ),
        "provider_http_ca_builder" => write(
            root,
            "src/services/agentx-v2-runtime/src/trigger.rs",
            "fn forbidden() { let _ = agentx_service_kit::reqwest_client_builder_with_ca(\"CA_PATH\"); }\n",
        ),
        "provider_http_nested_module" => write(
            root,
            "src/services/agentx-v2-runtime/src/worker_runtime/provider.rs",
            "type ProviderClient = reqwest::Client;\n",
        ),
        _ => panic!("unknown fixture category {category}"),
    }
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
    fs::write(path, content).expect("write fixture file");
}
