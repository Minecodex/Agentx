"""Session-scoped real CPU embedding provider for local acceptance."""

from __future__ import annotations

import json
import os
import secrets
import socket
import time
from collections.abc import Iterator
from typing import Any

import httpx
import pytest
import yaml

from tests.e2e.support import ROOT, redact, run, start_process

pytestmark = [pytest.mark.cluster, pytest.mark.product]
MODEL = "BAAI/bge-small-zh-v1.5"


@pytest.fixture(scope="session")
def cpu_embedding_service(run_id: str) -> Iterator[dict[str, Any]]:
    directory = ROOT / ".local/artifacts/e2e" / run_id / "cpu-embedding"
    directory.mkdir(parents=True, exist_ok=True)
    image = os.environ.get("AGENTX_E2E_CPU_EMBEDDING_IMAGE", "")
    if "@sha256:" not in image:
        raise RuntimeError("Set AGENTX_E2E_CPU_EMBEDDING_IMAGE to the built CPU embedding image digest")
    receipt = {"image": image}
    namespace = f"agentx-e2e-{run_id}-embedding"
    key = secrets.token_urlsafe(32)
    namespace_manifest = {
        "apiVersion": "v1",
        "kind": "Namespace",
        "metadata": {"name": namespace, "labels": {"agentx.io/plane": "dependencies"}},
    }
    run(("kubectl", "apply", "-f", "-"), input_text=json.dumps(namespace_manifest))
    forward = None
    try:
        manifests = [
            document
            for document in yaml.safe_load_all(
                run(("kubectl", "kustomize", ROOT / "deploy/kustomize/addons/cpu-embedding")).stdout
            )
            if document
        ]
        manifests.append(
            {
                "apiVersion": "v1",
                "kind": "Secret",
                "metadata": {"name": "agentx-embedding-secrets"},
                "stringData": {"API_KEY": key},
            }
        )
        for document in manifests:
            if document["kind"] == "Deployment":
                document["spec"]["template"]["spec"]["containers"][0]["image"] = receipt["image"]
        run(("kubectl", "-n", namespace, "apply", "-f", "-"), input_text=yaml.safe_dump_all(manifests))
        rollout = run(
            ("kubectl", "-n", namespace, "rollout", "status", "deployment/cpu-embedding", "--timeout=300s"),
            timeout=330,
        )
        (directory / "rollout.txt").write_text(rollout.stdout)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        forward = start_process(
            ("kubectl", "-n", namespace, "port-forward", "service/cpu-embedding", f"{port}:7997"),
            stdout_path=directory / "port-forward.log",
            stderr_path=directory / "port-forward-error.log",
        )
        base_url = f"http://127.0.0.1:{port}/v1"
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            if forward.process.poll() is not None:
                raise AssertionError("CPU embedding port-forward exited")
            try:
                response = httpx.get(base_url + "/models", headers={"Authorization": f"Bearer {key}"}, timeout=3)
                if response.status_code == 200:
                    break
            except httpx.HTTPError:
                pass
            time.sleep(0.5)
        else:
            raise AssertionError("CPU embedding model endpoint did not become ready")
        yield {"url": base_url, "apiKey": key, "namespace": namespace, "directory": directory, "receipt": receipt}
    finally:
        if forward is not None:
            forward.stop()
        logs = run(("kubectl", "-n", namespace, "logs", "deployment/cpu-embedding", "--tail=200"), check=False)
        (directory / "provider.log").write_text(redact(logs.stdout + logs.stderr).replace(key, "<redacted>"))
        run(("kubectl", "delete", "namespace", namespace, "--wait=true", "--timeout=120s"), timeout=150)
        (directory / "cleanup.json").write_text(json.dumps({"namespace": namespace, "deleted": True}) + "\n")
