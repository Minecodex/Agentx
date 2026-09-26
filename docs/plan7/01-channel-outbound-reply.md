# P7-A：IM 渠道出站回复闭环

## 1. 目标与边界

补齐 plan4 留下的出站半边：入站消息触发 Workflow 后，把结果通过原平台机器人回复出去，形成"群内 @机器人 → 工作流执行 → 机器人回话"的完整闭环。本线取代 plan4 §20 的方案讨论，将其 L1/L2/L3 分层落为正式契约与实现。

范围：

```text
Execution 终态 / 回复节点触发
  → Delivery Outbox（权威投递记录）
  → Provider Send Client（钉钉/飞书/企微）
  → 受控公网出口（egress-gateway 白名单）
  → 投递状态查询 / 重试 / 死信
```

不做：被动同步回复（企微 5 秒限制不可满足，plan4 §14.4 已结论）；把平台凭证或 sessionWebhook 映射进 Workflow 输入；设计时绑定具体渠道实例；为每个平台复制一套节点执行链路。

## 2. 现状事实

- 入站已全部落地：HTTP 回调验签/Challenge/幂等（`src/services/agentx-v2-runtime/src/webhook.rs`）、钉钉 Stream（`stream/dingtalk_stream.rs`）、飞书 WS 长连接（`stream/feishu_ws.rs`）；企微仅回调。normalize/map_input 已从 webhook.rs 拆出供 stream 复用。
- `sessionWebhook` 已进入 Trigger Context 快照（plan4 P4-06）， expressly 留作出站投递；渠道凭证已内置化（`channel_config_json` + Vault，plan4 §15）。
- 无任何出站代码：webhook.rs 无 reply 发送逻辑。egress 现状（勘察修正）：`egress.rs` 的 `http_provider_services()`（:561-570，默认 `echo-mcp,echo-node,lightrag,mem0`）只约束**集群内明文 HTTP fixture 服务**；公网 HTTPS 走 CONNECT 隧道（`client_for_url` :372-412 签发单次 token，egress-gateway 校验 CONNECT-only/JWT/端口 443/非私网段），**没有主机名白名单机制**——平台 API 域名今天已能出网（钉钉/飞书 Stream bootstrap 就在用同一通道）。「为平台 API 域名单独放行」实际是新增机制：在 delivery Send Client 内做 per-provider 域名后缀校验（见 §3.4）。
- Invocation 快照保存 Trigger Context（会话 ID、发送者、sessionWebhook 等），是出站寻址的数据源（`execution.rs` project_chat_message_input 同源）。
- Runtime 已有成熟 outbox 模式可复制：`execution_outbox`/`trace_outbox` + claim/lease + Redis Stream 派发（`bin/workflow-runtime.rs` 的 outbox/event 循环）。claim/lease/fencing 具体写法范本：`execution.rs:107-162`（SKIP LOCKED + fencing_token + 条件写全部带 owner+fencing+lease 未过期）、`trace_delivery.rs:337-435`（含 available_at 退避与 Redis Stream 丢失重建）。
- 渠道凭证模板缺口（勘察新增）：`webhook_provider_templates.rs:40-91` 现只覆盖验签/长连接凭证（企微 token/encodingAESKey、钉钉 stream clientId/secret、飞书 appId/appSecret）；**出站所需的企微 corpid/corpsecret/agentid、钉钉正式 API robotCode 均不在模板中**，P7-A1 契约阶段必须补充（敏感值走 Vault，非敏感进 channel_config_json）。
- builtin 原生节点是**纯函数无 IO**（`worker_runtime_builtin.rs:11-18`，拿不到 pool/Vault/HTTP）；worker 结果统一在 `engine.rs submit_worker_result_resolved` 的 MySQL 事务里结算——L2/L3「与 attempt 结算同事务入队」必须是两段式（见 §3.5）。
- BFF 查询有完整先例：`runtime_bff.rs search_executions`（:381-520）权限过滤 → 请求 content_hash → delegation token → POST runtime 内部 query API → 回填名称；deliveries API 照抄该三件套。
- `AGENTX_RUNTIME_ROLES` 白名单（`bin/workflow-runtime.rs:377-392`）与 helm values `runtime.services.workflowRuntime.roles` 均需新增 `delivery`。

## 3. 设计

### 3.1 三层回复能力（采纳 plan4 §20 分层）

| 层 | 形态 | 用户感知 |
|---|---|---|
| L1 渠道自动回复 | 渠道配置 `reply { enabled, outputField, 模板 }`，Execution 终态后由投递器取输出原路回发 | 工作流零感知，勾选即用 |
| L2 回复来源会话节点 | 统一"回复消息"节点（`reply_message`）：内容 = 任意 workflow 变量；目标默认 = 来源会话，运行时经 ExecutionOrigin → Invocation Trigger Context 解析 | 工作流控制回复内容与时机，跨平台复用 |
| L3 发送到指定会话节点 | `send_message` 节点：内容 + 显式目标（channel 解析：显式渠道 ID → 该 provider 唯一渠道 → primary 渠道） | 主动推送、定时通知场景 |

