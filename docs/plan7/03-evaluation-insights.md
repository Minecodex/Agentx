# P7-C：评测与分析面补全

## 1. 目标与边界

三件事，全部是"后端已有或数据已有、产品面缺失"的还账项：

1. **llm_judge 进 UI**：后端契约完整、前端未暴露；
2. **评测对比报告**：跨 Run/版本对比（`docs/05-platform-business.md` §12 已宣称、未实现）；
3. **Insights 聚合页**：错误分布、成本趋势、成功率、节点耗时四类图（ClickHouse 聚合 API 已有、无 BFF 无页面）。

不做：docs/05 §12 中其余评测器类型（Tool 是否正确调用、循环检测等）——本计划结束时在文档中明确标记为未实现后续项；A/B 实验、线上流量分层、CI 回归门禁 API；自定义代码评测器（`EVALUATOR_UNSUPPORTED` 维持）。

## 2. 现状事实

### 2.1 llm_judge：后端完整、UI 缺失

- 后端接受并校验 `llm_judge`（`configuration.modelId` UUID + `configuration.prompt` ≤64KiB，`dataset_api.rs:863-880`）；缺模型授权时 422 `MODEL_EVALUATOR_GRANT_REQUIRED`（`work_packages.rs:1396-1399`）；
- 执行链完整：prompt 上传为不可变对象 → bundle-builder 合成 2 节点 judge workflow（`agentx-bundle-builder/src/lib.rs:827-910`）→ runtime 子执行（`work_package_execution.rs:296-496`）→ `converge_model_evaluator` 评分回写（:498-593，`detail.modelResult` 已带完整 judge 输出）；
- 前端 Profile 对话框规则类型只有 `['exact','contains','regex','json_schema']`（`evaluation-profile-dialog.tsx:75`），configuration 是裸 JSON textarea（:77）；i18n 同步缺 llm_judge（`locales/*/evaluations.ts:59-64`）。

### 2.2 评测对比：无任何 API

- `GET /api/v1/evaluations` 只返回单 Run 列表（`governance_api.rs:591-603`）；报告 `get_evaluation_report`（:611-678）为单 Run 结构；指标卡固定 8 项（:680-737）；
- 数据源具备：`evaluation_case_projection` + `evaluation_rule_results` 已由 projector 全量投影（`projector.rs:416-445`），含 targetExecutionId/actualOutput/ruleResults/cost；
- 评测详情页 4 张 MetricCard + 逐 case 手风琴（`evaluation-detail-page.tsx:41-42`），无对比、无图表。

### 2.3 Insights：聚合 API 完整但三个缺口

- `aggregates:query` 契约（`observability/src/main.rs:1089-1168`）：metrics `count/durationMillis/costMicros/inputTokens/outputTokens/errorRate`、维度 `hour/day/workflow/application/status/errorCode/provider/resourceType`（≤3 维）、filters 白名单、窗口 ≤30 天、limit ≤1000、租户并发 4、5s 超时 + KILL QUERY；表 `workflow_trace_events`（0002 迁移，TTL 180 天）字段齐全；
- **缺口 1**：消费端写入 `workflow_id`/`application_id` 恒 NULL（`main.rs:145-146`；`TraceEventEnvelopeV1` 无这两个字段，`query.rs:487-526`）→ 按 workflow/application 聚合当前出不了数。勘察利好：`TraceEventEnvelopeV1` 唯一构造点在 `trace_delivery.rs:273`，且 `enqueue`（:224-229）本就查询 `workflow_executions`——把该查询扩成 JOIN（workflow_id 直取、application_id 经 invocation_id JOIN `application_invocations`）即覆盖全部 34 处发射点，**无需逐点改发射代码**；
- **缺口 2**：`traces:search` 与 `aggregates:query` 无公共暴露——platform-control 只代理了 trace/span 两个端点（`runtime_bff.rs:56-60`），委托 JWT + request hash 模式已有先例（:927-1015）；
- **缺口 3**：`traces:search` 无 workflowId 过滤、无分页 next；metrics 无分位数（p50/p95）；
- 前端**零图表库**（package.json 无 recharts 等，瀑布图用 CSS div）；`GET /dashboard/summary` 的 5 项运行指标返回 null（`operations_api.rs:34-51`），仪表盘成功率/成本卡显示 0/—。
- 勘察修正：executions 列表**不支持 errorCode 筛选**（前端 `executions-page.tsx:50-51`、BFF `ExecutionListQuery`、runtime 契约 `ExecutionSearchRequestV1` 三层均无此参数；时间窗 createdAfter/createdBefore 已支持）——§3.3 的"错误分类点击跳转 executions 带参过滤"需要新增 errorCodes 全链路筛选（先决定案 3，开发期无兼容负担直接加）。

