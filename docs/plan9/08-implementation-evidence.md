# 实施记录与实际证据

状态：complete。更新于 2026-11-02。P9-00–06 已实现，64 份原始 AI 页面、128 次合格网页执行、20 类 MCP、8 类 OpenCode 与 38 项门禁完成；原失败和复核分别保留。结论、质量、费用和证据入口见 [最终报告](evidence/2026-11-02-acceptance.md)。以下保留实施时的历史状态，不把早期失败改写为通过。

## 已有证据

| 检查 | 结果 | 证据 |
| --- | --- | --- |
| APIMart 指定模型认证、流式与工具调用 | 2 项通过；真实请求已计量 | `.local/artifacts/e2e/plan9-model-preflight/` |
| 预算预占、未知用量保留和脱敏 | 预算测试 7 项通过；初始脱敏相关合计 14 项通过 | `tests/acceptance/test_live_model_budget.py` 与预检记录 |
| rmcp 3.8.0 实际 HTTP 协议 | 2025-11-25 与既有 2025-03-26 调用各通过；仅为 SDK 预检 | `.local/artifacts/e2e/plan9-sdk-verified/` 中初始 SDK 预检记录 |
| 正式浏览器隔离和 SDK | 9 个浏览器场景通过，由 pytest 编排；合成表单只证明 SDK，不计真实 AI 页面 | `.local/artifacts/e2e/plan9-sdk-verified/browser/plan9-isolation/` |
| Runtime MySQL/Redis 集成 | 扩展的 runtime_slice 通过，包含成对准备/激活、旧页面冲突、原页面历史、后缀冲突不覆盖他人及 HTML GC 保护 | `cargo test --locked -p agentx-v2-runtime --test runtime_slice` |
| Control 空库 API 回归 | 既有完整 API-first 测试通过；新发布管理仍需系统实测 | `api_first_control_closure_uses_empty_schema_and_public_routes` |
| 登录与换号 | 9 项前端测试通过，包含 401 刷新不跨账号、退出后的迟到响应失效 | `application-page-client.test.ts`、`auth-guards.test.tsx` |
| 前端类型、局部样式检查 | 当前新增页面与相关组件通过 | `tsc -b`、相关文件 oxlint |
| 工具审批 Core/Worker 集成 | 真实 Core 挂起、原模型结果复用、换 Worker 后批准一次、拒绝零写入和重复结算已通过；尚不是完整调度恢复验收 | `.local/artifacts/plan9-model/runtime-slice-final.log`，2 个 Runtime 集成测试通过 |
| 应用 MCP HTTP 服务 | 真实 rmcp 路由发现四工具、正式 Invocation、重复提交、停用入口保留原任务、查询与取消通过；尚未经过外部 AI | 同一 Runtime 集成记录，`application_mcp_contract_is_real_http_and_uses_own_invocation_authority` |
| Hydra/MySQL 实际协议 | 固定镜像的 PKCE、登录/同意、opaque token、刷新、撤销与 introspection 通过；登录/同意由预检驱动，尚不是 Agentx 登录页面验收 | `.local/artifacts/e2e/plan9-hydra-verified/hydra-preflight.json` |
| SDK 并发重复提交 | 两次并发相同提交在私有端口只形成一次请求；隔离的 9 个浏览器场景再次通过 | `.local/artifacts/e2e/plan9-sdk-concurrent/` |
| OpenCode 隔离 | 已安装的 1.1.56 实际读取独立 home/config/data/cache/state；仅指定模型、应用 MCP 权限、无插件；退出清理通过。此检查不调用模型 | `.local/artifacts/e2e/plan9-opencode-verified/` |
| 真实 AI 页面与正式网页执行 | S01 第一轮由指定模型生成 13,578 字节 HTML；成对发布后，仅具应用使用权限的员工在真实页面输入两组未参与生成的数据，正式 Runtime 分别返回 4552.39 / 3379.86；刷新恢复通过，截图已检查。外部修复轮数为 0，首次 Run 内发生一次工具参数错误并由 AI 自行修正 | `.local/artifacts/e2e/plan9-s01-v4/plan9/page-results.json` 及 `browser/` |
| 确定模型失败与恢复 | Core 22 项通过，包含 Host 恢复后不重发已确认失败请求；Runtime 库 176 项通过、2 项既有忽略，包含空输出与截断拒绝 | `.local/artifacts/plan9-model/core-confirmed-failure.log`、`runtime-output-limit-unit-verified.log` |
| MCP 发布命令接管 | 实际 MySQL 中过期 processing 租约可重新领取，未过期租约不能并发领取 | `.local/artifacts/plan9-model/mcp-admission-recovery-verified.log` |
| MCP 管理页诊断与客户端登记 | 正式浏览器点击诊断，四项检查通过；实际登记 Hydra 客户端通过。尚不能代表个人登录同意及业务调用通过 | `.local/artifacts/e2e/plan9-mcp-oauth-s01/plan9/personal-oauth-register.json` |
| OpenCode 低思考配置与脱敏 | 独立安装客户端读取低思考参数的配置验证通过；脱敏 8 项通过，覆盖 Ory access/refresh token 和回调 code | `.local/artifacts/e2e/plan9-opencode-low-effort/`、`tests/acceptance/test_evidence_redaction.py` |
| S01 正式 MCP 协议复测 | 独立客户端发现四工具、两组正式报价、相同请求重放和第三任务取消通过；MCP 服务主体拥有独立 Invocation | `.local/artifacts/e2e/plan9-s01-client-v6/plan9/mcp-protocol-results.json` |
| 原生 OpenCode / rmcp 基础互通 | 已安装客户端在有会话和无会话 HTTP 两种模式均成功读取固定 SDK 的 Echo 工具，原有两版本协议回归同时通过，共 4 项。此检查不调用模型 | `.local/artifacts/e2e/plan9-native-sdk-v3/` |

浏览器最初发现禁用表单同时阻断了正常 submit 事件。正式方案允许 `allow-forms` 并在两层 iframe 强制 `form-action none`，复测确认外部表单和自身导航在发出网络前被阻断；初始失败保留于 `plan9-sdk-browser`、`plan9-sdk-forms`，未删除。

## 已实现范围与待验证项

- `taskId = invocationId` 的只读投影、本人游标历史、真实待办、消息与授权文件；输入等待 responseSchema 固化、服务端统一校验。任务/会话/文件按调用者归属检查，尚需完整权限与故障矩阵。
- 工作流与页面 Profile 使用同一个设计态 Host、持久 Session/Run/Effect 和 Core；页面五工具、正常保存、精确修订、浏览器 ACK 与模拟预览已接入。S01 已通过完整页面创作链；仍需其余场景和受影响画布 AI 回归。
- 应用“访问界面”、独立 `/a/{slug}`、受限 SDK、签名页面版本、Runtime 复制/校验/激活、历史页面与回退入口已实现。S01 已经正式 Kubernetes/Playwright 全链验证；复杂交互、旧版本历史及故障矩阵尚未完成。
- Agent 具体工具审批、Hydra 个人 OAuth、应用 MCP Tab/协议服务已有实现与上表基础证据；完整调度恢复、正式登录同意页面及 OpenCode 自主业务调用仍未通过验收。
- 20 类可重放工作流定义与网页测试驱动已加入 `tests/e2e/product/plan9`，尚不能把定义数量算作执行成功。真实页面通过率仍须以实际浏览器报告为准。
- 真实模型统一账本位于 `.local/artifacts/plan9-model/budget.json`，包含预检与后续创作请求；未知用量继续保留预占。累计上限 USD 30，不按测试批次重置。
- `plan9-first-s01` 首次页面试跑发现异步草稿载入后初始需求未发送，未产生页面生成模型请求；已修复并新增前端回归。`plan9-s01-v2` 已实际调用模型读取契约，但较长 HTML 生成受到固定超时截断，同时暴露状态枚举不一致，三次结果未知均保留费用预占；未计任何页面成功。两次临时环境已完整清理，失败 HTML、事件、截图与日志保留在对应 E2E 目录。
- 模型期限与状态数据库回归已通过：超过 150 秒但未到 Run 期限仍为 sent，到期与已发出请求取消为 outcome_unknown；同时再次通过应用 MCP 协议、幂等、停用后原任务查询与取消。证据：`.local/artifacts/plan9-model/model-deadline-receipts.log`。
- `plan9-s01-v3` 真实请求耗尽输出预算但没有生成 HTML，未计成功；后续修复 Run 在诊断后终止，其未知费用继续预占。指定模型原默认最大思考耗尽输出额度，现使用该模型正式支持的低思考参数，没有更换模型。`plan9-s01-v4` 的页面通过，但 MCP 初始化连接断开；当时测试清理接口错误覆盖了原错误，已改用正式撤销接口并保留原始异常。
- `plan9-mcp-oauth-s01` 再次保留 MCP 初始化断连失败；只对不创建业务任务的初始化增加有界就绪等待，不重试业务写入。个人 OAuth 在真实员工登录后被挑战长度限制拒绝：Hydra 实际挑战约 1380 字符，已将限制调整为 16 KiB；正式登录同意尚待修复后重测。两项失败均未标记通过。
- 批量网页测试按不同应用最多并行 2 个，同一应用三轮串行。每份页面使用独立、原子保存的结果文件，由 Python 汇总；第三轮实际点击旧任务检查原页面恢复。额外四条画布 AI 链和原生 OpenCode OAuth 驱动已有实现，尚未实际通过。
- `plan9-s01-client-v6` 的个人登录、同意、正式报价、刷新、用户撤销与访问/刷新失效断言均通过，但客户端删除调用 Hydra consent 接口时缺少 subject，整体测试仍记失败。删除客户端改用固定版本支持的 client token 清理和客户端删除接口，尚待复测。OpenCode 首次正式接入因工具列表 Schema 被拒绝，没有形成业务任务，不能记为自然语言成功；原生验证器日志已进一步定位为 `outputs` / `error` 使用布尔 Schema。现用已有 schemars `ReplaceBoolSchemas` 生成语义相同的对象形式 MCP Schema，保留原 TaskView 与固定协议，不增加兼容分支。


## 矩阵试跑与修复（2026-11-01）

- `plan9-matrix-v7` / `v8` / `v9` / `v10` 的失败均保留，临时 Namespace 已清理。依次发现空数组示例无法冻结明细类型、测试 MCP 服务未允许 Runtime 访问、业务输出名不符合正式命名规则，以及等待输入类型未传给编译器；不以跳过这些场景替代验收。
- `plan9-matrix-v11` 已通过正式个人 OAuth 全链测试；S01、S02、S03、S04、S05、S10、S20 的独立 MCP 协议执行已通过。20 类协议总门禁仍失败，逐场景结果见该运行的 `plan9/mcp-protocol-results.json`。
- 同一运行中，原生 OpenCode 已自主完成 S01、S04，实际工具回执、Invocation、文件内容和调用身份断言通过。8 类自然语言总门禁仍失败，不把两个成功场景写成全量通过。
- 同一运行还通过原生 OpenCode 个人 OAuth：客户端自行生成 PKCE、打开 Agentx 登录与同意页面、处理回调并缓存令牌，随后自主运行正式报价。Invocation 的真实用户身份已核对；删除客户端后缓存令牌不能继续连接。证据为 `plan9-matrix-v11/plan9/opencode-oauth-results.json`，该检查与手工协议 OAuth 分别记录。
- 等待输入类型传递缺陷已由 Rust 测试实际复现；修复将 responseSchema 固定为等待节点 content 的有效输出契约，内核 91 项通过。审批分支测试同时改用真实 `decision:approved` / `decision:rejected` 端口；编译器不再接受永远不会发出结果的泛用 decision 连线。
- 真实业务模型请求丢失部署默认参数/输出限制的问题已修复，共享 Model 资源契约加入冻结参数及上限，业务执行不读取创作模型设置。Runtime 库 177 项通过、2 项既有忽略；新契约下的系统回归仍待完成。
- 审批回执恢复的最新 MySQL/Redis 集成共 2 项通过，包括原工具成功回执复用与发送后未知结果在策略撤销后仍不重放；证据 `.local/artifacts/plan9-model/approval-receipt-recovery-v3.log`。
- 已补充自然语言修改已有页面、失败轮次独立继续、页面回退/旧任务/Control 停机/Worker 替换，以及审批前策略撤销的系统测试。这些新增测试尚未执行通过。页面最多两轮外部修复和至少 64/128 的门禁不变。

## 候选镜像与完整验收记录

