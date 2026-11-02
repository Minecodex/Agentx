# 工作流与网页创作共用的设计态 Agent

状态：complete。共用 Core/Host、模型冻结、两种 Profile、运行中指导、故障恢复及受影响画布回归均通过，完整证据和知识检索模型流专项复测见 [最终报告](evidence/2026-11-02-acceptance.md)。网页生成使用现有工作流生成选中的平台模型，复用同一 pi 行为底座，通过提示词、工具和目标上下文区分任务。

## 1. 共用范围与现有事实

这里的“pi Agent 底座”指 [plan3](../plan3/01-target-architecture.md) 已采用的 Rust `agentx-agent-core`，不是新增 TypeScript pi SDK 运行时。plan8 的 Studio Builder 已在 Control 内驱动同一个 Core，模型调用经 Runtime 设计态 API 完成。

| 层 | 共用内容 | 差异所在 |
| --- | --- | --- |
| 模型设置 | `platform_ai_model_alias_id`、授权凭据及统一可用性 | 不提供网页单独模型、应用覆盖或备用模型 |
| Agent Core | 消息、Turn/Tool loop、Context、Compaction、Pending/Resume、steering/follow-up | Host 提供两种明确创作 Profile |
| 设计态 Host | Run 驱动、Lease/Fencing、会话/Entry/Effect 持久化、SSE、停止、预算、Usage | 目标授权、工具 Dispatcher、上下文投影与领域命令 |
| 模型 Adapter | Provider-neutral 请求、Runtime Provider/Vault/Egress、Receipt、取消与对账 | 来源目标及精确签名授权 |
| 创作 UI 基础组件 | 对话、工具进度、Token/成本、错误、问题答复、停止和差异 | 画布命令或 HTML 草稿/预览 |

只提取这两种创作确实共用的代码，不建设用户可配置 Agent 工厂或 Profile 管理菜单。Profile 是服务端固定类型，不能由用户/模型通过 Prompt 切换。业务 Worker 继续使用同一 Core 的既有业务 Adapter，不把业务运行迁入 Control。

## 2. 两个固定 Profile

| 项目 | `workflow_builder` | `application_ui_builder` |
| --- | --- | --- |
| 目标 | 当前打开的 Workflow/Draft/Editor Binding | 当前应用、页面草稿、明确部署/Bundle 和 Editor Binding |
| 系统提示词 | 读取权威节点/资源，编辑当前画布，静态校验 | 读取应用契约/SDK，生成并修改单文件 HTML，检查可用交互 |
| 核心上下文 | Definition/Editor、局部图、资源状态、修订和用户目标 | HTML/修订、输入输出/交互契约、SDK/样式、用户需求和检查 |
| 工具 | 沿用 plan8 的七工具 | 下表五工具 |
| 领域写入 | 同一画布 Dispatcher 与保存协议 | 同一页面草稿 Dispatcher 与保存协议 |
| 运行/发布 | 用户 UI 发起 | 用户 UI 发起；AI 仅可使用模拟预览 |

两类任务不互相取得编辑工具。网页 Agent 可以读取为页面服务的公开工作流契约和阶段说明，不能修改工作流、Grant、部署或模型设置。工作流 Builder 不因共用底座而取得 HTML 或应用发布权限。

## 3. 网页 Agent 工具

| 工具 | action / 输入 | 输出和边界 |
| --- | --- | --- |
| `application_contract` | summary、input、output、interactions、sample；字段范围/分页 | 绑定发布的权威契约/Hash、阶段、文件与会话说明；sample 仅限合成数据或用户明确提供的授权脱敏样例 |
| `page_read` | source、summary、sdk、styles、diagnostics；范围/分页 | 当前 HTML/修订、真实 SDK 类型、设计令牌和精确检查；不读取任意文件/应用 |
| `page_edit` | replace 或基于精确源码的 edits；baseRevision、contentHash | 原子编辑命令、差异、影响和稳定 commandId；经当前编辑器 ACK/保存，不另建 AI 保存链 |
| `page_check` | static、preview；精确 HTML/契约/SDK Hash | 静态错误、字段/行定位；preview 在隔离浏览器中用合成 SDK 检查 DOM/事件/状态，不调用正式应用 |
| `ask_user` | 业务展示问题及可选答案 | 持久问题与经过认证的回答，只补充需求，不扩大权限 |

