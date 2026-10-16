# 系统架构

## 1. 业务分层

### Workflow Studio

面向设计者和测试者：

- Workflow 画布
- 节点配置面板
- 表达式编辑器
- 手动执行
- 单节点执行
- Pin Data
- 执行路径高亮
- Trace 和 Checkpoint 查看
- Playground
- Dataset 和评测报告

### Workflow 控制面

管理静态配置和业务资源：

- Tenant、Department、User、Role
- Workflow、Draft、Version、Deployment
- Node Definition 和 Node Version
- Model、Tool、Skill、RAG、Memory、Credential
- Application、API Key 和 Trigger 草稿/发布配置
- Approval、Notification、Evaluation 和 Retention 的治理投影
- Dataset、Evaluation Profile Version、Evaluation Run
- Node Catalog、Manifest Version 和动态 Provider

### Workflow 运行面

处理所有运行状态：

- Runtime Gateway
- Workflow Runtime Coordinator/Trigger/Command/Recovery/Quota/Trace Relay
- Capability Queue 和 Workflow Worker
- Resource Runtime 和 Node Runner
- Sandbox Manager
- Runtime Query、Event Export 和 Trace Outbox

### 数据层

- MySQL：权威业务数据和运行状态
- Redis：任务派发、缓存、租约、短期状态和限流
- ClickHouse：Workflow Trace 和分析明细
- MinIO 或 S3：文件、二进制输出、大型节点结果、Checkpoint Payload
- OpenSandbox：独立安装的不可信代码和高风险工具运行 Provider；本地使用 Docker Runtime，Kubernetes RuntimeClass 强隔离作为后续生产强化

## 2. 推荐部署单元

### web-console

承载管理控制台、Workflow Studio、Playground、Trace 和评测页面。

### platform-control

控制面模块化服务，包含：

- IAM
- Workflow 管理
- 资源管理
- 应用发布配置
- 审批、通知和运行治理投影
- 测试集和报告
- Runtime/Observability BFF

### runtime-gateway

负责：

- Application API
- Webhook
- SSE 连接
- 请求认证
- 输入 Schema 校验
- 幂等键
- 创建 Session、Message、Invocation 和 Runtime Command

### workflow-runtime

负责：

- 创建 Execution
- 固化 Execution Snapshot
- 计算初始节点
- 推进节点状态
- Join 和 Loop 计算
- Wait 和 Approval 恢复
- 超时、取消和故障任务回收
- Schedule/Poll/Lifecycle Trigger、Outbox、Artifact、Quota 和 Trace Relay

### workflow-worker

负责：

- 消费节点任务
- 取得节点租约
- 解析表达式
- 加载 Credential
- 执行具体 Node Runner
- 保存节点结果
- 创建 Checkpoint
- 发送 Trace Event

### sandbox-manager（可选）

通过稳定的 Agentx 内部协议管理 Sandbox 生命周期、资源配额、输入输出和短期凭证注入。它使用 Rust `OpenSandboxAdapter` 直接调用 OpenSandbox Lifecycle REST API 和 execd REST/SSE API；Worker 不直接依赖 OpenSandbox SDK、OpenAPI 生成 DTO 或私有对象。

`sandbox-manager` 是 OpenSandbox API Key 和 execd 短期 Token 的唯一持有者。生产调用链固定为 `workflow-worker -> sandbox-manager -> OpenSandbox`，不增加 Go/Python Sidecar 或 CLI 子进程；官方 Go SDK 和 `osb` CLI 只允许作为测试环境的差分契约基准。这样保留单一 Rust 构建、部署、监控和取消链路，同时把供应商协议变化限制在基础设施 Adapter 内。

Sandbox disabled 时不部署该服务。remote 模式下一个逻辑 Manager 可多副本共享 MySQL Lease并连接一个 OpenSandbox Lifecycle Endpoint；OpenSandbox 再按会话创建任意多个运行实例。本阶段不实现多 Provider 容量调度。

### observability

Trace Consumer 从受限 Runtime Redis Stream 批量写入 ClickHouse，Query Role 提供 Trace、成本和聚合查询。Observability 不持有任何 MySQL Credential；ClickHouse 暂时不可用时，Workflow 执行不能因此失败。

### agentx-egress-gateway

