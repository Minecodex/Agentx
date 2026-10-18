# Workflow 上下游业务模块

## 1. 单公司、部门和权限

首期一个部署只支持一个公司，不提供 Tenant CRUD 或切换入口。所有业务实体仍包含 tenant_id，作为隔离、索引和后续扩展边界。

组织模型：

- Tenant
- Department
- User
- Role
- Permission
- Workflow Service Identity

权限分为两层。

### 操作权限

示例：

- workflow:create
- workflow:edit
- workflow:run
- workflow:publish
- workflow:view_execution
- workflow:fork_execution
- workflow:manage_permission
- model:manage
- mcp:view
- mcp:manage
- mcp:discover
- mcp:debug
- skill:manage
- dataset:manage
- approval:handle
- application:invoke

### 数据范围

- 本人
- 本部门
- 本部门及子部门
- 全租户
- 指定 Workflow
- 指定资源

设计者的权限只决定他能否配置 Workflow。生产运行时使用 Workflow Service Identity，避免 Workflow 继承创建者个人权限。

Execution 可以把发起人的用户、部门与角色快照作为普通业务变量，用于模板、数据映射和条件分支；这些变量不是授权主体，不能改变 Model、MCP、Credential 或其他资源的 Grant 结果。资源授权始终只认发布或调试 Work Package 中固化的 Workflow Service Identity。角色名称是展示快照，业务中的稳定判断使用角色 ID 或编码。

Bootstrap Admin 是 Company Admin，可以管理全部部门、用户和角色。Department Admin 只能管理其授权部门及子部门，不能访问父级或兄弟部门；新用户由服务端设置固定的一次性初始密码 `123456`，首次登录必须修改，正式密码仍执行 12–128 位规则。

## 2. Workflow 资源授权

资源授权需要表达：

- 哪些 Workflow 可以使用某 Model
- 哪些 Workflow 可以使用某 MCP Server
- 哪些 Workflow 可以调用某个自动发现的 MCP Tool
- 哪些 Workflow 可以加载某 Skill
- 哪些 Workflow 可以查询某 RAG Collection
- 哪些 Workflow 可以使用某记忆连接（同时读取和写入）
- 哪些 Workflow 可以使用某 Credential

授权控制面使用统一“资源授权”列表聚合 Credential、Model、MCP Server、MCP Tool、Skill、RAG 和 Memory。资源详情页只负责配置、版本和连接诊断，不再各自维护授权面板。统一列表可按资源类型筛选，并分别向 Department 或 Workflow Service Identity 授予 `view/use/read/write/manage`。

发布时进行完整检查，运行时再次校验。资源授权被撤销后，新执行不能继续使用该资源。MCP Tool 的依赖链为 `MCP Tool Version → MCP Server Version → Credential`，按所选不可变版本展开，不以当前连接配置替换旧版本依赖。STDIO MCP 还包括版本化 Sandbox Profile 与环境变量 Credential。完整依赖包必须逐项授权，任一 Grant 都不能隐式传递给下一项。

Workflow Studio 的资源选择器列出当前用户可见的全部有效资源，并按资源对 Workflow Service Identity 的状态显示 `authorized/grantable/requestable/pending/rejected/unavailable`。资源选项按 `resourceType + operation` 独立查询，API 支持搜索和分页；Company 数据范围角色、部门闭包和已有 Department View Grant 使用同一套可见性规则。只有 `authorized` 可选择；有工作流编辑权、`resource:grant` 且能授权完整依赖包的用户可以在画布确认后直接原子授权，其他编辑者可以提交设计期资源授权申请。直接授权和审批通过都只刷新资源状态，不自动修改 Draft 或选择资源。

设计期资源授权申请与运行时 `approval_tasks` 相互独立。申请只接受主资源、操作、来源节点、Draft Revision 和可选说明，完整依赖包必须由服务端展开；Model 与 Credential、MCP Tool 与 MCP Server/Credential、Skill 与递归依赖都逐项授权。同部门资源合并为一个 Review，跨部门必须全部会签；任一部门拒绝则整包拒绝，全部通过后在一个事务内创建全部 Grant。直接授权、申请和审批动作均使用 Idempotency Key；审批还携带预期 Review Version，并重新校验资源状态、依赖指纹、工作流状态和申请人编辑权，变化后标记 `stale`，不得按旧依赖授权。

