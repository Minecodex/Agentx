"""Model streaming E2E (plan7 P7-B B6).

Uses the extended echo-mcp behavior models (echo-slow-stream, echo-no-usage)
to drive the full delta chain worker sink -> invocation_events -> gateway SSE,
through a real gateway application session, then asserts delta ordering and
the final assistant message content on the same invocation.
"""

from __future__ import annotations

import json
import time
from typing import Any

import httpx
import pytest

from tests.e2e.runtime.test_agent_attachments import (
    _access_token,
)

pytestmark = [pytest.mark.cluster, pytest.mark.product]


FIXTURE_MODEL_SECRET = "m5-model-secret"  # noqa: S105 -- isolated E2E fixture credential


def _fixture_credential(control: httpx.Client, headers: dict[str, str], department: str) -> str:
    existing = control.get("/api/v1/credentials?pageSize=100&search=Streaming Fixture", headers=headers)
    existing.raise_for_status()
    for item in existing.json().get("items", []):
        if item.get("name") == "Streaming Fixture Credential":
            return item["id"]
    created = control.post(
        "/api/v1/credentials",
        headers=headers,
        json={
            "name": "Streaming Fixture Credential",
            "credentialType": "bearer",
            "secret": FIXTURE_MODEL_SECRET,
            "ownerDepartmentId": department,
        },
    )
    assert created.status_code in (200, 201), created.text
    return created.json()["id"]


def _upsert_model(
    control: httpx.Client,
    headers: dict[str, str],
    alias: str,
    endpoint: str,
    model_name: str,
    department: str,
    credential_id: str,
) -> None:
    existing = control.get(f"/api/v1/models/aliases?pageSize=100&search={alias}", headers=headers)
    existing.raise_for_status()
    if any(item.get("alias") == alias for item in existing.json().get("items", [])):
        return
    response = control.post(
        "/api/v1/models/aliases",
        headers=headers,
        json={
            "connectionName": f"Streaming {alias}",
            "providerType": "openai_compatible",
            "endpoint": f"{endpoint}/v1",
            "credentialId": credential_id,
            "ownerDepartmentId": department,
            "alias": alias,
            "modelName": model_name,
            "price": {"currency": "USD", "inputPerMillion": "1", "outputPerMillion": "2"},
        },
    )
    assert response.status_code in (200, 201), response.text


def _stream_workflow(control: httpx.Client, headers: dict[str, str], name: str, model_resource_id: str) -> str:
    created = control.post(
        "/api/v1/workflows",
        headers=headers,
        json={"name": name, "description": "plan7 P7-B streaming e2e", "visibility": "company"},
    )
    created.raise_for_status()
    workflow_id = created.json()["id"]
    draft = control.get(f"/api/v1/workflows/{workflow_id}/draft", headers=headers)
    draft.raise_for_status()
    definition = draft.json()["definition"]
    definition["start"]["inputs"] = {
        "type": "object",
        "properties": {"message": {"type": "string"}},
        "required": ["message"],
        "additionalProperties": False,
    }
    reference = {
        "kind": "template",
        "segments": [
            {
                "kind": "reference",
                "selector": {
                    "namespace": "inputs",
                    "run": {"kind": "current"},
                    "item": {"kind": "current"},
                    "path": ["message"],
                },
                "missingPolicy": {"kind": "error"},
            }
        ],
    }
    answer = {
        "kind": "reference",
        "selector": {
            "namespace": "outputs",
            "sourceNodeId": "model",
            "port": "main",
            "run": {"kind": "current"},
            "item": {"kind": "first"},
            "path": ["text"],
        },
        "missingPolicy": {"kind": "error"},
    }
    definition["nodes"].append(
        {
            "id": "model",
            "key": "stream_model",
            "type": "model",
            "typeVersion": 1,
            "name": "Stream Model",
            "parameters": {
                "prompt": {
                    "kind": "template",
                    "segments": [{"kind": "text", "text": "Answer with the canned mock stream."}],
                },
                "userQuestion": reference,
                "responseMode": "text",
                "stream": "true",
            },
            "contextWrites": [],
            "resourceReferences": [
                {
                    "bindingRole": "model",
                    "resourceType": "model",
                    "resourceId": model_resource_id,
                    "operation": "use",
                }
            ],
        }
    )
    exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
    exit_node["parameters"]["outputs"] = {"answer": answer}
    definition["end"] = {
        "completion": "first_return",
        "outputs": {"answer": {"schema": {"type": "string"}, "required": True, "sensitive": False}},
        "error": {"outputs": {}},
    }
    definition["connections"] = [
        {
            "id": "c-sm",
            "sourceNodeId": "__start__",
            "sourceHandle": "main",
            "targetNodeId": "model",
            "targetHandle": "main",
            "order": 0,
        },
        {
            "id": "c-me",
            "sourceNodeId": "model",
            "sourceHandle": "main",
            "targetNodeId": exit_node["id"],
            "targetHandle": "main",
            "order": 0,
        },
    ]
    saved = control.put(
        f"/api/v1/workflows/{workflow_id}/draft",
        headers=headers,
        json={"expectedRevision": draft.json()["revision"], "definition": definition},
    )
    assert saved.status_code in (200, 204), saved.text
    return workflow_id


