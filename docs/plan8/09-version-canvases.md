# 工作流版本与画布管理

状态：已完成。2026-10-16 最终候选通过相关 Rust、契约、前端和临时 Kubernetes 系统验收，测试环境已清理。本文取代 plan8 原多独立草稿产品语义；既有历史验收事实保留。

## 对象与状态

Workflow 承载权限、成员、运行身份和全局递增版本号。新建时原子创建未发布 v1；新建画布创建下一个空白未发布版本。导入创建未发布 v1，其初始修订为导入内容。

Version 增加 publicationStatus、archived、workingDraftId、publishedAt、updatedAt 和 updatedBy。未发布版本随所属工作文档保存更新；已发布版本的 Definition、Editor Document、来源修订、Manifest 与资源快照永久冻结。baseVersionId 表示版本来源，不能修改。工作文档由版本唯一拥有，修订始终递增；不提供独立草稿创建、命名或归档。

已发布版本的工作文档与冻结内容的 Definition/Editor 两种 hash 不同时才显示有草稿。打开编辑器不产生待发布修改。未发布版本不展示草稿状态。运行、子工作流、应用和评测只允许消费已发布版本；Studio 调试继续消费精确工作修订。

## 发布与归档

发布到环境由服务端统一编排：未发布版本原号定稿；已发布版本无草稿时直接部署；有草稿时默认派生下一已发布版本，base 指向原版本，也可选择部署原内容。派生后原版本工作文档追加恢复基线的修订，新版本拥有独立工作文档。

版本状态、工作文档、部署记录及幂等回执同事务提交。请求明确保存修订，过期拒绝；重复请求不重复创建版本或部署。环境切换不改变版本号。归档只锁定编辑与还原，不删除草稿、不停止运行，允许发布已保存内容。

## 画布与还原

详情路由只读：显示版本内容，不打开工作文档、不提供 AI、运行、保存或还原。编辑路由打开该版本的唯一工作文档。画布移除发布和保存为版本入口。

还原需要确认并立即保存新修订：已发布 v3 的草稿恢复 v3，不能误用 v3 的来源 base；未发布版本恢复初始 revision 0（新建为空白，导入为导入内容）。还原保持精确 Definition、Editor Document 和资源引用；清理当前 Undo/Redo、恢复缓存与调试覆盖，不修改执行历史或部署。

还原、发布和归档均使旧编辑效果失效；取消活动 Builder Run，旧修订保存拒绝。还原协调在途保存；失败保留本地内容。只读、归档或正在调试时禁止还原。

## 界面与验收

详情页展示环境状态和版本列表。行操作为详情、编辑、发布、更多（执行记录、归档/取消归档），前三项使用一致的文字按钮样式。页头从左到右为成员管理、新建画布；版本列表内部不再放置新建入口。成员通过弹框管理。复制、差异、放弃草稿及版本删除不在范围内。

直接修改初始化 DDL，不提供历史迁移、旧路由或兼容分支。复用已有组件与 Python pytest/TypeScript Playwright 基础设施，在临时 Kubernetes 命名空间验收；证据存入 .local/artifacts，结束后清理并恢复开发服务副本。

## 实现落点

| 范围 | 实现 |
|---|---|
| 生命周期 | `workflow_versions.rs` 负责版本摘要、编辑上下文、新建、归档和还原；`workflow_publishing.rs` 负责定稿、派生、环境切换与回滚。沿用现有幂等表，Workflow 行锁协调保存和版本编号。 |
| 数据边界 | 初始化 DDL 增加发布状态、独立归档标记、工作文档唯一关联及 Builder generation。生产、应用、子工作流和评测在 Control 检查已发布状态；Runtime 继续读取不可变执行快照。 |
| 画布 | 独立版本详情和编辑路由；只读 Store、Inspector、代码及变量编辑器统一拒绝修改，隐藏连线、端口、便签及布局修改入口。只读分组可临时展开，不写入文档。还原等待在途保存、写入新修订、重新加载文档并清理本地历史和调试覆盖。 |
| Builder | 绑定工作文档 generation；还原、发布或归档后取消活动 Run，旧快照和迟到 Effect ACK 拒绝。 |
| 管理 | 环境状态、版本列表、发布弹框与成员弹框；运行记录按版本及其工作文档筛选，Control 与 Runtime 查询契约同步更新。 |
| 清理 | 删除独立草稿管理、旧草稿画布路由、画布发布与保存为版本、工作流归档入口；更新 OpenAPI、前端生成类型和全部测试调用方。 |

