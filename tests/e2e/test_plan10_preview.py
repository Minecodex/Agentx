"""Production batch SDK/inspector browser checks, through the one E2E entry point."""

import os

from tests.e2e.support import ROOT, run_playwright


def test_batch_page_inspector_browser_contract(run_id):
    run_playwright(
        ROOT,
        "plan10-preview",
        ("tests/plan10-batch-preview.spec.ts", "tests/plan9-page-checks.spec.ts", "tests/plan9-chat-preview.spec.ts"),
        {**os.environ, "AGENTX_E2E_STAGE": "plan10", "AGENTX_E2E_RUN_ID": run_id},
    )
