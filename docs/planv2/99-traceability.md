# Agentx V2 架构与产品能力追踪矩阵

状态只允许 `planned`、`in_progress`、`blocked` 和 `done`。完成任务时必须补充测试路径、命令和证据目录。

V2 完成需要同时关闭两张矩阵：第一张证明控制面/执行面隔离成立，第二张证明当前产品能力没有因破坏性重构而丢失。原 [功能追踪矩阵](../plan/99-feature-traceability.md) 是产品回归基线；其现有 `done` 只表示 V1 已实现，不能直接继承为 V2 `done`。

## 1. 架构隔离矩阵

| 能力 | 任务 | 主要服务 | 权威存储 | 关键测试 | 状态/证据 |
|---|---|---|---|---|---|
| Control/Runtime 编译依赖隔离 | V2A-001～010 | 全部 | 无 | 越界 Cargo/SQL/Env CI | done；`agentx-boundary-check` / [证据](evidence/v2-00.md) |
| 当前表逐表处置目录 | V2A-006 | 全部 | Control/Runtime MySQL | Schema Catalog 无遗漏、机器允许列表 | done；133 张唯一处置 |
| Spec/Admission 分层 | V2A-007 | `platform-control --role=publisher` / `runtime-gateway` / `workflow-runtime --role=coordinator` | 两 MySQL | Bundle Payload 签名；Admission 为独立 Command/Epoch | done；Contracts Schema 测试 |
| 通用 Claim/Lease 与 Fencing | V2A-008、V2S-001 | 后台服务 | 所属 MySQL | 并发 Claim、旧 Owner 写入拒绝、过期接管 | done；公共协议与全 Role 审计均通过，[V2-06A 证据](evidence/v2-06.md) |
| 跨面信任与 V2 滚动兼容 | V2A-009～010 | 全部 | Kubernetes/Internal API | RS256 Service/Delegation JWT、双 `kid`、版本窗口/Expand-Contract | done（契约冻结）；部署隔离由 V2K 追踪 |
| Control 初始 Schema | V2D-001 | `platform-control` | Control MySQL | 空库 Migration | done；[V2-01 证据](evidence/v2-01.md) |
| Runtime 初始 Schema | V2D-002 | runtime 全服务 | Runtime MySQL | 空库 Migration/状态约束 | done；[V2-01 证据](evidence/v2-01.md) |
| 双 MySQL/单 Runtime Redis | V2D-004、V2D-009 | deploy/Control 服务 | 2 MySQL、1 个 Runtime Redis | 无 Control Redis 代码/清单/Secret/配置；Control 不连接 Runtime Redis | done；[Kubernetes Run `20260813t0021`](evidence/v2-01.md) |
| OSS 三域权限隔离 | V2D-005 | control/runtime/trace | OSS | 越域读写负向测试 | done；[Kubernetes Run `20260813t0021`](evidence/v2-01.md) |
| 不可变 Execution Spec Bundle | V2P-001、V2P-004、V2P-007 | `platform-control` / `runtime-gateway` | 两 MySQL/Runtime OSS | Hash/签名/依赖闭包/对象完整性 | done；[V2-02 证据](evidence/v2-02.md) |
| Prepare/Activate/Rollback | V2P-002～005 | `platform-control --role=publisher` / `runtime-gateway` | Control Outbox、Runtime Inbox | 重放/崩溃/回滚 | done；[E2E-V2-001](evidence/v2-02.md#e2e-v2-001发布与激活) |
| 基础垂直切片和阶段级控制面离线 | V2P-008 | Gateway/Coordinator/Worker/Query | Runtime MySQL/Redis/OSS | 关闭 Control 后基础 `no_op` Workflow 完整终态 | done；[基础版 E2E-V2-002](evidence/v2-02.md#基础版-e2e-v2-002控制面离线) |
| Bundle Reference/Retention/GC 基础骨架 | V2P-009 | `workflow-runtime` | Runtime MySQL/OSS | Active/Prepared/Head/Reference/Hold 保护、未绑定 Ready 对象回收、幂等 GC 和失败重试 | done；Runtime Slice/GC 自动化测试与 [证据](evidence/v2-02.md) |
| Session/Wait/Checkpoint/Fork 完整引用保护 | V2R-011/015 | `workflow-runtime` | Runtime MySQL/OSS | 完整运行语义建立/释放 Reference 与 Retention Hold | done；Runtime Slice 与 [V2-04 Run `20260815013106`](evidence/v2-04.md) 覆盖 Wait、Checkpoint、Fork、GC 和 Retention Hold |
| API Key 和 Application Route 本地化 | V2G-001 | runtime-gateway | Runtime MySQL | Control DB 离线鉴权 | done；[V2-03 证据](evidence/v2-03.md) |
| Session/Message/Invocation 与 Gateway SSE/Cancel/Resume 基础切片 | V2G-002～003、007 | runtime-gateway | Runtime MySQL/OSS；Redis 仅 SSE 唤醒 | 幂等响应丢失、跨 Pod SSE、Redis 重建、Cancel/Resume 唯一 Command | done；[Run `20260813152731`](evidence/v2-03.md#kubernetes-e2e)；完整 Wait/Query 仍由 V2R/V2Q 跟踪 |
| Schedule/Poll/Lifecycle | V2G-004、008 | `workflow-runtime --role=trigger` | Runtime MySQL | 多副本扫描、DST/时钟回拨、Cursor 无重复 | done；[V2-03 证据](evidence/v2-03.md) |
| 独立 Runtime Ingress | V2G-005 | runtime-gateway | 无 | Web/Platform/Control MySQL 零副本调用 | done；[Run `20260813152731`](evidence/v2-03.md#kubernetes-e2e) |
| Bundle 驱动 Execution Snapshot | V2R-001 | coordinator | Runtime MySQL | 无 Control 查询 | done；完整 Bundle 在 Control 离线时执行且在途 Snapshot 不受 Head 变化影响，[V2-04 证据](evidence/v2-04.md) |
| Runtime 授权与撤权 | V2R-002 | coordinator/worker | Runtime MySQL | LKG/Staleness/Revoke | done；Run `20260815013106` 覆盖 Grant Revoke、旧 Epoch 和 72 小时 LKG Fail Closed |
| Vault/资源运行解析 | V2R-003、V2R-008 | workflow-worker | Runtime MySQL/Vault/Runtime OSS | 无明文/Control OSS 回退 | done；真实 Vault、Model/MCP、LightRAG、Mem0 与 OSS 故障隔离通过，[V2-04 证据](evidence/v2-04.md) |
| Command/Outbox/Recovery/Artifact Role | V2R-004 | `workflow-runtime` 后台 Role | Runtime MySQL/Redis/OSS | 多副本 Claim | done；多副本、强退、Artifact、Quota Leader 与 Fencing 自动化/Kubernetes 测试通过 |
| Runtime 状态机语义 | V2R-005、V2R-010～014 | coordinator/worker | Runtime MySQL | 分支/循环/Wait/Fork/Evaluation/Composite/Debug | done；Runtime Slice 与十二场景 Kubernetes E2E 覆盖完整 Engine 能力，[V2-04 证据](evidence/v2-04.md) |
| Capability Worker Pool | V2R-006 | workflow-worker | Runtime Redis/MySQL | 不兼容任务隔离 | done；20 路 Claim、兼容过滤、过期接管、旧 Token 与重复结果拒绝通过 |
| Runtime Redis 重建 | V2R-007 | `workflow-runtime` / `workflow-worker` | Runtime MySQL/Redis | 空 Redis 恢复 | done；Outbox/Pending/Running 三阶段删除重建均由 MySQL 权威事实收敛 |
| Runtime 当前状态与查询索引 | V2Q-001 | `runtime-gateway` / `workflow-runtime` | Runtime MySQL | 与领域状态/Outbox 同事务；关闭 Consumer 后查询和恢复仍正确 | done；[V2-05 场景 2](evidence/v2-05.md#3-e2e-场景) |
| Runtime→Control 治理投影 | V2Q-002 | `platform-control --role=projector` | 两 MySQL/Runtime Event Export API | 治理投影白名单按 Cursor 拉取、离线恢复、Runtime 零反向调用 | done；72 小时等价积压和 Snapshot 重建通过 |
| Runtime 列表与治理投影 | V2Q-003、V2Q-007 | `platform-control --role=api/projector` / `runtime-gateway` | Runtime Query API、两 MySQL | Execution 列表只走 BFF；治理投影不复制列表、成本或 Runtime Status | done；强一致分页与五类治理 Generation 通过 |
| 实时运行详情 | V2Q-004 | `platform-control --role=api` / `runtime-gateway` | Runtime MySQL | 无直连 Runtime DB | done；Delegation 单次调用和 Runtime Detail 边界通过 |
| Trace Pipeline | V2Q-005 | `workflow-runtime --role=trace-relay` / `observability --role=trace-consumer` | Runtime MySQL/Redis/ClickHouse | Consumer 无 Runtime DB 凭据、多 Relay/CH 故障 | done；重放去重且 CH 恢复 32 秒补齐 |
| Observability Query | V2Q-006 | `observability --role=query` | ClickHouse | 租户/范围/超时 | done；查询预算、Delegation、降级和只读权限通过 |
| 全服务 Claim/Lease 审计 | V2S-001～004 | 后台服务 | 所属 MySQL | 竞争/过期接管/Provider 对账 | done；审计目录、20 路 Claim、Pod UID Owner、旧 Token 和强退接管通过，[Run `20260816-v206a-final6`](evidence/v2-06.md) |
| PDB/Drain/metrics | V2S-005 | 常驻服务 | Kubernetes | 用户驱动扩缩容/强退 | done；七类工作负载真实 `2→4→2`、SSE 重连、PDB Eviction 和零残留通过；当前 Agentx 不创建 HPA，只暴露 metrics，[V2-06A 历史证据](evidence/v2-06.md) |
| 背压和容量 | V2S-006 | runtime 全服务 | 全部 Runtime 依赖 | 容量矩阵 | planned（延期到 V2-08B）；执行正式容量、公平性、背压和两小时稳定性 |
| 三面 Namespace 和 NetworkPolicy | V2D-007、V2K-001～003 | deploy | Kubernetes | DB/Redis 越界拒绝、Internal API/Provider 白名单正向测试 | V2D-007 done；V2-01 跨域 DB/Redis 拒绝已验证，V2K-001～003 继续追踪生产强化 |
| 独立 Migration | V2K-004 | migrate jobs | 两 MySQL/ClickHouse | 并发锁/空库 | in_progress；V2-07A 已实现独立 Expand/Contract Target，真实集群竞争待验证 |
| 备份恢复和独立升级 | V2K-005～006 | deploy/runbook | 全部权威存储 | 恢复/持续探针 | in_progress；RPO/RTO、Adapter、独立部署 Action 已实现，真实 PITR + Redis 重建演练用例已就绪（tests/e2e/upgrade/test_backup_recovery_drill.py），真实窗口执行待验证 |
| 删除 V1 架构 | V2C-001 | 全部 | 无 | 静态扫描 | done；151/151 API 处置、Legacy 例外为零、Workspace/边界门禁和 [V2-08A 证据](evidence/v2-08.md) |
| 控制面完全离线运行 | V2C-003 | runtime 全服务 | Runtime 依赖 | E2E-V2-002 | done；最终本地三域 Run 与 V2-04 专项矩阵共同证明完整 Runtime 链路离线运行 |
| Runtime 故障恢复 | V2C-004 | runtime 全服务 | Runtime 依赖 | E2E-V2-003/004/007 | done（本地语义）；MySQL Fail Closed、Redis 重建、CH 独立终态及 OSS/Vault/OpenSandbox 专项故障均通过 |
| 本地产品闭环 | V2C-002/007 | 全部 | 三域权威存储 | E2E-V2-001～011 + 产品矩阵 | done；Run `20260818-final4`，见 [V2-08A 证据](evidence/v2-08.md) |
| 生产容量与最终发布 | V2C-005/006 | 全部 | 全部 | E2E-V2-012 + 生产矩阵 | planned（延期到 V2-08B）；关闭容量、恢复、安全和发布认证 |

## 2. 产品能力等价矩阵

| 当前能力基线 | Control 权威 | Runtime 权威 | 跨面契约 | V2 主要任务 | V2 回归重点 | 状态/证据 |
|---|---|---|---|---|---|---|
| 企业初始化、登录和租户上下文 | Tenant/User/Auth/IAM | Tenant Runtime Status、Runtime Identity Projection | Tenant Bootstrap/Disable Admission Command | V2D-001/002、V2R-002 | 初始化、Token 轮换、伪造 Tenant、停用传播 | done；API-first、Control UI 与授权专项回归，见 [V2-08A 证据](evidence/v2-08.md) |
| 部门、用户、角色和数据范围 | Department/Role/Permission/Data Scope | Service Identity/Policy Epoch Projection | Policy Snapshot/Revoke | V2R-002、V2Q-002 | 部门闭包、数据范围、撤权、Staleness | done；Control UI、资源会签、Revoke/LKG/Staleness 专项回归通过 |
| Application Deployment→基础 `no_op` 发布和回滚 | Workflow/Version/Control Deployment | Bundle/Runtime Head/Reference | Prepare/Activate/Rollback | V2P-001～009 | 确定性 Bundle、响应丢失重放、在途固定 Bundle、回滚不恢复撤权 | done；[E2E-V2-001](evidence/v2-02.md#e2e-v2-001发布与激活) |
| Workflow 草稿、版本、部署和回滚 | Workflow/Draft/Version/Control Deployment | Bundle/Runtime Head/Reference | Prepare/Activate/Rollback | V2P-001～009 | Revision 冲突、版本不可变、回滚不回滚 Admission | done；Workflow 5.0、Studio 与发布/回滚回归通过 |
| Credential、Model、MCP、Skill 和资源授权 | Metadata/Version/Grant/Secret Reference | 精确 Binding、Grant Projection、Handle | Bundle + Resource Revoke | V2R-002/003/008 | Secret 脱敏、固定版本、递归依赖、撤权 | done；类型化 Binding/Handle、真实 Vault/Provider、递归闭包与撤权通过 [V2-04 E2E](evidence/v2-04.md) |
| 设计期资源授权申请与部门会签 | Grant Request/Review/Grant/Audit | 只接收最终 Grant Projection | Grant Updated/Revoked | V2R-002、V2Q-002 | 六态、跨部门会签、Stale 检查、运行拒绝 | done；`resource-grant-requests` UI/API 与 Runtime 授权传播回归通过 |
| LightRAG、Mem0 控制面和 Addon | Resource Metadata/Version/Health | 固定 Endpoint/Binding/Call | Bundle/Health Event | V2R-008、V2K-001 | Addon 组合、固定版本、故障隔离 | done（本地功能）；V2-04 与 V2-08A 完成真实 Binding/调用，生产 NetworkPolicy 认证仍由 V2K-001 追踪 |
| API Key Admission + Application Route 基础切片 | Application/Key 管理记录 | Route/Head/Key Hash/Tenant Admission | Publish/Key Admission | V2P-003/004/008 | Control 离线鉴权、Key 撤销、Epoch 单调、Rollback 不回退 Admission | done；[V2-02 证据](evidence/v2-02.md) |
| Application、API Key、Session、Gateway 和 SSE | Application Metadata/Key 管理记录 | Route/Head/Key Hash/Session/Message/Invocation/SSE Cursor | Publish/Key Admission/Event | V2G-001～008 | Key 轮换、幂等、三种 Session Policy、跨 Pod SSE/Redis 重建 | done；[V2-03 证据](evidence/v2-03.md) |
| API Key + `no_op` Invocation 到真实 Execution | 无运行时权威数据 | Invocation/Execution/Snapshot/Attempt/Output | Runtime Query API | V2P-008 | 控制面离线连续 10 次执行，输出、Bundle ID 和 Admission Epoch 可查询 | done；[基础版 E2E-V2-002](evidence/v2-02.md#基础版-e2e-v2-002控制面离线) |
| Application Invocation 到真实 Execution | BFF 鉴权与关联入口；无通用列表投影 | Invocation/Execution/Output | Runtime Query API + 治理 Event | V2P-008、V2G-002/003、V2Q-002～004 | Message/Invocation/Execution/Trace 双向定位，列表只读 Runtime | done；Runtime Query/BFF/Trace 单一来源由 [V2-05 E2E](evidence/v2-05.md) 验证 |
| Dataset、Evaluation Profile Version 和规则 | Dataset/Profile/Definition/Run Intent | Work Package/Case Runtime | Evaluation Request/Event | V2R-012、V2Q-002 | 版本不可变、导入原子、Runtime 不可用零伪记录 | done；固定 Work Package、真实 Evaluation/取消和 Control Generation 重建通过 |
| Evaluation 批量真实 Execution 和指标 | 报告与聚合投影 | Case/Evaluator Execution、实际输出、成本 | Evaluation Progress/Terminal | V2R-012、V2Q-002/006 | 批量执行、取消、规则、成本、Trace | done；Runtime Case/Evaluator、治理投影和 Observability 查询链路通过 |
| 审批任务和站内通知外围能力 | IAM 校验、Action Submission/Audit、Notification/Read | Approval Task/Decision/Resume | Decision Command/Applied Event | V2R-010、V2Q-001/002 | 候选资格、并发终态、响应丢失、跳转 | done；Runtime Decision CAS、通知和 Snapshot/增量投影通过 |
| Approval Node 恢复和运行通知闭环 | 控制台投影 | Wait/Approval/Resume Token/Execution | Decision/Resume/Event | V2R-010 | 决策恢复、输出端口、重复命令 | done；并发决策、响应丢失、`approved/rejected` 端口与唯一恢复通过 [V2-04 E2E](evidence/v2-04.md) |
| Item、Expression、Node Protocol 和基础节点 | Definition/Manifest/UI Schema | IR/Snapshot/Item/Lineage/Attempt | Execution Spec Bundle | V2R-005/006 | 多来源 Lineage、AST、协议和 Manifest 版本 | done；Runtime Kernel/Worker Protocol、Workflow 5.0 回归与完整 Bundle E2E 通过 |
| IF、Switch、Merge、Loop、Wait、Sub-workflow | Definition/固定子版本 | IR/Delivery/Activation/Wait/Child Execution | Bundle 依赖闭包 | V2R-005/010/013 | Connection Order、循环预算、固定子版本 | done；Kernel、Wait 与多层固定 Composite 在控制面离线时通过 |
| API、Webhook、Schedule、Poll 和 SSE 触发 | Trigger 配置和发布策略 | Route/Binding/Lease/Cursor/Invocation | Bundle + Admission Command | V2G-004/005/008 | 防重放、Misfire、补偿、Poll Cursor、SSE 回放 | done；[V2-03 证据](evidence/v2-03.md) |
| Execution、Node Activation/Attempt/Delivery 和可靠调度 | BFF 查询入口；无通用摘要投影 | Execution/Snapshot/Attempt/Delivery/Lease | Runtime Query API + 治理 Event | V2R-004～007、V2Q-003/004 | 重复投递、Lease、超时、崩溃、恢复和列表权威来源 | done；V2-04 调度/恢复与 V2-05 强一致 BFF 列表/详情均通过 |
| Execution 列表、Trace、查询和 Artifact | BFF 查询入口；Control 无运行列表副本 | Runtime 终态/Artifact；ClickHouse Trace | Runtime Query API/治理 Event/Trace Stream | V2Q-002～007 | 列表单一来源、脱敏、Span、Artifact 授权、CH 故障 | done；[V2-05 E2E](evidence/v2-05.md) 场景 2～12 |
| 模型成本、MCP 错误和 Agent Trace | Observability 查询入口；无 Control Cost 副本 | Runtime Call/Agent Iteration；ClickHouse 明细 | Observability API/Trace Stream | V2R-008/009、V2Q-005/006 | Token、成本、循环、错误、补投和列表单一来源 | done；真实 Provider Ledger、Trace Pipeline、CH 补投和 Observability Query 通过 |
| Checkpoint、Fork 和部分执行 | 发起权限和审计投影 | Checkpoint/Fork/Bundle Reference/Artifact | Fork Command/Event | V2R-011 | 原记录不变、依赖恢复、副作用确认、GC 保护 | done；内联/外置 Checkpoint、固定来源 Fork、副作用确认和 GC 引用保护通过 [V2-04 E2E](evidence/v2-04.md) |
| Model、Agent、MCP、Skill、RAG 和 Memory 运行 | 资源版本和授权治理 | Binding/Handle/Runtime Call/Agent State | Bundle/Grant Command/Event | V2R-008/009 | 预算、撤权、Attempt 恢复、递归依赖 | done；真实 Model/MCP/LightRAG/Mem0/Skill/Agent、撤权与强退恢复通过 [V2-04 E2E](evidence/v2-04.md) |
| OpenSandbox 与 Code Runtime | Profile/Policy/发布治理 | Sandbox Binding/Lease/Handle | Bundle/Runtime Event | V2R-009、V2S-003 | TTL、配额、网络、Secret、取消和强退 | done（本地功能）；V2-04/06A/08A 完成真实 OpenSandbox、Reaper 和闭环，生产强隔离由下一行追踪 |
| Sandbox 生产强隔离与供应链 | Policy/Image/签名治理 | RuntimeClass/Lease/执行事实 | Bundle/Health Event | V2R-009、V2K-002/003 | RuntimeClass、资源限制、双栈 Egress、攻击矩阵 | planned |
| Application 发布状态 UI 基础切片 | Deployment/Publish Attempt | Runtime Receipt/Head | Publish Attempt 查询/Retry/Rollback | V2P-005 | 既有页面展示发布状态、脱敏错误、Retry/Rollback，视觉体系不变 | done；Web 181 项测试与 [证据](evidence/v2-02.md) |
| Workflow Studio、Draft Debug 和发布 | Draft/Editor/Overlay/Catalog | Debug Work Package/Execution | Debug Package API/Event | V2R-014、V2P-001 | Definition/Editor 分离、真实调试、发布、Control OSS 隔离 | done；签名 Debug Work Package、TTL/取消、真实节点结果及 Studio/BFF 接线已在 V2-04/08A 回归 |
| Workflow 5.0 Start/End、Expression 和 Context | Definition/Contract | IR/Input/Output/Context CAS | Bundle/Work Package | V2R-005/014 | 类型、基数、Patch/CAS、正式 End Output | done；Runtime Kernel、正式 End Output、Context CAS 与 Kubernetes 完整 Bundle 回归通过 |
| Composite Node 与 Workflow Package | Package/签名/导入和资源重绑定 | 固定 Composite 闭包 | Bundle Object Manifest | V2P-007、V2R-013 | 递归、签名、Multipart、Control 离线执行 | done；多层固定 Composite 离线执行，递归和可变 Head 发布拒绝均通过 |
| 配额、保留、扩容和发布 | Policy/Retention Plan | Admission/Reservation/Ledger/Hold/GC | Policy/Retention Command/Event | V2R-015、V2S-005/006、V2K-005/006 | 零漂移、背压、容量、恢复、独立升级 | in_progress；V2-04 完成 Ledger/Retention，V2-06A 完成多副本/PDB/Drain 和功能性零残留；当前扩缩容由用户平台负责，容量、公平性和两小时稳定性仍属 V2S-006，独立升级仍属 V2K |
| 业务国际化与安全删除 | Localized Control API/UI、删除治理 | Tombstone/Disable/Retention | Disable/Retention Command/Receipt | V2R-015、V2Q-002、V2C-007 | 双语、引用预检、竞态、跨库 Fail Closed | done；中英文 UI、安全删除、引用保护和 Tombstone 回归通过 [V2-08A 证据](evidence/v2-08.md) |

## 维护规则

- 一项能力只能有一个主要完成任务，可以有多个依赖任务。
- 任务标记 `done` 时必须补充可复现证据，不能只引用代码提交。
- 架构边界变化先更新 `00-target-architecture.md`，再调整任务和矩阵。
- 新增常驻服务必须说明其数据所有权、多副本协议、健康检查和故障恢复方式。
- 产品能力行不得因为 V1 已有 `done` 证据直接标记 V2 `done`；至少需要新的边界测试和对应回归证据。
- 最终发布前两张矩阵都不能存在非 `done` 状态。
