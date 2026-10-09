# V2-00 服务运行契约

本文冻结 V2 的 7 类常驻构建产物及其 Role。V2-00～V2-05 已完成分域、Runtime Engine、Query、Projector 和 Observability 主链；V2-06A 已完成多副本、Drain 和 PDB。六类后端应用的 Live/Ready 位于 `8080`、Drain 管理端口为 `9091`、低基数 Prometheus 文本格式指标端口为 `9092`；Web Console 使用 Nginx Live/Ready 和优雅退出。Drain/指标端口不由 Ingress 或业务 Service 暴露。Agentx 不安装 Prometheus、Prometheus Adapter 或 Metrics Server，也不创建 HPA/KEDA；用户平台负责抓取、告警和扩缩容。06A 的历史多副本可复现证据见 [Run `20260816-v206a-final6`](../evidence/v2-06.md)。

| 构建产物 | Plane / Role | 端口 | 数据和 Secret | 允许网络 | Drain、容量与恢复责任 |
|---|---|---|---|---|---|
| `web-console` | Control / web | `8080` | 无数据库、无基础设施 Secret | Control Ingress、`platform-control:8080` | 无权威状态；滚动时由 Ingress Drain；静态资源可重建 |
| `platform-control` | Control / api,publisher,projector,retention | `8080`,`9091` admin,`9092` metrics | Control MySQL、Control OSS；每个 Role 独立 RS256 私钥；api 可持有 Delegation 签发 Key | Control Ingress；Runtime Internal API；Control OSS、Vault；禁止 Redis/Runtime MySQL/CH | API 停止接新请求；Claim 在 30s 后接管；Outbox/Projector Cursor/Retention 由 Control MySQL 恢复 |
| `runtime-gateway` | Runtime / gateway | `8080`,`9091` admin,`9092` metrics | Runtime MySQL、Runtime OSS、Runtime Redis（仅 SSE 唤醒）；Service/User JWT 双 `kid` 公钥；API Key 运行投影；Webhook/Resource Vault 只读身份 | Runtime Ingress；Runtime MySQL/OSS；Runtime Redis Pub/Sub；只读 Vault；获准 Provider（仅类型化 Resource Check/Discovery/Debug）；禁止 Control DB/DNS/OSS | 停止接新 Invocation，SSE 发送 MySQL 重连 Cursor；Redis 丢失时每秒回查 MySQL；幂等键从 Runtime MySQL 恢复 |
| `workflow-runtime` | Runtime / coordinator,trigger,command,outbox,recovery,artifact,quota,trace-relay | `8080`,`9091` admin,`9092` metrics | Runtime MySQL、Runtime Redis、Runtime OSS；Runtime 双 `kid` 公钥 | Runtime 内部网络、Redis/OSS、Observability Trace Stream；禁止 Control DB/DNS | Claim 后提交短事务再做 I/O；30s Lease、10s Heartbeat；Quota 全量 Redis 校准先取得 `runtime_role_leases` 单 Leader/Fencing；Outbox、Wait、Quota、Artifact 可从 MySQL 恢复 |
| `workflow-worker` | Runtime / capability pools | `8080`,`9091` admin,`9092` metrics | Runtime MySQL/Redis/OSS、Vault/Provider/OpenSandbox 所需最小 Secret | Runtime Queue/Coordinator、Runtime OSS、获准 Provider；禁止 Control DB/OSS | 停止领取新 Attempt，已有 Attempt Heartbeat；模糊提交由 Attempt/Fencing/Provider 对账恢复 |
| `sandbox-manager` | Runtime / manager,reaper | `8080`,`9091` admin,`9092` metrics | Runtime MySQL、Vault、OpenSandbox；短租约签发材料 | Runtime Worker、Runtime MySQL、OpenSandbox/Vault；禁止 Control DB | 停止 Create；Reaper Lease 过期接管；外部 Sandbox 按标签对账 |
| `observability` | Observability / trace-consumer,query | `8080`,`9091` admin,`9092` metrics | ClickHouse、Runtime Redis Trace Consumer Credential、Observability OSS；不得持有 Runtime MySQL | Query Ingress、Runtime Redis、CH/OSS；禁止两个 MySQL | Consumer Group 重领；CH 批次按事件 ID 去重；CH 恢复后清空 Trace 积压 |