def _grant_model_to_workflow(
    control: httpx.Client,
    headers: dict[str, str],
    alias: str,
    workflow_id: str,
    credential_id: str,
) -> None:
    """Grant both the model alias and its credential to the workflow identity."""
    listing = control.get(f"/api/v1/models/aliases?pageSize=100&search={alias}", headers=headers)
    listing.raise_for_status()
    model = next(item for item in listing.json()["items"] if item["alias"] == alias)
    identity = control.get(f"/api/v1/workflows/{workflow_id}", headers=headers)
    identity.raise_for_status()
    subject = identity.json().get("serviceIdentityId")
    assert subject, identity.text
    for resource_type, resource_id in (("model", model["id"]), ("credential", credential_id)):
        response = control.post(
            f"/api/v1/resources/{resource_type}/{resource_id}/grants",
            headers=headers,
            json={
                "subjectType": "workflow_service_identity",
                "subjectId": subject,
                "resourceVersionId": None,
                "operation": "use",
            },
        )
        assert response.status_code in (200, 201, 409), response.text


@pytest.fixture(scope="module")
def streaming_application(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    e2e_providers: dict[str, str],
    run_id: str,
) -> dict[str, Any]:
    """A published chat application whose model node binds the slow-stream mock."""
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        environments = control.get("/api/v1/environments", headers=headers)
        environments.raise_for_status()
        environment = next(item for item in environments.json() if item["code"] == "development")
        echo = e2e_providers["echo_mcp"]
        credential_id = _fixture_credential(control, headers, me["departmentId"])
        _upsert_model(
            control,
            headers,
            "stream-slow",
            echo,
            "echo-slow-stream",
            me["departmentId"],
            credential_id,
        )
        listing = control.get("/api/v1/models/aliases?pageSize=100&search=stream-slow", headers=headers)
        listing.raise_for_status()
        model_resource_id = next(item for item in listing.json()["items"] if item["alias"] == "stream-slow")["id"]
        workflow_id = _stream_workflow(control, headers, f"Streaming E2E {run_id}", model_resource_id)
        _grant_model_to_workflow(control, headers, "stream-slow", workflow_id, credential_id)
        latest_draft = control.get(f"/api/v1/workflows/{workflow_id}/draft", headers=headers)
        latest_draft.raise_for_status()
        published = control.post(
            f"/api/v1/workflows/{workflow_id}/versions",
            headers=headers,
            json={"draftRevision": latest_draft.json()["revision"]},
        )
        assert published.status_code in (200, 201), published.text
        version_id = published.json()["id"]
        # The application deployment gate requires the version to be the
        # workflow-level active deployment in the environment first.
        workflow_deploy = control.post(
            f"/api/v1/workflows/{workflow_id}/deployments",
            headers=headers,
            json={"environmentId": environment["id"], "workflowVersionId": version_id},
        )
        assert workflow_deploy.status_code in (200, 201), workflow_deploy.text
        application = control.post(
            "/api/v1/applications",
            headers=headers,
            json={
                "workflowId": workflow_id,
                "name": f"Streaming App {run_id}",
                "slug": f"stream-{run_id[:8]}",
                "description": "plan7 P7-B streaming e2e",
                "visibility": "company",
            },
        )
        assert application.status_code in (200, 201), application.text
        application_id = application.json()["id"]
        api_key_response = control.post(
            f"/api/v1/applications/{application_id}/api-keys",
            headers=headers,
            json={"name": f"stream {run_id}"},
        )
        assert api_key_response.status_code == 201, api_key_response.text
        api_key = api_key_response.json()["secret"]
        last_error = ""
        for _ in range(10):
            publish = control.post(
                f"/api/v1/applications/{application_id}/deployments",
                headers=headers,
                json={
                    "workflowVersionId": version_id,
                    "environmentId": environment["id"],
                    "sessionVersionPolicy": "pinned",
                },
            )
            if publish.status_code == 202:
                break
            last_error = f"{publish.status_code} {publish.text}"
            time.sleep(3)
        else:
            raise AssertionError(f"publish failed: {last_error}")
        deadline = time.monotonic() + 240
        while time.monotonic() < deadline:
            deployments = control.get(f"/api/v1/applications/{application_id}/deployments", headers=headers)
            deployments.raise_for_status()
            latest = deployments.json()[0]  # plain array, newest first
            if latest["status"] == "active":
                break
            assert latest["status"] != "rejected", latest
            time.sleep(2)
        else:
            raise AssertionError("deployment did not become active")
        mapping = control.put(
            f"/api/v1/applications/{application_id}/deployments/{latest['id']}/playground-config",
            headers=headers,
            json={
                "expectedVersion": 0,
                "mapping": {
                    "questionInput": "message",
                    "fileInput": None,
                    "answerOutput": "answer",
                    "answerFilesOutput": None,
                },
            },
        )
        assert mapping.status_code in (200, 201, 202, 204), mapping.text
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            config = control.get(
                f"/api/v1/applications/{application_id}/deployments/{latest['id']}/playground-config",
                headers=headers,
            )
            config.raise_for_status()
            status = config.json().get("publishStatus")
            if status == "active":
                break
            assert status != "failed", config.text
            time.sleep(2)
        else:
            raise AssertionError("playground mapping did not publish")
        detail = control.get(f"/api/v1/applications/{application_id}", headers=headers)
        detail.raise_for_status()
        yield {
            "applicationId": application_id,
            "applicationSlug": detail.json()["slug"],
            "apiKey": api_key,
            "token": token,
        }


