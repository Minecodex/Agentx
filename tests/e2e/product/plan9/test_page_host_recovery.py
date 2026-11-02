"""A real page Agent keeps its pending question and original Effect after the shared Host is replaced."""

import copy
import json
import threading
import uuid
from pathlib import Path

import pytest

from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.plan9.test_real_pages import assemble_page_results
from tests.e2e.product.studio_builder_faults import studio_faults as studio_faults
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


@pytest.mark.parametrize("studio_faults", [None], indirect=True)
def test_real_page_agent_recovers_its_original_question(plan9_suite, studio_faults):
    suite = plan9_suite
    scenario = copy.deepcopy(next(row for row in suite["cases"] if row["scene"] == "S01"))
    scenario["scene"] = "S01-recovery"
    scenario["slug"] += "-recovery"
    inputs = suite["directory"] / "host-recovery-scenario.json"
    inputs.write_text(json.dumps([scenario], ensure_ascii=False, indent=2))
    receipt = suite["directory"] / "host-recovery-fault.json"
    report = suite["directory"] / "host-recovery-page-results.json"
    stopped = threading.Event()

    def notify():
        while not stopped.wait(0.2):
            if studio_faults:
                temporary = receipt.with_suffix(".tmp")
                temporary.write_text(json.dumps(studio_faults[0], indent=2))
                temporary.replace(receipt)
                return

    notifier = threading.Thread(target=notify, daemon=True)
    notifier.start()
    try:
        run_playwright(
            Path(suite["installed"]["root"]),
            "plan9-page-host-recovery",
            ("tests/plan9-real-pages.spec.ts", "--workers=1"),
            {
                **browser_environment(suite),
                "AGENTX_E2E_PLAN9_SCENARIOS": str(inputs),
                "AGENTX_E2E_PLAN9_REPORT": str(report),
                "AGENTX_E2E_PLAN9_ROUNDS": "1",
                "AGENTX_E2E_PLAN9_HOST_RECOVERY_RECEIPT": str(receipt),
            },
        )
        results = assemble_page_results(report)
        assert len(results) == 1 and results[0]["passed"] and len(results[0]["executions"]) == 2, results
        result = results[0]
        identifier = uuid.UUID(result["attempts"][0]["run"]["id"]).hex
        assert uuid.UUID(hex=result["hostRecovery"]["runId"]).hex == identifier
        run_id = str(uuid.UUID(hex=identifier))
        counts = _runtime_mysql(
            suite["installed"],
            f"SELECT COUNT(*),COUNT(DISTINCT idempotency_key),SUM(status='succeeded') FROM runtime_calls WHERE source_kind='application_ui_builder' AND JSON_UNQUOTE(JSON_EXTRACT(design_target_json,'$.target.runId'))='{run_id}'",  # noqa: S608 -- UUID parsed from the actual Run result.
        )
        total, distinct, succeeded = map(int, counts.split())
        assert total == distinct == succeeded and total > 0, counts
        entries = result["attempts"][-1]["entries"]
        usages = {
            row["payload"]["effectId"]
            for row in entries
            if row["payload"]["type"] == "usage" and row["runId"] == run_id
        }
        assert len(usages) == total
        questions = [row for row in entries if row["payload"]["type"] == "question"]
        answers = [row for row in entries if row["payload"]["type"] == "answer"]
        assert len(questions) == len(answers) == 1
        assert questions[0]["payload"]["questionId"] == answers[0]["payload"]["questionId"]
        (suite["directory"] / "host-recovery-proof.json").write_text(
            json.dumps(
                {
                    "runId": run_id,
                    "providerCalls": total,
                    "uniqueProviderIdentities": distinct,
                    "usageEntries": len(usages),
                    "questionId": questions[0]["payload"]["questionId"],
                },
                indent=2,
            )
        )
    finally:
        stopped.set()
        notifier.join(timeout=5)
        assemble_page_results(report)
