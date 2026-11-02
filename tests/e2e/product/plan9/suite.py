"""One temporary set of real published applications shared by Web, MCP and external AI acceptance."""

from __future__ import annotations

import json
import os
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.live_text_support import Secret
from tests.e2e.product.next_batch_support import create_user
from tests.e2e.product.plan9.matrix import SCENES, reviewers, scenarios
from tests.e2e.product.plan9.scenarios import sandbox_profile
from tests.e2e.product.studio_builder_support import _model, post
from tests.e2e.product.user_auth import UserTokenAuth
from tests.e2e.runtime.test_agent_attachments import ADMIN_PASSWORD, ADMIN_USERNAME, _access_token


@pytest.fixture(scope="session")
def plan9_suite(installed_agentx, service_urls, run_id, existing_model, existing_model_secret, request):
    directory = Path(installed_agentx["artifact_dir"]) / "plan9"
    directory.mkdir(exist_ok=True)
    with httpx.Client(base_url=service_urls["web"], timeout=120, trust_env=False) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        control.auth = UserTokenAuth(
            service_urls["web"], Secret(token), username=ADMIN_USERNAME, password=Secret(ADMIN_PASSWORD)
        )
        model = _model(
            control,
            me["departmentId"],
            run_id,
            "plan9",
            existing_model_secret.value,
            existing_model["endpoint"],
            existing_model["modelName"],
            {
                "currency": "USD",
                "inputPerMillion": existing_model["inputPrice"],
                "outputPerMillion": existing_model["outputPrice"],
            },
            {key: existing_model[key] for key in ("defaultParameters", "maxInputTokens", "maxOutputTokens")},
        )
        setting = control.get("/api/v1/platform-ai/model-setting")
        setting.raise_for_status()
        selected = control.put(
            "/api/v1/platform-ai/model-setting",
            json={"expectedVersion": setting.json()["version"], "modelAliasId": model["id"]},
        )
        assert selected.status_code == 200, selected.text
        assert selected.json()["status"] == "ready", selected.text
        profile = sandbox_profile(control, me["departmentId"], run_id)
        users = reviewers(control, me["departmentId"], run_id)
        selected = os.environ.get("AGENTX_E2E_PLAN9_SCENES", ",".join(SCENES)).split(",")
        cases = scenarios(
            control, profile, model, users, me["departmentId"], run_id, selected, request, installed_agentx
        )
        role = post(
            control,
            "/roles",
            {
                "name": f"Plan9 employee {run_id}",
                "code": f"p9_employee_{run_id.replace('-', '_')}",
                "dataScope": "company",
                "permissions": ["application:invoke"],
            },
        )
        employee = create_user(control, f"p9-employee-{run_id}", me["departmentId"], role["id"])
        path = directory / "scenarios.json"
        path.write_text(json.dumps(cases, ensure_ascii=False, indent=2))
        crm_url = request.getfixturevalue("plan9_fixture_url") if any(case.get("crm") for case in cases) else ""
        suite = {
            "control": control,
            "directory": directory,
            "path": path,
            "cases": cases,
            "model": model,
            "profile": profile,
            "users": users,
            "employee": employee,
            "crmUrl": crm_url,
            "installed": installed_agentx,
            "urls": service_urls,
            "runId": run_id,
        }
        from tests.e2e.product.live_model_config import model_configuration
        from tests.e2e.product.plan9.evidence import summarize
        from tests.e2e.product.plan9.opencode_support import budget
        from tests.e2e.support import redact

        def meter():
            return budget(
                model_configuration()["endpoint"],
                existing_model_secret.value,
                Path(os.environ["AGENTX_E2E_LLM_CA_FILE"]),
            )

        before = meter()
        try:
            yield suite
        finally:
            try:
                summarize(suite, before, meter())
            except Exception as error:
                (directory / "evidence-collection-error.json").write_text(
                    json.dumps({"error": redact(f"{type(error).__name__}: {error}")}, ensure_ascii=False, indent=2)
                )


def browser_environment(suite):
    employee = suite["employee"]
    return {
        **os.environ,
        "AGENTX_E2E_RUN_ID": suite["runId"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": suite["urls"]["web"],
        "AGENTX_E2E_RUNTIME_URL": suite["urls"]["runtime"],
        "AGENTX_E2E_PLAN9_SCENARIOS": str(suite["path"]),
        "AGENTX_E2E_PLAN9_REPORT": str(suite["directory"] / "page-results.json"),
        "AGENTX_E2E_PLAN9_ROUNDS": os.environ.get("AGENTX_E2E_PLAN9_ROUNDS", "3"),
        "AGENTX_E2E_PLAN9_EMPLOYEE": employee["username"],
        "AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD": employee["password"].value,
        "AGENTX_E2E_PLAN9_CRM_URL": suite["crmUrl"],
        "AGENTX_E2E_PLAN9_REVIEWERS": json.dumps(
            {
                key: {"username": value["username"], "password": value["password"].value}
                for key, value in suite["users"].items()
            }
        ),
    }
