"""A stale pod tunnel must be discovered before the next business request."""

from __future__ import annotations

import socket
import sys
import time

import httpx

from tests.e2e.support import RestartingPortForward


def test_health_probe_reconnects_a_tunnel_that_stays_alive_until_used(tmp_path):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    script = tmp_path / "pod_tunnel.py"
    script.write_text(
        """import os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
root = Path(sys.argv[1])
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if (root / 'stale').exists():
            (root / 'stale').unlink()
            os._exit(1)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(str(os.getpid()).encode())
    def log_message(self, *args):
        pass
server = HTTPServer(('127.0.0.1', int(sys.argv[2])), Handler)
with (root / 'started').open('a') as record:
    record.write(str(os.getpid()) + '\\n')
server.serve_forever()
"""
    )
    url = f"http://127.0.0.1:{port}/health/live"
    forward = RestartingPortForward(
        (sys.executable, script, tmp_path, str(port)),
        stdout_path=tmp_path / "forward.log",
        stderr_path=tmp_path / "forward-error.log",
        health_url=url,
    )
    try:
        started = tmp_path / "started"
        deadline = time.monotonic() + 15
        while not started.exists():
            assert time.monotonic() < deadline
            time.sleep(0.05)
        original = started.read_text().splitlines()[0]
        (tmp_path / "stale").write_text("pod was replaced")
        # Do not send a test request to discover the stale tunnel. The
        # supervisor's read-only probe must trigger it and start a fresh one.
        while len(started.read_text().splitlines()) < 2:
            assert time.monotonic() < deadline
            time.sleep(0.05)
        response = httpx.get(url, timeout=2, trust_env=False)
        assert response.status_code == 200 and response.text != original
    finally:
        forward.stop()
