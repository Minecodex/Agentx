use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "cargo xtask")]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    Check {
        #[arg(long)]
        fast: bool,
    },
    Images(Images),
}

#[derive(Args)]
struct Images {
    #[arg(long)]
    values: PathBuf,
    #[arg(long)]
    service: Vec<String>,
    #[arg(long)]
    push: bool,
    #[arg(long)]
    skip_kubernetes_import: bool,
}

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf();
    match Cli::parse().command {
        Task::Check { fast } => check(&root, fast),
        Task::Images(args) => images(&root, args),
    }
}

fn run(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program_name(program))
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("start {program}"))?;
    if !status.success() {
        bail!("command failed: {program} {}", args.join(" "));
    }
    Ok(())
}

fn output(root: &Path, program: &str, args: &[&str], check: bool) -> Result<std::process::Output> {
    let output = Command::new(program_name(program))
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("start {program}"))?;
    if check && !output.status.success() {
        bail!("command failed: {program} {}", args.join(" "));
    }
    Ok(output)
}

fn program_name(program: &str) -> &str {
    if cfg!(windows) && program == "pnpm" {
        "pnpm.cmd"
    } else {
        program
    }
}

fn check(root: &Path, fast: bool) -> Result<()> {
    run(root, "cargo", &["fmt", "--all", "--", "--check"])?;
    run(
        root,
        "cargo",
        &[
            "test",
            "-p",
            "agentxctl",
            "-p",
            "agentx-key-material",
            "-p",
            "echo-mcp",
        ],
    )?;
    run(root, "uv", &["lock", "--check"])?;
    run(
        root,
        "cargo",
        &[
            "build",
            "--locked",
            "-p",
            "agentxctl",
            "-p",
            "agentx-backup-test-adapter",
        ],
    )?;
    run(
        root,
        "uv",
        &["run", "--group", "test", "ruff", "check", "."],
    )?;
    run(
        root,
        "uv",
        &["run", "--group", "test", "ruff", "format", "--check", "."],
    )?;
    run(
        root,
        "uv",
        &["run", "--group", "test", "pytest", "tests/acceptance"],
    )?;
    run(
        root,
        "uv",
        &[
            "run",
            "--group",
            "test",
            "pytest",
            "tests/e2e",
            "--collect-only",
            "-q",
            "--values",
            "deploy/values/local.yaml",
        ],
    )?;
    for values in [
        "local.yaml",
        "local-tls.yaml",
        "dockerhub-beta.yaml",
        "production.example.yaml",
    ] {
        run(
            root,
            "cargo",
            &[
                "run",
                "--quiet",
                "-p",
                "agentxctl",
                "--",
                "validate",
                "--values",
                &format!("deploy/values/{values}"),
            ],
        )?;
    }
    for directory in [
        "deploy/kustomize/addons/lightrag",
        "deploy/kustomize/addons/mem0",
        "deploy/kustomize/e2e-fixtures/runtime-providers",
    ] {
        run(root, "kubectl", &["kustomize", directory])?;
    }
    line_limits(root)?;
    if !fast {
        run(
            root,
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        )?;
        run(
            root,
            "cargo",
            &["test", "--workspace", "--", "--test-threads=1"],
        )?;
        run(
            root,
            "cargo",
            &[
                "run",
                "--quiet",
                "-p",
                "agentx-boundary-check",
                "--",
                "check",
                ".",
            ],
        )?;
        run(root, "pnpm", &["lint:web"])?;
        run(
            root,
            "pnpm",
            &["--filter", "@agentx/web", "test", "--maxWorkers=2"],
        )?;
        run(root, "pnpm", &["build:web"])?;
        run(
            root,
            "pnpm",
            &["--filter", "@agentx/e2e", "exec", "tsc", "--noEmit"],
        )?;
        for package in [
            "@agentx/plugin-sdk",
            "@agentx/plugin-ui",
            "@agentx/plugin-runner",
            "@agentx/builtin-data",
            "@agentx/builtin-http",
            "agentx-canvas-plugin-template",
        ] {
            run(root, "pnpm", &["--filter", package, "check"])?;
        }
        for package in ["@agentx/builtin-data", "@agentx/builtin-http"] {
            run(root, "pnpm", &["--filter", package, "build"])?;
        }
        for package in [
            "@agentx/plugin-sdk",
            "@agentx/plugin-runner",
            "@agentx/builtin-data",
            "@agentx/builtin-http",
            "agentx-canvas-plugin-template",
        ] {
            run(root, "pnpm", &["--filter", package, "test"])?;
        }
    }
    run(root, "git", &["diff", "--check"])?;
    println!("Agentx checks passed");
    Ok(())
}

