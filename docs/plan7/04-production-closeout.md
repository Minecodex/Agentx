# P7-D：P0 收口与生产认证

## 1. 目标与边界

两件事：

1. **关闭 `docs/todolist.md` 唯一条目**："测试对接 rag，mem，sandbox 这三块内容"——provider 对接测试虽已提交（e2f0d60），但 RAGFlow 用例全部环境变量 skip、OpenSandbox 需人工预启动、长期记忆只有拒绝路径，未收口；
2. **关闭生产认证门禁**：planv2 V2-06B/07B/08B 与 `docs/plan/12-integration-hardening-release.md` INT-009/011/012/014 全部剩余项，使 `docs/planv2/99-traceability.md` 与 `docs/plan/99-feature-traceability.md` 两个矩阵无非 done 行。

本线大部分任务是"验收器已实现、缺真实 Run 证据"，少量是缺实现（背压、容量编排、SBOM/签名）。

不做：gVisor/Kata（07B 明确延期，本线保持 RuntimeClass restricted + 固定 CIDR Egress 现状并在矩阵注明移交）；多区域多活；HPA（扩缩容交给用户平台，Agentx 只暴露 metrics，V2S-005 决策不变）。

## 2. 现状事实

### 2.1 provider 对接测试

- `tests/e2e/product/test_provider_integration.py`（93 行）：fail-fast 要求主机 `127.0.0.1:18080` OpenSandbox 健康，拉取 `opensandbox/code-interpreter:v1.1.0` digest，驱动 Playwright `provider-integration.spec.ts`（686 行，4 用例）；
- 主用例（LightRAG+Mem0+OpenSandbox 一体化工作流）默认执行；**RAGFlow 用例中 2 个依赖环境变量（`AGENTX_E2E_RAGFLOW_BASE_URL`/`ALIAS_BASE_URL`），未配置即 skip**（勘察修正：第三个"RAGflow 被 egress 策略拒绝"用例默认执行——其 rejectedBaseUrl 默认指向无服务的 `host.docker.internal:19380` 也成立，明文 HTTP 被 egress 拒绝即可通过）；**无集群内 RAGFlow fixture**（conftest `e2e_providers` 只部署 echo-mcp/echo-node/lightrag/mem0），且 `test_provider_integration.py` 不注入任何 RAGFlow 变量、靠外层进程环境透传；
- 长期记忆只验证拒用例（`AGENT_LONG_TERM_MEMORY_SUBJECT_REQUIRED`），无成功 recall 用例（勘察利好：API 级 write/recall 成功路径已有先例 `tests/e2e/runtime/test_agent_sessions.py:436`（P3-05，两轮 gateway invocation 断言 runtime_calls≥2 与审计），D1 只缺 provider-integration 套件里的 UI 两轮对话版本）；
- `docs/todolist.md` 因此保留该条目未删。

### 2.2 planv2 剩余项

- **V2-06B / V2S-006**（planned）：容量与背压——MySQL/Redis/OSS/CH/Provider 全局预算、按 Tenant/Capability/Provider 公平限流、Gateway Admission + Retry-After + 熔断、容量脚本与基线报告、"冻结阈值且 2 小时残留合格"；
- **V2-07B**（V2K-001~006 in_progress）：真实外部 TLS E2E、强 RuntimeClass 集群矩阵、Role 级 Secret/ServiceAccount 拆分 + 供应链门禁、多 Migration Job 竞争、真实 PITR/RPO/RTO 演练、持续 Invocation 升级验证——验收器/编排已冻结，缺真实执行证据；
- **V2-08B**（V2C-005/006 planned）：正式多副本容量矩阵（含 E2E-V2-012 滚动版本兼容混跑）与最终发布审查（Runbook/Schema Catalog/证据可复现）；
- `04-e2e-acceptance.md` §3 冻结阈值清单（7 组指标）要求在首次容量 Run 前冻结；§6 08B 完成条件含全部 E2E failures=0/skipped=0。

### 2.3 M7/INT 剩余项