L2/L3 都是内容/寻址分离的单一通用节点，provider 由渠道决定，不做每平台一个节点。

### 3.2 数据模型（Runtime MySQL 新迁移）

```text
delivery_outbox
  id, tenant_id, invocation_id, execution_id
  channel_binding_id        -- 来源渠道（webhook binding 快照 id）
  provider                  -- dingtalk / feishu / wecom
  target_json               -- 投递目标快照：conversation_id、sender_id、session_webhook(+expired_at) 或显式目标
  credential_ref_json       -- 渠道凭证 Vault 引用快照（发布时冻结）
  payload_json              -- 已渲染的回复内容（文本 + 可选 artifact 引用）
  status                    -- pending / delivering / delivered / failed / dead
  attempt_count, next_attempt_at, last_error_code, last_error_message
  provider_message_id       -- 平台返回的消息 ID（对账）
  idempotency_key           -- 唯一：execution_id + node/origin + seq，防重复投递
  created_at, updated_at

delivery_dead_letters       -- 死信归档（超过 max attempts 的记录迁入，供查询与人工重放）
```

要点：

- 投递记录是 Runtime 权威状态，与 `invocation_events` 同域；状态机由新的 delivery 投递循环推进；
- `credential_ref_json` 与 target 均为投递时刻冻结快照，渠道配置后续修改不影响已入队投递（与 Bundle 冻结语义一致）；
- 入队与 Execution 终态同事务提交（L1），或与回复节点 attempt 结算同事务（L2/L3），不依赖进程内存。
- 幂等键的 origin 维度必须区分 L1 终态入队与 L2/L3 节点入队（同一 Execution 可能既有终态回复又有节点回复），键空间形如 `execution_id:{origin}:{seq}`，origin ∈ {terminal, node:{node_id}}；
- `sessionWebhookExpiredTime` 为**毫秒**时间戳，投递循环判过期需毫秒→秒换算；`application_invocations.trigger_context_json` 即出站寻址权威数据源（webhook.rs `dispatch_event` :562-609 写入，含会话/发送者/sessionWebhook/过期时间）。

### 3.3 投递循环

复用 runtime 后台 Role 模式，在 `workflow-runtime` 增加 `delivery` Role（MySQL lease + fencing，多副本安全）：

```text
claim (SKIP LOCKED, next_attempt_at <= now, status pending/retry)
  → 按 provider 分派 Send Client
  → 成功：delivered + provider_message_id
  → 可重试失败（网络/5xx/限流）：attempt_count+1、指数退避 + 抖动（上限 5 次）
  → 不可重试失败（4xx 凭证失效/目标不存在/内容超限）：直接 failed/dead
```

投递事件写入 `invocation_events`（`delivery.completed` / `delivery.failed`），复用 SSE 游标流与 sse_wakeup，前端执行详情可见回复投递状态。注意（勘察新增）：gateway SSE 的唤醒由 sequencer 循环在处理 `execution_outbox` 的 `runtime_event` 行后触发——delivery 循环完成时必须**同时入队 runtime_event 类型的 execution_outbox 行**（携带 invocation_id）才能借既有链路触发 sse_wakeup，仅写 invocation_events 不够。

### 3.4 Provider Send Client

| 平台 | 路径 | 说明 |
|---|---|---|
| 钉钉 | `sessionWebhook` 优先（注意 `sessionWebhookExpiredTime`，过期回退机器人正式 API：robotCode + 会话/用户 ID + 渠道凭证） | sessionWebhook 只在 Trigger Context 快照中，不暴露给 Workflow 输入 |
| 飞书 | `POST /im/v1/messages`（chat_id + 渠道凭证换 tenant_access_token，token 短缓存） | receive_id_type=chat_id |
| 企微 | 主动消息 API（userid/touser + 应用凭证换 access_token） | 不做被动同步回复 |

- 出站全部经 egress-gateway（HTTPS CONNECT 隧道，复用 `ProviderHttpClient::from_env(EgressRole::WorkflowRuntime)`，`stream/mod.rs:45-77` 已示范）；因 gateway 无主机名白名单（见 §2），**域名管控在 Send Client 内实现**：目标 host 必须命中该 provider 的允许后缀（钉钉 `oapi.dingtalk.com`、飞书 `open.feishu.cn`、企微 `qyapi.weixin.qq.com`；sessionWebhook 的 host 单独校验钉钉域），不命中报 `DELIVERY_PROVIDER_REJECTED`；
- 自研 client（与 plan4 §16.3 同一决策）：所需 API 面为单条消息发送 + token 刷新，预估每平台 100–300 行，`reqwest` 已有；
- 回复内容首期只支持纯文本（Markdown 按平台能力可选），文件/图片回复明确不做（artifact 下载链接拼接属于模板能力，后续评估）。

