# ruff: noqa: S608 -- all interpolated identifiers belong to the isolated E2E API.
"""Real OpenSandbox limits, network denial, timeout and cancellation cleanup."""

from __future__ import annotations

import json
import time

import pytest

from tests.e2e.product.live_text_support import post
from tests.e2e.product.remaining_support import authorize, get, invoke, reference, report, save_version, start_execution
from tests.e2e.product.test_provider_integration import _sandbox_image_digest
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def code_workflow(state, suffix, source, *, ttl=120, output_example=None):
    control = state["control"]
    profile = post(
        control,
        "/sandbox-profiles",
        {
            "name": f"Boundary sandbox {suffix} {state['runId']}",
            "description": "CPU and memory bounds with deny-all network",
            "ownerDepartmentId": state["me"]["departmentId"],
            "runner": "python",
            "imageDigest": _sandbox_image_digest(),
            "cpuMillis": 500,
            "memoryBytes": 536870912,
            "pidsLimit": 512,
            "diskBytes": 1073741824,
            "timeoutSeconds": ttl,
            "outputLimitBytes": 1048576,
            "networkPolicy": {"defaultAction": "deny", "egressMode": "none"},
        },
    )
    workflow = post(
        control, "/workflows", {"name": f"Boundary Python {suffix} {state['runId']}", "visibility": "company"}
    )["id"]
    authorize(control, workflow, reference("sandbox_profile", profile["id"]))
    draft = get(control, f"/workflows/{workflow}/draft")
    definition = draft["definition"]
    definition["start"]["inputs"] = {
        "type": "object",
        "properties": {"message": {"type": "string"}},
        "required": ["message"],
        "additionalProperties": False,
    }
    code = {
        "id": "python",
        "key": "python",
        "type": "code",
        "typeVersion": 1,
        "name": f"Boundary Python {suffix}",
        "parameters": {
            "runner": "python",
            "inputs": {"kind": "object", "fields": {}},
            "source": source,
            "outputExample": output_example or {"ok": True},
            "networkPolicy": {"mode": "deny", "destinations": []},
        },
        "contextWrites": [],
        "resourceReferences": [reference("sandbox_profile", profile["id"])],
        "settings": {"timeoutMs": 180000},
    }
    definition["nodes"].append(code)
    exit_node = next(n for n in definition["nodes"] if n["type"] == "exit")
    exit_node["parameters"]["outputs"] = {
        "answer": {
            "kind": "reference",
            "selector": {
                "namespace": "outputs",
                "sourceNodeId": "python",
                "port": "main",
                "run": {"kind": "current"},
                "item": {"kind": "first"},
                "path": ["structuredOutput"],
            },
            "missingPolicy": {"kind": "error"},
        }
    }
    definition["end"] = {
        "completion": "first_return",
        "outputs": {"answer": {"schema": {"type": "object"}, "required": True, "sensitive": False}},
        "error": {"outputs": {}},
    }
    definition["connections"] = [
        {
            "id": "start-python",
            "sourceNodeId": "__start__",
            "sourceHandle": "main",
            "targetNodeId": "python",
            "targetHandle": "main",
            "order": 0,
        },
        {
            "id": "python-end",
            "sourceNodeId": "python",
            "sourceHandle": "main",
            "targetNodeId": exit_node["id"],
            "targetHandle": "main",
            "order": 0,
        },
    ]
    save_version(control, workflow, definition)
    return workflow


def wait_cleanup(state, execution):
    deadline = time.monotonic() + 90
    while True:
        count = int(
            _runtime_mysql(
                state["context"],
                f"SELECT COUNT(*) FROM sandbox_leases WHERE execution_id=UUID_TO_BIN('{execution}') AND status<>'terminated';",
            )
        )
        if count == 0:
            return
        assert time.monotonic() < deadline, f"Sandbox cleanup did not converge: {execution}"
        time.sleep(1)


