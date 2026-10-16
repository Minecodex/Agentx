# plan8 历史验证记录（r7–r33）

本文件保留各轮当时的结果与诊断，其中的“当前”“最终”“待验证”不代表最新状态。最新候选、未关闭项和验收结论以 [实施报告](implementation-report.md) 为准。失败记录不会因后续通过而改写。

# plan8 实施与测试报告

状态：in_progress。P8-00–04 已实现；26 项原有功能场景和 3 项模型设置场景已通过实际系统测试。完整仓库回归仍有未关闭项，P8-05-03 保持未完成。只有带实际成功证据的项目标记 done。

## 已实现的代码

- 多草稿、原子 revision 0、不可变版本来源和编号锁、历史版本只读画布、版本派生。来源归属由复合外键保护；版本保存精确 Manifest。
- 每画布独立 Store、完整文档序号/JCS hash、共用编辑事务、单批 Undo、保存竞态保护、Recovery 与可靠 ACK。
- 同一 Core 1.3 的 Control Builder Host、持久 Pending/Resume、租约与恢复、Runtime 设计态模型流式 Effect、取消、Receipt、价格/Quota。
- 七工具、当前画布/选中节点范围、静态校验、只读执行证据、持久问答、超大工具结果的分段读取。
- 租户模型设置、三权限交集、CAS、冻结凭据/价格、失效说明、撤销和删除保护。

代码入口：Control `studio_builder_*`、`workflow_drafts`、`workflow_revision_resources`；Runtime `builder_model`、`sandbox_resolve`；Web `workflow-canvas`、`builder-panel`、`editor-store` 和 Models 页面。

## 分层检查

| 检查 | 状态 | 证据 |
|---|---|---|
| 完整 `cargo xtask check` | done：Rust 累计 623 passed、0 failed、2 个既有 ignored；前端 445 passed / 94 文件；插件模板 3 passed；架构/部署验收 146 passed | `xtask-full-r33.log` / `xtask-r33-summary.json` |
| Clippy、契约漂移、边界、行数、Helm 和构建 | done：包含在最新完整检查中，浏览器及前端 TypeScript 检查通过 | `xtask-full-r33.log` / `browser-types-r32-followups.log` |
| 真实 MySQL/Redis Runtime 集成 | done：显式随机端口映射，28 秒通过；无自定义端口重试 | `runtime-slice-r32-explicit-mapping-final.log` |
| 部署/测试编排检查 | done：149 passed，包含本地 TLS 转发及异常退出清理；Ruff 与 diff 检查通过 | `acceptance-r33-forward.log` / `harness-r33-forward.log` |
| 大 Trace/结构化 XML 脱敏 | done：5 passed，含 390KB 长文本的 2 秒上限及 JUnit 结构、属性/正文凭据脱敏 | `redaction-xml-r32.log` |
| 统一候选镜像 | done：14 个镜像全部构建、导入，逐个核对 revision 与同一源码指纹 | `all-images-r33.log` / `image-source-r33.json` |
| 节点桌面 UI 与截图审查 | done：十三类节点流程通过；三张基线逐张检查，保留原 0.002 差异阈值；完整回归关闭更新 | `e2e-r32focus.xml` / `snapshot-review-r32.json` |
| 完整非压测系统回归 | 已完整执行，未收口：131 passed、5 failed、0 errors；136 selected、1 capacity deselected，未使用筛选或截图更新 | `e2e-r33full.xml` / `e2e-r33full-summary.json` |

## 系统测试记录

通过 `pytest tests/e2e` 创建独立 Control/Runtime/Dependencies Namespace 和空库，复用现有部署/Doctor、Provider Fixture 与 TypeScript Playwright。源 `glm-5.3-flash` 的连接、价格和凭据仅复制到测试租户，Secret 保持内存处理。

