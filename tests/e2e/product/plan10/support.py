from __future__ import annotations

import time
import uuid
from contextlib import contextmanager

import httpx

from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run
from tools.scripts.release.evidence import write_report


def scene(suite, kind):
    return next(case for case in suite["cases"] if case["kind"] == kind)


@contextmanager
def client(suite, case):
    with httpx.Client(
        base_url=suite["urls"]["runtime"],
        headers={"Authorization": f"Bearer {case['secret'].value}"},
        timeout=120,
        trust_env=False,
    ) as http:
        yield http


def create(http, case, rows, key=None, name="Batch acceptance"):
    response = http.post(
        f"/gateway/v1/applications/{case['slug']}/batches",
        headers={"Idempotency-Key": key or str(uuid.uuid4())},
        json={
            "name": name,
            "items": [{"clientItemId": f"row-{i + 1}", "input": value} for i, value in enumerate(rows)],
        },
    )
    assert response.status_code == 202, response.text
    return response.json()


def get(http, batch):
    response = http.get(f"/gateway/v1/batches/{batch}")
    assert response.status_code == 200, response.text
    return response.json()


def items(http, batch, **query):
    response = http.get(f"/gateway/v1/batches/{batch}/items", params={"limit": 50, **query})
    assert response.status_code == 200, response.text
    return response.json()


def command(http, batch, operation, body=None, key=None):
    response = http.post(
        f"/gateway/v1/batches/{batch}/{operation}", headers={"Idempotency-Key": key or str(uuid.uuid4())}, json=body
    )
    assert response.status_code in (200, 202), response.text
    return response.json()


def wait(read, predicate, timeout=180):
    until = time.monotonic() + timeout
    while True:
        value = read()
        if predicate(value):
            return value
        assert time.monotonic() < until, value
        time.sleep(0.3)


def sql(suite, query):
    namespace = suite["installed"]["runtime_namespace"]
    assert namespace.startswith("agentx-e2e-runtime-") and namespace.endswith(suite["runId"])
    return _runtime_mysql(suite["installed"], "SET NAMES utf8mb4 COLLATE utf8mb4_0900_ai_ci; " + query)


def hexid(value):
    return uuid.UUID(value).hex


def report(suite, name, **data):
    write_report(suite["installed"], f"plan10/{name}.json", {"passed": True, **data})


@contextmanager
def unknown_receipt(suite, task_id):
    """Inject one unsettled receipt tied to a real owned attempt; dispatch no provider call."""
    receipt = uuid.uuid4().hex
    target = hexid(task_id)
    sql(
        suite,
        f"INSERT INTO runtime_calls(id,tenant_id,execution_id,node_execution_id,attempt_id,call_index,call_kind,idempotency_key,request_fingerprint,side_effect,status) SELECT UNHEX('{receipt}'),a.tenant_id,a.execution_id,a.node_execution_id,a.id,987654,'mcp_tool','plan10-unknown-{receipt}',REPEAT('a',64),'irreversible','outcome_unknown' FROM node_attempts a JOIN application_invocations v ON v.tenant_id=a.tenant_id AND v.execution_id=a.execution_id WHERE v.id=UNHEX('{target}') ORDER BY a.id LIMIT 1",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
    )
    assert sql(suite, f"SELECT COUNT(*) FROM runtime_calls WHERE id=UNHEX('{receipt}')") == "1"  # noqa: S608 — normalized fixture IDs in a temporary namespace.
    try:
        yield receipt
    finally:
        sql(suite, f"DELETE FROM runtime_calls WHERE id=UNHEX('{receipt}')")  # noqa: S608 — normalized fixture IDs in a temporary namespace.


def replace_version(suite, case, mutate, api_user_id_input=None):
    import copy

    from tests.e2e.draft_fixtures import publish_fixture_version
    from tests.e2e.product.next_batch_support import get as read
    from tests.e2e.product.test_provider_integration import _publish_application_deployment

    control = suite["control"]
    current = read(control, f"/workflows/{case['workflowId']}/versions/{case['versionId']}")
    definition = copy.deepcopy(current["definition"])
    mutate(definition)
    edited = control.post(f"/api/v1/workflows/{case['workflowId']}/versions/{case['versionId']}/edit")
    assert edited.status_code == 200, edited.text
    document = edited.json()["document"]
    saved = control.put(
        f"/api/v1/workflows/{case['workflowId']}/drafts/{document['id']}",
        json={
            "editorDocument": document["editorDocument"],
            "expectedRevision": document["revision"],
            "definition": definition,
        },
    )
    assert saved.status_code == 200, saved.text
    published = publish_fixture_version(control, case["workflowId"], document["id"], saved.json()["revision"])
    assert published.status_code == 200, published.text
    version = published.json()
    deployments = read(control, f"/applications/{case['applicationId']}/deployments")
    environment = next(d["environmentId"] for d in deployments if d["id"] == case["deploymentId"])
    deployment = _publish_application_deployment(
        control, dict(control.headers), case["applicationId"], version["id"], environment, api_user_id_input
    )
    case.update(versionId=version["id"], deploymentId=deployment["id"])
    return deployment


