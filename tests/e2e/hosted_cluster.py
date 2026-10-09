"""Disposable Linux Minikube environment for GitHub-hosted E2E jobs."""

from __future__ import annotations

import hashlib
import ipaddress
import json
import os
import platform
from collections.abc import Iterator
from pathlib import Path

import httpx
import yaml

from tests.e2e.support import ROOT, redact, run

MINIKUBE_VERSION = "v1.39.0"
MINIKUBE_SHA256 = "099477eaf248bcb5bcea8ce78a2898e93ac01461c35189da1848c3de82ecd22e"
KUBERNETES_VERSION = "v1.36.1"
PROFILE = "agentx-ci"
MEMORY_MIB = 12288
CPU_COUNT = 4


def install_minikube() -> None:
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError("--minikube requires a Linux x86_64 runner")
    directory = ROOT / ".local/tools"
    directory.mkdir(parents=True, exist_ok=True)
    binary = directory / "minikube"
    url = f"https://github.com/kubernetes/minikube/releases/download/{MINIKUBE_VERSION}/minikube-linux-amd64"
    with httpx.Client(follow_redirects=True, timeout=120) as client:
        response = client.get(url)
        response.raise_for_status()
    if hashlib.sha256(response.content).hexdigest() != MINIKUBE_SHA256:
        raise ValueError("Minikube binary checksum mismatch")
    binary.write_bytes(response.content)
    binary.chmod(0o755)
    os.environ["PATH"] = f"{directory}{os.pathsep}{os.environ['PATH']}"


def node_image(image: str) -> str:
    listing = run(("minikube", "-p", PROFILE, "ssh", "--", "sudo", "ctr", "-n", "k8s.io", "images", "ls"))
    matches = [
        fields
        for line in listing.stdout.splitlines()
        if len(fields := line.split()) >= 3 and fields[0] in {image, f"docker.io/{image}"}
    ]
    if len(matches) != 1 or not matches[0][2].startswith("sha256:"):
        raise ValueError(f"imported fixture image digest is missing: {image}")
    return f"{matches[0][0].rsplit(':', 1)[0]}@{matches[0][2]}"


def hosted_values(source: dict, *, gateway: str, subnet: str) -> dict:
    # Values are a run artifact; the repository's versioned defaults remain intact.
    values = json.loads(json.dumps(source))
    global_values = values["global"]
    global_values["ingress"].update(serviceType="NodePort", httpNodePort=31080, httpsNodePort=31443)
    global_values["components"]["sandbox"].update(endpoint=f"http://{gateway}:18080", secureAccess=False)
    egress = global_values["network"]["egressGateway"]
    egress["sandboxAccess"].update(
        mode="nodePort", endpoint=f"https://{PROFILE}:31129", port=31129, sourceCidrs=[subnet]
    )
    egress["allowedPrivateCidrs"] = sorted(set([*egress.get("allowedPrivateCidrs", []), subnet]))
    global_values["network"].setdefault("externalEgress", {})["opensandbox"] = {
        "cidrs": [f"{gateway}/32"],
        "ports": [18080],
    }
    return values


def prepare_images(values: dict) -> None:
    images = values["global"]["images"]
    local = images["registry"] == "agentx" and not images.get("repositoryPrefix")
    selected = ["echo-mcp", "echo-node", "cpu-embedding", "mem0-server"]
    if local:
        selected = [*images["services"], *selected]
    command = ["cargo", "xtask", "images", "--values", "deploy/values/local.yaml"]
    for service in selected:
        command.extend(("--service", service))
    run(command, timeout=7200)
    os.environ["AGENTX_E2E_CPU_EMBEDDING_IMAGE"] = node_image("agentx/cpu-embedding:dev")
    os.environ["AGENTX_E2E_MEM0_IMAGE"] = node_image("agentx/mem0-server:dev")


def minikube_environment(values_path: Path, directory: Path) -> Iterator[Path]:
    install_minikube()
    directory.mkdir(parents=True, exist_ok=True)
    environment = {
        name: os.environ.get(name)
        for name in (
            "AGENTX_E2E_HOST",
            "AGENTX_E2E_DOCKER_NETWORK",
            "AGENTX_E2E_CPU_EMBEDDING_IMAGE",
            "AGENTX_E2E_MEM0_IMAGE",
        )
    }
    owned_network = False
    try:
        run(("docker", "network", "create", "--subnet=192.168.49.0/24", "--gateway=192.168.49.1", PROFILE))
        owned_network = True
        run(
            (
                "minikube",
                "start",
                "-p",
                PROFILE,
                "--driver=docker",
                "--container-runtime=containerd",
                f"--kubernetes-version={KUBERNETES_VERSION}",
                "--cni=calico",
                f"--cpus={CPU_COUNT}",
                f"--memory={MEMORY_MIB}",
                f"--network={PROFILE}",
                "--disk-size=40g",
                "--wait=all",
                "--wait-timeout=10m",
            ),
            timeout=900,
        )
        network = run(("docker", "network", "inspect", PROFILE)).json()[0]
        subnet = network["IPAM"]["Config"][0]["Subnet"]
        gateway = network["IPAM"]["Config"][0]["Gateway"]
        ipaddress.ip_network(subnet)
        ipaddress.ip_address(gateway)
        node_ip = run(("minikube", "-p", PROFILE, "ip")).stdout.strip()
        ipaddress.ip_address(node_ip)
        os.environ["AGENTX_E2E_HOST"] = gateway
        os.environ["AGENTX_E2E_DOCKER_NETWORK"] = PROFILE
        source = yaml.safe_load(values_path.read_text(encoding="utf-8"))
        prepare_images(source)
        path = directory / "hosted-values.yaml"
        path.write_text(yaml.safe_dump(hosted_values(source, gateway=gateway, subnet=subnet)))
        node = run(("kubectl", "get", "nodes", "-o", "json")).json()
        docker = run(("docker", "inspect", PROFILE)).json()[0]
        (directory / "hosted-environment.json").write_text(
            json.dumps(
                {
                    "minikubeVersion": MINIKUBE_VERSION,
                    "kubernetesVersion": KUBERNETES_VERSION,
                    "profile": PROFILE,
                    "cpuCount": CPU_COUNT,
                    "memoryMiB": MEMORY_MIB,
                    "nodeIp": node_ip,
                    "hostGateway": gateway,
                    "dockerSubnet": subnet,
                    "nodeResources": docker["HostConfig"],
                    "nodes": node["items"],
                },
                indent=2,
            )
            + "\n"
        )
        yield path
    finally:
        try:
            for name, command in {
                "nodes": ("kubectl", "get", "nodes", "-o", "wide"),
                "pods": ("kubectl", "get", "pods", "-A", "-o", "wide"),
                "events": ("kubectl", "get", "events", "-A", "--sort-by=.lastTimestamp"),
            }.items():
                try:
                    result = run(command, check=False, timeout=60)
                    content = result.stdout + result.stderr
                except Exception as error:
                    content = f"diagnostic collection failed: {error}"
                (directory / f"hosted-{name}.txt").write_text(redact(content))
        finally:
            try:
                if owned_network:
                    run(("minikube", "delete", "-p", PROFILE), timeout=300)
            finally:
                try:
                    if owned_network:
                        run(("docker", "network", "rm", PROFILE), check=False, timeout=60)
                finally:
                    for name, value in environment.items():
                        if value is None:
                            os.environ.pop(name, None)
                        else:
                            os.environ[name] = value
