"""Version lifecycle desktop acceptance in a disposable Kubernetes installation."""

import os
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.next_batch_support import create_user
from tests.e2e.product.studio_builder_support import post
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_workflow_version_canvases(installed_agentx, service_urls):
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as control:
        _access_token(control)
    run_playwright(
        Path(installed_agentx["root"]),
        "workflow-version-canvases",
        ("tests/workflow-version-canvases.spec.ts",),
        {
            **os.environ,
            "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
            "AGENTX_E2E_STAGE": "helm-agentxctl",
            "AGENTX_E2E_BASE_URL": service_urls["web"],
            "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        },
    )


def test_workflow_version_mutations_require_permissions(installed_agentx, service_urls):
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as control:
        token, me = _access_token(control)
        control.headers.update({"Authorization": f"Bearer {token}"})
        role = post(
            control,
            "/roles",
            {
                "code": f"canvas_viewer_{installed_agentx['run_id']}",
                "name": "Canvas viewer",
                "dataScope": "company",
                "permissions": ["workflow:view"],
            },
        )
        user = create_user(control, f"canvas-viewer-{installed_agentx['run_id']}", me["departmentId"], role["id"])
        workflow = post(control, "/workflows", {"name": "Version permission boundary", "visibility": "private"})
        root = f"/workflows/{workflow['id']}"
        version = f"{root}/versions/{workflow['createdVersionId']}"
        document = post(control, version + "/edit", {})["document"]
        membership = control.post("/api/v1" + root + "/members", json={"userId": user["id"], "memberRole": "viewer"})
        assert membership.status_code == 204, membership.text
        environment = control.get("/api/v1/environments").json()[0]["id"]
        with httpx.Client(
            base_url=service_urls["web"],
            timeout=60,
            trust_env=False,
            headers={"Authorization": f"Bearer {user['token'].value}"},
        ) as viewer:
            assert viewer.get("/api/v1" + version).status_code == 200
            attempts = [
                ("POST", root + "/versions", {}),
                ("POST", version + "/edit", {}),
                (
                    "PUT",
                    root + f"/drafts/{document['id']}",
                    {
                        "expectedRevision": document["revision"],
                        "definition": document["definition"],
                        "editorDocument": document["editorDocument"],
                    },
                ),
                ("POST", version + "/restore", {"expectedRevision": document["revision"]}),
                ("PATCH", version, {"archived": True}),
                (
                    "POST",
                    root + "/deployments",
                    {
                        "environmentId": environment,
                        "workflowVersionId": workflow["createdVersionId"],
                        "contentSource": "version",
                        "expectedRevision": document["revision"],
                    },
                ),
            ]
            for method, path, body in attempts:
                response = viewer.request(method, "/api/v1" + path, json=body)
                assert response.status_code == 403, (method, path, response.text)
        assert control.get("/api/v1" + version).json()["publicationStatus"] == "unpublished"
        assert control.get("/api/v1" + root + "/deployments").json() == []