def _sse_events(
    gateway: httpx.Client, headers: dict[str, str], invocation_id: str, timeout: float = 90
) -> list[tuple[str, dict[str, Any]]]:
    events: list[tuple[str, dict[str, Any]]] = []
    event_type = "message"
    terminal = {"invocation.completed", "invocation.failed", "invocation.cancelled"}
    with gateway.stream(
        "GET", f"/gateway/v1/invocations/{invocation_id}/events", headers=headers, timeout=timeout
    ) as response:
        assert response.status_code == 200, response.text
        for line in response.iter_lines():
            if line.startswith("event:"):
                event_type = line.split(":", 1)[1].strip()
            elif line.startswith("data:"):
                payload = line.split(":", 1)[1].strip()
                try:
                    events.append((event_type, json.loads(payload)))
                except json.JSONDecodeError:
                    events.append((event_type, {"raw": payload}))
                if event_type in terminal:
                    break
    return events


def test_stream_deltas_arrive_before_terminal_in_order(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    streaming_application: dict[str, Any],
) -> None:
    app = streaming_application
    with httpx.Client(base_url=service_urls["runtime"], timeout=90) as gateway:
        session = gateway.post(
            f"/gateway/v1/applications/{app['applicationSlug']}/sessions",
            headers={
                "Authorization": f"Bearer {app['token']}",
                "Idempotency-Key": f"stream-session-{time.time_ns()}",
            },
            json={"title": None, "externalUserId": None},
        )
        assert session.status_code == 201, session.text
        session_id = session.json()["id"]
        # The chat-message endpoint (not the raw application invocation API)
        # attaches the ChatMapping that drives input projection and the
        # assistant message projection at terminal.
        accepted = gateway.post(
            f"/gateway/v1/sessions/{session_id}/messages",
            headers={
                "Authorization": f"Bearer {app['apiKey']}",
                "Idempotency-Key": f"stream-invoke-{time.time_ns()}",
            },
            json={"parts": [{"partType": "text", "content": "stream the mock reply"}]},
        )
        assert accepted.status_code == 202, accepted.text
        invocation_id = accepted.json()["id"]
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            status = gateway.get(
                f"/gateway/v1/invocations/{invocation_id}",
                headers={"Authorization": f"Bearer {app['apiKey']}"},
            )
            status.raise_for_status()
            body = status.json()
            if body["status"] in {"completed", "succeeded", "failed", "cancelled"}:
                if body["status"] == "failed":
                    raise AssertionError(f"invocation failed: {json.dumps(body, ensure_ascii=False)}")
                break
            time.sleep(1)
        else:
            raise AssertionError("invocation did not settle")

        # Re-read the full event stream from sequence 0: delta order and the
        # terminal marker must be consistent for any reconnecting client.
        events = _sse_events(gateway, {"Authorization": f"Bearer {app['token']}"}, invocation_id)
    deltas = [payload.get("deltaText") for kind, payload in events if kind == "model.delta"]
    terminal_events = [kind for kind, _ in events]
    assert deltas, events
    assert "".join(filter(None, deltas)), deltas
    assert terminal_events[-1].startswith("invocation."), terminal_events
    assert terminal_events.index("model.delta") < len(terminal_events) - 1

    assistant: list[dict[str, Any]] = []
    deadline = time.monotonic() + 30
    with httpx.Client(base_url=service_urls["runtime"], timeout=60) as gateway:
        while time.monotonic() < deadline:
            messages = gateway.get(
                f"/gateway/v1/sessions/{session_id}/messages",
                headers={"Authorization": f"Bearer {app['token']}"},
            )
            messages.raise_for_status()
            assistant = [m for m in messages.json() if m.get("role") == "assistant"]
            if assistant:
                break
            time.sleep(2)
    assert assistant, "assistant message was not projected"
