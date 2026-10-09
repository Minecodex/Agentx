"""Real chat mapping publication and desktop Runtime network error feedback."""

from __future__ import annotations

import os
from pathlib import Path

import httpx
import pytest

from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_playground_mapping_and_network_errors(installed_agentx, service_urls):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        _access_token(control)
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
    }
    run_playwright(
        Path(installed_agentx["root"]),
        "playground-errors",
        ("tests/playground-errors.spec.ts",),
        environment,
    )
