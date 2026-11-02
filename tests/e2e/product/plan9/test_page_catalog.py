"""Directory links stay on the active AI page through pending/rejected publication."""

import json
import re
import time
import uuid
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.next_batch_support import create_user, get
from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.plan9.test_entrance_independence import web_state
from tests.e2e.product.plan9.test_page_boundaries import active
from tests.e2e.product.studio_builder_support import post
from tests.e2e.runtime.test_agent_attachments import _control_mysql
from tests.e2e.support import run, run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_catalog_advertises_only_the_activated_ai_page(plan9_suite):
    suite, report = plan9_suite, {"passed": False, "phases": []}
    case = next(row for row in suite["cases"] if row["scene"] == "S01")
    application = case["applicationId"]
    listing, original = active(suite, application)
    prior = next(
        row
        for row in listing["publications"]
        if row["status"] == "superseded" and row["uiVersionId"] != original["uiVersionId"]
    )
    details = get(suite["control"], f"/applications/{application}")
    department = get(suite["control"], "/auth/me")["departmentId"]
    role = post(
        suite["control"],
        "/roles",
        {
            "name": f"Catalog reader {suite['runId']}",
            "code": "catalog_" + suite["runId"].replace("-", "_"),
            "dataScope": "company",
            "permissions": ["application:view", "application:invoke"],
        },
    )
    viewer = create_user(suite["control"], f"catalog-{suite['runId']}", department, role["id"])
    runtime = suite["installed"]["runtime_namespace"]
    assert runtime == f"agentx-e2e-runtime-{suite['runId']}"
    replicas = {
        name: int(
            run(("kubectl", "-n", runtime, "get", f"deployment/{name}", "-o", "jsonpath={.spec.replicas}")).stdout
        )
        for name in ("runtime-gateway", "workflow-runtime")
    }
    stopped = False
    identifier = uuid.UUID(prior["uiVersionId"]).hex
    original_hash = _control_mysql(
        suite["installed"],
        f"SELECT contract_hash FROM application_ui_versions WHERE id=UNHEX('{identifier}')",  # noqa: S608 -- parsed UUID in the temporary database.
    )
    assert original_hash == prior["manifest"]["contractHash"]
    assert re.fullmatch(r"sha256:[0-9a-f]{64}", original_hash)

    def restore_runtime():
        nonlocal stopped
        if not stopped:
            return
        for name, count in replicas.items():
            run(("kubectl", "-n", runtime, "scale", f"deployment/{name}", f"--replicas={count}"))
        for name in replicas:
            run(("kubectl", "-n", runtime, "rollout", "status", f"deployment/{name}", "--timeout=180s"), timeout=200)
        stopped = False

    def check_catalog(phase, slug):
        observations = []
        for label, client in (("administrator", suite["control"]), ("reader", reader)):
            detail = client.get(f"/api/v1/applications/{application}")
            rows = client.get("/api/v1/applications", params={"pageSize": 100})
            assert detail.status_code == rows.status_code == 200
            item = next(row for row in rows.json()["items"] if row["id"] == application)
            assert detail.json()["publishedPageSlug"] == item["publishedPageSlug"] == slug
            observations.append({"actor": label, "slug": slug})
        browser_report = suite["directory"] / f"page-catalog-{phase}.json"
        run_playwright(
            Path(suite["installed"]["root"]),
            f"plan9-page-catalog-{phase}",
            ("tests/plan9-page-catalog.spec.ts",),
            {
                **browser_environment(suite),
                "AGENTX_E2E_PLAN9_CATALOG": json.dumps(
                    {"name": details["name"], "suffix": slug, "phase": phase, "report": str(browser_report)}
                ),
                "AGENTX_E2E_PLAN9_CATALOG_USER": viewer["username"],
                "AGENTX_E2E_PLAN9_CATALOG_PASSWORD": viewer["password"].value,
            },
        )
        report["phases"].append(
            {"phase": phase, "api": observations, "browser": json.loads(browser_report.read_text())}
        )

    with httpx.Client(base_url=suite["urls"]["web"], timeout=60, trust_env=False) as reader:
        login = reader.post(
            "/api/v1/auth/login", json={"username": viewer["username"], "password": viewer["password"].value}
        )
        assert login.status_code == 200
        reader.headers["Authorization"] = "Bearer " + login.json()["accessToken"]
        try:
            # Only metadata of a nonactive historical version is corrupted; the
            # genuine AI HTML, active publication and Runtime assets are untouched.
            _control_mysql(
                suite["installed"],
                f"UPDATE application_ui_versions SET contract_hash=CONCAT('sha256:',REPEAT('0',64)) WHERE id=UNHEX('{identifier}')",  # noqa: S608 -- parsed UUID, deliberate isolated integrity fault.
            )
            stopped = True
            for name in replicas:
                run(("kubectl", "-n", runtime, "scale", f"deployment/{name}", "--replicas=0"))
            for name in replicas:
                run(
                    (
                        "kubectl",
                        "-n",
                        runtime,
                        "wait",
                        "--for=delete",
                        "pod",
                        "-l",
                        f"app.kubernetes.io/name={name}",
                        "--timeout=120s",
                    ),
                    timeout=140,
                )
            pending_slug = f"p9-pending-{suite['runId']}"
            response = suite["control"].post(
                f"/api/v1/applications/{application}/ui/publications",
                json={
                    "draftId": None,
                    "uiVersionId": prior["uiVersionId"],
                    "expectedDraftRevision": None,
                    "expectedPublicationRevision": listing["revision"],
                    "slug": pending_slug,
                    "enabled": True,
                },
            )
            assert response.status_code == 200, response.text
            pending = response.json()
            assert active(suite, application)[1]["id"] == original["id"]
            check_catalog("preparing", original["slug"])
            restore_runtime()
            until = time.monotonic() + 150
            while True:
                statuses = get(suite["control"], f"/applications/{application}/ui/publications")
                rejected = next(row for row in statuses["publications"] if row["id"] == pending["id"])
                if rejected["status"] == "rejected":
                    break
                assert rejected["status"] != "active" and time.monotonic() < until, rejected
                time.sleep(0.5)
            assert active(suite, application)[1]["id"] == original["id"]
            check_catalog("rejected", original["slug"])
            _control_mysql(
                suite["installed"],
                f"UPDATE application_ui_versions SET contract_hash='{original_hash}' WHERE id=UNHEX('{identifier}')",  # noqa: S608 -- original validated content hash and parsed UUID only.
            )
            activated = web_state(suite, application, original["uiVersionId"], f"p9-activated-{suite['runId']}", True)
            check_catalog("active", activated["slug"])
            report.update(passed=True, original=original["id"], rejected=rejected, activated=activated)
        finally:
            _control_mysql(
                suite["installed"],
                f"UPDATE application_ui_versions SET contract_hash='{original_hash}' WHERE id=UNHEX('{identifier}')",  # noqa: S608 -- restore the original isolated metadata, never HTML.
            )
            restore_runtime()
            (suite["directory"] / "page-catalog.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
