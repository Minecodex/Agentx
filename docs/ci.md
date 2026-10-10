# PR 与 CI 合并规则

本仓库默认主分支为 `master`。日常开发从最新主分支创建功能分支，将功能分支推送到组织仓库，再通过 PR 合并。

```sh
git fetch origin
git switch -c feature/your-change origin/master
git push -u origin feature/your-change
```

主分支由仓库规则集保护：必须通过 PR，必须通过 `CI` 检查，合并前必须同步最新主分支，禁止强制推送和删除。规则没有管理员绕过名单；目前不额外要求他人批准，PR 上的审查讨论必须解决。

`CI` 是固定名称的汇总检查。它在所有 PR 上运行，任一必需任务失败、取消或意外跳过都会失败，避免工作流名称或矩阵版本变化导致保护规则失效。检查来源限定为 GitHub Actions。

## 自动检查范围

Ubuntu / Windows 原有 `cargo xtask check --fast` 门禁与新增前端 lint、单测、构建、浏览器测试类型检查。Rust 版本来自 `rust-toolchain.toml`；push 触发分支修正为实际 `master`。

浏览器默认不自动生成缺失的截图基线；新增或更新基线须显式设置 `AGENTX_E2E_UPDATE_SNAPSHOTS=1` 后复核。循环变量选择器分别保留 Linux 与既有桌面系统字体基线，组件布局和原像素差异阈值不改。

工作流也支持主分支 push 和手动运行。手动运行使用 Actions 页面的 Run workflow，选择待检查的分支；功能分支首次引入新工作流时，先创建 PR 触发检查。

Kubernetes E2E 固定在 GitHub 托管 `ubuntu-24.04` 上运行，由 `pytest tests/e2e --minikube` 创建 Minikube 1.39.0 / Kubernetes 1.36.1 / Calico 临时集群，结束时删除集群和 Docker 网络。它是必需任务，失败、取消或跳过均阻止 `CI`；不再使用自托管 Runner 或启用开关。

临时集群显式使用 API 端口 6443，符合现有 Ingress controller/证书 Job 的 API Egress 合同；Minikube 默认 8443 会在安装后已有默认拒绝策略的升级阶段被阻止。保持原网络策略，升级期间额外保存所有当前 run 所属 Namespace 的容器及 Helm Hook 失败日志。

Vault HTTPS 凭据创建/轮换与无效服务令牌测试明确使用 TLS 部署夹具及其自身 Web/Runtime/Sandbox 服务地址。普通部署与 TLS 部署复用同一端口转发准备/清理逻辑，不把明文 Vault 配置当作 TLS UI 验收；两种部署仍分别实际安装。

Windows 保留原生 Rust、ctl、Python/Helm 检查和 Release 二进制打包。服务镜像与 Kubernetes 节点为 Linux，因此不再重复部署一套 Windows Docker Desktop 集群 E2E。

Linux E2E 从当前 PR 源码调用 `cargo xtask images` 构建正式镜像及 Echo、CPU Embedding、Mem0、LightRAG 缓存测试镜像，并导入临时节点。Minikube 启动后显式设置 Docker 的 4 CPU 配额和 12 GiB 内存上限，容量证据核对实际容器限制。OpenSandbox 0.2.2 使用同一 Docker 网络自动启动。PR 必须完成受控提供商的实际业务、数据库、浏览器、恢复和隔离检查；真实模型质量与容量不计入这个 PR 检查的通过范围。

Bundled MinIO 和 RAGFlow 的 MinIO 使用固定多架构摘要的 Chainguard 公开镜像；初始化 Job 使用包含 `mc` 和 shell 的同源镜像。原 `quay.io/minio/minio` 与 `quay.io/minio/mc` 已不能匿名拉取，不能依赖本地旧缓存通过认证。Chart 变化会触发 ctl 重新编译；已构建的旧 Tag 安装包不会因工具分支修复而改变，必须通过包含新 Chart 的候选包重新验收。

每次版本发布先执行同一 Tag 的前端检查及 Windows/Linux 原生源码门禁，再构建候选二进制和镜像。随后在相同托管环境执行完整九领域 E2E、真实模型、容量矩阵和两小时连续采样，保留备份恢复、滚动升级、签名及 SPDX 门禁；失败仍阻止 Release。真实 Kimi 验收读取 `AGENTX_E2E_KIMI_API_KEY` Actions Secret，缺少凭证必须失败。没有定时任务。手动 quality 的 full 输入可在发布前执行包含真实模型和完整容量矩阵的验收，普通 PR 明确排除 capacity/live_model，而不是把它们报告为通过。

E2E 数据库断言通过共享 MySQL 客户端使用容器内 loopback TCP，不依赖镜像的默认 socket 路径；冷初始化回归会使用不同 server socket，并验证应用探针与管理断言都可实际查询。

共享管理客户端在已挂载 TLS CA 的数据库容器中使用 VERIFY_CA；明文 local profile 使用 MySQL 8 caching_sha2_password 的 RSA 公钥交换，避免热连接缓存掩盖冷库认证失败。生产数据库强制安全传输和应用身份校验的既有策略继续由部署合同执行。

Bundled MySQL 的 TLS 服务材料先由同一非 root 数据库身份从 Secret 投射到内存卷中的普通文件，校验证书链并使用 0600 权限；缺少材料或不可读取时初始化即失败。客户端仍只读取其 CA 投射。该路径覆盖 Control/Runtime 两套数据库，保持 require_secure_transport 与身份校验，并在完整 TLS 安装套件中验证。

