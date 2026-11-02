"""Competing and late decisions use the original concrete Effect and never duplicate writes."""

import copy
import json
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager

import httpx
import pytest

from tests.e2e.product.plan9.mcp_support import TERMINAL, application_client, check_result, crm_records
from tests.e2e.product.plan9.test_tool_recovery import effect
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


@contextmanager
def manager_client(suite):
    actor = suite["users"]["manager"]
    with httpx.Client(base_url=suite["urls"]["web"], timeout=90, trust_env=False) as client:
        login = client.post(
            "/api/v1/auth/login", json={"username": actor["username"], "password": actor["password"].value}
        )
        assert login.status_code == 200
        client.headers["Authorization"] = "Bearer " + login.json()["accessToken"]
        yield client


def pending_proposal(suite, client, published, case):
    task = client.submit(published["toolName"], published["revision"], case["inputs"][0])
    deadline = time.monotonic() + 300
    while not (task := client.task(task["taskId"]))["pendingApprovals"]:
        assert task["state"] not in TERMINAL and time.monotonic() < deadline, task
        time.sleep(0.2)
    return task


def read_approval(manager, identifier):
    until = time.monotonic() + 60
    while True:
        response = manager.get(f"/api/v1/approvals/{identifier}")
        if response.status_code == 200:
            return response.json()
        assert response.status_code == 404 and time.monotonic() < until, response.text
        time.sleep(0.2)


def scene(suite, suffix):
    case = copy.deepcopy(next(row for row in suite["cases"] if row["scene"] == "S17"))
    case["inputs"][0]["request_id"] += suffix
    case["expected"][0]["reference"] = case["inputs"][0]["request_id"]
    return case


def finish(client, identifier):
    until = time.monotonic() + 300
    while (task := client.task(identifier))["state"] not in TERMINAL:
        assert time.monotonic() < until, task
        time.sleep(0.2)
    return task


