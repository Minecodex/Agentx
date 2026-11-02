# 本地升级与费用报销示例交付

2026-11-02，按用户要求升级本机常驻 Agentx，保留一个可继续使用的工作流、AI 生成网页和应用 MCP。旧账号、应用、工作流及历史记录保留。以下是本轮本地交付结果；原 64 份页面、128 次执行仍见[原验收报告](2026-11-02-acceptance.md)，本轮没有重新执行完整矩阵。

## 直接使用

| 入口 | 地址 |
| --- | --- |
| 员工业务页面 | [费用报销服务台](https://agentx.localhost/a/expense-demo) |
| 工作流画布 | [费用报销 · 网页与 MCP 示例](https://agentx.localhost/workflows/01a19aea-bff1-75f0-a8dd-0ca2dc35346f/versions/01a19aea-bff6-7483-8ce1-7be1381c1ec1) |
| 页面管理 | [应用的访问界面](https://agentx.localhost/applications/01a19aea-c079-70d0-903c-cc4175521370?tab=ui) |
| MCP 管理 | [应用的 MCP 接入](https://agentx.localhost/applications/01a19aea-c079-70d0-903c-cc4175521370?tab=mcp) |
| 人工审批 | [审批中心](https://agentx.localhost/approvals) |

使用现有 `admin` 账号登录，密码保持原值。员工页面与审批中心可以开在两个标签页中，用同一账号体验提交和审批。此示例仅记录虚构报销数据，不发起付款。

1. 在业务页面填写报销单号、申请人、费用类型和金额，例如 `BX-TRY-001`、`体验用户`、`差旅费`、`168.50`。
2. 提交后，在当前任务中补充用途和发票号码，例如 `测试出差交通费用`、`FP-TRY-001`。
3. 另开审批中心，找到该申请，领取后选择批准或拒绝并确认。审批内容会显示原始申请和补充资料。
4. 返回业务页面查看结果。本人历史支持重新打开任务；刷新或重开不重复提交。

工作流为：申请受理 → 结构化补充资料 → 经理审批 → 批准或拒绝结果。输入等待与审批各保留 24 小时，工作流总期限 48 小时。

## MCP 接入

地址：`https://run.agentx.localhost/gateway/v1/mcp/applications/expense-demo`。

业务工具为 `submit_expense`；任务辅助工具为 `get_task`、`submit_task_input`、`cancel_task`。协议版本固定 `2025-11-25`，服务 SDK 为 `rmcp 3.8.0`。外部 AI 提交后继续查询原 `taskId`，根据真实待办补充资料，遇到审批由人在 Agentx 审批中心处理。

已准备本机 OpenCode 配置，均位于 `.local/dist/expense-demo/`：

- `opencode-service.json`：已配置独立应用服务 Key，可以直接连接。目录权限 700，配置权限 600；凭据不写入文档和验收报告。
- `opencode-oauth.json`：已预登记的个人 OAuth 客户端，包含应用 scope。客户端回调为 `http://127.0.0.1:19876/mcp/oauth/callback`。
- `local-ca.pem`：本地服务的公开 CA 证书，通过进程环境传入，不修改系统信任设置。

服务 Key 方式启动示例：

```sh
NODE_EXTRA_CA_CERTS=/Users/kakj/rust/Agentx/.local/dist/expense-demo/local-ca.pem \
OPENCODE_CONFIG=/Users/kakj/rust/Agentx/.local/dist/expense-demo/opencode-service.json \
opencode
```

个人方式将配置路径换为 `opencode-oauth.json`，再使用 `opencode mcp auth expense` 登录授权。个人 OAuth 本轮完成部署、客户端登记、元数据及诊断检查；本轮未重新完成真实个人同意、刷新和撤销闭环，不把此前临时环境中的 OAuth 结果计为本机新增实测。

本轮 OpenCode 1.1.56 原生 `mcp list` 已连接成功；完整业务链由独立标准 MCP 客户端实际执行，并在真实审批页面人工确认。本轮没有新增 OpenCode 自然语言业务测试或相关模型付费请求。

## 部署与数据

- 当前工作树构建全部 11 个产品镜像，以不可变 digest 部署。镜像标签为 `plan9-local-demo-20261102t060426z-ready`，产品源码 SHA256 为 `b8dd336e3b5aeed08302ca9f971a1d06349d1a8244d3300be50cad169eea9fe8`。
- Control、Runtime、Observability、Dependencies 四个部署域健康检查最终全部通过。8 个业务 Deployment 就绪；Hydra 及独立数据库、凭据纳入 Dependencies。
- 原 Control/Runtime 数据库保持未修改；当前基线初始化新数据库后，执行一次本地数据转存并逐表核对。112 张 Control 表、111 张 Runtime 表的保留行 Hash 均一致，不复制旧 SQLx 迁移记录。没有新增产品兼容层或迁移脚本。
- 原库为 `agentx_control_interactions_20261018_215640`、`agentx_runtime_interactions_20261018_215640`；活动库为 `agentx_control_p9_20261102_060426`、`agentx_runtime_p9_20261102_060426`。升级前备份受限保存，不清理用户数据。
- 页面 publication 为 `01a19aef-b17f-7fb3-9ab2-326aacd51ef1`（revision 1），绑定 Bundle `01a19aea-c198-7520-876b-79b469b5064f`。网页 HTML、SDK 和正式发布绑定均已核对。

## 真实执行与修复

网页通过正式界面生成，使用现有平台设置的 APIMart `glm-5.3-flash`，一个创作 Run、6 次成功模型调用，无人工修改 HTML。最终 HTML 保留为 `expense-demo-ai.html`。

| 入口 | 任务 | 实际结果 |
| --- | --- | --- |
| 网页批准 | `01a19b1e-075d-70a1-a42c-3c6077e5fabe` | 张小明，1,280.50 元，补资料后批准；等待任务跨 Runtime 升级后仍为原任务 |
| 网页拒绝 | `01a19b3a-0995-7050-9067-cef9befa6a5a` | 李晓雨，680.00 元，补资料后拒绝；页面显示真实拒绝结果 |
| MCP 批准 | `01a19b33-4323-7ea2-b3cd-2b567fb32bff` | 358.75 元，真实补充输入、审批中心批准、原任务完成；相同 requestId 重复提交复用原任务 |

两笔网页申请均由真实桌面 Chrome 提交、补充资料、领取及审批。独立 Playwright 又重新打开两笔历史任务，核对姓名、金额和中文决定，确认无新增业务提交。

联调发现并修复两个产品问题：

1. 输入等待恢复后，下一个审批节点可能先于补充输入的持久化读取表达式。调整同一事务内的顺序，先保存恢复输出，再调度下游；新增真实数据库回归，保留原失败证据。
2. 首次打开生成页时，真实 Chrome 中已设置 Blob URL 的 iframe 仍停留在 `about:blank`。宿主只挂载已有内容的绑定实例，双层 iframe 改用 `srcdoc`；保留 opaque sandbox、MessagePort 授权和禁止网络/审批能力等原边界。设计更新见[网页设计](../03-application-web-ui.md)。

其他失败也已保留：首轮 OCI index 比较误将构建证明差异当成镜像不同；观察断言误将未完成 outputs 的 `null` 预期为 `{}`；Dependencies 首次健康探测在滚动时收到 503，服务就绪后重新探测通过。未完成的测试申请通过原任务取消，未冒充成功。

## 验证、费用与证据

- 完整 `cargo xtask check` 通过；前端 589 项通过，Runtime 集成 2 项通过。默认 Helm 4 导致的首次环境失败保留，显式使用仓库要求的 Helm 3 后完整通过。
- `pytest tests/e2e` 编排的页面隔离回归 56 个浏览器用例通过，包含初次挂载及响应头组合。
- 保留本地部署验收 4 项通过（266.030 秒）：实际镜像/数据、MCP 业务、独立浏览器、原生 OpenCode 连接。历史任务结果专项 1 项通过（7.406 秒）。
- 新增创作输入 64,902 Token、输出 8,948 Token，平台现有配置计价为 295,400 micros（0.2954 美元）；这不是 Provider 账单。将其保守追加到原全历史已结算及未知预占 4.45005960 美元，总预算占用上界为 4.74545960 美元，低于 30 美元。本轮新增未知用量为 0，原 41 个未知请求的预占未清除。
- 临时管理员登录文件已清理；本机 MCP 交付配置有意保留供使用。升级前 SQL 备份及发布配置移入本轮证据的 `private-release/`，目录 700、文件 600。

本轮证据根目录：`.local/artifacts/plan9-local-demo-20261102T060426Z/`。主要入口为 `local-delivery-report.json`、`release.json`、`ready-upgrade-complete.json`、`retained-data-verification.json`、`web-business-verification.json`、`mcp-business-test.json`、`local-delivery-final-junit.xml`、`local-results-browser-junit.xml` 和 `cleanup.json`。`first-delivery-attempt/`、`second-delivery-attempt/` 及各次失败日志保持原样。

截图：`expense-demo.jpg`、`expense-approved.jpg`、`expense-rejected.jpg`、`expense-workflow.jpg`、`expense-mcp.jpg`，均来自实际本地部署。重跑保留部署验收需重新显式提供受控临时登录文件；交付后不保留管理员密码供自动复用。