MySQL 服务材料挂载在 `/etc/mysql/agentx-tls`，符合 Linux 宿主 MySQL AppArmor 配置目录策略。托管 Docker/Minikube 的实际探针确认 `/tls` 被 `/usr/sbin/mysqld (enforce)` 拒绝，而相同 UID/权限/证书在配置目录成功协商 TLS_AES_128_GCM_SHA256；没有停用宿主策略或改为 root。默认初始化、服务启动、探针与数据库内管理断言均使用同一目录，材料仍是只读内存卷，不持久化私钥。

MySQL AppArmor 定位证据来自独立临时 Minikube 的路径/策略对照，收集进程身份、策略、文件元数据及实际 TLS 连接。临时诊断工具已移出最终仓库；Kubernetes 正式系统验收继续使用唯一的 `pytest tests/e2e` 编排入口，由现有 TLS 安装/错误 CA/升级套件验证完整部署行为。

`agentxctl-release` 支持手动输入已有 Tag、原始构建 Run ID 和不同的兼容 Worker 摘要重新认证。手动运行先验证原始 Run 属于该 Tag、原生 Windows/Linux 构建均已成功，再下载原始二进制并分别在原生系统重验安装包，保留原始构建 provenance。公开镜像必须匹配 Tag 的源码 Commit 和 Tree 摘要；手动重跑只核验已推送的镜像，不覆盖它们。认证工具与被测源码分别检出，所有发布证据绑定被测 Tag，而不是工具分支。

CI 失败会阻止合并。修复失败后在同一个功能分支继续提交，重新运行检查；不要通过删除必需检查或设置管理员绕过来把失败当作通过。

受控系统套件保留全部失败结果，避免首个业务断言遮住其他领域的验收。L3 出站消息先发布基础 Workflow 版本，再创建引用该版本输入合同的渠道，最后发布带真实渠道 ID 的 send_message 版本；这与当前渠道保存规则一致，保留幂等投递、真实 pod 崩溃与恢复断言。

RAGFlow MySQL 就绪探针实际查询容器内 loopback TCP，避免镜像客户端默认 socket 与服务端 socket 不一致使已经运行的数据库永远不进入 Service endpoints。真实 LightRAG 工作区使用符合当前 ASCII 字母/数字/下划线合同的隔离 ID；Insights 等待本次工作流的真实 ClickHouse 行，而不是已有容量/业务测试的其他工作流行。

OpenSandbox 0.2.2 继续使用默认 Docker bridge 与实际 egress sidecar，通过 lifecycle server proxy 访问 execd；不把沙盒改到不支持 networkPolicy 的 Minikube 命名网络。真实模型浏览器验收显式准备 Agent 工作流及缺少应用会话的受控失败，按返回的工作流 ID 读取，不依赖其他测试先执行或首屏列表恰好包含该工作流。

Minikube 的普通与 TLS Egress NodePort 仅发布到实际 Docker 默认 bridge 的宿主接口，沙盒使用该接口 IP 连接 HTTPS CONNECT Gateway，避免依赖其他 Docker 网络中的容器名解析。准入源网段包含本次节点网络及实际沙盒 bridge；证书仍由正式安装器为当前 endpoint 生成并验证，Proxy 身份认证、端口白名单和拒绝策略保持生效。默认部署值不改，动态接口和端口映射写入本次运行证据。

稳定测试 URL 的 port-forward supervisor 定期发送只读健康请求，提前发现 rollout 后仍指向已删除 Pod 的旧隧道并重连；不通过重试业务 POST 隐藏重复执行或投递错误。

受控套件按当前合同准备渠道必填输入映射、合法的 LightRAG 工作区和当前应用会话；Redis 拒绝证据只统计本次应用，并通过真实新请求更新准入快照后核对恢复。画布首次可交互计时在画布和首节点就绪时结束，独立保留插件样式的五轮打开/关闭压力断言以及原 FPS、延迟和 3 秒预算。MCP Trace 核对冻结的实际工具名；大响应 fixture 同时覆盖 JSON 与真实 SSE，避免流式响应过小而没有触发既有外置对象合同。模型 HTTP 拒绝使用统一的 Provider 错误分类和账本，成功响应继续校验 SSE 完成与中断。

跨模块流式应用按 run 与模块生成独立 slug；真实知识文档的内容标记也按模块隔离，避免 provider 去重/删除串扰。长寿命用户夹具在令牌到期前重新登录，并在发送业务请求前更新凭据；服务器返回 401 时仍直接保留失败，不重试业务写入，也不放宽原有 15 分钟用户令牌期限。

`E2E diagnostics` 可手动聚焦受控、真实模型或容量套件，不产生必需的 `CI` 检查，也不作为发布认证。容量诊断明确使用 smoke 模式，记录首批受理阶段负载进程 CPU 与 cgroup throttling 的真实计数，帮助区分客户端预算与服务端延迟；500 ms P95 等冻结门槛和正式两小时要求保持原样。

安装期间持续保存失败容器的日志，避免 Helm 的自动回滚删除故障现场；最终诊断分别采集每个初始化容器、业务容器及上一次崩溃的日志，未启动的容器不会阻断其他日志。证据会脱敏，保留 Kubernetes Secret 引用和 YAML / JSON 结构。

Rust 格式检查使用仓库指定的 1.97.1 工具链；修复了首次 CI 检出的五个既有源文件格式问题。

Ruff 保留全部原有检查，仅通过官方 `allowed-confusables` 设置允许项目中文说明与错误文案中的常规全角标点；其他混淆字符仍会报错。设置说明见 https://docs.astral.sh/ruff/settings/#lint_allowed-confusables 。
