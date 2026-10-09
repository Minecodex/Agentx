"""Second-batch grant lifecycle against the actual Control and Runtime."""

# ruff: noqa: S608 -- identifiers only come from the isolated API fixture.

from __future__ import annotations

import os
import time
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.live_text_support import post
from tests.e2e.product.next_batch_support import (
    approval_resources as approval_resources,
)
from tests.e2e.product.next_batch_support import (
    authenticated,
    create_user,
    get,
    model_definition,
    publish_application,
    request_resource,
    review,
)
from tests.e2e.product.test_live_knowledge_memory import _ask
from tests.e2e.product.test_live_text_acceptance import message, session, terminal
from tests.e2e.runtime.test_agent_attachments import _control_mysql, _runtime_mysql
from tests.e2e.support import run_playwright
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_resource_review_desktop_flow(installed_agentx, service_urls, approval_resources, run_id):
    resource = approval_resources
    with authenticated(service_urls["web"], resource["editor"]["token"]) as editor:
        name = f"P7 desktop approval {run_id}"
        workflow = _new_workflow(editor, name)
        approved = request_resource(editor, workflow, resource["model"])
        rejected_workflow = _new_workflow(editor, f"P7 desktop rejection {run_id}")
        rejected = request_resource(editor, rejected_workflow, resource["model"])
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": run_id,
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        "AGENTX_E2E_REVIEW_REQUEST_ID": approved["id"],
        "AGENTX_E2E_REVIEW_REJECT_ID": rejected["id"],
        "AGENTX_E2E_REVIEW_WORKFLOW_NAME": name,
        "AGENTX_E2E_REVIEW_MODEL_NAME": resource["model"]["alias"],
        "AGENTX_E2E_REVIEW_SECRET_NAME": resource["credential"]["name"],
        "AGENTX_E2E_REVIEW_MODEL_USER": resource["modelReviewer"]["username"],
        "AGENTX_E2E_REVIEW_MODEL_PASSWORD": resource["modelReviewer"]["password"].value,
        "AGENTX_E2E_REVIEW_SECRET_USER": resource["secretReviewer"]["username"],
        "AGENTX_E2E_REVIEW_SECRET_PASSWORD": resource["secretReviewer"]["password"].value,
    }
    run_playwright(
        Path(installed_agentx["root"]), "live-resource-review", ("tests/live-resource-review.spec.ts",), environment
    )
    assert get(resource["control"], f"/resource-grant-requests/{approved['id']}")["status"] == "approved"
    assert len(_grants(resource["control"], resource, workflow)) == 2
    assert get(resource["control"], f"/resource-grant-requests/{rejected['id']}")["status"] == "rejected"
    assert not _grants(resource["control"], resource, rejected_workflow)


def _new_workflow(editor, name):
    return post(editor, "/workflows", {"name": name, "visibility": "company"})["id"]


def _grants(control, resource, workflow):
    identity = get(control, f"/workflows/{workflow}")["serviceIdentityId"]
    return [
        item
        for kind, rid in (("model", resource["model"]["id"]), ("credential", resource["credential"]["id"]))
        for item in get(control, f"/resources/{kind}/{rid}/grants")
        if item["subjectId"] == identity
    ]


