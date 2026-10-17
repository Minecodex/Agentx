"""Two real LightRAG instances and native RAGFlow dataset seeding."""

from __future__ import annotations

import copy
import json
import secrets
from pathlib import Path

import pytest
import yaml

from tests.e2e.conftest import RAGFLOW_USER_EMAIL, _ragflow_encrypt_password
from tests.e2e.product.knowledge_native_support import provider_client
from tests.e2e.product.live_text_support import Secret
from tests.e2e.support import ROOT, run


@pytest.fixture(scope="module")
def knowledge_instances(installed_agentx, cpu_embedding_service, existing_model, existing_model_secret):
    namespace = f"agentx-e2e-{installed_agentx['run_id']}-knowledge"
    directory = Path(installed_agentx["artifact_dir"]) / "knowledge-native"
    directory.mkdir(parents=True, exist_ok=True)
    key = secrets.token_urlsafe(32)
    rendered = list(
        yaml.safe_load_all(
            run(("kubectl", "kustomize", ROOT / "deploy/kustomize/e2e-fixtures/runtime-providers")).stdout
        )
    )
    names = {
        "lightrag",
        "lightrag-data",
        "lightrag-tokenizer-cache",
        "lightrag-tokenizer-cache-egress",
        "agentx-lightrag-config",
        "agentx-lightrag-secrets",
    }
    base = [d for d in rendered if d and d["metadata"]["name"] in names]
    run(
        ("kubectl", "apply", "-f", "-"),
        input_text=json.dumps(
            {
                "apiVersion": "v1",
                "kind": "Namespace",
                "metadata": {"name": namespace, "labels": {"agentx.io/plane": "dependencies"}},
            }
        ),
    )
    try:
        for suffix, workspace in (("", "alpha"), ("-b", "beta")):
            mapping = {name: name + suffix for name in names}

            def rename(value, mapping=mapping):
                if isinstance(value, dict):
                    return {k: rename(v, mapping) for k, v in value.items()}
                if isinstance(value, list):
                    return [rename(v, mapping) for v in value]
                return mapping.get(value, value) if isinstance(value, str) else value

            documents = rename(copy.deepcopy(base))
            for document in documents:
                if document["kind"] == "ConfigMap":
                    document["data"].update(
                        {
                            "WORKSPACE": workspace,
                            "LLM_BINDING_HOST": existing_model["endpoint"],
                            "LLM_MODEL": existing_model["modelName"],
                            "OPENAI_LLM_EXTRA_BODY": json.dumps(
                                {**existing_model["defaultParameters"], "stream": False}
                            ),
                            "EMBEDDING_BINDING_HOST": f"http://cpu-embedding.{cpu_embedding_service['namespace']}.svc:7997/v1",
                            "EMBEDDING_MODEL": "BAAI/bge-small-zh-v1.5",
                            "EMBEDDING_DIM": "512",
                            "MAX_ASYNC": "2",
                            "EMBEDDING_FUNC_MAX_ASYNC": "2",
                        }
                    )
                elif document["kind"] == "Secret":
                    document.pop("data", None)
                    document["stringData"] = {
                        "LIGHTRAG_API_KEY": key,
                        "LLM_BINDING_API_KEY": existing_model_secret.value,
                        "EMBEDDING_BINDING_API_KEY": cpu_embedding_service["apiKey"],
                    }
            run(("kubectl", "-n", namespace, "apply", "-f", "-"), input_text=yaml.safe_dump_all(documents))
            run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "wait",
                    "--for=condition=complete",
                    f"job/lightrag-tokenizer-cache{suffix}",
                    "--timeout=300s",
                ),
                timeout=330,
            )
            run(
                ("kubectl", "-n", namespace, "rollout", "status", f"deployment/lightrag{suffix}", "--timeout=600s"),
                timeout=630,
            )
        # Permit only provider-labelled pods in dependency namespaces to use the isolated embedding fixture.
        ingress = {
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {"name": "knowledge-provider-embedding"},
            "spec": {
                "podSelector": {"matchLabels": {"app.kubernetes.io/name": "cpu-embedding"}},
                "policyTypes": ["Ingress"],
                "ingress": [
                    {
                        "from": [
                            {
                                "namespaceSelector": {"matchLabels": {"agentx.io/plane": "dependencies"}},
                                "podSelector": {"matchLabels": {"agentx.io/runtime-provider": "allowed"}},
                            }
                        ],
                        "ports": [{"protocol": "TCP", "port": 7997}],
                    }
                ],
            },
        }
        run(
            ("kubectl", "-n", cpu_embedding_service["namespace"], "apply", "-f", "-"),
            input_text=yaml.safe_dump(ingress),
        )
        run(
            ("kubectl", "-n", installed_agentx["dependencies_namespace"], "apply", "-f", "-"),
            input_text=yaml.safe_dump(
                {
                    "apiVersion": "networking.k8s.io/v1",
                    "kind": "NetworkPolicy",
                    "metadata": {"name": "knowledge-ragflow-embedding"},
                    "spec": {
                        "podSelector": {"matchLabels": {"app.kubernetes.io/name": "ragflow"}},
                        "policyTypes": ["Egress"],
                        "egress": [
                            {
                                "to": [
                                    {
                                        "namespaceSelector": {
                                            "matchLabels": {
                                                "kubernetes.io/metadata.name": cpu_embedding_service["namespace"],
                                            }
                                        },
                                        "podSelector": {"matchLabels": {"app.kubernetes.io/name": "cpu-embedding"}},
                                    }
                                ],
                                "ports": [{"protocol": "TCP", "port": 7997}],
                            }
                        ],
                    },
                }
            ),
        )
        yield {"namespace": namespace, "directory": directory, "key": Secret(key), "embedding": cpu_embedding_service}
    finally:
        for service in ("lightrag", "lightrag-b"):
            logs = run(("kubectl", "-n", namespace, "logs", f"deployment/{service}", "--tail=150"), check=False)
            (directory / f"{service}.log").write_text(
                logs.stdout.replace(key, "<redacted>").replace(existing_model_secret.value, "<redacted>")
            )
        run(("kubectl", "delete", "namespace", namespace, "--wait=true", "--timeout=120s"), timeout=150)
        (directory / "cleanup.json").write_text(json.dumps({"namespace": namespace, "deleted": True}))


