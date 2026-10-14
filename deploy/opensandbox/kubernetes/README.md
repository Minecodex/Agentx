# OpenSandbox Kubernetes Runtime

在计算集群按 OpenSandbox 固定 Commit 的官方 Helm/Controller 文档安装 Lifecycle Server 与 Kubernetes Runtime。Agentx 只接入 Lifecycle Endpoint，不管理 OpenSandbox Namespace、Controller、RuntimeClass 或计算节点。

当前验收门禁覆盖 Kubernetes Pod 创建、资源限制、网络策略、TTL、取消、Manager 重启和清理。gVisor/Kata RuntimeClass 是后续生产隔离强化项；启用前应在目标集群验证镜像供应链、IPv4/IPv6 egress、Credential、跨租户隔离和节点容量。

升级顺序：先 Drain Agentx Sandbox Lease，再升级 OpenSandbox并验证 Lifecycle `/health`和认证列表接口，最后执行 `agentxctl upgrade --target runtime`与 `agentxctl doctor --target runtime`。卸载前确认无活跃 Lease和残留 Sandbox Pod。

私有 CA 通过 `AGENTX_OPENSANDBOX_TLS_CA_PATH` 同时用于 Lifecycle/execd HTTPS 和 PTY WebSocket WSS 连接。缺失、无效或不受信任的 CA 必须使连接失败，不关闭 TLS 验证。

Lifecycle `resourceLimits` 仅传 Kubernetes 支持的 CPU、内存和 ephemeral-storage。`pids` 不是合法的 Pod resource，传入会导致 Controller 无法创建 Pod；OpenSandbox Docker Runtime 的 PID 限制也来自 Server `docker.pids_limit`，不来自这个请求字段。Kubernetes 的 PID 上限由计算节点 kubelet `podPidsLimit` 管理，当前接入不承诺逐 Profile 的 PID 强制限制；生产计算集群必须另行配置和验收，不能以 Profile 的 `pidLimit` 值代替节点配置。
