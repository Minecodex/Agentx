"""Actual UI publication history controls the independently callable MCP contracts."""

from __future__ import annotations

import base64
import hashlib
import json
import os
import secrets
import time
import uuid
from contextlib import ExitStack
from pathlib import Path
from urllib.parse import urlencode

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.live_text_support import Secret, post
from tests.e2e.product.plan9.mcp_client import ApplicationMcpClient
from tests.e2e.product.plan9.oauth_callback import oauth_callback
from tests.e2e.product.plan9.opencode_client import isolated_opencode
from tests.e2e.product.plan9.public_origins import plan9_public_origins as plan9_public_origins
from tests.e2e.product.test_channel_delivery import _passthrough_workflow
from tests.e2e.product.test_live_memory_pipeline import _save_version
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import ROOT, run_playwright
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.gateway]


def wait_for(read, matches, timeout=120):
    until = time.monotonic() + timeout
    while True:
        value = read()
        if matches(value):
            return value
        assert time.monotonic() < until, value
        time.sleep(0.4)


def test_mcp_publications_coexist_until_deleted_in_real_ui(
    installed_agentx, service_urls, run_id, plan9_public_origins
):
    directory = Path(installed_agentx["artifact_dir"]) / "mcp-publications"
    directory.mkdir(exist_ok=True)
    with ExitStack() as stack, httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as control:
        token, _ = _access_token(control)
        control.headers["Authorization"] = "Bearer " + token
        workflow = _passthrough_workflow(control, dict(control.headers), f"MCP publication history {run_id}")
        environment = next(row for row in control.get("/api/v1/environments").json() if row["code"] == "development")
        applications = []
        for index in range(2):
            app = post(
                control,
                "/applications",
                {
                    "workflowId": workflow["workflowId"],
                    "name": f"MCP history {index} {run_id}",
                    "description": "A published business application",
                    "slug": f"mcp-history-{index}-{run_id}",
                    "visibility": "company",
                },
            )
            deployment = _publish_application_deployment(
                control, dict(control.headers), app["id"], workflow["versionId"], environment["id"]
            )
            applications.append((app, deployment))
        app, _ = applications[0]
        secret = post(control, f"/applications/{app['id']}/api-keys", {"name": "Publication test client"})["secret"]
        base = f"/api/v1/applications/{app['id']}/mcp/publications"
        old_slug, new_slug = f"history-old-{run_id}", f"history-new-{run_id}"
        report = {"passed": False, "phases": [], "tasks": []}

        def browser(phase, **values):
            output = directory / f"browser-{len(report['phases'])}-{phase}.json"
            run_playwright(
                ROOT,
                f"mcp-publications-{phase}-{len(report['phases'])}",
                ("tests/application-mcp-publications.spec.ts",),
                {
                    **os.environ,
                    "AGENTX_E2E_BASE_URL": service_urls["web"],
                    "AGENTX_E2E_STAGE": "mcp-publications",
                    "AGENTX_E2E_RUN_ID": run_id,
                    "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
                    "AGENTX_E2E_MCP_PUBLICATIONS": json.dumps(
                        {"applicationId": app["id"], "phase": phase, "report": str(output), **values}
                    ),
                },
            )
            result = json.loads(output.read_text())
            assert result["passed"]
            report["phases"].append({"phase": phase, **result})
            return result

        def settled(row):
            task = wait_for(
                lambda: row["client"].task(row["task"]["taskId"]),
                lambda value: value["state"] in {"completed", "failed", "cancelled", "outcome_unknown"},
            )
            assert task["state"] == "completed" and task["outputs"] == {"answer": row["message"]}, task
            report["tasks"].append(task)
            return task

        try:
            first = browser("first", toolName="run_original", slug=old_slug)["revision"]["publication"]
            with httpx.Client(base_url=service_urls["runtime"], timeout=60, trust_env=False) as transport:
                clients = {
                    slug: ApplicationMcpClient(transport, f"/gateway/v1/mcp/applications/{slug}", secret)
                    for slug in (old_slug, new_slug)
                }
                clients[old_slug].initialize()
                clients[old_slug].tools("run_original")
                original = clients[old_slug].submit(
                    "run_original", first["revision"], {"message": "original task remains"}
                )
                original_task = settled(
                    {"client": clients[old_slug], "task": original, "message": "original task remains"}
                )
                state = secrets.token_urlsafe(24)
                callback = stack.enter_context(oauth_callback(state))
                registered = browser("register", callback=callback.url)["client"]
                assert registered["publicationId"] == first["publicationId"]
                verifier = Secret(secrets.token_urlsafe(48))
                challenge = (
                    base64.urlsafe_b64encode(hashlib.sha256(verifier.value.encode()).digest()).decode().rstrip("=")
                )
                resource = service_urls["runtime"] + clients[old_slug].path
                authorization = (
                    service_urls["runtime"]
                    + "/oauth2/auth?"
                    + urlencode(
                        {
                            "response_type": "code",
                            "client_id": registered["clientId"],
                            "redirect_uri": callback.url,
                            "scope": registered["scope"],
                            "state": state,
                            "code_challenge": challenge,
                            "code_challenge_method": "S256",
                            "audience": resource,
                        }
                    )
                )
                browser("authorize", callback=callback.url, authorizationUrl=authorization)
                assert callback.completed.wait(5) and callback.code.value and not callback.error
                exchanged = transport.post(
                    "/oauth2/token",
                    data={
                        "grant_type": "authorization_code",
                        "code": callback.code.value,
                        "redirect_uri": callback.url,
                        "client_id": registered["clientId"],
                        "code_verifier": verifier.value,
                    },
                )
                assert exchanged.status_code == 200
                grant = exchanged.json()
                personal = ApplicationMcpClient(transport, clients[old_slug].path, Secret(grant["access_token"]).value)
                personal.initialize()
                own = personal.submit("run_original", first["revision"], {"message": "personal OAuth task"})
                own_task = settled({"client": personal, "task": own, "message": "personal OAuth task"})
                assert own_task["callerType"] == "user"
                changed_definition = fixture_draft(control, workflow["workflowId"])["definition"]
                changed_definition["start"]["inputs"]["properties"]["category"] = {"type": "string"}
                changed_definition["start"]["inputs"]["required"].append("category")
                changed_version = _save_version(control, workflow["workflowId"], changed_definition)
                changed_deployment = _publish_application_deployment(
                    control, dict(control.headers), app["id"], changed_version["id"], environment["id"]
                )
                second = browser(
                    "change", toolName="run_changed", slug=new_slug, description="Changed tool and address"
                )["revision"]["publication"]
                third = browser(
                    "change", toolName="run_another", slug=old_slug, description="Another tool on the retained address"
                )["revision"]["publication"]
                assert second["deploymentId"] == changed_deployment["id"] and second["bundleId"] != first["bundleId"]
                same_name = browser(
                    "change",
                    toolName="run_original",
                    slug=old_slug,
                    description="Same tool name with a changed business schema",
                )["revision"]["publication"]
                for pub in (first, second, third, same_name):
                    client = clients[pub["slug"]]
                    client.initialize()
                    tools = client.rpc("tools/list")["tools"]
                    business = {"run_original", "run_another"} if pub["slug"] == old_slug else {"run_changed"}
                    assert {tool["name"] for tool in tools} == business | {
                        "get_task",
                        "submit_task_input",
                        "cancel_task",
                    }
                    message = f"retained revision {pub['revision']}"
                    if pub["slug"] == old_slug:
                        current_schema = next(tool for tool in tools if tool["name"] == "run_original")["inputSchema"][
                            "properties"
                        ]
                        assert current_schema["publicationRevision"]["default"] == same_name["revision"]
                        assert "category" in current_schema["input"]["required"]
                    inputs = {
                        "message": message,
                        **({"category": "new-business-version"} if pub["revision"] != first["revision"] else {}),
                    }
                    task = client.submit(pub["toolName"], pub["revision"], inputs)
                    assert task["accessBinding"]["publicationId"] == pub["publicationId"]
                    settled({"client": client, "task": task, "message": message})
                assert personal.task(own["taskId"])["outputs"] == own_task["outputs"], (
                    "Changing a suffix must preserve the original personal client"
                )
                wrong_audience = transport.post(
                    clients[new_slug].path,
                    headers={"Authorization": "Bearer " + personal.key},
                    json={"jsonrpc": "2.0", "id": 7, "method": "tools/list"},
                )
                assert wrong_audience.status_code == 401, wrong_audience.text
                refreshed = transport.post(
                    "/oauth2/token",
                    data={
                        "grant_type": "refresh_token",
                        "refresh_token": grant["refresh_token"],
                        "client_id": registered["clientId"],
                    },
                )
                assert refreshed.status_code == 200
                personal.key = refreshed.json()["access_token"]
                assert personal.task(own["taskId"])["state"] == "completed"
                with isolated_opencode(
                    "https://127.0.0.1:1/v1",
                    "unused-no-model-request",
                    mcp_url=service_urls["runtime"] + clients[old_slug].path,
                    mcp_key=secret,
                ) as native:
                    connection = native.execute(["mcp", "list"], timeout=90)
                    assert "connected" in connection, connection
                    report["nativeClientConnectedToMultipleTools"] = True
                other, deployment = applications[1]
                # Collision checks include retained addresses, not just the most recent one.
                deploy_id = deployment["id"]
                candidate = {
                    "deploymentId": deploy_id,
                    "expectedRevision": 0,
                    "slug": old_slug,
                    "toolName": "other_application",
                    "description": "Owned by another application",
                    "enabled": True,
                }
                collision = control.post(f"/api/v1/applications/{other['id']}/mcp/publications", json=candidate)
                assert collision.status_code == 409 and collision.json()["code"] == "MCP_SLUG_CONFLICT", collision.text
                clients[new_slug].call(
                    "run_original",
                    {
                        "requestId": str(uuid.uuid4()),
                        "publicationRevision": first["revision"],
                        "input": {"message": "cross address forbidden"},
                    },
                    error=True,
                )
                browser("delete", revision=first["revision"])
                clients[old_slug].call(
                    "run_original",
                    {
                        "requestId": str(uuid.uuid4()),
                        "publicationRevision": first["revision"],
                        "input": {"message": "deleted revision forbidden"},
                    },
                    error=True,
                )
                assert clients[old_slug].task(original["taskId"])["outputs"] == original_task["outputs"]
                assert personal.task(own["taskId"])["outputs"] == own_task["outputs"], (
                    "Another retained publication on the same suffix keeps the original audience usable"
                )
                browser("delete", revision=second["revision"])
                denied = transport.post(
                    f"/gateway/v1/mcp/applications/{new_slug}",
                    headers={"Authorization": "Bearer " + secret},
                    json={"jsonrpc": "2.0", "id": 8, "method": "tools/list"},
                )
                assert denied.status_code == 404, denied.text
                clients[old_slug].initialize()
                browser("delete", revision=same_name["revision"])
                remaining = clients[old_slug].rpc("tools/list")["tools"]
                assert {tool["name"] for tool in remaining} == {
                    "run_another",
                    "get_task",
                    "submit_task_input",
                    "cancel_task",
                }
                browser("delete", revision=third["revision"])
                browser("empty")
                metadata = transport.get(
                    f"/gateway/v1/.well-known/oauth-protected-resource/mcp/applications/{old_slug}"
                )
                assert metadata.status_code == 404, metadata.text
                denied_personal = transport.post(
                    personal.path,
                    headers={"Authorization": "Bearer " + personal.key},
                    json={"jsonrpc": "2.0", "id": 8, "method": "tools/list"},
                )
                assert denied_personal.status_code == 401, denied_personal.text
                report["personalOAuth"] = {
                    "realLoginAndConsent": True,
                    "refreshPassed": True,
                    "oldAddressPreservedAfterPublish": True,
                    "crossAddressDenied": True,
                    "deletedAddressDenied": True,
                }
                # A final deletion releases the slug for another application.
                reused = control.post(f"/api/v1/applications/{other['id']}/mcp/publications", json=candidate)
                assert reused.status_code == 200, reused.text
                fourth = browser("first", toolName="run_republished", slug=new_slug)["revision"]["publication"]
                assert fourth["revision"] == same_name["revision"] + 1
                assert control.get(base).json()[0]["publication"]["revision"] == fourth["revision"]
                report.update(
                    passed=True,
                    addressReusePublication=reused.json(),
                    retiredTaskPreserved=original_task,
                    protocolRecords={slug: client.records for slug, client in clients.items()},
                )
        finally:
            write_report(installed_agentx, "mcp-publications/report.json", report)