工具采用严格判别 Schema；未知字段、任意 URL、租户/用户/其他应用目标均拒绝。工具返回的修订、错误路径、截断/分页和报告 Hash 必须进入模型可见 content，不只放在 UI 的 structuredResult 中。

编辑使用精确修订/Hash、原子整页替换或确定性文本改动，不引入自制 AST 页面 DSL。编辑失配时重新读取，不能模糊替换当前用户代码。大内容分页或 Artifact 化。

`page_check.preview` 的可信执行器复用现有隔离能力，通过窄输入/输出协议运行页面；Control 只编排和保存结果，不在控制面进程执行生成脚本。D9-07 先核对已有 Sandbox/浏览器依赖，确有缺口才引入维护中的实现，不开放任意 shell 或新常驻服务。

`page_check` 的每次检查都重新建立隔离场景，清除先前模拟任务并更换端口绑定；同一编辑器的检查串行执行。页面可以在提交后展示任务视图，不要求原表单持续留在 DOM。原 HTML、Hash 和校验约束不变，重复检查不能继承前一次交互状态而产生误报。

工具 Registry、Dispatcher、模型短期能力和正式领域 API 均拒绝生产调用、审批、授权及自动发布。网页 Agent 的 workspaceSandboxBinding 为空，不自动注册 read/write/edit/bash，不提供通用 HTTP、业务 MCP 或包安装工具。预览执行器的内部隔离资源不成为 Agent 可操作 Workspace。

## 4. 模型配置与调用来源

继续使用 `/api/v1/platform-ai/model-setting` 的同一个设置，不增加“网页模型”字段。实施时模型列表说明扩为“平台 AI 模型（工作流、网页与评测）”；选择/授权沿用 model:manage、company:manage、resource:grant，创作者只读名称与可用状态。

每个创作 Run 冻结设置版本、Alias、实际 Deployment、Credential、参数、价格、Prompt/工具/SDK 版本。普通切换只影响新 Run；撤销、停用或凭据失效阻止后续调用。配置缺失时明确提示设置入口，不使用业务工作流模型、其他用户密钥或备用模型。

现有 `DesignModelEffectRequest` 的 `StudioBuilderTarget` 是工作流专用，必须改为严格目标联合：

| 模型调用来源 | 必需目标身份 | 不允许冒用的身份 |
| --- | --- | --- |
| `studio_builder` | tenant、actor、workflow、draft、session、run、operation/effect | application UI 草稿、业务 Execution |
| `application_ui_builder` | tenant、actor、application、uiDraft、绑定 Bundle/契约、session、run、operation/effect | 假 workflow/draft 或业务 Execution |

共用模型 Effect API/Receipt，但签名声明、幂等域、SQL CHECK/索引、Quota、Usage、Trace、Artifact 授权/Retention 和取消都识别实际来源。Target 不能同时具有两种身份或两种都缺失，不能用虚构 workflowId 满足旧结构，也不能仅添加一个未经核验的字符串标签。

Runtime 只消费冻结绑定和短期能力，并按既有持久 Admission/Policy Epoch 接收撤销，不回读 Control。Credential 只由 Runtime Vault Adapter 解析。agent_turn 和 compaction 各有稳定 Effect/Receipt；创作成本单独显示，不计入员工某次业务任务。

## 5. 会话、命令与故障恢复

目标联合同时进入 Session、Run、Entry、待结算 Effect 和模型能力。Session Key 包含 tenant、Profile、目标草稿、actor、session，Editor Binding 另含实例标识；同一应用的不同用户/草稿不自动共享聊天，工作流与网页消息不能混入同一 Session。