- `p8-r7-20261014`：真实模型与 Fixture 的主流程均通过，首次完成真实模型创建、局部编辑、Undo、保存重开及 UI 手动运行。
- `p8-r9-20261014`：主流程 2/2 通过；扩展浏览器 14/18 通过。保存期间继续编辑、压缩和两类强退恢复已验证。新大图原文读取暴露 JSON 排序内存问题，已修复后复测。
- `p8-r10-20261014`：3/5 Python 场景通过，主流程 2/2、浏览器矩阵 15/18；设计账本回收和 Control 离线通过。大图/原文读取通过。Core/ACK 并发锁顺序和 Resolver 部署配置已修复，继续 r11 复测。
- `p8-r11-20261014`：部署阶段遇到 NodePort 与源环境冲突，应用场景未执行；已清理并更换独立 Run ID。
- `p8-r11b-20261014`：4/6 Python 场景通过；主流程 2/2、浏览器矩阵 18/18。压缩、Host 两副本接管、Gateway unknown、并发保存和三个设置场景全部通过；未结算模型删除保护也通过。Resolver 正例超时和故障后的外部端口转发恢复继续定位。
- `p8-r12a/r12b`：隔离环境的 egress/execd 镜像未缓存，Provider 创建阻塞；补齐原版本镜像，所有下载校验 digest，未修改源 OpenSandbox 配置。
- `p8-r13a/r14a`：Provider 能创建和清理，但纯 Resolver 被命令协议拒绝；已定位固定镜像 Node 路径及 Execd 去掉 stdout 换行导致相邻 JSON 无法解析，修复并补回归，等待 r15 系统复测。插件历史修订用例依赖 Resolver，同轮尚未通过。
- `p8-r15-20261014`：Python 5/7 通过；真实 GLM 与 Fixture 主流程 2/2，包含真实模型四轮大上下文、实际 threshold 压缩、多轮修改和手动运行。浏览器矩阵 17/20，overflow、跨会话拒绝、四个并发模型调用和准确历史 Version 运行通过。纯 Resolver 正例通过；桥接反例误用被截获的日志通道，已改为真正 RPC 帧。账本、Control 离线和插件历史保护通过。Loop Fixture、Mock 输出落库及拖动 p95=53.4ms 三项已修复，等待复测。
- `p8-r16full-20261014`：完整 135 项用例执行完毕，72 passed、22 failed、41 项准备失败，另有 1 项环境恢复报错。plan8 主流程 2/2（包括真实 GLM 四轮压缩）、矩阵 17/20、Resolver 六个正反例通过；保存/删除竞争、账本、Control 离线、未结算删除保护和插件历史通过。新增 Loop 自动保存、资源查询与大图门槛继续复测。源 LightRAG 已用相同不可变 digest 的缓存引用恢复。
- `p8-r17tlsfull-20261014`：完整 136 项回归为 76 passed、17 failed、43 errors、0 skipped。TLS/凭据 UI、渠道、模型设置三个场景、四种调试/Pin/Mock、版本来源、草稿资源申请隔离、保存/删除竞争、Host/Gateway 恢复和本地 11 镜像签名/证明/负信任校验通过。43 项准备错误中，12 缺 CPU embedding 镜像配置、13 缺 Kimi Vault 前提、16 被第三方 Provider 镜像就绪阻塞、2 依赖 Sandbox。矩阵为 17/20；实际发现 ACK Entry 外键顺序导致的 Core 提交死锁，已统一 Run → Session 锁顺序并通过真实 MySQL 回归。GLM 第十次模型连接失败；失败保留，不能算作本轮真实模型通过。
- `p8-r18final-20261015`：本地 TLS proxy 与 Docker runtime 设置在部署校验阶段不兼容，17 项均未进入应用测试，无 Namespace/数据创建，不作为功能证据。
- `p8-r18b-20261015`：17 项中 13 passed、4 failed。真实模型创建/局部修改/Undo/重开及两轮长上下文通过，第三轮压缩后失败；快照已落库，事件读取改为权威 JSON，并批量持久化流式 delta。矩阵 18/20，Loop 全图修改与自动保存通过，大图功能通过但 p95=63.5ms 超过 50ms。Resolver 六个正反例、资源申请/跨租户边界、历史插件保护、插件模板和五项真实 Sandbox 场景通过；两个插件浏览器入口发现旧测试 helper 参数数量错误，已修正。
- `p8-r19-20261015`：12 项中 10 passed、2 failed。主流程 2/2（真实 GLM 四轮压缩与手动运行）、矩阵 20/20、Resolver、账本、历史/删除、资源申请、跨租户和动态插件链全部通过。300 节点 median FPS 116.3、p95=40.1ms。失败为离线 Agent Fixture 漏了发布环境步骤，以及插件历史测试点击不可见的 SVG 容器。生产代码的锁/压缩/流式/布局改动均已得到成功系统证据。
- `p8-r20-20261015`：离线 Workflow/业务 Agent 通过，真实业务 Agent 模型调用 1 次。版本由 Playwright 点击 UI 创建/发布；临时环境已清理、源服务已恢复。插件用例的独立 Label 文本定位改成实际可访问输入框。
- `p8-r21-20261015`：插件导入、静态控件编辑、版本/运行和历史 Trace 浏览器主流程通过；Python Trace 降级助手的另一个旧签名已修正。
- `p8-r22-20261015`：插件版本/历史 Trace 系统用例通过，包含 ClickHouse 停止后的降级 UI 和恢复 UI；临时 Namespace 和进程已清理，源服务原副本已恢复就绪。
- `p8-r23-20261015`：五项未关闭回归中四项通过：插件文件/并发/Trace/清理、插件崩溃/超时/进程树、真实 stdio MCP、Redis 丢失重建。前端用例发现未选择草稿时的顶部“打开画布”错误自链接和旧路由断言，已修复并通过组件测试，等待新镜像系统复测。Namespace 与进程已清理，源服务恢复就绪。
- `p8-r24embed-20261015`：真实 CPU embedding 两项系统测试通过，验证 512 维非恒定向量、中文语义排序和无效凭据拒绝；临时 Namespace 已清理。原缺失的 CPU 镜像前提已准备。
- `p8-r25-20261015`：收集 137 项完整回归，在执行到 38 passed、4 failed、8 errors 时主动中断修复，不能视作完整回归。TLS Doctor/恢复演练、真实 GLM 流式/取消/评测/成本、CPU embedding、Mem0 CPU、Provider 恢复及多个浏览器场景通过。实际发现大图 Resolver 的进程参数超长，以及用户权限投影错误地用发布包 epoch 排序、使后续新 Token 仍返回 403；为避免该权限错误造成后续级联失败而停止。三组临时 Namespace 已清理，源服务已恢复。
- `p8-r28-20261015`：42 passed、2 failed、5 errors 后切换新修复镜像，属于中断的诊断回归。Insights、跨部门审批/撤权、流式顺序和 Debug delta Cursor 均通过。200 节点保存超过 60 秒，已确认逐节点创建容器每次约 4 秒；GLM 省略 stream 参数时默认返回 SSE，造成 LightRAG 非流式 SDK 准备失败，并连带影响记忆和 Agent UI。显式 `stream=false` 和 Mem0 现有结构化适配器的真实 GLM 探针通过，业务链路继续复测。第四个 CPU/Provider Namespace 及三组应用 Namespace 均已删除，源 12 个 Deployment 恢复就绪。

