"""Explicit opt-in verification of a retained local application; never installs a stack."""

from __future__ import annotations

import json
import os
import shutil
import ssl
import time
import uuid
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.plan9.mcp_client import ApplicationMcpClient
from tests.e2e.product.plan9.opencode_client import isolated_opencode
from tests.e2e.support import ROOT, run, run_playwright

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.infrastructure,
    pytest.mark.skipif(
        not os.environ.get("AGENTX_LOCAL_APPLICATION_DELIVERY_CONFIG"),
        reason="Requires an explicit retained local delivery and authorized reviewer credentials",
    ),
]


def load():
    config = json.loads(Path(os.environ["AGENTX_LOCAL_APPLICATION_DELIVERY_CONFIG"]).read_text())
    assert config["baseUrl"] == "https://agentx.localhost"
    assert config["runtimeUrl"] == "https://run.agentx.localhost"
    directory = Path(config["evidenceDir"])
    case = json.loads((directory / "scenario.json").read_text())
    delivery = json.loads((directory / "mcp-delivery.json").read_text())
    return config, directory, case, delivery


def write(directory, name, value):
    (directory / name).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def image_contents(digest):
    def content(value):
        return run(
            ("docker", "exec", "desktop-control-plane", "ctr", "--namespace", "k8s.io", "content", "get", value)
        ).json()

    document = content(digest)
    # CRI may report an equivalent OCI index with different build attestations.
    # Its runnable manifest, config and source label must still match exactly.
    if "manifests" in document:
        manifests = [row for row in document["manifests"] if row.get("platform", {}).get("os") == "linux"]
        assert len(manifests) == 1
        digest = manifests[0]["digest"]
        document = content(digest)
    config = content(document["config"]["digest"])
    return {
        "manifest": digest,
        "config": document["config"]["digest"],
        "sourceTreeSha256": config["config"]["Labels"]["io.agentx.source-tree-sha256"],
    }


def test_local_delivery_uses_current_images_and_retains_existing_data():
    _, directory, _, _ = load()
    release = json.loads((directory / "release.json").read_text())
    cutover = json.loads((directory / "cutover-complete.json").read_text())
    assert cutover["sourceDatabasesUnchanged"] and cutover["allRetainedRowHashesVerified"]
    assert cutover["allFourDoctorsPassed"]
    rows = json.loads((directory / "retained-data-verification.json").read_text())
    assert len(rows["control"]) >= 100 and len(rows["runtime"]) >= 100
    assert all(row["verified"] for plane in rows.values() for row in plane.values())
    result = []
    for namespace, names in {
        "agentx-prod-control": ["platform-control", "web-console"],
        "agentx-prod-runtime": [
            "runtime-gateway",
            "workflow-runtime",
            "workflow-worker",
            "sandbox-manager",
            "observability",
        ],
        "agentx-prod-deps": ["agentx-egress-gateway"],
    }.items():
        for name in names:
            deployed = run(("kubectl", "-n", namespace, "get", "deployment", name, "-o", "json")).json()
            assert deployed["status"]["readyReplicas"] == deployed["spec"]["replicas"] >= 1
            image = deployed["spec"]["template"]["spec"]["containers"][0]["image"]
            assert image.endswith("@" + release["digests"][name])
            selector = ",".join(f"{k}={v}" for k, v in deployed["spec"]["selector"]["matchLabels"].items())
            pods = run(("kubectl", "-n", namespace, "get", "pods", "-l", selector, "-o", "json")).json()["items"]
            running = [
                pod
                for pod in pods
                if pod["status"]["phase"] == "Running" and not pod["metadata"].get("deletionTimestamp")
            ]
            assert len(running) == deployed["spec"]["replicas"]
            for pod in running:
                assert pod["status"]["containerStatuses"][0]["ready"]
                expected = image_contents(release["digests"][name])
                actual = image_contents(pod["status"]["containerStatuses"][0]["imageID"].split("@")[-1])
                assert actual == expected
                assert actual["sourceTreeSha256"] == release["sourceTreeSha256"]
            result.append({"namespace": namespace, "deployment": name, "image": image, "ready": True})
    write(directory, "live-workload-verification.json", {"passed": True, "workloads": result})


