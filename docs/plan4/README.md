# Application 渠道对接实施计划

## 1. 目标与边界

本计划参考 n8n 的 Webhook Trigger 模式，为 Application 提供外部 HTTP 入口。外部平台调用入口后，Agentx 验证请求、提取字段、映射到 Workflow 开始节点参数并创建 Workflow Execution。

本阶段只负责入站触发：

```text
外部平台
  → Webhook Trigger
  → 验证、去重、解析
  → Workflow Start Input Mapping
  → Invocation / Execution
```

本阶段不负责根据 Workflow 结果向钉钉、企业微信、飞书或其他平台发送回复。平台收到的只是快速 ACK（通常为 `200 OK`）；业务回复暂由后续 Workflow 节点或独立出站计划负责。

不实现 n8n JSON、Credential 兼容、通用连接器市场、移动端体验或第二套 Workflow Runtime。项目处于开发期，不保留旧模型兼容层、迁移层或双入口。

## 2. Application 入口位置

Application 详情页新增独立的“渠道对接”Tab，与“部署”“API Keys”“会话”并列。

“触发器”Tab 继续用于 Workflow 内部触发器和 Schedule；HTTP Webhook 的现有 API 子资源路径继续保留，但前端入口归入“渠道对接”。

不新增独立一级路由。一个 Webhook Trigger 属于一个 Application，并通过 Application 当前 Deployment 间接指向 Workflow 版本。

## 3. 快速配置体验

参考 n8n Webhook 节点，普通用户只需完成：

1. 输入入口名称并选择平台模板；
2. 选择认证方式；
3. 保存配置，并在发布后复制系统生成的生产接入地址；
4. 把请求字段映射到 Workflow 开始节点参数；
5. 可选增加固定输入值；
6. 保存草稿并在 Application Deployment 后激活。

默认不显示 Routing、Delivery Target、回复映射、消息模板、重试策略或 Connector 术语。

平台模板只负责提供验证规则和推荐字段；未知平台选择 Custom HTTP 后，用户才需要填写请求 Schema 和 JSONPath/JSON Pointer。

### 3.1 接入地址规则

一个 Webhook Trigger 只生成一个可交付给外部平台的完整生产 Endpoint：

```text
https://{runtime-host}/gateway/v1/webhooks/{public_id}
```

`POST /gateway/v1/webhooks/{public_id}` 只是服务端路由契约，不是第二个接入地址，不能作为独立可复制项出现在配置界面。新增草稿尚无可用地址；保存并完成 Application Deployment 后，详情区才展示完整 Endpoint 和唯一的复制操作。

生产 Endpoint 属于 Webhook Trigger，不属于某个群聊。同一个平台机器人在多个群中使用时，仍可共用这一个 Endpoint；系统通过每次事件携带的稳定会话标识识别来源，而不是要求用户为每个群生成不同 URL。

## 4. 领域模型

### 4.1 Control 配置

```text
Application
  └── Application Webhook Trigger
      └── Webhook Configuration Revision
          ├── Provider Template
          ├── Auth Policy
          ├── Request Schema
          ├── Input Mappings
          └── Fixed Inputs
```

Webhook Trigger 是 Application 的入站配置，不是完整的双向渠道对象。它不保存出站 URL、回复目标或平台消息模板。

核心字段：

- `id`、`tenant_id`、`application_id`；
- `name`、`provider_type`、`status`；
- `public_id`；
- `auth_policy_json`；
- `request_schema_json`；
- `input_mapping_json`；
- `fixed_inputs_json`；
- `configuration_revision`、`configuration_hash`；
- `created_by`、`created_at`、`updated_at`。

### 4.2 Runtime 快照

Application Deployment 发布时，将 Webhook Trigger 的配置冻结到 Runtime Trigger Bundle。草稿或未激活 Revision 不可触发生产 Execution；历史 Invocation 使用创建时的 Bundle 和 Revision。

## 5. Provider 模板与动态字段

### 5.1 已知平台

钉钉、企业微信和飞书使用 Provider Template。模板声明事件版本、签名/解密要求、请求字段目录、事件 ID、幂等字段和推荐映射。

