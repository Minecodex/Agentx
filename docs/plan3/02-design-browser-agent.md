# 外层创作 Agent 的浏览器观察与视觉编辑

2026-11-03。范围为 Workflow 和应用业务页面的创建、修改；不改变 Workflow 中业务 Agent 的模型选择、权限或运行预算。代码及临时环境验收已完成，常驻环境保持原版本。验收事实见本文末尾。

## 执行方式

沿用 `AgentCore::run` 和设计态 Host。一条用户请求内可以发生多轮模型请求、工具调用、检查与修复。模型自行决定步骤，宿主不增加固定创作流水线，也不把“模型结束”转换成“质量通过”。现有 Run 预算、十分钟截止时间、租约、编辑事务、Revision/Hash 和浏览器 ACK 继续生效。

完整模型响应的截断、无效参数与服务明确返回的临时拒绝，由设计态 Host 分类为可纠正失败。Core 持久记录失败反馈，在同一用户任务内最多再尝试两次；重启和暂停不能重置次数，每次新请求独立受理和计费。SSE 层只负责完整帧与结束标记，响应语义校验放在用量结算之后；不完整参数始终不执行任何工具。断流或已发送但未确认的结果继续进入待核实状态，不自动重发。Workflow 内业务 Agent 不启用上述自动纠正；共用流解析仍会保留已确认响应的用量。

Workflow 增加 `workflow_browser`，业务页增加 `page_browser`。两者读取当前授权的编辑快照，检查调用参数中的 `expectedDocumentHash`，把冻结文档交给 Runtime。返回观察后再次检查授权与当前哈希；过期观察不能成为当前草稿的有效证据。

浏览器工具的 `options` 和各动作结构在模型的工具定义中直接展开，使用原生 JSON 对象、数组和布尔值。参数解析沿用严格 Schema 与 Serde 校验；类型错误反馈包含字段路径、预期类型和收到的类型，不回显被拒绝的业务值，也不自动把字符串转换为对象。这样模型可以依靠工具回执修正格式，无须用户逐轮指明修改。

## 浏览器边界

Control → 签名 `design-browser:observe` → Runtime Gateway → Sandbox Manager `sandboxes:preview` → 现有 OpenSandbox。使用固定 digest 的 Playwright 镜像，不新增常驻设计服务或业务 Invocation。

每次调用创建全新的隔离预览，只载入随版本构建的可信预览程序和当前目标文档。网络默认拒绝，浏览器只接收本地资产；不接收任意 URL、脚本或系统命令。业务页继续使用正式 `PageFrame` 的双层不透明 iframe、受限 SDK2 和模拟任务。Workflow 使用正式 `WorkflowFlow`、节点组件、样式与 Manifest，使用现有静态解析器获取逐节点的动态端口；解析问题随观察明确返回。