r25 后的修复与待验证边界：Resolver 使用现有 Execd 文件上传协议传入源码和 JSON 输入，命令只含固定路径；180KB 输入真实 Sandbox 探针通过，需新镜像系统复测。Runtime 用户投影按 `token_version` 排序，保留 admission epoch 最大值，真实 MySQL 集成回归验证不同 epoch 域、禁用及旧快照不可恢复权限。测试同时修正 Insights 异步投影等待条件、资源查询缺 Draft ID、Provider workspace 非法连字符、重复 Application slug 和渠道缺输入映射。以上修复尚不能代替完整系统复测成功证据。

r29 在同租户、同一次 Workflow 静态解析内复用短期容器，每个节点仍以全新受限进程执行。真实 MySQL 回归已验证串行占用、不同租户拒绝、关闭墓碑、失败清理、空闲回收和迟到子操作。r29/r29b 实际系统测试中，180KB 源码、进程全局变量隔离和六类静态隔离正反例均通过，但 200 节点保存仍超时；数据库显示每个子解析约 1.14 秒，60 秒内只完成 49 个。已定位 Execd 完成事件后的 1 秒尾等待，并在 r30 改为收到完整终态事件即返回；不能用本地回归代替新镜像的实际保存验证。证据脱敏同时覆盖 pytest 截断 fixture 容器，避免泄露无法按完整 JWT 识别的凭据尾部。

