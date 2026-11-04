"""Age only owned temporary objects and exercise the existing retention command/worker."""

import uuid

import pytest

from tests.e2e.product.plan10.support import client, command, create, get, hexid, items, report, scene, sql, wait
from tests.e2e.runtime.test_agent_attachments import _control_mysql

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.gateway, pytest.mark.live_model]


def test_batch_history_and_exports_follow_execution_retention_and_live_holds(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    tenant, actor = hexid(suite["me"]["companyId"]), hexid(suite["me"]["id"])
    # Provision the existing execution retention policy in the owned fixture; there is no separate batch policy.
    _control_mysql(
        suite["installed"],
        f"INSERT INTO retention_policies(tenant_id,data_type,retention_days,enabled,updated_by) VALUES(UNHEX('{tenant}'),'execution',1,TRUE,UNHEX('{actor}')) ON DUPLICATE KEY UPDATE retention_days=1,enabled=TRUE,version=version+1",
    )
    with client(suite, case) as http:
        expired = create(http, case, [case["inputs"][0]])
        live = create(http, case, [case["inputs"][1]])
        wait(lambda: get(http, live["batchId"]), lambda value: value["counts"]["waitingInput"] == 1)
        command(http, expired["batchId"], "cancel")
        wait(lambda: get(http, expired["batchId"]), lambda value: value["counts"]["ended"] == 1)
        exported = command(http, expired["batchId"], "exports")
        job = wait(
            lambda: http.get(f"/gateway/v1/batches/{expired['batchId']}/exports/{exported['exportId']}").json(),
            lambda value: value["status"] in ("completed", "failed"),
        )
        assert job["status"] == "completed", job
        task = items(http, expired["batchId"])["items"][0]["task"]
        wait(
            lambda: sql(
                suite,
                f"SELECT status FROM runtime_commands WHERE command_type='dispatch_batch' AND aggregate_id='{uuid.UUID(expired['batchId'])}'",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            ),
            lambda value: value == "completed",
        )
        for batch in (expired, live):
            sql(
                suite,
                f"UPDATE application_batches SET updated_at=UTC_TIMESTAMP()-INTERVAL 200 DAY WHERE id=UNHEX('{hexid(batch['batchId'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            )
        sql(
            suite,
            f"UPDATE application_invocations SET completed_at=UTC_TIMESTAMP()-INTERVAL 200 DAY WHERE id=UNHEX('{hexid(task['taskId'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )

        def retain():
            receipt = suite["control"].post(
                "/api/v1/retention-runs",
                json={
                    "dryRun": False,
                    "artifactRetentionDays": 1,
                    "traceRetentionDays": 1,
                    "messageRetentionDays": 1,
                    "evaluationRetentionDays": 1,
                },
            )
            assert receipt.status_code == 202, receipt.text
            return receipt.json()

        first = retain()
        wait(
            lambda: sql(
                suite,
                f"SELECT retention_deleted_at IS NOT NULL FROM application_batches WHERE id=UNHEX('{hexid(expired['batchId'])}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            ),
            lambda value: value == "1",
        )
        assert http.get(f"/gateway/v1/batches/{expired['batchId']}").status_code == 404
        assert http.get(f"/gateway/v1/batches/{expired['batchId']}/exports/{job['exportId']}/file").status_code == 404
        assert get(http, live["batchId"])["counts"]["waitingInput"] == 1
        scrubbed = sql(
            suite,
            f"SELECT COUNT(*) FROM application_batch_items WHERE batch_id=UNHEX('{hexid(expired['batchId'])}') AND JSON_TYPE(input_json)='NULL' AND user_identity_json IS NULL",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
        assert scrubbed == "1"
        assert (
            sql(
                suite,
                f"SELECT COUNT(*) FROM bundle_references WHERE owner_id=UNHEX('{hexid(expired['batchId'])}') AND released_at IS NULL",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            )
            == "0"
        )
        artifact = hexid(job["file"]["artifactId"])
        assert sql(suite, f"SELECT COUNT(*) FROM artifact_references WHERE artifact_id=UNHEX('{artifact}')") == "0"  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        sql(suite, f"UPDATE artifacts SET created_at=UTC_TIMESTAMP()-INTERVAL 200 DAY WHERE id=UNHEX('{artifact}')")  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        second = retain()
        wait(
            lambda: sql(suite, f"SELECT deleted_at IS NOT NULL FROM artifacts WHERE id=UNHEX('{artifact}')"),  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            lambda value: value == "1",
        )
        command(http, live["batchId"], "cancel", key=str(uuid.uuid4()))
        report(
            suite,
            "E10-V03-S03-retention",
            expired=expired,
            protected=live,
            export=job,
            firstRetention=first,
            secondRetention=second,
            scrubbedRows=scrubbed,
        )
