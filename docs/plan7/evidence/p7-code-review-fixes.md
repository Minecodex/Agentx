# plan7 已推送代码审查修复（2026-10-07）

本轮按“这些全部完善”覆盖审查发现的实现缺陷、缺失的产品界面和验收入口。修复保留工作树中已有的 TLS 安装与 HeroUI 技能改动；未提交、未 push，也未发布或重部署现有 production 命名空间。

## 回顾复核（2026-10-08）

本次复核的结论是：此前发现的主要实现缺陷已经修复并有实际业务证据，但 plan7 尚未全部完成。复核另外发现并修复了两类遗漏：

1. Trace BFF 的 `limit` 原先按原始值签发委托请求 Hash，转发时却限幅到 1–1000，导致 `limit=0` 或 `limit>1000` 时被 Observability 误判为鉴权失败。现在签名与转发复用同一个规范化值。
2. 发布门禁对稳定性时长/采样、场景完成数量以及恢复耗时没有完整检查非法数值，`NaN` 可绕过比较。现已拒绝非有限值、负数、布尔值、缺失值及错误类型；业务完成数、备份对象数和滚动探针数量必须是整数。新增回归在修复前复现 44 个失败用例，修复后相关 Python 回归 129 passed，完整 acceptance 为 138 passed、1 failed。唯一失败仍是已有暂存的根目录 `skills-lock.json` 与仓库布局约束冲突，该文件保留。

扩展静态检查另发现 IM mock 的协议校验使用 `assert`，在 Python 优化模式下会消失，也使全仓 Ruff 失败。已改为显式校验，并在 `python -O` 下实际发送钉钉/飞书有效与无效请求，分别得到 200/400，四项通过（`review-im-mock-protocol.log`）。

Control 回归 62 passed，fixture 回归 7 passed；Clippy `-D warnings`、Rust 格式、V2 boundary check、全仓 Ruff 与 236 个 Python 文件的格式检查通过。新校验重新审查上一轮真实报告后，备份与滚动探针仍通过，容量短测仍因缺少两小时场景与连续采样而阻断认证。日志位于 `.local/artifacts/plan7-fixes-20261007/review-*.log`，报告复核见 `review-gate-recheck.json`，本次汇总见 `review-verification-summary.json`。

本次小修后的源码摘要为 `d041bea6ff7d5e2c14366bcdbe410ad023e1a0d024b9d63e94468fd7a31c5482`。复核结束时没有重建镜像或重跑集群 E2E，以下原始专项证据对应上一轮 `60d80309…` 候选，不能转标为新候选认证。用户随后要求本地升级，同日 11 个正式镜像完成重建，本地四个 Release 与 Doctor 检查通过；新候选隔离集群评测/Judge UI 回归为 1 passed（`p7local1008ui`），详见 [本地部署证据](p7-d-local-production-redeploy.md)。该专项仍不替代全量产品或正式发布认证。

尚未完成的验收和收口包括：

- 同一次 Run 的九域全量 E2E、冻结硬件下两小时稳定性及正式发布汇总；专项 Run 不能拼接。
- 历史版本/Schema 升级与公网 OIDC 签名信任链认证；现有升级证据是同候选演练，恢复证据是备份快照还原，本地签名证据不能替代公网认证。
- P7-E4 的 RAGFlow 与 LightRAG 管理面检索联调，以及管理面上传后由 Agent 检索同一内容的组合验收；现有协议测试与 LightRAG 管理面测试不覆盖全部组合。
- P7-D3 的 Redis Stream 水位暴露/拒绝证据、P7-D7 尚未覆盖的安全场景、P7-D8 运维文档和两个追踪矩阵收口。当前 Gateway 按权威 MySQL 在途/排队数拒绝，不能据此勾选 Redis Stream 专项验收。
- 根目录 `skills-lock.json` 造成的仓库布局检查失败。

## 修复清单

