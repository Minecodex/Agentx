"""Frozen business inputs and identities coexist with live admission decisions."""

from __future__ import annotations

import json
import uuid

import pytest

from tests.e2e.product.live_text_support import Secret, post
from tests.e2e.product.plan10.support import (
    client,
    command,
    coordinator_paused,
    create,
    get,
    hexid,
    items,
    replace_version,
    report,
    scene,
    sql,
    wait,
)
from tests.e2e.support import run

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.product,
    pytest.mark.gateway,
    pytest.mark.security,
    pytest.mark.live_model,
]


def test_opaque_intent_and_row_ids_are_case_sensitive(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    key = str(uuid.uuid4())
    request = {
        "name": "Exact row identifiers",
        "items": [{"clientItemId": name, "input": case["inputs"][0]} for name in ("row", "ROW", "rów", "row ")],
    }
    with coordinator_paused(suite), client(suite, case) as http:
        batches = []
        for intent in (key.lower(), key.upper()):
            response = http.post(
                f"/gateway/v1/applications/{case['slug']}/batches", headers={"Idempotency-Key": intent}, json=request
            )
            assert response.status_code == 202, response.text
            batches.append(response.json())
        assert batches[0]["batchId"] != batches[1]["batchId"]
        assert [i["clientItemId"] for i in items(http, batches[0]["batchId"])["items"]] == ["row", "ROW", "rów", "row "]
        duplicate = {**request, "items": request["items"] * 2}
        denied = http.post(
            f"/gateway/v1/applications/{case['slug']}/batches",
            headers={"Idempotency-Key": str(uuid.uuid4())},
            json=duplicate,
        )
        assert denied.status_code in (400, 422)
        for batch in batches:
            command(http, batch["batchId"], "stop")
        report(suite, "E10-C02-C03-identifiers", batches=batches, duplicateStatus=denied.status_code)


def test_publication_switch_preserves_bundle_and_absent_user_identity(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    with coordinator_paused(suite), client(suite, case) as http:
        original = create(http, case, [case["inputs"][0]] * 3)
        original_id = hexid(original["batchId"])
        policy = sql(
            suite,
            f"SELECT CAST(identity_policy_json AS CHAR) FROM application_batches WHERE id=UNHEX('{original_id}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
        replace_version(
            suite,
            case,
            lambda definition: definition["start"]["inputs"]["properties"].update(
                {"optional_later": {"type": "string"}}
            ),
            api_user_id_input="request_id",
        )
        current = create(http, case, [case["inputs"][1]])
        assert current["bundleId"] != original["bundleId"]
        forged = http.post(
            f"/gateway/v1/applications/{case['slug']}/batches",
            headers={"Idempotency-Key": str(uuid.uuid4())},
            json={
                "name": "forged old binding",
                "items": [{"clientItemId": "one", "input": case["inputs"][0]}],
                "bundleId": original["bundleId"],
                "userId": "other",
            },
        )
        assert forged.status_code in (400, 422)
    with client(suite, case) as http:
        wait(lambda: get(http, original["batchId"]), lambda value: value["counts"]["waitingInput"] == 3)
        wait(lambda: get(http, current["batchId"]), lambda value: value["counts"]["waitingInput"] == 1)
        rows = sql(
            suite,
            f"SELECT COUNT(*),SUM(v.bundle_id=UNHEX('{hexid(original['bundleId'])}')),SUM(JSON_TYPE(v.user_identity_json)='NULL'),SUM(JSON_TYPE(JSON_EXTRACT(s.execution_context_json,'$.endUser'))='NULL') FROM application_invocations v JOIN application_batch_items i ON i.invocation_id=v.id JOIN execution_snapshots s ON s.tenant_id=v.tenant_id AND s.execution_id=v.execution_id WHERE i.batch_id=UNHEX('{original_id}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
        assert rows == "3\t3\t3\t3", rows
        new_identity = json.loads(
            sql(
                suite,
                f"SELECT CAST(v.user_identity_json AS CHAR) FROM application_invocations v JOIN application_batch_items i ON i.invocation_id=v.id WHERE i.batch_id=UNHEX('{hexid(current['batchId'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            )
        )
        assert new_identity["externalUserId"] == case["inputs"][1]["request_id"]
        for batch in (original, current):
            command(http, batch["batchId"], "cancel")
        report(
            suite,
            "E10-V01-identity",
            original=original,
            current=current,
            frozenPolicy=policy,
            frozenRows=rows,
            currentIdentity=new_identity,
            forgedStatus=forged.status_code,
        )
        replace_version(
            suite, case, lambda definition: definition["start"]["inputs"]["properties"].pop("optional_later")
        )


def test_revocation_blocks_pending_rows_and_manager_rerun_uses_manager_identity(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    control = suite["control"]
    key = post(
        control, f"/applications/{case['applicationId']}/api-keys", {"name": "Revocation during durable waiting"}
    )
    revoked_case = {**case, "secret": Secret(key["secret"])}
    with coordinator_paused(suite), client(suite, revoked_case) as http:
        batch = create(http, revoked_case, [case["inputs"][0]] * 3)
        revoked = control.post(f"/api/v1/applications/{case['applicationId']}/api-keys/{key['id']}/revoke")
        assert revoked.status_code == 204, revoked.text
        wait(
            lambda: sql(suite, f"SELECT status FROM api_key_admission WHERE key_id=UNHEX('{hexid(key['id'])}')"),  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            lambda value: value == "revoked",
        )
    base = f"/api/v1/applications/{case['applicationId']}/batches"

    def read_batch():
        response = control.get(f"{base}/{batch['batchId']}")
        assert response.status_code == 200, response.text
        return response.json()

    final = wait(read_batch, lambda value: value["counts"]["failed"] == 3)
    projects = control.get(f"{base}/{batch['batchId']}/items").json()["items"]
    assert all(row["task"] is None and row["canRerun"] for row in projects)
    with client(suite, revoked_case) as http:
        assert http.get(f"/gateway/v1/batches/{batch['batchId']}").status_code in (401, 403)
    derived = control.post(
        f"{base}/{batch['batchId']}/rerun",
        headers={"Idempotency-Key": str(uuid.uuid4())},
        json={"itemIds": [row["itemId"] for row in projects]},
    )
    assert derived.status_code == 200, derived.text
    new = derived.json()
    assert new["callerType"] == "user" and new["callerId"] == suite["me"]["id"]
    cancelled = control.post(f"{base}/{new['batchId']}/cancel", headers={"Idempotency-Key": str(uuid.uuid4())})
    assert cancelled.status_code == 200, cancelled.text
    report(suite, "E10-S02-revocation", original=final, rejected=projects, derived=new)


def test_backpressure_and_redis_restart_preserve_pending_rows(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    namespace = suite["installed"]["runtime_namespace"]
    with coordinator_paused(suite):
        run(
            (
                "kubectl",
                "-n",
                namespace,
                "set",
                "env",
                "deployment/workflow-runtime",
                "AGENTX_RUNTIME_GATEWAY_ADMISSION_MAX_RUNNING=0",
            )
        )
    try:
        with client(suite, case) as http:
            batch = create(http, case, [case["inputs"][0]] * 3)
            wait(
                lambda: sql(
                    suite,
                    f"SELECT attempt_count FROM runtime_commands WHERE command_type='dispatch_batch' AND aggregate_id='{uuid.UUID(batch['batchId'])}'",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
                ),
                lambda value: int(value) >= 2,
            )
            pending = get(http, batch["batchId"])
            assert pending["counts"]["queued"] == 3 and pending["counts"]["failed"] == 0
            assert all(row["task"] is None for row in items(http, batch["batchId"])["items"])
            run(("kubectl", "-n", namespace, "delete", "pod", "runtime-redis-0"), timeout=120)
            run(
                ("kubectl", "-n", namespace, "rollout", "status", "statefulset/runtime-redis", "--timeout=180s"),
                timeout=200,
            )
    finally:
        with coordinator_paused(suite):
            run(
                (
                    "kubectl",
                    "-n",
                    namespace,
                    "set",
                    "env",
                    "deployment/workflow-runtime",
                    "AGENTX_RUNTIME_GATEWAY_ADMISSION_MAX_RUNNING-",
                )
            )
    with client(suite, case) as http:
        final = wait(lambda: get(http, batch["batchId"]), lambda value: value["counts"]["waitingInput"] == 3)
        command(http, batch["batchId"], "cancel")
        report(suite, "E10-R02-R03-pressure", pending=pending, recovered=final)


def test_api_batch_without_a_page_keeps_that_acceptance_snapshot(plan10_suite):
    import httpx

    suite = plan10_suite
    case = scene(suite, "expense")
    employee = suite["employee"]
    login = suite["control"].post(
        "/api/v1/auth/login", auth=None, json={"username": employee["username"], "password": employee["password"].value}
    )
    assert login.status_code == 200
    with httpx.Client(
        base_url=suite["urls"]["runtime"],
        headers={"Authorization": f"Bearer {login.json()['accessToken']}"},
        timeout=60,
        trust_env=False,
    ) as http:
        batch = create(http, case, [case["inputs"][0]], name="API accepted before any business page")
        assert batch["source"] == "api" and batch["pagePublicationId"] is None
        command(http, batch["batchId"], "cancel")
        report(suite, "E10-V03-api-without-page", batch=batch, employeeId=employee["id"])


def test_disabling_application_rejects_only_its_unadmitted_rows(plan10_suite):
    from tests.e2e.product.next_batch_support import get as control_get
    from tests.e2e.product.test_provider_integration import _publish_application_deployment

    suite, control = plan10_suite, plan10_suite["control"]
    original = scene(suite, "expense")
    app = post(
        control,
        "/applications",
        {
            "workflowId": original["workflowId"],
            "name": "Disabled durable batch acceptance",
            "slug": f"disabled-batch-{suite['runId']}",
            "visibility": "company",
        },
    )
    deployments = control_get(control, f"/applications/{original['applicationId']}/deployments")
    environment = next(value["environmentId"] for value in deployments if value["id"] == original["deploymentId"])
    _publish_application_deployment(control, dict(control.headers), app["id"], original["versionId"], environment)
    key = post(control, f"/applications/{app['id']}/api-keys", {"name": "Disabled application admission"})
    case = {"applicationId": app["id"], "slug": app["slug"], "secret": Secret(key["secret"])}
    with coordinator_paused(suite), client(suite, case) as http:
        wait(
            lambda: http.get(f"/gateway/v1/applications/{case['slug']}/batches").status_code,
            lambda status: status == 200,
        )
        batch = create(http, case, [original["inputs"][0]] * 3)
        current = control_get(control, f"/applications/{app['id']}")
        disabled = control.patch(
            f"/api/v1/applications/{app['id']}",
            json={
                "name": current["name"],
                "description": current["description"],
                "visibility": current["visibility"],
                "status": "disabled",
                "version": current["version"],
            },
        )
        assert disabled.status_code == 200, disabled.text
        wait(
            lambda: sql(
                suite,
                f"SELECT status FROM application_routes WHERE application_id=UNHEX('{hexid(app['id'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            ),
            lambda status: status == "disabled",
        )
    path = f"/api/v1/applications/{app['id']}/batches/{batch['batchId']}"
    final = wait(lambda: control.get(path).json(), lambda value: value["counts"]["failed"] == 3)
    rows = control.get(path + "/items").json()["items"]
    assert all(row["task"] is None and row["dispatchState"] == "rejected" for row in rows)
    report(suite, "E10-S02-disabled-application", batch=final, rows=rows)


def test_disabling_user_invalidates_the_saved_caller_before_dispatch(plan10_suite):
    import httpx

    from tests.e2e.product.next_batch_support import create_user

    suite, control = plan10_suite, plan10_suite["control"]
    case = scene(suite, "expense")
    role = post(
        control,
        "/roles",
        {
            "name": "Temporary pending batch employee",
            "code": f"pending_{suite['runId'].replace('-', '_')}",
            "dataScope": "company",
            "permissions": ["application:invoke"],
        },
    )
    user = create_user(control, f"pending-{suite['runId']}", suite["me"]["departmentId"], role["id"])
    with (
        coordinator_paused(suite),
        httpx.Client(
            base_url=suite["urls"]["runtime"],
            headers={"Authorization": f"Bearer {user['token'].value}"},
            timeout=60,
            trust_env=False,
        ) as http,
    ):
        wait(
            lambda: http.get(f"/gateway/v1/applications/{case['slug']}/batches").status_code,
            lambda status: status == 200,
        )
        batch = create(http, case, [case["inputs"][0]] * 3)
        disabled = control.post(f"/api/v1/users/{user['id']}/disable")
        assert disabled.status_code == 204, disabled.text
        wait(
            lambda: sql(suite, f"SELECT status FROM runtime_user_admission WHERE user_id=UNHEX('{hexid(user['id'])}')"),  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            lambda status: status == "disabled",
        )
    path = f"/api/v1/applications/{case['applicationId']}/batches/{batch['batchId']}"
    final = wait(lambda: control.get(path).json(), lambda value: value["counts"]["failed"] == 3)
    rows = control.get(path + "/items").json()["items"]
    assert all(row["task"] is None and row["dispatchState"] == "rejected" for row in rows)
    report(suite, "E10-S02-disabled-user", batch=final, userId=user["id"], rows=rows)


def test_manager_cancels_original_api_tasks_as_the_actual_actor(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    with client(suite, case) as http:
        batch = create(http, case, [case["inputs"][0]] * 3)
        wait(lambda: get(http, batch["batchId"]), lambda value: value["counts"]["waitingInput"] == 3)
        original = items(http, batch["batchId"])["items"]
        response = suite["control"].post(
            f"/api/v1/applications/{case['applicationId']}/batches/{batch['batchId']}/cancel",
            headers={"Idempotency-Key": str(uuid.uuid4())},
        )
        assert response.status_code == 200, response.text
        final = wait(lambda: get(http, batch["batchId"]), lambda value: value["counts"]["cancelled"] == 3)
        task_ids = ",".join("UNHEX('" + hexid(item["task"]["taskId"]) + "')" for item in original)
        actors = sql(
            suite,
            f"SELECT COUNT(*),SUM(JSON_UNQUOTE(JSON_EXTRACT(c.payload_json,'$.actorType'))='user'),SUM(JSON_UNQUOTE(JSON_EXTRACT(c.payload_json,'$.actorId'))='{uuid.UUID(suite['me']['id'])}') FROM runtime_commands c JOIN application_invocations v ON c.aggregate_id=CAST(BIN_TO_UUID(v.execution_id) AS CHAR) COLLATE utf8mb4_0900_ai_ci WHERE c.command_type='cancel_execution' AND v.id IN ({task_ids})",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
        assert actors == "3\t3\t3", actors
        assert all(item["task"]["callerType"] == "api_key" for item in items(http, batch["batchId"])["items"])
        owners = sql(
            suite,
            f"SELECT COUNT(*) FROM application_invocations WHERE id IN ({task_ids}) AND caller_type='api_key' AND caller_id=UNHEX('{hexid(case['keyId'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
        assert owners == "3", owners
        report(
            suite,
            "E10-S02-manager-cancel",
            original=original,
            final=final,
            commandActors=actors,
            managerId=suite["me"]["id"],
        )
