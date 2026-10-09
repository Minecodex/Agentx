"""Real judge executions, case-key alignment and numeric Insights aggregates."""

from __future__ import annotations

import json
import math
import time
import uuid
from datetime import UTC, datetime, timedelta

import httpx
import pytest

from tests.e2e.product import test_channel_delivery as channels
from tests.e2e.product import test_model_streaming as models
from tests.e2e.runtime.test_agent_attachments import _access_token
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def post(client, path, body):
    response = client.post(path, json=body)
    assert response.status_code in (200, 201, 202), response.text
    return response.json()


def evaluation(client, workflow, profile, stamp, keys):
    dataset = post(client, "/api/v1/datasets", {"name": f"Compare {stamp}", "visibility": "company"})
    post(
        client,
        f"/api/v1/datasets/{dataset['id']}/import",
        {
            "expectedRevision": dataset["revision"],
            "format": "jsonl",
            "content": "\n".join(
                json.dumps({"caseKey": key, "name": key, "input": {"message": key}, "expectedOutput": {"answer": key}})
                for key in keys
            ),
        },
    )
    version = post(client, f"/api/v1/datasets/{dataset['id']}/versions", {"expectedRevision": dataset["revision"] + 1})
    result = post(
        client,
        "/api/v1/evaluations",
        {
            "name": f"Compare {stamp}",
            "workflowVersionId": workflow["versionId"],
            "datasetVersionId": version["id"],
            "evaluationProfileVersionId": profile["versionId"],
            "visibility": "company",
            "parameters": {"concurrency": 2},
        },
    )
    started = client.post(f"/api/v1/evaluations/{result['id']}/start")
    assert started.status_code == 202, started.text
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        report = client.get(f"/api/v1/evaluations/{result['id']}/report")
        assert report.status_code == 200, report.text
        value = report.json()
        assert value["run"]["status"] not in {"failed", "cancelled"}, value
        if value["run"]["status"] == "completed":
            assert len(value["results"]) == len(keys), value
            for case in value["results"]:
                assert case["status"] == "completed", case
                assert case["durationMs"] > 0 and case["costMicros"] > 0, json.dumps(case)
                judge = next(rule for rule in case["ruleResults"] if rule["key"] == "judge")
                assert judge["status"] == "passed" and judge["passed"] is True, json.dumps(judge)
                assert judge["durationMs"] > 0 and judge["costMicros"] > 0, json.dumps(judge)
                assert judge["evaluatorExecutionId"] and judge["detail"]["modelResult"], judge
                trace = client.get(f"/api/v1/executions/{judge['evaluatorExecutionId']}")
                assert trace.status_code == 200, trace.text
                assert trace.json()["workflowName"].startswith("Compare "), trace.text
            return result, value
        time.sleep(2)
    raise TimeoutError("judge evaluation did not reach its projected terminal report")


@pytest.fixture(scope="module")
def comparison_application(installed_agentx, service_urls, e2e_providers):
    stamp = uuid.uuid4().hex[:12]
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        control.headers.update(headers)
        workflow = channels._passthrough_workflow(control, headers, f"Compare {stamp}")
        credential = models._fixture_credential(control, headers, me["departmentId"])
        alias = f"judge-{stamp}"
        models._upsert_model(
            control, headers, alias, e2e_providers["echo_mcp"], "echo-model", me["departmentId"], credential
        )
        model = next(
            item
            for item in control.get(f"/api/v1/models/aliases?search={alias}").json()["items"]
            if item["alias"] == alias
        )
        models._grant_model_to_workflow(control, headers, alias, workflow["workflowId"], credential)
        profile = post(
            control,
            "/api/v1/evaluation-profiles",
            {
                "name": f"Compare judge {stamp}",
                "visibility": "company",
                "aggregation": "all",
                "passThreshold": "1",
                "rules": [
                    {
                        "key": "exact",
                        "name": "Exact",
                        "evaluatorType": "exact",
                        "configuration": {},
                        "weight": "1",
                        "required": True,
                    },
                    {
                        "key": "judge",
                        "name": "Judge",
                        "evaluatorType": "llm_judge",
                        "configuration": {
                            "modelId": model["id"],
                            "prompt": "Judge {{actualOutput}} against {{expectedOutput}}. Return passed, score and reason in the requested JSON schema.",
                        },
                        "weight": "1",
                        "required": True,
                    },
                ],
            },
        )
        baseline, _ = evaluation(control, workflow, profile, f"{stamp}-baseline", ["common", "baseline_only"])
        candidate, _ = evaluation(control, workflow, profile, f"{stamp}-candidate", ["common", "candidate_only"])
        compared = control.get("/api/v1/evaluations/compare", params={"runIds": f"{baseline['id']},{candidate['id']}"})
        assert compared.status_code == 200, compared.text
        comparison = compared.json()
        assert comparison["alignedCaseCount"] == 1 and comparison["totalCaseCount"] == 3, comparison
        common = next(item for item in comparison["caseDeltas"] if item["caseKey"] == "common")
        assert all(common["targetExecutionIdByRun"]) and common["statusByRun"] == ["completed", "completed"], common
        absent = next(item for item in comparison["caseDeltas"] if item["caseKey"] == "baseline_only")
        assert absent["statusByRun"][1] is None, absent
        judge = next(rule for rule in comparison["ruleAggregates"] if rule["ruleKey"] == "judge")
        assert all(len(result["evaluatorExecutionIds"]) == 2 for result in judge["resultsByRun"]), judge
        for ids in (f"{baseline['id']},{baseline['id']}", "invalid,uuid"):
            assert control.get("/api/v1/evaluations/compare", params={"runIds": ids}).status_code == 400
        now = datetime.now(UTC)
        query = {
            "from": (now - timedelta(hours=1)).isoformat(),
            "to": (now + timedelta(hours=1)).isoformat(),
            "metrics": ["count", "succeeded_count", "failed_count", "duration_p50", "duration_p95"],
            "dimensions": ["workflow"],
            "filters": {"workflowId": workflow["workflowId"]},
            "limit": 10,
        }
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            insight = control.post("/api/v1/insights/aggregates", json=query)
            assert insight.status_code == 200, insight.text
            rows = insight.json()["rows"]
            if rows:
                break
            time.sleep(2)
        assert rows, insight.text
        for row in rows:
            metrics = row["metrics"]
            assert all(
                isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)
                for value in metrics.values()
            ), row
            assert metrics["count"] == metrics["succeededCount"] + metrics["failedCount"], row
            assert metrics["durationP95"] > 0, row
        write_report(
            installed_agentx,
            "product/evaluation-comparison.json",
            {
                "status": "passed",
                "baselineId": baseline["id"],
                "candidateId": candidate["id"],
                "alignedCaseCount": 1,
                "totalCaseCount": 3,
                "judgeTraceLinks": True,
                "numericInsights": True,
            },
        )

        return {"baselineId": baseline["id"], "candidateId": candidate["id"], "modelAlias": alias}


def test_judge_report_comparison_and_insights(comparison_application):
    assert comparison_application["baselineId"] != comparison_application["candidateId"]
