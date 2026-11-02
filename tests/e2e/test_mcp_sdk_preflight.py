"""Real HTTP interoperability of the fixed SDK with current and existing resource clients."""

from __future__ import annotations

import json
import os
import secrets
import socket
import time
from dataclasses import dataclass, field
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.plan9.opencode_client import isolated_opencode
from tests.e2e.support import ROOT, run, start_process


@dataclass
class McpServer:
    client: httpx.Client
    token: str = field(repr=False)
    directory: Path


@pytest.fixture(scope="module")
def mcp_server(run_id, request):
    run(("cargo", "build", "--locked", "-p", "echo-mcp"), timeout=600)
    stateless = getattr(request, "param", False)
    directory = ROOT / ".local/artifacts/e2e" / run_id / ("mcp-sdk-stateless" if stateless else "mcp-sdk")
    directory.mkdir(parents=True, exist_ok=True)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    token = secrets.token_urlsafe(32)
    process = start_process(
        (ROOT / "target/debug" / ("echo-mcp.exe" if os.name == "nt" else "echo-mcp"),),
        stdout_path=directory / "server.jsonl",
        stderr_path=directory / "server-errors.txt",
        env={
            "AGENTX_BIND_ADDR": f"127.0.0.1:{port}",
            "AGENTX_FIXTURE_MCP_AUTH_TOKEN": token,
            "AGENTX_FIXTURE_MCP_STATELESS": "1" if stateless else "0",
        },
    )
    try:
        with httpx.Client(base_url=f"http://127.0.0.1:{port}", timeout=15, trust_env=False) as client:
            deadline = time.monotonic() + 30
            while True:
                try:
                    if client.get("/health").status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                if process.process.poll() is not None or time.monotonic() > deadline:
                    pytest.fail("The fixed MCP SDK fixture failed to start", pytrace=False)
                time.sleep(0.1)
            yield McpServer(client, token, directory)
    finally:
        process.stop()


def rpc(client, headers, identifier, method, params=None):
    response = client.post(
        "/mcp",
        headers=headers,
        json={
            "jsonrpc": "2.0",
            "id": identifier,
            "method": method,
            **({"params": params} if params is not None else {}),
        },
    )
    assert response.status_code == 200, f"{method}: HTTP {response.status_code}"
    if "application/json" in response.headers.get("content-type", ""):
        value = response.json()
    else:
        events = [
            json.loads(line[5:]) for line in response.text.splitlines() if line.startswith("data:") and line[5:].strip()
        ]
        value = next(value for value in events if value.get("id") == identifier)
    assert "error" not in value, value
    return response, value["result"]


@pytest.mark.parametrize("version", ["2025-03-26", "2025-11-25"])
def test_fixed_sdk_initialize_discover_and_call(mcp_server, version):
    client, directory = mcp_server.client, mcp_server.directory
    headers = {"Authorization": "Bearer " + mcp_server.token, "Accept": "application/json, text/event-stream"}
    assert client.post("/mcp", json={}).status_code == 401
    response, initialized = rpc(
        client,
        headers,
        1,
        "initialize",
        {
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "agentx-sdk-acceptance", "version": "1"},
        },
    )

    assert initialized["protocolVersion"] == version
    headers["MCP-Protocol-Version"] = initialized["protocolVersion"]
    session = response.headers.get("mcp-session-id")
    assert session
    headers["Mcp-Session-Id"] = session
    notification = client.post("/mcp", headers=headers, json={"jsonrpc": "2.0", "method": "notifications/initialized"})
    assert notification.status_code == 202
    _, discovered = rpc(client, headers, 2, "tools/list")
    assert [tool["name"] for tool in discovered["tools"]] == ["echo"]
    _, result = rpc(client, headers, 3, "tools/call", {"name": "echo", "arguments": {"text": "plan9 SDK verification"}})
    assert result.get("isError") is not True
    assert any("plan9 SDK verification" in item.get("text", "") for item in result["content"])
    assert client.delete("/mcp", headers=headers).is_success
    (directory / f"{version}.json").write_text(
        json.dumps(
            {
                "requestedProtocol": version,
                "negotiatedProtocol": initialized["protocolVersion"],
                "sdk": "3.8.0",
                "transport": "streamable-http",
                "tools": ["echo"],
                "status": "passed",
            },
            indent=2,
        )
        + "\n"
    )


@pytest.mark.parametrize("mcp_server", [False, True], indirect=True)
def test_native_opencode_fixed_sdk_discovery(mcp_server):
    native = None
    report = {"passed": False}
    try:
        with isolated_opencode(
            "https://127.0.0.1:1/v1",
            "synthetic-relay-credential",
            mcp_url=str(mcp_server.client.base_url).rstrip("/") + "/mcp",
            mcp_key=mcp_server.token,
        ) as native:
            report["connection"] = native.execute(["mcp", "list"], timeout=60)
            assert "✓ application" in report["connection"], report["connection"]
            report["passed"] = True
    finally:
        if native:
            report["clientLogs"] = native.diagnostics
        (mcp_server.directory / "opencode-native.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
