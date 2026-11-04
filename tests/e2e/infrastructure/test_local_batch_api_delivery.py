"""Retained demo through supported APIs; does not open or simulate browser access."""

from __future__ import annotations

import csv
import hashlib
import io
import json
import os
import ssl
import time
from contextlib import contextmanager
from datetime import UTC, datetime
from pathlib import Path

import httpx
import pytest

from tests.e2e.infrastructure.test_local_batch_delivery import checked, configuration

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.infrastructure,
    pytest.mark.skipif(
        not os.environ.get("AGENTX_LOCAL_BATCH_DELIVERY_CONFIG"),
        reason="Requires an explicitly authorized retained local batch application",
    ),
]


def save(directory, name, value):
    path = directory / name
    pending = path.with_suffix(".pending")
    pending.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    pending.replace(path)


def wait(read, accept, timeout=120):
    deadline = time.monotonic() + timeout
    while True:
        value = read()
        if accept(value):
            return value
        assert time.monotonic() < deadline, value
        time.sleep(0.5)


@contextmanager
def clients():
    config, directory, case = configuration()
    tls = ssl.create_default_context(cafile=config["caFile"])
    with (
        httpx.Client(base_url=config["baseUrl"], verify=tls, trust_env=False, timeout=60) as control,
        httpx.Client(base_url=config["runtimeUrl"], verify=tls, trust_env=False, timeout=60) as runtime,
    ):
        credentials = json.loads(Path(config["credentialsFile"]).read_text())
        token = checked(control.post("/api/v1/auth/login", json=credentials))["accessToken"]
        control.headers["Authorization"] = runtime.headers["Authorization"] = "Bearer " + token
        yield config, directory, case, control, runtime


def scenarios(config, directory):
    path = directory / "batch-api-scenarios.json"
    if path.exists():
        return json.loads(path.read_text())
    values = []
    for label, name, count in [
        ("A", "批量体验 · API 已完成", 3),
        ("B", "批量体验 · API 待办理", 3),
        ("C", "功能验证 · API 已取消", 2),
    ]:
        values.append(
            {
                "label": label,
                "key": "local-api:" + config["runId"] + ":" + label,
                "request": {
                    "name": name,
                    "items": [
                        {
                            "clientItemId": "=1+1" if label == "C" and index == 0 else f"demo-{label}-{index + 1}",
                            "input": {
                                "request_id": f"BX-DEMO-{label}-{index + 1:03d}",
                                "applicant": ["示例员工甲", "示例员工乙", "示例员工丙"][index],
                                "expense_type": ["差旅费", "办公采购", "客户招待"][index],
                                "amount": ([268.5, 135.75, 640] if label == "A" else [68.5, 144, 369.9])[index],
                            },
                        }
                        for index in range(count)
                    ],
                },
            }
        )
    save(directory, path.name, values)
    return values


def create(runtime, case, scenario):
    response = runtime.post(
        f"/gateway/v1/applications/{case['slug']}/batches",
        headers={"Idempotency-Key": scenario["key"]},
        json=scenario["request"],
    )
    assert response.status_code == 202, response.text
    batch = response.json()
    assert batch["source"] == "api" and batch["callerType"] == "user"
    assert batch["workflowVersionId"] == case["versionId"]
    assert batch["counts"]["total"] == len(scenario["request"]["items"])
    assert batch["accessBinding"] is None
    return batch


def projects(runtime, batch_id, **params):
    return checked(runtime.get(f"/gateway/v1/batches/{batch_id}/items", params=params))


def task(runtime, task_id):
    return checked(runtime.get(f"/gateway/v1/tasks/{task_id}"))


def approval(control, approval_id):
    def read():
        response = control.get(f"/api/v1/approvals/{approval_id}")
        return None if response.status_code == 404 else checked(response)

    return wait(read, bool, 60)


