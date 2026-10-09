from __future__ import annotations

import subprocess
import time
import uuid
from datetime import UTC, datetime, timedelta
from typing import Any

import httpx
import pytest

from tests.e2e.support import MYSQL_ROOT_CLIENT, run

ADMIN_USERNAME = "admin"
ADMIN_PASSWORD = "agentx-e2e-admin-password"  # noqa: S105 -- fixed disposable E2E credential


def _wait_for_runtime_dispatch_quiet(installed_agentx: dict[str, str]) -> None:
    """Keep the restart test independent from dispatch work left by earlier E2E cases."""
    deadline = time.monotonic() + 60
    state = "unknown"
    while time.monotonic() < deadline:
        state = _runtime_mysql(
            installed_agentx,
            "SELECT CONCAT("
            "(SELECT COUNT(*) FROM execution_outbox WHERE status='pending' AND message_type='dispatch_node' AND available_at<=UTC_TIMESTAMP(6)),"
            "':',(SELECT COUNT(*) FROM node_attempts WHERE status IN ('queued','running')));",
        )
        if state == "0:0":
            return
        time.sleep(0.5)
    raise AssertionError(f"Runtime dispatch did not become quiet before Loop recovery test: {state}")


@pytest.mark.cluster
@pytest.mark.runtime
def test_runtime_workloads_are_available(installed_agentx: dict[str, str]) -> None:
    namespace = installed_agentx["runtime_namespace"]
    for name in ("runtime-gateway", "workflow-runtime", "workflow-worker", "sandbox-manager"):
        subprocess.run(
            ["kubectl", "-n", namespace, "rollout", "status", f"deployment/{name}", "--timeout=300s"],
            check=True,
            shell=False,
        )


