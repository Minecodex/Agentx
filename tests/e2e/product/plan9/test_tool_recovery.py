"""A real business Agent resumes the same approved Effect on a replacement Worker."""

import copy
import json
import time
import uuid

import pytest

from tests.e2e.product.plan9.mcp_support import TERMINAL, application_client, check_result, crm_records, decide
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def effect(suite, task_id, effect_id=None):
    identifier = uuid.UUID(task_id).hex
    selected = f" AND f.id=UNHEX('{uuid.UUID(effect_id).hex}')" if effect_id else ""
    rows = _runtime_mysql(
        suite["installed"],
        f"SELECT HEX(CAST(JSON_OBJECT('effectId',LOWER(HEX(f.id)),'runId',LOWER(HEX(f.agent_run_id)),'attemptId',LOWER(HEX(f.attempt_id)),'nodeExecutionId',LOWER(HEX(f.node_execution_id)),'operationId',f.operation_id,'toolCallId',f.tool_call_id,'argumentsHash',f.arguments_hash,'approvalId',LOWER(HEX(f.approval_task_id)),'resumeCommandId',LOWER(HEX(f.resume_command_id)),'settled',f.result_json IS NOT NULL,'unknown',f.outcome_unknown) AS BINARY)) FROM agent_tool_effects f JOIN application_invocations i ON i.tenant_id=f.tenant_id AND i.execution_id=f.execution_id WHERE i.id=UNHEX('{identifier}'){selected}",  # noqa: S608 -- identifiers have been parsed as UUIDs.
    ).splitlines()
    assert len(rows) == 1, rows
    return json.loads(bytes.fromhex(rows[0]))


def test_tool_approval_restarts_worker_without_replaying_effect(plan9_suite):
    suite = plan9_suite
    case = copy.deepcopy(next(value for value in suite["cases"] if value["scene"] == "S17"))
    case["inputs"][0]["request_id"] += "-worker-recovery"
    case["expected"][0]["reference"] = case["inputs"][0]["request_id"]
    namespace = suite["installed"]["runtime_namespace"]
    assert namespace.startswith("agentx-e2e-runtime-") and namespace.endswith(suite["runId"])
    resource = ("kubectl", "-n", namespace)
    original = int(run((*resource, "get", "deployment/workflow-worker", "-o", "jsonpath={.spec.replicas}")).stdout)
    assert original > 0
    report = {"passed": False}
    try:
        with application_client(suite, case, "Worker replacement") as (client, published, _):
            reference = case["inputs"][0]["request_id"]
            before = len(crm_records(suite, reference))
            task = client.submit(published["toolName"], published["revision"], case["inputs"][0])
            deadline = time.monotonic() + 300
            while True:
                current = client.task(task["taskId"])
                if current["pendingApprovals"]:
                    break
                assert current["state"] not in TERMINAL, current
                assert time.monotonic() < deadline, current
                time.sleep(0.5)
            before_effect = effect(suite, task["taskId"])
            assert not before_effect["settled"] and not before_effect["unknown"]
            assert len(crm_records(suite, reference)) == before
            run((*resource, "scale", "deployment/workflow-worker", "--replicas=0"))
            run((*resource, "rollout", "status", "deployment/workflow-worker", "--timeout=120s"), timeout=140)
            run(
                (
                    *resource,
                    "wait",
                    "--for=delete",
                    "pod",
                    "-l",
                    "app.kubernetes.io/name=workflow-worker",
                    "--timeout=120s",
                ),
                timeout=140,
            )
            human = decide(
                suite, case, current["pendingApprovals"][0]["approvalId"], case["approvalDecisions"][0][0], "recovery"
            )
            deadline = time.monotonic() + 60
            while True:
                queued = effect(suite, task["taskId"])
                if queued["resumeCommandId"]:
                    break
                assert time.monotonic() < deadline, queued
                time.sleep(0.2)
            assert len(crm_records(suite, reference)) == before, (
                "An approval decision cannot execute a tool without its Worker"
            )
            run((*resource, "scale", "deployment/workflow-worker", f"--replicas={original}"))
            run((*resource, "rollout", "status", "deployment/workflow-worker", "--timeout=180s"), timeout=200)
            deadline = time.monotonic() + 300
            while (finished := client.task(task["taskId"]))["state"] not in TERMINAL:
                assert time.monotonic() < deadline, finished
                time.sleep(0.5)
            assert finished["state"] == "completed", finished
            proof = check_result(suite, client, case, 0, finished, before)
            after_effect = effect(suite, task["taskId"])
            for key in (
                "effectId",
                "runId",
                "attemptId",
                "nodeExecutionId",
                "operationId",
                "toolCallId",
                "argumentsHash",
                "approvalId",
            ):
                assert before_effect[key] == after_effect[key], key
            assert after_effect["settled"] and not after_effect["unknown"]
            assert after_effect["resumeCommandId"] == queued["resumeCommandId"]
            checkpoints = int(
                _runtime_mysql(
                    suite["installed"],
                    f"SELECT COUNT(*) FROM checkpoints WHERE execution_id=UNHEX('{uuid.UUID(proof['executionId']).hex}') AND checkpoint_type='agent_effect_resumed'",  # noqa: S608 -- validated UUID only.
                )
            )
            assert checkpoints == 1, "The original Effect must persist one durable resume checkpoint"
            report.update(
                passed=True,
                beforeEffect=before_effect,
                queuedEffect=queued,
                afterEffect=after_effect,
                humanDecision=human,
                task=finished,
                runtimeProof=proof,
                resumeCheckpoints=checkpoints,
            )
    finally:
        run((*resource, "scale", "deployment/workflow-worker", f"--replicas={original}"))
        run((*resource, "rollout", "status", "deployment/workflow-worker", "--timeout=180s"), timeout=200)
        (suite["directory"] / "tool-worker-recovery.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