审批和取消以申请主记录串行化；终态申请不能再改变部门 Review。依赖指纹绑定完整授权包及当前资源配置，知识文档内容变化不改变配置指纹。提交、部门决定、最终通过、拒绝、取消和失效动作与业务状态在同一事务写入既有审计表，申请详情返回真实历史及 RFC 3339 时间。当前资源 Grant 不包含到期时间；`stale` 表示申请依据已失效，资源撤销和用户 Token 撤销各自执行权威校验。

IAM 创建用户、变更部门/角色/角色权限及停用用户时，User Admission 与该用户对既有 Application 的调用/查询 Grant 在同一 Control 事务中写入 Outbox。Application Grant 使用该应用单调递增的 Admission Epoch；不以用户 Token Version 替代应用 Epoch。权限投影沿用发布时的统一可见性与角色规则，使新用户能够访问已发布的授权应用，撤销权限后的新登录 Token 也无法继续调用。

部门 Review 由其管理范围内同时拥有 `department_admin`、`approval:act` 和 `resource:grant` 的有效用户处理；没有有效部门审批人时由 Company Admin 兜底。审批人不需要目标工作流的管理权限，只看到申请用途与必要的脱敏上下文。待审批中心以“运行审批”和“资源授权”两个页签分别承载两套状态机。

## 3. 模型管理

模型对象：

- Model Deployment
- Model Alias
- Credential Reference
- Price Version
- Default Parameters
- Workflow Grant

Workflow 推荐引用 Model Alias，而不是将真实 Endpoint 和 Credential 写入节点参数。

连接不是独立资源。新建模型时在一个表单内同时填写连接名称、API 格式、Endpoint、Credential、模型名称、上游模型 ID、输入输出 Token 上限、所属部门和默认参数；后端在一个事务内创建不可变 Model Deployment Revision 与稳定的 Model Alias。Alias 在界面中作为“模型名称”供 Workflow 选择，供应商实际接收的 Model Name 显示为“上游模型 ID”。任一连接或模型配置变化时创建新的 Deployment Revision，并在同一事务切换 Alias；历史 Revision、价格和切换操作者始终可查询，不能被编辑覆盖。

API 格式描述供应商接受的模型调用协议，不等同于内部 Provider Adapter 类型。当前 Runtime 只支持 OpenAI Chat Completions，因此控制面只允许选择该格式；连接测试使用当前上游模型 ID 发送最小 `POST /chat/completions` 请求，与 Workflow 运行路径一致。实现 Responses Runtime 后再增加 OpenAI Responses 选项。

模型推理请求显式指定 `stream`。价格版本和 Agent 调用用途属于 Agentx 内部运行数据，保留在 Runtime Call Ledger 与 Trace 中，不附加到上游 `metadata`；这样使用严格校验请求字段的 OpenAI-compatible 服务时，也能沿用相同的模型与 Agent 调用路径。

新建 Model Alias 的连接状态固定为 `untested`。只有用户手动发起 Alias 级连接测试后，状态才更新为 `healthy` 或 `unhealthy`；Alias 或当前 Deployment Revision 的有效配置发生变化后，旧健康结果立即失效并重新显示 `untested`。

模型调用记录：

- Provider
- Model
- Alias
- 请求参数
- Token
- 成本
- 延迟
- 错误

模型创建和 Deployment Revision 切换时，币种、输入单价/百万 Token、输出单价/百万 Token 都是必填项；允许显式填写零价，但不允许创建无价格的 Model Deployment。发布 Bundle 冻结价格版本 ID、币种和两类单价，Runtime 使用 Provider 返回的输入/输出 Token 与该快照确定性计算微货币单位成本，不依赖 Provider 私有的成本字段。当前未单独配置缓存 Token 单价时，缓存输入仍按普通输入 Token 计价；历史执行始终按调用发生时的价格快照计算。

