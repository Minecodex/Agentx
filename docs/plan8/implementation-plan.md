# plan8 实施任务

状态：done（本次非压测范围）。43 个任务及 26+3 功能场景已完成并验证。完整回归与同一候选的失败项复测证据见实施报告；容量压测按用户要求 deferred。

## 1. P8-00：基线审计与契约冻结

依赖：当前源码、Plan3 Core、Plan6 Manifest/Runner、现有 E2E 可读取。

- [x] P8-00-01 审计全部单草稿 SQL、路由、Query/Recovery Key、资源申请、包引用、Work Package、删除和 Retention；形成完整切换清单。
- [x] P8-00-02 冻结 Draft/Version 来源、Editor Intent/Command/ACK、Builder Session/Run/Entry、静态报告、事件与错误 DTO，并由 Rust 生成 Schema/OpenAPI/TS。
- [x] P8-00-03 扩展通用平台 Tool Origin、Pending Effect/Resume 契约；用同一 Core 完成等待编辑器/用户答复、强退和一次结算 Spike。
- [x] P8-00-04 从业务 Model Adapter 抽取来源无关的模型 Use Case，冻结设计态 Effect/Receipt、来源判别、JWT、Ledger、Usage 和取消契约。
- [x] P8-00-05 固定 Builder 七工具与禁止能力；证明聊天、ask_user、未知工具、Provider、恢复与 JWT 均不能启动实际测试或发布。
- [x] P8-00-06 冻结 Draft revision 0 与 Version 全局编号锁、幂等来源、权限、初始 DDL/备份清单及协议版本切换。
- [x] P8-00-07 冻结 Draft Revision/Version 运行来源联合、账本相关 Usage/Quota/Artifact/Retention，以及纯静态 Resolver 的无网络/无资源隔离与 UI Provider 边界。

落点：src/crates/agentx-agent-core、agentx-domain、agentx-api-types、agentx-runtime-contracts、contracts、deploy/migrations、tools/agentx-boundary-check。

退出：Core 不新增基础设施依赖；设计态模型不伪造 Execution；Pending 不占住等待线程；契约可生成且边界/旧字段拒绝测试通过。

## 2. P8-01：多草稿与版本画布

依赖：P8-00。

- [x] P8-01-01 修改 Control 基线 DDL：Draft 元数据、Revision 作用域、Version 来源、Overlay、资源申请/包引用与删除保护。
- [x] P8-01-02 创建/导入 Workflow 和新建/版本派生 Draft 时原子写 revision 0、布局 hash 和引用；保留精确版本引用及可编辑问题。
- [x] P8-01-03 替换唯一 Draft CRUD/保存/修订/静态校验/导出 API；所有操作校验 Workflow 对象权限和 Draft 归属。
- [x] P8-01-04 Version 从准确不可变 Revision 编译；Workflow 行锁分配版本编号；来源完整、重复请求幂等、并发多 Draft 编号正确。
- [x] P8-01-05 更新 Workflow 列表/详情 DTO 与 SQL，移除 JOIN 唯一 Draft 和单一 revision 字段，提供草稿/版本入口。
- [x] P8-01-06 实现 Draft 列表、新建/归档/恢复、版本只读画布、基于版本创建 Draft 和明确来源展示；只读在命令、快捷键、面板、自动保存和 AI Binding 一致执行。
- [x] P8-01-07 切换显式 Draft/Version 路由、Query/Recovery/Inspector/资源和插件缓存；Store 按编辑器实例隔离，不复用全局单例跨画布状态。
- [x] P8-01-08 调整资源/Grant 申请来源、审批失效检查与所有草稿引用查询；Grant 主体保持 Workflow Service Identity。

落点：platform-control 的 workflow_api/operations/resources、resource_api/request_validation、canvas_plugin_*、deletion_api/work_packages，Web workflows/workflow-designer/router，Control DDL。

退出：两个版本可创建独立草稿且互不影响；从已有版本派生后有准确的 revision 0 快照，可直接由用户创建新版本；归档不影响已部署版本；跨对象访问拒绝。revision 0 手动运行的全链验收在 P8-02 完成，不能作为本阶段进入下一阶段的前置门禁。

