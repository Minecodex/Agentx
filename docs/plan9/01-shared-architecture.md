# 共享架构、任务契约与设计门禁

状态：complete。本文设计已实现并通过本次桌面 Web 与临时集群验收；D9-01–09 已冻结，具体实现、限制和实际证据见 [最终报告](evidence/2026-11-02-acceptance.md)。当前系统边界与 docs/01–14 及代码保持一致。

## 1. 当前基线

| 已有能力 | 主要定位 | 本计划复用方式 |
| --- | --- | --- |
| 应用、部署、签名包和发布 | Control `application_catalog_api.rs`、`work_packages.rs` | 页面/MCP 所依附的应用与发布链 |
| Invocation、Session、查询、SSE | Runtime `gateway.rs`、`execution.rs` 等 | 正式业务任务，不增加执行引擎 |
| 输入等待、消息和恢复 | `input_waits.rs`、`workflow_interaction.rs`、docs/14 | 页面/MCP 补充信息 |
| 运行审批与通知投影 | Runtime `approval_tasks`、Control 治理 BFF | 复用现有审批中心 |
| Core Pending/Effect 与运行适配器 | `agentx-agent-core`、`worker_runtime_agent_*` | 原操作挂起和恢复 |
| 设计态 Host、工具和状态 | Control `studio_builder_host.rs`、`studio_builder_tools.rs`、`studio_builder_api.rs` | 工作流与网页创作共用基础设施 |
| 平台 AI 模型与设计态调用 | Control `platform_ai_model.rs`、Runtime `builder_model.rs` | 沿用同一个选中模型、Provider/Vault/Egress/Receipt |
| 应用详情、登录和统一 UI | Web `applications`、AuthProvider、`shared/ui` | 两个应用 Tab、可信页面外壳和样式基线 |
| MCP、Schema 与浏览器测试 | `rmcp`、`schemars/jsonschema`、现有 Playwright | 先验证已有依赖能力 |

现有设计态模型请求的目标具体绑定 Workflow/Draft，账本来源为 `studio_builder`；不能只换 Prompt 就把网页任务伪装成工作流任务。具体联合与授权扩展见 [共享设计态 Agent](06-shared-design-agent.md)。

## 2. 两条运行路径

```text
设计态：工作流 AI 面板 / 应用“访问界面”AI 面板
  → Control 共用设计态 Host + 同一个 agentx-agent-core
  → 固定 Profile：系统提示词 / 工具 / 目标上下文 / 权限
  → Runtime 设计态 Model Effect → Provider / Vault / Egress / Receipt
  → 更新所属域草稿，静态检查与隔离预览；用户 UI 测试、发布

业务态：/a/{slug} 的可信宿主 + 隔离 HTML / MCP / Application API
  → Gateway 校验调用主体、应用、发布绑定、输入和幂等键
  → Invocation，固定实际 Bundle、来源和接入修订
  → Coordinator / Worker / 业务 Agent Core
      ├─ input_waits：补充业务信息
      └─ approval_tasks：批准某次节点或工具操作
  → 原 Execution / Agent Operation 恢复 → 输出 / Artifact / 状态
```

网页创作不会创建 Workflow Execution 或占用业务 Worker；员工打开已发布页面也不会启动创作 Agent。网页调用时的业务模型仍来自工作流自身配置，与平台创作模型分开。

## 3. 共享任务契约

单任务以 Invocation 为权威，不新增 Task 状态机。`TaskView` 至少包含：

- `taskId=invocationId`、applicationId、实际 deploymentId/workflowVersionId/Bundle、创建时间和可信主体。
- 来源类型和接入修订：Web 的 uiVersion/publication，MCP 的接入修订；不适用的来源字段明确为空。
- 真实业务状态、可公开阶段、正式输出、错误、授权后的文件引用。
- 全部可见输入等待、审批摘要、当前用户允许的动作、状态修订和事件游标。

状态区分已受理、运行、等待输入、等待审批、完成、失败、取消和结果待核实；并行待办返回完整集合，主状态优先级在 D9-05 固定。Trace 不决定业务终态，不使用生成的假百分比。

创建与操作按租户、应用、真实主体、动作和 requestId 幂等；相同键不同输入返回冲突。断线不自动取消任务。输入等待不赋予审批权，审批不赋予资源 Grant，取消以 Runtime 权威状态为准。

可选 responseSchema 固化到等待节点/版本/包，在所有接收入口统一服务端校验；未声明时沿用明确的原始 JSON 输入。waitId、预期修订和稳定请求键处理重复、迟到、超时与取消竞争。人工输入与原输出分别保存；完成后重新生成创建有关联的新 Invocation。

## 4. 身份与访问范围

