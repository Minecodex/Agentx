# Workflow Definition 8.0

## 1. 版本边界

Workflow Definition 只接受 `schemaVersion: "8.0"`。本版本是开发期破坏性升级，不读取、迁移或保存旧 Definition；旧字符串表达式直接返回 `LEGACY_EXPRESSION_NOT_SUPPORTED`。

本文冻结当前唯一的Definition 8.0契约。List/Loop数组边界、Exit顺序与可选null对齐、Draft引用保存门禁均由实现和自动化测试约束；环境验收结果见[plan5实施计划](plan5/implementation-plan.md)。

plan6不改变Definition版本，但改变节点契约来源：Node Manifest当前只接受3.0。插件节点仍只持久化`type/typeVersion/parameters/resourceReferences`，Control Catalog由该身份解析不可变包版本；Compiler把完整`PluginNodeBinding`、有效端口和输出Schema冻结到IR。同一Workflow中同一`packageId`出现不同`packageVersion/bundleDigest`时返回`PLUGIN_PACKAGE_VERSION_CONFLICT`。默认版本变化不改写Definition，显式切换仅改变节点版本并由现有Undo/Redo和服务端校验处理。

## 1a. 多结束节点（exit）

8.0 不再使用画布中央的 `__end__` 边界：终止点表达为多个 `exit` 类型的真实节点，每个 exit 暴露 `main` 与 `error` 两个输入端口，可从节点面板添加、可删除；工作流初始模板自带一个 `protected: true` 的 exit（不可删除），保证任何 Definition 至少有一个终止出口。`__end__` 只作为编译/校验期虚拟锚点，Connections 中不允许出现 `target == __end__`。

输出模型分为两层：

- **全局契约**（单一真源）：字段名、JSON Schema、required 与 sensitive 存于 `end.outputs` 与 `end.error.outputs`，对所有 exit 生效；Studio 在任一 exit 上编辑字段即修改全局契约，重命名字段会同步重命名所有 exit 的映射 key。
- **per-exit 映射**：每个 exit 节点的 `parameters.outputs` / `parameters.errorOutputs` 为每个契约字段提供一个 `InputBinding`；引用校验以该 exit 自身的可达前驱为准（扩展图上 exit 作为虚拟汇点计算），错误映射引用还必须是该 exit 全部错误来源的公共前驱。每个 exit 只对实际连接的 main/error 输入要求对应 `required` 契约字段映射（编译期 `EXIT_REQUIRED_MAPPING_MISSING`）；错误专用 Exit 不要求成功映射。已填写映射仍执行完整字段与引用校验。

执行语义：连到exit的边被编译为携带`targetExit`的terminal connection。`first_return`由首个main交付决定成功输出并取消其余激活；任一error交付到exit都按对应错误映射立即失败。`all_complete`等待全部可完成分支，只聚合实际到达的成功Exit并按IR中显式保存的Definition顺序排列；每个字段数组为每个到达Exit保留槽位，可选缺失写null，必填缺失/null失败。同一`(source, sourcePort)`扇出到多个exit被编译期拒绝（`DUPLICATE_TERMINAL_FANOUT`），保证结果确定。对外API和子流程输出Schema从同一全局契约与完成模式生成。

## 2. Binding与条件

普通字段只持久化递归`InputBinding`：`literal`保存JSON标量，`reference`保存单个稳定Selector与缺失策略，`template`保存文字和Selector片段，`array/object`递归保存子Binding。`ConditionSpec`是IF和List过滤专用结构，左右值也都是`InputBinding`。

Definition、Manifest和Studio不再公开递归表达式AST。普通字段不支持算术、函数、条件嵌套或对象构造模式；复杂转换使用Code。ConditionSpec只允许与左值Schema相容的比较操作，`matches`继续使用限长Regex。

Runtime 对一次节点激活同时生成 `ResolvedParameters.common` 与按输入端口、Item 顺序展平的 `ResolvedParameters.perItem`。批量级参数消费 `common`；IF和逐Item转换必须消费与当前Item对齐的`perItem`，不能用首项解析结果替代整批。List先求值用户选择的数组，再在数组元素上下文中解析过滤和排序字段。Trace记录脱敏、限长后的解析诊断；已删除的Remote Action协议不再消费这些结构。

## 3. Selector

`ValueSelector` 包含 namespace、可选 stable `sourceNodeId`、port、run、item 与类型化 path。可访问命名空间仅为 `inputs`、`outputs`、`contexts`、`execution`、`item` 和 `loop`。Output 引用必须指向拓扑可达的前驱节点；显示名称不参与持久身份。

Studio 在字段的 `allowedNamespaces` 包含 `execution` 时显示统一“运行信息”目录，按执行信息、当前节点、工作流、触发信息、发起人、应用与会话分组。Prompt、URL、Header、Body、条件、字段映射、Context Write 和 End Output 可按自身Binding契约开放该命名空间；Credential、资源选择和固定设置不得开放。可能缺失的人工发起人、会话和外部用户使用`error/null/omit`策略。

