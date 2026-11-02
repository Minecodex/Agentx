"""A protocol client independent of Agentx internals; no business execution shortcuts."""

from __future__ import annotations

import hashlib
import json
import uuid
from dataclasses import dataclass, field

import httpx

PROTOCOL = "2025-11-25"
HELPERS = {"get_task", "submit_task_input", "cancel_task"}


@dataclass
class ApplicationMcpClient:
    client: httpx.Client
    path: str
    key: str = field(repr=False)
    host: str = ""
    sequence: int = 0
    records: list[dict] = field(default_factory=list)

    def rpc(self, method, params=None):
        self.sequence += 1
        headers = {
            "Authorization": f"Bearer {self.key}",
            "MCP-Protocol-Version": PROTOCOL,
            "Accept": "application/json, text/event-stream",
        }
        if self.host:
            headers["Host"] = self.host
        response = self.client.post(
            self.path,
            headers=headers,
            json={
                "jsonrpc": "2.0",
                "id": self.sequence,
                "method": method,
                **({"params": params} if params is not None else {}),
            },
        )
        assert response.status_code == 200, f"{method}: HTTP {response.status_code}: {response.text}"
        if "application/json" in response.headers.get("content-type", ""):
            body = response.json()
        else:
            events = [
                json.loads(line[5:])
                for line in response.text.splitlines()
                if line.startswith("data:") and line[5:].strip()
            ]
            body = next(event for event in events if event.get("id") == self.sequence)
        self.records.append(
            {
                "sequence": self.sequence,
                "method": method,
                "tool": (params or {}).get("name"),
                "parametersHash": hashlib.sha256(json.dumps(params, sort_keys=True).encode()).hexdigest(),
                "response": body,
            }
        )
        assert "error" not in body, body
        return body["result"]

    def initialize(self):
        result = self.rpc(
            "initialize",
            {
                "protocolVersion": PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": "plan9-independent-protocol-client", "version": "1"},
            },
        )
        assert result["protocolVersion"] == PROTOCOL, result
        return result

    def tools(self, business):
        tools = self.rpc("tools/list")["tools"]
        assert {tool["name"] for tool in tools} == {business, *HELPERS}, tools
        assert len(tools) == 4, tools
        for tool in tools:
            assert tool["inputSchema"]["type"] == "object"
            assert tool["outputSchema"]["type"] == "object"
        return tools

    def call(self, name, arguments, *, error=False):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        assert (result.get("isError") is True) == error, result
        if error:
            return result
        task = result.get("structuredContent")
        assert task and task.get("taskId"), result
        assert task["callerType"] == ("user" if self.key.startswith("ory_at_") else "api_key"), task
        return task

    def submit(self, business, revision, inputs, *, request_id=None, session_id=None):
        return self.call(
            business,
            {
                "requestId": request_id or str(uuid.uuid4()),
                "publicationRevision": revision,
                "input": inputs,
                **({"sessionId": session_id} if session_id else {}),
            },
        )

    def task(self, task_id):
        return self.call("get_task", {"taskId": task_id})

    def input(self, task_id, wait_id, content, *, request_id=None):
        return self.call(
            "submit_task_input",
            {"taskId": task_id, "waitId": wait_id, "content": content, "requestId": request_id or str(uuid.uuid4())},
        )

    def cancel(self, task_id, *, error=False):
        return self.call("cancel_task", {"taskId": task_id, "requestId": str(uuid.uuid4())}, error=error)
