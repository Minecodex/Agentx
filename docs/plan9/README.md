# plan9：AI 应用界面、应用 MCP 发布与 Agent 工具审批

状态：complete。2026-11-02 完成 P9-00–06：原始 64 份真实 AI 页面、128 次合格网页执行、20 类 MCP、8 类 OpenCode 自主调用和 38 项门禁均已验收。原失败与专项复测分别保留，详见 [最终报告](evidence/2026-11-02-acceptance.md) 和 [实施记录](08-implementation-evidence.md)。原验收临时环境与凭据已清理；随后按用户要求完成本地常驻部署升级，保留费用报销网页和 MCP 示例，见 [本地交付与使用说明](evidence/2026-11-02-local-demo.md)。不改变 plan7/planv2 尚未关闭的生产认证状态。

后续按用户要求完成 [MCP 文档入口与并存发布改造](evidence/2026-11-02-mcp-publications.md)：多条发布同时可用、管理员主动删除决定兼容范围。代码和临时环境验收已通过，与网页编辑功能由主对话协调统一升级常驻环境。

后续批量调用扩展另见 [plan10 计划与 Demo](../plan10/README.md)，当前已集成代码并进行系统验收，尚未完成全部门禁；不改变本计划已完成范围和历史验收结果。

## 1. 产品结论

已发布的工作流应用可以交付一个由 AI 生成、平台托管的业务网页，也可以发布为外部 MCP 工具。两种入口统一形成 Invocation，复用现有 Workflow Runtime；具体工具操作需要批准时进入现有审批中心。

网页生成与工作流生成使用同一个平台 AI 模型设置，并复用现有、参考 pi 行为实现的 Rust `agentx-agent-core` 和设计态 Host 基础设施。两种创作任务通过固定的系统提示词、工具 Registry、目标上下文和授权范围区分，不增加另一套 Agent loop、模型选择器或 Node/Pi Runtime 服务。

| 讨论编号 | 能力 | 交付结果 |
| --- | --- | --- |
| 2 | 应用发布为 MCP 工具 | 外部 AI 发现并调用获准应用，查询任务、补充信息和取消 |
| 3 | AI 应用界面 | AI 根据应用输入/输出、交互契约和管理员需求生成 HTML，管理员发布到 `/a/{slug}`，员工登录后直接使用 |
| 4 | Agent 具体工具调用审批 | 在某次调用前展示真实工具与参数，批准或拒绝后继续原 Agent |

贯穿场景为“客户报价助手”：销售访问报价页面 → 填写客户与产品 → 查询和计算 → 主管在审批中心批准 CRM 写入 → 生成报价单。MCP 客户端能运行同一应用，并查询相同任务和结果。

## 2. 入口与使用流程

- 应用详情增加“访问界面”和“MCP 接入”两个 Tab；前者使用左侧页面、右侧持续聊天，顶部提供发布和发布历史，后者集中工具、认证和客户端接入。
- 应用目录提供“打开应用”和“复制访问地址”；有直接链接的员工不必进入管理后台。
- `/a/{slug}` 使用独立业务页面外壳，复用平台登录与应用使用权限，不挂载管理侧栏，也不要求应用管理权限。
- 现有审批中心扩展工具操作类型；不新增业务工作台、全局任务、批量任务或 MCP 中心菜单。
- 首版每个应用选择一个正式 Web 发布环境、一个稳定后缀；内部保留草稿与不可变页面版本，不建设通用网站搭建平台。

工作流应用发布后，进入“访问界面”自动准备唯一工作页面，管理员直接在右侧聊天中描述需求并持续修改，系统自动保存。首次发布弹框配置地址，后续发布沿用已有 URL。发布历史可独立预览和回滚，回滚只切换线上页面，当前工作 HTML 与本人的聊天始终保留；不提供恢复为新草稿或手改 HTML 的入口。生成和修改页面不自动执行生产工作流、不自动发布。交互与实测见[持续编辑收口](evidence/2026-11-02-continuous-page-editor.md)。

## 3. 文档目录

