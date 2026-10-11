# PR 与 CI 合并规则

日常开发使用功能分支和 PR。默认分支要求 GitHub Actions 的固定 `CI` 检查、同步最新主分支并解决审查讨论；失败、取消或意外跳过不能通过汇总门禁。禁止强推和删除，不设置管理员绕过。

## 执行层级

| 入口 | 实际范围 |
| --- | --- |
| 普通 PR / master push | Windows/Linux 原生源码、前端 lint/单测/构建；临时 Minikube 中 30 项核心系统测试 |
| quality 手动 full / 版本候选 | 全部受控系统、浏览器、提供商容器、权限、故障及升级回归；排除真实模型与生产容量 |
| agentxctl-release | 同一不可变 Tag 的完整托管软件检查；Windows/Linux 候选包构建、解压和原生执行；只保存 Actions 制品 |
| 独立环境 | 真实 Kimi/知识记忆质量、冻结容量矩阵和两小时窗口、公共注册表签名及正式发布认证 |

核心测试清单在 [ci_profiles.py](../tests/e2e/ci_profiles.py)。PR 保留安装、镜像归属、初始化、流式模型、应用会话、渠道投递、Loop 恢复、幂等与鉴权，以及画布插件/API-first/应用文档和资源加载的真实浏览器流程。完整提供商、画布性能、复杂业务组合和全部故障矩阵在托管发布范围执行。清单中的测试缺失或选择为空时直接失败。

系统入口保持 `pytest tests/e2e`，通过 `--ci-profile pr|release|independent` 选择范围。独立验收默认保留全部测试及原有容量阈值；Actions 不提供真实模型密钥，不把环境测试排除报告为通过。Minikube/网络及证据按运行所有权创建和清理。

完整托管软件验证：
```sh
uv run --frozen --group test pytest tests/e2e --minikube --values deploy/values/local.yaml --ci-profile release
```

独立环境具备后使用默认 independent 范围、真实临时提供商凭据和实际兼容 Worker 摘要执行既有认证；现有 `tools/scripts/release` 的完整证据、签名与公开发布校验继续保留。Actions 不推送 Docker Hub 镜像、不创建公开 CLI Release，也不依赖专用 Runner、Actions Secret 或 Environment。

`E2E diagnostics` 仅聚焦 PR/受控范围，不产生必需 CI 或正式认证。源码固定在当前候选，截图基线必须显式更新并正常比较复验。包、存储、网络、权限、TLS 与供给链的原有断言不放宽。没有定时任务。
