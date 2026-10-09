"""Real application/namespace memory isolation and Agent-to-Python data flow."""

# ruff: noqa: S608 -- only isolated API-generated UUIDs enter diagnostic SQL.

from __future__ import annotations

import copy
import json
import time
from itertools import pairwise

import httpx
import pytest

from tests.e2e.product.live_text_support import post
from tests.e2e.product.next_batch_support import get, publish_application
from tests.e2e.product.test_live_knowledge_memory import (
    _ask,
)
from tests.e2e.product.test_live_knowledge_memory import (
    live_agent_application as live_agent_application,
)
from tests.e2e.product.test_live_knowledge_memory import (
    live_indexed_document as live_indexed_document,
)
from tests.e2e.product.test_provider_integration import _publish_application_deployment, _sandbox_image_digest
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product]

TOOL_INSTRUCTION = (
    "你是工具调用验收智能体。用户指定 memory_write 时,必须先发出真实 memory_write 函数调用,"
    "等工具返回后才可回答。memory_recall 和 knowledge_search 也必须实际调用。"
    "禁止只口头确认保存,禁止自行猜测工具结果。调用失败必须报告失败。"
)


def _assert_written(context, execution):
    result = _memory_results(context, execution)
    assert any(item.get("event") in {"ADD", "UPDATE"} for item in result), result


def _authorize_definition(control, workflow, definition):
    seen = set()
    for node in definition["nodes"]:
        for reference in node.get("resourceReferences", []):
            key = (reference["resourceType"], reference["resourceId"], reference["operation"])
            if key in seen:
                continue
            seen.add(key)
            post(
                control,
                f"/workflows/{workflow}/resource-authorizations",
                {key: value for key, value in reference.items() if key != "bindingRole"},
            )


def _save_version(control, workflow, definition):
    draft = get(control, f"/workflows/{workflow}/draft")
    saved = control.put(
        f"/api/v1/workflows/{workflow}/draft", json={"expectedRevision": draft["revision"], "definition": definition}
    )
    assert saved.status_code == 200, saved.text
    return post(
        control,
        f"/workflows/{workflow}/versions",
        {"draftRevision": get(control, f"/workflows/{workflow}/draft")["revision"]},
    )


def _memory_results(context, execution):
    result = _runtime_mysql(
        context,
        f"SELECT response_json FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{execution['executionId']}') AND call_kind='memory' AND status='succeeded' ORDER BY ended_at DESC LIMIT 1;",
    )
    assert result, "Expected a real successful Mem0 call"
    return json.loads(result)["results"]


def _switch_application(control, app, version):
    environment = next(item for item in get(control, "/environments") if item["code"] == "development")
    post(
        control,
        f"/workflows/{app['workflowId']}/deployments",
        {"environmentId": environment["id"], "workflowVersionId": version},
    )
    deployed = _publish_application_deployment(
        control, dict(control.headers), app["applicationId"], version, environment["id"]
    )
    path = f"/applications/{app['applicationId']}/deployments/{deployed['id']}/playground-config"
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


