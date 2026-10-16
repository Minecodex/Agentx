# ruff: noqa: S608
# This Kubernetes E2E module queries only its isolated test database with API and fixture identifiers.

from __future__ import annotations

import json
import time
from typing import Any

import httpx
import pytest

from tests.e2e.runtime.test_agent_attachments import (
    WORKFLOW_ID,
    _access_token,
    _run_fixture_job,
    _runtime_mysql,
    _wait_admission_outbox,
    _wait_execution,
)


def _run_agent(client: httpx.Client, headers: dict[str, str], suffix: str) -> dict[str, Any]:
    response = client.post(
        f"/api/v1/workflows/{WORKFLOW_ID}/run",
        headers=headers,
        json={
            "input": {"message": f"p3-05-session-{suffix}"},
            "idempotencyKey": f"p3-05-session-{suffix}",
        },
    )
    assert response.status_code == 202, response.text
    return _wait_execution(client, headers, response.json()["executionId"])


def _wait_application_deployment(
    client: httpx.Client,
    headers: dict[str, str],
    application_id: str,
    deployment_id: str,
) -> dict[str, Any]:
    deadline = time.monotonic() + 300
    latest: dict[str, Any] = {}
    while time.monotonic() < deadline:
        response = client.get(f"/api/v1/applications/{application_id}/deployments", headers=headers)
        response.raise_for_status()
        latest = next(
            (item for item in response.json() if item["id"] == deployment_id),
            {},
        )
        if latest.get("status") == "active":
            return latest
        if latest.get("status") == "rejected":
            raise AssertionError(f"application deployment rejected: {json.dumps(latest, ensure_ascii=False)}")
        time.sleep(1)
    raise AssertionError(f"application deployment did not become active: {json.dumps(latest, ensure_ascii=False)}")


def _publish_application_deployment(
    client: httpx.Client,
    headers: dict[str, str],
    application_id: str,
    workflow_version_id: str,
    environment_id: str,
) -> dict[str, Any]:
    payload = {
        "workflowVersionId": workflow_version_id,
        "environmentId": environment_id,
        "sessionVersionPolicy": "pinned",
    }
    last_error = ""
    for _ in range(4):
        response = client.post(
            f"/api/v1/applications/{application_id}/deployments",
            headers=headers,
            json=payload,
        )
        if response.status_code == 202:
            try:
                return _wait_application_deployment(
                    client,
                    headers,
                    application_id,
                    response.json()["id"],
                )
            except AssertionError as error:
                last_error = str(error)
                if "ADMISSIONPREREQUISITEMISSING" not in last_error:
                    raise
        else:
            last_error = response.text
            if "WORKFLOW_VERSION_NOT_DEPLOYED" not in last_error:
                response.raise_for_status()
        time.sleep(2)
    raise AssertionError(f"application deployment did not converge: {last_error}")


