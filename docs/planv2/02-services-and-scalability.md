# V2 服务职责与横向扩展计划

## 1. 通用扩展协议

所有常驻服务必须满足：

1. 不在本地内存保存权威业务状态。
2. Pod 具有稳定的本次实例 ID，但业务身份不绑定 Pod。
3. 写操作使用业务幂等键、状态条件或乐观版本。
4. 队列消费使用 Consumer Group，并用数据库状态确认任务是否仍有效。
5. 数据库扫描任务使用 `FOR UPDATE SKIP LOCKED` 或 `locked_by/locked_until`。
6. 外部副作用使用稳定 Idempotency Key；不能通用保证 Exactly Once 时必须记录风险与补偿策略。
7. Readiness 反映必需依赖，Liveness 只反映进程存活。
8. 优雅终止先停止接收新请求/任务，再等待有界时间释放 Lease。

所有 MySQL Lease 的过期判定统一使用所属数据库时间，应用只提交 TTL/Duration，不以 Pod 本地时钟计算 `locked_until`；每次接管生成单调 Fencing Token，后续写入必须同时校验 Owner 和 Token。领取事务必须短小，外部 I/O 在提交后执行，索引至少覆盖状态、到期时间和分片键。

V2-06A 已完成紧凑 Profile 全 Role Claim 审计、多副本强退、Drain、PDB 和七类工作负载 `2→4→2`；证据见 [V2-06A 验收](evidence/v2-06.md)。V2-06B 仍负责容量、背压、租户/Provider 公平性和真实两小时稳定性，因此 V2-06 总阶段保持 `in_progress`。

## 2. 构建产物、Role 与 Deployment

V2 使用“少量构建产物 + 多 Role Deployment”。构建产物是代码、镜像和版本线的边界；Role 是同一构建产物的启动模式；Deployment 只是为了权限、资源或外部扩缩容边界而创建的 Kubernetes 工作负载，不自动成为新微服务。

| 构建产物 | Role | 建议 Deployment 名 |
|---|---|---|
| `platform-control` | `api` | `platform-api` |
| `platform-control` | `publisher/projector/retention` | `control-worker-*` |
| `workflow-runtime` | `coordinator` | `workflow-coordinator` |
| `workflow-runtime` | `trigger/command/outbox/recovery/artifact/quota/trace-relay` | `runtime-worker-*` |
| `observability` | `trace-consumer/query` | `trace-writer` / `observability-api` |

建议 Deployment 名只用于运维识别，不对应独立仓库、Crate、镜像或版本线；命令、任务和追踪矩阵统一使用 `构建产物 --role=...`。

默认紧凑 Profile 不创建上表中的全部建议 Deployment，而是按构建产物聚合 Role：

| 默认 Deployment | 默认 Role 集合 | 何时拆分 |
|---|---|---|
| `platform-control` | `api,publisher,projector,retention` | API 延迟受后台任务影响，或后台 Role 需要不同权限/扩缩容策略 |
| `workflow-runtime` | `coordinator,trigger,command,outbox,recovery,artifact,quota,trace-relay` | Coordinator 延迟、队列积压、资源类型或数据库连接预算证明需要独立扩容 |
| `workflow-worker` | 通用 capability 池 | 模型、Sandbox 或特定 Provider 出现显著不同的资源/安全边界 |
| `observability` | `trace-consumer,query` | 查询负载影响 Trace 摄取，或 ClickHouse 权限必须读写分离 |

Role 拆分必须记录基线指标和预期收益；没有证据时保持紧凑 Profile。实现允许 `--roles=a,b` 或等价配置启动 Role 集合，`--role=x` 只是单 Role 部署的简写。

## 2.1 服务扩展矩阵