可信顶层预览使用原生 CSP `worker-src blob:`，只允许 ELK 等随产品构建的 Blob Worker；业务页的两层 iframe 保持 `worker-src 'none'`。Service Worker 所需的 HTTP/HTTPS 脚本注册被策略拒绝，每次预览使用新建的浏览器上下文。固定浏览器镜像中的 Playwright 1.57.0 会在 `service_workers="block"` 下向不透明 iframe 注入脚本并产生安全错误，同一镜像的最小复现已确认。这里由原生策略实施限制，保留 iframe 的不透明来源和全部页面、控制台错误，不过滤异常或修改生成的业务代码。相关上游说明见 [Playwright #43113](https://github.com/microsoft/playwright/issues/43113) 和 [CSP worker-src](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/worker-src)。

工具接受有界的完整动作序列：点击、填写、选择、按键、上传小型模拟文本文件、等待、刷新、检查。Workflow 另支持 `focus_nodes`：空列表查看全图，指定节点查看局部；不修改业务文档。多次工具调用之间不共享浏览器，模型需要重放完整动作序列。一次调用内刷新保留原模拟 taskId/batchId，检查重复提交和明细丢失。

输出包括实际可见文字、表单元素、节点包围框、动作回执、脚本和控制台错误、模拟 SDK 计数及可选截图。页面检查仍沿用已有静态/模拟预览工具，画布语义检查仍使用 `workflow_validate`。浏览器观察本身不证明真实业务执行成功。

## 图片链路

截图为 WebP，质量 0.83、DPR 1、最长边 1440，压缩后最大 576 KiB。复杂画布可以聚焦节点或指定 CSS 区域，不能靠无限提高分辨率。截图保存在 Runtime Artifact 中，引用归属于 tenant、创作类型和 Session，遵循现有保留策略；不把 base64 写入聊天历史。

Core 1.4 的 `ToolResult.observations` 随工具结果一起提交，恢复时不会丢失或重复创建。`tool_observation` 与原 tool_call_id 关联。模型窗口最多保留最近四张工具截图，原用户图片输入不受该限制；旧截图过期或切换为普通模型时，窗口明确说明图片不可见并允许继续读取文字。历史消息保持原样，旧图在窗口中替换为明确的省略说明。

Runtime 在模型派发前校验冻结模型声明的 `vision` 能力、Artifact 归属、类型、大小和摘要，再转为标准 OpenAI `image_url.url=data:image/webp;base64,...`。工具文字回执仍使用 `tool` 角色；同一轮全部工具回执完成后，图片通过单独的多模态 `user` 消息输入模型。没有 vision 的模型明确拒绝截图，仍可选择 DOM 观察；不会将 DOM 检查声称为视觉检查。

## 编辑与界面

Workflow 首次拓扑布局继续由现有 ELK 完成。新增 `move_nodes` 意图只修改已有节点的 x/y，检查重复 ID、有限坐标和编辑范围，沿用一次 Undo 事务及 ACK。不能借布局修改节点参数、Binding、连线或业务 Definition。

功能位于原 Workflow「AI 创作」和应用「访问界面」的创作会话。会话分别展示模型终态、浏览器观察、截图捕获、实际检查问题和观察过期状态。运行、测试与发布沿用原入口。

## 验收与费用

所有 E2E 使用 `pytest tests/e2e` 编排，桌面浏览器操作由 TypeScript Playwright 执行。覆盖两类真实渲染器的中英文和明暗主题、任务/批次刷新、布局业务不变、图片回传、无视觉能力拒绝、真实 AI 一次任务内创建和修改，以及隔离资源清理。

本轮用户授权从零开始最多 190 次真实模型请求。原账本保留 187 次历史请求、已结算费用和未知预留，全局计数上限为 377，新增范围用量等于全局计数减 187；总费用上限仍为 30 美元，单次输出最多 16,000 tokens，思考深度使用 max。测试连接仅显式配置，凭据不进入证据。

最终验收使用临时环境 `designbrowser13`、候选版本 09，源码 SHA-256 为 `05750ea023a91ba30cd4b8cf76c4da1660e1538b69b528c324eaebc5140c5cdf`。11 个正式服务镜像与两个测试组件均已校验来源并固定 digest。证据目录为 `.local/artifacts/design-agent-browser-2026-11-03/`，总报告为 `acceptance-final.json`，系统结果为 `system-13.xml`。

四项系统用例全部通过：浏览器与图片能力协议、真实模型自主创建/修改、应用用户与会话输入、业务 Agent 的 MCP 调用与撤权。协议用例包括同一任务内从模拟 503 和截断响应自动恢复、两次实际截图与移动节点、历史恢复、普通模型切换、纯 DOM 观察，以及业务页不透明 iframe 无注入误报。实际渲染器的 11 项桌面中英文/明暗主题、刷新和 Worker 策略检查通过。

真实模型为显式配置的 `glm-5.3-flash`，每项任务只提交一条用户指令，没有额外追问或定点修复提示：

| 任务 | 用户指令 | 模型请求 | 实际截图 | 结果 |
| --- | ---: | ---: | ---: | --- |
| Workflow 创建 | 1 | 8 | 1 | 通过 |
| Workflow 修改 | 1 | 7 | 2 | 通过，业务 Definition 不变 |
| 业务页创建 | 1 | 15 | 2 | 通过，一次输出截断在同一任务内自动纠正 |
| 业务页修改 | 1 | 10 | 5 | 通过，刷新保留原任务、结果与完整编号，只创建一次任务 |

上述 40 次请求属于最终四个任务；整个开发验收范围累计使用 **92/190 次**，所有请求均使用 max，单次输出不超过 16,000 tokens，图片均为标准 base64 WebP。本轮已结算 `$0.10892222`，另保留 `$0.00611126` 未核实费用；原账本的 187 次历史请求、费用和未知预留全部保留，总费用上限仍为 $30。请求参数核对见 `request-parameters-audit.json`，费用见原 `.local/artifacts/plan10/model-budget.json`。

本地检查通过：前端 603 项、Control 85 项、Runtime 187 项（另有两个原有外部凭据用例未运行）、Core 单元 23 项及驱动集成 29 项、Runtime 契约 40 项及集成 15 项、Schema 漂移两项、模拟模型 15 项，以及相关 Clippy、TypeScript 和 Ruff 检查。并行构建时曾出现三项前端耗时失败，保留原始日志后以两个 Worker 单独复测，603 项全部通过，未改超时或断言。116 个已修改源码文件均不超过 2,000 行。

早期失败及修复记录保留：浏览器 options 的对象结构与准确类型反馈、已确认模型错误的有界自动纠正、截断响应的计费与拒绝执行、Playwright 的 Service Worker 注入误报，以及顶层策略需要允许 ELK Blob Worker。旧 P3 手工 fixture 在进入 Agent 前因缺少冻结 MCP policy 被拒绝，未计作通过；对应的当前公开 API、会话及业务 Agent/MCP 回归已通过，原记录见 `p3-fixture-regression.json`。

实际生成的页面及浏览器报告已归档为 `verified-page.html`、`verified-page.webp` 和 `verified-page-report.json`。全部临时命名空间、预览实例、连接进程、测试凭据及临时浏览器镜像别名已清理。常驻 17 个 Deployment 的 UID、副本数和镜像与开始时完全一致；证据见 `cleanup-final.json` 和 `permanent-deployments-check.json`。

测试复用现有 OpenSandbox 的正式 HTTPS 入口，保持同源限制及证书校验。可通过 `AGENTX_E2E_DESIGN_BROWSER_PROVIDER` 显式指定测试 JSON，字段为 `endpoint`、`secureAccess`、`apiKeyFile`、`caFile`。仅配置临时环境中的 Manager；凭据文件须为私有权限，永久服务不变。浏览器镜像使用固定 OCI index，计算节点应预先缓存对应架构的官方镜像，避免冷下载耗尽观察时限。
