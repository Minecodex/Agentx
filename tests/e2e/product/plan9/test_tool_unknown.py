"""Lose the actual non-idempotent provider reply and prove that recovery never replays the write."""

import copy
import json
import time

import pytest

from tests.e2e.product.plan9.mcp_support import TERMINAL, application_client, crm_records, decide
from tests.e2e.product.plan9.test_tool_recovery import effect
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_lost_tool_reply_stays_unknown_and_is_not_replayed(plan9_suite):
    suite = plan9_suite
    case = copy.deepcopy(next(value for value in suite["cases"] if value["scene"] == "S17"))
    case["inputs"][0]["request_id"] += "-lost-response"
    namespace = suite["installed"]["runtime_namespace"]
    assert namespace.startswith("agentx-e2e-runtime-") and namespace.endswith(suite["runId"])
    report = {"passed": False}
    try:
        with application_client(suite, case, "Lost CRM response") as (client, published, _):
            reference = case["inputs"][0]["request_id"]
            before = len(crm_records(suite, reference))
            task = client.submit(published["toolName"], published["revision"], case["inputs"][0])
            deadline = time.monotonic() + 300
            while True:
                current = client.task(task["taskId"])
                if current["pendingApprovals"]:
                    break
                assert current["state"] not in TERMINAL and time.monotonic() < deadline, current
                time.sleep(0.5)
            assert len(crm_records(suite, reference)) == before
            human = decide(
                suite, case, current["pendingApprovals"][0]["approvalId"], case["approvalDecisions"][0][0], "lost-reply"
            )
            deadline = time.monotonic() + 300
            while (final := client.task(task["taskId"]))["state"] not in TERMINAL:
                assert time.monotonic() < deadline, final
                time.sleep(0.5)
            assert final["state"] == "outcome_unknown", final
            assert len(crm_records(suite, reference)) == before + 1
            pending = effect(suite, task["taskId"])
            assert pending["unknown"] and not pending["settled"], pending
            run(("kubectl", "-n", namespace, "rollout", "restart", "deployment/workflow-worker"))
            run(
                ("kubectl", "-n", namespace, "rollout", "status", "deployment/workflow-worker", "--timeout=180s"),
                timeout=200,
            )
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                assert client.task(task["taskId"])["state"] == "outcome_unknown"
                assert len(crm_records(suite, reference)) == before + 1
                time.sleep(0.5)
            restored = effect(suite, task["taskId"])
            assert restored == pending
            report.update(
                passed=True,
                task=final,
                effect=pending,
                humanDecision=human,
                writesBefore=before,
                writesAfter=len(crm_records(suite, reference)),
                records=client.records,
            )
    finally:
        (suite["directory"] / "tool-lost-reply.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