复用 plan8 的持久 Operation、一个 Session 一个开放 Run、Lease/Fencing、单调事件 Cursor、ACK 与幂等 Settlement。将原本硬编码 Workflow/Draft 的授权和持久化字段按明确联合重构；共享部分只保留一套，不复制同语义的 Agent loop、压缩或恢复实现。表/类型破坏性替换初始化基线，不保留双版本存储或旧模型协议。

运行中追加指导使用同一 `/api/v1/design-agent/sessions/{session}/runs/{run}/inputs`，明确携带 messageId、editorInstanceId、steering/follow_up 和文本；工作流与网页输入框都只向自己的原 Run 投递，不新建 Run。每条最多 2048 字符，每 Run 最多 32 条，原消息身份重试须保持文本及类型一致。既有租户、创作者、目标草稿、Editor Binding、权限与 Run 状态检查继续生效，不能靠消息指定其他目标、模型、权限或工具。

消息复用不可变 Entry 持久化；共用 StatePort 将原 Run 的未见消息投影到 Core 已有队列，按 messageId 与完整 Core 历史/队列去重，沿用原 CAS 与 Fencing 提交。压缩只改变模型可见投影，完整历史仍作为去重依据。已派发模型或工具 Effect 的原请求不变，指导在其安全结算点处理；新 Run 不继承旧 Run 尚未消费的队列。完成与消息接收共用 Run 锁，先接收的指导必须处理后才能完成；取消、失败和未知结果不被新消息复活。UI 显示“追加指导”，消息仍属于创作数据，不进入业务任务或生成 HTML 的能力集合。

Core 输入明确携带必填 `continueCompleted`，由 Host 决定成功完成后的同一 Run 是否仍有已接收输入待处理。两个设计态 Profile 固定开启，并受上述原 Run 状态与消息归属门禁约束；业务 Runtime 固定关闭，旧任务完成后的重试仅结算原结果，不能消费属于下一 Workflow Execution 的排队消息。该字段不来自浏览器、提示词或工作流参数，也不放宽取消、失败、未知结果或预算终止。这是 Host 的输入接收权，不是模型可选配置。

业务 StatePort 的 `retry` 输入严格绑定当前 Attempt，其他排队 Execution 的原提示词由既有 Wakeup 唤醒后再消费；运行中的 Core 也不能提前吞掉它。队列只从持久待处理表加载，删除 Register 队列副本的旧回退路径。真实数据库回归同时验证两个 Attempt 的投影、原行保留，以及已消费消息不被旧副本复活。

画布 JSON 回执与 documentHash 使用已有 JCS 规范化语义比较，浏览器往返中的 `900.0` / `900` 等价数字表示不能导致误拒绝。自动布局仍只允许改变几何位置和尺寸，节点、参数、连线、顺序、折叠状态与其余编辑内容受原检查约束；数值变化和字符串/数字类型变化仍拒绝。HTML 编辑继续绑定精确源码及其 Hash。画布 Profile 明确 Agent v2 的业务答复来自 `text`、Code 的返回对象来自 `structuredOutput`，Python 命名输入必须与 `main` 的关键字参数精确匹配，跨节点引用使用明确的 outputs/sourceNodeId/port/item 选择。

画布的 Applied 回执确认已经持久记录的本地编辑事务，远端保存仍单列状态。编辑工具和完整校验在提交命令前，复用 Save 的只读资源准备逻辑，检查同一资源版本、可用性和工具审批策略；不新增保存工具，不增加业务执行或授权能力。受审批保护的工具只能挂载在无 `workspace_sandbox` 的 Agent，文件处理使用独立 Code/文件步骤。策略冲突返回实际错误码和 `nodes[i].resourceReferences` 字段，不让一个注定被 Save 拒绝的图先被标为可应用。重新载入后的创作修复通过既有恢复对话框恢复原本地副本，不丢弃原编辑来绕过错误。

