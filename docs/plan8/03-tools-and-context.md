# plan8 工具与上下文闭环

状态：done。七工具已由 Host Registry 注册，严格 Schema 同时供模型和 Dispatcher 校验。

## 1. 工具入口

Builder 只绑定用户已打开的 Workflow/Draft。读取和编辑工具不接受模型任意指定 tenantId、actorId 或目标 draftId；这些身份由 Host 注入。工具 action 使用严格判别联合，禁止未知字段和通用任意 HTTP 请求。

| 工具 | action / 核心输入 | 返回与边界 |
|---|---|---|
| workflow_read | summary、document、nodes、bindings；指定节点/字段范围 | 当前本地序号、已保存 revision、Definition/Editor 摘要、契约 hash、选中项与保存状态；大图分页/按子图读取 |
| node_catalog | search、describe、resolve；节点类型/精确版本、配置、上游契约 | 参数 Schema、默认值、端口/基数、有效输出契约、字段问题、内置示例；只解析，不执行业务 |
| resource_query | search、inspect、options；资源类型/操作/过滤/分页 | 稳定引用、版本、六态授权/就绪状态和平台已有/用户 UI 已获取的元数据选项；不调用通用 Provider，不返回 Credential 内容、不测试连接或授权 |
| workflow_edit | batch；基准文档序号/hash、目标范围、Editor Intent 列表 | 持久 commandId、差异、影响范围；等待统一命令应用 ACK 后才返回实际结果，不直接保存 |
| workflow_validate | draft 或 complete；当前文档 hash/需验证的命令候选 | 结构/引用/Schema/拓扑/资源/动态契约问题，nodeId/fieldPath、incomplete/invalid/ready、校验来源 hash |
| execution_inspect | summary、node、trace、artifact；用户 UI 选择的 executionId，筛选/分页 | 实际状态、脱敏输出、错误、来源身份、Mock/Pin 与真实执行范围；只读，不启动/取消/恢复执行 |
| ask_user | 设计问题、可选候选答案 | waiting_user 与已认证答复；只补充目标/配置/资源选择，不取得执行权限 |

所有返回采用结构化结果并附可读摘要：涉及的身份/hash、稳定错误码、字段问题、是否截断、继续读取 Cursor 与 Artifact Reference。当前 Core 模型消息只投影 ToolResult.content；Host 必须把模型继续工作所需的上述结构/引用序列化进 content，不能仅放 structuredResult/artifactRefs 让 UI 可见而模型不可见。敏感值和 Secret 不进入消息、Preview 或证据。

工具重放策略：读取/静态校验可安全重试；workflow_edit 必须使用稳定 commandId 和结果对账；ask_user 持久问题只生成一次，不能在恢复时重复弹出。工具沿用当前 Core 的顺序执行，不新增工具并行机制。非 Core 工具的 action/inputSchema 由 Host Dispatcher 严格验证，不能假定现有 Core 已为七工具完成参数校验。

## 2. 与早期九工具方案的收口

- 移除 workflow_debug：实际执行、测试、取消/恢复业务运行均由现有 UI 处理。
- 移除 workflow_commit：AI 修改当前 Editor Store 后，由既有自动保存/保存按钮处理，不建立第二条保存路径。
- workflow_edit 不创建 Workflow/Draft/Version，不切换画布，不发布；从版本创建 Draft 是用户 UI 动作。
- 不注册任意 bash/业务执行 MCP/通用请求工具；Core 的基础四工具由业务 Agent 原 Profile 保留。
- 静态校验无需用户逐工具确认；权限与运行范围是后端硬限制。Diff 是可查看的编辑反馈，不引入另一套计划审批模式。

## 3. Editor Intent 的覆盖范围

命令类型直接复用 Definition 8.0 与编辑器的领域结构，由公共 DTO 生成 Rust/TypeScript：

| 范围 | 需要覆盖的修改 |
|---|---|
| 节点 | 添加、删除、重命名、类型版本、settings、parameters、resourceReferences |
| 连线 | 添加、删除、替换；显式端口和稳定顺序；动态分支与基数 |
| Binding | 递归 InputBinding、ValueSelector、ConditionSpec；不可新增旧表达式 AST |
| 边界/输出 | Start 输入、全局 End main/error 契约、各 Exit 映射与完成模式 |
| Loop | 父子归属、体内 DAG 和作用域；不持久化派生的迭代开始/结束视图节点 |
| 编辑布局 | 节点位置、注释、分组；自动布局复用 ELK，不让模型逐坐标推算 |
| 整体修改 | 用户要求重构整个当前画布时允许批量替换运行图，保持同一命令、影响检查和 Undo |

节点和连接用稳定 ID，新增 ID 在命令准备时生成并在同一批 Intent 中引用。不能以数组下标、显示名或 React Flow 内部 Handle 成为持久身份。

修改输出契约/端口或删除节点，要返回下游 Selector、Exit、Loop、资源/包引用的影响。遵循现有断线引用保留和字段错误语义，不能为得到绿色状态静默删除非法引用。

## 4. 静态编辑循环

~~~text
读取当前文档与用户目标
  → 查询权威节点/资源契约
  → 准备批量编辑命令并静态检查
  → 当前 Editor Binding 原子应用 / ACK
  → 对实际应用后的文档静态校验
  → 存在可修复问题则生成下一命令
  → 汇报变更、校验和保存状态，等待用户 UI 实际测试
