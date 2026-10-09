"""Desktop error recovery, real comparison and revoked-session UI closure."""

import json
import os
from pathlib import Path

import pytest

from tests.e2e.product.live_text_support import post
from tests.e2e.product.next_batch_support import create_user
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_desktop_loading_error_recovery_empty_permission_and_five_run_comparison(boundary_state, five_comparisons):
    state = boundary_state
    role = post(
        state["control"],
        "/roles",
        {
            "code": f"boundary_desktop_{state['runId']}",
            "name": f"Boundary desktop viewer {state['runId']}",
            "description": "Disposable least-privilege desktop viewer",
            "dataScope": "company",
            "permissions": ["workflow:view"],
        },
    )
    user = create_user(state["control"], f"p7-desktop-{state['runId']}", state["me"]["departmentId"], role["id"])
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": state["runId"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": state["urls"]["web"],
        "AGENTX_E2E_RUNTIME_URL": state["urls"]["runtime"],
        "AGENTX_E2E_FIVE_COMPARE_IDS": json.dumps(five_comparisons["ids"]),
        "AGENTX_E2E_FIVE_COMPARE_NAMES": json.dumps([r["run"]["name"] for r in five_comparisons["reports"]]),
        "AGENTX_E2E_PERMISSION_USER": user["username"],
        "AGENTX_E2E_PERMISSION_PASSWORD": user["password"].value,
        "AGENTX_E2E_PERMISSION_ROLE": json.dumps({k: role[k] for k in ("id", "name", "description", "version")}),
    }
    run_playwright(
        Path(state["context"]["root"]), "remaining-desktop", ("tests/remaining-desktop.spec.ts",), environment
    )
