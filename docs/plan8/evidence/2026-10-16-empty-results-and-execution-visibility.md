# 空结果、执行可见性与复用错误出口

状态：done（本次功能范围）。代码、本地检查、9 个系统场景分轮验收、本地发布及用户原工作流实测已完成；容量压测 deferred。

## 实际故障

用户 Workflow `01a142c1-115d-7b10-8f68-4463e1e572cb` 的 revision 25，Execution `01a142ce-73f6-71c2-8198-3adaa1303d36` 在 2026-10-16 11:43:08–11:43:11（Asia/Shanghai）失败。LightRAG HTTP 200 返回 `status=failure, metadata.failure_reason=no_results, data={}`；协议层误判为 PROVIDER_REJECTED。error item 已送到 exit_retrieval_err，Agent 为 skipped。与此同时 Runtime 用户身份投影为空，Control 委托查询被 401 拒绝，界面无法展示已发生的失败。

只读证据：`.local/artifacts/plan8-execution-diagnosis-20261016/diagnosis.json`。本次不改写用户原草稿，也不改写历史执行。

## 修复范围

- 共享 LightRAG 检索协议精确识别 `no_results`，输出空 documents/citations/recordIds；独立知识节点产生空 text/documents/files。Agent 知识检索复用同一适配。其他失败和无法解析的协议继续报错，不依赖英文 message 猜测。
- Fixture 使用实际观察到的 no_results 响应，替代不充分的 success/chunks=[] 模拟。
- Control Admission Publisher 启动时分批重发当前用户快照，经持久 Outbox、签名命令、Runtime 正式 API 恢复投影；新命令避免命中失去投影前的旧 Receipt。禁用状态和 token_version 保留；不绕过授权、不访问对方数据库。
- 状态与事件独立读取，单个查询错误不丢失另一个结果或已读取事件；终态立即停止“运行中”。Trace 在执行推进和数据尚未就绪时继续刷新。Runtime 服务器错误不再被 BFF 误报成权限不足。
- AI 默认检查同作用域现有 End/Exit.error，优先复用包括受保护正常结束节点在内的兼容出口。正常输出保留，错误映射使用当前 error item；缺映射和共享契约原子补全。只有不兼容或用户明确要求时新建独立 Exit。Loop 作用域与现有自定义错误路径不变。

## 节点排查

| 范围 | 正常空结果行为与证据 |
|---|---|
| 独立知识检索、Agent 知识检索 | 修复 LightRAG no_results；RAGFlow chunks=[] 已成功；共享协议和 Worker 回归 |
| Model、Agent | Model 空文本可成功；Agent Core 无工具调用时接受空回复；结构化输出仍遵守声明的 Schema |
| MCP、记忆附件 | content=[] / results=[] 保持成功；Worker 回归验证，真实 Provider 错误不被抹去 |
| Code | stdout/stderr/files 空且有效 structuredOutput={} 成功；不符合输出契约仍报错 |
| HTTP | 200 空文本/数组、204 无内容成功；插件回归验证；网络和非成功 HTTP 状态仍报错 |
| Set、List | 空输入、筛选零匹配成功；插件回归验证 |
| If、Merge、Loop | If 无输入成功/无匹配走 else；Merge 可输出空集合；Loop 空数组不启动 body 并完成聚合；代码审查、现有 Runtime 套件及新增 If 空输入回归 |
| Sub-workflow、Approval、Reply/Send、Exit | 审核其边界：子流程/出口按声明契约处理空值；审批等待、拒绝和消息必填输入不是“检索无命中”。必填参数、权限、契约或实际操作失败仍准确记录，并允许有权限用户查看链路 |

上述区分正常业务空结果与非法请求/响应；不把所有 failure、null 或 HTTP 失败统一转换成成功。

## 验证状态