## 4. MCP Server 与 Tool

平台不提供人工创建 Tool 的入口。用户接入 MCP Server 后，控制面通过 `initialize` 和分页 `tools/list` 自动发现 MCP Tool；同名且 Schema Hash 相同的结果幂等复用，Schema 变化产生不可变 Tool Version，未再次发现的 Tool 标记为 unavailable。

支持 Streamable HTTP、Legacy SSE 和在 OpenSandbox 内运行的 STDIO。MCP Server 固化传输、Endpoint、Credential Reference 和非敏感配置 Hash；STDIO 另固化 Sandbox Profile Version 与环境变量 Credential Reference。MCP Tool 固化名称、输入输出 Schema、Annotation 和 Schema Hash。用户只能设置 Tool 的启停、调试开关、超时和副作用等级，不能修改服务端声明的 Schema。界面将 JSON Schema 渲染为字段树，直接展示字段名、类型、必填、说明、约束和嵌套结构，原始 JSON 不作为默认阅读界面。

调试调用需要 `mcp:debug`、资源可见性、参数 Schema 校验和危险操作确认。审计只保存参数 Hash、状态和耗时，不保存请求参数、响应正文或 Secret。Workflow Service Identity 必须分别获得 MCP Server、MCP Tool 和 Credential 的 Grant；Server 可见性不能替代 Tool 使用权限。

Sandbox 与 MCP 完全分离。Python、JavaScript、Shell 和其他动态代码由阶段 10 的 Sandbox Profile 与 `OpenSandboxAdapter` 处理，不是 MCP Server 或 Tool Runner 类型。Sandbox 网络和 Credential 权限独立校验，MCP Grant 不隐式授予 Sandbox 外网访问。

## 5. Skill 管理

Skill 是 Agent 可加载的版本化能力资源，用于封装稳定的指令、资产引用和依赖声明，而不是绕过平台执行权限的新节点。

Skill Definition 描述业务身份和生命周期；Alias 是租户内唯一的稳定可读标识，允许 Unicode 字母、数字、连字符和下划线，英文字母统一为小写。新建时自动创建不可删除的根文件 `SKILL.md`。根文件必须包含 `name` 和 `description` frontmatter，前端将 Skill 描述作为正文上方的独立必填多行文本框展示，后端统一生成 frontmatter 并与 Skill 元数据同步。用户可在线创建目录和 Markdown 文件、移动或重命名条目，并把图片、PDF、文本、代码或其他二进制拖拽上传到目录。只有 Markdown 可在线编辑；其他文件只读预览或显示元数据。

Markdown 使用开源 MDXEditor/Lexical 富文本界面编辑，用户通过标题、加粗、列表、链接和表格等可视化控件生成标准 Markdown，不需要掌握 Markdown 语法。编辑器可从当前 Skill 文件树选择目标并插入标准相对链接。移动或重命名文件时服务端同步重写内部相对链接；删除被引用文件会被拒绝。发布前通过 Markdown AST 校验引用，Skill Version 固化每个文件的路径、Artifact、Content Hash、引用目标 Hash，以及 Model、MCP Tool、Credential、Skill、RAG 和 Memory 依赖。ZIP 只用于当前工作区导入导出，不是 Skill 的持久化模型。

Workflow Draft 可以选择 Skill Definition 和目标版本；发布时必须固化 Skill Version，并检查所有直接和递归依赖对 Workflow Service Identity 均已授权。Skill Grant 不能隐式授予其 MCP Tool、Model 或 Credential 权限，任一依赖授权被撤销后，新执行不得继续加载该 Skill。

Agent 加载 Skill 指令时同时向模型提供被冻结的 `skillVersionId`，用于 `skill_resource` 读取附件。版本标识来自已签名的资源绑定，不能依靠模型猜测；附件读取仍逐项校验版本、Hash、授权和依赖闭包。

发布 Skill Version 时，其所属部门必须能使用完整依赖包，其他部门或工作流的 Grant 不构成发布权限。逐项按依赖声明的操作校验；Memory 统一使用 `use` Grant，同时允许召回和写入。

