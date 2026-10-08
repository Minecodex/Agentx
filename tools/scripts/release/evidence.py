"""Candidate identity shared by image builds, E2E evidence and release gates."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
IDENTITY_FIELDS = {"runId", "sourceCommit", "sourceTreeSha256", "imageManifestSha256"}


def source_tree_sha256(root: Path = ROOT) -> str:
    paths = (
        subprocess.run(
            ("git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"),
            cwd=root,
            capture_output=True,
            check=True,
        )
        .stdout.decode()
        .split("\0")
    )
    digest = hashlib.sha256()
    for name in sorted(set(paths)):
        if not (
            name.startswith(("src/", "contracts/", "deploy/", "tools/agentxctl/", "tools/xtask/"))
            or name in {"Cargo.toml", "Cargo.lock", "package.json", "pnpm-lock.yaml", "pnpm-workspace.yaml"}
        ):
            continue
        path = root / name
        if path.is_file():
            digest.update(name.encode() + b"\0" + hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def image_manifest_sha256(images: list[str]) -> str:
    return hashlib.sha256(json.dumps(sorted(set(images)), separators=(",", ":")).encode()).hexdigest()


def validate_identity(identity: dict[str, str]) -> None:
    import re

    if (
        set(identity) != IDENTITY_FIELDS
        or not re.fullmatch(r"[a-f0-9]{40}", identity["sourceCommit"])
        or not identity["runId"]
        or any(not re.fullmatch(r"[a-f0-9]{64}", identity[key]) for key in ("sourceTreeSha256", "imageManifestSha256"))
    ):
        raise ValueError("candidate identity is invalid")


def write_identity(context: dict[str, str], rendered_images: list[str]) -> dict[str, str]:
    root = Path(context["root"])
    commit = subprocess.run(
        ("git", "rev-parse", "HEAD"), cwd=root, capture_output=True, text=True, check=True
    ).stdout.strip()
    images = sorted(set(rendered_images))
    identity = {
        "runId": context["run_id"],
        "sourceCommit": commit,
        "sourceTreeSha256": source_tree_sha256(root),
        "imageManifestSha256": image_manifest_sha256(images),
    }
    path = Path(context["artifact_dir"]) / "candidate-identity.json"
    path.write_text(json.dumps({"identity": identity, "images": images}, indent=2) + "\n", encoding="utf-8")
    return identity


def evidence_identity(context: dict[str, str]) -> dict[str, str]:
    return json.loads((Path(context["artifact_dir"]) / "candidate-identity.json").read_text())["identity"]


def write_report(context: dict[str, str], relative_path: str, payload: dict[str, Any]) -> None:
    report = {**payload, "identity": evidence_identity(context)}
    path = Path(context["artifact_dir"]) / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--print-source-tree-sha", action="store_true", required=True)
    parser.parse_args()
    print(source_tree_sha256())
