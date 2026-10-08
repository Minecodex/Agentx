from __future__ import annotations

import json
import shutil
import tomllib
from pathlib import Path

import pytest
import yaml

from tests.e2e.support import render, run

TARGETS = ("dependencies", "control", "runtime", "observability")


def test_release_exposes_standalone_binaries_and_direct_download_links() -> None:
    workflow = Path(".github/workflows/agentxctl-release.yml").read_text(encoding="utf-8")
    readme = Path("README.md").read_text(encoding="utf-8")
    version = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    for asset in ("agentxctl-linux-x86_64", "agentxctl-windows-x86_64.exe"):
        assert f"standalone: {asset}" in workflow
        assert f"/agentxctl-v{version}/{asset}" in readme
    assert "python tools/scripts/release/package_agentxctl.py" in workflow
    assert "--version" in workflow and "--target" in workflow
    assert "path: .local/dist" in workflow


def test_backend_builder_copies_every_workspace_member_root() -> None:
    workspace = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]["members"]
    dockerfile = Path("deploy/docker/backend.Dockerfile").read_text(encoding="utf-8")
    copied = {
        line.split()[1].rstrip("/")
        for line in dockerfile.splitlines()
        if line.startswith("COPY ") and len(line.split()) >= 3
    }
    for member in workspace:
        assert any(member == source or member.startswith(f"{source}/") for source in copied), member


def test_chart_schemas_are_identical() -> None:
    canonical = json.loads(Path("deploy/values/values.schema.json").read_text(encoding="utf-8"))
    worker_capabilities = canonical["$defs"]["workerService"]["properties"]["capability"]["enum"]
    assert "remote_action" not in worker_capabilities
    for target in TARGETS:
        schema = json.loads(Path(f"deploy/helm/agentx-{target}/values.schema.json").read_text(encoding="utf-8"))
        assert schema == canonical