def test_local_mcp_runs_original_task_through_input_and_real_ui_approval():
    config, directory, _, delivery = load()
    context = ssl.create_default_context(cafile=config["caFile"])
    private_config = json.loads(Path(delivery["serviceConfiguration"]).read_text())
    credential = private_config["mcp"]["expense"]["headers"]["Authorization"].removeprefix("Bearer ")
    report = {"passed": False, "phase": "starting", "records": []}
    try:
        with httpx.Client(base_url=config["runtimeUrl"], verify=context, timeout=60, trust_env=False) as transport:
            client = ApplicationMcpClient(transport, "/gateway/v1/mcp/applications/expense-demo", credential)
            client.initialize()
            client.tools(delivery["toolName"])
            inputs = {
                "request_id": "BX-MCP-" + str(time.time_ns()),
                "applicant": "MCP 体验用户",
                "expense_type": "客户招待",
                "amount": 358.75,
            }
            supplement = {"purpose": "本地 MCP 联调使用的虚构业务招待费", "invoice_no": "FP-MCP-DEMO-001"}
            request_id = str(uuid.uuid4())
            task = client.submit(delivery["toolName"], delivery["revision"], inputs, request_id=request_id)
            task_id = task["taskId"]
            report.update(taskId=task_id, input=inputs, supplement=supplement, records=client.records, task=task)
            replay = client.submit(delivery["toolName"], delivery["revision"], inputs, request_id=request_id)
            assert replay["taskId"] == task_id
            deadline = time.monotonic() + 60
            while not task["pendingInputs"]:
                assert task["state"] not in {"completed", "failed", "cancelled", "outcome_unknown"}, task
                assert time.monotonic() < deadline, task
                time.sleep(0.5)
                task = client.task(task_id)
            assert task["state"] == "waiting_input"
            wait = task["pendingInputs"][0]
            assert wait["nodePath"] == "/details"
            client.input(task_id, wait["waitId"], supplement)
            deadline = time.monotonic() + 60
            while not task["pendingApprovals"]:
                assert task["state"] not in {"completed", "failed", "cancelled", "outcome_unknown"}, task
                assert time.monotonic() < deadline, task
                time.sleep(0.5)
                task = client.task(task_id)
            assert task["state"] == "waiting_approval" and task["outputs"] is None
            report.update(
                phase="waiting_for_ui_approval",
                taskId=task_id,
                input=inputs,
                supplement=supplement,
                approvalId=task["pendingApprovals"][0]["approvalId"],
                waitId=wait["waitId"],
                originalInvocationReused=True,
                task=task,
            )
            write(directory, "mcp-business-test.json", report)
            # Approval is performed by a separate browser through the real
            # approval center. The MCP client has no approval capability.
            run_playwright(
                ROOT,
                "local-mcp-approval",
                ("tests/local-application-delivery.spec.ts", "--grep", "pending MCP task"),
                {
                    **os.environ,
                    "AGENTX_E2E_BASE_URL": config["baseUrl"],
                    "AGENTX_E2E_STAGE": "local-delivery",
                    "AGENTX_E2E_RUN_ID": config["runId"],
                    "NODE_EXTRA_CA_CERTS": config["caFile"],
                },
            )
            deadline = time.monotonic() + 600
            while task["state"] == "waiting_approval":
                assert time.monotonic() < deadline, "Approve the task in the local approval center"
                time.sleep(2)
                task = client.task(task_id)
            while task["state"] not in {"completed", "failed", "cancelled", "outcome_unknown"}:
                assert time.monotonic() < deadline, task
                time.sleep(0.5)
                task = client.task(task_id)
            assert task["taskId"] == task_id and task["state"] == "completed", task
            assert task["outputs"] == {**inputs, **supplement, "decision": "approved"}
            assert not task["pendingInputs"] and not task["pendingApprovals"]
            report.update(passed=True, phase="completed", task=task, records=client.records)
    finally:
        write(directory, "mcp-business-test.json", report)


def test_published_local_page_opens_in_an_independent_browser():
    config, directory, _, _ = load()
    run_playwright(
        ROOT,
        "local-page-ui",
        ("tests/local-application-delivery.spec.ts", "--grep", "published local application"),
        {
            **os.environ,
            "AGENTX_E2E_BASE_URL": config["baseUrl"],
            "AGENTX_E2E_STAGE": "local-delivery",
            "AGENTX_E2E_RUN_ID": config["runId"],
            "NODE_EXTRA_CA_CERTS": config["caFile"],
        },
    )
    output = ROOT / ".local/artifacts/playwright/local-delivery" / config["runId"] / "local-page-ui"
    pictures = list(output.rglob("expense-demo.jpg"))
    assert len(pictures) == 1
    shutil.copyfile(pictures[0], directory / "expense-demo.jpg")
    for filename in ("expense-workflow.jpg", "expense-mcp.jpg", "expense-approved.jpg", "expense-rejected.jpg"):
        matches = list(output.rglob(filename))
        assert len(matches) == 1
        shutil.copyfile(matches[0], directory / filename)


def test_installed_opencode_discovers_the_published_local_application():
    config, directory, _, delivery = load()
    private_config = json.loads(Path(delivery["serviceConfiguration"]).read_text())
    credential = private_config["mcp"]["expense"]["headers"]["Authorization"].removeprefix("Bearer ")
    with isolated_opencode(
        "https://api.apimart.ai/v1",
        "unused-no-model-request",
        ca=config["caFile"],
        mcp_url=delivery["endpoint"],
        mcp_key=credential,
    ) as native:
        output = native.execute(["mcp", "list"], timeout=90)
        assert "connected" in output.lower(), output
        assert delivery["endpoint"] in output
        write(
            directory,
            "opencode-local-mcp.json",
            {"passed": True, "version": "1.1.56", "operation": "mcp list", "output": output, "paidModelRequests": 0},
        )
