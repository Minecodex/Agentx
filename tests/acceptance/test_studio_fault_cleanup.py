"""Fault tests must restore serving routes before the next business mutation."""

from types import SimpleNamespace

import httpx
import pytest

from tests.e2e.product import studio_builder_faults as faults


def test_cleanup_waits_for_draining_replicas_and_public_routes(monkeypatch):
    commands, probes = [], []

    def run(command, **kwargs):
        commands.append(command)
        return SimpleNamespace(
            json=lambda: {
                "items": [
                    {"metadata": {"name": "draining", "deletionTimestamp": "2026-10-16T07:17:02Z"}},
                    {"metadata": {"name": "serving"}},
                ]
            }
        )

    responses = iter((503, 401, httpx.ConnectError("restarting tunnel"), 200))

    def get(url, **kwargs):
        assert any("pod/draining" in command and "--for=delete" in command for command in commands)
        probes.append(url)
        result = next(responses)
        if isinstance(result, Exception):
            raise result
        return httpx.Response(result)

    monkeypatch.setattr(faults, "run", run)
    monkeypatch.setattr(faults.httpx, "get", get)
    monkeypatch.setattr(faults.time, "sleep", lambda _: None)
    faults.restore_builder_services(
        {"control_namespace": "control", "runtime_namespace": "runtime"},
        {"web": "http://web.invalid", "runtime": "http://runtime.invalid"},
    )
    assert probes == ["http://web.invalid/api/v1/auth/me"] * 2 + ["http://runtime.invalid/health/ready"] * 2
    assert not any("pod/serving" in command for command in commands)


def test_cleanup_fails_when_the_control_route_never_recovers(monkeypatch):
    monkeypatch.setattr(faults, "run", lambda *args, **kwargs: SimpleNamespace(json=lambda: {"items": []}))
    monkeypatch.setattr(faults.httpx, "get", lambda *args, **kwargs: httpx.Response(503))
    times = iter((0, 61))
    monkeypatch.setattr(faults.time, "monotonic", lambda: next(times))
    with pytest.raises(AssertionError, match=r"did not restore http://web\.invalid/api/v1/auth/me"):
        faults.restore_builder_services(
            {"control_namespace": "control", "runtime_namespace": "runtime"},
            {"web": "http://web.invalid", "runtime": "http://runtime.invalid"},
        )
