# ruff: noqa: S608
# This Kubernetes E2E module queries only its isolated test database with fixture identifiers.

from __future__ import annotations

import copy
import json
import time
from typing import Any

import httpx
import pytest

from tests.e2e.support import MYSQL_ROOT_CLIENT, redact, run

ADMIN_USERNAME = "admin"
ADMIN_PASSWORD = "agentx-e2e-admin-password"  # noqa: S105 -- isolated E2E company
WORKFLOW_ID = "018f0000-0000-7000-8000-000000000401"


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
            f'{MYSQL_ROOT_CLIENT} --batch --skip-column-names -uroot agentx_runtime -e "$1"',
            "agentx-p3-04-query",
            query,
        ),
        timeout=60,
    ).stdout.strip()


def _control_mysql(installed_agentx: dict[str, str], query: str) -> str:
    """Run a read-only query against the isolated Control database."""
    return run(
        (
            "kubectl",
            "-n",
            installed_agentx["control_namespace"],
            "exec",
            "statefulset/control-mysql",
            "--",
            "sh",
            "-ec",
            f'{MYSQL_ROOT_CLIENT} --batch --skip-column-names -uroot agentx_control -e "$1"',
            "agentx-p3-05-query",
            query,
        ),
        timeout=60,
    ).stdout.strip()


def _wait_admission_outbox(installed_agentx: dict[str, str], timeout: float = 120.0) -> None:
    """Wait until Control and Runtime have applied the latest admission epoch."""
    deadline = time.monotonic() + timeout
    query = (
        "SELECT COUNT(*) FROM outbox "
        "WHERE aggregate_type IN ('application_admission','runtime_user_admission',"
        "'workflow_admission','application_chat_mapping') "
        "AND status IN ('pending','processing','failed');"
    )
    latest = ""
    expected_epoch = "0"
    while time.monotonic() < deadline:
        latest = _control_mysql(installed_agentx, query)
        if latest == "0":
            expected_epoch = _control_mysql(
                installed_agentx,
                "SELECT COALESCE(MAX(CAST(JSON_UNQUOTE(JSON_EXTRACT(payload_json, '$.policyEpoch')) AS UNSIGNED)), 0) "
                "FROM outbox WHERE event_type='ServiceIdentityAdmissionChanged' "
                "AND aggregate_type='workflow_admission';",
            )
            projection = _runtime_mysql(
                installed_agentx,
                "SELECT CASE WHEN EXISTS("
                "SELECT 1 FROM service_identity_projection "
                "WHERE workflow_id=UUID_TO_BIN('018f0000-0000-7000-8000-000000000401') "
                f"AND policy_epoch >= {int(expected_epoch)}"
                ") AND NOT EXISTS("
                "SELECT 1 FROM resource_grant_projection rg "
                "JOIN service_identity_projection si ON si.identity_id=rg.subject_id "
                "WHERE si.workflow_id=UUID_TO_BIN('018f0000-0000-7000-8000-000000000401') "
                f"AND rg.policy_epoch < {int(expected_epoch)} AND rg.status='active'"
                ") THEN 1 ELSE 0 END;",
            )
            if projection == "1":
                return
        failed = _control_mysql(
            installed_agentx,
            "SELECT COUNT(*) FROM outbox WHERE aggregate_type IN "
            "('application_admission','runtime_user_admission','workflow_admission','application_chat_mapping') "
            "AND status='failed';",
        )
        if failed != "0":
            details = _control_mysql(
                installed_agentx,
                "SELECT event_type,last_error FROM outbox WHERE aggregate_type IN "
                "('application_admission','runtime_user_admission','workflow_admission','application_chat_mapping') "
                "AND status='failed' ORDER BY occurred_at DESC LIMIT 5;",
            )
            raise AssertionError(f"Control admission outbox has failed events: {details}")
        time.sleep(0.5)
    raise AssertionError(f"Control admission outbox did not settle: {latest}")


