# plan8 架构、Host 与契约

状态：done。接口、共享 Core 1.3 和 DTO 已实现并纳入生成契约；系统验证见实施报告。

## 1. 设计态与业务执行态

~~~text
Studio Editor Store / Builder Panel
    │ 当前文档、命令 ACK、SSE、用户答复
    ▼
Control：Studio Builder Host
    ├─ 同一个 agentx-agent-core
    ├─ ModelPort → Runtime 设计态 Model Effect API
    ├─ ToolPort → Catalog / Resource / Compiler / Editor Command
    ├─ StatePort → Control Builder Session/Entry/Run/Command
    └─ EventPort → Builder Event Cursor / Studio SSE

用户 UI 的保存、创建版本、发布、运行
    ▼
Control 正式领域 API → 不可变 Bundle / Work Package
    ▼
Runtime Gateway / Coordinator / Worker → Execution / Trace
~~~

Builder Core 不承载 Workflow 调度。设计态运行不创建 Workflow Execution、Node Execution 或 Attempt，不占用业务 Worker Claim/Lease，也不拥有业务运行工具。

## 2. ADR：Control 增加设计态 Core Host

当前 plan3 把 Core Host 放在业务 Worker，并规定 Control 不执行 Agent loop。plan8 增加设计态例外：Control 的 Studio 模块驱动同一个 Core，操作其拥有的 Draft、Catalog 和编辑器会话。

原因：AI 的编辑必须跟随当前未保存文档、用户对象权限和 Draft 修订；让 Worker 反向读取或修改 Control 会破坏生产离线边界。设计态会话有 Control 所有权，避免把编辑消息混入 Application Session 或 Runtime 业务状态。

影响与不变量：

- Core 保持基础设施零依赖，不包含 Control/Runtime 特判、SQL、HTTP、文件系统或 React Flow 类型。
- Host 实现 Model/Tool/State/Event/Clock/Budget Port，所有副作用有稳定 effectId、Intent、Settlement 和 Fencing。
- Runtime 不访问 Control DB/OSS/Repository；模型 Effect 收到精确资源快照和短期授权，在本域执行并返回 Receipt。
- Control 不直接调用外部 Provider、不执行节点或非可信代码，也不直连 Runtime DB/OSS/Redis/ClickHouse。
- 业务 Worker 和 Sandbox 的已有运行授权、快照、四工具及取消链路不因 Builder 改变。
- 不新增 Pi/Node Agent Runtime 或常驻服务构建产物；在现有 Control/Runtime 二进制内增加有界模块，独立预算与并发限制。
- Control API 的启动/恢复循环以 MySQL Lease 驱动 Builder Run；多副本无进程粘性，Drain 停止领取并对开放 Effect 对账。

实施阶段同步 docs/02、04、06、09、10、13 和 plan3/planv2 的有效边界；计划阶段只增加目标说明，不把现有事实改写成已实现。

## 3. 模型设计态 Effect

现有 runtime_calls 强制 execution_id/node_execution_id/attempt_id，现有 Model Adapter 也绑定业务 Attempt。复用 Provider、Vault、Egress、计量和结果收敛逻辑前，必须抽取与 Execution 无关的模型调用 Use Case；不能给 Builder 填伪造执行 ID。

目标为一个严格区分的调用来源：

| sourceScope | 必需身份 | 授权 |
|---|---|---|
| workflow | executionId、nodeExecutionId、Attempt/Lease | 冻结 Snapshot + Runtime Grant |
| studio_builder | builderSessionId、builderRunId、operationId、actor/Workflow | Control 签发的精确模型/凭据版本、短期能力与当前 Policy Epoch |

来源在 Rust DTO、SQL CHECK/索引、幂等键、Trace、Usage、Quota、Artifact 授权/Retention 和查询中均明确区分，不能形成同时有两类身份或两类均缺失的记录。不是仅放宽三列 NOT NULL；业务来源仍完整校验 Attempt。Builder 内的 agent_turn 与 compaction 使用相同来源，各自独立计量。

设计态 Model Effect 请求携带 effectId、请求 hash、deadline、模型与凭据版本、授权证据、Provider-neutral 消息和工具 Schema。Runtime 使用现有 ProviderHttpClient/Egress；Credential 仅在 Runtime Vault Adapter 解析。

接口支持提交/查询同一 Effect、流式增量、取消和最终 Receipt。断线后先查同一 effectId；已结算结果复用，外部结果未知按现有 Unknown 规则收敛，不发起新的重复推理。Receipt 含状态、使用量、成本、内容/Artifact 引用，Control 不自行把无 Receipt 的请求记成成功或零成本。

