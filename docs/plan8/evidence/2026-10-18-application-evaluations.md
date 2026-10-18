# 应用内测试集与平台 AI 评测验收

2026-10-18，按用户确认的应用评测重构计划实现。测试集、评分方案和评测批次的唯一管理入口为应用详情的“测试与评测”。本次使用初始化 DDL 和临时数据库验证，常驻环境数据库重置与产品部署不在交付范围。

## 实现结果

| 能力 | 当前行为 |
| --- | --- |
| 测试集归属与契约 | 固定所属应用、来源已发布工作流版本、完整输入输出契约和指纹；同契约版本共用，结构或校验约束变化须创建新集 |
| 契约规范化 | 按 Schema 位置识别关键字，忽略标题、说明和无语义顺序；保留名为 title/description 的业务属性及默认对象内容 |
| 草稿与快照 | 保存逐字段问题和待补全状态；开始评测时生成或复用相同修订的不可变快照，复用前仍检查附件有效性 |
| 复制与调用记录 | 同路径、同类型并经目标约束验证后带入，保留复制来源；新增或不合规字段待补全，可批量补值。调用导入读取实际历史版本和输入输出，由用户确认参考输出 |
| 方案与批次 | 方案仅含自然语言标准和通过分，修改生成不可变版本；一次提交多个测试集，各选多个匹配版本，按“快照 × 版本”派发子运行 |
| 幂等与恢复 | 接受时校验修订并冻结模型、凭据版本、参数、价格与提示词；Outbox 持久派发，任务标识稳定，租约接管有 fencing，重复提交不创建重复批次 |
| 平台模型与授权 | 平台 AI 模型供画布与评测共用；评测只读用户可读名称与可用状态。系统裁判具有租户、应用、批次限定的独立签名授权，业务工作流不能取得裁判资源，裁判不能调用工具 |
| 模型评分与撤销 | 严格解析 score/reason，按平台阈值判定；错误分数为空。普通切换只影响新批次，停用或撤销在启动、领取及结算时检查，不改用其他模型 |
| 报告与取消 | 汇总子运行进度，区分质量未通过、执行错误、评分错误；展示完整理由、双 Trace、耗时及分币种业务/评分成本。取消传播并发布终态报告，保留已完成结果 |
| 比较 | 同快照以最小版本号为默认基线，可切换；只对有效评分统计改善/退步。跨集按稳定来源对应并标注数据变化，不按名称猜测对应关系 |
| 桌面交互 | 使用现有 SchemaForm、表格、Dialog、Tabs 和设计令牌；中英文、深浅主题、只读空态沿用统一界面 |

应用接口统一位于 `/api/v1/applications/{applicationId}`。旧全局测试集、评分方案、评测管理路由及独立规则评分器已删除。OpenAPI、生成类型、调用方、初始表结构和架构文档同步更新，不提供兼容入口或历史数据转换。

主要实现为 Control 的 `application_testing`、`application_datasets`、`application_evaluation_*`、`test_contract`、`platform_ai_model`，Runtime 的 `platform_evaluation` 与 WorkPackage 执行链路，前端的 `features/applications/testing`。修改及新增的前后端源码文件均不超过 2000 行。

## 单元与工程门禁

证据根目录为 `.local/artifacts/application-testing/`，以下各行独立列出，专项测试与完整测试存在覆盖重叠，不累加为总数。

| 验证 | 实际结果 | 证据 |
| --- | --- | --- |
| Control 测试 | 74 通过，含真实 MySQL API 闭环、投影、派发接管和保留策略 | `control-tests-final.log` |
| Runtime 单元测试 | 171 通过，2 个既有渠道凭证探针忽略 | `runtime-unit-tests-final.log` |
| Runtime Slice | 1 通过，包含独立裁判授权、领取前撤销零 Provider 调用、取消报告和恢复 | `runtime-slice-final.log` |
| Runtime 契约 | 14 通过，覆盖签名篡改、错误租户及模型资源隔离 | `contracts-final.log` |
| 契约指纹专项 | 3 通过，覆盖输入/输出字段增删、改名、类型、必填、默认值、枚举、文件约束和注释/排列 | `contract-fingerprints-final.log` |
| 附件专项 | 8 通过，含可空与 all_complete 文件 Schema | `artifact-tests-final.log` |
| 前端完整测试 | 101 文件、500 项通过 | `web-tests-final.log` |
| 报告专项 | 3 通过，覆盖基线切换、错误空分数、双 Trace 和来源比较 | `report-tests.log` |
| 架构与自动化验收 | 212 项通过 | `architecture-tests-final.log` |
| 编译、格式与 lint | Rust 检查、cargo fmt、前端 TypeScript/Vite 构建、前端 lint、Python lint 通过；前端保留现有警告，无错误 | `rust-final-check.log`、`web-build-final.log`、`web-lint-final.log`、`python-lint-final.log` |
| 生成一致性与边界 | 119 份生成契约与仓库一致，V2 边界检查通过；155 项系统测试可收集 | `boundary-check.log`、`e2e-collection.log` |
| 仓库快速门禁 | `cargo xtask check --fast` 通过，包括 CLI/密钥/Echo 测试、依赖锁、Python 检查、212 项验收、155 项系统测试收集、四份部署配置、Kustomize 与源码行数限制 | `xtask-fast-final.log` |

