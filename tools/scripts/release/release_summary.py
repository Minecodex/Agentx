#!/usr/bin/env python3
"""plan7 P7-D8 release summarizer (V2C-006 + INT-014).

Aggregates every release-gate evidence domain and writes a summary report;
the `passed` marker file is only written when ALL required domains are green
(any missing or failed domain leaves no marker, mirroring the m7 release gate
behaviour: 阈值不足不生成 passed 文件).

Domains and their sources:
- junit <name>=<path> (repeatable): business/security/... pytest JUnit XML,
  failures=0, errors=0, skipped=0 required.
- capacity: newest `<evidence-run>/capacity/*.json` load report; frozen
  ceilings (p95 <= 500ms, p99 <= 2000ms) are re-asserted here.
- backup-recovery: `<evidence-run>/backup/pitr-report.json` and
  `redis-rebuild-report.json`, status=passed with the five-field receipt.
- rolling-upgrade: `<evidence-run>/upgrade-rolling/rolling-upgrade-report.json`,
  status=passed with zero probe losses.
- supply-chain: `<dist>/supply-chain-evidence.json`, status=passed.

Usage:
    uv run python tools/scripts/release/release_summary.py \
        --evidence .local/artifacts/e2e --dist .local/dist \
        --junit business=.local/artifacts/junit/business.xml \
        --output .local/dist/release-summary.json \
        --passed-marker .local/dist/release-gate.passed
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import xml.etree.ElementTree as ElementTree
from pathlib import Path
from typing import Any

CAPACITY_P95_LIMIT_MS = 500.0
CAPACITY_P99_LIMIT_MS = 2000.0
RECEIPT_FIELDS = {"status", "recoveryPointUtc", "objectCount", "contentSha256", "schemaVersionObserved"}


def _newest(root: Path, pattern: str) -> Path | None:
    # Accept both the runs root (<run>/<domain>/...) and a single run dir
    # (<domain>/...) so the summarizer works per-run or across runs.
    candidates = list(root.glob(f"*/{pattern}")) + list(root.glob(pattern))
    candidates.sort(key=lambda path: path.stat().st_mtime, reverse=True)
    return candidates[0] if candidates else None


def _check_junit(path: Path) -> dict[str, Any]:
    # Local pytest output, not untrusted input.
    root = ElementTree.parse(path).getroot()  # noqa: S314
    suites = [root] if root.tag == "testsuite" else list(root.iter("testsuite"))
    tests = sum(int(suite.get("tests", 0)) for suite in suites)
    failures = sum(int(suite.get("failures", 0)) for suite in suites)
    errors = sum(int(suite.get("errors", 0)) for suite in suites)
    skipped = sum(int(suite.get("skipped", 0)) for suite in suites)
    passed = failures == 0 and errors == 0 and skipped == 0 and tests > 0
    return {
        "status": "passed" if passed else "failed",
        "evidence": str(path),
        "tests": tests,
        "failures": failures,
        "errors": errors,
        "skipped": skipped,
    }


def _check_capacity(evidence_root: Path) -> dict[str, Any]:
    path = _newest(evidence_root, "capacity/*.json")
    if path is None:
        return {"status": "missing"}
    summary = json.loads(path.read_text(encoding="utf-8")).get("summary", {})
    breaches = []
    if not summary.get("totalRequests"):
        breaches.append("no requests recorded")
    if float(summary.get("acceptP95Ms", 0)) > CAPACITY_P95_LIMIT_MS:
        breaches.append(f"p95 {summary.get('acceptP95Ms')}ms > {CAPACITY_P95_LIMIT_MS}ms")
    if float(summary.get("acceptP99Ms", 0)) > CAPACITY_P99_LIMIT_MS:
        breaches.append(f"p99 {summary.get('acceptP99Ms')}ms > {CAPACITY_P99_LIMIT_MS}ms")
    if float(summary.get("nonSuccessRate", 0)) == 1.0:
        breaches.append("every request failed")
    return {
        "status": "failed" if breaches else "passed",
        "evidence": str(path),
        "summary": summary,
        "breaches": breaches,
    }


def _check_backup_recovery(evidence_root: Path) -> dict[str, Any]:
    checks: dict[str, Any] = {}
    for domain, filename in (("pitr", "pitr-report.json"), ("redis-rebuild", "redis-rebuild-report.json")):
        path = _newest(evidence_root, f"backup/{filename}")
        if path is None:
            checks[domain] = {"status": "missing"}
            continue
        report = json.loads(path.read_text(encoding="utf-8"))
        receipt = report.get("providerReceipt") or {}
        problems = []
        if report.get("status") != "passed":
            problems.append("report status is not passed")
        if domain == "pitr" and set(receipt) != RECEIPT_FIELDS:
            problems.append(f"receipt fields {sorted(receipt)} != {sorted(RECEIPT_FIELDS)}")
        checks[domain] = {
            "status": "failed" if problems else "passed",
            "evidence": str(path),
            "problems": problems,
        }
    overall = (
        "passed"
        if all(check["status"] == "passed" for check in checks.values())
        else ("missing" if all(check["status"] == "missing" for check in checks.values()) else "failed")
    )
    return {"status": overall, **checks}


def _check_rolling_upgrade(evidence_root: Path) -> dict[str, Any]:
    path = _newest(evidence_root, "upgrade-rolling/rolling-upgrade-report.json")
    if path is None:
        return {"status": "missing"}
    report = json.loads(path.read_text(encoding="utf-8"))
    problems = []
    if report.get("status") != "passed":
        problems.append("report status is not passed")
    if report.get("probe", {}).get("failures"):
        problems.append(f"probe losses: {report['probe']['failures'][:3]}")
    return {
        "status": "failed" if problems else "passed",
        "evidence": str(path),
        "problems": problems,
    }


def _check_supply_chain(dist: Path) -> dict[str, Any]:
    path = dist / "supply-chain-evidence.json"
    if not path.is_file():
        return {"status": "missing"}
    evidence = json.loads(path.read_text(encoding="utf-8"))
    return {
        "status": "passed" if evidence.get("status") == "passed" else "failed",
        "evidence": str(path),
        "signed": evidence.get("signedImageCount"),
        "attested": evidence.get("attestedImageCount"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, default=Path(".local/artifacts/e2e"))
    parser.add_argument("--dist", type=Path, default=Path(".local/dist"))
    parser.add_argument(
        "--junit",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="labelled JUnit XML evidence (repeatable)",
    )
    parser.add_argument("--output", type=Path, default=Path(".local/dist/release-summary.json"))
    parser.add_argument("--passed-marker", type=Path, default=Path(".local/dist/release-gate.passed"))
    args = parser.parse_args()

    domains: dict[str, Any] = {}
    for labelled in args.junit:
        name, _, path = labelled.partition("=")
        if not name or not path:
            raise SystemExit(f"--junit expects NAME=PATH, got {labelled!r}")
        junit_path = Path(path)
        domains[f"junit:{name}"] = (
            _check_junit(junit_path) if junit_path.is_file() else {"status": "missing", "evidence": path}
        )
    domains["capacity"] = _check_capacity(args.evidence)
    domains["backup-recovery"] = _check_backup_recovery(args.evidence)
    domains["rolling-upgrade"] = _check_rolling_upgrade(args.evidence)
    domains["supply-chain"] = _check_supply_chain(args.dist)

    all_passed = all(domain["status"] == "passed" for domain in domains.values())
    summary = {
        "schemaVersion": 1,
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "overall": "passed" if all_passed else "blocked",
        "domains": domains,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    # The marker is the gate: any missing or failed domain removes it.
    args.passed_marker.unlink(missing_ok=True)
    if all_passed:
        args.passed_marker.write_text(
            json.dumps({"generatedAt": summary["generatedAt"], "summary": str(args.output)}, indent=2) + "\n",
            encoding="utf-8",
        )
    for name, domain in domains.items():
        print(f"{domain['status']:7} {name}: {domain.get('evidence', '')}")
    print(f"{summary['overall']}: {args.output}")
    return 0 if all_passed else 1


if __name__ == "__main__":
    sys.exit(main())
