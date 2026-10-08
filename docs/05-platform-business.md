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
- 哪些 Workflow 可以读写某 Memory Namespace
- 哪些 Workflow 可以使用某 Credential

授权控制面使用统一“资源授权”列表聚合 Credential、Model、MCP Server、MCP Tool、Skill、RAG 和 Memory。资源详情页只负责配置、版本和连接诊断，不再各自维护授权面板。统一列表可按资源类型筛选，并分别向 Department 或 Workflow Service Identity 授予 `view/use/read/write/manage`。

发布时进行完整检查，运行时再次校验。资源授权被撤销后，新执行不能继续使用该资源。MCP Tool 的依赖链固定为 `MCP Tool → MCP Server → Credential`，三类资源必须逐项授权，任一 Grant 都不能隐式传递给下一项。

Workflow Studio 的资源选择器列出当前用户可见的全部有效资源，并按资源对 Workflow Service Identity 的状态显示 `authorized/grantable/requestable/pending/rejected/unavailable`。资源选项按 `resourceType + operation` 独立查询，API 支持搜索和分页；Company 数据范围角色、部门闭包和已有 Department View Grant 使用同一套可见性规则。只有 `authorized` 可选择；有工作流编辑权、`resource:grant` 且能授权完整依赖包的用户可以在画布确认后直接原子授权，其他编辑者可以提交设计期资源授权申请。直接授权和审批通过都只刷新资源状态，不自动修改 Draft 或选择资源。

设计期资源授权申请与运行时 `approval_tasks` 相互独立。申请只接受主资源、操作、来源节点、Draft Revision 和可选说明，完整依赖包必须由服务端展开；Model 与 Credential、MCP Tool 与 MCP Server/Credential、Skill 与递归依赖都逐项授权。同部门资源合并为一个 Review，跨部门必须全部会签；任一部门拒绝则整包拒绝，全部通过后在一个事务内创建全部 Grant。直接授权、申请和审批动作均使用 Idempotency Key；审批还携带预期 Review Version，并重新校验资源状态、依赖指纹、工作流状态和申请人编辑权，变化后标记 `stale`，不得按旧依赖授权。

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

M2.1 支持 Streamable HTTP 和 Legacy SSE，不支持 stdio。MCP Server 固化传输、Endpoint、Credential Reference 和非敏感配置 Hash；MCP Tool 固化名称、输入输出 Schema、Annotation 和 Schema Hash。用户只能设置 Tool 的启停、调试开关、超时和副作用等级，不能修改服务端声明的 Schema。界面将 JSON Schema 渲染为字段树，直接展示字段名、类型、必填、说明、约束和嵌套结构，原始 JSON 不作为默认阅读界面。

调试调用需要 `mcp:debug`、资源可见性、参数 Schema 校验和危险操作确认。审计只保存参数 Hash、状态和耗时，不保存请求参数、响应正文或 Secret。Workflow Service Identity 必须分别获得 MCP Server、MCP Tool 和 Credential 的 Grant；Server 可见性不能替代 Tool 使用权限。

Sandbox 与 MCP 完全分离。Python、JavaScript、Shell 和其他动态代码由阶段 10 的 Sandbox Profile 与 `OpenSandboxAdapter` 处理，不是 MCP Server 或 Tool Runner 类型。Sandbox 网络和 Credential 权限独立校验，MCP Grant 不隐式授予 Sandbox 外网访问。

## 5. Skill 管理

Skill 是 Agent 可加载的版本化能力资源，用于封装稳定的指令、资产引用和依赖声明，而不是绕过平台执行权限的新节点。

Skill Definition 描述业务身份和生命周期；Alias 是租户内唯一的稳定可读标识，允许 Unicode 字母、数字、连字符和下划线，英文字母统一为小写。新建时自动创建不可删除的根文件 `SKILL.md`。根文件必须包含 `name` 和 `description` frontmatter，前端将 Skill 描述作为正文上方的独立必填多行文本框展示，后端统一生成 frontmatter 并与 Skill 元数据同步。用户可在线创建目录和 Markdown 文件、移动或重命名条目，并把图片、PDF、文本、代码或其他二进制拖拽上传到目录。只有 Markdown 可在线编辑；其他文件只读预览或显示元数据。

Markdown 使用开源 MDXEditor/Lexical 富文本界面编辑，用户通过标题、加粗、列表、链接和表格等可视化控件生成标准 Markdown，不需要掌握 Markdown 语法。编辑器可从当前 Skill 文件树选择目标并插入标准相对链接。移动或重命名文件时服务端同步重写内部相对链接；删除被引用文件会被拒绝。发布前通过 Markdown AST 校验引用，Skill Version 固化每个文件的路径、Artifact、Content Hash、引用目标 Hash，以及 Model、MCP Tool、Credential、Skill、RAG 和 Memory 依赖。ZIP 只用于当前工作区导入导出，不是 Skill 的持久化模型。

