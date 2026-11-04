# 代码仓库架构

## 1. Monorepo

Agentx 使用单仓库管理 Rust 服务、公共 Crate、Rust `agentxctl`/`xtask`、前端、容器、分域 Migration、Helm Chart、可选 Kustomize Addon 和 Python系统验收工具。

```text
Agentx/
├── src/
│   ├── web/
│   ├── services/{platform-control,agentx-v2-runtime,agentx-egress-gateway,observability}/
│   ├── crates/                       # 领域、协议、应用、基础设施与部署操作实现
│   └── plugins/
│       ├── packages/{plugin-sdk,plugin-ui,plugin-runner}/
│       ├── builtin/{core,data,http}/
│       └── templates/canvas-plugin/
├── contracts/{openapi,schemas,sdk,vendor}/
├── deploy/
│   ├── docker/
│   ├── helm/
│   ├── kustomize/
│   ├── opensandbox/
│   ├── ingress-nginx/
│   ├── values/
│   ├── release/
│   └── migrations/{control,runtime,observability}/
├── tools/{agentxctl,agentx-boundary-check,xtask,scripts}/
├── tests/
│   ├── acceptance/
│   ├── e2e/                          # pytest 系统级编排入口
│   ├── browser/                      # Playwright 业务测试与图片基准
│   └── fixtures/{backup-adapter,echo-mcp,echo-node}/
└── docs/
```

Rust、pnpm、uv 工作区清单及锁文件保留根目录。仓库自动化证据位于 `.local/artifacts`，发布包位于 `.local/dist`，镜像导入归档位于 `.local/tmp/images`；工具默认的 `target`、`node_modules`、`.venv` 保留原位置。详细目录映射见 [仓库目录归并计划](plan/repository-layout.md)。


V1 `platform-api`、`trigger-gateway`、`workflow-coordinator`、`trace-writer`、旧 Worker/Sandbox 服务、`agentx-runtime-rpc`、共享 Infrastructure 和 `migrations/mysql` 已物理删除。

## 2. 后端构建产物

### platform-control

唯一承载 `/api/v1`，以 `api,publisher,projector,retention` Role 处理 IAM、Workflow、资源、应用发布、治理投影和 Runtime/Observability BFF。它访问 Control MySQL/OSS、只读 Vault、冻结 Internal API，以及 Dependencies 中 Hydra 的受限管理端点；不访问 Hydra 数据库。

### agentx-v2-runtime

一个 Package 提供四个 Runtime 二进制：

- `runtime-gateway`：`/gateway/v1`、Session/Message/Invocation、应用批次、Webhook、SSE、Cancel/Resume、Runtime Query、已发布业务页/任务、应用 MCP 与 Hydra 公共协议代理/令牌复核。
- `workflow-runtime`：Coordinator、Trigger、Command、Outbox、Recovery、Artifact、Quota 和 Trace Relay Role。
- `workflow-worker`：按 Capability 消费任务，使用 Attempt Lease/Fencing 执行 Node、Resource 和 Provider 调用。
- `sandbox-manager`：OpenSandbox 生命周期、短凭据、Reaper、TTL 和 Provider 对账。

四个二进制只使用 Runtime MySQL/Redis/OSS、只读 Vault 和获准 Provider/OpenSandbox，不访问 Control Repository 或数据。

批次领域位于 `agentx-v2-runtime/src/application_batches/`，分离原子受理、只读聚合、项目许可、命令派发、停止/取消/派生、CSV 和保留；`gateway/batches.rs` 只负责公开入口与来源授权，`internal.rs` 接收绑定请求摘要的管理委托。单项和批次共用 `application_caller.rs` 当前授权、`task_view.rs` 权威状态及 `invocation_cancel.rs` 原取消逻辑；没有第二套运行引擎或网关依赖反向进入领域。

Control 的 `application_batches_api.rs` 经现有 Runtime BFF 校验执行查看/取消/运行权限，并保留实际管理人身份。Web 的 `features/applications/batches/` 使用共享组件承载调用记录子页；`ui/page-batch-live.ts` 管理可信提交意图，`page-batch-preview.ts` 与 `page-batch-checks.ts` 只负责隔离预览和创作检查。SDK 版本 2 与 DTO/Schema/OpenAPI/生成 TS 同步，删除旧 SDK 源定义，已发布历史对象仍受正常引用与保留规则保护。

### agentx-egress-gateway

独立 Rust 二进制，提供 Runtime `3128` 明文集群内 CONNECT、Sandbox `3129` TLS CONNECT、`8080` 健康检查和 `9092` 低基数指标。它强制 KID/角色绑定、Runtime Token 单次消费、Sandbox Token 并发/次数/累计时长预算，以及解析后全地址校验和固定 IP 连接。它只依赖跨服务 JWT 契约、DNS/TCP/TLS 与 Service Kit，不依赖任何业务 Repository 或数据凭据。

### observability

以 `trace-consumer,query` Role 消费 Runtime Trace Stream、写入 ClickHouse并提供内部 Trace/成本/聚合查询。它不持有 Control 或 Runtime MySQL Credential。

## 3. 公共 Crate

### 纯领域与协议

