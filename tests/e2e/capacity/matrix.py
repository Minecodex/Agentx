"""Capacity fixture operations, topology changes and recorded failure drills."""

# ruff: noqa: S608 -- fixture UUIDs in the isolated E2E databases
from __future__ import annotations

import asyncio
import copy
import json
import socket
import time
import uuid
from contextlib import contextmanager
from pathlib import Path

import httpx

from tests.e2e.capacity.loadgen import probe_sse
from tests.e2e.capacity.runner import cluster_load
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run, start_process


def gateway_pods(context):
    namespace = context["runtime_namespace"]
    pods = run(
        ("kubectl", "-n", namespace, "get", "pods", "-l", "app.kubernetes.io/name=runtime-gateway", "-o", "json"),
        timeout=30,
    ).json()["items"]
    ready = sorted(
        (
            pod
            for pod in pods
            if not pod["metadata"].get("deletionTimestamp")
            and pod.get("status", {}).get("phase") == "Running"
            and any(
                condition["type"] == "Ready" and condition["status"] == "True"
                for condition in pod["status"].get("conditions", [])
            )
        ),
        key=lambda pod: pod["metadata"]["name"],
    )
    assert ready, "capacity load requires at least one ready Gateway pod"
    return ready


@contextmanager
def gateway_urls(context):
    directory = Path(context["artifact_dir"]) / "capacity"
    namespace = context["runtime_namespace"]
    processes = []
    bases = []
    try:
        for pod in gateway_pods(context):
            name = pod["metadata"]["name"]
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            process = start_process(
                ("kubectl", "-n", namespace, "port-forward", f"pod/{name}", f"{port}:8080"),
                stdout_path=directory / f"gateway-{name}-{port}.log",
                stderr_path=directory / f"gateway-{name}-{port}-error.log",
            )
            processes.append(process)
            base = f"http://127.0.0.1:{port}"
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                try:
                    if httpx.get(f"{base}/health/ready", timeout=2).status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                time.sleep(0.25)
            else:
                raise TimeoutError(f"capacity Gateway pod {name} did not become ready")
            bases.append(base)
        yield bases
    finally:
        for process in processes:
            process.stop()


def load(context, app, concurrent, *, count=None, duration=300, rate=None):
    return cluster_load(
        context, gateway_pods(context), app["slug"], app["apiKeys"], concurrent, duration, count=count, rate=rate
    )


def require_complete(report, count=None):
    summary = report.summary()
    assert summary["totalRequests"] > 0, summary
    assert summary["acceptedExecutions"] == summary["completedExecutions"], summary
    assert summary["nonSuccessRate"] == 0 and summary["rejected429Rate"] == 0, summary
    assert summary["acceptP95Ms"] <= 500 and summary["acceptP99Ms"] <= 2000, summary
    if count:
        assert summary["completedExecutions"] >= count, summary
    return summary


def scale(context, name, replicas, collector=None):
    if collector:
        with collector.changing_topology():
            scale(context, name, replicas)
        return
    run(
        ("kubectl", "-n", context["runtime_namespace"], "scale", f"deployment/{name}", f"--replicas={replicas}"),
        timeout=60,
    )
    run(
        ("kubectl", "-n", context["runtime_namespace"], "rollout", "status", f"deployment/{name}", "--timeout=300s"),
        timeout=330,
    )


