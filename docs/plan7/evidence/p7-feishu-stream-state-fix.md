# 飞书长连接发布状态与本地连接修复

日期：2026-10-08。范围是渠道连接状态展示、发布完成后的刷新、长下拉列表，以及用户当前飞书渠道的实际发布与连接。

## 原因与实现

- 原应用处于 `draft`，配置修订号为 3、已发布修订号为 0，没有应用部署或 Runtime 渠道绑定。旧界面把缺失连接记录显示为“等待连接”，不能说明尚未发布。
- 独立的 `ChannelConnectionStatus` 组件区分待发布、停用、运行状态暂不可用，以及 Runtime 的等待、已连接、重连、断开和异常。待发布时展示原因及已有创建部署入口；已有连接存在未发布变更时，说明当前连接仍使用已发布配置。
- 部署进入终态后重新获取应用与渠道数据；已发布长连接持续轮询，避免连接成功后不再刷新或发布完成后保留旧应用状态。
- E2E 暴露飞书来源菜单选项过多、菜单超出视口导致选项无法点击。共享 Radix Select 使用已有可用高度变量限制菜单高度，由 Radix Viewport 提供滚动。
- 本地集群把 `open.feishu.cn`、`msg-frontier.feishu.cn` 解析到 `198.18.0.0/15` 代理虚拟地址，Egress 按策略拒绝该地址段。沿用已验证的 Kimi 处理方式，在 CoreDNS 为 `feishu.cn` 区域使用 AliDNS 的 DNS-over-TLS，并验证 `dns.alidns.com` 证书。解析恢复为真实公网地址，应用仍通过受控出口访问平台。

CoreDNS 属于当前 Docker Desktop 集群基础设施；原始和更新后的 Corefile 均保留在本地证据中，重建集群后需重新应用该区域配置。

## 验证与本地发布

- 相关 Vitest：2 个文件、20 项通过，覆盖尚未发布、缺失运行状态、停用、已有连接的待发布变更、加载阶段及全部 Runtime 状态。
- 浏览器 TypeScript、页面 lint 和前端镜像构建通过。
- Kubernetes E2E：`p7streamstatus1008b`，`1 passed in 242.57s`。覆盖真实界面创建未发布飞书 Stream 渠道、正确状态与发布入口、长来源列表选择、删除测试渠道、完整输入/回复配置及启停、发布后自动更新“已发布”、签名入站与平台模拟回复。
- 首轮 `p7streamstatus1008` 的菜单越界失败证据保留；两个 Run 的临时 Namespace 均已清理。

Web 镜像为 `sha256:38a4ba5fd9c7e4b99d02b069010fe6ac8f4ad657fbc6698bfa740909b9f10c34`，源码树标签为 `24a91376…`。本地 Control Helm revision 为 15，8 个常驻应用服务为 1/1 Ready。通过已部署 Ingress 的临时端口转发，保留 `agentx.localhost` SNI 并验证 Ingress CA，门户入口、入口脚本、应用详情分块和运行配置均返回 HTTPS 200；验证后已关闭端口转发。

用户明确选择沿用工作流当前环境。在其已登录的应用界面创建 `v1 / Development` 部署，部署 ID 为 `01a11a4d-92b9-7511-a0fe-333da1ca3b17`。应用与发布状态均为 `active`，配置修订号与已发布修订号均为 3；原渠道修订号仍为 2。Runtime 显示 `connected`，错误为空，心跳持续更新，额外连续观察超过 35 秒仍保持连接。原页面已刷新并显示“已启用 · 已发布”和“已连接”。真实平台验收范围是鉴权、发布、WebSocket 建连及心跳，未手动发送真实 IM 测试消息。

证据目录：`.local/artifacts/plan7-stream-status-fix-20261008/`；界面截图为其中的 `feishu-connected.png`，连接与心跳记录为 `live-feishu-connection.json`。证据不包含凭证或连接票据，原 Git 暂存区内容保持不变。
