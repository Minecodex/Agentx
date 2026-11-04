"""Exercise employee controls on genuine AI HTML, with real safe failures and lineage."""

import json
import os
import uuid
from contextlib import ExitStack, contextmanager
from pathlib import Path

import pytest

from tests.e2e.product.next_batch_support import get as control_get
from tests.e2e.product.plan10.support import coordinator_paused, hexid, report, scene, sql, wait
from tests.e2e.product.plan10.test_08_history import employee_client
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.gateway, pytest.mark.live_model]


def select_page_business_version(suite, case, bundle):
    current = sql(
        suite,
        f"SELECT HEX(bundle_id) FROM deployment_heads WHERE application_id=UNHEX('{hexid(case['applicationId'])}')",  # noqa: S608 — normalized owned fixture UUID.
    )
    if current.lower() == hexid(bundle):
        return
    # Reusing this environment creates a fresh API credential. A normal publish
    # prepares its current admission snapshot before restoring the original page
    # version; rollback alone deliberately does not rebuild an old bundle.
    deployments = control_get(suite["control"], f"/applications/{case['applicationId']}/deployments")
    original = next(item for item in deployments if item["id"] == case["deploymentId"])
    _publish_application_deployment(
        suite["control"],
        dict(suite["control"].headers),
        case["applicationId"],
        original["workflowVersionId"],
        original["environmentId"],
    )
    response = suite["control"].post(
        f"/api/v1/applications/{case['applicationId']}/deployments/{case['deploymentId']}:rollback"
    )
    assert response.status_code == 202, response.text
    attempt = response.json()

    def activation():
        value = control_get(suite["control"], f"/applications/{case['applicationId']}/publish-attempts/{attempt['id']}")
        assert value["state"] != "rejected", value
        return value

    wait(
        activation,
        lambda value: value["state"] == "active",
        300,
    )
    wait(
        lambda: sql(
            suite,
            f"SELECT HEX(bundle_id) FROM deployment_heads WHERE application_id=UNHEX('{hexid(case['applicationId'])}')",  # noqa: S608 — normalized owned fixture UUID.
        ),
        lambda value: value.lower() == hexid(bundle),
    )


@contextmanager
def caller_unavailable(suite):
    """Inject and restore exactly one owned admission projection; keep the real account intact."""
    user, tenant = hexid(suite["employee"]["id"]), hexid(suite["me"]["companyId"])
    where = f"tenant_id=UNHEX('{tenant}') AND user_id=UNHEX('{user}')"
    assert sql(suite, f"SELECT status FROM runtime_user_admission WHERE {where}") == "active"  # noqa: S608 — normalized fixture UUIDs.
    try:
        sql(suite, f"UPDATE runtime_user_admission SET status='disabled' WHERE {where}")  # noqa: S608 — normalized fixture UUIDs.
        yield
    finally:
        sql(suite, f"UPDATE runtime_user_admission SET status='active' WHERE {where}")  # noqa: S608 — normalized fixture UUIDs.
        assert sql(suite, f"SELECT status FROM runtime_user_admission WHERE {where}") == "active"  # noqa: S608 — normalized fixture UUIDs.


def unavailable_page_bundle(suite, case, owner, publication, suffix, failed):
    """A disabled bound version is a business rejection, never a database outage or partial batch."""
    bundle = publication["contract"]["bundleId"]
    deployments = control_get(suite["control"], f"/applications/{case['applicationId']}/deployments")
    original = next(item for item in deployments if item["id"] == case["deploymentId"])
    _publish_application_deployment(
        suite["control"],
        dict(suite["control"].headers),
        case["applicationId"],
        original["workflowVersionId"],
        original["environmentId"],
    )
    where = f"id=UNHEX('{hexid(bundle)}') AND tenant_id=UNHEX('{hexid(suite['me']['companyId'])}')"
    previous = sql(suite, f"SELECT status FROM deployment_bundles WHERE {where}")  # noqa: S608 — normalized fixture UUIDs.
    assert previous == "superseded"
    count_sql = f"SELECT COUNT(*) FROM application_batches WHERE application_id=UNHEX('{hexid(case['applicationId'])}')"  # noqa: S608 — normalized fixture UUID.
    count = sql(suite, count_sql)
    try:
        projects = owner.get(f"/gateway/v1/batches/{failed['batchId']}/items").json()["items"]
        rerun = owner.post(
            f"/gateway/v1/batches/{failed['batchId']}/rerun",
            headers={"Idempotency-Key": str(uuid.uuid4())},
            json={"itemIds": [row["itemId"] for row in projects]},
        )
        assert rerun.status_code == 409, rerun.text
        assert sql(suite, count_sql) == count
        sql(suite, f"UPDATE deployment_bundles SET status='disabled' WHERE {where}")  # noqa: S608 — normalized fixture UUIDs.
        response = owner.post(
            f"/gateway/v1/pages/{suffix}/batches",
            headers={"Idempotency-Key": str(uuid.uuid4())},
            json={
                "publicationId": publication["publicationId"],
                "revision": publication["revision"],
                "name": "Unavailable page version",
                "items": [{"clientItemId": "rejected-row", "input": case["inputs"][0]}],
            },
        )
        assert response.status_code == 404, response.text
        assert sql(suite, count_sql) == count
        return {
            "status": response.status_code,
            "error": response.json(),
            "batchRowsUnchanged": True,
            "rerunRequiresCurrentBusinessVersion": rerun.status_code,
        }
    finally:
        sql(suite, f"UPDATE deployment_bundles SET status='superseded' WHERE {where}")  # noqa: S608 — restore the exact captured fixture lifecycle.
        select_page_business_version(suite, case, bundle)


