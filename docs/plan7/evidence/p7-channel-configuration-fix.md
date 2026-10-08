# P7-A 渠道配置表单与回复流程修复

日期：2026-10-08。范围是应用渠道表单、字段校验，以及从界面保存、发布到平台回复和投递记录查询的流程。

## 实现

- “自动回复”改为“开启回复”；开启后才展示必填的“回复输出字段”下拉框和可选的“回复模板”多行文本框。关闭再开启保留未保存的表单内容，模板留空时直接发送所选输出。
- 新建和编辑表单均先填写平台配置、输入映射和固定输入，再填写开启回复、回复输出字段和回复模板。新建默认启用，启用与停用统一由外部列表按钮操作，编辑配置保留当前状态。
- 输入下拉框包含应用绑定工作流的全部输入。新建时仅生成必填输入行，来源留空，由用户选择；可选输入按需添加。固定输入也从相同字段列表选择，数字和布尔值按声明类型转换。
- 首次应用部署前使用绑定工作流的最新不可变版本，已有活跃部署时使用该部署冻结的输入/输出 Schema。没有已发布工作流版本时提示先创建版本并禁止配置渠道，不读取可变 Draft。
- 前端和 Control 检查目标存在性、重复映射、固定输入冲突及必填输入遗漏；开启回复时必须选择已有输出字段。Control 对 IM 渠道在首次应用部署前也执行完整字段校验。
- 编辑原平台、原接入模式时，已存储的敏感字段允许留空保留；新建或切换平台/模式仍要求相应凭证。Control 按 Vault KV v2 的 `data.data` 读取指定版本和 key，再解析其中保存的渠道 JSON 文本，修复编辑保存返回 500。
- 映射与固定输入按钮增加明确的可访问名称，避免外层表单标签覆盖按钮名称；补齐钉钉和企微字段中英文文案。
- 投递列表请求删除 Runtime 不接受的 `apiVersion` 字段；Runtime 对死信记录按常量 `dead` 筛选，修复查询不存在的 `status` 列导致 503。“全部状态”使用非空选项值，修复 Radix 下拉框报错。

本轮未修改 API DTO、数据库结构、触发器冻结方式或平台签名策略。回复仍在工作流成功完成后发生，保存渠道配置后需要发布应用使修订生效。

## 验证

本地证据目录为 `.local/artifacts/plan7-channel-form-fix-20261008/`。

| 检查 | 结果 |
|---|---|
| 前端完整 Vitest | 92 个文件、418 项通过 |
| 前端构建与 lint | 通过；构建保留现有体积提示，lint 无错误 |
| Control / Runtime 编译检查 | `cargo check --locked -p platform-control`、`-p agentx-v2-runtime` 通过 |
| 浏览器脚本类型与 Python lint | TypeScript `tsc --noEmit`、Ruff 通过 |
| Kubernetes 系统 E2E | `p7channel1008f`：1 passed、228.33 秒；内部 Playwright 1 项完整流程通过 |

E2E 使用真实 Helm/agentxctl 安装、TLS MySQL/Vault、管理员登录、应用与渠道界面和平台模拟服务。流程覆盖首次应用部署前的输入/输出字段、三个服务端非法配置拒绝、回复开关显隐、多行模板、必填与可选映射、数字固定输入、编辑回显及留空密钥保存、应用发布、签名入站、实际平台多行回复，以及“已送达/死信/全部状态”筛选和消息 ID 展示。Python 进一步检查平台模拟服务收到的正文，不能只以投递状态判定成功。

证据位于 `.local/artifacts/e2e/p7channel1008f/` 与 `.local/artifacts/playwright/helm-agentxctl/p7channel1008f/channel-configuration/`。失败诊断记录保留在前序 Run，最终 Run 的三个临时 Namespace 均已清理。

本轮仅验证上述渠道配置与钉钉模拟回复流程，不替代三个真实平台、容量、安全、供应链或 plan7 全量生产认证。

## 本地升级

通过现有 `agentxctl upgrade` 依次升级 Runtime 和 Control，Helm Doctor 检查成功。`agentx-runtime` 为 revision 10，`agentx-control` 为 revision 11。8 个常驻应用服务均为 1/1 Ready。

