from __future__ import annotations

import re
import uuid

import pytest

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
