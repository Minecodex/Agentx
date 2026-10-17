"""LightRAG and Mem0 using the configured real LLM and shared CPU embeddings."""

from __future__ import annotations

import json
import secrets
from collections.abc import Iterator

import pytest
import yaml

from tests.e2e.product.live_text_support import Secret, post
from tests.e2e.product.user_auth import UserTokenAuth
from tests.e2e.support import ROOT, redact, run


@pytest.fixture(scope="session")
def live_providers(
    cpu_embedding_service,
    cpu_memory_service,
    existing_model,
    existing_model_secret,
    live_application,
    service_urls,
    installed_agentx,
    run_id,
) -> Iterator[dict]:
    namespace = cpu_embedding_service["namespace"]
    app = live_application
    directory = cpu_embedding_service["directory"] / "live-providers"
    directory.mkdir(parents=True, exist_ok=True)
    key = secrets.token_urlsafe(32)
    model_url = f"http://cpu-embedding.{namespace}.svc:7997/v1"
    rendered = run(("kubectl", "kustomize", ROOT / "deploy/kustomize/e2e-fixtures/runtime-providers")).stdout
    documents = [
        d
        for d in yaml.safe_load_all(rendered)
        if d
        and d["metadata"]["name"]
        in {
            "lightrag",
            "lightrag-data",
            "lightrag-tokenizer-cache",
            "lightrag-tokenizer-cache-egress",
            "agentx-lightrag-config",
            "agentx-lightrag-secrets",
        }
    ]
    for document in documents:
        name = document["metadata"]["name"]
        if name == "agentx-lightrag-config":
            document["data"].update(
                {
                    "LLM_BINDING_HOST": existing_model["endpoint"],
                    "LLM_MODEL": existing_model["modelName"],
                    "OPENAI_LLM_EXTRA_BODY": json.dumps({**existing_model["defaultParameters"], "stream": False}),
                    "EMBEDDING_BINDING_HOST": model_url,
                    "EMBEDDING_MODEL": "BAAI/bge-small-zh-v1.5",
                    "EMBEDDING_DIM": "512",
                    "MAX_ASYNC": "2",
                    "EMBEDDING_FUNC_MAX_ASYNC": "2",
                }
            )
        elif name == "agentx-lightrag-secrets":
            document.pop("data", None)
            document["stringData"] = {
                "LIGHTRAG_API_KEY": key,
                "LLM_BINDING_API_KEY": existing_model_secret.value,
                "EMBEDDING_BINDING_API_KEY": cpu_embedding_service["apiKey"],
            }
    documents.append(
        {
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {"name": "live-provider-dependencies"},
            "spec": {
                "podSelector": {
                    "matchExpressions": [
                        {"key": "app.kubernetes.io/name", "operator": "In", "values": ["lightrag", "mem0"]}
                    ]
                },
                "policyTypes": ["Egress"],
                "egress": [
                    {
                        "to": [{"namespaceSelector": {}, "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}}}],
                        "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}],
                    },
                    {
                        "to": [
                            {
                                "ipBlock": {
                                    "cidr": "0.0.0.0/0",
                                    "except": [
                                        "0.0.0.0/8",
                                        "10.0.0.0/8",
                                        "127.0.0.0/8",
                                        "169.254.0.0/16",
                                        "172.16.0.0/12",
                                        "192.168.0.0/16",
                                        "198.18.0.0/15",
                                        "224.0.0.0/4",
                                        "240.0.0.0/4",
                                    ],
                                }
                            }
                        ],
                        "ports": [{"protocol": "TCP", "port": 443}],
                    },
                ],
            },
        }
    )
    run(("kubectl", "-n", namespace, "apply", "-f", "-"), input_text=yaml.safe_dump_all(documents))
    try:
        run(
            (
                "kubectl",
                "-n",
                namespace,
                "wait",
                "--for=condition=complete",
                "job/lightrag-tokenizer-cache",
                "--timeout=300s",
            ),
            timeout=330,
        )
        run(("kubectl", "-n", namespace, "rollout", "status", "deployment/lightrag", "--timeout=600s"), timeout=630)
        memory = cpu_memory_service["client"]
        configured = memory.post(
            "/configure",
            json={
                "llm": {
                    "provider": "openai_structured",
                    "config": {
                        "model": existing_model["modelName"],
                        "api_key": existing_model_secret.value,
                        "openai_base_url": existing_model["endpoint"],
                        "temperature": 1,
                        "top_p": 0.95,
                        # Mem0 defaults to 2,000 tokens, which can be exhausted
                        # by reasoning before the extracted JSON is emitted.
                        "max_tokens": min(8192, existing_model["maxOutputTokens"]),
                        "reasoning_effort": existing_model["defaultParameters"].get("reasoning_effort"),
                    },
                },
                "embedder": {
                    "provider": "openai",
                    "config": {
                        "model": "BAAI/bge-small-zh-v1.5",
                        "api_key": cpu_embedding_service["apiKey"],
                        "openai_base_url": model_url,
                        "embedding_dims": 512,
                    },
                },
                "vector_store": {
                    "provider": "pgvector",
                    "config": {"embedding_model_dims": 512, "collection_name": "memories_bge_512"},
                },
            },
        )
        assert configured.status_code == 200, configured.text.replace(existing_model_secret.value, "<redacted>")
        registered = memory.post(
            "/auth/register",
            json={"name": "P7 Test Admin", "email": "p7-admin@example.com", "password": secrets.token_urlsafe(24)},
        )
        assert registered.status_code == 200, registered.text
        issued_key = memory.post(
            "/api-keys",
            headers={"Authorization": f"Bearer {registered.json()['access_token']}"},
            json={"label": f"Agentx integration {run_id}"},
        )
        assert issued_key.status_code == 201, "Mem0 integration API key creation failed"
        memory_key = Secret(issued_key.json()["key"])
        import httpx

        with httpx.Client(
            base_url=service_urls["web"], auth=UserTokenAuth(service_urls["web"], app["token"]), timeout=60
        ) as control:
            rag_credential = post(
                control,
                "/credentials",
                {
                    "name": f"P7 LightRAG {run_id}",
                    "credentialType": "bearer",
                    "secret": key,
                    "ownerDepartmentId": app["departmentId"],
                },
            )
            rag_connection = post(
                control,
                "/knowledge/connections",
                {
                    "name": f"P7 live LightRAG {run_id}",
                    "provider": "lightrag",
                    "endpoint": f"http://lightrag.{namespace}.svc:9621",
                    "healthPath": "/health",
                    "credentialId": rag_credential["id"],
                    "ownerDepartmentId": app["departmentId"],
                    "configuration": {},
                },
            )
            rag = post(
                control,
                f"/knowledge/connections/{rag_connection['id']}/queries",
                {
                    "name": f"P7 live knowledge {run_id}",
                    "description": "Live externally maintained project documentation",
                    "scope": control.get(f"/api/v1/knowledge/connections/{rag_connection['id']}/catalog").json()[0][
                        "scope"
                    ],
                    "ownerDepartmentId": app["departmentId"],
                },
            )
            memory_credential = post(
                control,
                "/credentials",
                {
                    "name": f"P7 Mem0 API key {run_id}",
                    "credentialType": "api_key",
                    "secret": memory_key.value,
                    "ownerDepartmentId": app["departmentId"],
                },
            )
            memory_connection = post(
                control,
                "/memory/connections",
                {
                    "name": f"P7 live Mem0 {run_id}",
                    "endpoint": f"http://mem0.{namespace}.svc:8000",
                    "healthPath": "/openapi.json",
                    "credentialId": memory_credential["id"],
                    "ownerDepartmentId": app["departmentId"],
                    "configuration": {},
                },
            )
            memory_resource = memory_connection
            yield {
                "namespace": namespace,
                "ragId": rag["id"],
                "ragCredentialId": rag_credential["id"],
                "memoryId": memory_resource["id"],
                "memoryCredentialId": memory_credential["id"],
                "memoryClient": memory,
                "control": control,
                "ragUrl": f"http://lightrag.{namespace}.svc:9621",
                "ragKey": Secret(key),
                "directory": directory,
                "modelName": existing_model["modelName"],
            }
    finally:
        for name in ("lightrag", "mem0"):
            logs = run(("kubectl", "-n", namespace, "logs", f"deployment/{name}", "--tail=150"), check=False)
            (directory / f"{name}.log").write_text(
                redact(logs.stdout + logs.stderr)
                .replace(existing_model_secret.value, "<redacted>")
                .replace(key, "<redacted>")
            )
