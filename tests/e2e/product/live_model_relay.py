"""TLS pass-through fixture: one authorized model, two requests, one USD ledger.

The upstream key is read without echo and never written to disk or a command line.
Run with the repository's uv test environment. Application and external-client
credentials reference the private relay credential file printed at startup.
"""

from __future__ import annotations

import argparse
import getpass
import hmac
import ipaddress
import json
import os
import secrets
import signal
import ssl
import threading
from contextlib import suppress
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import httpx
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

from tests.e2e.product.live_model_budget import BudgetError, ModelBudget


def private_file(path: Path, value: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(value)


def certificates(directory: Path, dns_names: tuple[str, ...] = ()) -> tuple[Path, Path, Path]:
    ca_path, cert_path, key_path = (directory / name for name in ("ca.pem", "server.pem", "server-key.pem"))
    if all(path.is_file() for path in (ca_path, cert_path, key_path)):
        return ca_path, cert_path, key_path
    if any(path.exists() for path in (ca_path, cert_path, key_path)):
        raise ValueError("Incomplete relay TLS material; use a fresh private directory")
    now = datetime.now(UTC)
    authority_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    authority_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Agentx isolated acceptance CA")])
    authority = (
        x509.CertificateBuilder()
        .subject_name(authority_name)
        .issuer_name(authority_name)
        .public_key(authority_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - timedelta(minutes=5))
        .not_valid_after(now + timedelta(days=7))
        .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
        .add_extension(x509.KeyUsage(False, False, False, False, False, True, True, False, False), critical=True)
        .sign(authority_key, hashes.SHA256())
    )
    server_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    server = (
        x509.CertificateBuilder()
        .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Agentx model budget relay")]))
        .issuer_name(authority.subject)
        .public_key(server_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - timedelta(minutes=5))
        .not_valid_after(now + timedelta(days=7))
        .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
        .add_extension(
            x509.SubjectAlternativeName(
                [
                    x509.DNSName("localhost"),
                    x509.DNSName("host.docker.internal"),
                    x509.IPAddress(ipaddress.ip_address("127.0.0.1")),
                    *(x509.DNSName(name) for name in dns_names),
                ]
            ),
            critical=False,
        )
        .sign(authority_key, hashes.SHA256())
    )
    private_file(ca_path, authority.public_bytes(serialization.Encoding.PEM))
    private_file(cert_path, server.public_bytes(serialization.Encoding.PEM))
    private_file(
        key_path,
        server_key.private_bytes(
            serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()
        ),
    )
    return ca_path, cert_path, key_path


class Relay:
    def __init__(self, upstream_key: str, relay_key: str, ledger: ModelBudget):
        self.upstream_key = upstream_key
        self.relay_key = relay_key
        self.ledger = ledger
        self.slots = threading.BoundedSemaphore(2)
        self.client = httpx.Client(timeout=httpx.Timeout(300, connect=15), trust_env=False, follow_redirects=False)

    def handler(self):
        context = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, _pattern, *_arguments):
                pass

            def authorized(self) -> bool:
                return hmac.compare_digest(self.headers.get("Authorization", ""), "Bearer " + context.relay_key)

            def response(self, status: int, payload: dict) -> None:
                body = json.dumps(payload).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)
                self.close_connection = True

            def do_GET(self):
                if self.path == "/health":
                    self.response(200, {"ready": True, "model": "glm-5.3-flash"})
                elif self.path == "/budget" and self.authorized():
                    self.response(200, context.ledger.snapshot())
                else:
                    self.response(404, {"error": "NOT_FOUND"})

            def do_POST(self):
                if self.path != "/v1/chat/completions" or not self.authorized():
                    self.response(401, {"error": {"code": "RELAY_AUTH_REQUIRED"}})
                    return
                try:
                    length = int(self.headers.get("Content-Length", "0"))
                    if not 0 < length <= 8 * 1024 * 1024:
                        raise ValueError("A bounded JSON request is required")
                    payload = json.loads(self.rfile.read(length))
                    if not isinstance(payload, dict):
                        raise ValueError("A JSON object is required")
                except (ValueError, json.JSONDecodeError):
                    self.response(400, {"error": {"code": "RELAY_REQUEST_INVALID"}})
                    return
                if not context.slots.acquire(timeout=30):
                    self.response(429, {"error": {"code": "REAL_MODEL_CONCURRENCY_LIMIT"}})
                    return
                try:
                    try:
                        identifier = context.ledger.reserve(payload)
                    except BudgetError as error:
                        self.response(429, {"error": {"code": "REAL_MODEL_ADMISSION_REJECTED", "message": str(error)}})
                        return
                    self.forward(payload, identifier)
                finally:
                    context.slots.release()

            def forward(self, payload: dict, identifier: str) -> None:
                began_response = False
                usage = None
                try:
                    with context.client.stream(
                        "POST",
                        "https://api.apimart.ai/v1/chat/completions",
                        headers={"Authorization": "Bearer " + context.upstream_key},
                        json=payload,
                    ) as upstream:
                        if upstream.status_code != 200:
                            context.ledger.unknown(identifier)
                            self.response(
                                upstream.status_code,
                                {"error": {"code": "UPSTREAM_HTTP_ERROR", "status": upstream.status_code}},
                            )
                            return
                        if payload.get("stream") is True:
                            self.send_response(200)
                            self.send_header("Content-Type", "text/event-stream")
                            self.send_header("Cache-Control", "no-cache")
                            self.send_header("Connection", "close")
                            self.end_headers()
                            began_response = True
                            for line in upstream.iter_lines():
                                if line.startswith("data:"):
                                    try:
                                        event = json.loads(line[5:].strip())
                                    except ValueError:
                                        event = {}
                                    if isinstance(event, dict) and isinstance(event.get("usage"), dict):
                                        usage = event["usage"]
                                self.wfile.write((line + "\n").encode())
                                self.wfile.flush()
                            self.close_connection = True
                        else:
                            raw = upstream.read()
                            try:
                                value = json.loads(raw)
                                usage = value.get("usage") if isinstance(value, dict) else None
                            except ValueError:
                                pass
                            self.send_response(200)
                            self.send_header("Content-Type", "application/json")
                            self.send_header("Content-Length", str(len(raw)))
                            self.send_header("Connection", "close")
                            self.end_headers()
                            began_response = True
                            self.wfile.write(raw)
                            self.close_connection = True
                    if isinstance(usage, dict):
                        # Ledger retains uncertainty or halts subsequent admission.
                        with suppress(BudgetError):
                            context.ledger.settle(identifier, usage)
                    else:
                        context.ledger.unknown(identifier)
                except (httpx.ConnectError, httpx.ConnectTimeout):
                    context.ledger.unsent(identifier)
                    if not began_response:
                        self.response(503, {"error": {"code": "UPSTREAM_CONNECTION_UNAVAILABLE"}})
                except (httpx.HTTPError, OSError):
                    context.ledger.unknown(identifier)
                    if not began_response:
                        self.response(502, {"error": {"code": "UPSTREAM_OUTCOME_UNKNOWN"}})

        return Handler


