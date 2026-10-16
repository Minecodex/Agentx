"""Canvas AI system acceptance through the repository's pytest/Playwright lifecycle."""

import json
import os
from pathlib import Path

import pytest

from tests.e2e.product import studio_builder_security as security
from tests.e2e.product.studio_builder_faults import studio_faults as studio_faults
from tests.e2e.runtime.test_agent_attachments import _control_mysql, _runtime_mysql
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


@pytest.mark.live_model
def test_plan8_node_inspectors(installed_agentx, service_urls, studio_live_models):
    studio_models = studio_live_models
    selected = ("tests/studio-builder-inspectors.spec.ts",)
    grep = os.environ.get("AGENTX_E2E_PLAYWRIGHT_GREP")
    if grep:
        selected = (*selected, "--grep", grep)
    try:
        run_playwright(
            Path(installed_agentx["root"]),
            "studio-builder-inspectors",
            selected,
            {
                **os.environ,
                "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
                "AGENTX_E2E_STAGE": "helm-agentxctl",
                "AGENTX_E2E_BASE_URL": service_urls["web"],
                "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
                "AGENTX_E2E_STUDIO_FIXTURE_ALIAS": studio_models["fixture"]["alias"],
                "AGENTX_E2E_STUDIO_LIVE_ALIAS": studio_models["live"]["alias"],
            },
        )
    finally:
        capture_builder_evidence(installed_agentx)


@pytest.mark.parametrize("studio_faults", [None], indirect=True)
def test_plan8_layout_protocol(installed_agentx, service_urls, studio_models, studio_faults):
    try:
        run_playwright(
            Path(installed_agentx["root"]),
            "studio-builder-layout-protocol",
            (
                "tests/studio-builder.spec.ts",
                "tests/studio-builder-protocol.spec.ts",
                "tests/studio-builder-versioning.spec.ts",
                "tests/studio-builder-recovery.spec.ts",
                "tests/studio-builder-layout.spec.ts",
                "--grep",
                os.environ.get("AGENTX_E2E_PLAYWRIGHT_GREP") or "Fixture:|P8-E2E-(09|11|12|20)",
            ),
            {
                **os.environ,
                "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
                "AGENTX_E2E_STAGE": "helm-agentxctl",
                "AGENTX_E2E_BASE_URL": service_urls["web"],
                "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
                "AGENTX_E2E_STUDIO_FIXTURE_ALIAS": studio_models["fixture"]["alias"],
            },
        )
    finally:
        capture_builder_evidence(installed_agentx)


@pytest.mark.live_model
def test_plan8_studio_builder_desktop(installed_agentx, service_urls, studio_live_models):
    studio_models = studio_live_models
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        "AGENTX_E2E_STUDIO_FIXTURE_ALIAS": studio_models["fixture"]["alias"],
        "AGENTX_E2E_STUDIO_LIVE_ALIAS": studio_models["live"]["alias"],
    }
    try:
        run_playwright(Path(installed_agentx["root"]), "studio-builder", ("tests/studio-builder.spec.ts",), environment)
    finally:
        capture_builder_evidence(installed_agentx)


def test_plan8_studio_builder_matrix(installed_agentx, service_urls, studio_models, studio_users, studio_faults):
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": service_urls["web"],
        "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
        "AGENTX_E2E_STUDIO_FIXTURE_ALIAS": studio_models["fixture"]["alias"],
        "AGENTX_E2E_STUDIO_FIXTURE_ID": studio_models["fixture"]["id"],
        "AGENTX_E2E_STUDIO_ALTERNATE_ALIAS": studio_models["alternate"]["alias"],
        "AGENTX_E2E_STUDIO_ALTERNATE_ID": studio_models["alternate"]["id"],
    }
    for kind, user in studio_users.items():
        environment[f"AGENTX_E2E_STUDIO_{kind.upper()}_USER"] = user["username"]
        environment[f"AGENTX_E2E_STUDIO_{kind.upper()}_PASSWORD"] = user["password"].value
        environment[f"AGENTX_E2E_STUDIO_{kind.upper()}_ID"] = user["id"]
    try:
        run_playwright(
            Path(installed_agentx["root"]),
            "studio-builder-matrix",
            (
                "tests/studio-builder-versioning.spec.ts",
                "tests/studio-builder-history.spec.ts",
                "tests/studio-builder-capacity.spec.ts",
                "tests/studio-builder-protocol.spec.ts",
                "tests/studio-builder-setting.spec.ts",
                "tests/studio-builder-recovery.spec.ts",
            ),
            environment,
        )
    finally:
        capture_builder_evidence(installed_agentx)