fn line_limits(root: &Path) -> Result<()> {
    fn visit(path: &Path, oversized: &mut Vec<String>) -> Result<()> {
        for entry in std::fs::read_dir(path)? {
            let path = entry?.path();
            if path.is_dir() {
                if matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some(
                        "node_modules"
                            | "dist"
                            | "vendor"
                            | "__pycache__"
                            | ".pytest_cache"
                            | ".ruff_cache"
                    )
                ) {
                    continue;
                }
                visit(&path, oversized)?;
                continue;
            }
            if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("rs" | "ts" | "tsx" | "py" | "mjs" | "js")
            ) {
                let count = std::fs::read_to_string(&path)?.lines().count();
                if count > 2000 {
                    oversized.push(format!("{}={count}", path.display()));
                }
            }
        }
        Ok(())
    }
    let mut oversized = Vec::new();
    for directory in [
        "src/web/src",
        "src/services",
        "src/crates",
        "src/plugins",
        "tests",
        "tools",
    ] {
        visit(&root.join(directory), &mut oversized)?;
    }
    if !oversized.is_empty() {
        bail!("2000-line limit exceeded: {}", oversized.join(", "));
    }
    Ok(())
}

fn images(root: &Path, args: Images) -> Result<()> {
    let values: Value = serde_yaml::from_str(&std::fs::read_to_string(&args.values)?)?;
    if values
        .pointer("/global/environment")
        .and_then(Value::as_str)
        == Some("production")
    {
        bail!("production images must be supplied by immutable digest");
    }
    let known: Vec<String> = values
        .pointer("/global/images/services")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    let explicit_selection = !args.service.is_empty();
    let local_runtime_fixture = values
        .pointer("/global/environment")
        .and_then(Value::as_str)
        == Some("local");
    let mut selected = if !explicit_selection {
        known.clone()
    } else {
        args.service
    };
    // The runtime-provider Kustomize overlay deploys this fixture from the
    // local Docker image, so a full local image build must refresh it too.
    if !explicit_selection && local_runtime_fixture {
        for fixture in ["echo-node", "echo-mcp"] {
            if !selected.iter().any(|service| service == fixture) {
                selected.push(fixture.to_owned());
            }
        }
    }
    for service in &selected {
        if !known.contains(service)
            && !(local_runtime_fixture
                && matches!(
                    service.as_str(),
                    "echo-node" | "echo-mcp" | "cpu-embedding" | "mem0-server" | "lightrag"
                ))
        {
            bail!("unknown image service: {service}");
        }
    }
    let source_hash = String::from_utf8(
        output(
            root,
            "uv",
            &[
                "run",
                "--frozen",
                "--group",
                "test",
                "python",
                "-m",
                "tools.scripts.release.evidence",
                "--print-source-tree-sha",
            ],
            true,
        )?
        .stdout,
    )?;
    let source_commit =
        String::from_utf8(output(root, "git", &["rev-parse", "HEAD"], true)?.stdout)?;
    for service in &selected {
        let image = image_reference(&values, service);
        let mut command = Command::new("docker");
        command
            .current_dir(root)
            .arg("build")
            .args([
                "--label",
                &format!("org.opencontainers.image.revision={}", source_commit.trim()),
            ])
            .args([
                "--label",
                &format!("io.agentx.source-tree-sha256={}", source_hash.trim()),
            ]);
        if service == "web-console" {
            command.args([
                "-f",
                "deploy/docker/web.Dockerfile",
                "--build-arg",
                "NGINX_CONFIG=deploy/docker/nginx-v2.conf",
            ]);
        } else if service == "mem0-server" {
            command.args(["-f", "deploy/docker/mem0.Dockerfile"]);
        } else if service == "lightrag" {
            command.args(["-f", "deploy/docker/lightrag.Dockerfile"]);
        } else if service == "cpu-embedding" {
            command.args(["-f", "deploy/docker/cpu-embedding.Dockerfile"]);
            if let Ok(index) = std::env::var("PIP_INDEX_URL") {
                command.args(["--build-arg", &format!("PIP_INDEX_URL={index}")]);
            }
        } else {
            command
                .args(["-f", "deploy/docker/backend.Dockerfile", "--build-arg"])
                .arg(format!(
                    "APP={}",
                    if service == "observability" {
                        "agentx-observability"
                    } else {
                        service
                    }
                ));
            let package = match service.as_str() {
                "observability" => Some("agentx-observability"),
                "workflow-worker" | "sandbox-manager" => Some("agentx-v2-runtime"),
                "agentx-egress-gateway" => Some("agentx-egress-gateway"),
                "echo-mcp" => Some("echo-mcp"),
                _ => None,
            };
            if let Some(package) = package {
                command.args(["--build-arg", &format!("CARGO_PACKAGE={package}")]);
            }
        }
        command.args(["-t", &image, "."]);
        let mut passed = false;
        for _ in 0..3 {
            if command.status()?.success() {
                passed = true;
                break;
            }
        }
        if !passed {
            bail!("Docker build failed for {service}");
        }
        if args.push {
            run(root, "docker", &["push", &image])?;
        }
    }
    if !args.skip_kubernetes_import {
        import_images(
            root,
            &values,
            &selected
                .iter()
                .map(|service| image_reference(&values, service))
                .collect::<Vec<_>>(),
        )?;
    }
    Ok(())
}

