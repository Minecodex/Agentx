"""IM platform mock server (plan7 P7-A A7).

Serves one HTTP server that emulates the three platforms' send APIs with
behavior switching by path prefix:

- /dingtalk/* : sessionWebhook (errcode envelope), /v1.0/* official API
- /feishu/*   : tenant_access_token + im/v1/messages ({code,data})
- /wecom/*     : gettoken + message/send ({errcode,access_token})

Behaviors: ok (default), rate-limit (429 / errcode 429), unauthorized
(401/60011), not-found (404 / errcode 500 invalid chat id). Requests are
logged so the E2E can assert the mock received the reply.
"""
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

RECEIVED = []
LOCK = threading.Lock()
# Failure behaviors are transient per path: rate-limit fails the first two
# hits, unauthorized the first one, so the delivery loop's backoff/retry and
# dead-letter replay drills can observe recovery without fixture restarts.
# flaky: pod-kill/resilience drills need a wide backoff window before recovery.
TRANSIENT_FAILURES = {"rate-limit": 2, "unauthorized": 1, "flaky": 6}
PATH_HITS = {}


def resolve_behavior(path):
    last = path.rstrip("/").split("/")[-1]
    for known in ("rate-limit", "unauthorized", "flaky", "slow", "not-found", "ok"):
        if last == known or last.endswith("-" + known):
            if known in TRANSIENT_FAILURES:
                with LOCK:
                    hits = PATH_HITS.get(path, 0)
                    PATH_HITS[path] = hits + 1
                    if hits < TRANSIENT_FAILURES[known]:
                        return known
                # Transient failure window elapsed: the provider recovered.
                return "ok"
            return known
    return "ok"


def record(entry):
    with LOCK:
        RECEIVED.append(entry)


def received_snapshot():
    with LOCK:
        return list(RECEIVED)


class Handler(BaseHTTPRequestHandler):
    def _read_body(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            return json.loads(raw or b"{}")
        except json.JSONDecodeError:
            return {"_raw": raw.decode("utf-8", "replace")}

    def _reply(self, status, payload, headers=None):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = self._read_body()
        path = self.path
        behavior = resolve_behavior(path)
        record({"path": path, "behavior": behavior, "body": body, "at": time.time()})

        if path.startswith("/dingtalk/"):
            # Official robot API subpaths (token then batch/group send).
            if "oauth2/accessToken" in path:
                if behavior == "unauthorized":
                    return self._reply(200, {"code": 401, "msg": "invalid app credential"})
                return self._reply(200, {"code": 0, "accessToken": "dt-mock-token", "expireIn": 7200})
            if "oToMessages/batchSend" in path or "groupMessages/send" in path:
                if behavior == "rate-limit":
                    return self._reply(429, {"code": 429, "msg": "too many requests"})
                if behavior == "not-found":
                    return self._reply(200, {"code": 404, "msg": "conversation not found"})
                return self._reply(200, {"code": 0, "message_id": "dt-mock-official-1", "processQueryKey": "dt-official-pqk"})
            if behavior in ("rate-limit", "flaky"):
                return self._reply(429, {"errcode": 429, "errmsg": "too many requests"})
            if behavior == "slow":
                # Hold the delivery loop in-flight so a crash mid-send can be
                # exercised; the reply still succeeds afterwards.
                time.sleep(60)
            if behavior == "unauthorized":
                return self._reply(401, {"errcode": 601, "errmsg": "unauthorized"})
            if behavior == "not-found":
                return self._reply(200, {"errcode": 500, "errmsg": "invalid conversation"})
            # sessionWebhook & official API success shapes (message_id matches
            # the delivery client's provider-message extraction key)
            return self._reply(200, {"errcode": 0, "errmsg": "ok", "message_id": "dt-mock-1", "processQueryKey": "dt-mock-pqk"})

        if path.startswith("/feishu/"):
            if "tenant_access_token" in path:
                if behavior == "unauthorized":
                    return self._reply(200, {"code": 99991663, "msg": "app secret is empty"})
                return self._reply(200, {"code": 0, "msg": "ok", "tenant_access_token": "t-mock", "expire": 7200})
            if behavior == "rate-limit":
                return self._reply(429, {"code": 99991400, "msg": "too many requests"})
            if behavior == "unauthorized":
                return self._reply(200, {"code": 99991663, "msg": "app secret is empty"})
            if behavior == "not-found":
                return self._reply(200, {"code": 230002, "msg": "chat not found"})
            return self._reply(200, {"code": 0, "data": {"message_id": "fs-mock-1"}, "msg": "ok"})

        if path.startswith("/wecom/"):
            if "gettoken" in path:
                if behavior == "unauthorized":
                    return self._reply(200, {"errcode": 40001, "errmsg": "invalid credential"})
                return self._reply(200, {"errcode": 0, "access_token": "wx-mock"})
            if behavior == "rate-limit":
                return self._reply(429, {"errcode": 45009, "errmsg": "api freq out of limit"})
            if behavior == "unauthorized":
                return self._reply(200, {"errcode": 40001, "errmsg": "invalid credential"})
            if behavior == "not-found":
                return self._reply(200, {"errcode": 86004, "errmsg": "invalid chatid"})
            return self._reply(200, {"errcode": 0, "msgid": "wx-mock-1"})

        return self._reply(404, {"error": "unknown mock path"})

    def do_GET(self):
        if self.path == "/health":
            return self._reply(200, {"status": "ok"})
        if self.path == "/received":
            return self._reply(200, {"items": received_snapshot()})
        if self.path == "/reset":
            with LOCK:
                RECEIVED.clear()
                PATH_HITS.clear()
            return self._reply(200, {"status": "ok"})
        return self._reply(404, {"error": "not found"})

    def log_message(self, _format, *_args):
        return


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8090), Handler).serve_forever()  # noqa: S104 -- in-cluster fixture