- `plan9-verified-20261101` 的 v4 候选已完成 11 个产品镜像与两个测试服务镜像构建、导入，产品镜像源码标签均为 `56aeda63a53aff2407d1d0d505f4af0e10ad0db9753e784bd94d2e98d91df88f`，并实际用于 `plan9-full-v12`。后续修复正在构建新候选；旧身份记录不能作为新源码的最终证据。v5/v6 的候选构建已主动停止，最终身份将在成功构建后更新。
- 最终候选下的 Control/契约/Bundle 检查 154 项、Runtime 库 177 项、编译内核 91 项通过；Runtime 有两项既有忽略，未计通过。最新 MySQL/Redis 集成 2 项再次通过，覆盖等待 Schema、MCP 标准错误返回、当前页面刷新继续会话与历史页面区分。对应记录位于 `.local/artifacts/plan9-model/`。
- `plan9-full-v12` 的 20 类工作流准备全部通过；MCP 逐场景已有 14 类通过。S06 首次回复使用全角冒号，暴露依赖标点的测试断言；S13 使用了与权威契约不同的循环等待路径。两处验收数据已修正，保留原失败。S17 实际产生审批、主管在正式中心批准后，Runtime 因初始化枚举缺少 `agent_effect_resumed` 而不能保存恢复检查点；写入尚未派发。已主动中止并清理该临时环境，保留诊断、审批和 Runtime 错误记录。初始化枚举已修正，Worker 恢复测试增加唯一检查点断言；尚待系统复测。
- 后续完整测试仍包含 60 份基础页面、4 条画布创作链、额外页面 Host 恢复和版本/账号/入口/故障检查。额外恢复页面不能抵消基础场景缺失，计数还要求独立草稿及创作会话。长报告会主动关闭一次真实 SDK 事件连接，验证带原任务游标重新订阅；这些新增系统断言尚未通过完整验收。
- 测试账号续期只允许使用显式注入的同一账号，验证用户和租户一致；已通过 5 项检查，防止长时间验收中将员工会话意外更新为管理员。OpenCode 私有进程/缓存清理检查也已再次通过。
- 最新前端全量 565 项、Python 仓库验收 223 项、架构边界测试 9 项及实际仓库边界检查通过。完整 `cargo xtask check` 已通过，记录为 `.local/artifacts/plan9-model/xtask-check-full-v9.log`，包含全工作区严格 Clippy、Rust 与实际 MySQL/Redis 集成、前端测试/构建、E2E 类型检查和插件检查。修复中同时收口了编译器可自动修正的已有样式问题；契约内存布局调整后重新生成 Schema/OpenAPI/TS，文件内容变化为 0。后续变更继续执行受影响检查。
- 补充输入辅助工具现按目标待办的原 responseSchema 校验，不让当前发布的静态 Schema 排除旧任务的合法类型。字符串、数组、对象三种历史类型的回归已实际通过，记录为 `.local/artifacts/plan9-model/mcp-original-wait-schema-v7.log`；原生 OpenCode 在此协议说明下仍需复测。
- v7 候选源码身份 `e9cc324c1ee5e556cc1b78595de6add10b87102b1c6d34332c094517acacf2f5` 已构建导入全部 13 个镜像并用于 `plan9-full-v13`。该运行的 20 类 MCP 协议全部通过，正式个人 OAuth 再次通过，OpenCode 自主完成 S01/S04/S06，包含真实会话后续消息。另以只读核查验证 40 个协议任务的原始输入与上传字节元数据，记录为 `plan9-full-v13/plan9/mcp-input-integrity.json`。
- `plan9-full-v13` 的 OpenCode S08 暴露无显式类型的通用补充输入 Schema 会让客户端持续发送 JSON 字符串；服务端按原待办拒绝，不能视为已补充成功。已主动中止后续测试，三个临时 Namespace 与客户端私有目录已清理，客户端尝试、诊断和既有通过记录保留。v8 镜像构建在前端类型检查时主动停止，不能作为最终候选。
- 后续真实模型预检证明：仅枚举全部 JSON 类型仍会诱发对象字符串化；直接使用真实单表单和并行表单 Schema 均通过。混合对象/文字时，仅 `anyOf` 的初试失败，补充等价显式类型的预检通过。证据分别在 `plan9-native-input-probe`、`plan9-concrete-input-probe`、`plan9-explicit-input-probe`，失败没有删除。v9 构建因此主动停止。正式修复合并同应用 MCP/网页历史的真实等待契约并补充可证明的类型，服务端仍按原待办验证；没有字符串解析兜底或旧类型排除。HTTP/MySQL 的新旧契约联测和两项 helper 单测通过，最终候选仍待系统复测。
- v10 候选已完成全部 13 个镜像构建与导入，11 个产品镜像标签全部核对为 `273caa1deb4e13edd9705368805236b750b03c0d7889b48e4c8da9bfd5c4f4e8`。原 v7 身份单独保留为 `image-identity-v7.json`，当前身份记录见 `.local/artifacts/plan9-model/final-image-identity.json`。新一轮指定模型的三项类型预检全部通过：`.local/artifacts/e2e/plan9-typed-input-verified/`；严格 Clippy、两项输入 helper 单测及前端 565 项也已通过。
- 最新 Runtime 集成中，MCP 路由通过；另一项基础恢复测试在并行镜像构建时触发原有 500 ms 锁检查超时，记录为 `runtime-slice-current-v11.log`。未放宽断言，构建结束后独立顺序复跑两项均通过，记录为 `runtime-slice-idle-v12.log`。完整系统验收现运行于 `plan9-full-v14`，包括新增的并发决定、重复/迟到决定、取消、审批超时、非法输入与对话原文检查；这些门禁须等待实际结果。
- `plan9-full-v14` 已再次通过全部 20 类 MCP 协议和正式个人 OAuth。独立 OpenCode 的 S01/S04/S06/S08 已通过，其中 S08 只调用一次 `submit_task_input`，提交原生交付对象并完成原任务，修复得到实际客户端验证。工具审批截图已人工检查：具体业务参数清晰可见，技术校验信息收起，文案和控件使用现有门户样式。其余客户端场景与完整页面矩阵仍在执行，不能将此进度记为最终通过。
- `plan9-full-v14` 最终取得 8 项通过：20 类准备、20 类 MCP、个人 OAuth、8 类 OpenCode、自身 OAuth、MCP 边界、审批后更换 Worker 和写入响应丢失不重放。随后撤权场景准备代码误用了仅含模型标识的轻量对象，缺少已发布模型版本，进入业务调用前报 `KeyError: deploymentId`；该轮整体仍失败，页面矩阵没有执行。已改为通过正式 API 读取同一模型的已发布快照。三个临时 Namespace 与私有客户端缓存已清理，诊断保留于 `plan9-full-v14/diagnosis.json`。
- 产品源码身份仍为 `273caa1deb4e13edd9705368805236b750b03c0d7889b48e4c8da9bfd5c4f4e8`，没有因测试准备修复重建产品。`plan9-pages-v15` 先执行 60 份页面及 4 条创作链，再验证剩余故障/发布边界，并对这些生成页面对应的 20 类应用复测 MCP。已通过的独立 AI 与 Worker 门禁使用同身份的 v14 证据，分阶段记录，不能把缺失或失败页面合并成成功。
- `plan9-pages-v15` 的 S01/S02 第一轮均首次通过，共 2 份真实页面、4 次正式网页执行；S01 的自然语言标题和两列布局修改、S02 的明细增删及结果表格已检查实际截图。第二轮的测试监听误选了加载既有草稿时创建的会话，实际新草稿 Run 均已完成，而测试一直轮询另一个会话。已保存原始 HTML、真实 Run/Session 标识和诊断，并中止该轮。修复仅让测试严格匹配新 draftId 的绑定响应，同时核对启动 Run 的 sessionId；没有修改生成 HTML，也不将两份未执行业务的页面计为 AI 失败或通过。完整矩阵转入 `plan9-pages-v16`。

## 环境

`plan9-full-v18` 使用源码 `61e8eb118a84afcb7d927e6f22ce2892bb16a1ca82c948eb1864add45d81c2e3` 的 13 个已核对镜像启动。20 类工作流准备通过；本轮 MCP 为 19/20，S18 在模型流完整结束标记之前中断，原任务失败且无工具 Effect、CRM 写入为零，保留失败而不放宽流完整性检查。个人 OAuth、OpenCode 的 8 类自主业务调用及原生个人 OAuth、MCP 权限边界、换 Worker 恢复和写入响应丢失不重放均通过。审批撤权、并发决定与取消后迟到决定暴露待修问题，不能引用更早的局部成功覆盖本轮失败。

v20 修复 Control 对 Runtime `applied:false` 的错误成功回执，已确认未派发的撤权操作保存确定失败；MySQL/Redis 集成 2 项及严格 Clippy 通过。浏览器反例 `plan9-window-red-v19` 使用合成结果证明站外打开者仍能收取转发，加入可信外壳 COOP/XFO 后已通过；未使用真实业务资料。预览重复通知和提交后标签误判也已修复，16 项隔离/预览浏览器检查通过；S08 第一轮最终 AI HTML 未改一字，在修复后的合成检查中连续通过两次，记录为 `plan9-recorded-preview-v20b`，此证据不计正式业务执行。

为继续复用本轮真实生成页面，2026-10-31 23:44 UTC 仅将本次临时 Control Namespace 的 Web 更新为 `plan9-final-v20-20261101`，源码 `0a1c3dd972cdc6e2cd55dce9fbab0d7527b48231650b958d91a72c9ef97c9ef3`；两个 Web 容器镜像同时更新，未改数据库、凭据、原 HTML 或发布绑定。分阶段身份记录见 `plan9-full-v18/v20-web-rollout.json`。这不是全程同一镜像的验收：原检查失败、滚动更新期间的端口转发中断和后续复核分别保留；完整新候选仍需闭合失败门禁。

v20 的 13 个镜像已全部构建、导入并核对源码标签，记录为 `image-identity-v20.json`。随后临时 Control、Gateway、Coordinator 和 Worker 分阶段更新，身份与原策略保存在 `v20-control-rollout.json`、`v20-runtime-rollout.json`，常驻服务未改变。Control 与 Runtime SDK 短暂不一致导致的发布拒绝保留，不要求 AI 修改 HTML 规避。清理连接中断时另误停了一条仍在等待的 S11 创作，该次取消作为实际失败保留；未补发旧 Effect。

这轮还定位到共享 Host 把短时收据查询不可用直接视为结果未知，甚至尚未提交模型的 Run 也立即终止。v21 改为同一 Effect 的 Pending/收据重查，临时提交响应丢失同样先检查原收据，明确拒绝与权威未知结果仍分别终止；源码 `0a2063169eb0cfd1b447dd15dc665e04d3c55212b44e0bb70f0fc96c15042cef`，严格 Clippy 通过，实际断 Gateway 的原 Run 恢复测试已加入，尚待系统执行。S20 第一轮三个启动和 S11 第三轮最后一个启动均以真实数据库核查证实没有任何 Runtime 调用，不将其冒充 AI 生成，也不将启动失败记为首次成功；证据为 `plan9-full-v18/plan9/pre-dispatch-failures.json`，已有实际 AI 调用仍计入两轮修复上限。

v21 的完整 `cargo xtask check` 已通过：前端 572 项、既有 Python 验收 223 项、Rust 与实际数据库集成、部署/契约/类型/边界检查均有日志；13 个镜像标签已核对，12 个文件系统与 v20 一致，只有 Control 改变。身份及记录为 `image-identity-v21.json`、`xtask-check-v21.log`。同一批次报价第一轮由指定模型完成自然语言布局/标题修改和两组正式报价，实际截图已检查；部分其他页面仍失败，不能因静态检查或局部成功将矩阵标成通过。

`plan9-full-v18` 的 S01 第二轮三次实际 AI 尝试都把允许小数的折扣控件限制为 `step=1`，两组留出数据中的合法小数在浏览器内被阻断，未产生调用。这是实际生成失败，保留其三次 HTML 和错误，不追加第四次。原预览只填整数，诊断也没有给出浏览器校验原因。v22 在 Schema 允许时使用有效小数，记录原生校验失败并标明等待表单所属契约；源码 `03319aab1871e2bbf250428722308eda3a42e82acb685a0a7a1f48bd59427b21`。6 项相关单测、类型检查和 18 项隔离/预览浏览器场景通过；同一份未改动的原 AI HTML 在 `plan9-recorded-numeric-v22` 中被准确拒绝，合成证据不计正式业务执行。

v18 批次最终留下 29 份页面结果，其中 14 份页面、28 次正式网页执行通过，整体失败。已在结束前保存全部 20 类工作流及页面发布绑定、27 份仍存草稿，原 HTML、各次模型记录和浏览器证据全部保留；三个临时 Namespace 已卸载，见 `plan9-full-v18/diagnosis.json`、`uninstall.json` 与 `plan9/cohort-snapshot-*/`。该批次经历镜像与测试诊断修订，不作为同一冻结候选的完整验收。

v22 已完成全量 `cargo xtask check`：前端 573 项、既有 Python 验收 223 项、Rust/严格 Clippy/真实 MySQL 与 Redis、Schema/Helm/边界和浏览器类型检查通过，日志为 `.local/artifacts/plan9-model/xtask-check-v22.log`。13 个镜像构建、导入和源码标签均已核对，见 `image-identity-v22.json`。新的 `plan9-full-v22` 固定产品源码与测试源码，在独立临时环境执行 41 项系统测试，包含所需 60+4 份页面以及两份单独统计的 Host/收据服务恢复页面；不将恢复页面抵消基础缺失。

本轮 20 类工作流准备、20 类 MCP 协议及正式个人 OAuth 已通过；OpenCode 为 7/8，原生 OAuth 也通过。客服会话 S06 首次因上游流缺少 `finish_reason` / `[DONE]` 而失败，原 Invocation 已明确失败且工具 Effect 为零；该失败记录为 `plan9-full-v22/plan9/native-S06-first-stream-failure.json`，不能写成首次成功。Worker 恢复、写入响应丢失不重放、撤权、并发决定和取消/超时迟到批准均通过。全局 USD 30 账本不重置。

v22 进入三份页面创作后，S04 第一轮首次完成两组文件业务执行；截图检查发现统一样式没有让较长 Hash/摘要换行，文字会挤入相邻结果字段。已停止该不完整候选并保留三份草稿、全部创作记录和原 HTML，三个 Namespace 经 `agentxctl uninstall --purge-data` 清理；见 `plan9-full-v22/diagnosis.json`、`plan9/cohort-snapshot/` 和 `uninstall.json`。中断时一份 Trace ZIP 未写完，无法安全解码脱敏，记录其 Hash/大小后删除不可读原文件，其余模型记录、HTML、结果和截图保留，见 `post-exit-redaction.json`。

v23 仅补齐通用长文本换行及创作规范提示，没有人工修改生成 HTML，也不扩大隔离权限。浏览器反例 `plan9-result-layout-red-v23` 实际复现失败，修复后 `plan9-result-layout-green-v23` 的 19 项浏览器场景全部通过。源码 `df93d5ccc105619d3527c0a19fbf91cb67aaf74adb015806b2c0801e334f68b4` 已完成全量 `cargo xtask check`，包含前端 573 项、Python 223 项及 Rust/真实数据库/类型/边界/部署检查；13 个镜像均已构建、导入并核对源码身份。10 个镜像文件系统与 v22 一致，变化仅为 Control、Web 和 Gateway，证据为 `xtask-check-v23.log`、`image-identity-v23.json`。2026-11-01 01:27 UTC 已启动 `plan9-full-v23` 的 41 项完整系统验收，先执行 60+4 页面矩阵，再执行客户端与故障回归，所有旧批次保持独立统计。