仅包含 Prompt 和静态资产的 Skill 可由 Agent Runtime 直接加载；包含 Python、JavaScript、Shell 或其他不可信代码的能力必须通过 OpenSandbox 执行，并沿用平台的超时、资源配额、默认拒绝网络和短期凭证注入策略。

## 6. RAG

知识库采用“外部连接 → 查询配置 → Workflow 使用”模型。Agentx 管理接入、部门可见性、授权和检索，外部平台管理文档与索引。管理面不提供上传、删除文档或索引任务。

连接只包含名称、协议、Endpoint、凭证和所属部门，支持 CRUD、停用和连接测试。连接详情管理多条查询；每条查询独立设置业务名称、用途、结构化内容范围、所属部门、状态和统一授权。同一外部范围可以对应不同授权的查询。查询仍属于 `rag` 资源，运行授权包含凭证依赖。

- RAGFlow：从 `/api/v1/datasets` 发现真实数据集，查询可多选 Dataset，不提供目录过滤。每个 Dataset 单独调用 `/api/v1/retrieval`，避免不同 embedding 模型混用。Bearer 认证。
- LightRAG：从认证后的 `/health` 读取实例的 `configuration.workspace`，空字符串表示真实默认工作区。每条查询只能选择该实例返回的工作区。检索调用 `/query/data`，使用 `naive` 结构化结果和 `x-api-key`。其他实例另建连接，不通过 namespace、请求参数或 header 切换 workspace。

Agent 每条查询派生独立知识工具，描述包含业务名称和用途。独立知识检索节点每次执行所有已选查询，支持跨连接多选。运行包冻结查询与连接版本、结构化范围和凭证版本；内容使用外部服务当前索引。

共享检索模块按连接与实际来源合并重复请求，最多并发 4，每来源最多 20 个候选；使用 RRF（常数 60）合并排名，最终 Top K 默认 5、允许 1–20。结果保留 `text/documents/files`，增加 `partial/sources`；每个命中保留查询标识、连接、Dataset/workspace、原始分数和融合分数。附件仍为外部来源引用，不授予平台 Artifact 权限。

授权在请求前统一校验，授权失败直接拒绝。部分来源成功时返回成功来源结果并明确标记失败来源；成功但无命中也算成功；全部失败走错误出口。查询测试弹窗复用相同执行器，显示命中、来源、耗时和错误。

接口以 `/knowledge/connections`、`/connections/{id}/catalog`、`/connections/{id}/queries`、`/queries/{id}`、`/queries/{id}/documents` 和 `/queries/{id}/retrieval-test` 为边界。删除查询只影响 Agentx 配置，保留 Workflow/Skill 引用保护；有查询的连接不能删除。旧 resources 路径不保留别名。

生产 Provider 端点使用 HTTPS；集群白名单 HTTP 端点由 `global.network.egressGateway.httpProviderServices` 明确配置。实现和验收规范见 [知识连接与多来源检索](plan8/11-knowledge-connections.md)。

## 7. Memory

首期提供 Mem0 Adapter，统一接口：

认证根据冻结的 Credential 类型选择 Mem0 原生协议：`api_key` 使用 `X-API-Key`，`bearer` 使用 `Authorization: Bearer` 登录令牌。持续部署使用可撤销的服务 API Key；登录令牌按 Mem0 的有效期管理。运行时必须匹配完整 Secret Reference 和 Secret Version，缺失绑定或其他凭证类型明确拒绝，不猜测认证方式。

- get
- search
- add
- update
- delete

记忆连接是唯一授权资源，固定支持读写，不提供独立资源名称、命名空间或访问模式。个人范围由租户、环境、应用、工作流、稳定节点和来源用户自动生成；版本、部署、会话及 API Key 不参与身份。无用户身份时不使用共享记忆。

应用部署的高级配置 `apiUserIdInput` 可选择顶层字符串输入，API 直接使用其值作为调用方声明的用户，不做用户认证、账号或归属核验。平台登录与渠道发送者自动识别。会话初始输入固定身份与业务参数，后续消息继承。身份仅用于记忆、归属和查询，不替代工作流运行授权。完整规则见 [应用用户与记忆连接](plan8/10-application-user-memory.md)。

