from __future__ import annotations

import json
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest
import yaml

from tests.e2e import support
from tests.e2e.conftest import _must_remain_available_during_scale_down
from tests.e2e.support import run


@pytest.mark.parametrize("fail_inside", [False, True])
def test_local_sandbox_tls_forward_is_scoped_and_always_stopped(monkeypatch, tmp_path, fail_inside):
    from tests.e2e import conftest as harness

    port = harness._free_port()
    values = tmp_path / "values.yaml"
    values.write_text(
        yaml.safe_dump(
            {
                "global": {
                    "network": {
                        "egressGateway": {
                            "sandboxAccess": {
                                "mode": "nodePort",
                                "endpoint": "https://host.docker.internal:31429",
                            }
                        }
                    }
                }
            }
        )
    )
    service = {
        "kind": "Service",
        "metadata": {"name": "agentx-egress-sandbox", "namespace": "isolated"},
        "spec": {"ports": [{"port": port}]},
    }
    calls = []

    class Forward:
        def __init__(self, command, *, stdout_path, stderr_path, health_url):
            assert health_url == f"https://127.0.0.1:{port}/"
            calls.append(tuple(command))
            stdout_path.write_text(f"Forwarding from 0.0.0.0:{port} -> 3129\n")
            self.process = SimpleNamespace(poll=lambda: None)

        def stop(self):
            calls.append("stopped")

    monkeypatch.setattr(harness, "RestartingPortForward", Forward)
    context = {"dependencies_namespace": "isolated", "artifact_dir": str(tmp_path)}
    try:
        with harness._local_sandbox_proxy(values, yaml.safe_dump(service), context):
            if fail_inside:
                raise RuntimeError("test failed")
    except RuntimeError as error:
        assert fail_inside and str(error) == "test failed"
    assert calls == [
        (
            "kubectl",
            "-n",
            "isolated",
            "port-forward",
            "--address=0.0.0.0",
            "service/agentx-egress-sandbox",
            f"{port}:{port}",
        ),
        "stopped",
    ]


def test_remote_sandbox_gateway_does_not_open_a_local_listener(monkeypatch, tmp_path):
    from tests.e2e import conftest as harness

    values = tmp_path / "values.yaml"
    values.write_text(
        yaml.safe_dump(
            {
                "global": {
                    "network": {
                        "egressGateway": {
                            "sandboxAccess": {
                                "mode": "nodePort",
                                "endpoint": "https://sandbox-node.example.test:31429",
                            }
                        }
                    }
                }
            }
        )
    )
    monkeypatch.setattr(harness, "RestartingPortForward", lambda *_a, **_kw: pytest.fail("unexpected listener"))
    with harness._local_sandbox_proxy(values, "", {}):
        pass


@pytest.mark.parametrize("shared", [False, True])
def test_shared_resource_auth_refreshes_only_live_test_identities(monkeypatch, shared):
    from tests.e2e.product import conftest as product

    token = SimpleNamespace(value="old-access-token")
    provider = SimpleNamespace(headers={"Authorization": "Bearer old-access-token"})
    boundary = SimpleNamespace(headers={"Authorization": "Bearer old-access-token"})
    values = {
        "service_urls": {"web": "http://control.invalid"},
        "live_application": {"token": token},
        "live_providers": {"control": provider},
        "boundary_state": {"token": token, "control": boundary, "mcpKey": "unchanged-provider-secret"},
    }
    requested = []

    def fixture(name):
        requested.append(name)
        return values[name]

    class Auth:
        def __init__(self, url, token):
            assert url == "http://control.invalid"
            self.token = token

        def refresh_if_needed(self):
            self.token.value = "new-access-token"

    monkeypatch.setattr(product, "UserTokenAuth", Auth)
    names = ["live_application", "live_providers", "boundary_state"] if shared else []
    product.refresh_shared_user_credentials.__wrapped__(
        SimpleNamespace(fixturenames=names, getfixturevalue=fixture), values["service_urls"]
    )
    if shared:
        assert token.value == "new-access-token"
        assert boundary.headers["Authorization"] == "Bearer new-access-token"
        assert requested == ["live_application", "boundary_state"]
    else:
        assert token.value == "old-access-token" and requested == []
    # Provider-specific authentication remains owned by its HTTP Auth handler.
    assert provider.headers["Authorization"] == "Bearer old-access-token"
    assert values["boundary_state"]["mcpKey"] == "unchanged-provider-secret"


