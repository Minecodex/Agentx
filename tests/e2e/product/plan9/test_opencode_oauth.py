"""The actual installed client's own OAuth cache, login flow and subsequent AI tool use."""

import json
import os
import socket
import sys
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.live_model_config import model_configuration
from tests.e2e.product.live_text_support import Secret
from tests.e2e.product.plan9 import browser_opener
from tests.e2e.product.plan9.mcp_support import check_result, publication
from tests.e2e.product.plan9.opencode_client import isolated_opencode
from tests.e2e.product.plan9.opencode_support import finish_existing, supervise
from tests.e2e.product.plan9.test_personal_oauth import browser
from tests.e2e.product.user_auth import UserTokenAuth

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]
CALLBACK = "http://127.0.0.1:19876/mcp/oauth/callback"


class PersonalObserver:
    def __init__(self, client):
        self.client = client

    def task(self, task_id):
        response = self.client.get(f"/gateway/v1/tasks/{task_id}")
        assert response.status_code == 200, response.text
        return response.json()


def authenticate(suite, native, common):
    directory = native.root / "bin"
    directory.mkdir(mode=0o700)
    opener = directory / "open"
    opener.write_text(f"#!{sys.executable}\n" + Path(browser_opener.__file__).read_text())
    opener.chmod(0o700)
    captured = native.root / "authorization-url"
    native.environment["PATH"] = str(directory) + os.pathsep + native.environment.get("PATH", "")
    native.environment["PLAN9_NATIVE_AUTH_FILE"] = str(captured)
    with ThreadPoolExecutor(max_workers=1) as pool:
        future = pool.submit(native.execute, ["mcp", "auth", "application"], timeout=180)
        try:
            deadline = time.monotonic() + 60
            while not captured.exists():
                if future.done():
                    raise AssertionError(f"The native client did not request login: {future.result()}")
                assert time.monotonic() < deadline, "The native client's OAuth browser request timed out"
                time.sleep(0.1)
            browser(
                suite,
                "authorize",
                {**common, "callback": CALLBACK, "authorizationUrl": captured.read_text(), "nativeCallback": True},
            )
            return future.result(timeout=30)
        finally:
            if not future.done():
                native.stop()
            captured.unlink(missing_ok=True)


def test_native_opencode_personal_oauth(plan9_suite, existing_model_secret):
    suite = plan9_suite
    case = next(value for value in suite["cases"] if value["scene"] == "S01")
    published = publication(suite, case)
    configured = model_configuration()
    common = {"applicationId": case["applicationId"], "name": f"OpenCode OAuth {suite['runId']}"}
    client_id = None
    report = {"passed": False, "clientVersion": "1.1.56"}
    native = None
    try:
        # Check ownership before the unmodified client binds its fixed callback port.
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 19876))
        registered = browser(suite, "register", {**common, "callback": CALLBACK})
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
            report["authentication"] = authenticate(suite, native, common)
            report["connection"] = native.execute(["mcp", "list"], timeout=60)
            assert "✓ application" in report["connection"], report["connection"]
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
            values = case["inputs"][1]
            prompt = f"使用企业应用办理报价：客户{values['customer']}，数量{values['quantity']}，单价{values['unitPrice']}，优惠{values['discountPercent']}%。请求标识 {uuid.uuid4()}。必须真实调用 MCP 应用并查询任务的最终结果。"
            task, events, human = supervise(
                suite, native, observer, case, suite["employee"]["id"], prompt, caller_type="user"
            )
            session = next(event["sessionID"] for event in events if event.get("sessionID"))
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
                human=human,
            )
            proof = check_result(suite, None, case, 1, task, 0)
            assert proof["callerType"] == "user"
            assert proof["callerId"] == uuid.UUID(suite["employee"]["id"]).hex
            assert not human
            report.update(task=task, events=events, runtimeProof=proof)
            removed = suite["control"].delete(f"/api/v1/applications/{case['applicationId']}/mcp/clients/{client_id}")
            assert removed.status_code == 200, removed.text
            report["revokedConnection"] = native.execute(["mcp", "list"], timeout=60)
            assert "✓ application" not in report["revokedConnection"], (
                "Removing the client must revoke its native cached tokens"
            )
            report["passed"] = True
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
            (suite["directory"] / "opencode-oauth-results.json").write_text(
                json.dumps(report, ensure_ascii=False, indent=2)
            )