```text
读取当前页面和绑定契约
  → 生成 page_edit 命令并持久化 Intent
  → 统一编辑器应用 / ACK，沿用草稿保存
  → 对精确新 Hash 进行静态/模拟检查
  → 有进展且可修复则继续；缺需求则 ask_user
  → 汇报编辑、保存、检查与待用户测试/发布状态
```

| 情况 | 处理 |
| --- | --- |
| 用户同时编辑、切换目标或绑定部署 | 旧命令 stale；更新目标事实并重新读取，不能跨草稿投递 |
| 已应用编辑、ACK 丢失 | 按原 commandId 对账；不能证明时显示未知/过期，不盲重放 |
| 保存失败 | 已应用与未保存分别显示，继续同一保存/冲突协议 |
| 模型结算后 Host 强退 | 查询同一 effectId Receipt，Entry/Usage 只结算一次 |
| 模型收据查询/提交响应短时不可用 | 原 Effect 保持 Pending，先重查收据；只有确认未开始才提交同一请求。明确拒绝停止，权威未知结果不重放 |
| 用户停止、离开、权限撤销 | 停止领取，取消模型/检查与待命令，保留已应用草稿 |
| Lease/Fencing 失效 | 拒绝迟到命令/结算，新 Owner 从持久 Operation 对账 |
| 输出截断或检查失败 | 保留明确失败及源码状态，不把不完整 HTML 发布为成功 |

等待用户/浏览器 ACK 不占执行线程或业务槽。复用现有设计态并发池，工作流与网页共同受租户开放 Run 限额约束，不能各自再获得一倍配额。沿用当前单 Run deadline、模型调用/输出/上下文上限和压缩机制；HTML 大小、Bridge 和隔离检查的具体上限在 D9-07/D9-08 固定。

## 6. 上下文、压缩与不可信内容

Host 每轮注入固定能力、当前 Profile、目标权限与不可执行生产的规则；应用说明、输出样例和 HTML 注释作为用户数据。它们不能覆盖系统工具能力，提示注入的“发布”“获取密钥”等要求仍由后端拒绝。

模型按需读取契约、SDK 和源码，不把全量工作流、全部历史执行或整个资源目录放入 Prompt。大内容分页/Artifact 化，缺失或截断明确标识。Core 压缩保留需求、未解决问题和工具调用完整性；当前修订/契约、检查报告和权限始终由结构化事实与工具重新校准。

静态检查、模拟预览、真实用户测试分别记录来源与 Hash。摘要不能把“预览提交成功”压成“已执行生产工作流”，也不能将旧报告用于新页面。Chat 消息不成为任务状态、发布状态、权限或长期记忆的权威。

## 7. 代码落点与验收

- Control 从现有 `studio_builder_*` 提取实际共用的 Host/状态/模型/事件模块，分别保留 workflow 与 application_ui 的上下文和工具模块；应用页面领域负责草稿/版本。
- `agentx-agent-core` 保持零基础设施依赖；本次原则上通过现有 Port 接入，必要契约变化同步生成 Schema/TS 与漂移测试。
- Runtime 重构 `builder_model.rs` 及现有设计态来源治理，复用 Provider/Vault/Egress/Receipt；不复制一套网页专属模型 Client。
- Web 复用对话/事件/进度等组件，画布和 HTML 编辑各自使用领域 Dispatcher；不让现有画布手动编辑依赖 AI 服务。

主交付 P9-02，发布宿主接入 P9-03；验收 E9-G01–G08。必须同时回归现有工作流创作、取消/恢复、模型设置与评测，保证共享改造没有扩大原工具权限或改变业务模型。

## 8. 实现补充：已确认失败的恢复

