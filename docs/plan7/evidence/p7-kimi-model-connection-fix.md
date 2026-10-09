# 本地 Kimi 模型连接 DNS 修复（2026-10-08）

模型 `kimi-k3`（Alias ID `01a11918-abae-7671-bed3-97b793f453f8`）已通过现有用户浏览器中的“测试连接”，Control 记录为 `healthy`，耗时 `4860 ms`，无错误码。此记录只证明该模型的连接检查通过，不代替完整 Workflow、流式、多模态或生产网络隔离验收。

## 根因

- 实际配置为 OpenAI Chat Completions、Endpoint `https://api.kimi.com/coding/v1`、上游模型 ID `k3`。两项均符合 [Kimi 官方模型配置](https://www.kimi.com/code/docs/kimi-code/models.html)，无需修改。
- Gateway Pod 与宿主机的普通 DNS 查询均得到 `198.18.0.109`，属于 `198.18.0.0/15` 合成地址范围。
- production Gateway 的 `AGENTX_EGRESS_ALLOW_DOCKER_DESKTOP_DNS=false`，在 CONNECT 授权阶段正确拒绝该解析结果。日志为 `benchmark address range is forbidden`；三次连接尝试在约 40–50 ms 内结束，Runtime 报 `PROVIDER_UNAVAILABLE`。
- Vault 凭证读取不是本次故障来源，请求尚未到达 Kimi。

## 集群恢复

只向 `kube-system/coredns` 的 `Corefile` 增加以下独立域名区块，原有 Kubernetes 与其他域名解析规则保留：

```text
api.kimi.com:53 {
    errors
    cache 30
    forward . tls://223.5.5.5 tls://223.6.6.6 {
        tls_servername dns.alidns.com
        health_check 5s
    }
}
```

采用 CoreDNS 已有 [forward 插件](https://coredns.io/plugins/forward/) 的 DNS-over-TLS 能力，使用系统 CA 与 `dns.alidns.com` 校验上游证书。两个 CoreDNS 副本均自动 reload 成功；Gateway 重新解析为真实公网地址 `103.143.17.156`（本次观测值，不固定 IP）。

应用镜像、模型配置、凭证、Egress 地址策略与 TLS 校验保持原配置。修复不引入应用内域名特判、IP 固定值或出网绕行。CoreDNS 属于平台基础设施，此配置会保留在当前集群；Docker Desktop Kubernetes 重建后需重新应用。

## 真实验证

2026-10-08 09:29（Asia/Shanghai）在用户已登录的原模型详情页点击“测试连接”：

1. Control 按当前 Deployment 与 Credential Version 发起资源检查。
2. Runtime 读取 Vault 中已有凭证，通过签名 CONNECT 使用 Egress Gateway。
3. Gateway 日志确认 `api.kimi.com:443` 的 tunnel `decision=allowed`，随后正常关闭。
4. Control `resource_health_checks` 最新记录为 `healthy`，`latency_ms=4860`，错误码与错误消息为空；界面显示“连接正常”。

本轮使用的是该真实界面流程，未新增或声称通过 pytest 自动化 Run。没有读取或输出凭证明文、在命令参数或证据中保存密钥，未更换客户端身份标识。

本地证据目录：`.local/artifacts/plan7-kimi-connection-fix-20261008/`，包含原始与恢复后的 Corefile、`summary.json`、脱敏 Gateway 日志和 `model-connection-healthy.png`。原用户暂存区二进制 Diff 的 SHA256 保持 `26ef4c190e1af60a205293778d19830dc543e5aaab37d5e63e39bff29a120c85`。