用户看到平台语义字段，例如“消息内容”“发送者 ID”“会话 ID”；平台原始字段由 Adapter 解析，不能成为 Workflow 的隐式公共契约。

Provider Adapter 必须把平台事件标准化为以下 Trigger Context：

```text
provider                  平台类型
provider_connection_id    平台机器人或应用连接
webhook_trigger_id        Agentx Webhook Trigger
provider_event_id         平台事件 ID，用于幂等
conversation.id           群聊、单聊或房间的稳定 ID
conversation.name         可选的会话显示名（钉钉 conversationTitle）
conversation.type         group / direct，取平台显式字段（钉钉 conversationType 1/2、飞书 chat_type p2p/group、企微 chattype single/group），不再用 ID 前缀猜测
sender.id                 平台内稳定发送者 ID
sender.name               可选的发送者昵称（钉钉 senderNick）
message.text              标准化文本内容
```

标准字段之外的映射来源统一走 `raw.<dotted.path>` 透传：Runtime 在验签解密后的平台报文上按 JSON pointer 取值原样传给 Workflow Start Input，Control 只校验路径形式不做静态类型检查。Provider 模板按 `provider × mode` 下发 `mappingSources` 目录（钉钉 senderCorpId/senderId/createAt/msgtype/atUsers，飞书 event.message.*/event.sender.*/header.*，企微 msgtype/createTime/agentId/toUserName），前端映射来源下拉 = 标准字段 + 该目录，另可手填任意 raw 路径覆盖目录未列出的字段。目录刻意不含 sessionWebhook，它只进 Trigger Context 快照留作出站投递（见第 20 节），不作为正式映射契约宣传。

其中 `conversation.id` 是区分多个群的首选依据，`sender.id` 只能区分发送者，不能代替群标识。若某个平台事件不提供稳定会话 ID，平台模板必须明确标记不支持可靠的多群来源识别；此时只能为不同范围创建独立平台连接或独立 Webhook Trigger，不能用群名称等可变字段猜测来源。

### 5.2 Custom HTTP

Custom HTTP 允许用户配置 HTTP Method、请求 Schema、JSONPath/JSON Pointer、Header 认证策略、事件 ID 路径、Workflow Start Input 映射和固定输入值。

示例：

```text
$.data.message    → question
$.data.sender.id  → external_user_id
$.data.channel.id → conversation_id
固定值           → department = customer_service
```

不得允许映射读取 Secret、Authorization Header、签名原文或内部运行字段。请求 Schema、映射目标和类型转换必须由服务端校验。

固定输入只表达该 Trigger 的业务上下文，例如 `department = customer_service` 或 `source = dingtalk`。它不能用于伪造 `conversation.id`、`sender.id` 或 `provider_event_id`，也不能作为多群识别方案。事件来源字段与固定输入发生目标冲突时禁止保存，不做隐式覆盖。

## 6. 入站运行流程

公网入口继续使用现有 Runtime Gateway：

```text
POST /gateway/v1/webhooks/{public_id}
```

标准流程：

```text
HTTP Request
  → 查询 active Webhook Binding
  → 校验请求大小、Content-Type 和时间窗口
  → Provider Adapter 验签 / Challenge / 解密
  → 事件幂等检查
  → 解析 Provider Payload 并生成标准 Trigger Context
  → Input Mapping + Fixed Inputs
  → 按 Deployment Schema 校验 Start Input
  → 创建 Invocation / Execution
  → 返回 202 Accepted
```

Challenge 按平台协议同步响应，不创建 Execution；不等待模型、Workflow、审批或人工输入完成。

幂等键优先使用 `tenant_id + webhook_binding_id + provider_event_id`。相同幂等键携带不同请求内容返回 `409`。

Trace 和审计只记录 Provider 类型、Webhook ID/Revision、事件类型 Hash、字段名/类型/大小、Invocation、Execution 和 requestId，不记录 Secret、签名原文、Authorization Header 或完整请求体。

## 7. Workflow 开始节点映射