def test_real_memory_isolated_by_application_and_namespace(
    installed_agentx, service_urls, live_providers, live_agent_application, run_id
):
    control = live_providers["control"]
    app_a = live_agent_application
    definition = copy.deepcopy(get(control, f"/workflows/{app_a['workflowId']}/draft")["definition"])
    next(node for node in definition["nodes"] if node["type"] == "agent")["parameters"]["systemPrompt"] = {
        "kind": "template",
        "segments": [{"kind": "text", "text": TOOL_INSTRUCTION}],
    }
    deployments = get(control, f"/applications/{app_a['applicationId']}/deployments")
    original_version = next(item for item in deployments if item["status"] == "active")["workflowVersionId"]
    workflow_b = post(control, "/workflows", {"name": f"P7 memory other app {run_id}", "visibility": "company"})["id"]
    _authorize_definition(control, workflow_b, definition)
    version_b = _save_version(control, workflow_b, definition)
    app_b = publish_application(control, workflow_b, version_b["id"], f"p7-other-memory-{run_id}")
    memory = get(control, f"/memory/namespaces/{live_providers['memoryId']}")
    namespace_b = post(
        control,
        "/memory/namespaces",
        {
            "connectionId": memory["connectionId"],
            "name": f"P7 alternate memory {run_id}",
            "externalNamespace": f"p7_alternate_{run_id}",
            "accessMode": "read_write",
            "ownerDepartmentId": memory["ownerDepartmentId"],
        },
    )
    alternate = copy.deepcopy(definition)
    agent = next(node for node in alternate["nodes"] if node["type"] == "agent")
    next(item for item in agent["resourceReferences"] if item["resourceType"] == "memory")["resourceId"] = namespace_b[
        "id"
    ]
    _authorize_definition(control, app_a["workflowId"], alternate)
    alternate_version = _save_version(control, app_a["workflowId"], alternate)
    alpha = f"memory-alpha-{run_id}"
    beta = f"memory-beta-{run_id}"
    gamma = f"memory-gamma-{run_id}"
    checked = []
    with httpx.Client(
        base_url=service_urls["runtime"], headers={"Authorization": f"Bearer {app_a['token'].value}"}, timeout=180
    ) as gateway:
        written_a, _ = _ask(
            gateway,
            app_a,
            "A 写入",
            f"必须实际调用 memory_write 保存事实:我的私人项目代号是 {alpha}。未调用工具不能回复保存成功。",
        )
        _assert_written(installed_agentx, written_a)
        recall_a, answer = _ask(
            gateway, app_a, "A 新会话召回", "请调用 memory_recall 查询我的私人项目代号,准确回答查到的代号。"
        )
        assert alpha in answer and _memory_results(installed_agentx, recall_a), answer
        isolated_b, answer = _ask(
            gateway, app_b, "同用户另一应用", "请调用 memory_recall 查询我的私人项目代号,没有结果就回答不知道。"
        )
        assert alpha not in answer and _memory_results(installed_agentx, isolated_b) == [], answer
        written_b, _ = _ask(
            gateway,
            app_b,
            "B 写入",
            f"必须实际调用 memory_write 保存事实:我的私人项目代号是 {beta}。未调用工具不能回复保存成功。",
        )
        _assert_written(installed_agentx, written_b)
        recall_b, answer = _ask(
            gateway, app_b, "B 新会话召回", "请调用 memory_recall 查询我的私人项目代号,准确回答查到的代号。"
        )
        assert beta in answer and alpha not in answer and _memory_results(installed_agentx, recall_b), answer
        _switch_application(control, app_a, alternate_version["id"])
        try:
            isolated_namespace, answer = _ask(
                gateway, app_a, "A 改绑另一命名空间", "请调用 memory_recall 查询我的私人项目代号,没有结果就回答不知道。"
            )
            assert (
                alpha not in answer
                and beta not in answer
                and _memory_results(installed_agentx, isolated_namespace) == []
            ), answer
            written_namespace, _ = _ask(
                gateway, app_a, "新命名空间写入", f"请调用 memory_write 保存事实:我的私人项目代号是 {gamma}。"
            )
            _assert_written(installed_agentx, written_namespace)
            recall_namespace, answer = _ask(
                gateway, app_a, "新命名空间召回", "请调用 memory_recall 查询我的私人项目代号,准确回答查到的代号。"
            )
            assert gamma in answer and alpha not in answer and _memory_results(installed_agentx, recall_namespace), (
                answer
            )
        finally:
            _switch_application(control, app_a, original_version)
        restored_a, answer = _ask(
            gateway, app_a, "恢复原命名空间", "请调用 memory_recall 查询我的私人项目代号,准确回答查到的代号。"
        )
        assert (
            alpha in answer
            and beta not in answer
            and gamma not in answer
            and _memory_results(installed_agentx, restored_a)
        ), answer
    for result, scope in (
        (written_a, "app-a/namespace-a"),
        (recall_a, "app-a/namespace-a"),
        (isolated_b, "app-b/namespace-a"),
        (written_b, "app-b/namespace-a"),
        (recall_b, "app-b/namespace-a"),
        (isolated_namespace, "app-a/namespace-b"),
        (written_namespace, "app-a/namespace-b"),
        (recall_namespace, "app-a/namespace-b"),
        (restored_a, "app-a/namespace-a"),
    ):
        scope_hash = _runtime_mysql(
            installed_agentx,
            f"SELECT SHA2(JSON_UNQUOTE(COALESCE(JSON_EXTRACT(request_json,'$.user_id'),JSON_EXTRACT(request_json,'$.filters.user_id'))),256) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{result['executionId']}') AND call_kind='memory' AND status='succeeded' ORDER BY ended_at DESC LIMIT 1;",
        )
        assert len(scope_hash) == 64, scope_hash
        checked.append({"executionId": result["executionId"], "scope": scope, "scopeHash": scope_hash})
    groups = {
        scope: {item["scopeHash"] for item in checked if item["scope"] == scope}
        for scope in {item["scope"] for item in checked}
    }
    assert all(len(value) == 1 for value in groups.values()) and len(set.union(*groups.values())) == 3, groups
    write_report(
        installed_agentx,
        "product/live-memory-scope-matrix.json",
        {
            "status": "passed",
            "sameAuthenticatedUser": True,
            "differentApplicationsIsolated": True,
            "differentNamespacesIsolated": True,
            "originalNamespaceRestored": True,
            "scopeEvidence": checked,
        },
    )