- `agentx-domain`：纯领域对象和值类型，不依赖 Axum、SQLx、Redis 或 ClickHouse。
- `agentx-runtime`：Workflow Item、图、表达式、包 Manifest Registry、有效契约、编译IR和状态机算法；不依赖具体存储。`src/plugins/builtin/{core,data,http}` 的 `manifest.json` 与节点 JSON 是内置业务 Manifest 的权威来源，Registry 只负责装载、校验和绑定 Rust 原生能力，不在 Rust 中重复构造参数/UI Schema。
- `agentx-node-protocol`：版本化 Node Manifest、Action/Lifecycle、Item/Lineage、Artifact、Credential Handle 和错误 DTO。
- `agentx-runtime-contracts`：跨面 Bundle、Work Package、Command/Event、Internal API、Definition/Manifest、IR、Worker Protocol 和 JWT DTO；使用严格版本与未知字段拒绝，并由同一生成管线写入 `contracts/schemas/runtime-v1` 与OpenAPI调用类型。
- `agentx-api-types`：公共 HTTP DTO，不包含数据库 Row。

### 应用与基础设施

- `agentx-application`：存储无关的业务 Port/Use Case。
- `agentx-bundle-builder`：确定性 Bundle/Work Package/对象闭包构建和签名。
- `agentx-control-infrastructure`：Control MySQL/OSS、Credential、Artifact 和 Outbox Adapter。
- `agentx-runtime-infrastructure`：Runtime MySQL/Redis/OSS、Vault 和 Provider Adapter。
- `agentx-mysql-lease`：数据库 UTC 时间、Pod UID Owner、Lease、Heartbeat、Fencing 和 `SKIP LOCKED` 公共语义。
- `agentx-service-kit`：配置、结构化日志、Live/Ready/Drain、指标和优雅退出。
- `agentx-v2-ops`：Migration、Bootstrap、Doctor 和 Key 工具。
- `agentx-boundary-check`：Cargo、SQL、Env、Secret、NetworkPolicy、Provider HTTP Client、V1 残留和 2000 行静态门禁；面向公网的 Runtime 模块禁止重新使用裸 `reqwest::Client::new()`。

## 4. 依赖方向

```text
agentx-domain / agentx-node-protocol
                ↑
      agentx-runtime-contracts
          ↑              ↑
 agentx-runtime   agentx-application
          ↑              ↑
 runtime-infra      control-infra
          ↑              ↑
 Runtime services   platform-control

observability → runtime-contracts / ClickHouse client
```

约束：

- 服务之间不得通过 Cargo 直接依赖，也不能共享 Repository 绕过 Internal API。
- Domain/Runtime Kernel 不依赖数据库、Redis、ClickHouse 或服务代码。
- Control Infrastructure 不依赖 Runtime Infrastructure、Redis 或 ClickHouse；Runtime Infrastructure 不依赖 Control Infrastructure。
- SQL Row、Settings、Repository 和数据库凭据不跨 Plane 导出。
- 跨面只共享版本化 Contracts；Control→Runtime/Observability 使用短期 RS256 Service/Delegation JWT。
- Runtime 不主动调用 Control，Observability 不持有任何 MySQL Credential。

## 5. 服务生命周期和配置

六类后端应用统一提供：

```text
GET  /health/live
GET  /health/ready
POST :9091/health/drain
GET  :9092/metrics
```

普通配置来自分面 ConfigMap，敏感配置来自工作负载 Secret，环境变量统一使用 `AGENTX_` 前缀。Drain 后立即摘除 Readiness并停止新写/新 Claim；已领取工作在限定时间内 Heartbeat，Kubernetes 终止宽限为 60 秒。

## 6. 文件组织与门禁

服务按 Auth/IAM、Workflow、Resource、Application、Evaluation、Governance、Runtime Engine、Query 和 Provider 等领域拆分模块。Workflow Compiler引用校验、Control Webhook以及Agent Runtime的Budget/State/Model/Trace均按该职责拆分。任何前端或后端生产源文件不得超过 2000 行；生成文件和 Migration 必须进入明确允许列表。

工作流与网页创作的通用持久驱动位于 Control `design_agent_*`，领域工具分别位于 `studio_builder_*` 与 `application_ui_*`，共用 `agentx-agent-core`，没有第二套 Agent loop。页面发布、应用 MCP、OAuth 和工具审批按领域模块拆分；`contracts/sdk` 保存固定发布的受限 DOM SDK/样式。Hydra 是 Dependencies 的外部基础设施，不新增自研 OAuth 服务或设计态运行服务。

Workflow 业务 Agent 通过 Runtime `worker_runtime_agent_*` 驱动同一个 Core，模型与附件来自签名 Bundle；设计态使用平台模型与目标专属工具。2026-11-03 的[整体执行方案](plan3/01-target-architecture.md#13-统一自主-agent-执行方案2026-11-03-调整)明确模型自主选择工具与修复步骤。[外层浏览器创作实现](plan3/02-design-browser-agent.md)复用 `agent_model_protocol` 的多模态投影，浏览器执行经 Sandbox Manager/OpenSandbox；Core 仅持久化带来源的观察部分，不引入浏览器、Provider 或存储客户端。没有新增固定业务流水线或另一套 Agent Runtime；验收状态见实施文档。

Runtime 的 Hydra 公共代理与 introspection 适配器是固定内网基础服务调用，与现有 Vault/OpenSandbox 适配器一同登记到 HTTP 边界检查白名单；端点来自部署设置，公共路径有明确允许列表，禁止重定向和环境代理。业务 Provider HTTP 继续统一经过 `ProviderHttpClient`。

当前完成事实和数据访问图见 [当前架构、服务与数据访问图](13-architecture-service-data-map.md)，本地收口证据见 [V2-08A 验收](planv2/evidence/v2-08.md)。生产容量、安全与恢复认证仍属于 V2-08B。