映射目标必须来自应用绑定的 Workflow Start Input Schema，不使用硬编码的 `question`、`message` 或 `ai_response` 约定。有活跃 Deployment 时使用该 Deployment 冻结的 Schema；首次应用部署前使用绑定工作流的最新不可变 Workflow Version。没有已发布工作流版本时禁止配置渠道并提示先发布版本，不从可变 Draft 推断字段。

输入下拉框列出该 Schema 中的全部输入，包括必填与可选输入。新建配置时仅预先生成必填输入行，来源字段留空，要求用户明确选择；可选输入通过“添加映射”按需配置。必填输入以 `*` 标记，也可以由固定输入提供。保存时阻止必填输入遗漏、空来源及映射和固定输入之间的目标冲突。

推荐映射：

```text
请求字段 / 固定值       → Workflow 开始参数
消息内容                → question (string)
发送者 ID               → external_user_id (string)
会话 ID                 → conversation_id (string)
固定值 customer_service → department (string)
```

同一 Endpoint 收到不同群的事件时，`webhook_trigger_id` 保持不变，`conversation.id` 随来源群变化。例如：

```text
客服一群事件 → conversation.id = cid_group_001 → conversation_id
客服二群事件 → conversation.id = cid_group_002 → conversation_id
```

Runtime 将标准 Trigger Context 保存在 Invocation 快照中，Workflow 只消费显式映射后的 Start Input。

服务端保存时检查必填参数、目标存在性、类型兼容、重复目标、缺失策略和敏感字段。支持 Workflow 5.0 的 `error`、`null`、`default`、`omit`；第一版默认必填参数使用 `error`。

## 8. API 与存储

沿用现有 Application Webhook 子资源，不新增通用 `/api/webhooks`：

```text
GET    /api/v1/applications/{applicationId}/webhooks
POST   /api/v1/applications/{applicationId}/webhooks
GET    /api/v1/applications/{applicationId}/webhooks/{webhookId}
PATCH  /api/v1/applications/{applicationId}/webhooks/{webhookId}
DELETE /api/v1/applications/{applicationId}/webhooks/{webhookId}
POST   /api/v1/applications/{applicationId}/webhooks/{webhookId}/rotate-secret
```

写操作使用 `expectedVersion` 或 Configuration Revision；冲突返回 `409` 和字段级 `fieldErrors`。

优先扩展现有 `application_webhooks`，并增加不可变 Revision/Mapping 快照。Runtime 复用 `webhook_bindings`、`application_invocations` 和 `runtime_idempotency_keys`。

使用现有 MySQL 约定：UUID 为 `BINARY(16)`，结构化字段为 `JSON`，时间为 UTC `TIMESTAMP(6)`；不使用 PostgreSQL `JSONB` 或独立通用 `webhooks` 表。

## 9. 前端界面

Application 详情页的“渠道对接”Tab 展示渠道名称、平台、状态、HTTP Callback、已映射参数数量、最近触发时间和配置操作。选中一个渠道后，详情区展示该 Trigger 唯一的完整生产 Endpoint、来源识别字段和当前 Workflow 映射；列表和弹窗不再重复展示另一个 URL。

添加渠道 Dialog 默认只显示：平台、Webhook 名称、Credential/认证方式、Workflow 开始参数映射、固定输入值和保存草稿。接入地址区域只提示“保存并发布后生成”，不展示路由模板或复制按钮。

映射与固定输入均以 Workflow 参数为第一列：用户先选要填的 Start Input，再选来源（标准字段、平台原始字段目录或手填 raw 路径）或直接填固定值；固定输入的值按 Start Input Schema 类型在保存时做 number/boolean 收敛，键名不允许脱离 Schema 自由填写。弹窗显示字段所对应的工作流版本，首次应用部署前也能配置完整映射。

高级设置只包含 Custom HTTP 请求 Schema、自定义 JSONPath、事件 ID 路径、自定义 Header 验证和 Replay Window。不显示回复目标、群组路由、出站 Webhook、消息模板或 Delivery。

