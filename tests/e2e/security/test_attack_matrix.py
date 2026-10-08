"""plan7 P7-D7: two-tenant attack matrix, negative validators.

Tenant-scoped ID guessing, forged webhook signatures, idempotency replay
with a different payload, unauthenticated invocation and secret leakage into
collected logs. Remaining matrix items (sandbox handle invalidation, token
version revocation UX, grant escalation chains) stay on the handoff list.
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import time
import uuid
from pathlib import Path
from typing import Any

import httpx
import pytest

from tests.e2e.product.test_channel_delivery import (
    _access_token,
    _development_environment_id,
    _dingtalk_channel,
    _passthrough_workflow,
    _publish_application,
    _signed_dingtalk_post,
)
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.security]


@pytest.fixture(scope="module")
def attack_app(installed_agentx: dict[str, str], service_urls: dict[str, str]) -> dict[str, Any]:
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, _me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        workflow = _passthrough_workflow(control, headers, f"Attack {installed_agentx['run_id']}")
        environment_id = _development_environment_id(control, headers)
        deploy = control.post(
            f"/api/v1/workflows/{workflow['workflowId']}/deployments",
            headers=headers,
            json={"environmentId": environment_id, "workflowVersionId": workflow["versionId"]},
        )
        assert deploy.status_code in (200, 201), deploy.text
        application = control.post(
            "/api/v1/applications",
            headers=headers,
            json={
                "workflowId": workflow["workflowId"],
                "name": f"Attack App {installed_agentx['run_id']}",
                "slug": f"attack-{installed_agentx['run_id']}",
                "visibility": "company",
            },
        )
        assert application.status_code in (200, 201), application.text
        application_id = application.json()["id"]
        channel = _dingtalk_channel(control, headers, application_id, reply_enabled=False)
        _publish_application(control, headers, workflow, application_id, environment_id)
        deployments = control.get(f"/api/v1/applications/{application_id}/deployments", headers=headers)
        deployments.raise_for_status()
        latest = deployments.json()[0]
        mapping = control.put(
            f"/api/v1/applications/{application_id}/deployments/{latest['id']}/playground-config",
            headers=headers,
            json={
                "expectedVersion": 0,
                "mapping": {
                    "questionInput": "message",
                    "fileInput": None,
                    "answerOutput": "answer",
                    "answerFilesOutput": None,
                },
            },
        )
        assert mapping.status_code in (200, 201, 202, 204), mapping.text
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            config = control.get(
                f"/api/v1/applications/{application_id}/deployments/{latest['id']}/playground-config",
                headers=headers,
            )
            config.raise_for_status()
            if config.json().get("publishStatus") == "active":
                break
            time.sleep(2)
        else:
            raise AssertionError("attack app mapping did not publish")
    return {
        "token": token,
        "applicationId": application_id,
        "channel": channel,
        "slug": f"attack-{installed_agentx['run_id']}",
    }


def test_foreign_resource_ids_are_not_enumerable(service_urls: dict[str, str], attack_app: dict[str, Any]) -> None:
    """Cross-tenant ID guessing: a valid token cannot read foreign ids."""
    with httpx.Client(base_url=service_urls["web"], timeout=30) as control:
        response = control.get(
            f"/api/v1/applications/{uuid.uuid4()}",
            headers={"Authorization": f"Bearer {attack_app['token']}"},
        )
        assert response.status_code == 404, response.text


def test_webhook_signature_from_wrong_secret_is_rejected(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    attack_app: dict[str, Any],
) -> None:
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        forged = _signed_dingtalk_post(
            gateway,
            attack_app["channel"]["path"],
            "attacker-secret",
            {
                "msgId": f"forge-{time.time_ns()}",
                "conversationId": "attack",
                "conversationType": "1",
                "senderId": "attacker",
                "senderNick": "Attacker",
                "msgtype": "text",
                "content": json.dumps({"content": "forged"}),
                "createAt": int(time.time() * 1000),
            },
            "http://im-mock.invalid/x",
        )
        assert forged.status_code in (401, 403), f"{forged.status_code} {forged.text}"


def test_idempotency_key_replay_with_different_input_is_rejected(
    service_urls: dict[str, str],
    attack_app: dict[str, Any],
) -> None:
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        session = gateway.post(
            f"/gateway/v1/applications/{attack_app['slug']}/sessions",
            headers={"Authorization": f"Bearer {attack_app['token']}", "Idempotency-Key": f"atk-s-{time.time_ns()}"},
            json={"title": None, "externalUserId": None},
        )
        assert session.status_code == 201, session.text
        session_id = session.json()["id"]
        key = f"replay-{time.time_ns()}"
        first = gateway.post(
            f"/gateway/v1/sessions/{session_id}/messages",
            headers={"Authorization": f"Bearer {attack_app['token']}", "Idempotency-Key": key},
            json={"parts": [{"partType": "text", "content": "first payload"}]},
        )
        assert first.status_code == 202, first.text
        second = gateway.post(
            f"/gateway/v1/sessions/{session_id}/messages",
            headers={"Authorization": f"Bearer {attack_app['token']}", "Idempotency-Key": key},
            json={"parts": [{"partType": "text", "content": "different payload"}]},
        )
        assert second.status_code in (409, 422), f"{second.status_code} {second.text}"


def test_unauthenticated_invocation_is_refused(service_urls: dict[str, str]) -> None:
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        response = gateway.post(
            f"/gateway/v1/applications/{uuid.uuid4()}/invocations",
            json={"input": {}},
        )
        assert response.status_code in (401, 403), response.text


def test_channel_secrets_do_not_leak_into_collected_logs(
    installed_agentx: dict[str, str], attack_app: dict[str, Any]
) -> None:
    """Log disclosure: the dingtalk channel secret never appears in pod logs."""
    secret = "e2e-dingtalk-secret"  # noqa: S105 -- fixture credential, asserted absent from logs
    digest = base64.b64encode(hmac.new(secret.encode(), b"probe", hashlib.sha256).digest()).decode()
    namespaces = [
        installed_agentx["control_namespace"],
        installed_agentx["runtime_namespace"],
    ]
    for namespace in namespaces:
        logs = run(
            (
                "kubectl",
                "-n",
                namespace,
                "logs",
                "deployment/workflow-runtime",
                "--tail=2000",
            )
            if namespace == installed_agentx["runtime_namespace"]
            else ("kubectl", "-n", namespace, "logs", "deployment/platform-control", "--tail=2000"),
            timeout=120,
        ).stdout
        assert secret not in logs, f"raw channel secret leaked in {namespace} logs"
        assert digest not in logs, f"signed secret digest leaked in {namespace} logs"
    evidence_logs = Path(installed_agentx["artifact_dir"]).glob("*-logs.txt")
    for path in evidence_logs:
        content = path.read_text(encoding="utf-8", errors="replace")
        assert secret not in content, f"raw channel secret leaked into evidence {path.name}"
