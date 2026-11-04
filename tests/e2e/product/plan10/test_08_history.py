"""Restore genuine generated materials; never substitute the latest publication."""

import hashlib
import json
import os
import uuid
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.plan10.support import hexid, report, sql, wait
from tests.e2e.support import run_playwright

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.product,
    pytest.mark.gateway,
    pytest.mark.security,
    pytest.mark.live_model,
]


def employee_client(suite, employee):
    response = suite["control"].post(
        "/api/v1/auth/login", auth=None, json={"username": employee["username"], "password": employee["password"].value}
    )
    assert response.status_code == 200
    return httpx.Client(
        base_url=suite["urls"]["runtime"],
        headers={"Authorization": f"Bearer {response.json()['accessToken']}"},
        timeout=60,
        trust_env=False,
    )


def republish(suite, case, enabled=True):
    path = f"/api/v1/applications/{case['applicationId']}/ui/publications"
    current = suite["control"].get(path).json()
    page = next(page for page in current["publications"] if page["id"] == current["activePublicationId"])
    response = suite["control"].post(
        path,
        json={
            "uiVersionId": page["uiVersionId"],
            "expectedPublicationRevision": current["revision"],
            "slug": current["slug"],
            "enabled": enabled,
        },
    )
    assert response.status_code == 200, response.text
    accepted = response.json()
    wait(lambda: suite["control"].get(path).json(), lambda value: value["activePublicationId"] == accepted["id"])
    return accepted