def test_real_python_cpu_memory_and_deny_network_policy(boundary_state):
    state = boundary_state
    # The host lifecycle endpoint is reachable before the sandbox policy is
    # tested. DNS failure alone is not accepted as proof of network denial.
    import httpx

    assert httpx.get("http://127.0.0.1:18080/health", timeout=5).status_code == 200
    baseline = json.loads(
        run(
            (
                "docker",
                "run",
                "--rm",
                "--add-host",
                "host.docker.internal:host-gateway",
                "--entrypoint",
                "/usr/bin/python3",
                _sandbox_image_digest(),
                "-c",
                "import json,socket,urllib.request; address=socket.gethostbyname('host.docker.internal'); status=urllib.request.urlopen('http://'+address+':18080/health',timeout=5).status; print(json.dumps({'address':address,'status':status}))",
            ),
            timeout=60,
        ).stdout
    )
    assert baseline["status"] == 200, baseline
    source = """def main(**inputs):
    import pathlib, socket
    root = pathlib.Path('/sys/fs/cgroup')
    cpu = (root / 'cpu.max').read_text().strip()
    memory = (root / 'memory.max').read_text().strip()
    pids = (root / 'pids.max').read_text().strip()
    address = ADDRESS_LITERAL
    denied = False
    try:
        with socket.create_connection((address, 18080), timeout=2):
            pass
    except OSError:
        denied = True
    return {'ok': True, 'cpu': cpu, 'memory': memory, 'pids': pids, 'networkDenied': denied, 'resolvedAddress': address}
""".replace("ADDRESS_LITERAL", json.dumps(baseline["address"]))
    workflow = code_workflow(
        state,
        "limits",
        source,
        output_example={
            "ok": True,
            "cpu": "50000 100000",
            "memory": "536870912",
            "pids": "512",
            "networkDenied": True,
            "resolvedAddress": baseline["address"],
        },
    )
    execution, _ = invoke(state, workflow)
    node = next(
        n
        for n in get(state["control"], f"/executions/{execution}/nodes")["items"]
        if n["nodeName"] == "Boundary Python limits"
    )
    value = node["output"]["main"][0]["json"]["structuredOutput"]
    quota, period = value["cpu"].split()
    assert int(quota) / int(period) == 0.5, value
    assert int(value["memory"]) == 536870912, value
    assert int(value["pids"]) == 512, value
    assert value["networkDenied"] is True, value
    wait_cleanup(state, execution)
    report(
        state,
        "remaining-sandbox-limits",
        {"executionId": execution, "observed": value, "unrestrictedNetworkBaseline": baseline, "cleanupComplete": True},
    )


def test_real_python_provider_timeout_terminates_sandbox(boundary_state):
    state = boundary_state
    workflow = code_workflow(
        state, "timeout", "def main(**inputs):\n    import time\n    time.sleep(120)\n    return {'ok': True}\n", ttl=60
    )
    execution, detail = invoke(state, workflow, expected="failed")
    nodes = get(state["control"], f"/executions/{execution}/nodes")["items"]
    assert any(n.get("errorCode") for n in nodes), nodes
    wait_cleanup(state, execution)
    report(
        state,
        "remaining-sandbox-timeout",
        {
            "executionId": execution,
            "executionStatus": detail["status"],
            "errorCodes": [n["errorCode"] for n in nodes if n.get("errorCode")],
            "cleanupComplete": True,
        },
    )


def test_real_python_cancel_interrupts_and_releases_lease(boundary_state):
    state = boundary_state
    workflow = code_workflow(
        state, "cancel", "def main(**inputs):\n    import time\n    time.sleep(120)\n    return {'ok': True}\n"
    )
    response = start_execution(state, workflow, {"message": "cancel"})
    assert response.status_code == 202, response.text
    execution = response.json()["executionId"]
    deadline = time.monotonic() + 90
    while (
        _runtime_mysql(
            state["context"],
            f"SELECT COUNT(*) FROM sandbox_leases WHERE execution_id=UUID_TO_BIN('{execution}') AND status IN ('running','interrupting');",
        )
        == "0"
    ):
        assert time.monotonic() < deadline
        time.sleep(0.5)
    cancel_started = time.monotonic()
    cancelled = state["control"].post(f"/api/v1/executions/{execution}/cancel", json={})
    assert cancelled.status_code == 200 and cancelled.json()["status"] == "accepted", cancelled.text
    while get(state["control"], f"/executions/{execution}")["status"] != "cancelled":
        assert time.monotonic() < deadline
        time.sleep(0.5)
    wait_cleanup(state, execution)
    elapsed = time.monotonic() - cancel_started
    assert elapsed < 30, f"Cancellation must interrupt the 120 second program promptly: {elapsed}"
    report(
        state,
        "remaining-sandbox-cancel",
        {"executionId": execution, "cancelled": True, "cleanupComplete": True, "cancelToCleanupSeconds": elapsed},
    )
