"""Opt-in real Kimi streaming, cancellation, Judge, versions and Insights."""

from __future__ import annotations

import time
from datetime import UTC, datetime, timedelta

import httpx
import pytest

from tests.e2e.product.test_model_streaming import _sse_events
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def session(gateway, app, title):
    response = gateway.post(
        f"/gateway/v1/applications/{app['applicationSlug']}/sessions",
        json={"title": title, "externalUserId": None},
        headers={"Idempotency-Key": f"live-session-{time.time_ns()}"},
    )
    assert response.status_code == 201, response.text
    return response.json()["id"]


def message(gateway, session_id, text):
    response = gateway.post(
        f"/gateway/v1/sessions/{session_id}/messages",
        json={"parts": [{"partType": "text", "content": text}]},
        headers={"Idempotency-Key": f"live-message-{time.time_ns()}"},
    )
    assert response.status_code == 202, response.text
    return response.json()["id"]


def terminal(gateway, invocation_id):
    deadline = time.monotonic() + 180
    while True:
        response = gateway.get(f"/gateway/v1/invocations/{invocation_id}")
        response.raise_for_status()
        result = response.json()
        if result["status"] in {"completed", "failed", "cancelled", "timed_out"}:
            return result
        assert time.monotonic() < deadline, result
        time.sleep(0.5)


def test_kimi_streams_real_text_persists_history_and_usage(installed_agentx, service_urls, live_application):
    app = live_application
    headers = {"Authorization": f"Bearer {app['token'].value}"}
    with httpx.Client(base_url=service_urls["runtime"], headers=headers, timeout=180) as gateway:
        session_id = session(gateway, app, "真实 Kimi 流式验收")
        invocation_id = message(
            gateway, session_id, "请以水星蓝桥开头,分五段说明如何设计可靠的工作流系统,控制在300字以内。"
        )
        started = time.monotonic()
        events = _sse_events(gateway, headers, invocation_id, timeout=180)
        elapsed = time.monotonic() - started
        deltas = [event.get("deltaText", "") for kind, event in events if kind == "model.delta"]
        assert len(deltas) > 1, [kind for kind, _ in events]
        text = "".join(deltas)
        assert "水星蓝桥" in text, text
        assert events[-1][0] == "invocation.completed", events[-1]
        result = terminal(gateway, invocation_id)
        assert result["status"] == "completed", result
        messages = gateway.get(f"/gateway/v1/sessions/{session_id}/messages").json()
        assistant = next(item for item in messages if item["role"] == "assistant")
        assert any(part.get("content") == text for part in assistant["parts"]), assistant
    with httpx.Client(base_url=service_urls["web"], headers=headers, timeout=60) as control:
        execution = control.get(f"/api/v1/executions/{result['executionId']}").json()
        assert execution["status"] == "succeeded", execution
        assert execution["inputTokens"] + execution["outputTokens"] > 0, execution
    write_report(
        installed_agentx,
        "product/live-kimi-stream.json",
        {
            "status": "passed",
            "sessionId": session_id,
            "invocationId": invocation_id,
            "executionId": result["executionId"],
            "deltaCount": len(deltas),
            "elapsedSeconds": elapsed,
            "answer": text,
            "persistedMatchesDeltas": True,
            "inputTokens": execution["inputTokens"],
            "outputTokens": execution["outputTokens"],
            "realProvider": "kimi/k3",
        },
    )


def test_kimi_generation_cancels_with_authoritative_terminal_state(installed_agentx, service_urls, live_application):
    app = live_application
    headers = {"Authorization": f"Bearer {app['token'].value}"}
    with httpx.Client(base_url=service_urls["runtime"], headers=headers, timeout=180) as gateway:
        session_id = session(gateway, app, "真实 Kimi 取消验收")
        invocation_id = message(gateway, session_id, "请用中文写5000字关于工作流引擎设计的教程。")
        event_type = ""
        delta_received = False
        with gateway.stream("GET", f"/gateway/v1/invocations/{invocation_id}/events", timeout=180) as stream:
            assert stream.status_code == 200
            for line in stream.iter_lines():
                if line.startswith("event:"):
                    event_type = line.split(":", 1)[1].strip()
                elif line.startswith("data:") and event_type == "model.delta":
                    delta_received = True
                    response = gateway.post(
                        f"/gateway/v1/invocations/{invocation_id}/cancel",
                        headers={"Idempotency-Key": f"live-cancel-{time.time_ns()}"},
                    )
                    assert response.status_code in (200, 202), response.text
                    break
                elif line.startswith("data:") and event_type in {
                    "invocation.completed",
                    "invocation.failed",
                    "invocation.cancelled",
                    "invocation.timed_out",
                }:
                    raise AssertionError(f"Invocation terminated before cancellation: {event_type}")
        assert delta_received
        result = terminal(gateway, invocation_id)
        assert result["status"] == "cancelled", result
    write_report(
        installed_agentx,
        "product/live-kimi-cancel.json",
        {"status": "passed", "invocationId": invocation_id, "terminalStatus": result["status"]},
    )


def test_real_kimi_judge_two_versions_and_absent_case_alignment(live_comparison):
    assert live_comparison["baselineId"] != live_comparison["candidateId"]


def test_insights_records_real_kimi_success_and_token_usage(
    installed_agentx, service_urls, live_application, live_comparison
):
    app = live_application
    now = datetime.now(UTC)
    body = {
        "from": (now - timedelta(hours=2)).isoformat(),
        "to": (now + timedelta(hours=1)).isoformat(),
        "metrics": [
            "count",
            "succeeded_count",
            "failed_count",
            "duration_p50",
            "duration_p95",
            "input_tokens",
            "output_tokens",
            "cost_micros",
        ],
        "dimensions": ["workflow"],
        "filters": {"workflowId": app["workflowId"]},
        "limit": 10,
    }
    with httpx.Client(
        base_url=service_urls["web"], headers={"Authorization": f"Bearer {app['token'].value}"}, timeout=60
    ) as control:
        deadline = time.monotonic() + 120
        while True:
            response = control.post("/api/v1/insights/aggregates", json=body)
            assert response.status_code == 200, response.text
            rows = response.json()["rows"]
            if rows:
                break
            assert time.monotonic() < deadline, response.text
            time.sleep(1)
        assert all(row["dimensions"]["workflow"] == app["workflowId"] for row in rows), rows
        assert all(row["metrics"]["succeededCount"] > 0 and row["metrics"]["durationP95"] > 0 for row in rows), rows
        write_report(installed_agentx, "product/live-kimi-insights.json", {"status": "passed", "rows": rows})
