"""Explicit frozen environments; workload and latency gates stay shared."""

from __future__ import annotations

from tests.e2e.hosted_cluster import CPU_COUNT, KUBERNETES_VERSION, MEMORY_MIB


def baseline_matches(nodes: list[dict], *, hosted: bool, environment: dict | None = None) -> bool:
    if len(nodes) != 1:
        return False
    capacity = nodes[0]["status"]["capacity"]
    info = nodes[0]["status"]["nodeInfo"]
    if not capacity["memory"].endswith("Ki") or info["kubeletVersion"] != KUBERNETES_VERSION:
        return False
    memory_gib = int(capacity["memory"][:-2]) / 1024**2
    if not hosted:
        return capacity["cpu"] == "10" and abs(memory_gib - 15.6) < 0.1 and info["architecture"] == "arm64"
    if not environment:
        return False
    resources = environment["nodeResources"]
    return (
        capacity["cpu"] == str(CPU_COUNT)
        and info["architecture"] == "amd64"
        and MEMORY_MIB / 1024 - 0.2 <= memory_gib <= 16.5
        and resources["NanoCpus"] == CPU_COUNT * 10**9
        and resources["Memory"] == MEMORY_MIB * 1024**2
        and environment["cpuCount"] == CPU_COUNT
        and environment["memoryMiB"] == MEMORY_MIB
    )
