# P7-B：LLM 流式输出与多模态透传

## 1. 目标与边界

两件事：

1. **流式**：模型输出以 token 级增量经既有 SSE 管道到达客户端，Playground 对话逐字渲染；最终输出契约不变（增量只是会话层事件，节点输出仍为冻结的一次性结果）。
2. **多模态**：带 image（首期）/audio（跟随验证）的消息在模型请求中以原生 OpenAI 多模态 content 数组传递，不再折叠为纯文本。

不做：Anthropic 等非 OpenAI 兼容原生协议；模型语音输出（TTS/STT）；图片生成；流式回写节点输出契约（`EffectiveOutputContractV1` 不加 partial 端口）；ClickHouse 存储 token delta（量级不合适，Trace 只记汇总）。

## 2. 现状事实

### 2.1 非流式硬编码

- model 节点：`worker_runtime_output.rs:38` `"stream":false`；响应解析 `openai_execution_output()`（:80-178）只读 `choices/0/message`；成功 payload 冻结为 `{text, reasoningContent, structuredOutput, citations, files, usage, finishReason, partial:false}`（:168-177，注释明示不得加别名）。
- Agent ModelPort：`worker_runtime_agent_model.rs:253` `"stream":false`；`block_in_place + block_on` 一次性调用（:260-272）；usage 累加并 UPSERT `agent_session_usages`（:340-373）。
- 计费/幂等：`runtime_calls` reserve/settle/replay（`worker_runtime.rs:1250-1324`）；指纹 + idempotency key（:826-869）。勘察新增：`runtime_calls.usage_estimated` 列已存在（DDL 0001:1026）但 INSERT 语句从未写入该列，回退估算标记只需补写入；model 调用 side_effect 恒为 `irreversible`（`worker_runtime_output.rs:291-311`），`sent` 状态的模型调用不可自动重发——中途断流只能走 attempt 级重试（新 attempt 是真实新流），幂等命中重放才补发合并帧。

### 2.2 事件管道（SSE 已端到端打通，但没有增量事件）

- Gateway SSE `/invocations/{id}/events`（`gateway.rs:445-500`）消费 **`invocation_events` 表**（MySQL 游标 + `Last-Event-ID` 断点续传 + 15s keepalive）；
- 唤醒：Redis pubsub `agentx:v2:invocation:wakeup:{id}`（`sse_wakeup.rs`），当前仅 event_sequencer_loop 一个发布点（`bin/workflow-runtime.rs:513-517`）；
- `invocation_events` 现有 6 种事件类型（accepted/running/cancel.requested/completed/failed/cancelled；勘察修正：`invocation.running` 是**非终态**事件先例，`engine.rs:288-297`——非终态事件过 SSE 已被验证），但仍无任何节点级/增量事件；
- 前端已能解析任意 event 帧（`features/playground/use-invocation-events.ts:18-51`），但当前 SSE 只用作 invalidate 触发器，内容等执行结束一次性拉取。勘察利好：gateway SSE 对 event_type **完全不过滤**（`gateway.rs:476` 任意类型直接透传），`model.delta` 无需改 gateway 代码即可下发，只需补契约与 OpenAPI（现 SSE response schema 是 opaque string，`gateway.rs:1204-1209`）。另注意 sequencer 循环消费的是 execution_outbox→integration_event_log、并不写 invocation_events——delta 直写 invocation_events 就**不经过 sequencer**，必须自带 wakeup 发布点（见 §2.3 勘察新增）。

### 2.3 Agent 内核无 delta 事件

- `CoreEventV1`（`agentx-agent-core/src/ports.rs:82-123`）只有 Agent/Turn/Model/Tool 的 intent/settled 级事件；模型调用一次性返回完整 `ModelResponseV1`（`driver.rs:657`）；
- 运行时 `EventCollector`（`worker_runtime_agent_core.rs:722-742`）把事件攒内存、run 结束后统一回放成 TraceDraft（`worker_runtime_agent_trace.rs`），不进 invocation_events。
- 勘察新增（两处基础设施缺口）：`ClaimedWorkerAttempt`（`engine_types.rs:9-25`）不含 invocation_id（claim SQL `engine.rs:879` 未 select），Worker 写 delta 需加字段；`RuntimeWorker` 无 Redis 字段，`bin/workflow-worker.rs` 的 per-lane `WorkerRedis` ConnectionManager（:85-110）没有传入 `execute`——第二发布点需要把 redis client 注入 RuntimeWorker（保持测试构造 `new_with_provider` 可用）。

