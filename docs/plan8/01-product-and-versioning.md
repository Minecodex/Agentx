# plan8 产品、画布与版本语义

状态：done。已实现多 Draft、历史版本画布及 AI 编辑；系统验收状态见实施报告。

## 1. 核心对象

| 对象 | 身份与职责 |
|---|---|
| Workflow | 业务对象、成员权限、运行身份及版本编号的共同归属 |
| Draft | 一个独立可编辑画布，拥有 draftId、名称、baseVersionId 和自己的修订序列 |
| Draft Revision | 保存后的不可变 Definition + Editor Document 快照，按 draftId/revision 唯一 |
| Version | 用户由某个 Draft Revision 创建的不可变版本，记录来源 Draft、Revision 和基础 Version |
| Deployment | 用户在某环境发布的精确 Version，不跟随 Draft 或新 Version 自动变化 |
| Builder Session | 某用户在某 Draft 的 AI 对话；不等同于 Application Session 或 Workflow Execution |

Draft 继承 Workflow 对象权限，不引入个人草稿 ACL。AI 会话按用户隔离，草稿内容由 Workflow 中有权编辑的成员共享。所有身份同时校验 tenantId 和 Workflow 归属。

## 2. 打开与编辑流程

1. 新建 Workflow 时仍创建一个初始 Draft，然后通过草稿入口打开画布。
2. Workflow 详情提供“草稿”和“版本”列表，展示草稿名称、基础版本、更新人/时间和版本来源。没有唯一 Draft 的隐式默认读写。
3. 草稿使用显式 draftId 路由。用户可同时打开不同草稿的画布；各自的选中项、Undo、Recovery、运行结果与 AI 会话隔离。
4. 用户点击 AI 图标展开对话面板，使用租户在模型列表统一设定的 Builder 模型并输入目标；画布内不选择模型。设置不修改 Workflow 节点的模型配置。
5. AI 查询节点契约和资源，生成编辑命令。命令通过统一 Editor Command 原子应用到当前画布，并形成一条 Undo 记录。
6. 当前文档变脏后按既有自动保存/保存按钮链路保存；保存失败沿用字段错误和冲突反馈。AI 无独立服务端保存工具。
7. AI 只汇报静态检查结果和实际保存状态；没有运行证据时不显示“已测试”“运行成功”。

AI 面板复用现有 Panel/Drawer、Composer、状态、Diff 与错误定位组件。画布、Inspector、AI 面板避免互相遮挡，宽度/布局使用现有桌面设计令牌，覆盖 1440×900 与现有最小桌面宽度。所有产品文案提供中英文、浅深主题和键盘焦点路径。

## 3. 基于历史版本继续编辑

~~~text
Workflow
├─ V1（不可变）──创建草稿── Draft A / baseVersionId=V1
├─ V2（不可变）──创建草稿── Draft B / baseVersionId=V2
├─ V3（不可变）
└─ Draft A revision 7 ──用户创建版本── V4（来源 A/7，基础 V1）
~~~

- 版本列表支持“查看画布”“基于此版本编辑”和选择两份版本查看差异。版本画布复用渲染器，所有变更、保存和 AI 编辑入口均只读。
- “基于此版本编辑”由用户 UI 操作创建独立 Draft，同时要求源 Version 可读和目标 Workflow 可编辑；复制精确 Definition、Editor Document 和节点/资源版本引用，不复制 Debug Overlay、执行结果、Undo 或 AI 历史。
- baseVersionId 在 Draft 创建时固定。要更换基础版本就创建新 Draft，不提供覆盖其他草稿的恢复动作。
- 打开历史版本上的 AI 入口只能引导用户点击创建草稿；完成后在新草稿发起会话。模型不能调用派生草稿 API。
- 新 Draft revision 从 0 开始，其他 Draft 的 revision 不改变。创建版本后 Draft 保持可编辑，不切换其基础版本。
- Workflow Version 编号在同一 Workflow 全局单调递增；从 V1 派生的版本可成为 V4。编号顺序和基础来源分别展示。
- 历史节点/资源不可用时保留原引用并展示问题；不静默替换为当前默认版本。能否保存、创建版本和运行按现有权限及校验门禁判定。

