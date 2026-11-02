"""Native OAuth AI submission -> same user's generated page -> actual approval -> native MCP result."""

import json
import os
import socket
import uuid
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.live_model_config import model_configuration
from tests.e2e.product.live_text_support import Secret
from tests.e2e.product.plan9.mcp_support import application_client, check_result, crm_records, publication
from tests.e2e.product.plan9.opencode_client import isolated_opencode
from tests.e2e.product.plan9.opencode_support import finish_existing, invocations, supervise
from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.plan9.test_opencode_oauth import CALLBACK, PersonalObserver, authenticate
from tests.e2e.product.plan9.test_personal_oauth import browser
from tests.e2e.product.user_auth import UserTokenAuth
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_personal_ai_task_web_input_and_tool_approval(plan9_suite, existing_model_secret):
    suite = plan9_suite
    case = next(value for value in suite["cases"] if value["scene"] == "S19")
    response = suite["control"].get(f"/api/v1/applications/{case['applicationId']}/ui/publications")
    assert response.status_code == 200, response.text
    page = next(value for value in response.json()["publications"] if value["status"] == "active")
    published = publication(suite, case)
    configured = model_configuration()
    common = {"applicationId": case["applicationId"], "name": f"OpenCode personal Web {suite['runId']}"}
    directory = suite["directory"] / "cross-entry"
    directory.mkdir(exist_ok=True)
    ui_suite = {**suite, "directory": directory, "runId": suite["runId"] + "-cross"}
    native = None
    client_id = None
    report = {"passed": False}
    try:
        with socket.socket() as probe:
            assert probe.connect_ex(("127.0.0.1", 19876)) != 0, "The native client's callback port is already occupied"
        registered = browser(ui_suite, "register", {**common, "callback": CALLBACK})
        client_id = registered["client"]["clientId"]
        with (
            isolated_opencode(
                configured["endpoint"],
                existing_model_secret.value,
                ca=Path(os.environ["AGENTX_E2E_LLM_CA_FILE"]),
                mcp_url=f"{suite['urls']['runtime']}/gateway/v1/mcp/applications/{published['slug']}",
                oauth={"clientId": client_id, "scope": registered["client"]["scope"]},
            ) as native,
            httpx.Client(base_url=suite["urls"]["runtime"], timeout=60, trust_env=False) as observer_client,
        ):
            report["authentication"] = authenticate(ui_suite, native, common)
            login = suite["control"].post(
                "/api/v1/auth/login",
                auth=None,
                json={"username": suite["employee"]["username"], "password": suite["employee"]["password"].value},
            )
            assert login.status_code == 200, login.text
            observer_client.auth = UserTokenAuth(
                suite["urls"]["web"],
                Secret(login.json()["accessToken"]),
                username=suite["employee"]["username"],
                password=suite["employee"]["password"],
            )
            observer = PersonalObserver(observer_client)
            before = len(crm_records(suite, case["inputs"][0]["request_id"]))
            task, events, human = supervise(
                suite,
                native,
                observer,
                case,
                suite["employee"]["id"],
                f"请通过企业应用提交这项业务：{json.dumps(case['inputs'][0], ensure_ascii=False)}，请求标识 {uuid.uuid4()}。只提交一次。遇到补充资料时停下来，把真实 taskId 告诉我，我会自己登录网页补充并请主管审批；你不要调用补充资料或审批操作，也不要取消或替换任务。",
                caller_type="user",
                before_writes=before,
            )
            assert task["pendingInputs"] and not human, task
            assert task["taskUrl"] == f"{suite['urls']['web']}/a/{page['slug']}?taskId={task['taskId']}"
            assert page["manifest"]["contract"]["bundleId"] == task["bundleId"]
            assert len(crm_records(suite, case["inputs"][0]["request_id"])) == before
            session = next(event["sessionID"] for event in events if event.get("sessionID"))
            original = invocations(suite, case, suite["employee"]["id"], "user")
            web_result = directory / "web-result.json"
            run_playwright(
                Path(suite["installed"]["root"]),
                "plan9-personal-web",
                ("tests/plan9-personal-web.spec.ts",),
                {
                    **browser_environment(suite),
                    "AGENTX_E2E_PLAN9_EXISTING_TASK": json.dumps(
                        {"scenario": case, "slug": page["slug"], "taskId": task["taskId"]}
                    ),
                    "AGENTX_E2E_PLAN9_EXISTING_TASK_REPORT": str(web_result),
                },
            )
            assert invocations(suite, case, suite["employee"]["id"], "user") == original
            task = observer.task(task["taskId"])
            task, events = finish_existing(
                suite,
                native,
                observer,
                task,
                events,
                {},
                session=session,
                case=case,
                caller_id=suite["employee"]["id"],
                caller_type="user",
            )
            # The personal client itself must query the actual completed MCP receipt.
            with application_client(suite, case, "Cross-entry service stranger") as (stranger, _, _):
                stranger.call("get_task", {"taskId": task["taskId"]}, error=True)
            proof = check_result(suite, None, {**case, "fileOutputs": False}, 0, task, before)
            assert proof["callerType"] == "user" and proof["callerId"] == uuid.UUID(suite["employee"]["id"]).hex
            report.update(
                passed=True,
                task=task,
                events=events,
                runtimeProof=proof,
                web=json.loads(web_result.read_text()),
                pagePublicationId=page["id"],
                sameInvocation=True,
                serviceCannotReadPersonalTask=True,
            )
    finally:
        try:
            if client_id:
                removed = suite["control"].delete(
                    f"/api/v1/applications/{case['applicationId']}/mcp/clients/{client_id}"
                )
                assert removed.status_code == 200, removed.text
        finally:
            if native:
                report["clientLogs"] = native.diagnostics
                if not report["passed"]:
                    report["clientOutput"] = native.last_output
            (directory / "result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