r29b 的真实 LightRAG 索引/检索、缺少 appSession 的调试拒绝和索引文档删除三项通过。真实 Agent 记忆写入因 GLM 默认 SSE 与 Mem0 非流式调用不匹配失败；两个记忆流水线场景被重复 Application slug 阻断。r31 启用固定 Mem0 镜像中已有的结构化适配器，以显式非流式请求对接该模型；测试应用的 slug 使用 Workflow UUID，独立模块显示名区分。源 Mem0 镜像、配置和模型连接未修改，以上修复继续系统复测。

r31focus 已结束：5 passed、1 failed、1 error。200 节点保存实际耗时 39.3 秒，原 60 秒超时未放宽；199 个子解析全部完成、批次关闭、无活跃租约，业务执行数不增加。180KB 源码、两个全新进程的全局变量隔离、网络/模块/宿主桥接/文件/业务请求拒绝共 8 项检查通过。真实 GLM 的 LightRAG 索引/检索、Agent 记忆写入与跨会话召回、缺 appSession 拒绝和文档删除通过。后两项记忆流水线在发布/文档上传时被 MinIO `507 Insufficient Storage` 阻断，不能算通过。只清理本次可重建的旧候选镜像和已确认的 Agentx Rust 构建缓存，将节点可用空间恢复到约 11GB；源 12 个 Deployment 恢复就绪。完整非压测 r31full 继续重跑这些场景。证据：`e2e-r31focus.xml`、`e2e-r31focus-summary.json`、`own-image-cleanup-r31.json`、`own-build-cache-cleanup-r31.log` 和 `.local/artifacts/e2e/p8-r31focus-20261015/`。

r31full 为中断的诊断回归：真实模型的不同应用/namespace 记忆隔离、改绑及恢复已通过。实际发现两个测试隔离问题，仍需修正后复测：错误 CA 注入修改了在线 SQL 客户端共用 Secret，恢复挂载传播期间影响后续插件和渠道测试；现改为只给 Doctor 指定独立的错误公开 CA，不修改在线 Secret。模块级文档 Fixture 在删除平台源记录后再次上传相同内容，但该删除语义明确保留外部索引，LightRAG 因内容重复拒绝；现为不同模块提供独立文档内容，事实与重复上传断言保留。相关失败证据保留，不能标记全绿。

r31full 的后续审计与 r32 待验改动：共享资源的 Control Token 只有 15 分钟有效期，增加模块/用例边界重新登录；Mem0 30 分钟 JWT 改成服务原生 API Key，使用既有 `api_key` 类型发送 `X-API-Key`。Rust Fixture 写 Vault 改用现有 CA 客户端构建器。浏览器 Sandbox 用 Python 已验证的准确镜像 digest，删除写死的旧 digest；共享初始化公司名与桌面用例统一，角色创建后通过现有搜索定位。Redis 拒绝计数改为准确 Application，排除其他定时应用的正常调用；真实模型费用按实际配置价格核算；Fixture 的大 Trace 载荷覆盖实际 SSE 路径，9 项 Fixture 回归通过。设计态画布恢复审批用户的 Control 本地元数据读取，精确允许内置审批 `users` 和子 Workflow `workflow_versions`，插件 Provider 仍禁用，3 项新前端回归通过。新增认证恢复行为回归后架构/部署验收 145 项通过。以上源码修正需重建候选并重新系统验证，不能用 r31 的旧镜像证明；r31 的画布 AI 主流程已再次通过，包含真实 GLM 多轮压缩，扩展矩阵继续执行。

