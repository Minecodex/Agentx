# 外部知识连接与多来源检索验收

本次实现以 [外部知识连接设计](../11-knowledge-connections.md) 为准。连接、查询配置、Workflow 绑定分离；文档和索引由外部 Provider 维护。

## 实现范围

- 连接独立 CRUD、停用和认证测试；连接内查询 CRUD、真实范围发现、部门与 Workflow 授权、只读外部文档、弹窗检索测试。
- RAGFlow 多 Dataset，逐 Dataset 请求；LightRAG 绑定实例真实 workspace，包括默认空工作区。不同实例使用独立连接，无目录过滤或动态 workspace header。
- 独立节点跨连接多选；Agent 每条查询独立工具，携带业务名称和用途。冻结查询、连接、范围和凭证版本。
- 共享执行器先校验授权，合并重复来源，并发 4、每来源最多 20 候选、RRF 60、最终 Top K 1–20。结果保留逐来源耗时、错误、查询标识、原始分数和融合分数。
- 部分成功、成功空结果、全部失败分别处理；查询和凭证授权撤销在请求前阻止执行。错误出口保留来源详情。
- 删除旧资源接口、本地知识文档表、索引任务、上传和删除文档入口，以及知识结果中的 Artifact 引用。外部文件保留其来源信息。
- 修改初始化基线，无历史数据迁移或兼容入口。已有数据库需要重建，不能用旧基线直接升级。Runtime 状态表按资源版本和内容指纹区分冻结配置。

## 首轮实现验证（r4）

| 检查 | 结果／证据 |
| --- | --- |
| 前端全量 Vitest | 99 个文件、487 项通过；`.local/artifacts/knowledge-web-final-tests.log` |
| 前端构建、浏览器 TypeScript | 通过；`knowledge-web-build.log`、`knowledge-browser-types.log` |
| Runtime 契约、生成器和漂移检查 | 54 项通过；`knowledge-contract-tests.log` |
| Runtime 编译与执行核心 | 87 项通过；`knowledge-runtime-core-tests.log` |
| Runtime 服务 | 165 项通过；2 项需要独立钉钉／飞书凭证的既有测试忽略；`knowledge-runtime-service-tests.log` |
| Control 实际 MySQL API 测试、版本快照测试 | 通过；`knowledge-control-api-test.log`、`knowledge-snapshot-test.log` |
| 格式、Clippy、架构边界、Python lint | 通过；`.local/artifacts/knowledge-format.log`、`knowledge-clippy.log`、`knowledge-boundary.log`、`knowledge-python-lint.log` |
| 修改文件行数 | 63 个源码文件均不超过 2000 行，最大 1906 行；`.local/artifacts/knowledge-connections/source-size-check.json` |
| 旧实现清理 | `src/`、`contracts/`、`tests/`、`deploy/` 无旧资源路由、本地文档表、索引任务、旧范围字段或动态 workspace header 引用；`git diff --check` 通过 |
| 最终 Kubernetes / Playwright | `knowledge-1017-r4`：4 项通过、142 项未选中，耗时 832.53 秒；包含 7 个通过的桌面浏览器场景；`.local/artifacts/knowledge-connections/e2e-r4.log` |
| 清理与恢复 | 测试 Namespace 已全部删除，13 个开发环境 Deployment 的可用副本均达到预期；`.local/artifacts/knowledge-connections/restored-services.json` |

系统验收覆盖四个 pytest 场景：

1. 管理页面：两种协议的连接与查询配置、部门授权、外部文档、检索成功／空结果／失败、停用／删除，中英文和深浅主题。
2. 多来源执行：重复 Dataset 合并、部分失败、正常空结果、全部失败、旧包范围保持不变、新版本使用新范围、引用保护、查询授权撤销。
3. 真实 Provider：原生 API 创建两个 RAGFlow Dataset，两个 LightRAG 实例分别使用 `alpha/beta` workspace；真实 CPU embedding 和已配置模型完成索引。验证隔离、交叉连接检索、外部追加文档可见、凭证依赖及撤销。
4. 画布：两种 Provider 的节点执行、授权、文本／附件、空结果和错误出口，跨连接多选、Agent 多知识附件保存重开，以及执行授权投影恢复。内部包含 5 个 Playwright 场景。

最终证据：

- 多来源执行与真实平台结果：`.local/artifacts/e2e/knowledge-1017-r4/product/knowledge-multisource.json`、`knowledge-native-scopes.json`。真实平台报告包含四个成功来源、外部新增内容及凭证撤销后的 422 拒绝结果。
- 授权恢复：`.local/artifacts/e2e/knowledge-1017-r4/admission-recovery.json`。
- 浏览器 JUnit：`.local/artifacts/playwright/helm-agentxctl/knowledge-1017-r4/` 下 `knowledge-connections/junit.xml`（2 项）、`studio-knowledge/junit.xml`（4 项）、`studio-recovered-execution/junit.xml`（1 项），均无失败或跳过。
- 环境安装、事件、服务日志和清理时序：`.local/artifacts/e2e/knowledge-1017-r4/`；恢复状态检查时间为 `2026-10-17T05:06:30Z`。

