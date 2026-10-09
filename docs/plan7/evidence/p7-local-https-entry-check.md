# 本地 HTTPS 入口检查（2026-10-09）

**最新状态：标准 80/443 入口已恢复并通过指定 CA 的检查；系统默认 CA 信任仍未完成。** 下文保留初次端口映射失败和临时入口诊断，后续进展见文末。

## 当前阻塞

1. 本机 Docker Desktop 为 4.82.0。`agentx-local-ingress` 仍为 `created`，127.0.0.1 的 80/443 均未监听。实际 `docker start agentx-local-ingress` 返回 `ports are not available`，原因是 `/var/run/com.docker.vmnetd.sock` 不存在；特权辅助程序也未安装。该版本启用特权端口映射需要系统授权，见 [Docker 官方说明](https://docs.docker.com/desktop/setup/install/mac-permission-requirements/#binding-privileged-ports)。
2. 现有门户与 Runtime 使用同一个 `Agentx Local Production CA`。指定此 CA 时 TLS 与域名验证通过；不指定 CA 的系统 curl 均返回 60，`unable to get local issuer certificate`。macOS `security verify-cert -p ssl` 对两个域名均返回 `CSSMERR_TP_NOT_TRUSTED`，需要配置本机证书信任。

服务证书的 SAN 分别为 `agentx.localhost`、`run.agentx.localhost`，有效期 2026-09-30 至 2036-09-27。此次没有更换证书或修改 Kubernetes Secret。

## 实际链路检查

使用固定的 Envoy 镜像、仓库已有 `envoy.yaml` 和 `kind` 网络创建临时容器，随机高端口仅绑定 127.0.0.1，转发到真实 MetalLB VIP `172.18.255.242:80/443`。Envoy 仅透传 TCP，TLS 仍由现有 Ingress 终止。所有正向 TLS 请求验证现有 CA 与域名，没有使用 `-k` 或关闭校验。

| 检查 | 实际结果 |
|---|---|
| 两个正式域名的标准 443，含强制回环连接 | 连接失败，curl 7 |
| 临时入口的门户 `/` | 200，TLS/域名验证通过 |
| `/api/v1/bootstrap/status` | 200，`required=false` |
| `/runtime-config.js` | 200 |
| HTML 引用的 15 个当前前端资源 | 全部 200 |
| 当前 `workflow-detail-page-*.js` 动态分块 | 200 |
| 不存在的 JS 资源 | 404，未返回 SPA HTML 冒充模块 |
| HTTP → HTTPS | 308，Location 为 `https://agentx.localhost` |
| Runtime 正确 UUID 路径的无授权请求 | 401，认证边界有效 |
| 门户 Origin 的 Runtime CORS 预检 | 204，允许的 Origin 与请求 Header 正确 |
| Runtime `/health/ready` 经公开 Ingress | 404，符合仅发布 `/gateway/v1` 的路由边界 |
| 不指定 CA 的默认 TLS 信任检查 | 两个域名均失败，curl 60；macOS 信任检查也失败 |

本机解析器当前给两个域名返回 `198.18.*` fake-IP，但临时入口的门户普通域名请求仍成功，不能仅凭该地址判断本机入口 DNS 已坏。本次未修改 DNS 或 `/etc/hosts`。

补充诊断中一个 Runtime URL 误用了四段 UUID，返回明确的 400 格式错误；原始记录保留。正确五段 UUID 路径已验证 401。最初临时容器仅等待 TCP 监听，存在启动竞态；后续改为等待经过 CA 校验的 HTTPS 200 后执行检查。

## 恢复步骤

1. 在 Docker Desktop → Settings → Advanced 启用特权端口映射，完成 macOS 管理员确认，使 Docker 特权辅助程序可用。
2. 将公开 CA `.local/artifacts/plan7-local-https-20261009/portal-ca.pem` 导入“钥匙串访问”，对该 Agentx CA 配置 SSL 信任。两个域名共用此 CA，无需分别导入服务证书。操作方法见 [Apple 官方信任设置说明](https://support.apple.com/zh-cn/guide/keychain-access/kyca11871/mac)。
3. 启动已经配置好的入口容器：`docker start agentx-local-ingress`。
4. 使用标准 443 地址、默认信任重新验证门户与 Runtime；这一步尚未通过，不能提前标记入口恢复。

公开 CA 的 SHA-256 指纹：

```text
CB:36:FA:55:88:3B:52:7C:07:09:EF:66:A8:33:D3:EE:96:30:43:85:9A:06:B0:E5:A5:48:B8:B0:73:1C:43:AC
```

两项系统设置需要本机交互确认，本次未修改系统信任、重装 Docker 或重启集群。

## 证据和清理

过程目录：`.local/artifacts/plan7-local-https-20261009/`。

- `start-ingress.json`：真实 443 映射失败。
- `https-checks.json`：标准入口失败及临时入口 TLS、API、资产状态。
- `trust-and-routing-checks.json`、`macos-certificate-trust.json`：默认信任失败、CORS 与公开路由检查。
- `portal-ca.pem`、`runtime-ca.pem`、服务证书与描述：仅保存公开材料，没有保存私钥或业务凭证。
- `summary.json`、两个 cleanup JSON：最终阻塞状态和临时容器删除记录。

临时容器均已删除，业务资源和原入口容器配置保留。本次为已有本地环境的只读网络诊断，没有创建业务数据，不计入此前 33 个 pytest 功能场景，也不替代生产认证。

## 后续进展（2026-10-09）

用户完成 Docker 特权端口辅助程序配置后，持久容器 `agentx-local-ingress` 已启动，宿主机回环 80/443 映射恢复。实际路径为 `hosts → 127.0.0.1 → Envoy TCP → MetalLB VIP → ingress-nginx → 门户/Runtime`；Envoy 不终止 TLS。用户同时配置两个域名的 hosts 和代理直连规则。

标准 443 经现有 CA 与域名校验后，门户及 Bootstrap 返回 200，Runtime 未授权请求返回 401，CORS 预检返回 204；门户动态分块可获取，不存在的 JS 仍返回 404。macOS 默认信任检查仍失败，用户自行处理浏览器访问和信任设置，此状态不能记为系统默认 HTTPS 验证全部通过。

随后调试台专项修复部署后，在用户浏览器中保存并发布 `question → result` 对话映射，Runtime 投影已生效，创建会话成功且输入框可用。未发送真实模型消息，未修改系统信任或浏览器安全设置。相关记录位于 `.local/artifacts/plan7-playground-fix-20261009-103533/`；自动化回归由 `tests/e2e/product/test_playground_errors.py` 编排，在临时 Namespace 验证映射、真实对话及网络阻断提示，`p7playground1009b` 为 1 passed，内部 Playwright 为 1 passed。