### 2.4 多模态只到存储层

- 网关已接收 text/json/image/audio/file parts 并校验 artifact 引用（`gateway.rs:291-305`），上传走 `/artifacts`（50MiB，`gateway.rs:502-635`）；
- `chat_message_payload()`（`gateway.rs:339-367`）把 text parts join 成 `question` 字符串，非文本 part 只变 `files` 引用数组；
- 模型请求只取 question 且 `json_text()` 成字符串（`worker_runtime_output.rs:21-27`；Agent 侧 `worker_runtime_agent_core.rs:690-707` 同样只取文本）；`ModelRequestV1.messages` content 是 `String`（ports.rs:15-20）；
- 模型资源无 capabilities/modality 字段（`platform-control/src/model_api.rs:64-126`，provider 仅 `openai_compatible|custom_http` :812）。

### 2.5 可复用的实现范本

- 插件 `trace.event` 通道：mpsc sink（容量上限+背压丢弃诊断）→ 逐条独立小事务写 trace_outbox（`worker_runtime_plugin.rs:108-153、1359-1463`）——"子进程增量 → 增量事件"的现成模式；
- `invocation_events` 序号机制（`MAX+1 FOR UPDATE`）保证消费端顺序与断线续传天然成立；
- artifact 体系 + `Item.binary/BinaryReference`（`agentx-node-protocol/src/lib.rs:21-38`）可承载多模态字节引用。

## 3. 设计

### 3.1 核心决策

1. **增量事件走 `invocation_events`，不走 trace 流**。delta 是会话实时体验数据，不是观测数据；消费端（SSE 游标 + Last-Event-ID + wakeup）现成，ClickHouse 不存 delta。
2. **最终输出契约不变**。节点 attempt 结算仍写一次性冻结 payload；流式期间 delta 独立记账，节点失败时 delta 丢弃，以 attempt 结果为准。避免引擎状态机、输出契约校验、Fork 重放语义被流式破坏——重放/Fork 时消费的是终态结果，天然幂等。
3. **计费以流结束 usage 为准**：请求加 `stream_options: {include_usage: true}`，最后一个 chunk 带 usage；无 usage 回退 chunk 累加估算并打 `usage_estimated` 标记。`runtime_calls` 幂等 ledger 不变。
4. **节流合并**：Worker 侧按"最多 50ms 或 24 token 合并一帧"节流写 `invocation_events`，避免每 token 一行 MySQL；SSE 下发同样合并。
5. **多模态以模型能力声明为门**：模型资源新增 `capabilities`（`vision`、`audio` 首期）；未声明 vision 的模型收到 image 引用时，行为由节点参数决定（默认报错 `MODEL_INPUT_UNSUPPORTED`，可配置降级为忽略并打标）。
6. **同步内核与流式的桥接**（勘察新增）：`AgentCore::run`/`ModelPort::invoke` 是同步调用（block_in_place + block_on），on_delta 回调内不能再 block_on——回调只 `try_send` 进 mpsc（`PluginTraceSink` 模式，`worker_runtime_plugin.rs:107-153`），由后台 task 以独立小事务（250ms 超时先例 `emit_live_trace_event` :1359-1463）写 invocation_events + publish sse_wakeup；50ms/24-token 节流计时器放后台 task（异步 sleep），不在同步回调里等时间。**sink 必须在 attempt 结算（submit_worker_result）前 drain 完毕**，否则 delta 帧在终态事件之后提交、SSE 客户端永远看不到——这是与 PluginTraceSink（可丢帧）的关键差异，需要带 join 的关闭语义；model 节点路径（async fn）与 Agent 路径统一走同一 sink。

### 3.2 新事件类型

`invocation_events` 增加（复用现有写入与游标语义）：

```text
model.delta
  { "executionId", "nodeAttemptId", "nodeId", "seq", "deltaText",
    "reasoningDeltaText"?, "finishReason"?, "usage"? }   -- 最后带 finishReason 的帧为流结束
delivery.completed / delivery.failed                     -- 由 P7-A 复用同一管道（互不阻塞，先行实现 model.delta）
```

