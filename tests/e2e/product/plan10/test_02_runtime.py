"""Actual durable dispatch, crash rollback, multiple replicas and original task cancellation."""

import time
import uuid

import pytest

from tests.e2e.product.plan10.support import (
    client,
    command,
    coordinator_paused,
    crash_coordinator,
    create,
    get,
    hexid,
    items,
    report,
    scene,
    sql,
    wait,
)
from tests.e2e.support import run

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.product,
    pytest.mark.runtime,
    pytest.mark.gateway,
    pytest.mark.live_model,
]


def test_closed_client_and_coordinator_restart_preserve_complete_inputs(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    rows = [{**case["inputs"][0], "request_id": f"restart-{i}"} for i in range(3)]
    with coordinator_paused(suite):
        with client(suite, case) as first:
            batch = create(first, case, rows)
        assert (
            sql(
                suite,
                f"SELECT COUNT(*) FROM application_batch_items WHERE batch_id=UNHEX('{hexid(batch['batchId'])}') AND invocation_id IS NULL",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            )
            == "3"
        )
    with client(suite, case) as http:
        restored = wait(lambda: get(http, batch["batchId"]), lambda b: b["counts"]["waitingInput"] == 3)
        actual = items(http, batch["batchId"])["items"]
        assert [item["input"] for item in actual] == rows
        assert len({item["task"]["taskId"] for item in actual}) == 3
        command(http, batch["batchId"], "cancel")
        wait(lambda: get(http, batch["batchId"]), lambda b: b["counts"]["cancelled"] == 3)
        report(suite, "E10-R01", accepted=batch, restored=restored, rows=actual)


def test_crash_between_invocation_insert_and_item_link_is_atomic(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    namespace = suite["installed"]["runtime_namespace"]
    with coordinator_paused(suite), client(suite, case) as http:
        batch = create(http, case, [case["inputs"][0]] * 3)
        target = hexid(batch["batchId"])
        sql(
            suite,
            f"CREATE TRIGGER plan10_admission_crash BEFORE UPDATE ON application_batch_items FOR EACH ROW SET @plan10_crash=IF(NEW.batch_id=UNHEX('{target}') AND OLD.dispatch_state='pending' AND NEW.dispatch_state='accepted' AND OLD.invocation_id IS NULL AND NEW.invocation_id IS NOT NULL,SLEEP(15),0)",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
    try:
        wait(
            lambda: sql(
                suite,
                "SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE DB=DATABASE() AND STATE='User sleep'",
            ),
            lambda n: int(n) > 0,
            120,
        )
        # MySQL reports the trigger's SET statement, rather than the outer
        # prepared UPDATE, while sleeping. This database has no other SLEEP.
        fault = sql(
            suite, "SELECT ID,STATE,INFO FROM information_schema.PROCESSLIST WHERE DB=DATABASE() AND STATE='User sleep'"
        )
        assert fault, "Capture the live injected transition before killing its coordinator"
        report(suite, "E10-R02-crash-point", passed=False, phase="fault_observed", faultProcess=fault)
        crash = crash_coordinator(suite)
    finally:
        sql(suite, "DROP TRIGGER IF EXISTS plan10_admission_crash")
    run(("kubectl", "-n", namespace, "rollout", "status", "deployment/workflow-runtime", "--timeout=180s"), timeout=200)
    with client(suite, case) as http:
        final = wait(lambda: get(http, batch["batchId"]), lambda b: b["counts"]["waitingInput"] == 3, 240)
        links = sql(
            suite,
            f"SELECT COUNT(*),COUNT(DISTINCT invocation_id) FROM application_batch_items WHERE batch_id=UNHEX('{target}')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
        assert links == "3\t3", links
        intents = sql(
            suite,
            f"SELECT COUNT(*) FROM application_invocations WHERE idempotency_key IN (SELECT CONCAT('batch:',BIN_TO_UUID(id)) FROM application_batch_items WHERE batch_id=UNHEX('{target}'))",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
        assert intents == "3", intents
        # Replay the same durable delivery after completion of dispatch.
        sql(
            suite,
            f"UPDATE runtime_commands SET status='pending',available_at=UTC_TIMESTAMP(6),locked_by=NULL,locked_until=NULL WHERE command_type='dispatch_batch' AND aggregate_id='{batch['batchId']}'",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
        )
        time.sleep(2)
        assert (
            sql(
                suite,
                f"SELECT COUNT(*) FROM application_invocations WHERE idempotency_key IN (SELECT CONCAT('batch:',BIN_TO_UUID(id)) FROM application_batch_items WHERE batch_id=UNHEX('{target}'))",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            )
            == "3"
        )
        command(http, batch["batchId"], "cancel")
        wait(lambda: get(http, batch["batchId"]), lambda b: b["counts"]["cancelled"] == 3)
        report(suite, "E10-R02-crash", faultProcess=fault, crash=crash, final=final, links=links, invocations=intents)


def test_parallel_waits_do_not_block_other_rows_or_other_batches(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    namespace = suite["installed"]["runtime_namespace"]
    initial = (
        run(("kubectl", "-n", namespace, "get", "deployment/workflow-runtime", "-o", "json"))
        .json()["spec"]
        .get("replicas", 1)
    )
    run(("kubectl", "-n", namespace, "scale", "deployment/workflow-runtime", "--replicas=2"))
    try:
        with client(suite, case) as http:
            batches = [
                create(http, case, [{**case["inputs"][0], "request_id": f"parallel-{n}-{i}"} for i in range(8)])
                for n in range(2)
            ]
            snapshots = []

            def read():
                current = [get(http, batch["batchId"]) for batch in batches]
                snapshots.extend(current)
                for batch in batches:
                    projects = items(http, batch["batchId"])["items"]
                    assert sum(bool(i["task"] and i["task"]["state"] in ("accepted", "running")) for i in projects) <= 4
                return current

            final = wait(read, lambda values: all(b["counts"]["waitingInput"] == 8 for b in values), 240)
            for batch in batches:
                projects = items(http, batch["batchId"])["items"]
                assert len({item["task"]["taskId"] for item in projects}) == 8
                assert all(item["task"]["sessionId"] is None for item in projects)
                command(http, batch["batchId"], "cancel")
                wait(lambda batch=batch: get(http, batch["batchId"]), lambda b: b["counts"]["cancelled"] == 8)
            report(suite, "E10-R02-R03-parallel", snapshots=snapshots, final=final)
    finally:
        run(("kubectl", "-n", namespace, "scale", "deployment/workflow-runtime", f"--replicas={initial}"))
        run(
            ("kubectl", "-n", namespace, "rollout", "status", "deployment/workflow-runtime", "--timeout=180s"),
            timeout=200,
        )


def test_stop_and_cancel_have_distinct_race_safe_effects(plan10_suite):
    suite = plan10_suite
    case = scene(suite, "expense")
    with client(suite, case) as http:
        batch = create(http, case, [case["inputs"][0]] * 40)
        stopped = command(http, batch["batchId"], "stop")
        original = items(http, batch["batchId"])["items"]
        accepted = {item["task"]["taskId"] for item in original if item["task"]}
        time.sleep(2)
        current = items(http, batch["batchId"])["items"]
        assert {item["task"]["taskId"] for item in current if item["task"]} == accepted
        assert all(item["dispatchState"] != "pending" for item in current)
        assert stopped["counts"]["cancelled"] == 40 - len(accepted)
        assert (
            sql(
                suite,
                f"SELECT COUNT(*) FROM application_invocations v JOIN application_batch_items i ON i.invocation_id=v.id JOIN application_batches b ON b.id=i.batch_id WHERE b.id=UNHEX('{hexid(batch['batchId'])}') AND v.created_at>b.updated_at",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
            )
            == "0"
        )
        key = str(uuid.uuid4())
        command(http, batch["batchId"], "cancel", key=key)
        command(http, batch["batchId"], "cancel", key=key)
        final = wait(lambda: get(http, batch["batchId"]), lambda b: b["counts"]["ended"] == 40)
        assert final["counts"]["cancelled"] == 40
        report(suite, "E10-R04-R05", stopped=stopped, final=final, acceptedBeforeStop=list(accepted))


def test_simultaneous_human_resumes_keep_four_project_limit(plan10_suite):
    from concurrent.futures import ThreadPoolExecutor

    from tests.e2e.product.live_text_support import Secret, post
    from tests.e2e.product.plan9.scenarios import reference
    from tests.e2e.product.plan9.workflow_builders import code, object_schema, publish
    from tests.e2e.product.plan9.workflow_builders import wait as wait_node

    suite = plan10_suite
    schema = object_schema({"value": {"type": "integer"}})
    notes = object_schema({"note": {"type": "string"}})
    worker = code(
        "work",
        "人工补充后的正式计算",
        suite["profile"],
        {"value": reference("inputs", ["value"])},
        "def main(**inputs):\n    import time\n    time.sleep(1)\n    return {'value': inputs['value']}\n",
        {"value": 0},
    )
    case = publish(
        suite["control"],
        suite["runId"],
        "R10",
        "并行等待恢复",
        schema,
        {"value": {"type": "integer"}},
        [wait_node("notes", "补充说明", notes), worker],
        {"value": reference("outputs", ["structuredOutput", "value"], "work")},
        [{"value": 1}],
        [{"value": 1}],
    )
    key = post(
        suite["control"], f"/applications/{case['applicationId']}/api-keys", {"name": "Simultaneous resume acceptance"}
    )
    case["secret"] = Secret(key["secret"])
    with client(suite, case) as http:
        wait(
            lambda: http.get(f"/gateway/v1/applications/{case['slug']}/batches").status_code, lambda value: value == 200
        )
        batch = create(http, case, [{"value": value} for value in range(1, 9)])
        wait(lambda: get(http, batch["batchId"]), lambda value: value["counts"]["waitingInput"] == 8)
        projects = items(http, batch["batchId"])["items"]

        def resume(project):
            task = project["task"]
            response = http.post(
                f"/gateway/v1/invocations/{task['taskId']}/waits/{task['pendingInputs'][0]['waitId']}/input",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={"content": {"note": "resume together"}},
            )
            assert response.status_code == 202, response.text

        with ThreadPoolExecutor(max_workers=8) as pool:
            list(pool.map(resume, projects))
        observed = []

        def read():
            current = items(http, batch["batchId"])["items"]
            active = sum(item["state"] in ("accepted", "running") for item in current)
            queued = sum(item["state"] == "queued" for item in current)
            assert active <= 4, current
            physical = int(
                sql(
                    suite,
                    f"SELECT COUNT(DISTINCT i.id) FROM application_batch_items i JOIN application_invocations v ON v.id=i.invocation_id JOIN node_attempts a ON a.execution_id=v.execution_id WHERE i.batch_id=UNHEX('{hexid(batch['batchId'])}') AND a.status IN ('queued','running')",  # noqa: S608 — normalized fixture IDs in a temporary namespace.
                )
            )
            assert physical <= 4
            observed.append({"active": active, "queued": queued, "physicalProjects": physical})
            return current

        final = wait(read, lambda current: all(item["state"] in ("completed", "failed") for item in current), 420)
        assert all(item["state"] == "completed" for item in final), final
        assert [item["task"]["outputs"]["value"] for item in final] == list(range(1, 9))
        assert any(snapshot["queued"] > 0 for snapshot in observed)
        report(suite, "E10-R03-simultaneous-resume", batch=batch, observed=observed, final=final)
