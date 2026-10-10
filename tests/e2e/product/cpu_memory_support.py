"""Mem0 and pgvector with real CPU vectors; infer=False does not call a cloud LLM."""

from __future__ import annotations

import os
import secrets
import socket
import time
from collections.abc import Iterator
from typing import Any

import httpx
import pytest
import yaml

from tests.e2e.product import cpu_embedding_support as embedding
from tests.e2e.support import ROOT, redact, run, start_process

pytestmark = [pytest.mark.cluster, pytest.mark.product]


@pytest.fixture(scope="session")
def cpu_memory_service(cpu_embedding_service: dict[str, Any]) -> Iterator[dict[str, Any]]:
    namespace = cpu_embedding_service["namespace"]
    directory = cpu_embedding_service["directory"] / "mem0"
    directory.mkdir(parents=True, exist_ok=True)
    model_url = f"http://cpu-embedding.{namespace}.svc:7997/v1"
    api_key = secrets.token_urlsafe(32)
    documents = list(yaml.safe_load_all(run(("kubectl", "kustomize", ROOT / "deploy/kustomize/addons/mem0")).stdout))
    documents.extend(
        [
            {
                "apiVersion": "networking.k8s.io/v1",
                "kind": "NetworkPolicy",
                "metadata": {"name": "cpu-memory-dependencies"},
                "spec": {
                    "podSelector": {"matchLabels": {"app.kubernetes.io/name": "mem0"}},
                    "policyTypes": ["Egress"],
                    "egress": [
                        {
                            "to": [{"podSelector": {"matchLabels": {"app.kubernetes.io/name": "mem0-postgres"}}}],
                            "ports": [{"protocol": "TCP", "port": 5432}],
                        },
                        {
                            "to": [{"namespaceSelector": {}, "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}}}],
                            "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}],
                        },
                    ],
                },
            },
            {
                "apiVersion": "v1",
                "kind": "ConfigMap",
                "metadata": {"name": "agentx-mem0-config"},
                "data": {
                    "AUTH_DISABLED": "false",
                    "MEM0_TELEMETRY": "false",
                    "APP_DB_NAME": "mem0_app",
                    "HISTORY_DB_PATH": "/app/history/history.db",
                    "OPENAI_BASE_URL": model_url,
                    "MEM0_DEFAULT_LLM_MODEL": "text-llm-not-configured",
                    "MEM0_DEFAULT_EMBEDDER_MODEL": embedding.MODEL,
                    "POSTGRES_HOST": "mem0-postgres",
                    "POSTGRES_PORT": "5432",
                    "POSTGRES_USER": "postgres",
                    "POSTGRES_DB": "postgres",
                    "POSTGRES_COLLECTION_NAME": "memories",
                },
            },
            {
                "apiVersion": "v1",
                "kind": "Secret",
                "metadata": {"name": "agentx-mem0-secrets"},
                "stringData": {
                    "JWT_SECRET": secrets.token_urlsafe(48),
                    "ADMIN_API_KEY": api_key,
                    "OPENAI_API_KEY": cpu_embedding_service["apiKey"],
                    "POSTGRES_PASSWORD": secrets.token_urlsafe(32),
                },
            },
        ]
    )
    for document in documents:
        if document["kind"] == "Deployment":
            document["spec"]["template"]["metadata"].setdefault("labels", {})["agentx.io/runtime-provider"] = "allowed"
            if document["metadata"]["name"] == "mem0" and os.environ.get("AGENTX_E2E_MEM0_IMAGE"):
                pod = document["spec"]["template"]["spec"]
                for container in (*pod.get("initContainers", []), *pod["containers"]):
                    container["image"] = os.environ["AGENTX_E2E_MEM0_IMAGE"]
    run(("kubectl", "-n", namespace, "apply", "-f", "-"), input_text=yaml.safe_dump_all(documents))
    forward = None
    try:
        for name in ("mem0-postgres", "mem0"):
            run(("kubectl", "-n", namespace, "rollout", "status", f"deployment/{name}", "--timeout=300s"), timeout=330)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        forward = start_process(
            ("kubectl", "-n", namespace, "port-forward", "service/mem0", f"{port}:8000"),
            stdout_path=directory / "port-forward.log",
            stderr_path=directory / "port-forward-error.log",
        )
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            try:
                if httpx.get(f"http://127.0.0.1:{port}/openapi.json", timeout=3).status_code == 200:
                    break
            except httpx.HTTPError:
                pass
            time.sleep(0.5)
        else:
            raise AssertionError("Mem0 endpoint did not become ready")
        with httpx.Client(base_url=f"http://127.0.0.1:{port}", headers={"X-API-Key": api_key}, timeout=60) as client:
            configured = client.post(
                "/configure",
                json={
                    "embedder": {
                        "provider": "openai",
                        "config": {
                            "model": embedding.MODEL,
                            "api_key": cpu_embedding_service["apiKey"],
                            "openai_base_url": model_url,
                            "embedding_dims": 512,
                        },
                    },
                    "vector_store": {
                        "provider": "pgvector",
                        "config": {
                            "embedding_model_dims": 512,
                            "collection_name": "memories_bge_512",
                        },
                    },
                },
            )
            assert configured.status_code == 200, configured.text
            yield {"client": client, "namespace": namespace, "directory": directory}
    finally:
        if forward is not None:
            forward.stop()
        logs = run(("kubectl", "-n", namespace, "logs", "deployment/mem0", "--tail=200"), check=False)
        (directory / "provider.log").write_text(redact(logs.stdout + logs.stderr).replace(api_key, "<redacted>"))