写入点在 Worker（model 节点）与 Agent 运行时（Agent turn），写入后直接 publish sse_wakeup（新增第二个发布点）。

### 3.3 Provider 流式执行

- `WorkerProvider` trait 增加 `post_json_stream()`（或 SSE 变体方法）：`reqwest` `bytes_stream()` 增量读，OpenAI SSE 帧解析（`data: {...}` / `data: [DONE]`），中途错误分类为可重试（网络）与不可重试（协议/鉴权）；
- `openai_chat_request()`（`worker_runtime_output.rs`）按参数 `stream`（节点级开关，默认对 chat 类执行开启）置 `"stream":true` + `stream_options`；
- 幂等语义：流式调用同样走 reserve/fingerprint；**重放（幂等命中）不重发流**，直接以已存结果生成一次性"合并 delta 帧"补发，保证重试路径客户端体验一致；
- 工具调用流式：`tool_calls` 增量拼装在流结束聚合，delta 帧不拆 tool call 片段（首期）。

### 3.4 Agent 内核流式

- `ModelPort::invoke` 增加流式变体：`invoke_stream(&request, &context, on_delta: &(dyn Fn(ModelDeltaChunk) + Send/Sync)) -> ModelResponseV1`（最终仍返回完整响应，驱动循环逻辑不变）；
- `CoreEventV1` 增加 `ModelDelta { attempt, text }`（由 runtime 适配层的 on_delta 直接转发，不进 EventCollector 攒批——EventCollector 保持攒批用于 Trace，delta 走旁路实时通道，参照插件 trace.event 模式）；
- Agent 运行时把 `ModelDelta` 节流后写 `invocation_events`（`model.delta` 带 `agentRun` 标记字段）；工具调用/思考阶段可以发轻量 `model.status` 事件（首期只做 delta，status 列入可选）。

### 3.5 多模态透传

- **模型资源**：`model_api.rs` 增加 `capabilities: ["vision","audio"...]`（Control 校验 + `model_deployments` 列 + Runtime binding `RuntimeResourceConfigurationV1::Model` 透传）；
- **请求构造**：`openai_chat_request()` 的 user content 从 `String` 改为 parts 数组：`[{type:"text",...},{type:"image_url", image_url:{url: data-uri 或 artifact 临时 URL}}]`；artifact → base64 data URI 在 Worker 完成（大小上限 8MiB/图，超限报错 `MODEL_INPUT_TOO_LARGE`）；audio 首期 `input_audio`（base64, 格式按 content_type）；
- **输入管道**：`chat_message_payload()`（gateway）不再把非文本 part 仅折叠为 files——file_input 映射标记 `x-agentx-artifact-array` 的字段保持既有语义；新增约定：Workflow Start Input Schema 中类型为 `string` 且带 `x-agentx-modality: image` 标记的输入，由 `project_chat_message_input()` 投递 artifact 引用数组；model 节点参数 `inputs.image` 绑定该引用，构造请求时解析为 image part；
- **Agent**：`ModelRequestV1.messages` content 改为枚举 `Text(String) | Parts(Vec<ContentPartV1>)`；兼容策略（勘察新增，先决定案 2）：`AgentMessageV1.content` 直改类型会让存量 `agent_sessions` 内嵌 state（deny_unknown_fields）反序列化失败——serde 用 `#[serde(untagged)]` 且 `Text` 变体在前（纯字符串旧数据仍可读），`CORE_CONTRACT_VERSION` 1.1→1.2；Agent 初始 prompt 若绑定 workspace 附件中的 image 引用则构造 Parts；
- **响应侧**：模型输出的多模态（图片生成等）不在本线范围。

### 3.6 前端

- `use-invocation-events.ts`：按 `event` 类型分发（新增 `onModelDelta` 回调），保留 invalidate 行为；
- `conversation-test-workspace.tsx`：气泡维护增量 buffer——`model.delta` 追加渲染，`invocation.completed` 后以最终 parts 替换；断线重连由 Last-Event-ID 兜底补齐中间帧；
- Studio 调试 Rail：模型节点执行中显示流式 tail（复用 delta 事件，只读预览，不参与 Pin/Mock）；
- model 节点参数面板增加 `stream` 开关与多模态输入绑定提示（依赖 Schema 标记）。

