from __future__ import annotations

import copy
import hashlib
from pathlib import Path

import pytest
import yaml

from tests.e2e.capacity.baseline import baseline_matches
from tests.e2e.hosted_cluster import CPU_COUNT, MEMORY_MIB, PROFILE, hosted_values
from tests.e2e.product.live_text_support import live_kimi_secret


@pytest.mark.parametrize("tampered", [False, True])
def test_minikube_download_must_match_the_pinned_release_digest(monkeypatch, tmp_path, tampered):
    from types import SimpleNamespace

    from tests.e2e import hosted_cluster as module

    trusted = b"isolated-minikube-binary-fixture"
    monkeypatch.setattr(module, "ROOT", tmp_path)
    monkeypatch.setattr(module, "MINIKUBE_SHA256", hashlib.sha256(trusted).hexdigest())
    monkeypatch.setattr(module.platform, "system", lambda: "Linux")
    monkeypatch.setattr(module.platform, "machine", lambda: "x86_64")
    monkeypatch.setenv("PATH", "isolated-test-path")

    class Client:
        def __init__(self, **kwargs):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

        def get(self, url):
            assert "/v1.39.0/minikube-linux-amd64" in url
            return SimpleNamespace(content=b"tampered" if tampered else trusted, raise_for_status=lambda: None)

    monkeypatch.setattr(module.httpx, "Client", Client)
    binary = tmp_path / ".local/tools/minikube"
    if tampered:
        with pytest.raises(ValueError, match="checksum mismatch"):
            module.install_minikube()
        assert not binary.exists()
    else:
        module.install_minikube()
        assert binary.read_bytes() == trusted


def test_hosted_values_keep_the_candidate_images_and_limit_private_network_access():
    source = yaml.safe_load(Path("deploy/values/dockerhub-beta.yaml").read_text())
    snapshot = copy.deepcopy(source)
    values = hosted_values(source, gateway="192.168.49.1", subnet="192.168.49.0/24")
    assert source == snapshot
    assert values["global"]["images"] == source["global"]["images"]
    assert values["global"]["ingress"]["serviceType"] == "NodePort"
    assert values["global"]["components"]["sandbox"]["endpoint"] == "http://192.168.49.1:18080"
    access = values["global"]["network"]["egressGateway"]["sandboxAccess"]
    assert access["endpoint"] == f"https://{PROFILE}:31129"
    assert access["sourceCidrs"] == ["192.168.49.0/24"]
    assert values["global"]["network"]["externalEgress"]["opensandbox"] == {
        "cidrs": ["192.168.49.1/32"],
        "ports": [18080],
    }


def baseline_fixture():
    nodes = [
        {
            "status": {
                "capacity": {"cpu": str(CPU_COUNT), "memory": f"{MEMORY_MIB * 1024}Ki"},
                "nodeInfo": {"architecture": "amd64", "kubeletVersion": "v1.36.1"},
            }
        }
    ]
    environment = {
        "cpuCount": CPU_COUNT,
        "memoryMiB": MEMORY_MIB,
        "nodeResources": {"NanoCpus": CPU_COUNT * 10**9, "Memory": MEMORY_MIB * 1024**2},
    }
    return nodes, environment


def test_hosted_capacity_requires_the_frozen_quota_and_visible_hardware():
    nodes, environment = baseline_fixture()
    assert baseline_matches(nodes, hosted=True, environment=environment)
    assert not baseline_matches(nodes, hosted=True)
    assert not baseline_matches([*nodes, nodes[0]], hosted=True, environment=environment)
    # Kubelet can report host memory, but the explicit container limit remains mandatory.
    nodes[0]["status"]["capacity"]["memory"] = f"{16 * 1024**2}Ki"
    assert baseline_matches(nodes, hosted=True, environment=environment)
    environment["nodeResources"]["Memory"] = 16 * 1024**3
    assert not baseline_matches(nodes, hosted=True, environment=environment)


@pytest.mark.parametrize(
    ("section", "field", "value"),
    [
        ("capacity", "cpu", "8"),
        ("capacity", "memory", "1024000Ki"),
        ("nodeInfo", "architecture", "arm64"),
        ("nodeInfo", "kubeletVersion", "v1.37.0"),
    ],
)
def test_changed_hosted_hardware_cannot_pass_the_frozen_capacity_gate(section, field, value):
    nodes, environment = baseline_fixture()
    nodes[0]["status"][section][field] = value
    assert not baseline_matches(nodes, hosted=True, environment=environment)


def test_ci_model_credential_is_required_hidden_and_cleared(monkeypatch):
    monkeypatch.delenv("AGENTX_E2E_KIMI_API_KEY", raising=False)
    with pytest.raises(RuntimeError, match="AGENTX_E2E_KIMI_API_KEY"):
        next(live_kimi_secret.__wrapped__("missing"))
    monkeypatch.setenv("AGENTX_E2E_KIMI_API_KEY", "isolated-fixture-key")
    generator = live_kimi_secret.__wrapped__("hosted")
    secret = next(generator)
    assert secret.value == "isolated-fixture-key"
    assert "isolated-fixture-key" not in repr(secret)
    generator.close()
    assert secret.value == ""


def test_hosted_cluster_is_deleted_even_when_startup_and_diagnostics_fail(monkeypatch, tmp_path):
    from tests.e2e import hosted_cluster as module
    from tests.e2e.support import Result

    commands = []
    monkeypatch.setattr(module, "install_minikube", lambda: None)

    def command(args, **kwargs):
        commands.append(args)
        if args[:2] == ("minikube", "start"):
            raise RuntimeError("cluster startup failed")
        if args[0] == "kubectl":
            raise RuntimeError("diagnostic timeout")
        return Result(tuple(args), "", "", 0)

    monkeypatch.setattr(module, "run", command)
    with pytest.raises(RuntimeError, match="cluster startup failed"):
        next(module.minikube_environment(tmp_path / "values.yaml", tmp_path))
    assert ("minikube", "delete", "-p", PROFILE) in commands
    assert ("docker", "network", "rm", PROFILE) in commands