def settle_item(control, runtime, item, decision, number):
    """Reply and decide as the actual signed-in admin, without changing the Workflow."""
    task_id = item["task"]["taskId"]
    supplement = {"purpose": f"批量接口验收示例 {number}，仅为虚构费用", "invoice_no": f"FP-DEMO-{number:03d}"}
    current = task(runtime, task_id)
    if current["state"] == "waiting_input":
        pending = current["pendingInputs"][0]
        receipt = runtime.post(
            f"/gateway/v1/invocations/{task_id}/waits/{pending['waitId']}/input",
            headers={"Idempotency-Key": "local-input:" + task_id},
            json={"content": supplement},
        )
        assert receipt.status_code == 202, receipt.text
        current = wait(lambda: task(runtime, task_id), lambda t: t["state"] in ("waiting_approval", "completed"))
    if decision is None:
        assert current["state"] == "waiting_approval", current
        view = approval(control, current["pendingApprovals"][0]["approvalId"])
        return {"taskId": task_id, "supplement": supplement, "pendingApproval": view}
    if current["state"] == "waiting_approval":
        approval_id = current["pendingApprovals"][0]["approvalId"]
        view = approval(control, approval_id)
        assert view["nodeId"] == "manager"
        assert item["input"]["request_id"] in view["title"]
        assert decision in {button["id"] for button in view["buttons"]}
        if view["claimedBy"] is None:
            checked(control.post(f"/api/v1/approvals/{approval_id}/claim", json={"version": view["version"]}))
            view = wait(lambda: approval(control, approval_id), lambda v: v["claimedBy"] is not None)
        checked(
            control.post(
                f"/api/v1/approvals/{approval_id}/decide",
                json={
                    "version": view["version"],
                    "decisionId": decision,
                    "reason": "本地批量接口验收，虚构费用，不涉及付款。",
                    "idempotencyKey": "local-decide:" + task_id,
                },
            )
        )
    completed = wait(lambda: task(runtime, task_id), lambda t: t["state"] == "completed")
    assert completed["taskId"] == task_id and completed["sessionId"] is None
    assert completed["outputs"] == {**item["input"], **supplement, "decision": decision}
    assert not completed["pendingInputs"] and not completed["pendingApprovals"]
    return {"taskId": task_id, "supplement": supplement, "decision": decision, "result": completed}


def export(runtime, control, case, directory, batch, suffix):
    base = f"/gateway/v1/batches/{batch['batchId']}/exports"
    key = {"Idempotency-Key": "local-export:" + batch["batchId"]}
    job = checked(runtime.post(base, headers=key))
    replay = checked(runtime.post(base, headers=key))
    assert job["exportId"] == replay["exportId"]
    job = wait(
        lambda: checked(runtime.get(base + "/" + job["exportId"])), lambda j: j["status"] in ("completed", "failed")
    )
    assert job["status"] == "completed", job
    response = runtime.get(base + "/" + job["exportId"] + "/file")
    assert response.is_success and response.headers["content-type"].startswith("text/csv")
    data = response.content
    assert len(data) == job["file"]["sizeBytes"]
    assert hashlib.sha256(data).hexdigest() == job["file"]["sha256"]
    (directory / f"expense-batch-{suffix}.csv").write_bytes(data)
    managed = control.get(
        f"/api/v1/applications/{case['applicationId']}/batches/{batch['batchId']}/exports/{job['exportId']}/file"
    )
    assert managed.is_success and managed.content == data
    denied = httpx.get(
        str(response.request.url),
        verify=ssl.create_default_context(cafile=configuration()[0]["caFile"]),
        trust_env=False,
    )
    assert denied.status_code in (401, 403, 404)
    rows = list(csv.DictReader(io.StringIO(data.decode("utf-8-sig"))))
    assert len(rows) == batch["counts"]["total"]
    return {"job": job, "rows": rows, "unauthorizedDownloadStatus": denied.status_code, "managerDownloadMatches": True}


