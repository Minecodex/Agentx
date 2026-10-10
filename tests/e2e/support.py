from __future__ import annotations

import json
import os
import re
import signal
import subprocess
import threading
import uuid
import zipfile
from collections.abc import Sequence
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]

# MySQL image distributions and test configurations use different socket
# paths. Administrative assertions run inside the database container and
# always connect to its explicitly bound loopback TCP listener.
MYSQL_ROOT_CLIENT = (
    'mysql_tls="--ssl-mode=DISABLED --get-server-public-key"; '
    'if test -r /tls/ca.crt; then mysql_tls="--ssl-mode=VERIFY_CA --ssl-ca=/tls/ca.crt"; fi; '
    'MYSQL_PWD="$(cat /run/secrets/agentx/root-password)" '
    "mysql --protocol=TCP --host=127.0.0.1 --port=3306 $mysql_tls"
)


def agentxctl() -> str:
    configured = os.getenv("AGENTXCTL_BIN")
    if configured:
        return configured
    suffix = ".exe" if os.name == "nt" else ""
    return str(ROOT / "target" / "debug" / f"agentxctl{suffix}")


def backup_adapter() -> str:
    configured = os.getenv("AGENTX_BACKUP_ADAPTER_BIN")
    if configured:
        return configured
    suffix = ".exe" if os.name == "nt" else ""
    binary = ROOT / "target" / "debug" / f"agentx-backup-test-adapter{suffix}"
    if not binary.is_file():
        completed = subprocess.run(
            ("cargo", "build", "--locked", "-p", "agentx-backup-test-adapter"),
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=1200,
            check=False,
        )
        if completed.returncode != 0 or not binary.is_file():
            raise RuntimeError(f"failed to build E2E backup adapter\n{redact(completed.stdout + completed.stderr)}")
    return str(binary)


@dataclass(frozen=True)
class Result:
    command: tuple[str, ...]
    stdout: str
    stderr: str
    returncode: int

    def json(self) -> Any:
        return json.loads(self.stdout)


@dataclass
class ManagedProcess:
    process: subprocess.Popen[str]
    stdout_handle: Any
    stderr_handle: Any

    def stop(self, timeout: int = 15) -> None:
        if self.process.poll() is None:
            try:
                if os.name == "nt":
                    self.process.send_signal(signal.CTRL_BREAK_EVENT)
                else:
                    os.killpg(self.process.pid, signal.SIGTERM)
                self.process.wait(timeout=timeout)
            except (OSError, ProcessLookupError, subprocess.TimeoutExpired):
                self.process.kill()
                self.process.wait(timeout=5)
        self.stdout_handle.close()
        self.stderr_handle.close()


def start_process(
    command: Sequence[str | Path], *, stdout_path: Path, stderr_path: Path, env: dict[str, str] | None = None
) -> ManagedProcess:
    stdout_path.parent.mkdir(parents=True, exist_ok=True)
    stdout_handle = stdout_path.open("w", encoding="utf-8")
    stderr_handle = stderr_path.open("w", encoding="utf-8")
    kwargs: dict[str, Any] = (
        {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP} if os.name == "nt" else {"start_new_session": True}
    )
    process = subprocess.Popen(
        tuple(str(part) for part in command),
        stdout=stdout_handle,
        stderr=stderr_handle,
        text=True,
        encoding="utf-8",
        errors="replace",
        shell=False,
        env={**os.environ, **(env or {})},
        **kwargs,
    )
    return ManagedProcess(process, stdout_handle, stderr_handle)


class RestartingPortForward:
    """Keep a test URL stable when its target pod is replaced or scaled down."""

    def __init__(self, command: Sequence[str | Path], *, stdout_path: Path, stderr_path: Path):
        self.command = command
        self.stdout_path = stdout_path
        self.stderr_path = stderr_path
        self.stopped = threading.Event()
        self.current = start_process(command, stdout_path=stdout_path, stderr_path=stderr_path)
        self.thread = threading.Thread(target=self._supervise, daemon=True)
        self.thread.start()

    @property
    def process(self):
        return self.current.process

    def _supervise(self):
        restart = 0
        while not self.stopped.wait(0.5):
            if self.current.process.poll() is None:
                continue
            self.current.stop()
            restart += 1
            self.current = start_process(
                self.command,
                stdout_path=self.stdout_path.with_name(f"{self.stdout_path.stem}-reconnect-{restart}.log"),
                stderr_path=self.stderr_path.with_name(f"{self.stderr_path.stem}-reconnect-{restart}.log"),
            )

    def stop(self):
        self.stopped.set()
        self.thread.join(timeout=5)
        self.current.stop()


