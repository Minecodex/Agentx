use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

#[derive(Clone, Debug, Deserialize)]
struct TableDisposition {
    current_table: String,
    decision: String,
    control_replacement: Option<String>,
    runtime_replacement: Option<String>,
    authoritative_writer: String,
    allowed_readers: Vec<String>,
    cross_plane_contract: String,
    retention_and_delete_rule: String,
    owning_task: String,
}

#[derive(Clone, Debug, Deserialize)]
struct V2SchemaTableOwnership {
    table: String,
    schema_file: Option<String>,
    plane: String,
    authoritative_writer: String,
    allowed_readers: Vec<String>,
    cross_plane_contract: String,
    retention_and_delete_rule: String,
    owning_task: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LegacyDependency {
    package: String,
    dependency: String,
    reason: String,
    owner: String,
    delete_stage: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CountedException {
    path: String,
    value: String,
    count: usize,
    reason: String,
    owner: String,
    delete_stage: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FileException {
    path: String,
    reason: String,
    owner: String,
    delete_stage: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BoundaryPolicy {
    runtime_packages: BTreeSet<String>,
    control_packages: BTreeSet<String>,
    observability_packages: BTreeSet<String>,
    #[serde(default)]
    runtime_gateway_redis_allowed_modules: BTreeSet<String>,
    legacy_direct_dependencies: Vec<LegacyDependency>,
    #[serde(default)]
    legacy_rust_imports: Vec<CountedException>,
    legacy_sql_references: Vec<CountedException>,
    legacy_dynamic_sql: Vec<CountedException>,
    legacy_boundary_tokens: Vec<CountedException>,
    legacy_network_policy_files: Vec<FileException>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Plane {
    Control,
    Runtime,
    Observability,
    V2Operations,
    Legacy,
    Shared,
}

#[derive(Clone, Debug)]
struct Finding {
    path: String,
    value: String,
}

fn main() -> Result<()> {
    let mut arguments = env::args().skip(1);
    let command = arguments.next().unwrap_or_else(|| "check".into());
    let root = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    match command.as_str() {
        "check" => {
            validate_production_sources(&root)?;
            check_repository(&root)
        }
        "snapshot" => snapshot(&root),
        "fixture" => check_fixture(&root),
        _ => bail!("usage: agentx-boundary-check [check|snapshot|fixture] [root]"),
    }
}

fn check_repository(root: &Path) -> Result<()> {
    let tables = load_tables(root)?;
    let v2_tables = load_v2_tables(root)?;
    let policy = load_policy(root)?;
    let mut failures = Vec::new();
    check_policy_metadata(&policy, &mut failures);
    check_table_catalog(root, &tables, &mut failures)?;
    check_v2_initial_schemas(root, &tables, &mut failures)?;
    check_v2_incremental_schemas(root, &v2_tables, &mut failures)?;
    check_cargo(root, &policy, &mut failures)?;
    check_agent_core_boundary(root, &mut failures)?;
    check_sql(root, &tables, &v2_tables, &policy, &mut failures)?;
    check_boundary_tokens(root, &policy, &mut failures)?;
    check_runtime_gateway_redis(root, &policy, &mut failures)?;
    check_provider_http_clients(root, &mut failures)?;
    check_network_policies(root, &policy, &mut failures)?;
    check_line_limits(root, &mut failures)?;
    finish(failures)
}

fn validate_production_sources(root: &Path) -> Result<()> {
    for directory in ["src/crates", "src/services", "src/web/src", "src/plugins"] {
        let path = root.join(directory);
        if !path.is_dir() {
            bail!("required source directory missing: {directory}");
        }
        let has_source = WalkDir::new(&path)
            .into_iter()
            .filter_entry(|entry| {
                !matches!(
                    entry.file_name().to_str(),
                    Some("node_modules" | "dist" | "vendor")
                )
            })
            .filter_map(Result::ok)
            .any(|entry| {
                entry.file_type().is_file()
                    && matches!(
                        entry.path().extension().and_then(|value| value.to_str()),
                        Some("rs" | "ts" | "tsx" | "mjs")
                    )
            });
        if !has_source {
            bail!("required source directory contains no source files: {directory}");
        }
    }
    Ok(())
}

fn check_agent_core_boundary(root: &Path, failures: &mut Vec<String>) -> Result<()> {
    let crate_root = root.join("src/crates/agentx-agent-core");
    if !crate_root.is_dir() {
        return Ok(());
    }
    let cargo = fs::read_to_string(crate_root.join("Cargo.toml"))?;
    for dependency in [
        "sqlx",
        "redis",
        "reqwest",
        "object_store",
        "kube",
        "k8s-openapi",
        "vaultrs",
        "tokio",
    ] {
        let dependency_line = Regex::new(&format!(r"(?m)^{}\s*=", regex::escape(dependency)))?;
        if dependency_line.is_match(&cargo) {
            failures.push(format!(
                "Agent Core Cargo boundary violation: {dependency} is a direct dependency"
            ));
        }
    }
    let source_root = crate_root.join("src");
    for path in WalkDir::new(&source_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("rs"))
        .filter(|path| !path.starts_with(source_root.join("bin")))
    {
        let source = fs::read_to_string(&path)?;
        for forbidden in [
            "std::fs",
            "std::process",
            "tokio::process",
            "reqwest::",
            "sqlx::",
            "redis::",
            "object_store::",
            "kube::",
        ] {
            if source.contains(forbidden) {
                failures.push(format!(
                    "Agent Core infrastructure boundary violation: {} contains {forbidden}",
                    relative(root, &path)
                ));
            }
        }
    }
    Ok(())
}

fn check_provider_http_clients(root: &Path, failures: &mut Vec<String>) -> Result<()> {
    let runtime_source = root.join("src/services/agentx-v2-runtime/src");
    if !runtime_source.is_dir() {
        return Ok(());
    }
    let direct_client_allowlist = BTreeSet::from([
        "src/services/agentx-v2-runtime/src/egress.rs",
        "src/services/agentx-v2-runtime/src/sandbox.rs",
        "src/services/agentx-v2-runtime/src/vault.rs",
        "src/services/agentx-v2-runtime/src/bin/runtime-fault-proxy.rs",
        "src/services/agentx-v2-runtime/src/bin/sandbox-manager.rs",
    ]);
    let forbidden_patterns = [
        (
            Regex::new(r"\breqwest\s*::\s*(?:Client|ClientBuilder)\b")?,
            "direct reqwest Client/ClientBuilder reference",
        ),
        (
            Regex::new(r"(?s)\buse\s+reqwest\s*::\s*\{[^}]*\b(?:Client|ClientBuilder)\b")?,
            "reqwest Client/ClientBuilder import",
        ),
        (
            Regex::new(r"(?m)\b(?:use|extern\s+crate)\s+reqwest\s+as\s+\w+\s*;")?,
            "aliased reqwest crate import",
        ),
        (
            Regex::new(r"\breqwest_client_builder_with_ca\s*\(")?,
            "unmanaged CA-aware reqwest builder",
        ),
    ];
    for path in WalkDir::new(runtime_source)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("rs"))
    {
        let relative_path = relative(root, &path);
        if direct_client_allowlist.contains(relative_path.as_str()) {
            continue;
        }
        let source = fs::read_to_string(path)?;
        for (pattern, description) in &forbidden_patterns {
            if pattern.is_match(&source) {
                failures.push(format!(
                    "Provider HTTP boundary violation: {relative_path} contains {description}; external requests must use ProviderHttpClient"
                ));
            }
        }
    }
    Ok(())
}

fn check_v2_incremental_schemas(
    root: &Path,
    tables: &BTreeMap<String, V2SchemaTableOwnership>,
    failures: &mut Vec<String>,
) -> Result<()> {
    let control = incremental_migration_table_names(&root.join("deploy/migrations/control"))?;
    let runtime = incremental_migration_table_names(&root.join("deploy/migrations/runtime"))?;
    let observability =
        incremental_migration_table_names(&root.join("deploy/migrations/observability"))?;
    let expected_control = tables
        .values()
        .filter(|row| {
            row.plane == "control"
                && !row
                    .schema_file
                    .as_deref()
                    .is_some_and(|file| file.ends_with("0001_initial.sql"))
        })
        .map(|row| row.table.clone())
        .collect::<BTreeSet<_>>();
    let expected_runtime = tables
        .values()
        .filter(|row| {
            row.plane == "runtime"
                && !row
                    .schema_file
                    .as_deref()
                    .is_some_and(|file| file.ends_with("0001_initial.sql"))
        })
        .map(|row| row.table.clone())
        .collect::<BTreeSet<_>>();
    let expected_observability = tables
        .values()
        .filter(|row| row.plane == "observability")
        .map(|row| row.table.clone())
        .collect::<BTreeSet<_>>();
    compare_schema_tables("Control incremental", &control, &expected_control, failures);
    compare_schema_tables("Runtime incremental", &runtime, &expected_runtime, failures);
    compare_schema_tables(
        "Observability incremental",
        &observability,
        &expected_observability,
        failures,
    );
    for row in tables.values() {
        if !matches!(row.plane.as_str(), "control" | "runtime" | "observability") {
            failures.push(format!(
                "{} has invalid V2 schema plane {}",
                row.table, row.plane
            ));
        }
        if row.authoritative_writer.trim().is_empty()
            || row.allowed_readers.is_empty()
            || row.cross_plane_contract.trim().is_empty()
            || row.retention_and_delete_rule.trim().is_empty()
            || row.owning_task.trim().is_empty()
        {
            failures.push(format!("{} has incomplete V2 schema ownership", row.table));
        }
    }
    Ok(())
}

fn incremental_migration_table_names(directory: &Path) -> Result<BTreeSet<String>> {
    if !directory.exists() {
        return Ok(BTreeSet::new());
    }
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("sql")
            || path.file_name().and_then(|value| value.to_str()) == Some("0001_initial.sql")
        {
            continue;
        }
        for name in migration_table_names(&path)? {
            if !names.insert(name.clone()) {
                bail!("incremental migrations create table {name} more than once");
            }
        }
    }
    Ok(names)
}

fn check_v2_initial_schemas(
    root: &Path,
    tables: &BTreeMap<String, TableDisposition>,
    failures: &mut Vec<String>,
) -> Result<()> {
    let control_path = root.join("deploy/migrations/control/0001_initial.sql");
    let runtime_path = root.join("deploy/migrations/runtime/0001_initial.sql");
    if !control_path.exists() && !runtime_path.exists() {
        return Ok(());
    }
    if !control_path.exists() || !runtime_path.exists() {
        failures.push("Control and Runtime initial schemas must be introduced together".into());
        return Ok(());
    }
    let control = migration_table_names(&control_path)?;
    let runtime = migration_table_names(&runtime_path)?;
    let mut expected_control = BTreeSet::new();
    let mut expected_runtime = BTreeSet::new();
    for row in tables.values() {
        if let Some(name) = row.control_replacement.as_deref() {
            let name = replacement_table_name(name);
            if name != "_sqlx_migrations" {
                expected_control.insert(name.to_owned());
            }
        }
        if let Some(name) = row.runtime_replacement.as_deref() {
            let name = replacement_table_name(name);
            if name != "_sqlx_migrations" {
                expected_runtime.insert(name.to_owned());
            }
        }
    }
    for row in load_v2_tables(root)?.values().filter(|row| {
        row.schema_file
            .as_deref()
            .is_some_and(|file| file.ends_with("0001_initial.sql"))
    }) {
        match row.plane.as_str() {
            "control" => {
                expected_control.insert(row.table.clone());
            }
            "runtime" => {
                expected_runtime.insert(row.table.clone());
            }
            _ => {}
        }
    }
    compare_schema_tables("Control", &control, &expected_control, failures);
    compare_schema_tables("Runtime", &runtime, &expected_runtime, failures);
    for deleted in ["release_schema_contract", "trace_delivery_offsets"] {
        if control.contains(deleted) || runtime.contains(deleted) {
            failures.push(format!(
                "delete table {deleted} appears in a V2 initial schema"
            ));
        }
    }
    let observability_path = root.join("deploy/migrations/observability/0001_initial.sql");
    if !observability_path.exists() {
        failures.push("Observability initial schema is missing".into());
        return Ok(());
    }
    let observability = fs::read_to_string(observability_path)?;
    for required in [
        "event_id UUID",
        "tenant_id UUID",
        "execution_id UUID",
        "ReplacingMergeTree",
    ] {
        if !observability.contains(required) {
            failures.push(format!("Observability migration is missing {required}"));
        }
    }
    Ok(())
}

fn replacement_table_name(value: &str) -> &str {
    value.rsplit('.').next().unwrap_or(value)
}

fn migration_table_names(path: &Path) -> Result<BTreeSet<String>> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let pattern = Regex::new(r"(?im)^CREATE TABLE\s+`?([a-z0-9_]+)`?")?;
    Ok(pattern
        .captures_iter(&source)
        .map(|capture| capture[1].to_owned())
        .collect())
}

fn compare_schema_tables(
    plane: &str,
    actual: &BTreeSet<String>,
    expected: &BTreeSet<String>,
    failures: &mut Vec<String>,
) {
    if actual != expected {
        failures.push(format!(
            "{plane} initial schema mismatch: missing={:?}, extra={:?}",
            expected.difference(actual).collect::<Vec<_>>(),
            actual.difference(expected).collect::<Vec<_>>()
        ));
    }
}

fn snapshot(root: &Path) -> Result<()> {
    let tables = load_tables(root)?;
    let v2_tables = load_v2_tables(root)?;
    let mut policy = load_policy(root)?;
    policy.legacy_rust_imports = legacy_import_findings(root)?
        .into_iter()
        .fold(
            BTreeMap::<(String, String), usize>::new(),
            |mut counts, item| {
                *counts.entry((item.path, item.value)).or_default() += 1;
                counts
            },
        )
        .into_iter()
        .map(|((path, value), count)| CountedException {
            owner: owner_for_path(&path).into(),
            delete_stage: delete_stage_for_path(&path).into(),
            path,
            value,
            count,
            reason: "frozen direct Legacy Rust import; replace through the owning plane infrastructure or contract"
                .into(),
        })
        .collect();
    policy.legacy_sql_references = sql_findings(root, &tables, &v2_tables)?
        .into_iter()
        .filter(|finding| !table_access_is_allowed(root, finding, &tables, &v2_tables))
        .fold(
            BTreeMap::<(String, String), usize>::new(),
            |mut counts, item| {
                *counts.entry((item.path, item.value)).or_default() += 1;
                counts
            },
        )
        .into_iter()
        .map(|((path, value), count)| {
            let task = tables
                .get(&value)
                .map(|row| row.owning_task.as_str())
                .or_else(|| v2_tables.get(&value).map(|row| row.owning_task.as_str()))
                .unwrap_or("V2A-009");
            CountedException {
                owner: format!("{} / {task}", owner_for_path(&path)),
                delete_stage: delete_stage_for_task(task).into(),
                path,
                value,
                count,
                reason:
                    "frozen V1 cross-plane SQL; replacement is assigned by table-disposition.json"
                        .into(),
            }
        })
        .collect();
    policy.legacy_dynamic_sql = dynamic_sql_findings(root)?
        .into_iter()
        .fold(
            BTreeMap::<(String, String), usize>::new(),
            |mut counts, item| {
                *counts.entry((item.path, item.value)).or_default() += 1;
                counts
            },
        )
        .into_iter()
        .map(|((path, value), count)| CountedException {
            owner: owner_for_path(&path).into(),
            delete_stage: delete_stage_for_path(&path).into(),
            path,
            value,
            count,
            reason: "frozen V1 dynamic SQL; replace with statically attributable queries".into(),
        })
        .collect();
    policy.legacy_boundary_tokens = boundary_token_findings(root)?
        .into_iter()
        .fold(
            BTreeMap::<(String, String), usize>::new(),
            |mut counts, item| {
                *counts.entry((item.path, item.value)).or_default() += 1;
                counts
            },
        )
        .into_iter()
        .map(|((path, value), count)| CountedException {
            owner: owner_for_path(&path).into(),
            delete_stage: delete_stage_for_path(&path).into(),
            path,
            value,
            count,
            reason: "frozen V1 shared configuration; typed V2 construction has no alias fallback"
                .into(),
        })
        .collect();
    let path = root.join("docs/planv2/contracts/boundary-policy.json");
    let mut output = serde_json::to_vec_pretty(&policy)?;
    output.push(b'\n');
    fs::write(&path, output).with_context(|| format!("write {}", path.display()))?;
    println!("updated frozen V1 boundary snapshot at {}", path.display());
    Ok(())
}

fn load_tables(root: &Path) -> Result<BTreeMap<String, TableDisposition>> {
    let path = root.join("docs/planv2/contracts/table-disposition.json");
    let rows: Vec<TableDisposition> = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )?;
    let mut result = BTreeMap::new();
    for row in rows {
        if result.insert(row.current_table.clone(), row).is_some() {
            bail!("table disposition contains a duplicate table");
        }
    }
    Ok(result)
}

fn load_v2_tables(root: &Path) -> Result<BTreeMap<String, V2SchemaTableOwnership>> {
    let path = root.join("docs/planv2/contracts/v2-schema-table-ownership.json");
    let rows: Vec<V2SchemaTableOwnership> = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )?;
    let mut result = BTreeMap::new();
    for row in rows {
        if result.insert(row.table.clone(), row).is_some() {
            bail!("V2 schema ownership contains a duplicate table");
        }
    }
    Ok(result)
}

