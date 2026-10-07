"""Provider integration E2E: LightRAG knowledge, Mem0 memory and OpenSandbox sandbox.

对真实 Provider 容器验证完整的「UI 接入 → 资源授权 → Workflow 绑定 → 执行 → Trace」链路：

1. ``installed_agentx`` + ``e2e_providers`` 在临时 Namespace 部署 Agentx 与
   LightRAG/Mem0/echo Provider（Kustomize fixture）；
2. 主机上的 OpenSandbox Server（``http://host.docker.internal:18080``，API Key
   ``agentx-local-opensandbox-key``，与 agentxctl 生成的 runtime secret 一致）必须已经启动，
   否则本测试直接失败并给出启动指引；
3. Playwright 套件 ``tests/provider-integration.spec.ts`` 驱动真实 Web Console 完成
   知识库/记忆连接创建、测试连接、沙箱配置、Workflow 设计器绑定与调试执行，并断言
   rag/memory runtime_call 与 sandbox Trace span。

前置版本要求：Agent 长期记忆/知识槽位绑定与沙箱 Digest 表单修复必须已包含在安装镜像中
（v0.0.4-beta 之前的镜像存在槽位版本校验与镜像格式校验两处缺陷，套件会失败）。

可选环境变量：

- ``AGENTX_E2E_SANDBOX_UI_CREATE=1`` 通过 UI（而不是 API）创建沙箱配置；
- ``AGENTX_E2E_RAGFLOW_BASE_URL`` + ``AGENTX_E2E_RAGFLOW_API_KEY`` + ``AGENTX_E2E_RAGFLOW_DATASET_ID``
  额外执行 RAGFlow 协议适配器用例（端点需要以 `ragflow` 为首标签的集群内 Service 暴露，
  并把 `ragflow` 加入 values 的 ``global.network.egressGateway.httpProviderServices``，
  Service 端口需在 8080/8081/8090/9621/8000 白名单内，Pod 打上
  ``agentx.io/runtime-provider: allowed``、Namespace 打上 ``agentx.io/plane: dependencies``）；
- ``AGENTX_E2E_RAGFLOW_REJECTED_BASE_URL`` 覆盖「非白名单明文 HTTP 端点被 egress 拒绝」用例
  的目标地址（默认 ``http://host.docker.internal:19380``）；
- ``AGENTX_E2E_RAGFLOW_ALIAS_BASE_URL`` 额外执行「无适配器时 LightRAG 协议无法命中 RAGFlow」
  的历史对照用例。
"""

from __future__ import annotations

import os
from pathlib import Path

import httpx
import pytest
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
            "OpenSandbox Server 未在 18080 端口就绪（install doctor 的 opensandbox-health 检查同样依赖它）。"
            "请按 deploy/opensandbox/README.md 启动：api_key=agentx-local-opensandbox-key、"
            f"runtime=docker、execd_image=opensandbox/execd:v1.0.21。探测结果：{reason if response is None else response.status_code}"
        )


def _sandbox_image_digest() -> str:
    run(("docker", "pull", SANDBOX_IMAGE_TAG), timeout=600)
    inspect = run(
        ("docker", "inspect", SANDBOX_IMAGE_TAG, "--format", "{{index .RepoDigests 0}}"),
        timeout=60,
    )
    digest = inspect.stdout.strip()
    if "@sha256:" not in digest:
        pytest.fail(f"无法解析 {SANDBOX_IMAGE_TAG} 的镜像摘要：{digest}")
    return digest


@pytest.mark.cluster
@pytest.mark.product
def test_provider_integration_suite(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    e2e_providers: dict[str, str],
) -> None:
    _require_opensandbox()
    environment = os.environ.copy()
    environment["AGENTX_E2E_RUN_ID"] = installed_agentx["run_id"]
    environment["AGENTX_E2E_STAGE"] = "helm-agentxctl"
    environment["AGENTX_E2E_BASE_URL"] = service_urls["web"]
    environment["AGENTX_E2E_ECHO_BASE_URL"] = e2e_providers["echo_mcp"]
    environment["AGENTX_E2E_LIGHTRAG_BASE_URL"] = e2e_providers["lightrag"]
    environment["AGENTX_E2E_MEM0_BASE_URL"] = e2e_providers["mem0"]
    environment.setdefault("AGENTX_E2E_SANDBOX_IMAGE", _sandbox_image_digest())
    run_playwright(
        Path(installed_agentx["root"]), "provider-integration", ("tests/provider-integration.spec.ts",), environment
    )
