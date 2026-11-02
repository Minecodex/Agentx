"""Supervise real external AI calls without choosing or submitting its business tools for it."""

from __future__ import annotations

import json
import ssl
import time
import uuid
from concurrent.futures import ThreadPoolExecutor

import httpx

from tests.e2e.product.plan9.mcp_support import TERMINAL, crm_records, decide
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql


def invocations(suite, case, caller_id, caller_type="api_key"):
    assert caller_type in ("api_key", "user")
    return set(
        _runtime_mysql(
            suite["installed"],
            f"SELECT LOWER(HEX(id)) FROM application_invocations WHERE application_id=UNHEX('{uuid.UUID(case['applicationId']).hex}') AND caller_type='{caller_type}' AND caller_id=UNHEX('{uuid.UUID(caller_id).hex}')",  # noqa: S608 -- the caller type is restricted above and both identifiers are parsed UUIDs.
        ).splitlines()
    )


def budget(endpoint, credential, ca):
    context = ssl.create_default_context(cafile=str(ca))
    with httpx.Client(verify=context, timeout=30, trust_env=False) as client:
        response = client.get(
            endpoint.removesuffix("/v1") + "/budget", headers={"Authorization": f"Bearer {credential}"}
        )
        assert response.status_code == 200
        return response.json()


def supervise(
    suite,
    native,
    observer,
    case,
    caller_id,
    prompt,
    *,
    session=None,
    before_writes=0,
    caller_type="api_key",
    existing_task=None,
    human=None,
):
    original = invocations(suite, case, caller_id, caller_type)
    evidence = human if human is not None else []
    approvals = {value["approvalId"] for value in evidence}
    task = existing_task
    deadline = time.monotonic() + 600
    with ThreadPoolExecutor(max_workers=1) as pool:
        future = pool.submit(native.run, prompt, session=session)
        try:
            while True:
                created = invocations(suite, case, caller_id, caller_type) - original
                assert len(created) <= (0 if existing_task else 1), (
                    "One natural-language task must create exactly one Invocation"
                )
                if task or created:
                    task_id = task["taskId"] if task else str(uuid.UUID(next(iter(created))))
                    task = observer.task(task_id)
                    for approval in task["pendingApprovals"]:
                        if approval["approvalId"] in approvals:
                            continue
                        if case.get("crm"):
                            assert len(crm_records(suite, case["inputs"][0]["request_id"])) == before_writes, (
                                "Approval must precede every CRM write"
                            )
                        decision = case["approvalDecisions"][0][len(approvals)]
                        evidence.append(
                            decide(suite, case, approval["approvalId"], decision, f"opencode-{case['scene']}")
                        )
                        approvals.add(approval["approvalId"])
                if future.done():
                    events = future.result()
                    assert task, "OpenCode's text alone cannot prove that an application was invoked"
                    return task, events, evidence
                assert time.monotonic() < deadline, "External AI task timed out"
                time.sleep(0.5)
        finally:
            if not future.done():
                native.stop()


def task_receipts(events):
    def walk(value):
        if isinstance(value, str):
            try:
                value = json.loads(value)
            except ValueError:
                return
        if isinstance(value, dict):
            if isinstance(value.get("taskId"), str) and isinstance(value.get("state"), str):
                yield value
            else:
                for item in value.values():
                    yield from walk(item)
        elif isinstance(value, list):
            for item in value:
                yield from walk(item)

    for event in events:
        if event.get("type") == "tool_use":
            yield from walk(event.get("part", {}).get("state", {}).get("output"))


def finish_existing(
    suite,
    native,
    observer,
    task,
    events,
    inputs,
    *,
    session,
    case,
    caller_id,
    human=None,
    before_writes=0,
    caller_type="api_key",
):
    all_events = list(events)
    # Continuations always query/continue the original task; they never create
    # a replacement task or perform the client's input calls in Python.
    for _ in range(3):
        if task["state"] in TERMINAL:
            break
        task, events, _ = supervise(
            suite,
            native,
            observer,
            case,
            caller_id,
            f"继续原任务 {task['taskId']}，不要重新提交应用。如有当前待办，按我之前提供的资料补充；如果仍在运行或审批，查询原任务并如实汇报。资料：{json.dumps(inputs, ensure_ascii=False)}",
            session=session,
            existing_task=task,
            human=human,
            before_writes=before_writes,
            caller_type=caller_type,
        )
        all_events.extend(events)
        task = observer.task(task["taskId"])
    assert task["state"] == "completed", task
    if not any(item["taskId"] == task["taskId"] and item["state"] == "completed" for item in task_receipts(all_events)):
        all_events.extend(
            native.run(f"查询原任务 {task['taskId']} 的最终结果和文件并总结，不创建新任务。", session=session)
        )
    assert any(
        item["taskId"] == task["taskId"] and item["state"] == "completed" for item in task_receipts(all_events)
    ), "The external AI must actually obtain a completed MCP receipt"
    return task, all_events
