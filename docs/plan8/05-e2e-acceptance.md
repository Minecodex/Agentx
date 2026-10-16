# plan8 验收与证据

状态：done（本次非压测范围）。26 项原有功能场景与 3 项模型设置场景均已通过，完整非压测回归及同一候选的失败项复测已关闭。本文保留验收要求；逐项结果和证据见 [实施报告](implementation-report.md)。系统编排唯一入口为 pytest tests/e2e，业务 UI 继续使用 tests/browser 的 TypeScript Playwright。

## 1. 分层验证

| 层 | 必须验证 |
|---|---|
| 领域/状态机 | Draft 来源/编号锁、Editor Command 单事务、localDocumentRevision、ACK 去重、Pending/Resume、静态报告来源、压缩不丢约束 |
| 协议/边界 | Rust/Schema/OpenAPI/TS 漂移、Platform Origin、严格 action/schema、禁止执行 Registry/JWT、来源联合与 revision 0 |
| Runtime 集成 | 设计态模型 Receipt/Usage/Quota/Artifact、未知结果、撤权、取消、无伪 Execution、静态 Resolver 隔离 |
| Web 组件 | 命令/Undo、版本只读、保存期间编辑/旧错误、草稿切换/Recovery、面板、i18n/主题/键盘 |
| 系统 E2E | 正式 API/UI、临时 Kubernetes、真实浏览器、强退/并发/权限/账本断言与清理 |

测试围绕可观察行为和真实不变量，不为可逆文案改动新增镜像实现的测试。阶段检查通过后，仅在新改动、失败或未解决问题出现时重复扩大测试。

## 2. 系统矩阵

| ID | 场景与关键断言 |
|---|---|
| P8-E2E-01 | UI 新建 Workflow/Draft，管理员先在模型列表设置租户模型，再打开 AI 图标输入需求；图结构/配置正确，业务执行数为 0 |
| P8-E2E-02 | AI 局部修改节点参数/Binding；无关节点/布局保留，一次 Undo/Redo 恢复完整变更 |
| P8-E2E-03 | AI 全图重构，覆盖分支、Code 静态配置、Exit 契约；静态问题定位准确，不运行代码 |
| P8-E2E-04 | UI 从 V1/V2 创建两个独立 Draft；修改、保存、刷新、资源状态、Recovery 和 AI 会话互不串数据 |
| P8-E2E-05 | 查看历史版本画布，通过键盘、拖拽、Inspector、Start/Exit、复制粘贴和 AI 入口均无法修改/保存该 Version |
| P8-E2E-06 | 从可运行 Version 派生 Draft revision 0；有不可变快照，用户不修改即可手动运行/创建新 Version；来源完整 |
| P8-E2E-07 | 两个 Draft 并发创建 Version；编号不冲突、来源不串、重复请求同一结果；部署仍固定原 Version |
| P8-E2E-08 | 保存发出后人工继续编辑；旧响应不清除新 dirty/Recovery；新一次保存持久化后续编辑 |
| P8-E2E-09 | AI 准备命令后人工编辑/Undo；旧序号/hash 命令拒绝，无覆盖，重新生成明确新命令 |
| P8-E2E-10 | 同 Draft 双窗口 revision 冲突；由用户 UI 决定保留/载入/覆盖，AI 无自动覆盖动作 |
| P8-E2E-11 | 命令应用后 ACK 丢失、SSE 重连/reload/Undo；同 ID 不重复应用，无法证明的结果不盲重放 |
| P8-E2E-12 | 切换 Draft/关闭面板/停止/归档时有进行中 Model Effect 或命令；迟到结果拒绝，已应用编辑保留 |
| P8-E2E-13 | AI 聊天、ask_user 答复、恶意工具/Provider/代码要求运行、测试、发布；业务 Execution/Attempt/Deployment 不增加或改变 |
| P8-E2E-14 | 用户 UI 手动运行，然后选择结果让 AI 分析修复；有真实 executionId/来源，AI 修复不自动再次运行 |
| P8-E2E-15 | 手动四种 Debug 模式与 Pin/Mock，创建后保存其他 Draft/同 Draft Head；实际 Snapshot 保持选定来源，Mock/重放被准确标记 |
| P8-E2E-16 | 不同 tenant/user/Workflow/Draft 的读取、编辑命令、ACK、会话、Version 派生和执行 Artifact 越权均拒绝；只读用户不能派生草稿 |
| P8-E2E-17 | Model/Credential/资源撤权与缺依赖；不泄露 Secret，不自动授权，失效配置显示字段问题；保存失败 revision 不增加 |
| P8-E2E-18 | 非法下游引用、动态端口、Loop 作用域、保护 Exit、全局输出变化；按同一手动规则处理，错误不靠静默删引用收敛 |
| P8-E2E-19 | threshold/overflow Compaction、多轮设计修改；保留静态-only/目标范围/命令/来源/证据，历史完整、压缩使用量不重复 |
| P8-E2E-20 | Host/Runtime 强退、Drain/多副本接管；Pending/Resume 和 Model Receipt 只结算一次，等待用户不占住执行槽 |
| P8-E2E-21 | 设计态模型账本、Token/非零成本、Quota、Artifact/Retention 与 Trace 来源；无伪 Workflow Execution，业务成本不混入 Builder |
| P8-E2E-22 | 静态 Resolver 的顶层 fetch、动态 import、宿主桥接、凭据/文件逃逸和业务请求反例；只允许无资源/无网络静态计算，资源 options 不调用通用 Provider |
| P8-E2E-23 | Control 缩容离线；已部署业务 Agent/Workflow 正常运行，Builder 明确不可用，Worker 无 Control 回读 |
| P8-E2E-24 | zh-CN/en-US、浅深主题、1440×900/最小桌面宽度、键盘焦点/错误定位、加载/空/失败状态和草稿/版本切换 |
| P8-E2E-25 | 150/300 节点、大文档/输出、并发设计态模型；使用现有性能基线与有界预算，检查 UI 卡顿、上下文大小和业务容量影响 |
| P8-E2E-26 | 归档/恢复与资源/包删除影响覆盖全部 Draft、Version/Execution 来源和未结算 Effect；无悬空引用、错误 GC |