fn load_policy(root: &Path) -> Result<BoundaryPolicy> {
    let path = root.join("docs/planv2/contracts/boundary-policy.json");
    serde_json::from_slice(&fs::read(&path).with_context(|| format!("read {}", path.display()))?)
        .context("parse boundary policy")
}

fn check_policy_metadata(policy: &BoundaryPolicy, failures: &mut Vec<String>) {
    const GENERIC_OWNERS: &[&str] = &[
        "source file owner",
        "table owning task",
        "service owning task",
        "owner",
        "tbd",
    ];
    let mut check = |kind: &str, path: &str, reason: &str, owner: &str, stage: &str| {
        if reason.trim().is_empty()
            || owner.trim().is_empty()
            || stage.trim().is_empty()
            || GENERIC_OWNERS
                .iter()
                .any(|generic| owner.trim().eq_ignore_ascii_case(generic))
        {
            failures.push(format!(
                "{kind} exception {path} has missing or generic ownership metadata"
            ));
        }
    };
    for item in &policy.legacy_direct_dependencies {
        check(
            "Cargo",
            &item.package,
            &item.reason,
            &item.owner,
            &item.delete_stage,
        );
    }
    for (kind, items) in [
        ("Legacy Rust import", policy.legacy_rust_imports.as_slice()),
        ("SQL", policy.legacy_sql_references.as_slice()),
        ("dynamic SQL", policy.legacy_dynamic_sql.as_slice()),
        ("Env/Secret", policy.legacy_boundary_tokens.as_slice()),
    ] {
        for item in items {
            check(
                kind,
                &item.path,
                &item.reason,
                &item.owner,
                &item.delete_stage,
            );
        }
    }
    for item in &policy.legacy_network_policy_files {
        check(
            "NetworkPolicy",
            &item.path,
            &item.reason,
            &item.owner,
            &item.delete_stage,
        );
    }
}

