"""Frozen capacity gates from docs/planv2/evidence/capacity-thresholds.md.

Missing, non-finite or negative measurements fail rather than defaulting to
zero. This validator is shared by the E2E producer and the release gate.
"""

from __future__ import annotations

import math
from typing import Any

LIMITS = {
    "gateway": {"nonSuccessRate": 0.005, "acceptP95Ms": 500, "acceptP99Ms": 2000, "rejected429Rate": 0.05},
    "sse": {"connectFailureRate": 0.01, "reconnectFailureRate": 0.01, "catchupSeconds": 10, "terminalDrainSeconds": 30},
    "queues": {
        "oldestReadySeconds": 60,
        "oldestOutboxSeconds": 60,
        "oldestInboxSeconds": 60,
        "oldestTraceSeconds": 60,
        "recoverySeconds": 120,
    },
    "mysql": {"connections": 120, "poolWaitP95Ms": 100, "lockWaitP95Ms": 200, "deadlocks": 0, "slowQueryRate": 0.001},
    "redis": {"usedMemoryBytes": 512 * 1024 * 1024, "pending": 5000, "consumerLag": 2000, "rebuildSeconds": 300},
    "provider": {"poolUtilization": 0.9, "queueTimeouts": 0, "circuitRecoverySeconds": 120},
    "residual": {
        "leases": 0,
        "reservations": 0,
        "holds": 0,
        "businessOutbox": 0,
        "businessInbox": 0,
        "taskReceipts": 0,
        "diagnosticBacklog": 10,
    },
}
REQUIRED_SCENARIOS = {
    "100-executions",
    "500-nodes",
    "200-sse",
    "1000-cases",
    "200-node-workflow",
    "5000-attempts",
    "2-hour-stability",
}


def _valid_measurement(value: Any) -> bool:
    return (type(value) is int or (type(value) is float and math.isfinite(value))) and value >= 0


def capacity_problems(report: dict[str, Any], *, require_matrix: bool = True) -> list[str]:
    problems = []
    groups = report.get("groups", {})
    for group, measurements in LIMITS.items():
        values = groups.get(group, {})
        for field, maximum in measurements.items():
            value = values.get(field)
            if not _valid_measurement(value):
                problems.append(f"{group}.{field}: missing or invalid measurement")
            elif value > maximum:
                problems.append(f"{group}.{field}: {value} exceeds {maximum}")
    # Every graded intake run must meet the same frozen thresholds. A long
    # low-load soak must not hide the percentile of an earlier failing burst.
    for row in report.get("matrix", []):
        if "load" not in row:
            continue
        values = row["load"]
        if require_matrix and values.get("transport") != "in-cluster-http":
            problems.append(f"matrix:{row.get('scenario')}: load was not generated inside the cluster")
        for field, maximum in LIMITS["gateway"].items():
            value = values.get(field)
            if not _valid_measurement(value):
                problems.append(f"matrix:{row.get('scenario')}.{field}: missing or invalid measurement")
            elif value > maximum:
                problems.append(f"matrix:{row.get('scenario')}.{field}: {value} exceeds {maximum}")
        if any(
            type(values.get(field)) is not int or values[field] <= 0
            for field in ("totalRequests", "completedExecutions")
        ):
            problems.append(f"matrix:{row.get('scenario')}: no valid accepted and completed execution counts")
        if not report.get("overload") and values.get("rejected429Rate") != 0:
            problems.append(f"matrix:{row.get('scenario')}: unexpected 429")
    gateway = groups.get("gateway", {})
    if any(
        type(gateway.get(field)) is not int or gateway[field] <= 0 for field in ("totalRequests", "completedExecutions")
    ):
        problems.append("gateway: no accepted and completed execution evidence")
    if not report.get("overload") and gateway.get("rejected429Rate") != 0:
        problems.append("gateway: 429 responses are forbidden outside overload scenarios")
    live = groups.get("sse", {}).get("peakLiveConnections")
    if not _valid_measurement(live) or live < 200:
        problems.append("sse: fewer than 200 simultaneous live connections observed")
    if groups.get("residual", {}).get("diagnosticBacklog", 0) and not report.get("diagnosticBacklogExplanation"):
        problems.append("residual: diagnostic backlog has no explanation")
    if require_matrix:
        if report.get("baselineVerified") is not True:
            problems.append("baseline: the frozen hardware was not verified")
        scenarios = {item.get("scenario") for item in report.get("matrix", []) if item.get("status") == "passed"}
        if missing := REQUIRED_SCENARIOS - scenarios:
            problems.append(f"matrix: missing successful scenarios {sorted(missing)}")
        rows = {item.get("scenario"): item for item in report.get("matrix", [])}
        requirements = [
            ("100-executions", "load", "completedExecutions", 100),
            ("500-nodes", "observed", "nodes", 500),
            ("200-sse", "measurements", "connections", 200),
            ("200-sse", "measurements", "peakLiveConnections", 200),
            ("1000-cases", None, "completedCases", 1000),
            ("200-node-workflow", "observed", "workflowNodes", 200),
            ("5000-attempts", "observed", "attempts", 5000),
        ]
        for name, group, field, minimum in requirements:
            value = rows.get(name, {})
            if group:
                value = value.get(group, {})
            count = value.get(field)
            if type(count) is not int or count < minimum:
                problems.append(f"matrix: {name} did not observe {minimum} {field}")
        for name in ("runtime-gateway", "workflow-runtime", "workflow-worker", "observability"):
            if any(rows.get(f"replicas:{name}:{replicas}", {}).get("status") != "passed" for replicas in (1, 2, 3, 4)):
                problems.append(f"replicas: {name} has no complete independent 1/2/3/4 run")
        for replicas in (1, 2, 3, 4):
            targets = rows.get(f"replicas:runtime-gateway:{replicas}", {}).get("load", {}).get("gatewayTargets", {})
            if (
                not isinstance(targets, dict)
                or len(targets) != replicas
                or any(type(value) is not int or value <= 0 for value in targets.values())
            ):
                problems.append(f"replicas: runtime-gateway:{replicas} did not exercise every replica")
        samples = report.get("stabilityMeasurementSamples")
        gap = report.get("stabilityMaxMeasurementGapSeconds")
        if type(samples) is not int or samples < 360 or not _valid_measurement(gap) or gap > 30:
            problems.append("stability: insufficient continuous measurements")
        duration = report.get("stabilityDurationSeconds")
        if not _valid_measurement(duration) or duration < 7200:
            problems.append("stability: less than two hours observed")
        if set(report.get("replicasObserved", [])) != {1, 2, 3, 4}:
            problems.append("replicas: the 1/2/3/4 matrix is incomplete")
        if report.get("mixedWorkerVersionsVerified") is not True:
            problems.append("replicas: distinct worker versions were not verified")
    return problems