v23 在五份页面进入创作后停止，S04 三轮均首次完成正式业务，共三份页面、六次执行。S08 的真实生成 HTML 把 `allowedActions` 对象当成数组调用 `indexOf`，渲染在订阅前失败；页面自己捕获并显示了 TypeError，原检查报告却只有“任务未更新/缺少待办表单”，无法给出具体原因。已保留五份草稿、全部创作与检查记录；先关闭浏览器 Runner 再单独中断 pytest，正常 fixture 清理删除三个临时 Namespace，本次 Trace 均可读取脱敏。证据为 `plan9-full-v23/diagnosis.json`、`plan9/cohort-snapshot/`、`uninstall.json`。首次模拟预览单独复核为 3/4，另有一页尚未形成该检查，见 `plan9/first-preview-audit.json`；这些局部计数不是完整验收。

v24 为预览添加有界可见文字和本次点击异常，契约工具直接提供权威 TaskView Schema，并明确 allowedActions 对象结构。两项反例在 `plan9-error-diagnostic-red-v24` 复现，修复后 21 项浏览器场景通过。S08 原 AI HTML 未改动，在 `plan9-recorded-caught-error-v24` 中准确报告原 TypeError；合成复核不计正式业务成功。相关前端 9 项、Web/E2E 类型和 Control 严格 Clippy 均通过。源码 `d2459ba8aa719ec5fd4986bdc1f74bfa8d1e8de61402bf36299268e1e18ff397` 的完整 `cargo xtask check` 已通过，包含前端 573 项、Python 223 项及 Rust/真实数据库/类型/边界/部署检查，日志为 `xtask-check-v24.log`。13 个镜像已全部构建、导入并核对源码身份，见 `image-identity-v24.json`；新环境的 `plan9-full-v24` 于 2026-11-01 02:03 UTC 启动 41 项完整系统验收。当前仍未达到 64/128 与全部门禁的完成条件。

v24 的两份初稿进入检查后，正式保存检查报告的接口因未知 `diagnosticText` 字段拒绝请求；此前只补了浏览器报告，遗漏了严格的服务端 DTO。没有页面发布或正式业务执行通过，不能把它归因于 AI 不会生成。原 HTML、接口拒绝和调用记录保留在 `plan9-full-v24/plan9/cohort-snapshot/`。采证汇总另因拒绝 ACK 没有 mode 而报错，已让汇总跳过非检查回执，并从原结果生成 `post-exit-audit.json`；没有补造未取得的 Runtime 调用账本。正常 fixture 清理已卸载三个 Namespace，见 `uninstall.json`、`diagnosis.json`。

v25 补齐 Rust 报告 DTO、8000 字符限制、持久化和 OpenAPI，并让网页使用生成类型。切换生成类型后，未同步契约的原代码实际被 TypeScript 拒绝，记录为 `report-contract-red-v25.log`；同步生成后 `report-contract-green-v25.log` 通过，相关前端 9 项也通过。源码 `4329a69082991d1099e0205fb9c034f746e770fa220b0f596d2fc0d0638160ed` 的完整 `cargo xtask check` 已通过，前端 573 项、Python 223 项及 Rust/真实数据库/类型/边界/部署检查有日志；13 个镜像已构建、导入并核对源码身份，见 `xtask-check-v25.log`、`image-identity-v25.json`。`plan9-full-v25` 于 2026-11-01 02:40 UTC 开始 41 项系统验收，新环境先执行四条画布到页面的完整链，再运行 60 份基础页面及全部客户端、故障与回归门禁，仍不增加旧页面的修复次数或手改 HTML。

v25 的 S08/S12/S14 三条画布到页面链均首次创作后完成两组真实网页输入。S19 画布静态校验与发布通过、网页模拟也通过，但真实工具批准写入后，AI 生成的 `parse_crm` 绑定名 `crmText` 与 Python `main(crm_text=None, **_)` 不匹配，原程序解析 None 失败。测试错误地把这类后端失败连续反馈给页面 Agent，耗尽三个页面 Run；该旧页面保持失败，不追加第四次。原始工作流、HTML、全部创作事件和任务、Sandbox 日志保留；03:10 UTC 正常清理三个临时 Namespace，见 `plan9-full-v25/diagnosis.json` 和 `uninstall.json`。

本次只修正验收编排的跨 Agent 反馈与累计预算，不修改产品、业务图或 HTML。新增六项检查已通过，覆盖运行失败转画布、静态修复占用额度、版本变化不能获得第四次、普通页面错误不重建流程、未知/未结算 Effect 阻止重试，以及只读入参诊断。E2E TypeScript 类型检查通过。后续 `plan9-full-v26` 使用同一 v25 产品源码和镜像，重新执行完整矩阵；旧轮次不合并为本轮成功数。

v26 的 S08 创作链再次首次通过，两组网页输入完成真实等待、恢复和输出；实际桌面截图已检查。S12 画布创作的第七次模型调用返回 `DESIGN_MODEL_STREAM_INVALID`，原 Run `22677740-9912-4e89-8544-2f2e5a5a4a46` 保留 `outcome_unknown`，未执行该流程业务。测试在第一次创作未知时提前退出，尚余两个显式新创作 Run 的额度；03:28 UTC 结束该批并正常清理三个 Namespace。补齐画布验收驱动的有界新请求，先读已保存草稿、保留原 Effect 和未知费用，不重放原调用；上限仍为三个。增加既有 Gateway 故障回归中的实际新 Run 恢复断言，原 Run 必须保持未知且没有新增旧调用。类型检查与六项编排检查通过；此处的创作恢复与业务未知写入禁止重试保持区分。`plan9-full-v27` 继续复用完全相同的 v25 产品镜像重新验收，旧轮次仍为未完成。

v27 的 S08/S12/S14 再次首次通过，共六次正式网页执行。S19 的真实画布编辑遇到 `EDITOR_ACK_INVALID`：原命令 `outputExample.total` 为 `900.0`，浏览器回执为 `900`，业务定义只有这一处数字表示差异，其他差异全部是允许自动布局的坐标。已保存原命令、被拒回执和完整创作记录，03:48 UTC 正常清理三个临时 Namespace；这一轮保持未完成。v28 使用现有 JCS/documentHash 语义校验回执，保留全部布局、修订、权限和业务值检查；反例先实际失败，修复后布局测试三项通过，记录为 `layout-number-red-v28.log` / `layout-number-green-v28.log`。画布 Profile 同时补齐实际 Agent/Code 输出字段、Python 关键字和跨节点引用说明，未人工修改生成图或 HTML。源码 `5fc07aa49b3348ca73813f1dd5e6902bfa13d6f83fe7e371b7a691cdf87ec0e1` 正在执行完整 `cargo xtask check`，随后构建和核对全部 13 个镜像，再启动新的完整系统验收。

v28 的完整 `cargo xtask check` 已通过，包含前端 573 项、Python 229 项（新增六项编排检查）、全工作区 Rust/严格 Clippy、真实数据库、契约/类型/边界/部署与插件检查，日志为 `xtask-check-v28.log`。当前源码的隔离/预览/布局浏览器 21 项也全部通过，零失败、零跳过，证据为 `plan9-isolation-v28` 与 `isolation-v28-proof.json`；这些合成检查不计入 64 份真实页面或 128 次业务执行。13 个新候选镜像正在构建、导入与核对，系统验收未因此提前标为完成。

13 个 v28 镜像现已完成构建、导入，并核对全部源码标签和 Kubernetes containerd 中的实际镜像，证据为 `image-identity-v28.json`。`plan9-full-v28` 已启动 41 项完整系统验收，产品及测试源码在执行期间冻结，完整链路、基础 60 页面、协议/客户端和故障回归继续按同一候选验证，所有旧批次保持独立统计。

v28 的 S08 画布在首个 Run 内多次自检修正后发布成功，网页初稿及模拟检查通过。正式任务正常进入 `/await_delivery` 等待，测试却仍按预置流程的 `/delivery` 取数据，报 `No unseen input configured`；该准备错误不作为 AI 界面错误或成功。04:23 UTC 已保存原始图/HTML/事件并清理三个 Namespace，本轮未完成。测试改为从不可变 AI 图绑定实际路径，保留两组输入原值，明确拒绝歧义、业务字段变化和错误子流程版本；10 项检查通过，原始 S08 图的只读映射也已验证，见 `plan9-full-v28/plan9/diagnostics/real-graph-input-binding.json`。后续 v29 复用完全相同的 v28 产品镜像和源码，仅修正测试编排，不给旧页面追加第四次创作，也不人工修改 AI 内容。

另一个历史按钮失败来自测试在异步列表渲染前判断按钮数量，随后点击了没有事件的行；已等待真实行可见后操作原按钮。历史文件按授权下载内容/Hash 验证，取消未知结果按真实状态和禁止重放验证。旧失败均保留，不能通过手动改 HTML 或显示内部标识代替真实交互。

`plan9-pages-v16` 在 12 份页面进入创作后主动结束，4 份页面及其 8 次网页执行通过；该轮没有满足完整矩阵。S01/S02 的历史查询误用应用 ID、修复后发布就绪判断只比较同一后缀，均已修正测试；原失败未改写成通过。S04 的正式下载进一步定位到 Bridge 使用 RFC 版本位严格校验，拒绝 Runtime 合法的确定性 Artifact 标识，下载请求未发出；这是平台缺陷，不能交给 AI 改 HTML 规避。已保存 12 份草稿及诊断，三个临时 Namespace 全部清理，原始记录位于 `plan9-pages-v16/diagnosis.json` 与 `plan9/interrupted-drafts/`。

下一候选将 Bridge 统一为 UUID 形状校验，服务端归属授权保持原逻辑，并增加真实私有 MessagePort 的合法/非法标识回归。模拟任务改为异步返回等待/完成，要求页面订阅或查询更新；字段缺失、第二行数组标记和行删除动作在检查时直接报告。历史任务导航保留当前入口路径，避免旧版本后缀失效。新源码的前端 572 项、浏览器隔离/检查 13 项及类型检查通过；浏览器记录为 `plan9-page-quality-v18/`，合成页面不计入真实 64/128。v17 镜像构建在发现文件标识缺陷后中止，v18 使用独立 `plan9-quality-20261101` 标签重新构建；实际矩阵仍待新候选验收。

已使用一次性 Docker MySQL/Redis 容器进行集成测试；最终还会核对并清理本次遗留容器。Docker 凭据助手阻塞通过独立匿名 Docker 配置解决；没有更改日常 Docker 凭据。前六次临时 Kubernetes 试跑环境均已清理。Hydra 与独立 MySQL 曾实际部署就绪；常驻 Kubernetes 服务未缩容、升级或替换。测试使用已安装的 Helm 3，未替换系统 Helm 4。真实模型上游密钥只在计量进程内存中，临时环境只接收 relay 凭据；最终统一清理。

v29 已归档并清理三个 Namespace，完成基础页面 9 份/18 次正式执行及 S08/S12/S14 创作闭环 3 份/6 次执行；这些局部结果不构成完整验收。S19 创作链路在三个 Canvas 和三个 Page Run 后仍因整数输出契约拒绝真实小数金额，保留失败。S08-r1、S12-r3 也用尽三次，不能追加第四次。另两份进行中页面随中断保留，不能按成功统计。问题及原 HTML/图/任务见 `plan9-full-v29/diagnosis.json`、`plan9/cohort-snapshot/` 和各轮目录。

v30 修正预览实际提交载荷校验、待办容器定位、仅保留类型的脱敏诊断，以及数学数值一致的 Schema 推导；增加受保护节点与金额示例提示。旧预览的两个数据反例、数字推导反例和容器/按钮反例均先实际失败，保留红测日志；不人工改动 AI 图或 HTML。后续重新构建新候选，执行全库检查和完整矩阵。

v30 的完整 `cargo xtask check` 已通过（前端 575 项、Python 234 项、全工作区 Rust/严格 Clippy、真实数据库、契约/类型/边界/Helm 与插件检查）；对应源码为 `01f29fba27fadc1938bf4319b469fed343de06b4ca3d69a685a72d45d4d1e597`，日志 `xtask-check-v30.log`。浏览器隔离/预览/表单检查 23 项全通过，零跳过，证据 `isolation-v30-proof.json`。这些检查不计作 64 份真实页面。正在构建并核对 13 个新候选镜像，随后执行完整系统验收。

v30 已实际通过 S08/S12 两条完整创作链及四次正式网页执行（每条 Canvas/Page 均首 Run 完成）；S08 首次模拟检查存在标签错误，由 AI 在原 Run 内修复，首次草稿与首次 Run 指标分开记录。S14 随后暴露编辑器表示问题：合法 Exit 参数省略可选 `errorOutputs`，浏览器补齐空对象后触发严格比较。只读调用原序列化器复现，差异仅两处空映射，见 `plan9-full-v30/plan9/diagnostics/canvas-roundtrip-difference.json`；06:14 UTC 结束候选，正常清理三个 Namespace、保留全部原始命令/图/HTML，零损坏 Trace。该轮保持未完成，S14 实际创作次数按原数据库及逐轮报告核对，不以中断时尚未更新的汇总零值替代。

v31 修复设计工具的类型默认值规范化，并为其他不一致表示提供有界、不含业务值的具体路径。反例 `exit-defaults-red-v31.log` 先实际失败；修复后后端单项及前端序列化/诊断九项通过，日志为 `exit-defaults-green-v31.log`、`canvas-representation-v31.log`。业务映射未人工修改，精确 Hash/ACK 校验保留。源码 `aab1cf88da4ee2e867335fb4075e4a2e7d7d049b7087cc65257a9e6b3897ecc9` 开始全库检查及新候选准备，仍需完整 64/128 和其余门禁。

v31 的完整 `cargo xtask check` 已通过，包含前端 578 项、Python 234 项、全部 Rust/严格 Clippy、真实数据库、契约/类型/边界/Helm 和插件检查；日志 `xtask-check-v31.log`。新源码的浏览器隔离/预览/表单 23 项全通过、零跳过，证据 `isolation-v31-proof.json`。现在构建、导入并核对 13 个 v31 镜像，随后执行新的完整系统矩阵；旧轮次仍为未完成。

