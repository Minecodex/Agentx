"""Object authorization, exact CSV parsing/export and safe derived intentions."""

from __future__ import annotations

import csv
import io
import uuid

import httpx
import pytest

from tests.e2e.product.live_text_support import post
from tests.e2e.product.plan10.support import (
    client,
    command,
    coordinator_paused,
    crash_coordinator,
    create,
    get,
    hexid,
    items,
    replace_version,
    report,
    scene,
    sql,
    unknown_receipt,
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


def upload(http, text="Batch source file\n"):
    response = http.post(
        "/gateway/v1/artifacts",
        headers={"Idempotency-Key": str(uuid.uuid4())},
        files={"file": ("source.txt", text.encode(), "text/plain")},
    )
    assert response.status_code == 201, response.text
    return response.json()


def test_batch_object_and_file_access_do_not_expand_to_other_keys(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "file")
    key = post(
        suite["control"], f"/applications/{case['applicationId']}/api-keys", {"name": "Other independent batch caller"}
    )
    with coordinator_paused(suite), client(suite, case) as http:
        artifact = upload(http)
        batch = create(http, case, [{"request_id": "FILE-OWNED", "document": artifact}])
        with httpx.Client(
            base_url=suite["urls"]["runtime"], headers={"Authorization": f"Bearer {key['secret']}"}, timeout=120
        ) as other:
            for suffix in ("", "/items", "/exports"):
                if suffix == "/exports":
                    response = other.post(
                        f"/gateway/v1/batches/{batch['batchId']}{suffix}",
                        headers={"Idempotency-Key": str(uuid.uuid4())},
                    )
                else:
                    response = other.get(f"/gateway/v1/batches/{batch['batchId']}{suffix}")
                assert response.status_code in (401, 403, 404), response.text
            foreign = other.post(
                f"/gateway/v1/applications/{case['slug']}/batches",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={
                    "name": "foreign file",
                    "items": [
                        {"clientItemId": "foreign", "input": {"request_id": "FILE-FOREIGN", "document": artifact}}
                    ],
                },
            )
            assert foreign.status_code in (400, 401, 403, 404, 422), foreign.text
        command(http, batch["batchId"], "stop")
        references = sql(
            suite,
            f"SELECT COUNT(*) FROM artifact_references r JOIN application_batch_items i ON r.owner_id=CAST(BIN_TO_UUID(i.id) AS CHAR) COLLATE utf8mb4_0900_ai_ci AND r.owner_type='batch_item' WHERE i.batch_id=UNHEX('{hexid(batch['batchId'])}') AND r.artifact_id=UNHEX('{hexid(artifact['artifactId'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
        assert references == "1", "Stopped, unadmitted input must remain retained with its history"
        report(suite, "E10-S01-S03", batch=batch, artifactId=artifact["artifactId"], foreignStatus=foreign.status_code)


def test_csv_tsv_paste_are_bounded_and_do_not_invoke_business(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "quote")
    with client(suite, case) as http:
        before = sql(suite, "SELECT COUNT(*) FROM application_invocations")
        path = f"/gateway/v1/applications/{case['slug']}/batch-imports/parse"

        def key():
            return {"Idempotency-Key": str(uuid.uuid4())}

        parsed = http.post(
            path,
            headers=key(),
            json={"text": 'name,value\n"first\nsecond",1\na,2\na,2\n', "artifactId": None, "delimiter": ","},
        )
        assert parsed.status_code == 200, parsed.text
        assert parsed.json() == {
            "headers": ["name", "value"],
            "rows": [["first\nsecond", "1"], ["a", "2"], ["a", "2"]],
            "lines": [2, 4, 5],
        }
        tab = http.post(path, headers=key(), json={"text": "name\tvalue\na\t=1+1\n", "delimiter": "\t"})
        assert tab.status_code == 200 and tab.json()["rows"][0][1] == "=1+1"
        for text, delimiter in (("a,b\n1\n", ","), ("a\n" + "x\n" * 101, ","), ("a\n1", ";")):
            rejected = http.post(path, headers=key(), json={"text": text, "delimiter": delimiter})
            assert rejected.status_code in (400, 422), rejected.text
        remote = http.post(path, headers=key(), json={"url": "http://169.254.169.254/", "delimiter": ","})
        assert remote.status_code in (400, 422)
        assert sql(suite, "SELECT COUNT(*) FROM application_invocations") == before
        report(suite, "E10-S04", csv=parsed.json(), tsv=tab.json(), remoteStatus=remote.status_code)


def test_frozen_export_preserves_queued_and_cancelled_states_and_escapes_formulas(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    namespace = suite["installed"]["runtime_namespace"]
    with coordinator_paused(suite), client(suite, case) as http:
        response = http.post(
            f"/gateway/v1/applications/{case['slug']}/batches",
            headers={"Idempotency-Key": str(uuid.uuid4())},
            json={
                "name": "Export snapshot",
                "items": [
                    {"clientItemId": "=SUM(A1:A2)", "input": case["inputs"][0]},
                    {"clientItemId": "second", "input": case["inputs"][1]},
                ],
            },
        )
        assert response.status_code == 202, response.text
        batch = response.json()
        exported = command(http, batch["batchId"], "exports")
        command(http, batch["batchId"], "stop")
        export_id = hexid(exported["exportId"])
        sql(
            suite,
            f"CREATE TRIGGER plan10_export_crash BEFORE UPDATE ON runtime_commands FOR EACH ROW SET @plan10_export_crash=IF(OLD.id=UNHEX('{export_id}') AND OLD.status='processing' AND NEW.status='completed',SLEEP(15),0)",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
    try:
        fault = wait(
            lambda: sql(
                suite,
                "SELECT ID,STATE,INFO FROM information_schema.PROCESSLIST WHERE DB=DATABASE() AND STATE='User sleep'",
            ),
            bool,
            120,
        )
        # The blob was written before finish_tx updates this command; metadata
        # and quota are still uncommitted. Recovery must validate that same blob.
        report(suite, "E10-S05-crash-point", passed=False, phase="fault_observed", faultProcess=fault)
        crash = crash_coordinator(suite)
    finally:
        sql(suite, "DROP TRIGGER IF EXISTS plan10_export_crash")
    run(("kubectl", "-n", namespace, "rollout", "status", "deployment/workflow-runtime", "--timeout=180s"), timeout=200)
    with client(suite, case) as http:
        job = wait(
            lambda: http.get(f"/gateway/v1/batches/{batch['batchId']}/exports/{exported['exportId']}").json(),
            lambda job: job["status"] in ("completed", "failed"),
        )
        assert job["status"] == "completed", job
        downloaded = http.get(f"/gateway/v1/batches/{batch['batchId']}/exports/{job['exportId']}/file")
        assert downloaded.status_code == 200
        rows = list(csv.DictReader(io.StringIO(downloaded.text)))
        assert [row["state"] for row in rows] == ["queued", "queued"]
        assert rows[0]["clientItemId"] == "'=SUM(A1:A2)"
        assert items(http, batch["batchId"])["items"][0]["clientItemId"] == "=SUM(A1:A2)"
        assert get(http, batch["batchId"])["counts"]["cancelled"] == 2
        attempts = int(sql(suite, f"SELECT attempt_count FROM runtime_commands WHERE id=UNHEX('{export_id}')"))  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        assert attempts >= 2
        report(
            suite,
            "E10-S05",
            faultProcess=fault,
            crash=crash,
            commandAttempts=attempts,
            snapshot=rows,
            export=job,
            current=get(http, batch["batchId"]),
        )


def test_safe_failed_items_create_current_version_derived_batches(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "file")
    with coordinator_paused(suite), client(suite, case) as http:
        artifact = upload(http, "Recover this exact accepted file.\n")
        batch = create(http, case, [{"request_id": "SAFE-DERIVED-FILE", "document": artifact}])
        sql(
            suite,
            f"UPDATE artifacts SET deleted_at=UTC_TIMESTAMP(6) WHERE id=UNHEX('{hexid(artifact['artifactId'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
    try:
        with client(suite, case) as http:
            rejected = wait(lambda: get(http, batch["batchId"]), lambda b: b["counts"]["failed"] == 1)
            project = items(http, batch["batchId"])["items"][0]
            assert project["task"] is None and project["canRerun"] and project["dispatchState"] == "rejected"
            sql(suite, f"UPDATE artifacts SET deleted_at=NULL WHERE id=UNHEX('{hexid(artifact['artifactId'])}')")  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            replace_version(
                suite,
                case,
                lambda definition: definition["start"]["inputs"]["properties"].update(
                    {"new_optional_field": {"type": "string"}}
                ),
            )
            conflict = http.post(
                f"/gateway/v1/batches/{batch['batchId']}/rerun",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={"itemIds": [project["itemId"]]},
            )
            assert conflict.status_code == 409, conflict.text
            replace_version(
                suite, case, lambda definition: definition["start"]["inputs"]["properties"].pop("new_optional_field")
            )
            key = str(uuid.uuid4())
            derived = command(http, batch["batchId"], "rerun", {"itemIds": [project["itemId"]]}, key)
            assert derived["batchId"] != batch["batchId"] and derived["bundleId"] != batch["bundleId"]
            assert derived["sourceBatchId"] == batch["batchId"]
            replay = command(http, batch["batchId"], "rerun", {"itemIds": [project["itemId"]]}, key)
            assert replay["batchId"] == derived["batchId"]
            final = wait(lambda: get(http, derived["batchId"]), lambda b: b["counts"]["ended"] == 1, 300)
            assert final["counts"]["completed"] == 1, items(http, derived["batchId"])
            assert items(http, batch["batchId"])["items"][0]["task"] is None
            row = items(http, derived["batchId"])["items"][0]
            assert row["sourceItemId"] == project["itemId"] and row["input"] == project["input"]
            base = f"/api/v1/applications/{case['applicationId']}/batches"
            manager = suite["control"].post(
                f"{base}/{batch['batchId']}/rerun",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={"itemIds": [project["itemId"]]},
            )
            assert manager.status_code == 200, manager.text
            manager = manager.json()
            assert manager["callerType"] == "user" and manager["callerId"] == suite["me"]["id"]
            manager_final = wait(
                lambda: suite["control"].get(f"{base}/{manager['batchId']}").json(),
                lambda value: value["counts"]["ended"] == 1,
                300,
            )
            assert manager_final["counts"]["completed"] == 1
            manager_row = suite["control"].get(f"{base}/{manager['batchId']}/items").json()["items"][0]
            assert manager_row["input"] == project["input"] and manager_row["sourceItemId"] == project["itemId"]
            report(
                suite,
                "E10-R06-derived",
                original=rejected,
                derived=derived,
                final=final,
                project=row,
                schemaConflict=conflict.json(),
                managerDerived=manager,
                managerFinal=manager_final,
                managerProject=manager_row,
            )
    finally:
        sql(suite, f"UPDATE artifacts SET deleted_at=NULL WHERE id=UNHEX('{hexid(artifact['artifactId'])}')")  # noqa: S608 — normalized fixture IDs in a temporary namespace.


def test_unknown_effect_receipt_blocks_rerun_without_new_calls(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "quote")
    with client(suite, case) as http:
        batch = create(http, case, [case["inputs"][0]])
        wait(lambda: get(http, batch["batchId"]), lambda b: b["counts"]["ended"] == 1, 300)
        project = items(http, batch["batchId"])["items"][0]
        with unknown_receipt(suite, project["task"]["taskId"]):
            unknown = get(http, batch["batchId"])
            assert unknown["counts"]["outcomeUnknown"] == 1 and unknown["counts"]["ended"] == 0
            before = sql(suite, "SELECT COUNT(*) FROM application_invocations")
            denied = http.post(
                f"/gateway/v1/batches/{batch['batchId']}/rerun",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={"itemIds": [project["itemId"]]},
            )
            assert denied.status_code == 409 and sql(suite, "SELECT COUNT(*) FROM application_invocations") == before
            report(
                suite,
                "E10-R06-unknown",
                mode="injected_authoritative_unsettled_receipt",
                batch=unknown,
                denied=denied.json(),
            )
