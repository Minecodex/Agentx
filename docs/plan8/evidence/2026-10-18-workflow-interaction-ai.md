# AI 创建工作流与 API Key / Hook 闭环验收

2026-10-18，通过工作流画布的“AI 创作”生成完整流程，并使用真实 `glm-5.3-flash` 执行智能体。最终验收 run ID 为 `ai-hook-1018d`：系统 pytest 用例 1 项通过，包含两条实际 API 分支；桌面 Playwright 创建、重开与执行记录复核均通过。

## 实际流程

开始 → 需求分析智能体 → 中间回复 → 等待输入 → 条件分支。

- 原始文本等于“确认”：确认执行智能体 → 结束。
- 其他文本：需求调整智能体 → 结束。
- 执行节点的错误出口连接错误结束。

等待节点只输出原始内容。分支规则属于下游 Condition，三个 Agent 都绑定真实模型。正常结束通过两个可选输出引用的模板取得实际经过分支的 `text`。

节点和连线由实际 AI 创作命令生成，通过现有自动保存、资源授权、校验和发布接口交付。最终工作流为 `01a14e8f-9645-7cd3-bb2e-e1e7bfdb7629`，应用为 `01a14e95-1b0c-7022-b568-500f5c3cc41b`，slug 为 `ai-input-ai-hook-1018d`。

## 实际客户链路

1. 客户持应用 API Key 调用 `POST /gateway/v1/applications/{slug}/invocations`。
2. 前置 Agent 完成推理，回复节点生成持久化消息；此时最终输出为空。
3. 等待节点产生 `waitId`，Hook 接收器实际收到 `workflow.input_required`。
4. 接收器返回 200 后，原等待仍保持活动。客户端使用原 Invocation 查询中间消息，用于模拟 TTS 播报。
5. 客户向通知中的 `resumeUrl` 提交模拟 ASR 文本，附 API Key 和 `Idempotency-Key`。
6. 原 Invocation / Execution 恢复，执行对应 Agent 和结束节点；相同回传重复提交返回原 `receiptId`。

| 回传内容 | 原执行 ID | 实际执行分支 | 结果 |
| --- | --- | --- | --- |
| 确认 | `01a14e95-24d8-70f1-881c-f2981c593f6d` | `approved` | 输出以“确认分支：”开头的两步活动建议 |
| 先别执行，预算改成500元 | `01a14e95-6dc9-7611-bd30-0ff42e57bbb1` | `revise` | 输出以“调整分支：”开头，采用 500 元预算限制 |

两次运行各有两条成功 Agent Run，各调用模型一次：前置 Agent 和实际命中的分支 Agent。未命中的 Agent 没有成功执行记录；前置 Agent 没有因回传或重复请求再次执行。等待历史保存原文、`source=api` 和 `status=received`。

本次 Hook 接收器使用受控 Kubernetes fixture，ASR 使用文本模拟；未接真实电话、ASR 服务或 TTS 服务。

## 发现的限制

- Hook 固定通知只携带等待元数据和恢复地址，不携带回复正文。客户还需查询原 Invocation 的 `messages`，或订阅事件流。
- 新建 API Key 到 Runtime 生效存在同步窗口，立即请求可能返回 401。本次调用驱动使用同一幂等键等待 Key 生效后重试。
- 真实 AI 创作发生两次 `DESIGN_MODEL_STREAM_INVALID` / `outcome_unknown`，同一画布重试后完成。它会查询节点目录并依据拒绝结果修正不合法引用，不能将一次模型返回视为可运行工作流。
- 较早的创作尝试把动态条件出口写为泛化的 `case`，静态校验未拒绝；当前验收额外检查必须使用 `case:<caseId>`，并通过实际执行验证两条分支。该静态校验缺口仍需后续产品修复。

首轮提示中错误要求成功 Exit 引用 `item.text`，后修正为正式 `outputs` 引用模板。首轮 Key 同步问题、后续创作异常和一次复用旧快照的失败记录均保留；最终闭环结果来自 `ai-hook-1018d/interaction-ai` 内新生成的流程。

## 证据与环境

- 系统入口：`tests/e2e/product/test_workflow_interaction_ai.py`。
- 浏览器入口：`tests/browser/tests/workflow-interaction-ai.spec.ts`。
- 主证据：`.local/artifacts/e2e/ai-hook-1018d/interaction-ai/`，包含 AI 提示、实际定义、校验、两次 API 调用、Hook、回传、节点轨迹和截图。
- 完成日志：`.local/artifacts/workflow-interactions/ai-demo-d.log`；最终 `1 passed in 637.56s`。
- 创建与复核 Trace：`.local/artifacts/playwright/helm-agentxctl/ai-hook-1018d/`。
- 产品镜像：`interaction-20261018c`，源码指纹 `9261aa267a91c4074de9877907f442c84cfc79a4f6320a5f86b03775e024a009`，以 `candidate-identity.json` 对照 11 个产品镜像。

验证使用隔离临时 namespace，完成后均已删除。保留的流程 JSON 中模型绑定和 Hook 地址属于已清理的测试环境，复用时需要重新绑定。

## 同日本地发布

随后完成 `local-workflow-interactions-20261018-215640` 的本地构建与发布。11 个产品镜像按摘要部署，四个 Helm Release / Doctor 检查通过；源码指纹与上述系统验收一致。主工作台已包含 `reply_message` 和 `await_input` v1。

通过当前产品初始化 SQL 新建 Control / Runtime 数据库，完整保留 112 / 108 张表、2732 / 14283 行数据，并核对所有保留列的行哈希；原数据库继续保留作为恢复点。切换前确认旧写入 Pod 已全部退出、旧库业务连接归零。未增加产品历史兼容代码。

发布后登录、既有工作流和应用、Gateway 认证读取、SQLx 校验、前端静态资源与桌面节点目录复核通过，临时初始化 Job 已清理。本地证据保存在 `.local/artifacts/local-workflow-interactions-20261018-215640/`，入口为 `release-summary.md`、`verification.json` 和 `post-publish-smoke.log`。
