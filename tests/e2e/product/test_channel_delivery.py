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

    # 5b. dead letter replay: the mock's unauthorized behavior is transient
    # (401 once), so replaying the archived delivery now succeeds.
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        replay = control.post(
            f"/api/v1/deliveries/{dead_row['id']}/retry",
            headers={"Authorization": f"Bearer {token}"},
        )
        assert replay.status_code in (200, 202), f"{replay.status_code} {replay.text}"
    replayed: dict[str, Any] | None = None
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        rows = _delivery_rows(installed_agentx, me["companyId"], dead_execution)
        replayed = next((row for row in rows if row["status"] == "delivered"), None)
        if replayed:
            break
        time.sleep(3)
    assert replayed, (
        f"dead letter replay did not deliver: {_delivery_rows(installed_agentx, me['companyId'], dead_execution)}"
    )

    # 3. transient 429 → exponential backoff retry → delivered with attempts.
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        throttled = _signed_dingtalk_post(
            gateway,
            webhook_path,
            secret,
            {
                "msgId": f"delivery-throttle-{run_id}",
                "conversationId": "e2e-chat",
                "conversationType": "1",
                "senderId": "e2e-sender",
                "senderNick": "E2E",
                "msgtype": "text",
                "content": json.dumps({"content": "throttle probe"}),
                "createAt": int(time.time() * 1000),
            },
            f"{im_mock['url']}/dingtalk/session-rate-limit",
        )
        assert throttled.status_code in (200, 202), f"{throttled.status_code} {throttled.text}"
    throttle_execution: str | None = None
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and throttle_execution is None:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT BIN_TO_UUID(execution_id) FROM application_invocations "
            f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') AND provider_event_id='delivery-throttle-{run_id}';",
        )
        if raw and raw.lower() != "null":
            throttle_execution = raw
        time.sleep(2)
    assert throttle_execution, "throttled invocation not created"
    throttled_row: dict[str, Any] | None = None
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        rows = _delivery_rows(installed_agentx, me["companyId"], throttle_execution)
        throttled_row = next((row for row in rows if row["status"] == "delivered"), None)
        if throttled_row:
            break
        time.sleep(3)
    assert throttled_row, (
        "rate-limited delivery did not recover: "
        f"{_delivery_rows(installed_agentx, me['companyId'], throttle_execution)}"
    )
    # The mock fails the first two hits (429), so a delivered row proves the
    # backoff loop retried: attemptCount >= 3 and the error was cleared.
    assert throttled_row["attemptCount"] >= 3, throttled_row
    assert throttled_row["lastErrorCode"] is None, throttled_row
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if any(item["behavior"] == "ok" for item in _mock_received(deps_ns, "dingtalk/session-rate-limit")):
            break
        time.sleep(2)
    assert any(item["behavior"] == "ok" for item in _mock_received(deps_ns, "dingtalk/session-rate-limit")), (
        "im-mock did not record the recovered throttle delivery"
    )


