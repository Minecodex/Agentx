use crate::{
    assets::{EmbeddedAssets, RELEASE_SCHEMA},
    config::DeploymentConfig,
    helm::{Helm, INGRESS_RELEASE, release_name},
    process, secrets,
};
use anyhow::{Result, bail};
use chrono::Utc;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub async fn validate(
    config: &DeploymentConfig,
    assets: &EmbeddedAssets,
    targets: &[&str],
    cluster: bool,
) -> Result<Value> {
    let helm = Helm { config, assets };
    let mut tools = helm.tool_versions().await?;
    helm.lint(targets).await?;
    let secret_status = if cluster {
        let cluster =
            process::run_command(["kubectl", "cluster-info"], None, None, 30, true, None).await?;
        tools.as_object_mut().unwrap().insert(
            "cluster".into(),
            cluster.stdout.lines().next().unwrap_or("reachable").into(),
        );
        let mut manifests = Vec::new();
        for target in targets {
            manifests.push(helm.template(target).await?);
        }
        secrets::ensure_existing_secret_references(config, &manifests).await?
    } else {
        json!({"status":"not-checked"})
    };
    Ok(json!({
        "status":"valid", "environment":config.environment(), "targets":targets,
        "namespaces":config.namespaces(), "tools":tools, "secrets":secret_status,
    }))
}

pub async fn render(
    config: &DeploymentConfig,
    assets: &EmbeddedAssets,
    targets: &[&str],
) -> Result<String> {
    let helm = Helm { config, assets };
    let mut rendered = Vec::new();
    for target in targets {
        rendered.push(helm.template(target).await?.trim_end().to_owned());
    }
    Ok(format!("{}\n", rendered.join("\n---\n")))
}

async fn ensure_namespaces(config: &DeploymentConfig, targets: &[&str]) -> Result<()> {
    let _ = targets;
    // Namespace labels are cluster-global security state: the NetworkPolicies
    // of every plane trust the dependencies namespace's ingress label, so a
    // narrow `--target control` upgrade must still reconcile all of them.
    // kubectl apply is idempotent, so this is safe for any target.
    let planes = vec!["dependencies", "control", "runtime"];
    for plane in planes {
        let mut labels = Map::from_iter([
            ("agentx.io/plane".into(), Value::String(plane.into())),
            (
                "app.kubernetes.io/managed-by".into(),
                Value::String("agentxctl".into()),
            ),
        ]);
        if plane == "dependencies" {
            labels.insert("agentx.io/ingress".into(), "allowed".into());
        }
        if plane != "dependencies" {
            for key in ["enforce", "audit", "warn"] {
                labels.insert(
                    format!("pod-security.kubernetes.io/{key}"),
                    "restricted".into(),
                );
            }
        }
        let manifest = json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":config.namespace(plane),"labels":labels}});
        process::run_command(
            ["kubectl", "apply", "-f", "-"],
            None,
            Some(&serde_json::to_string(&manifest)?),
            60,
            true,
            None,
        )
        .await?;
    }
    Ok(())
}

fn replica_paths(target: &str) -> BTreeMap<&'static str, &'static str> {
    match target {
        "control" => BTreeMap::from([
            ("web-console", "control.services.webConsole.replicas"),
            (
                "platform-control",
                "control.services.platformControl.replicas",
            ),
        ]),
        "runtime" => BTreeMap::from([
            (
                "runtime-gateway",
                "runtime.services.runtimeGateway.replicas",
            ),
            (
                "workflow-runtime",
                "runtime.services.workflowRuntime.replicas",
            ),
            (
                "workflow-worker",
                "runtime.services.workflowWorker.replicas",
            ),
            (
                "sandbox-manager",
                "runtime.services.sandboxManager.replicas",
            ),
        ]),
        "observability" => BTreeMap::from([(
            "observability",
            "observability.services.observability.replicas",
        )]),
        "dependencies" => BTreeMap::from([(
            "agentx-egress-gateway",
            "dependencies.services.egressGateway.replicas",
        )]),
        _ => BTreeMap::new(),
    }
}

