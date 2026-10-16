# plan8 实施与测试报告

2026-10-16 追加修复已完成并本地发布：合法简写引用的面板崩溃、参数结构门禁、消息内容契约、Monaco Worker、Markdown、布局与刷新回执恢复。26 项逐类节点场景分轮验证/复测通过，最终 6 项恢复协议通过；独立记录见 [AI 节点编辑回归](evidence/2026-10-15-node-editing.md)。

状态：**done（本次非压测范围）**。43 个实施任务、26 项原有场景和 3 项模型设置场景已完成并验证。完整 136 项回归为 135 passed、1 failed；唯一失败修正测试交互后，在同一产品候选上复测通过。容量压测按用户要求 deferred，硬件基线及原交互预算未修改。

## 已实现的代码


- 多草稿、原子 revision 0、不可变版本来源和编号锁、历史版本只读画布、版本派生。来源归属由复合外键保护；版本保存精确 Manifest。
- 每画布独立 Store、完整文档序号/JCS hash、共用编辑事务、单批 Undo、保存竞态保护、Recovery 与可靠 ACK。
- 同一 Core 1.3 的 Control Builder Host、持久 Pending/Resume、租约与恢复、Runtime 设计态模型流式 Effect、取消、Receipt、价格/Quota。
- 七工具、当前画布/选中节点范围、静态校验、只读执行证据、持久问答、超大工具结果的分段读取。
- 租户模型设置、三权限交集、CAS、冻结凭据/价格、失效说明、撤销和删除保护。

代码入口：Control `studio_builder_*`、`workflow_drafts`、`workflow_revision_resources`；Runtime `builder_model`、`sandbox_resolve`；Web `workflow-canvas`、`builder-panel`、`editor-store` 和 Models 页面。

## 最新检查与系统结果

以下证据默认位于 `.local/artifacts/plan8-20261014/`。每轮使用独立空库及临时 Kubernetes Namespace；浏览器操作由 `pytest tests/e2e` 统一编排。

| 检查或 Run | 实际结果 | 证据 |
|---|---|---|
| r34 Runtime 单元 | 82 passed，包括分支 id 放错层级、合法名称、保留/非法/重复名称的具体诊断 | `runtime-unit-r34.log` |
| r34 浏览器类型检查 | 通过；失败 Run 清理使用 Playwright 自动 Fixture，不依赖页面是否仍打开 | `browser-types-r34.log` |
| r34 完整 `cargo xtask check` | 通过：Rust 累计 624 passed、2 既有 ignored，前端 445/94 文件；架构/部署 149；含契约漂移、Clippy、边界、Helm 与构建 | `xtask-full-r34.log`、`xtask-r34-summary.json` |
| r34 统一候选镜像 | 14 个全部构建、导入并核对同一源码指纹；节点可用空间约 15GB | `all-images-r34.log`、`image-source-r34.json` |
| r34 完整非压测系统复测 | **135 passed、1 failed、0 errors**；136 selected、1 capacity deselected，5290.74 秒；未筛选浏览器套件或开启截图更新 | `e2e-r34full.xml`、`e2e-r34full-summary.json` |
| r34 plan8 全套 | 10 个 Python 入口全部通过；主流程 2/2、扩展矩阵 20/20；两次强退无错误、设计并发峰值 4 | `.local/artifacts/e2e/p8-r34full-20261015/` |
| r34 插件悬停复测 | **1 passed、0 failed、0 errors**，589.80 秒；插件、Trace 降级和恢复三个浏览器场景 3/3 通过 | `e2e-r34canvas.xml`、`e2e-r34canvas-summary.json` |
| 最终类型与差异检查 | 浏览器 TypeScript 与 `git diff --check` 通过；源码指纹与候选镜像一致 | `browser-types-r34-hover.log`、`image-source-r34.json` |
| 最终环境恢复 | 原 12 个 Deployment 全部就绪；无本次测试 Namespace/进程遗留；源模型元数据与测试前一致 | `final-environment-verification-r34.json`、`final-process-verification-r34.json` |