def sse_matrix(context, app, api_keys, collector):
    # A completed invocation only proves replay throughput. Keep this one
    # pending until the live connection gauge witnesses all 200 subscribers.
    scale(context, "workflow-worker", 0, collector)
    try:
        run(
            (
                "kubectl",
                "-n",
                context["runtime_namespace"],
                "wait",
                "--for=delete",
                "pod",
                "-l",
                "app.kubernetes.io/name=workflow-worker",
                "--timeout=120s",
            ),
            timeout=150,
        )
        with gateway_urls(context) as bases, httpx.Client(base_url=bases[0], timeout=30) as gateway:
            accepted = gateway.post(
                f"/gateway/v1/applications/{app['applicationSlug']}/invocations",
                headers={
                    "Authorization": f"Bearer {api_keys[0]}",
                    "Idempotency-Key": f"capacity-live-sse-{context['run_id']}",
                },
                json={"input": {"message": "200 concurrent subscribers"}, "responseMode": "async"},
            )
            assert accepted.status_code == 202, accepted.text

            def observe_and_resume():
                with collector.sample_lock:
                    live = sum(value["agentx_sse_connections"] for value in collector.pod_metrics().values())
                assert live >= 200, {"peakLiveConnections": live}
                scale(context, "workflow-worker", 1, collector)
                return int(live)

            async def on_all_connected():
                return await asyncio.to_thread(observe_and_resume)

            return asyncio.run(probe_sse(bases, api_keys, accepted.json()["id"], on_all_connected=on_all_connected))
    finally:
        scale(context, "workflow-worker", 1, collector)


def case_matrix(control_url, app):
    with httpx.Client(
        base_url=control_url, headers={"Authorization": f"Bearer {app['token']}"}, timeout=180
    ) as control:
        dataset = control.post(
            "/api/v1/datasets", json={"name": f"Capacity cases {app['applicationId']}", "visibility": "company"}
        )
        dataset.raise_for_status()
        dataset_id = dataset.json()["id"]
        content = "\n".join(
            json.dumps(
                {
                    "caseKey": f"capacity-{index}",
                    "name": f"Case {index}",
                    "input": {"message": f"case {index}"},
                    "expectedOutput": {"answer": f"case {index}"},
                }
            )
            for index in range(1000)
        )
        imported = control.post(
            f"/api/v1/datasets/{dataset_id}/import",
            json={"expectedRevision": dataset.json()["revision"], "format": "jsonl", "content": content},
        )
        imported.raise_for_status()
        version = control.post(
            f"/api/v1/datasets/{dataset_id}/versions", json={"expectedRevision": dataset.json()["revision"] + 1}
        )
        version.raise_for_status()
        profile = control.post(
            "/api/v1/evaluation-profiles",
            json={
                "name": f"Capacity exact {dataset_id}",
                "visibility": "company",
                "rules": [
                    {
                        "key": "exact",
                        "name": "Exact",
                        "evaluatorType": "exact",
                        "configuration": {},
                        "weight": "1",
                        "required": True,
                    }
                ],
            },
        )
        profile.raise_for_status()
        run_response = control.post(
            "/api/v1/evaluations",
            json={
                "name": f"Capacity evaluation {dataset_id}",
                "workflowVersionId": app["versionId"],
                "datasetVersionId": version.json()["id"],
                "evaluationProfileVersionId": profile.json()["versionId"],
                "visibility": "company",
                "parameters": {"concurrency": 20, "sampleSize": 1000},
            },
        )
        run_response.raise_for_status()
        evaluation_id = run_response.json()["id"]
        started = control.post(f"/api/v1/evaluations/{evaluation_id}/start")
        started.raise_for_status()
        deadline = time.monotonic() + 1800
        while time.monotonic() < deadline:
            report = control.get(f"/api/v1/evaluations/{evaluation_id}/report")
            report.raise_for_status()
            value = report.json()
            if value["run"]["status"] == "completed":
                assert len(value["results"]) == 1000, value["run"]
                assert all(case["status"] == "completed" for case in value["results"]), value["run"]
                return {
                    "scenario": "1000-cases",
                    "status": "passed",
                    "evaluationId": evaluation_id,
                    "completedCases": 1000,
                }
            assert value["run"]["status"] not in {"failed", "cancelled"}, value["run"]
            time.sleep(2)
        raise TimeoutError("1000-case evaluation did not complete")


