# 本地 Vault 凭证保存修复（2026-10-08）

用户在凭证页面保存 Bearer 凭证时，`POST /api/v1/credentials` 返回 `VAULT_UNAVAILABLE`，请求 ID 为 `01a118fa-fb8a-7b63-9bdf-3731aca89929`。诊断只使用服务身份和随机测试值，没有使用或记录用户提供的真实模型密钥。

## 根因与恢复

- 外部 Vault HTTPS、CA 和 KV v2 的 `agentx-v2` mount 正常。
- Control Pod 中的 Token 与工作负载 Secret、Dependencies 权威 Secret 一致，但该值在实际 Vault 中不是有效身份；Runtime 权威 Token 同样无效。`capabilities-self` 返回 403、`invalid token`，管理员 lookup 返回 `bad token`。
- 在当前外部 Vault 签发与既有权威值匹配的 Control/Runtime 服务身份，并配置独立最小权限策略：Control 管理凭证与 Webhook、写入运行凭证；Runtime 只读相应路径。没有给应用 root 权限，没有重置数据库、账号或已有业务密钥。
- 现有 Pod 无需更换环境变量即可使用恢复后的身份。本地验证写入 v1、轮换 v2、实际 Worker 按引用 mount 读取 v2、销毁两个测试版本和清理测试元数据全部成功。
- 本次服务 Token 使用外部 Vault 默认有效期，当前到期时间约为 **2026-11-09 08:54（Asia/Shanghai）**，需由外部 Vault 运维按既定生命周期续期/轮换；不能当作永久密码。Token 生命周期与 `sync-secrets` 规则见 [部署文档](../../07-deployment.md#5-secret-权威源)。

## 部署检查补强

原 Dependencies Doctor 在 production 只检查 Egress，在 local/test 额外检查 Vault 健康；两者都没有验证应用 Token，所以此前 Doctor 通过不能证明凭证保存可用。

新增所有环境的 Control/Runtime Vault `lookup-self` 检查，使用已有 CA，认证响应体丢弃，错误日志保留 HTTP 状态且不输出 Token ID/accessor。已更新本地 Dependencies Release，两个实际服务身份检查通过。应用镜像沿用此前已部署版本，本次修复属于外部身份配置和部署门禁。

## 验证范围

- 部署契约：9 passed，覆盖四套配置的 Helm lint/render。
- 本地真实 HTTPS KV 生命周期：写入/轮换 200，Runtime 读取 200 且匹配 v2，销毁 204，销毁后读取 404，测试元数据清理 204。
- TLS 临时集群中，桌面浏览器通过可见表单创建 Bearer 凭证并轮换，响应 201/200、版本 v1→v2，API 只返回脱敏提示。
- 同一临时集群分别注入无效 Control、Runtime Token，Doctor 均非零退出，对应认证检查返回 403；恢复原值后 Doctor 重新 healthy。

自动化入口为 `pytest tests/e2e/infrastructure/test_vault_credentials.py`；运行 ID 为 `p7vault1008`。诊断、收据与报告位于 `.local/artifacts/plan7-vault-save-fix-20261008/`，浏览器报告位于 `.local/artifacts/playwright/helm-agentxctl/p7vault1008/vault-credentials/`。本次专项验收不替代 plan7 全量生产认证。
