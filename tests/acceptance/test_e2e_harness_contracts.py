from __future__ import annotations

import threading
from pathlib import Path

import pytest
import yaml

from tests.e2e.conftest import _must_remain_available_during_scale_down
from tests.e2e.support import run


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


def test_playwright_harness_uses_the_windows_executable_shim_and_new_stage_name() -> None:
    source = Path("tests/e2e/product/test_playwright.py").read_text(encoding="utf-8")
    harness = Path("tests/e2e/support.py").read_text(encoding="utf-8")

    assert '("corepack.cmd", "pnpm") if os.name == "nt" else ("corepack", "pnpm")' in harness
    assert 'environment["AGENTX_E2E_STAGE"] = "helm-agentxctl"' in source
    assert 'environment["AGENTX_V2_08_CONTEXT_OUTPUT"]' in source
    assert '"tests/v2-08-api-first.spec.ts"' in source
    assert '"tests/m2.1-control-plane.spec.ts"' in source
    assert '"tests/m6-workflow-studio.spec.ts"' in source
    assert '[*pnpm, "--filter", "@agentx/e2e", "exec", "playwright", "test", *tests, *snapshot_args]' in harness
    assert '[pnpm, "--filter", "@agentx/e2e", "test"]' not in harness


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
    assert 'for fixture in ["echo-node", "echo-mcp"]' in xtask
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
