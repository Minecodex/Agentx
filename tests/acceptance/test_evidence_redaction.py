import zipfile
from types import SimpleNamespace

import pytest

from tests.e2e.conftest import pytest_runtest_makereport
from tests.e2e.support import redact, redact_browser_artifacts


def test_evidence_redacts_vault_and_provider_credentials() -> None:
    for text, value in (
        ("Unseal Key: fixture-unseal", "fixture-unseal"),
        ("Root Token: fixture-root", "fixture-root"),
        ("AGENTX_OPENSANDBOX_API_KEY=fixture-api", "fixture-api"),
        ('{"apiKey":"fixture-camel"}', "fixture-camel"),
        ("Authorization: fixture-auth", "fixture-auth"),
        ("Authorization: Bearer fixture-bearer", "fixture-bearer"),
        ('{"authorization":"Basic fixture-basic"}', "fixture-basic"),
        ("fixture = 'eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmaXh0dXJlIn0.fixture-signature'", "eyJhbGciOiJIUzI1NiJ9"),
        ("fixture = 'axk_" + "a" * 43 + "'", "axk_" + "a" * 43),
    ):
        sanitized = redact(text)
        assert value not in sanitized
        assert "<redacted>" in sanitized


def test_failed_junit_and_diagnostics_are_redacted_without_hiding_failure(tmp_path) -> None:
    credential = "axk_" + "a" * 43
    report = SimpleNamespace(
        failed=True, longrepr=f"assert {credential}", sections=[("stdout", credential)], when="call"
    )
    item = SimpleNamespace(
        nodeid="tests/e2e/product/example.py::case", funcargs={"installed_agentx": {"artifact_dir": str(tmp_path)}}
    )
    hook = pytest_runtest_makereport(item, None)
    next(hook)
    with pytest.raises(StopIteration) as completed:
        hook.send(report)
    assert completed.value.value is report and report.failed
    assert credential not in report.longrepr
    assert credential not in report.sections[0][1]
    diagnostic = next((tmp_path / "failures").glob("*.txt")).read_text()
    assert diagnostic == "assert <redacted>"


def test_browser_trace_tokens_are_redacted_and_binary_resources_are_preserved(tmp_path) -> None:
    credential = "axk_" + "a" * 43
    image = b"\x89PNG\xff\x00\x01"
    path = tmp_path / "trace.zip"
    with zipfile.ZipFile(path, "w") as trace:
        trace.writestr("0-trace.network", '{"headers":[{"name":"Authorization","value":"Bearer ' + credential + '"}]}')
        trace.writestr("resources/image.png", image)
    (tmp_path / "junit.xml").write_text(credential)
    redact_browser_artifacts(tmp_path)
    with zipfile.ZipFile(path) as trace:
        assert credential.encode() not in trace.read("0-trace.network")
        assert trace.read("resources/image.png") == image
    assert (tmp_path / "junit.xml").read_text() == "<redacted>"
