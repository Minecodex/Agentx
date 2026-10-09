"""Vault TLS credential lifecycle and invalid service-token deployment gates."""

from __future__ import annotations

import base64
import json
import os
import subprocess
from pathlib import Path

import httpx
import pytest
import yaml

from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import agentxctl, run, run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.infrastructure]


def _set_token(namespace, secret_name, key, value):
    secret = run(("kubectl", "-n", namespace, "get", "secret", secret_name, "-o", "json")).json()
    secret["data"][key] = value
    # Pass private material through stdin, never through command arguments.
    result = subprocess.run(("kubectl", "replace", "-f", "-"), input=json.dumps(secret), capture_output=True, text=True)
    assert result.returncode == 0, "Vault fault-injection Secret update failed"


def test_tls_credential_creation_and_rotation_ui(installed_agentx, service_urls):
    values = yaml.safe_load(Path(installed_agentx["values"]).read_text())
    assert values["global"]["components"]["secretProvider"]["endpoint"].startswith("https://")
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        _access_token(control)
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
    }
    run_playwright(
        Path(installed_agentx["root"]), "vault-credentials", ("tests/vault-credentials.spec.ts",), environment
    )


@pytest.mark.parametrize("role", ["control", "runtime"])
def test_doctor_rejects_invalid_vault_service_token(installed_agentx, role):
    values = yaml.safe_load(Path(installed_agentx["values"]).read_text())
    namespace = installed_agentx["dependencies_namespace"]
    secret_name = values["global"]["secrets"]["dependencies"]
    key = f"{role.upper()}_VAULT_TOKEN"
    original = run(("kubectl", "-n", namespace, "get", "secret", secret_name, "-o", f"jsonpath={{.data.{key}}}")).stdout
    invalid = base64.b64encode(b"invalid-vault-token-e2e").decode()
    command = (
        agentxctl(),
        "doctor",
        "--values",
        installed_agentx["values"],
        "--run-id",
        installed_agentx["run_id"],
        "--target",
        "dependencies",
        "--output",
        "json",
    )
    try:
        _set_token(namespace, secret_name, key, invalid)
        result = run(command, check=False, timeout=600)
        assert result.returncode != 0, "Doctor must reject invalid Vault application identities"
        pods = run(
            ("kubectl", "-n", namespace, "get", "pods", "-l", "job-name=dependencies-doctor", "-o", "json")
        ).json()["items"]
        container = next(
            item for pod in pods for item in pod["status"]["containerStatuses"] if item["name"] == f"vault-{role}-auth"
        )
        assert container["state"]["terminated"]["exitCode"] == 22
        logs = run(("kubectl", "-n", namespace, "logs", "job/dependencies-doctor", "-c", f"vault-{role}-auth")).stdout
        assert "403" in logs
        assert "invalid-vault-token-e2e" not in logs
    finally:
        _set_token(namespace, secret_name, key, original)
        assert run(command, timeout=600).json()["status"] == "healthy"
