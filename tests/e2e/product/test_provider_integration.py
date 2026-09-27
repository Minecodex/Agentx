# ruff: noqa: S608
"""Provider integration E2E: LightRAG knowledge, Mem0 memory and OpenSandbox sandbox.

对真实 Provider 容器验证完整的「UI 接入 → 资源授权 → Workflow 绑定 → 执行 → Trace」链路:

1. ``installed_agentx`` + ``e2e_providers`` 在临时 Namespace 部署 Agentx 与
   LightRAG/Mem0/RAGFlow/echo Provider(Kustomize fixture);RAGFlow 默认随
   fixture 部署(容量窗口可用 ``AGENTX_E2E_RAGFLOW_DISABLE=1`` 显式关闭);
2. OpenSandbox Server(``http://host.docker.internal:18080``,API Key
   ``agentx-local-opensandbox-key``):``opensandbox_server`` fixture 在未运行时
   通过 uvx 以固定 0.2.2 版本自动拉起,已运行则直接复用;
3. Playwright 套件 ``tests/provider-integration.spec.ts`` 驱动真实 Web Console 完成
   知识库/记忆连接创建、测试连接、沙箱配置、Workflow 设计器绑定与调试执行,并断言
   rag/memory runtime_call 与 sandbox Trace span;
4. 长期记忆成功路径在 Application Session 内两轮对话验证(写入 → 召回 → 审计)。

前置版本要求:Agent 长期记忆/知识槽位绑定与沙箱 Digest 表单修复必须已包含在安装镜像中
(v0.0.4-beta 之前的镜像存在槽位版本校验与镜像格式校验两处缺陷,套件会失败)。

可选环境变量:

- ``AGENTX_E2E_SANDBOX_UI_CREATE=1`` 通过 UI(而不是 API)创建沙箱配置;
- ``AGENTX_E2E_RAGFLOW_DISABLE=1`` 不部署 RAGFlow fixture(协议用例随之 skip,
  供容量 Run 等独占窗口使用;默认部署以保证 skipped=0);
- ``AGENTX_E2E_RAGFLOW_BASE_URL`` 等变量由 conftest fixture 自动注入,无需手工配置。
"""

from __future__ import annotations

import json
import os
import time
from pathlib import Path
from typing import Any

import httpx
import pytest

from tests.e2e.runtime.test_agent_attachments import (
    _access_token,
    _run_fixture_job,
    _runtime_mysql,
    _wait_admission_outbox,
)
from tests.e2e.support import run, run_playwright

OPENSANDBOX_HEALTH = "http://127.0.0.1:18080/health"
OPENSANDBOX_API_KEY = "agentx-local-opensandbox-key"
SANDBOX_IMAGE_TAG = "opensandbox/code-interpreter:v1.1.0"


def _require_opensandbox() -> None:
    try:
        response = httpx.get(
            OPENSANDBOX_HEALTH,
            headers={"Open-Sandbox-Api-Key": OPENSANDBOX_API_KEY},
            timeout=5,
        )
    except httpx.HTTPError as error:
        response = None
        reason = str(error)
    if response is None or response.status_code != 200 or response.json().get("status") != "healthy":
        pytest.fail(
            "OpenSandbox Server 未在 18080 端口就绪(install doctor 的 opensandbox-health 检查同样依赖它)。"
            "opensandbox_server fixture 拉起失败;请查看本次 run 的 opensandbox-server*.log 证据文件,"
            f"或按 deploy/opensandbox/README.md 手工启动。探测结果:{reason if response is None else response.status_code}"
        )


def _sandbox_image_digest() -> str:
    run(("docker", "pull", SANDBOX_IMAGE_TAG), timeout=600)
    inspect = run(
        ("docker", "inspect", SANDBOX_IMAGE_TAG, "--format", "{{index .RepoDigests 0}}"),
        timeout=60,
    )
    digest = inspect.stdout.strip()
    if "@sha256:" not in digest:
        pytest.fail(f"无法解析 {SANDBOX_IMAGE_TAG} 的镜像摘要:{digest}")
    return digest


def _ragflow_environment(e2e_providers: dict[str, str]) -> dict[str, str]:
    environment: dict[str, str] = {}
    if "ragflow" in e2e_providers:
        environment["AGENTX_E2E_RAGFLOW_BASE_URL"] = e2e_providers["ragflow"]
        environment["AGENTX_E2E_RAGFLOW_ALIAS_BASE_URL"] = e2e_providers["ragflow_alias"]
        environment["AGENTX_E2E_RAGFLOW_API_KEY"] = e2e_providers["ragflow_api_key"]
        environment["AGENTX_E2E_RAGFLOW_DATASET_ID"] = e2e_providers["ragflow_dataset_id"]
    return environment


