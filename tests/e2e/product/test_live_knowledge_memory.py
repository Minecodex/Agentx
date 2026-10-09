"""Actual indexing, retrieval and subject-scoped Agent memory, without mocks."""

# ruff: noqa: S608 -- UUIDs and call kinds come from this isolated E2E run.

from __future__ import annotations

import json
import time

import httpx
import pytest

from tests.e2e.product.live_text_support import post
from tests.e2e.product.test_live_text_acceptance import message, session, terminal
from tests.e2e.product.test_model_streaming import _grant_model_to_workflow, _stream_workflow
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.runtime.test_agent_attachments import _control_mysql, _runtime_mysql
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


@pytest.fixture(scope="module")
def live_indexed_document(live_providers, installed_agentx, run_id):
    control = live_providers["control"]
    path = f"/api/v1/knowledge/resources/{live_providers['ragId']}"
    marker = f"sapphire-orbit-{run_id}"
    content = f"水星计划由蓝桥团队负责。水星计划唯一发布代号是 {marker}。正式上线日期为2026年11月5日。负责人是林晓。"
    uploaded = control.post(
        f"{path}/documents", files={"file": ("p7-live-document.md", content.encode(), "text/markdown")}
    )
    assert uploaded.status_code == 202, uploaded.text
    document = uploaded.json()
    duplicate = control.post(f"{path}/documents", files={"file": ("duplicate.md", content.encode(), "text/markdown")})
    assert duplicate.status_code == 422 and duplicate.json()["code"] == "KNOWLEDGE_DOCUMENT_DUPLICATED", duplicate.text
    invalid = control.post(f"{path}/documents", files={"file": ("invalid.exe", b"invalid", "application/octet-stream")})
    assert invalid.status_code in (400, 415, 422), invalid.text
    oversized = control.post(
        f"{path}/documents", files={"file": ("oversized.txt", b"x" * (8 * 1024 * 1024 + 1), "text/plain")}
    )
    assert oversized.status_code == 422 and oversized.json()["code"] == "KNOWLEDGE_DOCUMENT_TOO_LARGE", oversized.text
    busy = control.delete(f"{path}/documents/{document['id']}")
    assert busy.status_code == 409, busy.text
    deadline = time.monotonic() + 600
    while True:
        response = control.get(f"{path}/documents")
        response.raise_for_status()
        current = next(item for item in response.json() if item["id"] == document["id"])
        assert current["status"] != "failed", current
        if current["status"] == "indexed":
            break
        assert time.monotonic() < deadline, current
        time.sleep(2)
    retrieval = control.post(f"{path}/retrieval-test", json={"query": "水星计划唯一发布代号是什么", "topK": 5})
    assert retrieval.status_code == 200, retrieval.text
    assert any(marker in item.get("content", "") for item in retrieval.json()["documents"]), retrieval.text
    write_report(
        installed_agentx,
        "product/live-lightrag-index.json",
        {
            "status": "passed",
            "ragId": live_providers["ragId"],
            "documentId": current["id"],
            "externalDocumentId": current["externalDocumentId"],
            "marker": marker,
            "documents": retrieval.json()["documents"],
            "realCpuEmbedding": True,
            "realLlmExtraction": "kimi/k3",
        },
    )
    return {"documentId": document["id"], "marker": marker}


def test_lightrag_indexes_and_retrieves_actual_document(live_indexed_document):
    assert live_indexed_document["documentId"]


