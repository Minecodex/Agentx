"""Advertise the actual temporary port-forward addresses to OAuth clients.

Only existing environment settings of the owned temporary deployments change.
The public issuer, redirects, Host validation and CORS retain their normal checks;
no local DNS edits, daily ingress changes or client protocol interception is used.
"""

import json
import time
from pathlib import Path

import httpx
import pytest

from tests.e2e.support import run


@pytest.fixture(scope="session")
def plan9_public_origins(installed_agentx, service_urls):
    control, runtime = service_urls["web"], service_urls["runtime"]
    changes = [
        (
            installed_agentx["control_namespace"],
            "platform-control",
            {"AGENTX_RUNTIME_PUBLIC_ORIGIN": runtime, "AGENTX_OAUTH_ISSUER": runtime},
        ),
        (
            installed_agentx["runtime_namespace"],
            "runtime-gateway",
            {
                "AGENTX_RUNTIME_PUBLIC_ORIGIN": runtime,
                "AGENTX_OAUTH_ISSUER": runtime,
                "AGENTX_RUNTIME_CORS_ALLOWED_ORIGIN": control,
            },
        ),
        (installed_agentx["control_namespace"], "web-console", {"AGENTX_RUNTIME_PUBLIC_BASE_URL": runtime}),
        (
            installed_agentx["dependencies_namespace"],
            "hydra",
            {
                "URLS_SELF_ISSUER": runtime,
                "URLS_LOGIN": f"{control}/oauth/login",
                "URLS_CONSENT": f"{control}/oauth/consent",
            },
        ),
    ]
    old_pods = []
    for namespace, deployment, environment in changes:
        assert namespace.startswith("agentx-e2e-"), "Only the owned temporary environment can be changed"
        selector = run(("kubectl", "-n", namespace, "get", "deployment", deployment, "-o", "json")).json()["spec"][
            "selector"
        ]["matchLabels"]
        pods = run(
            (
                "kubectl",
                "-n",
                namespace,
                "get",
                "pods",
                "-l",
                ",".join(f"{key}={value}" for key, value in selector.items()),
                "-o",
                "json",
            )
        ).json()["items"]
        old_pods.extend((namespace, pod["metadata"]["name"]) for pod in pods)
        run(
            (
                "kubectl",
                "-n",
                namespace,
                "set",
                "env",
                f"deployment/{deployment}",
                *(f"{key}={value}" for key, value in environment.items()),
            )
        )
    for namespace, deployment, _ in changes:
        run(
            ("kubectl", "-n", namespace, "rollout", "status", f"deployment/{deployment}", "--timeout=180s"), timeout=200
        )
    # A service port-forward remains bound to its original Pod during graceful
    # termination. Its liveness endpoint can stay 200 while business routes
    # correctly return 503 for draining. Wait for those exact Pods to leave.
    for namespace, pod in old_pods:
        run(("kubectl", "-n", namespace, "wait", "--for=delete", f"pod/{pod}", "--timeout=120s"), timeout=140)
    wait_public_entrypoints(control, runtime)
    (Path(installed_agentx["artifact_dir"]) / "public-origins.json").write_text(
        json.dumps(
            {
                "web": control,
                "runtime": runtime,
                "temporaryDeploymentEnvironment": [
                    {"namespace": namespace, "deployment": deployment, "environment": environment}
                    for namespace, deployment, environment in changes
                ],
            },
            indent=2,
        )
    )
    return service_urls


def wait_public_entrypoints(control: str, runtime: str) -> None:
    """Wait for supervised read-only tunnels after all temporary Pod rollouts."""
    until = time.monotonic() + 90
    with httpx.Client(timeout=5, trust_env=False) as client:
        while True:
            try:
                ready = client.get(control + "/api/v1/bootstrap/status").status_code == 200
                ready = ready and client.get(runtime + "/.well-known/oauth-authorization-server").status_code == 200
                if ready:
                    break
            except httpx.HTTPError:
                pass
            assert time.monotonic() < until, "Temporary public entrypoints did not become ready after rollout"
            time.sleep(0.5)
