"""Real service outages and transport faults retain honest errors and recover."""

from __future__ import annotations

import time

import httpx
import pytest

from tests.e2e.product.live_text_support import post
from tests.e2e.product.remaining_support import (
    agent_definition,
    authorize,
    get,
    invoke,
    model_workflow,
    new_model,
    publish_chat,
    reference,
    report,
    save_version,
)
from tests.e2e.product.test_live_knowledge_memory import live_indexed_document as live_indexed_document
from tests.e2e.product.test_live_text_acceptance import message, session, terminal
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.product]


@pytest.mark.parametrize("failure", ["unavailable", "bad-key", "abort-stream", "timeout"])
def test_model_transport_faults_are_failed_and_have_call_trace(boundary_state, failure):
    state = boundary_state
    model = new_model(
        state,
        f"fault-{failure}",
        upstream={
            "unavailable": "echo-unavailable",
            "bad-key": "echo-model",
            "abort-stream": "echo-abort-stream",
            "timeout": "echo-timeout",
        }[failure],
        secret="invalid-fixture-credential" if failure == "bad-key" else None,
    )
    workflow = model_workflow(state, model, f"fault-{failure}", timeout_ms=1000 if failure == "timeout" else None)
    execution, detail = invoke(state, workflow["workflowId"], expected="failed")
    calls = get(state["control"], f"/executions/{execution}/runtime-details")["calls"]
    assert calls and any(c["status"] != "succeeded" and c.get("errorCode") for c in calls), calls
    assert not any(c["callKind"] == "model" and c["status"] == "succeeded" for c in calls), calls
    nodes = get(state["control"], f"/executions/{execution}/nodes")["items"]
    assert any(n.get("errorCode") for n in nodes), nodes
    report(
        state,
        f"remaining-model-fault-{failure}",
        {
            "executionId": execution,
            "executionStatus": detail["status"],
            "callErrors": [{"status": c["status"], "errorCode": c.get("errorCode")} for c in calls],
        },
    )


def test_real_lightrag_unavailable_restart_and_invalid_auth_are_observable(
    boundary_state, live_providers, live_indexed_document, cpu_embedding_service
):
    state = boundary_state
    control = live_providers["control"]
    namespace = cpu_embedding_service["namespace"]
    path = f"/api/v1/knowledge/resources/{live_providers['ragId']}"
    deployment = ("kubectl", "-n", namespace, "scale", "deployment/lightrag")
    run((*deployment, "--replicas=0"), timeout=60)
    run(
        (
            "kubectl",
            "-n",
            namespace,
            "wait",
            "--for=delete",
            "pod",
            "-l",
            "app.kubernetes.io/name=lightrag",
            "--timeout=120s",
        ),
        timeout=150,
    )
    try:
        failed = control.post(f"{path}/retrieval-test", json={"query": "水星计划", "topK": 5})
        assert failed.status_code in {502, 503} and failed.json().get("code"), failed.text
        marker = f"recovered-orbit-{state['runId']}"
        uploaded = control.post(
            f"{path}/documents",
            files={"file": ("restart-recovery.txt", f"The recovery release code is {marker}.".encode(), "text/plain")},
        )
        assert uploaded.status_code == 202, uploaded.text
        document = uploaded.json()
        run(
            (
                "kubectl",
                "-n",
                state["context"]["control_namespace"],
                "rollout",
                "restart",
                "deployment/platform-control",
            ),
            timeout=60,
        )
        run(
            (
                "kubectl",
                "-n",
                state["context"]["control_namespace"],
                "rollout",
                "status",
                "deployment/platform-control",
                "--timeout=300s",
            ),
            timeout=330,
        )
    finally:
        run((*deployment, "--replicas=1"), timeout=60)
        run(("kubectl", "-n", namespace, "rollout", "status", "deployment/lightrag", "--timeout=600s"), timeout=630)
    deadline = time.monotonic() + 360
    while True:
        response = control.get(f"{path}/documents")
        assert response.status_code == 200, response.text
        document = next(d for d in response.json() if d["id"] == document["id"])
        assert document["status"] != "failed", document
        if document["status"] == "indexed":
            break
        assert time.monotonic() < deadline, document
        time.sleep(1)
    recovered = control.post(f"{path}/retrieval-test", json={"query": "recovery release code", "topK": 5})
    assert recovered.status_code == 200 and marker in recovered.text, recovered.text
    wrong = post(
        control,
        "/credentials",
        {
            "name": f"Boundary wrong RAG key {state['runId']}",
            "credentialType": "bearer",
            "secret": "invalid-local-rag-key",
            "ownerDepartmentId": state["me"]["departmentId"],
        },
    )
    connection = post(
        control,
        "/knowledge/connections",
        {
            "name": f"Boundary wrong RAG {state['runId']}",
            "provider": "lightrag",
            "endpoint": live_providers["ragUrl"],
            "healthPath": "/health",
            "credentialId": wrong["id"],
            "ownerDepartmentId": state["me"]["departmentId"],
            "configuration": {},
        },
    )
    original = get(control, f"/knowledge/resources/{live_providers['ragId']}")
    resource = post(
        control,
        "/knowledge/resources",
        {
            "name": f"Boundary wrong RAG resource {state['runId']}",
            "connectionId": connection["id"],
            "externalResourceId": original["externalResourceId"],
            "ownerDepartmentId": state["me"]["departmentId"],
        },
    )
    rejected = control.post(
        f"/api/v1/knowledge/resources/{resource['id']}/retrieval-test", json={"query": "release code", "topK": 5}
    )
    assert rejected.status_code == 422 and rejected.json()["code"] == "PROVIDER_REJECTED", rejected.text
    report(
        state,
        "remaining-lightrag-recovery",
        {
            "unavailable": failed.json(),
            "documentId": document["id"],
            "controlRestartRecoveredIndex": True,
            "retrievalRecovered": True,
            "badCredentialRefused": rejected.json(),
        },
    )


