"""Real cluster measurements; unavailable collectors invalidate capacity evidence."""

from __future__ import annotations

import json
import math
import socket
import threading
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import httpx
import yaml

from tests.e2e.runtime.test_agent_attachments import _control_mysql, _runtime_mysql
from tests.e2e.support import ManagedProcess, redact, run, start_process

MINIMUM_FREE_STORAGE_BYTES = 5 * 1024**3

# Cursor and entries are read atomically. 2,001 is a saturation marker above
# the frozen 2,000 limit, not an approximation that could pass an overload.
UNREAD_STREAM_SNAPSHOT = """
if redis.call('EXISTS', KEYS[1]) == 0 then return {} end
local result = {}
for _, group in ipairs(redis.call('XINFO', 'GROUPS', KEYS[1])) do
  local name, pending, cursor, native = nil, nil, nil, false
  for i = 1, #group, 2 do
    if group[i] == 'name' then name = group[i+1] end
    if group[i] == 'pending' then pending = group[i+1] end
    if group[i] == 'last-delivered-id' then cursor = group[i+1] end
    if group[i] == 'lag' then native = group[i+1] end
  end
  local entries = redis.call('XRANGE', KEYS[1], '(' .. cursor, '+', 'COUNT', 2001)
  result[#result+1] = {name, pending, #entries, cursor, native}
end
return result
"""


def verify_redis_unread_measurement(context: dict[str, str]) -> dict[str, Any]:
    """Exercise the measurement against the actual isolated Redis server."""
    key = f"agentx:capacity-measurement:{context['run_id']}"
    proof = {}
    try:
        redis_command(
            context, "EVAL", "for i=1,6 do redis.call('XADD',KEYS[1],i..'-0','probe','value') end return 6", "1", key
        )
        redis_command(context, "XGROUP", "CREATE", key, "probe", "2-0")
        redis_command(context, "XDEL", key, "3-0")
        snapshot = redis_command(context, "EVAL", UNREAD_STREAM_SNAPSHOT, "1", key)
        assert snapshot[0][1:3] == [0, 3], snapshot
        proof["deletedUnread"] = snapshot
        redis_command(context, "XREADGROUP", "GROUP", "probe", "reader", "COUNT", "2", "STREAMS", key, ">")
        snapshot = redis_command(context, "EVAL", UNREAD_STREAM_SNAPSHOT, "1", key)
        assert snapshot[0][1:3] == [2, 1], snapshot
        proof["pendingIsSeparate"] = snapshot
        redis_command(context, "DEL", key)
        redis_command(
            context,
            "EVAL",
            "for i=1,2030 do redis.call('XADD',KEYS[1],i..'-0','probe','value') end return 2030",
            "1",
            key,
        )
        redis_command(context, "XGROUP", "CREATE", key, "probe", "0-0")
        snapshot = redis_command(context, "EVAL", UNREAD_STREAM_SNAPSHOT, "1", key)
        assert snapshot[0][2] == 2001, snapshot
        proof["aboveLimitSaturates"] = snapshot
        return proof
    finally:
        redis_command(context, "DEL", key)


def parse_metrics(body: str) -> dict[str, float]:
    metrics = {}
    for line in body.splitlines():
        if line.startswith("agentx_"):
            name, value = line.split()
            number = float(value)
            if not math.isfinite(number):
                raise ValueError(f"non-finite metric {name}")
            metrics[name] = number
    return metrics


def redis_command(context: dict[str, str], *args: str) -> Any:
    config = yaml.safe_load(Path(context["values"]).read_text())
    tls = config["global"]["components"]["runtimeRedis"].get("caSecretName")
    flags = "--tls --cacert /tls/ca.crt " if tls else ""
    mode = "--raw" if args[0] == "INFO" else "--json"
    result = run(
        (
            "kubectl",
            "-n",
            context["runtime_namespace"],
            "exec",
            "statefulset/runtime-redis",
            "--",
            "sh",
            "-ec",
            f'redis-cli {flags}--no-auth-warning {mode} -a "$REDIS_PASSWORD" "$@"',
            "agentx-capacity-redis",
            *args,
        ),
        timeout=30,
    )
    return result.stdout if args[0] == "INFO" else json.loads(result.stdout)