def main() -> None:
    parser = argparse.ArgumentParser(description="Private TLS relay for bounded real-model acceptance")
    parser.add_argument("--private-directory", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--listen", default="127.0.0.1")
    parser.add_argument("--limit-usd", type=Decimal, default=Decimal("30"))
    parser.add_argument("--input-price", type=Decimal, required=True)
    parser.add_argument("--output-price", type=Decimal, required=True)
    args = parser.parse_args()
    if not 0 < args.limit_usd <= 30:
        parser.error("The authorized acceptance limit is at most USD 30")
    args.private_directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    args.private_directory.chmod(0o700)
    key_file = args.private_directory / "relay-credential"
    if not key_file.exists():
        private_file(key_file, secrets.token_urlsafe(40).encode())
    if key_file.stat().st_mode & 0o077:
        parser.error("The relay credential file must be private")
    ledger = ModelBudget(
        args.report, limit=args.limit_usd, input_price=args.input_price, output_price=args.output_price
    )
    ca, cert, key = certificates(args.private_directory)
    upstream_key = getpass.getpass("APIMart test credential (not echoed): ").strip()
    if not upstream_key or any(character.isspace() for character in upstream_key):
        parser.error("A single nonempty provider credential is required")
    relay = Relay(upstream_key, key_file.read_text().strip(), ledger)
    server = ThreadingHTTPServer((args.listen, args.port), relay.handler())
    server.daemon_threads = True
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.minimum_version = ssl.TLSVersion.TLSv1_2
    tls.load_cert_chain(cert, key)
    server.socket = tls.wrap_socket(server.socket, server_side=True)
    port = server.server_address[1]
    print(
        json.dumps(
            {
                "hostEndpoint": f"https://127.0.0.1:{port}/v1",
                "clusterEndpoint": f"https://host.docker.internal:{port}/v1",
                "credentialFile": str(key_file),
                "caFile": str(ca),
                "budgetReport": str(args.report),
            },
            indent=2,
        ),
        flush=True,
    )

    def stop(_number, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        relay.client.close()
        relay.upstream_key = ""
        upstream_key = ""


if __name__ == "__main__":
    main()