Migration Job 不计入 7 类常驻产物：`control-migrate`、`runtime-migrate` 和 `clickhouse-migrate` 分别只持有自己的迁移凭据。

V2-07A 的 production Profile 只接受外部托管基础设施：MySQL 使用 `verify_identity`，Redis 使用 `rediss://`，S3、Vault、OpenSandbox 和 ClickHouse 使用 HTTPS，并通过 Projected Secret 注入 CA。Production 部署器不创建 Dependencies Namespace、不生成 Secret、不渲染数据库或对象存储 StatefulSet；镜像必须固定 digest。Control、Runtime 和 Observability Target 可以独立 Validate、Render、Install、Upgrade、Status、Rollback、Uninstall 和 Doctor，应用回滚不回滚 Schema 或 Admission State。Profile 的 `replicas` 只用于首次安装，`maxReplicas` 只用于容量预算；Upgrade/Rollback 保留工作负载当前副本数。

开发与生产统一通过 Rust `agentxctl` 和相同四个 Helm Release 安装。local/test 可用 `local-tls.yaml` 在同一流程中创建 bundled TLS 依赖；CA 注入按组件配置生效。local 的 OpenSandbox TLS 代理只提供独立 Provider 的本地入口。该配置不改变 production 的外部基础设施、existing Secret、镜像摘要和私有 Sandbox LoadBalancer 门禁。

备份由外部平台 Adapter 执行，Agentx 只接收并校验 `agentx.io/backup-manifest/v1` Receipt。MySQL、OSS、ClickHouse 和 Redis 重建的冻结 RPO/RTO及 Adapter 约束见 [Backup Provider Adapter](backup-provider-adapter.md)。

## 当前后台循环 Owner

| 循环 | 当前 Owner / Role | 权威/恢复来源 |
|---|---|---|
| Control Publish Outbox | `platform-control/publisher` | Control MySQL Outbox/Receipt |
| Governance Event/Snapshot Pull | `platform-control/projector` | Runtime Export API + Control Cursor/Receipt/Generation |
| Control Retention | `platform-control/retention` | Control MySQL Retention Intent/Lease |
| Knowledge Index Reconcile | `platform-control/api` | Control MySQL 文档、固定 Provider/Vault 快照、30s Lease/5s Heartbeat/claim token；Provider track/file_path 对账 |
| Schedule/Poll/Lifecycle | `workflow-runtime/trigger` | Runtime MySQL Trigger Binding/Cursor/Lease |
| Runtime Command | `workflow-runtime/command` | Runtime MySQL Inbox/Command/Receipt |
| Execution/Trace Outbox | `workflow-runtime/outbox,trace-relay` | Runtime MySQL Outbox；Redis 可重建 |
| Lease/Wait/Quota/GC Recovery | `workflow-runtime/recovery,quota` | Runtime MySQL 当前状态、Ledger 和 Fencing |
| Artifact/Checkpoint 外置 | `workflow-runtime/artifact` | Runtime MySQL Reference + Runtime OSS Hash |
| Capability Task/Heartbeat | `workflow-worker` | Runtime MySQL Attempt Lease + Redis 派发 |
| Sandbox Health/Reaper | `sandbox-manager/reaper` | Runtime MySQL Lease + Provider Label 对账 |
| Trace Consume/Query | `observability/trace-consumer,query` | Redis Consumer Group + ClickHouse Event ID/Hash |

所有 V1 循环及其服务源文件已在 V2-08A 删除。当前循环均有唯一 Owner；新增循环必须先进入本表并证明多副本协议。

V2-06A 的完整 Role→表/Stream→Claim→Lease→Heartbeat→Fencing→索引→恢复来源目录见 [Claim/Lease 审计](claim-lease-audit.md)。MySQL Owner 优先取 Pod UID，Lease/Heartbeat/批量固定为 `30s/10s/100`；Drain 后停止新 Claim，已有工作最多 Heartbeat 45 秒。

## 调用与信任