def _wait_gateway_invocation(client: httpx.Client, headers: dict[str, str], invocation_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + 300
    latest: dict[str, Any] = {}
    while time.monotonic() < deadline:
        response = client.get(f"/gateway/v1/invocations/{invocation_id}", headers=headers)
        response.raise_for_status()
        latest = response.json()
        if latest.get("status") in {"completed", "failed", "cancelled", "timed_out"}:
            if latest["status"] == "completed":
                latest["status"] = "succeeded"
            return latest
        time.sleep(1)
    raise AssertionError(f"gateway invocation did not reach a terminal state: {json.dumps(latest, ensure_ascii=False)}")


@pytest.mark.cluster
@pytest.mark.runtime
def test_p305_invocation_sessions_are_isolated_and_entries_are_durable(
    installed_agentx: dict[str, str],
    e2e_providers: dict[str, str],
    service_urls: dict[str, str],
    run_id: str,
) -> None:
    """Exercise the real published Agent path and its durable Session store.

    The fixture Agent uses ``invocation`` policy.  Each execution therefore
    gets its own register and Entry tree.  The browser suite covers the
    diagnostics BFF and clear receipt; this test reads the same Runtime
    authority directly so it also works against an already-installed image
    while the Control Plane image is being rebuilt.
    """
    assert e2e_providers["echo_mcp"].endswith(":8090")
    with httpx.Client(base_url=service_urls["web"], timeout=60) as client:
        token, me = _access_token(client)
        headers = {"Authorization": f"Bearer {token}"}
        _runtime_mysql(
            installed_agentx,
            "SELECT COUNT(*) FROM agent_session_registers;",
        )
        environments = client.get("/api/v1/environments", headers=headers)
        environments.raise_for_status()
        environment = next(item for item in environments.json() if item["code"] == "development")
        fixture = _run_fixture_job(
            installed_agentx,
            run_id,
            me,
            environment["id"],
            job_suffix="sessions",
        )
        assert fixture["tenantId"] == me["companyId"]

        first = _run_agent(client, headers, "one")
        second = _run_agent(client, headers, "two")
        assert first["status"] == second["status"] == "succeeded", json.dumps(
            {"first": first, "second": second}, ensure_ascii=False
        )

        rows = _runtime_mysql(
            installed_agentx,
            "SELECT JSON_ARRAYAGG(JSON_OBJECT("
            "'sessionKey',session_key,'nodeKey',stable_agent_node_key,"
            "'sessionId',session_id,'updatedAt',DATE_FORMAT(updated_at,'%Y-%m-%dT%H:%i:%s.%fZ'),"
            "'entryCount',(SELECT COUNT(*) FROM agent_session_entries e "
            "WHERE e.tenant_id=r.tenant_id AND e.session_key=r.session_key "
            "AND e.stable_agent_node_key=r.stable_agent_node_key))) "
            "FROM agent_session_registers r "
            f"WHERE r.tenant_id=UUID_TO_BIN('{me['companyId']}') AND r.session_key LIKE 'invocation:%';",
        )
        invocation_items = json.loads(rows) if rows and rows != "null" else []
        assert len(invocation_items) >= 2
        keys = {item["sessionKey"] for item in invocation_items}
        assert len(keys) == len(invocation_items), "invocation sessions must not share a key"
        assert all(item["entryCount"] >= 1 for item in invocation_items)

        cleared = next(item for item in invocation_items if first["id"] in item["sessionKey"])
        clear_request = {
            "sessionKey": cleared["sessionKey"],
            "stableAgentNodeKey": cleared["nodeKey"],
            "idempotencyKey": f"p3-05-session-clear-{run_id}",
        }
        clear_response = client.post("/api/v1/agent-sessions/clear", headers=headers, json=clear_request)
        assert clear_response.status_code == 200, clear_response.text
        clear_receipt = clear_response.json()
        assert clear_receipt["clearedEntries"] >= 1, clear_receipt

        replay = client.post("/api/v1/agent-sessions/clear", headers=headers, json=clear_request)
        assert replay.status_code == 200, replay.text
        assert replay.json() == clear_receipt

        remaining = _runtime_mysql(
            installed_agentx,
            "SELECT "
            "(SELECT COUNT(*) FROM agent_session_registers WHERE tenant_id=UUID_TO_BIN('"
            f"{me['companyId']}') AND session_key='{cleared['sessionKey']}' AND stable_agent_node_key='{cleared['nodeKey']}') + "
            "(SELECT COUNT(*) FROM agent_session_entries WHERE tenant_id=UUID_TO_BIN('"
            f"{me['companyId']}') AND session_key='{cleared['sessionKey']}' AND stable_agent_node_key='{cleared['nodeKey']}') + "
            "(SELECT COUNT(*) FROM agent_session_operations WHERE tenant_id=UUID_TO_BIN('"
            f"{me['companyId']}') AND session_key='{cleared['sessionKey']}' AND stable_agent_node_key='{cleared['nodeKey']}') + "
            "(SELECT COUNT(*) FROM agent_session_usages WHERE tenant_id=UUID_TO_BIN('"
            f"{me['companyId']}') AND session_key='{cleared['sessionKey']}' AND stable_agent_node_key='{cleared['nodeKey']}') + "
            "(SELECT COUNT(*) FROM agent_session_pending_entries WHERE tenant_id=UUID_TO_BIN('"
            f"{me['companyId']}') AND session_key='{cleared['sessionKey']}' AND stable_agent_node_key='{cleared['nodeKey']}');",
        )
        assert int(remaining or "0") == 0
        clear_audit = _runtime_mysql(
            installed_agentx,
            "SELECT COUNT(*) FROM audit_events "
            f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') "
            f"AND id=UUID_TO_BIN('{clear_receipt['auditId']}') "
            "AND action='agent_session.clear';",
        )
        assert int(clear_audit or "0") == 1


@pytest.mark.cluster
@pytest.mark.runtime
def test_p305_application_session_continues_across_gateway_executions(
    installed_agentx: dict[str, str],
    e2e_providers: dict[str, str],
    service_urls: dict[str, str],
    run_id: str,
) -> None:
    """Publish the fixture with application_session and execute it twice.

    The Session ID is created by Runtime and supplied back through the Gateway;
    the Agent Worker must therefore load the same durable Agent Session on both
    executions.  No client-provided external user identifier is used as the
    Agent Session key.
    """
    assert e2e_providers["echo_mcp"].endswith(":8090")
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        control_headers = {"Authorization": f"Bearer {token}"}
        environments = control.get("/api/v1/environments", headers=control_headers)
        environments.raise_for_status()
        environment = next(item for item in environments.json() if item["code"] == "development")
        fixture = _run_fixture_job(
            installed_agentx,
            run_id,
            me,
            environment["id"],
            job_suffix="application-session",
            session_policy="application_session",
        )
        application_id = fixture["applicationId"]
        application_slug = fixture["applicationSlug"]
        api_key_response = control.post(
            f"/api/v1/applications/{application_id}/api-keys",
            headers=control_headers,
            json={"name": f"P3-05 application session {run_id}"},
        )
        assert api_key_response.status_code == 201, api_key_response.text
        api_key = api_key_response.json()["secret"]
        _wait_admission_outbox(installed_agentx)
        _publish_application_deployment(
            control,
            control_headers,
            application_id,
            fixture["workflowVersionId"],
            environment["id"],
        )

    with httpx.Client(base_url=service_urls["runtime"], timeout=60) as gateway:
        gateway_headers = {
            "Authorization": f"Bearer {api_key}",
            "Idempotency-Key": f"p3-05-session-create-{run_id}",
        }
        session_response = gateway.post(
            f"/gateway/v1/applications/{application_slug}/sessions",
            headers=gateway_headers,
            json={"title": "P3-05 durable application session"},
        )
        assert session_response.status_code == 201, session_response.text
        session_id = session_response.json()["id"]

        def invoke(index: int) -> dict[str, Any]:
            response = gateway.post(
                f"/gateway/v1/applications/{application_slug}/invocations",
                headers={
                    "Authorization": f"Bearer {api_key}",
                    "Idempotency-Key": f"p3-05-application-session-{run_id}-{index}",
                },
                json={
                    "input": {"message": f"p3-05-application-session-{index}"},
                    "sessionId": session_id,
                    "responseMode": "async",
                },
            )
            assert response.status_code == 202, response.text
            return _wait_gateway_invocation(
                gateway,
                {"Authorization": f"Bearer {api_key}"},
                response.json()["id"],
            )

        first = invoke(1)
        second = invoke(2)
        assert first["status"] == second["status"] == "succeeded", json.dumps(
            {"first": first, "second": second}, ensure_ascii=False
        )

    rows = _runtime_mysql(
        installed_agentx,
        "SELECT JSON_ARRAYAGG(JSON_OBJECT("
        "'sessionKey',session_key,'sessionId',session_id,"
        "'entryCount',(SELECT COUNT(*) FROM agent_session_entries e "
        "WHERE e.tenant_id=r.tenant_id AND e.session_key=r.session_key "
        "AND e.stable_agent_node_key=r.stable_agent_node_key))) "
        "FROM agent_session_registers r "
        f"WHERE r.tenant_id=UUID_TO_BIN('{me['companyId']}') "
        f"AND r.session_key='application:{application_id}:{session_id}';",
    )
    items = json.loads(rows) if rows and rows.lower() != "null" else []
    assert len(items) == 1, items
    assert items[0]["sessionKey"] == f"application:{application_id}:{session_id}"
    assert items[0]["entryCount"] >= 4, items


@pytest.mark.cluster
@pytest.mark.runtime
def test_p305_threshold_compaction_is_durable_and_accounted(
    installed_agentx: dict[str, str],
    e2e_providers: dict[str, str],
    service_urls: dict[str, str],
    run_id: str,
) -> None:
    """A low threshold forces a real Compaction Model Effect in Runtime."""
    assert e2e_providers["echo_mcp"].endswith(":8090")
    with httpx.Client(base_url=service_urls["web"], timeout=60) as client:
        token, me = _access_token(client)
        headers = {"Authorization": f"Bearer {token}"}
        environments = client.get("/api/v1/environments", headers=headers)
        environments.raise_for_status()
        environment = next(item for item in environments.json() if item["code"] == "development")
        _run_fixture_job(
            installed_agentx,
            run_id,
            me,
            environment["id"],
            job_suffix="threshold-compaction",
            fixture_env={
                "AGENTX_V2_FIXTURE_COMPACTION_THRESHOLD": "512",
                "AGENTX_V2_FIXTURE_MAX_TOTAL_TOKENS": "64000",
                "AGENTX_V2_FIXTURE_SYSTEM_PROMPT": "P3_THRESHOLD_COMPACTION",
                "AGENTX_V2_FIXTURE_USER_QUESTION": "context " * 400,
            },
        )
        result = _run_agent(client, headers, "threshold-compaction")
        assert result["status"] == "succeeded", {
            key: result.get(key) for key in ("id", "status", "errorCode", "errorMessage", "output")
        }

    execution_id = result["id"]
    calls = json.loads(
        _runtime_mysql(
            installed_agentx,
            "SELECT JSON_ARRAYAGG(JSON_OBJECT('kind',call_kind,'status',status)) "
            f"FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{execution_id}');",
        )
        or "[]"
    )
    compaction_calls = sum(call["kind"] == "compaction" for call in calls)
    assert compaction_calls >= 1, calls
    usage = _runtime_mysql(
        installed_agentx,
        "SELECT COUNT(*) FROM agent_session_usages "
        f"WHERE session_id LIKE 'invocation:{execution_id}:%' AND usage_kind='compaction';",
    )
    assert int(usage or "0") == compaction_calls
    unique_usage = _runtime_mysql(
        installed_agentx,
        "SELECT COUNT(DISTINCT effect_id) FROM agent_session_usages "
        f"WHERE session_id LIKE 'invocation:{execution_id}:%' AND usage_kind='compaction';",
    )
    assert int(unique_usage or "0") == compaction_calls
    entries = _runtime_mysql(
        installed_agentx,
        "SELECT COUNT(*) FROM agent_session_entries "
        f"WHERE session_key LIKE 'invocation:{execution_id}:%' AND entry_kind='compaction';",
    )
    assert int(entries or "0") == compaction_calls


@pytest.mark.cluster
@pytest.mark.runtime
def test_p305_provider_overflow_compaction_retries_once(
    installed_agentx: dict[str, str],
    e2e_providers: dict[str, str],
    service_urls: dict[str, str],
    run_id: str,
) -> None:
    """Echo Provider emits a context overflow; Core compacts and retries once."""
    assert e2e_providers["echo_mcp"].endswith(":8090")
    with httpx.Client(base_url=service_urls["web"], timeout=60) as client:
        token, me = _access_token(client)
        headers = {"Authorization": f"Bearer {token}"}
        environments = client.get("/api/v1/environments", headers=headers)
        environments.raise_for_status()
        environment = next(item for item in environments.json() if item["code"] == "development")
        _run_fixture_job(
            installed_agentx,
            run_id,
            me,
            environment["id"],
            job_suffix="overflow-compaction",
            fixture_env={
                "AGENTX_V2_FIXTURE_SYSTEM_PROMPT": "P3_CONTEXT_OVERFLOW",
                "AGENTX_V2_FIXTURE_COMPACTION_THRESHOLD": "64000",
            },
        )
        result = _run_agent(client, headers, "overflow-compaction")
        assert result["status"] == "succeeded", result

    execution_id = result["id"]
    calls = json.loads(
        _runtime_mysql(
            installed_agentx,
            "SELECT JSON_ARRAYAGG(JSON_OBJECT('index',call_index,'kind',call_kind,'status',status,'error',error_code)) "
            f"FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{execution_id}');",
        )
        or "[]"
    )
    calls.sort(key=lambda call: call["index"])
    model_calls = [call for call in calls if call["kind"] == "model"]
    compaction_kind = _runtime_mysql(
        installed_agentx,
        "SELECT JSON_UNQUOTE(JSON_EXTRACT(register_json,'$.compaction.kind')) "
        "FROM agent_session_registers "
        f"WHERE session_key LIKE '%{execution_id}%' LIMIT 1;",
    )
    assert len(model_calls) == 3, {"calls": calls, "compactionKind": compaction_kind}
    assert model_calls[0]["status"] == "failed", calls
    assert sum(call["kind"] == "compaction" for call in calls) == 1, calls
    assert model_calls[0]["error"] == "PROVIDER_REJECTED", calls


@pytest.mark.cluster
@pytest.mark.runtime
def test_p305_subject_memory_write_recall_is_scoped_and_audited(
    installed_agentx: dict[str, str],
    e2e_providers: dict[str, str],
    service_urls: dict[str, str],
    run_id: str,
) -> None:
    """Exercise trusted Subject Memory across two gateway executions.

    The first invocation writes a marker and the second recalls it.  Both
    calls use the authenticated gateway subject; no request-body user ID is
    used to construct the namespace.
    """
    assert e2e_providers["mem0"].endswith(":8000")
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        environments = control.get("/api/v1/environments", headers=headers)
        environments.raise_for_status()
        environment = next(item for item in environments.json() if item["code"] == "development")
        fixture = _run_fixture_job(
            installed_agentx,
            run_id,
            me,
            environment["id"],
            job_suffix="subject-memory",
            session_policy="application_session",
            fixture_env={
                "AGENTX_V2_FIXTURE_MEMORY_OPERATION": "manage",
                "AGENTX_V2_FIXTURE_MEMORY_ACCESS_MODE": "read_write",
            },
        )
        application_id = fixture["applicationId"]
        api_key_response = control.post(
            f"/api/v1/applications/{application_id}/api-keys",
            headers=headers,
            json={"name": f"P3-05 subject memory {run_id}"},
        )
        assert api_key_response.status_code == 201, api_key_response.text
        api_key = api_key_response.json()["secret"]
        _wait_admission_outbox(installed_agentx)
        _publish_application_deployment(
            control,
            headers,
            application_id,
            fixture["workflowVersionId"],
            environment["id"],
        )

    with httpx.Client(base_url=service_urls["runtime"], timeout=60) as gateway:
        # Create the session under the authenticated user identity. The
        # runtime derives Subject Memory scope from this trusted mapping;
        # the application API key is only used to invoke the application.
        session_headers = {"Authorization": f"Bearer {token}"}

        def create_session(index: int) -> str:
            response = gateway.post(
                f"/gateway/v1/applications/{fixture['applicationSlug']}/sessions",
                headers={
                    **session_headers,
                    "Idempotency-Key": f"p3-05-memory-session-{run_id}-{index}",
                },
                json={
                    "title": f"P3-05 subject memory session {index}",
                    "externalUserId": "00000000-0000-7000-8000-00000000dead",
                },
            )
            assert response.status_code == 201, response.text
            return response.json()["id"]

        write_session_id = create_session(1)
        recall_session_id = create_session(2)

        def invoke(index: int, marker: str, session_id: str) -> dict[str, Any]:
            response = gateway.post(
                f"/gateway/v1/applications/{fixture['applicationSlug']}/invocations",
                headers={
                    "Authorization": f"Bearer {api_key}",
                    "Idempotency-Key": f"p3-05-memory-{run_id}-{index}",
                },
                json={
                    "input": {"message": marker},
                    "sessionId": session_id,
                    "responseMode": "async",
                },
            )
            assert response.status_code == 202, response.text
            return _wait_gateway_invocation(gateway, {"Authorization": f"Bearer {api_key}"}, response.json()["id"])

        first = invoke(1, "P3_MEMORY_WRITE", write_session_id)
        second = invoke(2, "P3_MEMORY_RECALL", recall_session_id)
        assert first["status"] == second["status"] == "succeeded", json.dumps(
            {"first": first, "second": second}, ensure_ascii=False
        )

    call_rows = _runtime_mysql(
        installed_agentx,
        "SELECT JSON_ARRAYAGG(JSON_OBJECT('executionId',BIN_TO_UUID(execution_id),'callKind',call_kind,'toolName',tool_name_snapshot,'status',status)) "
        f"FROM runtime_calls WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') "
        f"AND execution_id IN (UUID_TO_BIN('{first['executionId']}'),UUID_TO_BIN('{second['executionId']}'));",
    )
    calls = json.loads(call_rows) if call_rows and call_rows.lower() != "null" else []
    registry_json = _runtime_mysql(
        installed_agentx,
        "SELECT JSON_EXTRACT(runtime_settings_json,'$.agentBundle.agents[0].attachmentRegistry') "
        "FROM execution_snapshots "
        f"WHERE execution_id=UUID_TO_BIN('{first['executionId']}');",
    )
    registry = json.loads(registry_json) if registry_json else None
    assert sum(call["callKind"] == "memory" for call in calls) >= 2, calls
    assert registry is not None and {tool["name"] for tool in registry["tools"]} >= {
        "memory_recall",
        "memory_write",
    }, {"registry": registry, "calls": calls}
    audit_rows = _runtime_mysql(
        installed_agentx,
        "SELECT JSON_ARRAYAGG(JSON_OBJECT('operation',operation,'subject',BIN_TO_UUID(authenticated_subject_id))) "
        "FROM agent_subject_memory_audit "
        f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') AND application_id=UUID_TO_BIN('{application_id}') "
        f"AND authenticated_subject_id=UUID_TO_BIN('{me['id']}') "
        "AND operation IN ('write','recall');",
    )
    audits = json.loads(audit_rows) if audit_rows and audit_rows.lower() != "null" else []
    assert {item["operation"] for item in audits} >= {"write", "recall"}

    memory_version_id = next(tool["resourceVersionId"] for tool in registry["tools"] if tool["name"] == "memory_recall")
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        clear_response = control.post(
            "/api/v1/agent-subject-memory/clear",
            headers={"Authorization": f"Bearer {token}"},
            json={
                "applicationId": application_id,
                "memoryResourceVersionId": memory_version_id,
                "idempotencyKey": f"p3-05-subject-memory-clear-{run_id}",
            },
        )
        assert clear_response.status_code == 200, clear_response.text
        clear_receipt = clear_response.json()
        assert clear_receipt["status"] == "applied", clear_receipt

    with httpx.Client(base_url=service_urls["runtime"], timeout=60) as gateway:
        third_session_response = gateway.post(
            f"/gateway/v1/applications/{fixture['applicationSlug']}/sessions",
            headers={
                "Authorization": f"Bearer {token}",
                "Idempotency-Key": f"p3-05-memory-session-{run_id}-3",
            },
            json={
                "title": "P3-05 cleared subject memory session",
                "externalUserId": "00000000-0000-7000-8000-00000000dead",
            },
        )
        assert third_session_response.status_code == 201, third_session_response.text
        third_session_id = third_session_response.json()["id"]
        response = gateway.post(
            f"/gateway/v1/applications/{fixture['applicationSlug']}/invocations",
            headers={
                "Authorization": f"Bearer {api_key}",
                "Idempotency-Key": f"p3-05-memory-{run_id}-3",
            },
            json={
                "input": {"message": "P3_MEMORY_RECALL"},
                "sessionId": third_session_id,
                "responseMode": "async",
            },
        )
        assert response.status_code == 202, response.text
        third = _wait_gateway_invocation(gateway, {"Authorization": f"Bearer {api_key}"}, response.json()["id"])
        assert third["status"] == "succeeded", third

    cleared_provider_effects = _runtime_mysql(
        installed_agentx,
        "SELECT COUNT(*) FROM runtime_calls "
        f"WHERE execution_id=UUID_TO_BIN('{third['executionId']}') AND call_kind='memory';",
    )
    assert int(cleared_provider_effects or "0") == 0
    clear_audit = _runtime_mysql(
        installed_agentx,
        "SELECT COUNT(*) FROM agent_subject_memory_audit "
        f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') "
        f"AND application_id=UUID_TO_BIN('{application_id}') "
        f"AND authenticated_subject_id=UUID_TO_BIN('{me['id']}') "
        f"AND memory_resource_version_id=UUID_TO_BIN('{memory_version_id}') "
        "AND operation='clear';",
    )
    assert int(clear_audit or "0") == 1