| 范围 | 修复后的行为 | 主要实现 |
|---|---|---|
| 知识库作用域 | 文档列表、上传、删除、检索均复用资源的部门范围；同租户跨部门也拒绝越权 | platform-control/external_resource_api、knowledge_document_api |
| 索引受理与成功状态 | 上传返回 202；track_id 只表示受理，processed 才标记 indexed 并保存最终文档 ID | knowledge_indexing；Control 0013 DDL |
| 索引重启与并发 | 固定 endpoint/workspace/indexVersion/Vault version；租约续租、claim token fencing、稳定 file_source 找回崩溃前任务 | knowledge_indexing |
| 文档引用与删除 | 所有状态计入额度；事务注册/释放原件引用；上传/索引中拒绝删除；资源删除检查文档引用 | knowledge_document_api、external_resource_api |
| 真实 LightRAG 检索 | 使用 /query/data，返回真实 chunk；workspace header 绑定快照，调用输入不能覆盖 workspace/indexVersion | runtime-contracts/rag、Worker 与 Control |
| IM 官方协议 | 钉钉 header/msgParam、飞书 bearer/content、企微 GET token/URL 编码/整数 agentid 与业务码校验 | delivery_send；严格 im-mock |
| 循环投递幂等 | origin/delivery ID 绑定实际 node_execution_id；不同循环激活独立，同一激活重试稳定 | engine_persistence、delivery |
| SSE 解析 | 分片 UTF-8、CRLF、多行 data；保留 tool call ID；缺 finish_reason/DONE、错误帧或截断不成功 | worker_runtime_stream |
| 流式 usage | Provider usage 保留原值，缺失时估算 prompt 与输出并标记 usage_estimated | worker_runtime_stream |
| delta 写入 | 有界 async 回压，定时批量写；flush 确认持久写入成功后才能结算 | worker_runtime_delta |
| Invocation cursor | delta、结算、delivery 按 Invocation 父行锁分配序号 | engine、worker_runtime_delta、workflow-runtime |
| SSE Drain | 终态重连可排空；断开归还 connection gauge，停止 Redis 订阅；唤醒失败不阻断权威数据 | gateway、sse_wakeup、service-kit |
| Studio 流式预览 | Debug 独立模型 delta cursor、授权 BFF、终态排空、文本/推理/错误提示，随执行 retention 删除 | Runtime Query、use-model-deltas、RuntimePanel |
| Playground 图片 | 上传响应 MIME 决定 image/audio/file part，图片进入原生多模态请求 | playground-types、conversation-test-workspace |
| 评测比较 | 2–5 次不同运行，caseKey/ruleKey 对齐、缺失项、状态/评分/耗时/费用及基线变化 | governance_api、EvaluationComparison |
| judge 产品链路 | 真实模型下拉、双语可见范围、严格 passed/score/reason schema、不可变 system prompt 与 case 变量；evaluatorExecutionId 穿过 Runtime 事件和 Control 投影；真实成本/耗时；Trace 按 workflowId 查询名称 | bundle-builder、Worker、work_package_execution、profile dialog、projector、runtime_bff |
| Insights 口径 | 终态执行成功/失败数，正确的 RuntimeCall 成本/token 与真实耗时；数值解码及 camelCase 键一致 | observability、trace_delivery、dashboard/insights |
| 查询与错误跳转 | Query Scope 空范围不放行；工具维度真实工具名；errorCodes 参与 SQL、URL 和 cursor hash | query_authority、query、runtime_bff、executions |
| 网关准入 | 排队调用计入水位、满额拒绝、MySQL 查询失败不放行；一次查询取得两个水位 | admission、gateway |
| 租约与超时恢复 | 先读过期候选、按主键复核与更新；每个到期执行单独结算；恢复器不锁活跃任务或新派发的索引间隙 | execution::recover_dispatches、timeout_expired_attempts |
| Runtime 事务策略 | 固定 READ COMMITTED；保留父行锁/租约/CAS，消除不存在的幂等键导致的独立预留间隙死锁 | runtime-infrastructure/mysql |
| Worker 参数读取 | 仅在参数绑定引用 Outputs 时加载历史输出；嵌套数组/对象/模板仍正确解析，literal 内的引用保持数据 | engine_parameter_resolution、engine |
| 配额释放锁 | 按所属执行/任务读取候选，再以稳定主键顺序释放；无预算任务不锁其他执行的配额 | quota；Runtime 0001 DDL |
| 大结果外置 | Artifact 按索引筛选 ID，Event/Trace 先锁 ID，再按主键读取大 JSON，避免超出 MySQL 默认排序缓冲区 | artifact、event_export、trace_delivery |
| 大工作流持久化 | 仅更新有变化的激活与投递；完整状态机/Hash/检查点仍同事务保存，避免反复写全部历史记录 | engine_persist_machine、agentx-runtime/state |
| Trace 消费批次 | 一批最多 100 条，批量写入/冲突检测/ACK；相同 Hash 重放去重，并发不同 Hash 记录冲突并标记 degraded | observability/process_items、ingest_trace_batch |
| 血缘批量落库 | 原始 Item 关系与幂等不变，最多 256 行一批，同状态转换事务提交 | engine_persistence |
| Trace relay 批次 | 最多 100 个 ID 领取、Redis pipeline 及逐事件回执；Owner/Fencing/有效租约校验与失败退避保留 | trace_delivery、workflow-runtime |
| Trace cursor 与完整性 | 诊断事件由 MySQL 自增索引分配全局 cursor，不锁执行主行；同一视图查询 MAX 与 COUNT，按实际事件数确认入库完整性，允许 cursor 间隙 | execution_events DDL、trace_delivery、Query/BFF、observability |
| Redis 未读量 | 同一 Lua 快照读取消费组 cursor 与实际未读消息；pending 单独计数，超过 2,000 时返回 2,001 触发原门禁；保留原生 lag 对照 | capacity/collector、实际 Redis 测量自检 |
| 任务恢复去重 | 同租户/Attempt 已有未读或 Pending 消息时返回原 Stream ID；原子 ACK 清理索引；旧 ACK 不删除重建指针；Redis 丢失仍由 MySQL 重建 | runtime_task_queue；真实 Redis 并发/丢失回归 |
| 评测收敛锁与批量报告 | 当前 Case/judge 按执行 ID 索引定位并只锁对应行；父行串行聚合不锁旧 Case；无模型规则不加载整个包；最终报告批量读取规则 | work_package_execution；Runtime 0001 DDL |
| 测试磁盘保护 | 节点可用空间低于 5 GiB 即停止当前临时 Run 的写入与负载，保留失败；拒绝操作 production 命名空间 | capacity/collector |
| Artifact 候选索引 | 载荷大小生成列与覆盖索引筛选候选，仅命中后读 JSON，避免每个副本重复扫描历史大载荷 | artifact；Runtime 0001/0004 DDL |
| 已发布派发恢复 | 覆盖索引非锁定筛选 ID，每个候选独立事务按主键复核与重派发；不连表排序/锁定历史 JSON | execution::recover_dispatches；Runtime 0001 DDL |
| 混跑归属证明 | 只统计本次 Invocation 的成功 Attempt，以 Worker UUID 对应 Ready Pod UID 与镜像摘要，要求两个版本均实际执行 | capacity/matrix |
| Worker 领取锁 | 仅锁 Attempt；执行、节点、快照和上下文只读，避免参数解析期间阻塞执行主行上的 Trace 序号分配 | engine::claim_worker_attempt |
| Worker 租约台账 | 删除旧 lease_token 唯一约束；同一 Worker 的每个 Attempt 各有一条独立租约，不覆盖其他任务或共用一条锁 | Runtime 0001 DDL；quota_concurrency 回归 |
| 无消费方 outbox | 删除应用创建时永久 pending 的 ApplicationCreated；保留已有审计记录及实际准入/部署 outbox | application_catalog_api |
| 容量验收 | 集群内负载生成、真实发布/调用/成功终态、七组实际采集、逐场景阈值、逐 Pod 流量、200 SSE 同时在线指标、副本/混跑/故障恢复/两小时采样；smoke 不认证 | tests/e2e/capacity |
| 供应链与发布 | 11 个摘要镜像实际签名/attestation 验证，SBOM 篡改阻断；候选身份绑定九域 JUnit 和专项报告，缺证据不生成 passed、不发布 | release/evidence、signature_verification、release_summary、publish_release |

