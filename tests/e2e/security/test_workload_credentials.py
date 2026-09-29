"""plan7 P7-D7: role-level ServiceAccount and credential matrix assertions.

Every serving workload must run under its own dedicated ServiceAccount and
may only reference secrets belonging to its own plane.
"""

from __future__ import annotations

import json

import pytest

from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.security]

WORKLOADS = [
    # The control plane shares one dedicated ServiceAccount; runtime and
    # dependencies services each get their own.
    ("control", "platform-control", "platform-control"),
    ("control", "web-console", "platform-control"),
    ("runtime", "runtime-gateway", "runtime-gateway"),
    ("runtime", "workflow-runtime", "workflow-runtime"),
    ("runtime", "workflow-worker", "workflow-worker"),
    ("runtime", "sandbox-manager", "sandbox-manager"),
    ("runtime", "observability", "observability"),
    ("dependencies", "agentx-egress-gateway", "agentx-egress-gateway"),
]


def _deployment(installed_agentx: dict[str, str], plane: str, name: str) -> dict:
    namespace = installed_agentx[f"{plane}_namespace"]
    return run(("kubectl", "-n", namespace, "get", "deployment", name, "-o", "json"), timeout=60).json()


@pytest.mark.parametrize("plane,name,expected_sa", WORKLOADS)
def test_serving_workload_uses_dedicated_service_account(
    installed_agentx: dict[str, str], plane: str, name: str, expected_sa: str
) -> None:
    deployment = _deployment(installed_agentx, plane, name)
    assert deployment["spec"]["template"]["spec"]["serviceAccountName"] == expected_sa, name


def test_workloads_only_reference_plane_secrets(installed_agentx: dict[str, str]) -> None:
    """Env secretKeyRefs and volumes must stay inside the workload's plane.

    The shared egress TLS CA (agentx-egress-tls) is legitimately consumed by
    egress clients in every plane, so it is exempt from the plane check.
    """
    foreign_markers = {
        "control": ("agentx-runtime-", "agentx-dependencies-"),
        "runtime": ("agentx-control-",),
        "dependencies": ("agentx-control-",),
    }
    for plane, name, _sa in WORKLOADS:
        deployment = _deployment(installed_agentx, plane, name)
        spec = json.dumps(deployment["spec"]["template"]["spec"])
        for marker in foreign_markers[plane]:
            assert marker not in spec, f"{name} references foreign secrets via {marker}"
