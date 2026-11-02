"""Twenty complete applications through an independent standard JSON-RPC client."""

import json
import time
import traceback
import uuid

import pytest

from tests.e2e.product.plan9.mcp_support import (
    TERMINAL,
    application_client,
    check_result,
    crm_records,
    prepare_files,
    settle,
)
from tests.e2e.product.plan9.protocol_retest import fresh_protocol_case, is_model_stream_interruption
from tests.e2e.support import redact

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def verify_applications(suite):
    results = []
    report = suite["directory"] / "mcp-protocol-results.json"
    preparation = json.loads((suite["directory"] / "scenario-preparation.json").read_text())
    results.extend(
        {"scene": row["scene"], "passed": False, "executions": [], "error": row["error"]}
        for row in preparation
        if not row["prepared"]
    )
    for original in suite["cases"]:
        result = {"scene": original["scene"], "passed": False, "executions": [], "error": None, "attempts": []}
        results.append(result)
        for ordinal in range(3):
            case = fresh_protocol_case(original, ordinal) if ordinal else original
            attempt = {
                "attempt": ordinal,
                "passed": False,
                "executions": [],
                "error": None,
                "businessReferences": [value["request_id"] for value in case["inputs"] if "request_id" in value],
            }
            result["attempts"].append(attempt)
            report.write_text(json.dumps(results, ensure_ascii=False, indent=2))
            _execute_case(suite, case, attempt)
            for field in ("executions", "error", "records", "cancellation"):
                if field in attempt:
                    result[field] = attempt[field]
            result["passed"] = attempt["passed"]
            result["firstAttemptPassed"] = result["attempts"][0]["passed"]
            result["transportRetests"] = len(result["attempts"]) - 1
            report.write_text(json.dumps(results, ensure_ascii=False, indent=2))
            if result["passed"] or not attempt.get("retestEligible"):
                break
    assert all(item["passed"] for item in results), [
        {"scene": item["scene"], "error": item["error"]} for item in results if not item["passed"]
    ]
    return results


def _execute_case(suite, case, result):
    client, index, before = None, 0, 0
    try:
        with application_client(suite, case, f"MCP protocol {case['scene']} attempt {result['attempt']}") as (
            client,
            published,
            key,
        ):
            result["callerKeyId"] = key["id"]
            for index, source in enumerate(case["inputs"]):
                inputs = prepare_files(client, source)
                before = len(crm_records(suite, source["request_id"])) if case.get("crm") else 0
                request_id = str(uuid.uuid4())
                created = client.submit(published["toolName"], published["revision"], inputs, request_id=request_id)
                replay = client.submit(published["toolName"], published["revision"], inputs, request_id=request_id)
                assert replay["taskId"] == created["taskId"], "Idempotent protocol submission must reuse one Invocation"
                task, human = settle(
                    suite,
                    client,
                    created["taskId"],
                    case,
                    index,
                    before_writes=before,
                    tag=f"mcp-{case['scene']}-{index}",
                )
                proof = check_result(suite, client, case, index, task, before)
                assert proof["callerId"] == uuid.UUID(key["id"]).hex
                for followup in case.get("followups", [[], []])[index]:
                    following = client.submit(
                        published["toolName"],
                        published["revision"],
                        {**inputs, "message": followup["message"]},
                        session_id=task["sessionId"],
                    )
                    following, _ = settle(suite, client, following["taskId"], case, index)
                    assert following["sessionId"] == task["sessionId"]
                    assert following["outputs"] == {"answer": followup["answer"]}
                result["executions"].append({"task": task, "humanDecisions": human, "runtimeProof": proof})
            # Every application also exercises the standard cancellation helper.
            created = client.submit(
                published["toolName"], published["revision"], prepare_files(client, case["inputs"][0])
            )
            cancelled = client.rpc(
                "tools/call",
                {"name": "cancel_task", "arguments": {"taskId": created["taskId"], "requestId": str(uuid.uuid4())}},
            )
            until = time.monotonic() + 60
            while (final := client.task(created["taskId"]))["state"] not in TERMINAL:
                assert time.monotonic() < until, final
                time.sleep(0.2)
            if cancelled.get("isError"):
                assert final["state"] == "completed", "Only an already finished task may reject cancellation"
            else:
                assert final["state"] in {"cancelled", "completed"}, final
            result.update(passed=True, cancellation={"receipt": cancelled, "task": final}, records=client.records)
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        result["traceback"] = redact(traceback.format_exc())
        if is_model_stream_interruption(error):
            result["failedTask"] = error.task
            try:
                writes = len(crm_records(suite, case["inputs"][index]["request_id"])) if case.get("crm") else before
                result["transportFailure"] = {
                    "businessReference": case["inputs"][index].get("request_id"),
                    "writeCountBefore": before,
                    "writeCountAtFailure": writes,
                    "originalTaskAndEffectWillNotBeReplayed": True,
                }
                result["retestEligible"] = before <= writes <= before + 1
            except Exception as verification_error:
                result["retestBlocked"] = redact(f"Cannot verify original writes: {verification_error}")
    finally:
        if client:
            result["records"] = client.records


def test_twenty_formal_mcp_applications(plan9_business_protocol):
    assert len(plan9_business_protocol) == 20
    assert all(item["passed"] for item in plan9_business_protocol)
