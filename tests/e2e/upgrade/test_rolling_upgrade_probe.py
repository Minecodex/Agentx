# ruff: noqa: S608
"""plan7 P7-D5 rolling upgrade drills (INT-009).

Two-phase rollout (expand -> rolling upgrade -> contract) with a continuous
invocation probe: while the runtime plane is upgraded via agentxctl, a
background thread keeps issuing signed webhook invocations and asserts every
accepted invocation completes losslessly. The unknown-protocol drill proves a
Worker task stamped with a future worker_protocol_version is re-offered by the
MySQL dispatch backlog but never claimed by the live Worker.

Dual-tag Previous->Candidate mixing needs per-service image overrides that
agentxctl does not support today (see the P7-D4 survey note); that matrix
stays on the long-run handoff list.
"""

from __future__ import annotations

import json
import threading
import time
from pathlib import Path
from typing import Any

import httpx
import pytest

from tests.e2e.product.test_channel_delivery import (
    _access_token,
    _development_environment_id,
    _dingtalk_channel,
    _passthrough_workflow,
    _publish_application,
    _signed_dingtalk_post,
)
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import agentxctl, run

pytestmark = [pytest.mark.cluster, pytest.mark.upgrade]


@pytest.fixture(scope="module")
def probe_channel(installed_agentx: dict[str, str], service_urls: dict[str, str]) -> dict[str, Any]:
    """Published application with an inbound webhook channel as the probe target."""
    run_id = installed_agentx["run_id"]
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        workflow = _passthrough_workflow(control, headers, f"Rolling Probe {run_id}")
        environment_id = _development_environment_id(control, headers)
        deploy = control.post(
            f"/api/v1/workflows/{workflow['workflowId']}/deployments",
            headers=headers,
            json={"environmentId": environment_id, "workflowVersionId": workflow["versionId"]},
        )
        assert deploy.status_code in (200, 201), deploy.text
        application = control.post(
            "/api/v1/applications",
            headers=headers,
            json={
                "workflowId": workflow["workflowId"],
                "name": f"Rolling Probe App {run_id}",
                "slug": f"rolling-probe-{run_id}",
                "visibility": "company",
            },
        )
        assert application.status_code in (200, 201), application.text
        channel = _dingtalk_channel(control, headers, application.json()["id"], reply_enabled=False)
        _publish_application(control, headers, workflow, application.json()["id"], environment_id)
    return {
        "path": channel["path"],
        "publicId": channel["publicId"],
        "secret": "e2e-dingtalk-secret",
        "tenant": me["companyId"],
        "prefix": f"rolling-probe-{run_id[:8]}",
    }


def _execution_status(installed_agentx: dict[str, str], tenant: str, event_id: str) -> str | None:
    raw = _runtime_mysql(
        installed_agentx,
        "SELECT e.status FROM application_invocations i "
        "JOIN workflow_executions e ON e.tenant_id=i.tenant_id AND e.id=i.execution_id "
        f"WHERE i.tenant_id=UUID_TO_BIN('{tenant}') AND i.provider_event_id='{event_id}' LIMIT 1;",
    )
    return raw or None


