"""The same ctl install/upgrade/doctor flow with local TLS dependencies."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
import yaml

from tests.e2e.support import agentxctl, redact, run

pytestmark = [pytest.mark.cluster, pytest.mark.infrastructure]


def _values(context: dict[str, str]) -> dict:
    values = yaml.safe_load(Path(context["values"]).read_text(encoding="utf-8"))
    assert values["global"]["components"]["runtimeMysql"]["tlsMode"] == "verify_identity", (
        "TLS installation acceptance requires --values deploy/values/local-tls.yaml"
    )
    return values


def _ctl(context: dict[str, str], operation: str, target: str = "all") -> tuple[str, ...]:
    return (
        agentxctl(),
        operation,
        "--values",
        context["values"],
        "--run-id",
        context["run_id"],
        "--target",
        target,
        "--output",
        "json",
    )


def _ca(namespace: str, name: str) -> str:
    # Read only the public trust anchor; never collect private Secret data.
    return run(("kubectl", "-n", namespace, "get", "secret", name, "-o", r"jsonpath={.data.ca\.crt}")).stdout


def _set_ca(namespace: str, name: str, value: str) -> None:
    run(
        (
            "kubectl",
            "-n",
            namespace,
            "patch",
            "secret",
            name,
            "--type=merge",
            "-p",
            json.dumps({"data": {"ca.crt": value}}),
        )
    )


def test_tls_doctors_confirm_encryption(tls_agentx: dict[str, str]) -> None:
    values = _values(tls_agentx)
    for plane in ("control", "runtime", "observability"):
        namespace = tls_agentx[f"{plane}_namespace"] if plane != "observability" else tls_agentx["runtime_namespace"]
        receipt = run(("kubectl", "-n", namespace, "logs", f"job/{plane}-doctor", "-c", "doctor")).json()
        assert receipt["databaseTlsEncrypted"] is True
        assert receipt["objectStorageTlsEncrypted"] is True
        assert receipt["leastPrivilegeReady"] is True
        if plane != "observability":
            assert receipt["databaseTlsMode"] == "verify_identity"
        if plane == "runtime":
            assert receipt["redisTlsEncrypted"] is True
    egress = values["global"]["network"]["egressGateway"]["sandboxAccess"]
    certificate = run(
        (
            "kubectl",
            "-n",
            tls_agentx["dependencies_namespace"],
            "get",
            "secret",
            egress["tlsSecretName"],
            "-o",
            r"jsonpath={.data.tls\.crt}",
        )
    ).stdout
    assert certificate and certificate == _ca(tls_agentx["runtime_namespace"], egress["caSecretName"])
    assert json.loads(Path(tls_agentx["artifact_dir"], "install.json").read_text())["status"] == "ready"


def test_ctl_doctor_rejects_wrong_ca_and_recovers(tls_agentx: dict[str, str]) -> None:
    values = _values(tls_agentx)
    namespace = tls_agentx["runtime_namespace"]
    ca_name = values["global"]["components"]["runtimeMysql"]["caSecretName"]
    egress_ca = values["global"]["network"]["egressGateway"]["sandboxAccess"]["caSecretName"]
    original = _ca(namespace, ca_name)
    wrong = _ca(namespace, egress_ca)
    assert original and wrong and original != wrong
    try:
        _set_ca(namespace, ca_name, wrong)
        result = run(_ctl(tls_agentx, "doctor", "runtime"), check=False, timeout=600)
        assert result.returncode != 0
        logs = run(("kubectl", "-n", namespace, "logs", "job/runtime-doctor", "-c", "doctor"), check=False).stdout
        assert any(term in logs.lower() for term in ("certificate", "unknownissuer", "unknown issuer"))
        assert "Could not automatically determine" not in logs
        Path(tls_agentx["artifact_dir"], "tls-doctor-rejection.txt").write_text(
            redact(result.stderr + logs), encoding="utf-8"
        )
    finally:
        _set_ca(namespace, ca_name, original)
        assert run(_ctl(tls_agentx, "doctor", "runtime"), timeout=600).json()["status"] == "healthy"


def test_ctl_upgrade_reuses_tls_and_waits_for_bootstrap(tls_agentx: dict[str, str]) -> None:
    values = _values(tls_agentx)
    namespace = tls_agentx["dependencies_namespace"]
    ca_name = values["global"]["components"]["objectStorage"]["caSecretName"]
    original = _ca(namespace, ca_name)
    result = run(_ctl(tls_agentx, "upgrade", "dependencies"), timeout=3600).json()
    assert result["status"] == "ready"
    assert _ca(namespace, ca_name) == original
    jobs = run(("kubectl", "-n", namespace, "get", "jobs", "-o", "json")).json()["items"]
    for prefix in ("vault-bootstrap-", "object-storage-bootstrap-"):
        latest = max(
            (job for job in jobs if job["metadata"]["name"].startswith(prefix)),
            key=lambda job: int(job["metadata"]["name"].rsplit("-", 1)[-1]),
        )
        assert latest["status"].get("succeeded") == 1, latest["metadata"]["name"]
