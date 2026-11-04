"""Explicit external OpenSandbox TLS configuration for an owned E2E environment."""

import hashlib
import json
import os
from pathlib import Path
from urllib.parse import urlparse

from tests.e2e.support import run


def configure_external_sandbox(installed_agentx):
    """Bind an explicitly configured external browser provider in the owned environment."""
    filename = os.environ.get("AGENTX_E2E_DESIGN_BROWSER_PROVIDER")
    if not filename:
        return
    configured = json.loads(Path(filename).read_text())
    assert set(configured) == {"endpoint", "secureAccess", "apiKeyFile", "caFile"}
    assert urlparse(configured["endpoint"]).scheme == "https"
    namespace = installed_agentx["runtime_namespace"]
    assert namespace.startswith("agentx-e2e-"), "Only the owned environment may change"
    key_file = Path(configured["apiKeyFile"])
    assert key_file.stat().st_mode & 0o077 == 0, "Provider credentials must be private"
    certificate = Path(configured["caFile"]).read_text()
    provider_object = "design-browser-provider"
    secret = {
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {"name": provider_object, "namespace": namespace},
        "stringData": {"ca.crt": certificate, "api-key": key_file.read_text().strip()},
    }
    run(("kubectl", "-n", namespace, "apply", "-f", "-"), input_text=json.dumps(secret))
    patch = {
        "spec": {
            "template": {
                "spec": {
                    "volumes": [{"name": provider_object, "secret": {"secretName": provider_object}}],
                    "containers": [
                        {
                            "name": "sandbox-manager",
                            "env": [
                                {"name": "AGENTX_OPENSANDBOX_ENDPOINT", "value": configured["endpoint"]},
                                {
                                    "name": "AGENTX_OPENSANDBOX_SECURE_ACCESS",
                                    "value": str(configured["secureAccess"]).lower(),
                                },
                                {
                                    "name": "AGENTX_OPENSANDBOX_TLS_CA_PATH",
                                    "value": "/var/run/design-browser-provider/ca.crt",
                                },
                                {
                                    "name": "AGENTX_OPENSANDBOX_API_KEY",
                                    "valueFrom": {"secretKeyRef": {"name": provider_object, "key": "api-key"}},
                                },
                            ],
                            "volumeMounts": [
                                {
                                    "name": provider_object,
                                    "mountPath": "/var/run/design-browser-provider",
                                    "readOnly": True,
                                }
                            ],
                        }
                    ],
                }
            }
        }
    }
    run(
        (
            "kubectl",
            "-n",
            namespace,
            "patch",
            "deployment",
            "sandbox-manager",
            "--type=strategic",
            "--patch-file=/dev/stdin",
        ),
        input_text=json.dumps(patch),
    )
    run(("kubectl", "-n", namespace, "rollout", "status", "deployment/sandbox-manager", "--timeout=120s"), timeout=140)
    run(
        (
            "kubectl",
            "-n",
            namespace,
            "exec",
            "deployment/sandbox-manager",
            "--",
            "node",
            "-e",
            "const https=require('node:https'),fs=require('node:fs');"
            "https.get(process.env.AGENTX_OPENSANDBOX_ENDPOINT+'/v1/sandboxes',"
            "{ca:fs.readFileSync(process.env.AGENTX_OPENSANDBOX_TLS_CA_PATH),"
            "headers:{'Open-Sandbox-Api-Key':process.env.AGENTX_OPENSANDBOX_API_KEY},timeout:8000},"
            "r=>{r.resume();if(r.statusCode!==200){console.error('Browser provider status',r.statusCode);process.exit(1)}})"
            ".on('error',()=>{console.error('Browser provider TLS/API preflight failed');process.exit(1)})"
            ".on('timeout',function(){this.destroy()})",
        ),
        timeout=15,
    )
    evidence = {
        "endpoint": configured["endpoint"],
        "secureAccess": configured["secureAccess"],
        "caSha256": hashlib.sha256(certificate.encode()).hexdigest(),
        "temporaryNamespace": namespace,
    }
    (Path(installed_agentx["artifact_dir"]) / "design-browser-provider.json").write_text(json.dumps(evidence, indent=2))