- INT-009 双阶段滚动升级：验收器已实现（勘察修正：历史 0016/0017 迁移编号已不存在，`deploy/migrations` 现最高 0010；**现行双阶段入口是 `agentxctl migrate --phase expand|contract`**（`tools/agentxctl/src/operations.rs:554-668`）+ chart schemaGate initContainer + `agentx-v2-ops` 多迁移锁（MySQL GET_LOCK + ClickHouse lock 表，`agentx-v2-ops/src/lib.rs:109-190`）），缺 M6→M7、Previous→Candidate 真实滚动与回滚证据；
- INT-011 安全：Vault/撤销/TLS Registry/SBOM/签名/攻击矩阵代码历史上实现过，**当前工作树已无 SBOM/cosign 实现**（历史 `m7-*.ps1` 已删，行为规格幸存于 `docs/plan/m7-acceptance-evidence.md` §3，`deploy/release/*.schema.json` 幸存可用），需按现有 `agentxctl`/release 脚本入口重建；`verify_release.py` 只有 digest + 3 次有界重试。勘察新增（D7 阻塞项，先决定案 4）：幸存的 `supply-chain-evidence.schema.json` 写死 `imageCount=7`，与现行发布契约的 **11 镜像**（8 常驻 + 3 Job，`dockerhub-beta.yaml:12-23`、`package_agentxctl.py`、`v2-release-manifest.schema.json` 三处互相印证）冲突——D7 动手前必须先对齐，否则 verify 与 supply-chain 两条链互相矛盾；
- INT-012 容量："100 Execution、500 Node、200 SSE、1000 Case、200 节点和 2 小时结果尚未执行"；**容量编排无存活代码入口**（历史 performance.json 生产者已删）；
- INT-014 发布门禁："升级、回滚、容量和签名证据尚未满足最终发布汇总器"。

### 2.4 已有基础设施

- e2e 编排：`agentxctl install/uninstall --purge-data`、临时 Namespace、`--scale-down-development` 停开发服务、产物脱敏写 `.local/artifacts/e2e/<run_id>/`；
- 现存故障注入：runtime 重启恢复（test_runtime.py:306-326）、ClickHouse 缩容降级（test_playwright.py:111-135）、副本扩缩+升级回滚（test_release_history.py）；
- 背压现状：仅每 caller 进程内令牌桶（`rate_limit.rs`，默认 50rps/burst100，非分布式，429 + retry-after 响应形态已有）与 quota reserve/release（`quota.rs` MySQL 租约投影）。勘察新增三点：① 配额超限现状是 **400 `AdmissionPrerequisiteMissing`**（`quota.rs:203-208`）而非 429——D3 需明确分层：分布式准入（租户并发/队列水位）归 429 + Retry-After，业务前置缺失归 400；② 并发准入（execution_concurrency/node_concurrency）发生在 Coordinator claim 与 attempt 派发侧（`engine.rs:146-156、:1890-1912`），**Gateway 接受路径无任何检查**（过载时请求已被 202 接受）——Admission 是新增链路而非加固；③ `MetricsRegistry`（`agentx-service-kit/src/lib.rs`）是无标签静态白名单 gauge（12 个 `&'static str`），按 Tenant/Provider 维度暴露指标需先改造 service-kit 支持标签（或降级为预定义汇总名）；
- Helm：八服务 replicas:1 / maxReplicas:4。勘察修正：`maxReplicas` 无任何 HPA 模板引用（纯声明死字段，与 V2S-005"扩缩容交用户平台"一致）；resources **硬编码在模板**（如 `deployment-workflow-runtime.yaml:66`）非 values 可配——容量 Run 绑定环境规格冻结前可能需要加 values 透传。

## 3. 实施阶段

### P7-D1 Provider 对接测试收口（todolist 唯一条目）