class ContinuousProbe(threading.Thread):
    """Issues webhook invocations until stopped; accepted ones must complete."""

    daemon = True

    def __init__(self, installed_agentx: dict[str, str], gateway_url: str, probe: dict[str, Any]):
        super().__init__()
        self.installed_agentx = installed_agentx
        self.gateway_url = gateway_url
        self.probe = probe
        self.stop = threading.Event()
        self.records: list[dict[str, Any]] = []
        self.failures: list[dict[str, Any]] = []
        self._seq = 0
        self._lock = threading.Lock()

    def run(self) -> None:
        with httpx.Client(base_url=self.gateway_url, timeout=30) as gateway:
            while not self.stop.is_set():
                with self._lock:
                    self._seq += 1
                    seq = self._seq
                event_id = f"{self.probe['prefix']}-{seq}"
                started = time.monotonic()
                try:
                    accepted = self._post(gateway, event_id)
                except httpx.HTTPError as error:
                    # Pod switches briefly refuse connections; the invocation
                    # was never accepted, so nothing can be lost. Recorded as
                    # a separate availability sample, not a loss.
                    self.records.append({"seq": seq, "kind": "unreachable", "error": repr(error)})
                    self.stop.wait(1.0)
                    continue
                if accepted:
                    status = self._wait_terminal(event_id)
                    record = {
                        "seq": seq,
                        "kind": "invocation",
                        "accepted": True,
                        "finalStatus": status,
                        "latencyMs": round((time.monotonic() - started) * 1000),
                    }
                    self.records.append(record)
                    if status != "succeeded":
                        self.failures.append(record)
                else:
                    record = {"seq": seq, "kind": "invocation", "accepted": False}
                    self.records.append(record)
                    self.failures.append(record)
                self.stop.wait(2.0)

    def _post(self, gateway: httpx.Client, event_id: str) -> bool:
        response = _signed_dingtalk_post(
            gateway,
            self.probe["path"],
            self.probe["secret"],
            {
                "msgId": event_id,
                "conversationId": "rolling-probe",
                "conversationType": "1",
                "senderId": "rolling-sender",
                "senderNick": "Rolling",
                "msgtype": "text",
                "content": json.dumps({"content": "rolling probe"}),
                "createAt": int(time.time() * 1000),
            },
            f"http://im-mock.invalid/{event_id}",
        )
        return response.status_code in (200, 202)

    def _wait_terminal(self, event_id: str) -> str | None:
        # In-flight invocations must always resolve: aborting on stop would
        # record an artificial loss when the probe drains.
        deadline = time.monotonic() + 90
        status: str | None = None
        while time.monotonic() < deadline:
            status = _execution_status(self.installed_agentx, self.probe["tenant"], event_id)
            if status in ("succeeded", "failed", "cancelled", "timed_out"):
                return status
            time.sleep(2)
        return status

    def summary(self) -> dict[str, Any]:
        invocations = [record for record in self.records if record["kind"] == "invocation"]
        latencies = sorted(record["latencyMs"] for record in invocations if record["finalStatus"] == "succeeded")
        return {
            "samples": len(self.records),
            "invocations": len(invocations),
            "succeeded": len(latencies),
            "unreachable": sum(1 for record in self.records if record["kind"] == "unreachable"),
            "p50LatencyMs": latencies[len(latencies) // 2] if latencies else None,
            "maxLatencyMs": latencies[-1] if latencies else None,
            "failures": self.failures,
        }


def _archive(installed_agentx: dict[str, str], name: str, command: tuple[str, ...]) -> Any:
    payload = run(command, timeout=120).stdout
    path = Path(installed_agentx["artifact_dir"]) / "upgrade-rolling" / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(payload, encoding="utf-8")
    return json.loads(payload)


def test_two_phase_rolling_upgrade_with_continuous_probe(
    installed_agentx: dict[str, str], service_urls: dict[str, str], probe_channel: dict[str, Any]
) -> None:
    namespace = installed_agentx["runtime_namespace"]
    deployment = ("deployment", "workflow-runtime")

    def replicas() -> int:
        spec = run(("kubectl", "-n", namespace, "get", *deployment, "-o", "json"), timeout=60).json()
        return int(spec["spec"]["replicas"])

    def scale(count: int) -> None:
        run(("kubectl", "-n", namespace, "scale", *deployment, f"--replicas={count}"), timeout=60)
        run(("kubectl", "-n", namespace, "rollout", "status", *deployment, "--timeout=600s"), timeout=630)

    original = replicas()

    # EXPAND: a second old-version pod joins before anything rolls.
    scale(original + 1)
    _archive(
        installed_agentx,
        "pods-expanded.json",
        ("kubectl", "-n", namespace, "get", "pods", "-o", "json"),
    )

    probe = ContinuousProbe(installed_agentx, service_urls["runtime"], probe_channel)
    probe.start()
    time.sleep(5)  # at least one green baseline sample before anything rolls

    # ROLLING: helm-driven replacement of the runtime plane.
    rolling_started = time.time()
    run(
        (
            agentxctl(),
            "upgrade",
            "--values",
            installed_agentx["values"],
            "--run-id",
            installed_agentx["run_id"],
            "--target",
            "runtime",
            "--output",
            "json",
        ),
        timeout=1800,
    )
    rolling_seconds = time.time() - rolling_started

    # CONTRACT: back to the original replica count.
    contract_started = time.time()
    scale(original)
    contract_seconds = time.time() - contract_started

    probe.stop.set()
    probe.join(timeout=300)
    assert not probe.is_alive(), "continuous probe did not drain"

    summary = probe.summary()
    report = {
        "schemaVersion": 1,
        "phase": "expand->rolling->contract",
        "originalReplicas": original,
        "rollingSeconds": round(rolling_seconds, 1),
        "contractSeconds": round(contract_seconds, 1),
        "probe": summary,
        "status": "passed" if not summary["failures"] else "failed",
    }
    path = Path(installed_agentx["artifact_dir"]) / "upgrade-rolling" / "rolling-upgrade-report.json"
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    _archive(
        installed_agentx,
        "events.json",
        ("kubectl", "-n", namespace, "get", "events", "--sort-by=.lastTimestamp", "-o", "json"),
    )
    _archive(
        installed_agentx,
        "deployment-after.json",
        ("kubectl", "-n", namespace, "get", *deployment, "-o", "json"),
    )

    assert summary["succeeded"] >= 3, f"probe barely ran: {summary}"
    assert not summary["failures"], f"invocations lost during rolling upgrade: {summary['failures']}"


def test_worker_never_claims_future_protocol_attempts(
    installed_agentx: dict[str, str], service_urls: dict[str, str], probe_channel: dict[str, Any]
) -> None:
    """A future worker_protocol_version attempt is re-offered but never claimed."""
    event_id = f"{probe_channel['prefix']}-future-protocol"
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        response = _signed_dingtalk_post(
            gateway,
            probe_channel["path"],
            probe_channel["secret"],
            {
                "msgId": event_id,
                "conversationId": "rolling-probe",
                "conversationType": "1",
                "senderId": "rolling-sender",
                "senderNick": "Rolling",
                "msgtype": "text",
                "content": json.dumps({"content": "future protocol"}),
                "createAt": int(time.time() * 1000),
            },
            f"http://im-mock.invalid/{event_id}",
        )
        assert response.status_code in (200, 202), response.text
    deadline = time.monotonic() + 90
    status = None
    while time.monotonic() < deadline:
        status = _execution_status(installed_agentx, probe_channel["tenant"], event_id)
        if status == "succeeded":
            break
        time.sleep(2)
    assert status == "succeeded", f"baseline probe invocation did not complete: {status}"

    tenant = probe_channel["tenant"]
    attempt_id = _runtime_mysql(
        installed_agentx,
        "SELECT BIN_TO_UUID(a.id) FROM node_attempts a "
        "JOIN application_invocations i ON i.tenant_id=a.tenant_id AND i.execution_id=a.execution_id "
        f"WHERE i.tenant_id=UUID_TO_BIN('{tenant}') AND i.provider_event_id='{event_id}' "
        "ORDER BY a.attempt_number DESC LIMIT 1;",
    )
    assert attempt_id and attempt_id.lower() != "null", "probe attempt row not found"

    # Requeue the finished attempt stamped with a protocol version the live
    # Worker does not speak, and re-arm its dispatch backlog entry so the
    # engine actually offers the task again.
    _runtime_mysql(
        installed_agentx,
        "UPDATE node_attempts SET status='queued',worker_protocol_version=2,lease_token=NULL,"
        "worker_instance_id=NULL,locked_until=NULL,heartbeat_at=NULL,"
        "deadline_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 1 HOUR),fencing_token=fencing_token+1 "
        f"WHERE id=UUID_TO_BIN('{attempt_id}');",
    )
    _runtime_mysql(
        installed_agentx,
        "UPDATE execution_outbox SET status='published',"
        "published_at=DATE_SUB(UTC_TIMESTAMP(6),INTERVAL 10 SECOND) "
        f"WHERE message_type='dispatch_node' AND attempt_id=UUID_TO_BIN('{attempt_id}');",
    )

    def attempt_state() -> str:
        return _runtime_mysql(
            installed_agentx,
            "SELECT CONCAT(status,':',COALESCE(worker_instance_id IS NULL,1),':',"
            "COALESCE(DATE_FORMAT(published_at,'%Y%m%d%H%i%s'),'none')) FROM node_attempts a "
            "LEFT JOIN execution_outbox o ON o.attempt_id=a.id AND o.message_type='dispatch_node' "
            f"WHERE a.id=UUID_TO_BIN('{attempt_id}');",
        )

    baseline = attempt_state()
    offered = False
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        state = attempt_state()
        # published_at moving forward proves the MySQL backlog re-offered the
        # task to the Redis queue; the Worker then refused it at the claim gate.
        if not state.split(":")[2].startswith(baseline.split(":")[2][:14]):
            offered = True
        assert state.startswith("queued:1"), f"future-protocol attempt was claimed: {state}"
        time.sleep(3)
    assert offered, "dispatch backlog never re-offered the future-protocol task"

    logs = run(
        ("kubectl", "-n", installed_agentx["runtime_namespace"], "logs", "deployment/workflow-runtime", "--tail=2000"),
        timeout=120,
    ).stdout
    assert "WORKER_TASK_MISMATCH" in logs, "live Worker never hit the protocol claim gate"

    # Retire the drill row so the backlog stops re-offering it and the Worker
    # retry loop does not spam mismatches until the synthetic deadline.
    _runtime_mysql(
        installed_agentx,
        "UPDATE node_attempts SET status='timed_out',error_code='E2E_FUTURE_PROTOCOL_DRILL',"
        "deadline_at=UTC_TIMESTAMP(6),locked_until=NULL,heartbeat_at=NULL "
        f"WHERE id=UUID_TO_BIN('{attempt_id}');",
    )
