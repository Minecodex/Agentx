"""Authoring browser tools through real Control, Runtime and OpenSandbox services."""

import base64
import hashlib
import json
import os
from pathlib import Path

import httpx
import pytest
import yaml

from tests.e2e.product.plan9.metered_provider import plan9_metered_provider as plan9_metered_provider
from tests.e2e.product.plan9.public_origins import plan9_public_origins as plan9_public_origins
from tests.e2e.product.plan9.scenarios import reference
from tests.e2e.product.plan9.workflow_builders import object_schema, publish
from tests.e2e.product.studio_builder_support import _model, _wait_admin_admission
from tests.e2e.product.test_studio_builder import capture_builder_evidence
from tests.e2e.runtime.test_agent_attachments import _access_token, _runtime_mysql
from tests.e2e.support import ROOT, run, run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


@pytest.fixture(scope="session")
def design_browser_state(installed_agentx, service_urls, run_id, studio_provider):
    with httpx.Client(base_url=service_urls["web"], timeout=120, trust_env=False) as client:
        token, me = _access_token(client)
        client.headers["Authorization"] = f"Bearer {token}"
        _wait_admin_admission(installed_agentx, client, me)
        price = {"currency": "USD", "inputPerMillion": "1", "outputPerMillion": "2"}
        models = {
            kind: _model(
                client,
                me["departmentId"],
                run_id,
                f"browser-{kind}",
                "m5-model-secret",
                studio_provider,
                "echo-studio-builder",
                price,
                {"capabilities": ["vision"] if kind == "vision" else []},
            )
            for kind in ("vision", "text")
        }
        schema = object_schema({"customer": {"type": "string", "minLength": 1}})
        application = publish(
            client,
            run_id,
            "VIS",
            "Browser acceptance",
            schema,
            {"customer": {"type": "string"}},
            [],
            {"customer": reference("inputs", ["customer"])},
            [{"customer": "Fresh customer"}],
            [{"customer": "Fresh customer"}],
        )
        state = {"models": models, "application": application}
        path = Path(installed_agentx["artifact_dir"]) / "design-browser-state.json"
        path.write_text(json.dumps(state, indent=2))
        return {**state, "path": path, "department": me["departmentId"]}


def browser_environment(installed, urls, state):
    return {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed["run_id"],
        "AGENTX_E2E_STAGE": "design-browser",
        "AGENTX_E2E_BASE_URL": urls["web"],
        "AGENTX_E2E_RUNTIME_URL": urls["runtime"],
        "AGENTX_E2E_DESIGN_BROWSER_STATE": str(state["path"]),
        "AGENTX_E2E_STUDIO_FIXTURE_ALIAS": state["models"]["vision"]["alias"],
    }