必须覆盖无 active Deployment、Credential 缺失、必填参数未映射、Schema 类型不兼容、Endpoint 尚未激活、Revision 冲突和最近 Invocation 查询失败。

## 10. 实施阶段

### P4-01 入站契约

- [x] 冻结 Webhook Trigger、Provider Template、Request Schema、Input Mapping 和 Runtime Bundle DTO；
- [x] 更新 Control/Runtime OpenAPI；
- [x] 明确钉钉、企微、飞书和 Custom HTTP 的验证边界；
- [x] 更新 Application、数据模型和 Runtime 数据图文档。

门禁：协议测试、Schema 测试、Boundary Check 和 OpenAPI diff 通过。

### P4-02 Application 配置

- [x] Application 详情页新增“渠道对接”Tab；
- [x] 实现渠道列表、快速配置 Dialog、自动映射和固定输入；
- [ ] 实现 Secret 一次性显示、轮换、权限、字段错误和安全删除；
- [x] 详情区只显示一个真实生产 Endpoint，并展示会话 ID、发送者 ID、事件 ID 与当前 Workflow Start Schema 的映射。

门禁：桌面端中英文、浅深主题、权限、依赖缺失和删除影响测试通过。

### P4-03 Runtime Webhook Trigger

- [x] 实现钉钉文本事件入站 Adapter；
- [x] 实现签名、Challenge、时间戳和幂等；
- [x] Deployment 后激活 Runtime Binding；
- [x] 完成 Input Mapping、Fixed Inputs 和 Start Input Schema 校验；
- [x] 创建 Invocation/Execution 并返回平台成功 ACK；
- [ ] 验证无 Deployment、停用 Binding 和非法请求的终态。

门禁：Runtime Gateway 契约测试和故障恢复测试通过。

### P4-04 其他平台与未来出站

- [x] 企业微信和飞书只增加各自入站 Adapter 与 Fixture；
- [x] Stream/WebSocket/Polling 作为后续 Connector 计划，不改变 Trigger Input 契约（已随第二阶段实施，见下文）；
- [ ] Workflow 内部增加 HTTP Response、Provider Send Message 等出站节点另行立项（方案讨论见下文第二阶段第 20 节）；
- [ ] 回复协议、Delivery Outbox、重试和投递查询不属于本阶段完成定义。

## 11. E2E 验收

Kubernetes 系统级 E2E 以 `pytest tests/e2e` 为唯一编排入口，浏览器操作继续使用 `tests/browser` 的 TypeScript Playwright。测试使用临时 Namespace，完成后清理；证据不得包含 Secret、Token、完整请求体或完整用户输入。

最小闭环：

1. 通过 Web UI 打开 Application 的“渠道对接”Tab；
2. 创建钉钉 Webhook Trigger；
3. 验证系统根据 Workflow Start Schema 生成推荐映射；
4. 验证新增弹窗不展示第二个 URL，草稿状态没有可复制 Endpoint；
5. 配置一个固定输入值；
6. 发布 Application Deployment，验证详情区只出现一个完整生产 Endpoint；
7. 使用相同 Endpoint 分别发送两个不同 `conversation.id` 的有效签名 Provider Fixture；
8. 两次请求均收到平台兼容的 `200` 成功 ACK，并分别创建 Invocation；
9. 验证两个 Workflow Start Input 的 `conversation_id` 不同，其他请求映射和固定值正确；
10. 验证重复事件只创建一个 Invocation/Execution；
11. 验证缺失稳定会话 ID、签名错误、Schema 错误、无 Deployment 和停用 Binding；
12. 在 UI 查看 Invocation/Execution、标准来源字段和脱敏 Trace；
13. 删除前验证引用保护、取消删除和确认删除路径；
14. 测试结束后删除临时 Namespace。

## 12. 完成定义

P4 只有同时满足以下条件才算完成：