## 8. Application

发布后的 Workflow 可以创建 Application。Application 包含：

- 当前 Deployment
- 输入输出 Schema
- Playground 配置
- Session 配置
- API 配置
- API Key
- 版本路由策略

Application 可以：

- 创建会话
- 发送消息
- SSE 流式响应
- 异步调用
- Webhook 回调
- 查询 Execution 状态
- 取消 Execution
- IM 渠道出站回复（plan7 P7-A）：渠道配置 `reply { enabled, outputField, template }` 冻结进 trigger revision；Execution 成功终态在终态事务内向 `delivery_outbox` 入队平台回复，由 delivery Role 异步投递（租约+栅栏+指数退避，5 次后死信），投递记录可查、死信可重放（`POST /api/v1/deliveries/{id}/retry`）；工作流内亦可用 `reply_message`/`send_message` 节点控制回复内容与时机。

渠道表单按平台配置、输入映射、固定输入、回复配置的顺序填写；新建和编辑使用相同顺序。新建渠道默认启用，启用与停用通过渠道列表按钮操作，编辑配置保留当前状态，表单不展示状态字段。回复配置使用“开启回复”开关，开启后展示“回复输出字段”下拉框和“回复模板”多行文本框。输出字段必须来自应用部署固定的输出 Schema；首次应用部署前使用绑定工作流最新不可变版本的输出。模板可留空，留空时直接回复所选输出字段；有模板时可使用 `{{output.字段名}}` 组织最终回复。

输入映射下拉框列出同一 Schema 的全部输入，默认仅创建必填输入的空映射行，由用户明确选择来源，可选输入按需添加。映射或固定输入必须覆盖全部必填输入。没有已发布工作流版本时，界面提示先发布版本并禁止配置渠道；执行仍以实际部署和发布的触发器快照为准。

长连接在应用部署发布后启动。尚未发布且没有运行连接记录时，渠道详情显示“待发布”和创建部署入口；已发布但未取得运行状态时，显示“连接状态暂不可用”。等待连接、已连接、重连中和连接异常来自 Runtime 实际状态；已有连接的配置发生变更时，单独提示当前连接仍使用已发布配置。发布完成后刷新应用与渠道数据，已发布长连接持续刷新状态。

应用详情的“渠道对接”包含“渠道配置”和“投递记录”两个子 Tab，默认进入渠道配置。投递记录汇总当前应用所有渠道的消息投递，保留状态筛选、失败原因和死信重放操作；页面通过 `tab=channels&channelTab=config|deliveries` 保存子 Tab，支持刷新和浏览器前进、后退。

## 9. Session 和 Message

一个 Session 包含多轮 Message，每轮用户输入产生一次独立 Workflow Execution：

- Session
  - User Message
  - Workflow Execution
  - Assistant Message

这样每轮对话都有独立 Trace、成本、Checkpoint 和错误记录。

Session 默认固定 Workflow Version，保证长会话行为稳定。Application 可以选择：

- Session 固定版本
- 每轮使用当前 Deployment
- 管理员显式升级 Session 版本

Playground 使用相同的 Application API，不维护另一套运行路径。

Application 的 Request/Response Contract 由不可变 Workflow Version 决定：Deployment 固化发布时的 Start Input Schema 和 End Output Schema；没有 End Output 的 Version 不能部署为 Application。Playground 的参数测试根据该 Schema 渲染完整表单，通过正式 Application Invocation API 创建无 Session 的 Invocation，历史查询使用 `sessionMode=stateless`。

对话测试只提供 question 和 files 两个 Composer 能力。`ChatMappingV1` 把它们显式映射到顶层 string 与 Artifact 输入，并把 Assistant 文本和附件映射到顶层非敏感 string 与 Artifact 输出；未映射的必填输入必须有 Schema 默认值。映射按 Application Deployment 共享，Control 用乐观锁保存修订并经 Outbox 发布到 Runtime；发布未完成时禁止发送。