@pytest.fixture(scope="module")
def live_agent_application(live_providers, live_application, installed_agentx, service_urls, run_id):
    model_app = live_application
    control = live_providers["control"]
    headers = dict(control.headers)
    workflow = _stream_workflow(control, headers, f"P7 real Agent {run_id}", model_app["modelId"])
    _grant_model_to_workflow(control, headers, model_app["modelAlias"], workflow, model_app["credentialId"])
    identity = control.get(f"/api/v1/workflows/{workflow}").json()["serviceIdentityId"]
    for kind, resource, operation in (
        ("rag", live_providers["ragId"], "read"),
        ("memory", live_providers["memoryId"], "manage"),
        ("credential", live_providers["ragCredentialId"], "use"),
        ("credential", live_providers["memoryCredentialId"], "use"),
    ):
        post(
            control,
            f"/resources/{kind}/{resource}/grants",
            {
                "subjectType": "workflow_service_identity",
                "subjectId": identity,
                "resourceVersionId": None,
                "operation": operation,
            },
        )
    draft = control.get(f"/api/v1/workflows/{workflow}/draft").json()
    node = next(node for node in draft["definition"]["nodes"] if node["type"] == "model")
    reference = node["parameters"]["userQuestion"]
    node["type"] = "agent"
    node["typeVersion"] = 2
    node["name"] = "Real Kimi Agent"
    node["settings"] = {"timeoutMs": 180000}
    draft["definition"]["settings"]["timeoutMs"] = 300000
    node["resourceReferences"][0]["bindingRole"] = "model"
    selected_model = control.get(f"/api/v1/models/aliases/{model_app['modelId']}")
    selected_model.raise_for_status()
    node["resourceReferences"][0]["resourceVersionId"] = selected_model.json()["deploymentId"]
    node["parameters"] = {
        "systemPrompt": {
            "kind": "template",
            "segments": [
                {
                    "kind": "text",
                    "text": "严格执行用户指定的工具操作。知识问题必须先调用 knowledge_search 工具。记忆写入必须调用 memory_write,回忆必须调用 memory_recall。每个问题只调用一次指定工具,得到结果后立即回答。工具没有数据时明确回答不知道,不要猜测。",
                }
            ],
        },
        "userQuestion": reference,
        "sessionPolicy": {"mode": "application_session"},
        "maxIterations": 6,
        "maxModelCalls": 6,
        "maxToolCalls": 6,
        "maxTotalTokens": 12000,
        "maxOutputTokens": 1024,
        "maxCost": 1,
        "maxDurationSeconds": 180,
        "limitAction": "fail",
    }
    node["resourceReferences"].extend(
        [
            {
                "bindingRole": "knowledge",
                "resourceType": "rag",
                "resourceId": live_providers["ragId"],
                "operation": "read",
            },
            {
                "bindingRole": "long_term_memory",
                "resourceType": "memory",
                "resourceId": live_providers["memoryId"],
                "operation": "manage",
            },
        ]
    )
    saved = control.put(
        f"/api/v1/workflows/{workflow}/draft",
        json={"expectedRevision": draft["revision"], "definition": draft["definition"]},
    )
    assert saved.status_code in (200, 204), saved.text
    revision = control.get(f"/api/v1/workflows/{workflow}/draft").json()["revision"]
    version = post(control, f"/workflows/{workflow}/versions", {"draftRevision": revision})
    environment = next(item for item in control.get("/api/v1/environments").json() if item["code"] == "development")
    post(
        control,
        f"/workflows/{workflow}/deployments",
        {"environmentId": environment["id"], "workflowVersionId": version["id"]},
    )
    application = post(
        control,
        "/applications",
        {
            "workflowId": workflow,
            "name": f"P7 real Agent App {run_id}",
            "slug": f"p7-agent-{run_id}",
            "visibility": "company",
        },
    )
    deployed = _publish_application_deployment(control, headers, application["id"], version["id"], environment["id"])
    path = f"/api/v1/applications/{application['id']}/deployments/{deployed['id']}/playground-config"
    config = control.get(path).json()
    saved = control.put(
        path,
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
    assert saved.status_code in (200, 202), saved.text
    deadline = time.monotonic() + 120
    while control.get(path).json()["publishStatus"] != "active":
        assert time.monotonic() < deadline
        time.sleep(1)
    try:
        yield {
            "applicationId": application["id"],
            "applicationSlug": application["slug"],
            "workflowId": workflow,
            "token": model_app["token"],
        }
    finally:
        executions = _runtime_mysql(
            installed_agentx,
            f"SELECT BIN_TO_UUID(id),status,error_code,error_message FROM workflow_executions WHERE workflow_id=UUID_TO_BIN('{workflow}') ORDER BY created_at;",
        )
        resources = _runtime_mysql(
            installed_agentx,
            "SELECT b.resource_kind,BIN_TO_UUID(b.resource_id),b.resource_version,b.state_epoch,s.state_epoch,s.status,b.content_hash=s.content_hash FROM runtime_resource_bindings b JOIN runtime_resource_states s USING(tenant_id,resource_kind,resource_id) "
            f"WHERE b.bundle_id IN (SELECT id FROM deployment_bundles WHERE application_id=UUID_TO_BIN('{application['id']}'));",
        )
        write_report(
            installed_agentx,
            "product/live-agent-diagnostics.json",
            {"executions": executions, "resourceStates": resources},
        )


def _ask(gateway, app, title, question):
    session_id = session(gateway, app, title)
    invocation = message(gateway, session_id, question)
    result = terminal(gateway, invocation)
    assert result["status"] == "completed", result
    messages = gateway.get(f"/gateway/v1/sessions/{session_id}/messages").json()
    answer = next(item for item in messages if item["role"] == "assistant")
    return result, "".join(part.get("content", "") for part in answer["parts"] if isinstance(part.get("content"), str))


def test_real_agent_searches_managed_knowledge_and_recalls_across_sessions(
    installed_agentx, service_urls, live_providers, live_agent_application, live_indexed_document, run_id
):
    app = live_agent_application
    headers = {"Authorization": f"Bearer {app['token'].value}"}
    with httpx.Client(base_url=service_urls["runtime"], headers=headers, timeout=180) as gateway:
        knowledge, answer = _ask(
            gateway, app, "真实知识检索", "请调用知识检索工具查找水星计划的唯一发布代号,并准确回答该代号。"
        )
        assert live_indexed_document["marker"] in answer, answer
        marker = f"水星记忆-{run_id}"
        written, _ = _ask(
            gateway, app, "真实记忆写入", f"请调用 memory_write 把这个事实存入我的长期记忆:我的私人项目代号是{marker}。"
        )
        write_calls = _runtime_mysql(
            installed_agentx,
            f"SELECT COUNT(*) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{written['executionId']}') AND call_kind='memory' AND status='succeeded';",
        )
        assert int(write_calls) > 0, "Memory write must actually succeed before testing recall"
        recalled, answer = _ask(
            gateway, app, "新会话记忆召回", "请先调用 memory_recall 查询我的私人项目代号,准确回答工具查到的代号。"
        )
        assert marker in answer, answer
        control = live_providers["control"]
        role = post(
            control,
            "/roles",
            {
                "code": f"p7_chat_{run_id}",
                "name": f"P7 chat user {run_id}",
                "dataScope": "company",
                "permissions": ["application:view", "application:invoke"],
            },
        )
        username = f"p7-memory-reader-{run_id}"
        user = post(
            control,
            "/users",
            {
                "username": username,
                "displayName": username,
                "departmentId": live_application_department(control),
                "roleId": role["id"],
            },
        )
        first_login = control.post("/api/v1/auth/login", json={"username": username, "password": "123456"})
        assert first_login.status_code == 200, first_login.text
        changed = control.post(
            "/api/v1/auth/change-password",
            json={"token": first_login.json()["changePasswordToken"], "password": f"P7-private-password-{run_id}"},
        )
        assert changed.status_code == 200, changed.text
        deadline = time.monotonic() + 120
        while True:
            pending = _control_mysql(
                installed_agentx,
                "SELECT COUNT(*) FROM outbox WHERE aggregate_type IN ('runtime_user_admission','application_admission') AND status IN ('pending','processing','failed');",
            )
            admitted = _runtime_mysql(
                installed_agentx,
                "SELECT COUNT(*) FROM runtime_user_admission u JOIN runtime_user_application_grants g USING(tenant_id,user_id) "
                f"WHERE u.user_id=UUID_TO_BIN('{user['id']}') AND g.application_id=UUID_TO_BIN('{app['applicationId']}') AND u.status='active' AND g.status='active' AND g.can_invoke=TRUE;",
            )
            if pending == "0" and admitted == "1":
                break
            assert time.monotonic() < deadline, (pending, admitted)
            time.sleep(0.5)
        gateway.headers["Authorization"] = f"Bearer {changed.json()['accessToken']}"
        isolated, other_answer = _ask(
            gateway,
            app,
            "另一用户的独立会话",
            "请先调用 memory_recall 查询我的私人项目代号。如果工具没有找到就回答不知道。",
        )
        assert marker not in other_answer, other_answer
        memory_results = _runtime_mysql(
            installed_agentx,
            f"SELECT JSON_LENGTH(JSON_EXTRACT(response_json,'$.results')) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{isolated['executionId']}') AND call_kind='memory' AND status='succeeded' ORDER BY ended_at DESC LIMIT 1;",
        )
        assert memory_results == "0", memory_results
        revoked = control.patch(
            f"/api/v1/roles/{role['id']}",
            json={
                "name": role["name"],
                "description": None,
                "dataScope": "company",
                "version": role["version"],
                "permissions": ["application:view"],
            },
        )
        assert revoked.status_code == 200, revoked.text
        deadline = time.monotonic() + 120
        while True:
            revoked_grants = _runtime_mysql(
                installed_agentx,
                "SELECT COUNT(*) FROM runtime_user_admission u JOIN runtime_user_application_grants g USING(tenant_id,user_id) "
                f"WHERE u.user_id=UUID_TO_BIN('{user['id']}') AND g.application_id=UUID_TO_BIN('{app['applicationId']}') AND u.status='active' AND u.token_version>=3 AND g.can_invoke=FALSE;",
            )
            if revoked_grants == "1":
                break
            assert time.monotonic() < deadline, revoked_grants
            time.sleep(0.5)
        signed_in = control.post(
            "/api/v1/auth/login", json={"username": username, "password": f"P7-private-password-{run_id}"}
        )
        assert signed_in.status_code == 200, signed_in.text
        gateway.headers["Authorization"] = f"Bearer {signed_in.json()['accessToken']}"
        denied = gateway.post(
            f"/gateway/v1/applications/{app['applicationSlug']}/sessions",
            json={"title": "撤销权限后应拒绝", "externalUserId": None},
            headers={"Idempotency-Key": f"live-revoked-{time.time_ns()}"},
        )
        assert denied.status_code == 401, denied.text
    calls = []
    leases = []
    for result, kind in ((knowledge, "rag"), (written, "memory"), (recalled, "memory")):
        count = _runtime_mysql(
            installed_agentx,
            f"SELECT COUNT(*) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{result['executionId']}') AND call_kind='{kind}' AND status='succeeded';",
        )
        assert int(count) > 0, (result["executionId"], kind, count)
        calls.append({"executionId": result["executionId"], "kind": kind, "succeededCalls": int(count)})
        lease = json.loads(
            _runtime_mysql(
                installed_agentx,
                f"SELECT JSON_OBJECT('executionId',BIN_TO_UUID(execution_id),'durationSeconds',TIMESTAMPDIFF(SECOND,started_at,ended_at),'status',status,'leaseReleased',locked_until IS NULL AND heartbeat_at IS NULL) FROM node_attempts WHERE execution_id=UUID_TO_BIN('{result['executionId']}') AND capability='agent' AND status='succeeded' LIMIT 1;",
            )
        )
        assert lease["status"] == "succeeded" and lease["leaseReleased"] == 1, lease
        leases.append(lease)
    write_report(
        installed_agentx,
        "product/live-agent-knowledge-memory.json",
        {
            "status": "passed",
            "trustedApplicationSubject": True,
            "newSessionRecall": True,
            "anotherAuthenticatedUserIsolated": True,
            "roleRevocationDeniedFreshToken": True,
            "isolatedExecutionId": isolated["executionId"],
            "calls": calls,
            "attemptLeases": leases,
        },
    )


def live_application_department(control):
    response = control.get("/api/v1/auth/me")
    response.raise_for_status()
    return response.json()["departmentId"]


def test_debug_agent_rejects_missing_application_session_before_memory_access(
    installed_agentx, live_providers, live_agent_application
):
    control = live_providers["control"]
    workflow = live_agent_application["workflowId"]
    draft = control.get(f"/api/v1/workflows/{workflow}/draft").json()
    response = control.post(
        f"/api/v1/workflows/{workflow}/debug-executions",
        headers={"Idempotency-Key": f"live-subject-refusal-{time.time_ns()}"},
        json={
            "expectedRevision": draft["revision"],
            "mode": "full",
            "input": {"message": "请调用 memory_recall 查询我的私人项目代号"},
            "context": {},
            "overlayIds": [],
            "sideEffectDecisions": {"model": "execute"},
        },
    )
    assert response.status_code == 202, response.text
    execution_id = response.json()["executionId"]
    deadline = time.monotonic() + 60
    while True:
        execution = control.get(f"/api/v1/executions/{execution_id}").json()
        if execution["status"] in {"failed", "succeeded"}:
            break
        assert time.monotonic() < deadline, execution
        time.sleep(0.5)
    assert execution["status"] == "failed" and execution["errorCode"] == "AGENT_SESSION_REQUIRED", execution
    calls = _runtime_mysql(
        installed_agentx,
        f"SELECT COUNT(*) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{execution_id}') AND call_kind='memory';",
    )
    assert calls == "0", calls
    write_report(
        installed_agentx,
        "product/live-debug-subject-refusal.json",
        {
            "status": "passed",
            "workflowId": workflow,
            "executionId": execution_id,
            "errorCode": execution["errorCode"],
            "providerMemoryCalls": 0,
        },
    )


def test_indexed_document_delete_releases_platform_record(
    installed_agentx, service_urls, live_providers, live_indexed_document, live_agent_application, run_id
):
    control = live_providers["control"]
    resource_path = f"/api/v1/knowledge/resources/{live_providers['ragId']}"
    version = control.get(resource_path).json()["version"]
    path = f"{resource_path}/documents"
    deleted = control.delete(f"{path}/{live_indexed_document['documentId']}")
    assert deleted.status_code == 204, deleted.text
    assert all(item["id"] != live_indexed_document["documentId"] for item in control.get(path).json())
    assert control.get(resource_path).json()["version"] == version
    test_debug_agent_rejects_missing_application_session_before_memory_access(
        installed_agentx, live_providers, live_agent_application
    )
    marker = f"emerald-comet-{run_id}"
    uploaded = control.post(
        f"{resource_path}/documents",
        files={
            "file": ("p7-live-update.md", f"海王计划的新发布代号是 {marker},负责人是林夏。".encode(), "text/markdown")
        },
    )
    assert uploaded.status_code == 202, uploaded.text
    document_id = uploaded.json()["id"]
    deadline = time.monotonic() + 180
    while True:
        document = next(item for item in control.get(path).json() if item["id"] == document_id)
        if document["status"] == "indexed":
            break
        assert document["status"] != "failed" and time.monotonic() < deadline, document
        time.sleep(1)
    assert control.get(resource_path).json()["version"] == version
    app = live_agent_application
    with httpx.Client(
        base_url=service_urls["runtime"], headers={"Authorization": f"Bearer {app['token'].value}"}, timeout=180
    ) as gateway:
        execution, answer = _ask(
            gateway, app, "已发布应用检索新文档", "请调用 knowledge_search 查询海王计划的新发布代号。"
        )
    assert marker in answer, answer
    test_debug_agent_rejects_missing_application_session_before_memory_access(
        installed_agentx, live_providers, live_agent_application
    )
    write_report(
        installed_agentx,
        "product/live-document-resource-lifecycle.json",
        {
            "status": "passed",
            "configurationVersion": version,
            "documentChangesKeepRuntimeBinding": True,
            "existingApplicationSearchesNewDocument": True,
            "executionId": execution["executionId"],
        },
    )