- Application 详情页有独立“渠道对接”Tab；
- Webhook Trigger 可以通过已发布 Application Deployment 触发 Workflow；
- 入站验证、幂等、Schema 校验、字段映射和固定输入有稳定契约；
- 每个 Trigger 只有一个完整生产 Endpoint，UI 不把内部 Path 模板显示为第二个地址；
- 同一 Endpoint 可通过标准化 `conversation.id` 区分不同群来源；
- 必填 Start Input 未映射时不能保存或发布；
- 草稿配置不会提前影响生产入口；
- 历史 Invocation 使用当时的 Revision 快照；
- Runtime 只返回快速 ACK，不等待 Workflow 或回复平台；
- Secret、签名、请求体和 Trace 满足脱敏要求；
- Kubernetes E2E、协议测试、静态边界检查和错误状态测试全部通过；
- 出站回复、平台消息模板和 Delivery 能力明确延期，不伪装成已完成。

## 13. 明确不做的旧方案

- 把 Webhook 设计成同时负责入站和出站的完整渠道对象；
- 在渠道页面配置 Delivery Target、群组路由和回复协议；
- 等待 Workflow 完成后同步返回平台业务消息；
- 使用 `ai_response`、`answer` 或 `message` 猜测 Workflow 输出；
- 将 Stream、WebSocket、Polling 强行建模为 Webhook 字段；
- 将平台 Token、出站 URL 或签名密钥明文保存；
- 为三种平台分别复制一套 Workflow 执行链路；
- 使用 `alert()`、`confirm()`、Emoji 图标或内存 Mock 作为正式实现。

# 第二阶段：Stream 长连接与渠道凭证内置化

第一阶段交付了 HTTP 回调模式的渠道入站。第二阶段重构渠道层，实施范围为三件事：

1. 渠道凭证内置化：渠道配置以 JSON 存于渠道表自身，不再引用凭证表，并按平台模板渲染不同的动态表单；
2. 入站双模式：在回调（hook）之外增加钉钉 Stream、飞书长连接的反向长连接（stream）模式，两种模式并存按渠道选择；
3. 企微无反向连接能力，只保留回调模式，不新增能力。

出站回复不在第二阶段实施范围内，第 20 节仅记录方案讨论结论（是否适配后续插件回复体系），供未来立项时使用。项目处于开发期，不保留旧模型兼容层，`credential_id` 直接删除。

## 14. 平台机制结论

### 14.1 三平台对比

| | 钉钉 | 飞书 | 企微 |
|---|---|---|---|
| 接收方式 | HTTP 回调 / Stream 模式（平台侧二选一，互斥） | HTTP 回调 / 长连接模式（二选一） | 仅 HTTP 回调 |
| 长连接形态 | WS 反向连接，免公网地址、免加解密 | WS 反向连接，免公网地址 | 无 |
| 所需凭证 | clientId (AppKey) + clientSecret | app_id + app_secret | Token + EncodingAESKey |
| 限制 | ticket 90 秒一次性 | 仅企业自建应用；每应用最多 50 连接；事件 3 秒内须处理完；多连接集群分发不广播 | 被动回复须 5 秒内同步返回 |

### 14.2 钉钉 Stream 协议要点

- `POST https://api.dingtalk.com/v1.0/gateway/connections/open`，携带 clientId、clientSecret、subscriptions（topic + type），返回 endpoint 与 ticket；ticket 90 秒有效且只能建立一条连接，禁止存储复用；
- WS 握手 `GET {endpoint}?ticket={ticket}`；机器人消息订阅 topic `/v1.0/im/bot/messages/get`（群聊需 @ 机器人，单聊不需要）；
- 推送帧 `{specVersion, type: SYSTEM|EVENT|CALLBACK, headers{topic,contentType,messageId,time}, data}`，data 为 JSON 字符串非对象；
- 客户端 ACK：`{code:200, message:"OK", headers:{messageId}, data:"{\"response\":null}"}`；SYSTEM 帧 topic 为 `ping` 时将 data 内 opaque 原样回写，`disconnect` 帧无需响应；
- 机器人消息为 fire-and-forget（不重推），事件订阅超时会重推；官方要求幂等，复用现有 `runtime_idempotency_keys` + `provider_event_id`；
- 机器人消息 data 字段：`conversationId`、`conversationType`、`senderStaffId`、`text.content`、`msgId`、`sessionWebhook`（带 `sessionWebhookExpiredTime`）。字段结构与 HTTP 回调一致，第一阶段 normalize 路径可复用。