| 服务/Role | 水平扩展 | 协作方式 | V2 改造重点 |
|---|---:|---|---|
| web-console | 是 | 无状态/CDN | 去除生产 Gateway 反向代理职责 |
| `platform-control --role=api` | 是 | Control MySQL | 移除所有运行后台循环和 Runtime DB 直连 |
| `platform-control --role=publisher` | 是 | Control Outbox Claim + Runtime Inbox | Prepare/Activate、签名、幂等 |
| `platform-control --role=projector` | 是 | Runtime Event Export Cursor + Projection Receipt | Control 主动拉取 Runtime 治理投影，不复制通用执行列表 |
| `platform-control --role=retention` | 有条件 | 分区 Claim/Leader Lease | 规划任务与删除执行分离 |
| runtime-gateway | 是 | Runtime MySQL | HTTP、SSE 无 Sticky Session |
| `workflow-runtime --role=trigger` | 是 | Trigger Row Lease | Schedule/Poll/Lifecycle 从 Gateway 拆出 |
| `workflow-runtime --role=coordinator` | 是 | Runtime MySQL 条件状态机 | 只承载 gRPC/状态推进 |
| `workflow-runtime --role=command` | 是 | `SKIP LOCKED` | Runtime Command 消费 |
| `workflow-runtime --role=outbox` | 是 | Outbox Claim | Redis 派发 |
| `workflow-runtime --role=recovery` | 是 | Lease/Wait Claim | Reaper 和 Resume 扫描分片 |
| `workflow-runtime --role=artifact` | 是 | Artifact Claim | Checkpoint/Result 外置 |
| `workflow-runtime --role=quota` | 主备或分片 | Leader Lease/租户分片 | Redis 全量校准不得每 Pod 执行 |
| `workflow-runtime --role=trace-relay` | 是 | Trace Outbox Claim | Runtime MySQL → Runtime Redis，不向可观测面发数据库凭据 |
| workflow-worker | 是 | Redis Consumer Group + Attempt Lease | 按 capability 独立池扩容 |
| sandbox-manager | 是 | Sandbox Lease Claim | Reaper 独占认领 |
| `observability --role=trace-consumer` | 是 | Redis Consumer Group | 批量写 ClickHouse、Event ID 去重；不连接 Runtime MySQL |
| `observability --role=query` | 是 | ClickHouse 只读 | 查询成本、范围和租户限流 |
| Migration Job | 否 | DB Advisory/Schema Lock | 每个目标同一时刻一个 Job |

只有出现独立权限、独立资源模型、独立发布节奏或显著故障隔离收益，并通过 ADR 后，Role 才能升级为新的构建产物。

## 2.2 服务运行契约

V2-00 为每个常驻服务/Role 建立一张可执行检查的契约：

| 契约项 | 要求 |
|---|---|
| 数据权限 | 列出可读表、可写表和禁止表；使用独立最小权限数据库账号 |
| 网络权限 | 列出允许访问的 Service/端口、OSS Domain 和外部 Provider |
| API | 对外、内部管理、健康和指标端口分离，标明身份与限流 |
| 幂等 | 每个写入/外部副作用的稳定 Idempotency Key 和冲突结果 |
| 并发 | Claim/Lease、乐观 Version、Consumer Group 或 Leader Lease |
| 依赖 | Readiness 必需依赖、可降级依赖和降级行为 |
| 终止 | 停止接单、Drain、Lease 释放/过期和强退恢复流程 |
| 容量 | 单 Pod 并发、连接池上限、队列指标和租户/Provider 限额 |
| 恢复 | Owner、可重放来源、RPO/RTO 和对账任务 |

CI 根据契约扫描 Deployment Env/Secret、SQL 表名和 NetworkPolicy。运行时 Doctor 使用与服务相同的账号验证“应允许”和“应拒绝”的依赖，而不是只做管理员账号连通性测试。

### 2.3 紧凑 Profile 与外部扩缩容