Builder 模型由租户管理员在模型列表统一设定，保存 Alias ID 和授权凭据 ID；设置要求 model:manage/company:manage/resource:grant。租户设置只授权设计态模型调用，不向各 Workflow 写业务 Grant。每个 Run 冻结部署/凭据/价格版本，设置变化影响新 Run；模型/凭据失效停止后续调用，换用其他凭据需要重新设置。

## 4. Core 挂起与恢复

Port 保持同步接口；浏览器 ACK 和用户答复使用通用 Pending Effect + Resume Result 契约：

1. Core 保存工具请求、完整 Operation State 和稳定 Effect 身份。
2. Host 返回 Pending 状态后释放执行槽，不用阻塞 Future/线程等待浏览器或用户。
3. Control 持久保存命令/问题，Run 处于 waiting，具体等待内容由持久 Effect 的工具类型和状态区分。
4. 已认证结果到达后提交 Effect Settlement，以原 Operation Identity 继续同一个 Core。
5. 恢复只能消费已登记的结果，不能跳过挂起位置重新从 Prompt 开始。

Pending 语义由通用 Core 状态机实现，Builder 业务 DTO/操作只在 Host；不得复制 Agent loop、增加第二套压缩或把状态藏在闭包中。P8-00 以实际源码 Spike 冻结新 Core Contract 版本及生成物，破坏性替换旧契约，不维护双版本运行。

Run 状态为 queued/running/waiting/completed/cancelled/failed/outcome_unknown。未解决的外部结果记录在 Effect 状态，不用“Run 完成”掩盖。Session 同时只有一个开放 Run；steering/follow-up 复用现有 Core 顺序和持久队列。

## 5. 状态所有权

| 数据 | 权威所有者 |
|---|---|
| 当前未保存画布、选中项、历史 | 当前 Editor Binding 的前端 Store；选择/视口不属于运行 Definition |
| 已保存 Draft/Revision、Version、引用 | Control Workflow 领域 |
| Builder 消息、压缩、运行状态、待应用命令和 ACK | Control Builder 领域 |
| Builder Model Call Receipt/Usage/Artifact | Runtime 设计态调用账本；经内部 API 返回 Control |
| 用户手动执行的状态/输出 | Runtime Execution Repository；Trace 作为补充 |

Builder 捕获的文档快照只用于特定操作的读集、hash 和静态校验，不成为另一份可保存的 Draft Head。大文档/消息使用所属域的 Artifact；Control/Runtime 不共享对象存储凭据。

Builder 成本单独显示，不能计入某次用户 Workflow Execution 的成本。消息 Entry 不进入长期记忆，不复用业务 Agent Session 表或用户可随意填写的 Application Session ID。

## 6. 权限与工具 Profile

