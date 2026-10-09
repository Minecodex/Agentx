"""Run the existing Python load generator inside the isolated cluster."""

from __future__ import annotations

import base64
import hashlib
import importlib.metadata
import io
import json
import time
import zipfile
from contextlib import contextmanager
from pathlib import Path

from packaging.requirements import Requirement

from tests.e2e.capacity.loadgen import LoadReport, LoadSample
from tests.e2e.support import run
from tools.scripts.release.evidence import write_report


def dependency_archive() -> tuple[bytes, dict[str, str]]:
    """Reuse the frozen uv environment; no second installer or dependency set."""
    pending = ["httpx"]
    packages = {}
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        while pending:
            name = pending.pop()
            distribution = importlib.metadata.distribution(name)
            name = distribution.metadata["Name"]
            if name in packages:
                continue
            packages[name] = distribution.version
            for requirement in distribution.requires or []:
                requirement = Requirement(requirement)
                if requirement.marker is None or requirement.marker.evaluate({"extra": ""}):
                    pending.append(requirement.name)
            for entry in sorted(distribution.files or []):
                if ".." in entry.parts or entry.suffix == ".pyc":
                    continue
                if entry.suffix in {".so", ".pyd", ".dylib"}:
                    raise ValueError(f"capacity dependency {name} contains a native extension")
                source = Path(distribution.locate_file(entry))
                if source.is_file():
                    info = zipfile.ZipInfo(str(entry), date_time=(1980, 1, 1, 0, 0, 0))
                    info.compress_type = zipfile.ZIP_DEFLATED
                    archive.writestr(info, source.read_bytes())
    payload = output.getvalue()
    if len(payload) > 900_000:
        raise ValueError("capacity dependencies exceed the ConfigMap budget")
    return payload, packages


@contextmanager
def load_runner(context):
    namespace = context["runtime_namespace"]
    directory = Path(context["artifact_dir"]) / "capacity"
    directory.mkdir(parents=True, exist_ok=True)
    dependencies, versions = dependency_archive()
    (directory / "dependencies.zip").write_bytes(dependencies)
    source = Path(__file__).with_name("loadgen.py").read_text()
    runner_label = {"agentx.io/capacity-load": "true"}
    gateway_label = {"app.kubernetes.io/name": "runtime-gateway"}
    documents = [
        {
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {"name": "capacity-load", "namespace": namespace},
            "data": {"loadgen.py": source},
            "binaryData": {"dependencies.zip": base64.b64encode(dependencies).decode()},
        },
        {
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {"name": "capacity-load-egress", "namespace": namespace},
            "spec": {
                "podSelector": {"matchLabels": runner_label},
                "policyTypes": ["Egress"],
                "egress": [{"to": [{"podSelector": {"matchLabels": gateway_label}}], "ports": [{"port": 8080}]}],
            },
        },
        {
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {"name": "capacity-load-ingress", "namespace": namespace},
            "spec": {
                "podSelector": {"matchLabels": gateway_label},
                "policyTypes": ["Ingress"],
                "ingress": [{"from": [{"podSelector": {"matchLabels": runner_label}}], "ports": [{"port": 8080}]}],
            },
        },
        {
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {"name": "capacity-load", "namespace": namespace, "labels": runner_label},
            "spec": {
                "restartPolicy": "Never",
                "automountServiceAccountToken": False,
                "securityContext": {"runAsUser": 65532, "runAsGroup": 65532, "fsGroup": 65532, "runAsNonRoot": True},
                "containers": [
                    {
                        "name": "load",
                        "image": "python:3.12-slim",
                        "imagePullPolicy": "IfNotPresent",
                        "command": ["python", "-c", "import httpx, time; time.sleep(14400)"],
                        "env": [
                            {"name": "PYTHONPATH", "value": "/capacity/dependencies.zip"},
                            {"name": "TMPDIR", "value": "/scratch"},
                        ],
                        "resources": {
                            "requests": {"cpu": "100m", "memory": "64Mi"},
                            "limits": {"cpu": "1", "memory": "256Mi"},
                        },
                        "securityContext": {
                            "allowPrivilegeEscalation": False,
                            "readOnlyRootFilesystem": True,
                            "capabilities": {"drop": ["ALL"]},
                            "seccompProfile": {"type": "RuntimeDefault"},
                        },
                        "volumeMounts": [
                            {"name": "code", "mountPath": "/capacity", "readOnly": True},
                            {"name": "tmp", "mountPath": "/scratch"},
                        ],
                    }
                ],
                "volumes": [{"name": "code", "configMap": {"name": "capacity-load"}}, {"name": "tmp", "emptyDir": {}}],
            },
        },
    ]
    try:
        run(
            ("kubectl", "apply", "--server-side", "--field-manager=agentx-capacity", "-f", "-"),
            input_text=json.dumps({"apiVersion": "v1", "kind": "List", "items": documents}),
            timeout=60,
        )
        run(
            ("kubectl", "-n", namespace, "wait", "--for=condition=Ready", "pod/capacity-load", "--timeout=120s"),
            timeout=150,
        )
        write_report(
            context,
            "capacity/load-generator.json",
            {
                "mode": "in-cluster-http",
                "image": "python:3.12-slim",
                "dependencies": versions,
                "dependencyArchiveSha256": hashlib.sha256(dependencies).hexdigest(),
                "uvLockSha256": hashlib.sha256((Path(context["root"]) / "uv.lock").read_bytes()).hexdigest(),
                "loadgenSha256": hashlib.sha256(source.encode()).hexdigest(),
            },
        )
        yield
    finally:
        result = run(("kubectl", "-n", namespace, "logs", "pod/capacity-load"), check=False, timeout=30)
        (directory / "load-generator.log").write_text(result.stdout)
        for kind, name in (
            ("pod", "capacity-load"),
            ("configmap", "capacity-load"),
            ("networkpolicy", "capacity-load-egress"),
            ("networkpolicy", "capacity-load-ingress"),
        ):
            run(
                ("kubectl", "-n", namespace, "delete", f"{kind}/{name}", "--ignore-not-found=true", "--wait=true"),
                check=False,
                timeout=90,
            )


def cluster_load(context, pods, slug, api_keys, concurrent, duration, *, count=None, rate=None):
    targets = [f"http://{pod['status']['podIP']}:8080" for pod in pods]
    parameters = {
        "base_urls": targets,
        "slug": slug,
        "api_keys": api_keys,
        "concurrent": concurrent,
        "duration_seconds": duration,
        "max_requests": count,
        "requests_per_second": rate,
    }
    directory = Path(context["artifact_dir"]) / "capacity"
    with (directory / "gateway-targets.jsonl").open("a", encoding="utf-8") as output:
        output.write(
            json.dumps(
                {
                    "observedAt": time.time(),
                    "pods": [{"name": pod["metadata"]["name"], "ip": pod["status"]["podIP"]} for pod in pods],
                    "concurrency": concurrent,
                }
            )
            + "\n"
        )
    result = run(
        (
            "kubectl",
            "-n",
            context["runtime_namespace"],
            "exec",
            "-i",
            "pod/capacity-load",
            "--",
            "python",
            "/capacity/loadgen.py",
        ),
        input_text=json.dumps(parameters),
        timeout=duration + 180,
    ).json()
    return LoadReport(
        [LoadSample(**sample) for sample in result["samples"]],
        result["duration_seconds"],
        result["connection_setup_ms"],
        transport="in-cluster-http",
    )
