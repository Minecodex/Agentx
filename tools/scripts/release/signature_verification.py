"""Fail-closed verification of digest-pinned image signatures and SPDX attestations."""

from __future__ import annotations

import base64
import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any

REQUIRED_IMAGE_COUNT = 11


def digest_reference(item: dict[str, Any]) -> str:
    digest = item.get("digest", "")
    image = item.get("image", "")
    if not isinstance(image, str) or not image or not re.fullmatch(r"sha256:[a-f0-9]{64}", digest):
        raise ValueError("every image receipt must contain a valid image and sha256 digest")
    repository = image.split("@", 1)[0]
    prefix, separator, leaf = repository.rpartition("/")
    leaf = leaf.split(":", 1)[0]
    repository = f"{prefix}{separator}{leaf}"
    return f"{repository}@{digest}"


def verification_args(
    *, key: str | None, identity: str | None, issuer: str | None, registry_ca: Path | None, local_registry: bool
) -> list[str]:
    if key:
        args = ["--key", key]
    elif identity and issuer:
        args = ["--certificate-identity", identity, "--certificate-oidc-issuer", issuer]
    else:
        raise ValueError("verification requires a public key or an exact certificate identity and issuer")
    if registry_ca:
        args += ["--registry-cacert", str(registry_ca)]
    if local_registry:
        if not key or not registry_ca:
            raise ValueError("local registry verification requires an explicit key and registry CA")
        # Local key trust verifies signatures directly without publishing test
        # subjects to the public transparency service. TLS remains verified.
        args += ["--insecure-ignore-tlog=true"]
    return args


def _run(command: list[str]) -> Any:
    completed = subprocess.run(command, capture_output=True, text=True, check=False, timeout=180)
    if completed.returncode:
        raise RuntimeError(f"{command[1]} failed for {command[-1]}")
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError:
        try:
            return [json.loads(line) for line in completed.stdout.splitlines() if line.strip()]
        except json.JSONDecodeError as error:
            raise RuntimeError(f"{command[1]} did not return verification evidence") from error


def verify_signatures(
    receipts: dict[str, Any],
    *,
    key: str | None = None,
    identity: str | None = None,
    issuer: str | None = None,
    registry_ca: Path | None = None,
    local_registry: bool = False,
) -> list[dict[str, str]]:
    if shutil.which("cosign") is None:
        raise RuntimeError("cosign is required for release verification")
    images = receipts.get("images")
    if not isinstance(images, list) or len(images) != REQUIRED_IMAGE_COUNT:
        raise ValueError(f"release verification requires exactly {REQUIRED_IMAGE_COUNT} image receipts")
    references = [digest_reference(item) for item in images]
    if len(set(references)) != REQUIRED_IMAGE_COUNT:
        raise ValueError("release image receipts must be distinct")
    policy = verification_args(
        key=key, identity=identity, issuer=issuer, registry_ca=registry_ca, local_registry=local_registry
    )
    verified = []
    for reference in references:
        digest = reference.rsplit("@", 1)[1]
        signatures = _run(["cosign", "verify", *policy, reference])
        if not isinstance(signatures, list) or not any(
            item.get("critical", {}).get("image", {}).get("docker-manifest-digest") == digest for item in signatures
        ):
            raise ValueError(f"signature subject does not match {reference}")
        attestations = _run(["cosign", "verify-attestation", *policy, "--type", "spdxjson", reference])
        if isinstance(attestations, dict):
            attestations = [attestations]
        valid = False
        sbom_hash = None
        for envelope in attestations if isinstance(attestations, list) else []:
            try:
                statement = json.loads(base64.b64decode(envelope["payload"], validate=True))
            except (KeyError, ValueError, TypeError, json.JSONDecodeError):
                continue
            subject_matches = any(
                subject.get("digest", {}).get("sha256") == digest.removeprefix("sha256:")
                for subject in statement.get("subject", [])
            )
            predicate = statement.get("predicate", {})
            spdx = predicate.get("Data", predicate)
            if (
                subject_matches
                and statement.get("predicateType") == "https://spdx.dev/Document"
                and isinstance(spdx, dict)
                and str(spdx.get("spdxVersion", "")).startswith("SPDX-2.")
            ):
                valid = True
                sbom_hash = hashlib.sha256(json.dumps(spdx, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
                break
        if not valid:
            raise ValueError(f"verified SPDX attestation subject does not match {reference}")
        verified.append({"reference": reference, "digest": digest, "sbomCanonicalSha256": sbom_hash})
    return verified