缺失策略为：

- `error`：确定性失败。
- `null`：产生 JSON null。
- `omit`：在对象、可选 End Output 或 Context Write 中删除/跳过该成员。

## 4. 输出契约

Model 与 Agent 固定返回同一 `AiResponse`：`text/reasoningContent/structuredOutput/files/citations/usage/finishReason/partial`。`usage` 固定包含 `inputTokens/outputTokens/totalTokens/costMicros`。普通变量目录不公开 Provider message、tool calls、Agent iterations 或原始响应；这些诊断信息只进入受权 Trace，原始响应超过 16 KiB 时写入 Artifact 并由 Trace span 引用。

其他节点也必须由Manifest声明输出Schema。工具资源能力返回`text/structuredOutput/files`；HTTP返回`statusCode/headers/body/files`且`body`固定为字符串，JSON子字段必须由Code解析并声明；Code返回`stdout/stderr/exitCode/structuredOutput/files/partial`；Approval使用稳定任务/决策字段。List与Loop使用契约声明的单一数组结果字段；Loop体内通过`loop.item/loop.items/loop.index`读取当前元素、完整数组和序号，编辑器的迭代开始/结束节点及其连线仅为由体内DAG入口/出口推导的视图，不进入Definition。Set与Merge继续传递Item字段，不增加兼容别名。所有Error Port固定返回`code/message/retryable/details/sourceNodeId/nodeExecutionId`。

Compiler 将基础Schema和节点实例配置合成为`EffectiveOutputContractV1`并冻结到每个`CompiledNodeV1`；Code从`outputExample`推导结构化输出Schema，Model结构化模式使用显式Schema。Worker成功提交前必须逐端口验证，失败统一为`NODE_OUTPUT_SCHEMA_VALIDATION_FAILED`。

InputBinding解析后按目标JSON Schema执行唯一转换矩阵：string可接收标量或规范JSON文本，number/integer只接收数值或严格数值字符串，boolean只接收布尔或`true/false`，object/array可接收正确类型或严格JSON字符串。Picker区分直接兼容、运行时可转换和不可选择；实际转换失败返回字段错误并记录脱敏Trace。未声明目标Schema时保持原始类型。

## 5. 失败与恢复

引用缺失、Binding/Condition校验、Context Write、节点输出和 End Schema 失败属于确定性错误：Attempt/Node/Execution 进入不可重试终态，Lease 被释放，Worker Result 被接收并 ACK。基础设施错误回滚事务并交给 Recovery。

每个 Attempt 有绝对 `deadline_at`。过期任务不再 Claim；Recovery 将 Execution 标记 `timed_out`，Attempt/Node 标记 `timed_out`，释放 Worker Lease 并完成终态物化，随后不会重新排队。查询详情从 Attempt 或最后一个 Worker Lease 返回真实 `workerId`。

Worker 在执行、结果固化和结果提交重试阶段统一续租。心跳仅对瞬态数据库不可用执行 50/100/150ms 的有界重试；Lease 冲突和 fencing 失败立即停止，避免瞬态 MySQL deadlock 丢弃已完成结果，也不会让失去所有权的 Worker 继续提交。

Redis Stream 和 Consumer Group 丢失后由 Worker 按 capability 原地重建，并从 `0-0` 继续消费。Redis 发布失败会以 owner 和 fencing token 条件立即释放 MySQL Outbox Lease 后有界重派；重复发布由 Attempt Claim 幂等吸收，不等待 30 秒 Lease 与节点绝对 deadline 竞争。

## 6. Studio

所有可绑定字段使用SmartInput：用户可以输入固定文字、插入变量胶囊并继续拼接；输入`{{`、`/`或点击`{}`打开Picker。数组/对象使用JSON5编辑语法，变量可以作为数组元素或对象值，保存时转换为递归InputBinding。ConditionSpec使用左右一致的SmartInput和按左值Schema决定的操作符。复制粘贴使用Agentx自定义MIME保留Selector，界面不展示协议字符串或可编辑类型选择。

Picker只展示目标节点经execution边可达的上游输出；没有明确目标时输出目录为空，binding/resource边不建立数据依赖。Exit main/error映射分别使用自己的虚拟目标。断线后的已有引用保留并显示字段级错误；后端Draft保存执行同源轻量引用校验，拒绝非法引用且不增加revision。发布/创建版本仍执行完整Compiler校验。

## 独立知识检索与草稿资源缺项

`knowledge_retrieval` 是公开 Action，复用已授权 RAG 资源与 Runtime 检索协议，输入 query/Top K，输出 text/documents/files。files 为现有外部附件来源引用，不是 Artifact，不下载原件。Agent 的 knowledge 附件继续保留。草稿允许缺少资源；完整编译要求必需资源槽/选择器已绑定，错误类型和重复单选绑定仍拒绝。设计与验收见 [plan8 资源补全](plan8/08-resource-completion-and-knowledge.md)。