2026-10-15 用户确认压测暂缓。最终收口范围为完整非压测 136 项，容量认证记为 deferred，不修改冻结硬件或阈值，不把未执行的压测标记通过。

r17 的 Sandbox 失败已确认：现有 OpenSandbox 0.2.2 Docker runtime 不支持 secureAccess；本地 TLS proxy 的部署规则要求该选项。保留原部署规则和源 Sandbox 配置，Docker 功能验收使用已有本地预设；TLS Doctor/凭据验收与 Sandbox TLS 组合的完成状态分别记录。压缩事件改用 Core 的不可变 compaction snapshotId 去重，不使用随机事件键。

前轮真实模型失败原因包括严格工具 Schema 在 Provider 中的兼容、连线序号归一化、压缩投影顺序；均修复后有成功复测。测试中错误的调试字段、过期命令等待条件和半成品保存假设也已更正。失败证据保留，不以静态结果替代系统成功。

## 逐项验收

26 项原始要求见 [验收矩阵](05-e2e-acceptance.md)。三个新增模型设置场景为 P8-MODEL-01/02/03。以下逐项状态来自 r19/r20 的实际成功证据；全量系统回归仍存在独立未完成项，不能将本表等同于全仓库全绿。

| 场景 | 当前验证 | 状态与证据 |
|---|---|---|
| 01 | 模型列表 UI 设置，真实 GLM/Fixture 创建，AI 期间业务执行数 0 | done：r19 主流程 |
| 02 | 局部改名/绑定保留，无关节点/布局不变，单事务 Undo/Redo | done：r19 主流程/矩阵 |
| 03 | 全图分支、Code 与 Loop 静态修改和自动保存，不运行代码 | done：r18b/r19 矩阵 |
| 04 | V1/V2 派生独立草稿、多窗口文档/缓存/资源/会话隔离 | done：r19 矩阵 + 草稿申请隔离 |
| 05 | 历史版本只读、键盘拒绝修改，准确旧版本仍可手动运行 | done：r19 矩阵 |
| 06 | 版本派生 revision 0，未修改直接 UI 运行/创建版本，完整来源 | done：r19 矩阵 |
| 07 | 两个 Draft 并发版本编号、重复来源幂等、部署固定旧版本 | done：r19 矩阵 |
| 08 | 保存中继续编辑，旧响应保留 dirty/Recovery，重开保留后续修改 | done：r19 矩阵 |
| 09 | 人工编辑后旧命令 stale，重新读取并生成新命令 | done：r19 矩阵 |
| 10 | 同 Draft 双窗口冲突由 UI 选择载入，AI 无覆盖动作 | done：r19 矩阵 |
| 11 | ACK 丢失、重连/reload、Undo 后不重复应用，同 ID 去重 | done：r19 主流程/矩阵 |
| 12 | 停止/关闭/归档取消，迟到结果拒绝，已应用修改保留 | done：r19 矩阵 |
| 13 | 问答/未知工具/运行诱导不增加执行、尝试、版本和部署 | done：r19 矩阵 |
| 14 | UI 手动运行后选择真实结果供 AI 只读分析修复，无再次执行 | done：r19 主流程/矩阵 |
| 15 | 四种 Debug、准确 Revision、独立 Overlay、实际 Pin/Mock 回放 | done：r19 矩阵 |
| 16 | 跨用户 Session/Entries/Receipt/有效 ACK、跨对象/租户拒绝，只读不能派生 | done：r19 矩阵 + 跨租户九项 API 拒绝 |
| 17 | 禁用凭据取消、替换凭据需重设、资源失效、保存失败不增 Revision | done：r19 矩阵 |
| 18 | Exit 保护、非法端口、缺 Loop parent、下游引用及合法 Loop | done：r19 矩阵 |
| 19 | 真实 GLM 四轮长上下文压缩；Fixture threshold/overflow；快照/使用量去重 | done：r19 主流程/矩阵 |
| 20 | Host 强退、两副本接管、Gateway unknown 不重发，真实 Receipt 单次结算 | done：r19 矩阵/故障日志 |
| 21 | Token/非零成本、Quota、分段原文、Retention、无伪业务执行 | done：r19 账本/矩阵 |
| 22 | 纯 Resolver 与五类逃逸反例、无网络/资源/宿主桥接、设计租约归零 | done：r19 Resolver 六例 |
| 23 | Control 离线：Builder 不可用，已发布 Workflow/业务 Agent 完成；真实模型调用 1 次 | done：r20，UI 发布 + Gateway/数据库证据 |
| 24 | 中英文、浅深主题、1280/1440 桌面、焦点和错误定位 | done：r19 矩阵 |
| 25 | 150/300 节点 Undo/重开/原文，四个设计 Run；300 节点 FPS 116.3、p95 40.1ms | done：r19 矩阵；生产容量认证独立未关闭 |
| 26 | 归档恢复、历史资源/插件与 unknown 删除保护、保存/删除竞争、草稿申请隔离 | done：r19 矩阵/生命周期/资源 API |
| MODEL-01 | 租户共享、三权限交集、编辑者能用而不可管理、只读不能用 | done：r19 矩阵 |
| MODEL-02 | 未配置/清除/禁用/删除保护；普通手动编辑和保存可用 | done：r19 矩阵 |
| MODEL-03 | CAS 冲突，新旧 Run 各冻结模型/部署/凭据/价格，无备用切换 | done：r19 矩阵 |