部署在 Dependencies Namespace 的安全基础设施，为 `runtime-gateway`、`workflow-runtime`、`workflow-worker` 和显式开启联网的 Sandbox 提供统一 HTTPS CONNECT 出口。Gateway 使用目标绑定的短期 RS256 JWT，KID 前缀与调用角色强绑定，并支持新旧公钥重叠轮换；逐次解析并固定公共 IP，只允许 Profile 明确登记的 HTTPS 端口，任一 DNS 结果落入阻断范围即拒绝整次连接。Sandbox Token 另受单 Token 并发、连接次数、累计 Tunnel 时长和 Sandbox TTL 限制。私网、Pod/Service CIDR、Kubernetes API、Metadata、回环、链路本地和保留地址始终拒绝。Gateway 不持有 MySQL、Redis、Vault、OSS 或 Provider Credential，也不解析或记录 TLS 请求正文。

### 指标与扩缩容责任

Agentx 后端常驻服务在独立的 `9092` 端口暴露低基数 Prometheus 文本格式指标，Kubernetes `*-metrics` Service 只提供集群内抓取入口。监控组件所在 Namespace 必须显式添加 `agentx.io/metrics-access=true` 标签才能通过 NetworkPolicy 抓取。Agentx 不安装或管理 Prometheus、Prometheus Adapter、Metrics Server，也不创建 HPA、KEDA `ScaledObject` 或其他自动扩缩容器。

指标抓取、长期存储、告警和扩缩容策略属于用户 Kubernetes 平台的责任。用户可以手工调整 Deployment 副本数，也可以使用自建 Prometheus、云监控、HPA、KEDA 或自定义控制器消费 Agentx 指标。Agentx Profile 中：

- `replicas` 只定义首次安装时的初始副本数；
- `maxReplicas` 定义连接池和外部依赖容量预算上限，不会自动创建扩缩容资源；
- Upgrade/Rollback 保留 Deployment 当前副本数，避免覆盖用户或外部控制器已经调整的值；升级器仅在首次迁移时清理历史 Agentx HPA，此后不删除用户创建的 HPA/KEDA；
- 外部扩缩容不得超过 `maxReplicas`，变更上限前必须重新校验 MySQL、Redis、OSS、ClickHouse 和 Provider 容量预算。

Agentx 继续维护 Readiness/Liveness、Drain、PDB、Claim/Lease/Fencing 和优雅终止契约，使用户执行滚动发布或扩缩容时不会依赖单副本正确性。

## 3. 模块依赖规则

[plan8 多草稿与 AI 编辑](plan8/README.md) 增加 Control Studio Builder Host：复用同一个 Agent Core 承载设计态会话，通过 Runtime 专用设计态 API 调用模型；这是“Control 不执行业务 Agent loop”的明确设计态扩展，已实现。Builder 只编辑当前画布并静态校验，保存沿用编辑器链路，实际运行/测试、创建版本和部署由用户 UI 触发。业务 Worker 不修改 Draft、Runtime 不回读 Control 的边界保持；来源、账本、ACK 与恢复目标见 [plan8 架构](plan8/02-architecture-and-contracts.md)。以下条目仍描述当前已实现边界。

- Studio 只能通过 Platform Control BFF 和 Runtime Gateway 调用后端。
- Model、MCP、Memory、RAG、HTTP Request 和插件 `ctx.http/model` 的公网 HTTPS 必须使用 `ProviderHttpClient -> agentx-egress-gateway`；基础设施 Client 继续只访问固定内部依赖。
- Platform Control 不直接执行节点，也不直连 Runtime MySQL/Redis/OSS 或 ClickHouse。
- Worker 不修改 Workflow Draft。
- Execution 只能运行不可变 Snapshot：生产入口使用 Version Source，Studio 调试使用精确 Draft Revision Source；任何入口都不能运行实时变化的 Draft Head。
- Workflow Definition、Editor Document 和 Debug Overlay 分离；Compiler/Worker 只读取 Definition 和 Execution Snapshot。
- Platform API、Compiler 和 Studio 通过同一 Node Catalog 解析 Manifest；不得分别维护节点类型和参数协议。
- Scheduler 不依赖进程内状态判断工作流进度。
- Redis 中的消息不是权威状态，消费前必须校验 MySQL。
- ClickHouse 丢失或延迟不能影响 Execution 状态正确性。
- 大型 Payload 通过 Artifact Reference 传递，避免数据库行无限增长。

