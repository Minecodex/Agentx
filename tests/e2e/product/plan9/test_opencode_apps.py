"""Eight business tasks chosen and executed by the installed independent AI client."""

import json
import os
import time
import traceback
import uuid
from pathlib import Path

import pytest

from tests.e2e.product.live_model_config import model_configuration
from tests.e2e.product.plan9.mcp_support import application_client, check_result, crm_records, prepare_files
from tests.e2e.product.plan9.opencode_client import isolated_opencode
from tests.e2e.product.plan9.opencode_support import budget, finish_existing, supervise
from tests.e2e.support import redact

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]
SCENES = ("S01", "S04", "S06", "S08", "S11", "S14", "S18", "S19")


def test_eight_natural_language_application_tasks(plan9_suite, existing_model_secret):
    suite = plan9_suite
    configured = model_configuration()
    ca = Path(os.environ["AGENTX_E2E_LLM_CA_FILE"])
    results = []
    output = suite["directory"] / "opencode-results.json"
    preparation = json.loads((suite["directory"] / "scenario-preparation.json").read_text())
    results.extend(
        {"scene": row["scene"], "passed": False, "events": [], "error": row["error"]}
        for row in preparation
        if row["scene"] in SCENES and not row["prepared"]
    )
    for case in (case for case in suite["cases"] if case["scene"] in SCENES):
        result = {"scene": case["scene"], "passed": False, "events": [], "error": None}
        results.append(result)
        began = time.monotonic()
        native = None
        try:
            with application_client(suite, case, f"OpenCode {case['scene']}") as (observer, published, key):
                inputs = prepare_files(observer, case["inputs"][0])
                before = len(crm_records(suite, case["inputs"][0]["request_id"])) if case.get("crm") else 0
                waits = case.get("waitInputs", [{}, {}])[0]
                prompt = f"请通过当前企业业务应用帮我办理 {case['scene']} 业务。业务资料为 {json.dumps(inputs, ensure_ascii=False)}。如果流程要求补充资料，我的答复为 {json.dumps(waits, ensure_ascii=False)}，请只针对任务当前返回的 waitId 填写。提交标识固定用 {uuid.uuid4()}。一次业务只提交一次；等待业务审批时由主管人工处理，你不能代批。请查询同一任务，取得真实完成结果及文件后总结，不使用本地文件、Shell、编辑或子 Agent，不把自己的推测当作工作流结果。"
                if case["scene"] == "S01":
                    prompt = f"请为{inputs['customer']}办理报价：采购{inputs['quantity']}件，单价{inputs['unitPrice']}元，优惠{inputs['discountPercent']}%。使用当前企业应用并查询真实报价结果。请求标识用 {uuid.uuid4()}，不要自己计算后冒充工作流结果。"
                before_budget = budget(configured["endpoint"], existing_model_secret.value, ca)
                result["budgetBefore"] = before_budget
                with isolated_opencode(
                    configured["endpoint"],
                    existing_model_secret.value,
                    ca=ca,
                    mcp_url=suite["urls"]["runtime"] + observer.path,
                    mcp_key=observer.key,
                    host=observer.host,
                ) as native:
                    connection = native.execute(["mcp", "list"], timeout=60)
                    result["connection"] = connection
                    assert "✓ application" in connection, connection
                    task, events, human = supervise(
                        suite, native, observer, case, key["id"], prompt, before_writes=before
                    )
                    session = next(event["sessionID"] for event in events if event.get("sessionID"))
                    result["events"].extend(events)
                    task, events = finish_existing(
                        suite,
                        native,
                        observer,
                        task,
                        events,
                        waits,
                        session=session,
                        case=case,
                        caller_id=key["id"],
                        human=human,
                        before_writes=before,
                    )
                    result["events"] = events
                    proof = check_result(suite, observer, case, 0, task, before)
                    assert proof["callerId"] == uuid.UUID(key["id"]).hex
                    for followup in case.get("followups", [[], []])[0]:
                        next_prompt = f"请继续刚才的企业应用会话 {task['sessionId']}，将这句话发给应用：{followup['message']}。请真正调用应用并查询结果，不要直接凭你自己的聊天记忆回答。新请求标识 {uuid.uuid4()}。"
                        following, new_events, _ = supervise(
                            suite, native, observer, case, key["id"], next_prompt, session=session
                        )
                        following, new_events = finish_existing(
                            suite,
                            native,
                            observer,
                            following,
                            new_events,
                            {},
                            session=session,
                            case=case,
                            caller_id=key["id"],
                        )
                        result["events"].extend(new_events)
                        assert following["sessionId"] == task["sessionId"]
                        assert following["outputs"] == {"answer": followup["answer"]}
                    tools = [event["part"]["tool"] for event in result["events"] if event.get("type") == "tool_use"]
                    result["toolErrors"] = [
                        event["part"]
                        for event in result["events"]
                        if event.get("type") == "tool_use"
                        and event["part"].get("state", {}).get("status") != "completed"
                    ]
                    assert all(
                        tool
                        in {
                            f"application_{published['toolName']}",
                            "application_get_task",
                            "application_submit_task_input",
                            "application_cancel_task",
                        }
                        for tool in tools
                    ), tools
                    if waits:
                        assert "application_submit_task_input" in tools, (
                            "The external AI must submit the real pending inputs itself"
                        )
                    assert len(human) == len(case.get("approvalDecisions", [[], []])[0])
                    result.update(
                        passed=True,
                        task=task,
                        runtimeProof=proof,
                        humanDecisions=human,
                        tools=tools,
                        records=observer.records,
                    )
        except Exception as error:
            result.update(error=f"{type(error).__name__}: {error}", traceback=redact(traceback.format_exc()))
            if native:
                result["clientOutput"] = native.last_output
        finally:
            if native:
                result["events"] = native.events
                result["clientLogs"] = native.diagnostics
            try:
                result["budgetAfter"] = budget(configured["endpoint"], existing_model_secret.value, ca)
            except Exception as error:
                result["budgetError"] = redact(f"{type(error).__name__}: {error}")
            result["elapsedSeconds"] = time.monotonic() - began
            output.write_text(json.dumps(results, ensure_ascii=False, indent=2))
    assert results and all(result["passed"] for result in results), [
        {"scene": value["scene"], "error": value["error"]} for value in results if not value["passed"]
    ]