v31 构建期间仅把四条创作链的验收顺序改为 S14、S19、S08、S12，优先验证已发现问题；四条链与 60 份基础页面均保留。编排/次数限制/路径映射 11 项再次通过，产品源码和镜像身份不变，最终测试指纹在启动前统一固定。

v31 已通过 S14/S19/S08 三条完整创作链及六次正式执行。S19 的只读图/AST 证明先等待后 Agent、关键字绑定一致且金额示例包含小数；真实 CRM 凭证、一次写入和下载均通过，见 `authoring-semantics-readonly.json` 与逐次任务证据。随后发现 S08 首次 HTML 的外层卡片与内层表单同时带 waitId，测试错误地视为两个表单；初稿本身合法，后续由该误报触发的 AI 修复原样保留，单独归为测试问题。07:01 UTC 结束候选并正常清理三个 Namespace，本轮仍未完成。

v32 仅修正测试定位为最内层非动作待办容器，不修改产品或原生成内容。浏览器反例先失败；五种合法嵌套布局及真正独立表单的歧义检查加入后，隔离/预览共 27 项全通过，零跳过；E2E 类型检查和编排 11 项通过。复用源码 `aab1cf88da4ee2e867335fb4075e4a2e7d7d049b7087cc65257a9e6b3897ecc9`、已通过的 v31 全库检查和原 13 个 v31 镜像，重新核对全部源码标签及集群实际镜像后启动 `plan9-full-v32`。验收顺序先 S08/S12，再 S14/S19，最后基础 60 页和其余门禁；新旧轮次独立统计，仍须全部 64/128 通过。

v32 的四条完整创作链现已全部通过，共八次正式网页执行；Canvas 外部 Run 次数分别为 S08=1、S12=1、S14=2、S19=3，Page 次数分别为 2、1、1、1。S14 的首次模型流未知与 S19 的参数/文档 Schema 错误均保留，不能算首次通过。基础矩阵目前通过 S04 三轮及 S08 第一轮，共四份/八次执行；S04 第三轮历史页面重开与文件内容验证通过。其余基础页面、协议/独立客户端与故障回归仍在执行，完整验收状态保持未完成。

v32 随后完成 S04/S08/S12 全部九份基础页面及十八次正式执行，加上四条创作链，共十三份页面、二十六次正式执行。S08-r2 两次外部 AI 修复后通过，其余八份基础页面首 Run 通过；原 HTML 和失败保留。S19-r1 正式任务暴露预置流程的 `receipt.outputExample.total=0.0` 被正确推导成整数，拒绝真实小数金额；同类样例还影响 S01/S02/S03/S17/S18。这是测试流程定义错误，页面 AI 无法修复，不归为页面效果失败或成功。已中断无效页面修复、保留原任务和中断时实际 Run 快照，08:06 UTC 正常清理三个 Namespace，证据见 `plan9-full-v32/diagnosis.json` 与 `plan9/cohort-snapshot/`。本轮继续保持未完成。

v33 只修正预置数值类型示例和编排：不改变业务输入/预期值、不修改 AI 图或 HTML；20 类正式 MCP 协议作为共享 session fixture，先全部通过才可付费生成页面。Python 验收 234 项、45 个 plan9 文件的 Ruff/格式检查通过，41 项系统测试收集完整。产品源码仍为 `aab1cf88da4ee2e867335fb4075e4a2e7d7d049b7087cc65257a9e6b3897ecc9`，继续复用 v31 全库检查、27 项当前浏览器证明和原 13 个镜像。新的完整候选需独立完成所有 64/128 及剩余门禁，旧轮次不晋升为最终通过。

v33 已实际完成全部 20 类 MCP 协议场景，40 次业务执行、每类幂等与取消辅助检查均通过；包含 S01/S02/S03 的小数与类型检查，以及 S17/S18/S19 的真实具体工具审批、拒绝后新提案、CRM 一次写入和文件内容验证。共享前置 fixture 已放行四条 Canvas → Page 创作链；正式 64 份页面及 8 类 OpenCode 自主调用仍待本轮验证，协议调用不计入 128 次网页执行。证据为 `plan9-full-v33/plan9/mcp-protocol-results.json`。

v33 的 S08/S12 完整创作链通过，两份真实页面及四次网页执行完成；S08 Canvas 三个 Run（前两个模型流未知），S12 Canvas 首 Run，两个 Page 均首 Run。S14 三个 Canvas Run 全部因预算终止而失败，未创建 Page，不能追加第四次。S19 首个 Canvas Run 随停止保留；基础 60 页面尚未开始。08:50 UTC 已正常清理三个 Namespace，20 类 MCP/40 次业务执行仍只作为本轮局部通过，全部原始命令、事件和页面保留。只读前端复现把重复通用拒绝定位到 `exit-error:parameters.outputs.request_id` 空绑定，见 `plan9-full-v33/plan9/diagnostics/s14-original-rejected-validation.json`。第三个 Run 的浏览器取证被原令牌过期打断；数据库快照确认其仍是 budget_exhausted，完整事件已另行恢复，原报告的零耗时保留为取证缺失，不按实际零耗时统计。

v34 修正实际诊断链，不人工更改图或 HTML：ACK 保留有界错误码/节点/字段，工具与 Exit 参数拒绝复用现有 Schema 返回具体路径且不回显业务值；无新增依赖、无校验放宽。补齐现有 Profile 对 upsert 字段层级、错误专用 Exit 空映射的说明。实际失败形态的前端反例先失败；修复后前端十项与后端五项通过，见 `canvas-field-diagnostics-red-v34.log`、`canvas-field-diagnostics-green-v34.log`、`canvas-tool-diagnostics-v34.log`。长创作测试改为使用真实 UI 续期响应的令牌，并在读取事件前持久化耗时；取证错误继续使测试失败。新源码 `1fa5b4332c3b61785e4482acf752603a6a5bfd216a65f8b08206d462c25d605e` 正在全库检查，随后重建并核对十三个镜像，再执行完整系统验收。

v34 的完整 `cargo xtask check` 已通过：前端 580 项、Python 234 项、全部适用 Rust/严格 Clippy、真实数据库、契约/类型/边界/Helm 与插件检查。首次检查调用系统 Helm 4 后在部署校验处停止，日志另存 `xtask-check-v34-environment-failure.log`；切换仓库要求的 Helm 3 后完整重跑通过，见 `xtask-check-v34.log`。当前源码的浏览器隔离/预览/表单 27 项全部通过，零跳过，见 `isolation-v34-proof.json`；E2E 类型检查通过。源码身份复核未变，十三个新镜像正在构建、导入与核对；这些静态/合成检查仍不计作正式 64/128。

十三个 v34 镜像现已构建、导入，并核对 Docker 源码标签及 Kubernetes containerd 实际镜像，见 `image-identity-v34.json`。`plan9-full-v34` 已启动同一源码的 41 项完整系统验收：先协议 20，再四条创作链、基础 60 页面和其余恢复/权限/回归门禁；本次执行期间产品和测试源码冻结，仍须独立满足全部 64/128。

v34 已实际通过全部二十类 MCP 协议场景及四十次业务执行，包含文件、会话、并行/子流程/循环等待、两级审批、工具拒绝后新提案、CRM 写入计数、文件内容与取消检查。证据为 `plan9-full-v34/plan9/mcp-protocol-results.json`；这些调用不计入 128 次网页执行。四条 Canvas → Page 真实创作闭环已开始，64/128 和其余门禁仍未完成。

v34 的 S08/S12/S14 三条完整创作链已通过，三个 Page 均首 Run 完成，两组业务输入合计六次正式网页执行。S08 Canvas 首次被提供方拒绝，第二 Run 完成；S12/S14 Canvas 首 Run 完成。S14 已实际走主管批准和拒绝两条分支，本轮首次成功与 v33 三次失败分别保留，不能将旧轮次改写为通过；对照证据为 `plan9-full-v34/plan9/diagnostics/s14-after-feedback-fix.json`。S19 创作链及基础 60 页面仍未完成。

v34 的四条完整创作链现已全部通过，共四份真实页面、八次正式网页执行；四个 Page 均首 Run，Canvas Run 次数 S08=2、S12=1、S14=1、S19=1。S19 已实际完成补充资料、主管工具审批、CRM 一次写入与文件交付，两组留出输入均通过。证据为 `plan9-full-v34/plan9/diagnostics/four-creation-chains-v34.json` 及逐轮原始记录。基础二十类×三轮矩阵已开始；完整 64/128、八类独立 AI 与其余门禁仍未完成。

v34 随后通过六份基础页面（S04 三轮、S08 前两轮、S12 首轮），合计十份页面、二十次正式网页执行。本轮 S08-r3 的原始 HTML 首次模拟成功，第二次检查却因复用已切换到任务视图的 DOM 而误报初始字段缺失；只读浏览器复现确认初始表单和结构化等待均成功，随后相同 HTML 的重复检查失败，见 `plan9-full-v34/plan9/diagnostics/repeated-preview-readonly-reproduction.json`。本轮已停止、保留全部原始尝试与两份中断页面，三个 Namespace 正常清理，仍为未完成，不向旧页面追加第四次创作。

v35 修复预览生命周期：每次检查撤销旧端口，清空合成任务/订阅/计时器，重新握手并串行检查；HTML、Hash、契约及业务校验不变，正式业务宿主不调用该重置。新增实际页面视图切换的重复检查、旧任务清理和端口恢复反例；修复前失败，修复后十一项相关单测通过。未改动的 v34 原始 AI HTML 在新的独立浏览器场景中连续完成两轮补充输入，隔离/预览二十七项均通过，零跳过；证据为 `preview-reset-red-v35.log`、`preview-frame-green-v35.log`、`recorded-repeat-green-v35.xml`。这些合成结果不计正式 64/128；新候选仍需全库检查、镜像核对和完整系统验收。

v35 的完整 `cargo xtask check` 已通过：前端 583 项、Python 234 项、全部适用 Rust/严格 Clippy、真实数据库、契约/类型/边界/Helm 与插件检查。源码身份为 `ac16cf2db1a698897057a14fbc7d93aed6db4e9989f77e70303284b860c2f2a9`，日志 `xtask-check-v35.log`。新增测试的消息参数类型在全量前端阶段前修正，类型检查和全量 583 项均通过；产品源码未随此测试修正变化。接下来构建并核对十三个镜像，再执行完整系统验收；所有旧批次保留未完成状态。

十三个 v35 镜像现已全部构建、导入并核对源码标签及 Kubernetes containerd 实际镜像，见 `image-identity-v35.json`；源码为 `ac16cf2db1a698897057a14fbc7d93aed6db4e9989f77e70303284b860c2f2a9`。`plan9-full-v35` 的四十一项完整系统验收已启动，先准备与协议二十类，再执行四条创作链、基础六十页和恢复/权限/独立客户端/既有回归；执行期间产品与测试源码冻结。

v35 已实际通过二十类 MCP 协议场景与四十次业务执行，包含文件、会话、并行/子流程/循环等待、工作流审批、具体工具审批及拒绝后新提案、CRM 写入计数和结果文件内容；证据为 `plan9-full-v35/plan9/mcp-protocol-results.json`。四条创作链已开始；协议调用不计入所需 128 次网页执行，完整验收仍未完成。

v35 的 S08/S12/S14 三条完整创作链通过，共三份页面、六次正式网页执行，三个 Page 均首 Run；Canvas 次数为 1、1、2。S14 首 Run 因 `DESIGN_MODEL_STREAM_INVALID` 保留为结果未知，第二 Run 完成，见 `diagnostics/s14-first-provider-stream-failure.json`。S19 首 Canvas Run 完成，但原图为审批工具 Agent 配置了 Workspace，Save 正确返回 `TOOL_APPROVAL_AGENT_REQUIRED`；编辑前校验漏掉该资源策略，导致本地已应用图无法保存。原请求与 HTTP 422 记录已保留在 `diagnostics/s19-rejected-save-bd3d0d853b97.json`、`s19-save-http-errors.json`。重载后的恢复对话框又阻挡了剩余创作 Run，本轮没有伪造发布或手改图。

本轮同时出现多个新页面的 `DESIGN_MODEL_PROVIDER_REJECTED`，最小独立连接检查返回 `UPSTREAM_CONNECTION_UNAVAILABLE`（503），APIMart 公开 HTTPS 握手失败而 GitHub 正常。已停止无效请求并正常清理三个 Namespace，v35 仍为未完成，失败页不追加第四次；个别拒绝的原始上游状态未在当时账本保留，不能事后臆断，后续证据补充脱敏错误消息。11:06 UTC 公开 HTTPS 恢复，11:07 UTC 最小认证请求返回 200/OK 与真实 Usage，见 `apimart-public-recheck-v36.json`、`apimart-auth-recheck-v36.json`；诊断请求不算页面生成或业务执行。

v36 将编辑前和完整校验接入 Save 现有的只读资源准备逻辑，保持本地事务 ACK 和远端保存分列；不放宽审批与 Workspace 限制、不添加保存或执行能力。真实 MySQL/API 反例先证明旧校验返回 `issues:[]` 而 Save 拒绝；修复后 API 集成通过，受保护配置在两处均得到相同拒绝与字段路径，普通未受保护工具可校验，草稿保持不变，见 `save-policy-red-v36.log`、`save-policy-green-v36.log`。创作编排补齐正式恢复操作和实际保存错误反馈，类型检查通过；新候选仍须全库检查、镜像核对与完整 64/128。

v36 的完整 `cargo xtask check` 已通过：前端 583 项、Python 234 项、全部适用 Rust/严格 Clippy、真实 MySQL/Redis、契约/类型/边界/Helm 和插件检查，源码 `851c7607379aefc29b03252e49d0965d0a2950e1592bf8483927e183eb31860f`。首次全库检查在新增测试的动态 SQL 清理处被仓库边界拒绝，见 `xtask-check-v36-boundary-failure.log`；改为固定 SQL，并使测试策略头与不可变版本一致后，完整重跑通过，见 `xtask-check-v36.log`。11:31 UTC 再次确认 APIMart 公开 HTTPS 正常；准备构建并核对十三个镜像，再启动新候选，旧批次不晋升。

