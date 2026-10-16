# 资源补全、知识检索与错误出口验收

状态：此前交付已完成；2026-10-16 真实使用暴露的空结果与用户投影问题现已修复并完成增量验收，见 [后续修复](2026-10-16-empty-results-and-execution-visibility.md)。实现、真实模型验证、17 个系统场景分轮验收和本地发布均已完成。容量压测按用户要求不执行。失败尝试及对应修正保留在下文。

## 实现范围

- 资源查询标注完整结果中的唯一有效名称匹配，提供真实 ID/版本；AI 按节点必需槽位和用户指定用途查询，缺项留空，歧义询问，不代授权。静态检查增加缺配置、不可用、禁用、待授权和未接错误出口提示。
- 公开 `knowledge_retrieval` Action：一个 RAG 资源、query、Top K；复用 LightRAG `/query/data` 与 RAGFlow `/api/v1/retrieval`、冻结资源、调用账本和凭据链路。输出 text、documents、files；文件为已返回的附件来源引用，不下载原件、不伪造 Artifact。
- Agent 无模型时可以保存草稿；已有引用的类型、槽位、版本和单选数量继续严格检查。完整编译仍要求必需资源。
- Exit 必填映射按接入的 main/error 端口检查；AI 默认将顶层节点的未处理错误分别连到 Exit.error，Loop 内遵守容器传播规则。
- 节点发现支持多词和本地化名称；Exit 边界契约来自 Rust 的 ExitParameters/WorkflowEnd Schema，示例区分共享输出声明与每个 Exit 的 InputBinding 映射。

设计详见 [增量设计](../08-resource-completion-and-knowledge.md)。

## 本地检查

- Domain 11 项、编译器 86 项、Control 70 项通过；新增节点搜索/Exit 契约 2 项通过。
- V2 Runtime 首轮 167 通过、1 失败、2 原有忽略；唯一失败是参数消费矩阵未登记知识节点，补齐后定向复测通过。最终知识输出 2 项、RAG 协议 4 项通过。
- 前端首轮 467 通过、1 失败；唯一失败是目录仍断言 13 类 Action，改为 14 类后相关 2 文件/9 项通过。
- 前端生产构建、类型检查、Lint、浏览器测试类型检查、相关 Rust Clippy、验收约束 149 项和架构边界检查通过。没有用这些检查替代浏览器运行证据。

本地日志在 `.local/artifacts/plan8-knowledge-20261016/`，首批日志也包括 `.local/artifacts/plan8-resource-*.log`。

## 首轮系统测试

`p8-knowledge-r2-20261016` 使用 `plan8-knowledge-20261016-r2`，12 镜像源码标签一致：`74ac6aafec1bbceb6a588fc50166be28ab9b58c73e20dbded5bfb67773217857`。

七个浏览器场景：2 通过、5 失败。

- 知识节点的合法简写 Binding、Markdown、布局、点击编辑、保存重开和 Undo 通过；已有多 Exit 手动编辑/运行回归通过。
- 两种知识库协议场景在编辑阶段被正确拒绝：Fixture 将共享出口字段写成 sourceNodeId，违反小写字段名规则。已改为 source_node_id，Binding 仍引用 item.sourceNodeId，等待实际复测。
- 真实 GLM 已通过资源查询找到指定知识库和模型；多词节点搜索漏掉 model，且 Exit 无可查询契约，导致模型使用 Agent 并混淆 Exit Schema/Binding。错误修改被拦截，未覆盖草稿。已补齐搜索与边界契约，单测通过。
- 真实模型后续修正请求及另外两个真实场景均返回 `DESIGN_MODEL_PROVIDER_REJECTED`。Runtime 日志确认是 HTTP 402 Payment Required（不是网络地址拒绝）；未替换连接或凭据，已请用户恢复服务商额度。
- 首轮 pytest 失败后正常删除三个临时 Namespace、恢复开发服务；Mem0 恢复为 1/1。

失败记录保留在 `.local/artifacts/e2e/p8-knowledge-r2-20261016/` 和 `.local/artifacts/playwright/helm-agentxctl/p8-knowledge-r2-20261016/studio-knowledge/`；不覆盖失败报告。

## 第二轮系统测试

`p8-knowledge-r3-20261016` 使用 `plan8-knowledge-20261016-r3`，12 镜像源码标签一致：`3a2a2a3b646e797c2c093c1e2f829ee97b63b7df8875e8ce6559f90d099c42ae`。真实模型因 402 未重试；五个固定浏览器场景中 4 通过、1 失败。

