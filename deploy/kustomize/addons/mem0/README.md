# Mem0 Addon

该目录是可选 Kustomize Addon，不属于四个 Agentx 核心 Helm Release。它部署 Mem0 `2.0.15`、PostgreSQL 和两个 PVC；Provider 地址与模型由 `agentx-mem0-config` 提供，Provider API Key、PostgreSQL 密码和 JWT Secret 位于 `agentx-mem0-secrets`。

E2E 组合清单会生成仅供测试使用的 ConfigMap 与 Secret，并通过 `kubectl apply -k deploy/kustomize/e2e-fixtures/runtime-providers` 显式安装。非测试环境必须在应用本目录前提供同名 ConfigMap 与 Secret；外部 Mem0 由其所有者独立部署，Agentx 只通过 UI/API 创建 Credential、Connection 和 Grant。

升级使用同一条 `kubectl apply -k` 命令。卸载使用 `kubectl delete -k`，默认不会删除 `mem0-history-data` 与 `mem0-postgres-data` PVC；数据删除必须由操作者单独确认并执行。核心 `agentxctl install|upgrade|uninstall` 不隐式管理本 Addon。

真实模型 E2E 使用 `deploy/docker/mem0.Dockerfile` 派生的隔离镜像。该镜像在固定基础镜像中启用已安装的 `openai_structured` 适配器，不新增依赖；适配器显式发送非流式请求，适用于省略 stream 时默认返回 SSE 的 GLM 接口。源环境的 Mem0 镜像及配置保持原值。

本地 `cargo xtask images` 会构建并导入该测试镜像，也可用 `--service mem0-server` 单独构建。将实际 OCI digest 写入 `AGENTX_E2E_MEM0_IMAGE`，与 `AGENTX_E2E_CPU_EMBEDDING_IMAGE` 一起传给 `pytest tests/e2e`；测试结束删除隔离 Namespace。