v36 的十三个镜像已全部构建、导入，并核对源码标签及 Kubernetes containerd 实际镜像，见 `image-identity-v36.json`。`plan9-full-v36` 已启动同一冻结源码的四十一项完整系统验收，仍按二十类协议、四条创作链、六十份基础页面及其余恢复/权限/独立客户端/回归顺序执行；全部 64/128 和三十八项门禁均待实际结果。

v36 已通过二十类 MCP 协议及四十次业务执行，证据为 `plan9-full-v36/plan9/mcp-protocol-results.json`，不计所需网页执行次数。S08 完整创作链通过两组留出输入：Canvas 首 Run 完成，Page 首 Run 在约五分钟后得到提供方拒绝，新的修复 Run 完成生成、检查、发布和两次业务执行。原始失败与运行账本保存在 `diagnostics/first-page-timeout-runtime.json`；同一模型的最小认证请求随后返回 200/OK，公开 HTTPS 正常，见 `diagnostics/auth-connectivity-after-timeout.json`、`public-connectivity-after-timeout.json`。这只能证明诊断时连通正常，不能据此补写该次失败的具体上游 HTTP 状态。其余三条创作链、基础六十页和全部剩余门禁仍未完成。

v36 最终通过 S08/S12/S14 三条完整链路、六次正式网页执行；四条 Canvas 均已生成并发布工作流，Canvas 次数为 1、1、2、1。S14 首 Run 的 `DESIGN_MODEL_STREAM_INVALID` 保留为结果未知，第二 Run 完成。S19 首份页面漏了下载按钮的稳定动作标记，正式任务已完成补充输入、主管审批、CRM 一次写入并产出文件，但下载未经核验，仍判失败。原 HTML 未改动，平台 Hash 为 `sha256:07563cc5b57a5c96626d9362035ae6c6521e73928c828445ab193572f458a4c9`，见 `plan9-full-v36/plan9/diagnostics/s19-download-failure-proof.json` 与 `s19-first-business-result.json`。

下载等待与按钮点击同时超时时，未处理的下载 Promise 令浏览器测试提前结束，打断 AI 修复。S19 实际已启动第二个 Page Run，完成报告仅记录首个，数据库快照按两次保留；基础 S04-r1/S08-r1 各一个 Run 随本轮中止。已正常清理三个 Namespace，完成后置脱敏，v36 保持未完成，不向旧实例追加第四次。新合成反例先复现一失败一通过，再将三处下载检查统一为动作可见后并行接住事件和点击；修复后两个浏览器反例与测试类型检查通过，见 `download-waiter-red-v37.log`、`download-waiter-green-v37.log`、`harness-validation-v37.json`。

v37 仅修正测试编排，产品源码仍为 `851c7607379aefc29b03252e49d0965d0a2950e1592bf8483927e183eb31860f`；复用 v36 的全库检查和十三个已核验镜像，并重新核对实际镜像身份。新测试源码单独冻结，准备在新临时环境执行同一四十一项完整系统验收；64/128 和三十八项门禁仍需完整实际结果。

十三个镜像实际 ID、源码标签和 Kubernetes containerd 均已重新核对，与 v36 完全相同，见 `image-identity-v37.json`。`plan9-full-v37` 已实际通过二十类 MCP 协议与四十次业务执行，见 `plan9/mcp-protocol-results.json`；四条新创作链开始运行，仍不将协议调用计入所需 128 次网页执行。页面产物审计另核对 AI 编辑命令、原始 HTML、双重 Hash、SDK/样式、不可变契约、真实 Run 启动数及任务 Bundle，下载审计核对浏览器实际保存字节与原任务 Artifact 的 SHA-256；这些只读审计不生成页面或业务请求，不补足缺失场景。

v37 四条完整创作链现已全部通过，共四份真实页面、八次正式网页执行，四个 Page 均首 Run 通过；Canvas 次数 S08=1、S12=2、S14=1、S19=1。S12 首 Run 的 `DESIGN_MODEL_STREAM_INVALID` 保留在 `diagnostics/s12-first-run-unknown.json`，新 Run 完成创作。S19 实际经过补充资料、主管工具审批、CRM 单次写入和下载，两份文件的 SHA-256/长度均与原 Task Artifact 相符，见 `diagnostics/s19-download-hash-proof.json`。四份 HTML 与实际 AI 编辑命令、发布 Hash、SDK/样式、任务 Bundle 的只读核对均无不一致；完整不可变版本与 Run 数量审计待矩阵快照。基础六十页已开始，完整 64/128、独立 AI、恢复与三十八项门禁仍未完成。

v37 最终完成基础八份页面及十六次网页执行，连同四条创作链，共十二份页面、二十四次正式执行。S08-r3 三个 Page Run 均未通过模拟检查，原始 HTML 与错误完整保留：页面轮询重建待办表单，提交按钮自身带 waitId，`closest` 取到按钮而非表单；不将此归为平台已证实的误报，也不向旧页面追加第四次。S19-r1、S11-r1 随中断保留，未通过部分不计成功。

复核 E9-G02 时确认共享设计态 Host 尚缺运行中用户指导的接口和界面，两个 Profile 都不能实际接收已有 Core 队列。因这项确定的功能缺失结束 v37，三个 Namespace 已清理，九十份 Trace 均可读并已后置脱敏；该候选保持未完成。v38 使用原 Session/Run、不可变输入 Entry 和 Core 既有 steering/follow-up 队列补齐能力，绑定原用户和编辑器，不新增创作 Run，不改变在途 Effect。运行完成与输入接收共用 Run 锁，已接收的迟到指导必须继续消费；失败、取消和结果未知均不得因此重放。API 反例、界面反例及 Core 完成边界反例先实际失败，修复后重新执行全库和真实矩阵。S08 创作链分别通过正常画布和页面界面追加展示要求，独立记录原轮次消息、实际消费及最终输出，不提供两组留出业务输入，不重置最多三个 Run 的限制。

v38 的完整 `cargo xtask check` 已通过，包含前端 584 项、Python 234 项、全部 Rust/严格 Clippy、真实数据库、契约/类型/边界/Helm 与插件检查。首次检查在新增前端测试的定位参数上发现类型错误，原失败保留为 `xtask-check-v38-first-typecheck.log`；修正后重跑完整检查，日志为 `xtask-check-v38.log`。浏览器隔离、预览、待办容器及下载等待共 29 项通过，零跳过，属于合成回归，不计 64/128。

冻结源码为 `ce79563b3e1bb1195391aa29732a51ce0b8ceabcb775913591fc608804f301cd`。十三个新镜像均已构建、导入并核对源码标签、实际镜像 ID 和 Kubernetes containerd，见 `image-identity-v38.json`。`plan9-full-v38` 已启动同一源码与独立冻结测试指纹的四十一项完整系统验收；仍需取得全部 64 份必需真实页面、128 次正式网页执行和三十八项门禁证据，不能用历史局部通过替代。

v38 在环境初始化期间主动结束，未进入任何测试函数、未生成页面，新增模型请求为零。只读 Core 反例证实“完成后追加指导”的通用恢复条件会让旧业务任务重试消费其他 Execution 的排队消息，实际模型调用数错误地从 1 增至 2；见 `completed-business-queue-red-v39.log` 和该轮 `diagnosis.json`。三个 Namespace 已正常清理，历史轮次只保留未完成，不计任何 AI 成功或失败。

v39 将继续成功终态的权力明确放入必填 Core 输入 `continueCompleted`：两个设计态 Host 固定开启，业务 Worker 固定关闭，浏览器与工作流参数不能设置。Core 完成反例及取消/失败/未知保护均通过，见 `continuation-core-v39.log`。复核同一会话的运行中队列时，真实数据库反例另证实 StatePort 会把其他 Attempt 的 `retry` 提示词一起投影给当前任务，见 `runtime-queue-ownership-red-v39.log`；修正为仅当前 Attempt 的 retry 行，保留其他任务的持久 Wakeup，并删除旧 Register 队列回退，防止消息复活。此修正仍需全库、真实矩阵与三十八项门禁验证。

队列归属数据库回归已通过。首次完整检查随后在新增测试的动态建表写法处被架构门禁拒绝，保留 `xtask-check-v39-first-boundary.log` 与当时镜像身份；尚未启动系统验收或新增模型请求。测试改用已有 `migrate_runtime_mysql`，同一业务断言和独立架构检查均通过，见 `runtime-queue-canonical-fixture-v39.log`、`boundary-green-v39.log`。

最终 v39 冻结源码为 `124fbebe1104367f00472339efab8fe3288ee94f0268cae81c0e7354afb16ffc`；完整 `cargo xtask check` 已通过，包含前端 584 项、Python 234 项、全部 Rust/严格 Clippy、真实数据库、契约/类型/边界/Helm 与插件检查。十三个镜像已按最新源码全部重建、导入并核对实际身份，见 `image-identity-v39.json`。`plan9-full-v39` 已启动四十一项完整系统验收。浏览器 SDK 与 UI 实现未变，29 项合成隔离/预览回归仍只作对应浏览器能力证据；所需 64 份真实页面、128 次正式网页执行和三十八项门禁均待本轮实际结果。

v39 在二十类 MCP 中通过十九类、三十八次正式业务执行；S18 原任务已完成拒绝、重新提案、主管批准和一次 CRM 写入，最终模型流缺少 `finish_reason`/`[DONE]`，正式任务失败，不能计成功。只读核对 CRM 确为一次写入、原模型调用已失败，证据为 `plan9-full-v39/plan9/diagnostics/s18-stream-failure.json`；不据此推断具体网络或提供方原因。协议前置门禁使四条创作链和基础六十页未启动。个人 OAuth 与 OpenCode 四类已通过，后续客户端场景随中断保留；三个 Namespace 已清理，三十二份 Trace 均可读并已脱敏。本轮仍未完成，不把局部结果补作 64/128。

v40 仅调整测试编排：严格匹配上述模型流中断，核对旧写入后最多进行两次独立业务编号的新协议复测，保留旧任务、Effect、写入与未知用量，普通业务错误、未知或不可核对写入不复测。首次成功率、复测和失败分别记录，画布/页面三个 Run 上限不变。新增编排回归及其余 Python 验收共 243 项通过，Ruff、格式和 222 项 E2E 收集通过；记录为 `python-acceptance-v40.log`、`harness-validation-v40.json`。产品源码仍为 `124fbebe1104367f00472339efab8fe3288ee94f0268cae81c0e7354afb16ffc`，复用已通过的完整全库检查；十三个 v39 镜像实际 ID、源码标签和集群镜像已重新核对，见 `image-identity-v40.json`。`plan9-full-v40` 已启动同一四十一项完整系统验收，全部必需页面、正式网页执行和三十八项门禁仍待实际结果。

v40 的新编排错误地假定所有输入都有 `request_id`，在 S01 的协议前置阶段失败；S01/S02/S03/S06 实际没有该字段。没有开始必需页面的创作或执行。个人 OAuth 和 OpenCode 的 S01/S04 已通过，其余随中断保留；三个 Namespace 已清理，六份 Trace 均可读并已脱敏。该测试错误不归为 AI 质量失败，原始诊断继续保留。

v41 将业务编号处理限定为 Schema 实际已有字段；每次复测始终使用新的正式客户端与请求身份，对话使用该客户端的新会话。新增四类实际输入形状回归，Python 验收共 247 项通过。另逐项只读核对实际二十类 Fixture、两轮八十组复测输入，确认原字段集合、非编号业务值和原始数据均保持正确，见 `protocol-matrix-shape-audit-v41.json`；这些核对不计业务执行。产品源码仍未改变，十三个已核验镜像复用并再次确认，`plan9-full-v41` 已启动完整系统矩阵，64/128 与三十八项门禁仍待实际结果。

v41 的二十类 MCP 全部首轮通过、零传输复测，共四十次正式业务执行。四条完整创作链随后全部通过，产出四份真实页面及八次网页执行：Canvas 次数 S08=1、S12=1、S14=1、S19=3；Page 次数分别为 1、2、1、1。画布和页面各自首轮成功为 3/4，整条创作链均首轮成功为 2/4；不能把三种指标混用。

S08 在正常创作界面分别追加节点名和页面标题要求，两个原 Run 各消费一次指导、没有额外 Run，并通过最终图、页面标题和正式执行核对。S12 首份页面错误地将 JSON Pointer 直接当对象键，模拟检查失败；保留原 HTML，由 AI 在第一个修复 Run 修改。额外具体诊断通过同一输入接口投递，原 Run 消费一次，实际共两次 Page Run，见 `diagnostics/s12-precise-guidance.json`、`s12-guidance-consumption-proof.json`；这是额外诊断指导，单独统计，不混作无辅助首轮成功。S19 前两个 Canvas Run 分别为模型结果未知和模型错误，第三 Run 完成；原记录与未知用量保留，未向旧轮次重放。S19 的页面、两次主管审批、CRM 各一次写入及文件下载均通过。当前开始基础六十页矩阵，所需 64/128、独立客户端、恢复及三十八项门禁仍需完整结果。

2026-11-02 00:36（本地时间）的中间结果：基础十三页与四条创作链通过，共十七份必需页面、三十四次正式网页执行。S11-r1、S19-r1 首 Run 结果未知，各在第一个修复 Run 通过；不补写未知调用的具体上游原因，未知费用继续预占。S11-r2 首份页面通过一组正式输入，第二组补充输入超时，仍判该份页面失败；只读核对原 HTML 另发现每次任务更新都会清空重建有效待办表单。具体诊断通过正式输入接口提交给第一个修复 Run，由 AI 修改，原 Run 消费一次、没有新建 Run，最终两组正式输入通过。证据为 `plan9-full-v41/plan9/diagnostics/s11-r2-precise-guidance.json`、`s11-r2-guidance-consumption-proof.json`；这一额外诊断指导单独统计，不计无辅助首轮成功。所有原始 HTML、失败与业务记录保留，当前仍未完成完整验收。

