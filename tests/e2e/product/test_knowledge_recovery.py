"""Scoped knowledge access, crash recovery and source-reference cleanup."""

# ruff: noqa: S608 -- UUIDs belong to the isolated E2E database
from __future__ import annotations

import time
import uuid

import httpx
import pytest

from tests.e2e.runtime.test_agent_attachments import _access_token, _control_mysql
from tests.e2e.support import run
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def created(client, path, body):
    response = client.post(path, json=body)
    assert response.status_code == 201, response.text
    return response.json()


def test_knowledge_department_scope_restart_and_cleanup(installed_agentx, service_urls, e2e_providers):
    context = installed_agentx
    stamp = uuid.uuid4().hex[:12]
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        department_a = created(
            control, "/api/v1/departments", {"parentId": me["departmentId"], "name": f"Knowledge A {stamp}"}
        )
        department_b = created(
            control, "/api/v1/departments", {"parentId": me["departmentId"], "name": f"Knowledge B {stamp}"}
        )
        role = created(
            control,
            "/api/v1/roles",
            {
                "code": f"knowledge_{stamp}",
                "name": f"Knowledge {stamp}",
                "dataScope": "department_tree",
                "permissions": ["knowledge:view", "knowledge:manage"],
            },
        )
        username = f"knowledge-{stamp}"
        created(
            control,
            "/api/v1/users",
            {"username": username, "displayName": username, "departmentId": department_b["id"], "roleId": role["id"]},
        )
        credential = created(
            control,
            "/api/v1/credentials",
            {
                "name": f"Knowledge recovery {stamp}",
                "credentialType": "bearer",
                "secret": "agentx-v2-04-rag-key",
                "ownerDepartmentId": department_a["id"],
            },
        )
        connection = created(
            control,
            "/api/v1/knowledge/connections",
            {
                "name": f"Knowledge recovery {stamp}",
                "provider": "lightrag",
                "endpoint": e2e_providers["lightrag"],
                "healthPath": "/health",
                "credentialId": credential["id"],
                "ownerDepartmentId": department_a["id"],
                "configuration": {},
            },
        )
        resource = created(
            control,
            "/api/v1/knowledge/resources",
            {
                "name": f"Knowledge recovery {stamp}",
                "connectionId": connection["id"],
                "externalResourceId": f"knowledge_{stamp}",
                "ownerDepartmentId": department_a["id"],
            },
        )
        path = f"/api/v1/knowledge/resources/{resource['id']}"
        login = control.post("/api/v1/auth/login", json={"username": username, "password": "123456"})
        assert login.status_code == 200, login.text
        changed = control.post(
            "/api/v1/auth/change-password",
            json={"token": login.json()["changePasswordToken"], "password": f"knowledge-password-{stamp}"},
        )
        assert changed.status_code == 200, changed.text
        outsider = {"Authorization": f"Bearer {changed.json()['accessToken']}"}
        denied = [
            control.get(f"{path}/documents", headers=outsider),
            control.post(f"{path}/retrieval-test", headers=outsider, json={"query": "scoped content", "topK": 5}),
            control.post(
                f"{path}/documents", headers=outsider, files={"file": ("forbidden.txt", b"forbidden", "text/plain")}
            ),
        ]
        assert all(response.status_code in (403, 404) for response in denied), [response.text for response in denied]
        namespace = context["dependencies_namespace"]
        run(("kubectl", "-n", namespace, "scale", "deployment/lightrag", "--replicas=0"), timeout=60)
        run(
            (
                "kubectl",
                "-n",
                namespace,
                "wait",
                "--for=delete",
                "pod",
                "-l",
                "app.kubernetes.io/name=lightrag",
                "--timeout=120s",
            ),
            timeout=150,
        )
        try:
            content = f"The release recovery secret is sapphire-orbit-{stamp}. This knowledge document survives an API restart."
            uploaded = control.post(
                f"{path}/documents", files={"file": (f"recovery-{stamp}.txt", content.encode(), "text/plain")}
            )
            assert uploaded.status_code == 202, uploaded.text
            document = uploaded.json()
            assert document["status"] in {"uploading", "indexing"}, document
            duplicate = control.post(f"{path}/documents", files={"file": ("same.txt", content.encode(), "text/plain")})
            assert duplicate.status_code == 422 and duplicate.json()["code"] == "KNOWLEDGE_DOCUMENT_DUPLICATED", (
                duplicate.text
            )
            busy = control.delete(f"{path}/documents/{document['id']}")
            assert busy.status_code == 409, busy.text
            run(
                ("kubectl", "-n", context["control_namespace"], "rollout", "restart", "deployment/platform-control"),
                timeout=60,
            )
            run(
                (
                    "kubectl",
                    "-n",
                    context["control_namespace"],
                    "rollout",
                    "status",
                    "deployment/platform-control",
                    "--timeout=300s",
                ),
                timeout=330,
            )
        finally:
            run(("kubectl", "-n", namespace, "scale", "deployment/lightrag", "--replicas=1"), timeout=60)
            run(("kubectl", "-n", namespace, "rollout", "status", "deployment/lightrag", "--timeout=600s"), timeout=630)
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            listing = control.get(f"{path}/documents")
            assert listing.status_code == 200, listing.text
            document = next(item for item in listing.json() if item["id"] == document["id"])
            assert document["status"] != "failed", document
            if document["status"] == "indexed":
                break
            time.sleep(2)
        assert document["status"] == "indexed" and document["externalDocumentId"], document
        hit = control.post(f"{path}/retrieval-test", json={"query": f"release recovery secret {stamp}", "topK": 5})
        assert hit.status_code == 200, hit.text
        assert any(f"sapphire-orbit-{stamp}" in item.get("content", "") for item in hit.json()["documents"]), hit.text
        reference_sql = f"SELECT COUNT(*) FROM artifact_references WHERE owner_type='knowledge_document' AND owner_id='{document['id']}';"
        assert _control_mysql(context, reference_sql) == "1"
        forbidden_delete = control.delete(f"{path}/documents/{document['id']}", headers=outsider)
        assert forbidden_delete.status_code in (403, 404), forbidden_delete.text
        deleted = control.delete(f"{path}/documents/{document['id']}")
        assert deleted.status_code == 204, deleted.text
        assert _control_mysql(context, reference_sql) == "0"
        assert control.get(f"{path}/documents").json() == []
        write_report(
            context,
            "product/knowledge-recovery.json",
            {
                "status": "passed",
                "departmentIsolation": True,
                "restartRecovered": True,
                "externalDocumentId": document["externalDocumentId"],
                "artifactReferenceReleased": True,
            },
        )