Workflow Draft 可以选择 Skill Definition 和目标版本；发布时必须固化 Skill Version，并检查所有直接和递归依赖对 Workflow Service Identity 均已授权。Skill Grant 不能隐式授予其 MCP Tool、Model 或 Credential 权限，任一依赖授权被撤销后，新执行不得继续加载该 Skill。

仅包含 Prompt 和静态资产的 Skill 可由 Agent Runtime 直接加载；包含 Python、JavaScript、Shell 或其他不可信代码的能力必须通过 OpenSandbox 执行，并沿用平台的超时、资源配额、默认拒绝网络和短期凭证注入策略。

## 6. RAG

知识库接入按连接上的 `provider` 字段区分协议适配器，平台层使用统一接口：

- query
- retrieve
- insert
- delete
- healthCheck

已实现的 Provider：

- `lightrag`（默认）：`POST {endpoint}/query`，`x-api-key` 认证，外部资源 ID 为 workspace，支持 query 与 insert。
- `ragflow`：`POST {endpoint}/api/v1/retrieval`，`Authorization: Bearer <API Key>` 认证，外部资源 ID 为 dataset id（多个以逗号分隔），支持 query；RAGFlow 的 `{code, data}` 响应包映射为统一 documents 结构，`code != 0` 时按 `PROVIDER_REJECTED` 失败并透出 Provider 消息。健康检查路径需指向 RAGFlow 实际返回 200 的路径（如 `/v1/system/healthz`）。

生产环境 Provider 端点必须使用 HTTPS（egress 策略仅对集群内白名单 Service 放行明文 HTTP）；白名单通过 `global.network.egressGateway.httpProviderServices` 配置，默认只包含 E2E fixture（echo-mcp、echo-node、lightrag、mem0），自建明文 HTTP Provider（如集群内 RAGFlow）需要显式加入。

授权范围：

- Connection
- Workspace
- Knowledge Base 或 Collection
- Read 或 Write
- Department
- Workflow

RAG 节点的 Trace 包含查询、召回结果引用、Score、耗时和错误。

知识库管理面（plan7 P7-E）：

- LightRAG 资源支持文档上传→外部索引→状态跟踪→hit-testing 最小闭环。上传走 `POST /api/v1/knowledge/resources/{id}/documents`（multipart；text/plain、text/markdown、text/csv、application/json；8 MiB/文件、200 篇/资源、256 MiB 总量；同 sha256 去重）；索引由 Control 直连 LightRAG `documents/text`（同步、60s 超时），状态机 uploading→indexing→indexed/failed，`rag_resources.sync_status` 随之激活。
- RAGFlow 资源保持外部自管：文档区显示占位说明，写路径维持 `RAG_OPERATION_UNSUPPORTED`；hit-testing 同样支持（`POST .../retrieval-test`，query + topK 1..=20，结果归一 documents/citations/recordIds，历史落 `rag_retrieval_tests`）。
- 协议构造与响应归一单源于 `agentx-runtime-contracts::rag`（runtime worker、Agent 附件槽与 Control 共用；未支持的操作显式拒绝，delete 不再静默映射为查询）。

## 7. Memory

首期提供 Mem0 Adapter，统一接口：

- get
- search
- add
- update
- delete

Memory Scope：

- User
- Session
- Application
- Workflow

Workflow 需要明确自己可访问的 Namespace 及读写权限。

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

调用请求需要支持：

- API Key
- Idempotency Key
- 输入 Schema 校验
- 超时
- 同步、流式和异步模式

## 11. Dataset

Dataset 包含不可变 Dataset Version。Test Case 包含：

- Input
- Expected Output
- Context
- Tags
- Evaluation Profile Rule Override

测试数据改变后生成新版本，保证历史报告可复现。

## 12. Evaluation

Evaluation Run 将一个 Workflow Version、一个 Dataset Version 和一个不可变 Evaluation Profile Version 绑定，批量创建 Test Execution。

Evaluation Profile 是用户唯一需要管理的评测配置。一个 Profile 包含多条评分规则、聚合策略和通过阈值；发布后生成不可变 Profile Version。首期评分规则：

- Exact Match
- Contains
- Regex
- JSON Schema
- 自定义代码
- LLM Judge
- Tool 是否正确调用
- Tool 参数是否正确
- 是否出现 Agent 循环
- 节点错误次数
- 总成本
- 总耗时

报告内容：

- 总通过率
- 每个 Case 的结果
- Workflow Version 对比
- 失败节点分布
- Tool 错误分布
- 成本和耗时分布
- 失败 Case 的 Trace 链接

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
