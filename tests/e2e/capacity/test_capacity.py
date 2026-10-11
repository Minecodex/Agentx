"""Frozen capacity matrix. Short runs are explicitly incomplete release evidence."""

# ruff: noqa: S608 -- fixture UUIDs in the isolated E2E databases
from __future__ import annotations

import json
import time
from itertools import pairwise
from pathlib import Path

import httpx
import pytest
import yaml

from tests.e2e.capacity.baseline import baseline_matches
from tests.e2e.capacity.collector import (
    CapacityCollector,
    redis_command,
    residual_state,
    verify_redis_unread_measurement,
    wait_queues_empty,
)
from tests.e2e.capacity.fixtures import capacity_application, create_caller_keys
from tests.e2e.capacity.loadgen import LoadReport
from tests.e2e.capacity.matrix import (
    case_matrix,
    circuit_recovery,
    gateway_urls,
    load,
    mixed_workers,
    require_complete,
    scale,
    sse_matrix,
)
from tests.e2e.capacity.runner import load_runner
from tests.e2e.capacity.thresholds import capacity_problems
from tests.e2e.product import test_model_streaming as _streaming_tests
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.capacity]
streaming_application = _streaming_tests.streaming_application


@pytest.fixture(scope="module")
def e2e_providers(installed_agentx):
    context = installed_agentx
    namespace = context["dependencies_namespace"]
    fixture = Path(context["root"]) / "deploy/kustomize/e2e-fixtures/runtime-providers"
    rendered = run(("kubectl", "kustomize", fixture), timeout=120).stdout
    documents = [
        document
        for document in yaml.safe_load_all(rendered)
        if document and document["metadata"]["name"] in {"echo-mcp", "echo-node"}
    ]
    values = yaml.safe_load(Path(context["values"]).read_text())
    images = values["global"]["images"]
    if images["registry"] == "agentx" and not images.get("repositoryPrefix"):
        for document in documents:
            if document["kind"] == "Deployment":
                document["spec"]["template"]["spec"]["containers"][0]["image"] = (
                    f"agentx/{document['metadata']['name']}:{images['tag']}"
                )
    run(("kubectl", "-n", namespace, "apply", "-f", "-"), input_text=yaml.safe_dump_all(documents), timeout=120)
    for name in ("echo-mcp", "echo-node"):
        run(("kubectl", "-n", namespace, "rollout", "status", f"deployment/{name}", "--timeout=180s"), timeout=210)
    return {"echo_mcp": f"http://echo-mcp.{namespace}.svc:8090", "echo_node": f"http://echo-node.{namespace}.svc:8080"}


@pytest.fixture(scope="module")
def capacity_runner(installed_agentx):
    with load_runner(installed_agentx):
        yield


