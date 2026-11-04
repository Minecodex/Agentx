"""Reuse the existing deployment, model meter, publisher and workflow fixtures."""

from __future__ import annotations

import json
import time
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.live_text_support import Secret
from tests.e2e.product.next_batch_support import create_user, get
from tests.e2e.product.plan9.file_scenarios import single_file
from tests.e2e.product.plan9.matrix import reviewers
from tests.e2e.product.plan9.metered_provider import plan9_metered_provider as plan9_metered_provider
from tests.e2e.product.plan9.public_origins import plan9_public_origins as plan9_public_origins
from tests.e2e.product.plan9.public_origins import wait_public_entrypoints
from tests.e2e.product.plan9.scenarios import quotation, sandbox_profile
from tests.e2e.product.plan10.scenarios import expenses
from tests.e2e.product.studio_builder_support import _model, post
from tests.e2e.product.user_auth import UserTokenAuth
from tests.e2e.runtime.test_agent_attachments import ADMIN_PASSWORD, ADMIN_USERNAME, _access_token
from tests.e2e.support import run


@pytest.fixture(scope="session")
def existing_model(plan9_metered_provider):
    return plan9_metered_provider


@pytest.fixture
def plan10_paid_gate(request):
    assert request.session.testsfailed == 0, (
        "Real model acceptance is not started while earlier system gates are failing; "
        "retain the failures and repair the candidate before spending more budget."
    )


@pytest.fixture(scope="session")
def plan10_suite(
    installed_agentx, service_urls, run_id, existing_model, existing_model_secret, studio_provider, pytestconfig
):
    directory = Path(installed_agentx["artifact_dir"]) / "plan10"
    directory.mkdir(exist_ok=True)
    # The metered provider adds its CA after public origins are configured. A
    # port-forward can otherwise remain attached to that draining Gateway Pod.
    namespace = installed_agentx["runtime_namespace"]
    pods = run(
        ("kubectl", "-n", namespace, "get", "pods", "-l", "app.kubernetes.io/name=runtime-gateway", "-o", "json")
    ).json()["items"]
    for pod in pods:
        if pod["metadata"].get("deletionTimestamp"):
            run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "wait",
                    "--for=delete",
                    f"pod/{pod['metadata']['name']}",
                    "--timeout=180s",
                ),
                timeout=200,
            )
    wait_public_entrypoints(service_urls["web"], service_urls["runtime"])
    with httpx.Client(base_url=service_urls["web"], timeout=120, trust_env=False) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        control.auth = UserTokenAuth(
            service_urls["web"], Secret(token), username=ADMIN_USERNAME, password=Secret(ADMIN_PASSWORD)
        )
        if pytestconfig.getoption("--reuse-kept-environment"):
            from tests.e2e.product.plan10.resume import existing_suite

            yield existing_suite(control, me, installed_agentx, service_urls, run_id)
            return
        model = _model(
            control,
            me["departmentId"],
            run_id,
            "plan10",
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
        setting = get(control, "/platform-ai/model-setting")
        selected = control.put(
            "/api/v1/platform-ai/model-setting",
            json={"expectedVersion": setting["version"], "modelAliasId": model["id"]},
        )
        assert selected.status_code == 200, selected.text
        # Workflow resource references bind the published model deployment;
        # _model returns only the creation summary used by the design setting.
        model = get(control, f"/models/aliases/{model['id']}")
        assert model["deploymentId"]
        profile = sandbox_profile(control, me["departmentId"], run_id)
        users = reviewers(control, me["departmentId"], run_id)
        namespace = installed_agentx["dependencies_namespace"]
        run(("kubectl", "-n", namespace, "set", "env", "deployment/echo-mcp", "AGENTX_FIXTURE_PLAN9=1"))
        run(("kubectl", "-n", namespace, "rollout", "status", "deployment/echo-mcp", "--timeout=180s"), timeout=200)
        quote = quotation(control, profile, run_id)
        quote["kind"] = "quote"
        report = single_file(control, profile, run_id, studio_provider.removesuffix("/v1"))
        report["kind"] = "file"
        expense = expenses(control, run_id, users["manager"]["id"])
        cases = [expense, quote, report]
        role = post(
            control,
            "/roles",
            {
                "name": f"Batch employee {run_id}",
                "code": f"batch_employee_{run_id.replace('-', '_')}",
                "dataScope": "company",
                "permissions": ["application:invoke"],
            },
        )
        employee = create_user(control, f"batch-employee-{run_id}", me["departmentId"], role["id"])
        other = create_user(control, f"batch-other-{run_id}", me["departmentId"], role["id"])
        for case in cases:
            key = post(
                control, f"/applications/{case['applicationId']}/api-keys", {"name": "Batch protocol acceptance"}
            )
            case["keyId"] = key["id"]
            case["secret"] = Secret(key["secret"])
            with httpx.Client(
                base_url=service_urls["runtime"],
                headers={"Authorization": f"Bearer {case['secret'].value}"},
                timeout=30,
                trust_env=False,
            ) as gateway:
                deadline = time.monotonic() + 90
                while True:
                    ready = gateway.get(f"/gateway/v1/applications/{case['slug']}/batches", params={"limit": 1})
                    if ready.status_code == 200:
                        break
                    assert ready.status_code in (401, 503) and time.monotonic() < deadline, ready.text
                    time.sleep(0.3)
        public = [{key: value for key, value in case.items() if key != "secret"} for case in cases]
        (directory / "scenarios.json").write_text(json.dumps(public, ensure_ascii=False, indent=2))
        suite = {
            "control": control,
            "directory": directory,
            "cases": cases,
            "employee": employee,
            "other": other,
            "users": users,
            "model": model,
            "profile": profile,
            "me": me,
            "installed": installed_agentx,
            "urls": service_urls,
            "runId": run_id,
        }
        yield suite
