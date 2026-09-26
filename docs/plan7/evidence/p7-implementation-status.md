# plan7 实施状态与移交清单（2026-09-26）

分支 `plan7`，提交序列（截至本文）：`64159e3` (P7-A) → `9ed494a` (P7-B 核心) → `882ed7e` (P7-B 多模态) → `bba06bb` (P7-E) → `72a8208` (P7-C C2) → `4533320` (P7-C C1/C3/C4) → `9cc0d7d` (P7-D3) → `d1ca921` (P7-D4/D7)。

## 已完成（代码 + 本地门禁全绿）

每条线均通过：`cargo fmt` / `cargo clippy -D warnings` / boundary check（含 2000 行与表归属）/ ruff / acceptance 契约测试 / 相关 crate 单测 / vitest 404 / 前端 build / OpenAPI→generated.ts 再生成。

| 线 | 状态 | 说明 |
|---|---|---|
| P7-A 出站回复 | 代码完成 | 契约/迁移/delivery 状态机/三平台 Send Client/三层回复/BFF/前端；E2E 待集群 |
| P7-B 流式+多模态 | 代码完成 | model.delta 旁路(结算前 flush)/SSE 聚合同构结算/usage_estimated/capabilities 链/原生 parts/modality 投递/前端增量气泡 |
| P7-C 评测与 Insights | 代码完成 | 维度单点修复+P50/P95+spanName/llm_judge UI+judge Trace 链接/insights BFF+recharts 四图/对比报告 API |
| P7-D1 | 代码完成 | RAGFlow fixture(含依赖)/conftest 预置(RSA 注册/建 dataset)/OpenSandbox 一键/记忆成功召回用例 |
| P7-D2 | 完成 | docs/planv2/evidence/capacity-thresholds.md 冻结 |
| P7-D3 | 代码完成 | 网关分布式准入(429+Retry-After)/admission 指标；公平限流与 Provider 熔断在移交清单 |
| P7-D4/D7 | 入口完成 | capacity 域编排/供应链脚本(11 镜像对齐)；分级 Run 在移交清单 |
| P7-E 知识库 | 代码完成 | 协议共享抽取(delete 显式拒绝)/文档上传索引/hit-testing/前端三区 |

## 待执行的集群命令（镜像拉取恢复后）

本机 Docker Hub 拉取在本次会话窗口内受限（RAGFlow/ES 镜像未完成），以下命令在镜像就绪后依次执行：

```bash
# 1. 构建并导入全部本地镜像（若 xtask images 尚在进行则等待完成）
PATH="/opt/homebrew/opt/helm@3/bin:$PATH" cargo xtask images --values deploy/values/local.yaml

# 2. 全量 Rust 门禁（ClickHouse testcontainers 镜像就绪后）
PATH="/opt/homebrew/opt/helm@3/bin:$PATH" cargo test --workspace

# 3. D1 provider 收口 Run（RAGFlow 用例转默认执行）
uv run --group test pytest tests/e2e/product/test_provider_integration.py \
  --values deploy/values/local.yaml -m product --timeout=3600

# 4. P7-A/B/E 功能 E2E（按各线第 5 节补齐 mock fixture 后）
uv run --group test pytest tests/e2e -m "runtime or product" \
  --values deploy/values/local.yaml --scale-down-development --timeout=7200

# 5. 容量窗口（专用窗口，关闭 RAGFlow fixture）
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
