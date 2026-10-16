"""Fault injection watches persisted Builder state; browser actions remain in Playwright."""

from __future__ import annotations

import json
import threading
import time
import uuid
from pathlib import Path

import httpx
import pytest

from tests.e2e.runtime.test_agent_attachments import _control_mysql, _runtime_mysql
from tests.e2e.support import run


def restore_builder_services(context, urls):
    namespace = context["control_namespace"]
    run(("kubectl", "-n", namespace, "scale", "deployment/platform-control", "--replicas=1"), timeout=60)
    for plane, deployment in (("control", "platform-control"), ("runtime", "runtime-gateway")):
        namespace = context[f"{plane}_namespace"]
        run(
            ("kubectl", "-n", namespace, "rollout", "status", f"deployment/{deployment}", "--timeout=180s"), timeout=190
        )
        run(
            (
                "kubectl",
                "-n",
                namespace,
                "wait",
                "--for=jsonpath={.status.replicas}=1",
                f"deployment/{deployment}",
                "--timeout=180s",
            ),
            timeout=190,
        )
        # A successful scale request or rollout can precede the old replica's
        # preStop drain. Finish that lifecycle before the next test sends POSTs.
        pods = run(
            ("kubectl", "-n", namespace, "get", "pods", "-l", f"app.kubernetes.io/name={deployment}", "-o", "json")
        ).json()["items"]
        for pod in pods:
            if pod["metadata"].get("deletionTimestamp"):
                run(
                    (
                        "kubectl",
                        "-n",
                        namespace,
                        "wait",
                        "--for=delete",
                        f"pod/{pod['metadata']['name']}",
                        "--timeout=180s",
                    ),
                    timeout=190,
                )
    # Probe the same public routes used by the next case. The web server's own
    # health endpoint does not check its Control upstream. Never retry login or
    # another business mutation to hide an unfinished fault cleanup.
    for url, expected in ((f"{urls['web']}/api/v1/auth/me", 401), (f"{urls['runtime']}/health/ready", 200)):
        deadline = time.monotonic() + 60
        while True:
            try:
                if httpx.get(url, timeout=3, trust_env=False).status_code == expected:
                    break
            except httpx.HTTPError:
                pass
            if time.monotonic() >= deadline:
                raise AssertionError(f"Builder fault cleanup did not restore {url}")
            time.sleep(0.5)


@pytest.fixture
def studio_faults(installed_agentx, service_urls, request):
    stop = threading.Event()
    events = []
    failures = []
    seen = set()
    capacity_peak = 0

    def watch():
        nonlocal capacity_peak
        try:
            while not stop.wait(0.5):
                active_models = int(
                    _runtime_mysql(
                        installed_agentx,
                        "SELECT COUNT(*) FROM runtime_calls WHERE source_kind='studio_builder' AND status='sent'",
                    )
                )
                capacity_peak = max(capacity_peak, active_models)
                rows = _control_mysql(
                    installed_agentx,
                    "SELECT CONCAT(HEX(r.id),':',IF(r.prompt LIKE '%host_restart%','host','gateway')) FROM studio_builder_runs r JOIN studio_builder_sessions s ON s.id=r.session_id WHERE r.status='waiting' AND (r.prompt LIKE '%host_restart%' OR r.prompt LIKE '%gateway_restart%')",
                )
                for row in rows.splitlines():
                    identity, kind = row.split(":")
                    checked_id = str(uuid.UUID(hex=identity))
                    if identity in seen:
                        continue
                    if kind == "gateway":
                        ready = _runtime_mysql(
                            installed_agentx,
                            f"SELECT COUNT(*) FROM runtime_calls WHERE source_kind='studio_builder' AND status='sent' AND JSON_UNQUOTE(JSON_EXTRACT(studio_target_json,'$.runId'))='{checked_id}'",  # noqa: S608 -- UUID parsed from the isolated database.
                        )
                        if ready != "1":
                            continue
                    seen.add(identity)
                    namespace = installed_agentx["control_namespace" if kind == "host" else "runtime_namespace"]
                    deployment = "platform-control" if kind == "host" else "runtime-gateway"
                    pods = run(
                        (
                            "kubectl",
                            "-n",
                            namespace,
                            "get",
                            "pods",
                            "-l",
                            f"app.kubernetes.io/name={deployment}",
                            "-o",
                            "json",
                        )
                    ).json()["items"]
                    assert len(pods) == 1, pods
                    diagnostics = run(
                        ("kubectl", "-n", namespace, "logs", pods[0]["metadata"]["name"], "--tail=100"), check=False
                    )
                    (Path(installed_agentx["artifact_dir"]) / f"studio-{kind}-before-kill.log").write_text(
                        diagnostics.stdout, encoding="utf-8"
                    )
                    if kind == "host":
                        run(
                            ("kubectl", "-n", namespace, "scale", f"deployment/{deployment}", "--replicas=2"),
                            timeout=60,
                        )
                    run(
                        (
                            "kubectl",
                            "-n",
                            namespace,
                            "delete",
                            "pod",
                            pods[0]["metadata"]["name"],
                            "--grace-period=0",
                            "--force",
                            "--wait=false",
                        ),
                        timeout=60,
                    )
                    events.append({"runId": identity, "kind": kind, "pod": pods[0]["metadata"]["name"]})
        except Exception as error:
            failures.append(str(error))

    worker = threading.Thread(target=watch, name="studio-fault-injector", daemon=True)
    worker.start()
    try:
        yield events
    finally:
        stop.set()
        worker.join(timeout=70)
        path = Path(installed_agentx["artifact_dir"]) / "studio-faults.json"
        path.write_text(
            json.dumps({"events": events, "errors": failures, "maxConcurrentDesignModels": capacity_peak}, indent=2),
            encoding="utf-8",
        )
        restore_builder_services(installed_agentx, service_urls)
        assert not failures, failures
        assert not worker.is_alive(), "The fault injector did not stop"
        expected_peak = getattr(request, "param", 4)
        if expected_peak is not None:
            assert capacity_peak == expected_peak, (
                f"Expected {expected_peak} concurrent model calls, observed {capacity_peak}"
            )