- [x] RAGFlow 集群内 fixture：`deploy/kustomize/e2e-fixtures/runtime-providers` 增加 RAGFlow（含依赖）——kustomization 公共 labels 段会自动给所有资源打 `agentx.io/runtime-provider: allowed` 标签，Namespace 标签机制也已就绪（agentxctl `ensure_namespaces`）；勘察新增两个硬约束：**端口白名单是静态 Helm 模板**（`networkpolicy-runtime-provider-egress.yaml` 只放行 8080/8081/8090/9621/8000，RAGFlow 容器监听 9380——Service 需把白名单端口映射到 9380 或扩模板）；**`httpProviderServices` 只能改 e2e 所用 values 文件**（conftest 注入不了 env）——2 个 skip 用例转默认执行；
- [x] RAGFlow fixture 独立开关（marker/env），避免与容量 Run 抢单节点资源（镜像数 GB + 依赖重，与 8 服务 + 双 MySQL + ClickHouse 并存紧张）；
- [x] RAGFlow fixture 健康等待（`/v1/system/healthz`，rollout 预算参考 lightrag 600s 先例）与 dataset 预置 Job（照抄 `lightrag-tokenizer-cache` Job 形态：集群内 Job 调 RAGFlow API 建 dataset，id 注入 `AGENTX_E2E_RAGFLOW_DATASET_ID`；`ALIAS_BASE_URL` 指向同 Service 的无别名形式解除第 2 个 skip）；
- [x] OpenSandbox 拉起编排：`deploy/opensandbox` 提供一键脚本/Profile（或 pytest fixture 尝试自动拉起，失败再 fail-fast 并输出指引），消除"人工预启动"环节；
- [x] 长期记忆成功 recall 用例：Application Session 内两轮对话，断言 Mem0 写入与召回（受 subject 作用域约束）；
- [x] 固化主链路为可重复入口（`-m product` 一键），证据归档；勾选并清空 `docs/todolist.md`。

门禁：真实集群 Run failures=0、RAGFlow 用例 skipped=0。

### P7-D2 容量阈值冻结（V2S-006 前置）

- [x] 产出 `docs/planv2/evidence/capacity-thresholds.md`：按 `04-e2e-acceptance.md` §3 七组指标（Invocation 错误率/p95/p99/Admission Reject、SSE 建连重连/Drain、Attempt/Outbox/Inbox/Trace 最大年龄、MySQL 连接/锁等待/慢查询/IOPS、Redis 内存/Pending/Lag/重建、Provider/Sandbox 隔离池并发与熔断恢复、2 小时残留）绑定测试环境规格写死数值；
- [x] 阈值评审冻结后任何容量 Run 不得更改（门禁语义）。

### P7-D3 背压与公平限流实现（V2S-006 缺失实现部分）

- [x] 分布式准入：Gateway Admission 检查（租户并发 Invocation、队列深度水位）→ 429 + `Retry-After`（复用 quota 投影 MySQL 语义）；
- [x] 按维度公平限流：Tenant × Capability × Provider 的在途上限（worker 派发侧，`AGENTX_PROVIDER_MAX_INFLIGHT` 默认 32，`PROVIDER_BUSY`）与 Provider 熔断（连续 5 次失败开断 30s 冷却半开探测，`PROVIDER_CIRCUIT_OPEN`，`provider_breaker.rs` 4 项单测）；
- [ ] 队列水位：Redis Stream 情况暴露 + 水位超限拒绝（过载可解释拒绝、无雪崩）；
- [x] metrics 暴露（现有 `/metrics` 扩展）供容量脚本采集（`agentx_admission_rejections_total` 等，capacity 域 `test_metrics_endpoint_exposes_admission_counters` 断言）。

门禁：单测 + 故障注入 E2E（打满租户配额 → 429 + Retry-After；Provider 故障 → 熔断与恢复）。

### P7-D4 容量编排重建与执行（V2S-006 + INT-012 + V2C-005）

- [x] 重建容量编排：`tests/e2e` 新增 capacity 域（pytest marker `capacity`），负载生成器（异步 Invocation 打点）、指标采集（metrics 抓取 + MySQL/Redis/CH 水位查询）、阈值断言与报告 JSON（写 `.local/artifacts/e2e/<run_id>/capacity/`）；
- [ ] 执行矩阵：100 Execution / 500 Node / 200 SSE / 1000 Case / 200 节点 Workflow / 5000 Attempt 分级 Run + **2 小时稳定性 Run + 残留断言**（Lease/Reservation/Outbox/Inbox/Hold 业务残留=0）；
- [ ] 副本矩阵：Gateway/Coordinator/Worker/SSE/Trace 独立扩容 Run（V2C-005 冻结生产基线）；
- [ ] E2E-V2-012 滚动版本兼容：当前/上一版本混跑、超窗在执行前拒绝（勘察新增前置：e2e `--values` 单文件单镜像 tag，双版本混跑需双 values + `--target` 定向升级组合编排，当前 agentxctl 不支持 per-service 版本混布）。