完整前端测试采用 `--maxWorkers=2`，避免本机并行镜像构建时的资源争用。此前默认并发下一个无关调度表单测试超时，降低并发后完整 500 项通过；没有修改断言或放宽超时。Runtime Slice 曾在并行构建时触发既有 500ms 锁等待，独立复测通过，未更改锁超时。

快速门禁初轮发现格式化后测试夹具的 S608 注释位置不正确，已将 UUID SQL 夹具的抑制注释放回对应表达式；Python 全库 lint 与 333 份文件格式检查通过。第二轮在配置校验时检测到本机默认 Helm 4，与项目要求的 Helm 3 不符；切换至已有 Helm 3 路径后完整快速门禁通过。初轮记录保留在 `xtask-fast-initial.log`、`xtask-fast-helm4.log`，没有修改门禁或安装系统工具。

## 临时 Kubernetes 与桌面验收

统一入口：

```sh
PATH="$PWD/.local/artifacts/local-deploy-20261013/bin:$PATH" \
AGENTX_E2E_RAGFLOW_DISABLE=1 PYTHONUNBUFFERED=1 \
uv run --frozen --group test pytest \
  tests/e2e/product/test_application_evaluations.py \
  tests/e2e/product/test_live_text_acceptance.py::test_real_model_judge_two_versions_and_absent_case_alignment \
  --values .local/artifacts/application-testing/values.yaml \
  --scale-down-development --evidence-run-id app-evaluations-20261018g -q
```

Python 负责临时环境、服务进程、证据和清理，浏览器业务操作由现有 TypeScript Playwright 执行。使用现有 Echo Provider、Vault 和真实模型夹具；本次定向验收不依赖 RAGFlow。

`app-evaluations-20261018f` 的四项应用系统验收通过，其中桌面项实际运行两项 Playwright 测试，均通过且无跳过：

- 两份不同契约测试集分别选择两个版本，检查复制补全、批量填写、修订冲突、快照不变、方案版本不变、幂等与冲突请求、Control 重启派发、模型切换后冻结配置不变、评分异常与取消。
- 应用范围、只读评测权限、模型信息可读而管理不可写、跨应用拒绝和旧全局接口返回 404。
- 上传真实附件并评测，过期后标为待补全，即使同修订快照已存在也拒绝再次接受；历史报告保留原引用元数据。
- 经真实桌面 UI 建集、保存与补全草稿、复制预览、批量填值、创建方案、多集多版本评测、查看评分理由和双 Trace、按来源比较、调用记录加入及参考答案确认；覆盖中英文、深浅主题及只读空态。

对应 JSON 位于 `.local/artifacts/e2e/app-evaluations-20261018f/product/`，桌面 JUnit、截图及 Trace 位于 `.local/artifacts/playwright/application-testing/app-evaluations-20261018f/`。

## 真实模型记录与最终复测

`app-evaluations-20261018e` 真实模型验收通过：使用既有配置 `glm-5.3-flash`，两份测试集各对两个版本评测，共四个子运行、十二条有效评分；评分理由、评分执行状态、成本及两个复制来源对应关系均验证通过。证据为 `.local/artifacts/e2e/app-evaluations-20261018e/product/live-model-evaluation.json`。

`app-evaluations-20261018f` 的真实模型复测遇到供应商请求超时，随后触发 Provider 熔断，错误为 `PROVIDER_UNAVAILABLE` 和 `PROVIDER_CIRCUIT_OPEN`。这轮结果为四项应用系统验收通过、一项真实模型夹具错误；失败批次保留空分数与实际错误，没有切换模型或伪造零分。完整失败报告位于同轮 `product/live-model-evaluation-failure.json`。

