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

工作流也支持主分支 push 和手动运行。手动运行使用 Actions 页面的 Run workflow，选择待检查的分支；功能分支首次引入新工作流时，先创建 PR 触发检查。

Kubernetes E2E 固定在 GitHub 托管 `ubuntu-24.04` 上运行，由 `pytest tests/e2e --minikube` 创建 Minikube 1.39.0 / Kubernetes 1.36.1 / Calico 临时集群，结束时删除集群和 Docker 网络。它是必需任务，失败、取消或跳过均阻止 `CI`；不再使用自托管 Runner 或启用开关。

Windows 保留原生 Rust、ctl、Python/Helm 检查和 Release 二进制打包。服务镜像与 Kubernetes 节点为 Linux，因此不再重复部署一套 Windows Docker Desktop 集群 E2E。

Linux E2E 从当前 PR 源码调用 `cargo xtask images` 构建正式镜像及 Echo、CPU Embedding、Mem0 测试镜像，并导入临时节点。OpenSandbox 0.2.2 使用同一 Docker 网络自动启动；真实 Kimi 测试读取 `AGENTX_E2E_KIMI_API_KEY` Actions Secret。缺少凭证或环境启动失败会报错，不能通过跳过隐藏。Fork PR 无法读取仓库 Secret，须由维护者在受信任分支运行完整检查后合并。

PR 执行全部系统领域及显式短容量检查；短跑不产生生产容量通过证据。Tag 发布在相同托管环境执行完整容量矩阵和两小时连续采样，保留九领域 JUnit、备份恢复、滚动升级、签名及 SPDX 门禁；失败仍阻止 Release。

`agentxctl-release` 支持手动输入已有 Tag、原始构建 Run ID 和不同的兼容 Worker 摘要重新认证。手动运行先验证原始 Run 属于该 Tag、原生 Windows/Linux 构建均已成功，再下载原始二进制并分别在原生系统重验安装包，保留原始构建 provenance。公开镜像必须匹配 Tag 的源码 Commit 和 Tree 摘要；手动重跑只核验已推送的镜像，不覆盖它们。认证工具与被测源码分别检出，所有发布证据绑定被测 Tag，而不是工具分支。

CI 失败会阻止合并。修复失败后在同一个功能分支继续提交，重新运行检查；不要通过删除必需检查或设置管理员绕过来把失败当作通过。

Rust 格式检查使用仓库指定的 1.97.1 工具链；修复了首次 CI 检出的五个既有源文件格式问题。

Ruff 保留全部原有检查，仅通过官方 `allowed-confusables` 设置允许项目中文说明与错误文案中的常规全角标点；其他混淆字符仍会报错。设置说明见 https://docs.astral.sh/ruff/settings/#lint_allowed-confusables 。