def test_concurrent_tool_decisions_have_one_winner_and_one_write(plan9_suite):
    suite, report = plan9_suite, {"passed": False}
    case = scene(suite, "-decision-race")
    try:
        with (
            application_client(suite, case, "Concurrent tool decisions") as (client, published, _),
            manager_client(suite) as manager,
        ):
            before = len(crm_records(suite, case["inputs"][0]["request_id"]))
            task = pending_proposal(suite, client, published, case)
            original = effect(suite, task["taskId"])
            identifier = task["pendingApprovals"][0]["approvalId"]
            approval = read_approval(manager, identifier)
            path = f"/api/v1/approvals/{identifier}"
            claimed = manager.post(path + "/claim", json={"version": approval["version"]})
            assert claimed.status_code == 200, claimed.text
            version = claimed.json()["version"]
            requests = [
                {
                    "version": version,
                    "decisionId": "approved",
                    "reason": "并发决定验收",
                    "idempotencyKey": str(uuid.uuid4()),
                }
                for _ in range(2)
            ]
            assert len(crm_records(suite, case["inputs"][0]["request_id"])) == before
            with ThreadPoolExecutor(max_workers=2) as pool:
                responses = list(pool.map(lambda body: manager.post(path + "/decide", json=body), requests))
            assert sorted(response.status_code for response in responses) == [200, 409], [
                response.text for response in responses
            ]
            winner = next(index for index, response in enumerate(responses) if response.status_code == 200)
            replay = manager.post(path + "/decide", json=requests[winner])
            assert replay.status_code == 200 and replay.json() == responses[winner].json(), replay.text
            late = manager.post(path + "/decide", json={**requests[winner], "idempotencyKey": str(uuid.uuid4())})
            assert late.status_code == 409, late.text
            final = finish(client, task["taskId"])
            proof = check_result(suite, client, case, 0, final, before)
            settled = effect(suite, task["taskId"])
            assert settled["effectId"] == original["effectId"] and settled["settled"] and not settled["unknown"]
            report.update(
                passed=True,
                task=final,
                runtimeProof=proof,
                effect=settled,
                decisionStatuses=[response.status_code for response in responses],
                idempotentReplayStatus=replay.status_code,
                staleDecisionStatus=late.status_code,
                writesBefore=before,
                writesAfter=len(crm_records(suite, case["inputs"][0]["request_id"])),
            )
    finally:
        (suite["directory"] / "tool-decision-race.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))


def test_cancelled_and_expired_tool_proposals_reject_late_approval(plan9_suite):
    suite, results = plan9_suite, []
    try:
        for mode in ("cancel", "timeout"):
            case = scene(suite, "-late-" + mode)
            with (
                application_client(suite, case, "Late approval " + mode) as (client, published, _),
                manager_client(suite) as manager,
            ):
                reference = case["inputs"][0]["request_id"]
                before = len(crm_records(suite, reference))
                task = pending_proposal(suite, client, published, case)
                original = effect(suite, task["taskId"])
                identifier = task["pendingApprovals"][0]["approvalId"]
                approval = read_approval(manager, identifier)
                path = f"/api/v1/approvals/{identifier}"
                claimed = manager.post(path + "/claim", json={"version": approval["version"]})
                assert claimed.status_code == 200, claimed.text
                if mode == "cancel":
                    client.cancel(task["taskId"])
                else:
                    # Fault only the due timestamp in this temporary database. The
                    # production coordinator must perform timeout and recovery itself.
                    parsed = uuid.UUID(identifier).hex
                    _runtime_mysql(
                        suite["installed"],
                        f"UPDATE approval_tasks SET deadline_at=TIMESTAMPADD(SECOND,-1,UTC_TIMESTAMP(6)) WHERE id=UNHEX('{parsed}') AND status='claimed'",  # noqa: S608 -- parsed fixture UUID, isolated database fault.
                    )
                if mode == "cancel":
                    final = finish(client, task["taskId"])
                else:
                    # Expiration rejects this concrete proposal. The original
                    # Agent may legitimately stop or submit a fresh proposal;
                    # it must never acquire authority from the expired one.
                    deadline = time.monotonic() + 180
                    while True:
                        status = _runtime_mysql(
                            suite["installed"],
                            f"SELECT status FROM approval_tasks WHERE id=UNHEX('{uuid.UUID(identifier).hex}')",  # noqa: S608 -- parsed fixture UUID.
                        )
                        current = effect(suite, task["taskId"], original["effectId"])
                        if status == "timed_out" and current["settled"]:
                            break
                        assert time.monotonic() < deadline, (status, current)
                        time.sleep(0.2)
                    final = client.task(task["taskId"])
                late = manager.post(
                    path + "/decide",
                    json={
                        "version": claimed.json()["version"],
                        "decisionId": "approved",
                        "reason": "迟到审批",
                        "idempotencyKey": str(uuid.uuid4()),
                    },
                )
                assert late.status_code == 409, late.text
                assert len(crm_records(suite, reference)) == before
                current = effect(suite, task["taskId"], original["effectId"])
                assert current["effectId"] == original["effectId"] and not current["unknown"]
                status = _runtime_mysql(
                    suite["installed"],
                    f"SELECT status FROM approval_tasks WHERE id=UNHEX('{uuid.UUID(identifier).hex}')",  # noqa: S608 -- parsed fixture UUID.
                )
                assert status == ("cancelled" if mode == "cancel" else "timed_out"), status
                if mode == "cancel":
                    assert final["state"] == "cancelled", final
                fresh = [pending for pending in final["pendingApprovals"] if pending["approvalId"] != identifier]
                if final["state"] not in TERMINAL:
                    client.cancel(task["taskId"])
                    finish(client, task["taskId"])
                assert len(crm_records(suite, reference)) == before
                results.append(
                    {
                        "mode": mode,
                        "task": final,
                        "effect": current,
                        "approvalStatus": status,
                        "lateStatus": late.status_code,
                        "freshProposals": fresh,
                        "writes": before,
                    }
                )
        assert len(results) == 2
    finally:
        (suite["directory"] / "tool-late-decisions.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
