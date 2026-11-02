"""Browser proof for the page isolation architecture, without any business capability."""

from __future__ import annotations

import os
import shutil

from tests.e2e.support import ROOT, run_playwright


def test_page_isolation_spike(run_id):
    suite = "plan9-isolation"
    environment = {**os.environ, "AGENTX_E2E_STAGE": "plan9-spike", "AGENTX_E2E_RUN_ID": run_id}
    try:
        run_playwright(
            ROOT,
            suite,
            (
                "tests/application-isolation.spec.ts",
                "tests/plan9-window-boundary.spec.ts",
                "tests/plan9-preview-regression.spec.ts",
                "tests/plan9-page-checks.spec.ts",
                "tests/plan9-chat-preview.spec.ts",
                "tests/plan9-http-actions.spec.ts",
                "tests/plan9-page-download.spec.ts",
            ),
            environment,
        )
    finally:
        report = ROOT / ".local/artifacts/playwright/plan9-spike" / run_id / suite
        if report.exists():
            destination = ROOT / ".local/artifacts/e2e" / run_id / "browser" / suite
            shutil.copytree(report, destination, dirs_exist_ok=True)
