# 工作流中间消息与等待输入

## 边界与节点

`agentx/core` 内置节点包定义 `reply_message` 和 `await_input`；Runtime 负责执行、存储、投递和恢复。`agentx-agent-core` 只负责智能体推理，不接管通用工作流交互。

`reply_message` 接收支持变量和模板的 `content`，在节点结算事务内保存 `workflow_messages` 并发布 `workflow.message`，随后继续下游。渠道来源另行写入现有 `delivery_outbox`，实际投递状态由 delivery 事件表达。非渠道来源同样成功产生消息。节点输出的 `deliveryId` 是稳定消息标识，`status=emitted`；最终返回仍由 Exit 节点产生。`send_message` 的显式渠道发送语义不变。

`await_input` 接收任意原始 JSON 内容，包含文本、对象、布尔值和 null；不识别意图或确认。业务逻辑由后接的 If、Code、Model 等节点实现。非确认内容也恢复节点；下游分支可重新进入等待，产生新的等待实例。

配置面板依次展示等待超时、输出说明、执行出口，最下方是单个可选 Hook URL 与文档。Hook URL 支持现有变量引用。`timeoutMs` 不填则持续等待，直到输入或取消；执行出口为 `main`、`timed_out`、`error`。正常和超时输出均包含 `waitId/content/receivedAt/source`，超时内容和接收时间为 null。

## 持久化与恢复

`input_waits` 独立于审批任务，关联不可变执行版本、节点激活和检查点。检查点、等待记录、等待事件和 Hook outbox 在同一事务提交。挂起不创建 Worker 任务，也不阻塞其他可执行分支。

每个激活有稳定 `waitId`，同一节点再执行产生新激活、新 waitId；Attempt 重试不增加等待次数。根执行串行分配每个稳定节点路径的 occurrence，路径采用 JSON Pointer 转义，子流程包含调用节点，如 `/child/ask`。并行等待按数据库创建顺序分配序号，顺序和匹配结果持久化。

输入接收、超时和取消通过行锁竞争。接收事务保存原始内容与幂等凭据，并提交唯一 `resume_execution` 命令；恢复当前 Execution，不重新从 Start 执行。重复键与相同内容返回原接收凭据；重复键内容不同、其他键回传已结束等待、过期等待均拒绝。接收成功仅代表已持久化排队，最终结果通过原调用查询或事件流获得。

父执行结束或取消会清理其等待、子流程等待和渠道占用，并停止尚未发送的通知。已发出的网络请求无法撤回，通知接收方仍应按 eventId 去重。服务重启后由持久化命令、检查点和 outbox 租约继续执行。

## 客户系统接入

非预填运行填写 Hook URL 时，进入等待后 POST 固定通知。HTTPS 与现有 egress 策略适用；测试集群复用受控 fixture 地址。没有模板、凭据选择或通知模式配置。

```json
{
  "eventId": "<稳定通知 ID>",
  "type": "workflow.input_required",
  "invocationId": "<原调用 ID>",
  "executionId": "<实际挂起的执行 ID>",
  "nodeId": "ask",
  "nodeExecutionId": "<节点激活 ID>",
  "nodePath": "/ask",
  "occurrence": 1,
  "waitId": "<等待 ID>",
  "resumeUrl": "/gateway/v1/invocations/<原调用 ID>/waits/<等待 ID>/input",
  "expiresAt": null
}
```

通知接收端应快速确认送达，后续输入通过恢复地址异步提交。Hook 2xx 只确认送达，不恢复工作流。网络失败、429、408、5xx 沿用投递队列的有限重试和死信机制，同一通知重试保持 eventId。最终投递失败仍保留等待，可通过原调用查询恢复。

客户接收通知后，用平台公开地址加 `resumeUrl` 提交输入：

```bash
curl -X POST "$AGENTX_BASE_URL$RESUME_URL" \
  -H "Authorization: Bearer $AGENTX_API_KEY" \
  -H "Idempotency-Key: customer-asr-message-001" \
  -H "Content-Type: application/json" \
  -d '{"content":"我再考虑一下"}'
```

API Key 必须有原调用所属租户和应用的访问权，waitId 不授予权限。回传采用 `POST /gateway/v1/invocations/{id}/waits/{wait_id}/input`；平台用户操作通过 BFF、签名授权及同一 Runtime 接收服务处理。子流程等待关联原始 Invocation，客户无需创建新的调用。

不填写 Hook 时，`GET /gateway/v1/invocations/{id}` 返回消息与活动等待；原调用 SSE 发布 `workflow.message`、`workflow.input_required`、`workflow.input_received`、`workflow.input_timed_out`、`workflow.input_finished`。客户也可据此接入。语音客户将中间消息转为 TTS，并把后续 ASR 文本原样回传，业务确认规则仍在工作流中。

## 渠道和平台入口

| 入口 | 输入来源 | Hook |
|---|---|---|
| API Key / 客户 HTTP 系统 | 客户持 Key 调用恢复地址 | URL 非空时发送 |
| 规范化会话渠道 | 相同会话用户的下一条消息 | 无需填写；填写时也发送 |
| 定时任务 | 配置任务时保存的回复序列 | 不发送 |
| 手动执行、设计器调试 | 运行表单的回复序列 | 不发送 |
| 测试台参数和会话模式 | 本次运行的回复序列 | 不发送 |
| 测试集评测 | 用例版本快照的回复序列 | 不发送 |

规范化渠道按租户、应用、连接、会话、发送用户定位活动等待。相同作用域只能有一个活动等待，第二个等待走错误出口。渠道入口先查询等待，再决定创建新 Invocation；`channel_input_receipts` 在同一事务保存消息接收凭据，已消费消息在等待结束后重投也不会新建调用。没有会话/发送者身份的通用 HTTP Webhook 按客户系统方式显式回传。

平台入口使用共享发现接口递归检查实际版本的固定子流程，设计器限制到本次执行范围。界面按稳定路径编辑回复序列：

```json
{ "waitInputs": { "/ask": ["稍等", "确认"], "/child/ask": [{"ready":true}] } }
```

只有真实进入等待时才消费该节点下一项，未经过分支不消费。缺失或耗尽产生 `WAIT_INPUT_EXHAUSTED`，不推测确认。预填模式由经过授权的服务端入口决定；普通 API Key 不允许通过 waitInputs 选择预填模式或抑制通知。自动注入同样经过持久化接收和恢复命令。

回复计划冻结在 Work Package、执行快照、定时配置、测试集用例和版本快照中。测试契约指纹纳入等待节点路径，节点改名不改变契约；复制、JSONL 导入导出、CSV 导入、调用记录转用例均携带回复。运行详情和评测报告展示中间消息、实际等待次数、内容、来源和结束状态。

## 验证

Rust 状态机、协议和 Runtime Slice 使用真实临时 MySQL/Redis 验证挂起、幂等、超时、取消和预填。系统闭环通过 `pytest tests/e2e/product/test_workflow_interactions.py` 编排临时 Kubernetes namespace，调用真实发布/API/渠道入口及桌面 Playwright；证据统一写入 `.local/artifacts/e2e`，清理由现有 pytest fixture 完成。

真实模型的画布 AI 创建和 API Key/Hook 双分支验收由 `pytest tests/e2e/product/test_workflow_interaction_ai.py` 编排；覆盖三个 Agent、中间回复、原始输入等待、Hook 确认送达、原执行恢复、重复回传和实际分支轨迹。过程与当前限制见 [AI 创建与 Hook 闭环验收](plan8/evidence/2026-10-18-workflow-interaction-ai.md)。
