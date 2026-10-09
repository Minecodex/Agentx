# plan7 第三批自动化专项验收（2026-10-09）

按用户授权，继续完成现有资源足够的七类自动测试。33 个不同 pytest 场景均有通过证据，包含 13 个桌面 Playwright 用例；这是专项复测的覆盖汇总，不能作为九域同 Run 或两小时生产认证。首两批真实服务、隔离及组合链路见 [前两批验收报告](p7-live-provider-acceptance.md)。

最后一次截图检查发现 Runtime 缺失统计显示为 0、两项已知组件名称显示“未知”，已修复。新门户的 13 个浏览器用例和前端 428 项回归通过，本地 Control、Runtime、Dependencies 已升级；原始失败、配置摘要与清理记录均保留。

## 测试环境与结果

- Docker Desktop Kubernetes / ARM64；每次使用独立 Namespace，Python `pytest tests/e2e` 编排，浏览器由现有 TypeScript Playwright 执行。Helm 使用本机已安装的 3.22，4.1 不符合当前部署命令。
- 真实 Kimi Coding `k3`、512 维 CPU Embedding、LightRAG、Mem0；凭证读取已有 Vault 引用，不要求新 Key，不写入证据。
- MCP、故障模型和五运行 Judge 使用带认证的固定 HTTP 测试服务，实际经过 Control、Runtime、Worker 和 Trace。模型错误使用确定的 503、错误认证、超时和截断 SSE；没有把它们写成真实 Kimi 服务故障。真实 Kimi Judge 已在首批测试通过。
- OpenSandbox Server 0.2.2、真实 Code Interpreter v1.1.0，Docker/runc。CPU、内存限制通过 cgroup 读取，PID 512 为本地 Server 的固定上限；未证明可逐 Profile 调整 PID 或隔离临时磁盘。

| 范围 | pytest 场景 | 实际通过内容 |
|---|---:|---|
| MCP / Skill 授权 | 13 | 递归依赖包、跨部门全部通过后原子授权、脱敏、拒绝/取消/配置变化后 stale、撤销、禁止借用其他主体 Grant；STDIO 的 Profile/环境凭证依赖；只读 Memory 允许 read、拒绝 write；固定旧 MCP 版本使用旧服务器与凭证并实际调用 |
| 非零费用 | 1 | 真实 Kimi Token、调用账本、执行详情和 Insights 精确一致；测试单价 USD 1.25/2.5 每百万 Token，本次 111 输入 / 44 输出折算为 249 微单位，与实际 Kimi 账单无关 |
| 评测边界 | 5 | 3/4/5 运行对比、正反顺序基线、共同/缺失用例、费用差值、1/6/重复 ID 拒绝；Judge 缺 Model 或 Credential Grant 前置拒绝；非法 JSON / 503 不得判为通过，有失败 Trace |
| 服务故障与恢复 | 7 | 模型四类故障明确失败并有调用/节点错误；LightRAG 停服、错误 Key、索引处理中 Control 重启后恢复；Mem0 停服/错误认证和同应用恢复；真实 Kimi SSE 在 Gateway 替换后按 Last-Event-ID 续传，无重复增量，落库文本一致，之后新建工作流与执行成功 |
| 本地 Sandbox | 4 | CPU 0.5 核、内存 512 MiB、Server 512 PID 上限、实际 IP 出网阻断；超时和取消回收；真实 STDIO 会话及 tenant/worker/fence/lease 伪造和过期 Handle 拒绝 |
| 桌面 UI | 1 | 内部 13 个 Playwright 用例：九类资源列表的加载/503/恢复/空搜索；Runtime 错误恢复；五评测比较、改基线和 Judge Trace；无权访问及现有会话权限撤销；中英文、浅深主题 |
| Redis 准入 | 2 | 2000 条真实未消费消息与 Pending 均触发 API / Chat 429 + Retry-After，不产生拒绝请求的 MySQL 收据；已受理 Chat 幂等重放有效；排空后恢复；Redis 停服 503、恢复后重新受理 |
| 合计 | **33** | **每个场景均有专项通过记录，非单次全量认证** |

沙箱取消测试使用 120 秒程序，实际从取消到回收约 1.76 秒，并要求小于 30 秒。直接 IP 的网络测试先以无策略容器确认同一地址可达，再验证 Sandbox 被阻断，避免把 DNS 失败误记为网络隔离通过。

SSE 加强复测等待旧 Gateway Pod 删除，并用只读 readiness 请求恢复 `kubectl port-forward`，确保续传连接到替换后的 Gateway。没有自动重试写操作来掩盖重复受理问题。

## 本轮修复

