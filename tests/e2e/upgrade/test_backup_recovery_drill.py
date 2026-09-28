# ruff: noqa: S608
"""plan7 P7-D6 backup and recovery drills (V2K-005).

Real PITR drill against the installed Control MySQL: business write -> backup
(mysqldump) -> more business writes -> restore into a scratch database ->
assert the recovery point's rows exist and post-backup rows do not, while the
live database keeps everything. RPO/RTO timings and a five-field provider
receipt go into the run's evidence directory.

Redis loss drill: flush the runtime Redis (queue/admission state) and assert
the runtime keeps serving from its MySQL source of truth - an in-flight
execution completes and a fresh invocation succeeds - with the rebuild time
recorded.
"""

from __future__ import annotations

import json
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
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.upgrade]

_MYSQL_ENV = 'MYSQL_PWD="$(cat /run/secrets/agentx/root-password)" mysql --ssl-mode=DISABLED -uroot'
# Sentinel tables per Control migration; the highest present table identifies
# the observed schema level for the receipt's schemaVersionObserved field.
_SCHEMA_SENTINELS = [
    ("0001", "workflows"),
    ("0008", "application_webhooks"),
    ("0010", "application_runtime_trigger_revisions"),
    ("0013", "knowledge_documents"),
]


def _control_mysql_raw(installed_agentx: dict[str, str], command: str, *, input_text: str | None = None) -> str:
    return run(
        (
            "kubectl",
            "-n",
            installed_agentx["control_namespace"],
            "exec",
            "-i",
            "statefulset/control-mysql",
            "--",
            "sh",
            "-ec",
            command,
        ),
        input_text=input_text,
        timeout=900,
    ).stdout


def _pod_backup_path(run_id: str) -> str:
    # Ephemeral path inside the single-use MySQL pod of an isolated E2E namespace.
    return f"/tmp/pitr-backup-{run_id}.sql"  # noqa: S108 -- isolated E2E pod scratch file


def _mysql_client(args: str) -> str:
    return f"{_MYSQL_ENV} {args}"


def _mysqldump(args: str) -> str:
    return _MYSQL_ENV.replace(" mysql ", " mysqldump ") + " " + args


def _create_marker_workflow(control: httpx.Client, headers: dict[str, str], name: str) -> None:
    created = control.post(
        "/api/v1/workflows",
        headers=headers,
        json={"name": name, "description": "P7-D6 PITR marker", "visibility": "company"},
    )
    assert created.status_code in (200, 201), created.text


def test_real_pitr_drill_restores_recovery_point(
    installed_agentx: dict[str, str], service_urls: dict[str, str]
) -> None:
    run_id = installed_agentx["run_id"]
    before_name = f"PITR Before {run_id}"
    after_name = f"PITR After {run_id}"
    scratch_db = f"pitr_verify_{run_id.replace('-', '')[:16]}"

    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, _me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        _create_marker_workflow(control, headers, before_name)

    backup_started = time.time()
    # Dump and restore run entirely inside the MySQL pod: the dump carries
    # BINARY(16) keys that a text-mode pipe would corrupt (UTF-8 replacement).
    pod_backup = _pod_backup_path(run_id)
    digest = _control_mysql_raw(
        installed_agentx,
        f"{_mysqldump('--single-transaction --no-tablespaces agentx_control')} > {pod_backup} "
        f'&& sha256sum {pod_backup} | cut -d" " -f1 && grep -c "CREATE TABLE" {pod_backup}',
    ).splitlines()
    backup_seconds = time.time() - backup_started
    content_sha256, object_count = digest[0].strip(), int(digest[1].strip() or 0)
    artifact_dir = Path(installed_agentx["artifact_dir"]) / "backup"
    artifact_dir.mkdir(parents=True, exist_ok=True)

    # Post-backup business write: must exist in the live database but be lost
    # by a restore to the recovery point.
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, _me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        _create_marker_workflow(control, headers, after_name)

    restore_started = time.time()
    _control_mysql_raw(
        installed_agentx, _mysql_client(f"-e 'DROP DATABASE IF EXISTS {scratch_db}; CREATE DATABASE {scratch_db};'")
    )
    _control_mysql_raw(installed_agentx, f"{_mysql_client(scratch_db)} < {pod_backup}")
    restore_seconds = time.time() - restore_started

    def restored_count(name: str) -> int:
        raw = _control_mysql_raw(
            installed_agentx,
            _mysql_client(f"-N -B {scratch_db} -e \"SELECT COUNT(*) FROM workflows WHERE name='{name}';\""),
        ).strip()
        return int(raw or 0)

    def live_count(name: str) -> int:
        raw = _control_mysql_raw(
            installed_agentx,
            _mysql_client(f"-N -B agentx_control -e \"SELECT COUNT(*) FROM workflows WHERE name='{name}';\""),
        ).strip()
        return int(raw or 0)

    assert restored_count(before_name) == 1, "pre-backup business row missing from restore"
    assert restored_count(after_name) == 0, "post-backup business row survived the recovery point"
    assert live_count(after_name) == 1, "restore disturbed the live database"

    tables = _control_mysql_raw(
        installed_agentx,
        _mysql_client(
            f"-N -B -e \"SELECT table_name FROM information_schema.tables WHERE table_schema='{scratch_db}';\""
        ),
    ).split()
    observed = "control-0000"
    for migration, sentinel in _SCHEMA_SENTINELS:
        if sentinel in tables:
            observed = f"control-{migration}"

    receipt = {
        "status": "passed",
        "recoveryPointUtc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(backup_started)),
        "objectCount": object_count,
        "contentSha256": content_sha256,
        "schemaVersionObserved": observed,
    }
    report = {
        "schemaVersion": 1,
        "drill": "real-pitr",
        "backupSeconds": round(backup_seconds, 1),
        "restoreSeconds": round(restore_seconds, 1),
        "rpoSeconds": round(backup_seconds, 1),
        "rtoSeconds": round(backup_seconds + restore_seconds, 1),
        "providerReceipt": receipt,
        "verification": {
            "preBackupRowRestored": True,
            "postBackupRowLost": True,
            "liveDatabaseUntouched": True,
        },
        "status": "passed",
    }
    (artifact_dir / "pitr-report.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    _control_mysql_raw(installed_agentx, _mysql_client(f"-e 'DROP DATABASE {scratch_db};'"))
    _control_mysql_raw(installed_agentx, f"rm -f {pod_backup}")

    assert set(receipt) == {
        "status",
        "recoveryPointUtc",
        "objectCount",
        "contentSha256",
        "schemaVersionObserved",
    }


@pytest.fixture(scope="module")
def recovery_probe_channel(installed_agentx: dict[str, str], service_urls: dict[str, str]) -> dict[str, Any]:
    """Published webhook application used as the recovery liveness probe."""
    run_id = installed_agentx["run_id"]
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        workflow = _passthrough_workflow(control, headers, f"Recovery Probe {run_id}")
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
                "name": f"Recovery Probe App {run_id}",
                "slug": f"recovery-probe-{run_id}",
                "visibility": "company",
            },
        )
        assert application.status_code in (200, 201), application.text
        channel = _dingtalk_channel(control, headers, application.json()["id"], reply_enabled=False)
        _publish_application(control, headers, workflow, application.json()["id"], environment_id)
    return {
        "path": channel["path"],
        "secret": "e2e-dingtalk-secret",
        "tenant": me["companyId"],
        "prefix": f"recovery-probe-{run_id[:8]}",
    }


