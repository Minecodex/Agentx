"""TLS cluster ingress to the one existing budget ledger; upstream key stays in that process."""

from __future__ import annotations

import ipaddress
import json
import os
import tempfile
from pathlib import Path
from urllib.parse import urlparse

import pytest
import yaml

from tests.e2e.product.live_model_config import model_configuration
from tests.e2e.product.live_model_relay import certificates
from tests.e2e.support import ROOT, run


@pytest.fixture(scope="session")
def plan9_metered_provider(installed_agentx, plan9_public_origins):
    namespace = installed_agentx["dependencies_namespace"]
    runtime = installed_agentx["runtime_namespace"]
    configured = model_configuration()
    # This checkpoint always reasons. Use its supported low effort to leave room
    # for HTML inside the explicitly bounded 16,000-token acceptance output.
    configured["defaultParameters"].setdefault("reasoning_effort", "low")
    upstream = urlparse(configured["endpoint"])
    assert upstream.hostname in {"127.0.0.1", "localhost"}, "Plan9 requires the explicitly supplied local metered relay"
    upstream_ca = Path(os.environ["AGENTX_E2E_LLM_CA_FILE"]).read_text()
    host = f"plan9-model-relay.{namespace}.svc"
    port = upstream.port or 443
    # All private TLS material is removed even when DNS or the first deployment fails.
    with tempfile.TemporaryDirectory(prefix="plan9-provider-", dir=ROOT / ".local/tmp") as private:
        directory = Path(private)
        directory.chmod(0o700)
        ca, cert, key = certificates(directory, (host,))
        resolved = run(
            (
                "kubectl",
                "-n",
                runtime,
                "exec",
                "deployment/runtime-gateway",
                "--",
                "node",
                "-e",
                "require('node:dns').lookup('host.docker.internal',{family:4},(error,address)=>{if(error)process.exit(1);process.stdout.write(address)})",
            ),
            timeout=30,
        ).stdout.strip()
        address = ipaddress.ip_address(resolved)
        assert address.is_private, "The metered relay must resolve to the isolated local host"
        nginx = f"""events {{}}
    http {{
      access_log off;
      error_log /dev/stderr warn;
      server {{
        listen 8443 ssl;
        ssl_certificate /tls/server.pem;
        ssl_certificate_key /tls/server-key.pem;
        ssl_protocols TLSv1.2 TLSv1.3;
        client_max_body_size 8m;
        location / {{
          proxy_pass https://{address}:{port};
          proxy_ssl_server_name on;
          proxy_ssl_name host.docker.internal;
          proxy_ssl_verify on;
          proxy_ssl_trusted_certificate /tls/upstream-ca.pem;
          proxy_http_version 1.1;
          proxy_set_header Connection "";
          proxy_set_header Accept-Encoding "";
          proxy_buffering off;
          proxy_request_buffering off;
          proxy_read_timeout 300s;
        }}
      }}
    }}
    """
        peer = {
            "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": runtime}},
            "podSelector": {
                "matchExpressions": [
                    {
                        "key": "app.kubernetes.io/name",
                        "operator": "In",
                        "values": ["runtime-gateway", "workflow-worker"],
                    }
                ]
            },
        }
        documents = [
            {
                "apiVersion": "v1",
                "kind": "Secret",
                "metadata": {"name": "plan9-model-tls"},
                "stringData": {
                    "server.pem": cert.read_text(),
                    "server-key.pem": key.read_text(),
                    "upstream-ca.pem": upstream_ca,
                },
            },
            {
                "apiVersion": "v1",
                "kind": "ConfigMap",
                "metadata": {"name": "plan9-model-proxy"},
                "data": {"nginx.conf": nginx},
            },
            {
                "apiVersion": "v1",
                "kind": "Service",
                "metadata": {"name": "plan9-model-relay"},
                "spec": {
                    "selector": {"app.kubernetes.io/name": "plan9-model-relay"},
                    "ports": [{"name": "https", "port": 8443, "targetPort": 8443}],
                },
            },
            {
                "apiVersion": "apps/v1",
                "kind": "Deployment",
                "metadata": {"name": "plan9-model-relay"},
                "spec": {
                    "replicas": 1,
                    "selector": {"matchLabels": {"app.kubernetes.io/name": "plan9-model-relay"}},
                    "template": {
                        "metadata": {"labels": {"app.kubernetes.io/name": "plan9-model-relay"}},
                        "spec": {
                            "automountServiceAccountToken": False,
                            "containers": [
                                {
                                    "name": "proxy",
                                    "image": "nginx:1.27-alpine",
                                    "ports": [{"containerPort": 8443}],
                                    "readinessProbe": {
                                        "httpGet": {"path": "/health", "port": 8443, "scheme": "HTTPS"},
                                        "periodSeconds": 3,
                                    },
                                    "resources": {
                                        "requests": {"cpu": "20m", "memory": "32Mi"},
                                        "limits": {"cpu": "250m", "memory": "128Mi"},
                                    },
                                    "volumeMounts": [
                                        {
                                            "name": "config",
                                            "mountPath": "/etc/nginx/nginx.conf",
                                            "subPath": "nginx.conf",
                                            "readOnly": True,
                                        },
                                        {"name": "tls", "mountPath": "/tls", "readOnly": True},
                                    ],
                                }
                            ],
                            "volumes": [
                                {"name": "config", "configMap": {"name": "plan9-model-proxy"}},
                                {"name": "tls", "secret": {"secretName": "plan9-model-tls"}},
                            ],
                        },
                    },
                },
            },
            {
                "apiVersion": "networking.k8s.io/v1",
                "kind": "NetworkPolicy",
                "metadata": {"name": "plan9-model-proxy"},
                "spec": {
                    "podSelector": {"matchLabels": {"app.kubernetes.io/name": "plan9-model-relay"}},
                    "policyTypes": ["Ingress", "Egress"],
                    "ingress": [{"from": [peer], "ports": [{"protocol": "TCP", "port": 8443}]}],
                    "egress": [
                        {"to": [{"ipBlock": {"cidr": f"{address}/32"}}], "ports": [{"protocol": "TCP", "port": port}]},
                        {
                            "to": [{"namespaceSelector": {}, "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}}}],
                            "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}],
                        },
                    ],
                },
            },
        ]
        # The private leaf key goes only to an owned Kubernetes Secret via stdin.
        run(("kubectl", "-n", namespace, "apply", "-f", "-"), input_text=yaml.safe_dump_all(documents), timeout=60)
        run(
            ("kubectl", "-n", namespace, "rollout", "status", "deployment/plan9-model-relay", "--timeout=180s"),
            timeout=200,
        )
        ca_map = {
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {"name": "plan9-provider-ca"},
            "data": {"ca.pem": ca.read_text()},
        }
        run(("kubectl", "-n", runtime, "apply", "-f", "-"), input_text=yaml.safe_dump(ca_map))
        for service in ("runtime-gateway", "workflow-worker"):
            current = json.loads(run(("kubectl", "-n", runtime, "get", f"deployment/{service}", "-o", "json")).stdout)
            container = current["spec"]["template"]["spec"]["containers"][0]
            services = next(
                (
                    entry.get("value", "")
                    for entry in container.get("env", [])
                    if entry["name"] == "AGENTX_EGRESS_HTTP_PROVIDER_SERVICES"
                ),
                "",
            )
            allowed = ",".join(dict.fromkeys([*filter(None, services.split(",")), "plan9-model-relay"]))
            patch = {
                "spec": {
                    "template": {
                        "spec": {
                            "volumes": [{"name": "plan9-provider-ca", "configMap": {"name": "plan9-provider-ca"}}],
                            "containers": [
                                {
                                    "name": container["name"],
                                    "env": [
                                        {
                                            "name": "AGENTX_RUNTIME_PROVIDER_TLS_CA_PATH",
                                            "value": "/etc/plan9-provider/ca.pem",
                                        },
                                        {"name": "AGENTX_EGRESS_HTTP_PROVIDER_SERVICES", "value": allowed},
                                    ],
                                    "volumeMounts": [
                                        {
                                            "name": "plan9-provider-ca",
                                            "mountPath": "/etc/plan9-provider",
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
                    runtime,
                    "patch",
                    f"deployment/{service}",
                    "--type=strategic",
                    "-p",
                    json.dumps(patch),
                )
            )
            run(("kubectl", "-n", runtime, "rollout", "status", f"deployment/{service}", "--timeout=180s"), timeout=200)
        policy = {
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {"name": "plan9-provider-egress"},
            "spec": {
                "podSelector": peer["podSelector"],
                "policyTypes": ["Egress"],
                "egress": [
                    {
                        "to": [
                            {
                                "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": namespace}},
                                "podSelector": {"matchLabels": {"app.kubernetes.io/name": "plan9-model-relay"}},
                            }
                        ],
                        "ports": [{"protocol": "TCP", "port": 8443}],
                    }
                ],
            },
        }
        run(("kubectl", "-n", runtime, "apply", "-f", "-"), input_text=yaml.safe_dump(policy))
        yield {**configured, "endpoint": f"https://{host}:8443/v1"}