| 服务 | 初始/预算上限 | MySQL Pool/Pod | 建议暴露给外部平台的信号 |
|---|---:|---:|---|
| Web Console | 1/4 | 无 | CPU、请求延迟 |
| Platform Control | 1/4 | 10 | Ready/最老队列，CPU 辅助 |
| Runtime Gateway | 1/4 | 8 | 在途请求、SSE，CPU 辅助 |
| Workflow Runtime | 1/4 | 10 | Ready/最老队列、活跃 Lease |
| Workflow Worker | 1/4 | 6 | Ready Attempt、最老队列、活跃 Lease |
| Sandbox Manager | 1/4 | 2 | 最老任务、活跃 Lease |
| Observability | 1/4 | 无 MySQL | Trace Ready、最老年龄 |

Profile 使用 `replicas` 表示首次安装副本数，当前统一为 `1`；使用 `maxReplicas` 表示容量预算上限。单副本默认值降低资源占用，但不提供 Pod 级冗余。连接预算仍按 `maxReplicas × mysqlPool` 计算；Runtime 四类服务同时达到 4 副本时为 `104`，Profile 上限为 105，不超过 MySQL 151 连接上限的 70%。`maxReplicas` 不是自动扩缩容配置，Agentx 不据此创建 HPA、KEDA 或自定义 scaler。Upgrade/Rollback 不写回已经存在的 Deployment `spec.replicas`。升级器只在首次迁移时删除历史 Agentx HPA 并记录 Namespace 清理标记，后续不会删除用户创建的同名扩缩容资源。

`v2alpha2` Profile 固定三个物理 Namespace：Control、Runtime、Dependencies。Observability Deployment、ClickHouse 和 Observability 运维 Job 与 Runtime 共用物理 Namespace，但仍保留 `agentx.io/plane=observability` 工作负载标签和独立 Target/Release State；Status、Upgrade、Rollback 与 Uninstall 必须按逻辑 Plane 过滤，不能因共用 Namespace 操作到另一 Plane。

六类后端应用在独立 `9092` 端口暴露低基数 Prometheus 文本格式指标，5 秒缓存队列统计，抓取不会触发高频 SQL。Metrics Service 不属于业务 Service，也不经 Ingress 暴露；监控组件所在 Namespace 必须添加 `agentx.io/metrics-access=true` 标签才能通过 NetworkPolicy 抓取。Agentx 不安装 Prometheus、Prometheus Adapter 或 Metrics Server；抓取、存储、告警和扩缩容由用户 Kubernetes 平台负责。用户若采用 HPA、KEDA 或自定义控制器，应直接消费 `*-metrics` Service，并保证目标副本数不超过 Profile 的 `maxReplicas` 预算。指标系统故障不改变业务正确性。

## 3. Platform API

V2 后 Platform API 只承担同步控制请求。必须删除或移出：

- Runtime Projector。
- Runtime 主动 Event Relay（改为 Control Pull Projector）。
- Runtime Retention 扫描。
- 对 Coordinator 的生产调用代理；仅保留 Studio Debug 使用的受控 Runtime Internal Client。
- 对 Runtime MySQL 表的直接 SQL 查询。

Platform API 可以任意多副本，发布等长任务只写 Control Outbox，不在 HTTP 请求中等待执行面完成全部工作。前端通过 Publish Attempt 查询发布进度。

## 4. Runtime Gateway

Gateway 负责：

- Runtime API Key/JWT/Webhook 认证。
- Application Route 和 Deployment Head 解析。
- Input Schema 校验。
- Invocation、Session、Message 和 Idempotency。
- SSE、查询、Cancel、Wait Resume。

Gateway 不负责：

- Schedule/Poll 扫描。
- Bundle 编译。
- Control IAM/Department 查询。
- Execution 状态推进。

SSE Cursor 持久化在 Runtime MySQL，Redis 只做唤醒；任意 Gateway Pod 都能接续连接，因此不要求 Sticky Session。V2-03 的静态门禁只允许 `sse_wakeup` 模块构造 Gateway Redis Client，丢通知、断连或 Redis 空库重建时每秒回查 MySQL。Gateway 使用独立只读 Vault 身份解析 Bundle 中固定版本的 Webhook Secret Reference，不读取 Control Secret 或数据库明文。