### 3.5 节点协议（L2/L3）

新增两个 builtin 原生节点，进 `src/plugins/builtin/core/manifest.json`：

```text
reply_message（L2）
  参数：content（任意 workflow 变量引用，模板可混排）
  无显式目标参数；执行时若 Execution 无 IM Trigger 来源 → 节点失败 REPLY_TARGET_UNRESOLVED
  语义：非终态副作用（入队 delivery_outbox 后节点即成功，投递异步）

send_message（L3）
  参数：content + channelId（引用/固定值） + targetConversationId（变量/固定值，可选 senderId）
  发布时解析渠道：显式 ID 不存在 / provider 无渠道 / 多渠道歧义 → 阻塞发布（复用"必填 Start Input 未映射不能发布"门禁语义）
```

两者输出契约：`{ deliveryId, status: "queued" }`。节点执行不等待平台投递结果（与"Runtime 只返回快速 ACK"一致）；需要投递结果的场景由后续分支读 `delivery.completed` 事件或执行详情。

实现形态（勘察新增，关键设计）：builtin 节点无 IO，采用**两段式**——

1. 节点执行（纯函数）只渲染 content、产出结构化投递 intent（provider 无关），输出 `{ deliveryId, status: "queued" }`，其中 `deliveryId = deterministic_uuid(attempt_id, b"delivery:{node_key}")` **确定性派生**，保证节点输出与结算事务写入的 delivery_outbox 行 id 一致；
2. `engine.rs submit_worker_result_resolved` 结算事务识别 node_type 为 `reply_message`/`send_message` 时，从 intent INSERT `delivery_outbox`（同事务，满足"入队与结算同事务"）；`reply_message` 的目标解析（ExecutionOrigin → invocation trigger_context）也在此处做，数据库 IO 只发生在结算侧；
3. 非 IM 来源的 `reply_message`：结算侧解析不到目标时不产生 delivery 行，attempt 直接置失败 `REPLY_TARGET_UNRESOLVED`。

### 3.6 渠道配置扩展（L1）

`application_webhooks.channel_config_json` 增加：

```json
{ "mode": "...", "fields": { ... },
  "reply": { "enabled": true, "outputField": "answer", "template": "{{output.answer}}" } }
```

- Control 校验 `outputField` 必须存在于当前 Workflow Start/输出 Schema 映射；`template` 只允许引用输出字段的变量模板；
- 发布 Deployment 时随 trigger 清单一起冻结（复用 plan4 §15.1 的 revision 机制），Runtime 用冻结快照投递。注意（勘察新增）：`RuntimeTriggerConfigurationV1` 是 `deny_unknown_fields`（contracts gateway.rs:121-141），`reply` 段落需要**三处联动**——契约 Webhook 变体加 `#[serde(default)] reply: Option<WebhookReplyConfigV1>`、`webhook_bindings` 迁移加 `reply_config_json` 列、`publish.rs apply_trigger_bindings`（:1076-1082 的快照列 upsert）写入该列；configuration_hash 自动覆盖新字段，revision 机制无需改。

### 3.7 API 与前端

```text
GET  /api/v1/applications/{id}/deliveries            -- 投递记录列表（经 BFF 查 Runtime query API）
GET  /api/v1/deliveries/{deliveryId}                 -- 详情（含 attempts、provider_message_id）
POST /api/v1/deliveries/{deliveryId}:retry           -- 死信重放（权限 application:manage）
```

前端：

- Application 详情页"渠道对接"Tab 增加"回复设置"区（L1 开关、输出字段选择、模板编辑）；
- 新增"投递记录"入口（或并入"会话"Tab）：状态筛选、失败原因、重试按钮；
- 节点面板 integrate 分组加入 `reply_message`/`send_message`，参数表单走 Manifest 动态渲染。

## 4. 实施阶段

### P7-A1 契约冻结

- [ ] 冻结 DeliveryOutboxV1、ProviderSendRequestV1、Trigger Context 出站字段（sessionWebhook/过期时间/conversation/sender）DTO；
- [ ] 冻结 `reply_message`/`send_message` 节点 Manifest（参数 Schema、输出契约、错误码 `REPLY_TARGET_UNRESOLVED`/`SEND_CHANNEL_UNRESOLVED`/`DELIVERY_PROVIDER_REJECTED`）；
- [ ] 更新 `contracts/openapi`（渠道 reply 配置、deliveries 查询/重试 API）与 Runtime Internal API 契约；
- [ ] 出站凭证字段进 provider 模板（企微 corpid/corpsecret/agentid、钉钉 robotCode，敏感走 Vault、非敏感进 channel_config_json，见 §2 勘察新增）；
- [ ] `docs/planv2/contracts/v2-schema-table-ownership.json` 登记 `delivery_outbox`/`delivery_dead_letters`（boundary check 对增量迁移 CREATE TABLE 强制校验，漏登记 `cargo xtask check` 失败）；
- [ ] 更新 `docs/03-workflow-engine.md` 节点清单与 `docs/05-platform-business.md` 渠道章节。

