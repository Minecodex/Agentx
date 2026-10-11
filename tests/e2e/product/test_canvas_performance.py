"""Measure canvas budgets in an isolated run, retaining the real installed UI/API."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import httpx
import pytest

from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.browser_performance]


def test_workflow_canvas_performance(installed_agentx, service_urls):
    with httpx.Client(base_url=service_urls["control"], timeout=30) as client:
        _access_token(client)
    root = Path(installed_agentx["root"])
    pnpm = ("corepack.cmd", "pnpm") if os.name == "nt" else ("corepack", "pnpm")
    for action in ("build", "pack:plugin"):
        subprocess.run([*pnpm, "--filter", "agentx-canvas-plugin-template", action], cwd=root, check=True)
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_E2E_STAGE": "isolated-canvas-performance",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
    }
    run_playwright(
        root, "workflow-performance", ("tests/workflow-canvas-performance.spec.ts", "--workers=1"), environment
    )