Gateway 进入优雅终止时先从 Readiness 摘除并停止接受新的 Invocation/SSE；新写返回 `503 SERVICE_DRAINING` 和 `Retry-After: 5`。现有 SSE 在 15 秒内关闭并保留最后 Cursor，客户端以 `Last-Event-ID` 连接任意副本。后台工作最多 Drain 45 秒，Pod `terminationGracePeriodSeconds` 为 60。Invocation 接受事务成功但响应丢失时，调用方使用相同 Idempotency Key 查询既有结果，禁止创建第二条 Invocation。

## 5. Trigger Role

将当前 Gateway 内嵌的 Schedule、Poll 和 Lifecycle Loop 移到 `workflow-runtime` 的独立代码 Role，使其可单独启动和扩容；默认紧凑 Profile 仍与其他 `workflow-runtime` Role 同 Deployment 运行。所有扫描统一改为两步：

1. 事务内认领到期行并写入 `locked_by/locked_until`。
2. 持有 Lease 的实例执行外部调用或创建 Runtime Command。

Schedule 使用稳定的计划时刻作为幂等键；Poll 优先使用 Provider Event ID/Cursor，否则使用规范化响应 Hash；Lifecycle Operation 使用 Binding Revision + Operation 作为幂等键。

Trigger 契约还必须冻结时区数据库版本、Misfire Policy、补偿窗口、最大追赶次数和禁用后的在途行为。Claim 成功但创建 Runtime Command 的响应未知时，以同一计划时刻/Provider Event ID 重试，数据库唯一约束负责最终收敛。

V2-03 已实现并验证 IANA 时区、DST 不存在/重复时刻、时钟回拨，以及 `skip` 和最多一次 `fire_once`；Trigger 草稿只在下一次 Application Deployment 激活。V2-06A 已完成两个 Trigger Role 副本的 Claim/强退与唯一业务事实验证；更大规模容量和公平性仍由 V2-06B 负责，不回退已冻结的时间语义。

## 6. Coordinator 与 Runtime Worker

Coordinator 只保留低延迟状态机接口：

- Request Execution。
- Report Node Result。
- Heartbeat/Cancel/Resume/Fork。
- Execution 状态条件推进。

当前内嵌循环拆到 `workflow-runtime` 的后台 Role：

- `command`：认领 Runtime Command。
- `outbox`：发布 Execution Outbox 到 Runtime Redis。
- `recovery`：回收 Lease、恢复 Wait、处理 Timeout。
- `artifact`：外置 Checkpoint 和大型结果。
- `quota`：按租户分片校准，或使用 Leader Lease。

一个 Execution 的一次状态转换仍然串行化；横向扩展提高不同 Execution 之间的并行度，不允许多个 Coordinator 无锁并改同一状态机。

