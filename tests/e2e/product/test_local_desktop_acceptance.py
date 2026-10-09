"""Desktop and Python sandbox checks that do not require a cloud model key."""

from __future__ import annotations

import os
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.test_provider_integration import _sandbox_image_digest
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_desktop_themes_department_scope_and_real_python_sandbox(installed_agentx, service_urls):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as client:
        _access_token(client)
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        "AGENTX_E2E_SANDBOX_IMAGE": _sandbox_image_digest(),
    }
    run_playwright(
        Path(installed_agentx["root"]),
        "local-desktop-acceptance",
        ("tests/local-desktop-acceptance.spec.ts",),
        environment,
    )
