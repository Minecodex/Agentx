"""Nonzero prices, five-run baseline order and deterministic Judge failures."""

from __future__ import annotations

import time
from datetime import UTC, datetime, timedelta

import pytest

from tests.e2e.product.remaining_evaluation_support import ledger
from tests.e2e.product.remaining_support import (
    authorize,
    evaluation,
    get,
    invoke,
    model_workflow,
    new_model,
    profile,
    reference,
    report,
)

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_real_kimi_nonzero_price_matches_calls_execution_and_insights(boundary_state, live_kimi_secret):
    state = boundary_state
    model = new_model(
        state, "priced-kimi", upstream="k3", endpoint="https://api.kimi.com/coding/v1", secret=live_kimi_secret.value
    )
    workflow = model_workflow(state, model, "priced Kimi")
    execution, _ = invoke(state, workflow["workflowId"], input_value={"message": "请只回复:水星费用验证"})
    rows, detail = ledger(state, execution)
    now = datetime.now(UTC)
    body = {
        "from": (now - timedelta(hours=1)).isoformat(),
        "to": (now + timedelta(hours=1)).isoformat(),
        "metrics": ["count", "cost_micros", "input_tokens", "output_tokens"],
        "dimensions": ["workflow"],
        "filters": {"workflowId": workflow["workflowId"]},
        "limit": 10,
    }
    deadline = time.monotonic() + 120
    while True:
        response = state["control"].post("/api/v1/insights/aggregates", json=body)
        assert response.status_code == 200, response.text
        values = response.json()["rows"]
        if values and sum(row["metrics"]["costMicros"] for row in values) == detail["costMicros"]:
            break
        assert time.monotonic() < deadline, values
        time.sleep(0.5)
    assert sum(row["metrics"]["inputTokens"] for row in values) == sum(r["inputTokens"] for r in rows), values
    assert sum(row["metrics"]["outputTokens"] for row in values) == sum(r["outputTokens"] for r in rows), values
    report(
        state,
        "remaining-real-kimi-cost",
        {
            "executionId": execution,
            "ledger": rows,
            "executionCostMicros": detail["costMicros"],
            "insights": values,
            "testPriceOnly": True,
        },
    )


def test_three_to_five_reports_preserve_baseline_missing_cases_and_costs(five_comparisons):
    state = five_comparisons["state"]
    ids = five_comparisons["ids"]
    evidence = []
    for count in (3, 4, 5):
        for order in (ids[:count], list(reversed(ids[:count]))):
            response = state["control"].get("/api/v1/evaluations/compare", params={"runIds": ",".join(order)})
            assert response.status_code == 200, response.text
            value = response.json()
            assert [r["runId"] for r in value["runs"]] == order, value
            assert value["alignedCaseCount"] == 1 and value["totalCaseCount"] == count, value
            shared = next(c for c in value["caseDeltas"] if c["caseKey"] == "shared")
            assert all(s == "completed" for s in shared["statusByRun"]), shared
            assert all(c > 0 for c in shared["costMicrosByRun"]), shared
            assert shared["costDelta"] == shared["costMicrosByRun"][1] - shared["costMicrosByRun"][0], shared
            assert any(None in c["statusByRun"] for c in value["caseDeltas"]), value
            evidence.append(value)
    for order in (ids[:1], [*ids, ids[0]], [ids[0], ids[0]]):
        response = state["control"].get("/api/v1/evaluations/compare", params={"runIds": ",".join(order)})
        assert response.status_code == 400, response.text
    report(state, "remaining-evaluation-five-runs", {"runIds": ids, "comparisons": evidence, "judgeCostIncluded": True})


@pytest.mark.parametrize("missing", ["model", "credential"])
def test_judge_requires_each_resource_grant(boundary_state, missing):
    state = boundary_state
    target = new_model(state, f"target-{missing}")
    workflow = model_workflow(state, target, f"target-{missing}")
    judge = new_model(state, f"unauthorized-{missing}")
    if missing == "credential":
        # A lone model grant cannot bypass the separate credential grant.
        identity = get(state["control"], f"/workflows/{workflow['workflowId']}")["serviceIdentityId"]
        from tests.e2e.product.live_text_support import post

        post(
            state["control"],
            f"/resources/model/{judge['id']}/grants",
            {
                "subjectType": "workflow_service_identity",
                "subjectId": identity,
                "resourceVersionId": judge["deploymentId"],
                "operation": "use",
            },
        )
    value = profile(state["control"], judge, f"Boundary missing {missing} {state['runId']}")
    run_id, refusal = evaluation(
        state["control"], workflow, value, f"Boundary refused {missing} {state['runId']}", expected_start=422
    )
    assert "GRANT" in refusal["code"], refusal
    report(state, f"remaining-judge-grant-{missing}", {"runId": run_id, "refusal": refusal})


@pytest.mark.parametrize("upstream", ["echo-invalid-judge", "echo-unavailable"])
def test_judge_invalid_or_failed_provider_never_passes(boundary_state, upstream):
    state = boundary_state
    target = new_model(state, f"target-{upstream}")
    workflow = model_workflow(state, target, f"target-{upstream}")
    judge = new_model(state, upstream, upstream=upstream)
    authorize(state["control"], workflow["workflowId"], reference("model", judge["id"], judge["deploymentId"]))
    value = profile(state["control"], judge, f"Boundary invalid Judge {upstream} {state['runId']}")
    run_id, result = evaluation(state["control"], workflow, value, f"Boundary invalid {upstream} {state['runId']}")
    rule = result["results"][0]["ruleResults"][0]
    assert rule["status"] in {"error", "failed"} and rule["passed"] is not True, rule
    assert rule["evaluatorExecutionId"], rule
    detail = get(state["control"], f"/executions/{rule['evaluatorExecutionId']}")
    assert detail["status"] in {"failed", "cancelled", "timed_out"}, detail
    report(
        state,
        f"remaining-judge-failure-{upstream}",
        {"runId": run_id, "rule": rule, "executionStatus": detail["status"]},
    )
