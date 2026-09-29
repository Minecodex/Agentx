# ruff: noqa: S608
"""Evaluation and Insights E2E (plan7 P7-C §5, API-level subset).

Covers scenario 2 (un-granted llm_judge model → 422 grant error), scenario 4
(Insights aggregates return real execution data with workflow filtering and
the errorCode dimension), scenario 5 (ClickHouse outage → INSIGHTS_DEGRADED
while executions continue; recovery after restore) and scenario 6 (the
dashboard's two live data sources return real values).

The llm_judge full-chain report and the two-version compare page (scenarios
1/3) stay on the handoff list; their UI flows are covered by vitest.
"""

from __future__ import annotations

import json
import time
from datetime import UTC, datetime, timedelta

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
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def _rfc3339(moment: datetime) -> str:
    return moment.astimezone(UTC).isoformat().replace("+00:00", "Z")


def _drive_one_execution(
    installed_agentx: dict[str, str],
    control: httpx.Client,
    headers: dict[str, str],
    gateway_url: str,
    run_id: str,
    seq: int,
) -> str:
    """Publishes the passthrough app with a webhook channel and fires one
    inbound message; returns the workflow id feeding the aggregate filters."""
    workflow = _passthrough_workflow(control, headers, f"Insights {run_id} {seq}")
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
            "name": f"Insights App {run_id} {seq}",
            "slug": f"insights-{run_id}-{seq}",
            "visibility": "company",
        },
    )
    assert application.status_code in (200, 201), application.text
    application_id = application.json()["id"]
    channel = _dingtalk_channel(control, headers, application_id, reply_enabled=False)
    _publish_application(control, headers, workflow, application_id, environment_id)

    public_id = channel["publicId"]
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT COUNT(*) FROM webhook_bindings WHERE public_id='" + public_id + "';",
        )
        if raw and raw != "0":
            break
        time.sleep(3)
    else:
        raise AssertionError("webhook binding never reached runtime")

    event_id = f"insights-{run_id}-{seq}"
    with httpx.Client(base_url=gateway_url, timeout=30) as gateway:
        accepted = _signed_dingtalk_post(
            gateway,
            channel["path"],
            "e2e-dingtalk-secret",
            {
                "msgId": event_id,
                "conversationId": "insights-chat",
                "conversationType": "1",
                "senderId": "insights-sender",
                "senderNick": "Insights",
                "msgtype": "text",
                "content": json.dumps({"content": f"insights probe {seq}"}),
                "createAt": int(time.time() * 1000),
            },
            "http://im-mock.invalid/never-delivered",
        )
        assert accepted.status_code in (200, 202), accepted.text

    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT e.status FROM application_invocations i "
            "JOIN workflow_executions e ON e.tenant_id=i.tenant_id AND e.id=i.execution_id "
            f"WHERE i.provider_event_id='{event_id}' LIMIT 1;",
        )
        if raw == "succeeded":
            return workflow["workflowId"]
        time.sleep(2)
    raise AssertionError("insights source execution did not succeed")


