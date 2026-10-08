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
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

from tools.scripts.release.evidence import image_manifest_sha256, validate_identity
from tools.scripts.release.signature_verification import REQUIRED_IMAGE_COUNT, digest_reference, verify_signatures


def run(
    command: list[str], *, check: bool = True, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, capture_output=True, text=True, check=False, timeout=600, env=env)
    if check and result.returncode != 0:
        raise SystemExit(f"command failed ({result.returncode}): {' '.join(command)}\n{result.stdout}\n{result.stderr}")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--receipts", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--key-ref", default=None, help="cosign key reference (defaults to keyless OIDC)")
    parser.add_argument("--skip-sign", action="store_true")
    parser.add_argument("--verify-key")
    parser.add_argument("--certificate-identity")
    parser.add_argument("--certificate-oidc-issuer")
    parser.add_argument("--registry-ca", type=Path)
    parser.add_argument("--local-registry", action="store_true")
    args = parser.parse_args()

    args.output.unlink(missing_ok=True)
    receipts = json.loads(args.receipts.read_text(encoding="utf-8"))
    images: list[str] = [digest_reference(item) for item in receipts.get("images", [])]
    validate_identity(receipts["identity"])
    if (
        image_manifest_sha256(images) != receipts["identity"]["imageManifestSha256"]
        or receipts.get("sourceCommit") != receipts["identity"]["sourceCommit"]
    ):
        raise ValueError("image receipts do not match the certified candidate")
    if len(images) != REQUIRED_IMAGE_COUNT or len(set(images)) != REQUIRED_IMAGE_COUNT:
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
    registry_env = dict(os.environ)
    if args.registry_ca:
        registry_env["SYFT_REGISTRY_CA_CERT"] = str(args.registry_ca.resolve())
    for image in images:
        name = image.split("/")[-1].split("@")[0]
        sbom_path = sbom_dir / f"{name}.spdx.json"
        run(["syft", f"registry:{image}", "-o", f"spdx-json={sbom_path}"], env=registry_env)
        print(f"sbom: {sbom_path.name}")

    signed = 0
    attested = 0
    if not args.skip_sign:
        sign_args = ["cosign", "sign", "--yes"]
        if args.key_ref:
            sign_args += ["--key", args.key_ref]
        attest_args = ["cosign", "attest", "--yes"]
        if args.key_ref:
            attest_args += ["--key", args.key_ref]
        if args.registry_ca:
            sign_args += ["--registry-cacert", str(args.registry_ca)]
            attest_args += ["--registry-cacert", str(args.registry_ca)]
        if args.local_registry:
            if not args.key_ref or not args.registry_ca:
                raise ValueError("local registry signing requires an explicit key and CA")
            signing_config = args.output.parent / "local-signing-config.json"
            run(["cosign", "signing-config", "create", "--out", str(signing_config)])
            sign_args += ["--signing-config", str(signing_config)]
            attest_args += ["--signing-config", str(signing_config)]
        for image in images:
            result = run([*sign_args, image], check=False)
            if result.returncode == 0:
                signed += 1
            else:
                print(f"sign failed for {image}: {result.stderr.strip()}", file=sys.stderr)
        for image in images:
            predicate = sbom_dir / f"{image.split('/')[-1].split('@')[0]}.spdx.json"
            result = run([*attest_args, "--predicate", str(predicate), "--type", "spdxjson", image], check=False)
            if result.returncode == 0:
                attested += 1
            else:
                print(f"attest failed for {image}: {result.stderr.strip()}", file=sys.stderr)

    verified = []
    verification_key = args.verify_key
    if not args.skip_sign and signed == REQUIRED_IMAGE_COUNT and attested == REQUIRED_IMAGE_COUNT:
        if args.key_ref and not verification_key:
            public_key = args.output.parent / "cosign.pub"
            public_key.write_text(run(["cosign", "public-key", "--key", args.key_ref]).stdout, encoding="utf-8")
            verification_key = str(public_key)
        verified = verify_signatures(
            receipts,
            key=verification_key,
            identity=args.certificate_identity,
            issuer=args.certificate_oidc_issuer,
            registry_ca=args.registry_ca,
            local_registry=args.local_registry,
        )
    evidence = {
        "schemaVersion": "agentx.io/supply-chain-evidence/v2",
        "status": "passed" if len(verified) == REQUIRED_IMAGE_COUNT else "incomplete",
        "environment": "local-tls-registry" if args.local_registry else "release-ci",
        "registry": "local-tls" if args.local_registry else "docker.io",
        "trustScope": "local-key-and-ca" if args.local_registry else "release-policy",
        "isolationLevel": "signed-and-attested",
        "version": receipts.get("version", "dev"),
        "sourceCommit": receipts["sourceCommit"],
        "identity": receipts["identity"],
        "receipt": args.receipts.name,
        "receiptSha256": hashlib.sha256(args.receipts.read_bytes()).hexdigest(),
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "imageCount": REQUIRED_IMAGE_COUNT,
        "signedImageCount": signed if not args.skip_sign else 0,
        "attestedImageCount": attested if not args.skip_sign else 0,
        "manifestSignatureVerified": len(verified) == REQUIRED_IMAGE_COUNT,
        "verifiedImageCount": len(verified),
        "images": [
            {
                "image": image,
                "digest": image.rsplit("@", 1)[1],
                "sbom": f"sboms/{image.split('/')[-1].split('@')[0]}.spdx.json",
                "sbomSha256": hashlib.sha256(
                    (sbom_dir / f"{image.split('/')[-1].split('@')[0]}.spdx.json").read_bytes()
                ).hexdigest(),
                "attestedSbomCanonicalSha256": next(
                    (item["sbomCanonicalSha256"] for item in verified if item["reference"] == image), None
                ),
            }
            for image in images
        ],
    }
    args.output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"evidence: {args.output}")
    return 0 if evidence["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