接口以 `/workflows/{workflowId}` 为前缀：`POST /versions` 新建画布；`GET /versions/{versionId}` 读取版本；`POST /versions/{versionId}/edit` 取得编辑文档及还原目标；`POST /versions/{versionId}/restore` 按预期修订还原；`PATCH /versions/{versionId}` 切换归档；`POST /deployments` 接受版本、内容来源及预期保存修订。底层工作文档保存和精确修订调试接口继续复用。

## 验证记录

本地证据统一位于 `.local/artifacts`。2026-10-16 的相关检查结果：

| 检查 | 结果及证据 |
|---|---|
| Control Rust | 73 项服务测试和 7 项 fixture 测试通过，`version-control-tests.log`；增加资源引用、导入初始内容还原等断言后的数据库 API 闭环再次通过，`version-api-tests-final.log`。 |
| Runtime Rust | 172 项通过、2 项原有忽略；数据库恢复与版本筛选集成测试通过。见 `version-runtime-tests.log`、`version-runtime-slice-tests.log`。 |
| 契约 | 46 项通过，`version-contract-tests.log`；生成的 OpenAPI 与提交内容一致。 |
| 前端 | 98 个文件、482 项测试通过，`version-web-tests-final.log`；Web 与浏览器 TypeScript 检查、最终生产镜像构建通过。 |
| 静态检查 | Rust 格式、Python Ruff/格式及 `git diff --check` 通过；前端 lint 无错误，仍有 Fast Refresh 导出规则警告。所有改动的前后端源码文件均不超过 2000 行。 |
| 系统回归 | 最终候选 `e6b8909348`：5 项 pytest 全部通过，耗时 721.77 秒，包含版本生命周期 3 个、页面加载 4 个、Builder 15 个浏览器场景（共 22 个，无失败或跳过），以及版本权限和租户边界验证。见 `version-system-e2e-run8.log`、`playwright/helm-agentxctl/e6b8909348/*/junit.xml`。 |

系统测试覆盖首次发布不增号、草稿派生及 base、原版本内容冻结、跨环境发布原版本、发布失败回滚及幂等、还原当前版本/初始内容、在途保存、多标签页和迟到 AI 命令、归档保留草稿、成员角色操作、中英文和浅深主题。数据库 API 闭环额外验证资源引用恢复、导入还原、未完成内容可保存但不可发布、旧接口移除及生产消费限制。

测试准备期间修正了本地 Sandbox 端口转发缺少健康检查地址、旧 Runtime Gateway 镜像遗漏新查询契约，以及测试中的保存状态匹配和成员创建状态码断言。最终只读控件的首次复测还发现 Playwright 不支持对 fieldset 使用 `toBeDisabled()`：已改为检查 fieldset 的 `disabled` 属性，并保留内部按钮禁用断言，产品镜像未因此改变。Docker 构建使用临时凭据配置解决本机 credential helper 阻塞，未修改用户登录配置；后端基础镜像改用 HTTPS 软件源并复用固定工具链镜像的 CA。中间失败与复测日志保留，不能将中断轮次视为通过。

最终候选实际运行的 Pod 镜像摘要保存在 `version-canvases/final-candidate-images.json`：Control `9e3baa230a9a…`、Runtime `2331de5e24c3…`、Gateway `967d8cb752aa…`、Web `45c8ad8a58c5…`。这些是临时验收环境的镜像；系统验收阶段未将破坏性初始化 DDL 应用到既有开发数据库。后续本地发布见下文。

清理结果见 `version-canvases/cleanup-final.json`：本次各轮临时命名空间、镜像导入 Pod 和测试进程均已退出。已有开发环境 19 个 Deployment/StatefulSet 均为 1/1 就绪，记录在 `version-canvases/development-workloads-final.json`；复用的原有 OpenSandbox 服务保持运行。最终界面截图、浅深主题和只读分组证据位于 `playwright/helm-agentxctl/e6b8909348`，各浏览器场景均保留 Trace。

系统验收入口（本机使用 Helm 3）：

```sh
PATH=/opt/homebrew/opt/helm@3/bin:$PATH \
AGENTX_E2E_PLAYWRIGHT_GREP='Fixture:|P8-E2E-(03|04|08|09|11|12|13|14|15|18|19|20|24)' \
uv run --frozen --group test pytest \
  tests/e2e/product/test_workflow_version_canvases.py \
  tests/e2e/product/test_web_asset_loading.py \
  tests/e2e/product/test_studio_builder.py::test_plan8_layout_protocol \
  tests/e2e/product/test_studio_builder.py::test_plan8_studio_builder_tenant_boundaries \
  --values .local/artifacts/version-canvases/values.yaml -vv
```

