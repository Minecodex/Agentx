"""Regression cases for fail-open capacity and release evidence bugs."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from tests.e2e.capacity.thresholds import LIMITS, REQUIRED_SCENARIOS, capacity_problems
from tools.scripts.release.release_summary import REQUIRED_JUNIT, main, read_report, validate_domains
from tools.scripts.release.signature_verification import digest_reference, verify_signatures


def valid_capacity():
    groups = {group: {field: 0 for field in fields} for group, fields in LIMITS.items()}
    groups["gateway"].update(totalRequests=100, completedExecutions=100, transport="in-cluster-http")
    groups["sse"]["peakLiveConnections"] = 200
    return {
        "groups": groups,
        "baselineVerified": True,
        "matrix": [
            {
                "scenario": name,
                "status": "passed",
                "load": {"completedExecutions": 100, **groups["gateway"]},
                "observed": {"nodes": 500, "attempts": 5000, "workflowNodes": 200},
                "measurements": {"connections": 200, "peakLiveConnections": 200},
                "completedCases": 1000,
            }
            for name in REQUIRED_SCENARIOS
        ]
        + [
            {
                "scenario": f"replicas:{name}:{replicas}",
                "status": "passed",
                "load": {**groups["gateway"], "gatewayTargets": {f"gateway-{index}": 5 for index in range(replicas)}},
            }
            for name in ("runtime-gateway", "workflow-runtime", "workflow-worker", "observability")
            for replicas in (1, 2, 3, 4)
        ],
        "replicasObserved": [1, 2, 3, 4],
        "mixedWorkerVersionsVerified": True,
        "stabilityDurationSeconds": 7200,
        "stabilityMeasurementSamples": 720,
        "stabilityMaxMeasurementGapSeconds": 10,
    }


def test_capacity_rejects_missing_measurements_and_all_401_runs():
    good = valid_capacity()
    assert not capacity_problems(good)
    good["groups"]["gateway"]["nonSuccessRate"] = 1
    good["groups"]["gateway"]["completedExecutions"] = 0
    assert capacity_problems(good)
    missing = valid_capacity()
    del missing["groups"]["mysql"]["poolWaitP95Ms"]
    assert any("poolWaitP95Ms" in problem for problem in capacity_problems(missing))
    missing = valid_capacity()
    missing["stabilityDurationSeconds"] = 30
    assert capacity_problems(missing)


@pytest.mark.parametrize("invalid", [float("nan"), float("inf"), -1, None, True, "7200"])
@pytest.mark.parametrize(
    "field", ["stabilityDurationSeconds", "stabilityMeasurementSamples", "stabilityMaxMeasurementGapSeconds"]
)
def test_capacity_rejects_invalid_stability_measurements(field, invalid):
    report = valid_capacity()
    report[field] = invalid
    assert capacity_problems(report)


@pytest.mark.parametrize("invalid", [float("nan"), float("inf"), None, True, "5000"])
@pytest.mark.parametrize(
    ("scenario", "group", "field"),
    [
        ("100-executions", "load", "completedExecutions"),
        ("500-nodes", "observed", "nodes"),
        ("200-sse", "measurements", "connections"),
        ("1000-cases", None, "completedCases"),
        ("200-node-workflow", "observed", "workflowNodes"),
        ("5000-attempts", "observed", "attempts"),
    ],
)
def test_capacity_rejects_invalid_observed_work(scenario, group, field, invalid):
    report = valid_capacity()
    row = next(item for item in report["matrix"] if item["scenario"] == scenario)
    (row[group] if group else row)[field] = invalid
    assert capacity_problems(report)


@pytest.mark.parametrize("invalid", [float("nan"), float("inf"), None, True, "100"])
@pytest.mark.parametrize("field", ["totalRequests", "completedExecutions"])
def test_capacity_requires_valid_gateway_counts(field, invalid):
    report = valid_capacity()
    report["groups"]["gateway"][field] = invalid
    assert capacity_problems(report)


def recovery_evidence(tmp_path):
    identity = {"runId": "recovery-fixture"}
    backup = tmp_path / "backup"
    backup.mkdir()
    pitr = {
        "status": "passed",
        "identity": identity,
        "providerReceipt": {
            "status": "passed",
            "recoveryPointUtc": "2026-10-07T00:00:00Z",
            "objectCount": 114,
            "contentSha256": "a" * 64,
            "schemaVersionObserved": "control-0013",
        },
        "verification": {"preBackupRowRestored": True, "postBackupRowLost": True, "liveDatabaseUntouched": True},
    }
    redis = {
        "status": "passed",
        "identity": identity,
        "rebuildSeconds": 0.2,
        "verification": {"gatewayAcceptedAfterLoss": True, "invocationCompletedAfterLoss": True},
    }
    return identity, backup, pitr, redis


@pytest.mark.parametrize("seconds", [0, 0.2, 300, -1, float("nan"), float("inf"), True, None, "0.2"])
def test_release_recovery_gate_rejects_invalid_rebuild_duration(tmp_path, seconds):
    identity, backup, pitr, redis = recovery_evidence(tmp_path)
    redis["rebuildSeconds"] = seconds
    (backup / "pitr-report.json").write_text(json.dumps(pitr))
    (backup / "redis-rebuild-report.json").write_text(json.dumps(redis))
    result = validate_domains(tmp_path, tmp_path, {}, identity)["backup-recovery"]
    valid = type(seconds) in (int, float) and 0 <= seconds <= 300
    assert result["status"] == ("passed" if valid else "failed")


@pytest.mark.parametrize("count", [114, float("nan"), float("inf"), 0, -1, True, 1.5, None, "114"])
def test_release_recovery_gate_requires_a_positive_integer_object_count(tmp_path, count):
    identity, backup, pitr, redis = recovery_evidence(tmp_path)
    pitr["providerReceipt"]["objectCount"] = count
    (backup / "pitr-report.json").write_text(json.dumps(pitr))
    (backup / "redis-rebuild-report.json").write_text(json.dumps(redis))
    result = validate_domains(tmp_path, tmp_path, {}, identity)["backup-recovery"]
    assert result["status"] == ("passed" if type(count) is int and count > 0 else "failed")


@pytest.mark.parametrize("count", [1, 1.0, float("nan"), float("inf"), 0, -1, True, None, "1"])
def test_release_rolling_gate_requires_integer_completed_traffic(tmp_path, count):
    identity = {"runId": "rolling-fixture"}
    upgrade = tmp_path / "upgrade-rolling"
    upgrade.mkdir()
    report = {
        "status": "passed",
        "identity": identity,
        "probe": {"failures": [], "accepted": count, "completed": count},
    }
    (upgrade / "rolling-upgrade-report.json").write_text(json.dumps(report))
    result = validate_domains(tmp_path, tmp_path, {}, identity)["rolling-upgrade"]
    assert result["status"] == ("passed" if type(count) is int and count > 0 else "failed")


def test_missing_junit_removes_previous_passed_marker(tmp_path: Path):
    marker = tmp_path / "passed"
    marker.write_text("old passed")
    with pytest.raises(ValueError, match="JUnit domains"):
        main(["--evidence", str(tmp_path), "--dist", str(tmp_path), "--passed-marker", str(marker)])
    assert not marker.exists()
    assert len(REQUIRED_JUNIT) == 9


def test_report_cannot_mix_candidates(tmp_path: Path):
    report = tmp_path / "report.json"
    report.write_text(json.dumps({"status": "passed", "identity": {"runId": "old"}}))
    with pytest.raises(ValueError, match="identity mismatch"):
        read_report(report, {"runId": "new"})


def test_unsigned_tags_and_missing_cosign_fail_closed(monkeypatch):
    with pytest.raises(ValueError):
        digest_reference({"image": "registry.test/image:latest"})
    digest = "sha256:" + "a" * 64
    assert (
        digest_reference({"image": "localhost:5000/repo/image:tag", "digest": digest})
        == f"localhost:5000/repo/image@{digest}"
    )
    monkeypatch.setattr("tools.scripts.release.signature_verification.shutil.which", lambda _: None)
    with pytest.raises(RuntimeError, match="cosign is required"):
        verify_signatures({"images": []}, key="test.pub")


def test_loadgen_does_not_accept_auth_failures_or_unfinished_execution():
    import asyncio

    import httpx

    from tests.e2e.capacity.loadgen import drive_invocations

    def denied(request):
        assert request.headers["authorization"] == "Bearer auth-fixture"
        return httpx.Response(200 if request.url.path == "/health/ready" else 401)

    report = asyncio.run(
        drive_invocations(
            ["http://gateway.test"],
            "actual-app",
            ["auth-fixture"],
            1,
            3,
            max_requests=3,
            transport=httpx.MockTransport(denied),
        )
    )
    assert report.summary()["nonSuccessRate"] == 1
    assert report.summary()["completedExecutions"] == 0
    assert report.summary()["acceptP95Ms"] is None

    def failed(request):
        if request.method == "POST":
            return httpx.Response(202, json={"id": "invocation-fixture"})
        return httpx.Response(200, json={"status": "failed"})

    report = asyncio.run(
        drive_invocations(
            ["http://gateway.test"],
            "actual-app",
            ["auth-fixture"],
            1,
            3,
            max_requests=2,
            transport=httpx.MockTransport(failed),
        )
    )
    assert report.summary()["acceptedExecutions"] == 2
    assert report.summary()["completedExecutions"] == 0
    assert report.summary()["nonSuccessRate"] == 1


def test_public_publish_requires_a_fresh_gate_before_contacting_github(tmp_path, monkeypatch):
    import sys

    from tools.scripts.release import publish_release
    from tools.scripts.release.package_agentxctl import TARGETS, create_package

    source = tmp_path / "binary"
    source.write_bytes(b"unit-fixture")
    for target in TARGETS:
        create_package(source, "1.2.3-test", target, tmp_path)
    receipt = {
        "version": "1.2.3-test",
        "images": [{}] * 11,
        "status": "passed",
        "manifestSignatureVerified": True,
        "verifiedImages": [{}] * 11,
    }
    (tmp_path / "release-images.json").write_text(json.dumps(receipt))
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "publish_release",
            "--tag",
            "agentxctl-v1.2.3-test",
            "--directory",
            str(tmp_path),
            "--notes",
            str(tmp_path / "notes.md"),
        ],
    )
    calls = []
    monkeypatch.setattr(publish_release, "gh", lambda *args: calls.append(args))
    with pytest.raises(FileNotFoundError):
        publish_release.main()
    assert not calls


def test_capacity_soak_cannot_hide_a_failing_burst():
    report = valid_capacity()
    report["matrix"][0]["load"]["acceptP95Ms"] = 501
    assert any("matrix:" in item and "acceptP95Ms" in item for item in capacity_problems(report))


def test_capacity_requires_live_sse_and_traffic_to_every_gateway_replica():
    report = valid_capacity()
    report["groups"]["sse"]["peakLiveConnections"] = 199
    assert any("simultaneous" in item for item in capacity_problems(report))
    report = valid_capacity()
    row = next(item for item in report["matrix"] if item["scenario"] == "replicas:runtime-gateway:4")
    row["load"]["gatewayTargets"] = {"only-one-pod": 100}
    assert any("exercise every replica" in item for item in capacity_problems(report))


def test_capacity_certification_rejects_port_forward_latency():
    report = valid_capacity()
    report["matrix"][0]["load"]["transport"] = "local-http"
    assert any("not generated inside" in item for item in capacity_problems(report))


def test_loadgen_distributes_accepted_traffic_across_gateway_pods():
    import asyncio

    import httpx

    from tests.e2e.capacity.loadgen import drive_invocations

    accepted_hosts = []

    def handler(request):
        if request.method == "POST":
            accepted_hosts.append(request.url.host)
            return httpx.Response(202, json={"id": "invocation-fixture"})
        return httpx.Response(200, json={"status": "completed"})

    report = asyncio.run(
        drive_invocations(
            ["http://gateway-a.test", "http://gateway-b.test"],
            "actual-app",
            ["auth-fixture"],
            2,
            3,
            max_requests=4,
            transport=httpx.MockTransport(handler),
        )
    )
    assert accepted_hosts == ["gateway-a.test", "gateway-b.test", "gateway-a.test", "gateway-b.test"]
    assert report.summary()["completedExecutions"] == 4
    assert len(report.summary()["gatewayTargets"]) == 2
