"""Application user declarations, frozen sessions and connection-only Memory CRUD."""

from __future__ import annotations

import os
import time
from pathlib import Path

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.live_text_support import post
from tests.e2e.product.test_channel_delivery import _passthrough_workflow
from tests.e2e.product.test_live_memory_pipeline import _save_version
from tests.e2e.product.test_live_text_acceptance import message, terminal
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.runtime.test_agent_attachments import _access_token, _wait_admission_outbox
from tests.e2e.support import run_playwright
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_application_users_and_session_inputs(installed_agentx, service_urls, run_id):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        original = _passthrough_workflow(control, dict(control.headers), f"Application users {run_id}")
        workflow = original["workflowId"]
        definition = fixture_draft(control, workflow, None)["definition"]
        definition["start"]["inputs"]["properties"].update(
            {
                "customerId": {"type": "string"},
                "otherUser": {"type": "string"},
                "region": {"type": "string"},
            }
        )
        definition["start"]["inputs"]["required"].append("region")
        version = _save_version(control, workflow, definition)
        environment = next(item for item in control.get("/api/v1/environments").json() if item["code"] == "development")
        post(
            control,
            f"/workflows/{workflow}/deployments",
            {"contentSource": "version", "workflowVersionId": version["id"], "environmentId": environment["id"]},
        )
        app = post(
            control,
            "/applications",
            {
                "workflowId": workflow,
                "name": f"User mapping {run_id}",
                "slug": f"users-{run_id}",
                "visibility": "company",
            },
        )
        deployed = _publish_application_deployment(
            control, dict(control.headers), app["id"], version["id"], environment["id"], "customerId"
        )
        assert deployed["apiUserIdInput"] == "customerId"
        config_path = f"/api/v1/applications/{app['id']}/deployments/{deployed['id']}/playground-config"
        config = control.get(config_path).json()
        mapped = control.put(
            config_path,
            json={
                "expectedVersion": config["version"],
                "mapping": {
                    "questionInput": "message",
                    "fileInput": None,
                    "answerOutput": "answer",
                    "answerFilesOutput": None,
                },
            },
        )
        assert mapped.status_code == 202, mapped.text
        deadline = time.monotonic() + 120
        while control.get(config_path).json()["publishStatus"] != "active":
            assert time.monotonic() < deadline
            time.sleep(0.5)
        keys = [
            post(control, f"/applications/{app['id']}/api-keys", {"name": f"caller-{index}"})["secret"]
            for index in range(2)
        ]
        _wait_admission_outbox(installed_agentx, workflow_id=workflow)
        user = "customer with no AgentX account / 用户42"
        evidence = []
        with httpx.Client(
            base_url=service_urls["runtime"], headers={"Authorization": f"Bearer {keys[0]}"}, timeout=60
        ) as gateway:

            def session(input_value):
                response = gateway.post(
                    f"/gateway/v1/applications/{app['slug']}/sessions",
                    json={"input": input_value},
                    headers={"Idempotency-Key": f"session-{time.time_ns()}"},
                )
                assert response.status_code == 201, response.text
                return response.json()

            malformed = gateway.post(
                f"/gateway/v1/applications/{app['slug']}/sessions",
                json={"input": []},
                headers={"Idempotency-Key": f"malformed-{time.time_ns()}"},
            )
            assert malformed.status_code == 400 and malformed.json()["code"] == "INPUT_SCHEMA_VALIDATION_FAILED", (
                malformed.text
            )
            first = session({"customerId": user, "region": "Shanghai"})
            assert first["externalUserId"] == user and first["endUser"]["source"] == "api_parameter"
            result = terminal(gateway, message(gateway, first["id"], "first message"))
            assert result["status"] == "completed" and result["endUser"] == first["endUser"], result
            detail = control.get(f"/api/v1/executions/{result['executionId']}").json()
            assert detail["input"]["region"] == "Shanghai", detail
            assert detail["endUser"]["externalUserId"] == user and detail.get("initiatorUserId") is None, detail
            evidence.append(result["id"])
            gateway.headers["Authorization"] = f"Bearer {keys[1]}"
            same = session({"customerId": user, "region": "Beijing"})
            assert same["endUser"] == first["endUser"], same
            missing = session({"region": "Hangzhou"})
            assert missing["endUser"] is None
            no_user = terminal(gateway, message(gateway, missing["id"], "no personal user"))
            assert (
                no_user["status"] == "completed" and no_user["userIdentityDiagnostic"] == "MEMORY_USER_ID_UNAVAILABLE"
            ), no_user
            platform = gateway.post(
                f"/gateway/v1/applications/{app['slug']}/sessions",
                json={"input": {"customerId": user, "region": "Shanghai"}},
                headers={"Authorization": f"Bearer {token}", "Idempotency-Key": f"platform-{time.time_ns()}"},
            ).json()
            assert platform["endUser"]["source"] == "platform" and platform["externalUserId"] == me["id"], platform
            bad = gateway.post(
                f"/gateway/v1/applications/{app['slug']}/invocations",
                json={"input": {"message": "x", "region": "x", "customerId": 42}},
                headers={"Idempotency-Key": f"invalid-{time.time_ns()}"},
            )
            assert bad.status_code == 400 and bad.json()["code"] == "INPUT_SCHEMA_VALIDATION_FAILED", bad.text
            _publish_application_deployment(
                control, dict(control.headers), app["id"], version["id"], environment["id"], "otherUser"
            )
            frozen = gateway.post(
                f"/gateway/v1/applications/{app['slug']}/invocations",
                json={
                    "sessionId": first["id"],
                    "input": {"message": "frozen", "region": "Shanghai", "otherUser": "cannot-switch-session-user"},
                },
                headers={"Idempotency-Key": f"frozen-{time.time_ns()}"},
            )
            assert frozen.status_code in (200, 202), frozen.text
            assert frozen.json()["endUser"] == first["endUser"], frozen.text
            changed = session({"otherUser": "new-user", "region": "Shanghai"})
            assert changed["externalUserId"] == "new-user", changed
            _publish_application_deployment(
                control, dict(control.headers), app["id"], version["id"], environment["id"], None
            )
            unset = session({"customerId": user, "region": "Shanghai"})
            assert unset["endUser"] is None, unset
        filtered = control.get(
            f"/api/v1/applications/{app['id']}/sessions", params={"externalUserId": user, "userSource": "api_parameter"}
        )
        assert filtered.status_code == 200, filtered.text
        assert {item["id"] for item in filtered.json()} == {first["id"], same["id"]}, filtered.text
        records = control.get(
            "/api/v1/executions",
            params={"applicationIds": app["id"], "externalUserId": user, "userSource": "api_parameter"},
        )
        assert records.status_code == 200 and records.json()["items"], records.text
        assert all(item["endUser"]["externalUserId"] == user for item in records.json()["items"]), records.text
        invocations = control.get(
            f"/api/v1/applications/{app['id']}/invocations",
            params={"externalUserId": user, "userSource": "api_parameter"},
        )
        assert invocations.status_code == 200 and invocations.json()["items"], invocations.text
        assert all(item["endUser"]["externalUserId"] == user for item in invocations.json()["items"]), invocations.text
        run_playwright(
            Path(installed_agentx["root"]),
            "application-user-mapping",
            ["application-user-mapping.spec.ts"],
            {
                **os.environ,
                "AGENTX_E2E_BASE_URL": service_urls["web"],
                "AGENTX_E2E_RUN_ID": run_id,
                "AGENTX_USER_MAPPING_APP": app["id"],
                "AGENTX_USER_MAPPING_USER": user,
                "AGENTX_USER_MAPPING_VERSION": f"v{version['versionNumber']}",
                "AGENTX_USER_MAPPING_ENVIRONMENT": environment["name"],
            },
        )
        write_report(
            installed_agentx,
            "product/application-users.json",
            {
                "status": "passed",
                "applicationId": app["id"],
                "invocations": evidence,
                "sessionInputInherited": True,
                "identityFrozen": True,
                "apiKeyIndependent": True,
                "rawDeclarationAccepted": True,
            },
        )


