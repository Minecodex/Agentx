# MCP 文档入口与并存发布验收

状态：代码和临时环境验收完成；按用户确认，与网页编辑改造统一构建、保留数据升级和常驻联调已完成。原入口、服务 Key 和个人 OAuth 登记保留，详见[合并升级记录](2026-11-02-combined-upgrade.md)。

## 交互与发布规则

- 尚未发布时仅显示发布入口。首次弹框填写工具名称和访问后缀，描述沿用应用说明；多个正式环境时明确选择业务版本。
- 发布后主区域为接入文档：工具说明、服务地址、输入输出、客户端配置。右上角提供“更改发布”和“发布历史”。OAuth 客户端登记放在文档的展开项中。
- 更改弹框编辑工具名称、后缀和描述，绑定原环境当前正式业务版本，创建新的不可变发布。
- 历史列表展示每条发布的工具、后缀、说明、时间和状态，可以打开对应文档或删除。删除只撤回选中的发布，不取消原任务。
- 旧、新后缀同时可用；同后缀的不同工具同时被发现。同名工具展示最新保留版本的 Schema，未删除的旧版本仍可按 `publicationRevision` 调用其原 Bundle。
- 最后一条发布删除后，入口及资源发现返回不可用，后缀释放。再次发布继续递增版本号，不复用旧序号。

## 实现与数据边界

业务调用继续使用原 Invocation、输入等待、审批和文件授权；不增加任务状态机。发布与精确删除继续经过 Publisher Admission Outbox，Runtime 删除保存墓碑，延迟重投原发布不能重新启用它。

`application_mcp_heads` 改为单调发布序号；两域新增 `application_mcp_routes`，将一个后缀全局归属到唯一租户/应用，同应用多条发布可共享。Control 发布保存后缀、删除事实和删除 Outbox；Runtime 发布保存后缀及 active/deleted 状态。删除确认后只释放该条 Bundle 引用，最后一条删除确认后释放后缀。

个人 OAuth 客户端记录登记时的 publicationId，以保留原资源地址。新后缀需要对应登记；旧客户端不因发布新地址自动获得新 Audience。同一旧后缀仍有其他保留发布时，原授权继续可用。

结构直接修改初始化基线，没有增加产品迁移或兼容入口。统一升级所需旧数据映射已写入 `.local/artifacts/mcp-upgrade-coordination.json`。已只读核对常驻 Control/Runtime 各有一条原 MCP 发布、无历史后缀跨应用冲突；原 `expense-demo` 入口、API Key、OAuth 登记、网页草稿/会话及业务历史由统一升级保留。

## 实际验收

系统入口：`uv run --frozen --group test pytest tests/e2e/product/test_application_mcp_publications.py`，使用本机 Kubernetes 的三个临时 Namespace，并由现有 TypeScript Playwright 操作正式界面。

最终系统函数 **1 项通过，340.018 秒，无跳过**，包含 **12 个真实浏览器阶段、6 次正式业务执行**：

1. 未发布界面、两字段发布弹框、正式发布后的文档与历史。
2. 真实客户端登记、登录、同意授权及 Token 刷新。
3. 原业务版本仅要求 message；新版增加必填 category。通过更改发布绑定新版，保留旧版；同名工具发现新版 Schema 后，旧版仍能按原修订提交仅含 message 的输入并正确完成。
4. 四条发布、两个后缀和多个工具同时可用。核对每笔 Invocation 的 publicationId、业务输出及所属主体。
5. 旧 OAuth 客户端在新增后缀后继续使用原地址；跨地址请求 401。原生 OpenCode 连接含多个业务工具的地址成功，无模型请求。
6. 从历史弹框逐条删除：被删修订拒绝新调用，同地址其他工具继续可用，原任务仍可查询。删除同名新版后，另一保留工具不受影响。
7. 最后删除后的空态、服务/资源发现不可用、其他应用重新占用已释放后缀，以及原应用重新发布后的序号连续。

Runtime 真实数据库与 HTTP 集成同时验证删除幂等、延迟发布重投不复活、精确 Bundle 引用释放和旧任务保留。完整 `cargo xtask check` 通过，含 **594 项前端测试**及适用的后端、契约、数据库、部署、边界和插件检查。

实际查看历史截图后，仅补充表头与状态不换行的两个样式，避免长后缀挤压中文标题。整体源码逐字节比较证明，E2E 后的产品差异仅这两处样式；行为证据保留原候选来源，主对话统一重建当前源码。

## 失败、来源与清理

首轮系统测试尚未进入业务阶段，绑定旧 Pod 的服务转发在该 Pod 排空时收到正常 503。原失败保留；测试准备改为等待捕获的旧 Pod 完全退出，再等待正式 Bootstrap 查询和 OAuth 元数据就绪，没有重试业务提交或改变产品排空规则。

首次完整检查的格式与废弃文案检查失败均保留；修正后完整检查通过。所有真实业务通过来自最终重测，未覆盖或改写原 JUnit。

- 候选产品来源：`6d20d203179648f7027b9b2c6564a6f2fb08ff891c586d81d57bffe1739a67c9`；11 个镜像标签 `mcp-history-20261102t095007z-r2` 均核对来源并固定 digest。
- 系统原始证据：`.local/artifacts/e2e/mcp-history-20261102t095007z-r2/mcp-publications/report.json`。
- 本轮汇总：`.local/artifacts/mcp-history-20261102T095007Z/`，含 `system-business-report.json`、`system-test-junit.xml`、`full-check.log`、`candidate-images.json`、`post-e2e-style-only.json`、截图及原失败日志。
- 首轮失败：`.local/artifacts/e2e/mcp-history-20261102t095007z/` 与 `system-test-initial-junit.xml`。
- 临时 Namespace 和独立 OpenCode 缓存已清理。本轮没有付费模型调用，也未创建常驻管理员登录文件。只读部署配置快照受限归档；后续常驻合并升级由主对话完成，原阶段证据和来源不改写。
