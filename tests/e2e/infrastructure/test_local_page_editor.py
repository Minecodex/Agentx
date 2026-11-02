"""Explicit local authoring acceptance; preserves the application and publications for review."""

from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

import pytest

from tests.e2e.support import ROOT, run_playwright

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.infrastructure,
    pytest.mark.skipif(
        not os.environ.get("AGENTX_LOCAL_PAGE_EDITOR_CONFIG"),
        reason="Requires explicit retained local editor configuration and authorized model calls",
    ),
]


def test_retained_page_editor_real_chat_publish_preview_and_rollback():
    config = json.loads(Path(os.environ["AGENTX_LOCAL_PAGE_EDITOR_CONFIG"]).read_text())
    assert config["baseUrl"] == "https://agentx.localhost" or (
        config.get("sourcePreview") and config["baseUrl"].startswith("http://127.0.0.1:")
    )
    directory = Path(config["evidenceDir"])
    arguments = ("tests/local-page-editor.spec.ts",)
    if grep := os.environ.get("AGENTX_E2E_PLAYWRIGHT_GREP"):
        arguments = (*arguments, "--grep", grep)
    try:
        run_playwright(
            ROOT,
            "continuous-page-editor",
            arguments,
            {
                **os.environ,
                "AGENTX_E2E_BASE_URL": config["baseUrl"],
                "AGENTX_E2E_STAGE": "local-editor",
                "AGENTX_E2E_RUN_ID": config["runId"],
                "NODE_EXTRA_CA_CERTS": config["caFile"],
            },
        )
    finally:
        output = ROOT / ".local/artifacts/playwright/local-editor" / config["runId"] / "continuous-page-editor"
        for filename in (
            "continuous-page-editor.jpg",
            "continuous-page-editor-final.jpg",
            "page-publication-history.jpg",
        ):
            matches = list(output.rglob(filename)) if output.exists() else []
            if matches:
                shutil.copyfile(max(matches, key=lambda path: path.stat().st_mtime_ns), directory / filename)
    if grep and "Visible editor" in grep:
        report = json.loads((directory / "editor-layout-verification.json").read_text())
        assert report["passed"] and report["currentHtmlAndRunsUnchanged"]
        assert report["unexpectedModelOrBusinessRequests"] == []
        return
    report = json.loads((directory / "editor-business-test.json").read_text())
    assert report["passed"]
    assert report["rollback"]["workingHtmlUnchanged"]
    assert report["rollback"]["conversationRunsUnchanged"]
    assert report["reopened"]["sameSession"] and report["reopened"]["sameHtml"]