fn owner_for_path(path: &str) -> &'static str {
    if path.contains("agentx-boundary-check") {
        "architecture guard / V2A-009"
    } else if path.contains("agentx-runtime/src/compiler") {
        "runtime contracts / V2A-002"
    } else if path.contains("src/services/platform-api") {
        "platform-control / V2D-001"
    } else if path.contains("src/services/trace-writer") {
        "observability / V2Q-005"
    } else if path.contains("trigger-gateway") {
        "runtime gateway / V2G-001..008"
    } else if path.contains("workflow-coordinator") {
        "runtime coordinator / V2R-001..015"
    } else if path.contains("workflow-worker") {
        "runtime worker / V2R-005..013"
    } else if path.contains("sandbox-manager") {
        "runtime sandbox / V2R-009"
    } else if path.contains("agentx-runtime-infrastructure") {
        "runtime infrastructure / V2R-001"
    } else {
        "architecture migration / V2C-001"
    }
}

fn delete_stage_for_path(path: &str) -> &'static str {
    if path.contains("agentx-boundary-check") || path.contains("agentx-runtime/src/compiler") {
        "V2-00"
    } else if path.contains("src/services/platform-api")
        || path.contains("src/services/trace-writer")
    {
        "V2-05"
    } else if path.contains("trigger-gateway") {
        "V2-03"
    } else {
        "V2-04"
    }
}

