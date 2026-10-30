"""Card, route, desktop visual and debug acceptance in an isolated installation."""

from __future__ import annotations

import os
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.test_provider_integration import _sandbox_image_digest
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_workflow_canvas_routing(installed_agentx, service_urls):
    with httpx.Client(base_url=service_urls["web"], timeout=30) as client:
        _access_token(client)
    run_playwright(
        Path(installed_agentx["root"]),
        "workflow-canvas-routing",
        (
            "tests/workflow-canvas-routing.spec.ts",
            "tests/workflow-loop-input-regressions.spec.ts",
            "tests/workflow-version-canvases.spec.ts",
            "tests/m6-workflow-studio.spec.ts",
            "--grep",
            "Canvas error exits|Canvas routing|Canvas cards|Workflow Loop supports|Version canvases:|M6 Studio builds and reopens|M6 Studio renders all thirteen",
        ),
        {
            **os.environ,
            "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
            "AGENTX_E2E_STAGE": "canvas-routing",
            "AGENTX_E2E_BASE_URL": service_urls["web"],
            "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
            "AGENTX_E2E_SANDBOX_IMAGE": os.getenv("AGENTX_E2E_SANDBOX_IMAGE") or _sandbox_image_digest(),
        },
    )