def queue_state(context: dict[str, str]) -> dict[str, float]:
    raw = _runtime_mysql(
        context,
        "SELECT JSON_OBJECT("
        "'oldestReadySeconds',(SELECT COALESCE(MAX(TIMESTAMPDIFF(MICROSECOND,created_at,UTC_TIMESTAMP(6))),0)/1000000 FROM node_attempts WHERE status='queued'),"
        "'oldestOutboxSeconds',(SELECT COALESCE(MAX(TIMESTAMPDIFF(MICROSECOND,created_at,UTC_TIMESTAMP(6))),0)/1000000 FROM execution_outbox WHERE status IN ('pending','failed')),"
        "'oldestInboxSeconds',(SELECT COALESCE(MAX(TIMESTAMPDIFF(MICROSECOND,created_at,UTC_TIMESTAMP(6))),0)/1000000 FROM runtime_commands WHERE status IN ('pending','processing','failed')),"
        "'oldestTraceSeconds',(SELECT COALESCE(MAX(TIMESTAMPDIFF(MICROSECOND,created_at,UTC_TIMESTAMP(6))),0)/1000000 FROM trace_outbox WHERE status IN ('pending','failed')),"
        "'pending',(SELECT COUNT(*) FROM node_attempts WHERE status IN ('queued','running'))+"
        "(SELECT COUNT(*) FROM execution_outbox WHERE status IN ('pending','failed'))+"
        "(SELECT COUNT(*) FROM runtime_commands WHERE status IN ('pending','processing','failed'))+"
        "(SELECT COUNT(*) FROM trace_outbox WHERE status IN ('pending','failed')));",
    )
    values = {key: float(value) for key, value in json.loads(raw).items()}
    control = _control_mysql(
        context,
        "SELECT JSON_OBJECT('pending',COUNT(*),'oldestSeconds',COALESCE(MAX(TIMESTAMPDIFF(MICROSECOND,occurred_at,UTC_TIMESTAMP(6))),0)/1000000) FROM outbox WHERE status IN ('pending','processing','failed');",
    )
    control_values = json.loads(control)
    values["oldestOutboxSeconds"] = max(values["oldestOutboxSeconds"], float(control_values["oldestSeconds"]))
    values["pending"] += int(control_values["pending"])
    return values


def wait_queues_empty(context: dict[str, str], timeout: float = 120) -> float:
    started = time.monotonic()
    while time.monotonic() - started < timeout:
        if queue_state(context)["pending"] == 0:
            return time.monotonic() - started
        time.sleep(1)
    raise TimeoutError("business and trace queues did not drain within the frozen recovery window")


def residual_state(context: dict[str, str]) -> dict[str, int]:
    raw = _runtime_mysql(
        context,
        "SELECT JSON_OBJECT("
        "'leases',(SELECT COUNT(*) FROM worker_leases WHERE released_at IS NULL)+(SELECT COUNT(*) FROM node_attempts WHERE status='running' OR locked_until IS NOT NULL),"
        "'reservations',(SELECT COUNT(*) FROM quota_reservations WHERE status='active'),"
        "'holds',(SELECT COUNT(*) FROM runtime_retention_holds WHERE released_at IS NULL AND (expires_at IS NULL OR expires_at>UTC_TIMESTAMP(6)))+(SELECT COUNT(*) FROM bundle_retention_holds WHERE released_at IS NULL AND (expires_at IS NULL OR expires_at>UTC_TIMESTAMP(6))),"
        "'businessOutbox',(SELECT COUNT(*) FROM execution_outbox WHERE status IN ('pending','failed'))+(SELECT COUNT(*) FROM delivery_outbox WHERE status NOT IN ('delivered','dead')),"
        "'businessInbox',(SELECT COUNT(*) FROM runtime_commands WHERE status IN ('pending','processing','failed')),"
        "'diagnosticBacklog',(SELECT COUNT(*) FROM trace_outbox WHERE status IN ('pending','failed')));",
    )
    values = {key: int(value) for key, value in json.loads(raw).items()}
    values["businessOutbox"] += int(
        _control_mysql(context, "SELECT COUNT(*) FROM outbox WHERE status IN ('pending','processing','failed');")
    )
    cursor = "0"
    keys = set()
    while True:
        cursor, page = redis_command(
            context, "SCAN", cursor, "MATCH", "agentx:v2:tasks:dedup:v1:*", "COUNT", "1000", "TYPE", "hash"
        )
        keys.update(page)
        if cursor == "0":
            break
    values["taskReceipts"] = sum(redis_command(context, "HLEN", key) for key in sorted(keys))
    return values