def memory_application(state, live_providers, suffix, namespace_id):
    model = new_model(state, suffix)
    workflow = model_workflow(state, model, suffix)
    authorize(state["control"], workflow["workflowId"], reference("memory", namespace_id, operation="read"))
    definition = agent_definition(state["control"], workflow["workflowId"], model)
    node = next(n for n in definition["nodes"] if n["type"] == "agent")
    node["resourceReferences"].append(reference("memory", namespace_id, operation="read", role="long_term_memory"))
    version = save_version(state["control"], workflow["workflowId"], definition)
    return publish_chat(state, {**workflow, "versionId": version["id"]}, suffix)


def memory_ask(state, app):
    with httpx.Client(
        base_url=state["urls"]["runtime"], headers={"Authorization": f"Bearer {state['token'].value}"}, timeout=120
    ) as gateway:
        invocation = message(gateway, session(gateway, app, "Mem0 fault proof"), "P3_MEMORY_RECALL p3-subject-memory")
        result = terminal(gateway, invocation)
    calls = get(state["control"], f"/executions/{result['executionId']}/runtime-details")["calls"]
    return result, [c for c in calls if c["callKind"] == "memory"]


def test_real_mem0_outage_auth_failure_and_same_application_recovery(
    boundary_state, live_providers, cpu_embedding_service
):
    state = boundary_state
    app = memory_application(state, live_providers, "memory-fault", live_providers["memoryId"])
    namespace = cpu_embedding_service["namespace"]
    run(("kubectl", "-n", namespace, "scale", "deployment/mem0", "--replicas=0"), timeout=60)
    run(
        (
            "kubectl",
            "-n",
            namespace,
            "wait",
            "--for=delete",
            "pod",
            "-l",
            "app.kubernetes.io/name=mem0",
            "--timeout=120s",
        ),
        timeout=150,
    )
    try:
        failed, failed_calls = memory_ask(state, app)
        assert failed_calls and all(c["status"] == "failed" and c.get("errorCode") for c in failed_calls), failed_calls
    finally:
        run(("kubectl", "-n", namespace, "scale", "deployment/mem0", "--replicas=1"), timeout=60)
        run(("kubectl", "-n", namespace, "rollout", "status", "deployment/mem0", "--timeout=300s"), timeout=330)
    recovered, recovered_calls = memory_ask(state, app)
    assert recovered_calls and all(c["status"] == "succeeded" for c in recovered_calls), recovered_calls
    credential = post(
        state["control"],
        "/credentials",
        {
            "name": f"Boundary wrong Mem0 {state['runId']}",
            "credentialType": "bearer",
            "secret": "invalid-mem0-jwt",
            "ownerDepartmentId": state["me"]["departmentId"],
        },
    )
    connection = post(
        state["control"],
        "/memory/connections",
        {
            "name": f"Boundary wrong Mem0 connection {state['runId']}",
            "endpoint": f"http://mem0.{namespace}.svc:8000",
            "healthPath": "/openapi.json",
            "credentialId": credential["id"],
            "ownerDepartmentId": state["me"]["departmentId"],
            "configuration": {},
        },
    )
    resource = post(
        state["control"],
        "/memory/namespaces",
        {
            "name": f"Boundary wrong Mem0 namespace {state['runId']}",
            "connectionId": connection["id"],
            "externalNamespace": f"wrong_mem0_{state['runId']}",
            "accessMode": "read_write",
            "ownerDepartmentId": state["me"]["departmentId"],
        },
    )
    wrong_app = memory_application(state, live_providers, "wrong-memory", resource["id"])
    denied, denied_calls = memory_ask(state, wrong_app)
    assert denied_calls and all(c["status"] == "failed" and c.get("errorCode") for c in denied_calls), denied_calls
    report(
        state,
        "remaining-mem0-recovery",
        {
            "failedExecutionId": failed["executionId"],
            "recoveredExecutionId": recovered["executionId"],
            "badCredentialExecutionId": denied["executionId"],
            "failureCodes": [c["errorCode"] for c in failed_calls + denied_calls],
            "recoveredCallCount": len(recovered_calls),
        },
    )


