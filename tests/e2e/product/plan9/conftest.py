"""Real-model fixtures for plan9; they never read a persistent deployment credential."""

import pytest

from tests.e2e.conftest import _free_port, _wait_http
from tests.e2e.product.plan9.metered_provider import plan9_metered_provider as plan9_metered_provider
from tests.e2e.product.plan9.public_origins import plan9_public_origins as plan9_public_origins
from tests.e2e.product.plan9.suite import plan9_suite as plan9_suite
from tests.e2e.support import RestartingPortForward


@pytest.fixture(scope="session")
def existing_model(plan9_metered_provider):
    return plan9_metered_provider


@pytest.fixture(scope="session")
def plan9_business_protocol(plan9_suite):
    """Verify formal business definitions before paying to author their pages."""
    from tests.e2e.product.plan9.test_mcp_protocol import verify_applications

    return verify_applications(plan9_suite)


@pytest.fixture(scope="session")
def plan9_fixture_url(installed_agentx, studio_provider):
    from pathlib import Path

    port = _free_port()
    directory = Path(installed_agentx["artifact_dir"])
    forward = RestartingPortForward(
        (
            "kubectl",
            "-n",
            installed_agentx["dependencies_namespace"],
            "port-forward",
            "service/echo-mcp",
            f"{port}:8090",
        ),
        stdout_path=directory / "plan9-crm-forward.log",
        stderr_path=directory / "plan9-crm-forward-errors.log",
        health_url=f"http://127.0.0.1:{port}/health",
    )
    try:
        _wait_http(forward, f"http://127.0.0.1:{port}/health")
        yield f"http://127.0.0.1:{port}"
    finally:
        forward.stop()
