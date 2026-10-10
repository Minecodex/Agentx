"""Collect each container independently, including its last crashed instance."""

from __future__ import annotations

import threading
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from pathlib import Path

from tests.e2e.support import redact, run


def workload_logs(namespace: str, selector: str) -> str:
    command = ("kubectl", "-n", namespace, "get", "pods", "-o", "json")
    if selector:
        command += ("-l", selector)
    result = run(command, check=False, timeout=60)
    if result.returncode:
        return redact(result.stdout + result.stderr)
    sections = []
    for pod in result.json().get("items", []):
        name = pod["metadata"]["name"]
        status = pod.get("status", {})
        states = {
            container["name"]: container
            for container in [*status.get("initContainerStatuses", []), *status.get("containerStatuses", [])]
        }
        spec = pod["spec"]
        for container in [*spec.get("initContainers", []), *spec.get("containers", [])]:
            container_name = container["name"]
            state = states.get(container_name, {})
            current = state.get("state", {})
            versions = []
            if "running" in current or "terminated" in current:
                versions.append(False)
            else:
                sections.append(f"[{name}/{container_name}] logs unavailable: {current}\n")
            if state.get("restartCount", 0):
                versions.append(True)
            for previous in versions:
                command = ["kubectl", "-n", namespace, "logs", name, "-c", container_name, "--tail=1000"]
                if previous:
                    command.append("--previous")
                sections.append(f"[{name}/{container_name}{' previous' if previous else ''}]\n")
                try:
                    logs = run(command, check=False, timeout=30)
                    sections.append(logs.stdout + logs.stderr + "\n")
                except Exception as error:
                    sections.append(f"log collection failed: {error}\n")
    return redact("".join(sections))


def crash_logs(namespace: str, collected: set[tuple[str, str, str, int]], stopped: threading.Event) -> str:
    result = run(("kubectl", "-n", namespace, "get", "pods", "-o", "json"), check=False, timeout=5)
    if result.returncode:
        return ""
    sections = []
    for pod in result.json().get("items", []):
        status = pod.get("status", {})
        for container in [*status.get("initContainerStatuses", []), *status.get("containerStatuses", [])]:
            if stopped.is_set():
                return redact("".join(sections))
            current = container.get("state", {}).get("terminated", {})
            previous = container.get("lastState", {}).get("terminated", {})
            failed = current if current.get("exitCode", 0) else previous
            if not failed.get("exitCode", 0):
                continue
            metadata = pod["metadata"]
            key = (namespace, metadata["uid"], container["name"], container.get("restartCount", 0))
            if key in collected:
                continue
            command = ["kubectl", "-n", namespace, "logs", metadata["name"], "-c", container["name"], "--tail=1000"]
            if not current.get("exitCode", 0):
                command.append("--previous")
            logs = run(command, check=False, timeout=5)
            if logs.returncode:
                continue
            collected.add(key)
            sections.append(
                f"[{namespace}/{metadata['name']}/{container['name']} exit={failed['exitCode']}]\n"
                + logs.stdout
                + logs.stderr
                + "\n"
            )
    return redact("".join(sections))


@contextmanager
def installation_diagnostics(namespaces: Sequence[str], artifact_dir: Path) -> Iterator[None]:
    """Save crashes before Helm's atomic rollback removes the failed workload."""
    stopped = threading.Event()

    def collect() -> None:
        collected: set[tuple[str, str, str, int]] = set()
        path = artifact_dir / "installation-crashes.txt"
        while not stopped.is_set():
            for namespace in namespaces:
                if stopped.is_set():
                    return
                try:
                    content = crash_logs(namespace, collected, stopped)
                    if content:
                        with path.open("a", encoding="utf-8") as stream:
                            stream.write(content)
                        print(content, flush=True)
                except Exception as error:
                    with path.open("a", encoding="utf-8") as stream:
                        stream.write(redact(f"diagnostic collection failed: {error}\n"))
            stopped.wait(5)

    thread = threading.Thread(target=collect, name="installation-diagnostics", daemon=True)
    thread.start()
    try:
        yield
    finally:
        stopped.set()
        thread.join(timeout=15)