### 14.3 飞书长连接协议要点

- 实施时按官方 Go SDK 现行协议改为 endpoint 引导：`POST {domain}/callback/ws/endpoint`（AppID/AppSecret）返回携带 device_id/service_id 的一次性 WS URL，无需 tenant_access_token 与过期刷新；
- WS 帧为 protobuf 编码（`event.v1` proto，以官方 Go SDK `oapi-sdk-go` ws 包源码为准），workspace 已有 `prost 0.13` + `tonic`，codegen 成本可控；
- 长连接用应用凭证鉴权，无 encryptKey 验签环节；事件帧即标准 v2 schema（`header.event_id`、`event.message.chat_id`），第一阶段 normalize 路径直接复用；
- 事件 3 秒内必须处理完成，否则平台重推；入站只创建 Invocation 后立刻 ACK，天然满足，靠幂等表兜底重复。

### 14.4 企微

只有回调一条路（GET echostr 验证 + POST msg_signature/SHA1 + AES 解密），第一阶段实现保留不动。被动回复 5 秒同步限制对长流程不可满足，出站只能走主动消息 API，见第 20 节。

## 15. 渠道配置模型（凭证内置化）

现状问题：渠道必须引用凭证管理中预先创建的 `custom_json` 凭证，但字段名（token/aesKey/encodingAESKey/encryptKey/appSecret…）是 Runtime Adapter 的隐式约定，界面无提示，填错直到平台事件 401 才暴露；stream 模式字段又完全不同，继续套 `custom_json` 凭证只会更糟。

新模型：

- `application_webhooks` 增加 `channel_config_json`，结构 `{ "mode": "callback" | "stream", "fields": { ... } }`；删除 `credential_id` 列，渠道不再引用凭证表；
- Provider 模板（服务端声明式定义）按 `provider × mode` 声明字段：键名、类型、是否敏感、是否必填。模板字段键与 Runtime Adapter 的查找键同源，保存时服务端校验字段齐全性；
- 前端按模板为不同平台渲染不同的动态表单，用户不再先建凭证对象；
- 敏感字段由 Control 在保存渠道时自动写入 Vault（路径如 `application-webhooks/{id}`），继续用 `secret_ref_json` 引用，全程不落库明文；
- `mode` 为单选：两家平台在开放平台侧本身就是二选一，切换模式等于换一套配置字段；
- 凭证管理（credentials 表）保留给 MCP、模型等场景，与渠道无关。

### 15.1 渠道变更随 Deployment 发布

渠道是应用级配置，但 Runtime 只执行 Deployment 中冻结的 trigger 清单。渠道创建、修改或删除后，Control 推进 `runtime_config_revision` 并将变更标记为待发布；创建下一版 Application Deployment 时，Publisher 冻结当前渠道清单并整体激活。Runtime 不提供绕过 Deployment 的 `triggers:sync` 入口。Deployment 回滚会同时恢复该版本冻结的 trigger 清单，因此 Workflow、渠道配置和运行时行为始终属于同一个可审计快照。

## 16. Stream Connector

### 16.1 模块结构

```text
src/services/agentx-v2-runtime/src/stream/
  mod.rs                 # supervisor：租约领取、连接启停、退避重连、状态回写
  dingtalk_stream.rs     # open API + WS + JSON 帧 + ping/ACK
  feishu_ws.rs           # token 刷新 + WS + prost 帧 + pong/ACK
```

supervisor 对两家是同一套，平台差异封装在协议 adapter；事件出口统一调用从 `webhook.rs` 拆出的 `normalize()`/`map_input()`（验签解密留在 HTTP 路径，长连接无验签环节）。

### 16.2 生命周期与多副本

