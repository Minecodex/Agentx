# ruff: noqa: S608
"""Channel outbound delivery E2E (plan7 P7-A A7).

Drives the L1 auto-reply loop end to end against the in-cluster IM mock:
signed dingtalk inbound webhook message (sessionWebhook pointing at the mock)
→ workflow execution → terminal-transaction delivery enqueue → delivery loop
POSTs to the mock → delivery record delivered with provider_message_id. Also
exercises the permanent-failure dead-letter path and its replay.
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import time
from pathlib import Path
from typing import Any

import httpx
import pytest

from tests.e2e.runtime.test_agent_attachments import (
    _access_token,
    _runtime_mysql,
)
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.product]


@pytest.fixture(scope="module")
def im_mock(installed_agentx: dict[str, str]) -> dict[str, str]:
    namespace = installed_agentx["dependencies_namespace"]
    fixture = Path(installed_agentx["root"]) / "deploy/kustomize/e2e-fixtures/im-mock"
    run(("kubectl", "-n", namespace, "apply", "-k", fixture), timeout=120)
    result = run(
        ("kubectl", "-n", namespace, "rollout", "status", "deployment/im-mock", "--timeout=180s"),
        check=False,
        timeout=210,
    )
    assert result.returncode == 0, "im-mock fixture did not become ready"
    return {"url": f"http://im-mock.{namespace}.svc:8090"}


def _passthrough_workflow(control: httpx.Client, headers: dict[str, str], name: str) -> dict[str, str]:
    """message-in / answer-out workflow with no model dependency."""
    created = control.post(
        "/api/v1/workflows",
        headers=headers,
        json={"name": name, "description": "P7-A delivery e2e", "visibility": "company"},
    )
    created.raise_for_status()
    workflow_id = created.json()["id"]
    draft = control.get(f"/api/v1/workflows/{workflow_id}/draft", headers=headers)
    draft.raise_for_status()
    definition = draft.json()["definition"]
    definition["start"]["inputs"] = {
        "type": "object",
        "properties": {"message": {"type": "string"}},
        "required": ["message"],
        "additionalProperties": False,
    }
    exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
    exit_node["parameters"]["outputs"] = {
        "answer": {
            "kind": "reference",
            "selector": {
                "namespace": "inputs",
                "run": {"kind": "current"},
                "item": {"kind": "current"},
                "path": ["message"],
            },
            "missingPolicy": {"kind": "error"},
        }
    }
    definition["end"] = {
        "completion": "first_return",
        "outputs": {"answer": {"schema": {"type": "string"}, "required": True, "sensitive": False}},
        "error": {"outputs": {}},
    }
    definition["connections"] = [
        {
            "id": "c-exit",
            "sourceNodeId": "__start__",
            "sourceHandle": "main",
            "targetNodeId": exit_node["id"],
            "targetHandle": "main",
            "order": 0,
        }
    ]
    saved = control.put(
        f"/api/v1/workflows/{workflow_id}/draft",
        headers=headers,
        json={"expectedRevision": draft.json()["revision"], "definition": definition},
    )
    assert saved.status_code in (200, 204), saved.text
    latest = control.get(f"/api/v1/workflows/{workflow_id}/draft", headers=headers)
    latest.raise_for_status()
    published = control.post(
        f"/api/v1/workflows/{workflow_id}/versions",
        headers=headers,
        json={"draftRevision": latest.json()["revision"]},
    )
    assert published.status_code in (200, 201), published.text
    return {"workflowId": workflow_id, "versionId": published.json()["id"]}


def _development_environment_id(control: httpx.Client, headers: dict[str, str]) -> str:
    environments = control.get("/api/v1/environments", headers=headers)
    environments.raise_for_status()
    return next(item for item in environments.json() if item["code"] == "development")["id"]


def _publish_application(
    control: httpx.Client,
    headers: dict[str, str],
    workflow: dict[str, str],
    application_id: str,
    environment_id: str,
) -> dict[str, Any]:
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        publish = control.post(
            f"/api/v1/applications/{application_id}/deployments",
            headers=headers,
            json={
                "workflowVersionId": workflow["versionId"],
                "environmentId": environment_id,
                "sessionVersionPolicy": "pinned",
            },
        )
        if publish.status_code == 202:
            break
        time.sleep(3)
    else:
        raise AssertionError("application publish did not converge")
    while time.monotonic() < deadline:
        deployments = control.get(f"/api/v1/applications/{application_id}/deployments", headers=headers)
        deployments.raise_for_status()
        latest = deployments.json()[0]
        if latest["status"] == "active":
            return {"applicationId": application_id, "deploymentId": latest["id"]}
        assert latest["status"] != "rejected", latest
        time.sleep(2)
    raise AssertionError("application deployment did not become active")


def _dingtalk_channel(
    control: httpx.Client, headers: dict[str, str], application_id: str, reply_enabled: bool
) -> dict[str, Any]:
    created = control.post(
        f"/api/v1/applications/{application_id}/webhooks",
        headers=headers,
        json={
            "name": "DingTalk Delivery E2E",
            "providerType": "dingtalk",
            "channelMode": "callback",
            "channelConfig": {"secret": "e2e-dingtalk-secret", "aesKey": "", "robotCode": "e2e-robot"},
            "reply": {"enabled": reply_enabled, "outputField": "answer"},
            "inputMappings": [{"source": "message.text", "target": "message", "missingPolicy": "error"}],
            "fixedInputs": {},
        },
    )
    assert created.status_code in (200, 201), created.text
    return created.json()


def _signed_dingtalk_post(
    gateway: httpx.Client, path: str, secret: str, payload: dict[str, Any], session_webhook: str
) -> httpx.Response:
    timestamp = str(int(time.time() * 1000))
    digest = hmac.new(secret.encode(), f"{timestamp}\n{secret}".encode(), hashlib.sha256).digest()
    sign = base64.b64encode(digest).decode()
    body = {
        **payload,
        "sessionWebhook": session_webhook,
        "sessionWebhookExpiredTime": int(time.time() * 1000) + 3600_000,
    }
    return gateway.post(
        path,
        params={"timestamp": timestamp, "sign": sign},
        json=body,
    )


def _mock_received(ns: str, path_contains: str) -> list[dict[str, Any]]:
    result = run(
        (
            "kubectl",
            "-n",
            ns,
            "exec",
            "deployment/im-mock",
            "--",
            "python3",
            "-c",
            "import json,urllib.request;print(json.dumps(json.load(urllib.request.urlopen('http://127.0.0.1:8090/received'))))",
        ),
        check=False,
        timeout=60,
    )
    if result.returncode != 0:
        return []
    return [item for item in json.loads(result.stdout).get("items", []) if path_contains in item.get("path", "")]


def _delivery_rows(installed_agentx: dict[str, str], tenant: str, execution_id: str) -> list[dict[str, Any]]:
    raw = _runtime_mysql(
        installed_agentx,
        "SELECT JSON_ARRAYAGG(JSON_OBJECT('id',BIN_TO_UUID(id),'status',status,'origin',origin,"
        "'provider',provider,'attemptCount',attempt_count,'lastErrorCode',last_error_code,"
        "'providerMessageId',provider_message_id)) FROM delivery_outbox "
        f"WHERE tenant_id=UUID_TO_BIN('{tenant}') AND execution_id=UUID_TO_BIN('{execution_id}');",
    )
    return json.loads(raw) if raw and raw.lower() != "null" else []


def test_l1_reply_delivered_and_dead_letter_replay(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    im_mock: dict[str, str],
    run_id: str,
) -> None:
    deps_ns = installed_agentx["dependencies_namespace"]
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        workflow = _passthrough_workflow(control, headers, f"Delivery E2E {run_id}")
        environment_id = _development_environment_id(control, headers)
        # Application deployments require the workflow to be deployed at the
        # workflow level first.
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
                "name": f"Delivery App {run_id}",
                "slug": f"delivery-{run_id}",
                "visibility": "company",
            },
        )
        assert application.status_code in (200, 201), application.text
        application_id = application.json()["id"]
        # The channel must exist before the application publish so the
        # deployment snapshots the trigger revision that already contains the
        # webhook manifest; activation then creates the runtime binding.
        channel = _dingtalk_channel(control, headers, application_id, reply_enabled=True)
        _publish_application(control, headers, workflow, application_id, environment_id)

    # The gateway nests the public router under /gateway/v1 and the
    # port-forwarded service targets the same pods, so channel.path is used
    # verbatim.
    webhook_path = channel["path"]
    secret = "e2e-dingtalk-secret"  # noqa: S105 -- isolated E2E fixture credential
    public_id = channel["publicId"]

    def _binding_state() -> str:
        return _runtime_mysql(
            installed_agentx,
            "SELECT CONCAT(COUNT(*), ':', COALESCE(MAX(w.status),'none')) FROM webhook_bindings w "
            f"WHERE w.public_id='{public_id}';",
        )

    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and _binding_state().startswith("0:"):
        time.sleep(3)
    binding = _binding_state()
    assert not binding.startswith("0:"), f"webhook binding never reached runtime: {binding}"

    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        # 1. happy path: reply delivered to the mock sessionWebhook.
        accepted = _signed_dingtalk_post(
            gateway,
            webhook_path,
            secret,
            {
                "msgId": f"delivery-ok-{run_id}",
                "conversationId": "e2e-chat",
                "conversationType": "1",
                "senderId": "e2e-sender",
                "senderNick": "E2E",
                "msgtype": "text",
                "content": json.dumps({"content": "hello delivery"}),
                "createAt": int(time.time() * 1000),
            },
            f"{im_mock['url']}/dingtalk/session-ok",
        )
        assert accepted.status_code in (200, 202), (
            f"inbound POST {accepted.status_code} {accepted.text} path={webhook_path}"
        )

    execution_id = None
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline and execution_id is None:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT BIN_TO_UUID(execution_id) FROM application_invocations "
            f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') AND provider_event_id='delivery-ok-{run_id}';",
        )
        if raw and raw.lower() != "null":
            execution_id = raw
        time.sleep(2)
    assert execution_id, "inbound message did not create an invocation"

    # The L1 reply is enqueued in the execution's terminal transaction, so the
    # execution must complete first; surfacing its failure directly beats
    # waiting on an empty delivery outbox.
    def _execution_diag() -> str:
        return _runtime_mysql(
            installed_agentx,
            "SELECT CONCAT('machine=',COALESCE(LEFT(r.machine_state_json,300),'none'),' attempts=',"
            "COALESCE((SELECT GROUP_CONCAT(CONCAT(a.status,':',a.capability,':',COALESCE(a.error_code,'')) SEPARATOR ' ; ') "
            "FROM node_attempts a WHERE a.tenant_id=e.tenant_id AND a.execution_id=e.id),'none'),"
            "' outbox=',COALESCE((SELECT GROUP_CONCAT(CONCAT(o.message_type,':',o.status) SEPARATOR ' ; ') "
            "FROM execution_outbox o WHERE o.tenant_id=e.tenant_id AND o.execution_id=e.id),'none'),"
            "' commands=',COALESCE((SELECT GROUP_CONCAT(CONCAT(c.command_type,':',c.status) SEPARATOR ' ; ') "
            "FROM runtime_commands c WHERE c.tenant_id=e.tenant_id AND c.aggregate_id=BIN_TO_UUID(e.id)),'none')) "
            "FROM workflow_executions e JOIN execution_runtime_state r ON r.tenant_id=e.tenant_id "
            f"AND r.execution_id=e.id WHERE e.tenant_id=UUID_TO_BIN('{me['companyId']}') "
            f"AND e.id=UUID_TO_BIN('{execution_id}');",
        )

    terminal = ""
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        terminal = _runtime_mysql(
            installed_agentx,
            "SELECT CONCAT(e.status,'|',COALESCE(e.error_code,''),'|',LEFT(COALESCE(e.error_message,''),200),"
            "'|',LEFT(COALESCE(e.output_json,''),200)) FROM workflow_executions e "
            f"WHERE e.tenant_id=UUID_TO_BIN('{me['companyId']}') AND e.id=UUID_TO_BIN('{execution_id}');",
        )
        if terminal and not terminal.startswith(("queued|", "running|", "waiting|", "suspended|")):
            break
        time.sleep(2)
    if not terminal.startswith("succeeded|"):
        worker_logs = run(
            (
                "kubectl",
                "-n",
                installed_agentx["runtime_namespace"],
                "logs",
                "deployment/workflow-worker",
                "--tail=300",
            ),
            timeout=120,
        ).stdout[-3000:]
        pytest.fail(
            f"inbound execution did not succeed: {terminal} diag=[{_execution_diag()}] worker_logs={worker_logs}"
        )

    delivered: list[dict[str, Any]] = []
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        delivered = [
            row
            for row in _delivery_rows(installed_agentx, me["companyId"], execution_id)
            if row["status"] == "delivered"
        ]
        if delivered:
            break
        time.sleep(3)
    if not delivered:
        binding_state = _runtime_mysql(
            installed_agentx,
            f"SELECT CONCAT(status, '|', COALESCE(reply_config_json, '')) FROM webhook_bindings WHERE public_id='{public_id}';",
        )
        trigger_context = _runtime_mysql(
            installed_agentx,
            f"SELECT LEFT(COALESCE(trigger_context_json, ''), 400) FROM application_invocations "
            f"WHERE execution_id=UUID_TO_BIN('{execution_id}');",
        )
        worker_logs = run(
            (
                "kubectl",
                "-n",
                installed_agentx["runtime_namespace"],
                "logs",
                "deployment/workflow-runtime",
                "--tail=400",
            ),
            timeout=120,
        ).stdout[-4000:]
        pytest.fail(
            "L1 delivery did not reach delivered: "
            f"rows={_delivery_rows(installed_agentx, me['companyId'], execution_id)} "
            f"terminal={terminal} binding={binding_state} context={trigger_context} logs={worker_logs}"
        )
    assert delivered[0]["providerMessageId"], delivered
    deadline = time.monotonic() + 60
    received: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        received = _mock_received(deps_ns, "dingtalk/session-ok")
        if any(item["body"].get("text", {}).get("content") == "hello delivery" for item in received):
            break
        time.sleep(2)
    assert received, "im-mock did not record the outbound reply"

    # 2. permanent failure path: 401 mock endpoint → dead letter → replay ok.
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        failing = _signed_dingtalk_post(
            gateway,
            webhook_path,
            secret,
            {
                "msgId": f"delivery-dead-{run_id}",
                "conversationId": "e2e-chat",
                "conversationType": "1",
                "senderId": "e2e-sender",
                "senderNick": "E2E",
                "msgtype": "text",
                "content": json.dumps({"content": "dead letter probe"}),
                "createAt": int(time.time() * 1000),
            },
            f"{im_mock['url']}/dingtalk/session-unauthorized",
        )
        assert failing.status_code in (200, 202), f"{failing.status_code} {failing.text}"
    dead_execution: str | None = None
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and dead_execution is None:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT BIN_TO_UUID(execution_id) FROM application_invocations "
            f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') AND provider_event_id='delivery-dead-{run_id}';",
        )
        if raw and raw.lower() != "null":
            dead_execution = raw
        time.sleep(2)
    assert dead_execution, "dead-letter invocation not created"
    # dead() archives the row into delivery_dead_letters and removes it from
    # the outbox, so the dead letter table is the authoritative place to look.
    deadline = time.monotonic() + 180
    dead_row: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT JSON_OBJECT('id',BIN_TO_UUID(id),'provider',provider,'origin',origin,"
            "'attemptCount',attempt_count,'lastErrorCode',last_error_code) FROM delivery_dead_letters "
            f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') AND execution_id=UUID_TO_BIN('{dead_execution}');",
        )
        dead_row = json.loads(raw) if raw and raw.lower() != "null" else None
        if dead_row:
            break
        time.sleep(3)
    assert dead_row, "unauthorized delivery did not reach the dead letter archive"
    assert dead_row["lastErrorCode"] == "DELIVERY_PROVIDER_REJECTED", dead_row
