"""Local TLS registry: all 11 actual images are signed, attested and verified."""

from __future__ import annotations

import copy
import hashlib
import ipaddress
import json
import re
import socket
import time
from datetime import UTC, datetime, timedelta

import httpx
import pytest
import yaml
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID

from tests.e2e.support import ROOT, agentxctl, run
from tools.scripts.release.evidence import image_manifest_sha256
from tools.scripts.release.release_summary import validate_domains
from tools.scripts.release.signature_verification import verify_signatures

pytestmark = [pytest.mark.cluster, pytest.mark.security]


def certificates(directory):
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Agentx isolated registry")])
    now = datetime.now(UTC)
    cert = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(name)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - timedelta(minutes=1))
        .not_valid_after(now + timedelta(days=1))
        .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
        .add_extension(
            x509.SubjectAlternativeName([x509.DNSName("localhost"), x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]),
            critical=False,
        )
        .sign(key, hashes.SHA256())
    )
    ca = directory / "registry-ca.crt"
    private = directory / "registry.key"
    ca.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    private.write_bytes(
        key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption())
    )
    private.chmod(0o600)
    return ca, private


def test_tls_registry_signature_attestation_and_negative_trust(deployment_values, run_id):
    artifact = ROOT / ".local/artifacts/e2e" / run_id
    artifact.mkdir(parents=True, exist_ok=True)
    directory = ROOT / ".local/dist" / f"local-supply-chain-{run_id}"
    directory.mkdir(parents=True, exist_ok=True)
    ca, _private = certificates(artifact)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    registry = f"localhost:{port}"
    container = f"agentx-signing-{run_id}"
    run(
        (
            "docker",
            "run",
            "-d",
            "--name",
            container,
            "-p",
            f"127.0.0.1:{port}:5000",
            "-v",
            f"{artifact.resolve()}:/certs:ro",
            "-e",
            "REGISTRY_HTTP_TLS_CERTIFICATE=/certs/registry-ca.crt",
            "-e",
            "REGISTRY_HTTP_TLS_KEY=/certs/registry.key",
            "registry:3.0.0",
        ),
        timeout=120,
    )
    try:
        import ssl

        tls = ssl.create_default_context(cafile=str(ca))
        with httpx.Client(verify=tls, timeout=5) as client:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                try:
                    assert client.get(f"https://{registry}/v2/").status_code == 200
                    break
                except httpx.HTTPError:
                    time.sleep(0.25)
            else:
                pytest.fail("local TLS registry did not start")
        values = yaml.safe_load(deployment_values.read_text())
        services = values["global"]["images"]["services"]
        rendered = run((agentxctl(), "render", "--values", deployment_values, "--target", "all"), timeout=120).stdout
        sources = sorted(
            {
                image
                for image in re.findall(r'^\s+image:\s*["\']?([^\s"\']+)', rendered, re.MULTILINE)
                if image.rsplit("/", 1)[-1].split(":", 1)[0].removeprefix("agentx-") in services
                or image.rsplit("/", 1)[-1].split(":", 1)[0] in services
            }
        )
        assert len(sources) == 11, sources
        images = []
        build_identities = set()
        for source in sources:
            metadata = run(("docker", "image", "inspect", source), timeout=30).json()[0]
            labels = metadata["Config"]["Labels"]
            build_identities.add((labels["org.opencontainers.image.revision"], labels["io.agentx.source-tree-sha256"]))
            name = source.rsplit("/", 1)[-1].split(":", 1)[0]
            target = f"{registry}/agentx/{name}:candidate"
            run(("docker", "tag", source, target), timeout=30)
            run(("docker", "push", target), timeout=300)
            inspected = run(("docker", "image", "inspect", target), timeout=30).json()[0]
            digest = next(
                item.rsplit("@", 1)[1]
                for item in inspected["RepoDigests"]
                if item.startswith(f"{registry}/agentx/{name}@")
            )
            images.append(
                {"image": target, "digest": digest, "platform": f"{inspected['Os']}/{inspected['Architecture']}"}
            )
        assert len(build_identities) == 1, "all release images must come from the same source tree"
        commit, tree = next(iter(build_identities))
        references = [f"{item['image'].split(':candidate')[0]}@{item['digest']}" for item in images]
        identity = {
            "runId": run_id,
            "sourceCommit": commit,
            "sourceTreeSha256": tree,
            "imageManifestSha256": image_manifest_sha256(references),
        }
        receipt = directory / "release-images-inspected.json"
        receipt.write_text(
            json.dumps({"version": "local-test", "sourceCommit": commit, "identity": identity, "images": images}) + "\n"
        )
        key_prefix = artifact / "cosign"
        environment = {"COSIGN_PASSWORD": ""}
        run(("cosign", "generate-key-pair", "--output-key-prefix", key_prefix), env=environment, timeout=60)
        key_prefix.with_suffix(".key").chmod(0o600)
        evidence = directory / "supply-chain-evidence.json"
        run(
            (
                "uv",
                "run",
                "--frozen",
                "--group",
                "test",
                "python",
                "-m",
                "tools.scripts.release.supply_chain",
                "--receipts",
                receipt,
                "--output",
                evidence,
                "--key-ref",
                key_prefix.with_suffix(".key"),
                "--registry-ca",
                ca,
                "--local-registry",
            ),
            env=environment,
            timeout=3600,
        )
        proof = json.loads(evidence.read_text())
        assert proof["status"] == "passed" and proof["verifiedImageCount"] == 11
        verified = verify_signatures(
            json.loads(receipt.read_text()),
            key=str(key_prefix.with_suffix(".pub")),
            registry_ca=ca,
            local_registry=True,
        )
        assert len(verified) == 11
        wrong_prefix = artifact / "wrong-cosign"
        run(("cosign", "generate-key-pair", "--output-key-prefix", wrong_prefix), env=environment, timeout=60)
        wrong_prefix.with_suffix(".key").chmod(0o600)
        with pytest.raises(RuntimeError):
            verify_signatures(
                json.loads(receipt.read_text()),
                key=str(wrong_prefix.with_suffix(".pub")),
                registry_ca=ca,
                local_registry=True,
            )
        changed = copy.deepcopy(json.loads(receipt.read_text()))
        changed["images"][0]["digest"] = "sha256:" + "0" * 64
        with pytest.raises(RuntimeError):
            verify_signatures(changed, key=str(key_prefix.with_suffix(".pub")), registry_ca=ca, local_registry=True)
        domains = validate_domains(artifact, directory, {}, identity)
        assert domains["supply-chain"]["status"] == "passed", domains["supply-chain"]
        sbom = directory / proof["images"][0]["sbom"]
        original = sbom.read_bytes()
        try:
            altered = json.loads(original)
            altered["name"] = "tampered"
            sbom.write_text(json.dumps(altered))
            domains = validate_domains(artifact, directory, {}, identity)
            assert domains["supply-chain"]["status"] == "failed"
        finally:
            sbom.write_bytes(original)
        (artifact / "supply-chain-negative-trust.json").write_text(
            json.dumps(
                {
                    "identity": identity,
                    "status": "passed",
                    "wrongKeyRejected": True,
                    "digestSubstitutionRejected": True,
                    "sbomTamperingRejected": True,
                    "evidenceSha256": hashlib.sha256(evidence.read_bytes()).hexdigest(),
                }
            )
            + "\n"
        )
    finally:
        run(("docker", "rm", "-f", container), check=False, timeout=60)
        for name in ("registry.key", "cosign.key", "wrong-cosign.key"):
            (artifact / name).unlink(missing_ok=True)