00:52 的中间结果为二十五份必需页面、五十次正式网页执行，七类优先复杂场景的三种布局均已通过。S11-r3 同样存在重建待办表单的问题，诊断追加到第一个修复 Run 后通过，实际两次 Run。S13-r3 首份页面的添加按钮没有绑定事件；第一个修复 Run 收到具体诊断后，仍因多提交空白明细而失败，最后一次允许的修复通过，实际三次 Run。两条指导均只消费一次，没有额外 Run，见 `diagnostics/s11-r3-guidance-consumption-proof.json`、`s13-r3-guidance-consumption-proof.json`。截至此时额外诊断指导共四条（S12-chain、S11-r2、S11-r3、S13-r3），与计划中的两条 S08 展示要求分别统计；不把指导后通过等同于无辅助首轮成功。S20 三种布局均首 Run 完成真实长任务、消息、断流恢复、下载与取消检查。当前进入其余基础场景；完整 64/128、该冻结版本的独立客户端、恢复和三十八项门禁仍待结果。

v41 已结束并保持未完成。最终通过基础三十页和四条创作链，共三十四份必需页面、六十八次正式网页执行；S02-r3 用尽三次 Run，S05-r2/S06-r1 随中断保留。S02-r3 原页的历史项直接调用 `getTask/renderTask`，没有使用 `openTask` 加载原页面，两组业务虽正确仍判历史恢复失败；两个修复 Run 均遇到模型请求拒绝，未追加第四次。S03-r3 两次模型拒绝后在最后一次允许的 Run 通过。后续最小指定模型请求返回 200、OK 与真实 Usage，只能证明检查时连接正常，不能反推先前拒绝的原因。二十类 MCP 均首轮通过、零传输复测；本轮个人 OAuth 与原生 OpenCode S01 通过，S04 随中断保留，不能算作八类通过。全部旧结果保留于 `plan9-full-v41/diagnosis.json`，历史索引累计三十三个关闭批次。

S02-r2 添加按钮未绑定事件，在原修复 Run 收到具体诊断后通过，实际两次 Run；S02-r3 的诊断也仅消费一次，但其修复未通过，不计成功。额外诊断指导最终六条，与计划的两条 S08 展示要求分列。三十四份通过页面的真实 AI 编辑命令、原始 HTML、不可变版本、实际 Run 数量与任务 Bundle 核对均无不一致；二十五次实际下载均与原 Task Artifact 的 SHA-256/长度一致，见 `page-provenance-audit.json`、`download-hash-audit.json`。三个 Namespace 已通过现有 `agentxctl` 清理，客户端凭据缓存和观测到的子进程均已退出；共用预算 Relay 仅为仍在进行的实现任务保留。

后置脱敏检查发现：一百一十七份 ZIP 容器均完整，但八份独立 JSON/解包资源中的数值 `fencingToken` 被误当成凭据，造成不可解析；不能把 ZIP 完整等同于内容无损。原件保留，语法修复副本只给既有脱敏标记加引号，不恢复任何被抹去的数值，见 `redaction-content-validation.json`。相关 Core 消息证据仅读取消息 ID、正文和队列归属，不使用这些数值。两个反例先复现失败，修正 JSON 标量引号并保留公开 Fencing 计数后，脱敏十项测试通过，见 `redaction-json-red-v42.log`、`redaction-json-green-v42.log`。

v42 正在补齐现有模拟检查漏掉的实际数组按钮、等待期间输入保留和历史 SDK 动作，使用同一编辑器检查函数回归原始失败 HTML，不手改旧页面、不放宽业务契约或授权。新候选仍须完整检查、新镜像身份核对和新的 64/128；v41 不因后续修改而晋升通过。

v42 的三个合成反例与三份原始 AI 失败 HTML 均先证实旧检查误通过，相关浏览器记录各为三失败、三十二通过，见 `preview-interactions-red-v42.log`、`preview-recorded-red-v42.log`。补强后，原始失败页能被同一生产检查路径拒绝，正常交互通过；加入固定长度数组、上限移除再恢复及原始空白行页面后，三十八个浏览器场景全部通过，零跳过，见 `preview-edges-green-v42.log`。这些属于模拟回归，不计新一轮的真实页面或业务执行。新增测试的类型声明已修正，前端与浏览器类型检查通过；共用检查不再依赖 JSX 组件类型。

模拟器的无 Schema 等待也由反例证实曾错误调用 JSON Schema 验证器并崩溃；修正为契约约定的任意 JSON，声明的 Schema 仍严格校验。共用预览、编辑器和 Frame 十五项单测通过，见 `preview-raw-json-red-v42.log`、`preview-state-green-v42.log`。现进入新源码的完整全库检查，尚未启动新模型矩阵或更新常驻部署。

v42 的完整 `cargo xtask check` 已通过，包含前端 587 项、Python 249 项、全部适用 Rust/严格 Clippy、真实数据库、契约/类型/边界/Helm 与插件检查。首轮误用了本机默认 Helm 4，在部署工具校验处失败，原记录保留为 `xtask-check-v42-first-helm4.log`；改用项目要求的 Helm 3 后完整重跑通过，见 `xtask-check-v42.log`。冻结源码为 `163e030d891ff768457e39b9bd1b292e425ae3428614a75c1e8b26891b84c910`；十三个新镜像正在构建，尚未宣布完整系统验收通过。

指定模型工具调用、流式与真实 Usage 两项复检通过，全部仍经同一累计 30 美元预算 Relay，见 `model-preflight-v42.log`、`plan9-model-check-v42/model-preflight/`。APIMart 公开默认有效单价重新核验为输入 USD 0.06/百万 Token、输出 USD 0.20/百万 Token，计量仍对缓存输入按完整输入价格保守计费，见 `pricing-source-v42.json`。这些预检不计真实页面和正式业务执行。

v42 的十三个新镜像已全部构建、导入，并核对源码标签、实际镜像 ID 与 Kubernetes containerd，见 `image-identity-v42.json`。`plan9-full-v42` 已在新临时环境启动同一四十一项完整系统验收，产品源码与测试指纹分别冻结。二十类工作流已经准备并发布，正在执行 MCP 协议前置门禁；必需 64 份真实页面、128 次正式网页执行、独立 AI 客户端与三十八项门禁尚待本轮结果，历史局部通过不替代本轮验收。

v42 的二十类 MCP 协议全部通过，共四十次正式业务执行，见 `plan9-full-v42/plan9/mcp-protocol-results.json`；这些不计入所需网页执行次数。现已进入四条 AI 工作流与页面的完整创作链，完整 64/128 与三十八项门禁仍未完成。

v42 的 S08、S12、S14 三条完整创作链均通过，Canvas 和 Page 各只启动一个 Run，完成六次正式网页执行。S19 的三个 Canvas Run 均为 `outcome_unknown`，没有完成工作流发布，该链路判为未通过；不向该逻辑链路追加第四个 Run，不手写图或 HTML。具体模型调用与终态保存在 `diagnostics/s19-canvas-three-unknown-runs.json`，未知费用继续预占，不补写无法核实的上游原因。其余六十份基础页面、独立客户端与恢复回归继续执行，整体验收保持未完成。

02:48（本地时间）的中间结果为基础十六页及三条创作链通过，共十九份必需页面、三十八次正式网页执行。其中十七份首 Run 通过，S08-r1 和 S11-r2 各在第一个修复 Run 通过。S08-r1 原页的待办表单被隐藏样式遮住，由自动修复提示交给 AI 修改；S11-r2 将 JSON Pointer 直接作为对象键，具体诊断通过原修复 Run 的输入接口追加，消费一次、没有新增 Run，见 `diagnostics/s11-r2-precise-guidance.json` 和 `s11-r2-guidance-consumption-proof.json`。这条额外诊断单独统计，不混作无辅助首轮成功。S08 计划内两条展示指导也已核对原 Core 状态，均为一个 Run、一次消费且队列无残留，见 `diagnostics/planned-s08-guidance-consumption-proof.json`。原始失败 HTML 和业务记录均保留，整体未完成状态不变。

03:08 的中间结果为三十四份必需页面、六十八次正式网页执行。S11-r3 的两组业务正确，但历史刷新按钮藏在任务区域，首轮历史恢复失败；AI 在第一个修复 Run 调整后通过，未追加具体人工指导。S06-r1 首页及第一个修复页均在刷新后丢失会话关联，后续消息创建了新会话；第一个修复只在发送成功回调中保存会话，并未覆盖恢复入口。具体诊断追加到最后允许的修复 Run，消息消费一次，但该 Run 因重复提交未登记的 `oldText_hash` 编辑参数而失败。该页用尽三个 Run，仍判失败，不追加第四次；见 `diagnostics/s06-r1-three-run-failure.json`、`s06-r1-guidance-consumption-proof.json`。额外诊断指导累计两条，其中一条通过、一条未通过，与 S08 两条计划内展示要求分别统计。后续场景继续，整体未完成。

S06-r2 首页使用普通 `invoke` 提交，未走正式会话消息接口；第一修复 Run 收到具体会话初始化及刷新恢复诊断后通过两组真实对话，消费一次，见 `diagnostics/s06-r2-guidance-consumption-proof.json`。额外诊断指导累计三条，其中两条通过、一条未通过。

S06-r3 原页缺少 `send-message` 动作标记，首轮业务检查失败；随后测试在点击前创建的 `waitForResponse` Promise 超时未收口，打断了刚启动的第二个修复 Run。已关闭 Trace 的根错误为该未处理等待；数据库实际启动两个 Run，结果行只记录一个，不按“三次 AI 修复失败”统计。证据为 `diagnostics/s06-r3-harness-interruption.json`；保存后通过正式取消接口停止孤立的第二 Run，未启动新 Run。其余矩阵继续，产品与测试源码保持冻结，待本轮结束后修正此编排问题。

03:36 的中间结果为基础四十页及三条创作链通过，共四十三份必需页面、八十六次正式网页执行。S05-r2 在最后一次允许的修复通过；其首 Run 的具体模型 Effect 日志确认收到 HTTP 502，见 `diagnostics/s05-r2-provider-rejection-log.json`，不能据此推断其他请求原因。S05-r3 的上传完成前提交问题在第一次修复通过，S07-r3 也在第一次修复通过。

S09-r1 首 Run 的结构化等待错误地把 JSON Pointer 直接当对象键，具体诊断追加到第一个修复 Run，消费一次；该 Run 最终结果未知，独立的最后一个 Run 完成生成、检查和发布。随后测试元数据查询仍使用十五分钟前的初始 JWT，返回 `INVALID_TOKEN`，未开始两组正式业务执行，不能计通过。关闭 Trace 确认浏览器续期接口返回 200，而测试查询携带已过期凭据；记录仅保留 exp/iat、路径、状态和时间，不保存原 Token，见 `diagnostics/s09-r1-auth-evidence-reader.json`。此项按验收读取凭据中断单列，不改服务端登录有效期，不把最终生成 Run 计成模型失败。额外诊断累计四条，均消费一次；对应两份页面通过、一份失败、一份因编排中断未完成，见各 `guidance-consumption-proof.json`。整体仍未完成。

03:42 临时 Control 容器在 512 MiB 上限发生 `OOMKilled` 并自动重启；S14-r3、S15-r3 均已完成两组业务，在历史恢复时收到网关 502。两项实际各一个生成 Run，不按 AI 修复用尽统计；后续等待新 Run 响应的测试 Promise 超时也保留，见 `diagnostics/approval-history-oom-impact.json`。03:46 仅对本轮临时 Control Pod 原位提高内存上限到 1 GiB，确认实际 cgroup 已应用、Pod UID 与重启数未变化，见 `diagnostics/control-memory-resize.json`。当时节点仍有约 14 GB 可用内存，不是通过缩停常驻部署缓解；该观察只证明容器触及上限，不确定为内存泄漏。后续内存观测单独记录。

基础六十页矩阵已全部结束：五十五页通过，其中四十六页首 Run 通过；连同三条完整创作链，共五十八份必需页面、一百一十六次合格网页执行。五个基础未完成项为 S06-r1、S06-r3、S09-r1、S14-r3、S15-r3，另有 S19 创作链未通过。五十八份通过页的原始 AI HTML、不可变版本、SDK、实际 Run 数量及正式任务绑定核对无不一致，见 `page-provenance-audit.json`；该审计仍为 incomplete，缺少的六项没有被补作成功。生成期安全冻结参数另存 `generation-model-freezes.json`，共六百五十四次设计态调用、八十五个 Run，指定模型和实际输出上限 16000 均已核对。

个人 OAuth 登录、同意授权、刷新与撤销通过。独立 OpenCode 1.1.56 的八类自然语言任务随后全部通过，见 `opencode-results.json`，包含正式调用、真实等待、主管 UI、写入和文件核对；不计入所需网页执行次数。当前继续原生客户端个人 OAuth、权限、恢复和既有回归，整体未完成。

个人原生 OAuth、任务/文件归属、Worker 重启、写入响应丢失、策略撤销、审批竞争及取消/迟到决定均已通过。个人 MCP 与网页跨入口失败，原任务在等待节点得到 `WAIT_INPUT_EXHAUSTED`：未提供 `waitInputs` 被请求默认值和调用者类型判断组合成空预设回复集。只读数据库核对显示个人调用为 preset、服务调用为 external，见 `diagnostics/personal-mcp-wait-mode.json`。这属于请求边界缺陷；需明确区分省略与显式预设，保持原等待状态机和授权。

四种桌面语言/主题及真实认证失败后的刷新恢复通过；截图已人工查看。账号切换子项确实销毁旧 Frame、主管页面无业务 iframe，任务读取返回 401，而测试只接受 403/404，故组合门禁仍失败，见 `diagnostics/appearance-account-switch-status.json`、`visual-review.json`。不得将该测试断言问题写成已观察到的跨账号泄露。

页面升级恢复测试另暴露 Bundle 激活错误依赖服务 Key：S08 新版本的租户、路由、运行身份均有效，无资源或 Grant 前置条件，但四个历史测试 Key 已正常撤销，激活因此被拒绝。证据为 `diagnostics/lifecycle-publication-prerequisites.json`；后续 Control 停机与 Worker 故障阶段未执行。应将服务 Key 保持为独立入口凭据，在实际调用时检查撤销，不应阻断网页登录应用的新版本发布或回退，旧 Key 不得因修复重新生效。