class CapacityCollector(threading.Thread):
    def __init__(self, context: dict[str, str]):
        super().__init__(daemon=True)
        self.context = context
        self.stop_event = threading.Event()
        self.sample_lock = threading.RLock()
        self.errors: list[str] = []
        self.samples: list[dict[str, Any]] = []
        self.storage_abort: dict[str, int] | None = None
        self.last_lock_max = 0
        self.forwards: dict[str, tuple[ManagedProcess, str]] = {}
        self.directory = Path(context["artifact_dir"]) / "capacity"
        self.directory.mkdir(parents=True, exist_ok=True)
        _runtime_mysql(
            context,
            "UPDATE performance_schema.setup_consumers SET ENABLED='YES' WHERE NAME='events_statements_history_long'; UPDATE performance_schema.setup_instruments SET ENABLED='YES',TIMED='YES' WHERE NAME LIKE 'statement/%';",
        )
        _runtime_mysql(context, "SET GLOBAL innodb_monitor_enable='lock_deadlocks';")
        self.baseline = self.mysql_status()

    def mysql_status(self) -> dict[str, int]:
        raw = _runtime_mysql(
            self.context,
            "SHOW GLOBAL STATUS WHERE Variable_name IN ('Threads_connected','Innodb_row_lock_time_max');",
        )
        values = dict(line.split("\t") for line in raw.splitlines())
        if set(values) != {"Threads_connected", "Innodb_row_lock_time_max"}:
            raise ValueError("MySQL status metrics are incomplete")
        deadlocks = json.loads(
            _runtime_mysql(
                self.context,
                "SELECT JSON_OBJECT('count',COUNT,'enabled',STATUS='enabled') FROM information_schema.INNODB_METRICS WHERE NAME='lock_deadlocks';",
            )
        )
        if deadlocks["enabled"] != 1:
            raise ValueError("InnoDB deadlock monitoring is unavailable")
        values["Innodb_deadlocks"] = deadlocks["count"]
        return {key: int(value) for key, value in values.items()}

    def pod_metrics(self) -> dict[str, dict[str, float]]:
        result = {}
        for plane, names in [
            ("runtime", {"runtime-gateway", "workflow-runtime", "workflow-worker", "sandbox-manager", "observability"}),
            ("control", {"platform-control"}),
        ]:
            namespace = self.context[f"{plane}_namespace"]
            pods = run(("kubectl", "-n", namespace, "get", "pods", "-o", "json"), timeout=30).json()["items"]
            for pod in pods:
                name = pod["metadata"]["name"]
                service = pod["metadata"].get("labels", {}).get("app.kubernetes.io/name")
                if (
                    service not in names
                    or pod["metadata"].get("deletionTimestamp")
                    or pod.get("status", {}).get("phase") != "Running"
                    or not pod["status"].get("containerStatuses")
                ):
                    continue
                if not all(item.get("ready") for item in pod["status"].get("containerStatuses", [])):
                    continue
                key = f"{namespace}/{name}"
                if key not in self.forwards:
                    with socket.socket() as listener:
                        listener.bind(("127.0.0.1", 0))
                        port = listener.getsockname()[1]
                    process = start_process(
                        ("kubectl", "-n", namespace, "port-forward", f"pod/{name}", f"{port}:9092"),
                        stdout_path=self.directory / f"{name}-metrics.log",
                        stderr_path=self.directory / f"{name}-metrics-error.log",
                    )
                    self.forwards[key] = process, f"http://127.0.0.1:{port}/metrics"
                process, url = self.forwards[key]
                deadline = time.monotonic() + 10
                while True:
                    if process.process.poll() is not None:
                        del self.forwards[key]
                        process.stop()
                        raise RuntimeError("metrics port-forward closed")
                    try:
                        response = httpx.get(url, timeout=3)
                        response.raise_for_status()
                        metrics = parse_metrics(response.text)
                        if service != "observability" and metrics.get("agentx_mysql_pool_wait_samples", 0) <= 0:
                            if time.monotonic() >= deadline:
                                raise ValueError("MySQL pool has no acquire-wait samples")
                            time.sleep(0.25)
                            continue
                        result[key] = metrics
                        break
                    except httpx.HTTPError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(0.25)
        if not result:
            raise ValueError("no live service metrics were collected")
        return result

    def redis_state(self) -> dict[str, int]:
        info = redis_command(self.context, "INFO", "memory")
        memory = dict(line.split(":", 1) for line in info.splitlines() if ":" in line)
        pending = lag = 0
        cursor = "0"
        keys = set()
        while True:
            cursor, page = redis_command(self.context, "SCAN", cursor, "COUNT", "1000", "TYPE", "stream")
            keys.update(page)
            if cursor == "0":
                break
        for key in sorted(keys):
            groups = redis_command(self.context, "EVAL", UNREAD_STREAM_SNAPSHOT, "1", key)
            for group in groups:
                name, group_pending, unread, cursor_id, native_lag = group
                if any(type(value) is not int or value < 0 for value in (group_pending, unread)):
                    raise ValueError(f"Redis unread-entry snapshot is invalid for {key}")
                with (self.directory / "redis-unread-snapshots.jsonl").open("a", encoding="utf-8") as output:
                    output.write(
                        json.dumps(
                            {
                                "observedAt": time.time(),
                                "stream": key,
                                "group": name,
                                "pending": group_pending,
                                "unread": unread,
                                "lastDeliveredId": cursor_id,
                                "nativeLag": native_lag,
                                "saturated": unread == 2001,
                            }
                        )
                        + "\n"
                    )
                pending += group_pending
                lag += unread
        return {"usedMemoryBytes": int(memory["used_memory"]), "pending": pending, "consumerLag": lag}

    def sample(self) -> dict[str, Any]:
        pods = self.pod_metrics()
        status = self.mysql_status()
        if status["Innodb_row_lock_time_max"] > self.last_lock_max:
            prepared = json.loads(
                _runtime_mysql(
                    self.context,
                    "SELECT JSON_ARRAYAGG(JSON_OBJECT('sql',SQL_TEXT,'executions',COUNT_EXECUTE,'maxDurationMs',MAX_TIMER_EXECUTE/1000000000)) FROM (SELECT SQL_TEXT,COUNT_EXECUTE,MAX_TIMER_EXECUTE FROM performance_schema.prepared_statements_instances ORDER BY MAX_TIMER_EXECUTE DESC LIMIT 20) recent;",
                )
            )
            with (self.directory / "mysql-lock-peaks.jsonl").open("a", encoding="utf-8") as output:
                output.write(
                    json.dumps(
                        {
                            "observedAt": time.time(),
                            "nativeMaxLockWaitMs": status["Innodb_row_lock_time_max"],
                            "preparedStatements": prepared,
                        }
                    )
                    + "\n"
                )
            self.last_lock_max = status["Innodb_row_lock_time_max"]
        history = json.loads(
            _runtime_mysql(
                self.context,
                "SELECT JSON_ARRAYAGG(JSON_OBJECT('durationMs',TIMER_WAIT/1000000000)) FROM performance_schema.events_statements_history_long WHERE TIMER_WAIT IS NOT NULL AND (EVENT_NAME='statement/com/Execute' OR EVENT_NAME LIKE 'statement/sql/%' AND EVENT_NAME NOT IN ('statement/sql/begin','statement/sql/commit','statement/sql/rollback','statement/sql/error'));",
            )
        )
        if not history:
            raise ValueError("MySQL statement timing instrumentation produced no samples")
        values = list(pods.values())
        pool_wait = max(value["agentx_mysql_pool_wait_p95_ms"] for value in values)
        return {
            "observedAt": time.time(),
            "pods": pods,
            "queues": queue_state(self.context),
            "mysql": {
                "connections": status["Threads_connected"],
                "poolWaitP95Ms": pool_wait,
                # A maximum is a conservative upper bound for p95; it
                # does not loosen the 200 ms row-lock threshold.
                "lockWaitP95Ms": status["Innodb_row_lock_time_max"],
                "deadlocks": status["Innodb_deadlocks"] - self.baseline["Innodb_deadlocks"],
                "slowQueryRate": sum(float(item["durationMs"]) > 1000 for item in history) / len(history),
                "statementSamples": len(history),
            },
            "redis": self.redis_state(),
            "provider": {
                "poolUtilization": max(value["agentx_provider_pool_utilization"] for value in values),
                "queueTimeouts": int(
                    _runtime_mysql(
                        self.context,
                        "SELECT COUNT(*) FROM runtime_calls WHERE error_code IN ('PROVIDER_BUSY','PROVIDER_QUEUE_TIMEOUT')",
                    )
                ),
            },
        }

    @contextmanager
    def changing_topology(self):
        # SQL sampling resumes after ready pods have produced at least one
        # pool probe. Pod replacement is outside healthy-load measurements.
        with self.sample_lock:
            yield
            for process, _ in self.forwards.values():
                process.stop()
            self.forwards.clear()
            time.sleep(6)

    def run(self) -> None:
        while not self.stop_event.is_set():
            try:
                with self.sample_lock:
                    storage = run(
                        (
                            "kubectl",
                            "get",
                            "--raw",
                            f"/api/v1/nodes/{self.context['capacity_node']}/proxy/stats/summary",
                        ),
                        timeout=30,
                    ).json()["node"]["fs"]
                    available = storage["availableBytes"]
                    if type(available) is not int or available < 0:
                        raise ValueError("node free storage is unavailable")
                    if available < MINIMUM_FREE_STORAGE_BYTES:
                        self.abort_low_storage(available)
                        raise RuntimeError(f"capacity stopped: only {available} bytes free on the node")
                    sample = self.sample()
                    sample["storage"] = {"availableBytes": available}
                self.samples.append(sample)
                with (self.directory / "measurements.jsonl").open("a", encoding="utf-8") as output:
                    output.write(json.dumps(sample) + "\n")
            except Exception as error:  # collect every acquisition failure; no silent thread death
                message = redact(f"{type(error).__name__}: {error}")
                self.errors.append(message)
                with (self.directory / "collection-errors.jsonl").open("a", encoding="utf-8") as output:
                    output.write(json.dumps({"observedAt": time.time(), "error": message}) + "\n")
            self.stop_event.wait(10)

    def abort_low_storage(self, available: int) -> None:
        namespace = self.context["runtime_namespace"]
        if not namespace.startswith("agentx-e2e-runtime-") or not namespace.endswith(self.context["run_id"]):
            raise ValueError("storage abort can only stop this run's isolated E2E namespace")
        self.storage_abort = {"availableBytes": available, "minimumFreeBytes": MINIMUM_FREE_STORAGE_BYTES}
        try:
            for name in ("workflow-worker", "workflow-runtime"):
                run(("kubectl", "-n", namespace, "scale", f"deployment/{name}", "--replicas=0"), timeout=30)
        finally:
            # Interrupt cluster_load so pytest unwinds without waiting for
            # the invocation deadline, even if a workload cannot be scaled.
            try:
                run(
                    (
                        "kubectl",
                        "-n",
                        namespace,
                        "delete",
                        "pod/capacity-load",
                        "--grace-period=0",
                        "--force",
                        "--wait=false",
                    ),
                    check=False,
                    timeout=30,
                )
            finally:
                self.stop_event.set()

    def close(self) -> dict[str, dict[str, float]]:
        self.stop_event.set()
        self.join(timeout=120)
        for process, _ in self.forwards.values():
            process.stop()
        if self.is_alive() or self.errors or not self.samples:
            raise ValueError(f"capacity measurements are incomplete: {self.errors[:10]}")
        groups: dict[str, dict[str, float]] = {}
        for sample in self.samples:
            for name in ("queues", "mysql", "redis", "provider"):
                target = groups.setdefault(name, {})
                for key, value in sample[name].items():
                    target[key] = max(target.get(key, 0), value)
        groups["queues"].pop("pending", None)
        return groups