Runtime MySQL 应用连接统一使用 `READ COMMITTED`；每个 Execution 的修改仍以父行 `FOR UPDATE`、Lease/Fencing 和版本 CAS 保证原子串行化。默认 `REPEATABLE READ` 对不存在的配额幂等键保留 gap lock，使不同 Execution 的新预留插入互相死锁；连接初始化明确设置隔离级别，不提供配置开关，也不改变 Control 的事务策略。配额策略行锁仍串行化同一维度的上限检查，非锁定查询读取当前已提交额度。只读分页使用既有 cursor/upper bound，而不能依赖跨语句重复读。此决定依据 [MySQL 8.4 隔离级别文档](https://dev.mysql.com/doc/refman/8.4/en/innodb-transaction-isolation-levels.html)，真实 MySQL 回归覆盖独立并发预留及上限不超额。

V2 首期使用 `execution_id + state_version` 乐观条件更新配合短事务行锁串行化单次状态转换，不引入按 Execution 的常驻 Actor。Report Result 必须携带 `attempt_id + worker_id + fencing_token + result_hash`；Coordinator 已提交但响应丢失时，Worker 重报得到相同终态，不能重新执行节点。租约台账以 Attempt 为主键，稳定 Worker UUID 不是跨任务唯一 Token；同一 Worker 的多个 Attempt 保留并独立释放各自的租约行。

## 7. Workflow Worker

Worker 保持以下协议：

```text
Redis Stream Message
  → 校验 Protocol/IR/Compiler/Capability
  → Runtime MySQL Claim Attempt
  → Worker Lease + Heartbeat
  → 执行 Node
  → Coordinator Report Result
  → Runtime MySQL 原子推进
  → Redis ACK
```

按 capability 建立逻辑领取边界：

- `builtin/http`
- `model/agent`
- `mcp/rag/memory/skill`
- `sandbox`

默认紧凑 Profile 由通用 Worker Pool 按 capability 领取兼容任务；V2-06A 的历史 `2→4→2` Run 已验证 Ready Attempt、最老消息年龄和活跃 Lease 指标以及多副本正确性。只有 V2-06B 证明模型、Sandbox 或特定 Provider 达到独立资源/安全边界和队列阈值后，才为对应 capability 使用独立副本数、Pod 并发、资源限额和外部扩缩容策略；06A 不授权拆分 Profile。

## 8. Runtime 当前状态与查询表

V2 不设置异步 Runtime Projector。Invocation/Assistant Message、Approval Runtime、Evaluation Case Runtime 和 Execution 当前状态由拥有该状态转换的事务同步写入；Runtime Query API 读取这些领域表或同事务维护的必要查询索引。

同一事务可以写 Runtime Event/Outbox，但 Event 只用于 Control 治理投影、通知、审计和可观测投递。删除 Event 或暂停 Consumer 不能导致 Runtime 丢失当前状态、无法查询、无法恢复或无法继续推进。Control 治理投影仍由 `platform-control --role=projector → Runtime Event Export API` 按 Cursor 拉取，并独立维护 Receipt。

每次状态转换在同一事务内锁住已存状态版本，并与上次完整状态机比较；`node_executions`、边投递和结束投递只写新增或变化的记录，Loop 公共输出只更新受影响的来源。完整状态机、Hash 和 Checkpoint 仍原子持久化，查询表不成为恢复权威。不得随每个节点结算重复 upsert 全部历史节点和投递，以免大工作流的 SQL 次数和持锁时间不断增长。

评测收敛与取消以 Work Package 父行串行化；目标 Case 与 judge 规则分别按租户/执行 ID 索引定位，只锁当前 Case 或规则。终态数量在同一条一致视图聚合中读取，不锁住全部历史 Case。确定性规则已结算、无待启动的模型规则时，不读取整份签名包。终态报告一次按租户/评测 Run 读取全部规则，再按 Case 分组，避免在父行锁内逐 Case 查询；Case/规则顺序、评分、成本与终态事件仍在原事务中确认。

血缘查询索引保留所有来源与目标 Item 序号，每批最多 256 行写入；不能在父行锁内逐条发起相同 INSERT。批次仍属于原有状态转换事务，失败整体回滚，唯一约束维持重报幂等。

Trace 索引采用数据库递增游标分配顺序，不在 `workflow_executions` 主行递增诊断计数器。`execution_events` 与 `trace_outbox` 同事务提交，业务状态转换失败时同步回滚。查询以租户/执行范围内的 MAX 游标及 COUNT 为一致快照；ClickHouse 按 watermark 范围内的唯一事件数判定完整性，不能只比较最大序号。这样外置大输入的诊断 Trace 不需要等待大工作流的权威状态行锁，也不引入新的计数器表或后台权威。

## 9. Sandbox Manager

Sandbox Manager API 可以多副本，但必须删除“每个 Pod 无条件扫描全部到期 Lease”的实现。Reaper 使用：

```text
ready/running/orphaned
  → terminating + locked_by + locked_until
  → OpenSandbox terminate
  → terminated
```

只有状态 Claim 成功的实例可以执行外部终止。创建、恢复和终止均使用稳定 Idempotency Key 与 OpenSandbox Label 做对账。

## 10. Trace Pipeline

将 Trace Pipeline 分为两个故障域：

- `workflow-runtime --role=trace-relay`：Runtime MySQL Trace Outbox → Runtime Redis Trace Stream。
- `observability --role=trace-consumer`：Runtime Redis Trace Stream → ClickHouse。

Relay 必须先 Claim Outbox，再 XADD，禁止多个副本先读取相同 Pending 行。Consumer 使用 Consumer Group；ClickHouse 行包含稳定 Event ID，并采用可验证的去重策略。写入成功、Runtime MySQL 标记成功和 Redis ACK 之间允许重复，但不能丢事件。

Relay 每次以 `SKIP LOCKED` 领取最多 100 个 ID、批量取得 Payload，提交 Claim 后使用 Redis pipeline 发出 XADD，再以每条 Event 的 Owner/Fencing/有效租约批量记录对应 Stream ID。失败批次按每条 Fencing 计算退避；旧 Owner 不能确认新 Owner 的租约。Redis pipeline 部分成功或结果未知时允许同 Event 重放，由消费者稳定 ID/Hash 去重。

Consumer 每批最多 100 条，整批校验后一次写入 Trace，随后一次查询批内 Event ID 的 Hash 冲突、批量写冲突记录及健康记录，再整批 ACK。同 Hash 的重放由既有 `ReplacingMergeTree` 与查询 `FINAL` 去重；不同 Hash 保留冲突内容并将所有受影响的租户/执行标记为 degraded。冲突检查放在持久写入之后，避免并发消费者都在插入前读到空结果。任一步失败都保留 Redis Pending 供重试。

Consumer 默认不持有 Runtime MySQL Credential。若未来因合规需要对 Runtime Trace Receipt 反向确认，只能通过最小 Runtime Internal API，不能开放业务库直连。

## 11. Observability API

Observability API 是纯查询服务，可任意多副本，但必须限制：

- 最大查询时间范围。
- 最大行数和分页游标。
- 每租户并发和成本。
- ClickHouse Query ID 与超时取消。
- Artifact 下载授权和短期 URL。

增加 API 副本不能无界放大 ClickHouse 重查询。

## 12. 扩容后的主要瓶颈

应用服务扩容后，容量上限将依次受到以下因素约束：

1. Runtime MySQL 事务、热点行、连接数和 IOPS。
2. Runtime Redis Stream 分片与网络。
3. 外部模型/MCP/Vault/OpenSandbox 限流。
4. OSS 请求数和带宽。
5. ClickHouse 批量写入和复杂查询。
6. 单 Session Context 的乐观锁冲突。
7. 单 Execution 状态机的必要串行部分。

容量计划必须统一计算所有 Pod 的数据库连接预算，不能让每个副本都使用当前默认最大连接数。

## 13. 连接、背压和过载规则

- 为 Control MySQL、Runtime MySQL、Redis、ClickHouse 和每个外部 Provider 建立全局连接/并发预算；单 Pod 默认值由“总预算 ÷ 最大副本数”推导。
- Gateway 在 Runtime MySQL/队列达到 Admission 阈值时返回带稳定错误码和 `Retry-After` 的显式拒绝，不能先接受再无限积压。
- Coordinator/Worker 按 Tenant、Capability 和 Provider 分层限流，避免一个热租户占满全部 Lease 或连接。
- Agentx 暴露最老消息年龄、Ready Attempt、活跃 Lease 和处理延迟等指标；用户的外部扩缩容策略可使用这些业务信号，CPU 只建议作为辅助信号。
- 缩容前必须验证 Drain 后的剩余 Lease、Pending、SSE 和连接数；超过超时则让 Lease 到期接管，不强行宣称成功完成。

V2-06A 已证明上述机制在功能性 `2→4→2`、Pod 强退和滚动场景下正确；100/500/1000 Execution、5000 Attempt、1000 SSE、热租户/Provider 公平性和两小时稳定性仍必须由 V2-06B 给出容量证据。

## 14. 控制复杂度的服务拆分门禁

新增独立常驻服务必须同时说明以下至少一项收益，并更新 ADR、运行契约和 E2E：

1. 需要独立安全权限或网络边界。
2. 负载形态不同且需要独立扩缩容策略或资源限制。
3. 故障会阻断不同可用性目标，必须隔离。
4. 发布节奏或协议兼容窗口明确不同。

如果只为代码目录整齐、函数过长或职责命名方便，使用模块/Crate/Role 即可，不新增网络服务。V2 首期禁止为每个后台循环创建独立二进制和 Service；优先维持少量部署制品、多 Role Deployment 的形态。

首期应用构建产物硬上限为 7 类；Kubernetes Deployment 数量可以因 Role、权限或外部扩缩容边界不同而更多，但这些 Deployment 不增加新的业务 API、数据库所有权或独立版本线。超过上限必须先更新 ADR-V2-017 和容量/故障证据。

紧凑 Profile 的常驻应用 Deployment 同样以 7 类为上限，其中 `sandbox-manager` 可选。Role 拆分 Profile 只有在容量或安全测试给出阈值后才能启用；不得把拆分 Profile 作为开发、CI 和首期生产的默认拓扑。

新增基础设施或跨面查询副本同样受门禁：

- Control MySQL 的 Outbox/Claim 出现实测瓶颈时，先优化索引、批次、分区和连接预算；普通 HTTP 突发先由 Ingress/API 限流处理。只有上述措施仍不满足冻结的 SLO，才允许 ADR 评估 Control Redis。
- Runtime Query 出现实测瓶颈时，先优化覆盖索引、分页、缓存和只读副本，再评估 Runtime 内部 Query Store；Control Runtime Read Model 只有在业务明确要求 Runtime 查询故障时仍浏览历史摘要，且接受最终一致语义时才能评估。
- Kafka/Pulsar/NATS 和 Event Sourcing 不作为 V2 的扩容预案。若未来业务范围发生根本变化，必须作为独立架构重构重新论证，不能通过预留接口绕开本计划。

## 15. 后续功能迭代路径

新增功能先按影响范围分类，避免所有需求都被迫跨三面开发：

| 功能类型 | 默认改动范围 |
|---|---|
| 纯设计/治理功能 | Control API、Control MySQL、Web；不创建 Runtime 契约 |
| 只读运行展示 | 优先扩展 Runtime Query/Observability API；不默认增加 Control 投影 |
| 影响新 Execution 的运行配置 | Control 配置 + Bundle Builder/Admission Command + Runtime 本地投影 |
| 影响在途 Execution 的控制动作 | 版本化 Command、Runtime CAS 状态机、结果 Event |
| 新节点/Provider | Manifest/IR/Capability/Worker Adapter；只有资源治理变化时才改 Control 数据模型 |

为 Runtime 功能提供统一开发骨架：Contracts Schema、Bundle Materializer、Inbox/Outbox、Claim/Lease、Runtime Query、Event Export、测试 Fixture 和边界扫描都复用公共库。业务实现不得复制一套私有发布/幂等协议。

每个跨面功能 PR 必须回答：权威事实在哪一面、是否需要进入 Spec 或 Admission、控制面离线时的行为、版本/回滚语义、Runtime Query/Event 是否必要、旧 Bundle 支持窗口和对应 E2E。纯 Control 功能不需要为形式一致强行经过 Runtime。