## 3. P8-02：编辑事务、保存和手动执行来源

依赖：P8-01。

- [x] P8-02-01 在现有 commit/history 上实现统一 applyEditorCommand，归拢手动/Inspector/连接/复制粘贴/删除/布局语义，不新增并行 Reducer。
- [x] P8-02-02 批次操作覆盖全部内置节点、动态分支、Loop、保护 Exit、Binding/输出、布局/注释/分组；一批命令一个 Undo 事务。
- [x] P8-02-03 增加完整 localDocumentRevision、文档 hash、Binding、命令去重回执/ACK 和 stale/未知结果对账；Undo 后禁止旧命令重放。
- [x] P8-02-04 修复旧保存响应清除后续 dirty/Recovery、跨草稿旧响应污染、保存失败及冲突；字段错误按提交快照定位实体，AI 无覆盖冲突或独立保存动作。
- [x] P8-02-05 手动 Debug API/Work Package/Source/幂等/Snapshot/投影全链采用真实来源联合：Draft ID 与精确 Revision ID 或 Version ID；接受有效 revision 0，删除 Head 再读取竞态。
- [x] P8-02-06 Overlay、运行结果和“交给 AI 分析”入口按 Draft/Execution 隔离；保留四种手动调试、Pin/Mock、现有运行/取消/恢复 UI。
- [x] P8-02-07 对版本创建/执行/导出/手动调试及新 UI 路径补齐现有对象权限检查；保存与校验门禁对人工/AI一致。

退出：保存期间继续编辑不丢脏状态；切换与 Undo/重连不串文档；从版本派生的 Draft 无需修改即可手动运行 revision 0，执行永远消费用户选定快照；旧单草稿入口不可调用。

## 4. P8-03：Builder Host 与设计态模型

依赖：P8-00、P8-02；生产模型/压缩复用部分必须有对应 Plan3 回归证据。

- [x] P8-03-01 实现 Control Builder Session/Run/Entry/Command Repository、单开放 Run、Lease/Fencing、deadline/预算、保留引用与恢复循环。
- [x] P8-03-02 以同一 AgentCore 注入 Control Model/Tool/State/Event/Clock/Budget Adapter，支持通用 Pending/Resume；禁止复制业务 Loop 或压缩。
- [x] P8-03-03 Runtime 增加受限设计态 Model Effect 提交/查询/流式/取消/Receipt，复用 Provider/Vault/Egress/计量，隔离业务并发和凭证。
- [x] P8-03-04 扩展 runtime_calls/Usage/Trace/Quota/Artifact 授权与 Retention 的来源和 Query/CHECK；Builder 成本不计入 Workflow Execution，不新增伪 Attempt。
- [x] P8-03-05 实现租户设置的设计态 Model/Credential 授权、冻结授权 epoch、操作者/Workflow 权限及短期目标绑定 JWT；撤权立即停止后续效果。
- [x] P8-03-06 实现 SSE Cursor/消息结算、命令 ACK/问题答复、断线对账、切换/关闭/停止和迟到结果拒绝。
- [x] P8-03-07 补齐 Host/Runtime 强退、Drain、多副本恢复、已结算 Receipt 去重、Unknown Outcome、空闲回收与 Retention。
- [x] P8-03-08 静态 resolveDefinition 通过现有 Sandbox Manager/OpenSandbox 的固定 resolve-only 隔离模式执行；新增设计操作租约来源，禁止网络/Secret/Host 桥接/业务动作，覆盖顶层源码副作用与逃逸反例。

落点：Control 新 studio_builder_* 模块，Runtime 新 builder_model_* 模块，共用模型调用 Use Case、账本/内部协议与查询；保持现有构建产物和数据域。

退出：同一 Core 可完成多 Turn/工具/挂起/压缩与恢复；无宿主文件/任意 shell；Control 离线不影响既有生产 Agent；使用量与终态有可信 Receipt。

## 5. P8-04：工具、上下文与 AI 面板

依赖：P8-03。

