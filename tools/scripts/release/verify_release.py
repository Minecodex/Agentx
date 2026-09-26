"""Verify the release version and every public image rendered by the packaged CLI."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import yaml

from tools.scripts.release.package_agentxctl import release_images, run

ROOT = Path(__file__).resolve().parents[3]


def verify_versions(version: str, root: Path = ROOT) -> None:
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    cli = tomllib.loads((root / "tools/agentxctl/Cargo.toml").read_text(encoding="utf-8"))
    values = yaml.safe_load((root / "deploy/values/dockerhub-beta.yaml").read_text(encoding="utf-8"))
    if workspace["workspace"]["package"]["version"] != version or cli["package"]["version"] != {"workspace": True}:
        raise ValueError("release tag, workspace and agentxctl versions must agree")
    if values["global"]["images"]["tag"] != f"v{version}":
        raise ValueError("Docker Hub image tag differs from the release")
    for chart in (root / "deploy/helm").glob("*/Chart.yaml"):
        metadata = yaml.safe_load(chart.read_text(encoding="utf-8"))
        if metadata["version"] != version or metadata["appVersion"] != version:
            raise ValueError(f"Chart version differs from the release: {chart}")
    readme = (root / "README.md").read_text(encoding="utf-8")
    for asset in ("agentxctl-linux-x86_64", "agentxctl-windows-x86_64.exe"):
        if f"/agentxctl-v{version}/{asset}" not in readme:
            raise ValueError(f"README does not download this release: {asset}")


def inspect_image(image: str) -> dict[str, object]:
    for attempt in range(3):
        completed = subprocess.run(
            ("docker", "manifest", "inspect", "--verbose", image),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            timeout=180,
        )
        if completed.returncode == 0:
            break
        if attempt == 2:
            raise RuntimeError(f"cannot inspect public image {image}: {completed.stderr.strip()}")
        time.sleep(attempt + 1)
    payload = json.loads(completed.stdout)
    entries = payload if isinstance(payload, list) else [payload]
    manifests = [entry["Descriptor"] for entry in entries]
    runnable = [item for item in manifests if item.get("platform", {}).get("os") != "unknown"]
    if len(runnable) != 1 or runnable[0].get("platform") != {"os": "linux", "architecture": "amd64"}:
        raise ValueError(f"release image must provide exactly linux/amd64: {image}")
    return {"image": image, "digest": runnable[0]["digest"], "platform": "linux/amd64"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--binary", type=Path, help="Also verify all public images rendered by this CLI")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    version = args.version.removeprefix("agentxctl-v")
    verify_versions(version)
    receipt: dict[str, object] = {"version": version}
    if args.binary:
        binary = args.binary.resolve(strict=True)
        with tempfile.TemporaryDirectory(prefix="agentx-release-images-") as temporary:
            cwd = Path(temporary)
            if run(binary, ("--version",), cwd) != f"agentxctl {version}":
                raise ValueError("packaged CLI version differs from the release")
            images = release_images(run(binary, ("render", "--target", "all"), cwd), version)
        with ThreadPoolExecutor(max_workers=3) as executor:
            receipt["images"] = list(executor.map(inspect_image, images))
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()


def verify_signatures(receipts: dict) -> bool:
    """plan7 P7-D7: verify cosign signatures when the tool is available."""
    import shutil
    import subprocess

    if shutil.which("cosign") is None:
        print("cosign not installed; signature verification skipped")
        return True
    images = [item["image"] for item in receipts.get("images", [])]
    failed = [
        image for image in images if subprocess.run(["cosign", "verify", image], capture_output=True).returncode != 0
    ]
    if failed:
        print(f"signature verification failed for {len(failed)} images", file=sys.stderr)
        return False
    print(f"signatures verified for {len(images)} images")
    return True