## 3. 设计

### 3.1 C1 llm_judge 进 UI

- Profile 对话框：规则类型下拉加 `llm_judge`；选中时 configuration 区从 JSON textarea 切换为专用表单——模型选择器（复用 `/models` 列表，只列可授权模型）+ prompt 多行编辑器（64KiB 前置校验、支持 `{{actualOutput}}`/`{{expectedOutput}}` 插入按钮）+ 结构化输出说明（judge 需输出 `{passed, score, reason}`）；
- 保存前调用既有校验；`MODEL_EVALUATOR_GRANT_REQUIRED`（start 时 422）在 Run 启动处映射为可读文案 + 指向资源授权页；
- 报告页 rule 行对 `evaluatorType==='llm_judge'` 展开 `detail.modelResult`（judge 的完整结构化输出：passed/score/reason）；
- 后端**零改动**（契约已冻结）——唯一例外（勘察新增）：judge 子执行 Trace 链接当前不可达——`evaluator_execution_id` 只存在于运行库，投影事件 `RuntimeEvaluationRuleResultV1` 不携带该字段、控制库恒 NULL。最低成本修复：runtime 在 `converge_model_evaluator` 写 `detail_json` 时补 `evaluatorExecutionId`（一行；detail 是自由 JSON，不动冻结契约，projector 自动透传）。

### 3.2 C2 观测面维度修复（Insights 与失败分布的共同前置）

- `TraceEventEnvelopeV1` 增加 `workflow_id`/`application_id` 可选字段（契约版本升级；**`#[serde(default)]`**）。升级顺序约束（勘察新增）：observability 消费循环对反序列化失败是整体重试（`main.rs:475-481`，会卡死消费），**observability 必须先于 runtime 升级**——同仓同版本发布风险可控，但升级 E2E 用例要覆盖此顺序；
- Runtime 发射处补字段：execution 级 span 与节点 span 构造时从执行上下文带入（`engine_trace.rs`、`worker_runtime*.rs` 各发射点）；
- ClickHouse 0003 迁移：无 DDL 变化（列已存在），只需确认写入路径；历史数据不回填（开发期不保留历史兼容）；
- `traces:search` 顺带加 `workflowId` filter 与 `nextCursor` 分页（聚合页跳转 Trace 列表需要）；
- metrics 增加分位数 `durationP50`/`durationP95`（`ObservabilityMetricV1` + `metric_sql`，ClickHouse `quantile` 函数）；
- 勘察新增：`aggregates:query` 无 `span_name` 维度（`dimension_sql` 只有 hour/day/workflow/application/status/errorCode/provider/resourceType，`main.rs:1216-1229`）——§3.3 图 4"节点耗时分布"需要新增 `spanName` 维度枚举 + dimension_sql 分支，属本线契约变更，随 C2 一并冻结。

### 3.3 C3 Insights BFF 与页面

- platform-control 新增 `insights_api.rs`：`POST /api/v1/insights/aggregates`（权限 `runtime:view` 复用或新 `insight:view`，倾向复用；权限检查用标准 `actor.require`，不抄 trace 端点 `permissions.any(...)` 的非常规写法）→ mint 委托 JWT（scope `observability.aggregate.read`）→ `x-agentx-request-hash` 转发 aggregates:query；**request_hash 必须与 observability 侧逐字节一致**（hash 基准是 `json!({"operation":"aggregate-query","request":request})` 的 serde 序列化，`main.rs:1115`）——BFF 直接以 `ObservabilityAggregateRequestV1` Rust 类型构造再序列化，禁止手拼 JSON（键序/Option null/枚举 snake_case 任何偏差都会 401）；勘察新增：现有 `observability_json` helper 把一切非 2xx 折成 503，区分 422 `OBSERVABILITY_QUERY_BUDGET_EXCEEDED` 需要扩展该 helper 读响应体 code；降级（ClickHouse 缩容 0）映射为 `INSIGHTS_DEGRADED`，前端显示降级提示（Trace 降级先例：`test_playwright.py:111-135`）；dashboard 与 Insights 共享每租户 4 并发信号量，仪表盘自动轮询需加退避/降频避免吃掉配额；
- 前端新页面 `features/insights/insights-page.tsx`：

