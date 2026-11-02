# 页面持续编辑与 MCP 的常驻合并升级

2026-11-02，按用户“两个任务需要协调后一起升级”的确认，两对话完成统一构建、保留数据升级和实际联调。最终排版与 MCP 发布历史均已进入本机常驻部署，原账号和示例继续使用。

## 实际入口

- [应用界面编辑](https://agentx.localhost/applications/01a19aea-c079-70d0-903c-cc4175521370?tab=ui)：同一工作 HTML、右侧持续聊天、首次发布配置地址、后续直接发布；历史独立预览，回滚只切换线上发布。
- [员工报销页面](https://agentx.localhost/a/expense-demo)：提交申请、补充用途与发票号、等待主管审批、查看正式结果。
- [MCP 接入文档](https://agentx.localhost/applications/01a19aea-c079-70d0-903c-cc4175521370?tab=mcp)：已发布应用的说明、Schema、地址与客户端配置，提供更改发布和发布历史。
- [审批中心](https://agentx.localhost/approvals)：经理领取并作出批准或拒绝，网页和 MCP 均不替代审批人。

原 MCP 地址仍为 `https://run.agentx.localhost/gateway/v1/mcp/applications/expense-demo`，业务工具仍为 `submit_expense`。本机 `.local/dist/expense-demo/` 的客户端配置和 CA 有意保留；服务 Key 配置为受限文件，不在报告中展示凭据。

## 来源与数据保留

统一源码 SHA256：`fe05d1330dd528b10648ca8e66ab968e645dcf145330e049a411b3814819291c`。11 个镜像均核对来源标签并固定 digest；8 个正式工作负载的实际 OCI 配置和来源与构建回执一致。Dependencies、Runtime、Observability、Control 四个部署域的 Doctor 全部通过，各服务恢复原副本数。

使用正式 Rust 初始化工具创建新数据域，结构采用当前初始化基线，没有产品迁移或兼容接口。停写后备份原库，按已确认的 MCP 字段映射复制 Control 122 张既有表、5,302 行，以及 Runtime 117 张既有表、27,173 行；每行转换后 Hash 与目标一致，原库逐行复核不变。新 MCP 地址映射、原 OAuth 客户端与确切 publicationId 的绑定另行核对；原 HTML、聊天、任务、审批、API Key 和 Hydra 数据保留，持久密钥未轮换。

首次部署编排尝试以零副本暂存新版配置，被现有 Helm 最小副本检查拒绝。保留 `cutover.log` 和失败回执；没有放宽 Chart 校验。确认全部旧写入服务已退出后，以原副本数通过正式 Helm 路径依次启动绑定新数据域的新版服务，最后统一执行四域 Doctor。原数据复制不重做，原库未重新接收业务写入。

## 升级后实测

系统编排均由 `uv run --frozen --group test pytest tests/e2e` 入口执行，浏览器操作使用现有 TypeScript Playwright，目标为正式本机 HTTPS 地址。

1. 本地交付组 **4 项通过，15.46 秒**：镜像与数据保留；标准 MCP 发现、幂等提交、原任务补充输入、真实审批中心领取并批准、最终输出核对；员工网页、画布和新 MCP 文档；独立 OpenCode 1.1.56 原生连接。
2. 页面编辑组 **1 项通过，6.15 秒**：1440×900 和 1280×720 下聊天输入与发送可见；历史读取冻结物料、独立预览、实际回滚及再次发布；工作 HTML、聊天 Run 列表和访问后缀均保持不变。
3. 原网页的批准与拒绝历史均从原 taskId 打开，业务字段和结果正确，没有重放提交或补充输入。原 OAuth 登记精确绑定原发布和资源地址；其完整登录、同意、刷新与并存发布行为的临时环境验收另见 [MCP 专项记录](2026-11-02-mcp-publications.md)，不把该阶段测试冒充为本次常驻重测。
4. 页面真实模型连续修改及关窗重开已在前阶段通过，详见[持续编辑记录](2026-11-02-continuous-page-editor.md)。本次合并升级新增付费模型请求为零；累计保守预算占用仍为 USD 5.59647560，原未知用量预占保留。原 64 页、128 次执行矩阵未在本轮重跑。

本次 MCP 实际任务：`01a19bde-d04f-7153-9528-7971e26b1b58`，审批：`01a19bde-d312-7b00-a81c-1ca28c763db6`。业务数据均为虚构报销，不执行付款。

## 证据与清理

证据目录：`.local/artifacts/e2e/plan9-combined-20261102T102301Z/`。

主要回执为 `frozen-source.json`、`image-receipts.json`、`retained-data-verification.json`、`mcp-retained-binding-verification.json`、`cutover-complete.json`、四份 `doctor-*.json`、`live-workload-verification.json`、`delivery-junit.xml`、`editor-junit.xml`、`mcp-business-test.json`、`editor-layout-verification.json`、`opencode-local-mcp.json` 和界面截图。原失败与后续恢复分别保存在 `cutover.log` 和 `forward-upgrade.log`。

临时登录文件和测试配置删除；含敏感部署配置及原库备份仅放在受限归档目录。原源码预览、其所属进程和初始化临时 Job 已清理，当前保留正式应用、MCP 客户端交付文件和实际业务记录供用户体验。其他对话的资源与无关工作树修改未清理或回退。