## 当前验证证据

所有本地日志位于 `.local/artifacts/plan7-fixes-20261007/`；临时集群原始证据位于 `.local/artifacts/e2e/<run_id>/`。下面只记录实际已执行的结果。

- Rust 相关库/二进制回归通过，包括 Runtime 最新完整库回归 158 passed（2 个既有 ignored）、bundle-builder 14 passed、Control 62 passed、contracts 27 passed；多个 judge 结算状态的两个用例与真实 MySQL Runtime slice 回归也通过。恢复器新增“持锁但未过期的任务不应阻塞恢复”断言，真实 MySQL 切片 1 passed、25.85 秒。Clippy `-D warnings` 与 V2 boundary check 通过。
- 最新增量持久化回归：核心状态机库 81 passed、Runtime 库 158 passed；真实 MySQL 切片 1 passed、24.65 秒（`runtime-incremental-history3.log`），新增“锁住已完成节点及边记录，后继结算仍在 500 ms 内成功”的断言，终态与投递数量均验证。Observability 5 passed（`observability-batch-regression.log`），真实 ClickHouse 验证 100 条重复批次去重及并发不同 Hash 冲突持久化。三个相关 crate 的 Clippy 与边界检查通过。
- 批次优化回归：`runtime-bounded-batches-final2.log` 为真实 MySQL/Redis 切片 1 passed、25.55 秒；验证 600 条血缘关系跨 256 行批次保持来源/目标序号，151 条大 Trace 的并发领取、失败退避、旧 Owner 拒绝及每条 Redis Stream 回执对应。Runtime 库仍为 158 passed，Clippy 与 boundary check 通过。Python 最新 46 passed（`acceptance-bounded-batches.log`），新增存储中止保护仅允许当前临时命名空间，并验证扩缩容失败时仍中断负载；Ruff 通过。
- 大小索引与派发恢复回归：`runtime-recovery-index-final.log` 为真实 MySQL/Redis 切片 1 passed、27.73 秒；查询计划确认检查点与恢复候选均由覆盖索引读取，历史 JSON 不进入扫描；两个恢复器并发只重派发一次。Runtime 库 158 passed（2 个既有 ignored）、Clippy、Rust 格式和 boundary check 通过。发布/门禁/脱敏/编排 42 passed，架构 4 passed；Ruff 检查及格式通过。
- Worker 领取锁回归：`runtime-attempt-lock-scope.log` 为真实 MySQL/Redis 切片 1 passed、31.87 秒。持锁冻结执行主行、节点、快照和状态时，20 路领取在 500 ms 内完成且只成功 1 路。相关 Python 48 passed（`acceptance-attempt-lock-scope.log`）；新 Pod 在原 10 秒采集期限内等待首个 acquire 样本，始终无样本仍失败。该轮 Redis Stream 总长度上界随后在 `p7capacity14` 暴露误报，已改为原子读取真实未读量。
- Worker 租约台账回归：`runtime-worker-lease-ledger.log` 为真实 MySQL/Redis 切片 1 passed、28.34 秒。同一 Worker 并发领取 20 个独立执行，每个 Attempt 均有独立的有效租约，结算后 20 条均释放；额度为 2 时仍只有 2 个执行/租约。DDL 直接删除过时唯一约束，无新迁移或兼容层；Lease 仍按 Attempt/Worker/Fencing 校验。架构 boundary check 通过。
- Trace cursor 回归：`runtime-trace-cursor3.log` 为真实 MySQL/Redis 切片 1 passed、26.20 秒。持锁冻结执行主行时，12 路诊断事件在 500 ms 内完成；cursor 唯一且有序、事件与 outbox 同事务、回滚不改变 MAX/COUNT。`observability-trace-snapshot.log` 为真实 ClickHouse 5 passed、5.41 秒：较晚事件不能掩盖缺失的中间事件，补齐后完整，重复重放不增加数量。相关 contracts/Control 回归、Clippy 与 boundary check 通过。
- 前端 Vitest 90 文件、405 测试通过；新增 Playground 上传图片到实际发送请求的回归通过。TypeScript 构建检查通过；OpenAPI/Runtime schema 已重新生成。
- 最新 Trace cursor 候选的前端完整回归仍为 90 文件、405 passed（`web-test-trace-cursor-final.log`，46.43 秒），TypeScript 检查通过；发布门禁、部署、脱敏、编排和架构回归 48 passed（`acceptance-trace-cursor-final.log`，13.13 秒）。
- 任务去重真实 Redis 回归 `runtime-task-dedup.log` 为 2 passed、0.48 秒；32 路并发发布同一 Attempt 仅一个 Stream ID，Pending 重派发复用、Stream 单独丢失、旧 ACK、Redis 全量丢失与不同重试 Attempt 均验证，排空后的索引为 0。Python 最新 48 passed（`acceptance-task-dedup-final.log`，13.35 秒），Ruff 通过；新增在途索引零残留验收与锁等待峰值发生时的即时 prepared statement 诊断。
- 评测收敛最新回归 `runtime-task-evaluation-final3.log`：真实 MySQL/Redis Runtime 切片 1 passed、30.11 秒；锁住第一条已完成 Case 及其规则，第二条 Case 连同终态报告仍在 500 ms 内成功，模型规则实际结算与成本/耗时回归仍通过。独立 Redis 回归 2 passed、0.40 秒；最新 Runtime 完整库 158 passed、2 个既有 ignored（`runtime-task-evaluation-final.log`）。
- 新的兼容混跑对照 `plan7-compatible-previous-dedup-20261007` 使用同一 Task 去重与评测结构，仍只保留较早的连表领取锁实现；源码摘要为 `4685462b0805e667cb10e411b24c3d827059db267d06d7d93055e4bd9d4e6af0`，镜像摘要为 `sha256:614d53278d27a1caaa8d9a03a1fe29bf898e1f8ad8b5104cd6cc6d7630b336b5`。当时源码已按字节恢复，恢复后的 Clippy `-D warnings` 通过（`clippy-task-evaluation-restored.log`）；候选源码摘要为 `60d803095d7c0998495b2bee96d693248640ee5d343f489cadc03a469a56b62e`，随后通过下述 `p7capacity17` 集群短测。
- Python 架构、部署、发布包、门禁、脱敏及 E2E 编排回归最新 43 passed（`acceptance-recovery.log`）；门禁用例覆盖全 401、缺指标、长跑掩盖超标、单 Pod 冒充扩容、缺少在线 SSE 证明、错误的本机转发测量、缺 JUnit、混候选、无验签工具、过期 passed marker。
- 全量 acceptance 的上一轮结果为 43 passed、1 failed（`python-regression-final.log`）。唯一失败是仓库布局检查要求根目录不存在 `skills-lock.json`，与开始工作前已有的用户暂存文件冲突；本轮保留用户文件，也未改弱该检查。
- `p7signature7`：最终恢复修复镜像的本地 TLS Registry 演练中，11 个真实镜像全部签名、SPDX attestation 和验签通过；错误公钥、摘要替换和本地 SBOM 篡改拒绝。`1 passed`，162.54 秒。原始 JUnit 为 `signature-junit7.xml`，完整供应链文件在 `.local/dist/local-supply-chain-p7signature7/`。源码摘要为 `f3d895dc0710975406793a5e6bfe958fe580f524b6c518e2b6ee5ee887601a25`；该结果是本地信任域演练，不替代公网 release-ci 证据。
- `p7signature8`：最新 Attempt 领取锁候选的 11 个真实镜像全部签名、SPDX attestation 和验签通过，三个负向信任检查均拒绝；1 passed、177.15 秒，JUnit 的 failures/errors/skipped 均为 0。原始 JUnit 为 `signature-junit8.xml`，完整供应链文件在 `.local/dist/local-supply-chain-p7signature8/`；13 个主服务/测试 Provider 镜像的源码摘要一致为 `76cb5d66615b45fbc2b364019beda9f8c42ccec125ee70d3314ec9232b9d8258`。私钥和临时 Registry 已清理，本地信任域不代替公网认证。
- `p7signature9`：当前 `60d80309…` 候选的 11 个真实镜像全部签名、SPDX attestation、验签与三个负向信任检查通过；1 passed、160.77 秒，JUnit 的 failures/errors/skipped 均为 0。原始 JUnit 为 `signature-junit9.xml`，完整供应链文件在 `.local/dist/local-supply-chain-p7signature9/`；该源码摘要与 `p7capacity17` 相同。临时 Registry 与私钥已清理，仍是本地信任域演练，不替代公网 release-ci 认证。
- `p7product5`：12 passed，537.54 秒，failures/errors/skipped 均为 0。覆盖三个 IM 官方协议与循环激活、四个既有投递场景、模型流式与原生图片、Studio 独立持久 cursor、Insights 权限与降级、知识库跨部门隔离/Provider 停机/Control 重启恢复/引用释放、实际 judge 输出/成本/耗时及 Trace、比较页面与配置保存。源码摘要与 `p7signature7` 相同；原始 JUnit 为 `product-junit5.xml`，四份业务报告位于 `.local/artifacts/e2e/p7product5/product/`，页面截图和 Trace 位于 `.local/artifacts/playwright/helm-agentxctl/p7product5/plan7-ui/`。临时 control/runtime/deps 命名空间已清理。
- `p7product6`：最新 Attempt 领取锁候选完整复测 12 passed、467.73 秒，JUnit 的 failures/errors/skipped 均为 0；覆盖范围与 `p7product5` 相同。源码摘要与 `p7signature8` 相同，原始 JUnit 为 `product-junit6.xml`，业务报告位于 `.local/artifacts/e2e/p7product6/product/`，浏览器截图/Trace 位于 `.local/artifacts/playwright/helm-agentxctl/p7product6/plan7-ui/`。临时 control/runtime/deps 命名空间均已清理。
- `p7product7`：当前 `60d80309…` 候选完整复测 12 passed、554.42 秒，JUnit 的 failures/errors/skipped 均为 0。覆盖三个 IM 官方协议与循环激活、既有投递重试/死信场景、模型 SSE/原生图片、Studio delta、评测 judge 真实成本与 Trace/比较界面、Insights 权限与降级、知识库跨部门隔离/Provider 停机/Control 重启恢复/引用释放。源码摘要与 `p7capacity17`、`p7signature9` 相同；原始 JUnit 为 `product-junit7.xml`，业务报告位于 `.local/artifacts/e2e/p7product7/product/`，浏览器证据位于 `.local/artifacts/playwright/helm-agentxctl/p7product7/plan7-ui/`；临时 namespace 已全部清理。
- `p7upgrade-final`：同一源码候选的补充恢复与升级演练 4 passed、340.98 秒，JUnit 的 failures/errors/skipped 均为 0。Control 的 114 表备份恢复验证了备份前业务行、备份后行与活库隔离，五字段收据记录 `control-0013`、内容 SHA、RPO 0.5 秒/RTO 2.2 秒；这是备份快照恢复演练。Redis FLUSHALL 后新调用完成，服务恢复 0.2 秒；在途 Worker 任务重建由容量场景另行验证（21.11 秒）。同候选扩容→Helm 升级→缩容期间 5 次受理/5 次成功、不可达为 0；未知 Worker 协议实际拒绝。该演练不代替历史版本/历史 schema 的升级认证。原始 JUnit 为 `upgrade-junit-final.xml`，报告位于 `.local/artifacts/e2e/p7upgrade-final/backup/` 与 `upgrade-rolling/`，临时 namespace 已清理。
- 失败 JUnit、即时诊断和浏览器 Trace ZIP 均使用统一脱敏规则，裸 JWT/API Key 也会清理；保留失败状态和断言，不将失败改为通过。Trace ZIP 中的图片等二进制资源不变，相关三个脱敏回归通过。
- `p7capacity3`、`p7capacity4` 的失败证据保留，不能记为通过。后者 100 个并发请求全部成功完成，但本机 port-forward 接受延迟 p95 为 831.02 ms。`p7profile1` 中同一轮 100 并发的服务端 p95 为 249 ms、本机为 688.11 ms，确认 TCP/SPDY 转发抬高了测量值。负载生成器已移入临时集群；每个虚拟用户独立预热 HTTP 连接，状态每秒查询一次，仍按冻结的 500/2000 ms 判定。`driver-profile4.log` 中 100 次调用全部成功，p95 为 252.04 ms、p99 为 253.80 ms。该诊断不替代完整容量矩阵。
- `p7capacity5` 暴露 ConfigMap 客户端 apply 注解大小限制，已改为 server-side apply；`p7capacity6` 暴露 Redis INFO 原始文本解析、预编译 SQL 计时遗漏、恢复器范围 UPDATE 的死锁和没有消费方的应用创建 outbox。相关实现已修复，并通过真实 MySQL 与实际指标读取；两个失败集群均已清理。`p7capacity7` 中前三个场景成功，但大工作流继续暴露跨作用域配额释放死锁、大 JSON 排序内存错误；测试节点磁盘随后耗尽，仅余 108 MiB，不能将该 Run 记为容量通过。原始 InnoDB、Pod 事件与失败 JUnit 已保留；临时 namespace 已全部清理，旧构建缓存回收 4.476 GB。相关代码修复已通过真实 MySQL 回归（`runtime-capacity-final3.log`，1 passed、20.92 秒），同时验证了大结果外置与 Integration Event 排空；Runtime 库 158 passed，边界检查通过。`p7capacity8` 在 100 次调用全部完成时仍失败（p95 622.95 ms，168 次死锁）；InnoDB 原始记录定位于默认 REPEATABLE READ 的缺失幂等键 gap lock。Runtime 连接隔离策略已明确为 READ COMMITTED，并以实际服务入口验证 20 路无额度限制启动、额度 2 只启动 2 路且拒绝 18 路、预留全部释放；`runtime-slice-read-committed2.log` 为 1 passed、27.04 秒。主动预留与已提交使用量采用同一条 SQL 的一致视图。该失败集群已清理，正在重新构建一致候选。