def seed_ragflow(installed, providers, instances, run_id):
    embedding = instances["embedding"]
    with provider_client(
        installed["dependencies_namespace"], "ragflow", 9380, instances["directory"], health="/v1/system/config"
    ) as client:
        login = client.post(
            "/v1/user/login", json={"email": RAGFLOW_USER_EMAIL, "password": _ragflow_encrypt_password()}
        )
        assert login.status_code == 200 and login.headers.get("Authorization")
        configured = client.post(
            "/v1/llm/add_llm",
            headers={"Authorization": login.headers["Authorization"]},
            json={
                "llm_factory": "OpenAI-API-Compatible",
                "model_type": "embedding",
                "llm_name": "BAAI/bge-small-zh-v1.5",
                "api_key": embedding["apiKey"],
                "api_base": f"http://cpu-embedding.{embedding['namespace']}.svc:7997/v1",
                "max_tokens": 512,
            },
        )
        assert configured.status_code == 200 and configured.json()["code"] == 0, configured.text.replace(
            embedding["apiKey"], "<redacted>"
        )
        client.headers["Authorization"] = f"Bearer {providers['ragflow_api_key']}"
        result = []
        for index in range(2):
            dataset = client.post(
                "/api/v1/datasets",
                json={
                    "name": f"knowledge-{run_id}-{index}",
                    "embedding_model": "BAAI/bge-small-zh-v1.5___OpenAI-API@OpenAI-API-Compatible",
                    "chunk_method": "naive",
                },
            ).json()
            assert dataset["code"] == 0, dataset
            dataset_id = dataset["data"]["id"]
            marker = f"ragflow-sapphire-{run_id}-{index}"
            content = f"水星计划的发布代号是 {marker}。这是第 {index} 个团队的独立项目文档。"
            uploaded = client.post(
                f"/api/v1/datasets/{dataset_id}/documents",
                files={"file": (f"team-{index}.txt", content.encode(), "text/plain")},
            ).json()
            assert uploaded["code"] == 0, uploaded
            document_id = uploaded["data"][0]["id"]
            chunk = client.post(
                f"/api/v1/datasets/{dataset_id}/documents/{document_id}/chunks",
                json={"content": content, "important_keywords": ["水星计划", "发布代号"]},
            ).json()
            assert chunk["code"] == 0, chunk
            result.append({"datasetId": dataset_id, "documentId": document_id, "marker": marker})
        return result
