"""Authenticated users and real model resources for the second local batch."""

from __future__ import annotations

import copy
import time
from contextlib import contextmanager

import httpx
import pytest

from tests.e2e.product.live_text_support import Secret, post
from tests.e2e.product.test_provider_integration import _publish_application_deployment


def get(client, path):
    response = client.get(f"/api/v1{path}")
    response.raise_for_status()
    return response.json()


@contextmanager
def authenticated(base_url, token):
    with httpx.Client(base_url=base_url, headers={"Authorization": f"Bearer {token.value}"}, timeout=60) as client:
        yield client


def create_user(control, username, department, role):
    user = post(
        control, "/users", {"username": username, "displayName": username, "departmentId": department, "roleId": role}
    )
    password = f"P7-next-private-{username}"
    login = control.post("/api/v1/auth/login", json={"username": username, "password": "123456"})
    assert login.status_code == 200, login.text
    changed = control.post(
        "/api/v1/auth/change-password", json={"token": login.json()["changePasswordToken"], "password": password}
    )
    assert changed.status_code == 200, changed.text
    return {
        "id": user["id"],
        "username": username,
        "password": Secret(password),
        "token": Secret(changed.json()["accessToken"]),
    }


def request_resource(client, workflow, model):
    draft = get(client, f"/workflows/{workflow}/draft")
    return post(
        client,
        f"/workflows/{workflow}/resource-grant-requests",
        {
            "resourceType": "model",
            "resourceId": model["id"],
            "resourceVersionId": model["deploymentId"],
            "operation": "use",
            "sourceNodeId": None,
            "sourceRevision": draft["revision"],
            "message": "P7 next: cross-department model and credential package",
        },
    )


def review(client, request, department, action="approve"):
    own = next(item for item in request["reviews"] if item["ownerDepartmentId"] == department)
    response = client.post(
        f"/api/v1/resource-grant-requests/{request['id']}/reviews/{department}/{action}",
        headers={"Idempotency-Key": f"p7-review-{time.time_ns()}"},
        json={"expectedVersion": own["version"], "comment": "P7 real approval acceptance"},
    )
    assert response.status_code == 200, response.text
    return response.json()


def publish_application(control, workflow, version, slug):
    environment = next(item for item in get(control, "/environments") if item["code"] == "development")
    post(
        control,
        f"/workflows/{workflow}/deployments",
        {"environmentId": environment["id"], "workflowVersionId": version},
    )
    app = post(control, "/applications", {"workflowId": workflow, "name": slug, "slug": slug, "visibility": "company"})
    deployment = _publish_application_deployment(control, dict(control.headers), app["id"], version, environment["id"])
    path = f"/applications/{app['id']}/deployments/{deployment['id']}/playground-config"
    current = get(control, path)
    response = control.put(
        f"/api/v1{path}",
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
    assert response.status_code in (200, 202), response.text
    deadline = time.monotonic() + 120
    while get(control, path)["publishStatus"] != "active":
        assert time.monotonic() < deadline
        time.sleep(0.5)
    return {"applicationId": app["id"], "applicationSlug": app["slug"], "workflowId": workflow}


@pytest.fixture(scope="module")
def approval_resources(live_application, live_kimi_secret, service_urls, run_id):
    app = live_application
    with authenticated(service_urls["web"], app["token"]) as control:
        me = get(control, "/auth/me")
        model_department = post(
            control, "/departments", {"parentId": me["departmentId"], "name": f"P7 model review {run_id}"}
        )
        secret_department = post(
            control, "/departments", {"parentId": me["departmentId"], "name": f"P7 credential review {run_id}"}
        )
        credential = post(
            control,
            "/credentials",
            {
                "name": f"P7 approval Kimi {run_id}",
                "credentialType": "bearer",
                "secret": live_kimi_secret.value,
                "ownerDepartmentId": secret_department["id"],
            },
        )
        model = post(
            control,
            "/models/aliases",
            {
                "connectionName": f"P7 approval model {run_id}",
                "providerType": "openai_compatible",
                "endpoint": "https://api.kimi.com/coding/v1",
                "credentialId": credential["id"],
                "ownerDepartmentId": model_department["id"],
                "alias": f"p7-approval-{run_id}",
                "modelName": "k3",
                "price": {"currency": "USD", "inputPerMillion": "0", "outputPerMillion": "0"},
            },
        )
        role = post(
            control,
            "/roles",
            {
                "code": f"p7_editor_{run_id}",
                "name": f"P7 editor {run_id}",
                "dataScope": "company",
                "permissions": [
                    "workflow:view",
                    "workflow:create",
                    "workflow:edit",
                    "workflow:publish",
                    "execution:view",
                    "execution:run",
                    "trace:view",
                    "model:view",
                    "credential:view",
                    "approval:view",
                    "notification:view",
                ],
            },
        )
        roles = get(control, "/roles?pageSize=100")["items"]
        reviewer_role = next(item for item in roles if item["code"] == "department_admin")
        editor = create_user(control, f"p7-editor-{run_id}", me["departmentId"], role["id"])
        model_reviewer = create_user(control, f"p7-model-{run_id}", model_department["id"], reviewer_role["id"])
        secret_reviewer = create_user(control, f"p7-secret-{run_id}", secret_department["id"], reviewer_role["id"])
        yield {
            "control": control,
            "model": model,
            "credential": credential,
            "editor": editor,
            "modelReviewer": model_reviewer,
            "secretReviewer": secret_reviewer,
            "modelDepartmentId": model_department["id"],
            "secretDepartmentId": secret_department["id"],
            "app": app,
        }


def model_definition(control, source_workflow, model):
    definition = copy.deepcopy(get(control, f"/workflows/{source_workflow}/draft")["definition"])
    node = next(node for node in definition["nodes"] if node["type"] == "model")
    node["parameters"]["prompt"] = {
        "kind": "template",
        "segments": [{"kind": "text", "text": "准确按用户要求回答,保持简短。"}],
    }
    node["resourceReferences"] = [
        {
            "bindingRole": "model",
            "resourceType": "model",
            "resourceId": model["id"],
            "resourceVersionId": model["deploymentId"],
            "operation": "use",
        }
    ]
    return definition