`app-evaluations-20261018g` 全部五项系统验收通过，用时 669.32 秒，包含两项真实 Playwright 桌面测试（10.98 秒、无跳过）和真实模型四个子运行、十二条评分、零错误。桌面场景新增验证方案编辑生成 v2，以及打开失败批次、筛选评分错误、查看完整错误和评分 Trace。中英文、深浅主题、错误详情及只读空态截图已经人工查看。所有临时测试命名空间已删除，开发服务副本和 readiness 已恢复。

随后收尾检查补充了 Control 取消终态保护：子运行已全部完成而批次汇总尚未更新时，不再把已完成结果改成取消。真实 MySQL API 回归确定性构造这一窗口，断言最终报告仍完成且没有取消 Outbox；更新后的 Control 全部 74 项测试通过。

`app-evaluations-20261018h` 使用最新 Control 镜像完成上述真实模型用例的增量复测：一项系统测试通过，用时 473.43 秒（含安装、真实调用、清理和恢复）。四个子运行、十二条评分均完成且零错误；其中四条实际输出为“北京是中国的首都”，参考输出为“中国的首都是北京”，均以 1.0 分通过，模型理由明确指出词序不同但语义一致。完成后再次取消仍保持完成状态。业务调用成本合计 USD 0.005100，评分调用成本 USD 0.007908，分开记账。

增量命令为前述系统入口仅选择 `tests/e2e/product/test_live_text_acceptance.py::test_real_model_judge_two_versions_and_absent_case_alignment`，运行 ID 改为 `app-evaluations-20261018h`。证据为 `.local/artifacts/application-testing/e2e-semantic-final.log` 和 `.local/artifacts/e2e/app-evaluations-20261018h/product/live-model-evaluation.json`。最终五项业务系统测试按 g 轮完整验收、h 轮增量复测记录，不将重复执行累加为新测试数量。

g、h 两轮结束后均已删除临时测试命名空间，开发 Control、Web、Runtime、可观测及依赖服务均回到原副本并就绪，镜像加载 Pod 已清理；最终健康清单见 `.local/artifacts/application-testing/final-health.json`。h 轮只新增取消保护和语义样本的增量验证，完整桌面流程仍引用 g 轮证据。

## 交付边界

测试集评测限于工作流单次输入输出；附件仅验证引用与元数据，不解析正文评分。本次没有执行常驻数据库重置或部署新产品版本。临时测试沿用开发服务副本暂停与恢复流程，结束后检查测试命名空间清理及开发服务恢复。定向系统验收不替代全量生产容量、安全或发布认证。

## 调用记录加入测试集的交互修正

后续桌面反馈修正了字段完整却显示“待补全”、串联建集和用例编辑弹窗、重复输入名称及历史英文校验提示的问题。调用样本读取时即按实际版本契约和附件授权校验，编辑使用 `POST /applications/{application}/test-case-validation` 校验当前草稿。字段问题有稳定错误代码，参考答案未确认与数据缺项分开；“确认加入”或完整用例的保存即完成参考答案确认，不再单独勾选。

完整样本在一个确认窗口选择目标测试集；缺项直接进入右侧补全。新建目标测试集时在同一窗口填写“测试集名称”，用例名称自动生成，展开编辑和名称冲突重试均保留内容。`POST /applications/{application}/datasets` 支持 `initialCase`，测试集与首条用例同事务保存，失败回滚；取消不创建空测试集。

此轮验证结果：

- Control 75 项测试、前端 506 项测试、212 项仓库验收通过；前端构建、OpenAPI 及生成类型一致性通过，lint 零错误。
- `pytest tests/e2e/product/test_application_evaluations.py` 在 `invocation-authoring-20261018a` 临时环境中 5 项通过，用时 484.69 秒；其中实际运行 2 项 Playwright 桌面测试，无跳过。覆盖进入即校验、完整样本确认、取消无空集、名称保留和冲突重试、新建集与首例一起保存、参考答案确认、权限、附件失效及原评测流程。
- 桌面截图已查看，完整样本的确认窗口、展开编辑的单一侧栏与保存结果均符合预期。
- 临时环境准备时补齐缺失的 Echo fixture 镜像；fixture 沿用未修改的已有构建，导入记录保存在本轮构建证据中。常驻服务无需暂停，结束后临时命名空间和镜像加载 Pod 均已删除，原 22 项工作负载副本数和 readiness 正常。

