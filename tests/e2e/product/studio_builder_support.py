"""Studio acceptance resources. The configured provider secret never leaves memory."""

from __future__ import annotations

from pathlib import Path

import httpx
import pytest
import yaml

from tests.e2e.draft_fixtures import idempotent_headers
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import ROOT, run


def post(client: httpx.Client, path: str, payload: dict) -> dict:
    response = client.post(f"/api/v1{path}", json=payload, headers=idempotent_headers())
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
    # Catalog discovery and retrieval tests run in Control. Only this isolated
    # fixture needs its nonstandard 8090 port on that direct provider path.
    peer = {
        "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": namespace}},
        "podSelector": {"matchLabels": {"app.kubernetes.io/name": "echo-mcp"}},
    }
    control_peer = {
        "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": installed_agentx["control_namespace"]}},
        "podSelector": {"matchLabels": {"app.kubernetes.io/name": "platform-control"}},
    }
    for target, name, selector, direction, rules in (
        (
            installed_agentx["control_namespace"],
            "knowledge-fixture-egress",
            control_peer["podSelector"],
            "Egress",
            {"to": [peer]},
        ),
        (namespace, "knowledge-fixture-ingress", peer["podSelector"], "Ingress", {"from": [control_peer]}),
    ):
        rules["ports"] = [{"protocol": "TCP", "port": 8090}]
        policy = {
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {"name": name},
            "spec": {"podSelector": selector, "policyTypes": [direction], direction.lower(): [rules]},
        }
        run(("kubectl", "-n", target, "apply", "-f", "-"), input_text=yaml.safe_dump(policy))
    return f"http://echo-mcp.{namespace}.svc:8090/v1"


def _model(client, department_id, run_id, mode, secret, endpoint, name, price, parameters=None):
    credential = post(
        client,
        "/credentials",
        {
            "name": f"P8 {mode} {run_id}",
            "credentialType": "bearer",
            "secret": secret,
            "ownerDepartmentId": department_id,
        },
    )
    alias = f"p8-{mode}-{run_id}"
    model = post(
        client,
        "/models/aliases",
        {
            "connectionName": alias,
            "alias": alias,
            "providerType": "openai_compatible",
            "endpoint": endpoint,
            "credentialId": credential["id"],
            "ownerDepartmentId": department_id,
            "modelName": name,
            "price": price,
            **(parameters or {}),
        },
    )
    return {"id": model["id"], "alias": alias, "credentialId": credential["id"]}


@pytest.fixture(scope="session")
def studio_models(service_urls, run_id, studio_provider):
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as client:
        token, me = _access_token(client)
        client.headers.update({"Authorization": f"Bearer {token}"})
        return {
            mode: _model(
                client,
                me["departmentId"],
                run_id,
                mode,
                "m5-model-secret",
                studio_provider,
                "echo-studio-builder",
                {"currency": "USD", "inputPerMillion": "1", "outputPerMillion": "2"},
            )
            for mode in ("fixture", "alternate")
        }


@pytest.fixture(scope="session")
def studio_live_models(service_urls, run_id, studio_models, existing_model, existing_model_secret):
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as client:
        token, me = _access_token(client)
        client.headers.update({"Authorization": f"Bearer {token}"})
        live = _model(
            client,
            me["departmentId"],
            run_id,
            "live",
            existing_model_secret.value,
            existing_model["endpoint"],
            existing_model["modelName"],
            {
                "currency": existing_model["currency"],
                "inputPerMillion": existing_model["inputPrice"],
                "outputPerMillion": existing_model["outputPrice"],
            },
            {key: existing_model[key] for key in ("defaultParameters", "maxInputTokens", "maxOutputTokens")},
        )
        return {**studio_models, "live": live}


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
