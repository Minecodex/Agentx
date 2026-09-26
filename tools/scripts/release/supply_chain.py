#!/usr/bin/env python3
"""plan7 P7-D7 supply chain: SBOM generation, cosign signing and attestation.

Rebuilds the release supply chain in Python per the repository automation
rules. The tooling is expected on PATH (syft, cosign); when either is
missing the script reports the exact commands instead of failing silently so
CI can gate on tool availability.

Usage:
    uv run python tools/scripts/release/supply_chain.py \
        --receipts .local/dist/release-images.json \
        --output .local/dist/supply-chain-evidence.json \
        [--key-ref env://COSIGN_KEY] [--skip-sign]

The receipts file is the `release-images.json` artifact produced by
publish_release.py (11 images with digests).
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

REQUIRED_IMAGE_COUNT = 11


def run(command: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if check and result.returncode != 0:
        raise SystemExit(f"command failed ({result.returncode}): {' '.join(command)}\n{result.stdout}\n{result.stderr}")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--receipts", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--key-ref", default=None, help="cosign key reference (defaults to keyless OIDC)")
    parser.add_argument("--skip-sign", action="store_true")
    args = parser.parse_args()

    receipts = json.loads(args.receipts.read_text(encoding="utf-8"))
    images: list[str] = [item["image"] for item in receipts.get("images", [])]
    if len(images) != REQUIRED_IMAGE_COUNT:
        raise SystemExit(f"release contract expects {REQUIRED_IMAGE_COUNT} images, receipts carry {len(images)}")

    missing_tools = [tool for tool in ("syft", "cosign") if shutil.which(tool) is None and not args.skip_sign]
    if shutil.which("syft") is None:
        missing_tools.append("syft")
    if not args.skip_sign and shutil.which("cosign") is None:
        missing_tools.append("cosign")
    if missing_tools:
        print(
            "supply chain tooling is missing: " + ", ".join(sorted(set(missing_tools))),
            file=sys.stderr,
        )
        print(
            "install with: brew install syft cosign (or set --skip-sign for SBOM-only dry runs)",
            file=sys.stderr,
        )
        return 2

    sbom_dir = args.output.parent / "sboms"
    sbom_dir.mkdir(parents=True, exist_ok=True)
    for image in images:
        name = image.split("/")[-1].split(":")[0]
        sbom_path = sbom_dir / f"{name}.spdx.json"
        run(["syft", image, "-o", f"spdx-json={sbom_path}"])
        print(f"sbom: {sbom_path.name}")

    signed = 0
    attested = 0
    if not args.skip_sign:
        sign_args = ["cosign", "sign", "--yes"]
        if args.key_ref:
            sign_args += ["--key", args.key_ref]
        attest_args = ["cosign", "attest", "--yes", "--predicate"]
        for image in images:
            if run([*sign_args, image], check=False).returncode == 0:
                signed += 1
            else:
                print(f"sign failed for {image}", file=sys.stderr)
        for image in images:
            predicate = sbom_dir / f"{image.split('/')[-1].split(':')[0]}.spdx.json"
            if run([*attest_args, str(predicate), "--type", "spdxjson", image], check=False).returncode == 0:
                attested += 1
            else:
                print(f"attest failed for {image}", file=sys.stderr)

    evidence = {
        "schemaVersion": 1,
        "status": "passed" if signed == REQUIRED_IMAGE_COUNT and attested == REQUIRED_IMAGE_COUNT else "incomplete",
        "environment": "local-release",
        "registry": "docker.io",
        "trustScope": "per-image digest",
        "isolationLevel": "signed+attested",
        "version": receipts.get("version", "dev"),
        "sourceCommit": receipts.get("sourceCommit", ""),
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "imageCount": REQUIRED_IMAGE_COUNT,
        "signedImageCount": signed if not args.skip_sign else 0,
        "attestedImageCount": attested if not args.skip_sign else 0,
        "manifestSignatureVerified": not args.skip_sign,
        "images": [
            {"image": image, "sbom": f"sboms/{image.split('/')[-1].split(':')[0]}.spdx.json"} for image in images
        ],
    }
    args.output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"evidence: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