门禁：全部指标 ≤ 冻结阈值，报告归档。

### P7-D5 双阶段滚动升级真实验证（INT-009 + V2K-006）

- [x] `tests/e2e/upgrade` 扩展：双阶段（expand→滚动→contract）、持续 Invocation 探针（升级期间持续打流量断言无损）、未知协议版本任务不被旧 Worker 领取断言（`test_rolling_upgrade_probe.py`，published_at 前移证明确实重投 + WORKER_TASK_MISMATCH 日志）；应用回滚可用已由 `test_release_history.py` 覆盖。双 tag Previous→Candidate 混跑受 agentxctl 单镜像 tag 限制，进长时移交清单；
- [x] 真实本地集群 Run 证据（2026-09-29 `pytest tests/e2e/upgrade -m upgrade` **8 passed** skipped=0，timeline/事件/资源/报告归档于 run 目录 `upgrade-rolling/`）。

### P7-D6 备份恢复真实演练（V2K-005）

- [x] 在五字段 Adapter 契约测试之上补真实 PITR 演练：业务数据写入 → backup → 继续写入 → restore 到恢复点 → 数据校验；RPO/RTO 计时入报告（2026-09-29 全绿，pod 内 dump/restore 避免 BINARY(16) 经文本管道损坏）；
- [x] Redis 丢失重建（从 Runtime MySQL Outbox/状态重建）演练计时（2026-09-29 全绿，FLUSHALL 后新 Invocation 正常完成）；
- [x] 证据含 `providerReceipt` 五字段（pitr-report.json 按同构字段生成并断言）。

### P7-D7 供应链与安全产物（INT-011 + V2K-003）

- [x] 重建供应链链路（Python，入 `tools/scripts/release/`）：11 镜像对齐的 SBOM 生成（syft）、cosign 签名与 Attestation、`verify_release.py` 验签扩展均已实现（`supply_chain.py`；本机缺 syft/cosign 时报告所需命令不静默）；真实签名链执行随 D7 其余项移交。
- [x] Role 级 Secret/ServiceAccount 拆分验证（`test_workload_credentials.py`：8 工作负载专用 SA 断言 + 跨平面 Secret 引用检查，2026-09-29 全绿）；
- [ ] 双租户攻击矩阵 Run：跨租户 ID 猜测、伪造签名、幂等重放、未认证调用、日志泄密 5 项已真实执行全绿（`test_attack_matrix.py` 2026-09-29）；Sandbox Handle 失效等 3 项移交；
- [ ] 本地 TLS Registry 签名链验证。

### P7-D8 最终发布审查（V2C-006 + INT-014）

- [ ] 前置：D2–D7 证据齐备；
- [ ] 发布汇总器 Run：全新集群安装 → MVP 十二步 → 升级 → 回滚 → 容量 → 恢复 → 安全全链路，JUnit/HTML/Trace 证据汇总（汇总器已实现并验证绿/门禁双路径：`tools/scripts/release/release_summary.py`，任何域缺失或超阈值不生成 passed 标记）；
- [ ] Runbook 与 Schema Catalog 更新（运维手册：扩容、恢复、轮换、故障处置）；
- [ ] 更新两个追踪矩阵全部剩余行为 done（gVisor/Kata 等明确移交项除外，单独标注移交计划）；
- [ ] 发布 Release Manifest，版本从 beta 转正决策交由评审。

## 4. 执行口径

1. 所有 Run 使用临时 Namespace + `--scale-down-development`，结束删除 Namespace 并恢复开发服务；
2. 证据统一 `.local/artifacts/e2e/<run_id>/`（timeline、脱敏日志、resources/events、报告 JSON），验收以 `failures=0、skipped=0` 为准；
3. 阈值一经冻结不得调参后重跑替代门禁（planv2 §7 全局完成定义）；
4. 容量/升级/恢复 Run 建议夜间或低峰执行（2 小时稳定性 Run 需要独占窗口），可结合 OffPeak 执行编排迭代。