def test_cross_department_approval_rejection_cancellation_and_runtime_revocation(
    installed_agentx, service_urls, approval_resources, run_id
):
    resource = approval_resources
    control = resource["control"]
    with (
        authenticated(service_urls["web"], resource["editor"]["token"]) as editor,
        authenticated(service_urls["web"], resource["modelReviewer"]["token"]) as model_reviewer,
        authenticated(service_urls["web"], resource["secretReviewer"]["token"]) as secret_reviewer,
    ):
        workflow = _new_workflow(editor, f"P7 approved {run_id}")
        request = request_resource(editor, workflow, resource["model"])
        assert isinstance(request["createdAt"], str) and isinstance(request["updatedAt"], str)
        assert [item["action"] for item in request["history"]] == ["submitted"], request["history"]
        assert request["status"] == "pending" and len(request["reviews"]) == 2 and len(request["items"]) == 2, request
        assert request_resource(editor, workflow, resource["model"])["id"] == request["id"]
        assert not _grants(control, resource, workflow)
        for client, own_kind in ((model_reviewer, "model"), (secret_reviewer, "credential")):
            view = get(client, f"/resource-grant-requests/{request['id']}")
            assert all((item["name"] is not None) == (item["resourceType"] == own_kind) for item in view["items"]), view
        wrong = model_reviewer.post(
            f"/api/v1/resource-grant-requests/{request['id']}/reviews/{resource['secretDepartmentId']}/approve",
            json={"expectedVersion": 1, "comment": None},
        )
        assert wrong.status_code == 403, wrong.text
        first = review(model_reviewer, request, resource["modelDepartmentId"])
        assert first["status"] == "pending" and not _grants(control, resource, workflow)
        final = review(secret_reviewer, request, resource["secretDepartmentId"])
        assert final["status"] == "approved" and len(_grants(control, resource, workflow)) == 2
        assert [item["action"] for item in final["history"]] == [
            "submitted",
            "review_approved",
            "review_approved",
            "approved",
        ], final["history"]
        draft = get(editor, f"/workflows/{workflow}/draft")
        saved = editor.put(
            f"/api/v1/workflows/{workflow}/draft",
            json={
                "expectedRevision": draft["revision"],
                "definition": model_definition(control, resource["app"]["workflowId"], resource["model"]),
            },
        )
        assert saved.status_code == 200, saved.text
        version = post(
            editor,
            f"/workflows/{workflow}/versions",
            {"draftRevision": get(editor, f"/workflows/{workflow}/draft")["revision"]},
        )
        app = publish_application(control, workflow, version["id"], f"p7-approved-{run_id}")
        with httpx.Client(
            base_url=service_urls["runtime"],
            headers={"Authorization": f"Bearer {resource['app']['token'].value}"},
            timeout=180,
        ) as gateway:
            success, answer = _ask(gateway, app, "会签后真实执行", "请逐字只回复:水星蓝桥授权成功")
            assert "水星蓝桥授权成功" in answer, answer
            grant = next(item for item in _grants(control, resource, workflow) if item["resourceType"] == "credential")
            revoked = control.delete(
                f"/api/v1/resources/credential/{resource['credential']['id']}/grants/{grant['id']}"
            )
            assert revoked.status_code == 204, revoked.text
            invoked = message(gateway, session(gateway, app, "撤销后新执行"), "请回复授权测试")
            refusal = terminal(gateway, invoked)
            assert refusal["status"] == "failed", refusal
            calls = _runtime_mysql(
                installed_agentx,
                f"SELECT COUNT(*) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{refusal['executionId']}') AND status='succeeded';",
            )
            assert calls == "0", calls
        rejected_workflow = _new_workflow(editor, f"P7 rejected {run_id}")
        rejected = request_resource(editor, rejected_workflow, resource["model"])
        result = review(model_reviewer, rejected, resource["modelDepartmentId"], "reject")
        assert result["status"] == "rejected" and not _grants(control, resource, rejected_workflow)
        options = get(
            editor,
            f"/workflows/{rejected_workflow}/resource-options?resourceType=model&operation=use&search={resource['model']['alias']}",
        )["items"]
        assert next(item for item in options if item["id"] == resource["model"]["id"])["accessState"] == "rejected"
        renewed = request_resource(editor, rejected_workflow, resource["model"])
        assert renewed["id"] != rejected["id"] and renewed["status"] == "pending"
        cancelled = editor.post(
            f"/api/v1/resource-grant-requests/{renewed['id']}/cancel", json={"expectedVersion": renewed["version"]}
        )
        assert cancelled.status_code == 200 and cancelled.json()["status"] == "cancelled", cancelled.text
        after_cancel = model_reviewer.post(
            f"/api/v1/resource-grant-requests/{renewed['id']}/reviews/{resource['modelDepartmentId']}/approve",
            json={"expectedVersion": 1, "comment": None},
        )
        assert after_cancel.status_code == 409, after_cancel.text
    write_report(
        installed_agentx,
        "product/live-grant-lifecycle.json",
        {
            "status": "passed",
            "allDepartmentsRequired": True,
            "departmentRedaction": True,
            "wrongReviewerDenied": True,
            "rejectionCreatesNoGrant": True,
            "cancelledRequestCannotBeApproved": True,
            "approvedExecutionId": success["executionId"],
            "revokedExecutionId": refusal["executionId"],
            "revokedExecutionSuccessfulCalls": 0,
        },
    )