目录测试同时发现管理员详情使用准备中后缀，列表和 Runtime 仍使用已激活页面；准备失败阶段尚未继续。需统一详情与现有目录查询的已激活版本来源，不能让未完成发布提前替换访问地址。本轮继续既有画布、评测和文件回归，源码与测试保持冻结，以上失败不会被后续修复改写。

既有节点 Inspector 组共三十个浏览器场景已结束：十五类真实指定模型创作与节点编辑全部通过；十五类 Fixture 均在旧间距断言处失败，尚未执行各自后续断言。现行边界节点宽 184、层间距 96，实际横向位置差 280；测试仍按宽 240、净间距至少 159 判断，见 `diagnostics/canvas-inspector-geometry.json`。修复测试应遵循当前设计令牌，不能改产品间距以迎合历史数值。

后续修复已在忽略目录暂存，尚未应用到本轮源码或测试。包含个人等待模式、发布与 Key 解耦、目录版本来源、页面模拟会话恢复和有效可见性、模型流安全诊断及验收编排修复。五份原始失败 HTML 的 Hash 全部与当时尝试记录相符；暂存补丁仅完成 Rust 语法格式和十个前端模块的语法检查，不能据此宣称类型、功能或完整验收通过。待当前四十一项系统函数结束、证据核对及清理后再应用、验证和冻结新的候选。

暂存候选的实际 TypeScript 编译器覆盖检查已通过前端与浏览器两个项目，冻结文件未被替换；暂存 Python 按原仓库路径的 Ruff 与格式检查也已通过。这些仍只是暂存静态检查，Rust 类型、实际功能及完整全库检查均待应用后执行，不能提前计入本轮。

既有画布桌面完整创作通过，随后矩阵正式 JUnit 为二十项中十七项通过、三项失败。Trace 内轮询期间的排队状态不是最终故障证据；通过项保持通过。上下文压缩失败另经只读 Core 状态确认：四个长提示中第二、第四个仅保留了 2048 字符，测试在上一 Run 的指导输入框仍生效时填入下一 Run，尚未达到压缩阈值，不是已证实的压缩算法失败。新 Run 操作须先等待界面解除指导长度限制，并核对实际填入内容；历史条目也按既有分页接口完整读取。证据为 `diagnostics/canvas-matrix-runs.json`、`compaction-state.json`，尚未复测。

三百节点失败停在原编辑命令尚未 ACK，实际模型响应均成功；一百五十节点的布局编辑约五十七秒，三百节点在三分钟测试期限被正式清理取消。原始命令已保存用于隔离性能复现，不能将其归为模型创作失败或用延长测试时间直接宣布通过。网关中断测试则在冻结十分钟截止时间之前等待结果，原 Run 到期取消、原 Model Effect 保持 `outcome_unknown` 且未重放；下一候选须分别核对 Run 与 Effect，保留真实截止时间和未知语义。当前完整系统验收仍继续，旧失败不回填成功。

v42 已完整结束，四十一项系统函数为三十一通过、十项失败；三个临时 Namespace、所属浏览器/转发进程、OpenCode 配置和凭据缓存均已清理，sandbox Pod 无残留，常驻部署未升级。冻结源码与测试指纹审计通过，原结果保存在 `final-cohort-audit-frozen.json`，完整候选仍为 incomplete。六十份已通过页面产物审计中包含五十八份必需页面和两份额外恢复页面，不能称为六十/六十四；二十八次实际下载的字节与原 Artifact 均一致。

ZIP/JSONL 后置核对发现另一条脱敏缺陷：URL userinfo 正则会跨越紧凑 JSON 的引号，将后续 `page@...` 或 `locator@...` 当成认证信息。四百三十八份 ZIP 容器 CRC 均通过，但其中共三千二百四十行 typed JSONL 已损坏；独立 JSON 和被识别的 JSON 响应仍可解析。原件保留，不重建已经丢失的字段，不再把“ZIP 可读”表述为“完整 Trace 可用”。三个精确反例先失败，限制 URL 字符边界后全部十四项脱敏回归通过，见 `redaction-trace-red-v43.log`、`redaction-trace-green-v43.log`。

v43 已在 v42 封存后应用暂存补丁。五份原始 AI 失败 HTML 使用同一生产预览路径回归，新增会话、可见性、隐式提交及响应等待检查；第一次合成上传测试另暴露 Playwright JSON 绑定不能直接传递 ArrayBuffer，修正测试桥接后四十九项浏览器回归全部通过、零跳过，见 `preview-chat-buffer-green-v43.log`。它们仍是模拟检查和原件回归，不计新一轮真实页面或正式业务执行。

个人等待模式的省略/显式空预设契约测试通过。未知来源保护的真实 Control/MySQL API 反例先显示 cancelled Run 的模型删除影响为空、错误地允许删除；修复后模型/凭据影响、工作流及模型删除拒绝、未知历史保留与已知回执后的正常 Retention 均通过，见 `unresolved-model-reference-red-v43.log`、`unresolved-model-reference-green-v43.log`。模型结果与费用仍以 Runtime 为准。

三百节点的原始命令及 ELK 输入均未修改。隔离 CPU 取样与选项组合核对证实原强制顺序及放置参数组合耗时过长；保留固定端口和排序后的原图、采用已有 ELK 默认放置策略后，同一输入约二点三秒完成。一百五十/三百节点的业务定义和连接未被简化。九项布局单测包含新增三百分支共享 Exit 的耗时、确定性和业务内容不变检查，全部通过，见 `layout-option-groups-benchmark-v42.json`、`canvas-layout-v43.log`。实际画布 E2E 和完整新候选仍须重跑。

网关契约与前端类型已由既有生成器重建，实际前端及浏览器类型检查通过，Python 静态与格式检查通过。新的完整检查已启动；下一候选收集为五十项系统函数，比原四十一项增加九项受等待 DTO 变化影响的既有交互回归，并把独立画布/应用回归放在真实六十四页矩阵之前。尚未启动该候选的集群验收或声明任何新真实页面通过。

v43 的完整 `cargo xtask check` 已通过：前端五百八十八项、Python 二百五十三项，以及全部适用 Rust/严格 Clippy、真实数据库、契约/类型/边界/Helm 与插件检查，见 `xtask-check-v43.log`。冻结源码为 `b755e5aaf59fc873ba3deb520c57b2f34cc96e623c99f3ab89f1b4af31917bae`；五项指定模型真实预检通过，累计预算继续沿用原账本。十三个镜像进入构建与实际身份核对，随后执行已收集的五十项系统函数；所需六十四份新真实页面、一百二十八次网页执行和三十八项门禁均尚未通过。

v43 的十三个镜像已完成构建、导入及源码标签/实际镜像 ID/集群镜像核对，见 `image-identity-v43.json`。`plan9-full-v43` 已收集并启动五十项系统函数，按既有画布、应用和九项输入交互回归优先执行。源码与测试指纹均冻结；新的六十四页、一百二十八次执行与三十八项门禁仍待实际结果，旧页面不计入本轮。

v43 在前置 Inspector 组被主动终止，尚未开始六十四页矩阵，不能计入任何必需页面或完整系统函数通过。首个执行查询发生在初始化用户权限同步完成之前；两个真实 Outbox 事件均第七次投递成功，约一分钟后 Runtime 投影有效，见 `baseline-query-admission-diagnosis.json`。初始化逻辑已正确发送事件，不能据此称为管理员权限未实现。首个真实模型 Inspector Run 的三个 Model Effect 已成功返回，旧测试却在一百八十秒时取消仍有进展的 Run，而正式冻结期限为十分钟，见 `baseline-model-diagnosis.json`。原失败与中断保留，不转写成功；四份 ZIP 及一万一千六百六十五行 typed JSONL 完整性检查通过，三个临时 Namespace、所属进程和凭据缓存已清理。

下一验收轮 v44 沿用 v43 冻结的产品源码和十三个镜像，只修正验收准备和证据范围：等待真实用户权限投影后断言正式委托查询返回 200；每个画布 Run 按正式十分钟截止时间及回执收口等待，不改变产品期限、审批语义或三个 Run 限额；长测试读取跟随浏览器实际续期凭据，显式传入的其他调用者仍保留。费用报告按本轮冻结模型和受控计量入口的资源身份筛选，确定性 Fixture 的 Token 单列排除，原始 Runtime 账本完整保留。全局账本持续累计，v43 关闭时已知费用 3.33810536 美元、未知用量预占 0.24488496 美元，未重置额度。当前仍为 incomplete，完整五十项系统函数及三十八项门禁待重新验收。

v44 的完整检查已通过：前端五百八十八项、Python 二百五十四项和适用后端、数据库、契约、类型、部署与插件检查。首次检查因本机默认 Helm 4 与仓库要求不一致停止，原日志保留为 `xtask-check-v44-environment-failure.log`；显式使用 Helm 3 后完整通过，见 `xtask-check-v44.log`。五项真实指定模型预检通过。十三个镜像的 ID 与 v43 完全相同，源码标签、文件系统层及集群 containerd 中的实际引用均重新核对，见 `image-identity-v44.json`；产品源码 Hash 未变化。`plan9-full-v44` 已收集并启动完整五十项系统函数，测试指纹重新冻结，所有三十八项门禁保持待审计。

v44 初始化等待已确认真实 Runtime 用户投影，正式执行查询返回 200，见 `studio-admin-admission.json`。节点 Inspector 组正式 JUnit 为三十项中二十八项通过、一项 error、一项 failure，零跳过：确定性样例十四/十五、真实指定模型十四/十五，见 `diagnostics/canvas-inspector-results.json`。代码节点样例的正常“草稿已保存”通知悬停在发送按钮上，点击和未共同收口的响应等待均超时；网络证据确认只有第一轮已提交，第二轮未提交，见 `canvas-send-toast.json`。通过真实关闭按钮处理通知并共同等待点击/响应的修正仅暂存，实际 TypeScript 编译器检查通过，尚未应用到冻结测试。

循环节点的真实模型第三次调用返回不完整的工具参数，Runtime 安全诊断为 `model stream has incomplete tool arguments`，原 Model Effect 与 Run 保持 `outcome_unknown`；前两次模型调用成功，没有据此重放原调用或将该场景计通过，见 `canvas-loop-model-unknown.json`。节点组结束时生产与测试指纹均未变化，两份原始失败 Trace 在整组脱敏后记录 Hash，见 `checkpoints/inspector-end-freeze.json`。当前继续其余四十九项系统函数，新的六十四页、一百二十八次正式网页执行仍待实际结果。

后续画布布局/恢复组六项与桌面真实创作两项全部通过；二十项矩阵十八项通过，零跳过。Pin/Mock 子项在四种调试及版本断言通过后，被节点双击的第二下打开条件下拉框阻挡，见 `canvas-pin-mock-click.json`；三百节点的编辑与保存已确认，测试模型却未读取大型验证报告的 Artifact 引用，连续验证二十九次后正确触发轮次预算，见 `canvas-large-validation-artifact.json`。两项均保留失败，相关测试修正只暂存；三百节点后续流畅度、撤销重做等尚未通过。两次网关中断均按原十分钟期限取消原 Run，原 Model Effect 保持未知且没有新增调用；对应冻结期限证明保存在 `studio-builder/gateway-unknown-deadline-*.json`。

八项画布资源/离线/保留/删除/历史/隔离回归和七项应用回归通过；九项人工输入回归七项通过。两项原测试用服务 Key 读取个人或渠道任务而得到正确的 404：预设业务任务实际完成，渠道任务实际等待，现有管理员正式接口亦返回对应记录。读取身份修正已暂存，保留越权拒绝断言，不放宽生产授权；见 `interaction-observer-ownership.json`。原系统函数仍失败，静态检查不替代复测。

二十类正式工作流已发布，二十类 MCP 发现/Schema/执行/等待/审批/结果校验均首轮通过，共四十次业务执行，传输复测为零。四条画布 AI→页面 AI→正式执行闭环随后全部通过，四个工作流均一个 Run，页面 Run 数为 S08 两个、S12/S14/S19 各一个，共四份必需页面、八次合格网页执行。S08 首次等待界面未通过正式操作，AI 修复后通过；原 HTML 和失败记录保留，未手改页面。S08 的中文浅色桌面完成态已实际查看，见 `s08-visual-review.json`；其他主题/语言及完整六十页仍待验收。当前已启动基础六十页矩阵，整体继续为 incomplete。

v44 的三百节点测试模型修正已在忽略目录中的同依赖源码副本上实际编译验证：原实现一项通过、三项失败，提案四项全部通过，零忽略；见 `pending-v44-harness/isolated-fixture-proof.json`。冻结的正式测试与测试服务源码均未替换，仍须关闭原批次后应用、完整检查和受影响系统复测。补充复测计划为节点 Inspector 全组三十项、画布矩阵全组二十项及九项人工输入回归，不能将旧 JUnit 的失败改写为成功。

v44 页面矩阵继续运行。截至中途快照，十九份合格页面完成三十八次正式网页执行；基础十五份均首 Run 通过，四条创作链页面三份首 Run、一份修复后通过。文件分析、结构化交付、子流程资料、并行待办及资料后工具审批的三种界面均已实际执行。中途质量快照尤其区分两个口径：当时十八份合格页面中十七份首个完整 Run 通过，但仅五份首次模拟检查通过，首 Run 内三十六次工具错误保留，不能把内部自我修正算作首版零错误。见 `checkpoints/partial-preview-quality.json`。十九份样本的首个创作 Run 中位一百六十七点五五四秒、整页验证中位一百九十四点七一九秒，见 `checkpoints/partial-page-timing.json`；均为中途指标，最终须按完整结果重新计算。

S08 创作链首次失败已只读定位为 AI 原始代码的表单缓存键不一致，同一 waitId 的多次任务更新生成两个独立待办表单，并非测试选择器误报；AI 在第二个允许 Run 内修复后通过，见 `diagnostics/s08-chain-duplicate-wait-form.json`。S04 文件报告与 S19 工具审批、员工完成页的实际中文浅色截图已查看；审批截图在领取后、决定前保存，批准结果来自正式 UI/API 回执与写入计数，不能从截图文件名推断已批准。见 `s04-visual-review.json`、`s19-visual-review.json`。其余桌面主题/语言、完整六十四页和最终三十八项门禁仍保持待验收。

