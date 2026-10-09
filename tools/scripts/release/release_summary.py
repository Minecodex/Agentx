"""Require complete, candidate-bound E2E evidence before creating a release marker."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
import time
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any

from tests.e2e.capacity.thresholds import capacity_problems
from tools.scripts.release.evidence import image_manifest_sha256, validate_identity
from tools.scripts.release.signature_verification import digest_reference

REQUIRED_JUNIT = {
    "infrastructure",
    "publishing",
    "gateway",
    "runtime",
    "observability",
    "security",
    "upgrade",
    "product",
    "capacity",
}
RECEIPT_FIELDS = {"status", "recoveryPointUtc", "objectCount", "contentSha256", "schemaVersionObserved"}


def read_report(path: Path, identity: dict[str, str]) -> dict[str, Any]:
    if not path.is_file():
        raise ValueError(f"missing evidence: {path}")
    report = json.loads(path.read_text(encoding="utf-8"))
    if report.get("identity") != identity:
        raise ValueError(f"candidate/run identity mismatch: {path}")
    if report.get("status") != "passed":
        raise ValueError(f"evidence is not passed: {path}")
    return report


def check_junit(path: Path, name: str, identity: dict[str, str]) -> dict[str, Any]:
    root = ET.parse(path).getroot()  # noqa: S314 -- local pytest evidence
    properties = {item.get("name"): item.get("value") for item in root.iter("property")}
    if any(properties.get(f"agentx.{key}") != value for key, value in identity.items()):
        raise ValueError(f"JUnit candidate/run identity mismatch: {path}")
    cases = [case for case in root.iter("testcase") if case.get("classname", "").startswith(f"tests.e2e.{name}.")]
    if not cases:
        raise ValueError(f"JUnit contains no {name} E2E cases: {path}")
    failed = [
        case.get("name") for case in cases if any(case.find(tag) is not None for tag in ("failure", "error", "skipped"))
    ]
    if failed:
        raise ValueError(f"JUnit has failed or skipped {name} cases: {failed}")
    return {
        "status": "passed",
        "evidence": str(path),
        "tests": len(cases),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }


def validate_domains(
    evidence: Path, dist: Path, labelled_junit: dict[str, Path], identity: dict[str, str]
) -> dict[str, Any]:
    domains = {}

    def check(name: str, operation):
        try:
            result = operation()
            domains[name] = {"status": "passed", **(result or {})}
        except (ValueError, KeyError, TypeError, OSError, ET.ParseError) as error:
            domains[name] = {"status": "failed", "problem": str(error)}

    for name in sorted(REQUIRED_JUNIT):
        check(f"junit:{name}", lambda name=name: check_junit(labelled_junit[name], name, identity))

    def capacity():
        path = evidence / "capacity/capacity-report.json"
        report = read_report(path, identity)
        if problems := capacity_problems(report):
            raise ValueError("; ".join(problems))
        return {"evidence": str(path)}

    def backup():
        pitr = read_report(evidence / "backup/pitr-report.json", identity)
        receipt = pitr["providerReceipt"]
        if (
            set(receipt) != RECEIPT_FIELDS
            or receipt["status"] != "passed"
            or not re.fullmatch(r"[a-f0-9]{64}", receipt["contentSha256"])
            or type(receipt["objectCount"]) is not int
            or receipt["objectCount"] <= 0
            or not receipt["schemaVersionObserved"]
        ):
            raise ValueError("PITR receipt is incomplete or invalid")
        if not all(
            pitr["verification"].get(key) is True
            for key in ("preBackupRowRestored", "postBackupRowLost", "liveDatabaseUntouched")
        ):
            raise ValueError("PITR assertions are incomplete")
        redis = read_report(evidence / "backup/redis-rebuild-report.json", identity)
        seconds = redis["rebuildSeconds"]
        if (
            type(seconds) not in (int, float)
            or (type(seconds) is float and not math.isfinite(seconds))
            or not 0 <= seconds <= 300
        ) or not all(
            redis["verification"].get(key) is True
            for key in ("gatewayAcceptedAfterLoss", "invocationCompletedAfterLoss")
        ):
            raise ValueError("Redis rebuild exceeds the frozen gate or lacks business verification")

    def upgrade():
        report = read_report(evidence / "upgrade-rolling/rolling-upgrade-report.json", identity)
        probe = report["probe"]
        accepted, completed = probe.get("accepted"), probe.get("completed")
        if (
            probe["failures"]
            or type(accepted) is not int
            or type(completed) is not int
            or accepted <= 0
            or completed != accepted
        ):
            raise ValueError("Rolling probe has losses or no completed traffic")

    def supply_chain():
        report = read_report(dist / "supply-chain-evidence.json", identity)
        if (
            any(
                report.get(field) != 11
                for field in ("imageCount", "signedImageCount", "attestedImageCount", "verifiedImageCount")
            )
            or report.get("manifestSignatureVerified") is not True
        ):
            raise ValueError("All 11 image signatures and attestations must be verified")
        if len(report["images"]) != 11 or len({image["image"] for image in report["images"]}) != 11:
            raise ValueError("Supply-chain image coverage is incomplete")
        receipt = dist / report["receipt"]
        if (
            not receipt.resolve().is_relative_to(dist.resolve())
            or hashlib.sha256(receipt.read_bytes()).hexdigest() != report["receiptSha256"]
        ):
            raise ValueError("supply-chain image receipt was replaced")
        receipt_data = json.loads(receipt.read_text())
        if (
            receipt_data.get("identity") != identity
            or {digest_reference(item) for item in receipt_data["images"]}
            != {item["image"] for item in report["images"]}
            or image_manifest_sha256([item["image"] for item in report["images"]]) != identity["imageManifestSha256"]
        ):
            raise ValueError("signed images differ from the certified candidate")
        for image in report["images"]:
            if digest_reference(image) != image["image"]:
                raise ValueError("supply-chain image digest mismatch")
            path = dist / image["sbom"]
            if not path.resolve().is_relative_to(dist.resolve()) or not path.is_file():
                raise ValueError("SBOM is missing or outside the release directory")
            sbom = json.loads(path.read_text())
            if (
                not str(sbom.get("spdxVersion", "")).startswith("SPDX-2.")
                or hashlib.sha256(path.read_bytes()).hexdigest() != image["sbomSha256"]
                or hashlib.sha256(json.dumps(sbom, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
                != image["attestedSbomCanonicalSha256"]
            ):
                raise ValueError("SBOM does not match the verified SPDX attestation")

    check("capacity", capacity)
    check("backup-recovery", backup)
    check("rolling-upgrade", upgrade)
    check("supply-chain", supply_chain)
    return domains


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", required=True, type=Path, help="one E2E run directory")
    parser.add_argument("--dist", required=True, type=Path)
    parser.add_argument("--junit", action="append", default=[], metavar="DOMAIN=PATH")
    parser.add_argument("--output", type=Path, default=Path(".local/dist/release-summary.json"))
    parser.add_argument("--passed-marker", type=Path, default=Path(".local/dist/release-gate.passed"))
    args = parser.parse_args(argv)
    args.passed_marker.unlink(missing_ok=True)
    paths = {}
    for value in args.junit:
        name, separator, path = value.partition("=")
        if not separator or name in paths or name not in REQUIRED_JUNIT:
            raise ValueError("--junit must contain each required domain exactly once")
        paths[name] = Path(path)
    if paths.keys() != REQUIRED_JUNIT:
        raise ValueError(f"required JUnit domains missing: {sorted(REQUIRED_JUNIT - paths.keys())}")
    candidate = json.loads((args.evidence / "candidate-identity.json").read_text())
    identity = candidate["identity"]
    validate_identity(identity)
    if (
        len(set(candidate["images"])) != 11
        or image_manifest_sha256(candidate["images"]) != identity["imageManifestSha256"]
        or any(not re.search(r"@sha256:[a-f0-9]{64}$", image) for image in candidate["images"])
    ):
        raise ValueError("release gate requires the 11 immutable images actually installed in the E2E run")
    domains = validate_domains(args.evidence, args.dist, paths, identity)
    passed = all(domain["status"] == "passed" for domain in domains.values())
    summary = {
        "schemaVersion": 2,
        "identity": identity,
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "overall": "passed" if passed else "blocked",
        "domains": domains,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if passed:
        args.passed_marker.write_text(
            json.dumps(
                {
                    "identity": identity,
                    "summary": str(args.output.resolve()),
                    "summarySha256": hashlib.sha256(args.output.read_bytes()).hexdigest(),
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
    print(json.dumps({"overall": summary["overall"], "domains": domains}, ensure_ascii=False))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