def test_retained_api_batches_replay_inputs_approvals_pagination_and_exports():
    with clients() as (config, directory, case, control, runtime):
        planned = scenarios(config, directory)
        report = {"passed": False, "mode": "real_api_no_browser", "batches": [], "interactions": []}
        try:
            for index, scenario in enumerate(planned[:2]):
                batch = create(runtime, case, scenario)
                report["batches"].append(batch)
                save(directory, "batch-api-business-verification.json", report)
                replay = create(runtime, case, scenario)
                assert replay["batchId"] == batch["batchId"]
                conflict = runtime.post(
                    f"/gateway/v1/applications/{case['slug']}/batches",
                    headers={"Idempotency-Key": scenario["key"]},
                    json={**scenario["request"], "name": scenario["request"]["name"] + " changed"},
                )
                assert conflict.status_code == 409, conflict.text
                ready = wait(
                    lambda batch_id=batch["batchId"]: projects(runtime, batch_id),
                    lambda p: (
                        len(p["items"]) == 3
                        and all(
                            i["task"] and i["state"] in ("waiting_input", "waiting_approval", "completed")
                            for i in p["items"]
                        )
                    ),
                )
                assert [i["input"] for i in ready["items"]] == [i["input"] for i in scenario["request"]["items"]]
                assert [i["clientItemId"] for i in ready["items"]] == [
                    i["clientItemId"] for i in scenario["request"]["items"]
                ]
                assert len({i["task"]["taskId"] for i in ready["items"]}) == 3
                page1 = projects(runtime, batch["batchId"], limit=2)
                assert len(page1["items"]) == 2 and page1["next"]
                page2 = projects(runtime, batch["batchId"], limit=2, cursor=page1["next"])
                assert len(page2["items"]) == 1 and page2["next"] is None
                assert [i["itemId"] for i in page1["items"] + page2["items"]] == [i["itemId"] for i in ready["items"]]
                targets = [(0, "approved"), (1, None)] if index else [(0, "approved"), (1, "approved"), (2, "rejected")]
                for ordinal, decision in targets:
                    report["interactions"].append(
                        settle_item(control, runtime, ready["items"][ordinal], decision, index * 3 + ordinal + 1)
                    )
                    save(directory, "batch-api-business-verification.json", report)
                settled = wait(
                    lambda batch_id=batch["batchId"]: checked(runtime.get(f"/gateway/v1/batches/{batch_id}")),
                    lambda b, completed=1 if index else 3: b["counts"]["completed"] == completed,
                )
                if index:
                    assert settled["state"] == "waiting"
                    assert settled["counts"]["waitingInput"] == 1 and settled["counts"]["waitingApproval"] == 1
                    assert len(projects(runtime, batch["batchId"], state="waiting_input")["items"]) == 1
                    denied_rerun = runtime.post(
                        f"/gateway/v1/batches/{batch['batchId']}/rerun",
                        headers={"Idempotency-Key": "local-rerun-denied:" + batch["batchId"]},
                        json={"itemIds": [ready["items"][2]["itemId"]]},
                    )
                    assert denied_rerun.status_code == 409, denied_rerun.text
                    report["unsafeRerunRejected"] = True
                else:
                    assert settled["state"] == "completed" and settled["counts"]["ended"] == 3
                    report["export"] = export(runtime, control, case, directory, settled, "api-completed")
                    assert {r["output/decision"] for r in report["export"]["rows"]} == {"approved", "rejected"}
                report["batches"][index] = settled
                managed = checked(
                    control.get(f"/api/v1/applications/{case['applicationId']}/batches/{batch['batchId']}")
                )
                assert managed["batchId"] == settled["batchId"] and managed["counts"] == settled["counts"]
                foreign = control.get(
                    f"/api/v1/applications/{case['originalApplicationId']}/batches/{batch['batchId']}"
                )
                assert foreign.status_code in (403, 404)
            owner = checked(runtime.get(f"/gateway/v1/applications/{case['slug']}/batches"))
            managed = checked(control.get(f"/api/v1/applications/{case['applicationId']}/batches"))
            expected = {b["batchId"] for b in report["batches"]}
            assert expected <= {b["batchId"] for b in owner["items"]} & {b["batchId"] for b in managed["items"]}
            report.update(
                passed=True,
                completedAt=datetime.now(UTC).isoformat(),
                idempotentReplay=True,
                changedIntentRejected=True,
                paginationVerified=True,
                managerAndOwnerAgree=True,
                applicationScopeVerified=True,
            )
        finally:
            save(directory, "batch-api-business-verification.json", report)


def test_retained_api_batch_cancellation_and_formula_safe_export():
    with clients() as (config, directory, case, control, runtime):
        scenario = scenarios(config, directory)[2]
        batch = create(runtime, case, scenario)
        report = {"passed": False, "batch": batch, "mode": "real_api_no_browser"}
        try:
            cancelled = checked(
                runtime.post(
                    f"/gateway/v1/batches/{batch['batchId']}/cancel",
                    headers={"Idempotency-Key": "local-cancel:" + batch["batchId"]},
                )
            )
            assert cancelled["batchId"] == batch["batchId"]
            final = wait(
                lambda: checked(runtime.get(f"/gateway/v1/batches/{batch['batchId']}")),
                lambda b: b["counts"]["cancelled"] == 2,
            )
            assert final["state"] == "cancelled" and final["counts"]["ended"] == 2
            items = projects(runtime, batch["batchId"])["items"]
            assert all(i["state"] == "cancelled" for i in items)
            assert all(not i["task"] or not i["task"]["allowedActions"]["canCancel"] for i in items)
            report.update(
                batch=final, items=items, export=export(runtime, control, case, directory, final, "api-cancelled")
            )
            assert report["export"]["rows"][0]["clientItemId"] == "'=1+1"
            report.update(passed=True, completedAt=datetime.now(UTC).isoformat(), formulaTriggerEscaped=True)
        finally:
            save(directory, "batch-api-cancellation-verification.json", report)