async fn current_replica_values(helm: &Helm<'_>, target: &str) -> Result<BTreeMap<String, u64>> {
    if helm.release_status(target).await?.is_none() {
        return Ok(BTreeMap::new());
    }
    let result = process::run_command(
        [
            "kubectl",
            "-n",
            helm.config.namespace(target),
            "get",
            "deployment",
            "-o",
            "json",
        ],
        None,
        None,
        60,
        true,
        None,
    )
    .await?
    .json()?;
    let paths = replica_paths(target);
    let mut values = BTreeMap::new();
    for item in result
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = item.pointer("/metadata/name").and_then(Value::as_str) else {
            continue;
        };
        if let Some(path) = paths.get(name) {
            values.insert(
                (*path).into(),
                item.pointer("/spec/replicas")
                    .and_then(Value::as_u64)
                    .unwrap_or(1),
            );
        }
    }
    Ok(values)
}

pub async fn install(
    config: &DeploymentConfig,
    assets: &EmbeddedAssets,
    targets: &[&str],
    run_doctor: bool,
    preserve_replicas: bool,
) -> Result<Value> {
    validate(config, assets, targets, true).await?;
    let helm = Helm { config, assets };
    if !targets.contains(&"dependencies") && helm.release_status("dependencies").await?.is_none() {
        bail!("{} requires the agentx-dependencies release", targets[0]);
    }
    ensure_namespaces(config, targets).await?;
    if config.string("/global/secrets/mode") == Some("generated-local") {
        secrets::ensure_local_secrets(config, targets).await?;
        let mut manifests = Vec::new();
        for target in targets {
            manifests.push(helm.template(target).await?);
        }
        secrets::ensure_secret_references(config, &manifests).await?;
    }
    if targets.contains(&"dependencies") {
        helm.install_ingress().await?;
    }
    for target in targets {
        let replicas = if preserve_replicas {
            current_replica_values(&helm, target).await?
        } else {
            BTreeMap::new()
        };
        helm.upgrade_install(target, &replicas).await?;
    }
    if run_doctor {
        doctor(config, assets, targets).await?;
    }
    let mut result = status(config, assets, targets, Some("ready")).await?;
    if config.environment() == "production" {
        result.as_object_mut().unwrap().insert(
            "releaseManifest".into(),
            write_release_manifest(config)?
                .to_string_lossy()
                .into_owned()
                .into(),
        );
    }
    Ok(result)
}

pub async fn doctor(
    config: &DeploymentConfig,
    assets: &EmbeddedAssets,
    targets: &[&str],
) -> Result<Value> {
    let helm = Helm { config, assets };
    let mut checked = Vec::new();
    for target in targets {
        if helm.release_status(target).await?.is_none() {
            bail!("release is not installed: {}", release_name(target));
        }
        helm.test_release(target).await?;
        checked.push(*target);
    }
    Ok(json!({"status":"healthy","targets":checked}))
}