```text
筛选栏：时间范围（复用 date-time-picker）、Workflow（多选）、Application、Status
图 1  成功率/失败趋势        -- 维度 day × metrics errorRate/count（line）
图 2  成本趋势              -- 维度 day × costMicros，可切 input/output tokens（line/bar）
图 3  错误分布              -- 维度 errorCode × count（bar，Top N）
图 4  节点耗时分布          -- 维度 span_name 或 workflow × durationP50/P95（bar）
点击错误分类 → 跳转 executions 列表（带 errorCode/时间窗 URL 参数，现有筛选已支持）
```

- 图表库选型：引入 **recharts**（React 生态成熟、维护活跃、与 Tailwind 令牌可对接，AGENTS.md 规则 12/15 优先成熟库）；图表色值统一走 `--ui-*` 语义令牌保证深浅主题一致；
- 仪表盘 `dashboard/summary` 的 5 项 null 指标：顺带接通（控制面调同一 insights BFF 拿当天汇总）或从响应中删除 null 字段——**采取接通**，仪表盘四卡数据补全为真实值；
- 遵循页面新增模式：router 注册（`RequirePermission`）、navigation.ts runtime 组、双语言 i18n、OpenAPI → generated.ts 再生成。

### 3.4 C4 评测对比报告

- 新 API：`GET /api/v1/evaluations/compare?runIds=a,b`（2..=5 个 Run；数据源 `evaluation_case_projection` + `evaluation_rule_results` + `evaluation_runs`，服务端按 `sourceCaseId` 对齐用例）：

```text
响应：{
  runs: [{runId, name, workflowVersionId, datasetVersionId, profileVersionId, metrics(8项)}],
  caseDeltas: [{caseKey, baselineStatus, candidateStatus, scoreDelta, costDelta}],
  ruleAggregates: [{ruleKey, name, evaluatorType, passRateByRun: {...}}],
  perCaseTraceLinks: [...targetExecutionId]
}
```

- 同一 Workflow 不同版本、或不同 Profile 对同一 Dataset 的 Run 均可对比；case 不对齐（Dataset 版本不同）时按 caseKey 交集对齐并在响应标记 `alignedCaseCount/totalCaseCount`。勘察新增：`evaluation_case_projection` **无 case_key 列**——对齐需每个 Run 各自 JOIN 其 dataset_version 的 `dataset_version_cases`（报告查询 `governance_api.rs:641` 已有先例）再按 case_key 求交集；跨 Profile 对比时 rule_key 集合不同，`ruleAggregates` 按 rule_key 交集对齐、缺失侧标记 absent；
- 前端：评测列表页加"对比"多选入口；详情页增加"对比"Tab（指标对照表、逐 case 状态变化列表、规则通过率对比条形图）；失败 case 一键跳转两侧 Trace；
- 失败节点分布/工具错误分布：作为评测详情页的一个区块，按 `targetExecutionId` 集合批量查询观测面（依赖 3.2 维度修复 + traces:search errorCode/resourceType 过滤），展示 Top 失败节点与工具错误码。勘察新增成本提示：`traces:search` 现只支持单 execution_id 过滤且 `next` 恒 None（`main.rs:1084`，契约有 cursor 字段但实现未用）——批量场景按循环调用实现（窗口/limit 预算内），不为此扩契约。

## 4. 实施阶段

### P7-C1 llm_judge UI

