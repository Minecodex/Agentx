"""Diagnose TLS file access in the actual disposable CI container runtime."""

from __future__ import annotations

import base64
import json
import subprocess
import time
from datetime import UTC, datetime, timedelta
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

NAMESPACE = "mysql-tls-probe"
OUTPUT = Path(".local/artifacts/mysql-tls-probe")


def kubectl(*args: str, data: dict | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["kubectl", "-n", NAMESPACE, *args],
        input=json.dumps(data) if data is not None else None,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )


def material() -> dict[str, str]:
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    now = datetime.now(UTC)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "isolated-mysql-probe")])
    ca = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(name)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - timedelta(minutes=1))
        .not_valid_after(now + timedelta(hours=1))
        .add_extension(x509.BasicConstraints(ca=True, path_length=0), True)
        .sign(key, hashes.SHA256())
    )
    server_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    certificate = (
        x509.CertificateBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")]))
        .issuer_name(name)
        .public_key(server_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - timedelta(minutes=1))
        .not_valid_after(now + timedelta(hours=1))
        .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH, ExtendedKeyUsageOID.CLIENT_AUTH]), False)
        .sign(key, hashes.SHA256())
    )
    return {
        name: base64.b64encode(value).decode()
        for name, value in {
            "ca.crt": ca.public_bytes(serialization.Encoding.PEM),
            "tls.crt": certificate.public_bytes(serialization.Encoding.PEM),
            "tls.key": server_key.private_bytes(
                serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()
            ),
        }.items()
    }


def pod(name: str, tls_path: str, *, runtime_default: bool = False) -> dict:
    security = {
        "runAsNonRoot": True,
        "runAsUser": 999,
        "runAsGroup": 999,
        "fsGroup": 999,
        "seccompProfile": {"type": "RuntimeDefault"},
    }
    if runtime_default:
        security["appArmorProfile"] = {"type": "RuntimeDefault"}
    container_security = {"allowPrivilegeEscalation": False, "capabilities": {"drop": ["ALL"]}}
    return {
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {"name": name, "namespace": NAMESPACE},
        "spec": {
            "automountServiceAccountToken": False,
            "restartPolicy": "Never",
            "securityContext": security,
            "initContainers": [
                {
                    "name": "prepare",
                    "image": "mysql:8.4",
                    "command": ["sh", "-ec"],
                    "args": [
                        "umask 077; for f in ca.crt tls.crt tls.key; do cat /source/$f > /tls/$f; chmod 0600 /tls/$f; done; openssl verify -CAfile /tls/ca.crt /tls/tls.crt"
                    ],
                    "securityContext": container_security,
                    "volumeMounts": [
                        {"name": "source", "mountPath": "/source", "readOnly": True},
                        {"name": "tls", "mountPath": "/tls"},
                    ],
                }
            ],
            "containers": [
                {
                    "name": "mysql",
                    "image": "mysql:8.4",
                    "securityContext": container_security,
                    "args": [
                        f"--ssl-ca={tls_path}/ca.crt",
                        f"--ssl-cert={tls_path}/tls.crt",
                        f"--ssl-key={tls_path}/tls.key",
                        "--require_secure_transport=ON",
                    ],
                    "env": [{"name": "MYSQL_ROOT_PASSWORD", "value": "isolated-probe-password"}],
                    "resources": {"requests": {"cpu": "100m", "memory": "256Mi"}},
                    "volumeMounts": [
                        {"name": "tls", "mountPath": tls_path, "readOnly": True},
                        {"name": "data", "mountPath": "/var/lib/mysql"},
                    ],
                }
            ],
            "volumes": [
                {"name": "source", "secret": {"secretName": "tls", "defaultMode": 0o440}},
                {"name": "tls", "emptyDir": {"medium": "Memory"}},
                {"name": "data", "emptyDir": {}},
            ],
        },
    }


def main() -> None:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    documents = [
        {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE}},
        {"apiVersion": "v1", "kind": "Secret", "metadata": {"name": "tls", "namespace": NAMESPACE}, "data": material()},
    ]
    variants = [
        ("original", "/tls", False),
        ("mysql-config", "/etc/mysql/agentx-tls", False),
        ("runtime-default", "/tls", True),
    ]
    documents.extend(pod(name, path, runtime_default=profile) for name, path, profile in variants)
    applied = kubectl("apply", "-f", "-", data={"apiVersion": "v1", "kind": "List", "items": documents})
    if applied.returncode:
        raise RuntimeError(applied.stderr)
    results = {}
    for name, path, _ in variants:
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            result = kubectl(
                "exec",
                name,
                "-c",
                "mysql",
                "--",
                "mysql",
                "--protocol=TCP",
                "--host=127.0.0.1",
                "--user=root",
                "--password=isolated-probe-password",
                "--ssl-mode=VERIFY_CA",
                f"--ssl-ca={path}/ca.crt",
                "--execute=SHOW SESSION STATUS LIKE 'Ssl_cipher'",
            )
            if result.returncode == 0:
                break
            status = kubectl("get", "pod", name, "-o", "json")
            if status.returncode == 0 and json.loads(status.stdout).get("status", {}).get("phase") == "Failed":
                break
            time.sleep(3)
        results[name] = {"returncode": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
        for label, command in {
            "process": (
                "exec",
                name,
                "-c",
                "mysql",
                "--",
                "sh",
                "-ec",
                f"id; grep -E '^(Uid|Gid):' /proc/1/status; cat /proc/1/attr/current; stat -c '%a %u:%g %n' '{path}' '{path}/ca.crt' '{path}/tls.crt' '{path}/tls.key'",
            ),
            "logs": ("logs", name, "-c", "mysql"),
            "init": ("logs", name, "-c", "prepare"),
            "status": ("get", "pod", name, "-o", "json"),
        }.items():
            evidence = kubectl(*command)
            (OUTPUT / f"{name}-{label}.txt").write_text(evidence.stdout + evidence.stderr)
        print(name, result.returncode, result.stdout, flush=True)
    (OUTPUT / "results.json").write_text(json.dumps(results, indent=2))
    if not any(item["returncode"] == 0 for item in results.values()):
        raise SystemExit("No actual TLS path passed; inspect runtime confinement evidence")


if __name__ == "__main__":
    main()