- 复用 trigger poller 的租约模式（`locked_by`/`locked_until`/`fencing_token`）：抢到租约的副本为该 binding 建连；
- 发布/停用渠道时立即断开对应连接；重连用指数退避加抖动；
- supervisor 心跳回写连接状态（未激活/已连接/重连中），前端渠道详情区展示，替代 stream 模式下不存在的接入地址；
- 两家平台多连接均为负载均衡分发（飞书不广播但选一条），配合幂等表，多副本各建一条连安全，租约只为避免无谓重复。

### 16.3 协议客户端自研决定

全网 Rust SDK 盘点结论（2026-08）：

| 候选 | 状态 | 结论 |
|---|---|---|
| dingtalk-stream | 单人维护，停更 5 个月，3.8k 下载 | 不引入 |
| dingtalk-stream-sdk | 241 下载，自 open-dingtalk 官方组织迁至个人账号 | 不引入 |
| open-lark | 功能最全（约 15.9 万行），停更近一年 | 出站 API 层未来再评估 |
| larksuite-oapi-sdk-rs | 活跃但仅 2.5 个月历史 | 不引入 |
| feishu-sdk | 发版后即停更 | 不引入 |

自研理由：所需功能面（建连、收消息、ACK、重连）不足 open-lark 的 1%；连接生命周期必须由我们的 supervisor 精确控制（租约、发布驱动、状态回写、Vault 取密），通用 SDK 的事件循环与重连模型反而需要胶水逆向适配；钉钉协议官方公开，飞书协议可从官方 Go SDK 源码提取；`tokio-tungstenite`、`reqwest`、`prost` 均已是现有依赖。预估钉钉 300–500 行、飞书 500–800 行、supervisor 300–400 行。

## 17. 第二阶段实施阶段

### P4-05 渠道凭证内置化

- [x] Provider 模板（provider × mode 字段 schema）契约冻结；
- [x] `channel_config_json` 落库，删除 `credential_id`，敏感字段自动写 Vault；
- [x] 前端按平台渲染动态表单，替换 Credential 选择器；
- [x] Control 保存校验：字段齐全性、mode 合法性、与模板键一致。

门禁：契约测试、前端中英文与深浅主题测试、OpenAPI diff 通过。它是 Stream 的前置（stream 字段全靠模板定义）。

### P4-06 钉钉 Stream Connector

- [x] `webhook.rs` 拆分：normalize/map_input 独立，HTTP 验签留在 gateway 路径；
- [x] stream supervisor：租约、启停、退避重连、状态回写；
- [x] 钉钉协议客户端：open API、WS、JSON 帧、ping/ACK、机器人消息分发；
- [x] `sessionWebhook` 进 Trigger Context（出站投递伏笔，见第 20 节）；
- [x] 幂等与重复帧去重验证（复用 provider event 幂等键与 create_runtime_invocation 契约）。

门禁：fixture 帧回放契约测试、故障恢复测试（断连重连、租约切换）通过。

### P4-07 飞书长连接 Connector

- [x] pbbp2 proto（Frame/Header）以官方 Go SDK 为准手写 prost 结构；
- [x] endpoint 引导（AppID/AppSecret 换一次性连接 URL，现行协议无需 tenant_access_token）；
- [x] WS 帧 codec、ping/pong、ACK、拆包重组与事件分发；
- [x] 3 秒处理窗口（读超时 2×ping+5s）与超时重推下的幂等（共享 provider event 幂等键）。

门禁：proto 帧 fixture 契约测试、断连重连测试通过。

## 18. 第二阶段 E2E 验收要点

- Stream 模式无法用"伪造签名请求打 endpoint"测试，需 fixture 帧回放（钉钉 JSON 帧、飞书 proto 帧）或 mock WS 服务，纳入 `pytest tests/e2e` 编排；
- 最小闭环：创建 stream 渠道 → 发布 → 连接状态变为已连接 → fixture 事件触发 → Invocation 创建 → 重复事件幂等；
- UI 覆盖：不同平台渲染不同动态表单、stream 渠道不显示接入地址而显示连接状态、mode 切换换配置字段；
- 证据不得包含 Secret、Token、完整请求体。

## 19. 第二阶段完成定义

