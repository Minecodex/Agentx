#!/usr/bin/env python3
"""plan7 P7-D7 local signing-chain drill (INT-011 local scope).

Runs the real supply chain against an ephemeral local registry:

1. start registry:2 on an ephemeral host port;
2. push the 11 local agentx images (docker agentx/<service>:dev);
3. generate SBOMs with syft and sign + attest every image with cosign
   (local key pair, non-interactive password);
4. verify signatures and attestations against the public key;
5. write the evidence JSON (and the passed marker) to --output.

Usage:
    uv run python tools/scripts/release/local_signing_drill.py \\
        --output .local/dist/local-signing-drill.json
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
COSIGN_PASSWORD = "agentx-local-drill"  # noqa: S105 -- throwaway local-only key
SERVICES = [
    "agentx-migrate",
    "agentx-bootstrap",
    "agentx-doctor",
    "platform-control",
    "web-console",
    "runtime-gateway",
    "workflow-runtime",
    "workflow-worker",
    "sandbox-manager",
    "agentx-egress-gateway",
    "observability",
]


def run(command: list[str], *, check: bool = True, timeout: int = 600) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(command, capture_output=True, text=True, check=False, timeout=timeout)
    if check and completed.returncode != 0:
        raise SystemExit(
            f"command failed ({completed.returncode}): {' '.join(command)}\n{completed.stdout}\n{completed.stderr}"
        )
    return completed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(".local/dist/local-signing-drill.json"))
    parser.add_argument("--registry-port", type=int, default=5061)
    args = parser.parse_args()

    missing = [tool for tool in ("docker", "syft", "cosign") if shutil.which(tool) is None]
    if missing:
        print(f"missing tools: {', '.join(missing)} (brew install syft cosign)", file=sys.stderr)
        return 2

    started = time.time()
    registry = f"localhost:{args.registry_port}"
    container = run(["docker", "run", "-d", "--rm", "-p", f"{args.registry_port}:5000", "registry:2"]).stdout.strip()
    try:
        deadline = time.time() + 60
        while time.time() < deadline:
            probe = run(
                ["docker", "exec", container, "wget", "-qO-", "http://127.0.0.1:5000/v2/"], check=False, timeout=30
            )
            if probe.returncode == 0:
                break
            time.sleep(2)
        else:
            raise SystemExit("local registry did not become ready")

        images: list[dict[str, str]] = []
        for service in SERVICES:
            local = f"agentx/{service}:dev"
            remote = f"{registry}/agentx/{service}:dev"
            run(["docker", "tag", local, remote])
            run(["docker", "push", remote], timeout=1200)
            digest = run(["docker", "inspect", "--format", "{{index .RepoDigests 0}}", remote]).stdout.strip()
            digest = digest.split("@", 1)[1] if "@" in digest else ""
            images.append({"image": remote, "digest": digest, "service": service})

        key_dir = args.output.parent / "cosign"
        key_dir.mkdir(parents=True, exist_ok=True)
        key = key_dir / "cosign.key"
        pub = key_dir / "cosign.pub"
        if not key.is_file():
            # generate-key-pair writes <prefix>.key / <prefix>.pub
            run(["cosign", "generate-key-pair", "--output-key-prefix", str(key_dir / "cosign")])
        signed = 0
        attested = 0
        sbom_dir = args.output.parent / "sboms"
        sbom_dir.mkdir(parents=True, exist_ok=True)
        for item in images:
            ref = item["image"]
            sbom_path = sbom_dir / f"{item['service']}.spdx.json"
            run(["syft", ref, "-o", f"spdx-json={sbom_path}"], timeout=1200)
            if run(["cosign", "sign", "--yes", "--key", str(key), ref], check=False).returncode == 0:
                signed += 1
            else:
                print(f"sign failed: {ref}", file=sys.stderr)
            if (
                run(
                    [
                        "cosign",
                        "attest",
                        "--yes",
                        "--key",
                        str(key),
                        "--predicate",
                        str(sbom_path),
                        "--type",
                        "spdxjson",
                        ref,
                    ],
                    check=False,
                ).returncode
                == 0
            ):
                attested += 1
            else:
                print(f"attest failed: {ref}", file=sys.stderr)

        verified = 0
        attestations_verified = 0
        for item in images:
            ref = item["image"]
            if run(["cosign", "verify", "--key", str(pub), ref], check=False).returncode == 0:
                verified += 1
            if (
                run(
                    ["cosign", "verify-attestation", "--key", str(pub), "--type", "spdxjson", ref],
                    check=False,
                ).returncode
                == 0
            ):
                attestations_verified += 1

        ok = (
            signed == len(images)
            and attested == len(images)
            and verified == len(images)
            and attestations_verified == len(images)
        )
        evidence = {
            "schemaVersion": 1,
            "drill": "local-signing-chain",
            "status": "passed" if ok else "failed",
            "environment": "local-registry",
            "registry": registry,
            "imageCount": len(images),
            "signedImageCount": signed,
            "attestedImageCount": attested,
            "verifiedSignatureCount": verified,
            "verifiedAttestationCount": attestations_verified,
            "elapsedSeconds": round(time.time() - started, 1),
            "images": images,
            "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        }
        args.output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(
            json.dumps(
                {
                    key: evidence[key]
                    for key in ("status", "signedImageCount", "verifiedSignatureCount", "verifiedAttestationCount")
                }
            )
        )
        return 0 if ok else 1
    finally:
        run(["docker", "rm", "-f", container], check=False, timeout=120)


if __name__ == "__main__":
    sys.exit(main())
