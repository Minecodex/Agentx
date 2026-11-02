"""The installed, unmodified OpenCode client with a private configuration and credential cache."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path

from tests.e2e.support import ROOT, redact


@dataclass
class OpenCode:
    binary: str
    root: Path
    workspace: Path
    environment: dict = field(repr=False)
    secrets: tuple[str, ...] = field(repr=False)
    active_process: subprocess.Popen | None = field(default=None, repr=False)
    last_output: str = field(default="", repr=False)
    diagnostics: dict[str, str] = field(default_factory=dict, repr=False)
    events: list[dict] = field(default_factory=list, repr=False)

    def stop(self):
        process = self.active_process
        if process and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

    def execute(self, arguments, *, timeout=600):
        process = subprocess.Popen(
            (self.binary, *arguments),
            cwd=self.workspace,
            env=self.environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.active_process = process
        timed_out = False
        try:
            output, errors = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.stop()
            output, errors = process.communicate(timeout=5)
            timed_out = True
        finally:
            self.active_process = None
        for secret in self.secrets:
            if secret:
                output, errors = output.replace(secret, "<redacted>"), errors.replace(secret, "<redacted>")
        self.last_output = redact(output + errors)
        if arguments and arguments[0] == "run":
            for line in redact(output).splitlines():
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                if isinstance(event, dict):
                    self.events.append(event)
        assert not timed_out, "The isolated OpenCode process exceeded its deadline"
        assert process.returncode == 0, self.last_output
        return redact(output)

    def run(self, prompt, *, session=None):
        arguments = ["run", "--model", "apimart/glm-5.3-flash", "--agent", "acceptance", "--format", "json"]
        if session:
            arguments += ["--session", session]
        output = self.execute([*arguments, prompt])
        events = [json.loads(line) for line in output.splitlines() if line.strip().startswith("{")]
        tools = [event["part"] for event in events if event.get("type") == "tool_use"]
        assert tools, "OpenCode must make actual MCP calls; its final prose is not proof"
        assert all(part.get("tool", "").startswith("application_") for part in tools), tools
        return events


@contextmanager
def isolated_opencode(
    endpoint, credential, *, ca=None, mcp_url=None, mcp_key=None, host=None, oauth=None
) -> Iterator[OpenCode]:
    binary = shutil.which("opencode")
    assert binary, "The independent OpenCode 1.1.56 client is required"
    with tempfile.TemporaryDirectory(prefix="plan9-opencode-", dir=ROOT / ".local/tmp") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        workspace = root / "workspace"
        workspace.mkdir()
        for name in ("config", "data", "cache", "state", "home", "managed"):
            (root / name).mkdir(mode=0o700)
        # Do not inherit ambient provider keys, plugins or the user's daily authorization caches.
        environment = {
            key: os.environ[key] for key in ("PATH", "LANG", "LC_ALL", "TMPDIR", "SYSTEMROOT") if key in os.environ
        }
        environment.update(
            {
                "XDG_CONFIG_HOME": str(root / "config"),
                "XDG_DATA_HOME": str(root / "data"),
                "XDG_CACHE_HOME": str(root / "cache"),
                "XDG_STATE_HOME": str(root / "state"),
                "OPENCODE_TEST_HOME": str(root / "home"),
                "OPENCODE_TEST_MANAGED_CONFIG_DIR": str(root / "managed"),
                "OPENCODE_CONFIG": str(root / "opencode.json"),
                "OPENCODE_DISABLE_PROJECT_CONFIG": "1",
                "OPENCODE_DISABLE_CLAUDE_CODE": "1",
                "OPENCODE_DISABLE_EXTERNAL_SKILLS": "1",
                "OPENCODE_DISABLE_DEFAULT_PLUGINS": "1",
                "OPENCODE_DISABLE_LSP_DOWNLOAD": "1",
                "OPENCODE_DISABLE_AUTOUPDATE": "1",
                "OPENCODE_DISABLE_MODELS_FETCH": "1",
                "OPENCODE_EXPERIMENTAL_DISABLE_FILEWATCHER": "1",
                "OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX": "16000",
                "PLAN9_RELAY_TOKEN": credential,
                "PLAN9_MCP_KEY": mcp_key or "",
            }
        )
        if ca:
            environment["NODE_EXTRA_CA_CERTS"] = str(ca)
        config = {
            "$schema": "https://opencode.ai/config.json",
            "model": "apimart/glm-5.3-flash",
            "small_model": "apimart/glm-5.3-flash",
            "enabled_providers": ["apimart"],
            "autoupdate": False,
            "share": "disabled",
            "snapshot": False,
            "plugin": [],
            "permission": {"*": "deny", "application_*": "allow"},
            "agent": {
                "acceptance": {
                    "mode": "primary",
                    "steps": 24,
                    "prompt": "你是企业业务应用的独立 MCP 客户端。只使用当前应用 MCP 工具，不使用文件、Shell、编辑或子 Agent。必须真实调用业务工具并查询原 taskId。waiting_input 时提交用户要求的资料；waiting_approval 时等待人工审批，禁止代为批准。只有原任务 completed 才报告完成；失败或结果未知必须如实说明。",
                }
            },
            "provider": {
                "apimart": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "APIMart metered acceptance",
                    "options": {"baseURL": endpoint, "apiKey": "{env:PLAN9_RELAY_TOKEN}"},
                    "models": {
                        "glm-5.3-flash": {
                            "name": "glm-5.3-flash",
                            "limit": {"context": 64000, "output": 16000},
                            "tool_call": True,
                            "reasoning": True,
                            "attachment": False,
                            "options": {"reasoningEffort": "low"},
                        }
                    },
                }
            },
        }
        if mcp_url:
            server = {"type": "remote", "url": mcp_url, "enabled": True, "timeout": 30000}
            if oauth:
                server["oauth"] = oauth
            else:
                server["oauth"] = False
                server["headers"] = {"Authorization": "Bearer {env:PLAN9_MCP_KEY}"}
            if host:
                server.setdefault("headers", {})["Host"] = host
            config["mcp"] = {"application": server}
        path = root / "opencode.json"
        path.write_text(json.dumps(config, ensure_ascii=False, indent=2))
        path.chmod(0o600)
        client = OpenCode(binary, root, workspace, environment, (credential, mcp_key or ""))
        assert client.execute(["--version"], timeout=30).strip() == "1.1.56", (
            "Acceptance fixes the installed client version"
        )
        try:
            yield client
        finally:
            client.stop()
            for log in root.glob("data/opencode/log/*.log"):
                content = log.read_text(errors="replace")
                for secret in client.secrets:
                    if secret:
                        content = content.replace(secret, "<redacted>")
                client.diagnostics[log.name] = redact(content)
            client.environment.clear()
            client.secrets = ()
