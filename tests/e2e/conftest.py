from __future__ import annotations

import base64
import os
import socket
import socketserver
import threading
import time
import uuid
from collections.abc import Iterator
from datetime import UTC, datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import httpx
import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import padding as asymmetric_padding

from tests.e2e.support import ManagedProcess, agentxctl, deployment_config, redact, run, start_process


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption("--values", action="store", default=os.getenv("AGENTX_E2E_VALUES"))
    parser.addoption("--keep-on-failure", action="store_true", default=False)
    parser.addoption("--scale-down-development", action="store_true", default=False)


@pytest.fixture(scope="session")
def deployment_values(pytestconfig: pytest.Config) -> Path:
    value = pytestconfig.getoption("--values")
    if not value:
        pytest.skip("cluster E2E requires --values or AGENTX_E2E_VALUES")
    return Path(value).resolve()


@pytest.fixture(scope="session")
def run_id() -> str:
    return uuid.uuid4().hex[:10]


def _timeline(timeline: list[str], message: str) -> None:
    timeline.append(f"{datetime.now(UTC).isoformat()} {message}")


def _must_remain_available_during_scale_down(deployment: dict[str, object]) -> bool:
    labels = deployment.get("metadata", {}).get("labels", {})
    return (
        labels.get("app.kubernetes.io/name") == "ingress-nginx"
        and labels.get("app.kubernetes.io/component") == "controller"
    )


def _scale_development(values: Path, enabled: bool) -> dict[tuple[str, str], int]:
    if not enabled:
        return {}
    config = deployment_config(values)
    replicas: dict[tuple[str, str], int] = {}
    for namespace in dict.fromkeys(config["namespaces"].values()):
        result = run(("kubectl", "-n", namespace, "get", "deployment", "-o", "json"), check=False, timeout=60)
        if result.returncode != 0:
            continue
        for deployment in result.json().get("items", []):
            if _must_remain_available_during_scale_down(deployment):
                continue
            name = deployment["metadata"]["name"]
            replicas[(namespace, name)] = int(deployment.get("spec", {}).get("replicas", 1))
            run(("kubectl", "-n", namespace, "scale", f"deployment/{name}", "--replicas=0"), timeout=60)
    return replicas


def _restore_development(replicas: dict[tuple[str, str], int]) -> None:
    for (namespace, name), count in replicas.items():
        run(
            ("kubectl", "-n", namespace, "scale", f"deployment/{name}", f"--replicas={count}"),
            check=False,
            timeout=60,
        )


def _collect_artifacts(context: dict[str, str], artifact_dir: Path, timeline: list[str]) -> None:
    for plane in ("control", "runtime", "dependencies"):
        namespace = context[f"{plane}_namespace"]
        commands = {
            "resources": ("kubectl", "-n", namespace, "get", "all,ingress,networkpolicy,pdb", "-o", "yaml"),
            "events": ("kubectl", "-n", namespace, "get", "events", "-o", "yaml"),
            "logs": (
                "kubectl",
                "-n",
                namespace,
                "logs",
                "-l",
                f"agentx.io/plane={plane}",
                "--all-containers=true",
                "--prefix=true",
                "--tail=1000",
            ),
        }
        for label, command in commands.items():
            result = run(command, check=False, timeout=180)
            content = result.stdout + (f"\n{result.stderr}" if result.stderr else "")
            (artifact_dir / f"{plane}-{label}.txt").write_text(redact(content), encoding="utf-8")
    (artifact_dir / "timeline.txt").write_text("\n".join(timeline) + "\n", encoding="utf-8")