~~~

校验不能自动调用节点、执行代码或测试外部连接。Code 的语法/声明 Schema 可检查，但业务行为没有执行证据时只能给静态结论。HTTP 的 body 是字符串，Model/Agent 的输出是现有 AiResponse，引用必须依当前有效 Schema 构造。

草稿校验与完整编译校验分开：允许缺少哪些配置以现有保存门禁为准，安全/身份必填与非法引用仍拒绝。手动和 AI 使用同一门禁：资源可留空，完整编译检查必需槽；sessionPolicy 仍必须显式配置，Exit 按已连接入口检查必需映射，非法结构与引用仍拒绝。

连续重复错误且文档/诊断 hash 无变化时停止修复；budget/deadline、取消、缺必要信息也会停止。停止保留已应用编辑，准确说明未解决事项。

## 5. 用户测试后的只读辅助

用户在 UI 手动执行、选择结果交给 AI 后，execution_inspect 读取权威 Runtime 状态/输入输出；Trace 只补充模型/工具内部诊断。执行属于别的 Draft/Revision 时明确标注，与当前图逐项比较后才能提出修复。

Artifact 走现有授权下载 API，不给模型或浏览器对象存储直链。Pin/Mock 输出和历史重放必须标明来源，不能汇报为真实节点已执行。结果分析后只修改和静态检查，下一次实际验证仍由用户 UI 发起。

## 6. 上下文分层

| 层 | 投影方式 |
|---|---|
| 固定约束 | 当前 Draft 范围、七工具能力、静态-only、用户创建版本/部署、数据隔离；由 Host 每次注入 |
| 用户目标 | 原始指令、明确限制、选中节点、答复；压缩保留业务意图 |
| 设计事实 | draftId、baseVersionId、localDocumentRevision、serverRevision、文档/契约 hash；以当前快照校准 |
| 按需契约 | 搜索摘要 → 精确 Manifest/有效 Schema/资源状态；不把完整 Catalog 塞入 Prompt |
| 编辑进度 | commandId、Intent/ACK、实际差异、保存状态、未解决字段问题 |
| 证据 | 静态报告绑定文档 hash；手动 executionId 绑定来源 Revision，并保留 Mock/真实来源 |

小图可投影完整结构；大图投影全图摘要、目标节点及可达上下游、Loop 作用域和全局边界，详情通过工具读取。体积大的代码/输出分页或 Artifact 化。

同一草稿人工编辑、Undo/Redo、节点契约变化、资源撤权与保存响应都会更新设计事实；变更通知使相关旧快照失效。静态报告仅对其 hash 有效，聊天摘要不能替代 Draft、权限或 Runtime 记录。

## 7. Session 与 Compaction

Session Key 使用 tenantId/workflowId/draftId/userId/builderSessionId，编辑器绑定另有 editorInstanceId。新草稿、新会话或不同用户不自动共享 AI 消息；baseVersion 不会继承原草稿对话。

复用同一 Core 的 Entry 投影、threshold/overflow 压缩、retained tail 和 steering/follow-up。Summary 为 Core 的文本摘要；当前 Draft/来源、命令 ID、文档 hash、保存状态和执行 ID 由结构化画布事实、持久 Operation/Receipt 和按需工具校准。摘要保留目标/约束与未解决问题，但不是授权或事实的权威来源。

文档和资源当前值由工具重新读取，过期 hash 不因压缩成为当前事实。不能拆散待结算 Tool Call/Result，把“未应用”压成“已应用”，或把静态通过压成运行通过。

完整历史按 Control Retention 保存；压缩只改变模型投影，不删除日志、不自动写入 Long-term Memory。压缩模型 Effect 有独立 Receipt/Usage，恢复不重复计费。Builder Panel 展示 Token/成本、进行中的工具、待用户答复、保存失败与停止原因。

## 8. 实现中的具体预算

每 Session 一个开放 Run，每租户最多四个开放设计 Run；单 Run 10 分钟、最多 32 个普通 Model Intent。模型输出上限冻结为不超过 16384 Token，上下文窗口不超过 64000 Token。文档上限 4 MiB，单批次最多 200 个 Intent，大图可通过一个 replace_document Intent 编辑。

Core 保留完整历史，投影时仅注入当前画布事实。压缩的 pending 请求冻结后复用同一 Operation ID；重新投影当前请求时保持它位于后续工具结果之前，防止恢复后反复读取。连续相同失败检测遇到成功进展即停止累计。静态报告绑定 documentHash；界面在文档变化后标记报告过期并禁用旧问题定位。

## 资源补全与错误路径追加

按 [追加设计](08-resource-completion-and-knowledge.md) 查询并匹配现有配置。resource_query.search 返回完整性标记、名称匹配及准确绑定数据；只有完整结果中的唯一可用精确匹配才提供 uniqueExactMatchId。workflow_validate 增加逐节点资源可用性、授权依赖和错误出口缺项；其检查不发起 Provider 调用。Agent 缺模型不再阻止草稿保存，完整编译通过 Manifest 必需槽检查阻断；Exit 必填映射只约束实际接入端口。AI 总结必须说明未配置、歧义、不可用或待授权资源。