def test_memory_connections_are_the_only_memory_resource(service_urls, run_id):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        resource = post(
            control,
            "/memory/connections",
            {
                "name": f"Mem0 CRUD {run_id}",
                "endpoint": "https://mem0.example.test",
                "healthPath": "/health",
                "ownerDepartmentId": me["departmentId"],
                "configuration": {},
            },
        )
        assert not {"externalNamespace", "accessMode", "connectionId"}.intersection(resource)
        path = f"/api/v1/memory/connections/{resource['id']}"
        assert control.get(path).json()["id"] == resource["id"]
        updated = control.patch(
            path,
            json={
                "name": "Updated Mem0",
                "endpoint": resource["endpoint"],
                "healthPath": "/openapi.json",
                "credentialId": None,
                "ownerDepartmentId": me["departmentId"],
                "status": "active",
                "version": resource["version"],
            },
        )
        assert updated.status_code == 200 and updated.json()["version"] == resource["version"] + 1, updated.text
        grant = {
            "subjectType": "department",
            "subjectId": me["departmentId"],
            "resourceVersionId": None,
            "operation": "read",
        }
        rejected = control.post(f"/api/v1/resources/memory/{resource['id']}/grants", json=grant)
        assert rejected.status_code == 400 and rejected.json()["code"] == "INVALID_MEMORY_OPERATION", rejected.text
        grant["operation"] = "use"
        assert control.post(f"/api/v1/resources/memory/{resource['id']}/grants", json=grant).status_code == 201
        deleted = control.delete(path, params={"expectedVersion": updated.json()["version"]})
        assert deleted.status_code == 204, deleted.text
        assert control.get(path).status_code == 404