@pytest.mark.parametrize("scale_fails", [False, True])
def test_development_restore_preserves_running_pod_images_and_rolls_back_scale_failure(monkeypatch, scale_fails):
    from tests.e2e import conftest as harness

    configured = "docker.io/acme/provider@sha256:" + "a" * 64
    running = configured.removeprefix("docker.io/")
    deployment = {
        "metadata": {"name": "provider"},
        "spec": {
            "replicas": 1,
            "selector": {"matchLabels": {"app": "provider"}},
            "template": {"spec": {"containers": [{"name": "provider", "image": configured}]}},
        },
    }
    state = {"image": running, "replicas": 1}
    commands = []

    def fake_run(command, **_kwargs):
        command = tuple(str(part) for part in command)
        commands.append(command)
        payload = {}
        if command[3:5] == ("get", "deployment"):
            payload = {"items": [deployment]}
        elif command[3:5] == ("get", "pods"):
            payload = {
                "items": [
                    {
                        "metadata": {"name": "provider-pod"},
                        "spec": {"containers": [{"name": "provider", "image": state["image"]}]},
                    }
                ]
            }
        elif command[3] == "scale":
            state["replicas"] = int(command[-1].split("=")[1])
            state["image"] = configured
            if state["replicas"] == 0 and scale_fails:
                raise RuntimeError("scale transport failed")
        elif command[3:5] == ("patch", "pod"):
            patch = json.loads(command[-1])
            assert patch[0] == {"op": "test", "path": "/spec/containers/0/image", "value": configured}
            state["image"] = patch[1]["value"]
        elif command[3:5] == ("rollout", "status"):
            assert state == {"image": running, "replicas": 1}
        return SimpleNamespace(returncode=0, json=lambda: payload)

    monkeypatch.setattr(harness, "run", fake_run)
    monkeypatch.setattr(harness, "deployment_config", lambda _values: {"namespaces": {"control": "source"}})
    if scale_fails:
        with pytest.raises(RuntimeError, match="scale transport failed"):
            harness._scale_development(Path("fixture-values"), True)
    else:
        snapshot = harness._scale_development(Path("fixture-values"), True)
        harness._restore_development(snapshot)
    assert state == {"image": running, "replicas": 1}
    assert deployment["spec"]["template"]["spec"]["containers"][0]["image"] == configured
    assert any(command[3:5] == ("patch", "pod") for command in commands)


@pytest.mark.parametrize("scale_error", [False, True])
def test_capacity_storage_abort_interrupts_load_even_if_scaling_fails(monkeypatch, scale_error):
    from tests.e2e.capacity import collector as module

    collector = module.CapacityCollector.__new__(module.CapacityCollector)
    collector.context = {"runtime_namespace": "agentx-e2e-runtime-storage", "run_id": "storage"}
    collector.stop_event = threading.Event()
    commands = []

    def capture(command, **kwargs):
        commands.append(command)
        if scale_error and "scale" in command:
            raise RuntimeError("scale unavailable")

    monkeypatch.setattr(module, "run", capture)
    if scale_error:
        with pytest.raises(RuntimeError, match="scale unavailable"):
            collector.abort_low_storage(1024)
    else:
        collector.abort_low_storage(1024)
    assert collector.stop_event.is_set()
    assert collector.storage_abort["availableBytes"] == 1024
    assert all(command[2] == "agentx-e2e-runtime-storage" for command in commands)
    assert "delete" in commands[-1] and "pod/capacity-load" in commands[-1]


def test_capacity_storage_abort_never_mutates_production(monkeypatch):
    from tests.e2e.capacity import collector as module

    collector = module.CapacityCollector.__new__(module.CapacityCollector)
    collector.context = {"runtime_namespace": "agentx-prod-runtime", "run_id": "storage"}
    commands = []
    monkeypatch.setattr(module, "run", lambda command, **kwargs: commands.append(command))
    with pytest.raises(ValueError, match="isolated E2E namespace"):
        collector.abort_low_storage(1024)
    assert commands == []


@pytest.mark.parametrize("unread", [0, 2001])
def test_capacity_redis_lag_uses_atomic_unread_snapshot_even_when_native_lag_is_unknown(monkeypatch, tmp_path, unread):
    from tests.e2e.capacity import collector as module

    collector = module.CapacityCollector.__new__(module.CapacityCollector)
    collector.context = {}
    collector.directory = tmp_path
    responses = {
        ("INFO", "memory"): "used_memory:512\r\n",
        ("SCAN", "0", "COUNT", "1000", "TYPE", "stream"): ["0", ["tasks"]],
        ("EVAL", module.UNREAD_STREAM_SNAPSHOT, "1", "tasks"): [["workers", 4, unread, "100-0", None]],
    }
    monkeypatch.setattr(module, "redis_command", lambda _context, *args: responses[args])
    assert collector.redis_state() == {"usedMemoryBytes": 512, "pending": 4, "consumerLag": unread}
    assert f'"unread": {unread}' in (tmp_path / "redis-unread-snapshots.jsonl").read_text()


def test_scale_down_keeps_the_ingress_admission_controller_available() -> None:
    ingress = {
        "metadata": {
            "labels": {
                "app.kubernetes.io/name": "ingress-nginx",
                "app.kubernetes.io/component": "controller",
            }
        }
    }
    application = {
        "metadata": {
            "labels": {
                "app.kubernetes.io/name": "platform-control",
                "app.kubernetes.io/component": "api",
            }
        }
    }

    assert _must_remain_available_during_scale_down(ingress)
    assert not _must_remain_available_during_scale_down(application)