def test_l2_reply_node_delivers_to_trigger_conversation(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    im_mock: dict[str, str],
    run_id: str,
) -> None:
    """plan7 P7-A A7 scenario 2: the reply_message node enqueues its delivery
    at attempt settlement (origin node:*) independently of the channel's L1
    auto-reply, which stays disabled here to isolate the two paths."""
    deps_ns = installed_agentx["dependencies_namespace"]
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        # start(message) → reply_message(literal content) → exit
        created = control.post(
            "/api/v1/workflows",
            headers=headers,
            json={"name": f"L2 Reply E2E {run_id}", "description": "P7-A L2", "visibility": "company"},
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
        definition["nodes"].append(
            {
                "id": "reply",
                "key": "reply",
                "type": "reply_message",
                "typeVersion": 1,
                "name": "Reply",
                "disabled": False,
                "parameters": {"content": "node-level reply"},
                "contextWrites": [],
                "resourceReferences": [],
                "settings": {},
            }
        )
        definition["end"] = {"completion": "first_return", "outputs": {}, "error": {"outputs": {}}}
        definition["connections"] = [
            {
                "id": "start-reply",
                "sourceNodeId": "__start__",
                "sourceHandle": "main",
                "targetNodeId": "reply",
                "targetHandle": "main",
                "order": 0,
            },
            {
                "id": "reply-exit",
                "sourceNodeId": "reply",
                "sourceHandle": "main",
                "targetNodeId": exit_node["id"],
                "targetHandle": "main",
                "order": 0,
            },
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
        version_id = published.json()["id"]

        environment_id = _development_environment_id(control, headers)
        deploy = control.post(
            f"/api/v1/workflows/{workflow_id}/deployments",
            headers=headers,
            json={"environmentId": environment_id, "workflowVersionId": version_id},
        )
        assert deploy.status_code in (200, 201), deploy.text
        application = control.post(
            "/api/v1/applications",
            headers=headers,
            json={
                "workflowId": workflow_id,
                "name": f"L2 Reply App {run_id}",
                "slug": f"l2-reply-{run_id}",
                "visibility": "company",
            },
        )
        assert application.status_code in (200, 201), application.text
        application_id = application.json()["id"]
        channel = _dingtalk_channel(control, headers, application_id, reply_enabled=False)
        _publish_application(
            control, headers, {"workflowId": workflow_id, "versionId": version_id}, application_id, environment_id
        )

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
    assert not _binding_state().startswith("0:"), f"webhook binding never reached runtime: {_binding_state()}"

    event_id = f"l2-reply-{run_id}"
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        accepted = _signed_dingtalk_post(
            gateway,
            webhook_path,
            secret,
            {
                "msgId": event_id,
                "conversationId": "e2e-chat",
                "conversationType": "1",
                "senderId": "e2e-sender",
                "senderNick": "E2E",
                "msgtype": "text",
                "content": json.dumps({"content": "trigger the reply node"}),
                "createAt": int(time.time() * 1000),
            },
            f"{im_mock['url']}/dingtalk/session-ok",
        )
        assert accepted.status_code in (200, 202), f"{accepted.status_code} {accepted.text}"

    execution_id: str | None = None
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and execution_id is None:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT BIN_TO_UUID(execution_id) FROM application_invocations "
            f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') AND provider_event_id='{event_id}';",
        )
        if raw and raw.lower() != "null":
            execution_id = raw
        time.sleep(2)
    assert execution_id, "L2 invocation not created"

    node_row: dict[str, Any] | None = None
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        rows = _delivery_rows(installed_agentx, me["companyId"], execution_id)
        node_row = next(
            (row for row in rows if row["status"] == "delivered" and (row["origin"] or "").startswith("node:")),
            None,
        )
        if node_row:
            break
        time.sleep(3)
    assert node_row, (
        f"reply node delivery not delivered: {_delivery_rows(installed_agentx, me['companyId'], execution_id)}"
    )
    assert node_row["providerMessageId"], node_row

    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        received = _mock_received(deps_ns, "dingtalk/session-ok")
        if any(item["body"].get("text", {}).get("content") == "node-level reply" for item in received):
            break
        time.sleep(2)
    assert any(
        item["body"].get("text", {}).get("content") == "node-level reply"
        for item in _mock_received(deps_ns, "dingtalk/session-ok")
    ), "im-mock did not record the node-level reply"


def _channel(
    control: httpx.Client,
    headers: dict[str, str],
    application_id: str,
    provider_type: str,
    channel_mode: str,
    channel_config: dict[str, Any],
) -> dict[str, Any]:
    created = control.post(
        f"/api/v1/applications/{application_id}/webhooks",
        headers=headers,
        json={
            "name": f"{provider_type} {channel_mode}",
            "providerType": provider_type,
            "channelMode": channel_mode,
            "channelConfig": channel_config,
            "inputMappings": [{"source": "message.text", "target": "message", "missingPolicy": "error"}],
            "fixedInputs": {},
        },
    )
    assert created.status_code in (200, 201), created.text
    return created.json()


def test_l3_send_message_idempotency_and_pod_crash_resilience(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    im_mock: dict[str, str],
    run_id: str,
) -> None:
    """plan7 P7-A A7 scenarios 3/7/8: send_message over the official robot
    API, duplicate inbound idempotency, and delivery survival across a
    workflow-runtime pod crash mid-send."""
    rt_ns = installed_agentx["runtime_namespace"]
    deps_ns = installed_agentx["dependencies_namespace"]
    # Point the official robot API at the mock for this isolated namespace.
    run(
        (
            "kubectl",
            "-n",
            rt_ns,
            "set",
            "env",
            "deployment/workflow-runtime",
            f"AGENTX_DELIVERY_DINGTALK_API_BASE={im_mock['url']}/dingtalk",
        ),
        timeout=120,
    )
    run(("kubectl", "-n", rt_ns, "rollout", "status", "deployment/workflow-runtime", "--timeout=300s"), timeout=330)

    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        created = control.post(
            "/api/v1/workflows",
            headers=headers,
            json={"name": f"L3 Send E2E {run_id}", "description": "P7-A L3", "visibility": "company"},
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
        application = control.post(
            "/api/v1/applications",
            headers=headers,
            json={
                "workflowId": workflow_id,
                "name": f"L3 Send App {run_id}",
                "slug": f"l3-send-{run_id}",
                "visibility": "company",
            },
        )
        assert application.status_code in (200, 201), application.text
        application_id = application.json()["id"]
        trigger_channel = _channel(
            control,
            headers,
            application_id,
            "dingtalk",
            "callback",
            {"secret": "e2e-dingtalk-secret", "aesKey": "", "robotCode": "e2e-robot"},
        )
        send_channel = _channel(
            control,
            headers,
            application_id,
            "dingtalk",
            "stream",
            {"clientId": "e2e-client", "clientSecret": "e2e-client-secret", "robotCode": "e2e-robot"},
        )
        definition["nodes"].append(
            {
                "id": "push",
                "key": "push",
                "type": "send_message",
                "typeVersion": 1,
                "name": "Push",
                "disabled": False,
                "parameters": {
                    "content": "proactive push",
                    "channelId": send_channel["id"],
                    "senderId": "e2e-sender",
                },
                "contextWrites": [],
                "resourceReferences": [],
                "settings": {},
            }
        )
        # The crash-drill channel enables the L1 auto reply, which needs an
        # answer output to render; map it from the input like the passthrough.
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
                "id": "start-push",
                "sourceNodeId": "__start__",
                "sourceHandle": "main",
                "targetNodeId": "push",
                "targetHandle": "main",
                "order": 0,
            },
            {
                "id": "push-exit",
                "sourceNodeId": "push",
                "sourceHandle": "main",
                "targetNodeId": exit_node["id"],
                "targetHandle": "main",
                "order": 0,
            },
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
        environment_id = _development_environment_id(control, headers)
        deploy = control.post(
            f"/api/v1/workflows/{workflow_id}/deployments",
            headers=headers,
            json={"environmentId": environment_id, "workflowVersionId": published.json()["id"]},
        )
        assert deploy.status_code in (200, 201), deploy.text
        _publish_application(
            control,
            headers,
            {"workflowId": workflow_id, "versionId": published.json()["id"]},
            application_id,
            environment_id,
        )

    webhook_path = trigger_channel["path"]
    secret = "e2e-dingtalk-secret"  # noqa: S105 -- isolated E2E fixture credential
    public_id = trigger_channel["publicId"]

    def _binding_state() -> str:
        return _runtime_mysql(
            installed_agentx,
            "SELECT CONCAT(COUNT(*), ':', COALESCE(MAX(w.status),'none')) FROM webhook_bindings w "
            f"WHERE w.public_id='{public_id}';",
        )

    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and _binding_state().startswith("0:"):
        time.sleep(3)
    assert not _binding_state().startswith("0:"), f"webhook binding never reached runtime: {_binding_state()}"

    # Scenario 3: send_message over the official robot API (token → direct send).
    event_id = f"l3-send-{run_id}"
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        accepted = _signed_dingtalk_post(
            gateway,
            webhook_path,
            secret,
            {
                "msgId": event_id,
                "conversationId": "e2e-chat",
                "conversationType": "1",
                "senderId": "e2e-sender",
                "senderNick": "E2E",
                "msgtype": "text",
                "content": json.dumps({"content": "trigger the push"}),
                "createAt": int(time.time() * 1000),
            },
            f"{im_mock['url']}/dingtalk/session-ok",
        )
        assert accepted.status_code in (200, 202), f"{accepted.status_code} {accepted.text}"

        # Scenario 7: replaying the identical inbound event is idempotent.
        duplicate = _signed_dingtalk_post(
            gateway,
            webhook_path,
            secret,
            {
                "msgId": event_id,
                "conversationId": "e2e-chat",
                "conversationType": "1",
                "senderId": "e2e-sender",
                "senderNick": "E2E",
                "msgtype": "text",
                "content": json.dumps({"content": "trigger the push"}),
                "createAt": int(time.time() * 1000),
            },
            f"{im_mock['url']}/dingtalk/session-ok",
        )
        assert duplicate.status_code in (200, 202), f"{duplicate.status_code} {duplicate.text}"

    execution_id: str | None = None
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and execution_id is None:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT BIN_TO_UUID(execution_id) FROM application_invocations "
            f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') AND provider_event_id='{event_id}';",
        )
        if raw and raw.lower() != "null":
            execution_id = raw
        time.sleep(2)
    assert execution_id, "L3 invocation not created"

    send_row: dict[str, Any] | None = None
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        rows = _delivery_rows(installed_agentx, me["companyId"], execution_id)
        send_row = next(
            (row for row in rows if row["status"] == "delivered" and (row["origin"] or "").startswith("node:")),
            None,
        )
        if send_row:
            break
        time.sleep(3)
    assert send_row, (
        f"send_message delivery not delivered: {_delivery_rows(installed_agentx, me['companyId'], execution_id)}"
    )
    assert send_row["providerMessageId"] == "dt-official-pqk", send_row
    assert len(_delivery_rows(installed_agentx, me["companyId"], execution_id)) == 1, (
        "duplicate event produced extra deliveries"
    )

    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if any("batchSend" in item["path"] for item in _mock_received(deps_ns, "/dingtalk/")):
            break
        time.sleep(2)
    assert any(
        "batchSend" in item["path"] and (item["body"].get("msgParam") or {}).get("content") == "proactive push"
        for item in _mock_received(deps_ns, "/dingtalk/")
    ), "im-mock did not record the official-API push"

    # Scenario 8: crash the delivery loop mid-send; the expired lease is
    # requeued and the delivery completes on the replacement pod. The drill
    # uses the L1 auto-reply path (the only one carrying a sessionWebhook),
    # so the app gets a second callback channel with replies enabled.
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        crash_channel = _dingtalk_channel(
            control, {"Authorization": f"Bearer {token}"}, application_id, reply_enabled=True
        )
        # A new channel bumps the trigger manifest revision; republish so the
        # runtime activates the fresh binding set.
        republish = None
        for _ in range(40):
            republish = control.post(
                f"/api/v1/applications/{application_id}/deployments",
                headers={"Authorization": f"Bearer {token}"},
                json={
                    "workflowVersionId": published.json()["id"],
                    "environmentId": environment_id,
                    "sessionVersionPolicy": "pinned",
                },
            )
            if republish.status_code == 202:
                break
            time.sleep(3)
        assert republish is not None and republish.status_code == 202, getattr(republish, "text", "")
    crash_public = crash_channel["publicId"]
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT COUNT(*) FROM webhook_bindings WHERE public_id='" + crash_public + "';",
        )
        if raw and raw != "0":
            break
        time.sleep(3)
    else:
        raise AssertionError("crash-drill channel binding never reached runtime")
    kill_event = f"l3-kill-{run_id}"
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        slow = _signed_dingtalk_post(
            gateway,
            crash_channel["path"],
            secret,
            {
                "msgId": kill_event,
                "conversationId": "e2e-chat",
                "conversationType": "1",
                "senderId": "e2e-sender",
                "senderNick": "E2E",
                "msgtype": "text",
                "content": json.dumps({"content": "crash mid delivery"}),
                "createAt": int(time.time() * 1000),
            },
            f"{im_mock['url']}/dingtalk/session-slow",
        )
        assert slow.status_code in (200, 202), f"{slow.status_code} {slow.text}"
    kill_execution: str | None = None
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and kill_execution is None:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT BIN_TO_UUID(execution_id) FROM application_invocations "
            f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') AND provider_event_id='{kill_event}';",
        )
        if raw and raw.lower() != "null":
            kill_execution = raw
        time.sleep(2)
    assert kill_execution, "crash-drill invocation not created"
    deadline = time.monotonic() + 120
    seen_delivering = False
    while time.monotonic() < deadline:
        rows = _delivery_rows(installed_agentx, me["companyId"], kill_execution)
        if any(row["status"] == "delivering" for row in rows):
            seen_delivering = True
            break
    if not seen_delivering:
        execution_status = _runtime_mysql(
            installed_agentx,
            "SELECT CONCAT(e.status,'|',COALESCE(e.error_code,''),'|',LEFT(COALESCE(e.error_message,''),300)) "
            f"FROM workflow_executions e WHERE e.id=UUID_TO_BIN('{kill_execution}');",
        )
        dead_letters = _runtime_mysql(
            installed_agentx,
            "SELECT COALESCE(GROUP_CONCAT(CONCAT(last_error_code,':',LEFT(COALESCE(last_error_message,''),120))),'none') "
            f"FROM delivery_dead_letters WHERE execution_id=UUID_TO_BIN('{kill_execution}');",
        )
        pytest.fail(
            "delivery never entered in-flight state for the crash drill: "
            f"rows={_delivery_rows(installed_agentx, me['companyId'], kill_execution)} "
            f"execution={execution_status} dead={dead_letters}"
        )
    pod = run(
        (
            "kubectl",
            "-n",
            rt_ns,
            "get",
            "pods",
            "-l",
            "app.kubernetes.io/name=workflow-runtime",
            "-o",
            "jsonpath={.items[0].metadata.name}",
        ),
        timeout=60,
    ).stdout.strip()
    run(("kubectl", "-n", rt_ns, "delete", "pod", pod, "--grace-period=0"), timeout=120)
    run(("kubectl", "-n", rt_ns, "rollout", "status", "deployment/workflow-runtime", "--timeout=300s"), timeout=330)

    kill_row: dict[str, Any] | None = None
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        rows = _delivery_rows(installed_agentx, me["companyId"], kill_execution)
        kill_row = next((row for row in rows if row["status"] == "delivered"), None)
        if kill_row:
            break
        time.sleep(3)
    assert kill_row, (
        "in-flight delivery was lost across the pod crash: "
        f"{_delivery_rows(installed_agentx, me['companyId'], kill_execution)}"
    )