每轮消息根据 Session 实际选择的 Bundle 读取最新映射，并把映射 Version 和完整内容写入 Invocation 快照。Execution 完成后只按该快照提取 Message Parts，不按 `message`、`answer`、`text` 或其他字段名猜测。修改映射只影响当前会话的下一轮，历史 Invocation 与消息不重算；清除映射后立即回到未配置状态。

Playground 新建 Session 不预先写入通用标题。Runtime 在首条用户文本消息与 Message 同一事务内为空标题 Session 补全会话名称，后续消息不再覆盖；标题合并空白并限制为 255 个字符。

Session Context 按 `tenantId + applicationDeploymentId + sessionId` 隔离，用于历史和连续会话；没有 Session ID 时不创建持久上下文。外部调用默认只能写 Start Inputs，只有 Context Contract 显式标记 `clientWritable` 的字段才可由 Adapter 开放。

## 10. API

建议的核心接口：

- 创建 Application Session
- 查询 Session
- 发送 Message
- 查询 Message 历史
- 查询 Execution
- 取消 Execution
- 从 Checkpoint Fork
- 获取 SSE Stream
- Webhook Trigger
- 预上传 Artifact 和 Multipart 文字加文件

应用 API 调用方可使用当前有效的 API Key，通过
`GET /gateway/v1/invocations/{id}/artifacts/{artifact_id}` 下载调用输出中明确引用的文件。
下载同时校验应用调用权限、输出引用和执行的 Artifact 归属，读取后核对大小与 SHA-256。
内部 Trace、工具中间结果等未出现在调用输出中的 Artifact 不通过此接口开放。
Runtime 启动执行时，统一依据冻结的 Start 输入 Schema 校验附件并保存执行引用，
使画布调试、应用调用和其他执行入口使用相同的附件归属规则；文件保留期限继续由现有 Retention 管理。
调试台上传向参数表单返回 `artifactId/fileName/contentType/sizeBytes/sha256`，
不向附件输入添加消息分段字段；对话发送时单独根据 Content-Type 确定消息的 `partType`。

调用请求需要支持：

- API Key
- Idempotency Key
- 输入 Schema 校验
- 超时
- 同步、流式和异步模式

## 11. Dataset

测试集属于应用，在应用详情的“测试与评测”中管理。创建时选择应用关联工作流的已发布版本，固定输入输出 Schema 和规范化契约指纹。任何验证语义变化（包括新增可选字段）均需新建测试集；标题、说明、字段排列、required 与枚举排列不改变契约。

用例保存输入、参考输出、标签、来源调用和稳定来源标识，允许保存待补全草稿。输入与参考输出均按固定契约检查；缺失/过期附件引用也列入待补全。复制仅带入同路径、同类型且符合目标约束的数据，不猜测改名或转换类型。调用样本通过 Runtime 授权查询读取执行当时的输入、输出及已发布版本，参考答案需用户确认。

评测前统一检查完整性并自动创建或复用该修订的不可变 Dataset Version，用户无需手动发布测试数据。快照保留完整用例数据与来源关系。历史报告引用的测试集禁止删除。

## 12. Evaluation

应用评测方案只保存自然语言评分标准和通过分，修改两者生成不可变方案版本。默认通过分 80；内部标准范围 0–1。移除规则类型、权重、聚合策略、逐用例规则覆盖及方案独立模型选择。

发起评测时选择多个测试集，再为每集选择契约相同的多个已发布版本；一次生成一份批次报告。每个“数据快照 × 工作流版本”是一个 Runtime WorkPackage，接受请求前完成归属、修订、发布状态、完整契约、数据完整性及模型可用性检查。Idempotency-Key、事务内快照与 Outbox 保证重复提交和进程恢复不会产生重复执行。

批次冻结数据、工作流、方案、系统提示词、模型 Deployment、参数、凭据和价格。裁判使用“平台 AI 模型（画布与评测）”的选中模型，通过签名包内的租户/应用/批次授权运行；授权在 Runtime 独立持久化并可通过 Admission 撤销，不加入业务工作流的资源及 Grant。裁判只运行系统生成的 Model/Exit 子执行，没有工具。输入、参考输出、实际输出和标准作为用户数据传入固定系统提示词。

