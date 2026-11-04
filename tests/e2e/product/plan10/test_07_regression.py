"""Formal pre-existing API, conversation and MCP entry points keep their original semantics."""

import uuid

import pytest

from tests.e2e.product.live_text_support import Secret, post
from tests.e2e.product.plan9.agent_scenarios import conversation
from tests.e2e.product.plan9.mcp_support import application_client
from tests.e2e.product.plan10.support import client, report, scene, wait
from tests.e2e.product.test_live_text_acceptance import message, terminal

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.gateway, pytest.mark.live_model]


def test_original_api_single_invocation_and_mcp_tools(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "quote")
    with client(suite, case) as http:
        key = str(uuid.uuid4())
        path = f"/gateway/v1/applications/{case['slug']}/invocations"
        body = {"input": case["inputs"][0], "responseMode": "async"}
        first = http.post(path, headers={"Idempotency-Key": key}, json=body)
        assert first.status_code == 202, first.text
        replay = http.post(path, headers={"Idempotency-Key": key}, json=body)
        assert replay.status_code == 202 and replay.json()["id"] == first.json()["id"]
        original = terminal(http, first.json()["id"])
        assert original["outputs"] == case["expected"][0] and original["sessionId"] is None
    with application_client(suite, case, "Existing MCP regression") as (mcp, publication, _key):
        request = str(uuid.uuid4())
        task = mcp.submit(publication["toolName"], publication["revision"], case["inputs"][1], request_id=request)
        replay = mcp.submit(publication["toolName"], publication["revision"], case["inputs"][1], request_id=request)
        assert task["taskId"] == replay["taskId"]
        final = wait(
            lambda: mcp.task(task["taskId"]),
            lambda value: value["state"] in ("completed", "failed", "outcome_unknown"),
            300,
        )
        assert final["state"] == "completed" and final["outputs"] == case["expected"][1]
        assert len(mcp.tools(publication["toolName"])) == 4
        report(suite, "E10-A04-api-mcp", original=original, mcp=final, toolCount=4)


def test_original_sessions_keep_history_and_independent_context(plan10_suite, plan10_paid_gate):
    suite = plan10_suite
    case = conversation(suite["control"], suite["model"], suite["runId"])
    key = post(
        suite["control"],
        f"/applications/{case['applicationId']}/api-keys",
        {"name": "Original conversation regression"},
    )
    case["secret"] = Secret(key["secret"])
    with client(suite, case) as http:
        wait(
            lambda: http.get(f"/gateway/v1/applications/{case['slug']}/my-tasks", params={"limit": 1}).status_code,
            lambda status: status == 200,
            90,
        )
        sessions, results = [], []
        for index, source in enumerate(case["inputs"]):
            created = http.post(
                f"/gateway/v1/applications/{case['slug']}/sessions",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={"input": {}, "title": f"Regression {index}"},
            )
            assert created.status_code == 201, created.text
            session = created.json()
            sessions.append(session)
            first = terminal(http, message(http, session["id"], source["message"]))
            assert first["outputs"] == case["expected"][index]
            results.append(first)
        assert sessions[0]["id"] != sessions[1]["id"]
        for index, session in enumerate(sessions):
            follow = case["followups"][index][0]
            result = terminal(http, message(http, session["id"], follow["message"]))
            assert result["outputs"]["answer"] == follow["answer"]
            results.append(result)
        report(suite, "E10-A04-sessions", sessions=sessions, invocations=results)
