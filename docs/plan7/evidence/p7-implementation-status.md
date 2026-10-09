# plan7 实施状态与移交清单（2026-09-26）

2026-10-07 的安装入口与 TLS 修复见 [P7-D 安装/TLS 证据](p7-d-ctl-tls-installation.md)。开发和生产统一使用 ctl；明文/TLS 安装 E2E 为 6 passed，本地 TLS 验收与外部生产基础设施、TLS Registry 和最终发布认证分别记录。

同日的本地 production Profile 镜像重建与重部署见 [P7-D 本地生产部署证据](p7-d-local-production-redeploy.md)：11 个镜像固定摘要、8 个应用工作负载就绪、四个 Doctor 通过，旧 E2E 与调试环境已清理。

2026-10-07 的已推送代码审查修复与当前验证状态见 [代码审查修复证据](p7-code-review-fixes.md)。以下 2026-09 的完成与移交记录属于历史快照；本轮对知识库状态、流式可靠性、评测比较、Insights 与发布认证作了修订，最终门禁以新证据为准。

上一轮已完成集群验证的源码候选 `60d80309…`：产品流程 12 passed、恢复与当前候选升级演练 4 passed、容量短测 24 阶段/七组门禁通过，11 个镜像本地签名与负向信任检查通过。知识库恢复/权限、IM 官方协议、Studio/Playground、judge/比较页与 Insights 均有实际业务和桌面浏览器证据；容量发现的租约覆盖、Trace 主行竞争、恢复重复派发与评测历史行锁已修复并复测。所有临时 namespace 已清理，原有暂存改动保留，未提交或 push。完整日志、候选摘要和边界见上述修复证据；正式同 Run 九域、两小时及公网 release-ci 认证尚未完成。

2026-10-08 回顾又修复了 Trace 查询限幅与委托 Hash 不一致、发布门禁误放行非法数值两类遗漏，并修正 IM mock 在 Python 优化模式下失效的协议校验。源码摘要更新为 `d041bea6…`，Control/fixture 69 项与相关 Python 129 项回归通过，完整 acceptance 为 138 passed、1 个既有布局冲突失败；全仓 Ruff/格式、V2 boundary check 与优化模式下四个 IM 请求检查通过。该次回顾结束时尚未重建镜像或执行新候选集群 E2E；RAGFlow 管理面组合验收、Redis 水位专项、安全剩余项与正式发布收口仍未完成，不能将 plan7 标为全部完成。