| 主体 | 身份来源 | 权限边界 |
| --- | --- | --- |
| 业务页面员工 | 现有平台登录与 IAM | 应用使用权、本人任务；无需应用管理或工作流编辑权 |
| 页面管理员/创作者 | 平台登录、应用对象范围和管理权限 | 生成、修改、预览与发布；各正式动作仍校验自己的权限 |
| 个人 MCP 用户 | 标准 OAuth 映射平台用户 | 客户端范围、应用授权和本人任务共同检查 |
| 服务调用者 | 已登记应用 API Key | 明确的服务主体与应用范围，不冒充员工 |
| 工作流运行身份 | 冻结 Workflow Service Identity | 资源、Credential、Sandbox 等独立 Grant |
| 审批人 | 平台登录、当前角色/部门和审批资格 | 只处理获准审批对象；生成页不能代为批准 |

`apiUserIdInput`、customerId、客户端名称、Origin 和输入 userId 都不构成认证。任务读取、SSE、补充信息、取消、会话、上传和下载执行相同主体/对象检查；服务跨 Key 范围在 D9-05 固定。

可信宿主保留认证状态，通过能力 Bridge 代隔离 HTML 调用绑定应用。HTML 不接收访问令牌或通用 API 客户端。业务页面登录与 MCP OAuth 分别实施，不能互相替代。

## 5. 数据所有权与发布

| 数据 | 权威域 | 处理方式 |
| --- | --- | --- |
| 页面草稿/版本、后缀、发布配置、MCP 配置、审批策略 | Control | 对象权限、不可变修订和签名发布；不是运行时回读来源 |
| 两类创作 Session/Run/Entry/Effect | Control 设计态 Host | 共用持久协议，以明确目标联合隔离；不混入 Application Session |
| 平台模型设置 | Control | 复用现有选中项与授权凭据，扩展到网页创作用途 |
| 创作模型 Receipt/Usage/文件 | Runtime 设计态账本 | `studio_builder` / `application_ui_builder` 来源隔离，沿用模型调用治理 |
| 已激活的页面发布记录、HTML 与资源副本 | Runtime | 从签名包准备、校验和激活，Gateway 只读本域副本 |
| OAuth 客户端授权和 IAM 映射 | 选定授权服务 + Control | 明确令牌权威、生命周期和撤销职责 |
| Invocation、等待、具体工具审批和结算 | Runtime | MySQL 权威，沿用 Outbox/Admission/Lease/Fencing |
| 治理投影 | Control | 消费事件/内部 API，不直查 Runtime DB |
| 文件/引用、Trace | 所属域 Artifact / Observability | 独立凭据、Hash/Retention/引用保护；Trace 不控制终态 |

页面发布固定 HTML Hash、SDK/样式版本、完整契约 Hash 和明确 Bundle。Runtime 完成所有依赖准备后，原子切换可用的页面/业务版本组合；不能先暴露地址再等待 HTML 或业务包复制完成。失败保留此前完整发布，不做字段转换或临时页面替代。

新提交携带预期发布修订，切换竞争时返回明确失配并重新加载，不能把旧表单输入静默交给新工作流。历史 Web 任务使用原 UI 版本，不能用历史 UI 创建新的旧版本任务。MCP 任务仅在存在匹配其实际 Bundle 的可用页面时提供 Web 链接，否则通过 MCP 查询；不强制先发布页面。

Web、个人 OAuth 与服务 Key 是独立入口。Bundle 和页面激活要求有效租户、路由、运行身份及实际资源/Grant；历史服务 Key 全部撤销不阻断 Web 发布、升级或回退，也不重新激活 Key。每次服务调用仍按其原 Key 身份检查撤销、有效期和范围。目录与管理员详情只展示 Runtime 已激活发布物的后缀，准备中的后缀保留不能提前替换正式访问地址。

正式调用省略或传入 null 的 `waitInputs` 使用 external 等待，与个人或服务主体无关。只有具备既有测试授权的平台用户可以显式传入预设计划；显式空对象仍表示 preset，不能在耗尽时转入 external。服务 Key 不能提交任何预设计划，包括空对象。

停用入口停止新调用，已有任务不隐式取消；历史查询仍按任务授权和保留规则处理。撤销用户应用使用权、停用应用和审批资格变化必须经过正式 Admission/权限链，不靠前端隐藏。

## 6. 架构变化与影响

1. 将现有设计态 Host 的 Workflow/Draft 专属目标扩展为工作流创作、应用页面创作的严格联合，共用 Core/恢复/模型/事件/预算代码；工具与领域写入留在各 Profile。
2. 平台 AI 模型授权用途增加应用页面创作，Runtime 设计态调用来源和签名能力同步扩展；不向业务工作流注入模型 Grant，不新增模型设置。
3. 增加独立业务页面路由、受限 SDK、HTML 隔离宿主和应用内配置，替换固定模板工作台；不增加侧栏菜单、通用批处理或网站服务。
4. 页面发布物和接入修订进入现有 Bundle/prepare/activate，Runtime 不回读 Control，Control 不直接执行不可信 HTML/脚本。
5. Gateway 新增 MCP 协议/资源服务器适配；审批增加 `agent_tool_effect` 对象和原 Operation 恢复，不复制业务执行逻辑。