- 知识节点 Inspector、Markdown、布局、编辑重开再次通过；多 Exit 手动回归再次通过。
- LightRAG 与 RAGFlow 均完成手动修改 Top K、保存、重开、授权和三次按钮运行。revision 2 返回命中片段和已有附件，Top K=1 被遵守；revision 3 返回空文本/数组；revision 4 经独立错误 Exit 进入 failed，映射的 error.outputs.source_node_id=retrieve。AI 编辑期间没有业务执行，只有 UI 点击运行后出现三条执行记录。
- 从两份 Playwright Trace 只读提取并再次断言六条终态记录，保存为 `retrieval-executions-r3.json`。
- Agent 五类绑定已应用并保存，但测试错误地要求无版本的 Rag/Memory 保留 resourceVersionId:null，而标准序列化省略该字段。Fixture 已改为有版本时才提供该字段，保存重开/清空资源的后续步骤需定向复测。产品源码与 r3 镜像未变。

## 阶段性待验收项（已在 r6 关闭）

- Agent 五类资源绑定、版本和未授权状态保存重开。
- 真实模型：命名知识/模型、未匹配缺项总结、MCP/记忆/技能/知识附件与错误路径自动配置。
- 最终候选环境清理与本地发布验证。

## 保存重开与额度恢复后的真实复测

- `p8-knowledge-r3-agent` 揭示真实存储问题：Agent 五类绑定已保存重开，下一次 AI 请求因 EDITOR_BINDING_STALE 被拒绝。原始 ACK 的坐标 209.66666666666666 经 MySQL 8.4.11 JSON 存储变为 209.66666666666663，而记录仍保留原 hash。只读 SQL CAST 已独立复现。
- 改为无损 JSON 文本列（LONGTEXT + JSON_VALID CHECK），覆盖 Draft/Revision/Version Definition/Editor 以及 Builder Session、Entry、Effect 的文档/命令/回执载荷；重连按 hash 恢复序号，未放宽 ACK。真实 MySQL 基线初始化、并发/重放及 13 个字段的 SQLx 写入读取/hash 检查通过。E2E 增加 ACK 布局与保存布局、会话文档 hash 相等断言。
- `p8-knowledge-r3-live-retry` 使用原 r3，在额度恢复后运行三个真实 GLM 场景：命名知识库/模型及两个独立错误 Exit 通过；Agent 的 MCP/技能/记忆/知识库/模型五类名称绑定及精确版本保存重开通过。两个通过场景均未产生业务执行记录。
- 无匹配资源场景失败揭示前后端门禁不一致：后端允许 Agent 缺模型草稿，前端仍以 AGENT_MODEL_REQUIRED 拒绝事务。已将前端改为仅拒绝重复模型，完整运行门禁保持；新增无模型草稿保存及重复模型错误定位回归，相关 2 文件/15 项通过，前端构建通过。待 r4 真实复测关闭。
- 全前端检查在最后一处前端修复前为 97 文件/468 项通过；最终修复相关定向测试 15 项通过。验收约束 149 项、边界检查通过。MySQL 测试曾因动态 SQL 违反边界门禁，改为固定 SQL 后重测通过，未增加豁免。

最终前端收口：新增中文提示先被词汇门禁拒绝，已将 Agent 改为“智能体”；另一个 Playground 并行测试瞬态失败在无并发构建的全量复测中通过。最终 97 文件/469 项全部通过，Control Infrastructure 全目标严格 Clippy、浏览器类型检查通过。构建期间有两次主动停止候选以纳入检查发现的修复，未将部分镜像用于发布；最终候选构建日志为 images-r4-checked.log。

## r4 系统回归与 SSE 收口

r4 的 12 镜像源码标签为 `de598df3ac103be4487fd59b0416dd1e96b944e40024128c7c190ace34703574`。知识/资源组 8 个场景中 6 通过、2 失败：两种检索协议、Inspector、多 Exit、真实缺资源草稿、真实 Agent 五类绑定通过。