S08 的运行中追加指导已进一步用实际 Runtime 请求核对：画布一条、页面两条（原始 Run 与修复 Run 各一条标题要求），三条均以原 Core 消息标识、user 角色和完整文本进入对应原 Run 的模型上下文，没有以新增 Run 冒充即时指导；见 `same-run-guidance-audit.json`。S01-r1 已通过额外的自然语言标题/两列布局修改并完成两组正式输入，变更前后 Hash 不同且草稿/会话相同，见 `s01-natural-language-modification-proof.json`。该明确要求的调整与失败修复分开记录。

S13-r3 首个 Run 用时五百点一一四秒，Run 正常结束但预览未通过；模型明确承认两项表单保留问题，发布门禁没有放行，不把 completed 等同于页面可用。原始代码在外层及内层表单重复标记 waitId，却比较全部 DOM 标记与单份待办集合，状态更新因此反复重建活动表单；实际检查在提交前检测到旧输入节点脱离，见 `diagnostics/s13-r3-first-run-form-preservation.json`。当前使用剩余允许轮次由 AI 修复，未人工改 HTML、未放宽检查；整个逻辑页面仍未判通过或最终失败。此时累计二十六份合格页面、五十二次正式网页执行，完整验收继续为 incomplete。

S13-r3 的最终未通过项随后被证实为检查器缺陷：原始 HTML 的外层 FORM 和实际提交 BUTTON 使用同一动作标记，旧检查器点击第一个 FORM，未触发真实提交。未修改 HTML 的隔离浏览器中，点击原始按钮即可提交并完成合成任务；仅替换暂存的 SDK 检查实现后，同一份 HTML 通过完整模拟预览。二十项实际浏览器检查全部通过，零跳过，包含原生表单、原生约束、嵌套动作及独立表单歧义；均无模型请求和正式业务执行。原失败、三次 Run、字节 Hash、对照 Trace 和当前源码/测试指纹证明保存在 `diagnostics/s13-native-form-confirmed.json` 及 `diagnostics/native-form-probe/`。正式产品源码仍保持冻结，修正尚未应用，不能据此计为合格页面。

后续按既定“检查器修复后复验原 HTML”的规则收口：先关闭并审计原 v44，再应用检查器及测试驱动修正，完整检查和构建镜像；使用原始 AI HTML 的完全相同字节，经正式草稿/检查/发布、两组留出业务输入及历史页面恢复复验，不新增第四个创作 Run。原生成身份、旧失败与新的交付绑定分别记录。修正只涉及私有预览检查的动作激活，不更改公共 SDK 方法、认证、业务请求、Schema 或工作流逻辑；最终必须以精确源码差异和受影响回归证明这一点，不能据此推定门禁通过或重新随机生成六十四页。

02:00 UTC 中途结果为六十一份合格必需页面、一百二十二次网页执行，剩余 S18 两种布局及 S13 原 HTML 正式复验。S14-r3 首次模型调用实际收到 HTTP 503，第二个允许 Run 完成并通过两组正式输入与历史恢复；不从首次成功率中排除该失败，见 `diagnostics/s14-r3-original-model-rejection.json`。S06/S13/S20 的部分真实截图已人工查看，见 `selected-chat-loop-report-visual-review.json`。其中名为 cancelled 的 S20 截图实际显示“结果未知”：TaskView 为 outcome_unknown、取消请求本身已受理、取消尚未确认，报告不能将其称为已确认取消，也未重放原业务操作。

原 HTML 复验的正式 Python/Playwright 用例、当前 SDK 隔离回归、十三镜像构建与受影响系统回归的编排均已暂存；静态检查通过，冻结的正式源码和测试仍未替换。复验会保存实际不可变页面/发布记录、精确工作流定义、两组输入与反复等待、旧 waitId 拒绝、历史页面恢复且无新增业务请求，并断言零新页面创作 Run/模型调用。独立合并审计保留原失败，只能以这些实际结果和精确私有检查器差异关闭门禁，当前未据此判为通过。

基础矩阵已正式收口为五十九/六十通过，加四条创作链共六十三份合格必需页面、一百二十六次网页执行；唯一原页面未通过项为 S13-r3 检查器误判。原始质量报告 `generation-quality-original.json` 同时记录首个完整创作 Run 五十六/六十四通过、首次模拟检查三十/六十二通过、两份未到达检查、全部一百四十三次内部工具错误及首次失败。首次 Run 报告耗时中位一百六十四点六三六五秒，整页含正式业务中位一百九十五点二〇五秒。原始六十份独立结果已与组装报告逐一核对；生成期实际工作流/页面/发布/Run/调用绑定已只读保存于 `generation-snapshot/`。

个人 OAuth 登录、同意、刷新和撤销，以及独立 OpenCode 1.1.56 八类自然语言任务和客户端自身 OAuth 缓存/撤销均通过。人工复核区分三十四次原生客户端工具调用事件与 Python 观察者轮询请求，并核对远程完成回执、真实 Runtime 执行/主体/Bundle；文件内容另经正式授权下载通道核对，见 `external-ai-acceptance-review.json`。不能把观察者请求数称为 AI 自主工具调用数，也不能将取得文件引用表述为无限制本地文件访问。

审批重启、写入响应丢失、策略撤销和取消/超时后的迟到决定通过；并发决定用例则在产生提案前遇到已知业务模型连接 HTTP 503，原任务失败、零 Tool Effect，未执行并发断言，见 `diagnostics/tool-race-model-connection-failure.json`。因此后续受影响验收扩为十三项系统函数，额外重跑整条并发决定用例，并准备 S13/S17 两个正式场景；不因此修改产品审批逻辑或增加页面创作。原失败继续保留，尚不能关闭并发审批门禁。

v44 最终五十项系统函数全部执行，四十一通过、九项失败。页面权限边界的十六项真实拒绝全部满足，末尾全 Namespace 计数却混入其他回归合法产生的四次管理员调用和二十五次调试执行；实际二十四个 AI 页面应用及其 Bundle 范围内，两项计数均为零，见 `diagnostics/authoring-boundary-counter-scope.json`。生命周期用例已完成业务升级、页面回退和 Worker 更换，停止 Control 后实际 HTTP 请求超时，而旧断言只接受 5xx，故后续业务恢复未执行。目录 API 已返回正确激活地址，浏览器却只在五行一页的第一页寻找目标；应使用既有搜索框，见 `diagnostics/lifecycle-and-catalog-observers.json`。

原批次源码与测试指纹均在修改前核对一致，失败审计保存为 `final-cohort-audit-frozen.json`；三个临时 Namespace、所属进程、OpenCode 缓存、sandbox Pod 和私有批次 values 已清理。六十五份已通过页面产物（六十三份必需页面及两份恢复页）源码/不可变版本/SDK/任务绑定一致，三十次下载字节全部匹配。完整 typed 证据核对另发现十六行 Trace JSONL 损坏，含报告副本，均位于既有 Inspector 和输入交互回归组；必需真实生成页、创作链及其业务 Trace 未受影响。根因为代码样例中 Authorization 头的脱敏吞掉了 JSON 结束引号前的转义符；原件保留，不修补丢失记录。实际反例在旧实现下十四通过、三项失败，最小转义修正后十七项全部通过，见 `redaction-quote-{red,green}-v44.log`。

后续复验最终确定为十六项系统函数：完整 Inspector 三十项、画布矩阵二十项、九项输入交互，以及并发工具决定、原 S13 HTML 正式执行、全部十六项页面边界操作、完整生命周期和目录三阶段。准备 S01/S08/S13/S17 四个正式场景，额外原 HTML 仅机械交付为这些回归准备版本，不计新生成或补充业务次数；所有页面创作 Run 必须为零。原失败与损坏记录保持原状态，合并审计只能用完整实际复测替换对应证据组。

修正已在原批次封存后应用。当前产品源码为 `5f07b61f364caf9a5810b20cf3f0527dbeccdc234241669587647643cfc1f22e`；仅把原 SDK 文件的两个私有检查动作改动在内存中替换回原字节，即精确重现原 `b755…` Hash，其他产品文件全部一致，见 `native-form-source-impact.json`。正式 `pytest tests/e2e` 隔离回归五十四项全部通过、零跳过，包含九份原始失败 AI HTML 和未修改的 S13 原件；二百一十六份 ZIP、三万八千四百五十二行 typed JSONL 均有效。当前仍在完整仓库检查，十三镜像构建和十六项正式复验尚未通过。

完整当前源码的 `cargo xtask check` 随后通过，十三镜像已构建、导入并核对源码身份，`plan9-revalidation-v45` 的十六项受影响复验已启动。完整 Inspector 三十项实际执行为二十九通过、一项失败；原 Code 和 Loop 用例通过，新增失败是 `knowledge_retrieval` 的真实模型流不完整。Run `a19b920c-9eca-4449-a8ab-ea846ac0d883` 前两次模型调用成功，第三次返回 `DESIGN_MODEL_STREAM_INVALID`，明确缺少 `finish_reason` 与 `[DONE]`，原 Run 保留 `outcome_unknown`，未知 Usage 继续预占。实际数据库证据见新批次 `plan9/diagnostics/knowledge-inspector-model-unknown.json`，不是把页面或工作流静态错误归为网络问题。

已准备在当前轮次全部结束和清理后，使用完全相同的产品/测试指纹及同一组十三镜像，只通过既有 `AGENTX_E2E_PLAYWRIGHT_GREP` 单独执行未修改的知识检索真实模型测试。它会明确创建一次新的独立回归 Run，不重放未知 Effect；原三十项报告和失败保留。只有其余二十九项实际通过、精确失败子用例实际复测通过且源码/测试完全一致时，才能合并该三十项覆盖；不得把原失败的系统函数或 JUnit 改为绿色。这不增加必需页面创作，不对 S13 追加第四次 Run，也不重新生成六十四份页面。其余复验尚未全部结束，当前仍未完成整体验收。


## 最终闭环（2026-11-02）

`plan9-revalidation-v45` 全部十六项系统函数执行完毕，十五通过、一项失败。画布综合二十项、全部九项输入交互、实际并发审批、原 S13 页面两组正式执行、十六项创作边界操作、完整版本/Control 离线/Worker 恢复及目录三阶段均通过。S13 原始 HTML SHA256 保持 `81397511f261410822e187f1ce73e14c912e597c6d55b69173c3f6e786949a30`，三个原始创作 Run 未变，新网页创作 Run 与模型调用均为零；历史页面打开没有正式业务修改请求。原始六十三页、一百二十六次合格执行因此补足为六十四页、一百二十八次，历史种子及额外准备页面不计入。

唯一剩余 Inspector 知识检索流不完整用例由 `plan9-inspector-retry-v46` 在完全相同的源码、测试指纹和重新核对的十三个镜像上，使用既有过滤参数执行同一个未修改的完整测试，实际一项通过。原三十项保留二十九通过、一项失败，专项再执行一次并通过，三十个不同用例的覆盖审计十项检查全通过；失败 JUnit、未知 Run/Effect 和费用预占没有被改写或重放。

所有新证据写入结束后按类型校验，归档副本逐文件 Hash 一致。归档后的 v45 为二百三十六份 ZIP、六十一份独立 JSON、六十一万九千五百七十二行归档 JSONL，v46 为四份 ZIP、一万一千八百二十八行 JSONL，均无格式错误；原十六条损坏记录及其副本照原字节保留，其对应回归由完整新组的有效证据替换。逐项三十八门禁评审通过，严格合并审计三十二项全部通过。

最终累计已结算 USD 4.19387750，未知 Usage 预占 USD 0.25618210，合计 USD 4.45005960，保留四十一个未知用量请求；同一总预算从未重置。原必需网页创作及计划内修改已报告输入 7,963,057 Token、输出 545,027 Token，Runtime 逐次舍入费用 USD 0.586804，缺失 Usage 不当作实际零费用。

各临时 Namespace、独立 OpenCode 缓存及测试进程已清理；最后停止共享计量服务并确认端口关闭，删除二十三份所属测试配置及两处私有缓存，四十二份非凭据探针文件移入证据目录。未升级常驻部署，未改变既有开发副本。最终来源、质量、原始失败、复测、预算和三十八项证据见 [完整验收报告](evidence/2026-11-02-acceptance.md)。

## 用户要求的后续本地交付（2026-11-02）

完成本地常驻部署重新构建与升级，保留原账号及 223 张表的既有数据，并发布“费用报销服务台”的真实工作流、AI 生成网页与 MCP。网页补充资料、批准和拒绝两条分支，以及独立 MCP 客户端的原任务续办均实际通过。联调修复输入恢复后下游审批读取顺序和首次页面挂载空白，产品源码冻结为 `b8dd336e3b5aeed08302ca9f971a1d06349d1a8244d3300be50cad169eea9fe8`；完整检查、56 项隔离浏览器回归、4 项本地系统验收及历史结果专项通过。原失败保留，管理员临时登录文件已清理；示例及本机 MCP 客户端配置有意保留。具体入口、使用步骤、验证边界和证据见 [本地升级交付说明](evidence/2026-11-02-local-demo.md)，不改写前述临时环境验收范围。

后续按用户反馈改为单一工作 HTML、持续聊天、首次弹框设置地址、再次直接发布和只回滚线上的发布历史。真实指定模型连续修改、发布、历史预览、回滚及关窗重开通过；高度/滚动排版与重复挂载修正通过两种桌面尺寸的检查。用户确认共享工作区的 MCP 变更需要协调后统一升级，两对话随后完成源码冻结、11 个镜像统一构建、239 张既有表共 32,475 行逐行核对，以及四个部署域升级和 5 项常驻系统验收。原报销入口、聊天、历史任务、服务 Key 和 OAuth 登记保留；临时源码预览和登录资料已清理。详见[持续编辑交付记录](evidence/2026-11-02-continuous-page-editor.md)、[MCP 发布验收](evidence/2026-11-02-mcp-publications.md)与[合并升级记录](evidence/2026-11-02-combined-upgrade.md)。
