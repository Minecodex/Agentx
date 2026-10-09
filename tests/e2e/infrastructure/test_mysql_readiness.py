from __future__ import annotations

import copy
import re
import time
import uuid

import pytest
import yaml

from tests.e2e.diagnostics import workload_logs
from tests.e2e.support import run


@pytest.mark.cluster
@pytest.mark.infrastructure
def test_mysql_readiness_rejects_wrong_credentials(installed_agentx: dict[str, str]) -> None:
    for plane in ("control", "runtime"):
        namespace = installed_agentx[f"{plane}_namespace"]
        pod_name = f"{plane}-mysql-0"
        pod = run(("kubectl", "-n", namespace, "get", "pod", pod_name, "-o", "json")).json()
        mysql = next(container for container in pod["spec"]["containers"] if container["name"] == "mysql")
        probes = {tuple(mysql[kind]["exec"]["command"]) for kind in ("startupProbe", "readinessProbe")}
        for command in probes:
            valid = run(("kubectl", "-n", namespace, "exec", pod_name, "-c", "mysql", "--", *command), check=False)
            assert valid.returncode == 0, valid.stderr
            invalid, count = re.subn(r"\$\(cat /run/secrets/agentx/[^)]+\)", f"Invalid{uuid.uuid4().hex}", command[-1])
            assert count == 1, "the probe must read its password from the mounted Secret"
            rejected = run(
                ("kubectl", "-n", namespace, "exec", pod_name, "-c", "mysql", "--", *command[:-1], invalid),
                check=False,
            )
            assert rejected.returncode != 0, "MySQL probe reported Ready despite invalid credentials"
            assert "Access denied" in rejected.stderr


@pytest.mark.cluster
@pytest.mark.infrastructure
def test_mysql_cold_initialization_uses_the_server_socket(installed_agentx: dict[str, str]) -> None:
    for plane in ("control", "runtime"):
        namespace = installed_agentx[f"{plane}_namespace"]
        statefulset = run(("kubectl", "-n", namespace, "get", "statefulset", f"{plane}-mysql", "-o", "json")).json()
        spec = copy.deepcopy(statefulset["spec"]["template"]["spec"])
        name = f"{plane}-mysql-socket-{uuid.uuid4().hex[:8]}"
        socket = "/var/lib/mysql/socket-regression.sock"
        spec["restartPolicy"] = "Never"
        spec["volumes"].append({"name": "data", "emptyDir": {}})
        mysql = next(container for container in spec["containers"] if container["name"] == "mysql")
        mysql["args"] = [*mysql.get("args", []), f"--socket={socket}"]
        pod = {
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": name,
                "namespace": namespace,
                "labels": {"agentx.io/plane": plane, "app.kubernetes.io/name": name},
            },
            "spec": spec,
        }
        selector = f"app.kubernetes.io/name={name}"
        try:
            run(("kubectl", "apply", "-f", "-"), input_text=yaml.safe_dump(pod))
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                observed = run(("kubectl", "-n", namespace, "get", "pod", name, "-o", "json")).json()
                states = observed.get("status", {}).get("containerStatuses", [])
                assert observed.get("status", {}).get("phase") != "Failed", workload_logs(namespace, selector)
                if states and all(state.get("ready") for state in states):
                    break
                time.sleep(1)
            else:
                pytest.fail(f"MySQL cold initialization timed out\n{workload_logs(namespace, selector)}")
            command = mysql["readinessProbe"]["exec"]["command"]
            query = command[-1].replace("SELECT 1", "SELECT @@socket")
            result = run(("kubectl", "-n", namespace, "exec", name, "--", *command[:-1], query))
            assert socket in result.stdout, result.stdout
        finally:
            run(("kubectl", "-n", namespace, "delete", "pod", name, "--ignore-not-found=true", "--wait=true"))