def test_pending_resource_request_expires_when_resource_becomes_unavailable(
    installed_agentx, service_urls, approval_resources, run_id
):
    resource = approval_resources
    control = resource["control"]
    with authenticated(service_urls["web"], resource["editor"]["token"]) as editor:
        workflow = _new_workflow(editor, f"P7 stale dependency {run_id}")
        request = request_resource(editor, workflow, resource["model"])
        credential = get(control, f"/credentials/{resource['credential']['id']}")
        disabled = control.patch(
            f"/api/v1/credentials/{credential['id']}",
            json={"name": credential["name"], "status": "disabled", "version": credential["version"]},
        )
        assert disabled.status_code == 200, disabled.text
        try:
            first = control.post(
                f"/api/v1/resource-grant-requests/{request['id']}/reviews/{resource['modelDepartmentId']}/approve",
                json={"expectedVersion": 1, "comment": None},
            )
            assert first.status_code == 409, first.text
            assert first.json()["code"] == "RESOURCE_GRANT_REQUEST_STALE", first.text
            response = control.post(
                f"/api/v1/resource-grant-requests/{request['id']}/reviews/{resource['secretDepartmentId']}/approve",
                json={"expectedVersion": 1, "comment": None},
            )
            latest = get(control, f"/resource-grant-requests/{request['id']}")
            assert latest["status"] == "stale", (
                first.status_code,
                response.status_code,
                latest,
            )
            assert any(item["action"] == "stale" for item in latest["history"]), latest["history"]
        finally:
            current = get(control, f"/credentials/{credential['id']}")
            restored = control.patch(
                f"/api/v1/credentials/{credential['id']}",
                json={"name": credential["name"], "status": "active", "version": current["version"]},
            )
            assert restored.status_code == 200, restored.text
        assert not _grants(control, resource, workflow)
    write_report(
        installed_agentx,
        "product/live-grant-stale.json",
        {
            "status": "passed",
            "inactiveDependencyInvalidatesRequest": True,
            "noGrantCreated": True,
            "requestId": request["id"],
        },
    )