def _access_token(client: httpx.Client) -> tuple[str, dict[str, Any]]:
    status = client.get("/api/v1/bootstrap/status")
    status.raise_for_status()
    if status.json()["required"]:
        response = client.post(
            "/api/v1/bootstrap",
            json={
                "companyName": "Agentx P3-04 E2E",
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
    payload = response.json()
    token = payload["accessToken"]
    me = client.get("/api/v1/auth/me", headers={"Authorization": f"Bearer {token}"})
    me.raise_for_status()
    return token, me.json()


def _fixture_job(
    installed_agentx: dict[str, str],
    run_id: str,
    me: dict[str, Any],
    environment_id: str,
    job_suffix: str = "attachment",
    session_policy: str | None = None,
    fixture_env: dict[str, str] | None = None,
) -> tuple[str, dict[str, Any]]:
    namespace = installed_agentx["control_namespace"]
    deployment = run(
        ("kubectl", "-n", namespace, "get", "deployment/platform-control", "-o", "json"),
        timeout=60,
    ).json()
    pod_spec = copy.deepcopy(deployment["spec"]["template"]["spec"])
    pod_spec.pop("initContainers", None)
    pod_spec.pop("terminationGracePeriodSeconds", None)
    pod_spec["restartPolicy"] = "Never"
    container = next(copy.deepcopy(item) for item in pod_spec["containers"] if item["name"] == "platform-control")
    for field in (
        "args",
        "lifecycle",
        "livenessProbe",
        "ports",
        "readinessProbe",
        "startupProbe",
    ):
        container.pop(field, None)
    container["name"] = "fixture"
    container["command"] = ["/usr/local/bin/agentx-p3-fixture"]
    overrides = {
        "AGENTX_V2_FIXTURE_DEPENDENCIES_NAMESPACE": installed_agentx["dependencies_namespace"],
        # Scope immutable fixture object/version IDs by scenario.  A single
        # E2E install runs several scenarios sequentially; reusing the same
        # Skill object ID after retention has collected it is intentionally
        # rejected by Runtime's immutable upload contract.
        "AGENTX_V2_FIXTURE_ID_SEED": f"{run_id}:{job_suffix}",
        "AGENTX_V2_FIXTURE_TENANT_ID": me["companyId"],
        "AGENTX_V2_FIXTURE_USER_ID": me["id"],
        "AGENTX_V2_FIXTURE_DEPARTMENT_ID": me["departmentId"],
        "AGENTX_V2_FIXTURE_ENVIRONMENT_ID": environment_id,
    }
    if session_policy is not None:
        if session_policy not in {"invocation", "application_session"}:
            raise ValueError(f"unsupported fixture session policy: {session_policy}")
        overrides["AGENTX_V2_FIXTURE_SESSION_POLICY"] = session_policy
    if fixture_env:
        overrides.update(fixture_env)
    container["env"] = [item for item in container.get("env", []) if item["name"] not in overrides] + [
        {"name": name, "value": value} for name, value in overrides.items()
    ]
    pod_spec["containers"] = [container]
    name = f"p3-04-{job_suffix}-{run_id}"
    labels = copy.deepcopy(deployment["spec"]["template"]["metadata"]["labels"])
    labels["agentx.io/e2e-fixture"] = "p3-04-attachments"
    job = {
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {"name": name, "namespace": namespace, "labels": labels},
        "spec": {
            "backoffLimit": 0,
            "template": {"metadata": {"labels": labels}, "spec": pod_spec},
        },
    }
    return name, job


def _run_fixture_job(
    installed_agentx: dict[str, str],
    run_id: str,
    me: dict[str, Any],
    environment_id: str,
    job_suffix: str = "attachment",
    session_policy: str | None = None,
    fixture_env: dict[str, str] | None = None,
) -> dict[str, Any]:
    namespace = installed_agentx["control_namespace"]
    name, job = _fixture_job(
        installed_agentx,
        run_id,
        me,
        environment_id,
        job_suffix,
        session_policy,
        fixture_env,
    )
    try:
        run(
            ("kubectl", "apply", "-f", "-"),
            input_text=json.dumps(job),
            timeout=60,
        )
        deadline = time.monotonic() + 300
        job_status = None
        completed = False
        while time.monotonic() < deadline:
            job_status = run(
                ("kubectl", "-n", namespace, "get", f"job/{name}", "-o", "json"),
                check=False,
                timeout=60,
            )
            if job_status.returncode == 0:
                conditions = job_status.json().get("status", {}).get("conditions", [])
                terminal = next(
                    (
                        condition
                        for condition in conditions
                        if condition.get("status") == "True" and condition.get("type") in {"Complete", "Failed"}
                    ),
                    None,
                )
                if terminal:
                    completed = terminal["type"] == "Complete"
                    break
            time.sleep(1)
        logs = run(
            ("kubectl", "-n", namespace, "logs", f"job/{name}"),
            check=False,
            timeout=60,
        )
        if not completed or logs.returncode != 0:
            describe = run(
                ("kubectl", "-n", namespace, "describe", f"job/{name}"),
                check=False,
                timeout=60,
            )
            raise AssertionError(
                "P3-04 Fixture Job failed:\n"
                + redact(
                    (job_status.stdout + job_status.stderr if job_status else "fixture job timed out\n")
                    + logs.stdout
                    + logs.stderr
                    + describe.stdout
                )
            )
        lines = [line for line in logs.stdout.splitlines() if line.strip().startswith("{")]
        assert lines, redact(logs.stdout)
        fixture = json.loads(lines[-1])
        _wait_admission_outbox(installed_agentx)
        return fixture
    finally:
        run(
            (
                "kubectl",
                "-n",
                namespace,
                "delete",
                "job",
                name,
                "--ignore-not-found=true",
                "--wait=true",
                "--timeout=120s",
            ),
            check=False,
            timeout=150,
        )


def _wait_execution(client: httpx.Client, headers: dict[str, str], execution_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + 300
    latest: dict[str, Any] = {}
    while time.monotonic() < deadline:
        response = client.get(f"/api/v1/executions/{execution_id}", headers=headers)
        if response.status_code == 200:
            latest = response.json()
            if latest["status"] in {
                "succeeded",
                "failed",
                "cancelled",
                "timed_out",
            }:
                return latest
        else:
            latest = {"statusCode": response.status_code, "body": response.text}
        time.sleep(1)
    raise AssertionError(f"execution did not reach a terminal state: {latest}")


def _wait_trace(client: httpx.Client, headers: dict[str, str], execution_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + 120
    latest = ""
    while time.monotonic() < deadline:
        response = client.get(f"/api/v1/executions/{execution_id}/trace?limit=1000", headers=headers)
        latest = response.text
        if response.status_code == 200 and response.json().get("complete"):
            return response.json()
        assert response.status_code in {200, 202}, latest
        time.sleep(1)
    raise AssertionError(f"trace did not become complete: {latest}")


def _assert_execution_succeeded(
    installed_agentx: dict[str, str],
    client: httpx.Client,
    headers: dict[str, str],
    execution: dict[str, Any],
) -> None:
    if execution["status"] == "succeeded":
        return
    details = client.get(f"/api/v1/executions/{execution['id']}/runtime-details", headers=headers)
    diagnostic: object = details.text
    if details.status_code == 200:
        diagnostic = details.json()
    call_errors = _runtime_mysql(
        installed_agentx,
        "SELECT COALESCE(JSON_ARRAYAGG(JSON_OBJECT("
        "'callKind',call_kind,'status',status,'errorCode',error_code,'errorMessage',error_message)),JSON_ARRAY()) "
        "FROM runtime_calls "
        f"WHERE execution_id=UUID_TO_BIN('{execution['id']}') AND status IN ('failed','outcome_unknown');",
    )
    raise AssertionError(
        json.dumps(
            {"execution": execution, "runtimeDetails": diagnostic, "runtimeCallErrors": call_errors},
            ensure_ascii=False,
            sort_keys=True,
        )
    )


@pytest.mark.cluster
@pytest.mark.runtime
def test_agent_attachments_follow_the_published_workflow_main_chain(
    installed_agentx: dict[str, str],
    e2e_providers: dict[str, str],
    service_urls: dict[str, str],
    run_id: str,
) -> None:
    assert e2e_providers["echo_mcp"].endswith(":8090")
    with httpx.Client(base_url=service_urls["web"], timeout=60) as client:
        token, me = _access_token(client)
        headers = {"Authorization": f"Bearer {token}"}
        environments = client.get("/api/v1/environments", headers=headers)
        environments.raise_for_status()
        environment = next(item for item in environments.json() if item["code"] == "development")
        fixture = _run_fixture_job(installed_agentx, run_id, me, environment["id"])
        assert fixture["tenantId"] == me["companyId"]
        assert fixture["environmentId"] == environment["id"]

        started = client.post(
            f"/api/v1/workflows/{WORKFLOW_ID}/run",
            headers=headers,
            json={
                "input": {"message": "p3-04-kubernetes-attachment"},
                "idempotencyKey": f"p3-04-attachment-{run_id}",
            },
        )
        assert started.status_code == 202, started.text
        execution_id = started.json()["executionId"]
        execution = _wait_execution(client, headers, execution_id)
        _assert_execution_succeeded(installed_agentx, client, headers, execution)

        nodes = client.get(f"/api/v1/executions/{execution_id}/nodes", headers=headers)
        nodes.raise_for_status()
        agent = next(item for item in nodes.json()["items"] if item["nodeId"] == "agent")
        assert agent["status"] == "succeeded", agent
        assert "skill_context=true" in json.dumps(agent["output"])

        details = client.get(f"/api/v1/executions/{execution_id}/runtime-details", headers=headers)
        details.raise_for_status()
        calls = details.json()["calls"]
        assert sum(call["callKind"] == "model" for call in calls) == 2
        assert sum(call["callKind"] == "mcp_tool" for call in calls) == 1
        assert all(call["status"] == "succeeded" for call in calls)

        registry_json = _runtime_mysql(
            installed_agentx,
            "SELECT JSON_EXTRACT(w.payload_json,'$.agentBundle.agents[0].attachmentRegistry') "
            "FROM runtime_work_packages w JOIN workflow_executions e ON e.work_package_id=w.id "
            f"WHERE e.id=UUID_TO_BIN('{execution_id}');",
        )
        registry = json.loads(registry_json)
        assert [context["origin"] for context in registry["contexts"]] == ["skill"]
        assert {tool["origin"] for tool in registry["tools"]} == {
            "mcp",
            "knowledge",
            "memory",
        }
        assert len(registry["authorizationEvidence"]) >= 4

        trace = _wait_trace(client, headers, execution_id)
        span_names = {(span["spanName"], span["status"]) for span in trace["spans"]}
        assert ("Agent run", "succeeded") in span_names
        assert ("Agent model operation", "succeeded") in span_names
        assert ("Agent tool operation", "succeeded") in span_names
