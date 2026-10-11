# Kubernetes E2E 测试规范

## 1. 分工

pytest 是 Kubernetes环境和系统级 E2E的唯一编排入口；TypeScript Playwright继续验证真实浏览器和 UI业务操作。Python只负责调用Rust `agentxctl`、port-forward、环境变量、调用pnpm Playwright和收集报告，不复制部署或浏览器逻辑。

## 2. 领域 Marker

| Marker | 范围 |
|---|---|
| `infrastructure` | Namespace、存储、Secret、TLS、Migration、Bootstrap和数据域 |
| `publishing` | Bundle Prepare/Activate/Rollback与跨面投递 |
| `gateway` | Invocation、SSE、Webhook和幂等 |
| `runtime` | Worker、恢复、Claim/Lease、Drain和 Sandbox |
| `observability` | Trace Relay、Redis、ClickHouse与降级恢复 |
| `security` | NetworkPolicy、受控公网出口、Secret隔离和 RuntimeClass |
| `upgrade` | 独立升级、Migration竞争、Rollback、备份恢复 |
| `product` | 完整 UI/API闭环、稳定性和残留检查 |

文件按业务域存放于 `tests/e2e/<domain>/`，不再使用阶段编号作为主组织方式。历史 Playwright文件可以逐步按产品域更名，但部署迁移不得改变测试行为。

## 3. 环境模型

每次运行生成唯一 Run ID，并派生：

- Control、Runtime、Dependencies三个临时 Namespace；
- 唯一 IngressClass与 ingress-nginx Release资源名；
- 独立 `.local/artifacts/e2e/<run-id>/` 证据目录；
- 两个有界生命周期的 Web/Runtime port-forward。

Fixture通过 `agentxctl install/upgrade/doctor/uninstall`管理四个 Helm Release。Addon/Echo Provider仅在请求对应 Fixture时通过 `deploy/kustomize/e2e-fixtures`安装，不进入核心 Release。

`--scale-down-development` 可在测试前记录常驻开发 Deployment副本并缩容为 0，结束时在 `finally`恢复。默认成功或失败都清理临时 Namespace；只有失败且显式使用 `--keep-on-failure` 才保留现场。

## 4. 数据和操作规则

- Bootstrap、登录、资源创建、编辑、授权、发布等业务数据必须通过页面或正式 API创建。
- API/数据库只允许环境准备、故障注入和最终证据断言，不能替代被测 UI操作。
- 画布连线使用真实鼠标拖动 Handle，不注入 React State。
- 文件场景使用浏览器文件选择或拖拽，不直接写对象存储。
- 升级场景必须证明副本、PVC、权威 Secret和业务数据保持；Rollback必须使用明确 Helm Revision。
- Migration并发、Bootstrap幂等、Secret轮换回滚、Dependencies卸载保护和 production Purge拒绝是系统级不变量。
- Backup/Restore使用workspace中的Rust可执行Fixture `agentx-backup-test-adapter`验证五字段Receipt、Evidence Schema和RPO/RTO；E2E不得依赖Python Adapter或Python部署模块。

## 5. Playwright 门禁

- Chromium桌面视口固定为 `1440×900`，worker为 1。
- zh-CN、en-US、浅色和深色按风险覆盖。
- 每个新增或修改业务按钮至少有一个真实点击路径。
- 高副作用操作同时覆盖取消与确认；乐观锁至少覆盖一次 409。
- 权限受限操作同时覆盖按钮状态和服务端拒绝。
- 关键页面覆盖加载、空结果、失败和权限不足中的适用状态。
- 使用可访问 Role、Label和稳定业务名称，不依赖 CSS层级或随机 ID。
- Playwright不得出现非条件性的 skipped 场景；条件 Skip必须说明外部能力前提。

Python设置 `AGENTX_E2E_BASE_URL`、`AGENTX_E2E_RUNTIME_URL`、Run ID和 Provider Endpoint，然后调用：

```bash
pnpm --filter @agentx/e2e test
```

## 6. 证据与脱敏

Playwright 配置位于 `tests/browser/playwright.config.ts`，按配置文件位置定位仓库，将报告写入 `.local/artifacts/playwright/<stage>/<run-id>/<suite>/`。历史证据迁移不改写日志内容，旧位置对应关系见 [目录计划](repository-layout.md)。

每个 Run保存：

- install/uninstall JSON；
- Kubernetes资源、事件、Pod日志和命令时间线；
- port-forward stdout/stderr；
- Playwright HTML、JUnit、Trace、Screenshot和 Video；
- 升级/回滚 Revision、备份 Receipt与验收 Manifest。

所有文本在写盘前脱敏 Password、Token、Secret、私钥和带凭据 URL。不得提交真实 Secret、数据库连接串或客户数据。

## 7. 运行命令

```bash
# 单域
uv run --frozen --group test pytest tests/e2e --values deploy/values/local.yaml -m infrastructure

# 多域
uv run --frozen --group test pytest tests/e2e --values deploy/values/local.yaml -m "security or upgrade"

# 产品闭环
uv run --frozen --group test pytest tests/e2e --values deploy/values/local.yaml -m product

# 失败时保留现场
uv run --frozen --group test pytest tests/e2e --values deploy/values/local.yaml -m product --keep-on-failure
```

## 8. 平台门禁

- GitHub 托管 Linux E2E 使用 `pytest tests/e2e --minikube` 创建固定版本临时集群和 Calico，实际执行 NetworkPolicy 负向测试，并自动启动 OpenSandbox Docker Runtime。临时集群、测试进程和 Docker 网络都由 pytest 清理。
- Windows 保留 ctl 原生构建、打包和命令测试；Linux 容器集群的功能闭环只运行一套 Linux E2E，不再重复使用 Windows Docker Desktop Runner。
- Docker Runtime 的功能与网络隔离证据不替代生产 gVisor/Kata RuntimeClass 强隔离验收；生产隔离仍须在对应计算节点完成。
- 不执行 NetworkPolicy的本地 CNI只能运行明确标注的非生产子集，不能跳过后形成生产安全证据。
- OpenSandbox官方 Go SDK Oracle位于 `tests/e2e/oracles/opensandbox/`；Docker镜像 Entrypoint仍使用 POSIX Shell，不要求容器安装 Python。

## 9. 完成定义

功能任务只有在相关单元、契约、Helm/Kustomize渲染、临时 Kubernetes系统测试和 Playwright场景全部通过后才能完成。成功和失败路径都必须证明开发副本恢复、后台进程停止、临时 Namespace与测试容器已清理；不能用人工点击替代自动化证据。
