# 运行记录、Checkpoint、沙箱与审批

## 1. Trace、Checkpoint 和审计事件

三者职责不同：

| 类型 | 用途 | 主要存储 |
|---|---|---|
| Trace | 观察节点、模型、工具和沙箱过程 | ClickHouse |
| Checkpoint | 恢复或派生执行 | MySQL 元数据加对象存储 |
| 审计事件 | 记录发布、审批、权限和人工操作 | MySQL |

Trace 丢失不应改变 Workflow 结果；Checkpoint 丢失会影响恢复；审计事件用于确认操作责任。因此三类数据不能只放在一张 Trace 表中。

## 2. Trace 层级

Workflow Trace 使用真实 Span 生命周期，而不是由查询端从 Runtime 明细推断调用关系。固定层级为：

- Execution → Start Boundary / Node / End Boundary
- Node → Attempt
- Attempt → Agent Run → Agent Iteration → Runtime Call
- Attempt → Sandbox
- Attempt → Plugin Operation；插件内嵌套 Operation 和宿主 Runtime Call 保留真实父子关系
- Node → Wait；Approval 使用 `waitKind=approval`

每个 Runtime 实体通过 `deterministic_uuid(entityId, "agentx-trace-span-v1:<kind>")` 生成稳定 Span ID。同一 Span 的 `started`、`updated`、`finished` 事件复用该 ID；`occurredAt` 是状态变化发生的业务时间，不由 Outbox、Redis Relay 或 ClickHouse Consumer 改写。重试创建新的 Attempt 实体及 Span，取消、超时、失败、等待恢复和 Outcome Unknown 都必须关闭相关活动 Span。

节点自动重试以状态机的 `attemptNumber` 为准；每次 Attempt 使用独立幂等键和 Span，`waitBetweenTriesMs` 映射为派发 Outbox 的 `available_at`，Node Span 在重试期间保持打开并发出 `retry_scheduled/retry_started` 更新。Worker operation deadline、Sandbox TTL、Execution 取消分别产生 `timed_out`、`sandbox.timed_out`、`sandbox.cancelled` 终止事件，后续 Reaper 清理不得把已有取消/超时终态覆盖成成功。

Start/End 是 `boundary` Span，不创建虚假的 Node Execution。Start Boundary 关联 Workflow 输入，End Boundary 关联最终输出和 End 字符串转换记录；Execution 根 Span 自身同样保留 `workflow_input/workflow_output`，因此高级瀑布稳定呈现 `Workflow → Start / Nodes / End`。字符串转换使用所属 Attempt 或 End Boundary 的 `conversion_record` Event，不创建额外 Span。

Trace Event 使用强类型 `contentKind` 区分内容语义：

- `workflow_input/workflow_output`
- `node_input/node_output`
- `attempt_input/attempt_output`
- `resolved_parameters`
- `runtime_request/runtime_response`
- `agent_input/agent_output`
- `iteration_input/iteration_output`
- `sandbox_request/sandbox_response`
- `wait_request/wait_response`
- `conversion_record`
- `plugin_content`

每个 Event 另外记录：

- Trace ID、Span ID、Parent Span ID
- Execution 和 Node Execution
- 开始和结束时间
- 状态和错误
- 模型及提供商
- Token 和成本
- Tool 名称、参数和结果
- RAG Query 和召回结果
- Sandbox ID 和资源用量
- Agent Iteration

小型内容先递归脱敏，再以不超过 Envelope 预算的 Preview 内联；大型内容复用 Runtime Artifact，通过 `contentRef + contentKind` 引用。Credential、Authorization、密码、Secret、API Key 和访问 Token 必须脱敏；`inputTokens/outputTokens/totalTokens/maxTokens` 等计量字段属于安全白名单，不得被误判为访问 Token。Artifact 仍由 Runtime 按所属 Execution 和 `trace:view` 权限授权查看或下载，Studio Runtime Panel、独立执行详情和 Node Inspector 都只能使用统一的 Execution-scoped 下载端点；Observability 不访问 Runtime MySQL 或对象存储。Trace 入队使用事务保存点降级，任何 Trace 序列化、预算或存储错误均不得回滚执行权威状态。

## 3. 平台 Trace 页面

不依赖 Prometheus 或 Grafana。平台直接通过内部 API 查询 MySQL 和 ClickHouse。

执行列表、详情、节点与调用详情使用同一查询范围。拥有公司数据范围的 `execution:view` 用户可查询本租户的全部执行；其他用户仍受 Application／Workflow Grant 与 Runtime User Admission 限制。委托 Token 不得在详情查询时丢失公司范围，也不能借助执行 ID 绕过 Runtime 的撤销校验。