@pytest.mark.cluster
@pytest.mark.runtime
def test_loop_checkpoint_recovers_after_workflow_runtime_restart(
    installed_agentx: dict[str, str],
    e2e_providers: dict[str, str],
    service_urls: dict[str, str],
    run_id: str,
) -> None:
    """Restart the coordinator with active Loop rounds and verify ordered completion."""
    with httpx.Client(base_url=service_urls["web"], timeout=60) as client:
        token = _access_token(client)
        headers = {"Authorization": f"Bearer {token}"}
        _wait_for_runtime_dispatch_quiet(installed_agentx)
        created = client.post(
            "/api/v1/workflows",
            headers=headers,
            json={
                "name": f"plan5-loop-recovery-{run_id}",
                "description": "Loop checkpoint restart E2E",
                "visibility": "company",
            },
        )
        created.raise_for_status()
        workflow_id = created.json()["id"]
        draft = client.get(f"/api/v1/workflows/{workflow_id}/draft", headers=headers)
        draft.raise_for_status()
        draft_payload = draft.json()

        dynamic_input = {
            "kind": "reference",
            "selector": {
                "namespace": "inputs",
                "run": {"kind": "current"},
                "item": {"kind": "current"},
                "path": ["items"],
            },
            "missingPolicy": {"kind": "error"},
        }
        loop_value = {
            "kind": "reference",
            "selector": {
                "namespace": "loop",
                "run": {"kind": "current"},
                "item": {"kind": "current"},
                "path": ["item", "value"],
            },
            "missingPolicy": {"kind": "error"},
        }

        def node(
            node_id: str,
            node_type: str,
            parameters: dict[str, Any],
            *,
            parent_id: str | None = None,
            settings: dict[str, Any] | None = None,
        ) -> dict[str, Any]:
            value: dict[str, Any] = {
                "id": node_id,
                "key": node_id,
                "type": node_type,
                "typeVersion": 1,
                "name": node_id,
                "disabled": False,
                "parameters": parameters,
                "contextWrites": [],
                "resourceReferences": [],
                "settings": settings or {},
            }
            if parent_id is not None:
                value["parentId"] = parent_id
            return value

        definition = {
            "schemaVersion": "8.0",
            "start": {
                "inputs": {
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {"value": {"type": "number"}},
                                "required": ["value"],
                                "additionalProperties": False,
                            },
                        }
                    },
                    "required": ["items"],
                    "additionalProperties": False,
                },
                "contexts": {},
            },
            "nodes": [
                node(
                    "loop",
                    "loop_over_items",
                    {
                        "input": dynamic_input,
                        "outputSelector": {
                            "kind": "reference",
                            "selector": {
                                "namespace": "outputs",
                                "sourceNodeId": "collect",
                                "port": "main",
                                "run": {"kind": "current"},
                                "item": {"kind": "current"},
                                "path": [],
                            },
                            "missingPolicy": {"kind": "error"},
                        },
                        "parallelism": 2,
                        "errorMode": "terminate",
                    },
                ),
                node(
                    "slow",
                    "declarative_http",
                    {
                        "method": "GET",
                        "url": {
                            "kind": "template",
                            "segments": [{"kind": "text", "text": f"{e2e_providers['echo_mcp']}/v1/plan5/delay"}],
                        },
                        "query": [],
                        "headers": [],
                    },
                    parent_id="loop",
                    settings={"timeoutMs": 30_000},
                ),
                node(
                    "collect",
                    "set",
                    {"values": {"kind": "object", "fields": {"value": loop_value}}, "keepOnlySet": True},
                    parent_id="loop",
                ),
                {
                    **node("exit", "exit", {"outputs": {}, "errorOutputs": {}}),
                    "protected": True,
                },
            ],
            "connections": [
                {
                    "id": "start-loop",
                    "sourceNodeId": "__start__",
                    "sourceHandle": "main",
                    "targetNodeId": "loop",
                    "targetHandle": "main",
                    "order": 0,
                },
                {
                    "id": "slow-collect",
                    "sourceNodeId": "slow",
                    "sourceHandle": "main",
                    "targetNodeId": "collect",
                    "targetHandle": "main",
                    "order": 0,
                },
                {
                    "id": "loop-exit",
                    "sourceNodeId": "loop",
                    "sourceHandle": "main",
                    "targetNodeId": "exit",
                    "targetHandle": "main",
                    "order": 0,
                },
            ],
            "end": {"completion": "first_return", "outputs": {}, "error": {"outputs": {}}},
            "settings": {"executionOrder": "deterministic", "activationBudget": 100},
        }
        saved = client.put(
            f"/api/v1/workflows/{workflow_id}/draft",
            headers={**headers, "Idempotency-Key": f"plan5-loop-draft-{run_id}"},
            json={
                "expectedRevision": draft_payload["revision"],
                "definition": definition,
                "editorDocument": draft_payload["editorDocument"],
            },
        )
        assert saved.status_code == 200, saved.text
        revision = saved.json()["revision"]
        input_items = [{"value": value} for value in range(8)]
        execution_id = ""
        checkpoint_rows = "0"
        checkpoint_total = "0"
        runtime_state = ""
        node_states = "none"
        execution: dict[str, Any] = {"status": "not_started"}
        for dispatch_attempt in range(2):
            started = client.post(
                f"/api/v1/workflows/{workflow_id}/debug-executions",
                headers=headers,
                json={
                    "expectedRevision": revision,
                    "mode": "full",
                    "targetNodeId": None,
                    "input": {"items": input_items},
                    "context": {},
                    "overlayIds": [],
                    "sideEffectDecisions": {},
                    "idempotencyKey": f"plan5-loop-recovery-{run_id}-{dispatch_attempt}",
                },
            )
            assert started.status_code == 202, started.text
            execution_id = started.json()["executionId"]
            checkpoint_deadline = time.monotonic() + 30
            while time.monotonic() < checkpoint_deadline:
                checkpoint_rows = _runtime_mysql(
                    installed_agentx,
                    "SELECT COUNT(*) FROM checkpoints "  # noqa: S608 -- execution_id is returned by Control in this test.
                    f"WHERE execution_id=UUID_TO_BIN('{execution_id}') "
                    "AND (payload_artifact_id IS NOT NULL OR "
                    "JSON_LENGTH(JSON_EXTRACT(payload_json,'$.machine.pending_loops'))>0);",
                )
                active_rounds = _runtime_mysql(
                    installed_agentx,
                    "SELECT COUNT(*) FROM node_executions "  # noqa: S608 -- execution_id is returned by Control in this test.
                    f"WHERE execution_id=UUID_TO_BIN('{execution_id}') "
                    "AND node_id='slow' AND status IN ('ready','running');",
                )
                if int(checkpoint_rows) > 0 and int(active_rounds) > 0:
                    break
                execution_response = client.get(f"/api/v1/executions/{execution_id}", headers=headers)
                execution_response.raise_for_status()
                execution = execution_response.json()
                if execution["status"] in {"succeeded", "failed", "cancelled", "timed_out"}:
                    break
                time.sleep(0.5)
            checkpoint_total = _runtime_mysql(
                installed_agentx,
                "SELECT COUNT(*) FROM checkpoints "  # noqa: S608 -- execution_id is returned by Control in this test.
                f"WHERE execution_id=UUID_TO_BIN('{execution_id}');",
            )
            runtime_state = _runtime_mysql(
                installed_agentx,
                "SELECT CONCAT(state_version,':',COALESCE(JSON_LENGTH(JSON_EXTRACT(machine_state_json,'$.pending_loops')),-1)) "  # noqa: S608 -- execution_id is returned by Control in this test.
                "FROM execution_runtime_state "
                f"WHERE execution_id=UUID_TO_BIN('{execution_id}');",
            )
            node_states = _runtime_mysql(
                installed_agentx,
                "SELECT COALESCE(GROUP_CONCAT(CONCAT(node_id,':',status) ORDER BY created_at SEPARATOR ','),'none') "  # noqa: S608 -- execution_id is returned by Control in this test.
                "FROM node_executions "
                f"WHERE execution_id=UUID_TO_BIN('{execution_id}');",
            )
            if int(checkpoint_rows) > 0:
                break
            if execution.get("status") == "failed" and node_states == "none" and dispatch_attempt == 0:
                _wait_for_runtime_dispatch_quiet(installed_agentx)
                continue
            break
        assert int(checkpoint_rows) > 0, (
            "Loop did not persist an active pendingLoops checkpoint: "
            f"execution={execution} checkpoints={checkpoint_total} state={runtime_state} nodes={node_states}"
        )

        namespace = installed_agentx["runtime_namespace"]
        deployment_before = run(
            ("kubectl", "-n", namespace, "get", "deployment/workflow-runtime", "-o", "json"),
            timeout=60,
        ).json()
        run(
            ("kubectl", "-n", namespace, "rollout", "restart", "deployment/workflow-runtime"),
            timeout=60,
        )
        run(
            (
                "kubectl",
                "-n",
                namespace,
                "rollout",
                "status",
                "deployment/workflow-runtime",
                "--timeout=300s",
            ),
            timeout=330,
        )
        deployment_after = run(
            ("kubectl", "-n", namespace, "get", "deployment/workflow-runtime", "-o", "json"),
            timeout=60,
        ).json()
        assert deployment_after["metadata"]["generation"] > deployment_before["metadata"]["generation"]

        execution = _wait_control_execution(client, headers, execution_id)
        assert execution["status"] == "succeeded", execution
        nodes = client.get(f"/api/v1/executions/{execution_id}/nodes", headers=headers)
        nodes.raise_for_status()
        loop_run = next(item for item in nodes.json()["items"] if item["nodeId"] == "loop")
        assert loop_run["output"]["main"][0]["json"]["items"] == input_items