| 文档 | 内容 |
| --- | --- |
| [01-shared-architecture.md](01-shared-architecture.md) | 当前基线、共享任务、身份、发布所有权、架构变化和设计门禁 |
| [02-application-mcp.md](02-application-mcp.md) | 应用内 MCP Tab、工具契约、OAuth、长任务、幂等和文件 |
| [03-application-web-ui.md](03-application-web-ui.md) | AI 页面初始化、HTML/SDK、登录、隔离、后缀、版本与发布 |
| [04-agent-tool-approval.md](04-agent-tool-approval.md) | 现有审批中心扩展、具体调用绑定、Pending/Resume 与恢复 |
| [05-e2e-acceptance.md](05-e2e-acceptance.md) | 页面生成、宿主隔离、MCP、审批及全链路验收矩阵 |
| [06-shared-design-agent.md](06-shared-design-agent.md) | 统一模型与 pi 行为底座、两种创作 Profile、工具和持久恢复 |
| [07-execution-and-live-acceptance.md](07-execution-and-live-acceptance.md) | 已批准执行范围、SDK/OAuth 决策、30 美元预算、64 份真实页面与外部 AI 验收 |
| [08-implementation-evidence.md](08-implementation-evidence.md) | 历史实施状态、原始失败、复核与最终闭环 |
| [evidence/2026-11-02-acceptance.md](evidence/2026-11-02-acceptance.md) | 完整交付、64/128 真实效果、首次质量、费用、原始失败与复测及 38 项证据 |
| [evidence/2026-11-02-local-demo.md](evidence/2026-11-02-local-demo.md) | 本地常驻升级、保留数据、费用报销网页体验与 MCP 客户端接入 |
| [evidence/2026-11-02-mcp-publications.md](evidence/2026-11-02-mcp-publications.md) | MCP 文档入口、并存发布、精确删除与真实 OAuth/外部客户端验收 |
| [evidence/2026-11-02-combined-upgrade.md](evidence/2026-11-02-combined-upgrade.md) | 持续页面编辑与 MCP 的常驻合并升级、239 张表保留核对及实际业务复测 |
| [implementation-plan.md](implementation-plan.md) | P9-00–06 任务、依赖、代码落点与退出门禁 |

## 4. 已有基础与新增范围

复用 Application/Deployment、Invocation/Session、正式调用/SSE/Artifact、输入等待、运行审批、Checkpoint/Fork、Core Pending Effect，以及 plan8 的设计态 Host、平台 AI 模型和模型调用账本。已有 `rmcp` 依赖不等于已完成应用 MCP 发布或 OAuth。

新增三条产品能力及其共用的任务授权、页面托管、版本和恢复契约。`taskId` 就是 Invocation ID，不增加权威 Task 状态机。HTML 是包含 CSS/JavaScript 的单文件静态前端；平台注入统一样式与受限 SDK，不为每个应用创建前端仓库、依赖安装或独立后端。

本次范围调整直接替换原固定表单/对话/批量模板工作台方案。首版不建设通用批次对象、派发引擎或 CSV 工作台。应用本身若声明数组输入，页面可按其契约提交一次 Invocation；这不等于平台具备逐项业务批处理能力。

## 5. 实施顺序

```text
P9-00 事实审计、契约冻结与关键 Spike
  → P9-01 共享任务查询、授权与交互
  → P9-02 共用设计态 Agent 与 HTML 生成
  → P9-03 业务页面宿主、SDK、登录与发布
  → P9-04 Agent 工具调用审批
  → P9-05 应用 MCP 发布与身份接入
  → P9-06 全链验收、删除收口与文档更新
```

P9-00 必须验证共用 Host/模型来源、页面隔离与发布一致性，同时完成 MCP/OAuth 和工具挂起的既有门禁。不能以文档推演、静态截图或 Echo Server 代替实际结果。

## 6. 范围约束与完成定义

1. 正式体验和浏览器验收覆盖桌面 Web，使用现有设计令牌、统一基础控件、中英文与浅深主题。
2. 生成页只获得绑定应用的有限 SDK；平台处理登录、任务、文件和授权，HTML 不持有 Token/APIKey，不获得管理或审批权限。
3. 页面与工作流契约、Bundle、SDK 版本绑定；发布成对生效，历史任务不按当前页面或最新工作流重算。
4. 工作流与网页创作共用 Core、模型设置及设计态运行基础设施；消息、目标、工具能力和费用来源明确隔离。业务 Agent 的运行模型保持自身配置。
5. MCP 只发布明确开放的应用；工具审批首版覆盖 Agent MCP Tool，批准/拒绝不等于资源授权，变参必须产生新提案。
6. 通过相关契约、前端、边界和临时 Kubernetes/Playwright E2E；使用真实模型生成可运行网页，真实客户端调用正式应用，并验证恢复、权限与文件。
7. 结构变化修改初始化基线，删除过时方案，不建立兼容层、迁移或 fallback；清理环境并恢复开发副本。

## 7. 参考

- 当前事实：[系统架构](../02-system-architecture.md)、[平台业务与模型](../05-platform-business.md)、[工作流交互](../14-workflow-interactions.md)、[plan8 设计态 Host](../plan8/02-architecture-and-contracts.md)、[plan3 原生 Core](../plan3/01-target-architecture.md)。
- 交互参考：[Dify MCP 发布](https://docs.dify.ai/en/self-host/use-dify/publish/publish-mcp)、[Dify Workflow Web Apps](https://docs.dify.ai/en/self-host/use-dify/publish/webapp/workflow-webapp)、[n8n 工具人审](https://docs.n8n.io/advanced-ai/human-in-the-loop-tools/)。参考支持交互选择，不代表 Agentx 已达到同等完成度。
- 浏览器宿主依据：[iframe sandbox](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/iframe)、[MessageChannel](https://developer.mozilla.org/en-US/docs/Web/API/MessageChannel)。具体隔离和客户端组合由 P9-00 实测固定。
