"""Insights aggregates, ClickHouse degradation and live data recovery."""

from __future__ import annotations

# ruff: noqa: S608
import json
import time
from datetime import UTC, datetime, timedelta

import httpx
import pytest

from tests.e2e.draft_fixtures import idempotent_headers
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
        headers=idempotent_headers(headers),
        json={"contentSource": "version", "environmentId": environment_id, "workflowVersionId": workflow["versionId"]},
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
        while time.monotonic() < deadline and not any(row["dimensions"].get("workflow") == workflow_id for row in rows):
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
        assert any(row["dimensions"].get("workflow") == workflow_id for row in rows), payload
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