def capture_browser_evidence(installed):
    capture_builder_evidence(installed)
    queries = {
        "leases": "SELECT JSON_OBJECT('operationId',BIN_TO_UUID(operation_id),'status',status,'sandboxId',sandbox_id) FROM sandbox_design_leases",
        "images": "SELECT JSON_OBJECT('artifactId',BIN_TO_UUID(a.id),'sha256',a.sha256,'storageKey',a.storage_key,'contentType',a.content_type,'sizeBytes',a.size_bytes,'ownerType',r.owner_type,'ownerId',r.owner_id) FROM artifacts a JOIN artifact_references r ON r.tenant_id=a.tenant_id AND r.artifact_id=a.id WHERE r.owner_type='design_preview'",
        "modelCalls": "SELECT JSON_OBJECT('id',BIN_TO_UUID(id),'source',source_kind,'target',design_target_json,'status',status,'error',error_code,'inputTokens',input_tokens,'outputTokens',output_tokens) FROM runtime_calls WHERE source_kind IN ('studio_builder','application_ui_builder')",
    }
    report = {
        name: [json.loads(line) for line in _runtime_mysql(installed, query).splitlines() if line]
        for name, query in queries.items()
    }
    directory = Path(installed["artifact_dir"]) / "design-browser-images"
    directory.mkdir(exist_ok=True)
    values = yaml.safe_load(Path(installed["values"]).read_text())
    bucket = values["global"]["components"]["objectStorage"]["domains"]["runtime"]["bucket"]
    for image in report["images"]:
        assert image["storageKey"].startswith("design-preview/")
        destination = directory / f"{image['sha256']}.webp"
        if not destination.exists():
            copied = run(
                (
                    "kubectl",
                    "-n",
                    installed["dependencies_namespace"],
                    "exec",
                    "statefulset/object-storage",
                    "--",
                    "sh",
                    "-ec",
                    'export MC_HOST_evidence="https://${MINIO_ROOT_USER}:${MINIO_ROOT_PASSWORD}@object-storage.$2.svc:9000"; '
                    "export SSL_CERT_FILE=/tls/CAs/ca.crt; "
                    'mc --config-dir /tmp/agentx-browser-evidence cat "$1" 2>/dev/null | base64',
                    "browser-evidence",
                    f"evidence/{bucket}/{image['storageKey']}",
                    installed["dependencies_namespace"],
                ),
                timeout=30,
            )
            contents = base64.b64decode(copied.stdout)
            assert len(contents) == image["sizeBytes"]
            assert hashlib.sha256(contents).hexdigest() == image["sha256"]
            destination.write_bytes(contents)
        image["localFile"] = str(destination)
    path = Path(installed["artifact_dir"]) / "design-browser-runtime.json"
    path.write_text(json.dumps(report, indent=2))


def test_browser_effects_and_image_capabilities(installed_agentx, service_urls, design_browser_state):
    try:
        run_playwright(
            ROOT,
            "design-browser-protocol",
            ("tests/design-browser-agent.spec.ts", "--grep", "Fixture:"),
            browser_environment(installed_agentx, service_urls, design_browser_state),
        )
        pending = _runtime_mysql(
            installed_agentx,
            "SELECT COUNT(*) FROM sandbox_design_leases WHERE status IN ('creating','executing','cleanup_pending')",
        )
        assert pending == "0", "Browser observations left an active isolated environment"
        corrected = _runtime_mysql(
            installed_agentx,
            "SELECT COUNT(*) FROM runtime_calls WHERE error_code='DESIGN_MODEL_OUTPUT_LIMIT' AND status='failed' AND input_tokens=32 AND output_tokens=48",
        )
        assert corrected == "1", "Truncated model output must settle usage before the same task corrects it"
    finally:
        capture_browser_evidence(installed_agentx)


@pytest.mark.live_model
def test_real_autonomous_creation_and_editing(
    installed_agentx, service_urls, design_browser_state, plan9_metered_provider, existing_model_secret, run_id
):
    configured = plan9_metered_provider
    with httpx.Client(base_url=service_urls["web"], timeout=120, trust_env=False) as client:
        token, _ = _access_token(client)
        client.headers["Authorization"] = f"Bearer {token}"
        live = _model(
            client,
            design_browser_state["department"],
            run_id,
            "browser-live",
            existing_model_secret.value,
            configured["endpoint"],
            configured["modelName"],
            {
                "currency": "USD",
                "inputPerMillion": configured["inputPrice"],
                "outputPerMillion": configured["outputPrice"],
            },
            {
                **{key: configured[key] for key in ("defaultParameters", "maxInputTokens", "maxOutputTokens")},
                "capabilities": ["vision"],
            },
        )
    environment = {
        **browser_environment(installed_agentx, service_urls, design_browser_state),
        "AGENTX_E2E_STUDIO_LIVE_ALIAS": live["alias"],
    }
    try:
        run_playwright(
            ROOT, "design-browser-live", ("tests/design-browser-agent.spec.ts", "--grep", "Real:"), environment
        )
    finally:
        capture_browser_evidence(installed_agentx)
