"""Desktop browser closure for the candidate's real comparison reports."""

from __future__ import annotations

import os

import pytest

from tests.e2e.product import test_evaluation_comparison as comparisons
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]
comparison_application = comparisons.comparison_application


def test_plan7_comparison_and_judge_configuration_ui(installed_agentx, service_urls, comparison_application):
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        "AGENTX_E2E_COMPARE_BASELINE": comparison_application["baselineId"],
        "AGENTX_E2E_COMPARE_CANDIDATE": comparison_application["candidateId"],
        "AGENTX_E2E_JUDGE_MODEL_ALIAS": comparison_application["modelAlias"],
    }
    from pathlib import Path

    run_playwright(Path(installed_agentx["root"]), "plan7-ui", ("tests/plan7-review-fixes.spec.ts",), environment)