def test_employee_generated_page_stop_cancel_and_safe_rerun(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "quote")
    completed = json.loads((suite["directory"] / "quote-complete.json").read_text())
    suffix = completed["publication"]["slug"]
    payload_path = suite["directory"] / "employee-operations-input.json"
    environment = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": suite["runId"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": suite["urls"]["web"],
        "AGENTX_E2E_RUNTIME_URL": suite["urls"]["runtime"],
        "AGENTX_E2E_PLAN10_EMPLOYEE_OPERATIONS": str(payload_path),
        "AGENTX_E2E_PLAN10_DIRECTORY": str(suite["directory"]),
        "AGENTX_E2E_PLAN9_EMPLOYEE": suite["employee"]["username"],
        "AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD": suite["employee"]["password"].value,
    }

    def browser(grep):
        run_playwright(
            Path(suite["installed"]["root"]),
            f"plan10-employee-{grep}",
            ("tests/plan10-employee-operations.spec.ts", "--grep", grep, "--workers=1"),
            environment,
        )

    select_page_business_version(suite, case, completed["batches"][0]["batch"]["bundleId"])
    with employee_client(suite, suite["employee"]) as owner:
        current = owner.get(f"/gateway/v1/pages/{suffix}")
        assert current.status_code == 200, current.text
        publication = current.json()["publication"]

        def create(name):
            response = owner.post(
                f"/gateway/v1/pages/{suffix}/batches",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={
                    "publicationId": publication["publicationId"],
                    "revision": publication["revision"],
                    "name": name,
                    "items": [
                        {
                            "clientItemId": f"employee-row-{index}",
                            "input": {
                                **case["inputs"][index % len(case["inputs"])],
                                "customer": f"employee-action-{uuid.uuid4()}",
                            },
                        }
                        for index in range(3)
                    ],
                },
            )
            assert response.status_code == 202, response.text
            return response.json()

        def batch(identifier):
            response = owner.get(f"/gateway/v1/batches/{identifier}")
            assert response.status_code == 200, response.text
            return response.json()

        with ExitStack() as restore:
            with coordinator_paused(suite):
                batches = {action: create(f"Employee {action} verification") for action in ("stop", "cancel", "failed")}
                payload = {
                    "suffix": suffix,
                    "employeeId": suite["employee"]["id"],
                    "publicationId": publication["publicationId"],
                    **batches,
                }
                payload_path.write_text(json.dumps(payload, indent=2))
                browser("stops")
                assert all(
                    batch(batches[action]["batchId"])["counts"]["cancelled"] == 3 for action in ("stop", "cancel")
                )
                restore.enter_context(caller_unavailable(suite))
            failed = wait(
                lambda: (
                    suite["control"]
                    .get(f"/api/v1/applications/{case['applicationId']}/batches/{batches['failed']['batchId']}")
                    .json()
                ),
                lambda value: value.get("counts", {}).get("failed") == 3,
            )
        wait(lambda: batch(failed["batchId"]), lambda value: value["canRerun"])
        unavailable = unavailable_page_bundle(suite, case, owner, publication, suffix, failed)
        browser("safe rerun")
        derived = json.loads((suite["directory"] / "E10-U06-employee-rerun.json").read_text())["derived"]
        final = wait(lambda: batch(derived["batchId"]), lambda value: value["counts"]["ended"] == 3, 360)
        assert final["counts"]["completed"] == 3
        assert final["bundleId"] == publication["contract"]["bundleId"]
        source_items = owner.get(f"/gateway/v1/batches/{failed['batchId']}/items").json()["items"]
        derived_items = owner.get(f"/gateway/v1/batches/{derived['batchId']}/items").json()["items"]
        assert {row["sourceItemId"] for row in derived_items} == {row["itemId"] for row in source_items}
        assert all(row["task"] is None for row in source_items)
        for row in derived_items:
            data = row["input"]
            assert row["task"]["outputs"]["customer"] == data["customer"]
            assert row["task"]["outputs"]["total"] == pytest.approx(
                data["quantity"] * data["unitPrice"] * (1 - data["discountPercent"] / 100), abs=0.01
            )
        assert batch(failed["batchId"])["counts"]["failed"] == 3
        payload["completed"] = {"batch": final, "item": derived_items[0]}
        payload_path.write_text(json.dumps(payload, indent=2))
        browser("keyboard")
        report(
            suite,
            "E10-U06-A03-employee-operations",
            source=failed,
            derived=final,
            sourceItems=source_items,
            derivedItems=derived_items,
            unavailableBundle=unavailable,
        )
