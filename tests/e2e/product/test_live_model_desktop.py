"""Browser acceptance against the real Kimi application and Judge reports."""

import os
from pathlib import Path

import pytest

from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_real_model_playground_history_stop_trace_and_comparison(
    installed_agentx, service_urls, live_application, live_comparison
):
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        "AGENTX_E2E_LIVE_APP_ID": live_application["applicationId"],
        "AGENTX_E2E_COMPARE_BASELINE": live_comparison["baselineId"],
        "AGENTX_E2E_COMPARE_CANDIDATE": live_comparison["candidateId"],
        "AGENTX_E2E_JUDGE_MODEL_ALIAS": live_application["modelAlias"],
    }
    run_playwright(
        Path(installed_agentx["root"]), "live-model-desktop", ("tests/live-model-desktop.spec.ts",), environment
    )
