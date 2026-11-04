"""Only this Python entry point prepares and launches the formal browser business matrix."""

import json
import os
from pathlib import Path

import pytest

from tests.e2e.product.plan10.resume import completed_page
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_three_real_generated_pages_six_formal_batches_eighteen_items(plan10_suite, plan10_paid_gate):
    suite = plan10_suite
    scenarios = suite["directory"] / "scenarios.json"
    scenarios.write_text(
        json.dumps(
            [{key: value for key, value in case.items() if key != "secret"} for case in suite["cases"]],
            ensure_ascii=False,
            indent=2,
        )
    )
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": suite["runId"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": suite["urls"]["web"],
        "AGENTX_E2E_RUNTIME_URL": suite["urls"]["runtime"],
        "AGENTX_E2E_PLAN10_SCENARIOS": str(scenarios),
        "AGENTX_E2E_PLAN10_DIRECTORY": str(suite["directory"]),
        "AGENTX_E2E_PLAN10_BUDGET_FILE": str(
            Path(suite["installed"]["root"]) / ".local/artifacts/plan10/model-budget.json"
        ),
        "AGENTX_E2E_PLAN9_EMPLOYEE": suite["employee"]["username"],
        "AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD": suite["employee"]["password"].value,
        "AGENTX_E2E_PLAN9_REVIEWERS": json.dumps(
            {
                key: {"username": value["username"], "password": value["password"].value}
                for key, value in suite["users"].items()
            }
        ),
    }
    missing = [case["kind"] for case in suite["cases"] if not completed_page(suite["directory"], case["kind"])]
    if not suite.get("resumed"):
        missing = [case["kind"] for case in suite["cases"]]
    environment["AGENTX_E2E_PLAN10_RESUME"] = "1" if suite.get("resumed") else "0"
    environment["AGENTX_E2E_PLAN10_DIAGNOSTICS"] = json.dumps(suite.get("resumeDiagnostics", {}))
    if suite.get("resumed"):
        run_playwright(
            Path(suite["installed"]["root"]),
            "plan10-check-visibility",
            ("tests/plan10-check-visibility.spec.ts", "--workers=1"),
            environment,
        )
    if missing:
        run_playwright(
            Path(suite["installed"]["root"]),
            "plan10-real-batches",
            ("tests/plan10-real-batches.spec.ts", "--workers=1", "--grep", "(" + "|".join(missing) + "):"),
            environment,
        )
    complete = [
        json.loads((suite["directory"] / f"{case['kind']}-complete.json").read_text()) for case in suite["cases"]
    ]
    assert len(complete) == 3 and all(result["passed"] for result in complete)
    assert all(completed_page(suite["directory"], case["kind"]) for case in suite["cases"]), (
        "Failed visual review of the generated HTML keeps the real-page gate incomplete"
    )
    batches = [batch for result in complete for batch in result["batches"]]
    assert len({batch["batch"]["batchId"] for batch in batches}) >= 6
    assert sum(len(batch["projects"]) for batch in batches) >= 18
    (suite["directory"] / "real-matrix.json").write_text(
        json.dumps(
            {
                "passed": True,
                "generatedPages": 3,
                "formalBatches": len(batches),
                "formalItems": sum(len(batch["projects"]) for batch in batches),
            },
            indent=2,
        )
    )
