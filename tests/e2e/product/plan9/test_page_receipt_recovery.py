"""A receipt-query outage preserves the original authoring Effect without dispatching it twice."""

import copy
import json
import threading
import time
import uuid
from pathlib import Path

import pytest

from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.plan9.test_real_pages import assemble_page_results
from tests.e2e.runtime.test_agent_attachments import _control_mysql, _runtime_mysql
from tests.e2e.support import run, run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_page_authoring_waits_for_receipt_service_without_replaying_the_model(plan9_suite):
    suite, installed = plan9_suite, plan9_suite["installed"]
    scenario = copy.deepcopy(next(row for row in suite["cases"] if row["scene"] == "S01"))
    scenario["scene"] = "S01-receipt-recovery"
    scenario["slug"] += "-receipt-recovery"
    directory = suite["directory"] / "receipt-recovery"
    directory.mkdir(exist_ok=True)
    inputs, receipt = directory / "scenario.json", directory / "fault.json"
    report = suite["directory"] / "receipt-recovery-page-results.json"
    inputs.write_text(json.dumps([scenario], ensure_ascii=False, indent=2))
    application = uuid.UUID(scenario["applicationId"]).hex
    namespace = installed["runtime_namespace"]
    assert namespace == f"agentx-e2e-runtime-{suite['runId']}"
    command = ("kubectl", "-n", namespace)
    replicas = int(run((*command, "get", "deployment/runtime-gateway", "-o", "jsonpath={.spec.replicas}")).stdout)
    assert replicas > 0
    existing = set(_control_mysql(installed, "SELECT LOWER(HEX(id)) FROM design_agent_runs").splitlines())
    stop, errors, proof = threading.Event(), [], {"passed": False}
    down = False

    def restore():
        nonlocal down
        if down:
            run((*command, "scale", "deployment/runtime-gateway", f"--replicas={replicas}"))
            run((*command, "rollout", "status", "deployment/runtime-gateway", "--timeout=180s"), timeout=200)
            down = False

    def fault():
        nonlocal down
        try:
            while not stop.wait(0.2):
                rows = _control_mysql(
                    installed,
                    f"SELECT LOWER(HEX(r.id)) FROM design_agent_runs r JOIN design_agent_sessions s ON s.id=r.session_id WHERE s.application_id=UNHEX('{application}') AND r.status='waiting' AND EXISTS(SELECT 1 FROM design_agent_effects e WHERE e.run_id=r.id AND e.tool_name='ask_user' AND e.status='pending')",  # noqa: S608 -- parsed UUID in the isolated database.
                )
                fresh = [row for row in rows.splitlines() if row not in existing]
                if not fresh:
                    continue
                assert len(fresh) == 1, fresh
                identifier, run_id = fresh[0], str(uuid.UUID(hex=fresh[0]))
                count_query = f"SELECT COUNT(*) FROM runtime_calls WHERE JSON_UNQUOTE(JSON_EXTRACT(design_target_json,'$.target.runId'))='{run_id}'"  # noqa: S608 -- parsed UUID.
                before = int(_runtime_mysql(installed, count_query))
                assert before > 0, "The original question must come from the real model"
                before_intents = int(
                    _control_mysql(
                        installed,
                        f"SELECT COUNT(*) FROM design_agent_entries WHERE run_id=UNHEX('{identifier}') AND JSON_UNQUOTE(JSON_EXTRACT(payload_json,'$.type'))='model_intent'",  # noqa: S608 -- parsed UUID.
                    )
                )
                down = True
                run((*command, "scale", "deployment/runtime-gateway", "--replicas=0"))
                run(
                    (
                        *command,
                        "wait",
                        "--for=delete",
                        "pod",
                        "-l",
                        "app.kubernetes.io/name=runtime-gateway",
                        "--timeout=120s",
                    ),
                    timeout=140,
                )
                proof.update(runId=run_id, modelCallsBefore=before, gatewayReplicasDuringFault=0)
                temporary = receipt.with_suffix(".tmp")
                temporary.write_text(json.dumps(proof, indent=2))
                temporary.replace(receipt)
                deadline = time.monotonic() + 90
                while True:
                    intents = int(
                        _control_mysql(
                            installed,
                            f"SELECT COUNT(*) FROM design_agent_entries WHERE run_id=UNHEX('{identifier}') AND JSON_UNQUOTE(JSON_EXTRACT(payload_json,'$.type'))='model_intent'",  # noqa: S608 -- parsed UUID.
                        )
                    )
                    if intents > before_intents:
                        break
                    assert time.monotonic() < deadline, "The author's answer did not resume its original Run"
                    time.sleep(0.2)
                observations = []
                until = time.monotonic() + 5
                while time.monotonic() < until:
                    status = _control_mysql(
                        installed,
                        f"SELECT status FROM design_agent_runs WHERE id=UNHEX('{identifier}')",  # noqa: S608 -- parsed UUID.
                    )
                    calls = int(_runtime_mysql(installed, count_query))
                    assert status in {"waiting", "running"}, (status, calls)
                    assert calls == before, "An unavailable receipt query must not create a provider call"
                    observations.append({"status": status, "runtimeCalls": calls})
                    time.sleep(0.2)
                restore()
                proof.update(whileUnavailable=observations, gatewayReplicasRestored=replicas)
                return
        except Exception as error:
            errors.append(f"{type(error).__name__}: {error}")
        finally:
            restore()

    worker = threading.Thread(target=fault, daemon=True, name="plan9-model-receipt-outage")
    worker.start()
    try:
        run_playwright(
            Path(installed["root"]),
            "plan9-page-receipt-recovery",
            ("tests/plan9-real-pages.spec.ts", "--workers=1"),
            {
                **browser_environment(suite),
                "AGENTX_E2E_PLAN9_SCENARIOS": str(inputs),
                "AGENTX_E2E_PLAN9_REPORT": str(report),
                "AGENTX_E2E_PLAN9_ROUNDS": "1",
                "AGENTX_E2E_PLAN9_HOST_RECOVERY_RECEIPT": str(receipt),
            },
        )
        worker.join(timeout=240)
        assert not worker.is_alive() and not errors, errors
        results = assemble_page_results(report)
        assert len(results) == 1 and results[0]["passed"] and len(results[0]["attempts"]) == 1, results
        original = results[0]["attempts"][0]
        assert original["run"]["id"] == proof["runId"] and original["run"]["status"] == "completed"
        identifier = str(uuid.UUID(proof["runId"]))
        counts = _runtime_mysql(
            installed,
            f"SELECT COUNT(*),COUNT(DISTINCT idempotency_key),SUM(status='succeeded') FROM runtime_calls WHERE JSON_UNQUOTE(JSON_EXTRACT(design_target_json,'$.target.runId'))='{identifier}'",  # noqa: S608 -- parsed UUID.
        )
        total, unique, succeeded = map(int, counts.split())
        usage = {
            entry["payload"]["effectId"]
            for entry in original["entries"]
            if entry["runId"] == identifier and entry["payload"]["type"] == "usage"
        }
        assert total == unique == succeeded == len(usage) and total > proof["modelCallsBefore"], counts
        proof.update(passed=True, runtimeCalls=total, uniqueProviderIdentities=unique, usageEntries=len(usage))
    finally:
        stop.set()
        worker.join(timeout=240)
        restore()
        assemble_page_results(report)
        proof["errors"] = errors
        (directory / "proof.json").write_text(json.dumps(proof, ensure_ascii=False, indent=2))