def test_already_issued_token_is_refused_after_role_revocation(
    installed_agentx, service_urls, live_application, run_id
):
    app = live_application
    with authenticated(service_urls["web"], app["token"]) as control:
        role = post(
            control,
            "/roles",
            {
                "code": f"p7_old_token_{run_id}",
                "name": f"P7 old token {run_id}",
                "dataScope": "company",
                "permissions": ["application:view", "application:invoke"],
            },
        )
        user = create_user(control, f"p7-old-token-{run_id}", app["departmentId"], role["id"])
        token_version = int(
            _control_mysql(installed_agentx, f"SELECT token_version FROM users WHERE id=UUID_TO_BIN('{user['id']}');")
        )
        deadline = time.monotonic() + 120
        while (
            _runtime_mysql(
                installed_agentx,
                f"SELECT COUNT(*) FROM runtime_user_application_grants g JOIN runtime_user_admission u USING(tenant_id,user_id) WHERE g.user_id=UUID_TO_BIN('{user['id']}') AND g.application_id=UUID_TO_BIN('{app['applicationId']}') AND g.status='active' AND g.can_invoke=TRUE AND u.status='active' AND u.token_version={token_version};",
            )
            != "1"
        ):
            assert time.monotonic() < deadline
            time.sleep(0.5)
        with httpx.Client(
            base_url=service_urls["runtime"], headers={"Authorization": f"Bearer {user['token'].value}"}, timeout=60
        ) as gateway:
            sid = session(gateway, app, "旧 Token 撤销前")
            changed = control.patch(
                f"/api/v1/roles/{role['id']}",
                json={
                    "name": role["name"],
                    "description": None,
                    "dataScope": "company",
                    "version": role["version"],
                    "permissions": ["application:view"],
                },
            )
            assert changed.status_code == 200, changed.text
            deadline = time.monotonic() + 120
            while (
                _runtime_mysql(
                    installed_agentx,
                    f"SELECT COUNT(*) FROM runtime_user_application_grants WHERE user_id=UUID_TO_BIN('{user['id']}') AND application_id=UUID_TO_BIN('{app['applicationId']}') AND can_invoke=FALSE;",
                )
                != "1"
            ):
                assert time.monotonic() < deadline
                time.sleep(0.5)
            creation = gateway.post(
                f"/gateway/v1/applications/{app['applicationSlug']}/sessions",
                json={"title": "旧 Token 应失效", "externalUserId": None},
                headers={"Idempotency-Key": f"old-token-{time.time_ns()}"},
            )
            existing = gateway.post(
                f"/gateway/v1/sessions/{sid}/messages",
                json={"parts": [{"partType": "text", "content": "不应调用模型"}]},
                headers={"Idempotency-Key": f"old-message-{time.time_ns()}"},
            )
            assert creation.status_code == existing.status_code == 401, (creation.status_code, existing.status_code)
        with authenticated(service_urls["web"], user["token"]) as stale:
            me = stale.get("/api/v1/auth/me")
            assert me.status_code == 401, me.text
    write_report(
        installed_agentx,
        "product/live-old-token-revocation.json",
        {
            "status": "passed",
            "oldTokenControlStatus": me.status_code,
            "oldTokenNewSessionStatus": creation.status_code,
            "oldTokenExistingSessionMessageStatus": existing.status_code,
        },
    )


@pytest.mark.parametrize("change", ["credential_rotation", "requester_role_revocation"])
def test_changed_dependency_or_requester_invalidates_pending_approval(
    installed_agentx, service_urls, approval_resources, live_kimi_secret, run_id, change
):
    resource = approval_resources
    control = resource["control"]
    role = post(
        control,
        "/roles",
        {
            "code": f"p7_stale_{change}_{run_id}",
            "name": f"P7 stale {change} {run_id}",
            "dataScope": "company",
            "permissions": ["workflow:view", "workflow:create", "workflow:edit", "model:view", "credential:view"],
        },
    )
    user = create_user(control, f"p7-stale-{change}-{run_id}", resource["app"]["departmentId"], role["id"])
    with authenticated(service_urls["web"], user["token"]) as editor:
        workflow = _new_workflow(editor, f"P7 stale {change} {run_id}")
        request = request_resource(editor, workflow, resource["model"])
    if change == "credential_rotation":
        credential = get(control, f"/credentials/{resource['credential']['id']}")
        response = control.post(
            f"/api/v1/credentials/{credential['id']}/rotate",
            json={"version": credential["version"], "secret": live_kimi_secret.value},
        )
    else:
        response = control.patch(
            f"/api/v1/roles/{role['id']}",
            json={
                "name": role["name"],
                "description": None,
                "dataScope": "company",
                "version": role["version"],
                "permissions": ["workflow:view", "model:view", "credential:view"],
            },
        )
    assert response.status_code == 200, response.text
    decision = control.post(
        f"/api/v1/resource-grant-requests/{request['id']}/reviews/{resource['modelDepartmentId']}/approve",
        json={"expectedVersion": 1, "comment": None},
    )
    assert decision.status_code == 409 and decision.json()["code"] == "RESOURCE_GRANT_REQUEST_STALE", decision.text
    latest = get(control, f"/resource-grant-requests/{request['id']}")
    assert latest["status"] == "stale" and not _grants(control, resource, workflow), latest
    write_report(
        installed_agentx,
        f"product/live-grant-stale-{change}.json",
        {"status": "passed", "change": change, "noGrantCreated": True, "requestId": request["id"]},
    )