## 环境与限制

最终检查确认没有本次 plan8 测试 Namespace 遗留，三个源 Namespace 的 12 个 Deployment 均恢复为 desired=1、ready=1；本次 pytest、端口转发和故障进程已退出。源 `glm-5.3-flash` 的 Alias、Deployment 和 Credential 仍 active，连接状态 healthy。源库未运行新 Schema 初始化，模型和凭据配置未修改。证据：`.local/artifacts/plan8-20261014/final-environment-verification.json` 与 `final-process-verification.json`。

源 LightRAG 恢复时，节点本地缓存缺少全限定镜像名的拉取记录；针对新 Pod 使用同一不可变 digest 的短名称缓存引用恢复启动，Deployment 模板仍保留原始 `docker.io/agentx/lightrag@sha256:374faec635d382ac64b2c0c9e62543a9925b6363616547822f7325a229b0cbbd`。未更换镜像内容或修改源服务配置。各轮记录保留在 `source-restore-r17.log` 至 `source-restore-r22.log`。

完整 r17 回归的 17 个失败用例中，15 个已在 r18b/r19/r22/r23 得到成功复测。剩余 `test_frozen_capacity_matrix` 与 `test_workflow_creation_and_chunk_recovery` 等待收口；后者已修复代码和测试，尚需新镜像系统证据。原 43 项准备错误涉及的 CPU embedding 镜像已构建并有真实向量证据；真实模型 Fixture 已共用现有 `glm-5.3-flash` 连接、当前 Vault 版本及价格，移除 Kimi endpoint/Secret Ref 硬编码；四个第三方 Provider 镜像已完整缓存。完整复测完成前，这些准备改动不能视作相关业务用例通过。

TLS Doctor/凭据流程有 r17 成功证据；当前 Docker OpenSandbox 不支持 secureAccess，因此生产 Sandbox TLS/CNI 隔离与容量门禁仍未认证。r17 镜像签名/证明检查只覆盖该轮统一源码镜像；r19 复用了未改动的 Runtime 镜像，未将混合源码镜像声明为统一发布包。

本地 OpenSandbox Fixture 提供可重复故障并运行真实隔离进程；生产 CNI、生产 Sandbox 和容量发布门禁不由该 Fixture 替代。150/300 图交互与四个设计 Run 的预算检查也不等同于生产容量压测。既有 Plan3/PlanV2 未关闭门禁保持原状态。