统一入口（本机默认 Helm 4，验收指定已安装的 Helm 3）：

```sh
PATH=/opt/homebrew/opt/helm@3/bin:$PATH \
AGENTX_E2E_CPU_EMBEDDING_IMAGE=agentx/cpu-embedding@sha256:9876b5946d3f6c582e0cb2ca93f59d917043fe51e396b73a6ba6a9466ab4f9d8 \
AGENTX_E2E_PLAYWRIGHT_GREP='P8 knowledge (lightrag|ragflow|multi-connection|fixture Agent)' \
uv run --frozen --group test pytest tests/e2e \
  --values .local/artifacts/knowledge-connections/values.yaml \
  --evidence-run-id knowledge-1017-r4 --scale-down-development \
  -k 'knowledge_connection_desktop or multisource_partial_empty_failure_and_frozen_scope or real_external_scopes_and_cross_connection_workflow or knowledge_canvas_and_ai_resource_completion' -q
```

## 迭代与边界

首轮环境校验因默认 Helm 4 终止，未创建测试环境。`r2` 发现测试 Provider 的 Control 网络规则缺口，以及手动追加未固定版本的查询后空字段造成的画布内容指纹不一致；分别修正测试网络规则与统一序列化，并补回归测试。`r3` 的真实平台、多来源执行和画布验证通过；管理页面剩余失败来自测试提前匹配“保存后配置授权”文本，已改为等待具体的授权弹窗，并增加实际部门授权断言。

截图检查修正了检索按钮换行，页面使用部门、连接和查询的业务名称。最终轮次已人工检查中文浅色的部分失败弹窗、英文深色的查询列表；成功来源与失败来源、耗时、原始分数与融合分数、部门名称及授权数量均正常展示。截图保存在 `.local/artifacts/playwright/helm-agentxctl/knowledge-1017-r4/knowledge-connections/artifacts/`。正式验收范围为桌面 Web；没有增加移动端验收。

本次是工作树实现与临时 Kubernetes 功能验收，不是发行包发布。原开发服务已由 fixture 恢复并通过健康检查，临时环境已统一清理。历史失败日志保留，不用后续通过结果覆盖。

## 复核后的补齐

`r4` 的成功不包含 Agent 实际多查询调用及新查询跨部门审批。复核还发现 Agent 适配层将全部来源失败压成普通字符串错误，丢失来源详情且允许模型继续。本轮补齐：

- Agent 知识失败保存结构化错误，使用已有工具终止机制进入错误出口，保留 `details.sources`；工具结算后崩溃恢复也遵循终止结果。
- 新增真实模型按业务用途选择查询，以及实际调用 RAGFlow 与两个 LightRAG 实例的多工具验证；通过持久化操作记录核对调用对象和次数。
- 新增 Agent 部分成功、正常空结果和两种 Provider 全部失败的验收，全部失败时断言只有一次模型调用、错误出口携带完整来源。
- 新增查询／凭证分属两部门的完整会签、拒绝、取消、授权隔离与撤销验证；复用现有 Playwright 审批页面测试，验证两位审批人的脱敏视图、会签、拒绝，以及批准后的实际检索。

补齐后的验证：

| 检查 | 结果／证据 |
| --- | --- |
| Runtime 服务 | 167 项通过，2 项既有凭证测试忽略；`knowledge-connections/closure-runtime-tests.log` |
| Agent Core（含崩溃恢复） | 49 项通过；`knowledge-connections/closure-agent-core-tests.log` |
| Clippy、格式、Python lint、浏览器 TypeScript | 通过；`knowledge-connections/closure-*.log` |
| 管理、跨部门审批、多来源执行、画布回归 | `r7` 的 4 个相关场景通过；`.local/artifacts/knowledge-connections/e2e-r7.log` |
| 桌面 Playwright | `r7` 的 8 个测试全部通过，无跳过；`knowledge-connections/closure-browser-summary.json` |
| 真实模型 Agent 结果 | `r8` 的部分成功、正常空结果、RAGFlow 全部失败及 LightRAG 全部失败 4 项通过；`e2e/knowledge-1017-r8/product/knowledge-agent-*.json` |
| 真实 Provider 与 Agent 多工具 | `r8` 通过；alpha 问题调用 1 个工具，多团队问题调用 3 个工具，覆盖两个 workspace 和两个 Dataset；`e2e/knowledge-1017-r8/product/knowledge-native-scopes.json` |
| 最终补齐运行 | `r8`：5 项通过、146 项未选中，耗时 751.76 秒；`knowledge-connections/e2e-r8.log` |
| 最终清理与恢复 | 无遗留测试 Namespace，13 个开发 Deployment 已恢复预期副本并可用；`knowledge-connections/closure-restored-services.json` |
| 文件规模与旧实现清理 | 68 个修改源码文件均不超过 2000 行，最大 1906 行；旧接口、表、索引任务与动态 workspace header 无残留；`knowledge-connections/closure-source-size.json`、`closure-legacy-scan.log` |