def _runtime_mysql(installed_agentx: dict[str, str], query: str) -> str:
    return run(
        (
            "kubectl",
            "-n",
            installed_agentx["runtime_namespace"],
            "exec",
            "statefulset/runtime-mysql",
            "--",
            "sh",
            "-ec",
            f'{MYSQL_ROOT_CLIENT} '
            '--batch --skip-column-names -uroot agentx_runtime -e "$1"',
            "agentx-p3-04-query",
            query,
        ),
        timeout=60,
    ).stdout.strip()


def _deadline(seconds: int = 120) -> str:
    return (datetime.now(UTC) + timedelta(seconds=seconds)).isoformat().replace("+00:00", "Z")


def _access_token(client: httpx.Client) -> str:
    status = client.get("/api/v1/bootstrap/status")
    status.raise_for_status()
    if status.json()["required"]:
        response = client.post(
            "/api/v1/bootstrap",
            json={
                "companyName": "Agentx plan5 E2E",
                "adminUsername": ADMIN_USERNAME,
                "adminDisplayName": "Agentx E2E Admin",
                "password": ADMIN_PASSWORD,
                "locale": "zh-CN",
                "timezone": "Asia/Shanghai",
            },
        )
    else:
        response = client.post(
            "/api/v1/auth/login",
            json={"username": ADMIN_USERNAME, "password": ADMIN_PASSWORD},
        )
    response.raise_for_status()
    return str(response.json()["accessToken"])


