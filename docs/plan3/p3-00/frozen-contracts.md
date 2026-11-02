# P3A-006 冻结契约说明

状态：`accepted`。机器可读 Schema 位于 `contracts/schemas/agent-core-v1`，Rust 权威类型位于 `src/crates/agentx-agent-core/src/contracts.rs`。

## 1. Definition 6.0 与 Manifest 2.0

Agent Definition 必须显式包含：

- 一个精确版本 Model Reference；缺失返回 `AGENT_MODEL_REQUIRED`。
- 一个显式 `sessionPolicy.mode=application_session|invocation`；持久模式缺可信 Session ID返回 `AGENT_SESSION_REQUIRED`。
- 可选一个 Workspace Sandbox Reference；重复返回 `AGENT_RESOURCE_SLOT_INVALID`。
- `mcpTools/skills/knowledge` 为 `0..N` Attachment，`longTermMemory` 为 `0..1` Attachment。

Manifest Slot固定为：

| 名称 | 类型 | placement | 基数 |
|---|---|---|---|
| `model` | model | inspector | 1 |
| `workspace_sandbox` | sandbox_profile | inspector | 0..1 |
| `mcp_tools` | mcp_tool | canvas | 0..N |
| `skills` | skill | canvas | 0..N |
| `knowledge` | rag | canvas | 0..N |
| `long_term_memory` | memory | canvas | 0..1 |

Model 不再生成 `bindingId`、Attachment Node 或 Canvas Port。独立 Model Node仍表示单次模型调用，不能连接 Agent。

## 2. Bundle 2.0 与传递闭包

相同 Definition、授权快照和资源版本必须得到确定的 Bundle Hash。Bundle 固化 Core Contract/参考 Commit、Model、可选 Agent Sandbox、派生四工具、附件版本及 Grant/Policy Epoch。

依赖闭包必须逐项存在显式 Grant：

```text
Agent → Model Version → Credential
Agent → Workspace Sandbox Profile Version
Agent → MCP Tool Version → Server Version → Runtime Sandbox Profile Version → Credential
Agent → Skill Version → signed objects/recursive dependencies
Agent → Knowledge Binding
Agent → Long-term Memory Binding + allowed operations
```

父资源授权不隐式授予子资源。Agent Workspace Sandbox不满足 MCP Runtime Sandbox依赖，反向也不成立。

Compiler/Publisher 验证：

- `workspaceSandbox=null` 时 `coreTools=[]`，Bundle 不含四工具 Grant。
- 有 Workspace Sandbox 时 `coreTools` 必须恰好为 `read/write/edit/bash`，名称冲突在发布阶段拒绝。
- 未授权、已撤权、版本不匹配或 Policy Epoch过期的能力不能进入模型 Tool 列表或 Context。
- Bundle 不保存 Session消息、Memory结果、Credential明文、Sandbox Endpoint、宿主路径或 Pi npm/JSONL配置。

## 3. AgentRunInputV1 与 Core Ports

Worker 在进程内构造 `AgentRunInputV1`，包含稳定 Run/Session ID、Fencing、Deadline、Model Reference、可选 Workspace Binding、授权 Attachment Tool、Prompt、队列输入和 Compaction Policy。

`continueCompleted` 是必填的 Host 输入接收决定。业务 Worker 固定为 `false`，完成后的原请求重试不得消费其他 Workflow Execution 的排队消息；plan9 两个设计态 Profile 固定为 `true`，只允许仍有效的原 Run 处理已接收且尚未消费的指导。Core 仅对成功完成的状态使用此决定，取消、失败、预算终止和未知结果保持终态。它不属于模型、工作流或浏览器可配置的权限。

Core 的唯一基础设施出口：

```text
ModelPort.invoke(ModelRequestV1, EffectContextV1)
ToolPort.execute(ToolCallV1, EffectContextV1)
StatePort.load / commit(expected_version, fencing_token)
EventPort.publish(CoreEventV1)
ClockPort.now_millis
BudgetPort.admit_turn / charge_model / charge_tool
```

`EffectContextV1` 必须包含稳定 Operation/Effect/Idempotency ID、Fencing 和 Deadline。任何 Secret、DSN、Endpoint 或宿主路径不得进入 Core State。

## 4. Session、Context、Compaction 和 Operation

- Session State可序列化，消息存储与模型 Context Projection分离。
- Projection固定为“最新 Compaction Summary + retained tail + cut之后的消息”；完整消息历史不被删除。
- steering 在当前 Tool/Turn结束后、下一模型调用前消费；无 steering 且 Agent本应结束时才消费 follow-up。
- Runtime 排队的 Workflow Execution 使用 `retry` 行并绑定自己的 Attempt；StatePort 只将当前 Attempt 的该类行投影给 Core，其余由既有持久 Wakeup 恢复原 Execution。待处理表是队列唯一权威，Register 中的历史副本不能复活已消费或其他任务的消息。
- threshold 在模型调用前触发；Provider overflow触发 overflow Compaction并只重试一次。Compaction是独立 Effect、Usage和 Trace。
- Operation 使用 `IntentPersisted → EffectStarted/EffectCompleted → Settled*`；无法判定的副作用进入 `UnknownOutcome`。

## 5. stdio MCP Process Session

MCP transport变为封闭 tagged enum。`stdio` Server Version的 `runtimeSandbox` 恰好一个，否则返回 `MCP_STDIO_SANDBOX_REQUIRED`；HTTP/SSE 禁止携带该字段。

`SandboxProcessSessionContractV1` 冻结 Process ID、Server Version、Runtime Sandbox精确版本、分离的 command/args、有界 Frame、Idle Timeout、Heartbeat 和 Lease/Fencing。P3C-002 的接口必须提供：

```text
start → stdin(frame) / stdout(frame) / stderr(bytes)
      → wait | interrupt | terminate
      → reconcile(lease, fencing) → running | exited | unknown
      → cleanup
```

stdout 只承载有界 MCP JSON-RPC Frame；stderr 不进入协议流。同一 Agent Run + MCP Server Version最多一个活跃进程并复用 Tool Call，跨 Server/Run不复用。

## 6. 契约生成与拒绝规则

```powershell
cargo run -p agentx-agent-core --bin generate-agent-core-contracts -- contracts/schemas/agent-core-v1 contracts/openapi/agent-core-contracts-v1.json
cargo test -p agentx-agent-core --test schema_drift
```

Serde 类型使用封闭枚举和 `deny_unknown_fields`。旧 Agent v1字段、未知枚举、非法状态组合直接拒绝，不提供兼容 DTO、migration、Feature Flag 或 fallback。