def test_frozen_capacity_matrix(installed_agentx, service_urls, streaming_application, capacity_runner, pytestconfig):
    context = installed_agentx
    smoke = pytestconfig.getoption("--capacity-smoke")
    previous = pytestconfig.getoption("--previous-worker-image")
    write_report(context, "capacity/redis-measurement-check.json", verify_redis_unread_measurement(context))
    if not smoke and not previous:
        pytest.fail("full capacity certification requires --previous-worker-image with a distinct compatible image")
    with httpx.Client(
        base_url=service_urls["web"], headers={"Authorization": f"Bearer {streaming_application['token']}"}, timeout=60
    ) as control:
        streaming_keys = [
            streaming_application["apiKey"],
            *create_caller_keys(control, streaming_application["applicationId"], 99),
        ]
    short = capacity_application(context, service_urls["web"], 5)
    large = capacity_application(context, service_urls["web"], 200)
    nodes = run(("kubectl", "get", "nodes", "-o", "json"), timeout=30).json()["items"]
    context["capacity_node"] = nodes[0]["metadata"]["name"]
    capacity = nodes[0]["status"]["capacity"]
    info = nodes[0]["status"]["nodeInfo"]
    hosted = pytestconfig.getoption("--minikube")
    environment = (
        json.loads((Path(context["artifact_dir"]) / "hosted-environment.json").read_text()) if hosted else None
    )
    baseline_verified = baseline_matches(nodes, hosted=hosted, environment=environment)
    if not smoke:
        assert baseline_verified, "capacity certification hardware differs from the frozen baseline"
    collector = CapacityCollector(context)
    collector.start()
    matrix = []
    report = {
        "status": "failed",
        "matrix": matrix,
        "replicasObserved": [],
        "mixedWorkerVersionsVerified": False,
        "stabilityDurationSeconds": 0,
        "mode": "smoke" if smoke else "certification",
    }
    report["baselineVerified"] = baseline_verified
    report["environment"] = {"capacity": capacity, "nodeInfo": info}
    if environment:
        report["environment"]["hostedRunner"] = environment
    all_samples = []
    try:
        for scenario, app, concurrency, count in [
            ("100-executions", short, 100, 100),
            ("500-nodes", short, 100, 125),
            ("200-node-workflow", large, 1, 1),
            ("5000-attempts", short, 100, 1250),
        ]:
            before = json.loads(
                _runtime_mysql(
                    context,
                    f"SELECT JSON_OBJECT('nodes',(SELECT COUNT(*) FROM node_executions n JOIN workflow_executions e ON e.id=n.execution_id WHERE e.workflow_id=UUID_TO_BIN('{app['workflowId']}')),'attempts',(SELECT COUNT(*) FROM node_attempts a JOIN workflow_executions e ON e.id=a.execution_id WHERE e.workflow_id=UUID_TO_BIN('{app['workflowId']}')));",
                )
            )
            load_report = load(context, app, concurrency, count=count, duration=1800)
            row = {"scenario": scenario, "status": "failed", "load": load_report.summary()}
            matrix.append(row)
            summary = require_complete(load_report, count)
            all_samples.extend(load_report.samples)
            observed = json.loads(
                _runtime_mysql(
                    context,
                    f"SELECT JSON_OBJECT('nodes',COUNT(*),'attempts',(SELECT COUNT(*) FROM node_attempts a JOIN workflow_executions e ON e.id=a.execution_id WHERE e.workflow_id=UUID_TO_BIN('{app['workflowId']}'))) FROM node_executions n JOIN workflow_executions e ON e.id=n.execution_id WHERE e.workflow_id=UUID_TO_BIN('{app['workflowId']}');",
                )
            )
            observed["workflowNodes"] = app["nodes"]
            observed["nodes"] -= before["nodes"]
            observed["attempts"] -= before["attempts"]
            if scenario == "500-nodes":
                assert observed["nodes"] >= 500, observed
            if scenario == "5000-attempts":
                assert observed["attempts"] >= 5000, observed
            row.update(status="passed", load=summary, observed=observed)
            write_report(context, "capacity/capacity-report.json", report)
        matrix.append(case_matrix(service_urls["web"], short))
        sse = sse_matrix(context, streaming_application, streaming_keys, collector)
        subscriber_deadline = time.monotonic() + 30
        while time.monotonic() < subscriber_deadline:
            channels = redis_command(context, "PUBSUB", "CHANNELS", "agentx:v2:invocation:wakeup:*")
            with collector.sample_lock:
                live = max(item["agentx_sse_connections"] for item in collector.pod_metrics().values())
            if not channels and live == 0:
                break
            time.sleep(0.5)
        assert not channels and live == 0, {"subscriberChannels": channels, "connections": live}
        sse["subscriberResidual"] = len(channels)
        sse["connectionGaugeAfterDrain"] = live
        report["groups"] = {"sse": sse}
        matrix.append(
            {
                "scenario": "200-sse",
                "status": "passed" if sse["reconnectFailureRate"] <= 0.01 else "failed",
                "measurements": sse,
            }
        )
        circuit = circuit_recovery(context, streaming_application, collector)
        recovery_started = time.monotonic()
        scale(context, "workflow-worker", 0, collector)
        try:
            with gateway_urls(context) as bases, httpx.Client(base_url=bases[0], timeout=30) as gateway:
                accepted = gateway.post(
                    f"/gateway/v1/applications/{short['slug']}/invocations",
                    headers={
                        "Authorization": f"Bearer {short['apiKey']}",
                        "Idempotency-Key": f"capacity-loss-{context['run_id']}",
                    },
                    json={"input": {"message": "redis recovery"}, "responseMode": "async"},
                )
                assert accepted.status_code == 202, accepted.text
            redis_command(context, "FLUSHALL")
        finally:
            scale(context, "workflow-worker", 1, collector)
        rebuild_load = load(context, short, 1, count=1)
        require_complete(rebuild_load, 1)
        recovery_seconds = wait_queues_empty(context)
        redis_seconds = time.monotonic() - recovery_started
        for name in ("runtime-gateway", "workflow-runtime", "workflow-worker", "observability"):
            for replicas in (1, 2, 3, 4):
                scale(context, name, replicas, collector)
                result = require_complete(load(context, short, 20, count=20), 20)
                if name == "runtime-gateway":
                    assert len(result["gatewayTargets"]) == replicas, result
                matrix.append(
                    {
                        "scenario": f"replicas:{name}:{replicas}",
                        "status": "passed",
                        "replicas": replicas,
                        "load": result,
                    }
                )
                report["replicasObserved"].append(replicas)
                write_report(context, "capacity/capacity-report.json", report)
            scale(context, name, 1, collector)
        if previous:
            with collector.changing_topology():
                matrix.append(mixed_workers(context, short, previous))
            report["mixedWorkerVersionsVerified"] = True
        duration = 30 if smoke else 7200
        stability_started = time.time()
        stability = load(context, short, 5, duration=duration, rate=1)
        require_complete(stability)
        all_samples.extend(stability.samples)
        report["stabilityDurationSeconds"] = stability.duration_seconds
        matrix.append(
            {
                "scenario": "2-hour-stability" if not smoke else "short-stability",
                "status": "passed",
                "load": stability.summary(),
            }
        )
        wait_queues_empty(context)
        groups = collector.close()
        observed = sorted(
            sample["observedAt"] for sample in collector.samples if sample["observedAt"] >= stability_started
        )
        report["stabilityMeasurementSamples"] = len(observed)
        points = [stability_started, *observed, time.time()]
        report["stabilityMaxMeasurementGapSeconds"] = max(right - left for left, right in pairwise(points))
        report["groups"].update(groups)
        report["groups"]["gateway"] = LoadReport(all_samples, transport="in-cluster-http").summary()
        report["groups"]["queues"]["recoverySeconds"] = recovery_seconds
        report["groups"]["redis"]["rebuildSeconds"] = redis_seconds
        report["groups"]["provider"]["circuitRecoverySeconds"] = circuit["circuitRecoverySeconds"]
        report["faults"] = circuit["faults"]
        report["groups"]["residual"] = residual_state(context)
        report["replicasObserved"] = sorted(set(report["replicasObserved"]))
        report["problems"] = capacity_problems(report, require_matrix=not smoke)
        report["status"] = (
            "incomplete" if smoke and not report["problems"] else "failed" if report["problems"] else "passed"
        )
        assert not report["problems"], report["problems"]
    finally:
        if collector.is_alive():
            collector.stop_event.set()
            collector.join(timeout=120)
            for process, _ in collector.forwards.values():
                process.stop()
        report["collectionErrors"] = collector.errors
        report["measurementSamples"] = len(collector.samples)
        if collector.storage_abort:
            report["storageAbort"] = collector.storage_abort
        try:
            for name in (
                ()
                if collector.storage_abort
                else ("runtime-gateway", "workflow-runtime", "workflow-worker", "observability")
            ):
                # Namespace teardown follows this finalizer. Restore desired
                # replicas without waiting for a failed workload to be Ready,
                # which would replace the original capacity failure.
                run(
                    ("kubectl", "-n", context["runtime_namespace"], "scale", f"deployment/{name}", "--replicas=1"),
                    timeout=60,
                )
        finally:
            write_report(context, "capacity/capacity-report.json", report)
