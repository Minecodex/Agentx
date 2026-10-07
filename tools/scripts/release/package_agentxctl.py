"""Build and smoke-test the five-file agentxctl release package."""

from __future__ import annotations

import argparse
import hashlib
import re
import shutil
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
TARGETS = {
    "x86_64-pc-windows-msvc": ("agentxctl.exe", "agentxctl-windows-x86_64.exe", "zip"),
    "x86_64-unknown-linux-musl": ("agentxctl", "agentxctl-linux-x86_64", "tar.gz"),
}
VALUES = ("local.yaml", "dockerhub-beta.yaml", "production.example.yaml")


def checksum(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def create_package(binary: Path, version: str, target: str, output: Path) -> tuple[Path, Path]:
    executable, standalone_name, extension = TARGETS[target]
    name = f"agentxctl-{version}-{target}"
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{name}.{extension}"
    standalone = output / standalone_name
    shutil.copy2(binary, standalone)
    with tempfile.TemporaryDirectory(prefix="agentxctl-package-") as directory:
        package = Path(directory) / name
        (package / "values").mkdir(parents=True)
        shutil.copy2(binary, package / executable)
        shutil.copy2(ROOT / "LICENSE", package / "LICENSE")
        for value in VALUES:
            shutil.copy2(ROOT / "deploy/values" / value, package / "values" / value)
        files = sorted(path for path in package.rglob("*") if path.is_file())
        if extension == "zip":
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as stream:
                for path in files:
                    stream.write(path, path.relative_to(package.parent).as_posix())
        else:
            with tarfile.open(archive, "w:gz") as stream:
                for path in files:
                    stream.add(path, arcname=path.relative_to(package.parent).as_posix(), recursive=False)
    for path in (archive, standalone):
        path.with_name(path.name + ".sha256").write_text(f"{checksum(path)}  {path.name}\n", encoding="utf-8")
    return archive, standalone


def unpack_package(archive: Path, destination: Path, version: str, target: str) -> Path:
    executable, _, extension = TARGETS[target]
    name = f"agentxctl-{version}-{target}"
    expected = {f"{name}/LICENSE", f"{name}/{executable}", *(f"{name}/values/{value}" for value in VALUES)}
    if extension == "zip":
        with zipfile.ZipFile(archive) as stream:
            if set(stream.namelist()) != expected or len(stream.infolist()) != len(expected):
                raise ValueError("release archive differs from the five-file contract")
            stream.extractall(destination)  # noqa: S202 -- every member is checked against exact safe names
    else:
        with tarfile.open(archive) as stream:
            if set(stream.getnames()) != expected or len(stream.getmembers()) != len(expected):
                raise ValueError("release archive differs from the five-file contract")
            stream.extractall(destination, filter="data")
    return destination / name


def run(binary: Path, args: tuple[str, ...], cwd: Path) -> str:
    result = subprocess.run(
        (str(binary), *args),
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=120,
    )
    return result.stdout.strip()


def release_images(rendered: str, version: str) -> list[str]:
    images = sorted(set(re.findall(r"""^\s+image:\s*["']?(kakj/agentx-[^\s"']+)""", rendered, re.MULTILINE)))
    if len(images) != 11 or any(not image.endswith(f":v{version}") for image in images):
        raise ValueError("embedded deployment must contain all 11 Docker Hub images at the CLI release version")
    return images


def smoke_package(archive: Path, standalone: Path, version: str, target: str) -> None:
    executable = TARGETS[target][0]
    with tempfile.TemporaryDirectory(prefix="agentxctl-release-smoke-") as directory:
        cwd = Path(directory)
        package = unpack_package(archive, cwd, version, target)
        binary = package / executable
        for candidate in (binary, standalone):
            if run(candidate, ("--version",), cwd) != f"agentxctl {version}":
                raise ValueError("release version does not match the binary")
            run(candidate, ("validate", "--output", "json"), cwd)
            release_images(run(candidate, ("render", "--target", "all"), cwd), version)
        values = str(package / "values/local.yaml")
        run(binary, ("validate", "--values", values, "--output", "json"), cwd)
        run(binary, ("render", "--values", values, "--target", "runtime"), cwd)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--binary", type=Path)
    args = parser.parse_args()
    version = args.version.removeprefix("agentxctl-v")
    if not version or any(
        character not in "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-+" for character in version
    ):
        raise ValueError("invalid release version")
    binary = (args.binary or ROOT / "target" / args.target / "release" / TARGETS[args.target][0]).resolve(strict=True)
    archive, standalone = create_package(binary, version, args.target, ROOT / ".local/dist")
    smoke_package(archive, standalone, version, args.target)
    print(f"Verified {archive.name} and {standalone.name}")


if __name__ == "__main__":
    main()