@pytest.mark.skipif(shutil.which("helm") is None, reason="Helm is not installed")
@pytest.mark.parametrize(
    "values_name", ["local.yaml", "local-tls.yaml", "dockerhub-beta.yaml", "production.example.yaml"]
)
def test_all_charts_lint_and_render(values_name: str) -> None:
    rendered = "\n".join(render(f"deploy/values/{values_name}", target) for target in TARGETS)
    documents = [document for document in yaml.safe_load_all(rendered) if isinstance(document, dict)]
    values = yaml.safe_load(Path(f"deploy/values/{values_name}").read_text(encoding="utf-8"))
    for document in documents:
        pod_spec = document.get("spec", {})
        if document.get("kind") in {"Deployment", "StatefulSet", "DaemonSet", "Job"}:
            pod_spec = pod_spec.get("template", {}).get("spec", {})
        for container in (*pod_spec.get("initContainers", []), *pod_spec.get("containers", [])):
            assert all(isinstance(argument, str) for argument in container.get("args", [])), (
                document.get("kind"),
                document.get("metadata", {}).get("name"),
                container.get("name"),
            )
    assert "agentx.io/deployment/v2alpha3" not in rendered
    assert "agentx-agentx-" not in rendered
    assert "kind: HorizontalPodAutoscaler" not in rendered
    if values_name in {"local-tls.yaml", "production.example.yaml"}:
        assert "--get-server-public-key" not in rendered
    else:
        assert "--get-server-public-key" in rendered
    if values_name == "local-tls.yaml":
        clickhouse = next(
            document
            for document in documents
            if document.get("kind") == "ConfigMap" and document["metadata"]["name"] == "clickhouse-init"
        )
        assert "<https_port>8123</https_port>" in clickhouse["data"]["tls.xml"]
        assert '<http_port remove="remove" />' in clickhouse["data"]["tls.xml"]
    assert "wait-for-control-schema" in rendered
    assert "wait-for-runtime-schema" in rendered
    assert "wait-for-observability-schema" in rendered
    assert "database=$AGENTX_CLICKHOUSE_DATABASE" in rendered
    for service in ("workflow-worker", "runtime-gateway"):
        deployment = next(
            item for item in documents if item.get("kind") == "Deployment" and item["metadata"]["name"] == service
        )
        pod = deployment["spec"]["template"]["spec"]
        container = pod["containers"][0]
        directory = next(item["value"] for item in container["env"] if item["name"] == "AGENTX_PLUGIN_WORK_DIR")
        mount = next(item for item in container["volumeMounts"] if item["mountPath"] == directory)
        volume = next(item for item in pod["volumes"] if item["name"] == mount["name"])
        assert "emptyDir" in volume
        assert pod["securityContext"]["fsGroup"] == 65532
        assert container["securityContext"]["readOnlyRootFilesystem"] is True
    sandbox_manager = next(
        document
        for document in documents
        if document.get("kind") == "Deployment" and document.get("metadata", {}).get("name") == "sandbox-manager"
    )
    sandbox_env = {item["name"] for item in sandbox_manager["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert {
        "AGENTX_RUNTIME_S3_ENDPOINT",
        "AGENTX_RUNTIME_S3_BUCKET",
        "AGENTX_RUNTIME_S3_ACCESS_KEY",
        "AGENTX_RUNTIME_S3_SECRET_KEY",
    } <= sandbox_env
    assert any(
        document.get("kind") == "NetworkPolicy"
        and document.get("metadata", {}).get("name") == "sandbox-manager-object-storage-egress"
        for document in documents
    )
    gateway_ingress = next(
        document
        for document in documents
        if document.get("kind") == "NetworkPolicy"
        and document.get("metadata", {}).get("name") == "agentx-egress-gateway-ingress"
    )
    sandbox_rule = next(
        rule for rule in gateway_ingress["spec"]["ingress"] if any(port.get("port") == 3129 for port in rule["ports"])
    )
    assert {source["ipBlock"]["cidr"] for source in sandbox_rule["from"]} == set(
        values["global"]["network"]["egressGateway"]["sandboxAccess"]["sourceCidrs"]
    )
    if values_name == "production.example.yaml":
        sandbox_service = next(
            item
            for item in documents
            if item.get("kind") == "Service" and item["metadata"]["name"] == "agentx-egress-sandbox"
        )
        assert (
            sandbox_service["metadata"]["annotations"]
            == values["global"]["network"]["egressGateway"]["sandboxAccess"]["serviceAnnotations"]
        )


@pytest.mark.skipif(shutil.which("helm") is None, reason="Helm is not installed")
def test_metallb_sandbox_service_uses_the_explicit_controller(tmp_path: Path) -> None:
    values = yaml.safe_load(Path("deploy/values/production.example.yaml").read_text(encoding="utf-8"))
    values["global"]["images"]["sourceCommit"] = "0" * 40
    access = values["global"]["network"]["egressGateway"]["sandboxAccess"]
    access.update(
        loadBalancerClass="metallb",
        endpoint="https://172.18.255.241",
        serviceAnnotations={
            "metallb.io/address-pool": "agentx-private",
            "metallb.io/loadBalancerIPs": "172.18.255.241",
        },
    )
    path = tmp_path / "metallb-values.yaml"
    path.write_text(yaml.safe_dump(values), encoding="utf-8")
    documents = list(yaml.safe_load_all(render(str(path), "dependencies")))
    service = next(
        item
        for item in documents
        if item.get("kind") == "Service" and item["metadata"]["name"] == "agentx-egress-sandbox"
    )
    assert service["spec"]["type"] == "LoadBalancer"
    assert service["spec"]["loadBalancerClass"] == "metallb"
    assert service["metadata"]["annotations"] == access["serviceAnnotations"]
    assert service["spec"]["loadBalancerSourceRanges"] == access["sourceCidrs"]


@pytest.mark.skipif(shutil.which("helm") is None, reason="Helm is not installed")
def test_kustomize_does_not_own_core_resources() -> None:
    core_names: set[tuple[str, str]] = set()
    for target in TARGETS:
        for document in yaml.safe_load_all(render("deploy/values/local.yaml", target)):
            if isinstance(document, dict) and document.get("metadata", {}).get("name"):
                core_names.add((document.get("kind", ""), document["metadata"]["name"]))
    kustomize_names: set[tuple[str, str]] = set()
    for directory in (
        "deploy/kustomize/addons/lightrag",
        "deploy/kustomize/addons/mem0",
        "deploy/kustomize/e2e-fixtures/runtime-providers",
    ):
        for document in yaml.safe_load_all(run(("kubectl", "kustomize", directory), timeout=120).stdout):
            if not isinstance(document, dict) or not document.get("metadata", {}).get("name"):
                continue
            kustomize_names.add((document.get("kind", ""), document["metadata"]["name"]))
            labels = document["metadata"].get("labels", {})
            assert labels.get("app.kubernetes.io/managed-by") != "Helm"
    assert core_names.isdisjoint(kustomize_names)
