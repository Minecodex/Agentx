"""Real configured-model resources for local acceptance; secrets stay in memory."""

from __future__ import annotations

import time
from dataclasses import dataclass, field

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft, fixture_draft_id
from tests.e2e.product.test_model_streaming import _grant_model_to_workflow, _stream_workflow
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.runtime.test_agent_attachments import _access_token


@dataclass
class Secret:
    value: str = field(repr=False)


def post(client: httpx.Client, path: str, payload: dict) -> dict:
    response = client.post(f"/api/v1{path}", json=payload)
    assert response.status_code in (200, 201, 202), response.text
    return response.json()


def publish_version(control: httpx.Client, workflow_id: str, prompt: str) -> dict:
    draft = control.get(f"/api/v1/workflows/{workflow_id}/drafts/{fixture_draft_id(control, workflow_id, None)}").json()
    definition = draft["definition"]
    model = next(node for node in definition["nodes"] if node["type"] == "model")
    model["parameters"]["prompt"] = {"kind": "template", "segments": [{"kind": "text", "text": prompt}]}
    for reference in model["resourceReferences"]:
        if reference["resourceType"] == "model":
            reference.pop("bindingRole", None)
            selected = control.get(f"/api/v1/models/aliases/{reference['resourceId']}")
            selected.raise_for_status()
            reference["resourceVersionId"] = selected.json()["deploymentId"]
    saved = control.put(
        f"/api/v1/workflows/{workflow_id}/drafts/{fixture_draft_id(control, workflow_id, None)}",
        json={
            "editorDocument": fixture_draft(control, workflow_id, None)["editorDocument"],
            "expectedRevision": draft["revision"],
            "definition": definition,
        },
    )
    assert saved.status_code in (200, 204), saved.text
    revision = control.get(
        f"/api/v1/workflows/{workflow_id}/drafts/{fixture_draft_id(control, workflow_id, None)}"
    ).json()["revision"]
    return post(
        control,
        f"/workflows/{workflow_id}/versions",
        {"draftId": fixture_draft(control, workflow_id, None)["id"], "draftRevision": revision},
    )


@pytest.fixture(scope="session")
def live_application(installed_agentx, service_urls, run_id, existing_model, existing_model_secret):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        control.headers.update(headers)
        credential = post(
            control,
            "/credentials",
            {
                "name": f"P7 live GLM {run_id}",
                "credentialType": "bearer",
                "secret": existing_model_secret.value,
                "ownerDepartmentId": me["departmentId"],
            },
        )
        alias = f"p7-live-model-{run_id}"
        model = post(
            control,
            "/models/aliases",
            {
                "connectionName": alias,
                **{
                    key: existing_model[key]
                    for key in (
                        "providerType",
                        "endpoint",
                        "modelName",
                        "defaultParameters",
                        "maxInputTokens",
                        "maxOutputTokens",
                    )
                },
                "credentialId": credential["id"],
                "ownerDepartmentId": me["departmentId"],
                "alias": alias,
                "price": {
                    "currency": existing_model["currency"],
                    "inputPerMillion": existing_model["inputPrice"],
                    "outputPerMillion": existing_model["outputPrice"],
                },
            },
        )
        workflow_id = _stream_workflow(control, headers, f"P7 live text {run_id}", model["id"])
        _grant_model_to_workflow(control, headers, alias, workflow_id, credential["id"])
        v1 = publish_version(
            control, workflow_id, "你是中文测试助手。准确回答用户要求。普通问答保持简短。用户要求逐字回复时严格遵循。"
        )
        v2 = publish_version(
            control,
            workflow_id,
            "你是中文测试助手。准确回答用户要求。普通问答分条详细说明。用户要求逐字回复时严格遵循。",
        )
        environment = next(item for item in control.get("/api/v1/environments").json() if item["code"] == "development")
        post(
            control,
            f"/workflows/{workflow_id}/deployments",
            {"environmentId": environment["id"], "workflowVersionId": v2["id"]},
        )
        app = post(
            control,
            "/applications",
            {
                "workflowId": workflow_id,
                "name": f"P7 live chat {run_id}",
                "slug": f"p7-live-{run_id}",
                "visibility": "company",
            },
        )
        deployment = _publish_application_deployment(control, headers, app["id"], v2["id"], environment["id"])
        config_path = f"/api/v1/applications/{app['id']}/deployments/{deployment['id']}/playground-config"
        current = control.get(config_path).json()
        mapped = control.put(
            config_path,
            json={
                "expectedVersion": current["version"],
                "mapping": {
                    "questionInput": "message",
                    "fileInput": None,
                    "answerOutput": "answer",
                    "answerFilesOutput": None,
                },
            },
        )
        assert mapped.status_code in (200, 202), mapped.text
        deadline = time.monotonic() + 120
        while True:
            mapping = control.get(config_path).json()
            if mapping["publishStatus"] == "active":
                break
            assert mapping["publishStatus"] != "failed", mapping
            assert time.monotonic() < deadline, "live chat mapping did not publish"
            time.sleep(1)
        return {
            "applicationId": app["id"],
            "applicationSlug": app["slug"],
            "workflowId": workflow_id,
            "v1": v1["id"],
            "v2": v2["id"],
            "modelId": model["id"],
            "modelAlias": alias,
            "credentialId": credential["id"],
            "departmentId": me["departmentId"],
            "token": Secret(token),
        }
