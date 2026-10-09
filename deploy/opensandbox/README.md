# OpenSandbox 接入

Agentx 使用 [OpenSandbox](https://github.com/opensandbox-group/OpenSandbox) 的官方 Lifecycle API 和 execd API。四个 Agentx Helm Chart都不部署 OpenSandbox，因为本地和生产使用不同 Runtime，且生产隔离配置必须由集群安全基线决定。

## 本地 Docker Desktop

当前开发机已验证 Windows 10 + WSL2 + Docker Desktop Linux Engine 可以运行 OpenSandbox Docker Runtime。使用官方 Docker 示例配置时至少设置：

- `[server].api_key`，禁止无认证启动。
- `[runtime].type = "docker"` 和固定版本的 execd image。
- `[docker].network_mode = "bridge"`、`no_new_privileges = true`、capability drop、PID 和端口范围限制。
- `[egress].image` 与 `mode = "dns+nft"`，使 `networkPolicy` 同时限制 DNS 和直接 IP 连接；`dns` 仅过滤域名，不能满足默认拒绝网络要求。模式说明见 [OpenSandbox 官方文档](https://github.com/opensandbox-group/OpenSandbox/blob/main/docs/components/egress.md)。
- `[storage].allowed_host_paths` 设置为专用非空目录白名单；OpenSandbox 0.2.2 的空数组表示不限制主机目录，不能作为拒绝策略。
- `[server].max_sandbox_timeout_seconds`，限制单 Sandbox TTL。

OpenSandbox Server 作为主机进程连接 Docker Desktop。集群内 `sandbox-manager` 使用：

```text
AGENTX_OPENSANDBOX_ENDPOINT=http://host.docker.internal:18080
AGENTX_OPENSANDBOX_SECURE_ACCESS=false
AGENTX_OPENSANDBOX_USE_SERVER_PROXY=true
AGENTX_OPENSANDBOX_API_KEY=<与 OpenSandbox Server 一致的密钥>
```

Server 只应监听 Agentx 可达的受控接口并由 Windows 防火墙限制来源。本机直接验证可绑定 `127.0.0.1`，但该地址通常不能供 Kubernetes Pod 访问。

Sandbox Profile的网络上限为`none|tcp_proxy`，Code节点使用`deny|allowlist`目标与端口段。只有Profile允许TCP代理且节点显式列出目标时，Manager才通过execd注入短期`HTTP_PROXY/HTTPS_PROXY/AGENTX_TCP_PROXY_URL`指向`agentx-egress-gateway`的3129入口；OpenSandbox NetworkPolicy仍只允许DNS和该代理地址，不允许Sandbox直连目标。Token携带节点白名单与策略hash，Gateway为每次CONNECT重新校验目标、端口和DNS解析结果。私有CA写入Sandbox临时目录并设置`SSL_CERT_FILE`，不会进入命令、Trace、日志或Artifact。

固定的 OpenSandbox Lifecycle Spec `0.1.0` 中 `NetworkRule` 只有 `action` 和 FQDN `target`，明确不支持端口字段。Agentx 因此不伪造 `port/ports`：Sandbox 规则只允许 Gateway 的专用域名，Kubernetes Service/私有 LB 只把 Profile Endpoint 的单一监听端口映射到容器 `3129`，生产私有 LB 还必须使用来源 CIDR 和受支持的内部 LB Annotation。该专用域名/IP 不得复用来暴露其他服务。Docker Desktop 本地环境使用其 LoadBalancer 端口转发暴露 `host.docker.internal:3129`；NodePort 无法从默认 OpenSandbox Docker bridge 稳定访问，不再作为本地基线。

本地约定使用 `18080`，避免与 Agentx Web 的 `8080` 冲突。Docker Runtime 不支持 Lifecycle `secureAccess=true`，因此本地 Overlay 必须显式关闭；该开关不允许沿用到生产 Kubernetes ingress，生产默认值保持 `true`。Docker Runtime 还可能返回 Pod 不可达且不带 scheme 的直接 execd Endpoint，因此 Agentx 默认请求 Server Proxy，并要求返回 URL 与 Lifecycle Server 同 Origin、路径精确匹配 `/v1/sandboxes/{sandboxId}/proxy/44772/`。

Endpoint 文档仍由 `sandbox-manager` 加密存储并经过 Host、CIDR 和 Header 白名单校验。无 scheme URL 只继承 Lifecycle Endpoint 的 scheme；query、fragment、userinfo、反斜杠、控制字符、跨 Origin Redirect 和未知 Header 都会被拒绝。API Key 只在同 Origin Lifecycle/Proxy 请求上临时附加，不写入 Endpoint 文档或发送到直连 execd Origin。只有 Manager 可以直接路由到 execd 且经过等价安全评审时才关闭 `AGENTX_OPENSANDBOX_USE_SERVER_PROXY`。探测必须同时校验 `/health` 返回 `{"status":"healthy"}`，并使用 API Key 请求 `/v1/sandboxes`；只检查 HTTP 200 会把 SPA fallback 误判为 OpenSandbox。

## 生产 Kubernetes

生产使用 OpenSandbox 官方 `kubernetes/charts/opensandbox` 和 Kubernetes Runtime，不在 Agentx 仓库复制 Controller、CRD 或 Runtime 模板。计算节点必须配置 gVisor、Kata 或经安全评审的等价 RuntimeClass；默认 runc 不能作为生产多租户强隔离门禁。

生产验收必须检查实际 Sandbox Pod 的 RuntimeClass、资源限制、只读文件系统、默认拒绝网络、短期 Credential、TTL 回收和跨租户隔离，而不只是 Server `/health`。

## E2E 生命周期

pytest Runtime/Product E2E执行以下顺序：

1. 探测或启动 OpenSandbox Server，验证 `/health` 和 API Key。
2. 记录测试前 Sandbox 清单，并拒绝复用未知 Sandbox。
3. 创建临时 `agentx-e2e` Namespace 并运行 Code/Agent 场景。
4. 验证命令、文件、Artifact、资源超限、默认断网、节点白名单与Profile上限、HTTP和原始TCP、显式私网允许、永久阻断地址及代理绕过拒绝、超时和取消。
5. 幂等销毁本次 Sandbox，断言无残留，再删除 Namespace。

运行入口：

```bash
uv run --frozen pytest tests/e2e --values deploy/values/local.yaml -m "runtime or product"
```

官方 Go SDK差分 Oracle保留在 `tests/e2e/oracles/opensandbox/`。完整环境结论和实测版本见 [OpenSandbox 可行性评估](../../docs/plan/opensandbox-feasibility.md)。