@pytest.fixture(scope="session")
def installed_agentx(
    deployment_values: Path,
    run_id: str,
    pytestconfig: pytest.Config,
    request: pytest.FixtureRequest,
) -> Iterator[dict[str, str]]:
    timeline: list[str] = []
    failures_before = request.session.testsfailed
    development = _scale_development(deployment_values, pytestconfig.getoption("--scale-down-development"))
    config = deployment_config(deployment_values, run_id=run_id)
    artifact_dir = Path(__file__).resolve().parents[2] / ".local" / "artifacts" / "e2e" / run_id
    artifact_dir.mkdir(parents=True, exist_ok=True)
    _timeline(timeline, "install started")
    install_command = (
        agentxctl(),
        "install",
        "--values",
        deployment_values,
        "--run-id",
        run_id,
        "--output",
        "json",
    )
    try:
        installed = run(install_command, timeout=3600)
    except Exception:
        run(
            (
                agentxctl(),
                "uninstall",
                "--values",
                deployment_values,
                "--run-id",
                run_id,
                "--purge-data",
                "--yes",
            ),
            check=False,
            timeout=1200,
        )
        _restore_development(development)
        raise
    (artifact_dir / "install.json").write_text(redact(installed.stdout), encoding="utf-8")
    _timeline(timeline, "install and Helm Doctor completed")
    context = {
        "values": str(deployment_values),
        "run_id": run_id,
        "control_namespace": config["namespaces"]["control"],
        "runtime_namespace": config["namespaces"]["runtime"],
        "dependencies_namespace": config["namespaces"]["dependencies"],
        "artifact_dir": str(artifact_dir),
        "root": str(Path(__file__).resolve().parents[2]),
    }
    try:
        yield context
    finally:
        failed = request.session.testsfailed > failures_before
        _timeline(timeline, f"test session completed failed={str(failed).lower()}")
        _collect_artifacts(context, artifact_dir, timeline)
        keep = failed and pytestconfig.getoption("--keep-on-failure")
        if not keep:
            _timeline(timeline, "purge started")
            result = run(
                (
                    agentxctl(),
                    "uninstall",
                    "--values",
                    deployment_values,
                    "--run-id",
                    run_id,
                    "--purge-data",
                    "--yes",
                    "--output",
                    "json",
                ),
                check=False,
                timeout=1200,
            )
            (artifact_dir / "uninstall.json").write_text(redact(result.stdout + result.stderr), encoding="utf-8")
        _restore_development(development)
        (artifact_dir / "timeline.txt").write_text("\n".join(timeline) + "\n", encoding="utf-8")


# RAGFlow v0.20.0 ships this RSA public key inside the image (conf/public.pem,
# paired with the "Welcome"-protected private key); the register/login API
# expects the password encrypted with it (PKCS#1 v1.5, base64 encoded).
RAGFLOW_PUBLIC_PEM = """-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEArq9XTUSeYr2+N1h3Afl/
z8Dse/2yD0ZGrKwx+EEEcdsBLca9Ynmx3nIB5obmLlSfmskLpBo0UACBmB5rEjBp
2Q2f3AG3Hjd4B+gNCG6BDaawuDlgANIhGnaTLrIqWrrcm4EMzJOnAOI1fgzJRsOO
UEfaS318Eq9OVO3apEyCCt0lOQK6PuksduOjVxtltDav+guVAA068NrPYmRNabVK
RNLJpL8w4D44sfth5RvZ3q9t+6RTArpEtc5sh5ChzvqPOzKGMXW83C95TxmXqpbK
6olN4RevSfVjEAgCydH6HN6OhtOQEcnrU97r9H0iZOWwbw3pVrZiUkuRD1R56Wzs
2wIDAQAB
-----END PUBLIC KEY-----"""

RAGFLOW_USER_EMAIL = "agentx-e2e@agentx.invalid"
RAGFLOW_USER_PASSWORD = "agentx-e2e-ragflow-password"  # noqa: S105 -- isolated E2E fixture credential


def _ragflow_encrypt_password() -> str:
    key = serialization.load_pem_public_key(RAGFLOW_PUBLIC_PEM.encode())
    encrypted = key.encrypt(RAGFLOW_USER_PASSWORD.encode(), asymmetric_padding.PKCS1v15())
    return base64.b64encode(encrypted).decode()


def _ragflow_preset(installed_agentx: dict[str, str]) -> dict[str, str]:
    """Register the fixture user, mint an API token and create a dataset.

    Runs from the host through a short-lived port-forward; RAGFlow has no
    healthz route, so readiness is probed via the unauthenticated
    /v1/system/config endpoint.
    """
    namespace = installed_agentx["dependencies_namespace"]
    artifact_dir = Path(installed_agentx["artifact_dir"])
    port = _free_port()
    forward = start_process(
        (
            "kubectl",
            "-n",
            namespace,
            "port-forward",
            "service/ragflow",
            f"{port}:9380",
        ),
        stdout_path=artifact_dir / "port-forward-ragflow.log",
        stderr_path=artifact_dir / "port-forward-ragflow-error.log",
    )
    base = f"http://127.0.0.1:{port}"
    try:
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            if forward.process.poll() is not None:
                raise RuntimeError("ragflow port-forward exited during readiness wait")
            try:
                if httpx.get(f"{base}/v1/system/config", timeout=3).status_code == 200:
                    break
            except httpx.HTTPError:
                pass
            time.sleep(2)
        else:
            raise RuntimeError("ragflow did not become ready within 300s")
        encrypted = _ragflow_encrypt_password()
        with httpx.Client(base_url=base, timeout=30) as client:
            register = client.post(
                "/v1/user/register",
                json={"nickname": "agentx-e2e", "email": RAGFLOW_USER_EMAIL, "password": encrypted},
            )
            data = register.json().get("data") if register.status_code == 200 else None
            if data:
                access = register.headers.get("Authorization")
            else:
                login = client.post(
                    "/v1/user/login",
                    json={"email": RAGFLOW_USER_EMAIL, "password": encrypted},
                )
                login.raise_for_status()
                access = login.headers.get("Authorization")
            if not access:
                raise RuntimeError("ragflow register/login did not return an Authorization token")
            token_response = client.post("/v1/system/new_token", headers={"Authorization": access})
            token_response.raise_for_status()
            api_key = token_response.json()["data"]["token"]
            dataset = client.post(
                "/api/v1/datasets",
                headers={"Authorization": f"Bearer {api_key}"},
                json={"name": f"agentx-e2e-{uuid.uuid4().hex[:8]}"},
            )
            dataset.raise_for_status()
            payload = dataset.json()
            if payload.get("code") != 0:
                raise RuntimeError(f"ragflow dataset creation failed: {payload}")
            return {"api_key": api_key, "dataset_id": payload["data"]["id"]}
    finally:
        forward.stop()