def test_hosted_cluster_does_not_delete_a_preexisting_network(monkeypatch, tmp_path):
    from tests.e2e import hosted_cluster as module
    from tests.e2e.support import Result

    commands = []
    monkeypatch.setattr(module, "install_minikube", lambda: None)

    def command(args, **kwargs):
        commands.append(args)
        if args[:3] == ("docker", "network", "create"):
            raise RuntimeError("network already exists")
        return Result(tuple(args), "", "", 0)

    monkeypatch.setattr(module, "run", command)
    with pytest.raises(RuntimeError, match="network already exists"):
        next(module.minikube_environment(tmp_path / "values.yaml", tmp_path))
    assert all("delete" not in args and "rm" not in args for args in commands)


def test_hosted_cluster_enforces_the_measured_cpu_limit(monkeypatch, tmp_path):
    import json

    from tests.e2e import hosted_cluster as module
    from tests.e2e.support import Result

    nodes, environment = baseline_fixture()
    resources = environment["nodeResources"]
    resources["NanoCpus"] = 0  # Minikube's observed default on a four-CPU host.
    values = tmp_path / "source.yaml"
    values.write_text(Path("deploy/values/local.yaml").read_text())
    monkeypatch.setattr(module, "install_minikube", lambda: None)
    monkeypatch.setattr(module, "prepare_images", lambda values: None)

    def command(args, **kwargs):
        payload = ""
        if args[:2] == ("docker", "update"):
            assert args == ("docker", "update", f"--cpus={CPU_COUNT}", PROFILE)
            resources["NanoCpus"] = CPU_COUNT * 10**9
        elif args[:3] == ("docker", "network", "inspect"):
            payload = json.dumps([{"IPAM": {"Config": [{"Subnet": "192.168.49.0/24", "Gateway": "192.168.49.1"}]}}])
        elif args == ("minikube", "-p", PROFILE, "ip"):
            payload = "192.168.49.2\n"
        elif args[:3] == ("kubectl", "get", "nodes") and "json" in args:
            payload = json.dumps({"items": nodes})
        elif args[:2] == ("docker", "inspect"):
            payload = json.dumps([{"HostConfig": resources}])
        return Result(tuple(args), payload, "", 0)

    monkeypatch.setattr(module, "run", command)
    directory = tmp_path / "artifacts"
    generator = module.minikube_environment(values, directory)
    try:
        next(generator)
        observed = json.loads((directory / "hosted-environment.json").read_text())
        assert baseline_matches(observed["nodes"], hosted=True, environment=observed)
    finally:
        generator.close()


def test_hosted_image_builder_includes_every_local_provider_fixture(monkeypatch):
    from tests.e2e import hosted_cluster as module
    from tests.e2e.support import Result

    fixture_files = [
        "deploy/kustomize/e2e-fixtures/runtime-providers/providers.yaml",
        "deploy/kustomize/e2e-fixtures/runtime-providers/lightrag-tokenizer-cache.yaml",
        "deploy/kustomize/addons/cpu-embedding/resources.yaml",
        "deploy/kustomize/addons/mem0/resources.yaml",
    ]
    required = set()
    for path in fixture_files:
        for document in yaml.safe_load_all(Path(path).read_text()):
            if document and document["kind"] in {"Deployment", "Job"}:
                pod = document["spec"]["template"]["spec"]
                for container in [*pod.get("initContainers", []), *pod.get("containers", [])]:
                    image = container["image"]
                    if image.startswith("agentx/"):
                        required.add(image.split("/", 1)[1].split(":", 1)[0])

    commands = []
    monkeypatch.setenv("AGENTX_E2E_CPU_EMBEDDING_IMAGE", "isolated-fixture")
    monkeypatch.setenv("AGENTX_E2E_MEM0_IMAGE", "isolated-fixture")
    monkeypatch.setattr(module, "node_image", lambda image: f"{image.split(':')[0]}@sha256:{'a' * 64}")

    def command(args, **kwargs):
        commands.append(args)
        return Result(tuple(args), "", "", 0)

    monkeypatch.setattr(module, "run", command)
    module.prepare_images(yaml.safe_load(Path("deploy/values/local.yaml").read_text()))
    invocation = commands[0]
    selected = {invocation[index + 1] for index, argument in enumerate(invocation) if argument == "--service"}
    assert required <= selected, f"fixture images omitted from the build: {required - selected}"


def test_linux_cluster_e2e_is_mandatory_and_release_keeps_full_certification():
    quality = yaml.safe_load(Path(".github/workflows/quality.yml").read_text())["jobs"]
    assert "kubernetes-e2e-windows" not in quality
    linux = quality["kubernetes-e2e-linux"]
    assert linux["runs-on"] == "ubuntu-24.04" and "if" not in linux
    assert "kubernetes-e2e-linux" in quality["ci-gate"]["needs"]
    matrix = quality["python-helm"]["strategy"]["matrix"]["os"]
    assert "windows-latest" in matrix
    release = yaml.safe_load(Path(".github/workflows/agentxctl-release.yml").read_text())["jobs"]
    certify = release["certify"]
    assert certify["runs-on"] == "ubuntu-24.04" and "if" not in certify
    suite = next(step["run"] for step in certify["steps"] if "pytest tests/e2e" in step.get("run", ""))
    assert "--capacity-smoke" not in suite
    assert "--minikube" in suite and "--previous-worker-image" in suite
    assert "certify" in release["publish"]["needs"]