def _wait_control_execution(
    client: httpx.Client,
    headers: dict[str, str],
    execution_id: str,
    timeout_seconds: int = 300,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    latest: dict[str, Any] = {}
    while time.monotonic() < deadline:
        response = client.get(f"/api/v1/executions/{execution_id}", headers=headers)
        if response.status_code == 200:
            latest = response.json()
            if latest["status"] in {"succeeded", "failed", "cancelled", "timed_out"}:
                return latest
        else:
            latest = {"statusCode": response.status_code, "body": response.text}
        time.sleep(1)
    raise AssertionError(f"execution did not reach a terminal state: {latest}")


def _proof(ids: dict[str, str], lease: dict[str, object], suffix: str) -> dict[str, object]:
    return {
        "apiVersion": 1,
        "tenantId": ids["tenant"],
        "executionId": ids["execution"],
        "nodeExecutionId": ids["node_execution"],
        "attemptId": ids["attempt"],
        "agentRunId": ids["agent_run"],
        "workerId": ids["worker"],
        "fencingToken": 1,
        "processSessionId": lease["processSessionId"],
        "leaseId": lease["leaseId"],
        "operationId": f"p3-04:{suffix}",
        "effectId": f"p3-04:{suffix}:effect",
        "idempotencyKey": f"p3-04:{ids['attempt']}:{suffix}",
        "deadline": _deadline(),
    }


def _mcp_process_source() -> str:
    return """import json,sys
sys.stderr.write('p3-04 diagnostic only\\n');sys.stderr.flush()
for line in sys.stdin:
    request=json.loads(line)
    method=request.get('method')
    if method=='initialize':
        result={'protocolVersion':'2025-03-26','capabilities':{'tools':{}},'serverInfo':{'name':'p3-04-stdio','version':'1'}}
    elif method=='tools/list':
        result={'tools':[{'name':'echo','description':'echo input','inputSchema':{'type':'object','properties':{'text':{'type':'string'}},'required':['text']}},{'name':'large','description':'large artifact','inputSchema':{'type':'object'}}]}
    elif method=='tools/call':
        arguments=request.get('params',{}).get('arguments',{})
        if request.get('params',{}).get('name')=='large':
            result={'content':[{'type':'text','text':'x'*70000}]}
        else:
            result={'content':[{'type':'text','text':arguments.get('text','')}]}
    else:
        continue
    sys.stdout.write(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result},separators=(',',':'))+'\\n');sys.stdout.flush()
"""


@pytest.mark.cluster
@pytest.mark.runtime
def test_stdio_mcp_process_session_uses_real_opensandbox_and_cleans_up(
    installed_agentx: dict[str, str], service_urls: dict[str, str]
) -> None:
    from tests.e2e.product.test_provider_integration import _sandbox_image_digest

    ids = {
        key: str(uuid.uuid4())
        for key in (
            "tenant",
            "workflow",
            "workflow_version",
            "execution",
            "trace",
            "node_execution",
            "attempt",
            "worker",
            "agent_run",
            "mcp_server_version",
            "sandbox_profile",
            "sandbox_profile_version",
        )
    }
    _runtime_mysql(
        installed_agentx,
        "INSERT INTO workflow_executions(id,tenant_id,workflow_id,workflow_version_id,trace_id,trigger_type,status,started_at) VALUES"
        f"(UUID_TO_BIN('{ids['execution']}'),UUID_TO_BIN('{ids['tenant']}'),UUID_TO_BIN('{ids['workflow']}'),"
        f"UUID_TO_BIN('{ids['workflow_version']}'),UUID_TO_BIN('{ids['trace']}'),'debug','running',UTC_TIMESTAMP(6));"
        "INSERT INTO node_executions(id,tenant_id,execution_id,node_id,node_key,node_name,node_type,node_version,generation,activation_slot,run_index,status,capability) VALUES"
        f"(UUID_TO_BIN('{ids['node_execution']}'),UUID_TO_BIN('{ids['tenant']}'),UUID_TO_BIN('{ids['execution']}'),"
        "'p3-04-stdio','p3_04_stdio','P3-04 stdio MCP','agent',2,0,0,0,'running','agent');"
        "INSERT INTO node_attempts(id,tenant_id,execution_id,node_execution_id,attempt_number,capability,compiler_version,manifest_version,status,idempotency_key,worker_instance_id,fencing_token,locked_until,deadline_at,started_at) VALUES"
        f"(UUID_TO_BIN('{ids['attempt']}'),UUID_TO_BIN('{ids['tenant']}'),UUID_TO_BIN('{ids['execution']}'),"
        f"UUID_TO_BIN('{ids['node_execution']}'),1,'agent','p3-04-e2e','2.0','running','p3-04:{ids['attempt']}','{ids['worker']}',1,"
        "DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 5 MINUTE),DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 5 MINUTE),UTC_TIMESTAMP(6));",
    )
    manager = service_urls["sandbox_manager"]
    start = {
        "apiVersion": 1,
        "identity": {
            "tenantId": ids["tenant"],
            "agentRunId": ids["agent_run"],
            "mcpServerVersionId": ids["mcp_server_version"],
            "sandboxProfileVersionId": ids["sandbox_profile_version"],
        },
        "executionId": ids["execution"],
        "nodeExecutionId": ids["node_execution"],
        "attemptId": ids["attempt"],
        "workerId": ids["worker"],
        "fencingToken": 1,
        "operationId": "p3-04:start",
        "effectId": "p3-04:start:effect",
        "idempotencyKey": f"p3-04:{ids['attempt']}:start",
        "command": "/usr/bin/python3",
        "args": ["-u", "-c", _mcp_process_source()],
        "environmentCredentials": [],
        "profile": {
            "resourceKind": "sandbox_profile",
            "resourceId": ids["sandbox_profile"],
            "resourceVersion": ids["sandbox_profile_version"],
            "stateEpoch": 1,
            "contentHash": "sha256:" + "0" * 64,
            "configuration": {
                "kind": "sandbox_profile",
                "provider": "opensandbox",
                "image": _sandbox_image_digest(),
                "cpuMillis": 500,
                "memoryBytes": 536870912,
                "diskBytes": 1073741824,
                "pidLimit": 256,
                "egressMode": "none",
                "maximumTtlSeconds": 120,
            },
            "objectIds": [],
        },
        "deadline": _deadline(180),
    }
    lease: dict[str, object] | None = None
    try:
        response = httpx.post(
            f"{manager}/internal/runtime/v1/sandbox-process-sessions:start",
            json=start,
            timeout=180,
        )
        assert response.status_code == 200, response.text
        started = response.json()
        lease = started["lease"]
        assert lease["status"] == "running"
        assert started["providerSandboxId"]

        def write(suffix: str, frame: dict[str, object], replay_policy: str = "safe") -> dict[str, object]:
            result = httpx.post(
                f"{manager}/internal/runtime/v1/sandbox-process-sessions/{lease['processSessionId']}:write",
                json={
                    **_proof(ids, lease, suffix),
                    "frame": frame,
                    "replayPolicy": replay_policy,
                },
                timeout=30,
            )
            result.raise_for_status()
            return result.json()

        initialize = write(
            "initialize",
            {
                "jsonrpc": "2.0",
                "id": "initialize-1",
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": {"name": "agentx-e2e", "version": "1"},
                },
            },
        )
        assert any(frame["payload"].get("id") == "initialize-1" for frame in initialize["frames"])
        write("initialized", {"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools = write("tools-list", {"jsonrpc": "2.0", "id": "tools-1", "method": "tools/list", "params": {}})
        tools_payload = next(frame["payload"] for frame in tools["frames"] if frame["payload"].get("id") == "tools-1")
        assert [tool["name"] for tool in tools_payload["result"]["tools"]] == ["echo", "large"]
        called = write(
            "tools-call",
            {
                "jsonrpc": "2.0",
                "id": "call-1",
                "method": "tools/call",
                "params": {"name": "echo", "arguments": {"text": "p3-04-opensandbox"}},
            },
        )
        call_payload = next(frame["payload"] for frame in called["frames"] if frame["payload"].get("id") == "call-1")
        assert call_payload["result"]["content"][0]["text"] == "p3-04-opensandbox"
        large = write(
            "large-call",
            {"jsonrpc": "2.0", "id": "large-1", "method": "tools/call", "params": {"name": "large", "arguments": {}}},
        )
        large_frame = next(frame for frame in large["frames"] if frame["payload"].get("id") == "large-1")
        assert large_frame["truncated"] is True
        assert len(large_frame["payload"]["artifactRefs"]) == 1

        read = httpx.post(
            f"{manager}/internal/runtime/v1/sandbox-process-sessions/{lease['processSessionId']}:read",
            json={**_proof(ids, lease, "read"), "afterSequence": 0, "maximumFrames": 100, "waitMillis": 100},
            timeout=30,
        )
        read.raise_for_status()
        frames = read.json()["frames"]
        assert any(frame["stream"] == "stderr" and frame["payload"].get("diagnostic") is True for frame in frames)
        assert all(frame["stream"] != "stderr" or "jsonrpc" not in frame["payload"] for frame in frames)
        for field, value in (
            ("tenantId", str(uuid.uuid4())),
            ("workerId", str(uuid.uuid4())),
            ("fencingToken", 2),
            ("leaseId", str(uuid.uuid4())),
        ):
            refused = httpx.post(
                f"{manager}/internal/runtime/v1/sandbox-process-sessions/{lease['processSessionId']}:read",
                json={
                    **_proof(ids, lease, f"invalid-{field}"),
                    field: value,
                    "afterSequence": 0,
                    "maximumFrames": 1,
                    "waitMillis": 0,
                },
                timeout=30,
            )
            assert refused.status_code in (401, 403, 404, 409, 422), refused.text
            assert "frames" not in refused.json(), refused.text

    finally:
        if lease is not None:
            terminate = httpx.post(
                f"{manager}/internal/runtime/v1/sandbox-process-sessions/{lease['processSessionId']}:terminate",
                json=_proof(ids, lease, "terminate"),
                timeout=60,
            )
            assert terminate.status_code == 200, terminate.text
            stale = httpx.post(
                f"{manager}/internal/runtime/v1/sandbox-process-sessions/{lease['processSessionId']}:write",
                json={
                    **_proof(ids, lease, "stale-write"),
                    "frame": {"jsonrpc": "2.0", "id": "after-close", "method": "tools/list"},
                    "replayPolicy": "safe",
                },
                timeout=30,
            )
            assert stale.status_code in (401, 403, 404, 409, 422), stale.text

        _runtime_mysql(
            installed_agentx,
            f"DELETE FROM artifact_references WHERE tenant_id=UUID_TO_BIN('{ids['tenant']}');"  # noqa: S608 -- UUID is generated by this test.
            f"DELETE FROM runtime_objects WHERE tenant_id=UUID_TO_BIN('{ids['tenant']}');"
            f"DELETE FROM sandbox_process_frames WHERE process_session_id IN (SELECT process_session_id FROM sandbox_process_sessions WHERE tenant_id=UUID_TO_BIN('{ids['tenant']}'));"
            f"DELETE FROM sandbox_process_sessions WHERE tenant_id=UUID_TO_BIN('{ids['tenant']}');"
            f"DELETE FROM node_attempts WHERE id=UUID_TO_BIN('{ids['attempt']}');"
            f"DELETE FROM node_executions WHERE id=UUID_TO_BIN('{ids['node_execution']}');"
            f"DELETE FROM workflow_executions WHERE id=UUID_TO_BIN('{ids['execution']}');",
        )