| 镜像 | 部署 digest |
|---|---|
| platform-control | `sha256:8dda683b6402ce199a8dbc9b60ab95390d61b112f6f6f350c0d6ef9fc42adcbb` |
| web-console | `sha256:d9a1be9ca41de805e4a3df13ea4b90d762e0c5704af4dc41c69b5c4e931e2322` |
| runtime-gateway | `sha256:f65e9f3c86a86e15df33759ce0933e1485db12546c29d5b3b15d6bac3d9a4638` |

Control/Web 来自源码树 `666aac5d…`，Runtime Gateway 的 SQL 修复来自后续 `fa31076f…`；其余镜像保持先前完整候选。各镜像的 OCI manifest、配置 digest 与源码标签记录在 `image-receipts.json`，本轮是本地组件升级验证。

升级前后，用户原应用与渠道的修订号和配置内容 SHA-256 一致；原 Git 暂存区内容校验值也保持一致。门户 HTTPS 首页、入口脚本、应用详情分块及 runtime-config 均返回 200，首页保留 `Cache-Control: no-store`。镜像加载 Pod、临时归档和 E2E Namespace 均已清理。

## 手动复验

1. 发布一个工作流版本，定义两个必填字符串输入和至少一个可选输入、两个输出。创建应用，在首次应用部署前打开“渠道对接 → 新增渠道”。
2. 确认默认只列出必填输入且来源为空，输入菜单包含全部输入。未配置来源保存应报错；映射必填输入后，按需添加可选映射或固定输入。
3. “开启回复”为关闭时两个回复字段隐藏；开启后输出菜单包含工作流输出，模板可输入多行。关闭再开启应保留内容，未选择输出时保存应报错。
4. 保存后重新编辑，检查映射、固定值、输出选择和多行模板完整回显，密钥留空可保存。发布应用后从专用测试平台发送消息，检查实际回复内容。
5. 打开“投递记录”，检查已送达状态及平台消息 ID，切换“已送达”“死信”“全部状态”，页面与查询均应正常。

## 输入先于回复的布局复验

2026-10-08 后续调整：将新建、编辑表单的三个回复字段整体移到固定输入之后，编辑时的状态位于回复模板之后。开启回复与输出字段并排，模板仍占整行。

现有渠道 E2E 增加页面实际坐标断言，检查新建和编辑的回复开关位于固定输入之后、编辑状态位于模板之后；完整保存、发布和平台回复流程仍通过。`p7channelorder1008` 为 `1 passed in 233.78s`，内部 Playwright 1 项通过。前端镜像构建、页面 lint、浏览器 TypeScript 检查均通过。

验证记录位于 `.local/artifacts/plan7-channel-layout-20261008/`，浏览器截图与报告位于 `.local/artifacts/playwright/helm-agentxctl/p7channelorder1008/channel-configuration/`。

新 Web 镜像为 `sha256:fa54c7cb27ec897087ad1298c9f3f1a24a2f34803ce35cec84acc9b405631ee1`，源码树标签为 `5af909b4…`。通过 `agentxctl upgrade --target control` 升级本地站点，Control revision 为 12，8 个常驻应用服务均为 1/1 Ready；首页、新入口脚本、应用详情分块与 runtime-config 均返回 HTTPS 200。临时测试 Namespace 和镜像加载 Pod 已清理。

## 渠道状态操作收敛

2026-10-08 后续调整：移除渠道编辑表单的状态字段。新建渠道由 Control 默认置为 `active`；编辑请求提交当前渠道状态，启用与停用由渠道列表按钮操作。

现有 E2E 检查新建和编辑表单均无状态选项、新建默认启用、启用状态编辑保持启用、列表停用后编辑保持停用、列表重新启用，以及随后发布和正常回复。`p7channelstatus1008` 为 `1 passed in 233.26s`，内部 Playwright 1 项通过；前端构建、页面 lint 和浏览器 TypeScript 检查通过。

证据位于 `.local/artifacts/plan7-channel-status-20261008/` 与 `.local/artifacts/playwright/helm-agentxctl/p7channelstatus1008/channel-configuration/`。Web 镜像为 `sha256:73cc940ed35781fcea8d4cb4fafd55b2c9f4967ed02e09c4525d9a9065894db7`，源码树标签为 `f836fb59…`。本地 Control 已升级至 revision 13，8 个常驻应用服务均为 1/1 Ready，门户及新应用详情分块返回 HTTPS 200。临时测试 Namespace、镜像加载 Pod 和镜像归档均已清理。