模型只返回 score 和 reason，平台根据固定阈值决定是否通过。质量未通过仍为正常完成；工作流执行错误和评分错误使批次失败，错误不写伪造零分。部分失败保留其他结果，取消通过持久任务传播到未完成运行，重新评测创建新批次。

报告提供进度、评分、质量通过率、错误、业务与评分耗时、按币种分开的业务/评分成本，以及两份 Trace。同一快照的版本对比默认以最小版本号为基线，可显式切换；跨测试集只按稳定来源对齐并标记数据变化，不推断相同名称/caseKey 是同一用例，不生成无条件整体优劣结论。

本期边界是工作流单次输入输出，不评测会话、渠道、用户记忆或附件正文。[实现及验收](plan8/12-application-evaluations.md)。

## 13. 消息中心

消息只围绕 Workflow 业务：

- 待审批
- 审批完成
- Execution 失败
- 测试集运行完成
- Workflow 发布成功或失败
- Agent 达到成本或循环限制

消息可以跳转到 Approval、Execution、Workflow 或 Evaluation Report。

## 14. 安全删除

可管理实体采用统一的“预检 + 事务内复检”删除协议。列表通过 `GET /api/v1/deletion-impact/{entityType}/{id}` 展示引用模块、实体名称、关系和画布节点位置；实际 `DELETE` 携带 `expectedVersion`，服务端锁定目标后再次执行同一检查。新增引用、目标版本变化或系统实体保护分别返回 `ENTITY_IN_USE`、`VERSION_CONFLICT` 或 `SYSTEM_ENTITY_IMMUTABLE`，客户端不能依赖预检结果直接假定删除成功。

Workflow、Environment、Application、Credential、Model Alias、MCP Server、Skill、Knowledge Resource、Memory Namespace、Sandbox Profile、Dataset、Evaluation Profile、Department、自定义 Role、Application Webhook 和 Schedule 支持物理删除。User、API Key、不可变版本和部署、Execution、Approval、Notification、Evaluation Run/Report、Session 与 Message 只允许禁用、撤销、归档或 Retention，不提供物理删除入口。

父实体自有的 Revision、Workspace、Version、Price、Policy 等不是外部引用；没有外部引用时由父实体事务按依赖顺序清理。不可变 Workflow Version、Draft、Skill Dependency、Resource Grant、Session、Invocation、Evaluation Run 和组织归属等外部关系一律阻止删除，用户必须在来源模块解除引用。Artifact 对象不在删除事务中直接移除，由现有 Retention 负责回收。

## 15. 用户输入唯一性与错误契约

用户输入型唯一字段采用统一写入协议：按领域规则规范化，按唯一索引真实作用域前置查询，并保留数据库约束处理并发竞态。前置检查和数据库兜底返回相同的稳定领域错误码、HTTP 409 与 `fieldErrors`，客户端不得展示 SQL 错误。幂等键、版本号、内容 Hash 和执行序号等内部约束仍由业务流程显式处理；未登记的唯一冲突返回 `INTERNAL_ERROR`，日志只记录索引、数据库错误码和 requestId，不记录冲突值。

Dataset 导入先检查文件内重复，再在事务中检查现有 Case，并用 `details.caseKey/line` 定位。Artifact 写入必须位于可执行的业务前检之后；后续事务失败立即补偿对象和配额，补偿失败由 Retention 兜底。

## 平台 AI 模型（画布与评测）

模型列表保留一个选中项，统一接口为 `/api/v1/platform-ai/model-setting`，字段为 `platform_ai_model_alias_id`、`platform_ai_credential_id`。设置权限继续要求 model:manage、company:manage、resource:grant；工作流编辑者和评测用户可读取名称及可用状态。

这份租户配置明确授权画布创作与系统评测，均不向业务 Workflow 自动写 Grant。每个新 Run/批次冻结具体部署、凭据、参数和价格；普通切换影响新任务。清除设置、停用模型或凭据通过持久 Admission 撤销相关未完成评分授权，错误进入报告，不切换备用模型。Alias 使用另一凭据后须重新设置。
