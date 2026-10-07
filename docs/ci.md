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

Kubernetes E2E 保留原有 `AGENTX_E2E_LINUX_ENABLED` / `AGENTX_E2E_WINDOWS_ENABLED` 开关和专用 self-hosted runner。启用时失败或取消会阻止 `CI`；关闭时跳过仅表示该集群验收未运行，不表示通过。

CI 失败会阻止合并。修复失败后在同一个功能分支继续提交，重新运行检查；不要通过删除必需检查或设置管理员绕过来把失败当作通过。

Rust 格式检查使用仓库指定的 1.97.1 工具链；修复了首次 CI 检出的五个既有源文件格式问题。

Ruff 保留全部原有检查，仅通过官方 `allowed-confusables` 设置允许项目中文说明与错误文案中的常规全角标点；其他混淆字符仍会报错。设置说明见 https://docs.astral.sh/ruff/settings/#lint_allowed-confusables 。
