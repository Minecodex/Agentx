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


@pytest.mark.live_model
def test_real_model_nonzero_price_matches_calls_execution_and_insights(
    boundary_state, existing_model, existing_model_secret
):
    state = boundary_state
    model = new_model(
        state,
        "priced-live-model",
        upstream=existing_model["modelName"],
        endpoint=existing_model["endpoint"],
        secret=existing_model_secret.value,
        price={
            "currency": existing_model["currency"],
            "inputPerMillion": existing_model["inputPrice"],
            "outputPerMillion": existing_model["outputPrice"],
        },
    )
    workflow = model_workflow(state, model, "priced real model")
    execution, _ = invoke(state, workflow["workflowId"], input_value={"message": "请只回复:水星费用验证"})
    rows, detail = ledger(state, execution, existing_model["inputPrice"], existing_model["outputPrice"])
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
        "remaining-real-model-cost",
        {
            "executionId": execution,
            "ledger": rows,
            "executionCostMicros": detail["costMicros"],
            "insights": values,
            "sourceModel": existing_model["alias"],
        },
    )


@pytest.mark.parametrize("missing", ["model", "credential"])
def test_platform_judge_is_independent_of_business_resource_grants(boundary_state, missing):
    state = boundary_state
    target = new_model(state, f"target-{missing}")
    workflow = model_workflow(state, target, f"target-{missing}")
    judge = new_model(state, f"unauthorized-{missing}")
    if missing == "credential":
        # A legacy business-model grant is irrelevant to the independent platform judge authority.
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
    value = profile(state["control"], judge, f"Boundary independent {missing} {state['runId']}", workflow)
    run_id, result = evaluation(state["control"], workflow, value, f"Boundary independent {missing} {state['runId']}")
    case = result["runs"][0]["cases"][0]
    assert result["status"] == "completed" and case["status"] == "passed", result
    report(state, f"remaining-platform-judge-{missing}", {"runId": run_id, "case": case})


@pytest.mark.parametrize("upstream", ["echo-invalid-judge", "echo-unavailable"])
def test_judge_invalid_or_failed_provider_never_passes(boundary_state, upstream):
    state = boundary_state
    target = new_model(state, f"target-{upstream}")
    workflow = model_workflow(state, target, f"target-{upstream}")
    judge = new_model(state, upstream, upstream=upstream)
    authorize(state["control"], workflow["workflowId"], reference("model", judge["id"], judge["deploymentId"]))
    value = profile(state["control"], judge, f"Boundary invalid Judge {upstream} {state['runId']}", workflow)
    run_id, result = evaluation(state["control"], workflow, value, f"Boundary invalid {upstream} {state['runId']}")
    rule = result["runs"][0]["cases"][0]
    assert rule["status"] == "error" and rule["score"] is None, rule
    assert rule["judgeExecutionId"], rule
    detail = get(state["control"], f"/executions/{rule['judgeExecutionId']}")
    assert detail["status"] in {"failed", "cancelled", "timed_out"}, detail
    report(
        state,
        f"remaining-judge-failure-{upstream}",
        {"runId": run_id, "rule": rule, "executionStatus": detail["status"]},
    )
