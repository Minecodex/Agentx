# 容量阈值冻结（V2S-006 前置 / plan7 P7-D2）

> 依据 `docs/planv2/04-e2e-acceptance.md` §3：V2-00 必须在首次容量 Run 前冻结一份与测试环境规格绑定的数值阈值。本文即该冻结文件。阈值一经冻结，任何容量 Run 不得调参后重跑替代门禁；变更需记录原因、硬件差异和审批人。

## 1. 绑定的测试环境规格（冻结基准）

| 项 | 规格 |
|---|---|
| 宿主机 | Apple Silicon（arm64），Docker Desktop Kubernetes v1.36.1 单节点 |
| CPU / 内存 | 10 核 / 15.6 GiB（2026-09-26 实测；与 V2-08A 时代的 16C/16Gi 不同，换环境须重新冻结） |
| 存储 | local-path-provisioner（无性能 SLA，IOPS 类阈值按"不构成瓶颈"口径记录） |
| 部署 | `agentxctl install --values deploy/values/local.yaml`，八常驻服务 replicas=1 |
| MySQL | bundled 8.x，control app 池 10 / runtime app 池 10 / gateway 8 / worker 6 / sandbox 2；server `max_connections=151` |
| Redis | bundled 7.x 单实例（`runtime-redis`），无持久化配置 |
| ClickHouse | bundled 单实例（Runtime Namespace） |
| 对象存储 | bundled MinIO（dependencies Namespace） |
| Provider | e2e fixture（echo-mcp/echo-node/lightrag/mem0/OpenSandbox 主机 18080）；容量窗口使用 `AGENTX_E2E_RAGFLOW_DISABLE=1` |
| 镜像 | `agentx/*:dev` 本地构建（cargo xtask） |

换环境（CPU/内存/存储类别/副本数/中间件规格任一变化）时本文全部数值作废，需重新冻结并记录差异。

## 2. 七组冻结阈值

### ① Invocation 接受质量

| 指标 | 阈值 |
|---|---|
| 非预期错误率（5xx/引擎异常，不含业务 4xx） | ≤ 0.5% |
| 接受延迟 p95 | ≤ 500 ms |
| 接受延迟 p99 | ≤ 2000 ms |
| Admission Reject（429）比例 | ≤ 5%（过载场景中可解释拒绝；非过载 Run 必须为 0） |

### ② SSE 稳定性

| 指标 | 阈值 |
|---|---|
| 建连/重连成功率 | ≥ 99% |
| 断线重连最大补帧延迟（Last-Event-ID 之后到追平） | ≤ 10 s |
| 终态后 SSE Drain 时间 | ≤ 30 s |

### ③ 队列积压与恢复

| 指标 | 阈值 |
|---|---|
| Ready Attempt / Outbox / Inbox / Trace 最大消息年龄 | ≤ 60 s |
| 故障恢复后清空时间 | ≤ 120 s |

### ④ Runtime MySQL 水位

| 指标 | 阈值 |
|---|---|
| 最大连接数 | ≤ 120（server 上限 151，留 31 余量给迁移/运维） |
| 池等待时间 p95 | ≤ 100 ms |
| 锁等待（InnoDB row lock）p95 | ≤ 200 ms |
| 死锁率 | = 0（出现任何死锁即失败） |
| 慢查询（>1s） | ≤ 0.1% |

### ⑤ Runtime Redis 水位

| 指标 | 阈值 |
|---|---|
| 内存 | ≤ 512 MiB |
| Stream Pending（trace/派发合计） | ≤ 5000 条 |
| Consumer Lag | ≤ 2000 条 |
| 空库重建（从 MySQL Outbox/状态）完成时间 | ≤ 300 s |

### ⑥ Provider / Sandbox 隔离池

| 指标 | 阈值 |
|---|---|
| 隔离池并发利用率 | ≤ 90%（不出现池耗尽排队） |
| 排队超时 | = 0 |
| 熔断恢复时间（半开探测到恢复） | ≤ 120 s |

### ⑦ 两小时稳定性残留（业务残留必须为零）

| 指标 | 阈值 |
|---|---|
| Lease / Reservation / Retention Hold 残留 | = 0（全部到期回收） |
| Outbox / Inbox 残留 | = 0（业务类；诊断类 ≤ 10 条且可解释） |

## 3. 采集通道现状与缺口

- 已有通道：各服务 `/metrics`（`agentx-service-kit` 12 项静态 gauge：queue_ready_items、queue_oldest_ready_seconds、active_leases、mysql_pool_waiters、sse_connections、egress_* 等）+ MySQL/Redis/ClickHouse 直查（kubectl exec）+ kubectl resources/events（conftest `_collect_artifacts`）。
- 缺口 A（D3 补）：`MetricsRegistry` 无标签维度，按 Tenant/Provider 维度的限流与熔断指标需先改造 service-kit。
- 缺口 B（D4 采集器补）：MySQL IOPS 与慢查询率、Redis Consumer Lag 当前无采集语句，容量编排的指标采集器需新增对应查询（MySQL `SHOW GLOBAL STATUS` / performance_schema、Redis `XPENDING`/`XINFO GROUPS`）。
- 缺口 C：Admission Reject 比例依赖 P7-D3 的 Gateway 429 实现落地后才有数据源；在此之前 ① 中该行只做数据采集不做门禁判定。

## 4. 冻结记录

| 项 | 值 |
|---|---|
| 冻结日期 | 2026-09-26 |
| 冻结人 | plan7 P7-D2（经 plan7 实施会话评审） |
| 依据 | `docs/planv2/04-e2e-acceptance.md` §3 七组清单 + 本机环境实测规格 |
| 变更规则 | 换环境或指标口径变化须重新冻结并记录差异，不得为通过失败 Run 原地放宽 |