最终证据索引：完整检查 `xtask-full-r19-final.log`；全仓库回归 `e2e-r17-summary.json`；功能矩阵/真实模型 `e2e-r19.xml` 与 `.local/artifacts/playwright/helm-agentxctl/p8-r19-20261015/`；Control 离线 `e2e-r20.xml` 与 `.local/artifacts/e2e/p8-r20-20261015/studio-control-offline.json`；插件降级/恢复 `e2e-r22.xml`；完整回归失败的后续状态 `final-validation-summary.json`。未注明目录的文件位于 `.local/artifacts/plan8-20261014/`。

最新诊断状态：r31full 在 Trace 脱敏正则的二次回溯期间停止，终端为 79 passed、20 failed、3 errors、1 deselected，不能作为完整非压测回归通过证据。浏览器主流程 2/2、扩展矩阵 20/20、资源加载 4/4、离线发布 1/1 均通过；矩阵清理阶段有一次 Kubernetes API 超时。200 节点保存 40.2 秒、199/199 完成，旧全库运行计数因其他定时应用递增而失败。离线业务运行、设计账本、未结算删除、历史包、保存/删除竞争、草稿资源申请和租户边界均通过。Trace URL 脱敏已改成在 Scheme 边界匹配，390KB 长文本回归与 XML 字段脱敏 5 项通过；31 份历史 JUnit 结构已校验。Native Docker 测试容器改用 testcontainers 的显式随机主机端口映射，移除自定义重试；不延长业务执行超时。临时 Namespace 已删除、源 12 个 Deployment 恢复就绪。等待 r32 统一镜像和最终完整 136 项复测。

追加检查：显式随机端口的真实 MySQL/Redis Runtime 集成通过（28 秒），架构/部署验收 146 项通过。证据脱敏按 XML 字段处理，保留 JUnit 结构及失败状态；历史 31 份报告已通过解析。r32 的完整检查和 14 个统一镜像已通过；完整 136 项非压测系统回归仍在运行。

最新重点复测 `p8-r32focus-20261015`：3 passed、1 failed，耗时 839.87 秒。真实附件主链、真实 GLM 知识/记忆到 Python 流水线和十三类节点 UI 通过。TLS 负例修改本地 values 未改变 Helm 已保存的测试 Hook，故注入未生效；现用相同 Chart 渲染独立错误 CA 的 Doctor Job，不改在线 Secret，再由正常 `agentxctl doctor` 验证恢复。修正后的实际结果由 r32full 给出，不能把旧负例标记通过。临时 Namespace 已删除、源服务已恢复。

`p8-r32full-20261015` 已完整执行，5492.53 秒，122 passed、11 failed、3 errors、1 capacity deselected。plan8 的 10 个 Python 场景全部通过，其中真实模型主流程 2/2、扩展浏览器矩阵 20/20；Host/Gateway 两次强退注入完成、无注入器错误、设计模型并发峰值为 4。200 节点保存/静态解析耗时 41.27 秒，199/199 动态节点完成。离线业务执行、设计账本、未结算删除保护、历史包、修订删除竞争、草稿资源申请及租户边界均通过。TLS 错误 CA 负例及恢复、11 镜像签名与负信任、备份恢复、持续探测升级和未来协议拒绝也通过。临时 Namespace 已删除，原服务恢复就绪。

本轮未关闭项包括：Redis 故障后的连接恢复、Sandbox 代理 TCP 连接、一次真实模型 SSE 中断，以及旧 UI 路由/分页/状态断言、共享测试令牌和全库会话计数带来的测试失败。旧 Echo 压缩 Fixture 仍读取已移除的模型 metadata，已调整为识别 Core 的显式摘要请求，尚需重建镜像及系统复测。已修改的测试不能反向改变本轮失败结论；P8-05-03 继续保持未完成。