def test_retained_owned_tsv_import_does_not_submit_business():
    with clients() as (config, directory, case, _, runtime):
        tasks_path = f"/gateway/v1/applications/{case['slug']}/my-tasks"
        before = checked(runtime.get(tasks_path, params={"limit": 50}))
        text = "单号\t申请人\t类型\t金额\nBX-TSV-001\t示例员工甲\t差旅费\t88.25\nBX-TSV-002\t示例员工乙\t办公采购\t169.50\n"
        data = text.encode()
        uploaded = runtime.post(
            "/gateway/v1/artifacts",
            headers={"Idempotency-Key": "local-import:" + config["runId"]},
            files={"file": ("expense-demo.tsv", data, "text/tab-separated-values")},
        )
        assert uploaded.status_code == 201, uploaded.text
        artifact = uploaded.json()
        assert artifact["sha256"] == hashlib.sha256(data).hexdigest()
        assert artifact["sizeBytes"] == len(data)
        parsed = checked(
            runtime.post(
                f"/gateway/v1/applications/{case['slug']}/batch-imports/parse",
                json={"artifactId": artifact["artifactId"], "delimiter": "\t"},
            )
        )
        assert parsed == {
            "headers": ["单号", "申请人", "类型", "金额"],
            "lines": [2, 3],
            "rows": [
                ["BX-TSV-001", "示例员工甲", "差旅费", "88.25"],
                ["BX-TSV-002", "示例员工乙", "办公采购", "169.50"],
            ],
        }
        after = checked(runtime.get(tasks_path, params={"limit": 50}))
        assert [t["taskId"] for t in before["items"]] == [t["taskId"] for t in after["items"]]
        save(
            directory,
            "batch-tsv-file-verification.json",
            {
                "passed": True,
                "artifact": artifact,
                "parsed": parsed,
                "newInvocations": 0,
                "taskIdsBefore": [t["taskId"] for t in before["items"]],
                "taskIdsAfter": [t["taskId"] for t in after["items"]],
            },
        )


def test_retained_stop_preserves_accepted_tasks_and_cancel_closes_them():
    with clients() as (config, directory, case, _, runtime):
        scenario = {
            "key": "local-api:" + config["runId"] + ":D",
            "request": {
                "name": "功能验证 · API 已受理后取消",
                "items": [
                    {
                        "clientItemId": f"demo-D-{i + 1}",
                        "input": {
                            "request_id": f"BX-DEMO-D-{i + 1:03d}",
                            "applicant": "取消验证员工",
                            "expense_type": "办公采购",
                            "amount": 22.5 + i,
                        },
                    }
                    for i in range(2)
                ],
            },
        }
        save(directory, "batch-api-accepted-cancel-scenario.json", scenario)
        batch = create(runtime, case, scenario)
        report = {"passed": False, "batch": batch, "mode": "real_api_no_browser"}
        try:
            path = f"/gateway/v1/batches/{batch['batchId']}"
            if batch["state"] != "cancelled":
                before = wait(
                    lambda: projects(runtime, batch["batchId"]),
                    lambda p: (
                        len(p["items"]) == 2 and all(i["state"] == "waiting_input" and i["task"] for i in p["items"])
                    ),
                )
                ids = [i["task"]["taskId"] for i in before["items"]]
                checked(runtime.post(path + "/stop", headers={"Idempotency-Key": "local-stop:" + batch["batchId"]}))
                stopped = projects(runtime, batch["batchId"])
                assert [i["task"]["taskId"] for i in stopped["items"]] == ids
                assert all(i["state"] == "waiting_input" for i in stopped["items"])
                report["stopPreservedOriginalAcceptedTasks"] = ids
                checked(runtime.post(path + "/cancel", headers={"Idempotency-Key": "local-cancel:" + batch["batchId"]}))
            final = wait(lambda: checked(runtime.get(path)), lambda b: b["counts"]["cancelled"] == 2)
            items = projects(runtime, batch["batchId"])["items"]
            assert final["counts"]["ended"] == 2
            assert len(items) == 2 and all(i["task"] and i["task"]["state"] == "cancelled" for i in items)
            assert all(not i["task"]["pendingInputs"] and not i["task"]["allowedActions"]["canCancel"] for i in items)
            report.update(passed=True, batch=final, items=items, completedAt=datetime.now(UTC).isoformat())
        finally:
            save(directory, "batch-api-accepted-cancellation-verification.json", report)