实测发现模型输出额度耗尽时可能只返回思考内容，不能作为创作完成。共用模型投影拒绝 `finish_reason=length` 和无正文/无工具的响应，保留已知 Usage 并报告具体失败。Core 的 Effect Observation 增加 `Failed`，用于 Host 重启后结算已确认的失败；它不重发模型或工具请求，也不把确定失败误报为结果未知。此变化只完善同一 Port 的恢复语义，不引入基础设施依赖或第二套任务状态。

真实验收继续固定 APIMart glm-5.3-flash，采用该模型支持的 `reasoning_effort=low`，仍保留 16,000 输出 Token 和同一预算上限。该模型不能关闭思考，参数依据为 [Z.AI GLM-5.3-Flash 官方说明](https://docs.z.ai/guides/vlm/glm-5.3-flash)。前期默认参数耗尽额度、未生成 HTML 的失败保留，不计成功。

## 9. 业务模型参数的发布快照

真实业务验收发现，原业务 Agent 的 Provider 适配器只发送模型名和消息，丢失了模型部署默认参数及输出上限。Runtime 的 Model 资源配置现将 `parameters` 与 `maxOutputTokens` 一并纳入签名 Bundle；Control 从既有模型版本快照读取这两项，业务执行不回读平台创作设置。Agent 与普通 Model 节点都使用冻结参数，实际输出上限同时受模型和 Agent 剩余预算限制。参数不能改写消息、工具、Provider 身份或凭据。共享契约直接更新，不为旧 Bundle 增加兼容入口。

创作提示明确 `upsert_node` 替换完整节点，编辑原受保护 Exit 时必须保留 id、type 和 `protected:true`，错误反馈指出缺失字段。Code 输出示例按 JSON 的数学数值推导类型；`900` 与 `900.0` 同为 integer，混合整数/小数数组推导 number。金额等允许小数的字段应使用真正带小数的示例，不能靠 `0.0` 保留类型提示，更不能强转或截断真实业务值。

画布设计工具对结束节点的 upsert 和整图替换先使用已有 `ExitParameters` 类型补齐可选空映射，再计算命令 Hash，保证与手工编辑器的表示一致。只补齐默认值，业务映射、资源、受保护状态和准确 ACK 校验不放宽；未知参数仍拒绝。编辑器拒绝不一致表示时返回最多八个差异 JSON Pointer，不泄露参数值，避免让 AI 反复猜测隐藏默认项。

画布编辑被浏览器拒绝时，ACK 的有界诊断保留错误码、nodeId 和 fieldPath，不只返回通用消息。后端工具参数仍由原 Serde 类型判定是否有效；对于已拒绝的参数，复用原工具 Schema 定位字段，以 `fieldErrors` 返回 JSON Pointer，不回显无效业务值。Exit 参数采用同样的定位方式，保留类型默认值、保护标记和精确文档 Hash/ACK 校验。Profile 明确 upsert_node 的 id/type/protected 位于 node 内，错误端口专用 Exit 不为未使用的 outputs 填空值；不通过放宽校验来提高生成成功率。


## 未知模型结果的来源保留

创作 Run 的停止、截止或授权撤销不代表已发送模型 Effect 已结算。Runtime 继续作为结果与费用的唯一权威；Control 在已有设计日志中、实际提交前持久记录 `model-dispatch:{effectId}`，只在正式 Runtime 回执确认 succeeded、failed 或未发送取消 cancelled 时记录对应 `model-resolved:{effectId}`。响应丢失、临时不可用及 outcome_unknown 均不能释放引用。此日志只为来源保留提供证据，不新增模型状态机，不把它当作业务完成结果。

模型、凭据、原工作流和设计历史的删除/Retention 同时检查开放 Run 与尚无已知回执的提交；后续 Session 的新 Run 不能覆盖旧 Effect 的引用。模型和凭据的删除影响中同时列出画布及页面创作。无需读取 Runtime 数据库或把调用者身份重新推断为结果；已有签名回执接口保持跨域边界。已知未发送取消仍能释放引用，过期且已结算的历史继续按既有 Retention 清理。
