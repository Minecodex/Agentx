# P7-D 安装入口与 TLS 修复证据（2026-10-07）

## 实现范围

开发、公开 Beta、本地 TLS 验收和生产统一使用 Rust `agentxctl` 的 `validate/install/upgrade/doctor/uninstall`，由四个核心 Helm Release 管理资源、初始化、等待和健康验收。移除独立的 `provision_local_production.py` 和误用 production 环境名的本地配置，使用 `deploy/values/local-tls.yaml` 验证 bundled TLS 依赖。

生产边界保持为外部基础设施、预置 Secret、镜像摘要和私有 Sandbox LoadBalancer；独立 OpenSandbox 仍是平台提供的 Provider。本次本地 TLS 验收不作为外部生产基础设施、生产 CNI、TLS Registry 签名链或 P7-D8 全链发布认证的完成证据。

## 修复内容

1. MySQL 初始化从挂载的密码文件读取应用和迁移密码，分别拒绝空值；TLS 账号要求 `REQUIRE SSL`，应用账号保留 DML 权限。
2. 所有相关常驻服务和运维 Job 在启动时显式安装 rustls CryptoProvider；Observability Redis 和 S3 客户端加载配置的 CA。
3. ClickHouse 启用 `rustls-tls-aws-lc` 和 native roots，允许 HTTPS 并加载私有 CA；真实集群首次执行发现的 `scheme is not http` 已修复。
4. 本地服务证书由现有 Rust 密钥库的 rcgen 签发，保存到权威 Secret 并按平面发布；升级复用证书，域名变化要求新 Namespace。Egress 的服务证书和 Runtime 信任锚来自同一权威材料。
5. CA 挂载、迁移等待和健康检查按组件的 `caSecretName` 启用。bundled MySQL、Redis、ClickHouse、MinIO、Vault 和本地 OpenSandbox 入口支持 TLS；Redis 禁用明文监听，ClickHouse 禁用明文 HTTP。
6. Ingress 默认保持 LoadBalancer，本地 NodePort 必须显式指定端口，ctl 返回带端口的 URL；临时 E2E 使用 ClusterIP。production 拒绝 NodePort 入口和缺少内部 LoadBalancer Annotation 的 Sandbox 配置。
7. ctl 等待 Helm Job 和 Doctor，失败不报告 ready；进程超时覆盖 Helm 原子回滚的等待窗口。迁移/Bootstrap Job 使用 `Never` 保留失败 Pod 日志；pytest 在安装失败时先采集证据，再检查清理结果，并等待被缩容的开发 Deployment 恢复。

## 本地检查

| 检查 | 结果 |
|---|---|
| `cargo test -p agentxctl` | 49 单元测试、3 CLI 测试通过 |
| Runtime infrastructure / service kit / v2 ops 单元测试 | 2 / 6 / 5 通过 |
| 相关 crate 与服务 `cargo clippy --all-targets -- -D warnings` | 通过，包含 Runtime、Control、Observability、Egress 和 ops |
| `cargo fmt --all --check`、`git diff --check` | 通过 |
| 修改的 Python 文件 Ruff check / format | 通过 |
| `tests/acceptance/test_deployment_contracts.py` | 8 通过，覆盖四组 Values 与四个 Chart |
| Kubernetes `--dry-run=client --validate=strict` | 本地 TLS 渲染通过 |
| 实际 MySQL 初始化脚本的空密码/正常密码检查 | Control、Runtime 共 6 组通过；空应用或迁移密码均在 SQL 调用前失败 |

本地日志和渲染产物在 `.local/artifacts/review-plan7/`。MySQL 脚本检查使用临时的 SQL 调用记录器，不读取实际业务凭据。

## Kubernetes 验收

统一入口：

```bash
cargo xtask images --values deploy/values/local-tls.yaml
uv run --frozen --group test pytest tests/e2e/infrastructure \
  --values deploy/values/local-tls.yaml --scale-down-development --timeout=7200
```

本次使用从 `local.yaml` 复制、仅替换为本次构建镜像的 Values 执行同一套 pytest；先验证明文安装，再通过 TLS Fixture 验证新 Namespace 的 TLS 安装。TLS Fixture 仍只调用 ctl，不创建第二套资源安装逻辑。

结果：**6 passed / 0 failed / 0 skipped，515.05 秒**。明文 Run ID 为 `74d17287b5`，TLS Run ID 为 `tls-74d17287b5`。

- 明文环境验证四个 Release 就绪、11 个配置镜像实际运行、三个平面 Bootstrap 重放幂等。
- TLS 环境验证三个 Doctor 的数据库和 S3 加密连接、Runtime Redis 加密连接、最小权限和 Egress 证书/信任锚一致。
- 将 Runtime MySQL 信任锚替换为错误 CA 后，ctl Doctor 非零退出，日志为 `invalid peer certificate: UnknownIssuer`；恢复正确 CA 后 Doctor 返回 healthy。
- Dependencies 重复升级返回 ready，CA 保持不变，Vault 和对象存储 Bootstrap Job 已完成。
- TLS Fixture 清理后等待明文开发 Deployment 恢复；两个 Run 的六个 Namespace 均已删除。首次故障定位 Run `33e6887442` 的三个 Namespace 和镜像导入创建的 `agentx-tls-deps` 也已删除，原有 Namespace 保留。

pytest / JUnit 结果在 `.local/artifacts/review-plan7/installation-e2e.log`、`installation-e2e-junit.xml`；安装/清理收据、时间线和脱敏日志在 `.local/artifacts/e2e/<run_id>/`。清理核验见 `.local/artifacts/review-plan7/cleanup.json`。

证据收集补充了 Vault unseal key、Provider API key 和 Authorization Bearer/Basic 的脱敏规则，acceptance 回归通过；本次已生成日志重新脱敏并确认没有未脱敏 unseal key。

## 全仓既有门禁失败

- 架构边界检查仍有 8 项失败：`governance_api.rs` 的表访问和动态 SQL、`delivery.rs` 的三条动态 SQL、`worker_runtime.rs` / `worker_runtime_delta.rs` 的 Redis 边界。以上文件在本次工作中未修改，没有增加白名单或放宽检查器。
- 全部 acceptance 检查为 35 passed / 1 failed；失败来自工作区原有、未跟踪的 `skills-lock.json` 违反根目录规则。该文件和用户已有的 HeroUI skill 目录保留。
- 因而本次只确认安装/TLS 修复的验证结果，不宣称全仓门禁全绿，也不更新生产认证总完成状态。

失败明细见 `.local/artifacts/review-plan7/boundaries.log`、`acceptance.log` 和 `repository-layout.log`。