fn import_images(root: &Path, values: &Value, images: &[String]) -> Result<()> {
    let directory = root.join(".local/tmp/images");
    std::fs::create_dir_all(&directory)?;
    let archive = tempfile::Builder::new()
        .prefix("agentx-images-")
        .suffix(".tar")
        .tempfile_in(&directory)?
        .into_temp_path();
    let remote = format!("/tmp/{}", archive.file_name().unwrap().to_string_lossy());
    let loader = format!(
        "agentx-image-loader-{}",
        &Uuid::now_v7().simple().to_string()[..8]
    );
    let archive_text = archive.strip_prefix(root)?.to_string_lossy().into_owned();
    let mut save = Command::new("docker");
    save.current_dir(root)
        .args(["save", "--output", &archive_text])
        .args(images);
    if !save.status()?.success() {
        bail!("docker save failed");
    }
    let desktop = output(
        root,
        "docker",
        &[
            "ps",
            "--filter",
            "name=^/desktop-control-plane$",
            "--format",
            "{{.Names}}",
        ],
        false,
    )?;
    let node = String::from_utf8_lossy(&desktop.stdout).trim().to_owned();
    if !node.is_empty() {
        for image in images {
            let containerd = if image.contains('/') {
                format!("docker.io/{image}")
            } else {
                image.clone()
            };
            let _ = output(
                root,
                "docker",
                &[
                    "exec",
                    &node,
                    "ctr",
                    "--namespace",
                    "k8s.io",
                    "images",
                    "remove",
                    &containerd,
                ],
                false,
            )?;
        }
        let status = Command::new("docker")
            .current_dir(root)
            .args([
                "exec",
                "-i",
                &node,
                "ctr",
                "--namespace",
                "k8s.io",
                "images",
                "import",
                "-",
            ])
            .stdin(std::fs::File::open(&archive)?)
            .status()
            .context("stream images into Kubernetes containerd")?;
        if !status.success() {
            bail!("Kubernetes containerd image import failed");
        }
        tag_digest_images(root, &["docker", "exec", &node], images)
    } else {
        import_with_loader(root, values, &archive_text, &remote, &loader, images)
    }
}