- 每个请求校验租户、Workflow 对象权限、Draft/Version 归属、操作者与 editorBinding。复用 require_workflow_access；不只检查全局权限。
- Builder Tool Registry 固定为 [03](03-tools-and-context.md) 的七工具，Origin 使用平台工具身份，不能伪装成 MCP 或强制依赖 Sandbox。
- Builder 的 workspaceSandboxBinding 为空，因此现有 Core 不自动注册 read/write/edit/bash。业务 Agent 选择 Sandbox 后仍注册完整四工具。
- Registry、ToolPort Dispatcher、设计态 JWT 能力、业务 API 授权四层都拒绝 AI 运行/测试/发布；Prompt 不是权限边界。
- node_catalog.resolveDefinition 使用现有解析协议，目标执行环境为 Runtime 经现有 sandbox-manager/OpenSandbox 创建的固定 resolve-only 隔离 Profile：无网络、无业务资源/凭据、无宿主 Secret、无宿主桥接，只接收精确包源码、配置和上游 Schema，返回有界 JSON。顶层 import/fetch、动态模块、文件逃逸和任意 Host 方法都在隔离/授权层拒绝，不把“不传 ctx”当作安全保证。
- 静态解析 Sandbox 的租约以设计操作身份为来源，不伪造 Execution/Attempt，不开启 Builder 的 workspaceSandboxBinding。复用现有摘要/配置/upstream hash 缓存、并发槽、deadline、取消和回收；无隔离能力时报告静态解析不可用，不能切回不受限进程执行。
- 固定 Runner 与解析 JSON 经 Execd 文件上传接口传入，shell 命令保持固定长度；包源码、配置和上游 Schema 不进入命令参数。受限 Node 进程只获准读取固定 Runner，解析输入由可信启动命令重定向到 stdin，避免大图触发系统参数长度限制。
- 一次 Workflow 静态解析在同租户、同批次内复用短期 resolve-only 容器，避免 200 节点逐一创建容器造成保存超时。每个节点仍以全新受限 Node 进程、全量覆盖的独立 stdin 和一次 RPC 调用执行；不复用模块、全局变量或业务进程。批次由 Control 显式关闭，关闭墓碑拒绝迟到调用；MySQL 租约为批次串行占用和故障回收提供权威状态，空闲超过 60 秒自动销毁，容器最长生存 300 秒。跨租户、并发占用、资源或 Provider 请求均不能进入该批次。单次节点解析继续用独立容器。
- 解析专用命令流在收到完整结束事件后结算，按字节缓存跨 chunk 的 UTF-8/JSON，校验终态并限制 8 MiB 输出；缺终态、非法帧和 Host RPC 仍拒绝。不等待 Execd 在终态之后的固定尾部保活，以避免每节点多出约 1 秒；业务命令链路保留原行为。[上游命令流实现](https://github.com/opensandbox-group/OpenSandbox/blob/48b0215/components/execd/pkg/web/controller/command.go) 在写入并 flush 结束事件后再等待 graceful-shutdown window。
- resource_query.options 仅读取平台目录和用户已在 UI 获取的、带契约/配置 hash 的元数据结果，不调用通用插件 Provider。需要外部字段发现/选项时让用户通过 Inspector UI 获取，再把结果作为只读事实交给 AI；不接受插件自称 readonly 作为调用授权。
- 画布保留两个由 Control 本地数据库读取的内置选项：审批节点的用户目录和子 Workflow 的不可变版本。两者按内置节点类型、capability 和 provider 名称精确匹配；插件同名 Provider 仍禁止自动调用，不能借此启动业务 Effect。
- execution_inspect 只读取用户在 UI 交给本会话且有权查看的执行和其 Artifact，记录来源版本/草稿/修订与证据类型。
- 用户回复 ask_user 只能补充设计信息，不升级能力、不生成业务运行授权或自动点击 UI。

AI 引起的参数/Inspector 更新不能通过组件挂载或自动 Effect 间接调用测试/通用 Provider。设计时 UI Host Bridge 区分静态 resolve-only 请求和用户 UI 发起的资源操作；业务调用在前者没有能力。版本只读同时由统一命令执行器、快捷键、Start/Exit/插件面板、自动保存和 Builder Binding 拒绝，不能只禁用工具栏。

## 7. 事件、取消和容错

Builder Session Entry 使用持久单调 Cursor，记录消息、工具结算、压缩、Run/命令状态。UI-only Entry 不进入模型上下文；token delta 是即时显示，重连以已结算完整消息/当前状态校准。

Core 状态提交、命令准备、ACK 与事件写入统一按 Run → Session 获取锁。Entry 插入会检查 Session 和 Run 两个外键，因此先显式取得 Run 共享锁，再写事件，避免数据库默认的外键检查顺序与 Core 提交形成死锁。事件按稳定 event_key 去重，重复 ACK 或恢复只保留一条结算记录。

关闭面板、离开/切换画布、归档 Draft、撤销权限或用户停止都会取消 Run、失效 Binding，并传播模型取消。已应用编辑不自动回滚，旧命令不能在恢复时重新覆盖当前文档。

| 故障位置 | 收敛结果 |
|---|---|
| 发送命令前强退 | 从 Intent 恢复并对账同一 commandId |
| 前端已应用，ACK 丢失 | 恢复记录/命令回执确认；不能证明则标记未知/过期，不盲重放 |
| 用户编辑导致文档 hash 改变 | stale，重新读取并创建新命令 |
| 前端已应用，保存失败 | applied 与 save_failed 分开；继续现有保存/冲突 UI |
| Runtime 模型已结算，Host 强退 | 查询同一 Effect Receipt 后提交一次 Entry/Usage |
| 租约/Fencing 丢失 | 拒绝新效果与迟到提交，新 Owner 从持久 Operation 恢复 |
| ClickHouse/Trace 不可用 | 只读诊断注明缺失，不改变权威输出或静态检查结论 |
| Control 离线 | Builder 不可用；既有生产 Bundle 执行仍正常，无 Control 回读 |

所有重试有 deadline 和预算，用户等待不永久占用执行槽。归档/Retention 保留被版本、执行、会话和未结算 Effect 引用的数据，不孤立删除来源快照。
