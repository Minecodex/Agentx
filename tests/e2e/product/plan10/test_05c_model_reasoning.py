"""Persist explicit reasoning settings through the desktop UI without calling a model."""

import json
import os
from pathlib import Path

import pytest

from tests.e2e.product.next_batch_support import get
from tests.e2e.product.plan10.support import report
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_model_reasoning_ui_persists_max_and_desktop_preferences(plan10_suite):
    suite = plan10_suite
    ledger = Path(suite["installed"]["root"]) / ".local/artifacts/plan10/model-budget.json"
    before = json.loads(ledger.read_text())
    model = suite["model"]
    run_playwright(
        Path(suite["installed"]["root"]),
        "plan10-model-reasoning",
        ("tests/plan10-model-reasoning.spec.ts", "--workers=1"),
        {
            **os.environ,
            "AGENTX_E2E_RUN_ID": suite["runId"],
            "AGENTX_E2E_STAGE": "helm-agentxctl",
            "AGENTX_E2E_BASE_URL": suite["urls"]["web"],
            "AGENTX_E2E_RUNTIME_URL": suite["urls"]["runtime"],
            "AGENTX_E2E_PLAN10_MODEL_ID": model["id"],
            "AGENTX_E2E_PLAN10_DIRECTORY": str(suite["directory"]),
        },
    )
    after = get(suite["control"], f"/models/aliases/{model['id']}")
    assert after["defaultParameters"] == {**model["defaultParameters"], "reasoning_effort": "max"}
    assert after["maxOutputTokens"] == model["maxOutputTokens"] == 16000
    assert after["deploymentId"] != model["deploymentId"]
    assert after["aliasVersion"] == model["aliasVersion"] + 1
    assert get(suite["control"], "/platform-ai/model-setting")["status"] == "ready"
    assert json.loads(ledger.read_text()) == before, "Editing the model must not spend the real-model budget"
    suite["model"] = after
    report(
        suite,
        "E10-U08-model-reasoning",
        modelId=model["id"],
        before={key: model[key] for key in ("defaultParameters", "deploymentId", "aliasVersion")},
        after={key: after[key] for key in ("defaultParameters", "deploymentId", "aliasVersion", "maxOutputTokens")},
        modelRequestsBefore=before["requests"],
        modelRequestsAfter=before["requests"],
    )