@pytest.mark.parametrize("platform", ["nt", "posix"])
def test_playwright_harness_uses_the_windows_executable_shim_and_new_stage_name(
    platform: str, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    calls = []

    def capture(command: list[str], **kwargs: object) -> None:
        calls.append((command, kwargs))

    monkeypatch.setattr(support, "os", SimpleNamespace(name=platform))
    monkeypatch.setattr(support.subprocess, "run", capture)
    tests = ["tests/v2-08-api-first.spec.ts", "tests/m2.1-control-plane.spec.ts", "tests/m6-workflow-studio.spec.ts"]
    environment = {
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_RUN_ID": "harness-contract",
        "AGENTX_V2_08_CONTEXT_OUTPUT": str(tmp_path / "context.json"),
    }
    support.run_playwright(tmp_path, "product", tests, environment)

    assert len(calls) == 1
    command, kwargs = calls[0]
    executable = "corepack.cmd" if platform == "nt" else "corepack"
    assert command == [executable, "pnpm", "--filter", "@agentx/e2e", "exec", "playwright", "test", *tests]
    assert kwargs["shell"] is False
    assert kwargs["check"] is True
    assert kwargs["cwd"] == str(tmp_path)
    assert kwargs["env"] == {**environment, "AGENTX_E2E_SUITE": "product"}


def test_control_plane_selects_its_scoped_echo_mcp_fixture() -> None:
    source = Path("tests/browser/tests/m2.1-control-plane.spec.ts").read_text(encoding="utf-8")

    assert "'echo', 'MCP 工具', 'workflow', '使用', 'Echo MCP / echo'" in source
    assert "name: /^echo Echo MCP \\/ echo$/" in source
    assert "name: /echo/" not in source
    assert "await expect(source).toBeInViewport()" in source
    assert "await expect(target).toBeInViewport()" in source
    assert "const toggle = row.locator('[data-tree-toggle]')" in source


def test_lightrag_tokenizer_cache_is_pinned_and_separate_from_the_runtime_pod() -> None:
    rendered = run(("kubectl", "kustomize", "deploy/kustomize/e2e-fixtures/runtime-providers"), timeout=120).stdout
    resources = [document for document in yaml.safe_load_all(rendered) if isinstance(document, dict)]
    deployment = next(
        resource
        for resource in resources
        if resource.get("kind") == "Deployment" and resource.get("metadata", {}).get("name") == "lightrag"
    )
    cache_job = next(
        resource
        for resource in resources
        if resource.get("kind") == "Job" and resource.get("metadata", {}).get("name") == "lightrag-tokenizer-cache"
    )
    policy = next(
        resource
        for resource in resources
        if resource.get("kind") == "NetworkPolicy"
        and resource.get("metadata", {}).get("name") == "lightrag-tokenizer-cache-egress"
    )

    pod_spec = deployment["spec"]["template"]["spec"]
    assert pod_spec["initContainers"][0]["name"] == "wait-for-tokenizer-cache"
    assert {item["name"]: item["value"] for item in pod_spec["containers"][0]["env"]}["TIKTOKEN_CACHE_DIR"] == (
        "/app/data/tiktoken-cache"
    )
    container = cache_job["spec"]["template"]["spec"]["containers"][0]
    assert container["image"] == "agentx/lightrag:dev"
    copy = container["args"][0]
    assert "/opt/tiktoken-cache/fb374d419588a4632f3f557e76b4b70aebbca790" in copy
    assert "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d" in copy
    assert "curl" not in copy
    assert cache_job["spec"]["backoffLimit"] == 5
    assert policy["spec"]["podSelector"]["matchLabels"]["app.kubernetes.io/name"] == "lightrag-tokenizer-cache"


def test_local_image_build_includes_the_runtime_node_fixture() -> None:
    xtask = Path("tools/xtask/src/main.rs").read_text(encoding="utf-8")
    assert '== Some("local")' in xtask
    assert 'for fixture in ["echo-node", "echo-mcp", "mem0-server"]' in xtask
    assert '"deploy/docker/mem0.Dockerfile"' in xtask
    assert "selected.push(fixture.to_owned())" in xtask

    rendered = run(("kubectl", "kustomize", "deploy/kustomize/e2e-fixtures/runtime-providers"), timeout=120).stdout
    resources = [document for document in yaml.safe_load_all(rendered) if isinstance(document, dict)]
    deployment = next(
        resource
        for resource in resources
        if resource.get("kind") == "Deployment" and resource.get("metadata", {}).get("name") == "echo-node"
    )
    container = deployment["spec"]["template"]["spec"]["containers"][0]
    assert container["image"] == "agentx/echo-node:dev"