def capture_builder_evidence(context):
    # Include frozen resource identities and design outputs, never Vault values or provider headers.
    queries = {
        "runs": (
            _control_mysql,
            "SELECT JSON_OBJECT('id',LOWER(HEX(r.id)),'status',r.status,'modelAliasId',LOWER(HEX(r.model_alias_id)),'settingVersion',r.setting_version,'draftId',LOWER(HEX(s.draft_id))) FROM studio_builder_runs r JOIN studio_builder_sessions s ON s.id=r.session_id",
        ),
        "effects": (
            _control_mysql,
            "SELECT JSON_OBJECT('id',LOWER(HEX(id)),'runId',LOWER(HEX(run_id)),'tool',tool_name,'status',status,'arguments',input_json,'result',result_json) FROM studio_builder_effects",
        ),
        "model-ledger": (
            _runtime_mysql,
            "SELECT JSON_OBJECT('id',LOWER(HEX(id)),'source',source_kind,'target',studio_target_json,'status',status,'kind',call_kind,'error',error_code,'input',input_tokens,'output',output_tokens,'costMicros',cost_micros,'currency',cost_currency,'executionId',execution_id,'attemptId',attempt_id) FROM runtime_calls WHERE source_kind='studio_builder'",
        ),
    }
    directory = Path(context["artifact_dir"]) / "studio-builder"
    directory.mkdir(exist_ok=True)
    for name, (query, sql) in queries.items():
        selected, source = sql.split(" FROM ", 1)
        encoded = "SELECT HEX(CAST(" + selected.removeprefix("SELECT ") + " AS BINARY)) FROM " + source  # noqa: S608 -- constant evidence projections, no user input.
        rows = [json.loads(bytes.fromhex(line)) for line in query(context, encoded).splitlines() if line]
        (directory / f"{name}.json").write_text(json.dumps(rows, ensure_ascii=False, indent=2), encoding="utf-8")


def test_plan8_studio_builder_resolve_only(installed_agentx, service_urls, studio_models):
    security.resolver_counterexamples(installed_agentx, service_urls)


def test_plan8_studio_builder_ledger_retention(installed_agentx, service_urls, studio_models):
    security.ledger_and_retention(installed_agentx, service_urls)


def test_plan8_studio_builder_control_offline(installed_agentx, service_urls, studio_models):
    security.control_offline(installed_agentx, service_urls, studio_models)


def test_plan8_studio_builder_unsettled_delete_protection(installed_agentx, service_urls, studio_models):
    security.protect_unsettled_sources(installed_agentx, service_urls, studio_models)


def test_plan8_studio_builder_package_history(installed_agentx, service_urls, studio_models):
    run_playwright(
        Path(installed_agentx["root"]),
        "studio-builder-package-history",
        ("tests/studio-builder-package-history.spec.ts",),
        {
            **os.environ,
            "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
            "AGENTX_E2E_STAGE": "helm-agentxctl",
            "AGENTX_E2E_BASE_URL": service_urls["web"],
            "AGENTX_E2E_STUDIO_RESOLVER_PACKAGE": str(
                Path(installed_agentx["artifact_dir"]) / "resolve-pure.agentx-plugin"
            ),
        },
    )


def test_plan8_studio_builder_revision_delete_race(installed_agentx, service_urls, studio_models):
    security.revision_delete_race(installed_agentx, service_urls, studio_models)


def test_plan8_studio_builder_resource_requests(installed_agentx, service_urls, studio_models):
    security.resource_request_isolation(installed_agentx, service_urls, studio_models)


def test_plan8_studio_builder_tenant_boundaries(installed_agentx, service_urls):
    security.tenant_boundary_isolation(installed_agentx, service_urls)
