"""Atomic acceptance and stable business intentions on the real Runtime database."""

import copy
import uuid
from concurrent.futures import ThreadPoolExecutor

import pytest

from tests.e2e.product.plan10.support import (
    client,
    command,
    coordinator_paused,
    create,
    hexid,
    items,
    report,
    scene,
    sql,
)

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.gateway, pytest.mark.live_model]


def test_atomic_validation_and_batch_boundaries(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    with coordinator_paused(suite), client(suite, case) as http:
        before = sql(suite, "SELECT COUNT(*) FROM application_batches")
        for rows in ([], [case["inputs"][0]] * 101):
            response = http.post(
                f"/gateway/v1/applications/{case['slug']}/batches",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={
                    "name": "invalid count",
                    "items": [{"clientItemId": str(i), "input": value} for i, value in enumerate(rows)],
                },
            )
            assert response.status_code in (400, 413, 422), response.text
        malformed = [case["inputs"][0], {**case["inputs"][0], "amount": "wrong type"}]
        response = http.post(
            f"/gateway/v1/applications/{case['slug']}/batches",
            headers={"Idempotency-Key": str(uuid.uuid4())},
            json={
                "name": "invalid row",
                "items": [{"clientItemId": str(i), "input": value} for i, value in enumerate(malformed)],
            },
        )
        assert response.status_code == 422, response.text
        assert any(error["field"].startswith("/items/1/input") for error in response.json()["fieldErrors"])
        oversized = {"name": "too large", "items": [{"clientItemId": "large", "input": {"value": "x" * 1048576}}]}
        large = http.post(
            f"/gateway/v1/applications/{case['slug']}/batches",
            headers={"Idempotency-Key": str(uuid.uuid4())},
            json=oversized,
        )
        assert large.status_code == 413, large.text
        assert sql(suite, "SELECT COUNT(*) FROM application_batches") == before
        accepted = [create(http, case, [case["inputs"][0]] * count) for count in (1, 100)]
        for batch, count in zip(accepted, (1, 100), strict=True):
            assert batch["counts"]["total"] == count and batch["counts"]["queued"] == count
            command(http, batch["batchId"], "stop")
        report(suite, "E10-C01", invalidRow=response.json(), oversizedStatus=large.status_code, accepted=accepted)


def test_concurrent_intent_replay_and_duplicate_content_rows(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    observed = []
    with coordinator_paused(suite), client(suite, case) as http:
        for _round in range(5):
            key = str(uuid.uuid4())
            with ThreadPoolExecutor(max_workers=4) as pool:
                responses = list(
                    pool.map(lambda _, batch_key=key: create(http, case, [case["inputs"][0]] * 2, batch_key), range(4))
                )
            assert len({value["batchId"] for value in responses}) == 1
            batch = responses[0]["batchId"]
            projects = items(http, batch)["items"]
            assert len(projects) == 2 and projects[0]["input"] == projects[1]["input"]
            assert len({value["clientItemId"] for value in projects}) == 2
            conflicting = http.post(
                f"/gateway/v1/applications/{case['slug']}/batches",
                headers={"Idempotency-Key": key},
                json={"name": "changed", "items": [{"clientItemId": "one", "input": case["inputs"][1]}]},
            )
            assert conflicting.status_code == 409, conflicting.text
            command(http, batch, "stop")
            observed.append({"replays": responses, "rows": projects, "conflict": conflicting.json()})
        report(suite, "E10-C02-C03", concurrentRounds=observed)


def test_item_pagination_cursor_scope_and_stopped_counts(plan10_suite):
    suite = plan10_suite
    expense, quote = scene(suite, "expense"), scene(suite, "quote")
    with coordinator_paused(suite), client(suite, expense) as http:
        batch = create(http, expense, [expense["inputs"][0]] * 7)
        first = items(http, batch["batchId"], limit=3)
        second = items(http, batch["batchId"], limit=3, cursor=first["next"])
        third = items(http, batch["batchId"], limit=3, cursor=second["next"])
        all_rows = [*first["items"], *second["items"], *third["items"]]
        assert [row["ordinal"] for row in all_rows] == list(range(1, 8))
        assert third["next"] is None
        changed_filter = http.get(
            f"/gateway/v1/batches/{batch['batchId']}/items", params={"state": "failed", "cursor": first["next"]}
        )
        assert changed_filter.status_code == 400
        with client(suite, quote) as other:
            cross = other.get(f"/gateway/v1/batches/{batch['batchId']}/items", params={"cursor": first["next"]})
            assert cross.status_code in (400, 401, 403, 404)
        stopped = command(http, batch["batchId"], "stop")
        assert stopped["counts"]["cancelled"] == 7 and stopped["counts"]["ended"] == 7
        assert not stopped["canStop"]
        assert (
            sql(
                suite,
                f"SELECT COUNT(*) FROM application_invocations WHERE id IN (SELECT invocation_id FROM application_batch_items WHERE batch_id=UNHEX('{hexid(batch['batchId'])}'))",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            )
            == "0"
        )
        report(suite, "E10-C04", first=first, second=second, third=third, stopped=stopped)


def test_batch_list_is_scoped_and_has_stable_newest_first_pagination(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    with coordinator_paused(suite), client(suite, case) as http:
        batches = [create(http, case, [copy.deepcopy(case["inputs"][0])], name=f"Ordered {i}") for i in range(3)]
        response = http.get(f"/gateway/v1/applications/{case['slug']}/batches", params={"limit": 1})
        assert response.status_code == 200, response.text
        page = response.json()
        assert page["items"][0]["batchId"] == batches[-1]["batchId"]
        next_page = http.get(
            f"/gateway/v1/applications/{case['slug']}/batches", params={"limit": 1, "cursor": page["next"]}
        ).json()
        assert next_page["items"][0]["batchId"] == batches[-2]["batchId"]
        for batch in batches:
            command(http, batch["batchId"], "stop")
        report(suite, "E10-C04-list", page=page, nextPage=next_page)
