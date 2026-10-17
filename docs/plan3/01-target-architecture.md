# Plan3 目标架构

## 1. 领域模型

Agent 节点是一个由 Agentx 原生 Agent Core 驱动的有状态智能体，不是 Model Node 与若干资源节点组成的隐藏子 Workflow，也不是调用远程 Pi Runtime 的代理节点。

```text
Workflow Node Attempt
  └─ Agent Core Run
      ├─ 内置且必选 Model
      ├─ Agent loop / Context / Compaction / Session
      ├─ optional Agent Workspace Sandbox
      ├─ conditional read / write / edit / bash
      ├─ 可选 MCP Tools
      ├─ 可选 Skills
      ├─ 可选 Knowledge
      └─ 可选 Long-term Memory
```

Workflow 主连接只传递 Node Input/Output Item。资源附件声明 Agent 可用能力，不推进 Scheduler，也不创建独立 Node Execution。

## 2. 参考实现与 Agentx 实现的关系

参考源为 [earendil-works/pi](https://github.com/earendil-works/pi) Commit `a69bef789bc95abf0acee16f7b4660b70b650bb9`。

| 参考 Pi 概念 | Agentx 目标实现 | 说明 |
|---|---|---|
| `Agent` / `agent-loop` | `agentx-agent-core` Run Driver | 保留消息、Turn、Tool loop、事件、steer/follow-up 行为 |
| `AgentMessage` | Agent Core Entry Payload | 支持 user/assistant/toolResult 及受控自定义消息 |
| `transformContext` / `convertToLlm` | Context Projector | 从完整历史构造当前模型可见 Context |
| Session Entry Tree | Runtime Agent Session Entries | 不使用 Pi JSONL/SQLite 格式；首期只启用 `main` Lane |
| Operation State | Runtime Agent Operation State | 完整状态作为恢复程序计数器，不靠日志反推 |
| Compaction | Agent Core Compaction | 阈值、overflow、Summary、retained tail 和 Usage 进入权威状态 |
| `read/write/edit/bash` | Optional Workspace Sandbox + Core Tool Definition | Agent 未选 Sandbox 时不注册，选择后经 OpenSandbox Adapter 执行 |
| Usage Ledger | Agentx Runtime Call/Usage Ledger | Token、成本、工具副作用仍由 Runtime 权威入账 |

不要求 Rust API、数据库 Schema 或事件名与参考仓库逐字相同；要求冻结的行为 Fixture、状态机不变量、故障恢复和用户可见语义一致。

## 3. 目标调用链

```text
Control Plane
  Workflow Draft / Resource Grant / Model / Skill / Memory / Knowledge
        │ publish
        ▼
Execution Spec Bundle 2.0
  Agent Definition + Core Profile + exact Resource Bindings
        │ prepare / activate
        ▼
Runtime Gateway / Workflow Runtime
  Invocation → Execution Snapshot → Agent Node Attempt
        │ queue / claim / lease
        ▼
workflow-worker (Rust, authority holder)
  ├─ Agent Runtime Adapter
  │    ├─ load/create Agent Session and complete Operation State
  │    ├─ construct agentx-agent-core
  │    └─ drive Run until terminal/suspend/cancel
  │
  ├─ agentx-agent-core (Rust library, no infrastructure client)
  │    ├─ message/context projection
  │    ├─ turn/tool loop and queues
  │    ├─ threshold/overflow compaction
  │    ├─ durable state transition requests
  │    └─ typed lifecycle events
  │
  └─ Runtime Adapters
       ├─ Model → Egress Gateway → Provider
       ├─ HTTP/SSE MCP → Egress Gateway → MCP
       ├─ stdio MCP → sandbox-manager Process Session
       ├─ Memory/Knowledge → governed adapters
       ├─ Skill → signed Runtime Object
       └─ read/write/edit/bash → sandbox-manager → OpenSandbox
        │
        ▼
Runtime MySQL + Runtime OSS + Trace Outbox
```

没有 `pi-agent-runtime` Service，没有 Rust↔TypeScript gRPC，也没有第八构建产物。

## 4. 模块职责

### 4.1 platform-control

- 管理 Model、MCP、Skill、Memory、Knowledge、Sandbox Profile 和 Grant。
- Compiler 校验 Agent 必选 Model、可选 Workspace Sandbox、Session Policy、四工具可用性和外挂能力闭包。
- Publisher 固化 Agent Core Contract Version、参考行为版本、Model Revision、Sandbox Profile、资源闭包和授权证据。
- 不执行 Agent loop，不保存运行消息或 Compaction。

### 4.2 workflow-runtime

- 创建 Execution/Attempt、调度、Claim、Lease、Fencing 和恢复。
- 管理 Agent Session、Entry、Operation State、Usage Reference、CAS 和 Retention。
- 将 Agent Workspace Checkpoint 与 Execution/Fork/Retention 关联。
- 不实现 Agent loop 的消息和工具决策。

### 4.3 workflow-worker Agent Runtime Adapter

- 从冻结 Bundle 和 Execution Snapshot 构造 `AgentRunInputV1`。
- 为 Agent Core 实现 Model、Tool Effect、State Store、Clock、Budget 和 Event Sink trait。
- 在外部副作用前提交 Intent，在结果可见后原子提交 Settlement、Usage 和下一完整 Operation State。
- 验证 Attempt Lease/Fencing、Deadline、稳定 Action ID、预算和 Grant/Policy Epoch。
- 把 Core Event 映射到 Agent Run/Turn/Tool/Compaction Trace 和应用流式输出。

### 4.4 `agentx-agent-core`

- 独立 Rust crate，禁止依赖 SQLx、Redis、OSS SDK、Vault、Reqwest、Kubernetes Client 和宿主文件 API。
- 维护 AgentMessage、Context Projection、Turn/Tool loop、steering/follow-up、Compaction 和 Operation State Transition。
- 只通过 trait 请求 Model/Tool/State Effect；不持有 Provider Credential、数据库连接或 Sandbox Endpoint。
- 其状态必须完全可序列化、可校验、可恢复；不可逆效果不能只存在于 Future/闭包/进程内变量。
- 首期只启用一个 `main` Lane；多 Lane、导航、分支和子 Agent 不进入本计划。

### 4.5 sandbox-manager

- 继续作为 OpenSandbox Credential、Endpoint、Lease 和 TTL 的唯一管理者。
- 为 Core `read/write/edit/bash` 提供稳定的文件、Patch 和 Command Adapter。
- 为 stdio MCP 提供受 Lease/Fencing 管理的长驻 Process Session：start、stdin write、stdout frame、stderr diagnostics、wait、interrupt、terminate 和 reconcile。
- 不感知 Agent Prompt、Model 或会话消息。

## 5. Agent 内部 Model

Agent Inspector 使用统一 Resource Picker 选择一个 `model/use` Reference。引用继续参加可见性、Workflow Service Identity Grant、发布冻结、撤权、价格、Token 和 Trace。

Model Reference 不带 Editor `bindingId`，因此不会物化为 Attachment Node 或 Binding Edge。独立 Model Node 继续表示一次模型调用，不能连接到 Agent。

Agent Core 只接收冻结的 `ModelDescriptor` 和 `ModelEffect` trait；Provider 格式转换、Credential、Egress、重试计费和 Runtime Call Ledger 留在 Worker Adapter。

## 6. Agent Core 状态模型

参考 Pi Harness 的三类权威数据，在 Agentx Runtime 中实现为：

```text
Agent Session Entries     append-only：message / tool_result / compaction / custom
Agent Session Registers   mutable：lane state / operation meta / complete operation state
Usage Ledger              append-only：model / compaction / external tool usage
```

核心不变量：

1. Entry 和 Usage 写入后不可修改；Compaction 改变模型投影，不删除完整历史。
2. `operation_state` 是完整程序计数器，恢复不扫描历史猜测下一步。
3. 一个持久 Session + Agent Node 同时最多一个开放 Operation。
4. 每个 Provider/Tool 外部效果采用 `intent commit → effect → settlement commit`。
5. Intent 已提交但 Settlement 未提交时，按照显式 Replay Policy 处理：安全读可重放，不安全写产生稳定 Unknown/Interrupted Tool Result，禁止盲目重复。
6. Terminal Transaction 原子写最终结果、清理开放 Operation Register 并释放 Session Busy。
7. Core State Transition 与 Attempt Lease/Fencing 绑定；失去 Lease 的 Worker 不能提交任何 Settlement。

## 7. 可选 Agent Workspace Sandbox 与内置工具

### 7.1 工具契约

| Tool | 参考语义 | Agentx 执行 | Replay / Execution |
|---|---|---|---|
| read | 相对路径、范围读取、文本/图片与截断 | OpenSandbox File Adapter | safe / parallel |
| write | 完整内容写入、父目录与结果摘要 | OpenSandbox File Adapter | never / sequential |
| edit | 精确替换、冲突、Diff 摘要 | OpenSandbox Patch Adapter | never / sequential |
| bash | command、cwd、timeout、流式 stdout/stderr | OpenSandbox Command Adapter | never / sequential |

Schema、错误、截断阈值和结果呈现由 `CoreToolContractV1` 冻结，并通过对参考 Commit 的差分 Fixture 验证。Agentx 可以增加安全限制，但不得悄悄改变模型看到的成功/失败语义。

所有路径相对于 Agent Workspace Root。拒绝绝对路径、`..`、软/硬链接越界、设备文件、挂载逃逸和跨 Workspace Handle。

Agent Workspace Sandbox Reference 的基数为 `0..1`：

- 未选择：Core Tool Registry 中不存在 `read/write/edit/bash`，System Prompt 也不描述这些能力；模型伪造同名调用返回未注册 Tool，不触发任何 Sandbox 创建。
- 已选择：Publisher 固化精确 Sandbox Profile Version，Runtime 获取独立 Lease 后注册四工具。
- stdio MCP 自己携带的 Runtime Sandbox 不满足这项条件，不能间接开启 Agent 四工具。

### 7.2 Workspace 生命周期

- `invocation`：Attempt-scoped Workspace，Run 结束后按 Retention 回收。
- `application_session`：Session + stable Agent Node scoped Workspace，按 TTL 复用并通过 Checkpoint 恢复。
- 同一 Agent Session 同时只允许一个可写 Run；并发输入排队或返回 `AGENT_SESSION_BUSY`。
- 取消、超时和 Lease 丢失停止 Core 驱动、撤销 Handle，并终止或释放 Sandbox Lease。

### 7.3 两类 Sandbox 不共享

| Sandbox | 配置所有者 | 基数 | 用途 | Runtime Scope |
|---|---|---:|---|---|
| Agent Workspace Sandbox | Agent Definition Inspector | 0..1 | 四个内置工具 | Attempt 或 Agent Session |
| MCP Runtime Sandbox | stdio MCP Server Version | 1 | MCP 长驻进程及其私有文件 | Agent Run + MCP Server Version |

两者有不同的 Resource Reference、Grant、Profile Version、Lease、Workspace、Credential 和网络策略。Publisher 可以把两者同时放进同一 Bundle，但 Runtime 不合并权限、不共用目录，也不因其中一方存在而推断另一方存在。

## 8. 外挂能力映射

### 8.1 MCP Tool

- 每个 Binding 映射为一个 `AgentTool`，名称使用发布时 `tool_name_snapshot`。
- Tool 明确声明 input schema、side effect、replay 和 sequential/parallel；不再只选择首个 MCP。
- 名称冲突在发布时失败，运行时不重命名。
- Streamable HTTP/SSE 继续通过受控 Egress 调用，不要求 Sandbox。
- stdio MCP Server Version 创建时必须选择一个 Runtime Sandbox Profile；自动发现的 Tool Version 继承 Server/Sandbox 版本，不逐 Tool 重复配置。
- 同一 Agent Run 中来自同一 stdio Server Version 的多个 Tool 共用一个 MCP Process Session；进程只解析 stdout JSON-RPC，stderr 作为受限诊断流。
- Agent 没有 Workspace Sandbox 仍可调用 stdio MCP，因为 MCP 使用自己的 Sandbox；反过来，MCP Sandbox 不开放 Agent 四工具。

### 8.2 Skill

- Skill Version、入口 `SKILL.md`、资产和递归依赖全部来自签名 Bundle。
- 参考 Pi 的 Skill Prompt/Resource 组合语义，但禁止从用户目录、项目目录或 npm/git 动态发现。
- Skill 依赖的 MCP/Model/Credential/Knowledge/Memory 逐项保留 Grant。

### 8.3 Knowledge

- 每个 Binding 注册稳定的检索工具；由 Agent 自主调用，不在每轮无条件注入全部结果。
- Query、Top K、Citation、耗时和错误进入 Ledger/Trace。
- 检索结果作为不可信数据，不能覆盖 System Prompt 或工具授权。

### 8.4 Long-term Memory

- Recall 可在当前 User Message 前作为带来源的 Context Entry 注入。
- 写入、更新和删除通过受权 Tool 或明确后台策略完成，不自动保存全部 Transcript。
- Memory 使用授权同时允许读写；内容不是 System Instruction。

## 9. Session、Context 与 Compaction

### 9.1 两类上下文

- **Run 内上下文**：Agent loop 必需，始终存在，包含本 Run 的 user/assistant/toolResult。
- **跨 Execution 短期上下文**：由 `sessionPolicy.mode=application_session|invocation` 显式决定。

Studio 可以显示“跨执行保留上下文”开关，但 Definition/Bundle 必须保存枚举。用户身份不参与短期 Session Key，所以同一用户再次执行不会自动加载短期历史。

### 9.2 持久 Session Key

```text
tenant_id
+ application_deployment_id
+ application_session_id
+ workflow_id
+ stable_agent_node_key
```

`application_session` 缺少可信 Application Session ID 时明确失败；`invocation` 创建临时 Session，并且下一次 Execution 不读取它。

### 9.3 Context Projection

Agent Core 从当前 Lane 的完整 Entry 链生成模型 Context：

1. 找到最新有效 Compaction Entry。
2. 投影 Summary + retained tail + 之后的 user/assistant/toolResult。
3. 注入受权的外部 Context Entry，但过滤 UI/Trace-only 自定义消息。
4. 将当前 User Message 追加一次，避免 Application Message 与 Invocation 输入重复。
5. 依据模型能力转换为 Provider-neutral Message，再由 Model Adapter 转换 Provider 格式。

### 9.4 Compaction

- 自动压缩属于 Agent Core，始终开启；不提供关闭开关。
- 仿照参考实现冻结 threshold 与 overflow 两条路径、`reserveTokens`、`keepRecentTokens`、结构化 Summary 和 retained tail。
- Compaction 模型调用走同一个 Agentx Model Adapter，产生独立 Runtime Call、Usage、成本和 Trace。
- Compaction Entry 追加到完整历史；旧 Entry 不删除，下次 Context Projection 从最新 Compaction 开始。
- 压缩准备、模型请求 Intent、Usage、结果和下一 Operation State 都可恢复。

## 10. 长期记忆身份

当前身份和作用域以 [应用用户与记忆连接](../plan8/10-application-user-memory.md) 为准。用户身份来自平台登录、渠道验签后的发送者或应用部署指定的 API 输入参数。API 参数为直接声明，不验证其账号、真实性或归属，来源与平台操作人分开存储。无身份时个人长期记忆不可用，不退化为共享范围。

记忆键包含租户、环境、应用、工作流、稳定节点和来源用户；不包含发布版本、部署 ID、API Key 或会话 ID。Studio 使用独立调试域。

## 11. 故障与恢复

| 故障/边界 | 目标行为 |
|---|---|
| Worker 在模型请求前强退 | Intent 未提交则不调用；已提交则按 Runtime Call 状态决定重试或 Unknown |
| Worker 在 Tool Effect 中强退 | `safe` 可按持久参数重放；`never` 生成 Interrupted/Unknown Tool Result 后继续或失败 |
| Worker 在 Settlement 后强退 | 新 Worker 从完整 Operation State 继续，不重复已结算效果 |
| Sandbox 强退 | Reaper 清理；Workspace 从 Checkpoint 恢复或明确失败 |
| MySQL 不可用 | 不提交 Intent/Settlement/State，停止推进 |
| Runtime OSS 不可用 | 不提交依赖 Object Reference 的新状态 |
| Redis 丢失 | 从 MySQL Attempt/Outbox 重建，不丢 Session Entry/Operation State |
| ClickHouse 不可用 | Trace 延迟补投，不改变 Agent 结果 |
| Control 全离线 | 已激活 Bundle 继续执行，Worker 不回读 Control |

## 12. 部署与容量

- 构建产物仍维持现有七类；`agentx-agent-core` 是 Rust Library，不新增 Deployment/Service。
- Workflow Worker 镜像不增加 Node.js、npm lockfile 或 Pi CLI。
- Worker 继续使用现有 ServiceAccount、NetworkPolicy、PDB 和滚动发布边界。
- 新增容量指标：开放 Agent Operation、Context Token、Compaction、Model/Tool effect pending、Session Busy、Core 驱动时长和 State 大小。
- Worker `2→4→2`、Drain、PDB 和滚动升级必须证明没有进程内 Session 粘性；任一新 Worker 可从 Runtime 权威状态恢复。
- Core crate 依赖边界由 Cargo/static check 固定，禁止未来偷偷引入基础设施客户端。