全部失败场景中模型调用次数均为 1，错误出口的来源与持久化工具结果一致；部分成功和正常空结果均调用模型 2 次并成功完成。真实平台使用原生 API 写入、真实 CPU embedding 和配置模型索引；外部追加文档可检索，凭证撤销后返回 422。上述相对证据路径均位于 `.local/artifacts/` 下。

当前 9 个相关系统场景由 `r7` 已通过的 4 项与 `r8` 的 5 项完成覆盖，包含 8 个桌面用例；没有执行全部 151 项系统测试。汇总见 `.local/artifacts/knowledge-connections/closure-summary.json`。修改保留在工作树，未提交或发布到开发环境；开发服务恢复原配置。

剩余真实执行验证的统一入口：

```sh
PATH=/opt/homebrew/opt/helm@3/bin:$PATH \
AGENTX_E2E_CPU_EMBEDDING_IMAGE=agentx/cpu-embedding@sha256:9876b5946d3f6c582e0cb2ca93f59d917043fe51e396b73a6ba6a9466ab4f9d8 \
uv run --frozen --group test pytest tests/e2e \
  --values .local/artifacts/knowledge-connections/values.yaml \
  --evidence-run-id knowledge-1017-r8 --scale-down-development \
  -k 'real_external_scopes_and_cross_connection_workflow or agent_knowledge_partial_empty_and_failed_error_outlet' -q
```

补齐验收的 `r5` 暴露两处测试问题：新 Agent 构造器把节点专用 `bindingRole` 误带入授权请求；管理页成功提示的文本定位同时匹配无障碍播报。分别改为授权后添加绑定角色、精确匹配提示开头。该轮的查询／凭证跨部门 API 与桌面审批、多来源执行已通过；失败证据保留，后续完整重跑用于判定结果。

`r6` 的管理页、跨部门审批、多来源执行与画布均通过。Agent 测试执行了构造器提前发布的普通检索版本，改为直接组装 Agent 并只发布一次；真实 LightRAG 在原生索引阶段连接外部模型失败，保留 `knowledge-native/lightrag.log` 中的 `APIConnectionError`，不将该轮计为真实 Provider 验收通过。

`r7` 的四项管理、审批、多来源和画布场景通过，4 份 JUnit 共 8 个桌面测试，无失败或跳过。真实模型已完成部分成功检索与 alpha 单查询选择，但自动取证遭 MySQL CLI 的 JSON 转义影响；改为查询 `HEX(operation_json)` 后解码，已在该轮数据库验证读取结果。四种 Agent 结果拆成独立参数化测试，后续只重跑这 4 项及真实 Provider 场景。

## 本地重新构建与发布（2026-10-17）

已将当前工作树重新构建为 `local-knowledge-20261017-162530`，通过现有 `cargo xtask images` 导入 11 个镜像，并使用重建后的 `agentxctl` 发布到 Docker Desktop 的 `agentx-prod-*` 环境。镜像标签与摘要均校验源码指纹 `11ef7cb7b81f3c01b4f7e8a3ee6efe160b6f8346ac7c68104dd713db300aa1d3`。

初始化新 Control、Runtime 数据库，原数据库保留。47 张结构未变的配置表共保留 252 条记录，逐行哈希核对；账号、模型、凭证及知识连接继续可用。旧库中 1 条知识查询、1 条本地文档记录和 1 条检索测试记录不转换为新版配置。新版连接详情中的查询列表从空列表开始，外部 LightRAG 内容与索引保持现状。没有新增产品迁移或兼容入口。

发布后 25 个既有工作负载达到预期副本；4 个 Helm Release 均为 deployed，Doctor、TLS、登录、页面资源及两侧基线校验通过。通过现有 LightRAG 连接创建临时查询，发现默认 workspace，浏览 1 个外部文档，检索命中 1 个片段且 `partial=false`；测试查询已删除。旧 `/knowledge/resources` 返回 404。

入口：`https://agentx.localhost`。发布汇总与验证记录分别保存在 `.local/artifacts/local-knowledge-20261017-162530/release-summary.json` 和 `verification.json`；同目录保存镜像摘要、历史部署配置、保留记录及发布日志。此次执行发布冒烟检查，未重复运行已通过的系统验收。