证据目录为 `.local/artifacts/invocation-dataset-authoring-20261018/`、`.local/artifacts/e2e/invocation-authoring-20261018a/` 和 `.local/artifacts/playwright/application-testing/invocation-authoring-20261018a/`。系统验收使用本轮新建的 Control/Web 镜像及未修改的 Runtime 镜像；完整构建日志另行保留。

在先前已授权的本地构建基础上，本轮另行更新常驻环境的 Control/Web 两个 Deployment，保留当前数据库、配置和其他服务。通过受本地 CA 校验的 HTTPS 读取用户实际调用并请求草稿校验，确认输入输出没有字段问题，未确认参考答案不会生成字段缺项；同时校验了新界面静态资源摘要。发布和最终健康记录为同轮 `upgrade-control.json`、`local-verification.json`、`final-verification.json`。这次更新不涉及数据库结构变更或数据重置。


## 测试集与评测列表交互调整

测试集元数据编辑和“从此测试集创建”移动到外层列表。用例列表支持逐条勾选、半选和全选，导入与导出相邻，新增用例位于最右侧；未勾选时禁用导出。沿用 dataset:view 导出、dataset:manage 导入，不增加独立权限。导出改为携带 expectedRevision 与 caseIds 的 POST 请求，在固定修订下只输出选中用例；空选择、跨集 ID 和修订过时均拒绝。

删除批量补充值的前端组件、接口和生成契约。复制预览随目标工作流版本变化立即校验；契约变化时，全部复制用例将 reference_confirmed 置为 false，显示草稿并逐条检查确认，即使原有字段值仍满足新约束也不自动放行。同契约复制保留原准备状态，原测试集不变，复制来源关系保留。

新建评测要求填写名称，逐行先选择完整可运行的测试集，再勾选其适配版本；支持多测试集和每集多版本，不默认勾选版本，切换测试集会清除旧版本选择。相同提交体重试复用幂等键，修改配置使用新键。列表按名称、方案、测试集、工作流版本、完成任务、平均分、状态、时间排列，测试集与版本逐行对应；平均分按已评分用例数加权，状态和查看报告操作保持单行。

验证结果：

- Control 76 项、前端完整 510 项、仓库验收 212 项通过；OpenAPI 生成结果一致，前端构建、lint、浏览器 TypeScript 检查通过。最终排版调整后，测试与评测模块 13 项及前端构建再次通过。
- `pytest tests/e2e/product/test_application_evaluations.py` 在 `dataset-interactions-20261018a` 与最终 `dataset-interactions-20261018b` 各完成 6 项测试。最终轮用时 407.83 秒，实际运行 2 项 Playwright 测试，零失败、零跳过；两轮不累加为新的测试数量。
- 桌面验证覆盖外层编辑/复制、草稿补全确认、单条和全选导出 JSONL、名称填写、两个测试集各选择两个适配版本、完整报告及异常定位。最终列表断言 4/4 任务完成、100 分和已完成状态；中英文、深浅主题及只读空态沿用真实 UI 验证。
- 已查看最终建评测、评测列表、测试集外层操作和复制草稿预览截图；首轮截图发现的状态与操作按钮换行已修正。

构建与本地检查证据位于 `.local/artifacts/dataset-evaluation-interactions-20261018/`；系统证据位于 `.local/artifacts/e2e/dataset-interactions-20261018b/`；桌面 JUnit、截图、下载内容和 Trace 位于 `.local/artifacts/playwright/application-testing/dataset-interactions-20261018b/`。本轮只重建变更的 Control/Web 镜像；最终排版仅增量重建 Web，镜像摘要与各自源码指纹分别记录在 build.json 和 build-layout.json 中。

本地 Control/Web 更新完成，agentx-control Release 为 deployed/ready。通过受本地 CA 校验的 HTTPS 确认新页面资源与部署容器文件摘要一致，选中导出接口生效，原应用的测试集、方案和评测批次仍可读取。发布前后配置比较仅两项镜像摘要与构建标签变化，数据库和其他配置保持原值；未重置数据。原 25 项工作负载副本与期望状态一致，临时测试命名空间和镜像加载 Pod 已清理。发布与最终验证记录为本轮 upgrade-control.json 和 final-verification.json。