Studio Runtime Panel、独立执行详情和 Node Inspector 复用同一个 Trace Workspace。默认节点视图从 Runtime MySQL 读取权威 Workflow 输入、Node Execution 输入输出、错误和最终结果，按 `开始 → 节点执行 → 结束` 展示；解析参数、Provider、Agent、Tool、Sandbox 和 Wait 等内部过程再由 ClickHouse 补充。普通视图按 Port 和 Item 解包语义字段，Model/Agent 优先展示 `text/usage/finishReason`，原始 Item JSON、Provider 原始响应和 Event Envelope 只进入高级区域。Node Execution 首次离开 `ready` 时冻结 `startedAt`，终态写入 `endedAt`；节点标题左侧元信息统一展示总生命周期耗时，Model/Agent 还展示按该节点 Runtime Call 聚合的本次成本，右侧只保留状态和展开动作。

高级瀑布使用 SkyWalking/Phoenix 风格的共享树形时间线：左侧展示可折叠 Span 层级、状态、耗时和时间轴，右侧按 Span 类型及 `contentKind` 动态生成内容页签，不为纯生命周期 Span伪造空输入/输出。页面能力：

- 按租户、Workflow、版本、状态、时间和发起者查询
- 在画布上显示运行路径
- 查看节点的每个 runIndex 和可选 loopIterationIndex
- 查看输入输出和 Artifact
- 查看模型请求、回复、Token 和成本
- 查看 Agent 每轮工具调用
- 查看节点错误和重试
- 查看审批等待时间
- 从 Checkpoint 创建 Fork
- 对比两次 Execution

Trace API 按 `(startedAt, spanId)` 稳定分页，默认 200、最大 1000，并支持 `nodeExecutionId` 过滤供单节点展开按需加载；高级瀑布继续稳定分页读取全量 Span。ClickHouse 先按 `span_id` 计算稳定页键和总数，再只回取当前页 Span 的生命周期事件，避免把完整 Execution Trace 读入 Observability 内存。Span Detail 返回按 `(executionSequence,eventId)` 有序的 `contents[]` 和原始 `events[]`，不再把不同阶段压缩为可互相覆盖的 `input/output/attributes`。查询端按 `spanId` 聚合乱序或重复生命周期 Event：最早 started Event 确定开始时间，finished Event 确定终态和结束时间；缺少 started 或 finished 时仍返回可诊断的不完整/运行中 Span。

Runtime MySQL 的 `trace_watermark` 与 ClickHouse 摄取水位决定诊断完整性：已有部分数据时返回 HTTP 200、Span 数据和 `warningCode=TRACE_DELAYED`；ClickHouse 完全不可用时返回明确 Observability 错误。两种情况下默认节点视图都继续展示 MySQL 权威数据，并把缺失诊断明确标记为“Trace 正在同步”或“Trace 暂不可用”，不能伪装成节点没有输入。

MySQL 保存列表摘要，ClickHouse 保存详细事件，避免每次列表查询扫描 Trace 明细。

执行记录列表由 Runtime Query Authority 直接按应用、Workflow、实际调用的 MCP Tool、执行时发起用户与部门、触发方式与名称、状态和创建时间筛选。单维度多值为 OR，维度之间为 AND；部门只匹配执行时记录的精确部门，不展开部门树。Tool 条件使用 `runtime_calls` 的明确 Tool ID 做 `EXISTS`，不从请求 JSON 或已配置资源推断调用事实。

列表使用 15 分钟查询快照和 opaque Cursor；筛选 Hash 覆盖全部条件，任一条件变化必须创建新快照。`search` 只用于 Execution ID、Trace ID 和错误码。Control BFF 只做参数校验、权限范围、当页应用/Workflow 名称批量补充和协议转发，不允许对 Runtime 返回的一页数据再次本地筛选。

## 4. ClickHouse 首期设计

首期使用统一追加表 workflow_trace_events，主要字段：

- tenant_id
- trace_id
- span_id
- parent_span_id
- execution_id
- node_execution_id
- event_type
- event_time
- workflow_id
- workflow_version_id
- node_id
- node_type
- model
- provider
- tool_name
- duration_ms
- input_tokens
- output_tokens
- cost
- status
- error_code
- attributes_json
- content_ref
- content_kind
- content_preview_json

大段 Prompt、Response、Tool Result 和文件放对象存储，ClickHouse 保存摘要或引用。

Trace Writer 应批量写入并支持重试。如果 ClickHouse 不可用，先将 Trace Event 留在队列或本地缓冲中，不阻塞节点完成事务。

## 5. Checkpoint

建议每个成功 Node Execution 之后形成逻辑 Checkpoint。重要 Wait、Approval 和副作用节点可以在执行前后都创建。

Checkpoint 包含：

- Workflow Version
- Execution Snapshot
- 当前图位置
- 已完成节点集合
- Node Activation Frontier
- Edge Delivery Cursor 和待满足输入集合
- Workflow 变量
- 节点输出引用
- Artifact 引用
- Agent State 引用
- Session State 引用
- 父 Execution 和父 Checkpoint
- State Hash