- Runtime Contracts 29、Workflow Runtime 86、V2 Runtime 172（原有 2 ignored）、Control 73 通过。
- 前端相关 8 文件 / 22 测试、TypeScript、生产构建通过；Lint 无错误。Data/HTTP 插件 7 测试通过。
- 验收约束 149、边界检查、相关 Rust 全目标严格 Clippy 通过。
- 首轮新增失败保留测试错误使用了 outcome_unknown=true，但断言 Failed；修正测试输入后通过。Clippy 同时清理节点目录测试中的冗余 into_iter。r1 未完成镜像构建主动停止，未测试或发布。
- r2 系统场景分轮通过：两种检索协议的 UI 运行、共用 End 的成功/失败路径、真实 GLM 自动复用出口、用户投影丢失/恢复/禁用与 Token 版本不变、恢复后浏览器历史与 Trace。
- 首轮系统测试的 Agent 附件 Fixture 在替换成功输出时清空了共用 End 的 errorOutputs，结构校验正确拒绝；修复测试数据保留错误映射后复测通过。恢复测试的 Outbox 等待改为当前用户投影，不复用依赖其他固定 Workflow ID 的等待函数。
- 首轮 `p8-empty-r2-20261016`：8 项浏览器场景中 5 通过、3 失败。除上述 Fixture 外，真实 Agent 附件保存及多出口运行因 MinIO 返回 HTTP 507 Insufficient Storage 失败；Docker 虚拟磁盘仅余 2.4 GiB。仅清理可重建的未使用 Docker Build Cache（6.603 + 8.786 GB），保留业务卷、镜像、模型和凭据；恢复约 15 GiB 可用空间后，使用相同候选镜像定向复测。首轮临时 Namespace 已清理，12 个本地 Deployment 恢复就绪。
- 定向复测 `p8-empty-r2-retest-20261016`：4 项浏览器业务场景全部通过（含上述 3 个失败项和 LightRAG 空检索重跑），新增恢复浏览器场景 1/1 通过。最终覆盖 9 个独立系统场景。恢复测试重建 3 个用户投影，包括禁用用户；投影缺失期间查询返回 403，恢复后 detail/nodes/events/checkpoints/runtime-details 全部 200，执行详情恢复前后完全相同。证据分别位于 `.local/artifacts/playwright/helm-agentxctl/` 和 `.local/artifacts/e2e/` 对应 Run ID。

候选：`plan8-empty-20261016-r2`，12 个镜像的源码 hash 均为 `fe2d4548b2b5f85fff1e1346414ef31dc5d973f90c3de777e85fa317690fe97f`。

## 本地发布与原工作流实测

- 本地版本 `local-plan8-empty-20261016` 使用上述候选 Digest 发布。12 个 Deployment 全部 1/1；四个 Helm Release 和 Doctor 均成功。HTTPS 使用本地 CA 验证，页面资源与当前容器一致；Control/Runtime 基线 checksum 一致，原草稿 Definition/Editor hash、revision、租户模型设置和 Alias 均未改变。
- Runtime 恢复当前 1 个用户投影，状态及 Token 版本与 Control 一致。通过原 Chrome 页面选择 11:43:08 的失败执行，事件、失败检索节点、跳过 Agent 和完整 Trace 可见；历史记录仍保留原始 PROVIDER_REJECTED。
- 通过原画布“运行”按钮提交“你好，请介绍一下你自己。”，创建 Execution `01a14311-9978-71a2-ad35-77fef57aa0d4`，来源仍为原 Draft revision 25。真实 LightRAG 再次返回 `failure_reason=no_results`；检索节点输出 `text="", documents=[], files=[]` 并成功；Agent 调用现有 `glm-5.3-flash` 成功，最终回答“我是 kakj。”及介绍，10,156 ms 完成。模型用量 62 输入 / 201 输出 Token、USD 0.001052。未改写原草稿或原错误连线。
- 第二轮临时 Namespace、Port-forward 已清理；本地缩容服务全部恢复。验证 JSON：`verification.json`、`live-original-workflow-result.json`；截图：`live-failed-trace.jpg`、`live-success-trace.jpg`，均在本次证据目录。

观察到的独立展示问题：历史 skipped 节点没有 ended_at 时，界面耗时仍会随当前时间增长；不影响本次空结果、错误查询或执行结果，未计入本次修复范围。

本次构建与测试证据：`.local/artifacts/plan8-empty-results-20261016/`；早期本地测试日志位于上述诊断目录。
