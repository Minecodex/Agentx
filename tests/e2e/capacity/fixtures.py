"""Create and publish the actual workflows used by capacity runs."""
# ruff: noqa: S608 -- fixture UUIDs in the isolated E2E databases

from __future__ import annotations

import time
from typing import Any

import httpx

from tests.e2e.draft_fixtures import fixture_draft, fixture_draft_id
from tests.e2e.product.test_channel_delivery import _development_environment_id, _publish_application
from tests.e2e.runtime.test_agent_attachments import _access_token, _runtime_mysql
from tests.e2e.workflow_fixtures import set_chain_definition


def create_caller_keys(control, application_id, count=100):
    keys = []
    for index in range(count):
        response = control.post(
            f"/api/v1/applications/{application_id}/api-keys", json={"name": f"Capacity caller {index}"}
        )
        response.raise_for_status()
        keys.append(response.json()["secret"])
    return keys


def capacity_application(context: dict[str, str], control_url: str, nodes: int) -> dict[str, Any]:
    with httpx.Client(base_url=control_url, timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        stamp = f"{context['run_id']}-{nodes}"
        created = control.post(
            "/api/v1/workflows", headers=headers, json={"name": f"Capacity {stamp}", "visibility": "company"}
        )
        assert created.status_code == 201, f"{created.status_code}: {created.text}"
        workflow_id = created.json()["id"]
        draft = control.get(
            f"/api/v1/workflows/{workflow_id}/drafts/{fixture_draft_id(control, workflow_id, headers)}", headers=headers
        ).json()
        definition = draft["definition"]
        definition = set_chain_definition(definition, nodes)
        saved = control.put(
            f"/api/v1/workflows/{workflow_id}/drafts/{fixture_draft_id(control, workflow_id, headers)}",
            headers=headers,
            json={
                "editorDocument": fixture_draft(control, workflow_id, headers)["editorDocument"],
                "expectedRevision": draft["revision"],
                "definition": definition,
            },
        )
        assert saved.status_code == 200, saved.text
        latest = control.get(
            f"/api/v1/workflows/{workflow_id}/drafts/{fixture_draft_id(control, workflow_id, headers)}", headers=headers
        ).json()
        version = control.post(
            f"/api/v1/workflows/{workflow_id}/versions",
            headers=headers,
            json={"draftId": fixture_draft(control, workflow_id, headers)["id"], "draftRevision": latest["revision"]},
        )
        version.raise_for_status()
        workflow = {"workflowId": workflow_id, "versionId": version.json()["id"]}
        environment = _development_environment_id(control, headers)
        deploy = control.post(
            f"/api/v1/workflows/{workflow_id}/deployments",
            headers=headers,
            json={"environmentId": environment, "workflowVersionId": workflow["versionId"]},
        )
        deploy.raise_for_status()
        slug = f"capacity-{stamp}"
        application = control.post(
            "/api/v1/applications",
            headers=headers,
            json={"workflowId": workflow_id, "name": f"Capacity {stamp}", "slug": slug, "visibility": "company"},
        )
        application.raise_for_status()
        application_id = application.json()["id"]
        control.headers.update(headers)
        keys = create_caller_keys(control, application_id)
        _publish_application(control, headers, workflow, application_id, environment)
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            count = _runtime_mysql(
                context,
                f"SELECT COUNT(*) FROM api_key_admission WHERE application_id=UUID_TO_BIN('{application_id}') AND status='active';",
            )
            if int(count) == len(keys):
                return {
                    **workflow,
                    "applicationId": application_id,
                    "slug": slug,
                    "apiKey": keys[0],
                    "apiKeys": keys,
                    "token": token,
                    "tenant": me["companyId"],
                    "nodes": nodes,
                }
            time.sleep(0.5)
        raise TimeoutError("capacity API key admission did not reach Runtime")
