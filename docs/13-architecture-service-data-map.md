# 当前架构、服务与数据访问图

本文描述 V2-08A 完成后的当前代码、Migration 与本地 Kubernetes 事实。V1 运行入口、共享 Migration 和兼容路径已经物理删除；尚未完成的生产容量、安全隔离、PITR/RPO/RTO 与供应链认证见 [V2-08B 状态](planv2/README.md#6-阶段状态)。

## 1. 当前服务访问关系

| 当前构建产物 | Plane / Role | 权威数据与允许依赖 | 明确禁止 |
|---|---|---|---|
| `web-console` | Control / web | `platform-control` 公共 `/api/v1`；浏览器按运行时配置直连 Runtime `/gateway/v1` | 数据库、Redis、ClickHouse 和基础设施 Secret |
| `platform-control` | Control / api,publisher,projector,retention | Control MySQL、Control OSS、只读 Vault；Runtime/Observability Internal API | Runtime MySQL/Redis/OSS、ClickHouse 直连和 Provider Secret 明文 |
| `runtime-gateway` | Runtime / gateway | Runtime MySQL、Runtime OSS、只读 Vault；Runtime Redis 仅用于 SSE 唤醒 | Control DB/DNS/OSS；用 Redis 代替 MySQL 权威 Cursor |
| `workflow-runtime` | Runtime / coordinator,trigger,command,outbox,recovery,artifact,quota,trace-relay | Runtime MySQL、Runtime Redis、Runtime OSS | Control DB/DNS/OSS；把 Event/Redis 当当前状态来源 |
| `workflow-worker` | Runtime / capability pools | Runtime MySQL/Redis/OSS、只读 Vault、获准 Provider | Control Repository/DB/OSS 和未获准 Provider |
| `sandbox-manager` | Runtime / manager,reaper | Runtime MySQL、只读 Vault、OpenSandbox | Control 数据和普通 Worker 的 Provider 权限并集 |
| `agentx-egress-gateway` | Dependencies / security infrastructure | DNS、Profile 允许端口上的公共 HTTPS；角色绑定的 Runtime/Sandbox 当前与轮换公钥 | MySQL、Redis、Vault、OSS、Provider Credential 与任意私网目标 |
| `observability` | Observability / trace-consumer,query | 受限 Runtime Redis Trace/JTI ACL、ClickHouse、Observability OSS | 任意 MySQL Credential、Runtime OSS 和 Control 数据 |

Migration 使用三个独立一次性目标：`control-migrate`、`runtime-migrate` 和 `clickhouse-migrate`。它们只持有所属 Schema 凭据，不持有应用、Redis、OSS、Vault 或 Provider Credential。

四个逻辑 Plane/依赖分组当前映射到三个物理 Kubernetes Namespace：

| 物理 Namespace | 逻辑归属与组件 |
|---|---|
| `agentx-control` | Control：`web-console`、`platform-control`、Control MySQL 与 Migration |
| `agentx-runtime` | Runtime + Observability：四类 Runtime 服务、`observability`、Runtime MySQL、Runtime Redis、ClickHouse 与两域 Migration |
| `agentx-deps` | Dependencies + Ingress：`agentx-egress-gateway`、Vault、MinIO、专用 ingress-nginx，以及可选 OpenSandbox/Addon |

Observability 与 Runtime 只共享 Namespace，不共享数据权限：Observability 仍使用独立 ServiceAccount、Secret、Redis ACL 和 ClickHouse 账号，NetworkPolicy 只允许它访问 Runtime Redis Trace/JTI 通道和 ClickHouse，禁止 Runtime MySQL。部署输入是四个 Chart 共享的 YAML Values 契约，顶层固定为 `global`、`control`、`runtime`、`observability`、`dependencies`；旧 deployment Profile 已删除且不做兼容转换。

当前 Kubernetes 应用层有 8 类常驻 Deployment；默认 local Values 首次安装时每类 Deployment 均为 1 个副本，并各有一个 PDB。Agentx 可通过 local Values 部署 MySQL、Redis、MinIO、Vault 和 ClickHouse 等开发依赖，但不再部署 Prometheus、Prometheus Adapter 或 Metrics Server，也不创建任何 HPA/KEDA 资源。七类后端应用与 Egress Gateway 继续通过独立的 `*-metrics` Service 暴露 `9092 /metrics`；采集、告警和扩缩容均由用户平台负责。

Values 的 `replicas` 是首次安装值，当前统一为 `1`；`maxReplicas` 只用于连接池容量预算。单副本默认值用于降低开发和初始部署资源占用，不提供 Pod 级冗余，但运行时正确性仍不能依赖单副本。Upgrade/Rollback 保留当前 Deployment 副本数；用户手工扩缩容或外部 scaler 必须遵守 `maxReplicas` 预算。PDB、健康探针、Drain 和 Claim/Lease/Fencing 仍由 Agentx 清单与运行时契约保证。

## 2. 当前调用链

```text
Browser ──/api/v1──> web-console ──> platform-control
Browser / API / Webhook ──/gateway/v1──> runtime-gateway

platform-control publisher/api/projector
  ──短期 RS256 Service/Delegation JWT──> Runtime Internal API
platform-control api
  ──短期 Delegation JWT──> Observability Internal API
platform-control api
  ──runtime.plugins.design Service JWT──> runtime-gateway ──Node Runner──> resolveDefinition/Provider

runtime-gateway ──Runtime Command──> workflow-runtime
workflow-runtime ──Capability Task──> workflow-worker / sandbox-manager
workflow-worker ──冻结PluginNodeBinding──> Node.js 24 Runner ──host RPC──> Rust Egress/Model/Vault/Artifact
workflow-runtime trace-relay ──Redis Stream──> observability ──> ClickHouse

runtime-gateway / workflow-runtime / workflow-worker
  ──HTTPS CONNECT + 60 秒目标绑定 JWT──> agentx-egress-gateway ──> 公共 HTTPS
sandbox ──TLS CONNECT + TTL/并发/次数/累计时长受限 Token──> agentx-egress-gateway
```

Runtime 不主动调用 Control。Runtime 当前状态只来自 Runtime MySQL；Control 治理页面使用按 Cursor 拉取的治理投影；Trace、成本和聚合只来自 ClickHouse。Execution 创建路径通过 `ExecutionOriginV1` 把用户、部门和触发来源的执行时审计快照送入 Runtime；这些快照用于历史展示与查询，不是当前 IAM 权威数据。应用和 Workflow 仍只保存稳定 ID，公共 BFF 对当前页 ID 向 Control MySQL 批量解析当前名称。

## 3. 当前数据边界

| 数据域 | Migration 目录 | 当前 `CREATE TABLE` 数 | 权威内容 |
|---|---|---:|---|
| Control MySQL | `deploy/migrations/control` | 113 | IAM、Workflow 草稿/版本/Deployment、画布插件包/导入/版本/对象清理队列、资源元数据/授权、Application 管理、治理投影和 Control Outbox |
| Runtime MySQL | `deploy/migrations/runtime` | 111 | Route、Session/Message/Invocation、Execution/Snapshot/Attempt、Wait/Approval、Checkpoint/Fork、Runtime Call、Quota、Retention、Event/Trace Outbox |
| ClickHouse | `deploy/migrations/observability` | 4 | Trace 明细、去重冲突和 Consumer Health；不是 Runtime 当前状态权威 |
| Runtime Redis | 无业务 Migration | 不适用 | Capability/Trace Stream、Consumer Group、SSE 唤醒和快速配额；可从 MySQL 权威事实重建 |
| 三域 OSS | 独立 Bucket/Prefix/IAM | 不适用 | Control 源对象、Runtime Bundle/Artifact/Checkpoint、Observability 内容分别隔离 |

历史 133 表的 `58/34/39/2` 是 V2-00 的迁移处置输入，不是当前分域空库的表数量。共享 `migrations/mysql` 已删除，Control/Runtime 之间不存在跨库外键、JOIN 或共享 DSN。

## 4. 契约与所有权

- `platform-control` 唯一实现 `/api/v1` 并生成 `contracts/openapi/platform-api.json`。
- V2 `runtime-gateway` 唯一实现 `/gateway/v1` 并生成 `contracts/openapi/trigger-gateway.json`。
- Runtime Internal API 与 Observability Internal API 分别由 `contracts/openapi/runtime-internal-v1.json` 和 `contracts/openapi/observability-internal-v1.json` 冻结，不经公网 Ingress。
- `agentx-runtime-contracts` 是跨面 Bundle、Work Package、Command/Event、Query、Worker Protocol 和 JWT DTO 的唯一共享入口。
- Workflow Definition当前只接受8.0。Rust Registry拥有Studio内置节点Catalog；Start/Exit由设计器边界组件创建，MCP/Skill/RAG/Memory只作为Agent资源能力。数据库Manifest必须与Registry来源、版本及hash一致，子流程Manifest只从固定Workflow Version派生。
- Node Manifest当前只接受3.0；画布插件包格式为1，SDK API与Runner RPC为2。Control拥有插件包、版本、引用、导入与审计，Runtime使用IR中的不可变源码制品引用和摘要。每次插件调用独占可回收的Node进程与文件目录，按配置有界并发；画布和保存复用后端整图契约解析。Set/List/HTTP使用内置TypeScript包，其他核心控制流和Agent/Code语义继续原生执行。
- Application渠道修改只推进Control的runtime config revision；Runtime仅在下一版Application Deployment激活其冻结trigger清单。不存在绕过Deployment的即时trigger同步入口，回滚会一并恢复该版本的渠道快照。
- Execution 查询由 Runtime Query Authority 按完整过滤条件创建 Cursor 快照；Control BFF 只计算不扩张的授权范围、转发查询并批量补充 Control 当前名称，禁止页后本地过滤或逐行名称查询。
- `agentx-control-infrastructure` 与 `agentx-runtime-infrastructure` 分别拥有所属存储 Adapter；服务不能共享 Repository 绕过 Internal API。
- `agentx-mysql-lease` 统一数据库时间、Owner、Lease、Heartbeat 和 Fencing；`agentx-boundary-check` 检查 Cargo、SQL、Env、Secret、NetworkPolicy、V1 残留和 2000 行限制。

## 5. 本地已验证与生产延期

Run `20260818-final4` 已验证三域空库部署、151/151 公共 API、产品闭环、控制面离线、Runtime MySQL Fail Closed、Redis 空库重建、ClickHouse 独立终态、50 并发 Execution、200 Attempt、100 SSE 和 30 分钟稳定运行，详见 [V2-08A 证据](planv2/evidence/v2-08.md)。

以下不是当前完成事实：正式 100/500/1000 Execution 与两小时容量门禁、生产 TLS/PITR/RPO/RTO、gVisor/Kata、Role 级 ServiceAccount/Secret 隔离、Cosign/Attestation 和多租户攻击矩阵。它们保留在 V2-08B，V2 总计划因此仍为 `in_progress`。

## Studio Builder 的服务映射

Control 的 studio_builder_* 模块注入同一个 agentx-agent-core，拥有设计会话、编辑回执、租户模型授权和对象权限。Runtime Gateway 仅接受绑定来源的设计模型 Effect，复用 Vault/Egress/价格与 Quota；Sandbox Manager 使用 sandbox_design_leases 承载无资源、无网络的契约解析，同一 Workflow 静态解析批次可复用短期容器，每个节点使用新进程；批次关闭或租约超时立即清理。Workflow Worker 继续消费不可变业务快照，不读 Control 草稿、不承担 Builder 编辑任务。

## 外部知识服务边界

Control 管理连接与 `rag` 查询配置，发现真实 Dataset/workspace，提供只读外部文档浏览和查询测试。Runtime 使用冻结范围与凭证执行，外部系统维护内容与索引。`agentx-runtime-contracts::rag` 负责协议，`retrieval` 负责来源展开、并发 4、RRF、去重和逐来源状态。不存在知识上传、Artifact 源文件或索引调度器。详见 [知识连接与多来源检索](plan8/11-knowledge-connections.md)。

## 应用测试与平台 AI 评测

Control 的 application_testing、application_datasets、application_evaluation_profiles 和 application_evaluations 模块拥有应用内测试集、固定契约、不可变数据/方案版本和批次。evaluation_batches 固定平台模型、凭据版本、参数、价格与系统提示词；evaluation_runs 保持一份测试集快照乘一个工作流版本。Control Outbox 由具备租约、心跳和 Fencing 的评测派发器消费，使用稳定 WorkPackage 标识恢复；Runtime 结果通过既有治理投影返回。

Runtime 的 platform_evaluation_authorizations 保存按租户、应用和批次限定的模型授权，使用签名 Admission 单调推进。系统评分子执行只拥有其冻结模型，业务工作流仍保留自身资源授权；两者不合并。模型撤销后待执行评分以明确错误结束，不切换备用模型。调用样本和附件校验经 Runtime 查询授权访问，Control 不直连 Runtime 数据库。测试与评测不扩展应用会话或记忆评测范围。详见 [应用测试集与平台 AI 评测](plan8/12-application-evaluations.md)。

## 工作流输入交互

中间回复和输入等待统一归入 `agentx/core`，持久化与恢复由 Runtime 承担。`input_waits`、`workflow_messages` 和 `channel_input_receipts` 分别保存等待、消息和渠道幂等凭据；Hook 复用 delivery outbox。平台入口共享等待节点发现及回复序列编辑器，定时和测试用例版本冻结 `waitInputs`，评测投影保存实际等待和消息。详见 [工作流交互协议](14-workflow-interactions.md)。