def _reference(node, path):
    return {
        "kind": "reference",
        "selector": {
            "namespace": "outputs",
            "sourceNodeId": node,
            "port": "main",
            "run": {"kind": "current"},
            "item": {"kind": "first"},
            "path": path,
        },
        "missingPolicy": {"kind": "error"},
    }


def test_real_agent_knowledge_and_memory_feed_python_in_same_execution(
    installed_agentx, service_urls, live_providers, live_agent_application, live_indexed_document, run_id
):
    control = live_providers["control"]
    source = live_agent_application
    definition = copy.deepcopy(get(control, f"/workflows/{source['workflowId']}/draft")["definition"])
    original_agent = next(node for node in definition["nodes"] if node["type"] == "agent")
    knowledge = copy.deepcopy(original_agent)
    knowledge["id"] = "knowledge_agent"
    knowledge["key"] = "knowledge_agent"
    knowledge["parameters"]["systemPrompt"] = {
        "kind": "template",
        "segments": [
            {"kind": "text", "text": "必须先实际调用 knowledge_search 工具检索用户的问题,只能使用真实检索结果回答。"}
        ],
    }
    knowledge["resourceReferences"] = [
        item for item in knowledge["resourceReferences"] if item["resourceType"] != "memory"
    ]
    knowledge["parameters"]["userQuestion"] = {
        "kind": "literal",
        "value": "请调用 knowledge_search 查询水星计划的唯一发布代号,准确回答代号。",
    }
    memory = copy.deepcopy(original_agent)
    memory["id"] = "memory_agent"
    memory["key"] = "memory_agent"
    memory["parameters"]["systemPrompt"] = {
        "kind": "template",
        "segments": [
            {"kind": "text", "text": TOOL_INSTRUCTION + "本节点第一步必须调用 memory_write,成功后回复写入的项目代号。"}
        ],
    }
    memory["resourceReferences"] = [item for item in memory["resourceReferences"] if item["resourceType"] != "rag"]
    marker = f"pipeline-memory-{run_id}"
    memory["parameters"]["userQuestion"] = {
        "kind": "literal",
        "value": f"请调用 memory_write 保存事实:我的私人项目代号是 {marker}。成功后准确回复该代号。",
    }
    profile = post(
        control,
        "/sandbox-profiles",
        {
            "name": f"P7 pipeline sandbox {run_id}",
            "description": "Real Agent to Python",
            "ownerDepartmentId": live_providers["control"].get("/api/v1/auth/me").json()["departmentId"],
            "runner": "python",
            "imageDigest": _sandbox_image_digest(),
            "cpuMillis": 500,
            "memoryBytes": 536870912,
            "pidsLimit": 256,
            "diskBytes": 1073741824,
            "timeoutSeconds": 120,
            "outputLimitBytes": 1048576,
            "networkPolicy": {"defaultAction": "deny", "egressMode": "none"},
        },
    )
    code = {
        "id": "python",
        "key": "python",
        "type": "code",
        "typeVersion": 1,
        "name": "Verify real Agent outputs in Python",
        "parameters": {
            "runner": "python",
            "inputs": {
                "kind": "object",
                "fields": {
                    "knowledge": _reference(knowledge["id"], ["text"]),
                    "memory": _reference(memory["id"], ["text"]),
                },
            },
            "source": f"def main(**inputs):\n    assert {live_indexed_document['marker']!r} in inputs['knowledge']\n    assert {marker!r} in inputs['memory']\n    print('p7-real-agent-python-ok')\n    return {{'answer': inputs['knowledge'] + '\\n' + inputs['memory'], 'verified': True}}",
            "outputExample": {"answer": "", "verified": True},
            "networkPolicy": {"mode": "deny", "destinations": []},
        },
        "contextWrites": [],
        "resourceReferences": [{"resourceType": "sandbox_profile", "resourceId": profile["id"], "operation": "use"}],
        "settings": {"timeoutMs": 180000},
    }
    exit_node = copy.deepcopy(next(node for node in definition["nodes"] if node["type"] == "exit"))
    exit_node["parameters"]["outputs"] = {"answer": _reference("python", ["structuredOutput", "answer"])}
    definition["nodes"] = [knowledge, memory, code, exit_node]
    definition["settings"]["timeoutMs"] = 480000
    ids = ["__start__", knowledge["id"], memory["id"], code["id"], exit_node["id"]]
    definition["connections"] = [
        {
            "id": f"pipeline-{i}",
            "sourceNodeId": start,
            "sourceHandle": "main",
            "targetNodeId": end,
            "targetHandle": "main",
            "order": 0,
        }
        for i, (start, end) in enumerate(pairwise(ids))
    ]
    workflow = post(control, "/workflows", {"name": f"P7 combined pipeline {run_id}", "visibility": "company"})["id"]
    _authorize_definition(control, workflow, definition)
    version = _save_version(control, workflow, definition)
    app = publish_application(control, workflow, version["id"], f"p7-pipeline-{run_id}")
    with httpx.Client(
        base_url=service_urls["runtime"], headers={"Authorization": f"Bearer {source['token'].value}"}, timeout=180
    ) as gateway:
        result, answer = _ask(gateway, app, "组合工作流", "请执行知识检索和记忆写入,将真实结果交给 Python 验证。")
        assert live_indexed_document["marker"] in answer and marker in answer, answer
    execution = result["executionId"]
    nodes = get(control, f"/executions/{execution}/nodes")["items"]
    output = next(node for node in nodes if node["nodeType"] == "code")["output"]["main"][0]["json"]
    assert (
        output["exitCode"] == 0
        and output["structuredOutput"]["verified"] is True
        and "p7-real-agent-python-ok" in output["stdout"]
    ), output
    for kind in ("rag", "memory"):
        calls = _runtime_mysql(
            installed_agentx,
            f"SELECT COUNT(*) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{execution}') AND call_kind='{kind}' AND status='succeeded';",
        )
        assert int(calls) >= 1, (kind, calls)
    _assert_written(installed_agentx, result)
    deadline = time.monotonic() + 60
    while True:
        runtime = get(control, f"/executions/{execution}/runtime-details")
        if runtime["sandboxes"] and all(item["status"] == "terminated" for item in runtime["sandboxes"]):
            break
        assert time.monotonic() < deadline, runtime
        time.sleep(0.5)
    deadline = time.monotonic() + 60
    while True:
        trace = get(control, f"/executions/{execution}/trace")
        traced_resources = {span.get("resourceType") for span in trace["spans"]}
        if {"rag", "memory"}.issubset(traced_resources):
            break
        assert time.monotonic() < deadline, traced_resources
        time.sleep(0.5)
    write_report(
        installed_agentx,
        "product/live-agent-python-pipeline.json",
        {
            "status": "passed",
            "executionId": execution,
            "ragAndMemoryInSameExecution": True,
            "pythonCheckedUpstreamValues": True,
            "memoryWritePersisted": True,
            "ragAndMemoryTraceVerified": True,
            "stdoutMarker": "p7-real-agent-python-ok",
            "sandboxesTerminated": True,
            "traceSpanCount": len(trace["spans"]),
        },
    )