## 5. 完成定义

- `docs/todolist.md` 为空；
- `docs/planv2/99-traceability.md` 与 `docs/plan/99-feature-traceability.md` 无 planned/in_progress/blocked（或显式移交项）；
- V2S-006/V2C-005/V2C-006/V2K-001~006/INT-009/011/012/014 全部有可复现证据文件；
- 容量与稳定性指标全部 ≤ 冻结阈值，2 小时残留=0；
- 备份恢复 RPO/RTO、滚动升级无损、供应链签名链可验证；
- README 中"不建议生产"自述可以移除（或明确剩余边界）。

## 6. 风险与边界

- 容量 Run 对本地集群规格敏感：阈值必须绑定环境规格冻结，换环境需重新冻结；resources 硬编码在模板、metrics 无标签维度是 D3/D4 的隐藏前置（见 §2.4 勘察新增），排期时预留；
- RAGFlow fixture 镜像较大，首次拉取时间纳入 fixture 超时预算（参考 lightrag 600s 先例）；
- gVisor/Kata 强隔离不在本线，矩阵中以"移交后续计划"标注，不伪装完成；
- 历史容量脚本/故障注入脚本已删除，D4/D5 是重建不是恢复——按现行 `pytest tests/e2e` 编排规范写 Python，不引入新脚本形态。

## 2026-10-07 审查修订与认证门禁

容量入口已改为真实应用发布、认证调用和终态验证；全 401、零完成、缺指标均失败。七组指标由服务 /metrics、Runtime/Control SQL、MySQL performance_schema 和 Redis INFO/XINFO 持续采集。每个分级场景独立检查冻结的 p95/p99，不能用两小时低负载汇总稀释早期超标。200 节点工作流的边界 Exit 不生成 NodeAttempt，报告分别记录 workflowNodes 与实际执行行数。

Gateway 负载生成器在临时 namespace 内复用现有 Python 镜像与 uv 冻结依赖，直连 Ready Pod；1/2/3/4 副本阶段记录各副本实际接受数与脚本/依赖摘要。本机 port-forward 的转发耗时不参与 Gateway 负载延迟测量。200 SSE 使用未完成调用与连接屏障，由实时 gauge 确认全部订阅同时在线后恢复 Worker，再验证重连、排空和订阅残留；不能把单 Pod 的 Service port-forward 或已完成调用回放写成扩容、在线容量证据。

`--capacity-smoke` 为显式短跑，只能产生 incomplete 证据。正式认证要求冻结硬件、100 Execution、500 Node、200 SSE、1000 Case、200 节点、5000 Attempt、四服务独立 1/2/3/4 副本、不同镜像的 Worker 实际混跑及两小时连续采样。`--previous-worker-image` 必须提供真实可兼容旧镜像，不能把当前镜像换 tag 冒充旧版。

候选证据共用 runId/sourceCommit/sourceTreeSha256/imageManifestSha256。最终汇总器要求九域 JUnit 全部非空、失败/错误/跳过为零，容量、PITR、Redis 重建、滚动升级及供应链专项报告与同一候选匹配。所有 11 镜像必须为摘要引用；旧 passed marker 先删除，缺证据不能发布。

供应链必须实际调用 Cosign verify 和 verify-attestation，并核对签名/attestation subject、SPDX 文件与其规范化摘要。缺 Cosign/信任策略、缺镜像或篡改均阻断。本地 TLS Registry 使用显式公钥和 CA，以及 Cosign 3 的无公共 tlog signing config；公网发布采用固定 OIDC 证书身份与 issuer。inspect-only 和 skip-sign 不产生 passed。GitHub 发布工作流先在绑定硬件的 macOS self-hosted runner 完成全链认证，再由 publish job 重新验签和审查证据。

本轮修复与验证结果见 [代码审查修复证据](evidence/p7-code-review-fixes.md)；代码完成不等于正式发布认证通过。