- `p7capacity9` 的 Runtime 死锁已消除；100 次调用全部完成，集群内接受延迟 p95 为 476.58 ms；500 个节点实际完成；200 节点工作流成功。进入 5,000 Attempt 阶段后，实测 Trace 消费 lag 达到 5,356，MySQL 原生最大锁等待达到 684 ms，因此中止并清理临时 namespace，仍保留 failed 状态。消费者逐事件 ClickHouse 查询/写入与状态机重复写历史记录的瓶颈已分别修复，正在构建和验证新的一致候选。
- `p7capacity10`：100 次调用、500 个节点、200 节点工作流成功，p95 分别为 416.50、227.62、36.86 ms；Runtime 无死锁、Trace lag 降至几十条。大工作流叠加阶段累计 3,765 个节点时再次耗尽测试节点磁盘；Runtime MySQL 的表数据约 0.94 GiB，而 binlog 约 12.9 GiB、对象存储约 2.4 GiB。磁盘耗尽后出现 507/AOF 写失败与超时，已中止并清理；不能算容量通过。原始占用和服务诊断以 `capacity10-*` 保存。新增批次优化及中止保护；5,000 Attempt 吞吐采用 1,250 次四个实际工作节点的调用，200 节点场景独立验证，原始数量与所有冻结阈值不变，也未调数据库参数。
- `p7capacity11`：5,000 个真实 Attempt 完成，101.77 秒；1,000 条评测用例、200 条实时 SSE、四服务各自 1/2/3/4 副本均成功。SSE 补帧 0.73 秒以内、终态排空约 0.006 秒，残留订阅与连接均为 0。混跑的 100 次调用实际完成，但验收器误把 Worker UUID 当成 Pod 名称而失败；已改用本次 Invocation 的成功 Attempt、Pod UID 与镜像摘要对应。另保留 `capacity11-slow-prepared.txt`：Artifact 的历史 JSON 扫描最高 2.34 秒，原生最大锁等待 524 ms。已改为大小生成列/覆盖索引，仍按 200 ms 的保守上界门禁验证。临时集群已清理，该 Run 不记为容量通过。
- `p7capacity12`：所有 24 个业务/副本/混跑/短稳定性场景完成，但最终采集和数值门禁未通过（1 failed、1030.72 秒）。接受延迟 p95：100 并发 494.02 ms、500 节点 219.63 ms；全程慢查询率为 0。前半段无死锁，但原生最大锁等待为 494 ms，定位到 Worker 连表领取期间持有执行主行；已收窄到 Attempt。混跑阶段 101 次死锁的原始记录为旧 `prod-20261007-worktree` Worker 的旧配额范围 UPDATE；下一轮使用真实前一候选 `plan7-previous-22694b56229d`，保留其原有源码摘要和镜像 ID。Redis XDEL/重建后 lag 无值与新 Pod 首个样本时序也已修复；该 Run 的诊断以 `capacity12-*` 保存，临时 namespace 已全部清理。
- `p7capacity13`：1 failed、243.56 秒；100 并发全部完成，接受延迟 p95 364.60 ms；500 节点全部完成但 p95 633.30 ms 超标，原生最大锁等待 722 ms。无死锁、无慢查询。原始 SQL 定位到 `worker_leases` upsert，台账显示 900 个实际 Attempt 仅有 1 条租约（`capacity13-lease-ledger.json`）：旧唯一约束把稳定 Worker UUID 当作每个 Attempt 的独立 Token，导致同一 Worker 所有任务覆盖并串行锁定一行。已删除该约束并以同 Worker 20 路真实 MySQL 回归验证；临时 namespace 已清理，候选待重建及复测。
- `p7capacity14`：所有 24 个矩阵阶段完成，最终门禁仍为 1 failed、1008.41 秒。100 并发/500 节点接受延迟 p95 为 442.89/274.81 ms；1,042 个 Attempt 对应 1,042 条租约，缺失为 0，旧台账覆盖问题已消除。死锁与慢查询均为 0，连接池等待最大 p95 为 95.075 ms。原生最大锁等待 831 ms 定位到诊断 Trace 更新执行主行时与约 6.17 MB 状态机事务竞争；Redis 无原生 lag 时使用总长度上界误报 2,168。上述问题分别由无执行主行锁的全局 cursor 和原子未读量测量修复，数值阈值保持不变。原始证据为 `capacity14-*`，临时 namespace 已清理。
- 新 Trace 结构不能与依赖旧执行行 watermark 的历史 Worker 混跑。本轮构建 `plan7-compatible-previous-20261007` 作为有实际代码差异的测试对照：使用新 Trace 结构，保留较早的连表领取实现；它不是历史发布版本。源码摘要为 `c58abd1ae3957627bc992a000cbcf3e1338921213aa87fdb0536368656d368aa`，镜像摘要为 `sha256:13c4b1a3ceaf4d3331271b2e8201f1ed2aeb31850a91644e686e25e686cc5a6e`。构建后已按字节恢复当前源码，最终候选独立构建；混跑须证明两种镜像均实际完成本次调用。
- `p7capacity15`：所有 24 个矩阵阶段完成，最终门禁为 1 failed、998.53 秒。100 并发/500 节点 p95 为 401.53/217.81 ms；5,000 个真实 Attempt 在 100.64 秒内完成；Redis 未读量自检与两个不同镜像的实际混跑通过，无采集错误、死锁或慢查询。失败项为原生最大锁等待 443 ms、真实未读量 2,041。1,000 Case 阶段的插件任务流达到 2,001 饱和值，Trace 未读量同期只有 40；实际 prepared statement 计数在 14:21:05 UTC 显示 5,188 次恢复重派发，不能把该值与随后 Pod 替换后的 0 当成同一个累计量。已实现 Task 发布去重。评测收敛同时消除整包重复读取、最终报告 N+1、按执行 ID 查找缺索引与全历史 Case 锁定，500 ms 持锁回归已通过。源码摘要为 `09cbae1fb74abf2e41c58bddd18afbdff82c228733ffcf9c22b780617cb17cf8`；原始证据以 `capacity15-*` 保留，临时集群已清理；最终候选待重建复测。
- `p7capacity16`：当前 `60d80309…` 候选在负载开始前，创建 Caller Key 时本机连接临时 Web 的 port-forward 超时（ConnectTimeout）；1 failed、201.01 秒，没有容量测量结果。Redis 实际测量自检通过。临时 Pod 均 Ready、无重启，已处理请求的 Web 日志为 201，port-forward 错误日志为空；本机 TCP 状态未显示端口耗尽。失败 JUnit 和环境诊断保留，临时 namespace 已清理；不改代码或阈值，重跑同一候选。
- `p7capacity17`：同一 `60d80309…` 候选完整容量短测 1 passed、1058.43 秒，JUnit 的 failures/errors/skipped 均为 0。24 个阶段与七组门禁全部通过：100 并发/500 节点接受 p95 为 441.80/227.06 ms，5,000 个实际 Attempt 在 98.25 秒内完成；1,000 Case、200 条实时 SSE、四服务各自 1/2/3/4 副本和两个不同 Worker 的实际执行归属均验证。原生最大锁等待 111 ms，连接池最大 p95 为 76.64 ms，死锁与慢查询为 0；真实未读量峰值 921，Pending 峰值 62，Redis 重建 21.11 秒。SSE 补帧 0.736 秒、终态排空 0.0034 秒，断开后订阅/连接均为 0；所有七项残留（含 Task 去重索引）为 0。原始报告在 `.local/artifacts/e2e/p7capacity17/capacity/`，核验摘要为 `capacity17-verified-summary.json`；临时 namespace 全部清理。稳定性只有 33 秒，报告明确为 `incomplete`，该短测不替代正式两小时认证。