1. **Redis 准入与 Chat 绕过**：原先只看 MySQL，Redis 积压不能阻止新受理，Chat 也没有调用准入。新增有界原子 Lua 快照，统计九类能力 Stream 的未消费数与 Pending；不可读取时拒绝新受理。新增三个真实水位/可用性指标，Chat 先识别已有幂等收据再检查新请求。
2. **完整资源依赖与权限提升**：MCP Tool、Server、Credential、STDIO Profile/环境凭证及嵌套 Skill 递归形成申请包，按选定不可变版本解析；Skill 的所属部门必须已拥有完整依赖和声明的具体操作权限。Tool Policy 等申请依据变化会标记 stale。
3. **冻结 MCP 版本**：工作流发布的凭证依赖现读取固定 Server Version，而非服务器当前配置。已批准旧 Tool 后修改服务器的新凭证，不再导致发布错误或替换运行时凭证。
4. **Judge 凭证前置校验**：同时要求评估 Model 与其 Credential 的工作流 Grant，缺失时返回 422 `MODEL_EVALUATOR_CREDENTIAL_GRANT_REQUIRED`，避免受理后才发现不可用。
5. **执行列表与详情范围一致**：公司范围的列表、详情、节点和 Trace 使用相同委托范围；管理员能查看其他申请人产生的执行，无关部门审核人仍拒绝。
6. **Gateway Drain 后继续操作**：Control 遇到 `SERVICE_DRAINING` 时对同一幂等命令最多尝试三次，用 `Connection: close` 淘汰旧 Pod 连接；服务故障返回 503，不再误报工作流命令冲突。
7. **运行中 Sandbox 取消**：活跃代码阶段的 `interrupting` Lease 原先未被取消命令纳入，120 秒任务继续运行。现释放该阶段的锁并触发既有 Reaper，实际快速回收。
8. **本地出网策略**：OpenSandbox 改为 `dns+nft`，实际阻断直接 IP 连接；`dns` 仅过滤 DNS，不能证明拒绝 IP 出网。[官方模式说明](https://github.com/opensandbox-group/OpenSandbox/blob/main/docs/components/egress.md)。
9. **诚实的界面状态**：共享列表组件和 Runtime 页区分加载、错误和正常空态，失败时不显示正常空表或零统计。缺失统计显示“—”，真实 0 保持为 0，补齐 Runtime 与 Node.js 插件的中英文名称。
10. **外部 Vault 诊断网络**：最终四 Release Doctor 暴露 Dependencies Doctor 的策略只允许同 Namespace 的 bundled Vault，production 的外部 Vault 检查超时。修复为只对 Doctor Pod 使用既有 `externalEgress.vault` CIDR/端口，实际两个 Token 检查均成功；没有修改或轮换 Token。失败诊断保存在 `local-doctor-before-network-fix.json`。

资源依赖、Judge、委托范围、准入及 Sandbox 文档已同步。相关源码保持模块化，已修改的前后端源码均未超过 2000 行。

## 验证与原始记录

基线 `78bb47fc9858bd8035f37a0a468e5e388090151d`，保留原有工作树修改，未提交或 push。过程目录为 `.local/artifacts/plan7-auto-remaining-20261008/`；业务证据为 `.local/artifacts/e2e/<run_id>/product/`；截图、视频和 Trace 为 `.local/artifacts/playwright/helm-agentxctl/<run_id>/remaining-desktop/`。

| Run | 原始结果 | 说明 |
|---|---|---|
| `p7autoverify1009` | 31 passed、2 failed、0 error/skip，1021.072 秒 | 后端及 STDIO 通过；桌面测试错误文案/路径不符，Gateway 换 Pod 后空闲本地转发仍指向旧 Pod，Redis 首次请求 502 |
| `p7autodesktop1009` | 4 passed、0 failed/error/skip，484.00 秒 | 修正测试文案与 Runtime 页面路径，确认旧 Pod 删除和只读 readiness；SSE、桌面及两 Redis 用例均通过，内部 Playwright 13 passed |
| `p7autoui1009` | 1 passed、0 failed/error/skip，337.902 秒 | 最终 Web 摘要；内部 Playwright 13 passed，增加缺失值/真实零值区分和中英文 Runtime 组件名称断言，截图已人工复核 |

首次 `auto-first`、`auto-corrected`、`auto-fault`、`auto-final`、`auto-closure`、`auto-sandbox` 与两个基线失败运行全部保留在 `run-history.json` 及对应 JUnit。失败涵盖实际缺陷，也包括错误 CPU 镜像摘要、重复注册 Session Fixture、Role 缺 code 和测试断言不符合契约；这些没有记为产品通过证据。

Runtime 库 159 passed、2 ignored（既有平台限定项），Control 62 passed，Runtime 集成切片 1 passed，agentxctl 50 passed；受影响 Target Clippy `-D warnings`、Rust 格式、Python Ruff / 格式、浏览器 TypeScript、前端构建、Dependencies Chart strict lint 通过。前端完整回归 **93 文件 / 428 测试通过**；lint 只有既有警告。

既有仓库 Acceptance 的 `skills-lock.json` 根目录布局失败及 `test_channel_configuration.py` 全仓格式差异未在本批修改，未写成全仓检查通过。`functional-coverage.json` 按完整 pytest node ID 去重，保留每个场景的通过 Run；不把重跑次数累加为新增场景。

## 本地升级与清理

最终本地值文件为 `.local/artifacts/plan7-auto-remaining-20261008/local-verified-values.yaml`，已通过 `agentxctl validate`。保留现有外部依赖与 Secret 引用；升级、Ready、Secret 哈希、Namespace/Sandbox 清理和脱敏收据在本批最终状态文件中记录。

Control revision 21、Runtime revision 13、Dependencies revision 14 的升级检查通过，最终 Dependencies / Control / Runtime / Observability 四个 Release Doctor 全部 healthy。9 个业务 Deployment 全部 Ready；53 个已有 Secret 的数据哈希与升级前一致。临时测试 Namespace 与 Sandbox 实例均为 0，空镜像加载 Namespace 已删除，健康的本地 OpenSandbox Server 保留供后续使用。

| 更新服务 | 本地镜像摘要 |
|---|---|
| platform-control | `sha256:92a4631acecf20b790a0d2dea65ddae75d2dae15c8775ba7fe58204ecd345ba8` |
| web-console | `sha256:12886f899298df2436650c3666325015d6b96e60dae440a7654854a46813df45` |
| runtime-gateway | `sha256:d17347c40e648a03dee3a3ba30a32c1cbe76f82fe1094fcdd9077f015249f866` |
| workflow-runtime | `sha256:fc326615f7d08d05f0c38881af39eb09a4c28183a5b126f6fe5c38c737db0eed` |
| workflow-worker | `sha256:b1dc3bde996fec4fed410683bca86383a90e051d2e47ec41ca5bb896c2d44ebf` |
| sandbox-manager | `sha256:cab4b7a2326f54a073cfc9147c257dbd8bd0a97dee4c51c88806a954cd4e298b` |

最终门户仅修改界面，后端场景不因该构建重复计数。完整 11 镜像引用位于最终值文件；上述摘要不构成同一不可变生产候选的九域认证。

Kimi Key 的明文和 Base64 扫描覆盖 783 个文件及 4420 个 ZIP 条目，残留为 0。结果、原始部署诊断、功能去重和清理分别见 `final-keyscan.json`、`local-final-doctor.json`、`functional-coverage.json`、`final-status.json`。

宿主机 80/443 仍受 Docker 特权端口映射设置阻塞，内部服务 Ready 不代表 `https://agentx.localhost` 已恢复。处理步骤沿用前两批报告中的 Docker Settings → Advanced → Allow privileged port mapping，需要本机管理员系统确认。

2026-10-09 补查确认 macOS 默认信任也未接受现有 Agentx CA；指定 CA 的实际链路诊断通过，标准 443 仍失败。公开 CA、恢复步骤与证据见 [本地 HTTPS 入口检查](p7-local-https-entry-check.md)。

## 保留范围与资源

| 未执行内容 | 当前原因 / 需要的资源 |
|---|---|
| 飞书、钉钉、企微真实回复 | 用户明确自行测试；HTTP 回调还需平台可达公网 HTTPS |
| 图片、音频、其他模型 | 用户明确暂缓；后续需要对应能力的模型与凭证 |
| RAGFlow 真实管理/检索组合 | 本机尚无运行中的兼容服务和已索引 Dataset；当前使用真实 LightRAG 完成 RAG 链路 |
| 生产 Sandbox 强隔离、逐实例 PID/磁盘、只读文件系统 | 需要 Kubernetes OpenSandbox 与批准的 Kata/gVisor 等 RuntimeClass；本机 Docker/runc 功能证据不覆盖这些项目 |
| 冻结硬件两小时容量及九域同 Run | 需要按生产门禁完整运行并绑定统一候选；本批是有界功能与故障专项 |
| PITR、真实旧版 Worker 混跑与公网供应链认证 | 需要备份/恢复资源、真实可兼容旧镜像、签名信任和公网 OIDC 配置；不由界面测试替代 |
| 宿主机 HTTPS 入口恢复 | 需要 Docker 特权端口映射与本机 CA 信任设置；隔离环境自动化使用真实 Pod 转发 |

本批没有新增资源缺失导致的功能跳过；上述项目属于已保留的手动、暂缓或生产验收范围。
