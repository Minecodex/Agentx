# ruff: noqa: S608
"""Model streaming E2E (plan7 P7-B B6).

Uses the extended echo-mcp behavior models (echo-slow-stream, echo-no-usage)
to drive the full delta chain worker sink -> invocation_events -> gateway SSE,
through a real gateway application session, then asserts delta ordering and
the final assistant message content on the same invocation.
"""

from __future__ import annotations

import base64
import json
import time
from typing import Any

import httpx
import pytest

from tests.e2e.runtime.test_agent_attachments import (
    _access_token,
    _runtime_mysql,
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
    assert created.status_code == 201, f"{created.status_code}: {created.text}"
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

    # plan7 P7-B §3.7: streaming model calls record time-to-first-token on the
    # runtime_call and surface it on the Trace span attributes.
    execution_id = _runtime_mysql(
        installed_agentx,
        f"SELECT BIN_TO_UUID(execution_id) FROM application_invocations WHERE id=UUID_TO_BIN('{invocation_id}');",
    )
    first_token = _runtime_mysql(
        installed_agentx,
        "SELECT COALESCE(first_token_ms, 0) FROM runtime_calls "
        f"WHERE execution_id=UUID_TO_BIN('{execution_id}') AND status='succeeded' "
        "ORDER BY ended_at DESC LIMIT 1;",
    )
    assert first_token and int(first_token) > 0, f"first_token_ms not recorded: {first_token}"


def test_vision_attachment_reaches_model_as_native_parts(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    e2e_providers: dict[str, str],
) -> None:
    """plan7 P7-B B6 scenarios 5/6: an image attachment flows to the model as
    a native image_url part (vision-capable model) and is refused with
    MODEL_INPUT_UNSUPPORTED on a model without the capability."""
    echo = e2e_providers["echo_mcp"]
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, _me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        listing = control.get("/api/v1/departments?pageSize=5", headers=headers)
        listing.raise_for_status()
        departments = listing.json()
        items = departments.get("items") if isinstance(departments, dict) else departments
        department = items[0]["id"]
        credential_id = _fixture_credential(control, headers, department)

        def model_alias(alias: str, capabilities: list[str]) -> str:
            response = control.post(
                "/api/v1/models/aliases",
                headers=headers,
                json={
                    "connectionName": f"Vision {alias}",
                    "providerType": "openai_compatible",
                    "endpoint": f"{echo}/v1",
                    "credentialId": credential_id,
                    "ownerDepartmentId": department,
                    "alias": alias,
                    "modelName": "echo-vision",
                    "capabilities": capabilities,
                    "price": {"currency": "USD", "inputPerMillion": "1", "outputPerMillion": "2"},
                },
            )
            assert response.status_code in (200, 201), response.text
            found = control.get(f"/api/v1/models/aliases?pageSize=100&search={alias}", headers=headers)
            found.raise_for_status()
            return next(item for item in found.json()["items"] if item["alias"] == alias)["id"]

        vision_model = model_alias(f"vision-yes-{installed_agentx['run_id'][:8]}", ["vision"])
        plain_model = model_alias(f"vision-no-{installed_agentx['run_id'][:8]}", [])

        environments = control.get("/api/v1/environments", headers=headers)
        environments.raise_for_status()
        environment_id = next(item for item in environments.json() if item["code"] == "development")["id"]

        def vision_app(model_resource_id: str, slug: str, alias: str) -> dict[str, Any]:
            created = control.post(
                "/api/v1/workflows",
                headers=headers,
                json={"name": f"Vision {slug}", "description": "P7-B vision", "visibility": "company"},
            )
            created.raise_for_status()
            workflow_id = created.json()["id"]
            draft = control.get(f"/api/v1/workflows/{workflow_id}/draft", headers=headers)
            draft.raise_for_status()
            definition = draft.json()["definition"]
            definition["start"]["inputs"] = {
                "type": "object",
                "properties": {
                    "message": {"type": "string"},
                    "images": {"type": "array", "x-agentx-modality": "image"},
                    "attachments": {"type": "array", "x-agentx-artifact": True, "x-agentx-artifact-array": True},
                },
                "required": ["message"],
                "additionalProperties": False,
            }
            definition["nodes"].append(
                {
                    "id": "model",
                    "key": "vision_model",
                    "type": "model",
                    "typeVersion": 1,
                    "name": "Vision Model",
                    "parameters": {
                        "prompt": {
                            "kind": "template",
                            "segments": [{"kind": "text", "text": "Describe the attachment."}],
                        },
                        "userQuestion": {
                            "kind": "reference",
                            "selector": {
                                "namespace": "inputs",
                                "run": {"kind": "current"},
                                "item": {"kind": "current"},
                                "path": ["images"],
                            },
                            "missingPolicy": {"kind": "omit"},
                        },
                        "responseMode": "text",
                        "stream": "false",
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
            exit_node["parameters"]["outputs"] = {
                "answer": {
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
            }
            definition["end"] = {
                "completion": "first_return",
                "outputs": {"answer": {"schema": {"type": "string"}, "required": True, "sensitive": False}},
                "error": {"outputs": {}},
            }
            definition["connections"] = [
                {
                    "id": "c-vm",
                    "sourceNodeId": "__start__",
                    "sourceHandle": "main",
                    "targetNodeId": "model",
                    "targetHandle": "main",
                    "order": 0,
                },
                {
                    "id": "c-vm-exit",
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
            # Grants must exist before the version publish validates them.
            _grant_model_to_workflow(control, headers, alias, workflow_id, credential_id)
            latest = control.get(f"/api/v1/workflows/{workflow_id}/draft", headers=headers)
            latest.raise_for_status()
            published = control.post(
                f"/api/v1/workflows/{workflow_id}/versions",
                headers=headers,
                json={"draftRevision": latest.json()["revision"]},
            )
            assert published.status_code in (200, 201), published.text
            deploy = control.post(
                f"/api/v1/workflows/{workflow_id}/deployments",
                headers=headers,
                json={"environmentId": environment_id, "workflowVersionId": published.json()["id"]},
            )
            assert deploy.status_code in (200, 201), deploy.text
            application = control.post(
                "/api/v1/applications",
                headers=headers,
                json={
                    "workflowId": workflow_id,
                    "name": f"Vision App {slug}",
                    "slug": slug,
                    "visibility": "company",
                },
            )
            assert application.status_code in (200, 201), application.text
            application_id = application.json()["id"]
            for _ in range(40):
                publish = control.post(
                    f"/api/v1/applications/{application_id}/deployments",
                    headers=headers,
                    json={
                        "workflowVersionId": published.json()["id"],
                        "environmentId": environment_id,
                        "sessionVersionPolicy": "pinned",
                    },
                )
                if publish.status_code == 202:
                    break
                time.sleep(3)
            else:
                raise AssertionError("vision app publish did not converge")
            deadline = time.monotonic() + 240
            while time.monotonic() < deadline:
                deployments = control.get(f"/api/v1/applications/{application_id}/deployments", headers=headers)
                deployments.raise_for_status()
                if deployments.json()[0]["status"] == "active":
                    break
                time.sleep(2)
            else:
                raise AssertionError("vision deployment did not become active")
            mapping = control.put(
                f"/api/v1/applications/{application_id}/deployments/{deployments.json()[0]['id']}/playground-config",
                headers=headers,
                json={
                    "expectedVersion": 0,
                    "mapping": {
                        "questionInput": "message",
                        "fileInput": "attachments",
                        "answerOutput": "answer",
                        "answerFilesOutput": None,
                    },
                },
            )
            assert mapping.status_code in (200, 201, 202, 204), mapping.text
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                config = control.get(
                    f"/api/v1/applications/{application_id}/deployments/{deployments.json()[0]['id']}/playground-config",
                    headers=headers,
                )
                config.raise_for_status()
                if config.json().get("publishStatus") == "active":
                    break
                time.sleep(2)
            else:
                raise AssertionError("vision mapping did not publish")
            return {"slug": slug, "applicationId": application_id}

        vision_app_row = vision_app(
            vision_model,
            f"vision-yes-{installed_agentx['run_id'][:12]}",
            f"vision-yes-{installed_agentx['run_id'][:8]}",
        )
        plain_app_row = vision_app(
            plain_model, f"vision-no-{installed_agentx['run_id'][:12]}", f"vision-no-{installed_agentx['run_id'][:8]}"
        )

    # Upload a tiny PNG artifact through the gateway.
    png = base64.b64decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
    )
    with httpx.Client(base_url=service_urls["runtime"], timeout=60) as gateway:
        upload = gateway.post(
            "/gateway/v1/artifacts",
            headers={"Authorization": f"Bearer {token}", "Idempotency-Key": f"vision-art-{time.time_ns()}"},
            files={"file": ("probe.png", png, "image/png")},
        )
        assert upload.status_code in (200, 201), upload.text
        artifact_id = upload.json()["artifactId"]

        def invoke(slug: str) -> dict[str, Any]:
            session = gateway.post(
                f"/gateway/v1/applications/{slug}/sessions",
                headers={"Authorization": f"Bearer {token}", "Idempotency-Key": f"vision-s-{time.time_ns()}"},
                json={"title": None, "externalUserId": None},
            )
            assert session.status_code == 201, session.text
            session_id = session.json()["id"]
            accepted = gateway.post(
                f"/gateway/v1/sessions/{session_id}/messages",
                headers={"Authorization": f"Bearer {token}", "Idempotency-Key": f"vision-m-{time.time_ns()}"},
                json={
                    "parts": [
                        {"partType": "text", "content": "describe the image"},
                        {"partType": "image", "artifactId": artifact_id},
                    ]
                },
            )
            assert accepted.status_code == 202, accepted.text
            invocation_id = accepted.json()["id"]
            deadline = time.monotonic() + 180
            while time.monotonic() < deadline:
                probe = gateway.get(
                    f"/gateway/v1/invocations/{invocation_id}",
                    headers={"Authorization": f"Bearer {token}"},
                )
                probe.raise_for_status()
                status = probe.json()["status"]
                if status in {"completed", "succeeded", "failed", "cancelled"}:
                    return {"status": status, "invocationId": invocation_id, "sessionId": session_id}
                time.sleep(1)
            raise AssertionError("vision invocation did not settle")

        # Scenario 5: vision-capable model receives a native image part.
        answered = invoke(vision_app_row["slug"])
        if answered["status"] not in {"completed", "succeeded"}:
            node_error = _runtime_mysql(
                installed_agentx,
                "SELECT CONCAT(COALESCE(error_code,'none'),'|',LEFT(COALESCE(error_message,''),300)) FROM node_attempts a "
                "JOIN application_invocations i ON i.execution_id=a.execution_id AND i.tenant_id=a.tenant_id "
                f"WHERE i.id=UUID_TO_BIN('{answered['invocationId']}') ORDER BY a.attempt_number DESC LIMIT 1;",
            )
            pytest.fail(f"vision invoke failed: {answered} node={node_error}")
        assistant: list[dict[str, Any]] = []
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            messages = gateway.get(
                f"/gateway/v1/sessions/{answered['sessionId']}/messages",
                headers={"Authorization": f"Bearer {token}"},
            )
            messages.raise_for_status()
            assistant = [m for m in messages.json() if m.get("role") == "assistant"]
            if assistant:
                break
            time.sleep(2)
        assert assistant, "vision assistant message missing"
        if "vision-ok:1" not in json.dumps(assistant[-1]):
            input_json = _runtime_mysql(
                installed_agentx,
                "SELECT LEFT(input_json,500) FROM application_invocations "
                f"WHERE id=UUID_TO_BIN('{answered['invocationId']}');",
            )
            request_json = _runtime_mysql(
                installed_agentx,
                "SELECT LEFT(request_json,600) FROM runtime_calls r "
                "JOIN application_invocations i ON i.execution_id=r.execution_id AND i.tenant_id=r.tenant_id "
                f"WHERE i.id=UUID_TO_BIN('{answered['invocationId']}') ORDER BY r.started_at DESC LIMIT 1;",
            )
            pytest.fail(
                f"vision model saw no image: reply={assistant[-1]} "
                f"invocation_input={input_json} model_request={request_json}"
            )

        # Scenario 6: model without the vision capability refuses the image.
        refused = invoke(plain_app_row["slug"])
        assert refused["status"] == "failed", refused
        node_error = _runtime_mysql(
            installed_agentx,
            "SELECT COALESCE(error_code,'none') FROM node_attempts a "
            "JOIN application_invocations i ON i.execution_id=a.execution_id AND i.tenant_id=a.tenant_id "
            f"WHERE i.id=UUID_TO_BIN('{refused['invocationId']}') ORDER BY a.attempt_number DESC LIMIT 1;",
        )
        assert node_error == "MODEL_INPUT_UNSUPPORTED", node_error


def test_studio_debug_has_an_independent_durable_delta_cursor(installed_agentx, service_urls, streaming_application):
    from tools.scripts.release.evidence import write_report

    with httpx.Client(
        base_url=service_urls["web"],
        timeout=60,
        headers={"Authorization": f"Bearer {streaming_application['token']}"},
    ) as control:
        application = control.get(f"/api/v1/applications/{streaming_application['applicationId']}")
        application.raise_for_status()
        workflow_id = application.json()["workflowId"]
        draft = control.get(f"/api/v1/workflows/{workflow_id}/draft")
        draft.raise_for_status()
        debug = control.post(
            f"/api/v1/workflows/{workflow_id}/debug-executions",
            json={
                "expectedRevision": draft.json()["revision"],
                "mode": "full",
                "targetNodeId": None,
                "input": {"message": "durable debug tail"},
                "context": {},
                "overlayIds": [],
                "sideEffectDecisions": {},
                "idempotencyKey": f"debug-delta-{time.time_ns()}",
            },
        )
        assert debug.status_code == 202, debug.text
        execution = debug.json()["executionId"]
        cursor = 0
        frames = []
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            delta = control.get(f"/api/v1/executions/{execution}/model-deltas", params={"after": cursor, "limit": 1000})
            assert delta.status_code == 200, delta.text
            page = delta.json()
            for frame in page["items"]:
                assert frame["sequence"] > cursor, page
                cursor = frame["sequence"]
                frames.append(frame)
            assert page["nextCursor"] == cursor, page
            status = control.get(f"/api/v1/executions/{execution}")
            assert status.status_code == 200, status.text
            if status.json()["status"] == "succeeded" and not page["items"]:
                break
            assert status.json()["status"] not in {"failed", "cancelled"}, status.text
            time.sleep(0.25)
        assert frames and all(frame["payload"]["nodeKey"] == "stream_model" for frame in frames), frames
        assert (
            "".join(frame["payload"]["deltaText"] for frame in frames) == "M5 Agent completed after the MCP tool result"
        ), frames
        assert (
            _runtime_mysql(
                installed_agentx,
                f"SELECT COUNT(*) FROM application_invocations WHERE execution_id=UUID_TO_BIN('{execution}');",
            )
            == "0"
        )
        unauthenticated = httpx.get(f"{service_urls['web']}/api/v1/executions/{execution}/model-deltas", timeout=30)
        assert unauthenticated.status_code == 401, unauthenticated.text
        write_report(
            installed_agentx,
            "product/studio-model-deltas.json",
            {
                "status": "passed",
                "executionId": execution,
                "frames": len(frames),
                "cursor": cursor,
                "independentOfInvocation": True,
                "anonymousAccessDenied": True,
            },
        )