小型状态可以保存在 MySQL，大型 Items 和二进制内容存对象存储。

## 6. Fork Execution

历史执行不可修改。从某节点重新执行时：

1. 找到该节点执行前的 Checkpoint。
2. 创建新的 Execution。
3. 写入 parent_execution_id 和 fork_checkpoint_id。
4. 复用 Checkpoint 之前的节点输出。
5. 重新执行选中节点。
6. 将所有下游视为未执行。

重新执行模式：

- 使用原 Workflow Version 和参数
- 使用兼容的新 Draft
- 覆盖本次节点输入
- Mock 指定节点
- 跳过指定副作用

LLM 和外部工具本身可能不确定，因此 Checkpoint 保证恢复输入状态，不保证重新执行产生相同输出。

## 7. 副作用节点

节点定义必须标明副作用级别：

- None：纯计算
- Idempotent：提供稳定幂等键
- Reversible：存在补偿操作
- Irreversible：不可自动撤销

重新执行 Irreversible 节点前，需要：

- 明确确认
- 或复用旧输出
- 或进入 Dry Run
- 或执行预定义补偿

## 8. Agent 运行

Agent 节点内部循环：

1. 加载消息、Memory、RAG 和上下文。
2. 调用模型。
3. 解析模型 Tool Call。
4. 检查 Tool 是否授权给当前 Workflow。
5. 执行 Tool。
6. 将结果追加到 Agent State。
7. 再次调用模型，直到输出结果或达到限制。

每轮记录：

- iteration
- model request
- model response
- tool request
- tool response
- tokens
- cost
- stop reason
- error

## 9. Agent 循环控制

Workflow 或 Agent 节点可以配置：

- 最大迭代次数
- 最大模型调用次数
- 最大工具调用次数
- 最大 Token
- 最大成本
- 最大持续时间
- 单工具最大错误次数
- 相同工具参数最大重复次数

工具调用指纹由 Tool 名称和标准化参数生成。平台检测：

- 连续相同调用
- A-B-A-B 周期
- 参数只有无意义变化
- 持续返回同一个错误
- 多轮调用后 Agent State 没有实质变化

命中策略：

- 终止节点
- 进入 Error Output
- 暂停等待人工确认
- 返回部分结果和警告

## 10. OpenSandbox

进入 OpenSandbox 的节点：

- Python
- JavaScript
- Shell
- 文件处理
- 浏览器自动化
- 高风险自定义 Tool
- Agent 动态生成代码

Sandbox Manager 负责：

- 创建、复用和销毁
- 模板及镜像版本
- CPU、内存、磁盘和时间限制
- 输入文件上传
- 输出 Artifact 收集
- 网络访问范围
- 短期 Credential 注入
- stdout、stderr 和 exitCode 收集

Agentx 保留与供应商无关的 `SandboxRuntime` Port，`sandbox-manager` 通过 Rust `OpenSandboxAdapter` 直接调用官方 Lifecycle API 和 execd API。Worker 只调用 Agentx 的稳定内部契约，不加载供应商 SDK 或 DTO，也不持有 OpenSandbox API Key。生产链路不得插入 Go/Python Sidecar 或调用 `osb` CLI；官方 Go SDK/CLI 只作为测试差分基准。

协议和版本边界：

- 构建必须固定 OpenSandbox 源码 Commit、Lifecycle/execd Spec Hash 和 Server/execd/egress/模板镜像版本；升级必须显式更新生成物、Fixture 和兼容矩阵。
- OpenAPI 只生成或校验内部 DTO 与普通 HTTP 端点，不视为完整 SDK；SSE、取消、背压、Sandbox 就绪轮询、Endpoint 解析、重试和错误标准化由可审查的 Rust 手写层实现。
- CI 必须校验 vendored Spec Hash 与生成物一致；运行时发现缺失必需字段、未知的不兼容事件或不受支持组件版本时返回 `SANDBOX_PROTOCOL_UNSUPPORTED`，不得猜测字段或降级为成功。

Endpoint 和凭证边界：

- Lifecycle 返回的 Endpoint 必须使用结构化 URL 解析，限定允许的 scheme、host、port 和配置化 CIDR/域名范围；每次刷新都重新校验，禁用跨 Host 重定向，禁止拼接 URL 绕过检查。
- 只转发固定协议定义的鉴权 Header 白名单。OpenSandbox API Key 只发送到配置的 Lifecycle Origin；execd Token 只发送到已校验的 Sandbox Endpoint，不能进入重定向、日志、Trace、Artifact 或用户可见错误。
- Endpoint、Header 或 Token 校验失败必须在网络请求前终止，并记录脱敏的审计原因。

SSE、取消和重试边界：

