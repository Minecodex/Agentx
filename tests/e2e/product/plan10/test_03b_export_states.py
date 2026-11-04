"""A real branching workflow exercises every CSV state without model randomness."""

import csv
import io
import json
import uuid

import pytest

from tests.e2e.product.live_text_support import Secret, post
from tests.e2e.product.plan10.scenarios import export_states
from tests.e2e.product.plan10.support import (
    client,
    command,
    create,
    get,
    hexid,
    items,
    report,
    sql,
    unknown_receipt,
    wait,
)

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.gateway, pytest.mark.live_model]


def test_csv_preserves_all_states_and_separate_currencies(plan10_suite):
    suite = plan10_suite
    case = export_states(suite["control"], suite["runId"], suite["users"]["manager"]["id"], suite["profile"])
    key = post(suite["control"], f"/applications/{case['applicationId']}/api-keys", {"name": "Mixed state export"})
    case["secret"] = Secret(key["secret"])
    modes = [
        ("waiting", "wait"),
        ("approval", "approval"),
        ("complete-cny", "complete"),
        ("complete-usd", "complete"),
        ("failure", "fail"),
        ("unknown", "unknown"),
        ("cancelled", "cancel"),
        *[(f"running-{i}", "run") for i in range(4)],
        ("queued", "run"),
    ]
    rows = [
        {"request_id": identifier, "mode": mode, "amount": 10.25 + index, "currency": "USD" if index % 2 else "CNY"}
        for index, (identifier, mode) in enumerate(modes)
    ]
    with client(suite, case) as http:
        wait(
            lambda: http.get(f"/gateway/v1/applications/{case['slug']}/batches").status_code, lambda value: value == 200
        )
        batch = create(http, case, rows)
        try:

            def ready():
                current = {item["input"]["request_id"]: item for item in items(http, batch["batchId"])["items"]}
                return current

            initial = wait(
                ready,
                lambda current: (
                    current["waiting"]["state"] == "waiting_input"
                    and current["approval"]["state"] == "waiting_approval"
                    and current["cancelled"]["state"] == "waiting_input"
                    and all(
                        current[value]["state"] == "completed" for value in ("complete-cny", "complete-usd", "unknown")
                    )
                    and current["failure"]["state"] == "failed"
                    and all(current[f"running-{i}"]["state"] == "running" for i in range(4))
                    and current["queued"]["task"] is None
                ),
                240,
            )
            cancelled = initial["cancelled"]["task"]["taskId"]
            receipt = http.post(
                f"/gateway/v1/invocations/{cancelled}/cancel", headers={"Idempotency-Key": str(uuid.uuid4())}
            )
            assert receipt.status_code == 202, receipt.text
            wait(ready, lambda current: current["cancelled"]["state"] == "cancelled")
            with unknown_receipt(suite, initial["unknown"]["task"]["taskId"]):
                snapshot = get(http, batch["batchId"])
                assert snapshot["counts"]["outcomeUnknown"] == 1 and snapshot["counts"]["ended"] == 4
                invocation_count = sql(
                    suite,
                    f"SELECT COUNT(*) FROM application_invocations WHERE application_id=UNHEX('{hexid(case['applicationId'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
                )
                exported = command(http, batch["batchId"], "exports")
                job = wait(
                    lambda: http.get(f"/gateway/v1/batches/{batch['batchId']}/exports/{exported['exportId']}").json(),
                    lambda value: value["status"] in ("completed", "failed"),
                )
                assert job["status"] == "completed", job
                downloaded = http.get(f"/gateway/v1/batches/{batch['batchId']}/exports/{job['exportId']}/file")
                assert downloaded.status_code == 200, downloaded.text
                records = list(csv.DictReader(io.StringIO(downloaded.text)))
                assert len(records) == len(rows)
                assert [json.loads(row["input"]) for row in records] == rows
                assert {row["state"] for row in records} == {
                    "queued",
                    "running",
                    "waiting_input",
                    "waiting_approval",
                    "completed",
                    "failed",
                    "cancelled",
                    "outcome_unknown",
                }
                by_id = {json.loads(row["input"])["request_id"]: row for row in records}
                assert (
                    by_id["complete-cny"]["output/currency"] == "CNY"
                    and by_id["complete-usd"]["output/currency"] == "USD"
                )
                assert (
                    by_id["complete-cny"]["output/amount"] == "12.25"
                    and by_id["complete-usd"]["output/amount"] == "13.25"
                )
                assert by_id["unknown"]["state"] == "outcome_unknown" and by_id["failure"]["error"]
                assert (
                    sql(
                        suite,
                        f"SELECT COUNT(*) FROM application_invocations WHERE application_id=UNHEX('{hexid(case['applicationId'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
                    )
                    == invocation_count
                )
                report(
                    suite,
                    "E10-S05-all-states",
                    mode="real_workflow_with_one_injected_unsettled_receipt",
                    snapshot=snapshot,
                    rows=records,
                    export=job,
                )
        finally:
            command(http, batch["batchId"], "cancel")
            # The original cancellation rules retain unsettled sandbox receipts
            # as outcome_unknown; cleanup must not rewrite those into success.
            settled = wait(
                lambda: get(http, batch["batchId"]),
                lambda value: value["counts"]["ended"] + value["counts"]["outcomeUnknown"] == len(rows),
                300,
            )
            report(suite, "E10-S05-all-states-cleanup", final=settled)
