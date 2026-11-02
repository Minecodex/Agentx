"""Actual Agentx login/consent, opaque personal tokens, refresh and UI grant revocation."""

import base64
import hashlib
import json
import secrets
import uuid
from pathlib import Path
from urllib.parse import urlencode, urlsplit

import httpx
import pytest

from tests.e2e.product.live_text_support import Secret
from tests.e2e.product.plan9.mcp_client import ApplicationMcpClient
from tests.e2e.product.plan9.mcp_support import application_client, check_result, publication, settle
from tests.e2e.product.plan9.oauth_callback import oauth_callback
from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def browser(suite, phase, parameters):
    output = suite["directory"] / f"personal-oauth-{phase}.json"
    run_playwright(
        Path(suite["installed"]["root"]),
        f"plan9-personal-oauth-{phase}",
        ("tests/plan9-oauth.spec.ts",),
        {
            **browser_environment(suite),
            "AGENTX_E2E_PLAN9_OAUTH": json.dumps({"phase": phase, **parameters}),
            "AGENTX_E2E_PLAN9_OAUTH_REPORT": str(output),
        },
    )
    return json.loads(output.read_text()) if output.exists() else None


def test_personal_oauth_login_consent_refresh_and_revocation(plan9_suite):
    suite = plan9_suite
    case = next(value for value in suite["cases"] if value["scene"] == "S01")
    published = publication(suite, case)
    name = f"Personal MCP {suite['runId']}"
    state = secrets.token_urlsafe(24)
    common = {"applicationId": case["applicationId"], "name": name}
    client_id = None
    client = None
    private = []
    report = {"passed": False}
    try:
        with (
            oauth_callback(state) as callback,
            httpx.Client(base_url=suite["urls"]["runtime"], timeout=60, trust_env=False) as transport,
        ):
            registered = browser(suite, "register", {**common, "callback": callback.url})
            client_id = registered["client"]["clientId"]
            verifier = Secret(secrets.token_urlsafe(48))
            private.append(verifier)
            challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.value.encode()).digest()).decode().rstrip("=")
            resource = f"{suite['urls']['runtime']}/gateway/v1/mcp/applications/{published['slug']}"
            authorization = f"{suite['urls']['runtime']}/oauth2/auth?" + urlencode(
                {
                    "response_type": "code",
                    "client_id": client_id,
                    "redirect_uri": callback.url,
                    "scope": f"agentx:application:{case['applicationId']} offline_access",
                    "state": state,
                    "code_challenge": challenge,
                    "code_challenge_method": "S256",
                    "audience": resource,
                }
            )
            browser(suite, "authorize", {**common, "callback": callback.url, "authorizationUrl": authorization})
            assert callback.completed.wait(5) and callback.code.value and not callback.error
            response = transport.post(
                "/oauth2/token",
                data={
                    "grant_type": "authorization_code",
                    "code": callback.code.value,
                    "redirect_uri": callback.url,
                    "client_id": client_id,
                    "code_verifier": verifier.value,
                },
            )
            assert response.status_code == 200, response.text
            callback.code.value = ""
            tokens = response.json()
            access, refresh = Secret(tokens.pop("access_token")), Secret(tokens.pop("refresh_token"))
            private.extend((access, refresh))
            opaque = access.value.startswith("ory_at_") and refresh.value.startswith("ory_rt_")
            assert opaque, "Hydra must return opaque access and refresh tokens"
            report.update(clientId=client_id, loginConsentSucceeded=True)
            assert tokens["token_type"].lower() == "bearer"
            client = ApplicationMcpClient(
                transport,
                f"/gateway/v1/mcp/applications/{published['slug']}",
                access.value,
                urlsplit(suite["urls"]["runtime"]).netloc,
            )
            client.initialize()
            client.tools(published["toolName"])
            task = client.submit(published["toolName"], published["revision"], case["inputs"][0])
            task, _ = settle(suite, client, task["taskId"], case, 0)
            proof = check_result(suite, client, case, 0, task, 0)
            assert proof["callerType"] == "user" and proof["callerId"] == uuid.UUID(suite["employee"]["id"]).hex
            with application_client(suite, case, "Personal task ownership negative test") as (service, _, _):
                service.call("get_task", {"taskId": task["taskId"]}, error=True)
            renewed = transport.post(
                "/oauth2/token",
                data={"grant_type": "refresh_token", "refresh_token": refresh.value, "client_id": client_id},
            )
            assert renewed.status_code == 200, renewed.text
            refreshed = renewed.json()
            renewed_access, renewed_refresh = (
                Secret(refreshed.pop("access_token")),
                Secret(refreshed.pop("refresh_token")),
            )
            private.extend((renewed_access, renewed_refresh))
            client.key = renewed_access.value
            report["refreshSucceeded"] = True
            assert client.task(task["taskId"])["state"] == "completed"
            parallel_publication = suite["control"].post(
                f"/api/v1/applications/{case['applicationId']}/mcp/publications",
                json={
                    "deploymentId": case["deploymentId"],
                    "expectedRevision": published["revision"],
                    "slug": published["slug"] + "-new",
                    "toolName": published["toolName"],
                    "description": published["description"],
                    "enabled": True,
                },
            )
            assert parallel_publication.status_code == 200, parallel_publication.text
            assert client.task(task["taskId"])["state"] == "completed", (
                "Publishing a new address must preserve the original OAuth client"
            )
            report["parallelPublicationId"] = parallel_publication.json()["publication"]["publicationId"]
            browser(suite, "revoke", common)
            revoked = transport.post(
                client.path,
                headers={"Authorization": f"Bearer {client.key}", "Accept": "application/json, text/event-stream"},
                json={"jsonrpc": "2.0", "id": 91, "method": "tools/list"},
            )
            assert revoked.status_code == 401, revoked.text
            rejected_refresh = transport.post(
                "/oauth2/token",
                data={
                    "grant_type": "refresh_token",
                    "refresh_token": renewed_refresh.value,
                    "client_id": client_id,
                },
            )
            assert rejected_refresh.status_code == 400, rejected_refresh.text
            report.update(
                passed=True,
                clientId=client_id,
                runtimeProof=proof,
                records=client.records,
                refreshSucceeded=True,
                revokedAccessStatus=revoked.status_code,
                revokedRefreshStatus=rejected_refresh.status_code,
            )
            tokens.clear()
            refreshed.clear()
            client.key = ""
    finally:
        for value in private:
            value.value = ""
        if client:
            client.key = ""
            report["records"] = client.records
        try:
            if client_id:
                for _ in range(2):
                    removed = suite["control"].delete(
                        f"/api/v1/applications/{case['applicationId']}/mcp/clients/{client_id}"
                    )
                    assert removed.status_code == 200, removed.text
        finally:
            (suite["directory"] / "personal-oauth-results.json").write_text(
                json.dumps(report, ensure_ascii=False, indent=2)
            )
