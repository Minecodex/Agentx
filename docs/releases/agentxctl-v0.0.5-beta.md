# Agentx v0.0.5-beta

本版本包含 plan7 的渠道回复、模型流式、评测与运行洞察、知识库管理和真实服务验收修复。同步提供 Windows/Linux x64 的 agentxctl、归档及 SHA-256 校验文件，以及全部 11 个 Linux AMD64 集群镜像。

发布工作流使用 GitHub 托管 Ubuntu / Minikube 执行完整系统验收和两小时容量门禁，不跳过认证。Windows 保留 ctl 原生构建与打包；Linux 容器集群只运行一套 Linux E2E。验收失败时 Release 不会公开。

`agentxctl 0.0.5-beta` 内嵌对应版本的 Helm Chart、Schema 和 `kakj/agentx-*:v0.0.5-beta` 配置。下载并校验 ctl 后执行 `install`，即可部署这一版本；使用旧 ctl 不会自动安装新镜像。

## 本版本内容

- 渠道结果回复、显式回复节点与发送工具，包含投递记录、重试、死信及幂等；渠道表单按工作流输入/输出配置映射和多行回复模板。
- 模型与 Agent 的文本 SSE 增量、停止、恢复及 Trace；多模态内容透传与模型能力校验。
- 规则和 LLM Judge 评测、2–5 次运行比较、基线选择，以及成本、错误分布和节点耗时查询。
- LightRAG 文档上传、索引状态与检索，Agent 知识工具、Mem0 记忆与用户/应用/Namespace 隔离。
- 修复 Redis/API/Chat 准入、Agent 租约续租、Sandbox 冷启动和取消回收，以及递归资源依赖、固定 MCP 凭证和 Skill 权限校验。
- 修复对话字段空标题、网络失败提示与 Request ID 展示，完善列表加载、错误、空态、中英文及 Runtime 状态。
- 部署工具补齐 TLS、Ingress/NodePort、MetalLB 和 Vault 身份检查；提供 CPU Embedding 与 Docker Desktop 本地 Ingress Addon。

## 安装与升级

需要 Helm 3、kubectl、具有默认 StorageClass 的 Kubernetes 集群，以及独立安装且可从 Runtime Namespace 访问的 OpenSandbox。模型、知识、记忆及 IM 服务仍需配置真实资源与凭证。

1. 从本版本 Release 下载 Windows 或 Linux x64 单文件 ctl，并校验对应 SHA-256。
2. 新部署运行 `install`；已有部署运行新版 ctl 的 `upgrade`。使用自定义 Values 时，将镜像标签或摘要同步到本版本。
3. 执行 `status --output json` 和 `doctor`，检查工作负载、依赖与入口。

本版本仍处于开发阶段，不保证历史数据或旧协议兼容。升级前备份需要保留的数据；已有自定义插件使用 SDK API 2 构建。OpenSandbox 和外部状态依赖由其独立生命周期管理。

## 验收范围

真实 Kimi 文本/SSE/Judge、CPU Embedding、LightRAG、Mem0、OpenSandbox、资源授权、故障恢复及桌面流程已有分批专项证据，详见 [plan7 状态](../plan7/README.md)、[真实服务验收](../plan7/evidence/p7-live-provider-acceptance.md) 和 [自动化专项验收](../plan7/evidence/p7-automatic-boundary-acceptance.md)。

真实飞书/钉钉/企微回复由用户手测；图片、音频及其他模型暂缓。RAGFlow 组合联调、生产 Sandbox 强隔离、冻结硬件两小时容量及完整生产认证仍按计划保留。分批功能验收不能替代同一发布候选的生产认证。

本地 HTTPS 需要同时解析门户与 Runtime 域名，并信任 Ingress 所用 CA。Docker Desktop 双网络节点的接入说明见 [本地 Ingress Addon](../../deploy/kustomize/addons/local-ingress/README.md)。