r34 全量通过 Redis 故障恢复、Provider 溢出压缩、实际 Python/JS/Shell Sandbox TLS 代理访问及拒绝场景、真实模型业务执行、200 节点静态保存（40.09 秒、199/199 动态节点）、设计账本/回收、离线业务 Agent、历史包与删除竞争、跨租户隔离、TLS、镜像签名/负信任、备份恢复与滚动升级。插件悬停失败及其后续通过分别保留原始报告。

## 已关闭的失败

| 问题 | 原因与改动 | 验证状态 |
|---|---|---|
| Mem0 真实写入失败 | 2000 token 中 1996 被推理消耗，结构化 JSON 截断；临时测试配置设为 `min(8192, 模型最大输出)` | r33close 和 r34full 实际通过 |
| Provider 协议负例提前失败 | LightRAG workspace 使用非法连字符；改成合法下划线后才验证协议不匹配 | r33close 和 r34full 4/4 通过 |
| M7 上传网络错误断言 | 仍期待旧通用文案；改为现有网络错误翻译 | r33close 和 r34full M7 通过 |
| Monaco 用例偶发失效 | 源码编辑器加载时的临时 textarea 被 Monaco 替换；等待真实 Editor content | r34full 产品闭环 14/14 通过 |
| 真实 GLM 编辑 | r33 首次创建通过、第二轮上游流 120 秒未结束，正确保留 unknown；r33close 将 `id` 放在 parameters 根部，笼统校验提示导致反复改名后提问 | r34 区分准确错误及字段路径；82 项 Runtime 单元及Fixture/真实 GLM 主流程 2/2 通过，含四轮长上下文与一次真实压缩；模型超时及 unknown 不重发规则保留 |
| 设计 Run 污染后续矩阵 | 失败测试遗留等待问答的 Run；容量用例只剩 3 个名额，后续级联失败 | r34 自动 Fixture 取消本测试创建的 Run；扩展矩阵 20/20 通过，设计模型并发峰值为 4 |
| 300 节点交互 | r33 p95 为 50.9ms，超过原 50ms；r32 同场景通过 | r34full 通过：中位 FPS 120.48、p95 39.6ms，原 50ms 阈值不变 |
| 插件画布悬停 | 合成 mouseover 未触发可见工具栏；辅助方法改为 Playwright 真实 hover，可见性断言保留；只改测试 | 同一候选 r34canvas 实际通过；插件及 Trace 降级/恢复 3/3，原失败记录保留 |

## 逐项功能验收的成功证据

26 项原始要求见 [验收矩阵](05-e2e-acceptance.md)。三个新增模型设置场景为 P8-MODEL-01/02/03。以下 29 项均在 r34full 的 10 个 plan8 Python 入口中通过。全仓库的独立插件失败由 r34canvas 关闭；分轮结果见上表。

