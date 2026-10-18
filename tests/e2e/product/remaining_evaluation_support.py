"""One shared priced evaluation package for API and browser assertions."""

# ruff: noqa: S608 -- IDs belong to the isolated typed E2E API.

from __future__ import annotations

import json
import time
from decimal import ROUND_HALF_UP, Decimal

from tests.e2e.product.remaining_support import get
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql


def cost(input_tokens, output_tokens, input_price, output_price):
    return int(
        (Decimal(input_tokens) * Decimal(input_price) + Decimal(output_tokens) * Decimal(output_price)).quantize(
            Decimal(1), rounding=ROUND_HALF_UP
        )
    )


def ledger(state, execution, input_price="1.25", output_price="2.5"):
    rows = json.loads(
        _runtime_mysql(
            state["context"],
            "SELECT COALESCE(JSON_ARRAYAGG(JSON_OBJECT('inputTokens',input_tokens,'outputTokens',output_tokens,'costMicros',cost_micros,'status',status)),JSON_ARRAY()) FROM runtime_calls "
            + f"WHERE execution_id=UUID_TO_BIN('{execution}') AND call_kind='model';",
        )
    )
    assert rows and all(r["status"] == "succeeded" for r in rows), rows
    assert all(
        r["costMicros"] == cost(r["inputTokens"], r["outputTokens"], input_price, output_price) and r["costMicros"] > 0
        for r in rows
    ), rows
    deadline = time.monotonic() + 60
    while True:
        detail = get(state["control"], f"/executions/{execution}")
        if detail["costMicros"] == sum(r["costMicros"] for r in rows):
            return rows, detail
        assert time.monotonic() < deadline, detail
        time.sleep(0.5)
