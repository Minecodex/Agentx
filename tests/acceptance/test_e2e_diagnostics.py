import json
import subprocess
import threading

import pytest

from tests.e2e import diagnostics
from tests.e2e.support import Result


def test_pending_migration_cannot_hide_mysql_crash_and_init_container_logs(monkeypatch) -> None:
    pods = [
        {
            "metadata": {"name": "control-migrate"},
            "spec": {"initContainers": [{"name": "wait-for-mysql"}], "containers": [{"name": "migrate"}]},
            "status": {
                "initContainerStatuses": [{"name": "wait-for-mysql", "state": {"running": {}}}],
                "containerStatuses": [{"name": "migrate", "state": {"waiting": {"reason": "PodInitializing"}}}],
            },
        },
        {
            "metadata": {"name": "control-mysql-0"},
            "spec": {"containers": [{"name": "mysql"}]},
            "status": {"containerStatuses": [{"name": "mysql", "state": {"running": {}}, "restartCount": 1}]},
        },
    ]
    commands = []

    def command(args, **kwargs):
        commands.append(args)
        if "get" in args:
            return Result(tuple(args), json.dumps({"items": pods}), "", 0)
        container = args[args.index("-c") + 1]
        assert container != "migrate"
        content = "first initialization failed, password=isolated-secret" if "--previous" in args else container
        return Result(tuple(args), content, "", 0)

    monkeypatch.setattr(diagnostics, "run", command)
    content = diagnostics.workload_logs("isolated-test", "agentx.io/plane=control")
    assert "wait-for-mysql" in content and "PodInitializing" in content
    assert "[control-mysql-0/mysql previous]" in content and "first initialization failed" in content
    assert "isolated-secret" not in content
    assert len([command for command in commands if "logs" in command]) == 3


def test_one_log_timeout_cannot_suppress_other_containers(monkeypatch) -> None:
    pods = [
        {
            "metadata": {"name": name},
            "spec": {"containers": [{"name": "app"}]},
            "status": {"containerStatuses": [{"name": "app", "state": {"terminated": {"exitCode": 1}}}]},
        }
        for name in ("timeout", "available")
    ]

    def command(args, **kwargs):
        if "get" in args:
            return Result(tuple(args), json.dumps({"items": pods}), "", 0)
        if "timeout" in args:
            raise subprocess.TimeoutExpired(args, 30)
        return Result(tuple(args), "actionable failure", "", 0)

    monkeypatch.setattr(diagnostics, "run", command)
    content = diagnostics.workload_logs("isolated-test", "agentx.io/plane=control")
    assert "log collection failed" in content and "actionable failure" in content


def test_installation_crash_is_saved_before_atomic_rollback_and_monitor_stops(monkeypatch, tmp_path) -> None:
    captured = threading.Event()
    rollback = False
    calls = []
    pod = {
        "metadata": {"name": "mysql-0", "uid": "isolated-uid"},
        "status": {
            "containerStatuses": [
                {
                    "name": "mysql",
                    "restartCount": 1,
                    "state": {"running": {}},
                    "lastState": {"terminated": {"exitCode": 1}},
                }
            ],
        },
    }

    def command(args, **kwargs):
        calls.append(args)
        if "get" in args:
            return Result(tuple(args), json.dumps({"items": [] if rollback else [pod]}), "", 0)
        assert "--previous" in args
        captured.set()
        return Result(tuple(args), "initialization failed, password=isolated-secret", "", 0)

    monkeypatch.setattr(diagnostics, "run", command)
    with (
        pytest.raises(RuntimeError, match="atomic rollback"),
        diagnostics.installation_diagnostics(["isolated-test"], tmp_path),
    ):
        assert captured.wait(2)
        rollback = True
        raise RuntimeError("atomic rollback")
    content = (tmp_path / "installation-crashes.txt").read_text()
    assert "exit=1" in content and "initialization failed" in content
    assert "isolated-secret" not in content
    assert not any(thread.name == "installation-diagnostics" for thread in threading.enumerate())
    assert len([call for call in calls if "logs" in call]) == 1