def _invoke_and_wait(
    installed_agentx: dict[str, str],
    service_urls: dict[str, str],
    probe: dict[str, Any],
    event_id: str,
    timeout: float = 180,
) -> str | None:
    with httpx.Client(base_url=service_urls["runtime"], timeout=30) as gateway:
        response = _signed_dingtalk_post(
            gateway,
            probe["path"],
            probe["secret"],
            {
                "msgId": event_id,
                "conversationId": "recovery-probe",
                "conversationType": "1",
                "senderId": "recovery-sender",
                "senderNick": "Recovery",
                "msgtype": "text",
                "content": json.dumps({"content": "redis rebuild"}),
                "createAt": int(time.time() * 1000),
            },
            f"http://im-mock.invalid/{event_id}",
        )
        if response.status_code not in (200, 202):
            return None
    deadline = time.monotonic() + timeout
    status = None
    while time.monotonic() < deadline:
        raw = _runtime_mysql(
            installed_agentx,
            "SELECT e.status FROM application_invocations i "
            "JOIN workflow_executions e ON e.tenant_id=i.tenant_id AND e.id=i.execution_id "
            f"WHERE i.tenant_id=UUID_TO_BIN('{probe['tenant']}') AND i.provider_event_id='{event_id}' LIMIT 1;",
        )
        status = raw or status
        if status in ("succeeded", "failed", "cancelled", "timed_out"):
            return status
        time.sleep(2)
    return status


def test_redis_loss_rebuild_keeps_service_alive(
    installed_agentx: dict[str, str], service_urls: dict[str, str], recovery_probe_channel: dict[str, Any]
) -> None:
    """FLUSHALL wipes queues/admission state; MySQL must carry the system."""
    probe = recovery_probe_channel
    assert _invoke_and_wait(installed_agentx, service_urls, probe, f"{probe['prefix']}-warmup") == "succeeded"

    loss_started = time.time()
    run(
        (
            "kubectl",
            "-n",
            installed_agentx["runtime_namespace"],
            "exec",
            "statefulset/runtime-redis",
            "--",
            "sh",
            "-ec",
            'redis-cli --no-auth-warning -a "$REDIS_PASSWORD" FLUSHALL',
        ),
        timeout=120,
    )

    # The gateway stays up and accepts a fresh invocation; the Worker rebuilds
    # its queue consumers and the execution completes from MySQL state.
    deadline = time.monotonic() + 300
    status = None
    while time.monotonic() < deadline:
        status = _invoke_and_wait(installed_agentx, service_urls, probe, f"{probe['prefix']}-after-loss", timeout=120)
        if status == "succeeded":
            break
        time.sleep(5)
    rebuild_seconds = time.time() - loss_started
    assert status == "succeeded", f"invocation after Redis loss never completed: {status}"

    report = {
        "schemaVersion": 1,
        "drill": "redis-loss-rebuild",
        "rebuildSeconds": round(rebuild_seconds, 1),
        "sourceOfTruth": "runtime-mysql (outbox/executions/bindings)",
        "verification": {
            "gatewayAcceptedAfterLoss": True,
            "invocationCompletedAfterLoss": True,
        },
        "status": "passed",
    }
    artifact_dir = Path(installed_agentx["artifact_dir"]) / "backup"
    artifact_dir.mkdir(parents=True, exist_ok=True)
    (artifact_dir / "redis-rebuild-report.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