fn import_with_loader(
    root: &Path,
    values: &Value,
    archive: &str,
    remote: &str,
    loader: &str,
    images: &[String],
) -> Result<()> {
    let namespace = values
        .pointer("/global/namespaces/dependencies")
        .and_then(Value::as_str)
        .unwrap();
    let namespace_manifest = serde_json::json!({
        "apiVersion":"v1","kind":"Namespace","metadata":{"name":namespace,"labels":{"agentx.io/plane":"dependencies","agentx.io/ingress":"allowed","app.kubernetes.io/managed-by":"agentxctl"}}
    });
    apply_json(root, &namespace_manifest)?;
    let pod = serde_json::json!({
        "apiVersion":"v1","kind":"Pod","metadata":{"name":loader,"namespace":namespace},"spec":{
            "restartPolicy":"Never","containers":[{"name":"loader","image":"ghcr.io/containerd/nerdctl:v2.3.5","command":["sleep","3600"],
            "securityContext":{"privileged":true},"volumeMounts":[{"name":"containerd","mountPath":"/run/containerd/containerd.sock"}]}],
            "volumes":[{"name":"containerd","hostPath":{"path":"/run/containerd/containerd.sock","type":"Socket"}}]
        }
    });
    let result = (|| -> Result<()> {
        apply_json(root, &pod)?;
        run(
            root,
            "kubectl",
            &[
                "-n",
                namespace,
                "wait",
                "--for=condition=Ready",
                &format!("pod/{loader}"),
                "--timeout=180s",
            ],
        )?;
        run(
            root,
            "kubectl",
            &[
                "-n",
                namespace,
                "cp",
                archive,
                &format!("{loader}:{remote}"),
            ],
        )?;
        run(
            root,
            "kubectl",
            &[
                "-n",
                namespace,
                "exec",
                loader,
                "--",
                "ctr",
                "--address",
                "/run/containerd/containerd.sock",
                "--namespace",
                "k8s.io",
                "images",
                "import",
                remote,
            ],
        )?;
        tag_digest_images(
            root,
            &["kubectl", "-n", namespace, "exec", loader, "--"],
            images,
        )
    })();
    let _ = output(
        root,
        "kubectl",
        &[
            "-n",
            namespace,
            "delete",
            "pod",
            loader,
            "--ignore-not-found",
            "--wait=true",
        ],
        false,
    );
    result
}

fn tag_digest_images(root: &Path, prefix: &[&str], selected: &[String]) -> Result<()> {
    let mut args = prefix[1..].to_vec();
    args.extend(["ctr", "--namespace", "k8s.io", "images", "ls"]);
    let listing = output(root, prefix[0], &args, true)?;
    let listing_text = String::from_utf8_lossy(&listing.stdout);
    let mut tagged = std::collections::HashSet::new();
    for line in listing_text.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3
            || !selected.iter().any(|image| {
                fields[0] == image || fields[0].strip_prefix("docker.io/") == Some(image.as_str())
            })
        {
            continue;
        }
        let repository = fields[0].rsplit_once(':').context("image has no tag")?.0;
        let reference = format!("{repository}@{}", fields[2]);
        let mut tag = prefix[1..].to_vec();
        tag.extend([
            "ctr",
            "--namespace",
            "k8s.io",
            "images",
            "tag",
            "--force",
            fields[0],
            &reference,
        ]);
        run(root, prefix[0], &tag)?;
        tagged.insert(fields[0]);
    }
    for image in selected {
        if !tagged
            .iter()
            .any(|name| *name == image || name.strip_prefix("docker.io/") == Some(image.as_str()))
        {
            bail!("Imported image was not found in Kubernetes containerd: {image}");
        }
    }
    Ok(())
}

fn apply_json(root: &Path, value: &Value) -> Result<()> {
    let mut child = Command::new("kubectl")
        .args(["apply", "-f", "-"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(value)?.as_bytes())?;
    if !child.wait()?.success() {
        bail!("kubectl apply failed");
    }
    Ok(())
}

fn image_reference(values: &Value, service: &str) -> String {
    let registry = values
        .pointer("/global/images/registry")
        .and_then(Value::as_str)
        .unwrap()
        .trim_end_matches('/');
    let prefix = values
        .pointer("/global/images/repositoryPrefix")
        .and_then(Value::as_str)
        .unwrap_or("");
    let repository = if service.starts_with(prefix) {
        service.to_owned()
    } else {
        format!("{prefix}{service}")
    };
    format!(
        "{registry}/{repository}:{}",
        values
            .pointer("/global/images/tag")
            .and_then(Value::as_str)
            .unwrap()
    )
}