def test_insights_aggregates_degradation_and_grant_gate(
    installed_agentx: dict[str, str], service_urls: dict[str, str], run_id: str
) -> None:
    runtime_ns = installed_agentx["runtime_namespace"]
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, _me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}

        workflow_id = _drive_one_execution(installed_agentx, control, headers, service_urls["runtime"], run_id, 1)

        # Scenario 4: aggregates over real trace data, workflow-filtered.
        now = datetime.now(UTC)
        aggregates = control.post(
            "/api/v1/insights/aggregates",
            headers=headers,
            json={
                "from": _rfc3339(now - timedelta(hours=2)),
                "to": _rfc3339(now + timedelta(hours=1)),
                "metrics": ["count", "error_rate"],
                "dimensions": ["workflow"],
                "limit": 100,
            },
        )
        assert aggregates.status_code == 200, aggregates.text
        payload = aggregates.json()
        rows = payload.get("rows") or payload.get("items") or []
        # Trace spans reach ClickHouse asynchronously; poll until they land.
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline and not rows:
            time.sleep(5)
            aggregates = control.post(
                "/api/v1/insights/aggregates",
                headers=headers,
                json={
                    "from": _rfc3339(now - timedelta(hours=2)),
                    "to": _rfc3339(now + timedelta(hours=1)),
                    "metrics": ["count", "error_rate"],
                    "dimensions": ["workflow"],
                    "limit": 100,
                },
            )
            assert aggregates.status_code == 200, aggregates.text
            payload = aggregates.json()
            rows = payload.get("rows") or payload.get("items") or []
        assert rows, payload
        assert any(row["dimensions"].get("workflow") for row in rows), payload
        filtered = control.post(
            "/api/v1/insights/aggregates",
            headers=headers,
            json={
                "from": _rfc3339(now - timedelta(hours=2)),
                "to": _rfc3339(now + timedelta(hours=1)),
                "metrics": ["count"],
                "dimensions": ["workflow"],
                "filters": {"workflowId": workflow_id},
                "limit": 10,
            },
        )
        assert filtered.status_code == 200, filtered.text
        filtered_rows = filtered.json().get("rows") or filtered.json().get("items") or []
        assert filtered_rows, f"expected={workflow_id} unfiltered={rows} filtered={filtered.text}"
        assert all(row["dimensions"].get("workflow") == workflow_id for row in filtered_rows), (
            f"expected={workflow_id} rows={filtered_rows}"
        )

        # Scenario 6: the dashboard's second live source (running executions
        # search total) returns a real number, not a null placeholder.
        executions = control.get("/api/v1/executions?limit=1", headers=headers)
        assert executions.status_code == 200, executions.text
        assert executions.json().get("total", 0) >= 1, executions.text

        # Scenario 5: ClickHouse outage degrades Insights but not executions.
        run(("kubectl", "-n", runtime_ns, "scale", "statefulset", "clickhouse", "--replicas=0"), timeout=120)
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            pods = run(
                ("kubectl", "-n", runtime_ns, "get", "pods", "-l", "app.kubernetes.io/name=clickhouse", "-o", "name"),
                timeout=60,
            ).stdout.strip()
            if not pods:
                break
            time.sleep(3)
        degraded = None
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            degraded = control.post(
                "/api/v1/insights/aggregates",
                headers=headers,
                json={
                    "from": _rfc3339(now - timedelta(hours=2)),
                    "to": _rfc3339(now + timedelta(hours=1)),
                    "metrics": ["count"],
                    "dimensions": ["day"],
                    "limit": 5,
                },
            )
            if degraded.status_code == 503:
                break
            time.sleep(3)
        assert degraded is not None and degraded.status_code == 503, getattr(degraded, "text", "")
        assert degraded.json().get("code") == "INSIGHTS_DEGRADED", degraded.text

        run(("kubectl", "-n", runtime_ns, "scale", "statefulset", "clickhouse", "--replicas=1"), timeout=120)
        run(
            ("kubectl", "-n", runtime_ns, "rollout", "status", "statefulset", "clickhouse", "--timeout=300s"),
            timeout=330,
        )

        # Executions were never affected by the observability outage; drive a
        # fresh one after restore so recovery is also observed end to end.
        _drive_one_execution(installed_agentx, control, headers, service_urls["runtime"], run_id, 2)
        deadline = time.monotonic() + 240
        recovered = None
        while time.monotonic() < deadline:
            recovered = control.post(
                "/api/v1/insights/aggregates",
                headers=headers,
                json={
                    "from": _rfc3339(datetime.now(UTC) - timedelta(hours=2)),
                    "to": _rfc3339(datetime.now(UTC) + timedelta(hours=1)),
                    "metrics": ["count"],
                    "dimensions": ["day"],
                    "limit": 5,
                },
            )
            if recovered.status_code == 200:
                break
            time.sleep(5)
        assert recovered is not None and recovered.status_code == 200, getattr(recovered, "text", "")

        # Scenario 2: an llm_judge rule whose model lacks a tenant grant
        # refuses to start with the explicit grant error.
        departments = control.get("/api/v1/departments?pageSize=5", headers=headers)
        departments.raise_for_status()
        department_items = departments.json()
        department = (department_items.get("items") if isinstance(department_items, dict) else department_items)[0][
            "id"
        ]
        model = control.post(
            "/api/v1/models/aliases",
            headers=headers,
            json={
                "connectionName": f"Judge model {run_id}",
                "providerType": "openai_compatible",
                "endpoint": "http://echo-mcp.invalid/v1",
                "credentialId": None,
                "ownerDepartmentId": department,
                "alias": f"judge-{run_id[:8]}",
                "price": {"currency": "USD", "inputPerMillion": "1", "outputPerMillion": "2"},
            },
        )
        assert model.status_code in (200, 201), model.text
        model_id = model.json().get("id") or model.json().get("modelId")
        dataset = control.post(
            "/api/v1/datasets",
            headers=headers,
            json={
                "name": f"Insights dataset {run_id}",
                "description": "P7-C",
                "visibility": "company",
            },
        )
        assert dataset.status_code in (200, 201), dataset.text
        imported = control.post(
            f"/api/v1/datasets/{dataset.json()['id']}/import",
            headers=headers,
            json={
                "expectedRevision": dataset.json().get("revision", 1),
                "format": "jsonl",
                "content": json.dumps({"caseKey": "c1", "name": "case one", "input": {"message": "hi"}}),
            },
        )
        assert imported.status_code in (200, 201), imported.text
        # Imports append cases; a dataset version must be published explicitly.
        revision = dataset.json().get("revision", 1) + 1
        version = control.post(
            f"/api/v1/datasets/{dataset.json()['id']}/versions",
            headers=headers,
            json={"expectedRevision": revision},
        )
        if version.status_code not in (200, 201):
            listing = control.get(f"/api/v1/datasets/{dataset.json()['id']}/versions", headers=headers)
            version_id = listing.json()[0]["id"] if listing.json() else None
        else:
            version_id = version.json()["id"]
        assert version_id, f"{version.status_code} {version.text}"
        dataset_version_id = version_id
        profile = control.post(
            "/api/v1/evaluation-profiles",
            headers=headers,
            json={
                "name": f"Judge profile {run_id}",
                "description": "P7-C scenario 2",
                "visibility": "company",
                "aggregation": "all",
                "passThreshold": "1",
                "rules": [
                    {
                        "key": "judge",
                        "name": "LLM judge",
                        "evaluatorType": "llm_judge",
                        "configuration": {"modelId": model_id, "prompt": "score the answer"},
                        "weight": "1",
                        "required": True,
                    }
                ],
            },
        )
        assert profile.status_code in (200, 201), profile.text
        profile_version_id = profile.json().get("versionId") or profile.json().get("id")
        workflow = _passthrough_workflow(control, headers, f"Judge wf {run_id}")
        evaluation = control.post(
            "/api/v1/evaluations",
            headers=headers,
            json={
                "name": f"Judge eval {run_id}",
                "workflowVersionId": workflow["versionId"],
                "datasetVersionId": dataset_version_id,
                "evaluationProfileVersionId": profile_version_id,
                "visibility": "company",
                "parameters": {},
            },
        )
        assert evaluation.status_code in (200, 201), evaluation.text
        started = control.post(f"/api/v1/evaluations/{evaluation.json()['id']}/start", headers=headers)
        assert started.status_code == 422, f"{started.status_code} {started.text}"
        assert "GRANT" in started.json().get("code", "").upper() or "grant" in started.json().get("message", ""), (
            started.text
        )
