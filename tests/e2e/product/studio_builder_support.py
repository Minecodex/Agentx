"""Studio acceptance resources. The configured provider secret never leaves memory."""

from __future__ import annotations

from pathlib import Path

import httpx
import pytest
import yaml

from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import ROOT, run


def post(client: httpx.Client, path: str, payload: dict) -> dict:
    response = client.post(f"/api/v1{path}", json=payload)
    assert response.status_code in (200, 201, 202), response.text
    return response.json()


@pytest.fixture(scope="session")
def studio_provider(installed_agentx: dict[str, str]) -> str:
    """Reuse the provider overlay, deploying only the model fixture this suite uses."""
    namespace = installed_agentx["dependencies_namespace"]
    manifests = run(("kubectl", "kustomize", ROOT / "deploy/kustomize/e2e-fixtures/runtime-providers")).stdout
    values = yaml.safe_load(Path(installed_agentx["values"]).read_text())
    documents = [
        document
        for document in yaml.safe_load_all(manifests)
        if document and document["metadata"]["name"] == "echo-mcp"
    ]
    assert {document["kind"] for document in documents} >= {"Deployment", "Service"}
    for document in documents:
        if document["kind"] == "Deployment":
            document["spec"]["template"]["spec"]["containers"][0]["image"] = (
                f"agentx/echo-mcp:{values['global']['images']['tag']}"
            )
    run(("kubectl", "-n", namespace, "apply", "-f", "-"), input_text=yaml.safe_dump_all(documents))
    run(("kubectl", "-n", namespace, "rollout", "status", "deployment/echo-mcp", "--timeout=300s"), timeout=330)
    return f"http://echo-mcp.{namespace}.svc:8090/v1"


@pytest.fixture(scope="session")
def studio_models(installed_agentx, service_urls, run_id, studio_provider, existing_model, existing_model_secret):
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as client:
        token, me = _access_token(client)
        client.headers.update({"Authorization": f"Bearer {token}"})
        result = {}
        for mode, secret, endpoint, name in [
            ("fixture", "m5-model-secret", studio_provider, "echo-studio-builder"),
            ("alternate", "m5-model-secret", studio_provider, "echo-studio-builder"),
            (
                "live",
                existing_model_secret.value,
                existing_model["endpoint"],
                existing_model["modelName"],
            ),
        ]:
            credential = post(
                client,
                "/credentials",
                {
                    "name": f"P8 {mode} {run_id}",
                    "credentialType": "bearer",
                    "secret": secret,
                    "ownerDepartmentId": me["departmentId"],
                },
            )
            alias = f"p8-{mode}-{run_id}"
            payload = {
                "connectionName": alias,
                "alias": alias,
                "providerType": "openai_compatible",
                "endpoint": endpoint,
                "credentialId": credential["id"],
                "ownerDepartmentId": me["departmentId"],
                "modelName": name,
                "price": {
                    "currency": existing_model["currency"],
                    "inputPerMillion": existing_model["inputPrice"],
                    "outputPerMillion": existing_model["outputPrice"],
                },
            }
            if mode == "live":
                payload.update(
                    {key: existing_model[key] for key in ["defaultParameters", "maxInputTokens", "maxOutputTokens"]}
                )
            model = post(client, "/models/aliases", payload)
            result[mode] = {"id": model["id"], "alias": alias, "credentialId": credential["id"]}
        yield result


@pytest.fixture(scope="session")
def studio_users(service_urls, run_id, studio_models):
    from tests.e2e.product.next_batch_support import create_user

    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as client:
        token, me = _access_token(client)
        client.headers.update({"Authorization": f"Bearer {token}"})
        users = {}
        for code, permissions in [
            ("editor", ["workflow:view", "workflow:create", "workflow:edit", "execution:view"]),
            ("viewer", ["workflow:view"]),
        ]:
            role = post(
                client,
                "/roles",
                {
                    "name": f"P8 {code}",
                    "code": f"p8_{code}_{run_id.replace(chr(45), chr(95))}",
                    "dataScope": "company",
                    "permissions": permissions,
                },
            )
            users[code] = create_user(client, f"p8-{code}-{run_id}", me["departmentId"], role["id"])
        yield users