pub async fn status(
    config: &DeploymentConfig,
    assets: &EmbeddedAssets,
    targets: &[&str],
    expected: Option<&str>,
) -> Result<Value> {
    let helm = Helm { config, assets };
    let mut releases = Vec::new();
    for target in targets {
        let helm_status = helm.release_status(target).await?;
        let mut release = json!({
            "target":target, "name":release_name(target), "namespace":config.namespace(target),
            "installed":helm_status.is_some(),
        });
        if let Some(helm_status) = helm_status {
            let object = release.as_object_mut().unwrap();
            object.insert(
                "revision".into(),
                helm_status.get("version").cloned().unwrap_or(Value::Null),
            );
            object.insert(
                "state".into(),
                helm_status
                    .pointer("/info/status")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            object.insert(
                "updated".into(),
                helm_status
                    .pointer("/info/last_deployed")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            let resources = process::run_command(
                [
                    "kubectl",
                    "-n",
                    config.namespace(target),
                    "get",
                    "deployment,statefulset,job,pdb",
                    "-l",
                    &format!("agentx.io/plane={target}"),
                    "-o",
                    "json",
                ],
                None,
                None,
                60,
                false,
                None,
            )
            .await?;
            if resources.status == 0 {
                object.insert(
                    "resources".into(),
                    serde_json::from_str::<Value>(&resources.stdout)?
                        .get("items")
                        .cloned()
                        .unwrap_or(json!([])),
                );
            }
        }
        releases.push(release);
    }
    let services = config.array("/global/images/services").unwrap();
    let images: Map<String, Value> = services
        .iter()
        .map(|service| {
            let name = service.as_str().unwrap();
            (name.into(), Value::String(image_reference(config, name)))
        })
        .collect();
    Ok(json!({
        "status":expected.unwrap_or("observed"), "environment":config.environment(), "namespaces":config.namespaces(),
        "releases":releases, "images":images,
        "endpoints":{
            "control":config.ingress_url("control"),
            "runtime":config.ingress_url("runtime"),
        }
    }))
}

fn write_release_manifest(config: &DeploymentConfig) -> Result<std::path::PathBuf> {
    let services = config.array("/global/images/services").unwrap();
    let images: Vec<_> = services.iter().map(|service| {
        let name = service.as_str().unwrap();
        json!({"name":name,"digest":config.string(&format!("/global/images/digests/{name}")).unwrap()})
    }).collect();
    let manifest = json!({
        "schemaVersion":"agentx.io/v2-release/v1", "version":config.string("/global/images/tag").unwrap(),
        "gitCommit":config.string("/global/images/sourceCommit").unwrap(), "generatedAt":Utc::now().to_rfc3339(),
        "protocolVersion":1, "compatibleProtocolVersions":[1], "images":images,
    });
    let schema: Value = serde_json::from_str(RELEASE_SCHEMA)?;
    jsonschema::validator_for(&schema)?
        .validate(&manifest)
        .map_err(|error| anyhow::anyhow!("release manifest validation failed: {error}"))?;
    let output_dir = std::env::current_dir()?.join(".local/artifacts/releases");
    std::fs::create_dir_all(&output_dir)?;
    let output = output_dir.join(format!(
        "{}-{}.json",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        config.string("/global/images/tag").unwrap()
    ));
    std::fs::write(&output, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(output)
}

pub async fn rollback(
    config: &DeploymentConfig,
    assets: &EmbeddedAssets,
    target: &str,
    revision: u64,
) -> Result<Value> {
    if target == "all" {
        bail!("rollback requires one explicit target");
    }
    process::run_command(
        [
            "helm".into(),
            "rollback".into(),
            release_name(target).into(),
            revision.to_string(),
            "--namespace".into(),
            config.namespace(target).into(),
            "--cleanup-on-fail".into(),
            "--wait".into(),
            "--wait-for-jobs".into(),
            "--timeout".into(),
            "10m".into(),
        ],
        None,
        None,
        720,
        true,
        None,
    )
    .await?;
    Helm { config, assets }.test_release(target).await?;
    status(config, assets, &[target], Some("rolled-back")).await
}

pub async fn uninstall(
    config: &DeploymentConfig,
    assets: &EmbeddedAssets,
    targets: &[&str],
    purge_data: bool,
    confirmed: bool,
) -> Result<Value> {
    if purge_data
        && (config.environment() == "production"
            || targets != ["dependencies", "control", "runtime", "observability"]
            || !confirmed)
    {
        bail!("data purge requires local/test values, --target all, and --yes");
    }
    let helm = Helm { config, assets };
    if targets == ["dependencies"] && helm.release_status("runtime").await?.is_some() {
        bail!("dependencies uninstall is refused while the runtime release exists");
    }
    let ingress_users = if targets.contains(&"dependencies") {
        ingress_users(config, targets).await?
    } else {
        Vec::new()
    };
    if !ingress_users.is_empty() && purge_data {
        bail!(
            "cannot purge while IngressClass {} is still used: {}",
            config.string("/global/ingress/className").unwrap(),
            ingress_users.join(", ")
        );
    }
    for target in targets.iter().rev() {
        process::run_command(
            [
                "helm",
                "uninstall",
                release_name(target),
                "--namespace",
                config.namespace(target),
                "--ignore-not-found",
            ],
            None,
            None,
            300,
            true,
            None,
        )
        .await?;
    }
    if targets.contains(&"dependencies") && ingress_users.is_empty() {
        process::run_command(
            [
                "helm",
                "uninstall",
                INGRESS_RELEASE,
                "--namespace",
                config.namespace("dependencies"),
                "--ignore-not-found",
            ],
            None,
            None,
            300,
            false,
            None,
        )
        .await?;
    }
    if purge_data {
        let namespaces: BTreeSet<_> = config.namespaces().into_values().collect();
        for namespace in &namespaces {
            process::run_command(
                [
                    "kubectl",
                    "delete",
                    "namespace",
                    namespace,
                    "--ignore-not-found",
                    "--wait=true",
                ],
                None,
                None,
                600,
                true,
                None,
            )
            .await?;
        }
        return Ok(json!({"status":"purged","namespaces":namespaces}));
    }
    Ok(json!({"status":"uninstalled","namespacesPreserved":true,"targets":targets}))
}

async fn ingress_users(config: &DeploymentConfig, targets: &[&str]) -> Result<Vec<String>> {
    let class_name = config.string("/global/ingress/className").unwrap();
    let ingresses = process::run_command(
        [
            "kubectl",
            "get",
            "ingress",
            "--all-namespaces",
            "-o",
            "json",
        ],
        None,
        None,
        60,
        false,
        None,
    )
    .await?;
    let mut users = Vec::new();
    if ingresses.status == 0 {
        for item in ingresses
            .json()?
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if item
                .pointer("/spec/ingressClassName")
                .and_then(Value::as_str)
                == Some(class_name)
            {
                let namespace = item
                    .pointer("/metadata/namespace")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let owner = item
                    .pointer("/metadata/annotations/meta.helm.sh~1release-name")
                    .and_then(Value::as_str);
                let removed_with_target = targets.iter().any(|target| {
                    owner == Some(release_name(target)) && namespace == config.namespace(target)
                });
                if removed_with_target {
                    continue;
                }
                users.push(format!(
                    "{}/{}",
                    namespace,
                    item.pointer("/metadata/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                ));
            }
        }
    }
    Ok(users)
}

pub async fn migrate(
    config: &DeploymentConfig,
    assets: &EmbeddedAssets,
    target: &str,
    contract: bool,
    timeout: u64,
) -> Result<Value> {
    if !["control", "runtime", "observability"].contains(&target) {
        bail!("migrate target must be control, runtime, or observability");
    }
    if contract {
        let result = process::run_command(
            [
                "kubectl",
                "-n",
                config.namespace(target),
                "get",
                "replicaset",
                "-o",
                "json",
            ],
            None,
            None,
            60,
            true,
            None,
        )
        .await?
        .json()?;
        let old: Vec<_> = result
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let replicas = item
                    .pointer("/spec/replicas")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let available = item
                    .pointer("/status/availableReplicas")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                (replicas > 0 && available == 0).then(|| {
                    item.pointer("/metadata/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned()
                })
            })
            .collect();
        if !old.is_empty() {
            bail!(
                "contract migration refused while old ReplicaSets exist: {}",
                old.join(", ")
            );
        }
        return Ok(json!({"status":"contract-ready","target":target,"schemaRollback":false}));
    }
    let manifest = Helm { config, assets }.template(target).await?;
    let mut jobs = Vec::new();
    for document in serde_yaml::Deserializer::from_str(&manifest) {
        let value = Value::deserialize(document)?;
        if value.get("kind").and_then(Value::as_str) == Some("Job")
            && value
                .pointer("/metadata/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .contains("migrate")
        {
            jobs.push(value);
        }
    }
    if jobs.len() != 1 {
        bail!("rendered {target} chart must contain exactly one migration Job");
    }
    let mut job = jobs.remove(0);
    let original = job
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap();
    let base = original
        .rsplit_once('-')
        .map(|(base, _)| base)
        .unwrap_or(original);
    let name = format!("{base}-manual-{}", Utc::now().format("%Y%m%d%H%M%S"));
    *job.pointer_mut("/metadata/name").unwrap() = name.clone().into();
    process::run_command(
        ["kubectl", "apply", "-f", "-"],
        None,
        Some(&serde_yaml::to_string(&job)?),
        60,
        true,
        None,
    )
    .await?;
    process::run_command(
        [
            "kubectl".into(),
            "-n".into(),
            config.namespace(target).into(),
            "wait".into(),
            "--for=condition=complete".into(),
            format!("job/{name}"),
            format!("--timeout={timeout}s"),
        ],
        None,
        None,
        timeout + 30,
        true,
        None,
    )
    .await?;
    Ok(json!({"status":"expanded","target":target,"job":name}))
}

pub async fn sync_secrets(config: &DeploymentConfig, assets: &EmbeddedAssets) -> Result<Value> {
    let updated = secrets::sync_existing_mirrors(config).await?;
    let restarts = [
        ("control", &["platform-control"][..]),
        (
            "runtime",
            &[
                "runtime-gateway",
                "workflow-runtime",
                "workflow-worker",
                "sandbox-manager",
            ][..],
        ),
        ("observability", &["observability"][..]),
        ("dependencies", &["agentx-egress-gateway"][..]),
    ];
    let helm = Helm { config, assets };
    let mut restarted = Vec::new();
    for (target, names) in restarts {
        if helm.release_status(target).await?.is_none() {
            continue;
        }
        for name in names {
            let _ = process::run_command(
                [
                    "kubectl",
                    "-n",
                    config.namespace(target),
                    "rollout",
                    "restart",
                    &format!("deployment/{name}"),
                ],
                None,
                None,
                60,
                false,
                None,
            )
            .await?;
            let _ = process::run_command(
                [
                    "kubectl",
                    "-n",
                    config.namespace(target),
                    "rollout",
                    "status",
                    &format!("deployment/{name}"),
                    "--timeout=300s",
                ],
                None,
                None,
                330,
                false,
                None,
            )
            .await?;
            restarted.push(format!("{}/{name}", config.namespace(target)));
        }
    }
    Ok(json!({"status":"synced","updatedSecrets":updated,"restarted":restarted}))
}

pub fn image_reference(config: &DeploymentConfig, service: &str) -> String {
    let registry = config
        .string("/global/images/registry")
        .unwrap()
        .trim_end_matches('/');
    let prefix = config
        .string("/global/images/repositoryPrefix")
        .unwrap_or("");
    let repository = if service.starts_with(prefix) {
        service.to_owned()
    } else {
        format!("{prefix}{service}")
    };
    let base = format!("{registry}/{repository}");
    match config.string(&format!("/global/images/digests/{service}")) {
        Some(digest) => format!("{base}@{digest}"),
        None => format!("{base}:{}", config.string("/global/images/tag").unwrap()),
    }
}

use serde::Deserialize;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TARGETS;
    use crate::process::{self, CommandRequest, test_support};
    use std::{path::PathBuf, sync::Arc};

    fn local_config(existing_secrets: bool) -> DeploymentConfig {
        let mut config = DeploymentConfig::load(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/values/local.yaml"),
            None,
        )
        .unwrap();
        if existing_secrets {
            *config.values.pointer_mut("/global/secrets/mode").unwrap() =
                "existing-kubernetes".into();
        }
        config
    }

    fn standard_response(request: &CommandRequest) -> Result<process::CommandResult> {
        let command = &request.command;
        let stdout = if command.starts_with(&["helm".into(), "version".into()]) {
            "v3.18.4"
        } else if command.starts_with(&["kubectl".into(), "version".into()]) {
            r#"{"clientVersion":{"gitVersion":"v1.33.4"}}"#
        } else if command.starts_with(&["helm".into(), "status".into()]) {
            r#"{"version":2,"info":{"status":"deployed","last_deployed":"now"}}"#
        } else if command
            .iter()
            .any(|argument| argument == "deployment,statefulset,job,pdb")
        {
            r#"{"items":[]}"#
        } else if command.starts_with(&[
            "kubectl".into(),
            "-n".into(),
            "agentx-runtime".into(),
            "get".into(),
            "deployment".into(),
        ]) {
            r#"{"items":[{"metadata":{"name":"workflow-runtime"},"spec":{"replicas":3}}]}"#
        } else if command.iter().any(|argument| argument == "secret")
            && command.iter().any(|argument| argument == "get")
        {
            r#"{"data":{}}"#
        } else if command.starts_with(&["kubectl".into(), "get".into(), "ingress".into()]) {
            r#"{"items":[]}"#
        } else {
            ""
        };
        Ok(test_support::result(request, 0, stdout))
    }

    #[test]
    fn image_reference_does_not_duplicate_prefix() {
        let config = DeploymentConfig {
            path: "values.yaml".into(),
            values: json!({"global":{"images":{"registry":"docker.io/acme","repositoryPrefix":"agentx-","tag":"dev","services":[]}}}),
        };
        assert_eq!(
            image_reference(&config, "agentx-runtime"),
            "docker.io/acme/agentx-runtime:dev"
        );
    }

    #[tokio::test]
    async fn install_uses_the_frozen_target_order_after_ingress() {
        let executor = Arc::new(test_support::RecordingExecutor::new(standard_response));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(true);
        process::with_command_executor(
            executor.clone(),
            install(&config, &assets, &TARGETS, false, false),
        )
        .await
        .unwrap();
        let releases = executor
            .requests()
            .into_iter()
            .filter(|request| {
                request.command.first().map(String::as_str) == Some("helm")
                    && request.command.get(1).map(String::as_str) == Some("upgrade")
            })
            .map(|request| request.command[3].clone())
            .collect::<Vec<_>>();
        assert_eq!(
            releases,
            [
                INGRESS_RELEASE,
                "agentx-dependencies",
                "agentx-control",
                "agentx-runtime",
                "agentx-observability",
            ]
        );
    }

    #[tokio::test]
    async fn install_labels_the_dependencies_namespace_for_ingress_access() {
        let executor = Arc::new(test_support::RecordingExecutor::new(standard_response));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(true);
        process::with_command_executor(
            executor.clone(),
            install(&config, &assets, &TARGETS, false, false),
        )
        .await
        .unwrap();

        let dependencies_namespace = executor
            .requests()
            .into_iter()
            .filter(|request| request.command == ["kubectl", "apply", "-f", "-"])
            .filter_map(|request| request.input)
            .filter_map(|input| serde_json::from_str::<Value>(&input).ok())
            .find(|manifest| {
                manifest.pointer("/kind").and_then(Value::as_str) == Some("Namespace")
                    && manifest.pointer("/metadata/name").and_then(Value::as_str)
                        == Some("agentx-deps")
            })
            .unwrap();

        assert_eq!(
            dependencies_namespace
                .pointer("/metadata/labels/agentx.io~1ingress")
                .and_then(Value::as_str),
            Some("allowed")
        );
    }

    #[tokio::test]
    async fn failed_dependency_install_never_installs_apps_or_reports_ready() {
        let executor = Arc::new(test_support::RecordingExecutor::new(|request| {
            if request.command.starts_with(&[
                "helm".into(),
                "upgrade".into(),
                "--install".into(),
                "agentx-dependencies".into(),
            ]) {
                return Ok(test_support::result(
                    request,
                    1,
                    "dependency bootstrap failed",
                ));
            }
            standard_response(request)
        }));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(true);
        let error = process::with_command_executor(
            executor.clone(),
            install(&config, &assets, &TARGETS, true, false),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("dependency bootstrap failed"));
        let requests = executor.requests();
        assert!(
            !requests
                .iter()
                .any(|request| request.command.starts_with(&["helm".into(), "test".into()]))
        );
        assert!(
            !requests.iter().any(|request| request.command.starts_with(&[
                "helm".into(),
                "upgrade".into(),
                "--install".into(),
                "agentx-control".into()
            ]))
        );
    }

    #[tokio::test]
    async fn a_single_target_requires_the_dependencies_release() {
        let executor = Arc::new(test_support::RecordingExecutor::new(|request| {
            if request.command.starts_with(&[
                "helm".into(),
                "status".into(),
                "agentx-dependencies".into(),
            ]) {
                return Ok(test_support::result(request, 1, ""));
            }
            standard_response(request)
        }));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(true);
        let error = process::with_command_executor(
            executor,
            install(&config, &assets, &["runtime"], false, false),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("requires the agentx-dependencies release"));
    }

    #[tokio::test]
    async fn upgrade_preserves_current_replicas() {
        let executor = Arc::new(test_support::RecordingExecutor::new(standard_response));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(true);
        process::with_command_executor(
            executor.clone(),
            install(&config, &assets, &["runtime"], false, true),
        )
        .await
        .unwrap();
        let upgrade = executor
            .requests()
            .into_iter()
            .find(|request| {
                request.command.starts_with(&[
                    "helm".into(),
                    "upgrade".into(),
                    "--install".into(),
                    "agentx-runtime".into(),
                ])
            })
            .unwrap();
        assert!(
            upgrade
                .command
                .windows(2)
                .any(|arguments| arguments
                    == ["--set", "runtime.services.workflowRuntime.replicas=3"])
        );
        let replica_query = executor
            .requests()
            .into_iter()
            .find(|request| {
                request.command.starts_with(&[
                    "kubectl".into(),
                    "-n".into(),
                    "agentx-runtime".into(),
                    "get".into(),
                    "deployment".into(),
                ])
            })
            .unwrap();
        assert!(
            !replica_query
                .command
                .iter()
                .any(|argument| argument == "-l")
        );
    }

    #[tokio::test]
    async fn dependencies_uninstall_is_refused_while_runtime_exists() {
        let executor = Arc::new(test_support::RecordingExecutor::new(standard_response));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(false);
        let error = process::with_command_executor(
            executor.clone(),
            uninstall(&config, &assets, &["dependencies"], false, false),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("refused while the runtime release exists"));
        assert!(!executor.requests().iter().any(|request| {
            request.command.starts_with(&[
                "helm".into(),
                "uninstall".into(),
                "agentx-dependencies".into(),
            ])
        }));
    }

    #[tokio::test]
    async fn ingress_users_block_purge_before_any_release_is_removed() {
        let executor = Arc::new(test_support::RecordingExecutor::new(|request| {
            if request
                .command
                .starts_with(&["kubectl".into(), "get".into(), "ingress".into()])
            {
                return Ok(test_support::result(
                    request,
                    0,
                    r#"{"items":[{"metadata":{"namespace":"other","name":"consumer"},"spec":{"ingressClassName":"agentx-nginx"}}]}"#,
                ));
            }
            standard_response(request)
        }));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(false);
        let error = process::with_command_executor(
            executor.clone(),
            uninstall(&config, &assets, &TARGETS, true, true),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("other/consumer"));
        assert!(!executor.requests().iter().any(|request| {
            request.command.first().map(String::as_str) == Some("helm")
                && request.command.get(1).map(String::as_str) == Some("uninstall")
        }));
    }

    #[tokio::test]
    async fn ingress_owned_by_a_selected_release_does_not_block_purge() {
        let executor = Arc::new(test_support::RecordingExecutor::new(|request| {
            if request
                .command
                .starts_with(&["kubectl".into(), "get".into(), "ingress".into()])
            {
                return Ok(test_support::result(
                    request,
                    0,
                    r#"{"items":[{"metadata":{"namespace":"agentx-control","name":"control-web","annotations":{"meta.helm.sh/release-name":"agentx-control"}},"spec":{"ingressClassName":"agentx-nginx"}}]}"#,
                ));
            }
            standard_response(request)
        }));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(false);
        let result = process::with_command_executor(
            executor.clone(),
            uninstall(&config, &assets, &TARGETS, true, true),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "purged");
        assert!(executor.requests().iter().any(|request| {
            request.command.starts_with(&[
                "helm".into(),
                "uninstall".into(),
                "agentx-control".into(),
            ])
        }));
    }

    #[tokio::test]
    async fn rollback_cleans_up_failures_waits_for_jobs_and_runs_release_test() {
        let executor = Arc::new(test_support::RecordingExecutor::new(standard_response));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(false);
        process::with_command_executor(executor.clone(), rollback(&config, &assets, "runtime", 4))
            .await
            .unwrap();
        let requests = executor.requests();
        assert!(requests.iter().any(|request| {
            request.command.starts_with(&[
                "helm".into(),
                "rollback".into(),
                "agentx-runtime".into(),
                "4".into(),
            ]) && request
                .command
                .iter()
                .any(|argument| argument == "--cleanup-on-fail")
        }));
        assert!(requests.iter().any(|request| {
            request
                .command
                .iter()
                .any(|argument| argument == "--wait-for-jobs")
        }));
        assert!(requests.iter().any(|request| {
            request
                .command
                .starts_with(&["helm".into(), "test".into(), "agentx-runtime".into()])
        }));
    }

    #[tokio::test]
    async fn migration_applies_and_waits_for_the_rendered_job() {
        let executor = Arc::new(test_support::RecordingExecutor::new(|request| {
            if request
                .command
                .starts_with(&["helm".into(), "template".into()])
            {
                return Ok(test_support::result(
                    request,
                    0,
                    "apiVersion: batch/v1\nkind: Job\nmetadata:\n  name: agentx-runtime-migrate-abc\n  namespace: agentx-runtime\nspec:\n  template:\n    spec:\n      restartPolicy: Never\n      containers: []\n",
                ));
            }
            standard_response(request)
        }));
        let assets = EmbeddedAssets::extract().unwrap();
        let config = local_config(false);
        let result = process::with_command_executor(
            executor.clone(),
            migrate(&config, &assets, "runtime", false, 45),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "expanded");
        let requests = executor.requests();
        assert!(
            requests
                .iter()
                .any(|request| request.command == ["kubectl", "apply", "-f", "-"])
        );
        assert!(requests.iter().any(|request| {
            request
                .command
                .iter()
                .any(|argument| argument == "--timeout=45s")
                && request
                    .command
                    .iter()
                    .any(|argument| argument.starts_with("job/agentx-runtime-migrate-manual-"))
        }));
    }

    #[tokio::test]
    async fn secret_sync_updates_mirrors_before_rolling_workloads() {
        let mut config = local_config(true);
        config
            .values
            .pointer_mut("/global/secrets")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(
                "workloads".into(),
                json!({"runtimeGateway":"runtime-gateway-secret"}),
            );
        let executor = Arc::new(test_support::RecordingExecutor::new(|request| {
            if request.command.iter().any(|argument| argument == "secret")
                && request.command.iter().any(|argument| argument == "get")
            {
                let value = if request
                    .command
                    .iter()
                    .any(|argument| argument == "agentx-dependencies-secrets")
                {
                    "bmV3"
                } else {
                    "b2xk"
                };
                return Ok(test_support::result(
                    request,
                    0,
                    format!(r#"{{"data":{{"SHARED_TOKEN":"{value}"}}}}"#),
                ));
            }
            standard_response(request)
        }));
        let assets = EmbeddedAssets::extract().unwrap();
        let result =
            process::with_command_executor(executor.clone(), sync_secrets(&config, &assets))
                .await
                .unwrap();
        assert_eq!(
            result["updatedSecrets"],
            json!(["agentx-runtime/runtime-gateway-secret"])
        );
        let requests = executor.requests();
        let apply = requests
            .iter()
            .position(|request| {
                request.command == ["kubectl", "apply", "-f", "-"]
                    && request
                        .input
                        .as_deref()
                        .is_some_and(|input| input.contains("runtime-gateway-secret"))
            })
            .unwrap();
        let restart = requests
            .iter()
            .position(|request| {
                request.command.iter().any(|argument| argument == "restart")
                    && request
                        .command
                        .iter()
                        .any(|argument| argument == "deployment/runtime-gateway")
            })
            .unwrap();
        assert!(apply < restart);
        assert!(requests.iter().any(|request| {
            request.command.iter().any(|argument| argument == "status")
                && request
                    .command
                    .iter()
                    .any(|argument| argument == "deployment/runtime-gateway")
        }));
    }
}
