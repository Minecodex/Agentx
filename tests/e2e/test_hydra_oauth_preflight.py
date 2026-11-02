"""Real pinned Hydra/MySQL contract spike: PKCE, opaque tokens, refresh and revocation."""

from __future__ import annotations

import base64
import hashlib
import json
import secrets
import socket
import subprocess
import time
from pathlib import Path
from urllib.parse import parse_qs, urlparse

import httpx

HYDRA = "oryd/hydra:v26.2.0@sha256:ff67c7fb5f95074fa53374d41151713554960504b340cd3f95b09e65deaea2a9"


def test_hydra_real_opaque_oauth_lifecycle(tmp_path: Path, run_id: str):
    name = f"plan9-oauth-{secrets.token_hex(5)}"
    private = tmp_path / "credentials"
    private.mkdir(mode=0o700)
    database_password = secrets.token_urlsafe(36).replace("-", "a").replace("_", "b")
    files: list[Path] = []
    containers: list[str] = []

    def docker(*args: str, timeout: int = 120) -> str:
        result = subprocess.run(["docker", *args], capture_output=True, text=True, timeout=timeout, check=False)
        if result.returncode:
            # Container startup output can contain a DSN; do not attach it to evidence.
            raise RuntimeError(f"Docker operation {args[0]} failed ({result.returncode})")
        return result.stdout.strip()

    def env_file(filename: str, values: dict[str, str]) -> str:
        path = private / filename
        path.touch(mode=0o600)
        path.write_text("".join(f"{key}={value}\n" for key, value in values.items()))
        files.append(path)
        return str(path)

    def port() -> int:
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            return listener.getsockname()[1]

    public_port, admin_port = port(), port()
    public, admin = f"http://127.0.0.1:{public_port}", f"http://127.0.0.1:{admin_port}"
    try:
        docker("network", "create", name)
        mysql_env = env_file(
            "mysql.env",
            {
                "MYSQL_ROOT_PASSWORD": secrets.token_hex(24),
                "MYSQL_DATABASE": "oauth",
                "MYSQL_USER": "hydra",
                "MYSQL_PASSWORD": database_password,
            },
        )
        mysql = docker(
            "run",
            "-d",
            "--name",
            f"{name}-mysql",
            "--network",
            name,
            "--network-alias",
            "mysql",
            "--env-file",
            mysql_env,
            "mysql:8.4",
        )
        containers.append(mysql)
        hydra_env = env_file(
            "hydra.env",
            {
                "DSN": f"mysql://hydra:{database_password}@tcp(mysql:3306)/oauth?parseTime=true&max_conns=10&max_idle_conns=4",
                "SECRETS_SYSTEM": secrets.token_hex(32),
                "URLS_SELF_ISSUER": public,
                "URLS_LOGIN": public + "/login",
                "URLS_CONSENT": public + "/consent",
                "OAUTH2_PKCE_ENFORCED": "true",
                "STRATEGIES_ACCESS_TOKEN": "opaque",
                "LOG_LEVEL": "warn",
                "LOG_LEAK_SENSITIVE_VALUES": "false",
            },
        )
        deadline = time.monotonic() + 90
        while True:
            ready = subprocess.run(
                ["docker", "exec", mysql, "mysqladmin", "ping", "--silent"], capture_output=True, timeout=5, check=False
            )
            if ready.returncode == 0:
                break
            assert time.monotonic() < deadline, "Hydra database did not become ready"
            time.sleep(1)
        docker("run", "--rm", "--network", name, "--env-file", hydra_env, HYDRA, "migrate", "sql", "-e", "--yes")
        hydra = docker(
            "run",
            "-d",
            "--name",
            f"{name}-server",
            "--network",
            name,
            "--env-file",
            hydra_env,
            "-p",
            f"127.0.0.1:{public_port}:4444",
            "-p",
            f"127.0.0.1:{admin_port}:4445",
            HYDRA,
            "serve",
            "all",
            "--dev",
            "--sqa-opt-out",
        )
        containers.append(hydra)
        with httpx.Client(timeout=10, follow_redirects=False, trust_env=False) as client:
            while True:
                try:
                    if client.get(admin + "/health/ready").status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                assert time.monotonic() < deadline, "Hydra did not become ready"
                time.sleep(0.5)
            client_id = secrets.token_hex(16)
            audience, scope = public + "/mcp/application", "agentx:application:spike"
            registered = client.post(
                admin + "/admin/clients",
                json={
                    "client_id": client_id,
                    "client_name": "Plan9 isolated acceptance",
                    "redirect_uris": [public + "/callback"],
                    "grant_types": ["authorization_code", "refresh_token"],
                    "response_types": ["code"],
                    "scope": scope + " offline_access",
                    "audience": [audience],
                    "token_endpoint_auth_method": "none",
                    "access_token_strategy": "opaque",
                },
            )
            assert registered.status_code == 201, f"Client registration status {registered.status_code}"
            verifier = secrets.token_urlsafe(64)
            challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
            authorization = client.get(
                public + "/oauth2/auth",
                params={
                    "client_id": client_id,
                    "response_type": "code",
                    "redirect_uri": public + "/callback",
                    "scope": scope + " offline_access",
                    "state": secrets.token_urlsafe(24),
                    "code_challenge": challenge,
                    "code_challenge_method": "S256",
                },
            )
            assert authorization.status_code == 302
            login_challenge = parse_qs(urlparse(authorization.headers["location"]).query)["login_challenge"][0]
            login = client.put(
                admin + "/admin/oauth2/auth/requests/login/accept",
                params={"login_challenge": login_challenge},
                json={"subject": "tenant:user", "remember": False},
            )
            assert login.status_code == 200
            consent_redirect = client.get(login.json()["redirect_to"])
            assert consent_redirect.status_code == 302
            consent_challenge = parse_qs(urlparse(consent_redirect.headers["location"]).query)["consent_challenge"][0]
            consent = client.put(
                admin + "/admin/oauth2/auth/requests/consent/accept",
                params={"consent_challenge": consent_challenge},
                json={
                    "grant_scope": [scope, "offline_access"],
                    "grant_access_token_audience": [audience],
                    "remember": False,
                    "session": {"access_token": {"agentx": {"tenantId": "tenant", "userId": "user"}}},
                },
            )
            assert consent.status_code == 200
            callback = client.get(consent.json()["redirect_to"])
            assert callback.status_code in (302, 303)
            code = parse_qs(urlparse(callback.headers["location"]).query)["code"][0]
            token = client.post(
                public + "/oauth2/token",
                data={
                    "grant_type": "authorization_code",
                    "client_id": client_id,
                    "code": code,
                    "redirect_uri": public + "/callback",
                    "code_verifier": verifier,
                },
            )
            assert token.status_code == 200, f"PKCE exchange status {token.status_code}"
            tokens = token.json()
            opaque_token = tokens["access_token"].startswith("ory_at_") and tokens["access_token"].count(".") != 2
            assert opaque_token
            introspect = client.post(admin + "/admin/oauth2/introspect", data={"token": tokens["access_token"]}).json()
            assert introspect["active"] is True and introspect["sub"] == "tenant:user"
            assert introspect["aud"] == [audience] and introspect["ext"]["agentx"]["userId"] == "user"
            refreshed = client.post(
                public + "/oauth2/token",
                data={"grant_type": "refresh_token", "client_id": client_id, "refresh_token": tokens["refresh_token"]},
            )
            assert refreshed.status_code == 200
            new_tokens = refreshed.json()
            revoked = client.delete(
                admin + "/admin/oauth2/auth/sessions/consent", params={"subject": "tenant:user", "client": client_id}
            )
            assert revoked.status_code == 204
            assert (
                client.post(admin + "/admin/oauth2/introspect", data={"token": new_tokens["access_token"]}).json()[
                    "active"
                ]
                is False
            )
            assert (
                client.post(
                    public + "/oauth2/token",
                    data={
                        "grant_type": "refresh_token",
                        "client_id": client_id,
                        "refresh_token": new_tokens["refresh_token"],
                    },
                ).status_code
                == 400
            )
            evidence = Path(".local/artifacts/e2e") / run_id / "hydra-preflight.json"
            evidence.parent.mkdir(parents=True, exist_ok=True)
            evidence.write_text(
                json.dumps(
                    {
                        "image": HYDRA,
                        "database": "mysql:8.4",
                        "opaqueAccessToken": True,
                        "pkce": True,
                        "explicitConsent": True,
                        "applicationAudience": True,
                        "introspection": True,
                        "refresh": True,
                        "revocation": True,
                        "revokedRefreshRejected": True,
                    },
                    indent=2,
                )
            )
    finally:
        for container in reversed(containers):
            subprocess.run(["docker", "rm", "-f", "-v", container], capture_output=True, timeout=30, check=False)
        subprocess.run(["docker", "network", "rm", name], capture_output=True, timeout=30, check=False)
        for path in files:
            path.unlink(missing_ok=True)
        private.rmdir()