fn delete_stage_for_task(task: &str) -> &'static str {
    if task.contains("V2G-") {
        "V2-03"
    } else if task.contains("V2R-") {
        "V2-04"
    } else if task.contains("V2Q-") {
        "V2-05"
    } else if task.contains("V2D-") {
        "V2-01"
    } else {
        "V2-08"
    }
}

fn check_table_catalog(
    root: &Path,
    tables: &BTreeMap<String, TableDisposition>,
    failures: &mut Vec<String>,
) -> Result<()> {
    let catalog = fs::read_to_string(root.join("docs/reference/mysql-schema-catalog.md"))?;
    let pattern = Regex::new(r"(?m)^### ([a-z0-9_]+)$")?;
    let names = pattern
        .captures_iter(&catalog)
        .map(|capture| capture[1].to_owned())
        .collect::<BTreeSet<_>>();
    let disposition_names = tables.keys().cloned().collect::<BTreeSet<_>>();
    if names != disposition_names {
        failures.push(format!(
            "table catalog mismatch: missing={:?}, extra={:?}",
            names.difference(&disposition_names).collect::<Vec<_>>(),
            disposition_names.difference(&names).collect::<Vec<_>>()
        ));
    }
    if names.len() < 133 {
        failures.push(format!(
            "expected at least 133 MySQL tables, found {}",
            names.len()
        ));
    }
    for row in tables.values() {
        if !matches!(
            row.decision.as_str(),
            "control" | "runtime" | "split" | "delete"
        ) {
            failures.push(format!(
                "{} has invalid decision {}",
                row.current_table, row.decision
            ));
        }
        if row.authoritative_writer.trim().is_empty()
            || row.allowed_readers.is_empty()
            || row.cross_plane_contract.trim().is_empty()
            || row.retention_and_delete_rule.trim().is_empty()
            || row.owning_task.trim().is_empty()
        {
            failures.push(format!(
                "{} has an incomplete disposition",
                row.current_table
            ));
        }
        match row.decision.as_str() {
            "control" if row.control_replacement.is_none() || row.runtime_replacement.is_some() => {
                failures.push(format!(
                    "{} has inconsistent control replacements",
                    row.current_table
                ));
            }
            "runtime" if row.runtime_replacement.is_none() || row.control_replacement.is_some() => {
                failures.push(format!(
                    "{} has inconsistent runtime replacements",
                    row.current_table
                ));
            }
            "split" if row.runtime_replacement.is_none() || row.control_replacement.is_none() => {
                failures.push(format!(
                    "{} must name both split replacements",
                    row.current_table
                ));
            }
            "delete" if row.runtime_replacement.is_some() || row.control_replacement.is_some() => {
                failures.push(format!(
                    "{} delete decision has a replacement table",
                    row.current_table
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn check_cargo(root: &Path, policy: &BoundaryPolicy, failures: &mut Vec<String>) -> Result<()> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .context("run cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let legacy_allowed = policy
        .legacy_direct_dependencies
        .iter()
        .map(|item| (item.package.as_str(), item.dependency.as_str()))
        .collect::<BTreeSet<_>>();
    for package in metadata["packages"]
        .as_array()
        .context("metadata packages")?
    {
        let name = package["name"].as_str().context("package name")?;
        let targets = package["targets"].as_array().context("package targets")?;
        if targets.is_empty()
            || targets.iter().any(|target| {
                target["src_path"]
                    .as_str()
                    .is_none_or(|path| !Path::new(path).is_file())
            })
        {
            bail!("workspace member has missing source targets: {name}");
        }
        let dependencies = package["dependencies"].as_array().context("dependencies")?;
        let dependency_names = dependencies
            .iter()
            .filter_map(|dependency| dependency["name"].as_str())
            .collect::<BTreeSet<_>>();
        if policy.runtime_packages.contains(name)
            && dependency_names.contains("agentx-control-infrastructure")
        {
            failures.push(format!(
                "runtime package {name} depends on Control Infrastructure"
            ));
        }
        if name == "agentx-control-infrastructure"
            && dependency_names.iter().any(|dependency| {
                matches!(
                    *dependency,
                    "agentx-runtime-infrastructure" | "redis" | "clickhouse"
                )
            })
        {
            failures.push("Control Infrastructure depends on Runtime/Redis/ClickHouse".into());
        }
        if name == "agentx-runtime-contracts"
            && dependency_names.iter().any(|dependency| {
                matches!(*dependency, "sqlx" | "redis" | "clickhouse" | "axum")
                    || dependency.starts_with("agentx-")
                        && !matches!(
                            *dependency,
                            "agentx-domain" | "agentx-node-protocol" | "agentx-agent-core"
                        )
            })
        {
            failures
                .push("Runtime Contracts has a forbidden infrastructure/service dependency".into());
        }
        if dependency_names.contains("agentx-infrastructure-legacy")
            && !legacy_allowed.contains(&(name, "agentx-infrastructure-legacy"))
            && name != "agentx-infrastructure-legacy"
        {
            failures.push(format!(
                "{name} introduces a new direct Legacy Infrastructure dependency"
            ));
        }
    }
    Ok(())
}

fn check_sql(
    root: &Path,
    tables: &BTreeMap<String, TableDisposition>,
    v2_tables: &BTreeMap<String, V2SchemaTableOwnership>,
    policy: &BoundaryPolicy,
    failures: &mut Vec<String>,
) -> Result<()> {
    let disallowed = sql_findings(root, tables, v2_tables)?
        .into_iter()
        .filter(|finding| !table_access_is_allowed(root, finding, tables, v2_tables))
        .collect::<Vec<_>>();
    compare_counts("SQL", disallowed, &policy.legacy_sql_references, failures);
    compare_counts(
        "dynamic SQL",
        dynamic_sql_findings(root)?,
        &policy.legacy_dynamic_sql,
        failures,
    );
    Ok(())
}

fn legacy_import_findings(root: &Path) -> Result<Vec<Finding>> {
    let pattern = Regex::new(r"\bagentx_infrastructure\b")?;
    let mut findings = Vec::new();
    for path in rust_sources(root)? {
        let relative = relative(root, &path);
        if relative.starts_with("src/crates/agentx-infrastructure/") {
            continue;
        }
        let source = fs::read_to_string(&path)?;
        for _ in pattern.find_iter(&source) {
            findings.push(Finding {
                path: relative.clone(),
                value: "agentx-infrastructure-legacy".into(),
            });
        }
    }
    Ok(findings)
}

fn sql_findings(
    root: &Path,
    tables: &BTreeMap<String, TableDisposition>,
    v2_tables: &BTreeMap<String, V2SchemaTableOwnership>,
) -> Result<Vec<Finding>> {
    let table_pattern = tables
        .keys()
        .chain(v2_tables.keys())
        .map(|table| regex::escape(table))
        .collect::<Vec<_>>()
        .join("|");
    let pattern = Regex::new(&format!(
        r"(?i)\b(?:from|join|update|into|table)\s+`?({table_pattern})`?\b"
    ))?;
    let mut findings = Vec::new();
    for path in rust_sources(root)? {
        let relative = relative(root, &path);
        for literal in rust_string_literals(&fs::read_to_string(&path)?) {
            for capture in pattern.captures_iter(&literal) {
                findings.push(Finding {
                    path: relative.clone(),
                    value: capture[1].to_ascii_lowercase(),
                });
            }
        }
    }
    Ok(findings)
}

fn dynamic_sql_findings(root: &Path) -> Result<Vec<Finding>> {
    let pattern = Regex::new(
        r#"(?is)format!\s*\(\s*(?:r[#]*|b?r[#]*|b)?"[^"]*(?:\bselect\b[^"]*\bfrom\b|\binsert\s+into\b|\bupdate\b[^"]*\bset\b|\bdelete\s+from\b|\bfrom\b|\bjoin\b|\btable\b)[^"]*\{[^"]*\}"#,
    )?;
    let mut findings = Vec::new();
    for path in rust_sources(root)? {
        if relative(root, &path).starts_with("tools/agentx-boundary-check/") {
            continue;
        }
        let source = fs::read_to_string(&path)?;
        for matched in pattern.find_iter(&source) {
            findings.push(Finding {
                path: relative(root, &path),
                value: normalized_snippet(matched.as_str()),
            });
        }
    }
    Ok(findings)
}

fn table_access_is_allowed(
    root: &Path,
    finding: &Finding,
    tables: &BTreeMap<String, TableDisposition>,
    v2_tables: &BTreeMap<String, V2SchemaTableOwnership>,
) -> bool {
    let plane = plane_for_path(root, Path::new(&finding.path));
    if let Some(row) = v2_tables.get(&finding.value) {
        return matches!(
            (plane, row.plane.as_str()),
            (Plane::Control, "control")
                | (Plane::Runtime, "runtime")
                | (Plane::Observability, "observability")
                | (Plane::V2Operations, _)
        );
    }
    let row = tables.get(&finding.value);
    let decision = row.map(|row| row.decision.as_str());
    matches!(
        (plane, decision),
        (Plane::Control, Some("control"))
            | (Plane::Runtime, Some("runtime"))
            | (Plane::V2Operations, _)
            | (Plane::Shared, _)
    ) || decision == Some("split")
        && ((plane == Plane::Control
            && finding
                .path
                .starts_with("src/crates/agentx-control-infrastructure/"))
            || (plane == Plane::Runtime
                && finding
                    .path
                    .starts_with("src/crates/agentx-runtime-infrastructure/"))
            || row.is_some_and(|row| match plane {
                Plane::Control => row
                    .control_replacement
                    .as_deref()
                    .is_some_and(|name| replacement_table_name(name) == finding.value),
                Plane::Runtime => row
                    .runtime_replacement
                    .as_deref()
                    .is_some_and(|name| replacement_table_name(name) == finding.value),
                _ => false,
            }))
}

fn boundary_token_findings(root: &Path) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    for path in relevant_text_files(root)? {
        let plane = plane_for_path(root, &path);
        let source = fs::read_to_string(&path)?;
        let tokens: &[&str] = match plane {
            Plane::Runtime => &[
                "AGENTX_CONTROL_MYSQL",
                "agentx-control-infrastructure",
                "control-mysql-secret",
            ],
            Plane::Control => &[
                "AGENTX_RUNTIME_REDIS",
                "AGENTX_RUNTIME_MYSQL",
                "agentx-runtime-infrastructure",
                "runtime-redis-secret",
                "AGENTX_REDIS",
            ],
            Plane::Observability => &[
                "AGENTX_RUNTIME_MYSQL",
                "AGENTX_CONTROL_MYSQL",
                "AGENTX_RUNTIME_S3",
                "AGENTX_CONTROL_S3",
                "runtime-mysql-secret",
                "control-mysql-secret",
            ],
            _ => &[],
        };
        for token in tokens {
            for _ in source.match_indices(token) {
                findings.push(Finding {
                    path: relative(root, &path),
                    value: (*token).into(),
                });
            }
        }
    }
    Ok(findings)
}

fn check_boundary_tokens(
    root: &Path,
    policy: &BoundaryPolicy,
    failures: &mut Vec<String>,
) -> Result<()> {
    compare_counts(
        "Legacy Rust import",
        legacy_import_findings(root)?,
        &policy.legacy_rust_imports,
        failures,
    );
    compare_counts(
        "Env/Secret boundary",
        boundary_token_findings(root)?,
        &policy.legacy_boundary_tokens,
        failures,
    );
    Ok(())
}

fn check_runtime_gateway_redis(
    root: &Path,
    policy: &BoundaryPolicy,
    failures: &mut Vec<String>,
) -> Result<()> {
    let source_root = root.join("src/services/agentx-v2-runtime/src");
    if !source_root.exists() {
        return Ok(());
    }
    for path in WalkDir::new(&source_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("rs"))
    {
        let relative_path = relative(root, &path);
        if policy
            .runtime_gateway_redis_allowed_modules
            .contains(&relative_path)
            || relative_path.starts_with("src/services/agentx-v2-runtime/src/bin/")
        {
            continue;
        }
        let source = fs::read_to_string(&path)?;
        for token in [
            "redis::",
            "RuntimeRedisSettings",
            "runtime_redis_client",
            "connect_runtime_redis",
            "AGENTX_RUNTIME_REDIS_",
        ] {
            if source.contains(token) {
                failures.push(format!(
                    "Runtime Gateway Redis boundary violation: {relative_path} uses {token}; only the declared admission, SSE and trace-delivery hosts may access Redis"
                ));
            }
        }
    }
    Ok(())
}

fn check_network_policies(
    root: &Path,
    policy: &BoundaryPolicy,
    failures: &mut Vec<String>,
) -> Result<()> {
    let cross_plane_database = Regex::new(
        r"(?s)(agentx\.io/plane: control.{0,240}agentx\.io/plane: runtime.{0,240}port: 3306|agentx\.io/plane: runtime.{0,240}agentx\.io/plane: control.{0,240}port: 3306)",
    )?;
    let allowed = policy
        .legacy_network_policy_files
        .iter()
        .map(|item| item.path.as_str())
        .collect::<BTreeSet<_>>();
    for path in WalkDir::new(root.join("deploy"))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("yaml" | "yml")
            )
        })
    {
        let source = fs::read_to_string(&path)?;
        if !source.contains("kind: NetworkPolicy") {
            continue;
        }
        let relative = relative(root, &path);
        let broad = source.contains("podSelector: {}")
            && source.contains("policyTypes: [Ingress, Egress]")
            && (source.contains("ingress:\n    - {}") || source.contains("egress:\n    - {}"));
        let cross_plane = cross_plane_database.is_match(&source);
        if (broad || cross_plane) && !allowed.contains(relative.as_str()) {
            failures.push(format!(
                "NetworkPolicy {relative} grants an unapproved broad/cross-plane path"
            ));
        }
    }
    Ok(())
}

fn check_line_limits(root: &Path, failures: &mut Vec<String>) -> Result<()> {
    for path in source_files(root)? {
        let relative_path = relative(root, &path);
        if relative_path.split('/').any(|part| part == "tests")
            || path
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|stem| stem.ends_with("_test") || stem.ends_with("_tests"))
        {
            continue;
        }
        let lines = fs::read_to_string(&path)?.lines().count();
        if lines > 2_000 {
            failures.push(format!(
                "{} has {lines} lines (maximum 2000)",
                relative_path
            ));
        }
    }
    Ok(())
}

fn compare_counts(
    label: &str,
    findings: Vec<Finding>,
    exceptions: &[CountedException],
    failures: &mut Vec<String>,
) {
    let mut actual = BTreeMap::<(String, String), usize>::new();
    for item in findings {
        *actual.entry((item.path, item.value)).or_default() += 1;
    }
    let allowed = exceptions
        .iter()
        .map(|item| ((item.path.clone(), item.value.clone()), item.count))
        .collect::<BTreeMap<_, _>>();
    for ((path, value), count) in actual {
        let maximum = allowed
            .get(&(path.clone(), value.clone()))
            .copied()
            .unwrap_or(0);
        if count > maximum {
            failures.push(format!(
                "{label} boundary violation: {path}: {value} ({count} > frozen {maximum})"
            ));
        }
    }
}

fn plane_for_path(_root: &Path, path: &Path) -> Plane {
    let value = path.to_string_lossy().replace('\\', "/");
    if value.contains("src/crates/agentx-infrastructure/") {
        Plane::Legacy
    } else if value.contains("src/crates/agentx-v2-ops/") {
        // This package is the only cross-plane schema/bootstrap/doctor runner. It
        // builds a plane-specific client from a plane-specific environment and
        // never exposes repositories to an application service.
        Plane::V2Operations
    } else if value.contains("src/services/platform-api/")
        || value.contains("src/services/platform-control/")
        || value.contains("src/crates/agentx-control-infrastructure/")
    {
        Plane::Control
    } else if value.contains("src/services/trace-writer/")
        || value.contains("src/services/observability/")
    {
        Plane::Observability
    } else if [
        "src/services/trigger-gateway/",
        "src/services/workflow-coordinator/",
        "src/services/workflow-worker/",
        "src/services/sandbox-manager/",
        "src/services/agentx-v2-runtime/",
        "src/crates/agentx-runtime-infrastructure/",
        "src/crates/agentx-runtime/",
    ]
    .iter()
    .any(|fragment| value.contains(fragment))
    {
        Plane::Runtime
    } else {
        Plane::Shared
    }
}

fn rust_string_literals(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut values = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let raw_start = if bytes[index] == b'r' {
            Some(index + 1)
        } else if bytes[index] == b'b' && bytes.get(index + 1) == Some(&b'r') {
            Some(index + 2)
        } else {
            None
        };
        if let Some(mut cursor) = raw_start {
            let mut hashes = 0;
            while bytes.get(cursor) == Some(&b'#') {
                hashes += 1;
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'\"') {
                let content_start = cursor + 1;
                cursor = content_start;
                while cursor < bytes.len() {
                    if bytes[cursor] == b'\"'
                        && bytes.get(cursor + 1..cursor + 1 + hashes)
                            == Some(&vec![b'#'; hashes][..])
                    {
                        values.push(source[content_start..cursor].to_owned());
                        index = cursor + 1 + hashes;
                        break;
                    }
                    cursor += 1;
                }
                if index >= cursor {
                    continue;
                }
            }
        }
        let quote = if bytes[index] == b'\"' {
            Some(index)
        } else if bytes[index] == b'b' && bytes.get(index + 1) == Some(&b'\"') {
            Some(index + 1)
        } else {
            None
        };
        if let Some(quote) = quote {
            let mut cursor = quote + 1;
            let mut escaped = false;
            while cursor < bytes.len() {
                if !escaped && bytes[cursor] == b'\"' {
                    values.push(source[quote + 1..cursor].to_owned());
                    index = cursor + 1;
                    break;
                }
                escaped = !escaped && bytes[cursor] == b'\\';
                if bytes[cursor] != b'\\' {
                    escaped = false;
                }
                cursor += 1;
            }
            if index >= cursor {
                continue;
            }
        }
        index += 1;
    }
    values
}

