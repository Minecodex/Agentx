"""Configured GLM connection shared by real-model system tests; secrets stay in memory."""

from __future__ import annotations

import base64
import json
import socket
import ssl
import time
from collections.abc import Iterator
from dataclasses import dataclass, field

import httpx
import pytest

from tests.e2e.support import ROOT, run, start_process


@dataclass
class ExistingSecret:
    value: str = field(default="", repr=False)


@pytest.fixture(scope="session")
def existing_model() -> dict:
    query = (
        "SELECT JSON_OBJECT('alias',a.alias,'endpoint',d.endpoint,'modelName',d.model_name,"
        "'providerType',d.provider_type,'defaultParameters',d.default_parameters,"
        "'maxInputTokens',d.max_input_tokens,'maxOutputTokens',d.max_output_tokens,"
        "'secretRef',v.secret_ref,'secretVersion',v.provider_version,'inputPrice',CAST(p.input_per_million AS CHAR),"
        "'outputPrice',CAST(p.output_per_million AS CHAR),'currency',p.currency) "
        "FROM model_aliases a JOIN model_deployments d ON d.id=a.deployment_id "
        "JOIN credentials c ON c.id=d.credential_id JOIN credential_secret_versions v ON v.credential_id=c.id "
        "AND v.version_number=c.current_secret_version JOIN model_price_versions p ON p.deployment_id=d.id "
        "AND p.version_number=(SELECT MAX(p2.version_number) FROM model_price_versions p2 WHERE p2.deployment_id=d.id) "
        "WHERE a.alias='glm-5.3-flash' AND a.status='active' AND d.status='active' AND c.status='active'"
    )
    deployment = run(
        ("kubectl", "-n", "agentx-prod-control", "get", "deployment/platform-control", "-o", "json")
    ).json()
    environment = deployment["spec"]["template"]["spec"]["containers"][0]["env"]
    database = next(item["value"] for item in environment if item["name"] == "AGENTX_CONTROL_MYSQL_DATABASE")
    result = run(
        (
            "kubectl",
            "-n",
            "agentx-prod-external",
            "exec",
            "control-mysql-0",
            "-c",
            "mysql",
            "--",
            "sh",
            "-c",
            'MYSQL_PWD="$MYSQL_ROOT_PASSWORD" mysql --user=root --database="$2" --default-character-set=utf8mb4 --batch --raw --skip-column-names --execute "$1"',
            "plan8-read-model-metadata",
            query,
            database,
        )
    )
    assert result.stdout.strip(), "The configured glm-5.3-flash is unavailable"
    return json.loads(result.stdout)


@pytest.fixture(scope="session")
def existing_model_secret(run_id: str, existing_model: dict) -> Iterator[ExistingSecret]:
    directory = ROOT / ".local/artifacts/e2e" / run_id / "model-source"
    directory.mkdir(parents=True, exist_ok=True)
    control_secret = run(
        ("kubectl", "-n", "agentx-prod-control", "get", "secret", "agentx-control-secrets", "-o", "json")
    ).json()["data"]
    token = base64.b64decode(control_secret["AGENTX_CONTROL_VAULT_TOKEN"]).decode()
    ca_data = run(
        ("kubectl", "-n", "agentx-prod-control", "get", "secret", "agentx-prod-vault-ca", "-o", "json")
    ).json()["data"]
    ca = directory / "vault-ca.pem"
    ca.write_bytes(base64.b64decode(ca_data["ca.crt"]))
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    forward = start_process(
        ("kubectl", "-n", "agentx-prod-external", "port-forward", "service/vault", f"{port}:8200"),
        stdout_path=directory / "vault-forward.log",
        stderr_path=directory / "vault-forward-error.log",
    )
    secret = ExistingSecret()
    try:
        with httpx.Client(verify=ssl.create_default_context(cafile=str(ca)), trust_env=False, timeout=10) as vault:
            deadline = time.monotonic() + 30
            while True:
                try:
                    response = vault.get(
                        f"https://127.0.0.1:{port}/v1/agentx-v2/data/{existing_model['secretRef']}",
                        params={"version": existing_model["secretVersion"]},
                        headers={"X-Vault-Token": token},
                        extensions={"sni_hostname": "vault.agentx-prod-external.svc"},
                    )
                    break
                except httpx.ConnectError:
                    if time.monotonic() > deadline:
                        raise RuntimeError("Source Vault forwarding did not become ready") from None
                    time.sleep(0.3)
            if response.status_code != 200:
                raise RuntimeError(f"Source Vault read failed: HTTP {response.status_code}")
            secret = ExistingSecret(response.json()["data"]["data"]["value"])
        yield secret
    finally:
        secret.value = ""
        token = ""
        forward.stop()
