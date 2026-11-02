"""An isolated loopback OAuth callback. Codes never enter logs or evidence files."""

from contextlib import contextmanager
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Event, Thread
from urllib.parse import parse_qs, urlsplit

from tests.e2e.product.live_text_support import Secret


@dataclass
class Callback:
    state: str
    url: str = ""
    code: Secret = field(default_factory=lambda: Secret(""), repr=False)
    error: str = ""
    completed: Event = field(default_factory=Event, repr=False)


@contextmanager
def oauth_callback(state):
    callback = Callback(state)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, _format, *_args):
            pass

        def do_GET(self):
            uri = urlsplit(self.path)
            query = parse_qs(uri.query)
            valid = uri.path == "/oauth/callback" and query.get("state") == [callback.state]
            if valid:
                callback.code.value = query.get("code", [""])[0]
                callback.error = query.get("error", [""])[0]
                callback.completed.set()
            body = (
                "<!doctype html><html lang='zh'><meta charset='utf-8'><title>Agentx OAuth</title><p>已收到授权结果，请返回客户端。</p></html>"
                if valid
                else "Invalid callback"
            ).encode()
            self.send_response(200 if valid else 400)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    callback.url = f"http://127.0.0.1:{server.server_port}/oauth/callback"
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield callback
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        callback.code.value = ""