fn rust_sources(root: &Path) -> Result<Vec<PathBuf>> {
    Ok(source_files(root)?
        .into_iter()
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("rs"))
        .collect())
}

fn source_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for directory in ["src/crates", "src/services", "src/web/src", "src/plugins"] {
        let directory = root.join(directory);
        if !directory.exists() {
            continue;
        }
        files.extend(
            WalkDir::new(directory)
                .into_iter()
                .filter_entry(|entry| {
                    !matches!(
                        entry.file_name().to_str(),
                        Some("node_modules" | "dist" | "vendor")
                    )
                })
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .map(|entry| entry.into_path())
                .filter(|path| {
                    matches!(
                        path.extension().and_then(|value| value.to_str()),
                        Some("rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "css")
                    )
                }),
        );
    }
    Ok(files)
}

fn relevant_text_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = source_files(root)?;
    for directory in ["deploy", "src/crates", "src/services"] {
        files.extend(
            WalkDir::new(root.join(directory))
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .map(|entry| entry.into_path())
                .filter(|path| {
                    matches!(
                        path.extension().and_then(|value| value.to_str()),
                        Some("toml" | "yaml" | "yml")
                    )
                }),
        );
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalized_snippet(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn finish(failures: Vec<String>) -> Result<()> {
    if failures.is_empty() {
        println!("Agentx V2 boundary checks passed");
        Ok(())
    } else {
        for failure in &failures {
            eprintln!("boundary error: {failure}");
        }
        bail!("{} boundary check(s) failed", failures.len())
    }
}

fn check_fixture(root: &Path) -> Result<()> {
    check_repository(root)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{check_agent_core_boundary, rust_string_literals, validate_production_sources};

    #[test]
    fn production_scan_rejects_missing_or_empty_source_roots() {
        let temporary = tempfile::tempdir().expect("temporary repository");
        let missing = validate_production_sources(temporary.path())
            .unwrap_err()
            .to_string();
        assert!(missing.contains("required source directory missing: src/crates"));
        for directory in ["src/crates", "src/services", "src/web/src", "src/plugins"] {
            fs::create_dir_all(temporary.path().join(directory)).unwrap();
        }
        let empty = validate_production_sources(temporary.path())
            .unwrap_err()
            .to_string();
        assert!(empty.contains("contains no source files"));
        for directory in ["src/crates", "src/services", "src/web/src", "src/plugins"] {
            fs::write(
                temporary.path().join(directory).join("source.ts"),
                "export {};",
            )
            .unwrap();
        }
        validate_production_sources(temporary.path()).unwrap();
    }

    #[test]
    fn sql_scanner_reads_normal_raw_and_byte_string_literals() {
        let values = rust_string_literals(
            r###"let a = "SELECT * FROM tenants"; let b = r#"UPDATE workflows SET name=?"#; let c = b"DELETE FROM users";"###,
        );
        assert!(values.iter().any(|value| value.contains("FROM tenants")));
        assert!(
            values
                .iter()
                .any(|value| value.contains("UPDATE workflows"))
        );
        assert!(
            values
                .iter()
                .any(|value| value.contains("DELETE FROM users"))
        );
    }

    #[test]
    fn agent_core_boundary_rejects_infrastructure_and_host_file_apis() {
        let temporary = tempfile::tempdir().expect("temporary repository");
        let crate_root = temporary.path().join("src/crates/agentx-agent-core");
        fs::create_dir_all(crate_root.join("src")).expect("create core source");
        fs::write(
            crate_root.join("Cargo.toml"),
            "[package]\nname='agentx-agent-core'\nversion='0.0.0'\n[dependencies]\nreqwest='0.12'\n",
        )
        .expect("write cargo");
        fs::write(
            crate_root.join("src/lib.rs"),
            "fn forbidden() { let _ = std::fs::read_to_string(\"host\"); }",
        )
        .expect("write source");
        let mut failures = Vec::new();
        check_agent_core_boundary(temporary.path(), &mut failures).expect("run boundary check");
        assert!(failures.iter().any(|failure| failure.contains("reqwest")));
        assert!(failures.iter().any(|failure| failure.contains("std::fs")));
    }
}
