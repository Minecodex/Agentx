"""Real design Host -> immutable page publication -> actual browser business execution."""

import json
import os
import shutil
import time
from pathlib import Path

import pytest

from tests.e2e.product.next_batch_support import get
from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.studio_builder_support import post
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.support import run_playwright

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.product,
    pytest.mark.live_model,
    pytest.mark.usefixtures("plan9_business_protocol"),
]


def assemble_page_results(path: Path):
    """Workers own separate result files; only the Python orchestrator assembles the report."""
    directory = path.with_suffix("")
    results = [json.loads(item.read_text()) for item in sorted(directory.glob("*.json"))]
    path.write_text(json.dumps(results, ensure_ascii=False, indent=2))
    return results


def test_real_generated_application_matrix(plan9_suite):
    suite = plan9_suite
    prepare_round_applications(suite)
    arguments = ("tests/plan9-real-pages.spec.ts", "--workers=2")
    if grep := os.environ.get("AGENTX_E2E_PLAYWRIGHT_GREP"):
        arguments = (*arguments, "--grep", grep)
    try:
        run_playwright(Path(suite["installed"]["root"]), "plan9-real-pages", arguments, browser_environment(suite))
    finally:
        assemble_page_results(suite["directory"] / "page-results.json")
        report = (
            Path(suite["installed"]["root"])
            / ".local/artifacts/playwright/helm-agentxctl"
            / suite["runId"]
            / "plan9-real-pages"
        )
        if report.exists():
            shutil.copytree(report, suite["directory"] / "browser", dirs_exist_ok=True)
    preparation = json.loads((suite["directory"] / "scenario-preparation.json").read_text())
    assert all(row["prepared"] for row in preparation), [row for row in preparation if not row["prepared"]]


def prepare_round_applications(suite):
    """Independent samples use separate applications; the product has one continuous editor per app."""
    control = suite["control"]
    rounds = int(os.environ.get("AGENTX_E2E_PLAN9_ROUNDS", "3"))
    for case in suite["cases"]:
        if case.get("creationChain") or rounds <= 1:
            continue
        original = get(control, f"/applications/{case['applicationId']}/deployments")
        source = next(row for row in original if row["id"] == case["deploymentId"])
        mapping = get(
            control, f"/applications/{case['applicationId']}/deployments/{case['deploymentId']}/playground-config"
        )
        case["roundApplications"] = [{key: case[key] for key in ("applicationId", "deploymentId", "slug")}]
        for number in range(2, rounds + 1):
            application = post(
                control,
                "/applications",
                {
                    "workflowId": case["workflowId"],
                    "name": f"{case['scene']} independent page {number} {suite['runId']}",
                    "slug": f"{case['slug']}-sample-{number}",
                    "visibility": "company",
                },
            )
            deployment = _publish_application_deployment(
                control, dict(control.headers), application["id"], case["versionId"], source["environmentId"]
            )
            if mapping.get("mapping"):
                path = f"/api/v1/applications/{application['id']}/deployments/{deployment['id']}/playground-config"
                current = control.get(path).json()
                result = control.put(path, json={"expectedVersion": current["version"], "mapping": mapping["mapping"]})
                assert result.status_code == 202, result.text
                deadline = time.monotonic() + 120
                while control.get(path).json()["publishStatus"] != "active":
                    assert time.monotonic() < deadline, "Conversation mapping publication did not settle"
                    time.sleep(0.5)
            case["roundApplications"].append(
                {"applicationId": application["id"], "deploymentId": deployment["id"], "slug": application["slug"]}
            )
    suite["path"].write_text(json.dumps(suite["cases"], ensure_ascii=False, indent=2))
