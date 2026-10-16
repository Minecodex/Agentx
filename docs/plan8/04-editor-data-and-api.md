# plan8 编辑事务、数据与 API

状态：done。已修改基线 DDL 和生成协议，删除单草稿入口。下文区分实施前审计和当前实现。

## 1. 实施前基线审计

| 实施前状态 | 审计代码位置 | 对应改造 |
|---|---|---|
| 一个 Workflow 唯一 Draft；Revision 按 Workflow 唯一 | [Control 基线 DDL](../../deploy/migrations/control/0001_initial.sql#L1451) | Draft 独立身份、Revision 按 Draft 唯一 |
| Version 只有 sourceRevision，没有来源 Draft/baseVersion | [版本创建](../../src/services/platform-control/src/workflow_api.rs#L590) | 来源和基础版本、Workflow 级编号锁 |
| 历史版本只有 JSON、画布只按 Workflow 打开 | [版本面板](../../src/web/src/features/workflow-designer/panels/version-dialog.tsx#L18)、[路由](../../src/web/src/app/router.tsx#L64) | 版本只读画布、版本派生、多草稿路由 |
| Store 私有 commit 支持一条实体 Patch，无通用批量命令 | [Editor Store](../../src/web/src/features/workflow-designer/store/editor-store.ts#L142) | 统一 applyEditorCommand；hydrate 不能用于 AI 编辑 |
| 保存成功无条件 markSaved/clearRecovery | [保存响应](../../src/web/src/features/workflow-designer/workflow-canvas.tsx#L440) | 以完整文档编辑序号判断是否仍有未保存变更 |
| Debug 先检查 revision，另一路再读 Draft Head | [入口](../../src/services/platform-control/src/workflow_operations.rs#L268)、[Work Package](../../src/services/platform-control/src/work_packages.rs#L491) | 直接读取精确不可变 Revision，传递完整身份 |
| Overlay、资源校验和授权申请按唯一 Draft 寻址 | [Overlay](../../src/services/platform-control/src/workflow_api.rs#L1183)、[资源申请](../../src/services/platform-control/src/resource_request_validation.rs#L103) | 全链 Draft 作用域、来源指纹 |
| 初始 Draft 为 revision 0，但没有对应 Revision 行；协议拒绝 Debug revision 0 | [创建 Workflow](../../src/services/platform-control/src/workflow_api.rs#L311)、[包校验](../../src/crates/agentx-runtime-contracts/src/package.rs#L1020) | 创建时原子写不可变 revision 0、协议接受有效的 0 修订 |

上述单草稿状态和薄授权入口已按后续各节切换；完整调用者清单见 [切换审计](cutover-audit.md)。当前调试、版本执行、导出和 Builder 入口复用 Workflow 对象权限，历史代码位置仅用于说明改造起点。

## 2. 统一 Editor Command

目标 EditorCommandV1 包含 commandId、builderRunId（AI 来源时）、editorBindingId、draftId、expectedLocalDocumentRevision、beforeHash、明确的目标范围和有序 Intent。

手动入口、Inspector、连线、复制粘贴、删除、布局和 AI 都调用统一 Dispatcher。业务语义复用当前 Store/GraphIndex/Manifest 实现并按需归拢，不能平行重写一套 AI Reducer。连接校验目前在 Flow 层，需进入命令执行路径；单纯调用 Store.connect 不足以保证合法。

下面是 AI 命令的投递/应用流程。手动编辑直接调用同一个本地 Dispatcher，不依赖 Builder Session、SSE、模型或 AI 服务可用性；两种来源共用变更语义、历史与后续保存链路。

1. Control 校验会话绑定、对象权限、Intent Schema、节点/资源身份及静态影响，持久化命令 Intent。
2. Event Stream 只把命令交给绑定的编辑器；前端校验当前 Draft、Instance、完整文档序号和 beforeHash。
3. 在一份当前文档上应用整个批次，调用已有删除/动态端口/Loop/Exit 语义；不暴露中间状态。
4. 拓扑变化或显式 `auto_layout` Intent 的画布范围命令带 `autoLayout=true`，编辑器在提交前复用 ELK 计算节点间距、Start 和 Loop 容器位置。`resultDocumentHash` 校验 Control 提供的待布局文档，ACK 的 `documentHash` 校验实际布局后的完整文档。Control 只允许坐标、尺寸和 Start 布局变化，Definition、节点身份、折叠状态、边、注释、分组和 viewport 必须与命令一致；参数修改和节点范围命令保留布局。回执持久化实际 Editor Document，重连重发同一结果。
5. 前端使用现有 createHistoryPatch 一次提交，更新 dirty 和 localDocumentRevision；用户可一键 Undo/Redo。
6. Recovery 同步保存实际文档与本 Binding 的已应用命令回执，再发送 ACK。ACK 包含 applied/stale/rejected、前后 hash、实际差异、当前序号和错误。
7. Control 校验 ACK 与命令范围、实际序列化结果及 hash，结算工具效果。自动保存另行进行，不能把 applied 写成 saved。

同一 commandId/payloadHash 重复投递只返回同一 ACK；同 ID 不同内容拒绝。重连/reload 需结合 Binding、Recovery、已保存文档和持久 ACK 对账，无法证明应用结果时标记未知/过期，不盲重放；Undo 后不能因重连自动再次应用旧命令。

当前读取基准使用完整文档序号/hash，写入范围使用冻结的 canvas 或 nodes 及实体 ID，防止节点范围请求改变无关节点、全局契约或其他 Draft。全图替换只能在用户明确要求整个当前画布时使用。返回前后的差异用于审计与 UI，不创建第二份独立 Candidate Draft。

## 3. 保存一致性

localDocumentRevision 对所有持久文档编辑递增，包括参数、布局、Start/End、注释、分组、Undo/Redo；它不同于仅反映部分结构变化的 graphRevision，也不同于服务端 Draft revision。

每次保存捕获 draftId、documentInstanceId、localDocumentRevision、expectedServerRevision 和完整序列化快照。响应更新服务端 revision；只有当前本地序号和保存快照一致，才清除 dirty/Recovery。保存期间有后续编辑时保持 dirty，并继续现有自动保存流程。

切换画布后的旧响应只能更新旧 Draft Query Cache，不能改变当前 Store、Recovery 或 AI 状态。所有 Query Key、Recovery Key、Inspector 选项、资源状态、插件解析、验证结果与保存 Mutation 均包含 Draft 身份；Recovery 同时绑定 tenant/user，防止账户切换串数据。

服务端冲突沿用现有“保留本地/载入服务端/用户选择覆盖”交互；AI 不选择覆盖，不自动重置或清空文档。保存失败不增加 revision、不重建资源引用、不宣称 AI 修改已保存。

保存/静态校验错误的 fieldPath 先在提交快照中映射到稳定实体 ID；旧响应对应的数组下标不能用于当前新文档定位。实体/字段已变化时标记诊断过期并重新验证，不把错误高亮到别的节点。

## 4. Control 数据模型

| 表/对象 | 目标改动 |
|---|---|
| workflow_drafts | 移除 Workflow 唯一键；增加名称、base_version_id、状态/创建人；tenant/workflow/id 归属约束与列表索引 |
| workflow_draft_revisions | 唯一键改为 tenant/draft_id/revision；保存 Definition/Editor 及其 hash；创建 Draft 时同步写 revision 0 |
| workflow_versions | 增加 source_draft_id、source_revision_id、base_version_id；来源快照固定；去重包含来源 Draft/Revision 和内容/布局 hash |
| workflows | 列表/详情返回草稿摘要/计数，不再通过 JOIN 唯一 Draft 返回单一 revision；不引入隐式默认草稿指针 |
| workflow_debug_overlays | 唯一身份改为 tenant/draft_id/node_id；Schema hash/stale/Artifact 语义保持 |
| workflow_debug_runs / publication | 使用 Draft Revision 或 Version 的明确来源联合；前者携带 draft_id/revision_id/revision/hash，后者携带 version_id/hash，不为版本运行伪造 Draft |
| workflow_draft_resources / 包引用 | 每个 Draft/Revision 的引用独立投影；替换只影响本 Draft，删除影响覆盖全部 Draft |
| resource_grant_requests | 申请来源增加 source_draft_id/revision；审批检查准确来源和依赖指纹；Grant 主体仍为 Workflow Service Identity |
| studio_builder_sessions | tenant/workflow/draft/user/editorInstance 身份、当前编辑快照、完整 Core State 与 CAS 修订 |
| studio_builder_runs | Prompt、editScope、冻结模型/凭据/价格/设置修订/授权 epoch、Lease/Owner/Fencing、deadline 与终态 |
| studio_builder_entries | 追加式消息、命令、ACK、问题、静态报告和单调 Cursor；索引强制顺序读取避免大 JSON 排序 |
| studio_builder_effects | 严格工具输入、命令/问题、稳定 ID、请求 hash、ACK/结果与 pending/settled/cancelled；超大结果保留在不可变回执中供分段读取 |

不另建业务 Scheduler、候选 Draft 表、长期记忆或通用任务平台。Builder 表在 Control 域；Runtime 模型来源/账本按 [02](02-architecture-and-contracts.md) 调整。表处置目录、边界检查器、备份/Retention/GC 引用与 Schema 清单同步更新。

### 4.1 原子创建与来源

新建 Workflow、导入、新建空 Draft、从 Version 派生都在事务内写 Draft 和 revision 0，计算图/布局 hash，并登记资源和包引用。版本来源必须属于同 tenant/Workflow。派生原样保留精确引用，即便当前不可用；与空编辑器相同的可编辑保存门禁和字段反馈，不静默升级依赖。

Draft 保存锁定准确 Draft；不可变 Revision 一经提交不修改。草稿归档不删除 Revision 或来源元数据；Version 和 Execution 的保留引用优先。

### 4.2 创建版本

用户请求指定 draftId/revision（及对应 hash），从该不可变 Revision 编译，不重复读取 Head。采用稳定锁顺序：Workflow 行 → Draft/来源 → 现有资源锁；同 Workflow 并发创建版本通过 Workflow 行锁分配 MAX(version_number)+1，不能只锁各自 Draft。

幂等请求 hash 包含完整来源、内容/布局 hash；同来源重复返回同 Version。不同 Draft 的相同 revision 数字不能去重为同来源。Version 保存来源 Draft、Revision ID、baseVersionId，归档不会改变关系。

## 5. 目标公开 API 与路由

所有接口遵循现有严格 DTO、Idempotency-Key、字段错误和对象权限协议。下表描述目标操作，OpenAPI 在 P8-00 由 Rust 类型派生。

| 入口 | 操作 |
|---|---|
| GET/POST /api/v1/workflows/{workflowId}/drafts | 列表；用户创建空 Draft 或从指定 Version 创建 Draft |
| GET/PUT /api/v1/workflows/{workflowId}/drafts/{draftId} | 读取；按 expectedRevision 原子保存 |
| PATCH /api/v1/workflows/{workflowId}/drafts/{draftId}/metadata | 修改名称/归档状态，不重置内容或基础版本 |
| GET /api/v1/workflows/{workflowId}/drafts/{draftId}/revisions | 当前草稿的不可变修订摘要列表；版本创建和调试服务按明确 revision 读取快照 |
| POST /api/v1/workflows/{workflowId}/drafts/{draftId}/validate | 草稿/完整静态校验 |
| POST /api/v1/workflows/{workflowId}/drafts/{draftId}/resolve-plugins | 当前配置的权威纯契约解析 |
| /api/v1/workflows/{workflowId}/drafts/{draftId}/debug-overlays | 用户 Pin/Mock 的读写/删除 |
| POST /api/v1/workflows/{workflowId}/drafts/{draftId}/debug-executions | 仅用户 UI 的精确 Revision 手动调试 |
| GET /api/v1/workflows/{workflowId}/versions/{versionId} | 版本与布局、来源、只读画布使用的精确契约 |
| POST /api/v1/workflows/{workflowId}/versions | 仅用户 UI，由明确 Draft Revision 创建版本 |
| /api/v1/workflows/{workflowId}/deployments | 现有用户 UI 的 Version 发布/回滚链路 |
| /workflows/{workflowId}/drafts/{draftId}/editor | 可编辑草稿画布 |
| /workflows/{workflowId}/versions/{versionId}/editor | 只读版本画布及“基于此版本编辑” |

资源选项、就绪检查和授权申请来源增加 draftId；共享 Catalog 查询不强加无关 Draft 参数，但任何当前配置/节点/上游契约解析必须绑定当前文档。

Builder UI API 位于 `/api/v1/studio-builder/sessions` 下，创建会话的 DTO 显式包含 workflowId、draftId 和 editorBindingId。会话子路由提供读取、发送消息、停止 Run、Cursor 事件、获取/ACK 编辑命令、回复问题、当前文档快照及用户选定的执行结果。每次访问验证会话所属用户和文档；工具仅由 Host Dispatcher 调用，不公开任意工具执行代理。

ACK/文档快照必须来自同一已认证用户和有效 editorBinding；不能拿客户端 draftId/actorId 作为授权。清空会话不提供清空 Draft API。

旧 /workflows/{id}/draft、workflow-only editor、旧 resolve/Overlay/调试来源路径及单一 revision 汇总语义直接删除；所有调用者、Fixture、生成客户端同时切换，不留自动选首个 Draft 的兼容入口。

## 6. 手动调试的精确来源

Draft Debug DTO、Work Package Spec、Source Reference、请求幂等 hash、execution snapshot、Control debug run 投影和查询均携带 draftId/draftRevisionId/revision/contentHash。revision 0 是合法的已保存快照，必须更新当前大于 0 的校验和相关 Fixture。

所有运行来源使用严格判别联合：draft_revision 保存完整 Draft/Revision 身份，version 保存 Version 身份。当前 Version 运行也写入 workflow_debug_runs 的 draft_revision，必须同步改造，不能为版本执行强行填写一个 Draft ID。来源展示、过滤、Artifact 授权和幂等均跟随真实来源；Version 的编辑祖先只是元数据，不是运行来源。

调试入口直接加载对应不可变 Revision，复用四种现有模式、输入来源、Overlay 和副作用决策。请求创建后另一草稿保存或本草稿 Head 变化，不能改变本次执行来源。Overlay 冻结到 Work Package，执行与结果按 executionId 隔离。

该 API 只属于用户 UI/正式业务调用链，Builder JWT、ToolPort 和工具 Schema 没有调用权限。用户提交运行后 Runtime 继续沿用既有执行、计量、Trace、取消与恢复机制。

## 7. 生命周期与引用保护

- Workflow 权限/归档影响所有 Draft 和 Builder；Draft 归档仅影响本草稿编辑会话，不改变 Version/Deployment。
- 节点/资源/包删除影响遍历所有草稿及不可变引用；资源申请来源定位准确 Draft/Node/Revision。
- baseVersion/来源 Draft Revision 的 FK 和 Retention 关联不允许悬空；历史版本画布加载精确包/契约，不读最新版替代。
- 对话、Snapshot 和命令保留服从所属域 Retention；未结算 Effect 和仍被业务来源引用的数据先对账，不能只按 TTL 删除。
- 新 API、公开 DTO、Work Package 生成物、前端类型、初始 DDL 和测试一次性切换；不为旧开发数据编写迁移。

资源目录的 `draftId` 查询参数与前端 Query Key 隔离当前草稿的待审/拒绝申请；跨 Workflow 的 Draft ID 返回 404。业务 Grant 属于 Workflow Service Identity，各草稿共享授权。用户直接授权后，同一 Workflow/资源/操作的待审申请都得到正式结算，保留各自的 Draft 来源。没有 Draft 上下文的目录查询只展示 Workflow 级申请，不自动选择一个草稿。

## 8. 租户模型与可靠回执

`GET/PUT /api/v1/studio-builder/model-setting` 使用 `tenant_settings.version` 做 CAS；PUT 必须显式提交 `expectedVersion` 与可空 `modelAliasId`。当前模型同时保存授权 Credential ID。写入需要三项管理权限，普通 Workflow 编辑者只消费配置。未配置、禁用或 Credential 改绑时，面板显示原因，普通保存独立可用。

每个 Run 冻结模型 Deployment、Credential Secret Version、Price Version 和 Alias/Credential 授权修订。清除设置、禁用模型或凭据会取消对应旧授权修订的开放 Run；重新启用后创建的新 Run 不受迟到撤销影响。业务资源 Grant 不由这些操作创建。

`GET /api/v1/studio-builder/sessions/{session}/commands/{command}/receipt` 对账命令的 pending/settled/cancelled。浏览器先持久化 Recovery，再发送 ACK；已应用标记和简短回执证明保留跨 reload/Undo 的去重依据。完整批次沿用一次 Undo。

超过 128 KiB 的工具结果返回有界摘要和 `studio-tool:<effectId>` 引用，全文保留在该 Session 的已结算工具回执中。`workflow_read artifact` 按字符 offset/limit 读取，最多 16000 字符；不能读取别的会话。原文随设计历史回收，未结算与 unknown 保留。设计历史和 Runtime 模型大载荷复用现有 Trace 保存期限，冻结价格、身份及账本总额继续保留。

## 2026-10-15 节点 Inspector 回归修正

InputBinding/ValueSelector 的 `run`、`item`、`path`、`missingPolicy` 按 Rust 契约允许省略；前端按协议默认值解释，读取不改写文档及 hash。草稿可缺少顶层待配置字段，但已提供的参数必须通过 Manifest 的结构校验，非法嵌套条件/Binding 不得进入编辑事务。新增所有 13 类 Action 和 Exit 的 Inspector、布局与 Markdown 回归；验收记录在测试实际完成后更新。

### 画布文档与 Builder 的无损 JSON 持久化

Workflow Draft/Revision/Version 的 definition_json、editor_json，以及 Builder Session 的 snapshot/state、Entry payload、Effect input/command/acknowledgement/result 使用 LONGTEXT + JSON_VALID CHECK。MySQL 8.4 的原生 JSON 数字转换已复现将 209.66666666666666 改成 209.66666666666663，破坏文档 hash 和 ACK 身份；这些列必须保留应用序列化后的数值。HTTP DTO 与 SQLx JSON 编解码不变，SQL JSON_EXTRACT 仍可用于标量查询。基线 DDL 直接更新，不提供旧列兼容或增量迁移。重连按当前文档 hash 和持久化快照身份恢复单调序号，不放宽命令/ACK 校验。

Builder SSE 响应显式设置 `X-Accel-Buffering: no`。编辑命令必须立即抵达前端才能获得 ACK 并恢复 Host；代理不能等待后续 token 或 KeepAlive 填充缓冲区。采用 [NGINX 官方支持的按响应关闭缓冲机制](https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_buffering)，并在真实 Web 代理后的浏览器场景检查已结束会话的最后一个小事件在 5 秒内抵达。