上一轮一致源码候选 `60d80309…` 的产品流程、容量短测、备份恢复/同候选升级演练与本地签名均通过。13 个镜像源码摘要已核验一致，原有暂存改动的 SHA 未改变，临时集群与签名私钥已清理；该轮汇总核验文件为 `final-verification-summary.json`。各专项 Run 使用不同 runId，不能拼接成正式 release 认证。在同一 Run 的九域 JUnit、两小时稳定性、历史版本升级及公网供应链证据全部齐备以前，plan7 的生产认证不能标记完成；本次复核后新候选的范围见本文顶部。

## 可复现入口

```bash
# 本机 PATH 须使用 Helm 3（当前系统默认 Helm 4 不符合 agentxctl 契约）
env PATH=/opt/homebrew/opt/helm@3/bin:$PATH \
  uv run --frozen --group test pytest tests/e2e/product/test_delivery_platforms.py \
  tests/e2e/product/test_channel_delivery.py tests/e2e/product/test_model_streaming.py \
  tests/e2e/product/test_evaluation_comparison.py tests/e2e/product/test_evaluation_insights.py \
  tests/e2e/product/test_knowledge_recovery.py \
  tests/e2e/product/test_plan7_ui.py --values <candidate-values.yaml> --timeout=3600

# 本地 TLS Registry 签名与负向测试
uv run --frozen --group test pytest tests/e2e/security/test_supply_chain.py \
  --values <candidate-values.yaml> --timeout=1800

# 正式容量：必须提供不同且可兼容的旧 Worker 镜像；短跑使用 --capacity-smoke
env PATH=/opt/homebrew/opt/helm@3/bin:$PATH \
  uv run --frozen --group test pytest tests/e2e/capacity \
  --values <candidate-values.yaml> --previous-worker-image <previous-worker-image> --timeout=14400

# 恢复与当前候选升级演练
env PATH=/opt/homebrew/opt/helm@3/bin:$PATH \
  uv run --frozen --group test pytest tests/e2e/upgrade/test_backup_recovery_drill.py \
  tests/e2e/upgrade/test_rolling_upgrade_probe.py --values <candidate-values.yaml> --timeout=3600

# 九域正式认证：由同一 Run 收集全部证据，包含两小时稳定性
env PATH=/opt/homebrew/opt/helm@3/bin:$PATH \
  uv run --frozen --group test pytest tests/e2e --values <candidate-values.yaml> \
  --evidence-run-id <certification-run-id> --previous-worker-image <previous-worker-image> \
  --junitxml=.local/artifacts/certification.xml --timeout=14400
```

公网发布认证由 `.github/workflows/agentxctl-release.yml` 的 certify → publish 两阶段执行。Cosign 3 的配置方式依据 [Sigstore 配置文档](https://docs.sigstore.dev/cosign/system_config/custom_components/)。