r33 修复待系统验证：Redis SDK 连接管理器增加每次握手的 5 秒超时、TCP keepalive 和 Linux 未确认传输超时；真实 Redis 加透明 TCP 中继的回归在旧实现上 15.58 秒超时失败，修复后 7.07 秒通过。流式模型 HTTP 拒绝改走统一 Provider 回执与错误分类，保留 SSE 解析错误的独立分类。`agentxctl --run-id` 保留配置的 Sandbox 代理主机，只分配独立端口；IPv4/DNS/IPv6 回归已加入。旧 Echo 压缩 Fixture 的 10 项单元测试通过。`p8-r32egress-20261015` 定向复测确认 IPv4 连接拒绝和 IPv6 地址错误，临时环境及源服务已清理恢复；新候选使用当前节点地址验证代理路径，尚不能标记系统通过。

`p8-r33focus-20261015`：3 passed、1 failed，705.67 秒。Redis 水位拒绝/恢复、Redis 故障后恢复、业务 Agent Provider 溢出压缩均通过；代理连接仍因本地 Sandbox 到节点 IP 超时失败。测试编排新增仅对 `host.docker.internal` 的临时 TLS Service 转发，18 项编排回归通过，远端地址不启用转发，成功或失败均关闭进程。代理访问继续用浏览器实际复测，不能先标记通过。

`p8-r33egress-20261015` 已运行到实际 TLS/鉴权后的目标检查：网关因 `host.docker.internal` 的 IPv6 解析不在临时平台白名单而返回 403，测试失败。保留所有 DNS 答案必须通过策略检查的规则；后续临时 values 仅增加实际观测 IPv6 的 `/128`，源配置不变。转发编排加入后的架构/部署检查为 149 passed。下一轮完整非压测回归验证该链路，当前不标记通过。

`p8-r33full-20261015` 正在执行完整 136 项非压测回归。当前已确认的一个失败为真实 Mem0 记忆提取：Provider 返回 HTTP 200，但默认 2,000 token 中 1,996 个被推理消耗，结构化 JSON 因长度截断无法解析，Mem0 返回 502。测试配置已显式设置 `min(8192, 模型最大输出)`，保持真实写入/召回断言；已启动的会话配置不变，需新会话实际复测。证据为 `mem0-upstream-r33.log` 和 `memory-r33-calls.jsonl`，本次失败不会改写为通过。

完整 `p8-r33full-20261015` 已结束：131 passed、5 failed、0 errors、1 capacity deselected，耗时 5316.24 秒。剩余失败为 Mem0 输出截断、M7 旧网络文案断言、协议负例的非法 workspace ID、真实 GLM 第二轮模型流 120 秒未完整结束、300 节点 p95=50.9ms 超过 50ms。后两项保持原超时与阈值，尚待实际复测。其余 131 项通过，含 200 节点静态解析 42.6 秒/199 个动态节点全部完成、两次强退注入无错误、Redis 故障恢复、TLS/镜像信任、备份恢复和滚动升级。临时 Namespace 已清理，原服务恢复；`p8-r33close-20261015` 使用同一产品源码和镜像对五项失败做实际复测，不能把本轮原始失败改写为通过。


## 最终收口补记

`p8-r33close-20261015`：2 passed、3 failed、1 teardown error，1951.70 秒。Provider 与真实记忆链路通过；产品套件漏前置资源/Monaco 加载竞争、GLM 错放分支 id 后提问、遗留等待 Run 引起矩阵并发级联失败，均保留原始证据。

`p8-r34full-20261015`：完整 136 项非压测，135 passed、1 failed、0 errors、1 capacity deselected，5290.74 秒。plan8 的 10 个 Python 入口全部通过：主流程 2/2、矩阵 20/20、200 节点解析 40.09 秒；300 节点 p95 39.6ms，强退注入两次且无错误，并发峰值 4。唯一失败为旧测试辅助使用合成 mouseover，工具栏未显示。

`p8-r34canvas-20261015`：改用 Playwright 真实 hover，在同一 r34 产品镜像上复测 1 passed，589.80 秒；插件及 Trace 降级/恢复三个浏览器场景全部通过。两轮环境均清理，最终原 12 个 Deployment 就绪，源 GLM 元数据与测试前一致。最终结论与分轮证据见 [实施报告](implementation-report.md)。