@contextmanager
def coordinator_paused(suite):
    namespace = suite["installed"]["runtime_namespace"]
    assert namespace.startswith("agentx-e2e-runtime-") and namespace.endswith(suite["runId"])
    current = run(("kubectl", "-n", namespace, "get", "deployment/workflow-runtime", "-o", "json")).json()
    replicas = current["spec"].get("replicas", 1)
    run(("kubectl", "-n", namespace, "scale", "deployment/workflow-runtime", "--replicas=0"))
    try:
        # Deployment counts exclude terminating Pods before their command loops
        # have exited. A paused fixture must have no remaining coordinators.
        pods = run(
            ("kubectl", "-n", namespace, "get", "pods", "-l", "app.kubernetes.io/name=workflow-runtime", "-o", "json")
        ).json()["items"]
        if pods:
            run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "delete",
                    *[f"pod/{pod['metadata']['name']}" for pod in pods],
                    "--grace-period=1",
                    "--ignore-not-found",
                    "--wait=true",
                    "--timeout=180s",
                ),
                timeout=200,
            )
            run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "wait",
                    "--for=delete",
                    *[f"pod/{pod['metadata']['name']}" for pod in pods],
                    "--timeout=180s",
                ),
                timeout=200,
            )
        wait(
            lambda: run(("kubectl", "-n", namespace, "get", "deployment/workflow-runtime", "-o", "json")).json(),
            lambda value: value["status"].get("replicas", 0) == 0,
            120,
        )
        wait(
            lambda: run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "get",
                    "pods",
                    "-l",
                    "app.kubernetes.io/name=workflow-runtime",
                    "-o",
                    "json",
                )
            ).json()["items"],
            lambda value: not value,
            120,
        )
        yield
    finally:
        run(("kubectl", "-n", namespace, "scale", "deployment/workflow-runtime", f"--replicas={replicas}"))
        run(
            ("kubectl", "-n", namespace, "rollout", "status", "deployment/workflow-runtime", "--timeout=180s"),
            timeout=200,
        )


def crash_coordinator(suite):
    """SIGKILL the actual service process; Pod deletion alone is not a crash barrier."""
    namespace = suite["installed"]["runtime_namespace"]
    assert namespace.startswith("agentx-e2e-runtime-") and namespace.endswith(suite["runId"])
    pods = run(
        ("kubectl", "-n", namespace, "get", "pods", "-l", "app.kubernetes.io/name=workflow-runtime", "-o", "json")
    ).json()["items"]
    active = [pod for pod in pods if not pod["metadata"].get("deletionTimestamp")]
    assert len(active) == 1, "Crash injection requires one unambiguous temporary coordinator"
    pod = active[0]
    name = pod["metadata"]["name"]
    before = next(
        container["restartCount"]
        for container in pod["status"]["containerStatuses"]
        if container["name"] == "workflow-runtime"
    )
    script = "const fs=require('node:fs');const ids=fs.readFileSync('/proc/1/task/1/children','utf8').trim().split(' ').filter(Boolean);const pid=ids.find(id=>fs.readFileSync('/proc/'+id+'/cmdline','utf8').split(String.fromCharCode(0))[0]==='/usr/local/bin/agentx-service');if(!pid)process.exit(7);fs.writeSync(1,pid);process.kill(Number(pid),'SIGKILL')"
    killed = run(("kubectl", "-n", namespace, "exec", name, "--", "node", "-e", script), timeout=30, check=False)
    assert killed.returncode in (0, 137, 143), killed.stderr

    def restarted():
        current = run(("kubectl", "-n", namespace, "get", "pod", name, "-o", "json")).json()
        return next(
            container
            for container in current["status"].get("containerStatuses", [])
            if container["name"] == "workflow-runtime"
        )

    after = wait(restarted, lambda container: container["restartCount"] > before and container["ready"], 180)
    return {
        "pod": name,
        "servicePid": killed.stdout.strip(),
        "restartBefore": before,
        "restartAfter": after["restartCount"],
    }