- 新增 ACK 捕获使用了 Playwright 默认 20 秒等待，提前结束仍在等待编辑回执的 Fixture；已改为和设计操作一致的有界等待，并用 Promise.all 同时处理请求捕获和创作完成，未删除 hash/布局断言。
- 真实知识+模型场景已经编辑并完成静态校验，但在最终摘要前超过测试 180 秒上限；真实模型场景改为 300 秒，仍低于产品 Run 的 10 分钟 deadline。
- Trace/数据库对照显示编辑命令在服务端已持久化，ACK 约 15 秒后到达；Web NGINX 默认缓冲，Builder SSE 未显式关闭。已按官方机制增加 X-Accel-Buffering:no，新增完成会话最后一个小事件须在 Web 代理后 5 秒内抵达的验证。该修复尚待 r5 实测，不将 r4 的时间限制失败宣称为通过。
- r4 的 9 个编辑/版本/恢复场景全部通过（10.4 分钟）：布局 ACK 前重载、取消、压缩、Host/Gateway 强退、版本来源及多草稿隔离、保存竞态、人工编辑冲突、原子 Undo 与 ACK 丢失重连。包含 SSE 修复的 r5 将重新执行这组回归。

## r5 候选复测

12 镜像源码标签一致：`61e5787a29aa37669db27d16ad02db0a5050169afd69aa1647847179805a36fd`。最终前端产物、Runtime、Control 与基线 DDL 均包含此次修复。

Agent Fixture 的 ACK/保存布局比对发现测试断言过严：React Flow 在 ACK 后正常补充节点 width/height。测试改为数量相等、已确认字段严格匹配，坐标仍逐值精确比较；没有四舍五入或删除 hash 验证。修正仅涉及测试，产品候选不变，需定向复测。

核对 RAGFlow 官方检索协议后补齐原生 `document_keyword`、`document_metadata` 和 `image_id` 映射。`image_id` 作为去重的外部附件引用保留，不拼接 URL、不获得下载权限。Fixture 改为官方原生字段形状，增加标题/元数据/附件 ID 的浏览器断言，避免只用扩展 files 数组验证。该修改进入 r6，需重新验证检索和最终候选。

r5 的真实 Agent 五类附件场景中，资源 ID/版本查询正确，但模型生成的 Exit 引用遗漏 selector 层。结构校验拒绝后，validate_documents 把字段路径和错误码丢掉，只留下通用消息；模型连续修错后询问用户，测试未完成。已让编辑事务返回原始 fieldErrors，Exit 解析错误保留具体缺失字段，并明确嵌套 selector 形状及共享契约/所有受影响 Exit 必须原子提交。错误结构仍拒绝，不增加宽松兼容。新增对实际失败形状与缺必填映射的反馈回归，r6 继续验收。为纳入此修复主动停止一次 r6 部分构建，未发布部分镜像。

r5 最终统计为知识/资源组 6 通过、2 失败；协议组 9 项全部通过（4.9 分钟）。两处失败分别是上述布局断言和真实 Agent 修错受阻。pytest 正常清理全部临时 Namespace，12 个开发 Deployment 恢复到 1/1，见 cleanup-r5.json。r6 本地复核：Domain 12、Control 73、相关编译器 47、知识输出 3 项通过，浏览器类型检查、严格 Clippy 和边界检查通过。

r6 的真实知识+模型场景已完成两类资源绑定和错误接线，但两个错误分支共享一个专用错误 Exit，因“两个独立 Exit”断言失败。共享处理是合法结构，不归因为 Schema 错误。测试自然语言输入改为明确请求分别创建两个独立错误结束节点（原输入只说“按默认规则”）；保留数量、非成功出口及完整错误映射断言，待同一 r6 候选定向复测。

r6 真实 Agent 场景已完成编辑、保存及静态校验，没有再次因 Selector 解析而反复失败；但把 Agent.error 连到受保护成功 Exit 的 error 输入，未通过独立错误节点断言。该场景也将测试需求明确为另建错误 Exit，再对同一候选复测。当前固定 Agent 场景完整通过：五类资源/版本、ACK 坐标精确一致、会话 hash、5 秒 SSE 小事件、保存重开与清空资源再次创作；两种知识协议的 6 条手动执行结果已从 Trace 核对并保存 retrieval-executions-r6.json。

r6 首轮最终为 15 个浏览器场景通过、2 个真实模型拓扑场景失败；其中 9 个编辑/版本/恢复场景全部通过（4.4 分钟）。首轮结果保留，不把改写测试输入当作已通过证据。下一轮只对这两个明确要求独立错误出口的场景做实际复测。

## r6 最终功能验收