随后按用户要求重建并升级了本地环境：候选 `d041bea6…` 的 11 个正式镜像使用 `plan7-local-20261008` 标签并固定摘要，8 个业务工作负载 Ready，四个 Doctor 通过，现有管理员账号保留。该候选的专项界面 E2E 为 1 passed，包含 2 个 Playwright 用例；临时 namespace 已清理。详见 [本地升级证据](p7-d-local-production-redeploy.md#plan7-最新修复本地升级2026-10-08) 与 [桌面端手动界面验收流程](../README.md#7-桌面端手动界面验收流程)。这次专项验证不替代全量产品 E2E、两小时容量与正式发布认证。

分支 `plan7`，提交序列（截至本文）：`64159e3` (P7-A) → `9ed494a` (P7-B 核心) → `882ed7e` (P7-B 多模态) → `bba06bb` (P7-E) → `72a8208` (P7-C C2) → `4533320` (P7-C C1/C3/C4) → `9cc0d7d` (P7-D3) → `d1ca921` (P7-D4/D7) → `daa4da1` (全量测试绿) → `2f2959b`/`cc370a4`/`cdda0b1` (D1 集群修复)。

## 已完成（代码 + 本地门禁全绿）

每条线均通过：`cargo fmt` / `cargo clippy -D warnings` / boundary check（含 2000 行与表归属）/ ruff / acceptance 契约测试 / 相关 crate 单测 / vitest 404 / 前端 build / OpenAPI→generated.ts 再生成。

| 线 | 状态 | 说明 |
|---|---|---|
| P7-A 出站回复 | **E2E 全绿(2026-09-29)** | 主链路 1 passed skipped=0: 钉钉 callback 渠道→签名入站→执行→L1 自动回复→im-mock delivered(provider_message_id)→401 死信 DELIVERY_PROVIDER_REJECTED；过程修复 9 个真实缺陷（见下） |
| P7-B 流式+多模态 | 代码完成 | model.delta 旁路(结算前 flush)/SSE 聚合同构结算/usage_estimated/capabilities 链/原生 parts/modality 投递/前端增量气泡 |
| P7-C 评测与 Insights | 代码完成 | 维度单点修复+P50/P95+spanName/llm_judge UI+judge Trace 链接/insights BFF+recharts 四图/对比报告 API |
| P7-D1 | **E2E 全绿(2026-09-27)** | `2 passed` 425s: Playwright 4 用例(LightRAG/Mem0/OpenSandbox 主链路 + RAGFlow 协议端到端/egress 拒绝/协议失配 3 用例默认执行) + 长期记忆成功召回; 证据 run `c2ca3c5c70`(见 .local/artifacts/e2e/), RAGFlow adapter 截图留档 |
| P7-D2 | 完成 | docs/planv2/evidence/capacity-thresholds.md 冻结 |
| P7-D3 | 代码完成 | 网关分布式准入(429+Retry-After)/admission 指标；公平限流与 Provider 熔断在移交清单 |
| P7-D4/D7 | 入口完成 | capacity 域编排/供应链脚本(11 镜像对齐)；分级 Run 在移交清单 |
| P7-E 知识库 | 代码完成 | 协议共享抽取(delete 显式拒绝)/文档上传索引/hit-testing/前端三区 |

## A7 全绿过程中修复的真实缺陷（2026-09-28/29，均只有集群运行才能暴露）

1. **引擎**：start→exit 直连工作流 `ExecutionMachine::new` 后无 activation 也无人推进，永久卡 `running`（此前无任何 e2e 覆盖该形态）；`state.rs` 终态推导 + 回归单测。
2. **网关**：钉钉 sessionWebhook 回调把消息文本放在 JSON 编码的 `content` 字段，`message_text` 只认 `text.content`，文本丢失为空串；补解码路径 + 单测。
3. **delivery.claim**：`target/credential_ref/payload` 列名与 SELECT 的 `*_json` 不匹配。
4. **delivery.complete/fail_retryable/dead**：WHERE 绑定顺序错位（owner/token/id 与 id/owner/token 互换），UUID 绑进 BIGINT 位 → 1292。
5. **delivery.dead**：`INSERT…SELECT…ON DUPLICATE KEY UPDATE id=id` 在 MySQL 的 INSERT…SELECT 形式下列歧义 → 1052；改源别名自更新。
6. **delivery_send.allowed_host**：额外白名单按后缀匹配，集群内服务是 `im-mock.<ns>.svc` 前缀形态 → 全拒；改为前缀 DNS 标签精确匹配（公网 provider 后缀语义不变）。
7. **fixture**：im-mock 缺 `agentx.io/runtime-provider: allowed` 标签，双向 NetworkPolicy 拒绝 8090 → PROVIDER_UNAVAILABLE×5 次重试后死信；行为路径只认末段精确词，`session-unauthorized` 落默认 ok；成功响应缺 `message_id`。
8. **配置链**：`deliveryDomainExtraAllowlist`/`httpProviderServices` 缺 im-mock；chart 与 values schema（4 chart + 顶层）需声明新键，且 agentxctl 内嵌 chart 必须重编才能生效。
9. **测试竞态**：egress 单测改进程环境变量与断言默认行为的用例并行竞态，偶发失败；共享 Mutex 串行化。

## 集群验证状态（2026-09-27 更新）

- ✅ 全部 11 服务本地镜像构建并导入集群；RAGFlow/ES/MySQL/Valkey/MinIO、mem0-server、lightrag(dev)、minio/mc、opensandbox(execd/egress/code-interpreter) 镜像齐备
- ✅ 全量 `cargo test --workspace` 67 个测试目标全绿
- ✅ D1 provider E2E `2 passed`(RAGFlow 用例默认执行, skipped=0)
- ✅ A7 渠道出站回复 E2E `1 passed 233s`(2026-09-29, run25, skipped=0)
- ✅ A7 扩展场景 run26 `1 passed 225s`: 429 退避重试 delivered(attemptCount≥3)、死信重放成功; mock 行为按路径有状态
- ✅ B6 first_token_ms run3 `1 passed 379s`
- ✅ A7 L2 reply_message 节点投递 run27 `2 passed 248s`(skipped=0): runtime_calls 新列(迁移 0012)+Trace span firstTokenMs 属性实测 >0
- ✅ D5/D6 升级与恢复演练 `8 passed 366s`(2026-09-29, skipped=0): 双阶段滚动升级+持续探针零丢失、
  未知协议版本经 backlog 重投但活体 Worker 在 claim 门禁拒绝(WORKER_TASK_MISMATCH)、
  真实 PITR(恢复点前行在/后写行丢/活库不受扰+五字段收据+RPO/RTO)、Redis FLUSHALL 重建存活
- ✅ 过程修复: 网络策略(RAGFlow 栈内互访端口)、object-storage/mc pullPolicy、ragflow kustomize 缩进、publish 等待 202 收敛、流式聚合保 headers、e2e 沙箱摘要本地读取

## 待执行的集群命令（后续窗口）

```bash
# D8 发布汇总（全链证据齐备后）
uv run python tools/scripts/release/release_summary.py \
  --evidence .local/artifacts/e2e --dist .local/dist \
  --junit business=<junit.xml> --junit security=<junit.xml> \
  --output .local/dist/release-summary.json --passed-marker .local/dist/release-gate.passed
```

```bash
# P7-A/B/E 功能 E2E（按各线第 5 节补齐 mock fixture 后）
uv run --group test pytest tests/e2e -m "runtime or product" \
  --values deploy/values/local.yaml --scale-down-development --timeout=7200

# 容量窗口（专用窗口，关闭 RAGFlow fixture）
AGENTX_E2E_RAGFLOW_DISABLE=1 uv run --group test pytest tests/e2e/capacity \
  --values deploy/values/local.yaml --timeout=10800
```

## 2026-09-29 补齐的 P7-B 两个缺口

1. `first_token_ms`：迁移 0012（runtime_calls 列）+ 流式臂首 delta 计时 + 结算 UPDATE + Trace span `firstTokenMs` 属性；B6 E2E 断言实测 >0。
2. `x-agentx-modality`：schema_contract 校验（仅 image/audio、必须 array 型，`INVALID_MODALITY_SCHEMA`）+ 投递单测（标记输入收匹配 part 的 artifact 引用数组、与通用 file 映射共存）。

## 2026-09-29 第二轮补齐（用户指令"修复 1/2/3/5"）——终局 22 passed skipped=0

四套件同集群会话全绿：A7（L1 主链/429 退避/死信重放/L2 reply 节点/L3 send_message 官方 API/幂等/Pod 强杀/REPLY_TARGET_UNRESOLVED）、B6（四流式行为+first_token_ms+vision 原生 parts+能力门禁拒绝）、P7-C API 级（聚合/过滤/降级/仪表盘/未授权 422）、安全矩阵（8 SA+平面 Secret+5 攻击负向）。

**集群验证新发现并修复的真实缺陷（6 个）**：
1. `send_message`/`reply_message` 清单 outputSchema 缺 content/channelId 等字段 → 集群执行 `NODE_OUTPUT_SCHEMA_VALIDATION_FAILED`（本地默认注册表宽容掩盖）；补齐字段+回归单测。
2. 网关附件上传不写 `runtime_objects`（resolve 的 JOIN 落空 → Multimodal artifact is not ready）；上传事务补 ready 行（含占位符计数与 content_hash 前缀两处修正）。
3. `resolve_multimodal_content` 的解析结果被 `claim.node_parameters` 原始绑定覆盖（input.question 优先级修正）；此前 B6 从未验证 userQuestion 绑定内容。
4. 模型节点 `userQuestion` 参数类型收窄为 string，数组输入被丢弃 → 放宽为 [string, array]。
5. Insights BFF `filters` 默认 Null 被观测侧判无效（422 误报 BUDGET_EXCEEDED）→ 默认空对象。
6. dataset import 的 `MAX(sort_order)` DECIMAL 解码 u64 失败 → CAST AS UNSIGNED；另修复 egress 单测环境变量竞态、`resolve_behavior` 非瞬态行为（slow/not-found）永远返回 ok 的 mock 缺陷。

**D7 真实执行**：`local_signing_drill.py`——本地 registry:2 推送 11 镜像 → syft SBOM → cosign 签名 11/11 + attestation 11/11 + 验签 11/11 全绿，证据 `.local/dist/local-signing-drill.json`。

## 移交清单（后续会话，2026-09-29 修订——已完成项移出）

1. **A7 场景 9**：UI 覆盖（回复设置表单/投递记录列表/重试/双语主题）与无 Deployment/停用渠道错误码变体。
2. **B6 场景 7/8**：Agent 节点流式+`first_token_ms` Trace 联动、UI 覆盖（stream 开关/上传预览/双语主题）。
3. **P7-C 场景 1/3/7**：llm_judge 全链报告（modelResult/Trace 链接）、两版本对比页、UI 覆盖。
4. **D4 分级 Run**：100/500/1000/200 节点/5000 Attempt/2h 稳定性+残留断言/副本矩阵/双版本混跑（入口 `tests/e2e/capacity` 已就绪，需独占集群窗口）。
5. **D7 剩余**：攻击矩阵 Sandbox Handle 失效等 3 项、TLS Registry 验证、公网 registry 签名（本地链已全绿）。
6. **D8 汇总 Run**：汇总器已实现并验证（绿路径写 passed 标记/阈值不足阻断），待全链证据齐备后执行；Runbook/Schema Catalog 更新与 Release Manifest 随最终发布评审。
7. **追踪矩阵**：两个 99-*.md 待上述证据落地后统一更新 done；README"不建议生产"自述因此暂不移除。

## 本机环境备忘（复跑 D1 时）

- OpenSandbox Server 以宿主进程运行: `uv tool run --from opensandbox-server==0.2.2 opensandbox-server --config /tmp/opensandbox.conf`（config = deploy/opensandbox/docker/config.local.toml 改 port=18080、sqlite 路径落 /tmp、host_ip=127.0.0.1；须保留 [egress] 段否则 networkPolicy 拒绝）
- egress sidecar 镜像 opensandbox/egress:v1.1.4 必须预拉; macOS 宿主访问沙箱须走 egress sidecar 的 host-mapped 端口(networkPolicy 必配)
- helm 需 3.x: PATH 前置 /opt/homebrew/opt/helm@3/bin（本机默认 helm 4 会被 agentxctl 拒绝）