### 3.7 观测

- Trace 维持现有 `runtime_call.*` 汇总 span（含 usage/cost/首尾延迟）；增加 `first_token_ms` 属性（首帧到首 delta 延迟）作为流式质量指标；
- 勘察修正：`invocation_events` **当前没有任何清理路径**（runtime retention.rs / gc.rs 均不覆盖，表无 FK 级联）——delta 高频写入会放大表增长，本线必须在 retention/gc 新增按 invocation 生命周期（或 created_at TTL）的清理规则并纳入断言，否则该表成为无限增长点。

## 4. 实施阶段

### P7-B1 契约冻结

- [x] `model.delta` 事件 schema（`agentx-runtime-contracts` engine/gateway 契约 + OpenAPI）；
- [x] `ModelRequestV1` 多模态 parts 枚举、模型资源 `capabilities` 字段、`x-agentx-modality` Schema 标记约定（标记约定入 schema_contract：仅 image/audio 且必须 array 型，`INVALID_MODALITY_SCHEMA`）；
- [x] 错误码冻结：`MODEL_STREAM_PROTOCOL_ERROR`、`MODEL_INPUT_UNSUPPORTED`、`MODEL_INPUT_TOO_LARGE`、`USAGE_ESTIMATED` 标记语义；
- [x] Control/Runtime 迁移（`model_deployments` capabilities 列）与 OpenAPI 再生成。

门禁：契约测试、Schema 测试、boundary check、OpenAPI diff。

### P7-B2 Provider 流式与事件写入

- [x] `WorkerProvider::post_json_stream()`（默认实现 `Err(Denied)` 保持测试替身可编译；实作含 `[DONE]`/usage chunk/错误帧解析，流式臂带 HTTP 状态码门——B6 期间发现并修复状态码吞噬缺陷）；
- [x] 基础设施缺口补齐：`ClaimedWorkerAttempt`/claim SQL 加 invocation_id；RuntimeWorker 注入 redis client；
- [x] model 节点 `stream` 参数接线；delta sink（mpsc + 后台小事务 + 结算前 drain）节流合并写 `invocation_events` + sse_wakeup 第二发布点；
- [x] 计费收口（usage chunk / 回退估算，`usage_estimated` 补进 INSERT 列）与 `runtime_calls` 重放不重发流、补发合并帧；
- [x] invocation_events 清理规则进 retention/gc。
- [x] `first_token_ms`：流式首 delta 计时进 `runtime_calls`（迁移 0012）与 runtime_call Trace span `firstTokenMs` 属性；B6 E2E 断言实测 >0（2026-09-29 run3）。

门禁：mock SSE fixture 单测（正常流、无 usage 流、中途错误、重放）、计费断言、幂等断言。

### P7-B3 Agent 循环流式

- [x] `ModelPort::invoke_stream` + `CoreEventV1::ModelDelta`；driver 调用点切换；
- [x] runtime 适配层旁路实时转发（不进 EventCollector 攒批）；
- [x] `agent_session_usages` 计费路径改读流式最终 usage。

门禁：agent-core 单测（delta 回调时序、工具调用聚合）、压缩/预算路径回归。

### P7-B4 网关与前端流式

- [x] Gateway SSE 对非终止事件透传 `model.delta`（契约与 keepalive 场景测试补齐）；
- [x] 前端事件分发、气泡增量渲染、断线重连补帧；
- [x] Studio Rail 流式 tail。

### P7-B5 多模态透传

- [x] 模型资源 capabilities（表列 + 校验 + binding 透传 + 前端表单，链上 4 个点，契约字段 `#[serde(default)]`）；
- [x] 请求构造 parts 化（model 节点 + Agent ModelPort 两处）+ artifact→data URI 解析与大小门；
- [x] `chat_message_payload`/`project_chat_message_input` 的 `x-agentx-modality` 投递（匹配 part 投递 artifact 引用数组，单测覆盖 image 过滤与通用 file 映射共存）。
- [x] Playground 附件图片 → 模型原生 vision 的端到端联调（2026-09-29 集群全绿：API 级上传→投递→原生 image_url part 断言；浏览器级交互留 UI 覆盖用例）。

