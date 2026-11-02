"""Desktop appearance and real account switching on an unmodified generated page."""

import json
from pathlib import Path

import pytest

from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.plan9.test_page_boundaries import active
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_generated_page_desktop_themes_languages_and_logout(plan9_suite):
    suite = plan9_suite
    case = next(row for row in suite["cases"] if row["scene"] == "S01")
    _, page = active(suite, case["applicationId"])
    report = suite["directory"] / "page-desktop-appearance.json"
    run_playwright(
        Path(suite["installed"]["root"]),
        "plan9-page-appearance",
        ("tests/plan9-page-appearance.spec.ts", "--workers=1"),
        {
            **browser_environment(suite),
            "AGENTX_E2E_PLAN9_APPEARANCE": json.dumps(
                {"scenario": case, "suffix": page["slug"], "report": str(report)}, ensure_ascii=False
            ),
        },
    )
    result = json.loads(report.read_text())
    assert len(result["appearance"]) == 4 and result["accountSwitch"]["detachedOriginalFrame"]
