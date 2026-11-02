"""Validate the published TaskView JSON Schema using the installed external client's validator."""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from tests.e2e.product.plan9.opencode_client import isolated_opencode
from tests.e2e.support import ROOT


def test_native_client_task_output_schema(run_id):
    schema = json.loads((ROOT / "contracts/schemas/runtime-v1/McpTaskViewV1.schema.json").read_text())

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            self.send_response(405)
            self.end_headers()

        def do_POST(self):
            message = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if "id" not in message:
                self.send_response(202)
                self.end_headers()
                return
            if message["method"] == "initialize":
                result = {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "schema-only", "version": "1"},
                }
            else:
                assert message["method"] == "tools/list"
                result = {
                    "tools": [
                        {
                            "name": "get_task",
                            "description": "Schema interoperability only",
                            "inputSchema": {"type": "object", "properties": {}},
                            "outputSchema": schema,
                        }
                    ]
                }
            body = json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    directory = ROOT / ".local/artifacts/e2e" / run_id
    directory.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    native = None
    report = {"passed": False, "schemaDialect": schema.get("$schema")}
    try:
        with isolated_opencode(
            "https://127.0.0.1:1/v1",
            "synthetic-relay-credential",
            mcp_url=f"http://127.0.0.1:{server.server_port}/mcp",
            mcp_key="synthetic-mcp-key",
        ) as native:
            report["connection"] = native.execute(["mcp", "list"], timeout=60)
            assert "✓ application" in report["connection"], report["connection"]
            report["passed"] = True
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        if native:
            report["clientLogs"] = native.diagnostics
        (directory / "native-schema.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
