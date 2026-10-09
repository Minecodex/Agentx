# P7-D 本地 production Profile 重部署（2026-10-07）

本次在 `docker-desktop` 集群重新构建并发布当前工作区的 11 个正式镜像，通过 Rust `agentxctl upgrade` 完成四个 Release 的生产配置部署。镜像标签为 `prod-20261007-worktree`，实际工作负载固定 OCI manifest digest。Git 基线为 `97cfcdac85e776b131d32e825014ca0f4fcb44ac`；构建包含未提交修改，工作区差异与源码文件摘要单独留档。

## 部署与平台准备

- Control：`agentx-prod-control`；Runtime/Observability：`agentx-prod-runtime`；Dependencies：`agentx-prod-deps`。
- MySQL、Redis、ClickHouse、S3、Vault、OpenSandbox 保持独立外部依赖；现有业务密码、签名密钥和持久化数据保留。
- 宿主机 Ingress 使用 Docker Desktop 原生 LoadBalancer，提供 `https://agentx.localhost` 与 `https://run.agentx.localhost`。
- Sandbox 使用独立 MetalLB `metallb` Class、`agentx-private` 地址池和 `https://172.17.255.241:3129`，限定 Docker 私网来源 CIDR。
- OpenSandbox 保持默认 bridge 网络及默认拒绝策略。平台通过 `docker network connect --gw-priority -1 bridge desktop-control-plane` 为 kind 节点提供该网络接口；节点默认网关仍由 kind 网络提供，MetalLB 在 bridge 网段发布私网地址。
- 外部 ClickHouse 补齐 `agentx_observability` Database 和持久化 SQL access storage；Query/Consumer 从只读 XML 定义改为使用原有密码的 SQL 用户，由 ctl 迁移授予权限。
- Egress 服务证书与 Runtime CA 改为同一份信任材料，SAN 包含实际私网入口 IP。证书签发复用 Rust 密钥库，私钥没有写入本地证据。

核心资源仍只有四个 Helm Release，平台网络和外部依赖准备不形成第二套 Agentx 安装器。新增通用 `loadBalancerClass` 配置、Sandbox Service Annotation 渲染和 MetalLB 私网 IP 校验，相关配置和设计见 `docs/07-deployment.md`。

## 运行验证

1. 8 个应用 Deployment 全部可用，当前 Pod 的 `imageID` 与本次构建的 digest 一致。
2. Control、Runtime、Observability 的迁移及首次安装初始化完成；四个 Doctor Job 全部成功。MySQL 和 ClickHouse Doctor 返回 Schema、最小权限、数据库 TLS 和对象存储 TLS 检查通过。
3. ctl 集群 Validate 和 Kubernetes 严格 client dry-run 通过。
4. 宿主机以配置的 CA 验证门户 HTTPS，返回 200；Runtime `/gateway/v1/sessions/<uuid>` 的无授权请求返回 401。
5. 默认 Docker bridge 容器以正确 CA 验证 Sandbox 私网 Egress TLS，无凭据请求返回 403；换用错误 CA 时 curl 返回 60，TLS 连接被拒绝。
6. ctl 单测 50 通过；最终配置与 Helm 改动再次执行的 12 + 6 单测通过；部署契约验收 9 通过；格式、ctl Clippy 和相关 Ruff 检查通过。

本次记录的是本地 production Profile 重部署和运行验证；D7 供应链、D8 全链发布认证继续使用各自证据。

## 清理和证据

通过 ctl 删除旧 `tmpseed` E2E 环境，集群不再存在 `agentx-e2e-*` Namespace。清理失败的迁移调试 Job、本次临时验证 Pod、旧重复 Ingress、失败 `lbtest` Release 以及已删除 Namespace 对应且占用宿主机 80/443 的旧 LB 转发容器。测试源码和已有验收证据保留。

证据目录为 `.local/artifacts/production-redeploy-20261007/`：

- `summary.json`、`deploy-complete.json`：最终状态与 Release 记录。
- `production-values.yaml`、`metallb-pools.yaml`：实际部署及平台地址池配置，不含业务 Secret 值。
- `image-digests.json`、`verified-workloads.json`：11 个镜像摘要与 8 个运行工作负载。
- `*-doctor.log`：四平面 Doctor 结果。
- `sandbox-private-lb-check.json`：实际 bridge 网络的正反向 TLS 检查。
- `validate-final.json`、`dry-run-final.log`：最终配置验证。
- `working-tree.patch`、`source-inputs.sha256`：未提交工作区留档。

生产发布清单由 ctl 写入 `.local/artifacts/releases/`。

## plan7 最新修复本地升级（2026-10-08）

按用户要求从当前工作区重新构建 11 个正式镜像，标签为 `plan7-local-20261008`，源码摘要为 `d041bea6ff7d5e2c14366bcdbe410ad023e1a0d024b9d63e94468fd7a31c5482`。构建使用 `cargo xtask images`，导入 Kubernetes 后核对 OCI manifest、配置摘要与源码标签；四个既有 Release 通过 `agentxctl` 按摘要升级，没有推送公共 Registry 或 Git。

旧库的初始 DDL 与本轮修复不一致。停写后复查确认 Runtime 没有执行、调用、Bundle、工作包或其他业务数据，仅有 GC/角色租约等派生记录；保留一份权限为 0600 的原库快照后重建该空数据域。Control 只重建零行的知识文档与检索记录表，并由 Rust Migration 重放当前 0013；管理员、组织、权限、配置和既有密钥保留。没有新增兼容层或历史数据迁移。另将 Control 的角色从仅 `api` 调整为 `api,publisher,projector,retention`，使发布、投影和保留后台完整运行。

运行结果：

- 8 个业务 Deployment 的 Ready Pod 均使用本次核验的镜像摘要。
- 四个 Target 的 Doctor 检查通过；Control 13 个、Runtime 12 个迁移 checksum 与当前 SQL 全部相符，原有管理员仍为 1 个。
- 门户使用原 CA 校验 HTTPS 返回 200，Bootstrap 状态 `required=false`；Runtime HTTPS 的无授权会话请求返回 401。
- 同源码候选在隔离 namespace 执行 `pytest tests/e2e/product/test_plan7_ui.py`：1 passed、401.96 秒，failures/errors/skipped 全为 0；内部两个 Playwright 场景验证真实评测比较、Judge Trace 跳转与评估模型配置。临时 namespace、导入/审计 Pod 已清理。
- 初次停写等待误包含旧的 Doctor 完成 Pod，已修正为只等待 Deployment 所属 Pod；初次 Dependencies Doctor 返回 curl 22，后续重新执行正常通过。两个失败记录保留，未跳过健康检查。

新证据位于 `.local/artifacts/plan7-local-upgrade-20261008/`，包括 `image-receipts.json`、`production-values.yaml`、`verified-workloads.json`、`verified-schemas.json`、四个升级/Doctor 结果、`ui-junit.xml` 与升级日志。门户入口仍为 `https://agentx.localhost`。完整桌面端手动测试步骤见 [plan7 手动界面验收](../README.md#7-桌面端手动界面验收流程)。

本次是本地功能升级与专项 UI 回归；两小时容量、同 Run 九域与公网供应链认证尚未关闭。