def test_real_kimi_stream_resumes_after_gateway_restart_without_duplicate_deltas(boundary_state, live_application):
    state = boundary_state
    app = live_application
    headers = {"Authorization": f"Bearer {app['token'].value}"}
    before = []
    cursor = None
    with httpx.Client(base_url=state["urls"]["runtime"], headers=headers, timeout=180) as gateway:
        sid = session(gateway, app, "Gateway restart SSE proof")
        iid = message(gateway, sid, "请以水星恢复为开头,写一篇五段约500字的工作流可靠性说明。")
        with gateway.stream("GET", f"/gateway/v1/invocations/{iid}/events") as stream:
            assert stream.status_code == 200
            event = "message"
            for line in stream.iter_lines():
                if line.startswith("id:"):
                    cursor = line.split(":", 1)[1].strip()
                elif line.startswith("event:"):
                    event = line.split(":", 1)[1].strip()
                elif line.startswith("data:") and event == "model.delta":
                    import json

                    before.append(json.loads(line.split(":", 1)[1]))
                    break
        assert before and cursor is not None
        namespace = state["context"]["runtime_namespace"]
        old_pods = run(
            (
                "kubectl",
                "-n",
                namespace,
                "get",
                "pods",
                "-l",
                "app.kubernetes.io/name=runtime-gateway",
                "-o",
                "json",
            )
        ).json()["items"]
        run(("kubectl", "-n", namespace, "rollout", "restart", "deployment/runtime-gateway"), timeout=60)
        run(
            ("kubectl", "-n", namespace, "rollout", "status", "deployment/runtime-gateway", "--timeout=300s"),
            timeout=330,
        )
        for pod in old_pods:
            run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "wait",
                    "--for=delete",
                    f"pod/{pod['metadata']['name']}",
                    "--timeout=120s",
                ),
                timeout=150,
            )
        # kubectl can keep an idle tunnel bound to the deleted Pod until its
        # next request. Reconnect with a read-only readiness probe before any
        # resumed stream or later mutation uses this test transport.
        ready_deadline = time.monotonic() + 30
        while True:
            try:
                ready = httpx.get(f"{state['urls']['runtime']}/health/ready", trust_env=False, timeout=3)
                if ready.status_code == 200:
                    break
            except httpx.HTTPError:
                pass
            assert time.monotonic() < ready_deadline, "replacement Gateway tunnel did not become ready"
            time.sleep(0.5)
        events = []
        sequences = [int(cursor)]
        deadline = time.monotonic() + 180
        reconnects = 0
        while not events or events[-1][0] not in {"invocation.completed", "invocation.failed", "invocation.cancelled"}:
            assert time.monotonic() < deadline, events[-1:]
            reconnects += 1
            with gateway.stream(
                "GET", f"/gateway/v1/invocations/{iid}/events", headers={"Last-Event-ID": cursor}
            ) as resumed:
                assert resumed.status_code == 200
                for line in resumed.iter_lines():
                    if line.startswith("id:"):
                        cursor = line.split(":", 1)[1].strip()
                        sequences.append(int(cursor))
                    elif line.startswith("event:"):
                        event = line.split(":", 1)[1].strip()
                    elif line.startswith("data:"):
                        events.append((event, json.loads(line.split(":", 1)[1])))
            if events and events[-1][0] not in {"invocation.completed", "invocation.failed", "invocation.cancelled"}:
                time.sleep(0.5)
        assert sequences == sorted(set(sequences)), sequences
        assert events[-1][0] == "invocation.completed", events[-1]
        result = terminal(gateway, iid)
        text = "".join(p.get("deltaText", "") for p in before + [p for kind, p in events if kind == "model.delta"])
        messages = gateway.get(f"/gateway/v1/sessions/{sid}/messages").json()
        assistant = next(m for m in messages if m["role"] == "assistant")
        assert any(p.get("content") == text for p in assistant["parts"]), assistant
    recovered_model = new_model(state, "after-gateway-restart")
    recovered_workflow = model_workflow(state, recovered_model, "after gateway restart")
    recovered_execution, _ = invoke(state, recovered_workflow["workflowId"])
    report(
        state,
        "remaining-real-sse-restart",
        {
            "invocationId": iid,
            "executionId": result["executionId"],
            "cursor": cursor,
            "resumedEventCount": len(events),
            "physicalReconnects": reconnects,
            "replacementGatewayConfirmed": True,
            "sequencesUnique": True,
            "persistedTextMatches": True,
            "newWorkflowAfterRestart": recovered_execution,
        },
    )
