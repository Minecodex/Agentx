# AI 工作流、应用用户记忆与文件闭环验收

2026-10-17，在本地 Docker Desktop Kubernetes 部署上完成真实业务验收。模型使用 `glm-5.3-flash` 和 `text-embedding-3-small`；Mem0 与 LightRAG 的 embedding 维度均为 1536。业务资料、用户偏好和测试集均为合成数据。

## 保留的平台内容

- 工作流：[客户订单分析 · 四能力与用户记忆闭环](https://agentx.localhost/workflows/01a14912-0c09-7f80-8627-2cc897ba3ac4)，当前发布 v2。
- 应用：[客户订单助手 · 用户记忆与文件闭环](https://agentx.localhost/applications/01a14932-f266-7841-8196-0ec52bd637be)，slug 为 `customer-order-closure`，部署 #2。
- [参数测试入口](https://agentx.localhost/playground?applicationId=01a14932-f266-7841-8196-0ec52bd637be&mode=parameters)。API 调用使用必填非空字符串 `userid`，应用部署冻结 `apiUserIdInput=userid`。平台登录调用继续使用平台身份。
- 新建知识查询“闭环演示 · 秋桐订单政策”和技能“闭环演示 · 客户订单分析”v3；保留原生 LightRAG API 写入并索引的秋桐订单资料。

AI 通过实际画布创作、保存和重开工作流。主线为开始、文件预处理插件、Agent、报告插件、正常结束，三个执行节点的错误出口接独立错误结束。Agent 实际绑定记忆连接、知识查询、Skill、MCP echo、模型和工作区沙箱。文件插件使用正式 Artifact SDK 读取输入、生成 Markdown 与 JSON 文件。

## 已验证的业务行为

| 场景 | 结果 |
| --- | --- |
| 画布调试 | 中文 CSV 和 TXT 输入成功；实际调用技能、知识、MCP，使用 `python3` 和 `decimal` 计算，生成两个可下载报告 |
| 正式 API Key 调用 | 四类资源均在持久化工具操作记录中成功结算；订单原价 620、折扣 62、实付 558，知识返回负责人林澄及唯一发布代号 |
| 不同用户的记忆 | 同一 Key 的 Alice/Bob 写入不同代号、报告和饮品偏好；新会话分别召回自身数据；新用户原生查询结果为空 |
| 同用户换 Key | Alice 使用另一个有效 Key，在新会话召回原记忆 |
| 并发与身份 | Alice/Bob 并发召回和生成文件互不串用；会话不接受覆盖用户字段；缺失、空字符串、数字 userid 均拒绝 |
| 幂等与附件 | 相同上传和调用请求复用原 ID；同上传键不同内容返回冲突；不存在的 Artifact 引用在执行前拒绝 |
| 输出文件 | Markdown、JSON 的实际下载字节、大小、SHA-256 和金额内容校验通过 |
| 下载授权 | 匿名、无关调用、其他应用和已撤销 Key 均被拒绝；同应用另一有效 Key 可下载 |
| 桌面页面 | 实际上传 CSV、运行已发布应用、点击报告下载，并浏览三个测试集及其详情页；平台身份与 API 用户记忆隔离 |

实际模型和工具结果、文件字节、浏览器截图及执行记录保存在 `.local/artifacts/application-closure-20261017-164656`。主要记录为 `api-matrix-summary.json`、`desktop-execution.json`、`debug-execution.json`、`api-订单分析报告.md` 和 `summary.json`。

## 平台测试集

整体业务验证通过后，通过正式 Dataset 接口创建并发布以下测试集，各 6 条、共 18 条。每条包含输入、示例期望输出和标签，文件样例引用实际上传的 Artifact。

| 测试集 | 样例 |
| --- | --- |
| [闭环演示 · 01 · 知识问答与订单政策](https://agentx.localhost/datasets/01a14941-5975-7902-8c4f-35c88fea7f6c) | 发布代号、负责人、折扣、未知订单、完整订单、来源 |
| [闭环演示 · 02 · 用户偏好与记忆步骤](https://agentx.localhost/datasets/01a14941-59b4-7fc1-b4c7-f65c439e4a2b) | Alice/Bob 写入与召回、新用户、偏好更新 |
| [闭环演示 · 03 · 文件输入与报告样例](https://agentx.localhost/datasets/01a14941-5a06-7dc0-ac3e-8778b4f9e4f0) | CSV 报告、中文文件名、重复附件、缺失附件、无效引用、空 userid |

本次只准备供审阅的数据，没有启动评测，没有更改测试集的运行身份、顺序、会话或评判流程。记忆样例中的步骤关系仅为数据表达；本次未验证它们在现有评测编排中的连续执行效果，期望答案准确率也不作为验收门槛。真实业务场景另由 API 和浏览器验收完成。

## 修正与回归

1. Runtime 在所有执行启动入口依据冻结的 Start Schema 校验并保留输入 Artifact 引用，修正画布调试上传文件不能被插件读取的问题。
2. 新增 `GET /gateway/v1/invocations/{id}/artifacts/{artifact_id}`。调用方使用有效应用 Key 或已有平台权限下载明确出现在调用输出中的文件；复用执行归属、对象大小和哈希校验。同步生成公开 OpenAPI。
3. 调试台上传返回标准文件引用，移除额外注入的消息 `type` 字段，使严格限制字段的附件输入可以提交；图片消息仍由外层 `partType` 表达。

技能配置另修正工作区相对路径和 `python3` 命令，并通过统一授权入口更新工作流对 Skill v3 的版本授权。

| 验证 | 结果与证据 |
| --- | --- |
| Runtime 服务单元测试 | 170 通过，2 个既有渠道凭证探针忽略；`runtime-tests.log` |
| Clippy、格式、契约与编排验收 | 通过；`clippy.log`、`acceptance-contracts.log`（22 项） |
| 调试台前端回归 | 14 项通过，覆盖严格附件参数和图片消息；`playground-web-tests-final.log` |
| 前端 lint、完整 TypeScript/Vite 构建、Gateway 类型漂移 | 通过；`playground-web-lint.log`、`build-images.log`；生成 Gateway 类型与已保存文件一致 |
| 临时 Kubernetes 文件专项 | `application-files-1017-a2`：1 项通过，覆盖调试引用和下载授权；`.local/artifacts/e2e/application-files-1017-a2/product/application-file-download.json` |
| 本地真实业务验收 | AI 创作、调试发布、API 场景、平台造数、桌面复核五个 pytest 场景通过；同目录 `pytest-*.log/xml` |
| 浏览器取证 | `.local/artifacts/playwright/local/application-closure-20261017-164656/` 下创作和页面复核 JUnit、截图与 Trace |

系统与浏览器编排均通过 `uv run --frozen --group test pytest tests/e2e`，复用 Rust 部署和现有 Playwright。临时 Python 入口及浏览器用例已移除，验收源码保存在证据目录；确定性的文件授权回归保留为 `tests/e2e/product/test_application_files.py`，前端回归保留在 `playground-page.test.tsx`。

初轮错误和重试证据保留：技能示例使用不存在的 `python` 命令导致预算耗尽；文件专项曾误取已发布工作流的旧版本文档；会话未知字段实际返回 422 而测试原先期望 400；页面字段标签自动首字母大写；真实页面上传多余 `type` 被严格 Schema 拒绝。分别修正配置、版本选择、测试定位和产品代码后复测。测试集数据与评测流程没有因这些修正改变。

## 最终本地发布

最终重新构建并发布 `local-application-closure-20261017-ui`，11 个产品镜像按摘要固定，源码指纹为 `608aa62196c32871a5e9f41f48d36935e632f9f30cf3fb874e5a6cdc02a14fdb`。之前的发布、Runtime 回归和临时 Kubernetes 文件回归证据保留在原位置；最后一轮仅修改调试台前端，完整前端构建和实际页面复测在最终镜像上通过。

22 个既有工作负载达到预期副本，Web readiness、登录及认证后的 Gateway 调用读取通过，应用部署与测试资料保留。临时测试命名空间和镜像加载 Pod 均已清理。本次没有重建业务数据库。发布摘要、镜像摘要和最终检查见 `images.json`、`final-health.json`、`summary.json`；本次修改保留在工作树，未创建提交。
