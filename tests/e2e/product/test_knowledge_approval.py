"""Query and credential approval by separate departments, then real execution."""

import os
from pathlib import Path

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.knowledge_native_support import knowledge_definition
from tests.e2e.product.next_batch_support import authenticated, create_user, get, review
from tests.e2e.product.remaining_support import invoke, start_execution
from tests.e2e.product.studio_builder_support import post
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run_playwright
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def query_request(editor, workflow, query):
    draft = fixture_draft(editor, workflow)
    return post(
        editor,
        f"/workflows/{workflow}/resource-grant-requests",
        {
            "resourceType": "rag",
            "resourceId": query["id"],
            "operation": "read",
            "sourceNodeId": "retrieve",
            "sourceDraftId": draft["id"],
            "sourceRevision": draft["revision"],
            "message": "Approve project knowledge and its credential dependency",
        },
    )


def workflow_grants(control, workflow, query, credential):
    identity = get(control, f"/workflows/{workflow}")["serviceIdentityId"]
    return [
        grant
        for kind, resource in (("rag", query), ("credential", credential))
        for grant in get(control, f"/resources/{kind}/{resource['id']}/grants")
        if grant["subjectId"] == identity
    ]


def draft_with_query(editor, name, query):
    workflow = post(editor, "/workflows", {"name": name, "visibility": "company"})["id"]
    draft = fixture_draft(editor, workflow)
    response = editor.put(
        f"/api/v1/workflows/{workflow}/drafts/{draft['id']}",
        json={
            "expectedRevision": draft["revision"],
            "editorDocument": draft["editorDocument"],
            "definition": knowledge_definition(editor, workflow, [query], "launch code"),
        },
    )
    assert response.status_code == 200, response.text
    return workflow


