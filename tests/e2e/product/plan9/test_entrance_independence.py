"""Web/MCP enablement is independent; existing owned tasks keep their formal results."""

import json
import time
import uuid
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.next_batch_support import get
from tests.e2e.product.plan9.mcp_support import application_client, check_result, settle
from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.plan9.test_page_boundaries import active
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def web_state(suite, application, version, slug, enabled):
    listing, _ = active(suite, application)
    response = suite["control"].post(
        f"/api/v1/applications/{application}/ui/publications",
        json={
            "draftId": None,
            "uiVersionId": version,
            "expectedDraftRevision": None,
            "expectedPublicationRevision": listing["revision"],
            "slug": slug,
            "enabled": enabled,
        },
    )
    assert response.status_code == 200, response.text
    until = time.monotonic() + 120
    while (current := active(suite, application))[0]["activePublicationId"] != response.json()["id"]:
        assert time.monotonic() < until, current
        time.sleep(0.5)
    assert current[1]["manifest"]["enabled"] == enabled
    return current[1]


def mcp_state(suite, application, enabled):
    path = f"/applications/{application}/mcp/publications"
    publication = get(suite["control"], path)[0]["publication"]
    if not enabled:
        for row in get(suite["control"], path):
            if not row["deleted"]:
                removed = suite["control"].delete("/api/v1" + path + "/" + row["publication"]["publicationId"])
                assert removed.status_code == 200, removed.text
        until = time.monotonic() + 120
        while not all(row["deleted"] and row["deletionStatus"] == "published" for row in get(suite["control"], path)):
            assert time.monotonic() < until
            time.sleep(0.5)
        return publication
    response = suite["control"].post(
        "/api/v1" + path,
        json={
            "deploymentId": publication["deploymentId"],
            "expectedRevision": publication["revision"],
            "slug": publication["slug"],
            "toolName": publication["toolName"],
            "description": publication["description"],
            "enabled": True,
        },
    )
    assert response.status_code == 200, response.text
    until = time.monotonic() + 120
    while (current := get(suite["control"], path)[0])["status"] != "published":
        assert time.monotonic() < until, current
        time.sleep(0.5)
    return current["publication"]


def test_web_and_mcp_entrances_do_not_disable_each_other(plan9_suite):
    suite, report = plan9_suite, {"passed": False}
    case = next(row for row in suite["cases"] if row["scene"] == "S01")
    application = case["applicationId"]
    _, page = active(suite, application)
    web_disabled, mcp_disabled = False, False
    try:
        with application_client(suite, case, "Independent entrances") as (client, published, _):
            web_state(suite, application, page["uiVersionId"], page["slug"], False)
            web_disabled = True
            created = client.submit(published["toolName"], published["revision"], case["inputs"][0])
            final, _ = settle(suite, client, created["taskId"], case, 0)
            check_result(suite, client, case, 0, final, 0)
            assert final["taskUrl"] is None
            report["mcpWhileWebDisabled"] = final
            login = suite["control"].post(
                "/api/v1/auth/login",
                auth=None,
                json={"username": suite["employee"]["username"], "password": suite["employee"]["password"].value},
            )
            assert login.status_code == 200
            with httpx.Client(
                base_url=suite["urls"]["runtime"],
                headers={"Authorization": "Bearer " + login.json()["accessToken"]},
                timeout=60,
                trust_env=False,
            ) as employee:
                assert employee.get(f"/gateway/v1/pages/{page['slug']}").status_code == 409
            restored = web_state(suite, application, page["uiVersionId"], page["slug"], True)
            web_disabled = False
            disabled = mcp_state(suite, application, False)
            mcp_disabled = True
            owned = client.client.get(
                f"/gateway/v1/invocations/{final['taskId']}/task", headers={"Authorization": "Bearer " + client.key}
            )
            assert owned.status_code == 200 and owned.json()["outputs"] == final["outputs"], owned.text
            rejected = client.client.post(
                client.path,
                headers={"Authorization": "Bearer " + client.key},
                json={
                    "jsonrpc": "2.0",
                    "id": 88,
                    "method": "tools/call",
                    "params": {
                        "name": disabled["toolName"],
                        "arguments": {
                            "requestId": str(uuid.uuid4()),
                            "publicationRevision": disabled["revision"],
                            "input": case["inputs"][1],
                        },
                    },
                },
            )
            assert rejected.status_code == 404, rejected.text
            report["disabledMcpRejection"] = {"status": rejected.status_code, "body": rejected.json()}
            output = suite["directory"] / "entrance-independence-web.json"
            run_playwright(
                Path(suite["installed"]["root"]),
                "plan9-entrance-independence",
                ("tests/plan9-existing-page.spec.ts",),
                {
                    **browser_environment(suite),
                    "AGENTX_E2E_PLAN9_ENTRANCE_WEB": json.dumps(
                        {"scenario": case, "suffix": restored["slug"], "report": str(output)}, ensure_ascii=False
                    ),
                },
            )
            web = json.loads(output.read_text())
            assert len(web) == 2
            report.update(passed=True, webWhileMcpDisabled=web, existingMcpTaskPreserved=True)
    finally:
        try:
            if mcp_disabled:
                mcp_state(suite, application, True)
            if web_disabled:
                web_state(suite, application, page["uiVersionId"], page["slug"], True)
        finally:
            (suite["directory"] / "entrance-independence.json").write_text(
                json.dumps(report, ensure_ascii=False, indent=2)
            )