def test_reply_node_fails_without_im_trigger(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    run_id: str,
) -> None:
    """plan7 P7-A A7 scenario 6: a reply_message node outside an IM trigger
    fails deterministically with REPLY_TARGET_UNRESOLVED."""
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, _me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        created = control.post(
            "/api/v1/workflows",
            headers=headers,
            json={"name": f"L2 NoTrigger E2E {run_id}", "description": "P7-A scenario 6", "visibility": "company"},
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
        definition["nodes"].append(
            {
                "id": "reply",
                "key": "reply",
                "type": "reply_message",
                "typeVersion": 1,
                "name": "Reply",
                "disabled": False,
                "parameters": {"content": "should not deliver"},
                "contextWrites": [],
                "resourceReferences": [],
                "settings": {},
            }
        )
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
                "id": "start-reply",
                "sourceNodeId": "__start__",
                "sourceHandle": "main",
                "targetNodeId": "reply",
                "targetHandle": "main",
                "order": 0,
            },
            {
                "id": "reply-exit",
                "sourceNodeId": "reply",
                "sourceHandle": "main",
                "targetNodeId": exit_node["id"],
                "targetHandle": "main",
                "order": 0,
            },
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
        version_id = published.json()["id"]
        environment_id = _development_environment_id(control, headers)
        deploy = control.post(
            f"/api/v1/workflows/{workflow_id}/deployments",
            headers=headers,
            json={"environmentId": environment_id, "workflowVersionId": version_id},
        )
        assert deploy.status_code in (200, 201), deploy.text
        application = control.post(
            "/api/v1/applications",
            headers=headers,
            json={
                "workflowId": workflow_id,
                "name": f"L2 NoTrigger App {run_id}",
                "slug": f"l2-notrigger-{run_id}",
                "visibility": "company",
            },
        )
        assert application.status_code in (200, 201), application.text
        application_id = application.json()["id"]
        _publish_application(
            control, headers, {"workflowId": workflow_id, "versionId": version_id}, application_id, environment_id
        )
        # Give the application a chat mapping so it can be invoked without a
        # webhook trigger context.
        deployments = control.get(f"/api/v1/applications/{application_id}/deployments", headers=headers)
        deployments.raise_for_status()
        latest_deployment = deployments.json()[0]
        mapping = control.put(
            f"/api/v1/applications/{application_id}/deployments/{latest_deployment['id']}/playground-config",
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
                f"/api/v1/applications/{application_id}/deployments/{latest_deployment['id']}/playground-config",
                headers=headers,
            )
            config.raise_for_status()
            if config.json().get("publishStatus") == "active":
                break
            time.sleep(2)
        else:
            raise AssertionError("playground mapping did not publish")
        detail = control.get(f"/api/v1/applications/{application_id}", headers=headers)
        detail.raise_for_status()
        slug = detail.json()["slug"]

    with httpx.Client(base_url=service_urls["runtime"], timeout=90) as gateway:
        session = gateway.post(
            f"/gateway/v1/applications/{slug}/sessions",
            headers={"Authorization": f"Bearer {token}", "Idempotency-Key": f"notrigger-session-{time.time_ns()}"},
            json={"title": None, "externalUserId": None},
        )
        assert session.status_code == 201, session.text
        session_id = session.json()["id"]
        accepted = gateway.post(
            f"/gateway/v1/sessions/{session_id}/messages",
            headers={"Authorization": f"Bearer {token}", "Idempotency-Key": f"notrigger-invoke-{time.time_ns()}"},
            json={"parts": [{"partType": "text", "content": "no im trigger"}]},
        )
        assert accepted.status_code == 202, accepted.text
        invocation_id = accepted.json()["id"]
        deadline = time.monotonic() + 180
        status = None
        while time.monotonic() < deadline:
            probe = gateway.get(
                f"/gateway/v1/invocations/{invocation_id}",
                headers={"Authorization": f"Bearer {token}"},
            )
            probe.raise_for_status()
            status = probe.json()["status"]
            if status in {"completed", "succeeded", "failed", "cancelled"}:
                break
            time.sleep(1)
        assert status == "failed", f"non-IM invocation should fail: {status}"

    node_error = _runtime_mysql(
        installed_agentx,
        "SELECT COALESCE(error_code,'none') FROM node_attempts a "
        "JOIN application_invocations i ON i.execution_id=a.execution_id AND i.tenant_id=a.tenant_id "
        f"WHERE i.id=UUID_TO_BIN('{invocation_id}') ORDER BY a.attempt_number DESC LIMIT 1;",
    )
    assert node_error == "REPLY_TARGET_UNRESOLVED", node_error