@pytest.fixture(scope="session")
def e2e_providers(installed_agentx: dict[str, str]) -> dict[str, str]:
    namespace = installed_agentx["dependencies_namespace"]
    fixture = Path(installed_agentx["root"]) / "deploy" / "kustomize" / "e2e-fixtures" / "runtime-providers"
    run(("kubectl", "-n", namespace, "apply", "-k", fixture), timeout=300)
    run(
        (
            "kubectl",
            "-n",
            namespace,
            "wait",
            "--for=condition=complete",
            "job/lightrag-tokenizer-cache",
            "--timeout=300s",
        ),
        timeout=330,
    )
    # RAGFlow is deployed by default so the protocol cases run with skipped=0;
    # capacity runs can opt out via AGENTX_E2E_RAGFLOW_DISABLE=1 because the
    # stack (server + ES + MySQL + MinIO) is too heavy to coexist with a
    # dedicated capacity window on a single-node cluster.
    ragflow_enabled = not os.getenv("AGENTX_E2E_RAGFLOW_DISABLE")
    deployments = ["echo-mcp", "echo-node", "lightrag", "mem0", "mem0-postgres"]
    if ragflow_enabled:
        deployments += ["ragflow-es", "ragflow-mysql", "ragflow-redis", "ragflow-minio", "ragflow"]
    for deployment in deployments:
        if deployment == "lightrag" or (deployment.startswith("ragflow-") and deployment != "ragflow-redis"):
            rollout_timeout = 600
        elif deployment == "ragflow":
            rollout_timeout = 2400
        else:
            rollout_timeout = 300
        result = run(
            (
                "kubectl",
                "-n",
                namespace,
                "rollout",
                "status",
                f"deployment/{deployment}",
                f"--timeout={rollout_timeout}s",
            ),
            check=False,
            timeout=rollout_timeout + 30,
        )
        if result.returncode != 0:
            raise RuntimeError(f"required E2E provider did not become ready: {namespace}/{deployment}")
    urls = {
        "echo_mcp": f"http://echo-mcp.{namespace}.svc:8090",
        "echo_node": f"http://echo-node.{namespace}.svc:8080",
        "lightrag": f"http://lightrag.{namespace}.svc:9621",
        "mem0": f"http://mem0.{namespace}.svc:8000",
    }
    if ragflow_enabled:
        preset = _ragflow_preset(installed_agentx)
        urls.update(
            {
                "ragflow": f"http://ragflow.{namespace}.svc:9380",
                "ragflow_alias": f"http://ragflow.{namespace}.svc:9380",
                "ragflow_api_key": preset["api_key"],
                "ragflow_dataset_id": preset["dataset_id"],
            }
        )
    return urls


OPENSANDBOX_HEALTH = "http://127.0.0.1:18080/health"
OPENSANDBOX_API_KEY = "agentx-local-opensandbox-key"


def _opensandbox_healthy() -> bool:
    try:
        response = httpx.get(
            OPENSANDBOX_HEALTH,
            headers={"Open-Sandbox-Api-Key": OPENSANDBOX_API_KEY},
            timeout=5,
        )
    except httpx.HTTPError:
        return False
    return response.status_code == 200 and response.json().get("status") == "healthy"


