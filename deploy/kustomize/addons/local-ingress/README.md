# Docker Desktop 双网络节点的本地 Ingress

此平台 Addon 适用于本仓库当前本地集群：Docker `kind` 网络为 `172.18.0.0/16`，正式 Dependencies Namespace 为 `agentx-prod-deps`，MetalLB 使用 `metallb` LoadBalancerClass。使用前检查该地址空闲。不同网络或 Namespace 必须同时调整地址池及 Envoy 上游地址。

Docker Desktop 的原生 Cloud Provider 在获取同时加入 `kind` 和默认 `bridge` 的节点地址时失败，导致 80/443 未暴露。节点的 `bridge` 接口用于 Docker OpenSandbox 访问独立的 Sandbox 私网 LB，不能直接删除。此配置通过既有 MetalLB 和上游 Envoy 提供持续运行的宿主机入口。

流量路径为：`127.0.0.1:80/443 → Docker Envoy → MetalLB 172.18.255.242:80/443 → ingress-nginx → Agentx`。Envoy 仅转发 TCP；TLS、域名路由、认证和上传限制仍由 Ingress/门户处理。Sandbox 地址池和来源限制独立。Envoy 没有管理端口，不挂载 Docker Socket 或密钥，宿主机端口只绑定回环。

先应用 `kubectl apply -k deploy/kustomize/addons/local-ingress`，在正式 Values 配置 `global.ingress.loadBalancerClass: metallb`，再通过 `agentxctl upgrade --target dependencies` 重建/升级 Ingress。LoadBalancerClass 是不可变字段，现存无 Class 的 Controller Service 需要在切换前删除；Deployment、Admission Service、证书和业务数据保留。

Envoy 使用固定镜像 `envoyproxy/envoy@sha256:29d778ba078e0404d18fa45ffb834c062071b0f81dc0a6a41501ce8e23a0059f`，Docker 容器名 `agentx-local-ingress`，网络 `kind`，`--restart unless-stopped`。将本目录 `envoy.yaml` 只读挂载到 `/etc/envoy/envoy.yaml`，映射 `127.0.0.1:80:8080`、`127.0.0.1:443:8443`；配置 `--user 65532:65532 --cap-drop ALL --security-opt no-new-privileges:true --read-only`，Envoy 参数为 `--concurrency 1 --disable-hot-restart --log-level warning`。此容器属于外部平台入口，不由四个 Agentx Helm Release 管理；集群重建时先恢复 MetalLB，再恢复这个 Addon 和容器。

macOS 上 Docker Desktop 的特权端口映射必须可用。缺少 `/var/run/com.docker.vmnetd.sock` 且 Docker 报 `ports are not available` 时，在 Docker Desktop 的 Advanced 设置启用特权端口映射并完成管理员确认，不能将该错误当作 Ingress 或 TLS 故障。配置及容器准备好后启动同一个容器即可。

本机 hosts 可配置 `127.0.0.1 agentx.localhost run.agentx.localhost`；启用代理时还需确保这两个域名走本地直连，例如使用 `DOMAIN-SUFFIX,agentx.localhost,DIRECT`。

还需信任对应 Ingress TLS Secret 中的公开 `ca.crt`；仅指定 CA 的测试成功不代表系统默认 HTTPS 校验已经通过。2026-10-09 初次诊断发现端口辅助程序和 CA 信任两项阻塞；用户配置辅助程序后，标准 80/443 入口已恢复并通过指定 CA 的门户/API/Runtime 检查，系统默认信任仍待用户处理，详见 [HTTPS 入口检查](../../../../docs/plan7/evidence/p7-local-https-entry-check.md)。

临时 E2E 继续使用既有 ClusterIP 与受测试进程管理的 port-forward，不部署本 Addon、不占用 80/443。迁移记录和实际 HTTPS 检查见 [本轮验收报告](../../../../docs/plan7/evidence/p7-live-provider-acceptance.md)。