## 本地发布

2026-10-16 20:07（Asia/Shanghai）按用户要求完成本地重建与发布，版本 `local-version-canvases-20261016`。11 个正式镜像全部重新构建并以不可变摘要导入、发布，源码摘要与上述系统验收候选一致。四个 Helm Release 均为 deployed，Doctor 成功，19 个既有工作负载恢复为原副本数并全部就绪。

Control 使用正式初始化脚本创建独立数据库 `agentx_control_versions_20261016`，13 份基线校验值与源码一致。97 张未改变结构的配置表按适用范围保留并校验；账号、登录凭据、租户设置及模型、知识库、MCP、记忆、技能等资源配置保留。旧工作流、版本、草稿和 Builder 历史不转换到新模型，新环境工作流列表为空；原数据库 `agentx_control_plan8_20261016` 完整保留。原工作流身份的授权不复制给新工作流。Runtime 数据库和外部依赖继续沿用。

`https://agentx.localhost/workflows` 返回 200；HTTPS 使用本地 CA 验证，入口资源与实际新 Web 容器一致，bootstrapRequired=false，Runtime 匿名访问仍被拒绝。临时初始化 Job 和构建、导入进程已清理。此次发布后的验证为服务、HTTPS、资源与持久配置检查；浏览器业务验收使用此前同源码候选的系统 E2E 证据。

证据：`.local/artifacts/local-version-canvases-20261016/` 中的 `images.json`、`production-values.yaml`、`upgrade.json`、`database-preparation.json`、`verification.json` 和 `cleanup.json`。本地构建入口默认额外尝试 Mem0 Fixture；11 个正式镜像完成后停止该无关构建，使用 `cargo xtask import-images` 成功导入正式镜像，原 Mem0 Addon 保持运行。

## 发布环境与页头操作修正

2026-10-16 20:33（Asia/Shanghai）完成后续修正并发布到本地站点：新建画布移至页头成员管理右侧；版本行按钮改为“发布”，与详情、编辑统一使用 ghost 样式。发布弹框区分环境加载中、请求失败和无可用环境，提供重新加载；空状态可在新标签页打开环境管理，返回后无需关闭弹框即可重新加载并选择环境。禁止在环境不存在或加载失败时提交。

本地缺少环境选项的根因是此前数据库重建保留配置时遗漏 `workflow_environments`，当前数据库与上一轮数据库均为空，并非下拉组件未实现。已从原始 `agentx_control` 恢复同租户原有 Development、Production 两条内置环境，保留原 ID；同时修正本地重建辅助脚本的配置表筛选。工作流及版本内容未修改。产品公司初始化仍正常创建两条内置环境，无需变更初始化 DDL。

最终候选 `e0f1acc817` 通过 `pytest tests/e2e/product/test_workflow_version_canvases.py`：2 项 pytest 通过，耗时 266.06 秒，包含 4 项桌面浏览器场景及服务端权限验证。新增场景实际验证页头顺序、行按钮样式、选择 Production 并发布、禁用全部环境后的空状态、通过管理入口恢复环境并重新加载，以及请求失败后的重试。首次复测因后台标签页未触发焦点事件暴露恢复入口不足，增加显式重新加载后通过；保留该轮失败证据。相关前端单元测试 6 项通过，Web 与浏览器 TypeScript、生产镜像构建通过，lint 无错误。

本次仅更新 Web 镜像，摘要为 `sha256:5f26003fcb7409d7ed273f7b12d4d141a73dbab19554d3fbcc0679934439220f`，Control Helm Release revision 8。19 个既有工作负载全部就绪，用户工作流地址 HTTPS 返回 200 且资源与新容器一致，原工作流保留，两条环境均启用。临时测试命名空间与进程清理完成，本轮未缩容开发服务。

证据位于 `.local/artifacts/workflow-publish-ui-20261016/`（`environment-restoration.json`、`unit-final.log`、`e2e-final.log`、`upgrade.json`、`verification.json`、`cleanup.json`）；浏览器截图及 Trace 位于 `.local/artifacts/playwright/helm-agentxctl/e0f1acc817/workflow-version-canvases/`。