@pytest.mark.cluster
@pytest.mark.product
def test_provider_integration_suite(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    e2e_providers: dict[str, str],
    opensandbox_server: None,
) -> None:
    _require_opensandbox()
    environment = os.environ.copy()
    environment["AGENTX_E2E_RUN_ID"] = installed_agentx["run_id"]
    environment["AGENTX_E2E_STAGE"] = "helm-agentxctl"
    environment["AGENTX_E2E_BASE_URL"] = service_urls["web"]
    environment["AGENTX_E2E_ECHO_BASE_URL"] = e2e_providers["echo_mcp"]
    environment["AGENTX_E2E_LIGHTRAG_BASE_URL"] = e2e_providers["lightrag"]
    environment["AGENTX_E2E_MEM0_BASE_URL"] = e2e_providers["mem0"]
    environment.update(_ragflow_environment(e2e_providers))
    environment.setdefault("AGENTX_E2E_SANDBOX_IMAGE", _sandbox_image_digest())
    run_playwright(
        Path(installed_agentx["root"]), "provider-integration", ("tests/provider-integration.spec.ts",), environment
    )


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
        # Publishing is asynchronous (202 Accepted): wait for the deployment
        # to converge to active instead of treating the ack as the result.
        if response.status_code == 202:
            return _wait_application_deployment(
                client,
                headers,
                application_id,
                response.json()["id"],
            )
        last_error = f"{response.status_code} {response.text}"
        time.sleep(3)
    raise AssertionError(f"publish deployment failed: {last_error}")


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
@pytest.mark.product
def test_provider_long_term_memory_write_recall(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    e2e_providers: dict[str, str],
    run_id: str,
) -> None:
    """Application Session 内两轮对话:第一轮写记忆,第二轮召回,并断言审计。

    与浏览器套件互补:浏览器主用例覆盖了 UI 建连接/绑定与「无 subject 时拒绝」,
    本用例补齐 provider-integration 套件的长期记忆成功路径(受 subject 作用域约束)。
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
            job_suffix="provider-memory",
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
            json={"name": f"provider memory {run_id}"},
        )
        assert api_key_response.status_code == 201, api_key_response.text
        api_key = api_key_response.json()["secret"]
        _wait_admission_outbox(installed_agentx)
        _publish_application_deployment(
            control, headers, application_id, fixture["workflowVersionId"], environment["id"]
        )

    with httpx.Client(base_url=service_urls["runtime"], timeout=60) as gateway:
        session_headers = {"Authorization": f"Bearer {token}"}

        def create_session(index: int) -> str:
            response = gateway.post(
                f"/gateway/v1/applications/{fixture['applicationSlug']}/sessions",
                headers={
                    **session_headers,
                    "Idempotency-Key": f"provider-memory-session-{run_id}-{index}",
                },
                json={
                    "title": f"provider memory session {index}",
                    "externalUserId": "00000000-0000-7000-8000-00000000beef",
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
                    "Idempotency-Key": f"provider-memory-{run_id}-{index}",
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

    calls_json = _runtime_mysql(
        installed_agentx,
        "SELECT JSON_ARRAYAGG(JSON_OBJECT('executionId',BIN_TO_UUID(execution_id),'callKind',call_kind,'status',status)) "
        f"FROM runtime_calls WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') "
        f"AND execution_id IN (UUID_TO_BIN('{first['executionId']}'),UUID_TO_BIN('{second['executionId']}'));",
    )
    calls = json.loads(calls_json) if calls_json and calls_json.lower() != "null" else []
    assert sum(call["callKind"] == "memory" for call in calls) >= 2, calls
    assert all(call["status"] == "succeeded" for call in calls if call["callKind"] == "memory"), calls
    audit_json = _runtime_mysql(
        installed_agentx,
        "SELECT JSON_ARRAYAGG(JSON_OBJECT('operation',operation)) "
        "FROM agent_subject_memory_audit "
        f"WHERE tenant_id=UUID_TO_BIN('{me['companyId']}') AND application_id=UUID_TO_BIN('{application_id}') "
        f"AND authenticated_subject_id=UUID_TO_BIN('{me['id']}') "
        "AND operation IN ('write','recall');",
    )
    audits = json.loads(audit_json) if audit_json and audit_json.lower() != "null" else []
    assert {item["operation"] for item in audits} >= {"write", "recall"}, audits