- [x] P8-04-01 实现 workflow_read/node_catalog/resource_query，按需读取本地文档、权威 Manifest、隔离的动态有效契约和只读资源六态/元数据；不转发通用 Provider。
- [x] P8-04-02 实现 workflow_edit/workflow_validate 的命令准备、影响/范围校验、ACK 后结算与实际文档静态验证，不直接保存或运行。
- [x] P8-04-03 实现 execution_inspect/ask_user：仅用户 UI 选定结果、现有 Artifact 授权、持久问题和设计答复；禁止升级运行能力。
- [x] P8-04-04 实现分层 Context Projection、Core 文本摘要及结构化当前事实校准、人工编辑/Undo/资源变化失效、上下文预算和重复无进展终止。
- [x] P8-04-05 实现 AI 图标/租户模型状态/对话/进度/差异/问题定位/停止，复用统一 UI、i18n、主题、键盘及桌面面板布局。
- [x] P8-04-06 区分已应用/待保存/已保存/静态通过/真实手动证据；引导用户 UI 测试、创建版本和发布，不自动提交。
- [x] P8-04-07 检查未知工具、运行诱导、敏感参数、注入指令、跨草稿结果和资源撤权；Registry/Dispatcher/JWT/API 四层拒绝越界，AI 组件挂载/参数变化不能间接执行测试。
- [x] P8-04-08 严格验证七工具 action/inputSchema，把必需错误/hash/Cursor/Artifact 引用投影到现有 ToolResult.content；同步 Platform Origin 的 Schema/TS/Fixture 与业务 Agent 回归。

退出：用户可在当前草稿完整 AI 创建/重构或局部编辑，保存重开/Undo 正确；AI 全程不增加业务执行计数，用户手动运行后可辅助分析修复。

## 6. P8-05：验收与删除收口

- [x] P8-05-01 完成 [E2E 矩阵](05-e2e-acceptance.md) 与契约、意义明确的单测、桌面浏览器、故障/权限/成本验证。
- [x] P8-05-02 删除旧单草稿 API/路由/DTO/SQL/缓存/Fixture、重复校验/命令、AI 运行/提交设计残留，不留兼容层或 fallback。
- [x] P8-05-03 执行 cargo xtask check、契约生成漂移、相关 Rust/前端检查、Helm/边界检查，再执行完整非压测 `pytest tests/e2e -m "not capacity"` 回归；不以聚焦测试替代完整回归。2026-10-15 用户明确压测暂缓，容量认证继续未完成，不修改冻结门禁。
- [x] P8-05-04 核对多画布/大图资源、设计态与业务容量隔离、多副本恢复、Secret/网络、Retention 引用和开发服务恢复/Namespace 清理。
- [x] P8-05-05 同步 docs/01–13 中受影响的事实、Plan3/PlanV2 有效边界、追踪矩阵和实际证据；只对真实完成阶段标记 done。

## 7. 全局门禁

- 不增加历史协议双读、数据迁移、旧路由兼容和替代 Agent Runtime。
- 前后端生产文件不超过 2000 行；先复用现有依赖，按职责拆模块。
- 自动化使用现有 uv/Python 与 Rust 运维入口；Playwright 为业务 UI 操作，pytest tests/e2e 为系统唯一编排入口。
- 本地证据写 .local/artifacts/e2e/<run-id>，成功/失败均清理临时 Namespace、进程并恢复原开发副本。
- 真实模型、生产 CNI/强隔离等未具备的门禁注明前提和未完成状态，不用 Fixture 或文档宣称实际验收通过。

## 8. 已勾选项的证据

P8-00–05 的勾选项对应 r34 完整 `cargo xtask check`、Fixture/真实 GLM 主流程 2/2、扩展矩阵 20/20 和全部 10 个 plan8 Python 入口。覆盖 Resolver 隔离、200 节点静态保存、账本/Retention、Control 离线业务执行、资源申请、跨租户、历史引用及保存/删除竞争。完整 136 项非压测回归为 135 passed、1 failed、0 errors；唯一插件悬停失败修正测试交互后，在同一候选 r34canvas 中通过，包含 Trace 降级与恢复。最终原环境、模型配置和进程清理已核对。分轮原始结果见 [实施报告](implementation-report.md)。