@pytest.fixture(scope="session")
def opensandbox_server(installed_agentx: dict[str, str]) -> Iterator[None]:
    """Start the OpenSandbox lifecycle server when it is not already running.

    Removes the manual pre-start step: the server runs as a host process via
    uvx (pinned 0.2.2, matching the documented local baseline) with a derived
    config that listens on the conventional 18080 port. An already-running
    server (started per deploy/opensandbox/README.md) is reused as-is.
    """
    if _opensandbox_healthy():
        yield
        return
    root = Path(installed_agentx["root"])
    artifact_dir = Path(installed_agentx["artifact_dir"])
    template = (root / "deploy" / "opensandbox" / "docker" / "config.local.toml").read_text(encoding="utf-8")
    config = template.replace("port = 8080", "port = 18080").replace(
        'path = "/data/opensandbox.db"',
        f'path = "{artifact_dir / "opensandbox.db"}"',
    )
    config_path = artifact_dir / "opensandbox.local.toml"
    config_path.write_text(config, encoding="utf-8")
    server = start_process(
        (
            "uvx",
            "--from",
            "opensandbox-server==0.2.2",
            "opensandbox-server",
            "--config",
            str(config_path),
        ),
        stdout_path=artifact_dir / "opensandbox-server.log",
        stderr_path=artifact_dir / "opensandbox-server-error.log",
    )
    try:
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if server.process.poll() is not None:
                raise RuntimeError("opensandbox-server exited during startup; see opensandbox-server*.log artifacts")
            if _opensandbox_healthy():
                yield
                return
            time.sleep(2)
        raise RuntimeError("opensandbox-server did not become healthy within 180s")
    finally:
        server.stop()


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


class _CodeEgressHttpHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        body = b'{"kind":"http","status":"ok"}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: object) -> None:
        return


class _CodeEgressTcpHandler(socketserver.BaseRequestHandler):
    def handle(self) -> None:
        payload = self.request.recv(4096).strip()
        self.request.sendall(b"tcp:" + payload + b"\n")


class _ThreadedTcpFixture(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


@pytest.fixture(scope="session")
def code_egress_fixtures() -> Iterator[dict[str, str]]:
    http_server = ThreadingHTTPServer(
        ("0.0.0.0", 0),  # noqa: S104 -- Docker Desktop must reach the host fixture.
        _CodeEgressHttpHandler,
    )
    tcp_server = _ThreadedTcpFixture(
        ("0.0.0.0", 0),  # noqa: S104 -- Docker Desktop must reach the host fixture.
        _CodeEgressTcpHandler,
    )
    threads = [
        threading.Thread(target=http_server.serve_forever, daemon=True),
        threading.Thread(target=tcp_server.serve_forever, daemon=True),
    ]
    for thread in threads:
        thread.start()
    try:
        yield {
            "host": "host.docker.internal",
            "http_port": str(http_server.server_port),
            "tcp_port": str(tcp_server.server_address[1]),
        }
    finally:
        http_server.shutdown()
        tcp_server.shutdown()
        http_server.server_close()
        tcp_server.server_close()
        for thread in threads:
            thread.join(timeout=5)


def _wait_http(process: ManagedProcess, url: str) -> None:
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if process.process.poll() is not None:
            raise RuntimeError(f"port-forward exited before {url} became ready")
        try:
            response = httpx.get(url, timeout=2)
            if response.status_code < 500:
                return
        except httpx.HTTPError:
            pass
        time.sleep(0.5)
    raise RuntimeError(f"timed out waiting for {url}")


@pytest.fixture(scope="session")
def service_urls(installed_agentx: dict[str, str]) -> Iterator[dict[str, str]]:
    artifact_dir = Path(installed_agentx["artifact_dir"])
    web_port, runtime_port, sandbox_manager_port = _free_port(), _free_port(), _free_port()
    forwards = [
        start_process(
            (
                "kubectl",
                "-n",
                installed_agentx["control_namespace"],
                "port-forward",
                "service/web-console",
                f"{web_port}:8080",
            ),
            stdout_path=artifact_dir / "port-forward-web.log",
            stderr_path=artifact_dir / "port-forward-web-error.log",
        ),
        start_process(
            (
                "kubectl",
                "-n",
                installed_agentx["runtime_namespace"],
                "port-forward",
                "service/runtime-gateway-public",
                f"{runtime_port}:8080",
            ),
            stdout_path=artifact_dir / "port-forward-runtime.log",
            stderr_path=artifact_dir / "port-forward-runtime-error.log",
        ),
        start_process(
            (
                "kubectl",
                "-n",
                installed_agentx["runtime_namespace"],
                "port-forward",
                "service/sandbox-manager",
                f"{sandbox_manager_port}:8080",
            ),
            stdout_path=artifact_dir / "port-forward-sandbox-manager.log",
            stderr_path=artifact_dir / "port-forward-sandbox-manager-error.log",
        ),
    ]
    urls = {
        "web": f"http://127.0.0.1:{web_port}",
        "runtime": f"http://127.0.0.1:{runtime_port}",
        "sandbox_manager": f"http://127.0.0.1:{sandbox_manager_port}",
    }
    try:
        _wait_http(forwards[0], f"{urls['web']}/health/live")
        _wait_http(forwards[1], f"{urls['runtime']}/health/live")
        _wait_http(forwards[2], f"{urls['sandbox_manager']}/health/live")
        yield urls
    finally:
        for process in reversed(forwards):
            process.stop()
