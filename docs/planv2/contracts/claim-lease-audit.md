# V2-06A Claim、Lease 与恢复审计目录

机器可读目录见 [claim-lease-audit.json](claim-lease-audit.json)。本目录覆盖所有紧凑 Profile 常驻后台循环，默认上限统一为 Lease 30 秒、Heartbeat 10 秒和批量 100；所有 MySQL 到期判断使用 `UTC_TIMESTAMP(6)`。

2026-10-07：Runtime 应用连接固定为 READ COMMITTED，避免不存在的幂等键持 gap lock 使独立预留互相死锁；执行权威仍由父行锁、Lease 与 CAS 保证。Recovery 到期扫描先以非锁定查询取得最多 100 个候选，再按主键校验状态和到期条件，逐对象更新。Worker Lease 释放与 Attempt 重排同事务提交；执行超时按 Execution 单独持锁和结算。无过期任务时不锁活跃租约或新派发的索引间隙，避免范围 UPDATE 与 Coordinator 插入的死锁。配额结算先读取所属 scope 的候选，再按主键排序更新，不以 status/reaper 索引范围 UPDATE 锁住无关 Reservation。Artifact 外置以覆盖索引选择候选并单独读取 payload；Integration Event 与 Trace Claim 先锁 ID，再按主键读取大 JSON，避免载荷进入 filesort。应用创建仅写既有 `audit_events`；删除没有消费方的 `ApplicationCreated` outbox 写入，部署与准入仍使用已有 Outbox/Receipt。

| Role | 权威队列/状态 | 协调方式 | 外部 I/O 边界 | 恢复来源 |
|---|---|---|---|---|
| Control Publisher / Admission | `publish_attempts` / `outbox` | `SKIP LOCKED` + Owner/Fencing | Claim 提交后调用 Runtime API/OSS | Attempt 下一动作、Outbox、Runtime Receipt |
| Control Projector / Retention | Cursor / `retention_runs` | 分区 Lease / `SKIP LOCKED` | Claim 提交后拉 Event 或发 Command | Cursor、Receipt、Generation、Retention Receipt |
| Runtime Command / Outbox / Sequencer | `runtime_commands` / `execution_outbox` / Cursor Row | `SKIP LOCKED` + 串行 Cursor | 提交后 Redis/SSE | MySQL Command、Outbox、唯一 `source_outbox_id` |
| Trigger / Recovery / Wait / Artifact | Binding、Attempt、Wait、Checkpoint | Lease、CAS、稳定对象键 | Provider/Redis/OSS 均在事务外 | Cursor、状态版本、Hash、幂等键 |
| Quota / GC / Retention / Trace Relay | Leader、GC Item、Retention Item、Trace Outbox | Leader Lease / `SKIP LOCKED` | Redis/OSS 在 Claim 提交后 | Ledger、审计清单、Trace Outbox |
| Worker / Sandbox | Attempt / Sandbox Lease | Redis Group + MySQL Lease / `SKIP LOCKED` | Provider 调用在 Claim 提交后 | Result Hash、Provider Label、稳定 Operation Key |
| Observability Consumer | Redis Pending | Consumer Group + Pending 接管 | ClickHouse 成功后 ACK | Pending Entry、`event_id + content_hash` |

Artifact 候选发现使用 MySQL 维护的载荷大小生成列及覆盖索引；检查点索引含外置状态、大小和 ID，输入索引同时覆盖 Tenant/ID 的引用检查。只对命中的 ID 读取 JSON，不能反复全表读取历史 payload 计算 `JSON_STORAGE_SIZE`。大小为派生索引数据，原 JSON、Hash、引用和 CAS 仍是业务事实。

已发布派发恢复使用 `idx_execution_outbox_recovery` 非锁定读取最多 100 个候选 ID，载荷不进入历史队列排序；每个 ID 在独立事务内按主键锁定，并复核 Outbox 状态/发布时间与 Attempt 的 queued 状态后才更新重派发时间。并发 Worker 领取或其他恢复副本已更新的候选跳过，不能在扫描阶段锁住历史已完成任务。

Redis Task 发布以租户/Attempt 为索引，用 Lua 原子检查对应 Stream 消息是否仍存在；未领取或 Pending 的同一消息仍存在时返回原 Stream ID，不重复 XADD。ACK 原子删除消息并仅清理同一个 Stream ID 的索引，旧 ACK 不能删除重建后的指针。索引只保留在途消息，不是执行权威；Stream 单独丢失或 Redis 全量丢失后，恢复器仍由 MySQL queued Attempt 重建。采用 Redis 的[原子脚本执行模式](https://redis.io/docs/latest/develop/programmability/eval-intro/)，不延长重派发间隔，也不以 TTL 掩盖积压。

Worker 领取时的多表读取使用 `FOR UPDATE OF a`，只锁 `node_attempts` 的主键行；不可变快照、节点与上下文读取不参与 Claim 锁。实际权限复核、Lease/Fencing 写入及 queued CAS 保留同一事务。MySQL 原生 [指定锁表语法](https://dev.mysql.com/doc/refman/8.4/en/select.html) 能避免参数解析期间占用执行主行，让 Trace 原子序号分配与业务结算不受无关领取读锁阻塞。

`worker_leases` 的主键是 `node_attempt_id`，一条租约属于一个 Attempt。稳定 Worker UUID 可以在多个 Attempt 的 `lease_token`/`worker_id` 中重复，不能存在 token 的全局唯一约束；领取、续租、结算仍同时校验 Attempt ID、Worker ID 和 Fencing。重试仅 upsert 相同 Attempt 的租约，不能覆盖同一 Worker 的其他任务。

Trace 顺序由诊断索引 `execution_events` 的递增游标分配，不更新执行主行；索引和 Outbox 同事务回滚。watermark 是游标位置，不能当作事件数量；MAX/COUNT 一致快照与 ClickHouse 范围内唯一事件数共同判定摄取完整性。持锁的执行权威行不得阻塞独立的 Artifact 输入外置 Trace。

静态门禁 `uv run --frozen pytest deploy/tests/test_contracts.py -k claim_lease` 校验目录完整性、源文件存在、批次上限、索引登记，并拒绝生产源码以 Pod 本地时间判断 `locked_until`；同文件的 Helm 测试校验 Pod UID 注入。动态的旧 Token 拒绝、强退接管和外部响应丢失必须由 `uv run --frozen pytest tests/e2e -m runtime --values <values.yaml>` 提供证据，静态目录不能替代 E2E。
