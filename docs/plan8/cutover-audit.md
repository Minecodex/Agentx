# plan8 切换审计

审计对象为当前工作树；新协议不保留单草稿入口或数据转换。系统验收状态见 [实施报告](implementation-report.md)。

| 旧假设 | 当前实现与调用者 |
|---|---|
| Workflow 唯一 Draft | `workflow_drafts` 列表/创建/元数据 API；Workflow DTO 使用 `draftCount`，创建响应返回 `createdDraftId` |
| workflow-only editor | `/workflows/:workflowId/drafts/:draftId/editor` 与只读 `/versions/:versionId/editor`；Store/RenderStore 每实例创建 |
| 自动读取 Head 进行运行 | Debug URL 指定 Draft，`expectedRevision` 查不可变 Revision；版本执行显式 `versionId`，来源联合保存在 Work Package/Runtime Snapshot/BFF |
| 单一保存修订和编号 | 每 Draft 修订；原子 revision 0；Version 源 Draft/Revision/BaseVersion 和 Workflow 行锁编号、重复来源幂等 |
| 保存响应即当前文档 | 提交时捕获文档序号；响应只确认其快照；后续 dirty/Recovery 保留；旧字段错误不污染新文档 |
| AI 独立修改/保存 | Dispatcher 共用 commit/history；一批命令一个 Undo；文档 hash/序号、命令 ID、ACK 和 Recovery 去重 |
| Workflow 作用域的 Overlay/资源申请 | Overlay、资源验证、Grant Request 来源、插件查询和 Recovery 带 Draft ID；业务 Grant 主体仍为 Workflow Service Identity |
| 删除只查当前 Head | 资源删除影响及保护包含不可变 Revision 的资源快照；插件包包含所有 Revision、Version 和发布包；未知 Builder Effect 保留引用 |
| Canvas 自选模型/用户资源 Grant | `tenant_settings` 的 Alias/Credential 授权及 CAS；三权限交集；每 Run 冻结部署、凭据/价格和授权 epoch |
| Control 不承载任何 Core | 明确设计态 Host 例外，依赖同一纯 `agentx-agent-core`；业务 Worker 不访问草稿或回读 Control |
| 模型调用必须是业务 Execution | Runtime `studio_builder` 来源，业务三种执行 ID 为空；独立 Receipt、Cost、Quota、Delta 和 Retention |
| Resolver 可使用通用 Provider/Host | Gateway 到 Sandbox Manager 的 resolve-only 路径；固定镜像、空资源/环境、网络拒绝、文件/桥接限制与设计租约 |

共享 Core 为 Contract 1.3；新增 Platform/StudioDocument Origin 和 Pending/Resume 冻结请求。Schema/OpenAPI/TS 由现有生成器产出；无新版/旧版双读。

调用者切换覆盖 Web 路由、Studio API、列表/详情/版本/发布、资源申请、查询缓存和所有仓库内浏览器/Python Fixture。`draft_fixtures` 仅用于明确创建一个画布的测试，断言列表长度等于一；多个画布测试必须直接携带 Draft ID。

删除的 `ExecutionWaitV1`/`ExecutionWaitPageV1` Schema 在当前生成器已无对应类型和调用者；未保留孤立的旧生成物。

空库初始化及并发初始化已验证；源环境不执行新基线 DDL。版本来源的复合外键约束同 tenant、Workflow、Draft、Revision ID/编号和 BaseVersion 归属。
