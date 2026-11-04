"""Opt-in checks for a retained local batch demo, without installing another stack."""

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

from tests.e2e.support import ROOT, run_playwright

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.infrastructure,
    pytest.mark.skipif(
        not os.environ.get("AGENTX_LOCAL_BATCH_DELIVERY_CONFIG"),
        reason="Requires an explicitly authorized retained local batch application",
    ),
]


def configuration():
    config = json.loads(Path(os.environ["AGENTX_LOCAL_BATCH_DELIVERY_CONFIG"]).read_text())
    assert config["baseUrl"] == "https://agentx.localhost"
    assert config["runtimeUrl"] == "https://run.agentx.localhost"
    directory = Path(config["evidenceDir"])
    case = json.loads((directory / "batch-scenario.json").read_text())
    assert case["slug"] == "expense-batch-demo"
    return config, directory, case


def checked(response):
    assert response.is_success, (response.status_code, response.text)
    return response.json()


def test_retained_batch_contract_parser_and_atomic_rejection():
    config, directory, case = configuration()
    tls = ssl.create_default_context(cafile=config["caFile"])
    with (
        httpx.Client(base_url=config["baseUrl"], verify=tls, trust_env=False, timeout=60) as control,
        httpx.Client(base_url=config["runtimeUrl"], verify=tls, trust_env=False, timeout=60) as runtime,
    ):
        credentials = json.loads(Path(config["credentialsFile"]).read_text())
        token = checked(control.post("/api/v1/auth/login", json=credentials))["accessToken"]
        control.headers["Authorization"] = runtime.headers["Authorization"] = "Bearer " + token
        draft = checked(control.get(f"/api/v1/applications/{case['applicationId']}/ui/drafts/{case['draftId']}"))
        original = json.loads((directory / "original-page-draft.json").read_text())
        assert draft["contract"]["sdkVersion"] == 2
        assert draft["contract"]["workflowVersionId"] == case["versionId"]
        assert draft["contract"]["inputSchema"] == original["contract"]["inputSchema"]
        assert draft["contract"]["outputSchema"] == original["contract"]["outputSchema"]
        assert draft["contract"]["waitInputNodes"] == original["contract"]["waitInputNodes"]
        assert draft["html"] == (directory / "expense-batch-page.html").read_text()
        path = f"/gateway/v1/applications/{case['slug']}/batches"
        deadline = time.monotonic() + 60
        while True:
            response = runtime.get(path)
            if response.is_success:
                break
            assert response.status_code in (401, 403, 404) and time.monotonic() < deadline
            time.sleep(0.5)
        before = checked(response)
        csv_text = (
            '单号,申请人,类型,金额\nBX-PARSE-001,"示例甲,分部",差旅费,268.50\nBX-PARSE-002,示例乙,办公采购,135.75\n'
        )
        table = checked(
            runtime.post(
                f"/gateway/v1/applications/{case['slug']}/batch-imports/parse",
                json={"text": csv_text, "delimiter": ","},
            )
        )
        assert table["headers"] == ["单号", "申请人", "类型", "金额"]
        assert table["lines"] == [2, 3]
        assert table["rows"] == [
            ["BX-PARSE-001", "示例甲,分部", "差旅费", "268.50"],
            ["BX-PARSE-002", "示例乙", "办公采购", "135.75"],
        ]
        inputs = [
            {"request_id": "BX-ATOMIC-001", "applicant": "示例甲", "expense_type": "差旅费", "amount": 268.5},
            {"request_id": "BX-ATOMIC-002", "applicant": "示例乙", "expense_type": "办公采购", "amount": 135.75},
            {"request_id": "BX-ATOMIC-003", "applicant": "示例丙", "expense_type": "客户招待", "amount": -1},
        ]
        invalid = runtime.post(
            path,
            headers={"Idempotency-Key": str(uuid.uuid4())},
            json={
                "name": "Atomic validation; no business work is accepted",
                "items": [{"clientItemId": str(index), "input": value} for index, value in enumerate(inputs)],
            },
        )
        assert invalid.status_code == 422, invalid.text
        assert any(error["field"].startswith("/items/2/input/amount") for error in invalid.json()["fieldErrors"])
        after = checked(runtime.get(path))
        assert [row["batchId"] for row in after["items"]] == [row["batchId"] for row in before["items"]]
        managed = checked(control.get(f"/api/v1/applications/{case['applicationId']}/batches"))
        assert {row["batchId"] for row in managed["items"]} >= {row["batchId"] for row in after["items"]}
        report = {
            "passed": True,
            "sameWorkflowVersionAndBusinessContract": True,
            "sdkVersion": 2,
            "csv": table,
            "atomicRejection": invalid.json(),
            "batchIdsBefore": [row["batchId"] for row in before["items"]],
            "batchIdsAfter": [row["batchId"] for row in after["items"]],
            "formalSubmissions": 0,
        }
        (directory / "batch-backend-verification.json").write_text(
            json.dumps(report, ensure_ascii=False, indent=2) + "\n"
        )


@pytest.mark.skipif(
    os.environ.get("AGENTX_LOCAL_BATCH_BROWSER_AUTHORIZED") != "1",
    reason="Requires browser site access approval to be granted before UI actions",
)
def test_retained_batch_page_publish_and_real_business():
    config, directory, _ = configuration()
    try:
        run_playwright(
            ROOT,
            "local-batch-demo",
            ("tests/local-batch-delivery.spec.ts",),
            {
                **os.environ,
                "AGENTX_E2E_BASE_URL": config["baseUrl"],
                "AGENTX_E2E_RUNTIME_URL": config["runtimeUrl"],
                "AGENTX_E2E_STAGE": "local-batch",
                "AGENTX_E2E_RUN_ID": config["runId"],
                "NODE_EXTRA_CA_CERTS": config["caFile"],
            },
        )
    finally:
        output = ROOT / ".local/artifacts/playwright/local-batch" / config["runId"] / "local-batch-demo"
        for filename in (
            "expense-batch-entry.jpg",
            "expense-batch-completed.jpg",
            "expense-batch-waiting.jpg",
            "expense-batch-list.jpg",
        ):
            matches = list(output.rglob(filename)) if output.exists() else []
            if matches:
                shutil.copyfile(max(matches, key=lambda path: path.stat().st_mtime_ns), directory / filename)
    report = json.loads((directory / "batch-browser-verification.json").read_text())
    assert report["passed"] and len(report["batches"]) == 2
    assert all(batch["counts"]["total"] == 3 for batch in report["batches"])
    assert report["formalBatchPosts"] == 2 and report["unexpectedInvocations"] == []