默认编辑场景至少在“AI 已应用”“静态检查完成”“自动保存完成”三个时间点断言业务运行计数未增加。纯 Resolver 计算和 Builder 模型 Call 用设计态来源记录，不以它们冒充业务执行。

AI 模型/会话/事件接口不可用时，普通手动编辑、Undo/Redo、保存、版本与运行 UI 仍能独立使用；不能因为复用命令 Dispatcher 就要求用户先创建 AI 会话。

## 3. Fixture 与真实模型

可重复的 Provider Fixture 固定返回工具序列、过期命令、执行诱导、压缩和错误，验证权限/状态机；同时至少有真实模型的画布创建、局部修改、多轮压缩和非零成本验证。

Fixture 不能替代真实模型兼容、生产 CNI/Sandbox 隔离、容量或正式安全验收。未具备前提的门禁保持 planned/未关闭，并记录条件；不通过非条件 skip 伪造全绿。

AI 界面操作均通过真实图标、Composer、节点/端口/面板、保存/版本/发布/运行按钮。人工业务操作不由测试注入 Zustand/React State 或直接数据库写入代替。故障注入和最终证据断言可以使用 API/数据库。

## 4. 环境和执行入口

新增 Python 测试按 product/runtime/security/publishing 等业务域进入现有 tests/e2e，浏览器规格进入 tests/browser；不新建 plan8 专属部署器或编排入口。

~~~bash
uv run --frozen --group test pytest tests/e2e --values deploy/values/local.yaml -m product
uv run --frozen --group test pytest tests/e2e --values deploy/values/local.yaml
~~~

聚焦命令用于开发定位，完整命令用于最终系统回归。仅在新改动/失败/未解决风险时扩大或重复检查，不把同一通过结果反复运行当成额外完成证据。

2026-10-15 用户明确“压测暂时不做”，本次最终回归使用 `pytest tests/e2e -m "not capacity"`，容量认证明确记为暂缓，不能算通过。200 节点的保存和静态解析是功能回归，仍验证一次批次关闭、独立进程和零业务执行。

通过现有 agentxctl install/doctor/uninstall 管理临时环境，生成独立 Run ID 和 Control/Runtime/Dependencies Namespace；复用现有 Observability/Helm 与 Python Fixture。需要缓解资源时记录并缩容常驻开发服务，finally 恢复原副本。

## 5. 证据和完成状态

证据统一写 .local/artifacts/e2e/<run-id>，保存 JUnit、Playwright Trace/Screenshot/Video、命令 ACK/来源 hash、账本/运行计数断言、故障恢复和清理记录；文本/截图均脱敏凭据和客户数据。

成功与失败均证明 Namespace 删除、后台进程停止、开发副本恢复。只有显式 keep-on-failure 才保留现场，并记录后续清理。生产源文件 2000 行、边界/生成物/Helm、相关 Rust/前端回归必须通过。

验收报告按“静态校验”“Fixture 系统行为”“真实模型”“用户 UI 手动运行”“生产隔离/容量”分别列出证据，不用静态通过说明业务已运行。实施任务和 [追踪矩阵](99-traceability.md) 只按真实结果更新。