def test_knowledge_query_cross_department_approval_and_rejection(
    installed_agentx, service_urls, studio_provider, run_id
):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        departments = {
            kind: post(
                control,
                "/departments",
                {
                    "parentId": me["departmentId"],
                    "name": f"Knowledge {kind} {run_id}",
                },
            )["id"]
            for kind in ("query", "credential")
        }
        credential = post(
            control,
            "/credentials",
            {
                "name": f"Knowledge approval credential {run_id}",
                "credentialType": "bearer",
                "secret": "m5-model-secret",
                "ownerDepartmentId": departments["credential"],
            },
        )
        connection = post(
            control,
            "/knowledge/connections",
            {
                "name": f"Knowledge approval connection {run_id}",
                "provider": "ragflow",
                "endpoint": studio_provider.removesuffix("/v1"),
                "credentialId": credential["id"],
                "ownerDepartmentId": departments["query"],
            },
        )
        query = post(
            control,
            f"/knowledge/connections/{connection['id']}/queries",
            {
                "name": "Department project search",
                "description": "Launch project documentation",
                "scope": {"kind": "ragflow", "datasetIds": ["dataset-a", "dataset-b"]},
                "ownerDepartmentId": departments["query"],
            },
        )
        role = post(
            control,
            "/roles",
            {
                "code": f"knowledge_editor_{run_id.replace('-', '_')}",
                "name": f"Knowledge editor {run_id}",
                "dataScope": "company",
                "permissions": [
                    "workflow:view",
                    "workflow:create",
                    "workflow:edit",
                    "workflow:publish",
                    "execution:view",
                    "execution:run",
                    "trace:view",
                    "knowledge:view",
                    "credential:view",
                    "approval:view",
                    "notification:view",
                ],
            },
        )
        reviewer_role = next(
            item["id"] for item in get(control, "/roles?pageSize=100")["items"] if item["code"] == "department_admin"
        )
        editor_user = create_user(control, f"knowledge-editor-{run_id}", me["departmentId"], role["id"])
        reviewers = {
            kind: create_user(control, f"knowledge-{kind}-{run_id}", department, reviewer_role)
            for kind, department in departments.items()
        }
        with (
            authenticated(service_urls["web"], editor_user["token"]) as editor,
            authenticated(service_urls["web"], reviewers["query"]["token"]) as query_reviewer,
            authenticated(service_urls["web"], reviewers["credential"]["token"]) as secret_reviewer,
        ):
            workflow = draft_with_query(editor, f"Knowledge approved {run_id}", query)
            denied = start_execution({"control": editor}, workflow)
            assert denied.status_code in {403, 422}, denied.text
            request = query_request(editor, workflow, query)
            assert request["status"] == "pending" and len(request["reviews"]) == 2, request
            assert {item["resourceType"] for item in request["items"]} == {"rag", "credential"}, request
            assert query_request(editor, workflow, query)["id"] == request["id"]
            for reviewer, kind in ((query_reviewer, "rag"), (secret_reviewer, "credential")):
                visible = get(reviewer, f"/resource-grant-requests/{request['id']}")
                assert all((item["name"] is not None) == (item["resourceType"] == kind) for item in visible["items"]), (
                    visible
                )
            wrong = query_reviewer.post(
                f"/api/v1/resource-grant-requests/{request['id']}/reviews/{departments['credential']}/approve",
                json={"expectedVersion": 1, "comment": None},
            )
            assert wrong.status_code == 403, wrong.text
            first = review(query_reviewer, request, departments["query"])
            assert first["status"] == "pending" and not workflow_grants(control, workflow, query, credential)
            still_denied = start_execution({"control": editor}, workflow)
            assert still_denied.status_code in {403, 422}, still_denied.text
            approved = review(secret_reviewer, request, departments["credential"])
            assert approved["status"] == "approved" and len(workflow_grants(control, workflow, query, credential)) == 2
            execution, result = invoke({"control": editor}, workflow)
            assert "sapphire-orbit" in result["output"]["text"] and len(result["output"]["sources"]) == 2, result

            rejected_workflow = draft_with_query(editor, f"Knowledge rejected {run_id}", query)
            rejection = query_request(editor, rejected_workflow, query)
            rejected = review(query_reviewer, rejection, departments["query"], "reject")
            assert rejected["status"] == "rejected" and not workflow_grants(
                control, rejected_workflow, query, credential
            )
            refused = start_execution({"control": editor}, rejected_workflow)
            assert refused.status_code in {403, 422}, refused.text
            renewed = query_request(editor, rejected_workflow, query)
            assert renewed["id"] != rejection["id"] and renewed["status"] == "pending"
            cancelled = editor.post(
                f"/api/v1/resource-grant-requests/{renewed['id']}/cancel", json={"expectedVersion": renewed["version"]}
            )
            assert cancelled.status_code == 200 and cancelled.json()["status"] == "cancelled", cancelled.text

            desktop_name = f"Knowledge desktop approval {run_id}"
            desktop_workflow = draft_with_query(editor, desktop_name, query)
            desktop_request = query_request(editor, desktop_workflow, query)
            desktop_rejected_workflow = draft_with_query(editor, f"Knowledge desktop rejection {run_id}", query)
            desktop_rejection = query_request(editor, desktop_rejected_workflow, query)
            run_playwright(
                Path(installed_agentx["root"]),
                "knowledge-query-review",
                ("tests/live-resource-review.spec.ts",),
                {
                    **os.environ,
                    "AGENTX_E2E_RUN_ID": run_id,
                    "AGENTX_E2E_STAGE": "helm-agentxctl",
                    "AGENTX_E2E_BASE_URL": service_urls["web"],
                    "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
                    "AGENTX_E2E_REVIEW_REQUEST_ID": desktop_request["id"],
                    "AGENTX_E2E_REVIEW_REJECT_ID": desktop_rejection["id"],
                    "AGENTX_E2E_REVIEW_WORKFLOW_NAME": desktop_name,
                    "AGENTX_E2E_REVIEW_MODEL_NAME": query["name"],
                    "AGENTX_E2E_REVIEW_SECRET_NAME": credential["name"],
                    "AGENTX_E2E_REVIEW_MODEL_USER": reviewers["query"]["username"],
                    "AGENTX_E2E_REVIEW_MODEL_PASSWORD": reviewers["query"]["password"].value,
                    "AGENTX_E2E_REVIEW_SECRET_USER": reviewers["credential"]["username"],
                    "AGENTX_E2E_REVIEW_SECRET_PASSWORD": reviewers["credential"]["password"].value,
                },
            )
            assert get(control, f"/resource-grant-requests/{desktop_request['id']}")["status"] == "approved"
            assert len(workflow_grants(control, desktop_workflow, query, credential)) == 2
            desktop_execution, desktop_result = invoke({"control": editor}, desktop_workflow)
            assert "sapphire-orbit" in desktop_result["output"]["text"], desktop_result
            assert get(control, f"/resource-grant-requests/{desktop_rejection['id']}")["status"] == "rejected"
            assert not workflow_grants(control, desktop_rejected_workflow, query, credential)

            grant = next(
                item
                for item in workflow_grants(control, workflow, query, credential)
                if item["resourceType"] == "credential"
            )
            revoked = control.delete(f"/api/v1/resources/credential/{credential['id']}/grants/{grant['id']}")
            assert revoked.status_code == 204, revoked.text
            after_revoke = start_execution({"control": editor}, workflow)
            assert after_revoke.status_code in {403, 422}, after_revoke.text
        write_report(
            installed_agentx,
            "product/knowledge-query-approval.json",
            {
                "status": "passed",
                "approvedRequestId": request["id"],
                "approvedExecutionId": execution,
                "desktopApprovalExecutionId": desktop_execution,
                "rejectedRequestId": rejection["id"],
                "cancelledRequestId": renewed["id"],
                "twoDepartmentReviewsRequired": True,
                "credentialDependencyGranted": True,
                "reviewerIsolation": True,
                "pendingAndRejectedExecutionDenied": True,
                "credentialRevocationStatus": after_revoke.status_code,
            },
        )