- SSE 解析必须支持分片和多行 `data`，限制单事件、累计输出和缓冲区大小，并设置连接、空闲和总超时；消费端使用有界队列形成背压，大输出转为 Artifact。
- 取消先停止读取流，再调用 execd interrupt；无论 interrupt 是否成功都进入 Sandbox 终止/回收流程，部分 stdout/stderr 只能标记为部分结果。
- 自动重试仅允许 health/get、Endpoint 解析等只读请求，以及契约明确幂等的 interrupt/terminate；create、command、upload 等操作只有携带并验证服务端支持的幂等键时才能重试。无法判断请求是否已提交时必须返回明确的不确定错误并由 Reaper 对账。

安全基线：

- Sandbox Profile 的强类型网络上限固定为 `{"defaultAction":"deny","egressMode":"none|tcp_proxy"}`。旧枚举不读取、不转换；环境按当前DDL重建。
- Code 节点使用 `networkPolicy.mode=deny|allowlist` 和最多32个目标的域名/IP/CIDR及端口段。只有节点白名单与绑定的 Sandbox Profile `tcp_proxy` 上限同时允许时才能联网；节点不能提升Profile能力。
- OpenSandbox NetworkPolicy只允许DNS和`agentx-egress-gateway`。Manager注入短期`HTTP_PROXY/HTTPS_PROXY/AGENTX_TCP_PROXY_URL`；Gateway逐次校验签名Token中的目标、端口、策略hash和DNS结果。显式白名单可以访问管理员允许的企业私网，但回环、链路本地、Metadata、Kubernetes API、Pod/Service网段、`.svc/.cluster.local`和Agentx内部服务永久拒绝。Token、CA和Credential不进入Definition、命令、Trace、日志或Artifact。
- 固定 OpenSandbox Lifecycle Spec 的网络规则只支持 FQDN，不支持端口字段；Agentx 不发送供应商未定义的 `port/ports`。端口收敛由 Gateway 专用 Service/NodePort/私有 LB 仅映射 Profile Endpoint 到容器 `3129` 实现，生产代理域名/IP 禁止复用其他服务。
- Gateway 对每个 CONNECT 重新解析并固定已验证的公共地址，永久拒绝私网、集群地址、Kubernetes API、Metadata、回环和链路本地地址。代码不能绕过代理直连公网。
- Runtime CONNECT Token 单次使用且最长 60 秒；Sandbox wildcard Token 不超过 Sandbox TTL，并默认限制为单 Token 4 个并发 Tunnel、32 次连接和累计 1 小时。四个签发身份的 KID 必须匹配各自角色前缀，轮换期间只允许当前/上一把公钥短暂重叠。
- 基础镜像使用 Sandbox Profile 指定的镜像 tag，默认只读；临时写入只进入受限工作目录，输出通过 Artifact 收集。tag 不提供 digest 级别的不可变性，生产环境应通过受控 Registry、镜像签名或发布流程保证 tag 不被静默改写。
- CPU、内存、进程数、磁盘、TTL 和租户并发在 Agentx 与 OpenSandbox 两侧同时限制。
- Credential 优先通过 OpenSandbox Credential Vault 或 Agentx 短期凭证代理注入，不得进入命令行、stdout、stderr、Trace 或持久镜像。
- Sandbox 完成、超时、取消、Worker 失联或 Lease 过期时都必须幂等终止；回收失败进入 Reaper，不得把节点标记为虚假成功。

运行环境边界：

- 本地开发和 CI 可使用 OpenSandbox Docker Runtime + runc，用于功能、资源限制和网络策略 E2E。
- runc 不能作为生产级多租户强隔离结论；生产必须使用 OpenSandbox Kubernetes Runtime，并配置 gVisor、Kata 或经安全评审的等价 RuntimeClass。
- 生产门禁必须检查实际 Pod RuntimeClass、网络策略、资源限制和残留 Sandbox，不能只检查 OpenSandbox `/health`。

支持两类生命周期：

- Node Sandbox：单次节点执行，用完释放。
- Session Sandbox：同一 Session 按 TTL 复用。

Worker 只接收标准化结果，不直接依赖 OpenSandbox 内部数据结构。

## 11. 审批节点

Approval Node 是 waitAndResume 节点。

进入审批时：

1. 创建审批前 Checkpoint。
2. 创建 Approval Task。
3. Node Execution 进入 Waiting。
4. Workflow Execution 进入 WaitingApproval。
5. Worker 释放资源。
6. 创建站内通知。

审批动作：

- Approve
- Reject
- Provide Input
- Cancel
- Timeout

审批完成后：

1. 保存审批动作和表单数据。
2. 将审批结果转换为标准 Item。
3. 按 Approved、Rejected 或 TimedOut 端口继续。
4. 将 Execution 重新加入调度队列。

待办中心只解决 Workflow 审批，不扩展成通用 OA。