## 4. 编辑与执行快照边界

Studio 保存 `Definition + Editor Document`，Pin/Mock 进入独立 Debug Overlay。Platform Control 对 Draft Revision 或 Version 做权限与资源校验并生成不可变 Bundle/Work Package；Runtime Coordinator 只从 Runtime 本地制品固化 Execution Snapshot，Worker 不读取 Control Draft、Version Head 或可变 Resource Head。

```text
Draft Head --save--> Draft Revision --debug--+
                                                +--> Execution Snapshot --> Coordinator/Worker
Version ---------------------------production--+
```

Draft Debug 和 Version Execution 只在来源解析阶段不同，后续共享调度、Checkpoint、Trace、取消、恢复和 Sandbox。`execution_snapshots` 与 Execution 一对一，Node/Agent/Sandbox 继续以必填 `execution_id` 作为快照和凭证作用域；`workflow_version_id` 只在 Version Source 下存在。

## 5. 技术栈建议

结合当前 Rust 项目：

- 后端：Rust、Axum、Tokio、SQLx
- 内部通信：版本化 REST/Internal API、MySQL Inbox/Outbox/Cursor；Worker 使用 Runtime-local 协议
- 外部调用：REST、SSE、Webhook
- 前端：React、TypeScript、Tailwind CSS、Radix UI、React Flow
- 前端状态：TanStack Query、Zustand
- 表单与校验：React Hook Form、Zod
- 数据库：MySQL 8
- 队列和短期状态：Redis Streams
- Trace：ClickHouse
- 文件和大对象：MinIO 或 S3
- 沙箱：OpenSandbox；本地 Docker Runtime + runc，Kubernetes Runtime；gVisor/Kata 或等价强隔离列为后续生产强化
- 部署：Kubernetes、Kustomize；固定 Helm 只用于脚本管理的专用 ingress-nginx

plan6 发布 TypeScript/Node.js 画布插件 SDK。管理员导入审核后的不可变 ZIP；Web 动态装载包内 React ESM/CSS，Runtime Worker 通过固定 Node.js 24.20.0 Runner执行冻结源码。Control只管理包、版本、引用和发布，设计时调用也转发给Runtime，不能启动Node；Worker只读Execution Snapshot中的精确包摘要，不回查Control。Set、List和HTTP已使用同一TypeScript执行协议，Loop、Approval、Sub-workflow、Model、Agent、Code与边界继续由Rust原生语义实现。Python/JavaScript/Shell的非可信通用代码仍通过Code节点进入OpenSandbox。

## 6. 一致性边界

MySQL 事务负责：

- Execution 状态变化
- Node Execution 状态变化
- 下游节点 Ready 判定
- Checkpoint 元数据
- Outbox 事件

Outbox Dispatcher 将事务内事件投递到 Redis。即使 Redis 投递重复，Worker 也通过节点状态和 Lease 保证不会无条件重复执行。

外部副作用无法获得通用 Exactly Once 保证。平台提供：

- Idempotency Key
- At-least-once 调度
- 节点副作用标记
- 重试前确认
- Dry Run
- 补偿分支


### plan8 设计态边界

Control 的 Studio Builder Host 使用同一纯 `agentx-agent-core`，Runtime Contracts 只引用其不可变请求/响应 DTO；Core 不依赖服务或基础设施。设计态模型调用通过绑定请求摘要的最小 JWT 交给 Runtime Gateway，账本来源为 `studio_builder`，所有业务 Execution/Attempt/Agent 标识为空。七个工具不具有保存、运行、测试、授权、版本或部署能力。

设计态表直接纳入初始 DDL，其 `schema_file` 在表所有权目录明确登记；历史表处置清单不再承担新增表目录职责。边界检查继续区分历史基线与增量表，并检查声明的初始化位置。Redis 的现有 Admission 限流、SSE 唤醒及 Trace Delivery Host 纳入具名模块约束，不向 Builder 开放 Redis 权限。

Draft revision 0 与后续修订都有不可变文档和资源快照；从版本派生及资源引用未变时保留原部署、价格和凭据 Secret Version。公开执行结果使用 `DraftRevision | Version` 来源，草稿调试不显示虚构版本。画布本地命令先持久化恢复数据再 ACK，重复 ACK 必须内容相同，Undo 后不重复应用同一命令。
