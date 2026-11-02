# plan9 实施计划

状态：complete。P9-00–06 已按对应证据关闭；64 份原始 AI 页面、128 次合格网页执行、20 类 MCP、8 类 OpenCode 及 38 项门禁完成。详见 [最终报告](evidence/2026-11-02-acceptance.md) 和 [实施记录](08-implementation-evidence.md)。原始失败与专项复测分别保留，常驻部署未升级。阶段编号以本版为准，原固定工作台/通用批量方案已替换。真实验收执行约定见 [执行与真实模型验收](07-execution-and-live-acceptance.md)。

## 1. P9-00：事实审计、契约冻结与关键验证

依赖：可读取现有应用、设计态 Host、Agent/审批/发布和系统测试基线。先关闭 [共享设计门禁](01-shared-architecture.md#7-p9-00-必须关闭的设计门禁)，再实施依赖其结论的任务。

- [x] P9-00-01 审计 Application/Invocation、可信主体、Artifact、现有登录/路由、应用 Tab、运行审批与发布；核对 rmcp、Schema、DOM/浏览器及 Sandbox 现有依赖。
- [x] P9-00-02 审计并实测两种创作共用 Core/Host：目标联合、Prompt/工具 Registry、会话/命令/ACK、模型来源、Quota/Receipt 与撤销；产出 D9-08。
- [x] P9-00-03 冻结 TaskView、状态/并行待办、主体/操作范围、文件/会话、稳定请求键与输入等待 responseSchema，产出 D9-05。
- [x] P9-00-04 完成具体 Tool Effect Pending/Resume Spike，实测 Intent/Checkpoint/审批/Outbox 提交、唯一决定、强退、拒绝和 Receipt；产出 D9-03/D9-04/D9-06。
- [x] P9-00-05 完成 OAuth 成熟实现选型、现有登录/IAM/撤销联调，固定 MCP 版本/SDK/真实客户端与网络/文件矩阵；产出 D9-01/D9-02。
- [x] P9-00-06 完成 HTML 隔离与 SDK Spike，实测 iframe/CSP/导航/Bridge/文件、预览禁写、DOM/JS 检查执行器及资源限制；产出 D9-07。
- [x] P9-00-07 固定后缀、页面/Bundle/SDK 契约、Runtime 副本/激活/缓存、并发切换和历史 UI 规则；产出 D9-09。
- [x] P9-00-08 固定 Control/Runtime 初始化 DDL、目标/来源联合、API/内部事件/签名能力、Retention 与引用；记录 ADR、依赖缺口和当前文档更新范围。

落点：现有 Control/Runtime 模块、`agentx-agent-core`、`agentx-runtime-contracts`、`agentx-api-types`、`contracts`、`deploy/migrations`。

退出门禁：D9-01–09 有具体结论和实际 Spike 证据；不以空泛“共用 Agent”跳过现有 Workflow/Draft 绑定重构，也不假定 JWT 等于 OAuth。HTML/审批恢复未证明的边界保持门禁未通过。

## 2. P9-01：共享任务、授权与业务交互

依赖：P9-00。

- [x] P9-01-01 以 Invocation 生成 TaskView，统一真实阶段、完整待办、输出、错误/OutcomeUnknown、来源和发布快照，不增加权威 Task 状态机。
- [x] P9-01-02 统一正式 API、BFF、SSE、本人记录、会话、取消/等待及 Artifact 的用户/应用/服务归属检查；员工使用与管理权限分开。
- [x] P9-01-03 实现稳定请求键、Hash/冲突、重复受理与发布修订；部署升级不改变原幂等任务。
- [x] P9-01-04 将 responseSchema 固化到等待节点/版本/包，所有入口统一校验；保留原始 JSON、原输出及人工输入，处理重复/迟到/取消竞争。
- [x] P9-01-05 完成状态快照/游标、能力摘要、原任务与新生成来源、文件/历史引用保护，为 Web SDK/MCP 提供同一服务。

落点：Runtime Gateway/查询/等待/会话/Artifact 和权限模块，Control 运行 BFF，共享 Rust/TS 契约及初始化 DDL。

退出门禁：E9-W03–W06、M03–M05 的服务端部分通过；断线可查、授权反例和竞争有确定结果，Trace 不决定终态。

## 3. P9-02：共用设计态 Agent 与 HTML 生成

依赖：P9-00、P9-01；具体设计见 [共用设计态 Agent](06-shared-design-agent.md)。

- [x] P9-02-01 从 studio_builder 提取真实共用的 Host/状态/模型/事件/预算代码，实现固定 workflow_builder/application_ui_builder Profile 和严格目标联合，保留画布领域命令。
- [x] P9-02-02 复用同一平台 AI 模型设置，扩展网页用途/签名授权与 Runtime 来源，冻结 Run 模型/凭据/参数/价格；同步来源 CHECK、Quota、Receipt、撤销、Artifact/Retention。
- [x] P9-02-03 实现 application_contract/page_read/page_edit/page_check/ask_user 的严格工具 Schema、目标注入和 Dispatcher 权限，不授予生产执行、发布、审批、通用请求或 shell。
- [x] P9-02-04 实现 HTML 草稿、精确修订/Hash、原子编辑、差异/ACK 和同一保存路径；复用持久 Operation、Lease/Fencing、SSE、停止、ask_user 与压缩。
- [x] P9-02-05 提供 SDK/样式/契约上下文，生成无额外需求也可用的页面；实现静态与隔离模拟检查、错误定位/反馈和有界修复，检查不能调用业务。
- [x] P9-02-06 在应用“访问界面”接入共用对话/进度/成本组件及 HTML/预览区，明确已应用/已保存/模拟/真实测试状态，不另建模型选择器。
- [x] P9-02-07 完成 E9-G01–G08 与真实模型生成/修改证据，回归现有工作流 AI 七工具、手动编辑、模型设置及评测。

落点：Control 共用设计态模块和两个领域 Profile、Runtime `builder_model.rs` 及来源治理、已有 Core Port、Web 应用 Tab/共享创作组件、共享协议和初始化 DDL。

退出门禁：两类创作由同一 Core/Host 驱动且授权隔离，真实模型生成 HTML 能在模拟 SDK 中完成交互；崩溃/竞争不重复编辑或计费；设计过程没有业务执行。此阶段不宣称页面已正式发布可用。

## 4. P9-03：页面宿主、SDK、登录与发布

依赖：P9-01、P9-02；具体设计见 [AI 应用界面](03-application-web-ui.md)。

- [x] P9-03-01 实现页面版本/Publication、HTML/SDK/样式/契约/Bundle 快照与 Artifact 引用，接入现有签名包复制、prepare/activate 和 Runtime 本域读取。
- [x] P9-03-02 实现后缀规范化/唯一索引/409、一个正式环境与地址配置、停用/改址和权限；不留旧后缀别名。
- [x] P9-03-03 新增 `/a/{slug}` 独立可信外壳，复用 AuthProvider/登录及安全 returnUrl；仅应用使用权限即可进入，处理续期/退出/换号。
- [x] P9-03-04 实现 iframe/CSP、受控资源与导航、绑定 Window/账号/应用/版本的 MessageChannel、严格消息校验/限额与失效回收；不传 Token/APIKey。
- [x] P9-03-05 实现受限 SDK 的提交、查询/订阅、本人历史、等待/取消、文件及声明的会话能力，复用 P9-01；预览禁写，实际测试由可信 UI 发起。
- [x] P9-03-06 完成发布门禁、物料 Hash、成对原子激活、修订竞争、历史页面/只操作原任务、停用/撤权和 Retention；不静默套用最新工作流。
- [x] P9-03-07 完成应用目录打开/复制、应用内发布/版本管理、中英文/主题/统一基础控件和桌面状态；不新增全局菜单或通用批次后端。
- [x] P9-03-08 完成 E9-W01–W12，使用 P9-02 实际生成 HTML 走正式调用；验证 Control 不可用、发布强退、隔离绕行及历史恢复。

落点：Web 路由、应用管理 Tab、独立宿主/SDK/基础样式适配，Control 页面领域/发布模块，Runtime 发布物与 Admission，Artifact 和初始化 DDL。

退出门禁：员工打开后缀登录后可完成真实输入/状态/等待/结果/文件流程；无需管理/Trace 权限。页面和业务版本一致，隔离与跨主体权限通过，预览没有生产副作用。

## 5. P9-04：Agent 具体工具调用审批

依赖：P9-01、P9-03 和 P9-00 的业务 Pending 实测。

- [x] P9-04-01 实现工具审批策略/不可变版本/包绑定，扩展审批对象联合与 Effect 唯一引用，保持资源授权、节点审批和输入等待独立。
- [x] P9-04-02 在 Runtime Tool Adapter 持久化原 Intent/参数 Hash/Pending/Checkpoint/Outbox/候选人，释放槽并保留原 Agent Operation。
- [x] P9-04-03 实现批准/拒绝唯一决定、当前审批资格/Grant/策略/取消/前置条件复核和正式恢复；变参产生新提案。
- [x] P9-04-04 实现拒绝/超时回传、同轮多工具身份、重复提案预算、会话/等待和 Workspace/STDIO 到期规则。
- [x] P9-04-05 扩展现有审批中心可信卡片，展示真实参数/材料和脱敏差异；页面仅显示摘要及受控跳转，SDK 无审批方法。
- [x] P9-04-06 完成 E9-A01–A08，覆盖竞争/撤权/强退/换 Worker、Receipt 一次消费、OutcomeUnknown 与等价写路径限制。

落点：Control 工具策略/包与审批投影，Runtime 审批/命令/业务 Agent Adapter，必要的通用 Core 契约，Web `approvals` 与待办摘要，初始化 DDL。

退出门禁：批准前零写入，批准后只执行原 Effect，拒绝后继续原 Agent；结果未知不盲重试。生成页、创作 Agent 和 MCP 均不能越过正式审批。

## 6. P9-05：应用 MCP 发布与身份接入

依赖：P9-01、P9-04，OAuth/SDK/客户端门禁已关闭；不要求应用必须发布 Web 页面。

- [x] P9-05-01 在应用“MCP 接入”Tab 实现配置/修订/发布/停用、部署环境绑定、稳定工具名与正式 Schema 生成，不新增菜单。
- [x] P9-05-02 使用已有或经验证升级的 rmcp 增加 Gateway 适配，提供业务提交、get_task、submit_task_input、cancel_task，调用已有共享服务。
- [x] P9-05-03 完成成熟 OAuth 授权服务/资源服务器、平台用户映射、客户端范围/撤销、服务 Key 与 Runtime Admission，补齐部署和安全诊断。
- [x] P9-05-04 实现稳定任务结果、长任务/断线/幂等/参数冲突和接入修订，发布切换不重投旧任务，不依赖未经验证的 MCP Tasks。
- [x] P9-05-05 完成 Artifact 引用/授权结果、可空任务链接、匹配页面与身份校验、配置示例/TLS/网络/文件诊断，不开放内部 Trace 或凭据。
- [x] P9-05-06 完成 E9-M01–M06 和实际客户端联调，记录协议/认证/网络/文件能力；Web 与 MCP 可分别发布/停用。

落点：Control 应用 MCP/客户端授权与发布，Runtime Gateway MCP/认证模块，既有部署/诊断，Web 应用内接入 Tab。

退出门禁：真实客户端发现并运行正式应用，经已有审批中心处理后查询原任务；无页面时亦可完成调用，服务/个人主体与撤销边界通过。

## 7. P9-06：全链验收与文档收口

依赖：P9-01–05。

- [x] P9-06-01 完成 E9-X01–X04：真实 AI 页面、后缀登录、Web/MCP、补充输入、主管审批、CRM 写入和文件交付，核对完整版本/主体链。
- [x] P9-06-02 使用真实模型、授权服务和实际客户端补齐证据，保留 Fixture 故障/写入次数断言及最终报价单内容核验；不将 Mock 标为真实。
- [x] P9-06-03 执行适用 Rust/前端/Schema/OpenAPI/TS、Helm/边界与 `cargo xtask check`，通过 `pytest tests/e2e` 编排系统回归，包含已有工作流 Builder/评测。
- [x] P9-06-04 核对 38 项验收逐项结果和证据，保存生成 HTML/Prompt/Profile/SDK/契约/Bundle 身份及失败复测；未执行/失败保持未完成。
- [x] P9-06-05 删除被替换的重复实现/旧来源协议及无用代码，更新 docs/01、02、04、05、06、09、10、13、14 和 plan9 实际状态；不建立兼容层。
- [x] P9-06-06 核对成功/失败后的 Namespace、进程、测试数据/Secret/Artifact 清理和开发副本恢复，区分原计划未关闭的生产认证。

退出门禁：产品全链、权限、发布一致性、持久恢复、真实生成/客户端和环境清理均有证据；功能任务不可凭文档或 Demo 勾选。

## 8. 全局实施约束

- 修改前检查工作树并保留无关内容；架构变化先记录原因/影响，同步设计文档。
- 前后端生产文件不超过 2000 行；保持模块化、统一 UI，先核对依赖，不复制成熟库或增加通用配置层。
- 结构变化改初始化 DDL，不写历史迁移、双协议、旧路由包装或 fallback。
- 自动化复用 uv/Python、Rust agentxctl/cargo xtask、现有 pytest/Playwright，不另建部署/测试入口。
- 证据在 `.local/artifacts`、发布包在 `.local/dist`、临时镜像在 `.local/tmp/images`；不提交真实客户数据/凭据。
- 文档检查仅证明计划一致，不代表功能已实现；阶段完成按实际退出门禁判定。
