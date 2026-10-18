# plan8：版本画布与 AI 工作流编辑

当前版本管理采用 [版本列表与画布管理](09-version-canvases.md)：版本成为唯一画布入口，草稿由版本拥有。当前产品规则见下文；原多独立草稿阶段的任务和验收记录保留历史事实。

2026-10-16 版本列表与画布管理重构已完成：版本定稿/草稿派生、事务发布、只读详情、还原、归档和成员弹框均已实现。最终候选通过 482 项前端测试及 5 项 Kubernetes 系统验收（22 个浏览器场景），Rust/契约检查与清理记录见 [重构验收](09-version-canvases.md#验证记录)。

状态：done（本次非压测范围）。依据 2026-10-14 的用户讨论及 grilling 决策，43 个任务和 26+3 功能场景已完成并验证。完整非压测回归及同一候选的失败项复测已关闭；分轮结果见 [实施与测试报告](implementation-report.md)。

2026-10-15 用户确认“压测暂时不做”。本次已执行完整非压测回归及失败项复测 `pytest tests/e2e -m "not capacity"`；容量认证保留未完成状态，不修改冻结阈值。200 节点保存/静态解析仍属于编辑功能回归。

2026-10-16 已完成追加的节点面板、Markdown、自动布局与回执恢复修复并发布本地版本。13 类内置节点的固定输入和真实 GLM 场景共 26 项经分轮验证/复测通过，最终 6 项恢复协议通过，详见 [AI 节点编辑回归](evidence/2026-10-15-node-editing.md)。

2026-10-18 [应用内测试集与平台 AI 评测](12-application-evaluations.md) 已实现并完成本地定向验收：应用内统一入口、固定输入输出契约、复制补全、不可变快照、多集多版本批次及独立平台裁判授权均已落地。完整系统场景和真实模型增量分轮通过，详细结果与恢复记录见 [应用评测验收](evidence/2026-10-18-application-evaluations.md)。本次未重置常驻数据库或部署产品版本。

2026-10-16 使用反馈增量验收已关闭：真实 LightRAG 无命中、用户身份投影恢复及错误执行可见性已修复；AI 默认优先复用兼容结束节点。9 个增量系统场景分轮通过，已发布本地版本 `local-plan8-empty-20261016`，原工作流 revision 25 的真实空检索 → Agent → End 执行成功。详见 [修复与验收报告](evidence/2026-10-16-empty-results-and-execution-visibility.md)。

2026-10-16 追加的 [资源补全、独立知识检索与错误出口](08-resource-completion-and-knowledge.md) 已实现并发布本地版本；17 个系统场景经同一最终候选分轮验证/复测通过，真实 GLM 创作、无损保存与 SSE 修复均已验证。失败历史、复测与原数据保留证据见 [增量验收记录](evidence/2026-10-16-knowledge-and-resources.md)。

## 1. 目标与已确认决策

用户创建 Workflow 后进入版本列表，点击版本的“编辑”或“新建画布”进入编辑器，再通过 AI 图标输入需求。AI 使用现有 agentx-agent-core 修改当前工作文档，沿用手动编辑的命令、历史、保存和权限链路。

| 决策 | 已确认的结果 |
|---|---|
| Agent 内核 | 设计态 Builder 与业务 Agent 复用同一个 Rust Core，分别注入 Host Adapter |
| 操作入口 | 用户先打开画布；AI 始终绑定当前 Workflow/Draft，不创建或切换业务对象 |
| 历史版本 | 每个版本最多拥有一个共享工作草稿；已发布内容冻结，草稿发布生成新版本并记录固定 baseVersionId |
| 编辑方式 | AI 变更进入与手动编辑相同的 Editor Store、Undo/Redo、Recovery、自动保存与保存按钮链路 |
| 版本与部署 | 未发布版本随保存更新；用户在列表发布时定稿或派生并部署，AI 无创建版本或部署能力 |
| AI 验证范围 | 只进行静态校验；所有 Workflow、节点、代码及集成的实际运行和测试，均由用户手动在 UI 触发 |
| 结果辅助修复 | 用户可把手动执行的结果交给 AI 只读检查；AI 修复后仍只静态校验，不自动重跑 |

最后一项执行约束优先于早期讨论中的自动调试方案。自然语言要求“帮我运行”、ask_user 回答、模型工具调用、恢复重试，都不能代替用户在 UI 的运行操作。

## 2. 文档目录

| 文档 | 内容 |
|---|---|
| [01-product-and-versioning.md](01-product-and-versioning.md) | 用户流程、版本工作文档、版本来源、界面与产品边界 |
| [09-version-canvases.md](09-version-canvases.md) | 版本列表、定稿与发布、只读详情、还原和本次重构验收 |
| [12-application-evaluations.md](12-application-evaluations.md) | 应用内测试集、完整契约、平台 AI 评分与批次报告 |
| [evidence/2026-10-18-application-evaluations.md](evidence/2026-10-18-application-evaluations.md) | 应用评测重构的代码、单元/契约、桌面和真实模型验收记录 |
| [11-knowledge-connections.md](11-knowledge-connections.md) | 外部知识连接、查询配置、跨连接多来源检索与验收 |
| [02-architecture-and-contracts.md](02-architecture-and-contracts.md) | Core/Host、数据所有权、模型设计态 Effect、恢复与授权 |
| [03-tools-and-context.md](03-tools-and-context.md) | 七个业务工具、静态闭环、上下文投影与限制 |
| [04-editor-data-and-api.md](04-editor-data-and-api.md) | 编辑命令/ACK、保存竞态、数据与 API、精确修订调试 |
| [implementation-plan.md](implementation-plan.md) | 实施顺序、原子任务、落点、删除与退出门禁 |
| [05-e2e-acceptance.md](05-e2e-acceptance.md) | 单元/契约/浏览器/临时 Kubernetes E2E 矩阵 |
| [implementation-report.md](implementation-report.md) | 已实现代码、逐项验收与真实证据 |
| [cutover-audit.md](cutover-audit.md) | 单草稿入口、调用者、引用保护、数据域和生成协议的切换清单 |
| [99-traceability.md](99-traceability.md) | 能力、任务与验收追踪 |

## 3. 当前基线与复用项

- 当前唯一运行 Definition 为 8.0，Node Manifest 为 3.0，插件 SDK/RPC 为 2；plan8 不新增 Workflow 执行 DSL，不因多草稿改变 Definition 内容语义。
- Agent Core Contract 常量为 1.3；Pi 固定参考为 a69bef789bc95abf0acee16f7b4660b70b650bb9 / 0.84.2。以生成器和当前代码常量为准，不沿用 plan3 历史文档中的旧版本号。
- Core 已有 Model/Tool/State/Event/Clock/Budget Port、工具注册、压缩与可恢复 Operation；同步 Port 通过持久 Pending、冻结请求与 Resume 实现等待 ACK/用户答复；挂起后释放 Host 调用栈。
- 现有画布已有手动命令、实体 Patch、Undo/Redo、1.4 秒服务端自动保存和 750ms 本地 Recovery。批量命令复用同一 Store/history；完整文档序号、回执、保存竞态保护已实现。
- 数据库支持多个独立 Draft 和原子 revision 0；历史 Version 有只读画布，版本来源由同 tenant/Workflow 的复合外键约束。
- 现有 Catalog、Compiler、动态契约解析、资源六态、发布、手动调试、Trace/Artifact 与 pytest/Playwright 基础设施继续复用。

基线定位与真实缺口见 [04](04-editor-data-and-api.md)。Plan3/Plan7 尚未关闭的门禁仍保持原状态；plan8 文档交付不能替代它们的验收。

## 4. 架构调整及影响

新增 Control Studio Builder Host，运行设计态 Core，并保存 Builder 会话/操作；模型调用仍经 Runtime 的受限设计态 API 执行。这对当前“Control 不执行 Agent loop”增加明确的设计态例外，原因是编辑必须与 Control 草稿、当前编辑器和对象权限一致。

业务 Worker 不修改 Draft、不回读 Control，业务 Agent 和生产执行仍只消费不可变 Snapshot。Runtime 设计态模型 Effect 必须有独立来源身份和账本，不能伪造 Workflow Execution/Node Execution。详情见 [02](02-architecture-and-contracts.md)。

多草稿要求 Draft、Revision、Overlay、资源申请、缓存、Recovery、Builder 和 Debug Source 全链携带 draftId；Version 编号仍在 Workflow 作用域递增。旧单草稿接口和假设在实施时直接删除，不保留兼容入口、迁移或 fallback。

## 5. 实施依赖与完成定义

~~~text
P8-00：事实审计、协议冻结、共享 Core 挂起/模型 Effect Spike
  → P8-01：多 Draft / Version 来源 / 历史版本画布
  → P8-02：统一编辑命令、保存一致性、精确来源调试
  → P8-03：Builder Host、设计态模型、会话与事件
  → P8-04：七工具、上下文、AI 面板、静态编辑闭环
  → P8-05：全量 E2E、删除旧入口、文档与证据收口
~~~

P8-00–05 已完成实现、相关回归、系统 E2E、文档与脱敏证据。完整 136 项回归为 135 passed、1 failed；唯一失败已修正测试交互，并在同一产品候选上复测通过。AI 编辑期间业务执行计数不增加、手动 UI 运行后才产生执行记录的断言已通过。任务以各自实际证据标记 done，容量认证保持 deferred。

## 6. 范围约束

- 正式验收仅为 web 桌面端，复用统一组件、设计令牌、国际化和主题。
- AI 只编辑当前 Draft 内的节点实例，不开发新插件类型，不创建新 Workflow/Draft，不修改其他画布。
- 不新增 Git 分支/合并体系、个人 Draft ACL、子 Agent、后台自主运行、计划审批模式或插件市场。
- Builder 不提供任意 shell、业务工具执行、Credential 测试、资源授权写入、版本创建、部署和运行入口。
- Core 原有 read/write/edit/bash 及业务 Agent Sandbox 行为继续保留；Builder 默认不绑定 Sandbox，不注册四个文件/命令工具。节点代码通过已有参数契约编辑，静态检查走专用校验工具。
- 自动化复用 uv、agentxctl、cargo xtask 和 pytest tests/e2e；新增仓库编排只使用 Python，浏览器业务测试继续使用 TypeScript Playwright。
- 修改所属基线 DDL，按仓库规则重建开发/测试数据；不写旧数据转换。证据在 .local/artifacts，发行物在 .local/dist。

## 7. 参考依据

- [仓库 Pi 行为审计](../plan3/p3-00/reference-audit.md)：内核、上下文、四工具与基础设施适配边界。
- [Pi 固定版本 Core 文档](https://github.com/earendil-works/pi/blob/a69bef789bc95abf0acee16f7b4660b70b650bb9/packages/agent/README.md)：可注入工具、事件和上下文投影。
- [n8n 工具参考](https://github.com/n8n-io/n8n/blob/master/packages/@n8n/instance-ai/docs/tools.md)：节点发现、资源查询与构建校验可参考；其中自动执行/发布能力不进入 Agentx Builder。
- [现有前端架构](../10-frontend-architecture.md)、[Definition 8.0](../12-workflow-5.md)、[插件契约](../plan6/contracts.md)、[E2E 规范](../plan/e2e-testing-standard.md)。