门禁：契约测试、Schema 测试、boundary check、OpenAPI diff 通过。

### P7-A2 投递基础设施

- [ ] Runtime 新迁移：`delivery_outbox` + `delivery_dead_letters`；
- [ ] delivery 投递循环 Role（lease + fencing + 退避重试 + 死信迁移）；
- [ ] 三平台 Send Client（钉钉 sessionWebhook→API 回退、飞书 token+im/v1/messages、企微主动消息）；
- [ ] Send Client per-provider 域名后缀校验（gateway 无主机白名单，见 §3.4）；`AGENTX_RUNTIME_ROLES` 与 helm values 增加 `delivery` Role；egress-smoke 覆盖三平台路径。

门禁：fixture 回放契约测试（平台 API mock：成功、限流、凭证失效、目标不存在）、投递重试与幂等测试、多副本投递循环测试（双副本无重复投递）。

### P7-A3 L1 渠道自动回复

- [ ] 渠道配置 `reply` 段落（Control 校验 + 发布冻结 + Runtime 投递器订阅 Execution 终态）；
- [ ] Execution 终态与 delivery 入队同事务；投递事件进 `invocation_events`；
- [ ] 前端"回复设置"表单与投递状态展示。

### P7-A4 L2 回复消息节点

- [ ] builtin manifest + 纯函数 intent 渲染（deliveryId 确定性派生，见 §3.5 两段式设计）；
- [ ] `submit_worker_result_resolved` 结算事务入队 delivery（目标解析在结算侧）；非 IM 来源执行失败路径测试。

### P7-A5 L3 发送节点与发布门禁

- [ ] `send_message` 节点 + 发布时 channel 解析门禁（三种解析路径与失败阻塞）；
- [ ] Studio 节点面板与参数表单接入。

### P7-A6 API 与前端投递可观测

- [ ] deliveries 列表/详情/重试 API（BFF → Runtime query）；
- [ ] 前端投递记录页 + 渠道详情投递状态。

### P7-A7 E2E 验收

- [ ] 见第 5 节。

## 5. E2E 验收（临时 Namespace，pytest 编排）

无法连真实钉钉/飞书/企微，使用平台 API mock fixture（Kustomize 部署三平台 mock 服务：标准成功响应、限流 429、凭证 401、目标不存在四种行为按路径切换），出站经 egress 白名单访问 mock：

1. UI 创建钉钉 stream 渠道 + 配置 L1 自动回复 → 发布 → fixture 入站消息 → 工作流执行完成 → 断言 mock 收到回复、投递记录 delivered、`provider_message_id` 落库；
2. 工作流含 `reply_message` 节点（内容引用模型输出）→ 断言回复内容与变量渲染一致；
3. `send_message` 主动推送到显式渠道会话 → delivered；
4. mock 返回 429 → 断言按退避重试后 delivered，attempt_count 正确；
5. mock 恒 401 → 断言不重试直接 failed、死信归档、UI 可见失败原因、重放后成功；
6. 无 Deployment/停用渠道/非 IM 触发的 `reply_message` → 终态错误码断言；
7. 重复终态/重复节点结算 → 幂等键防重复投递断言；
8. 投递循环 Pod 强杀重启 → 队列中投递不丢失；
9. UI 覆盖：回复设置表单（含校验失败路径）、投递记录列表/筛选/重试、中英文与深浅主题。

## 6. 完成定义

- 三平台入站消息可收到工作流结果的平台回复；L1/L2/L3 三层均可用；
- 投递有权威状态机：重试、退避、死信、重放、幂等全部有契约测试与 E2E 证据；
- 出站只走 egress-gateway 白名单，凭证只在 Vault 与冻结快照中出现，Trace 与证据无 Secret/完整请求体；
- `reply_message`/`send_message` 进节点目录与文档，发布门禁对无法解析目标的 L3 阻塞生效；
- plan4 §20 状态更新为"由 plan7 P7-A 落地"，勾选 plan4 §10 中出站相关未完成项并指向本计划证据。

## 7. 明确不做

- 企微被动同步回复、企微长连接（平台协议限制）；
- 每平台一个回复节点、设计时绑定渠道实例；
- 文件/图片/卡片消息首期支持（仅文本）；
- 把 sessionWebhook 或平台凭证变成 Workflow 输入或映射来源；
- 引入社区 IM SDK；
- Dify/n8n 出站节点兼容。
