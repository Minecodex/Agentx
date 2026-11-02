"""Formal MCP publication, authorized files and real human decisions shared by both clients."""

from __future__ import annotations

import hashlib
import json
import time
import uuid
from contextlib import contextmanager
from pathlib import Path
from urllib.parse import urlsplit

import httpx

from tests.e2e.product.live_text_support import Secret
from tests.e2e.product.next_batch_support import get
from tests.e2e.product.plan9.mcp_client import ApplicationMcpClient
from tests.e2e.product.plan9.protocol_retest import McpTaskFailure
from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.studio_builder_support import post
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run, run_playwright

TERMINAL = {"completed", "failed", "cancelled", "outcome_unknown"}


def runtime_origin(suite):
    return run(
        (
            "kubectl",
            "-n",
            suite["installed"]["runtime_namespace"],
            "get",
            "deployment/runtime-gateway",
            "-o",
            'jsonpath={.spec.template.spec.containers[0].env[?(@.name=="AGENTX_RUNTIME_PUBLIC_ORIGIN")].value}',
        )
    ).stdout.strip()


def publication(suite, case):
    control = suite["control"]
    path = f"/applications/{case['applicationId']}/mcp/publications"
    existing = get(control, path)
    if not existing:
        post(
            control,
            path,
            {
                "deploymentId": case["deploymentId"],
                "expectedRevision": 0,
                "slug": case["slug"],
                "toolName": "run_application",
                "description": f"{case['scene']} 正式业务应用。按输入契约提交，等待真实结果；审批必须由有资格的人员完成。",
                "enabled": True,
            },
        )
    deadline = time.monotonic() + 120
    while True:
        current = get(control, path)[0]
        if current["status"] == "published":
            assert current["publication"]["protocolVersion"] == "2025-11-25"
            return current["publication"]
        assert time.monotonic() < deadline, current
        time.sleep(0.5)


@contextmanager
def application_client(suite, case, label):
    published = publication(suite, case)
    key = post(suite["control"], f"/applications/{case['applicationId']}/api-keys", {"name": label})
    secret = Secret(key.pop("secret"))
    client = None
    try:
        with httpx.Client(base_url=suite["urls"]["runtime"], timeout=60, trust_env=False) as transport:
            client = ApplicationMcpClient(
                transport,
                f"/gateway/v1/mcp/applications/{published['slug']}",
                secret.value,
                urlsplit(runtime_origin(suite)).netloc,
            )
            # Key admission is asynchronous; no invocation is submitted while preparing it.
            until = time.monotonic() + 120
            while True:
                try:
                    response = transport.post(
                        client.path,
                        headers={
                            "Authorization": f"Bearer {secret.value}",
                            "Host": client.host,
                            "Accept": "application/json, text/event-stream",
                        },
                        json={
                            "jsonrpc": "2.0",
                            "id": 0,
                            "method": "initialize",
                            "params": {
                                "protocolVersion": "2025-11-25",
                                "capabilities": {},
                                "clientInfo": {"name": "admission-readiness", "version": "1"},
                            },
                        },
                    )
                except httpx.TransportError:
                    # The owned port-forward reconnects after the preceding
                    # Gateway rollout. Initialization creates no business task.
                    if time.monotonic() >= until:
                        raise
                    time.sleep(0.5)
                    continue
                if response.status_code == 200:
                    break
                assert response.status_code in (401, 404, 503) and time.monotonic() < until, (
                    f"MCP readiness: HTTP {response.status_code}"
                )
                time.sleep(0.5)
            client.initialize()
            client.tools(published["toolName"])
            yield client, published, key
    finally:
        try:
            if client:
                (suite["directory"] / f"mcp-client-{key['id']}.json").write_text(
                    json.dumps(
                        {"keyId": key["id"], "label": label, "records": client.records}, ensure_ascii=False, indent=2
                    )
                )
            response = suite["control"].post(
                f"/api/v1/applications/{case['applicationId']}/api-keys/{key['id']}/revoke"
            )
            assert response.status_code == 204, f"Test key revocation failed: HTTP {response.status_code}"
        finally:
            secret.value = ""
            if client:
                client.key = ""


def prepare_files(client, value):
    if isinstance(value, dict) and "__file" in value:
        file = value["__file"]
        content = file["content"].encode()
        response = client.client.post(
            "/gateway/v1/artifacts",
            headers={"Authorization": f"Bearer {client.key}", "Idempotency-Key": str(uuid.uuid4())},
            files={"file": (file["name"], content, file["mimeType"])},
        )
        assert response.status_code == 201, response.text
        artifact = response.json()
        assert artifact["sha256"] == hashlib.sha256(content).hexdigest()
        assert artifact["sizeBytes"] == len(content)
        return artifact
    if isinstance(value, dict):
        return {key: prepare_files(client, item) for key, item in value.items()}
    if isinstance(value, list):
        return [prepare_files(client, item) for item in value]
    return value


def crm_records(suite, reference):
    with httpx.Client(timeout=30, trust_env=False) as client:
        response = client.get(f"{suite['crmUrl']}/v1/plan9/quotes")
        response.raise_for_status()
        return [item for item in response.json() if item["reference"] == reference]


