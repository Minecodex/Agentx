"""Real Agent trace ancestry and desktop node content presentation."""

from __future__ import annotations

import json
import os
from datetime import datetime
from pathlib import Path

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.test_channel_delivery import _passthrough_workflow
from tests.e2e.product.test_live_memory_pipeline import _save_version
from tests.e2e.product.test_playwright import _verify_trace_degradation
from tests.e2e.runtime.test_agent_attachments import (
    WORKFLOW_ID,
    _access_token,
    _assert_execution_succeeded,
    _run_fixture_job,
    _wait_execution,
    _wait_trace,
)
from tests.e2e.support import run_playwright
from tests.e2e.workflow_fixtures import set_chain_definition
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.observability]

REPORT = "## 订单核算报告\n\n| 项目 | 金额 |\n| --- | --- |\n| **实付** | 558.00 |\n\n- 已核对 `order_result.json`"


def _assert_agent_ancestry(trace):
    spans = {span["spanId"]: span for span in trace["spans"]}
    assert trace["nextCursor"] is None, "fixture Trace must fit in a complete page"
    for span in spans.values():
        if span.get("parentSpanId"):
            assert span["parentSpanId"] in spans, (span["spanName"], span["parentSpanId"])
    calls = [span for span in spans.values() if span.get("runtimeCallId") and span.get("agentRunId")]
    assert len(calls) >= 3, spans
    for call in calls:
        parent = spans[call["parentSpanId"]]
        assert parent["spanKind"] == "agent_iteration", call
        assert parent["spanName"] == "Agent turn", parent
        assert parent["agentIterationId"] == call["agentIterationId"], (parent, call)
        assert datetime.fromisoformat(parent["startedAt"]) <= datetime.fromisoformat(call["startedAt"])
        assert datetime.fromisoformat(parent["endedAt"]) >= datetime.fromisoformat(call["endedAt"])
        visited = set()
        while parent["spanKind"] != "node":
            assert parent["spanId"] not in visited, "Trace ancestry must not cycle"
            visited.add(parent["spanId"])
            parent = spans[parent["parentSpanId"]]
        assert parent["nodeExecutionId"] == call["nodeExecutionId"], (parent, call)


def test_trace_ancestry_markdown_and_collapsible_node_content(installed_agentx, service_urls, e2e_providers, run_id):
    assert e2e_providers["echo_mcp"]
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        environment = next(item for item in control.get("/api/v1/environments").json() if item["code"] == "development")
        _run_fixture_job(installed_agentx, run_id, me, environment["id"], job_suffix="trace")
        agent_run = control.post(
            f"/api/v1/workflows/{WORKFLOW_ID}/run",
            json={"input": {"message": REPORT}, "idempotencyKey": f"trace-agent-{run_id}"},
        )
        agent_run.raise_for_status()
        agent_execution = agent_run.json()["executionId"]
        _assert_execution_succeeded(
            installed_agentx,
            control,
            dict(control.headers),
            _wait_execution(control, dict(control.headers), agent_execution),
        )
        trace = _wait_trace(control, dict(control.headers), agent_execution)
        _assert_agent_ancestry(trace)

        workflow = _passthrough_workflow(control, dict(control.headers), f"Trace report {run_id}")["workflowId"]
        definition = set_chain_definition(fixture_draft(control, workflow)["definition"], 2)
        report_node = next(node for node in definition["nodes"] if node["type"] == "set")
        report_node["name"] = "报告整理"
        report_node["parameters"] = {
            "values": {
                "kind": "object",
                "fields": {
                    "answer": {"kind": "reference", "selector": {"namespace": "inputs", "path": ["message"]}},
                    "payload": {"kind": "literal", "value": {"order": {"paid": 558, "verified": True}, "files": []}},
                },
            },
            "keepOnlySet": True,
        }
        _save_version(control, workflow, definition)
        report_run = control.post(
            f"/api/v1/workflows/{workflow}/run",
            json={"input": {"message": REPORT}, "idempotencyKey": f"trace-report-{run_id}"},
        )
        report_run.raise_for_status()
        report_execution = report_run.json()["executionId"]
        _assert_execution_succeeded(
            installed_agentx,
            control,
            dict(control.headers),
            _wait_execution(control, dict(control.headers), report_execution),
        )
    write_report(
        installed_agentx,
        "trace-presentation.json",
        {"agentExecutionId": agent_execution, "reportExecutionId": report_execution, "trace": trace},
    )
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": run_id,
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_TRACE_AGENT_EXECUTION": agent_execution,
        "AGENTX_E2E_TRACE_REPORT_EXECUTION": report_execution,
    }
    run_playwright(
        Path(installed_agentx["root"]),
        "trace-presentation",
        ("tests/trace-presentation.spec.ts",),
        environment,
    )
    execution_evidence = Path(installed_agentx["artifact_dir"]) / "trace-execution.json"
    execution_evidence.write_text(json.dumps({"executionId": report_execution}), encoding="utf-8")
    _verify_trace_degradation(
        environment, installed_agentx["root"], installed_agentx["runtime_namespace"], execution_evidence
    )
