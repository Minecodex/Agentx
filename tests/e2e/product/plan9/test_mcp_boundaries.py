"""Actual task/file authority, schema conflicts and cancellation at the MCP boundary."""

import json
import time
import uuid

import pytest

from tests.e2e.product.plan9.mcp_support import application_client, prepare_files, settle

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def waiting(client, task_id):
    deadline = time.monotonic() + 60
    while True:
        task = client.task(task_id)
        if task["pendingInputs"]:
            return task
        assert task["state"] not in {"failed", "cancelled", "outcome_unknown"}, task
        assert time.monotonic() < deadline, task
        time.sleep(0.2)


def test_mcp_owned_tasks_files_and_input_conflicts(plan9_suite):
    suite = plan9_suite
    cases = {case["scene"]: case for case in suite["cases"]}
    case = cases["S08"]
    report = {"passed": False, "checks": {}}
    try:
        with (
            application_client(suite, case, "Boundary owner") as (owner, published, _),
            application_client(suite, case, "Boundary other principal") as (other, _, _),
            application_client(suite, cases["S04"], "Boundary other application") as (file_owner, file_publication, _),
            application_client(suite, cases["S04"], "Boundary file stranger") as (file_stranger, _, _),
        ):
            request_id = str(uuid.uuid4())
            task = owner.submit(published["toolName"], published["revision"], case["inputs"][0], request_id=request_id)
            task = waiting(owner, task["taskId"])
            task_id = task["taskId"]
            wait_id = task["pendingInputs"][0]["waitId"]
            denied = {}
            denied["otherCaller"] = other.call("get_task", {"taskId": task_id}, error=True)
            denied["otherApplication"] = file_owner.call("get_task", {"taskId": task_id}, error=True)
            denied["otherCallerInput"] = other.call(
                "submit_task_input",
                {"taskId": task_id, "waitId": wait_id, "requestId": str(uuid.uuid4()), "content": {}},
                error=True,
            )
            denied["forgedActor"] = owner.call(
                "submit_task_input",
                {
                    "taskId": task_id,
                    "waitId": wait_id,
                    "requestId": str(uuid.uuid4()),
                    "content": {},
                    "userId": suite["employee"]["id"],
                },
                error=True,
            )
            denied["idempotencyConflict"] = owner.call(
                published["toolName"],
                {"requestId": request_id, "publicationRevision": published["revision"], "input": case["inputs"][1]},
                error=True,
            )
            denied["invalidDate"] = owner.call(
                "submit_task_input",
                {
                    "taskId": task_id,
                    "waitId": wait_id,
                    "requestId": str(uuid.uuid4()),
                    "content": {"date": "2026-02-30", "contact": "李敏", "email": "li.min@example.test"},
                },
                error=True,
            )
            assert owner.task(task_id)["pendingInputs"][0]["waitId"] == wait_id
            final, decisions = settle(suite, owner, task_id, case, 0)
            assert final["outputs"] == case["expected"][0] and not final["pendingApprovals"]
            denied["lateInput"] = owner.call(
                "submit_task_input",
                {
                    "taskId": task_id,
                    "waitId": wait_id,
                    "requestId": str(uuid.uuid4()),
                    "content": case["waitInputs"][0]["/delivery"][0],
                },
                error=True,
            )
            assert owner.task(task_id)["outputs"] == final["outputs"]
            cancelled = owner.submit(published["toolName"], published["revision"], case["inputs"][1])
            cancelled = waiting(owner, cancelled["taskId"])
            owner.cancel(cancelled["taskId"])
            deadline = time.monotonic() + 60
            while (state := owner.task(cancelled["taskId"]))["state"] != "cancelled":
                assert time.monotonic() < deadline, state
                time.sleep(0.2)
            denied["cancelledInput"] = owner.call(
                "submit_task_input",
                {
                    "taskId": cancelled["taskId"],
                    "waitId": cancelled["pendingInputs"][0]["waitId"],
                    "requestId": str(uuid.uuid4()),
                    "content": case["waitInputs"][1]["/delivery"][0],
                },
                error=True,
            )
            files = prepare_files(file_owner, cases["S04"]["inputs"][0])
            denied["foreignInputFile"] = file_stranger.call(
                file_publication["toolName"],
                {"requestId": str(uuid.uuid4()), "publicationRevision": file_publication["revision"], "input": files},
                error=True,
            )
            uploaded = file_owner.submit(file_publication["toolName"], file_publication["revision"], files)
            uploaded, _ = settle(suite, file_owner, uploaded["taskId"], cases["S04"], 0)
            artifact = uploaded["files"][0]["artifactId"]
            unauthorized = file_stranger.client.get(
                f"/gateway/v1/invocations/{uploaded['taskId']}/artifacts/{artifact}",
                headers={"Authorization": f"Bearer {file_stranger.key}"},
            )
            assert unauthorized.status_code in (403, 404), unauthorized.status_code
            portal = owner.client.post(
                owner.path,
                headers={
                    "Authorization": suite["control"].headers["Authorization"],
                    "Accept": "application/json, text/event-stream",
                },
                json={"jsonrpc": "2.0", "id": 100, "method": "tools/list"},
            )
            assert portal.status_code == 401, portal.status_code
            report.update(
                passed=True,
                checks=denied,
                completedTask=final,
                inputEvidence=decisions,
                cancelledTask=state,
                fileTask=uploaded,
                foreignFileStatus=unauthorized.status_code,
                browserJwtStatus=portal.status_code,
            )
    finally:
        (suite["directory"] / "mcp-boundaries.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
