"""Exercise real AI page versions and a pending task with Control offline and a replacement Worker."""

import copy
import json
import threading
import uuid
from pathlib import Path

import pytest

from tests.e2e.product.next_batch_support import get
from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.test_live_memory_pipeline import _save_version
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql
from tests.e2e.support import run, run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_generated_page_rollback_and_control_independent_recovery(plan9_suite):
    suite = plan9_suite
    case = next(row for row in suite["cases"] if row["scene"] == "S08")
    directory = suite["directory"] / "page-lifecycle"
    directory.mkdir(exist_ok=True)
    checkpoint, receipt = directory / "checkpoint.json", directory / "fault.json"
    installed = suite["installed"]
    control, runtime = installed["control_namespace"], installed["runtime_namespace"]
    assert control == f"agentx-e2e-control-{suite['runId']}"
    assert runtime == f"agentx-e2e-runtime-{suite['runId']}"
    replicas = int(
        run(("kubectl", "-n", control, "get", "deployment/platform-control", "-o", "jsonpath={.spec.replicas}")).stdout
    )
    stopped, events, errors = threading.Event(), [], []

    def inject():
        upgraded = False
        try:
            while not stopped.wait(0.2):
                if not checkpoint.exists():
                    continue
                state = json.loads(checkpoint.read_text())
                assert state["taskId"]
                if state["phase"] == "waiting_for_upgrade":
                    if upgraded:
                        continue
                    definition = copy.deepcopy(
                        get(suite["control"], f"/workflows/{case['workflowId']}/versions/{case['versionId']}")[
                            "definition"
                        ]
                    )
                    definition["start"]["inputs"]["properties"]["upgrade_confirmation"] = {
                        "type": "string",
                        "const": "new_contract",
                    }
                    definition["start"]["inputs"]["required"].append("upgrade_confirmation")
                    definition["end"]["outputs"]["business_revision"] = {
                        "schema": {"type": "integer"},
                        "required": True,
                        "sensitive": False,
                    }
                    for node in definition["nodes"]:
                        if node["type"] == "exit":
                            node["parameters"]["outputs"]["business_revision"] = {"kind": "literal", "value": 2}
                    version = _save_version(suite["control"], case["workflowId"], definition)
                    assert version.get("id") and version["id"] != case["versionId"], version
                    environment = next(
                        row for row in get(suite["control"], "/environments") if row["code"] == "development"
                    )
                    deployment = _publish_application_deployment(
                        suite["control"],
                        dict(suite["control"].headers),
                        case["applicationId"],
                        version["id"],
                        environment["id"],
                    )

                    identifier = uuid.UUID(case["applicationId"]).hex
                    bundle = _runtime_mysql(
                        installed,
                        f"SELECT LOWER(HEX(active_bundle_id)) FROM application_routes WHERE application_id=UNHEX('{identifier}')",  # noqa: S608 -- parsed UUID only.
                    )
                    upgrade = {
                        "workflowVersionId": version["id"],
                        "deploymentId": deployment["id"],
                        "activeBundleId": str(uuid.UUID(hex=bundle)),
                        "inputSchema": definition["start"]["inputs"],
                    }
                    temporary = directory / "upgrade.tmp"
                    temporary.write_text(json.dumps(upgrade, ensure_ascii=False, indent=2))
                    temporary.replace(directory / "upgrade.json")
                    upgraded = True
                    continue
                assert state["phase"] == "waiting_for_fault" and upgraded
                selector = "app.kubernetes.io/name=workflow-worker"
                before = run(("kubectl", "-n", runtime, "get", "pods", "-l", selector, "-o", "json")).json()
                run(("kubectl", "-n", control, "scale", "deployment/platform-control", "--replicas=0"))
                run(
                    (
                        "kubectl",
                        "-n",
                        control,
                        "wait",
                        "--for=delete",
                        "pod",
                        "-l",
                        "app.kubernetes.io/name=platform-control",
                        "--timeout=120s",
                    ),
                    timeout=140,
                )
                run(("kubectl", "-n", runtime, "rollout", "restart", "deployment/workflow-worker"))
                run(
                    ("kubectl", "-n", runtime, "rollout", "status", "deployment/workflow-worker", "--timeout=180s"),
                    timeout=200,
                )
                for pod in before["items"]:
                    run(
                        (
                            "kubectl",
                            "-n",
                            runtime,
                            "wait",
                            "--for=delete",
                            f"pod/{pod['metadata']['name']}",
                            "--timeout=120s",
                        ),
                        timeout=140,
                    )
                after = run(("kubectl", "-n", runtime, "get", "pods", "-l", selector, "-o", "json")).json()
                event = {
                    "taskId": state["taskId"],
                    "controlReplicas": 0,
                    "workerBefore": [row["metadata"]["uid"] for row in before["items"]],
                    "workerAfter": [row["metadata"]["uid"] for row in after["items"]],
                }
                assert set(event["workerBefore"]).isdisjoint(event["workerAfter"])
                events.append(event)
                temporary = receipt.with_suffix(".tmp")
                temporary.write_text(json.dumps(event))
                temporary.replace(receipt)
                return
        except Exception as error:
            errors.append(str(error))

    worker = threading.Thread(target=inject, daemon=True, name="plan9-page-lifecycle-fault")
    worker.start()
    try:
        run_playwright(
            Path(installed["root"]),
            "plan9-page-lifecycle",
            ("tests/plan9-page-lifecycle.spec.ts",),
            {
                **browser_environment(suite),
                "AGENTX_E2E_PLAN9_LIFECYCLE_SCENARIO": json.dumps(case, ensure_ascii=False),
                "AGENTX_E2E_PLAN9_LIFECYCLE_DIRECTORY": str(directory),
            },
        )
        assert events and not errors, errors
    finally:
        stopped.set()
        worker.join(timeout=360)
        run(("kubectl", "-n", control, "scale", "deployment/platform-control", f"--replicas={replicas}"))
        run(
            ("kubectl", "-n", control, "rollout", "status", "deployment/platform-control", "--timeout=180s"),
            timeout=200,
        )
        (directory / "fault-summary.json").write_text(
            json.dumps({"events": events, "errors": errors, "restoredControlReplicas": replicas}, indent=2)
        )
        assert not worker.is_alive() and not errors, errors