def mixed_workers(context, app, previous_image):
    namespace = context["runtime_namespace"]
    deployment = run(("kubectl", "-n", namespace, "get", "deployment/workflow-worker", "-o", "json"), timeout=30).json()
    previous = copy.deepcopy(deployment)
    previous["metadata"] = {"name": "workflow-worker-previous", "namespace": namespace}
    previous.pop("status", None)
    previous["spec"]["replicas"] = 1
    selector = {"app.kubernetes.io/name": "workflow-worker", "agentx.io/capacity-version": "previous"}
    previous["spec"]["selector"]["matchLabels"] = selector
    previous["spec"]["template"]["metadata"]["labels"].update(selector)
    previous["spec"]["template"]["spec"]["containers"][0]["image"] = previous_image
    run(("kubectl", "apply", "-f", "-"), input_text=json.dumps(previous), timeout=60)
    try:
        run(
            ("kubectl", "-n", namespace, "rollout", "status", "deployment/workflow-worker-previous", "--timeout=300s"),
            timeout=330,
        )
        pods = run(
            ("kubectl", "-n", namespace, "get", "pods", "-l", "app.kubernetes.io/name=workflow-worker", "-o", "json"),
            timeout=30,
        ).json()["items"]
        pods = [
            pod
            for pod in pods
            if not pod["metadata"].get("deletionTimestamp")
            and pod["status"].get("phase") == "Running"
            and pod["status"].get("containerStatuses")
            and all(container.get("ready") for container in pod["status"].get("containerStatuses", []))
        ]
        images = {pod["status"]["containerStatuses"][0]["imageID"] for pod in pods}
        assert len(images) == 2, "mixed-version proof requires two distinct image digests"
        report = load(context, app, 20, count=100)
        result = require_complete(report)
        invocation_ids = ",".join(f"UUID_TO_BIN('{uuid.UUID(sample.invocation_id)}')" for sample in report.samples)
        workers = _runtime_mysql(
            context,
            f"SELECT DISTINCT worker_instance_id FROM node_attempts WHERE status='succeeded' AND execution_id IN (SELECT execution_id FROM application_invocations WHERE id IN ({invocation_ids}));",
        ).splitlines()
        worker_pods = {
            pod["metadata"]["uid"]: {
                "name": pod["metadata"]["name"],
                "imageId": pod["status"]["containerStatuses"][0]["imageID"],
            }
            for pod in pods
        }
        observed = {worker_pods[worker]["imageId"] for worker in workers if worker in worker_pods}
        assert observed == images, {"workers": workers, "workerPods": worker_pods}
        return {
            "scenario": "mixed-worker-versions",
            "status": "passed",
            "imageIds": sorted(images),
            "workers": workers,
            "workerPods": worker_pods,
            "load": result,
        }
    finally:
        run(("kubectl", "-n", namespace, "delete", "deployment/workflow-worker-previous", "--wait=true"), timeout=180)


def circuit_recovery(context, app, collector):
    namespace = context["dependencies_namespace"]
    target = {"slug": app["applicationSlug"], "apiKeys": [app["apiKey"]]}
    faults = []
    run(("kubectl", "-n", namespace, "scale", "deployment/echo-mcp", "--replicas=0"), timeout=60)
    try:
        for _ in range(8):
            report = load(context, target, 1, count=1)
            faults.append(report.summary())
            assert report.samples[0].status == 202, report.summary()
            with collector.sample_lock:
                metrics = collector.pod_metrics()
            if any(value["agentx_provider_circuit_open"] > 0 for value in metrics.values()):
                break
        else:
            raise AssertionError("provider transport failures never opened a circuit")
    finally:
        run(("kubectl", "-n", namespace, "scale", "deployment/echo-mcp", "--replicas=1"), timeout=60)
        run(("kubectl", "-n", namespace, "rollout", "status", "deployment/echo-mcp", "--timeout=300s"), timeout=330)
    started = time.monotonic()
    while time.monotonic() - started < 120:
        report = load(context, target, 1, count=1)
        if report.summary()["completedExecutions"] == 1:
            return {
                "circuitRecoverySeconds": time.monotonic() - started,
                "faults": faults,
                "recovered": report.summary(),
            }
        time.sleep(2)
    raise TimeoutError("provider circuit did not recover within the frozen window")
