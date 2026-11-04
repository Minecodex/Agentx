"""Exercise the existing call-records position, object permissions and desktop themes."""

import json
import os
import uuid
from pathlib import Path

import pytest

from tests.e2e.product.live_text_support import Secret, post
from tests.e2e.product.plan10.support import client, coordinator_paused, create, hexid, report, scene, sql, wait
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_management_desktop_actions_and_authorization(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    control = suite["control"]
    revoked = post(control, f"/applications/{case['applicationId']}/api-keys", {"name": "Management source caller"})
    other = {**case, "secret": Secret(revoked["secret"])}
    path = suite["directory"] / "management-input.json"
    env = {
        **os.environ,
        "AGENTX_E2E_RUN_ID": suite["runId"],
        "AGENTX_E2E_STAGE": "helm-agentxctl",
        "AGENTX_E2E_BASE_URL": suite["urls"]["web"],
        "AGENTX_E2E_RUNTIME_URL": suite["urls"]["runtime"],
        "AGENTX_E2E_PLAN10_MANAGEMENT": str(path),
    }
    with coordinator_paused(suite), client(suite, case) as http, client(suite, other) as caller:
        wait(
            lambda: caller.get(f"/gateway/v1/applications/{case['slug']}/batches").status_code,
            lambda status: status == 200,
        )
        stop = create(http, case, [case["inputs"][0]] * 3, name="Manager stops pending work")
        cancel = create(http, case, [case["inputs"][1]] * 3, name="Manager cancels unfinished work")
        failed = create(caller, other, [case["inputs"][0]] * 3, name="Safe manager-derived source")
        receipt = control.post(f"/api/v1/applications/{case['applicationId']}/api-keys/{revoked['id']}/revoke")
        assert receipt.status_code == 204
        wait(
            lambda: sql(suite, f"SELECT status FROM api_key_admission WHERE key_id=UNHEX('{hexid(revoked['id'])}')"),  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            lambda value: value == "revoked",
        )
        path.write_text(
            json.dumps(
                {
                    "applicationId": case["applicationId"],
                    "stop": stop,
                    "cancel": cancel,
                    "failed": failed,
                    "managerId": suite["me"]["id"],
                },
                indent=2,
            )
        )
        run_playwright(
            Path(suite["installed"]["root"]),
            "plan10-management-operations",
            ("tests/plan10-management.spec.ts", "--grep", "stop and cancel"),
            env,
        )
    base = f"/api/v1/applications/{case['applicationId']}/batches"
    wait(lambda: control.get(f"{base}/{failed['batchId']}").json(), lambda value: value["counts"]["failed"] == 3)
    run_playwright(
        Path(suite["installed"]["root"]),
        "plan10-management-themes",
        ("tests/plan10-management.spec.ts", "--grep", "themes and safe rerun"),
        env,
    )
    # Current employee credentials have no execution-view or management permissions.
    employee = suite["employee"]
    login = control.post(
        "/api/v1/auth/login", json={"username": employee["username"], "password": employee["password"].value}, auth=None
    )
    assert login.status_code == 200
    import httpx

    with httpx.Client(
        base_url=suite["urls"]["web"], headers={"Authorization": f"Bearer {login.json()['accessToken']}"}, timeout=60
    ) as limited:
        for suffix in ("", f"/{stop['batchId']}", f"/{stop['batchId']}/items"):
            assert limited.get(base + suffix).status_code == 403
        for action in ("stop", "cancel", "rerun", "exports"):
            denied = limited.post(
                f"{base}/{failed['batchId']}/{action}",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={"itemIds": []} if action == "rerun" else None,
            )
            assert denied.status_code == 403
    report(suite, "E10-U01-U06-S02-management", stop=stop, cancel=cancel, failed=failed, limitedUserDenied=True)