### P7-B6 E2E 验收

- [x] 见第 5 节（主链路 2026-09-28 `test_model_streaming.py` 1 passed 373s：API 全链建凭证/模型/工作流/发布/聊天，SSE 断言 delta 先于终态按序到达，慢流/断流/无 usage/重放合并帧四行为；场景 5 vision 端到端(附件上传→x-agentx-modality 投递→echo-vision 断言原生 image_url part, vision-ok:1)与场景 6 无能力模型→MODEL_INPUT_UNSUPPORTED 终态——2026-09-29 全绿; 过程修复 4 个真实缺陷: 上传缺 runtime_objects 行、object_key ascii 列解码、userQuestion 解析结果被原始绑定覆盖(input.question 优先)、模型节点 userQuestion 参数类型收窄为 string; 场景 7 Agent 节点流式/8 UI 覆盖移交; `first_token_ms` 已实测 >0）。

## 5. E2E 验收（临时 Namespace）

使用可编程 mock OpenAI 兼容服务（fixture 已有 echo-node 模式可扩展：SSE chunk 序列、无 usage 流、中途断流、慢流）：

1. Playground 对话（真实模型或 mock）→ 浏览器断言逐字渲染顺序、最终消息与终态 parts 一致；
2. mock 慢流（每 token 100ms）→ 断言 SSE 断线重连后由 Last-Event-ID 补齐，无丢字/重复（序号校验）；
3. mock 中途断流 → 节点按重试策略处理，客户端无半截最终消息；
4. 同请求重试（幂等命中）→ 补发合并帧、无二次计费；
5. 带图片附件的消息（vision 模型 mock）→ 断言 mock 收到原生 `image_url` content 数组；
6. 无 vision 能力模型收到图片 → `MODEL_INPUT_UNSUPPORTED` 终态与 UI 错误呈现；
7. Agent 节点（绑定模型+工具）执行 → 对话式 Playground 逐字渲染 Agent 回复；Trace 中 `runtime_call` span 含 `first_token_ms`；
8. UI 覆盖：stream 开关、图片上传预览、中英文与深浅主题。

## 6. 完成定义

- 模型输出对客户端 token 级流式，断线重连无丢失/重复，终态契约与既有输出契约完全兼容；
- 执行引擎状态机、Checkpoint/Fork、输出契约校验零改动（delta 只是旁路事件）；
- 计费、幂等、重放语义在流式下保持正确（重放不重发流）；
- 图片消息在模型请求中为原生多模态 content，能力声明门禁生效；
- `first_token_ms` 进 Trace；`invocation_events` 有清理规则并有断言；契约测试、单测、Playwright、Kubernetes E2E 全绿。

## 7. 明确不做

- 非 OpenAI 兼容原生协议（Anthropic/Gemini 等）；
- 语音输入输出、图片生成模态；
- 流式写节点输出契约 / `partial` 端口语义（`partial:false` 冻结契约不变）；
- token delta 进 ClickHouse；
- 工具调用参数的增量流式渲染；
- n8n/Dify 节点级流式兼容。

## 2026-10-07 审查修订

- 原生 Chat SSE 保留 Provider 的 tool call ID，支持分片 UTF-8、CRLF/LF、多行 data 与 arguments 拼接。只有 finish_reason 和 `[DONE]` 完整到达才成功；截断、错误帧及未完成的工具调用失败。Provider 未返回 usage 时，以 prompt 与输出估算并明确 usage_estimated。
- delta 通过有界异步通道回压；50 ms 或 24 字符触发写入，结算前 flush 等待真实写入结果。丢帧或存储失败不再继续标记成功。Invocation 事件以父行锁串行分配 cursor，避免结算、delivery 与 delta 的 MAX(sequence) 竞态。
- 手动调试没有 Invocation，使用 Runtime 的 execution_model_deltas 表和独立 cursor；Control BFF 只转发已签名 Query Scope。Studio 持续读取该端点并在终态排空，显示文本、推理尾部和读取错误。该流不占用 Trace 水位，不投影到 ClickHouse；执行 retention 同时删除。
- Playground 依据上传响应的 MIME 类型生成 image/audio/file part，原生图片内容可进入具备 vision 能力的模型节点。
