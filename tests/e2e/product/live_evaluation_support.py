"""Real Kimi Judge execution and report fixtures shared by API and UI tests."""

import json
import time

import httpx
import pytest

from tests.e2e.product.live_text_support import post
from tools.scripts.release.evidence import write_report


def _evaluate(control, app, profile, version, name, keys):
    dataset = post(control, "/datasets", {"name": name, "visibility": "company"})
    post(
        control,
        f"/datasets/{dataset['id']}/import",
        {
            "expectedRevision": dataset["revision"],
            "format": "jsonl",
            "content": "\n".join(
                json.dumps(
                    {
                        "caseKey": key,
                        "name": key,
                        "input": {"message": f"请只输出:{key}"},
                        "expectedOutput": {"answer": key},
                    },
                    ensure_ascii=False,
                )
                for key in keys
            ),
        },
    )
    published = post(control, f"/datasets/{dataset['id']}/versions", {"expectedRevision": dataset["revision"] + 1})
    evaluation = post(
        control,
        "/evaluations",
        {
            "name": name,
            "workflowVersionId": version,
            "datasetVersionId": published["id"],
            "evaluationProfileVersionId": profile["versionId"],
            "visibility": "company",
            "parameters": {"concurrency": 1},
        },
    )
    started = control.post(f"/api/v1/evaluations/{evaluation['id']}/start")
    assert started.status_code == 202, started.text
    deadline = time.monotonic() + 600
    while True:
        report = control.get(f"/api/v1/evaluations/{evaluation['id']}/report").json()
        assert report["run"]["status"] not in {"failed", "cancelled"}, report
        if report["run"]["status"] == "completed":
            break
        assert time.monotonic() < deadline, report
        time.sleep(1)
    assert len(report["results"]) == len(keys), report
    for case in report["results"]:
        assert case["status"] == "completed", case
        judge = next(rule for rule in case["ruleResults"] if rule["key"] == "judge")
        assert judge["status"] == "passed" and judge["passed"] is True, judge
        assert judge["evaluatorExecutionId"] and judge["detail"]["modelResult"], judge
        assert control.get(f"/api/v1/executions/{judge['evaluatorExecutionId']}").json()["status"] == "succeeded"
    return evaluation["id"], report


@pytest.fixture(scope="session")
def live_comparison(installed_agentx, service_urls, live_application, run_id):
    app = live_application
    headers = {"Authorization": f"Bearer {app['token'].value}"}
    with httpx.Client(base_url=service_urls["web"], headers=headers, timeout=60) as control:
        profile = post(
            control,
            "/evaluation-profiles",
            {
                "name": f"P7 real Kimi Judge {run_id}",
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
                        "name": "Kimi Judge",
                        "evaluatorType": "llm_judge",
                        "configuration": {
                            "modelId": app["modelId"],
                            "prompt": "Compare {{actualOutput}} with {{expectedOutput}}. Ignore whitespace. Return only JSON with passed (boolean), score (0 to 1), reason (string).",
                        },
                        "weight": "1",
                        "required": True,
                    },
                ],
            },
        )
        baseline, baseline_report = _evaluate(
            control, app, profile, app["v1"], f"P7 V1 {run_id}", ["水星一", "水星二", "baseline_only"]
        )
        candidate, candidate_report = _evaluate(
            control, app, profile, app["v2"], f"P7 V2 {run_id}", ["水星一", "水星二", "candidate_only"]
        )
        response = control.get("/api/v1/evaluations/compare", params={"runIds": f"{baseline},{candidate}"})
        assert response.status_code == 200, response.text
        comparison = response.json()
        assert comparison["alignedCaseCount"] == 2 and comparison["totalCaseCount"] == 4, comparison
        absent = next(item for item in comparison["caseDeltas"] if item["caseKey"] == "baseline_only")
        assert absent["statusByRun"][1] is None, absent
        write_report(
            installed_agentx,
            "product/live-kimi-evaluation.json",
            {
                "status": "passed",
                "baselineId": baseline,
                "candidateId": candidate,
                "v1": app["v1"],
                "v2": app["v2"],
                "baselineReport": baseline_report,
                "candidateReport": candidate_report,
                "comparison": comparison,
            },
        )
        return {"baselineId": baseline, "candidateId": candidate, "modelAlias": app["modelAlias"]}