def decide(suite, case, approval_id, decision, tag):
    output = suite["directory"] / f"{tag}-approval-{approval_id}.json"
    run_playwright(
        Path(suite["installed"]["root"]),
        f"{tag}-approval-{approval_id}",
        ("tests/plan9-mcp-approval.spec.ts",),
        {
            **browser_environment(suite),
            "AGENTX_E2E_PLAN9_APPROVAL": json.dumps({"id": approval_id, "decision": decision, "scenario": case}),
            "AGENTX_E2E_PLAN9_APPROVAL_REPORT": str(output),
        },
    )
    return json.loads(output.read_text())


def settle(suite, client, task_id, case, index, *, before_writes=0, tag="mcp"):
    waits, approvals, occurrences, evidence = set(), set(), {}, []
    simultaneous = not case.get("simultaneousWaits")
    deadline = time.monotonic() + 600
    while True:
        task = client.task(task_id)
        assert task["accessBinding"]["kind"] == "mcp", task
        if case.get("simultaneousWaits") and not simultaneous:
            simultaneous = len(task["pendingInputs"]) == case["simultaneousWaits"]
        for wait in task["pendingInputs"] if simultaneous else []:
            if wait["waitId"] in waits:
                continue
            ordinal = occurrences.get(wait["nodePath"], 0)
            content = case["waitInputs"][index][wait["nodePath"]][ordinal]
            receipt = client.input(task_id, wait["waitId"], content)
            assert receipt["taskId"] == task_id
            waits.add(wait["waitId"])
            occurrences[wait["nodePath"]] = ordinal + 1
            evidence.append({"wait": wait, "submitted": content})
            if case.get("repeatNode") and ordinal == 0:
                client.call(
                    "submit_task_input",
                    {"taskId": task_id, "waitId": wait["waitId"], "content": content, "requestId": str(uuid.uuid4())},
                    error=True,
                )
        for approval in task["pendingApprovals"]:
            if approval["approvalId"] in approvals:
                continue
            if case.get("crm"):
                assert len(crm_records(suite, case["inputs"][index]["request_id"])) == before_writes, (
                    "Zero CRM writes before approval"
                )
            decision = case["approvalDecisions"][index][len(approvals)]
            evidence.append(decide(suite, case, approval["approvalId"], decision, tag))
            approvals.add(approval["approvalId"])
        if task["state"] in TERMINAL:
            if task["state"] != "completed":
                raise McpTaskFailure(task)
            assert simultaneous
            assert len(approvals) == len(case.get("approvalDecisions", [[], []])[index])
            return task, evidence
        assert time.monotonic() < deadline, task
        time.sleep(0.5)


def check_result(suite, client, case, index, task, before_writes):
    expected = case["expected"][index]
    assert {key: task["outputs"][key] for key in expected} == expected, task
    if not (case.get("fileOutputs") or case.get("generatedFields")):
        assert task["outputs"] == expected
    if case.get("crm"):
        records = crm_records(suite, case["inputs"][index]["request_id"])
        assert len(records) == before_writes + 1
        assert records[-1] == {
            "receiptId": task["outputs"]["receipt_id"],
            "customer": task["outputs"]["customer"],
            "total": task["outputs"]["total"],
            "discountPercent": task["outputs"]["discount_percent"],
            "reference": task["outputs"]["reference"],
        }
    if case.get("fileOutputs"):
        assert task["files"]
        for file in task["files"]:
            response = client.client.get(
                f"/gateway/v1/invocations/{task['taskId']}/artifacts/{file['artifactId']}",
                headers={"Authorization": f"Bearer {client.key}"},
            )
            assert response.status_code == 200, response.text
            assert hashlib.sha256(response.content).hexdigest() == file["sha256"]
            if case.get("downloadText"):
                assert response.text == case["downloadText"][index]
            else:
                assert task["outputs"]["receipt_id"] in response.text
    proof = _runtime_mysql(
        suite["installed"],
        f"SELECT HEX(CAST(JSON_OBJECT('invocationId',LOWER(HEX(i.id)),'callerType',i.caller_type,'callerId',LOWER(HEX(i.caller_id)),'executionId',LOWER(HEX(i.execution_id)),'bundleId',LOWER(HEX(e.bundle_id)),'state',e.status,'input',i.input_json) AS BINARY)) FROM application_invocations i JOIN workflow_executions e ON e.tenant_id=i.tenant_id AND e.id=i.execution_id WHERE i.id=UNHEX('{uuid.UUID(task['taskId']).hex}')",  # noqa: S608 -- the only interpolated value is parsed as a UUID and encoded as 32 hexadecimal digits.
    )
    proof = json.loads(bytes.fromhex(proof))
    check_submitted_input(proof["input"], case["inputs"][index])
    return proof


def check_submitted_input(actual, expected):
    if isinstance(expected, dict) and "__file" in expected:
        file = expected["__file"]
        content = file["content"].encode()
        assert actual["contentType"] == file["mimeType"]
        assert actual["sizeBytes"] == len(content)
        assert actual["sha256"] == hashlib.sha256(content).hexdigest()
    elif isinstance(expected, dict):
        for key, value in expected.items():
            check_submitted_input(actual[key], value)
    elif isinstance(expected, list):
        assert len(actual) == len(expected)
        for value, original in zip(actual, expected, strict=True):
            check_submitted_input(value, original)
    else:
        assert actual == expected, "The caller must preserve the submitted business value"