- [x] Profile 对话框规则类型 + 专用配置表单（模型选择器/prompt 编辑器/变量插入/64KiB 校验）；
- [x] i18n 双语词条；`MODEL_EVALUATOR_GRANT_REQUIRED` 文案与授权页跳转；
- [x] 报告页 llm_judge 结果展示（modelResult 展开组件）；runtime `converge_model_evaluator` 的 detail_json 补 `evaluatorExecutionId`（Trace 链接修复，见 §3.1 唯一例外）；
- [x] vitest 表单测试（校验路径、类型切换）。

门禁：前端测试、Playwright 评测域回归。

### P7-C2 观测面维度与指标修复

- [x] `TraceEventEnvelopeV1` 加 workflow_id/application_id（`#[serde(default)]`）+ `trace_delivery.rs enqueue` 扩 JOIN 覆盖全部发射点 + observability `TraceRow` 映射（契约测试同步；升级顺序 observability 先于 runtime，见 §3.2）；
- [x] `traces:search` 加 workflowId filter + 游标分页；metrics 加 durationP50/durationP95；
- [x] observability 契约测试与查询单测（维度分组正确性、分位数）。

门禁：契约测试、ClickHouse 真实查询集成测试（testcontainers 模式）。

### P7-C3 Insights BFF 与页面

- [x] platform-control `insights_api.rs`（委托转发 + 降级/预算错误映射 + OpenAPI）；
- [x] recharts 引入与图表主题令牌封装（`shared/components/charts/` 统一图表组件层，保证风格一致）；
- [x] insights 页面四图 + 筛选 + 降级态 + 跳转联动（errorCodes 全链路筛选：前端 URL 参数 → BFF → `ExecutionSearchRequestV1`，见 §2.3 勘察修正）；dashboard summary 接通真实指标（带退避，见 §3.3）；
- [x] 路由/导航/i18n/OpenAPI 再生成。

门禁：BFF 契约测试（mock observability）、Playwright（含 ClickHouse 缩容降级场景）。

### P7-C4 评测对比

- [x] compare API（服务端 case 对齐逻辑 + OpenAPI）；
- [x] 列表页对比入口 + 详情页对比 Tab（对照表/case 变化/规则通过率图）；
- [x] 失败节点分布区块（批量 trace 查询 + Top 列表）。

门禁：API 契约测试、前端测试、Playwright。

### P7-C5 E2E 验收

- [ ] 见第 5 节（本线计划门禁为契约/ClickHouse testcontainers/BFF mock/vitest/Playwright，均已绿；§5 的集群级 E2E 七场景未执行，移交清单）。

## 5. E2E 验收（临时 Namespace）

1. UI 创建含 `llm_judge` 规则的 Profile（选择模型、填 prompt）→ 发起评测 → 报告页断言 judge 规则结果与 `modelResult` 展示、judge 子执行 Trace 链接可跳转；
2. 未授权模型启动评测 → 422 文案与授权引导正确；
3. 同一 Workflow 两版本各跑一次评测 → 对比页指标对照、case 状态变化、规则通过率图正确；Dataset 不同的 Run 对齐计数标记正确；
4. Insights 页四图在真实执行数据下出数；按 workflow 筛选生效（验证维度修复）；错误分类点击跳转 executions 带参过滤；
5. ClickHouse 缩容 0 → Insights 页显示降级提示，执行不受影响；恢复后图表恢复；
6. 仪表盘成功率/成本卡显示真实值；
7. UI 覆盖：中英文、深浅主题、空数据态、30 天窗口边界、并发限制提示（可选）。

## 6. 完成定义

- llm_judge 从创建到报告全链路 UI 可用，无裸 JSON 配置入口；
- 任意 2–5 个评测 Run 可对比，case/rule/指标三个层次对齐正确；
- Insights 页四类图基于真实 Trace 出数，支持 workflow/application/错误码维度，降级有明确 UX；
- `workflow_trace_events` 的 workflow/application 维度写入非空并有断言；
- 仪表盘指标无 null 占位；docs/05 §12 与实现对齐（未实现项显式标记）。

## 7. 明确不做

- 自定义代码评测器、Tool 调用正确性/循环检测等新评测器类型（文档标记为后续）；
- A/B 实验、线上流量对照、CI 集成回归门禁；
- Trace 数据回填历史；Prometheus/Grafana 集成（roadmap 首期边界）；
- 自绘图表（统一 recharts，避免两套图形体系）。