原因是应用需要可快速交付的业务界面，同时保持创作与生产执行的所有权边界。计划入口同步到 docs/02 与 docs/05；功能实施时再更新 docs/01、04、06、09、10、13、14 的实际状态，不将目标写成已实现。

## 7. P9-00 必须关闭的设计门禁

| 编号 | 必须取得的结果 |
| --- | --- |
| D9-01 | 固定 MCP 协议/HTTP 传输、`rmcp` 与真实客户端矩阵及依赖变更 |
| D9-02 | 选定成熟 OAuth 实现，验证登录/IAM 映射、客户端范围与撤销 |
| D9-03 | 实测 Core Pending、审批、Checkpoint/Intent/Outbox 的提交与强退恢复 |
| D9-04 | 固定审批等待与活跃 deadline、累计预算、会话顺序、Workspace/STDIO TTL |
| D9-05 | 固定 TaskView、并行状态、本人/管理/服务范围、输入、文件、幂等和 Retention |
| D9-06 | 固定审批策略、当前资格、撤权与批准后业务前置条件复查 |
| D9-07 | 实测 HTML 隔离、不可放宽的 CSP、Bridge、文件与 SDK 行为；预览不能触发业务执行 |
| D9-08 | 实测两类创作共用 Core/Host，冻结目标联合、模型用途、来源账本、预算及已有画布回归 |
| D9-09 | 固定后缀唯一性、页面/Bundle 激活事务、缓存/权限撤销、历史 UI 与发布竞争处理 |

这些是有产物的实施任务，不要求逐项询问用户；只有确需改变已讨论产品范围时返回讨论。

### 当前已固定的实现决策

以下为已落入实现的边界，不等同于宣布上述门禁通过。单元、数据库、浏览器和真实模型证据分别记录在 [实施记录](08-implementation-evidence.md)；完整 64/128 和当前候选的恢复、权限与回归仍按实际结果验收。

| 门禁 | 固定结论 |
| --- | --- |
| D9-01 | `rmcp = 3.8.0`，应用服务显式使用 `2025-11-25` 与 Streamable HTTP；保留原调用方的协议回归。独立 AI 客户端为隔离配置的 OpenCode 1.1.56。 |
| D9-02 | 自托管 Ory Hydra v26.2.0，固定镜像摘要、独立数据库与 Secret。个人客户端预登记，使用 PKCE、现有账号登录、显式同意、opaque Token 和逐请求 introspection；服务调用保持应用 Key 主体。 |
| D9-03 | 工具提案、原 Attempt/Core 状态、具体 Effect 与审批关联持久化；决定经 Outbox 恢复原执行，已结算 Receipt 复用，未知写入不重放。 |
| D9-04 | 保存剩余活跃执行时间，审批等待释放业务槽；恢复仍使用原 Attempt 和累计预算。会话归属在等待期间保留；受保护工具 Agent 的 Workspace/STDIO 限制在校验和保存路径一致执行。 |
| D9-05 | `taskId = invocationId`，TaskView 为只读投影。未知写入优先于其余状态，其次为已知终态；非终态有输入等待时显示等待输入，再显示等待审批，队列显示已受理，其余显示运行。两类待办始终完整返回；输入、会话与文件使用同一主体授权。 |
| D9-06 | 审批绑定工具与策略版本、参数 Hash 和原 Effect，决定及执行前检查当前资格与授权。拒绝回到原 Agent，参数改变形成新提案。 |
| D9-07 | 两层 opaque iframe、不可放宽的 CSP 与私有 MessagePort；HTML 上限 1 MiB，上传上限 50 MiB，Bridge 校验方法、参数、并发与账号绑定。预览只使用合成 SDK，不执行正式业务。 |
| D9-08 | 两个 Profile 复用同一 Core、设计态 Host、模型设置、账本与并发池，分别固定七个和五个工具。运行中指导绑定原 Session/Run/编辑器并去重；只有设计态 Host 可以因新指导继续成功完成的 Core，业务 Worker 不具备此能力。 |
| D9-09 | 不可变 HTML、SDK、样式、完整交互契约和业务 Bundle 成对准备、原子激活；后缀唯一性和修订竞争由服务端处理。历史任务加载原 UI/Bundle，入口改址不保留旧址别名。 |


D9-01–09 已在本次范围内闭环，固定结论对应的实际协议、OAuth、Pending/Receipt、授权、隔离、共用 Host 和成对发布证据统一索引于 [完整验收报告](evidence/2026-11-02-acceptance.md) 的 38 项逐条结论；原始失败和复核仍分别保存，不把阶段完成视为常驻生产部署认证。
