"""One shared real Agent application and its fail-closed session-boundary probe."""

# ruff: noqa: S608 -- identifiers come from the owned E2E namespace.
from __future__ import annotations

import time

import pytest

from tests.e2e.product.live_text_support import post
from tests.e2e.product.test_model_streaming import _grant_model_to_workflow, _stream_workflow
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tools.scripts.release.evidence import write_report


@pytest.fixture(scope="session")
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


def record_missing_session_refusal(installed_agentx, live_providers, live_agent_application):
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
    return {"workflowId": workflow, "executionId": execution_id, "providerMemoryCalls": 0}
