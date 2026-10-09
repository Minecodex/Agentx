"""Real Kimi resources for opt-in local acceptance; secrets stay in memory."""

from __future__ import annotations

import base64
import os
import socket
import ssl
import time
from collections.abc import Iterator
from dataclasses import dataclass, field

import httpx
import pytest

from tests.e2e.product.test_model_streaming import _grant_model_to_workflow, _stream_workflow
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import ROOT, run, start_process


@dataclass
class Secret:
    value: str = field(repr=False)


def post(client: httpx.Client, path: str, payload: dict) -> dict:
    response = client.post(f"/api/v1{path}", json=payload)
    assert response.status_code in (200, 201, 202), response.text
    return response.json()


@pytest.fixture(scope="session")
def live_kimi_secret(run_id: str) -> Iterator[Secret]:
    reference = os.environ.get("AGENTX_E2E_LIVE_VAULT_SECRET_REF")
    if not reference:
        raise RuntimeError("AGENTX_E2E_LIVE_VAULT_SECRET_REF must identify the existing Kimi Vault credential")
    directory = ROOT / ".local/artifacts/e2e" / run_id / "live-model"
    directory.mkdir(parents=True, exist_ok=True)
    control_secret = run(
        ("kubectl", "-n", "agentx-prod-control", "get", "secret", "agentx-control-secrets", "-o", "json")
    ).json()["data"]
    vault_token = base64.b64decode(control_secret["AGENTX_CONTROL_VAULT_TOKEN"]).decode()
    ca_secret = run(
        ("kubectl", "-n", "agentx-prod-control", "get", "secret", "agentx-prod-vault-ca", "-o", "json")
    ).json()["data"]
    ca = directory / "vault-ca.pem"
    ca.write_bytes(base64.b64decode(ca_secret["ca.crt"]))
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    forward = start_process(
        ("kubectl", "-n", "agentx-prod-external", "port-forward", "service/vault", f"{port}:8200"),
        stdout_path=directory / "vault-forward.log",
        stderr_path=directory / "vault-forward-error.log",
    )
    try:
        with httpx.Client(verify=ssl.create_default_context(cafile=str(ca)), trust_env=False, timeout=10) as vault:
            deadline = time.monotonic() + 30
            while True:
                try:
                    response = vault.get(
                        f"https://127.0.0.1:{port}/v1/agentx-v2/data/{reference}?version=1",
                        headers={"X-Vault-Token": vault_token},
                        extensions={"sni_hostname": "vault.agentx-prod-external.svc"},
                    )
                    break
                except httpx.ConnectError:
                    if time.monotonic() > deadline:
                        raise
                    time.sleep(0.3)
            response.raise_for_status()
            secret = Secret(response.json()["data"]["data"]["value"])
        yield secret
        secret.value = ""
    finally:
        forward.stop()


def publish_version(control: httpx.Client, workflow_id: str, prompt: str) -> dict:
    draft = control.get(f"/api/v1/workflows/{workflow_id}/draft").json()
    definition = draft["definition"]
    model = next(node for node in definition["nodes"] if node["type"] == "model")
    model["parameters"]["prompt"] = {"kind": "template", "segments": [{"kind": "text", "text": prompt}]}
    for reference in model["resourceReferences"]:
        if reference["resourceType"] == "model":
            reference.pop("bindingRole", None)
            selected = control.get(f"/api/v1/models/aliases/{reference['resourceId']}")
            selected.raise_for_status()
            reference["resourceVersionId"] = selected.json()["deploymentId"]
    saved = control.put(
        f"/api/v1/workflows/{workflow_id}/draft", json={"expectedRevision": draft["revision"], "definition": definition}
    )
    assert saved.status_code in (200, 204), saved.text
    revision = control.get(f"/api/v1/workflows/{workflow_id}/draft").json()["revision"]
    return post(control, f"/workflows/{workflow_id}/versions", {"draftRevision": revision})


@pytest.fixture(scope="session")
def live_application(installed_agentx, service_urls, run_id, live_kimi_secret):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        control.headers.update(headers)
        credential = post(
            control,
            "/credentials",
            {
                "name": f"P7 live Kimi {run_id}",
                "credentialType": "bearer",
                "secret": live_kimi_secret.value,
                "ownerDepartmentId": me["departmentId"],
            },
        )
        alias = f"p7-kimi-{run_id}"
        model = post(
            control,
            "/models/aliases",
            {
                "connectionName": alias,
                "providerType": "openai_compatible",
                "endpoint": "https://api.kimi.com/coding/v1",
                "credentialId": credential["id"],
                "ownerDepartmentId": me["departmentId"],
                "alias": alias,
                "modelName": "k3",
                "price": {"currency": "USD", "inputPerMillion": "0", "outputPerMillion": "0"},
            },
        )
        workflow_id = _stream_workflow(control, headers, f"P7 live text {run_id}", model["id"])
        _grant_model_to_workflow(control, headers, alias, workflow_id, credential["id"])
        v1 = publish_version(
            control, workflow_id, "你是中文测试助手。准确回答用户要求。普通问答保持简短。用户要求逐字回复时严格遵循。"
        )
        v2 = publish_version(
            control,
            workflow_id,
            "你是中文测试助手。准确回答用户要求。普通问答分条详细说明。用户要求逐字回复时严格遵循。",
        )
        environment = next(item for item in control.get("/api/v1/environments").json() if item["code"] == "development")
        post(
            control,
            f"/workflows/{workflow_id}/deployments",
            {"environmentId": environment["id"], "workflowVersionId": v2["id"]},
        )
        app = post(
            control,
            "/applications",
            {
                "workflowId": workflow_id,
                "name": f"P7 live chat {run_id}",
                "slug": f"p7-live-{run_id}",
                "visibility": "company",
            },
        )
        deployment = _publish_application_deployment(control, headers, app["id"], v2["id"], environment["id"])
        config_path = f"/api/v1/applications/{app['id']}/deployments/{deployment['id']}/playground-config"
        current = control.get(config_path).json()
        mapped = control.put(
            config_path,
            json={
                "expectedVersion": current["version"],
                "mapping": {
                    "questionInput": "message",
                    "fileInput": None,
                    "answerOutput": "answer",
                    "answerFilesOutput": None,
                },
            },
        )
        assert mapped.status_code in (200, 202), mapped.text
        deadline = time.monotonic() + 120
        while True:
            mapping = control.get(config_path).json()
            if mapping["publishStatus"] == "active":
                break
            assert mapping["publishStatus"] != "failed", mapping
            assert time.monotonic() < deadline, "live chat mapping did not publish"
            time.sleep(1)
        return {
            "applicationId": app["id"],
            "applicationSlug": app["slug"],
            "workflowId": workflow_id,
            "v1": v1["id"],
            "v2": v2["id"],
            "modelId": model["id"],
            "modelAlias": alias,
            "credentialId": credential["id"],
            "departmentId": me["departmentId"],
            "token": Secret(token),
        }
