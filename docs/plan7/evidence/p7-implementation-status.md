# plan7 实施状态与移交清单（2026-09-26）

分支 `plan7`，提交序列（截至本文）：`64159e3` (P7-A) → `9ed494a` (P7-B 核心) → `882ed7e` (P7-B 多模态) → `bba06bb` (P7-E) → `72a8208` (P7-C C2) → `4533320` (P7-C C1/C3/C4) → `9cc0d7d` (P7-D3) → `d1ca921` (P7-D4/D7) → `daa4da1` (全量测试绿) → `2f2959b`/`cc370a4`/`cdda0b1` (D1 集群修复)。

## 已完成（代码 + 本地门禁全绿）

每条线均通过：`cargo fmt` / `cargo clippy -D warnings` / boundary check（含 2000 行与表归属）/ ruff / acceptance 契约测试 / 相关 crate 单测 / vitest 404 / 前端 build / OpenAPI→generated.ts 再生成。

| 线 | 状态 | 说明 |
|---|---|---|
| P7-A 出站回复 | 代码完成 | 契约/迁移/delivery 状态机/三平台 Send Client/三层回复/BFF/前端；E2E 待集群 |
| P7-B 流式+多模态 | 代码完成 | model.delta 旁路(结算前 flush)/SSE 聚合同构结算/usage_estimated/capabilities 链/原生 parts/modality 投递/前端增量气泡 |
| P7-C 评测与 Insights | 代码完成 | 维度单点修复+P50/P95+spanName/llm_judge UI+judge Trace 链接/insights BFF+recharts 四图/对比报告 API |
| P7-D1 | **E2E 全绿(2026-09-27)** | `2 passed` 425s: Playwright 4 用例(LightRAG/Mem0/OpenSandbox 主链路 + RAGFlow 协议端到端/egress 拒绝/协议失配 3 用例默认执行) + 长期记忆成功召回; 证据 run `c2ca3c5c70`(见 .local/artifacts/e2e/), RAGFlow adapter 截图留档 |
| P7-D2 | 完成 | docs/planv2/evidence/capacity-thresholds.md 冻结 |
| P7-D3 | 代码完成 | 网关分布式准入(429+Retry-After)/admission 指标；公平限流与 Provider 熔断在移交清单 |
| P7-D4/D7 | 入口完成 | capacity 域编排/供应链脚本(11 镜像对齐)；分级 Run 在移交清单 |
| P7-E 知识库 | 代码完成 | 协议共享抽取(delete 显式拒绝)/文档上传索引/hit-testing/前端三区 |

## 集群验证状态（2026-09-27 更新）

- ✅ 全部 11 服务本地镜像构建并导入集群；RAGFlow/ES/MySQL/Valkey/MinIO、mem0-server、lightrag(dev)、minio/mc、opensandbox(execd/egress/code-interpreter) 镜像齐备
- ✅ 全量 `cargo test --workspace` 67 个测试目标全绿
- ✅ D1 provider E2E `2 passed`(RAGFlow 用例默认执行, skipped=0)
- ✅ 过程修复: 网络策略(RAGFlow 栈内互访端口)、object-storage/mc pullPolicy、ragflow kustomize 缩进、publish 等待 202 收敛、流式聚合保 headers、e2e 沙箱摘要本地读取

## 待执行的集群命令（后续窗口）

```bash
# P7-A/B/E 功能 E2E（按各线第 5 节补齐 mock fixture 后）
uv run --group test pytest tests/e2e -m "runtime or product" \
  --values deploy/values/local.yaml --scale-down-development --timeout=7200

# 容量窗口（专用窗口，关闭 RAGFlow fixture）
AGENTX_E2E_RAGFLOW_DISABLE=1 uv run --group test pytest tests/e2e/capacity \
  --values deploy/values/local.yaml --timeout=10800
```

## 移交清单（后续会话）

1. **E2E mock fixture**：P7-A 三平台 IM mock（Kustomize，按路径切换 成功/429/401/目标不存在）、P7-B mock OpenAI 流 fixture（慢流/断流/无 usage/重放）——各线第 5 节已定义行为矩阵。
2. **D3 剩余**：Tenant×Capability×Provider 公平限流（worker 派发侧）与 Provider 熔断（runtime_calls 开窗）；MetricsRegistry 标签维度化。
3. **D4 分级 Run**：100/500/1000/200 节点/5000 Attempt/2h 稳定性/副本矩阵（入口 `tests/e2e/capacity` 已就绪）。
4. **D5/D6**：双阶段滚动升级用例扩展、真实 PITR 演练（`agentxctl migrate --phase` 与 backup/restore 入口均已存在）。
5. **D8**：发布汇总器与 Runbook/Schema Catalog 更新；README"不建议生产"自述按剩余边界处理。
6. **追踪矩阵**：两个 99-*.md 待上述证据落地后统一更新 done。

## 本机环境备忘（复跑 D1 时）

- OpenSandbox Server 以宿主进程运行: `uv tool run --from opensandbox-server==0.2.2 opensandbox-server --config /tmp/opensandbox.conf`（config = deploy/opensandbox/docker/config.local.toml 改 port=18080、sqlite 路径落 /tmp、host_ip=127.0.0.1；须保留 [egress] 段否则 networkPolicy 拒绝）
- egress sidecar 镜像 opensandbox/egress:v1.1.4 必须预拉; macOS 宿主访问沙箱须走 egress sidecar 的 host-mapped 端口(networkPolicy 必配)
- helm 需 3.x: PATH 前置 /opt/homebrew/opt/helm@3/bin（本机默认 helm 4 会被 agentxctl 拒绝）