def redact(value: str) -> str:
    value = re.sub(r"\beyJ[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+\b", "<redacted>", value)
    value = re.sub(r"\baxk_[a-zA-Z0-9_-]{32,}\b", "<redacted>", value)
    value = re.sub(r"(?i)([a-z][a-z0-9+.-]*://)[^/@\s]+:[^/@\s]+@", r"\1<redacted>@", value)
    value = re.sub(
        r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----.*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----",
        "<redacted-private-key>",
        value,
        flags=re.DOTALL,
    )

    def scalar(match: re.Match[str]) -> str:
        label, content = match.group(1), match.group(2)
        quote = content[0] if content.startswith(('"', "'")) else ""
        return f"{label}{quote}<redacted>{quote}"

    return re.sub(
        r"(?i)((?:password|secret|(?<!automountserviceaccount)token|private[_-]?key|unseal[ _-]?key|api[ _-]?key|authorization)[\"']?[ \t]*[=:][ \t]*)"
        r"""("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|(?:(?:Bearer|Basic)[ \t]+)?[^\"'\s,;{}\[\]]+)""",
        scalar,
        value,
    )


def run(
    command: Sequence[str | Path],
    *,
    input_text: str | None = None,
    timeout: int = 600,
    check: bool = True,
    env: dict[str, str] | None = None,
) -> Result:
    completed = subprocess.run(
        tuple(str(part) for part in command),
        cwd=ROOT,
        input=input_text,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        check=False,
        shell=False,
        env={**os.environ, **(env or {})},
    )
    result = Result(tuple(str(part) for part in command), completed.stdout, completed.stderr, completed.returncode)
    if check and result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(result.command)}\n{redact(result.stdout + result.stderr)}"
        )
    return result


def deployment_config(values: Path, run_id: str | None = None) -> dict[str, Any]:
    command = [agentxctl(), "validate", "--values", str(values), "--output", "json"]
    if run_id:
        command.extend(("--run-id", run_id))
    return run(command, timeout=300).json()


def render(values: Path | str, target: str, run_id: str | None = None) -> str:
    command = [agentxctl(), "render", "--values", str(values), "--target", target]
    if run_id:
        command.extend(("--run-id", run_id))
    return run(command, timeout=300).stdout


def redact_browser_artifacts(directory: Path) -> None:
    for path in directory.rglob("*"):
        if not path.is_file():
            continue
        if path.suffix == ".zip":
            temporary = path.with_suffix(".zip.redacting")
            try:
                with zipfile.ZipFile(path) as source, zipfile.ZipFile(temporary, "w") as target:
                    for entry in source.infolist():
                        content = source.read(entry)
                        with suppress(UnicodeDecodeError):
                            content = redact(content.decode("utf-8")).encode("utf-8")
                        target.writestr(entry, content)
                temporary.replace(path)
            finally:
                temporary.unlink(missing_ok=True)
        elif path.suffix in {".json", ".jsonl", ".txt", ".log", ".xml", ".md", ".html"}:
            path.write_text(redact(path.read_text(encoding="utf-8")), encoding="utf-8")


def run_playwright(root: Path, suite: str, tests: Sequence[str], environment: dict[str, str]) -> None:
    pnpm = ("corepack.cmd", "pnpm") if os.name == "nt" else ("corepack", "pnpm")
    snapshot_args = ("--update-snapshots=all",) if environment.get("AGENTX_E2E_UPDATE_SNAPSHOTS") == "1" else ()
    command = [*pnpm, "--filter", "@agentx/e2e", "exec", "playwright", "test", *tests, *snapshot_args]
    environment = {**environment, "AGENTX_E2E_SUITE": suite}
    environment.setdefault("AGENTX_E2E_RUN_ID", uuid.uuid4().hex)
    try:
        subprocess.run(command, check=True, shell=False, env=environment, cwd=str(root))
    finally:
        redact_browser_artifacts(
            root
            / ".local/artifacts/playwright"
            / environment.get("AGENTX_E2E_STAGE", "local")
            / environment["AGENTX_E2E_RUN_ID"]
            / suite
        )