| 场景 | 当前验证 | 状态与证据 |
|---|---|---|
| 01 | 模型列表 UI 设置，真实 GLM/Fixture 创建，AI 期间业务执行数 0 | done：r34full 主流程 |
| 02 | 局部改名/绑定保留，无关节点/布局不变，单事务 Undo/Redo | done：r34full 主流程/矩阵 |
| 03 | 全图分支、Code 与 Loop 静态修改和自动保存，不运行代码 | done：r34full 矩阵 |
| 04 | V1/V2 派生独立草稿、多窗口文档/缓存/资源/会话隔离 | done：r34full 矩阵 + 草稿申请隔离 |
| 05 | 历史版本只读、键盘拒绝修改，准确旧版本仍可手动运行 | done：r34full 矩阵 |
| 06 | 版本派生 revision 0，未修改直接 UI 运行/创建版本，完整来源 | done：r34full 矩阵 |
| 07 | 两个 Draft 并发版本编号、重复来源幂等、部署固定旧版本 | done：r34full 矩阵 |
| 08 | 保存中继续编辑，旧响应保留 dirty/Recovery，重开保留后续修改 | done：r34full 矩阵 |
| 09 | 人工编辑后旧命令 stale，重新读取并生成新命令 | done：r34full 矩阵 |
| 10 | 同 Draft 双窗口冲突由 UI 选择载入，AI 无覆盖动作 | done：r34full 矩阵 |
| 11 | ACK 丢失、重连/reload、Undo 后不重复应用，同 ID 去重 | done：r34full 主流程/矩阵 |
| 12 | 停止/关闭/归档取消，迟到结果拒绝，已应用修改保留 | done：r34full 矩阵 |
| 13 | 问答/未知工具/运行诱导不增加执行、尝试、版本和部署 | done：r34full 矩阵 |
| 14 | UI 手动运行后选择真实结果供 AI 只读分析修复，无再次执行 | done：r34full 主流程/矩阵 |
| 15 | 四种 Debug、准确 Revision、独立 Overlay、实际 Pin/Mock 回放 | done：r34full 矩阵 |
| 16 | 跨用户 Session/Entries/Receipt/有效 ACK、跨对象/租户拒绝，只读不能派生 | done：r34full 矩阵 + 跨租户九项 API 拒绝 |
| 17 | 禁用凭据取消、替换凭据需重设、资源失效、保存失败不增 Revision | done：r34full 矩阵 |
| 18 | Exit 保护、非法端口、缺 Loop parent、下游引用及合法 Loop | done：r34full 矩阵 |
| 19 | 真实 GLM 四轮长上下文压缩；Fixture threshold/overflow；快照/使用量去重 | done：r34full 主流程/矩阵 |
| 20 | Host 强退、两副本接管、Gateway unknown 不重发，真实 Receipt 单次结算 | done：r34full 矩阵/故障日志 |
| 21 | Token/非零成本、Quota、分段原文、Retention、无伪业务执行 | done：r34full 账本/矩阵 |
| 22 | 纯 Resolver 与五类逃逸反例、无网络/资源/宿主桥接、设计租约归零 | done：r34full Resolver 六例 |
| 23 | Control 离线：Builder 不可用，已发布 Workflow/业务 Agent 完成；真实模型调用 1 次 | done：r34full，UI 发布 + Gateway/数据库证据 |
| 24 | 中英文、浅深主题、1280/1440 桌面、焦点和错误定位 | done：r34full 矩阵 |
| 25 | 150/300 节点 Undo/重开/原文，四个设计 Run；300 节点 FPS 120.48、p95 39.6ms | done：r34full 矩阵；生产容量认证独立未关闭 |
| 26 | 归档恢复、历史资源/插件与 unknown 删除保护、保存/删除竞争、草稿申请隔离 | done：r34full 矩阵/生命周期/资源 API |
| MODEL-01 | 租户共享、三权限交集、编辑者能用而不可管理、只读不能用 | done：r34full 矩阵 |
| MODEL-02 | 未配置/清除/禁用/删除保护；普通手动编辑和保存可用 | done：r34full 矩阵 |
| MODEL-03 | CAS 冲突，新旧 Run 各冻结模型/部署/凭据/价格，无备用切换 | done：r34full 矩阵 |

## 环境、范围与证据解释

r34full 与 r34canvas 的临时 Namespace 已删除，所有本次测试进程已退出。原 12 个 Deployment 均为 desired=1、ready=1。源 `glm-5.3-flash` 的 Alias、Deployment、Credential 仍 active，连接记录 healthy；ID、版本与测试前元数据完全一致。最终核对时间和逐项结果保存在上述 r34 环境/进程证据中。

源 `glm-5.3-flash` 的模型、凭据及价格只复制到隔离测试环境，凭据值不写证据；源数据库没有运行新 Schema 初始化。Docker Desktop 的 Sandbox 使用测试专属 TLS Service 转发和实测 IPv6 `/128` 白名单，保留 TLS、鉴权和目标策略；源 Sandbox 配置不变。

容量压测按用户要求 deferred；生产 CNI/强隔离认证继续保留原计划边界，不由本地功能测试代替。200 节点保存、300 节点桌面交互与四设计 Run 上限属于功能验收，继续保留。

各轮原始失败、修复及早期成功记录见 [历史验证记录](validation-history.md)。最终结论由完整 r34full 回归及同一候选的 r34canvas 失败项复测共同支撑；原始分轮状态保留。
