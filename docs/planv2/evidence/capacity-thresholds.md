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
| Redis | bundled Valkey 8.x（Redis 协议）单实例（`runtime-redis`），无持久化配置 |
| ClickHouse | bundled 单实例（Runtime Namespace） |
| 对象存储 | bundled MinIO（dependencies Namespace） |
| Provider | e2e fixture（echo-mcp/echo-node/lightrag/mem0/OpenSandbox 主机 18080）；容量窗口使用 `AGENTX_E2E_RAGFLOW_DISABLE=1` |
| 镜像 | `agentx/*:dev` 本地构建（cargo xtask） |

换环境（CPU/内存/存储类别/副本数/中间件规格任一变化）时本文全部数值作废，需重新冻结并记录差异。

### GitHub 托管环境基准（2026-10-09）

本次用户明确要求“帮我改为 github 托管的机器不跳过”，因此新增独立托管环境基准。原本机基准保留用于本机认证；不把两种硬件的结果混为同一环境。

| 项 | 规格 |
|---|---|
| Runner | GitHub 公共仓库 `ubuntu-24.04` x64，4 vCPU / 16 GB RAM |
| 集群 | Minikube 1.39.0、Docker 驱动、containerd、Calico、Kubernetes v1.36.1，单节点 |
| 节点配额 | 4 CPU / 12,288 MiB；同时验证 Docker 的 CPU/内存配额和 Kubernetes 架构、版本及容量 |
| 存储 | Runner SSD 上的 Minikube 默认动态本地存储；保留低于 5 GiB 即中止的保护 |
| 部署与负载 | 与本机相同的单副本服务、中间件配置、完整矩阵与两小时连续采样 |
| 凭证 | Actions Secret `AGENTX_E2E_KIMI_API_KEY`；不读取开发机的 Vault/Namespace |
| 冻结依据 | 用户选择托管 Runner；在首次托管容量 Run 前确定规格，七组指标数值保持不变 |

节点内存报告可能显示容器限额或宿主物理内存，因此同时记录并检查明确的 12 GiB Docker 限额；节点可见内存只允许 11.8–16.5 GiB。更改配额、节点数、架构或 Kubernetes 版本会使门禁失败，不按失败 Run 放宽阈值。

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
| Redis Task 在途索引残留 | = 0（与消息一同 ACK 清理） |

## 3. 采集通道（2026-10-07 修订，数值阈值未更改）

- 各服务 `/metrics` 按 Pod 采集低基数汇总 gauge：真实 busy pool 连接数、5 秒 acquire 探针的 p95/样本数、SSE connection、Provider pool peak utilization 和 circuit open count。缺失指标、非有限值或没有 acquire 样本直接失败。
- Runtime MySQL 采集 Threads_connected、启用的 INNODB_METRICS lock_deadlocks、performance_schema 语句耗时。预编译查询从 statement/com/Execute 读取，不要求 SQL_TEXT 非空；普通 SQL 排除 BEGIN/COMMIT 等事务控制语句。InnoDB row lock time 最大值作为 p95 的保守上界，不能降低 200 ms 门禁。慢查询率为已启用的实际查询历史中 >1s 的比例。
- Redis INFO memory 使用 CLI 原始文本，SCAN TYPE stream、XINFO GROUPS pending/lag 使用 JSON 输出；业务/诊断消息年龄从 Runtime 与 Control 权威表读取。每次扩缩容后对所有新就绪 Pod 重新建指标通道。
- Redis 通过原子读取 `XINFO GROUPS` 的投递游标及其后的 `XRANGE` 条目测量未投递数量，Pending 单独计入；删除的条目不计入当前队列，已投递历史不能冒充 lag。每组最多读取 2,001 条，达到此值已超过冻结的 2,000 上限，不能判为通过；原生 lag（包括无值）与实际游标/计数一并保留。开始负载前用真实 Redis 验证删除、Pending 分离及上限饱和，探针键随后删除。新就绪 Pod 在原 10 秒采集期限内等待首个 acquire 样本；期限内仍缺失即失败。
- 每次原生最大锁等待升高时立即保存仍存活的 prepared statement 计数与耗时，防止 Pod 替换后丢失诊断；原始计时不参与替代原生锁等待门禁。
- Gateway 负载生成器运行在临时 namespace 内，使用既有 Python fixture 镜像及当前 uv 冻结环境中的 httpx 与依赖归档；每个虚拟用户独立预热持久 HTTP 连接并直接访问 Ready Pod，异步调用的状态每秒查询一次。报告保存依赖版本、归档/脚本摘要、实际目标和每阶段延迟；连接建立耗时单独记录 connectionSetupMs。kubectl port-forward 仅用于控制与采集，不能把本机 TCP/SPDY 转发耗时当成 Gateway 接受耗时。每个分级场景独立检查冻结的 500/2000 ms，长跑汇总不能掩盖某场景超标。401/404/超时/未达成功终态均为失败，正常流量 429 必须为零。
- Gateway 副本测试以轮询方式分配真实调用；报告记录每个目标接受数，1/2/3/4 副本各阶段必须覆盖全部副本。Service port-forward 固定单 Pod，不能用作多副本吞吐证明。
- 200 SSE 阶段先暂停 Worker，接受新的未完成调用；所有订阅收到成功响应后，以 Gateway connection gauge 的实际总和确认至少 200 个同时在线连接，随后恢复 Worker 并验证断开重连和终态排空。已完成调用的并发回放不能代替在线连接证明。
- 两小时认证必须至少 360 次连续采样、最大间隔不超过 30 秒；短跑 smoke 不构成认证证据。报告同时记录冻结硬件、实际副本、已发布应用、实际完成数、200 个 SSE 重连以及不同 Worker 镜像的实际领取情况。
- 5,000 Attempt 阶段以 1,250 次五节点工作流调用完成至少 5,000 个真实 NodeAttempt，单独记录阶段前后的增量和队列吞吐；200 节点工作流另有独立场景。不得减少 Attempt 数量或把定义中的 Exit 计为 Attempt。大工作流并发压力的历史失败原始数据继续保留。
- 测试持续采集节点可用存储；低于 5 GiB 时只停止本 Run 的临时 Worker/Runtime 和负载 Pod，立即中止并记录 failed，避免共享节点磁盘耗尽。此保护不改变性能阈值或数据库参数，也不能产生容量 passed。

## 4. 冻结记录

| 项 | 值 |
|---|---|
| 冻结日期 | 2026-09-26 |
| 冻结人 | plan7 P7-D2（经 plan7 实施会话评审） |
| 依据 | `docs/planv2/04-e2e-acceptance.md` §3 七组清单 + 本机环境实测规格 |
| 变更规则 | 换环境或指标口径变化须重新冻结并记录差异，不得为通过失败 Run 原地放宽 |