- 渠道配置不依赖凭证表，字段按 Provider 模板动态校验，不同平台渲染不同表单，敏感字段仅存 Vault；
- 钉钉 Stream、飞书长连接可免公网回调地址触发 Workflow，企微继续走回调，两种模式并存按渠道选择；
- 长连接有租约、重连、状态回写，多副本安全，重复事件幂等；
- 协议 fixture 契约测试、故障恢复测试、Kubernetes E2E 全部通过；
- 出站回复未实施，不伪装完成。

## 20. 方案讨论：出站回复与插件回复体系（本阶段不实施）

本节只回答一个问题：未来的插件式回复体系（参考 Dify 工具 / n8n 节点的形态，在 Workflow 内放回复插件）与本渠道模型是否适配。结论：适配，且不需要"工作流设计时绑定渠道"。

### 20.1 时序问题的解法

工作流在设计时渠道还不存在，但回复节点不需要在设计时关联渠道——它关联的是"消息来源"，来源是运行时数据。参考系：

- Dify（答案契约型）：工作流只产出结构化输出，渠道集成在应用层取输出并回发，工作流不感知渠道；
- n8n（凭证先行型）：出站节点引用凭证对象，目标从 Trigger 输出数据取。但 n8n 的 workflow 是顶层部署单元，而 Agentx 的 Workflow 被多个 Application 复用、渠道属于 Application，写死渠道 ID 会破坏复用，因此更靠近 Dify 分层。

### 20.2 建议的分层（L1/L2/L3）

- L1 渠道级自动回复：渠道配置 `reply { enabled, outputField, 模板 }`，渠道连接器在 Execution 完成后取输出原路回发，工作流零感知；
- L2 回复来源会话节点：workflow 内"回复消息"节点只填内容（任意工作流变量），不引用渠道；运行时经 ExecutionOrigin → Invocation Trigger Context 解析来源投递；
- L3 发送到指定会话节点：主动推送场景，节点声明 `channelRole`（第一版每 provider 一个隐式角色），Application 发布时解析：显式渠道 ID → 该 provider 唯一渠道 → primary 渠道；解析失败阻塞发布，复用"必填 Start Input 未映射不能发布"的门禁语义。

### 20.3 回复节点契约（内容/寻址分离）

```text
回复节点 = 内容（任意 workflow 变量，通常为模型输出）
         + 目标（默认：来源会话，运行时从 Invocation Trigger Context 解析；
              可选：显式变量/固定值，用于主动推送场景）
```

- 渠道 mapping 职责不变：只映射业务输入进 Start 入参，不承担配送寻址；
- 不强制"寻址走 Start 输入"：Invocation 快照已含全部寻址信息，强制映射会污染工作流契约，且钉钉光有 conversation_id 无法完成投递；
- 统一"回复消息"节点（provider 由触发来源决定）优先于每平台一个节点，保工作流跨平台复用。

### 20.4 各平台投递路径

| 平台 | 路径 |
|---|---|
| 钉钉 | 优先 sessionWebhook（注意有效期），过期走机器人正式 API（robotCode + 会话/用户 ID + 渠道凭证）兜底 |
| 飞书 | `/im/v1/messages`（chat_id + 渠道凭证换 tenant_access_token） |
| 企微 | 主动消息 API（userid + 应用凭证）；被动回复 5 秒限制对长流程不可满足，不做同步被动回复 |

出站与入站共用渠道配置中的同一份平台凭证。当前 mapping 白名单无 `sessionWebhook`，如需钉钉快速验证可临时加入，但不作为正式回复契约。

## 21. 第二阶段明确不做

- 引入社区 Rust SDK 承担长连接核心链路（盘点结论见 16.3）；
- 出站回复、回复节点、Delivery Outbox 的任何实施（仅第 20 节讨论）；
- 让工作流在设计时绑定具体渠道实例；
- 将 sessionWebhook 或平台凭证映射进 Workflow 输入作为正式回复契约；
- 企微长连接、被动同步回复（平台协议不支持或不可满足）；
- 渠道凭证明文落库或继续引用 credentials 表。
