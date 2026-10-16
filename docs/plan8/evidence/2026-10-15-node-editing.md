# AI 节点编辑、Markdown 与布局回归

状态：done。代码修复、逐类节点验证、恢复协议回归和本地发布均已完成；容量压测继续按用户要求 deferred。

## 发现与修正

用户报告的草稿 revision 10 中，Model.userQuestion 和 Exit.outputs.reply 使用合法的简写 Reference Binding：没有 run、item、missingPolicy。Rust 及 JSON Schema 将这些字段定义为可省略，前端却将其声明为必填，并在比较 Selector 时直接读取 selection.kind。已用同样的引用结构复现 TypeError；所有消费者现在按协议默认值解释，打开面板不修改原始参数或文档 hash。

排查同时发现两个额外的参数契约问题：

- validate_draft 过滤了 INVALID_NODE_PARAMETERS，使数组变成 null、错误类型或额外字段可能通过草稿门禁。现在区分“缺少顶层待配置字段”和“已填写字段结构非法”，后者阻断手动保存和 AI 编辑；禁用节点也检查结构。
- reply_message、send_message 的内容控件生成 InputBinding，但 Manifest 只声明字符串。内容字段统一使用标量 Binding 契约，相关调用者和 Fixture 同步更新。

节点发现同时返回从 Rust 生成的 InputBinding Schema，帮助模型按实际契约构造引用。

真实 GLM 代码节点场景进一步暴露 Monaco 生产包缺少 Worker 配置：JavaScript Worker 从 blob URL 解析相对 bootstrap 模块失败。按 Monaco/Vite 的 Worker 集成方式显式打包语言 Worker，并使用已安装 Monaco 0.56 的 package exports 路径；代码节点 E2E 额外等待真实 TypeScript Worker 初始化，避免仅看到编辑器外壳就提前通过。

AI 回复与流式文字使用 react-markdown + remark-gfm 的只读渲染，支持标题、列表、强调、行内代码、代码块和表格，沿用主题令牌；不启用原始 HTML，保留默认 URL 过滤。

拓扑变化和显式 auto_layout Intent 在提交前调用已有 ELK。Start 参与布局，层间净间距 160px，同层间距 80px，Loop 容器先布局内部再计算外部尺寸。大图分层布局按实际列宽和累计行高放置，避免宽容器重叠。参数或名称修改保留原布局，节点范围请求不能重排整个画布。

布局与业务修改组成一次 Undo。命令 hash 校验待布局文档，ACK hash 校验实际结果；Control 只允许节点几何及 Start 坐标变化，拒绝借布局改写业务参数、连线、折叠状态、分组、注释或 viewport。持久回执包含实际布局，丢失 ACK 后重发同一结果。

## 验证记录

- 用户原草稿只读导出，仍为 revision 10，未改写数据。
- 覆盖全部 13 类 Action 的合法简写参数、面板打开与名称编辑，另覆盖 Exit 输出映射。
- 编译器逐类验证相同 Fixture，拒绝错误参数、未知字段、嵌套条件及非法 Selector，包括禁用节点。
- 布局覆盖 Start、分支、Loop、宽容器、大图、确定性及输入不变；Markdown 覆盖流式片段和危险 HTML/URL。
- 新增 Playwright 26 场景：每类节点各一组固定 Provider 输入和一组实际 GLM 创建；覆盖点击、手动编辑、保存重开、Markdown、间距、Undo、ACK 丢失、非法参数阻断与零业务执行。

本地检查已通过：前端 96 文件 / 465 测试，最终布局与面板定向回归 25 测试；Runtime 编译器 84、Control 68、Runtime Contracts 27、V2 Runtime 166 测试（另 2 项原有忽略）；验收约束 149 测试、边界检查、相关 Rust Clippy、前后端类型检查、前端 Lint、生产构建与差异空白检查。容量压测按用户要求暂不执行。

本地证据：`.local/artifacts/plan8-node-inspector-20261015/`。

### 首轮系统验收与修复

`p8-inspect-r1-20261015` 在临时 Kubernetes 环境执行 26 个节点场景：21 通过、4 失败、1 错误。保留原始 JUnit、Trace、截图和脱敏模型账本，不覆盖失败记录。

- 真实模型的 Code 场景发现 Monaco Worker 生产加载错误，已修复；新断言检查实际语言 Worker 初始化及重开。
- Loop 场景的测试定位器误用了普通节点标识，已改为容器 Header；深入检查另外发现外层布局使用布局前的子节点坐标，已修正尺寸计算，并断言容器到 Exit 的净间距为 160px。
- Merge append Fixture 使用了不存在的 left 输入端口，被结构校验正确拒绝；Fixture 改用 Manifest 定义的 main。真实模型首轮自行修正并通过。
- 真实模型的 Sub-workflow 已编辑保存，但最后一次设计态模型调用返回 DESIGN_MODEL_CONNECTION_DENIED。保留出口拒绝日志，在新环境复测；没有放宽网络地址校验或更换模型连接。
- 5 个取消、强退、连接异常、并发编辑和 ACK 恢复场景通过。消息发送及循环投递通过；回复节点首轮入口出现空响应 502，第二轮全新环境中该场景已通过。

第二轮镜像 `plan8-inspectors-20261015-r3` 源码摘要为 `44f5c62788945444fdf78d9899960cd45ddea2470385b8a7facc68ebf80e9875`。`p8-inspect-r3-20261015` 结果如下：

