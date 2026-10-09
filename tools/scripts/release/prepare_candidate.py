"""Resolve the packaged CLI's public images, validate build identity and pin E2E values."""

from __future__ import annotations

import argparse
import json
import subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import yaml

from tools.scripts.release.evidence import image_manifest_sha256, source_tree_sha256, validate_identity
from tools.scripts.release.package_agentxctl import release_images, run
from tools.scripts.release.signature_verification import digest_reference
from tools.scripts.release.verify_release import ROOT, inspect_image, verify_versions


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--values", type=Path, required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    version = args.version.removeprefix("agentxctl-v")
    verify_versions(version)
    binary = args.binary.resolve(strict=True)
    images = release_images(
        run(binary, ("render", "--values", str(args.values.resolve()), "--target", "all"), ROOT), version
    )
    with ThreadPoolExecutor(max_workers=3) as executor:
        receipts = list(executor.map(inspect_image, images))
    commit = subprocess.run(
        ("git", "rev-parse", "HEAD"), cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.strip()
    tree = source_tree_sha256()
    for image in receipts:
        reference = digest_reference(image)
        subprocess.run(("docker", "pull", reference), check=True, capture_output=True, timeout=600)
        labels = json.loads(
            subprocess.run(
                ("docker", "image", "inspect", reference, "--format", "{{json .Config.Labels}}"),
                check=True,
                capture_output=True,
                text=True,
                timeout=30,
            ).stdout
        )
        if (
            labels.get("org.opencontainers.image.revision") != commit
            or labels.get("io.agentx.source-tree-sha256") != tree
        ):
            raise ValueError(f"image was built from a different source candidate: {reference}")
    identity = {
        "runId": args.run_id,
        "sourceCommit": commit,
        "sourceTreeSha256": tree,
        "imageManifestSha256": image_manifest_sha256([digest_reference(image) for image in receipts]),
    }
    validate_identity(identity)
    args.directory.mkdir(parents=True, exist_ok=True)
    (args.directory / "release-images-inspected.json").write_text(
        json.dumps(
            {
                "version": version,
                "sourceCommit": commit,
                "identity": identity,
                "images": receipts,
                "status": "inspected",
                "manifestSignatureVerified": False,
            },
            indent=2,
        )
        + "\n"
    )
    values = yaml.safe_load(args.values.read_text())
    mapping = {}
    for service in values["global"]["images"]["services"]:
        leaf = f"{values['global']['images'].get('repositoryPrefix', '')}{service.removeprefix('agentx-')}"
        matched = [image for image in receipts if image["image"].rsplit("/", 1)[1].split(":")[0] == leaf]
        if len(matched) != 1:
            raise ValueError(f"release image mapping is incomplete for {service}")
        mapping[service] = matched[0]["digest"]
    values["global"]["images"]["digests"] = mapping
    values["global"]["images"]["sourceCommit"] = commit
    (args.directory / "values.yaml").write_text(yaml.safe_dump(values, sort_keys=False))


if __name__ == "__main__":
    main()
