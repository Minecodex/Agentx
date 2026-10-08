from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tomllib
import zipfile
from pathlib import Path

import pytest

from tools.scripts.release import publish_release
from tools.scripts.release.package_agentxctl import TARGETS, checksum, create_package, release_images, unpack_package
from tools.scripts.release.verify_release import inspect_image, verify_versions


@pytest.mark.parametrize("target", TARGETS)
def test_release_archive_preserves_the_binary_and_exact_contents(tmp_path: Path, target: str) -> None:
    source = tmp_path / "source-binary"
    source.write_bytes(b"release-fixture-binary\x00\x01")
    source.chmod(0o755)
    archive, standalone = create_package(source, "1.2.3-test", target, tmp_path / "output")
    package = unpack_package(archive, tmp_path / "extracted", "1.2.3-test", target)
    assert (package / TARGETS[target][0]).read_bytes() == source.read_bytes()
    assert standalone.read_bytes() == source.read_bytes()
    assert (package / "LICENSE").is_file()
    assert len([path for path in package.rglob("*") if path.is_file()]) == 5
    for artifact in (archive, standalone):
        assert artifact.with_name(artifact.name + ".sha256").read_text(encoding="utf-8") == (
            f"{checksum(artifact)}  {artifact.name}\n"
        )


def test_release_archive_rejects_extra_files(tmp_path: Path) -> None:
    source = tmp_path / "source-binary"
    source.write_bytes(b"release-fixture")
    target = "x86_64-pc-windows-msvc"
    archive, _ = create_package(source, "1.2.3-test", target, tmp_path / "output")
    with zipfile.ZipFile(archive, "a") as stream:
        stream.writestr("unexpected.txt", "unexpected")
    with pytest.raises(ValueError, match="five-file contract"):
        unpack_package(archive, tmp_path / "extracted", "1.2.3-test", target)


def test_release_version_matches_workspace_charts_images_and_readme() -> None:
    version = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    verify_versions(version)
    with pytest.raises(ValueError, match="versions must agree"):
        verify_versions("99.0.0-test")


def test_packaged_cli_rejects_missing_or_stale_embedded_images() -> None:
    images = [f"kakj/agentx-service-{index}:v1.2.3-test" for index in range(11)]
    rendered = "\n".join(f'  image: "{image}"' for image in images)
    assert release_images(rendered, "1.2.3-test") == sorted(images)
    # Repeated init containers are expected; external dependency images do not belong to this release.
    assert release_images(rendered + '\n  image: mysql:8.4\n  image: "' + images[0] + '"', "1.2.3-test") == sorted(
        images
    )
    for invalid in (
        rendered.replace(images[0], "mysql:8.4"),
        rendered.replace(images[0], "kakj/agentx-service-0:v1.2.2"),
    ):
        with pytest.raises(ValueError, match="all 11 Docker Hub images"):
            release_images(invalid, "1.2.3-test")


def test_public_image_requires_linux_amd64(monkeypatch: pytest.MonkeyPatch) -> None:
    payload = [
        {"Descriptor": {"digest": "sha256:release", "platform": {"os": "linux", "architecture": "amd64"}}},
        {"Descriptor": {"digest": "sha256:attestation", "platform": {"os": "unknown", "architecture": "unknown"}}},
    ]

    def manifest(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(args, 0, json.dumps(payload), "")

    monkeypatch.setattr("tools.scripts.release.verify_release.subprocess.run", manifest)
    assert inspect_image("kakj/agentx-test:v1.2.3")["digest"] == "sha256:release"
    payload[0]["Descriptor"]["platform"]["architecture"] = "arm64"
    with pytest.raises(ValueError, match="exactly linux/amd64"):
        inspect_image("kakj/agentx-test:v1.2.3")


def test_public_image_reports_registry_failure_after_bounded_retries(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = []

    def unavailable(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append(args)
        return subprocess.CompletedProcess(args, 1, "", "TLS handshake timeout")

    monkeypatch.setattr("tools.scripts.release.verify_release.subprocess.run", unavailable)
    monkeypatch.setattr("tools.scripts.release.verify_release.time.sleep", lambda _: None)
    with pytest.raises(RuntimeError, match="TLS handshake timeout"):
        inspect_image("kakj/agentx-test:v1.2.3")
    assert len(calls) == 3


@pytest.mark.parametrize("state", ["missing", "draft", "published", "lookup_failure", "upload_failure"])
def test_release_stays_private_until_every_asset_is_uploaded(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    state: str,
) -> None:
    source = tmp_path / "binary"
    source.write_bytes(b"release-fixture")
    for target in TARGETS:
        create_package(source, "1.2.3-test", target, tmp_path)
    identity = {
        "runId": "unit-fixture",
        "sourceCommit": "a" * 40,
        "sourceTreeSha256": "b" * 64,
        "imageManifestSha256": "c" * 64,
    }
    (tmp_path / "release-images.json").write_text(
        json.dumps(
            {
                "version": "1.2.3-test",
                "images": [{}] * 11,
                "identity": identity,
                "status": "passed",
                "verifiedImages": [{}] * 11,
                "manifestSignatureVerified": True,
            }
        )
    )
    from tools.scripts.release.release_summary import REQUIRED_JUNIT

    domains = {f"junit:{name}" for name in REQUIRED_JUNIT} | {
        "capacity",
        "backup-recovery",
        "rolling-upgrade",
        "supply-chain",
    }
    summary = tmp_path / "release-summary.json"
    summary.write_text(
        json.dumps(
            {"identity": identity, "overall": "passed", "domains": {name: {"status": "passed"} for name in domains}}
        )
    )
    (tmp_path / "release-gate.passed").write_text(
        json.dumps({"identity": identity, "summarySha256": hashlib.sha256(summary.read_bytes()).hexdigest()})
    )
    (tmp_path / "supply-chain-evidence.json").write_text(
        json.dumps(
            {"identity": identity, "status": "passed", "trustScope": "release-policy", "environment": "release-ci"}
        )
    )
    notes = tmp_path / "notes.md"
    notes.write_text("Release notes", encoding="utf-8")
    monkeypatch.setattr(
        sys,
        "argv",
        ["publish_release", "--tag", "agentxctl-v1.2.3-test", "--directory", str(tmp_path), "--notes", str(notes)],
    )
    calls: list[tuple[str, ...]] = []

    def github(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
        calls.append(args)
        if args[:2] == ("release", "view") and len(calls) == 1:
            if state in ("missing", "lookup_failure"):
                error = "release not found" if state == "missing" else "connection failed"
                return subprocess.CompletedProcess(args, 1, "", error)
            return subprocess.CompletedProcess(args, 0, json.dumps({"isDraft": state != "published"}), "")
        if args[:2] == ("release", "upload") and state == "upload_failure":
            raise RuntimeError("upload failed")
        return subprocess.CompletedProcess(args, 0, "{}", "")

    monkeypatch.setattr(publish_release, "gh", github)
    if state in ("published", "lookup_failure", "upload_failure"):
        with pytest.raises((ValueError, RuntimeError)):
            publish_release.main()
        assert all(call[:2] != ("release", "edit") for call in calls)
        if state != "upload_failure":
            assert len(calls) == 1
    else:
        publish_release.main()
        actions = [call[1] for call in calls]
        assert actions.index("upload") < actions.index("edit")
        upload = next(call for call in calls if call[1] == "upload")
        assert len(upload[3:-1]) == 12
