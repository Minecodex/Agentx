"""One shared priced evaluation package for API and browser assertions."""

# ruff: noqa: S608 -- IDs belong to the isolated typed E2E API.

from __future__ import annotations

import json
import time
from decimal import ROUND_HALF_UP, Decimal

import pytest

from tests.e2e.product.remaining_support import evaluation, get, model_workflow, new_model, profile
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql


def cost(input_tokens, output_tokens):
    return int(
        (Decimal(input_tokens) * Decimal("1.25") + Decimal(output_tokens) * Decimal("2.5")).quantize(
            Decimal(1), rounding=ROUND_HALF_UP
        )
    )


def ledger(state, execution):
    rows = json.loads(
        _runtime_mysql(
            state["context"],
            "SELECT COALESCE(JSON_ARRAYAGG(JSON_OBJECT('inputTokens',input_tokens,'outputTokens',output_tokens,'costMicros',cost_micros,'status',status)),JSON_ARRAY()) FROM runtime_calls "
            + f"WHERE execution_id=UUID_TO_BIN('{execution}') AND call_kind='model';",
        )
    )
    assert rows and all(r["status"] == "succeeded" for r in rows), rows
    assert all(r["costMicros"] == cost(r["inputTokens"], r["outputTokens"]) and r["costMicros"] > 0 for r in rows), rows
    deadline = time.monotonic() + 60
    while True:
        detail = get(state["control"], f"/executions/{execution}")
        if detail["costMicros"] == sum(r["costMicros"] for r in rows):
            return rows, detail
        assert time.monotonic() < deadline, detail
        time.sleep(0.5)


@pytest.fixture(scope="session")
def five_comparisons(boundary_state):
    state = boundary_state
    model = new_model(state, "judge-priced")
    workflow = model_workflow(state, model, "five comparisons")
    value = profile(state["control"], model, f"Boundary priced Judge {state['runId']}")
    ids = []
    reports = []
    for i, keys in enumerate(
        (
            ("shared", "baseline_only"),
            ("shared", "candidate_only"),
            ("shared",),
            ("shared", "fourth_only"),
            ("shared", "fifth_only"),
        )
    ):
        run_id, result = evaluation(
            state["control"], workflow, value, f"Boundary comparison {i} {state['runId']}", keys
        )
        assert result["run"]["status"] == "completed", result
        for case in result["results"]:
            judge = case["ruleResults"][0]
            assert judge["status"] == "passed" and judge["passed"] is True, judge
            _calls, detail = ledger(state, judge["evaluatorExecutionId"])
            assert judge["costMicros"] == detail["costMicros"] and judge["costMicros"] > 0, judge
            _target, target_detail = ledger(state, case["targetExecutionId"])
            assert case["costMicros"] == target_detail["costMicros"] + judge["costMicros"], case
        ids.append(run_id)
        reports.append(result)
    return {"state": state, "ids": ids, "reports": reports}