`p8-knowledge-r6-topology` 使用同一 r6 产品候选，两个真实 GLM 用例均通过（8.3 分钟）：明确请求的知识/模型分别独立错误 Exit，以及 Agent 五类名称绑定、精确版本、独立错误 Exit、保存重开和零业务 Execution。没有取消或弱化结构断言，没有改为 Fixture 响应。结合 r6 首轮的 15 个通过场景，本次 17 个系统场景已分轮全部通过。

最终候选源码摘要：`26ad1338d743535544f03b33f1e80d9520205ae544cea25c92fd34182b0524a5`，12 个镜像标签一致。真实模型用于创作能力验证；LightRAG/RAGFlow 的协议 Fixture 用于可重复的命中、无命中和失败执行验证，六条按钮运行的精确修订结果见 retrieval-executions-r6.json。尚待最后一轮环境清理和本地发布完成后关闭交付状态。

## 本地发布过程

新 Control 库 `agentx_control_plan8_20261016` 由正式 migrator 初始化，13 份基线校验值与源码一致；Runtime 库不变。原 Control 库保留。最终复测退出后全部临时 Namespace 已删除，12 个开发服务恢复 1/1。

数据复制第一次在元数据解析时遇到空字段被 strip 去掉的问题，尚未开始 INSERT；保护逻辑恢复原 Control，确认 117 张目标表仍为空后修正为仅去除行结束符。第二次成功复制 117 表/648 行，逐行字节摘要一致，并保留自增高水位。凭据不导出，行值留在 MySQL 内。

首次正式 upgrade 因保留当前暂停副本数 0，与 Chart 的最低 1 副本约束冲突而退出。没有改 Chart 或门禁；在一次 Deployment 更新中同时设置 r6 Control 镜像、新库和 1 副本，然后重新执行 agentxctl upgrade 完成正式发布。正在等待滚动升级及发布后数据、TLS、镜像就绪验证。

## 最终交付状态

| 验收范围 | 最终状态与证据 |
|---|---|
| 契约、缺资源草稿、结构错误反馈 | done；Domain 12、Control 73、相关编译器 47、知识输出 3 项通过；原有编译器全量 86 项及 RAG 协议 4 项也已通过 |
| 画布配置、Markdown、布局和面板 | done；前端 97 文件/469 项通过，生产构建、类型与 Lint 检查通过；新增知识节点浏览器回归通过 |
| 真实模型创作 | done；使用原配置对应的 glm-5.3-flash，按名称绑定、缺项摘要、五类附件、精确版本及显式独立错误出口均通过 |
| 检索与错误终态 | done；两种协议各 3 次 UI 运行，命中/空结果/失败及精确修订、来源/附件引用通过；AI 阶段零业务执行 |
| 保存、回执与恢复 | done；无损布局/hash、5 秒 SSE 小事件、Undo、人工冲突、ACK 丢失重连、取消、压缩、Host/Gateway 强退、多草稿/版本来源通过 |
| 系统验收 | done；r6 首轮 15 通过、2 个拓扑预期失败，明确自然语言需求后同一 r6 候选 2 项真实复测通过，共 17 项关闭。共享错误处理仍是合法图结构，未为通过测试放宽结构断言 |
| 本地发布 | done；local-plan8-knowledge-20261016；12 个 Deployment 均为 1/1；4 个 Helm Release deployed，TLS/HTTP、页面资源和实际镜像验证通过 |
| 原环境及清理 | done；117 表/648 行复制时逐行摘要一致，自增计数保留；原画布/修订/模型设置发布前后相等；原 Control 库保留，Runtime/Vault/对象存储配置保留；临时 Namespace、测试进程和初始化 Job 已清理，缩容服务已恢复 |
| 容量压测 | deferred；按用户明确要求不执行 |

发布后核验时间：2026-10-16 11:07（Asia/Shanghai）。实际页面来自 r6 容器，bootstrapRequired=false。独立 Control 新库已使用当前基线初始化；没有修改原库的迁移校验值，没有导出凭据。

主要证据位于 `.local/artifacts/plan8-knowledge-20261016/`：`images-r6.json`、`e2e-r6-final.xml`、`e2e-r6-topology.xml`、`retrieval-executions-r6.json`、`cleanup-r6-topology.json`、`database-copy-verification.json`、`production-before.json`、`production-after.json`、`upgrade.json`、`verification.json`、`schema-job-cleanup.log`。Playwright Trace、视频、截图及临时环境证据分别保存在对应的 r6-final/r6-topology 运行目录。