def test_original_page_sdk_hashes_cross_user_and_conflicting_targets(plan10_suite, plan10_paid_gate):
    suite = plan10_suite
    observations = []
    with employee_client(suite, suite["employee"]) as owner, employee_client(suite, suite["other"]) as other:
        early = json.loads((suite["directory"] / "E10-V03-api-without-page.json").read_text())["batch"]
        expense_page = json.loads((suite["directory"] / "expense-complete.json").read_text())["publication"]
        missing = owner.get(f"/gateway/v1/pages/{expense_page['slug']}", params={"batchId": early["batchId"]})
        assert missing.status_code == 409, "A later page cannot become the original page of an older API batch"
        for case in suite["cases"]:
            completed = suite["directory"] / f"{case['kind']}-complete.json"
            assert completed.exists(), (
                f"Required generated page {case['kind']} has not completed formal business acceptance; "
                "partial history evidence does not pass this gate"
            )
            result = json.loads(completed.read_text())
            publication = result["publication"]
            batch = result["batches"][0]["batch"]
            task = result["batches"][0]["projects"][0]["task"]
            route = f"/gateway/v1/pages/{publication['slug']}"
            api_batch = None
            api_publication = None
            if case["kind"] == "quote":
                current_page = owner.get(route)
                assert current_page.status_code == 200, current_page.text
                api_publication = current_page.json()["publication"]["publicationId"]
                response = owner.post(
                    f"/gateway/v1/applications/{case['slug']}/batches",
                    headers={"Idempotency-Key": str(uuid.uuid4())},
                    json={
                        "name": "API with an existing exact page",
                        "items": [{"clientItemId": "api-row", "input": case["inputs"][0]}],
                    },
                )
                assert response.status_code == 202, response.text
                api_batch = response.json()
                assert (
                    api_batch["source"] == "api"
                    and api_batch["accessBinding"] is None
                    and api_batch["pagePublicationId"] == api_publication
                )
            first = owner.get(route, params={"batchId": batch["batchId"]})
            assert first.status_code == 200, first.text
            original = first.json()
            assert original["taskId"] is None and original["batchId"] == batch["batchId"]
            current = republish(suite, case)
            restored = owner.get(route, params={"batchId": batch["batchId"]})
            assert restored.status_code == 200, restored.text
            restored = restored.json()
            assert restored["historical"] and restored["publication"]["publicationId"] == publication["id"]
            assert current["id"] != publication["id"] and restored["publication"] == original["publication"]
            if api_batch:
                api_history = owner.get(route, params={"batchId": api_batch["batchId"]})
                assert api_history.status_code == 200, api_history.text
                assert (
                    api_history.json()["historical"]
                    and api_history.json()["publication"]["publicationId"] == api_publication
                )
            for asset, field in (("html", "html"), ("sdk", "sdkScript"), ("stylesheet", "stylesheet")):
                assert restored[field] == original[field]
                assert (
                    "sha256:" + hashlib.sha256(restored[field].encode()).hexdigest()
                    == restored["publication"][asset]["contentHash"]
                )
            conflict = owner.get(route, params={"batchId": batch["batchId"], "taskId": task["taskId"]})
            assert conflict.status_code in (400, 422)
            stale = owner.post(
                route + "/batches",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={
                    "publicationId": original["publication"]["publicationId"],
                    "revision": original["publication"]["revision"],
                    "name": "stale old page",
                    "items": [{"clientItemId": "one", "input": case["inputs"][0]}],
                },
            )
            assert stale.status_code == 409, stale.text
            for suffix in ("", "/items"):
                denied = other.get(f"/gateway/v1/batches/{batch['batchId']}{suffix}")
                assert denied.status_code in (401, 403, 404)
            assert other.get(route, params={"batchId": batch["batchId"]}).status_code in (401, 403, 404)
            export = result["batches"][0]["export"]
            assert other.get(
                f"/gateway/v1/batches/{batch['batchId']}/exports/{export['exportId']}/file"
            ).status_code in (401, 403, 404)
            sdk = original["publication"]["sdk"]
            publication_id = hexid(publication["id"])
            tenant = hexid(original["publication"]["tenantId"])
            digest = sdk["contentHash"]
            sql(
                suite,
                f"UPDATE application_page_publications SET manifest_json=JSON_SET(manifest_json,'$.sdk.contentHash',CONCAT('sha256:',REPEAT('0',64))) WHERE tenant_id=UNHEX('{tenant}') AND id=UNHEX('{publication_id}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            )
            try:
                broken = owner.get(route, params={"batchId": batch["batchId"]})
                assert broken.status_code in (400, 404, 409, 503), broken.text
                assert "html" not in broken.json()
            finally:
                assert digest.startswith("sha256:") and len(digest) == 71
                assert all(c in "0123456789abcdef" for c in digest[7:])
                sql(
                    suite,
                    f"UPDATE application_page_publications SET manifest_json=JSON_SET(manifest_json,'$.sdk.contentHash','{digest}') WHERE tenant_id=UNHEX('{tenant}') AND id=UNHEX('{publication_id}')",  # noqa: S608 — normalized fixture IDs and validated digest in a temporary namespace.
                )
            recovered = owner.get(route, params={"batchId": batch["batchId"]})
            assert recovered.status_code == 200 and recovered.json()["publication"] == original["publication"]
            observations.append(
                {
                    "batchId": batch["batchId"],
                    "originalPublication": publication["id"],
                    "currentPublication": current["id"],
                    "historical": restored["historical"],
                    "conflictingTargetsStatus": conflict.status_code,
                    "staleSubmitStatus": stale.status_code,
                    "tamperedSdkStatus": broken.status_code,
                    "tamperedField": "frozen_page_manifest.sdk.contentHash",
                    "restoredAfterFault": True,
                }
            )
            report(
                suite,
                "E10-V02-V03-S01-history-progress",
                passed=len(observations) == len(suite["cases"]),
                complete=len(observations) == len(suite["cases"]),
                cases=observations,
            )
    run_playwright(
        Path(suite["installed"]["root"]),
        "plan10-browser-history",
        ("tests/plan10-employee-operations.spec.ts", "--grep", "restores", "--workers=1"),
        {
            **os.environ,
            "AGENTX_E2E_RUN_ID": suite["runId"],
            "AGENTX_E2E_STAGE": "helm-agentxctl",
            "AGENTX_E2E_BASE_URL": suite["urls"]["web"],
            "AGENTX_E2E_PLAN10_DIRECTORY": str(suite["directory"]),
            "AGENTX_E2E_PLAN9_EMPLOYEE": suite["employee"]["username"],
            "AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD": suite["employee"]["password"].value,
        },
    )
    report(suite, "E10-V02-V03-S01-history", cases=observations, browserRestorationPassed=True)
