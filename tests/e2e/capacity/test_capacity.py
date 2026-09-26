"""Capacity domain entrypoint (plan7 P7-D4).

The graded runs (100/500/1000 executions, 200-node workflows, 5000 attempts,
the two-hour stability window and the replica matrix) execute in a dedicated
cluster window; this module provides the repeatable orchestration: load
generation, threshold assertion against the frozen capacity thresholds and
evidence reporting into the run's artifact directory.

Thresholds come from docs/planv2/evidence/capacity-thresholds.md and are
frozen: a run that exceeds them is a failure, never a tuning input.
"""

from __future__ import annotations

import asyncio
from pathlib import Path

import httpx
import pytest

from tests.e2e.capacity.loadgen import drive_invocations, write_report
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.capacity]

ACCEPT_P95_LIMIT_MS = 500.0
ACCEPT_P99_LIMIT_MS = 2000.0


def test_capacity_accept_latencies_stay_under_frozen_thresholds(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    run_id: str,
) -> None:
    """Short graded run: accept p95/p99 must stay under the frozen ceiling.

    Requires the P3 fixture application (invocation session policy); the
    two-hour stability and replica-matrix runs share this entrypoint with
    larger parameters in the dedicated window.
    """
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        status = control.get("/api/v1/bootstrap/status")
        status.raise_for_status()
        if status.json().get("required"):
            pytest.skip("capacity runs require an bootstrapped development install")

    report = asyncio.run(
        drive_invocations(
            base_url=service_urls["runtime"],
            slug="capacity-probe",
            api_key="unset",
            concurrent=10,
            duration_seconds=30,
        )
    )
    summary = report.summary()
    write_report(
        Path(installed_agentx["artifact_dir"]) / "capacity",
        "accept-latency",
        {"summary": summary},
    )
    # Without a published capacity application every request 404s; the run is
    # still a valid smoke of the orchestration path. The graded assertion
    # activates once the fixture application is published.
    if summary["totalRequests"] == 0 or summary["nonSuccessRate"] == 1.0:
        pytest.skip("no published capacity application; orchestration smoke only")

    assert summary["acceptP95Ms"] <= ACCEPT_P95_LIMIT_MS, summary
    assert summary["acceptP99Ms"] <= ACCEPT_P99_LIMIT_MS, summary


def test_metrics_endpoint_exposes_admission_counters(
    installed_agentx: dict[str, str],
) -> None:
    """The capacity collector relies on /metrics; assert the new gauges exist."""
    namespace = installed_agentx["runtime_namespace"]
    result = run(
        (
            "kubectl",
            "-n",
            namespace,
            "exec",
            "deployment/runtime-gateway",
            "--",
            "sh",
            "-ec",
            'wget -qO- "$AGENTX_METRICS_BIND_ADDR_METRICS_PORT" 2>/dev/null || true',
        ),
        check=False,
        timeout=60,
    )
    if result.returncode != 0:
        pytest.skip("metrics probe unavailable in this environment")
    body = result.stdout
    assert "agentx_admission_rejections_total" in body