## 4. 保存、创建版本与发布

保存仍采用当前自动保存和立即保存路径；不增加 AI 自动保存模式配置。统一命令应用和保存是两种不同状态，面板准确显示“已修改、等待保存”“已保存至修订号”或失败。

创建版本是用户 UI 操作：选择当前 Draft 的精确已保存 Revision，完整编译与资源校验后创建 Version。请求携带 draftId/revision/contentHash/editorHash；版本编号使用 Workflow 行锁串行化，重复请求返回同一结果。

发布到环境是另一项用户 UI 操作，只消费选定 Version。AI 会话、编辑、静态校验、创建新草稿以及新版本，都不能自动改变 Deployment。

草稿可按现有实体归档模式归档/恢复；归档后画布只读并取消其 Builder Run，不删除来源 Revision。已有 Version/Execution 来源、资源和包引用按既有引用保护保留，归档不等于解除引用或改变生产部署。

## 5. AI 与手动操作的一致性

- 普通手动操作与 AI 使用同一命令和历史实现，不允许 AI 直接 setState、hydrate 整图或写 Draft 表。
- 节点/边身份稳定；AI 重命名节点不改 Selector 身份。动态端口、Loop、保护 Exit、断线引用遵循现有编辑语义。
- 用户可在 AI 工作时手动编辑；命令基于 localDocumentRevision 和文档 hash。过期命令拒绝并重新读取，不自动覆盖。
- 切换草稿、离开画布或关闭 AI 面板时取消当前 Builder Run，保留已应用编辑与会话记录；迟到命令被 editorBinding/fencing 拒绝。
- 短暂网络断线暂停新的编辑效果，重连后对账 ACK 和当前文档再继续；不在没有有效编辑器绑定时后台写草稿。
- 清空对话只改变 AI 上下文，不清空画布、Undo、版本或执行记录。

## 6. 所有实际测试由用户 UI 触发

AI 不运行 Workflow、单节点、Code、MCP/HTTP/Knowledge/Memory 操作，也不测试模型或凭据连接。即使用户在聊天中要求运行，AI 仅提示对应 UI 入口；不能通过通用请求、Provider、shell、恢复任务或 ask_user 绕过。

允许的自动计算仅为 Builder 的模型推理/压缩，以及平台静态 Schema、拓扑、权限、契约解析。resolveDefinition 的目标入口必须在无网络、无凭据/业务数据的隔离环境中执行纯解析，不属于节点业务执行或测试。resolveDefinition 进入独立设计租约和固定 resolve-only Sandbox。其进程环境为空、网络默认拒绝、Node 文件访问仅开放 Runner；Host 桥接帧会被拒绝。逃逸反例和生产 CNI 门禁分别记录。

用户手动运行后，可从结果面板选择“让 AI 分析”或把有权读取的 executionId 带入会话：

~~~text
用户要求编辑 → AI 修改 → 静态校验 → 编辑器保存
  → 用户点击运行 → UI 展示实际结果
  → 用户让 AI 分析 → AI 只读检查/修复 → 静态校验/保存
  → 用户再次点击运行
~~~

人工审批、Mock/Pin、副作用决策和实际执行结果仍由现有运行 UI 处理。模型不会替用户点击审批或运行按钮。

## 7. 完成与阻塞反馈

一次 AI 请求完成时展示：改动的节点/连接/契约、静态问题、保存状态、缺失资源及建议的手动验证步骤。静态无错误的展示名称为“静态校验通过”。

缺权限/资源/业务输入时以字段问题和 ask_user 收敛；不自动授权、不编造资源 ID、不把未知输出声明为 any。达到预算、重复无进展或被取消时保留已经应用的编辑，说明剩余问题，不伪称整体完成。