- Control→Runtime 只允许冻结的 `/internal/runtime/v1/*` API；Runtime 不主动调用 Control。
- Control 模型、MCP、RAG 和 Memory 健康检查使用 `runtime.resources.check`；MCP 发现与已确认的调试调用使用 `runtime.resources.execute`。请求只携带版本化 Vault Reference，Runtime Gateway 用只读 Vault 身份解析并受 Provider Egress NetworkPolicy 约束；公共 `/api/v1` 响应形状保持不变。
- plan7 知识库管理是已确定的例外：上传索引与 retrieval-test 由 Control API Role 直接访问 dependencies 中获准的 LightRAG/RAGFlow，并读取固定 Vault KV secret version；部门作用域在每个操作前验证。Runtime Workflow/Agent 的 RAG 执行继续使用已签名 Bundle 的资源快照，复用相同纯协议构造。网络策略必须允许该管理路径。
- 模型预览事件保存在 Runtime MySQL，Debug 使用 execution_model_deltas 独立 cursor；BFF 使用签名 Query Scope 读取，不能借用 Trace cursor。运行评测事件与快照的 RuntimeEvaluationRuleResultV1 显式包含可空 evaluatorExecutionId，Control 必须将其持久投影，避免丢失 judge Trace 关系。
- Trace 的 executionSequence 是 `execution_events.sequence_number` 分配的数据库全局递增游标；它是排序位置，允许跨执行及事务回滚造成空号，不表示该执行的事件数。分配、索引与 Outbox 在同一事务提交，不更新执行权威行。ExecutionDetailV1 同一读取快照返回 traceWatermark（MAX）与 traceEventCount（COUNT），BFF 将二者绑定到授权摘要并传给 Observability。完整性要求已摄取最大游标达到 watermark，且该 watermark 范围内无冲突的唯一事件数等于快照数量；后续事件不能掩盖中间缺失。Invocation/SSE 与模型 delta 的独立 cursor 协议不变。
- Service JWT 固定 RS256，Claims 为 `iss/aud/sub/role/scope/iat/exp/jti`。Control Role 各自私钥和 `kid`；Runtime 仅保存当前与上一 `kid` 公钥。Service TTL 300 秒。
- BFF Delegation Token TTL 60 秒，并绑定 Tenant、Subject、Operation、Application/Execution/Session 范围。
- 写请求必须有 Idempotency Key 与业务 Version/Epoch；错误 Issuer、Audience、Role、Scope、租户、过期 Token 或已移除 Key 一律拒绝。
- Gateway 中只有 `src/services/agentx-v2-runtime/src/sse_wakeup.rs` 允许构造或调用 Runtime Redis；`invocation_events.sequence_number` 始终是 SSE 权威 Cursor，认证、Route 和 Invocation 接受不得依赖 Redis。边界检查器对其他模块的 Redis 引用直接失败。
- Webhook Bundle 只保存版本化 Vault KV v2 引用；Runtime Gateway 使用只读 Vault 身份，Vault 不可用时明确失败，不回退 Control API、数据库密文或共享 Secret。
- V2 不增加 mTLS Client Identity、请求签名、Nonce Store、Service Mesh。TLS 仍是传输要求，但身份来自上述 JWT。

## 容量与保留冻结值

- Control 最大离线目标：72 小时。
- Runtime Integration Event Log 最短 7 天；Inbox、Outbox、Receipt 最短 14 天。
- Invocation 非预期错误率 `<0.1%`；接受延迟 p95 `≤250ms`、p99 `≤750ms`。
- SSE 建连/重连成功率 `≥99.9%`，重放延迟 `≤5s`。
- Runtime 恢复后业务队列 `≤120s` 清空，Trace `≤300s` 清空。
- 每个实例的全部服务连接池预算总和不得超过其最大连接数的 70%。
- 2 小时稳定性结束后，业务 Lease、Reservation、Outbox、Inbox 残留为零。

这些阈值不得因 V2-06A 的历史功能性扩缩容结果放宽；Role 的单项阈值只能更严格。V2-06B 必须用容量、公平性和真实两小时稳定性 Run 验证它们。用户或外部 scaler 不得把副本数提高到 Profile `maxReplicas` 以上，除非先更新并重新验证容量预算。
