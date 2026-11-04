"""Actual authoring renderers, orchestrated through the repository E2E entry point."""

import os

from tests.e2e.support import ROOT, run, run_playwright


def test_design_browser_actual_renderers(run_id):
    run(("pnpm", "--filter", "@agentx/web", "build:design-preview"), timeout=180)
    run_playwright(
        ROOT,
        "design-browser-preview",
        ("tests/design-browser-preview.spec.ts",),
        {**os.environ, "AGENTX_E2E_STAGE": "design-browser", "AGENTX_E2E_RUN_ID": run_id},
    )