- 全部 13 个固定输入节点场景通过；真实 Code、Sub-workflow 通过。
- 真实 Loop 第二次设计态模型请求连接失败，账本为 DESIGN_MODEL_CONNECTION_DENIED、输入/输出 Token 均为 0，尚未应用节点修改。出口日志显示已建立隧道，但 TLS 请求没有收到返回字节；不将此场景记为通过。
- 原有 5 项恢复协议再次全部通过。
- 新增“布局已应用、ACK 尚未送达时重载”测试在编辑命令生成前触发 15 秒断言超时。根据当时仍在处理的事件和待完成 Effect，将异步等待上限调整为 90 秒；仍需实际复测。
- 回复节点从通道入口到回复投递的系统测试通过，关闭首轮空响应 502 的复测项。

第三轮 `p8-inspect-r3-retry` 保留两项失败：真实 Loop 再次在第二次模型调用前遭到出口私有地址拒绝；恢复测试在延长等待后实际进入了刷新场景，发现本地 ReceiptProof 的 `editorDocument` 被对象展开到 ACK 顶层，严格 DTO 拒绝并返回 422。已用专门的回执恢复函数将其放回 snapshot.document，并增加“序列化后与原 ACK 完全一致、顶层字段严格匹配”的单元回归；3 文件 / 12 项相关前端测试通过。该发现属于代码缺陷，最终镜像升级至 r4 后重新验证恢复协议。

### 最终候选与逐类结果

最终镜像 `plan8-inspectors-20261015-r4`，源码摘要 `ab408d526e2f0153326a41b94820c116e98cfcaa07b5502a94f7069f021b0607`。12 个构建镜像的源码标签一致，生产 Web 构建、类型检查及 Lint 通过。真实 Loop 在 `p8-inspect-r4-final` 完成创建、点击、编辑、保存重开与零业务执行检查，关闭前两次外部连接失败的复测项。外部模型连接的间歇性失败记录仍予保留。

节点场景为分轮验证与定向复测，不能表述为最后一轮重新执行了全部 26 项。完整历史见本地 `node-matrix.json`。

| 内置 Action | 固定输入、结构拒绝、Markdown、布局、Undo、编辑重开 | 真实 GLM 创建、编辑重开 |
|---|---|---|
| agent | done，r3 | done，r1 |
| approval | done，r3 | done，r1 |
| code | done，r3（检查实际 Worker） | done，r3（检查实际 Worker） |
| declarative_http | done，r3 | done，r1 |
| if | done，r3 | done，r1 |
| list | done，r3 | done，r1 |
| loop_over_items | done，r3（含容器净间距） | done，r4 |
| merge | done，r3 | done，r1 |
| model | done，r3（含首次 ACK 丢失） | done，r1 |
| reply_message | done，r3 | done，r1 |
| send_message | done，r3 | done，r1 |
| set | done，r3 | done，r1 |
| sub_workflow | done，r3 | done，r3 |

最终 `p8-inspect-r4-final` 的 6 项协议全部通过：刷新前丢失布局 ACK（重放原回执、保留刷新前画布、仅确认一次）、停止/关闭面板隔离迟到响应、Host 强退及双副本恢复、Gateway 强退后未知结果不重放、人工并发修改不被覆盖，以及原有 UI 设置/创建/Undo/ACK 丢失/重连流程。JUnit 为 6 tests / 0 failures / 0 errors。

所有测试均由 `pytest tests/e2e` 创建临时 Kubernetes 环境并调用真实生产构建的 Playwright。协议测试区分了应用时回执与挂载后尺寸测量的画布快照，分别断言回执完全一致和保存画布不变；失败轮次证据全部保留。最终两个 pytest 入口均通过，耗时 792.40 秒，包含环境初始化与清理。

### 本地发布与数据保留

2026-10-16 00:14（Asia/Shanghai）发布 `local-plan8-inspectors-20261015`，使用上述 r4 镜像摘要。`agentxctl validate --cluster` 返回 valid，upgrade 返回 ready；Helm revisions：dependencies 5、control 4、runtime 12、observability 4。12 个本地 Deployment 均恢复为 1/1。

使用本地 CA 验证 HTTPS，首页及入口 JS/CSS 返回 200，页面入口与运行容器文件一致；bootstrap.required=false，匿名 Runtime 会话访问返回 401。Control/Runtime Schema 校验和与现有基线一致。

发布前后对比 `production-before.json` / `production-after.json`：用户报告的草稿仍为 revision 10，contentHash、Definition 和 EditorDocument 哈希全部一致；原 glm-5.3-flash Alias、Deployment、授权凭据引用及租户设置 version=2 一致。未修改原草稿布局；用户可通过工具栏“自动布局”整理既有画布，新 AI 拓扑编辑自动应用布局。

四轮临时 Namespace 均已 purged，本地服务已恢复。发布后原环境验证采用带 CA 校验的 HTTP 与数据库只读对比；生产构建的交互操作由上述临时 Kubernetes Playwright 验证，未在不受信任的内置浏览器证书页面绕过 TLS。

关键证据：`node-matrix.json`、`images-r4.json`、`e2e-r4.xml`、`production-validation.json`、`upgrade.json`、`verification.json`、`cleanup.json`，均位于本节前述本地证据目录；各轮 JUnit/Trace/截图在 `.local/artifacts/playwright/helm-agentxctl/<run-id>/`。
