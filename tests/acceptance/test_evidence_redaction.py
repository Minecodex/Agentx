import json
import zipfile
from types import SimpleNamespace
from xml.etree import ElementTree

import pytest
import yaml

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
        ("custom+scheme://user:pass:with:colons@host.invalid/path", "pass:with:colons"),
    ):
        sanitized = redact(text)
        assert value not in sanitized
        assert "<redacted>" in sanitized


@pytest.mark.timeout(2)
def test_large_scheme_like_tool_results_do_not_trigger_quadratic_redaction() -> None:
    content = "tool output: " + "alpha-beta.v1" * 30_000
    assert redact(content) == content


def test_redaction_preserves_kubernetes_secret_references_and_yaml_structure() -> None:
    document = """automountServiceAccountToken: false
volumes:
  - name: mysql-secrets
    secret:
      defaultMode: 420
      secretName: agentx-control-secrets
      items:
        - key: AGENTX_CONTROL_MYSQL_PASSWORD
          path: app-password
password: isolated-password
"""
    sanitized = redact(document)
    parsed = yaml.safe_load(sanitized)
    assert parsed["automountServiceAccountToken"] is False
    assert parsed["volumes"] == yaml.safe_load(document)["volumes"]
    assert parsed["password"] == "<redacted>"  # noqa: S105 -- verifies the redaction marker
    assert "isolated-password" not in sanitized


def test_redaction_preserves_json_with_escaped_credentials_and_object_values() -> None:
    document = {"secret": {"secretName": "mysql-secrets"}, "password": 'isolated-"password\\value}', "token": "abc"}
    sanitized = redact(json.dumps(document))
    assert json.loads(sanitized) == {"secret": document["secret"], "password": "<redacted>", "token": "<redacted>"}


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


def test_truncated_fixture_dumps_cannot_expose_credential_suffixes() -> None:
    diagnostic = (
        "streaming_application = {'apiKey': 'already-truncated',...opaque-credential-suffix'}\n"
        "callers = ['first-key',...opaque-key-suffix']\n"
        "    saved = control.put(path)\n"
        "E   httpx.ReadTimeout: timed out"
    )
    sanitized = redact(diagnostic)
    assert "opaque-credential-suffix" not in sanitized
    assert "opaque-key-suffix" not in sanitized
    assert "saved = control.put(path)" in sanitized
    assert "httpx.ReadTimeout: timed out" in sanitized


def test_browser_trace_tokens_are_redacted_and_binary_resources_are_preserved(tmp_path) -> None:
    credential = "axk_" + "a" * 43
    image = b"\x89PNG\xff\x00\x01"
    path = tmp_path / "trace.zip"
    with zipfile.ZipFile(path, "w") as trace:
        trace.writestr("0-trace.network", '{"headers":[{"name":"Authorization","value":"Bearer ' + credential + '"}]}')
        trace.writestr("resources/image.png", image)
    (tmp_path / "junit.xml").write_text(
        f'<testsuite tests="1"><testcase name="case"><failure message="{credential}">'
        f"Authorization: Bearer {credential}</failure></testcase></testsuite>"
    )
    redact_browser_artifacts(tmp_path)
    with zipfile.ZipFile(path) as trace:
        assert credential.encode() not in trace.read("0-trace.network")
        assert trace.read("resources/image.png") == image
    report = ElementTree.parse(tmp_path / "junit.xml").getroot()  # noqa: S314 -- controlled local fixture
    assert report.attrib["tests"] == "1"
    failure = report.find("testcase/failure")
    assert failure is not None and failure.attrib["message"] == "<redacted>"
    assert failure.text == "Authorization: <redacted>"
