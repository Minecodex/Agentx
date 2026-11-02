"""Replay exact AI bytes after a proven inspector fix, without another authoring Run."""

import hashlib
import json
import os
import uuid
from pathlib import Path

import pytest

from tests.e2e.product.next_batch_support import get
from tests.e2e.product.plan9.suite import browser_environment
from tests.e2e.product.plan9.test_page_boundaries import verify_page_authoring_scope
from tests.e2e.product.studio_builder_support import post
from tests.e2e.runtime.test_agent_attachments import _control_mysql, _runtime_mysql, _wait_admission_outbox
from tests.e2e.support import run_playwright

pytestmark = [
    pytest.mark.cluster,
    pytest.mark.product,
    pytest.mark.live_model,
    # Ordinary future generation runs must not require this machine's history.
    # An explicitly supplied replay source is mandatory and never skipped for
    # missing/invalid evidence; the current scoped audit also forbids skips.
    pytest.mark.skipif(
        not os.environ.get("AGENTX_E2E_PLAN9_REVALIDATION_SOURCE"),
        reason="Historical inspector replay requires explicitly supplied original generation evidence",
    ),
]


def original_page(directory, round_, attempt, scene="S13"):
    row = json.loads((directory / f"page-results/{scene}-r{round_}.json").read_text())
    source = directory / f"{scene}-r{round_}-attempt{attempt}.html"
    raw = source.read_bytes()
    assert len(row["attempts"]) == attempt + 1
    return {
        "html": raw.decode("utf-8"),
        "rawSha256": hashlib.sha256(raw).hexdigest(),
        "htmlHash": row["attempts"][attempt]["htmlHash"],
        "sourceFile": str(source),
        "sourceDraftId": row["draftId"],
        "sourceRevision": row["attempts"][attempt]["revision"],
        "sourceSessionId": row["sessionId"],
        "sourceRunIds": [value["run"]["id"] for value in row["attempts"]],
        "originalPassed": row["passed"],
    }


def test_original_ai_page_after_inspector_fix(plan9_suite):
    suite = plan9_suite
    source = Path(os.environ["AGENTX_E2E_PLAN9_REVALIDATION_SOURCE"]).resolve() / "plan9"
    assert source.is_dir()
    diagnostic = json.loads((source / "diagnostics/s13-native-form-confirmed.json").read_text())
    assert diagnostic["originalLogicalPage"]["originalFailedStatusPreserved"]
    assert diagnostic["localBrowserDiagnostic"]["stagedSdkPassesFullPreviewOnSameHtml"]
    assert diagnostic["localBrowserDiagnostic"]["formalRequests"] == 0
    prepared = json.loads((source / "prepared-workflows/S13.json").read_text())
    case = next(value for value in suite["cases"] if value["scene"] == "S13")
    _wait_admission_outbox(suite["installed"], workflow_id=case["workflowId"])
    current = get(suite["control"], f"/workflows/{case['workflowId']}/versions/{case['versionId']}")
    # Existing formal fixture builders replay the same definition; no synthetic
    # business implementation or relaxed input/output contract is introduced.
    assert current["definition"] == prepared["workflow"]["definition"]
    for field in ("inputs", "expected", "waitInputs", "repeatNode"):
        assert case[field] == prepared["scenario"][field], field
    earlier, original = original_page(source, 2, 0), original_page(source, 3, 2)
    assert earlier["originalPassed"] and not original["originalPassed"]
    assert len(original["sourceRunIds"]) == 3
    assert original["htmlHash"] == diagnostic["originalLogicalPage"]["htmlHash"]
    assert original["rawSha256"] == diagnostic["originalLogicalPage"]["rawSha256"]
    origin_contract = json.loads((source / "S13-r2-attempt0-publication.json").read_text())["manifest"]["contract"]
    other_case = next(value for value in suite["cases"] if value["scene"] == "S17")
    other_page = original_page(source, 1, 0, scene="S17")
    assert other_page["originalPassed"]
    other_contract = json.loads((source / "S17-r1-attempt0-publication.json").read_text())["manifest"]["contract"]
    deliveries = []
    for scene, rounds in (("S01", (2, 3)), ("S08", (1, 2, 3))):
        delivery_case = next(value for value in suite["cases"] if value["scene"] == scene)
        for round_ in rounds:
            page = original_page(source, round_, 0, scene=scene)
            assert page["originalPassed"]
            contract = json.loads((source / f"{scene}-r{round_}-attempt0-publication.json").read_text())["manifest"][
                "contract"
            ]
            deliveries.append(
                {
                    "scenario": delivery_case,
                    "original": page,
                    "originContract": contract,
                    "suffix": f"{delivery_case['slug']}-r{round_}",
                }
            )
    # Reproduce the real directory condition: the target app need not be on
    # the first table page. These are setup records with another workflow;
    # they create no UI, model Run or business execution.
    directory_noise = [
        post(
            suite["control"],
            "/applications",
            {
                "workflowId": other_case["workflowId"],
                "name": f"Directory pagination {suite['runId']}-{index}",
                "slug": f"p9-directory-{suite['runId']}-{index}",
                "visibility": "company",
            },
        )["id"]
        for index in range(6)
    ]
    report = suite["directory"] / "original-page-revalidation.json"
    payload = {
        "scenario": case,
        "originContract": origin_contract,
        "earlier": earlier,
        "original": original,
        "suffix": case["slug"],
        "report": str(report),
        "sourceRunId": source.parent.name,
        "definitionExactReplay": True,
        "other": {"scenario": other_case, "original": other_page, "originContract": other_contract},
        "additionalDeliveries": deliveries,
    }
    runs_before = _control_mysql(
        suite["installed"],
        "SELECT COUNT(*) FROM design_agent_runs r JOIN design_agent_sessions s ON s.id=r.session_id WHERE s.target_kind='application_ui_builder'",
    )
    calls_before = _runtime_mysql(
        suite["installed"], "SELECT COUNT(*) FROM runtime_calls WHERE source_kind='application_ui_builder'"
    )
    assert runs_before == calls_before == "0"
    run_playwright(
        Path(suite["installed"]["root"]),
        "plan9-original-page-revalidation",
        ("tests/plan9-page-revalidation.spec.ts", "--workers=1"),
        {
            **browser_environment(suite),
            "AGENTX_E2E_PLAN9_REVALIDATION": json.dumps(payload, ensure_ascii=False),
        },
    )
    result = json.loads(report.read_text())
    assert result["passed"] and len(result["executions"]) == 2
    assert result["originalHtmlHash"] == original["htmlHash"]
    assert result["originalRawSha256"] == original["rawSha256"]
    assert result["history"]["activePublicationId"] != result["history"]["restoredPublicationId"]
    assert (
        _control_mysql(
            suite["installed"],
            "SELECT COUNT(*) FROM design_agent_runs r JOIN design_agent_sessions s ON s.id=r.session_id WHERE s.target_kind='application_ui_builder'",
        )
        == runs_before
    )
    assert (
        _runtime_mysql(
            suite["installed"], "SELECT COUNT(*) FROM runtime_calls WHERE source_kind='application_ui_builder'"
        )
        == calls_before
    )
    # Preserve authoritative immutable bindings before the temporary databases
    # disappear. No prompt, provider credential or runtime request is queried.
    queries = {
        "uiVersions": "SELECT HEX(CAST(JSON_OBJECT('id',LOWER(HEX(id)),'applicationId',LOWER(HEX(application_id)),'draftId',LOWER(HEX(ui_draft_id)),'draftRevision',draft_revision,'contract',contract_json,'contractHash',contract_hash,'html',html,'htmlHash',html_hash) AS BINARY)) FROM application_ui_versions",
        "uiPublications": "SELECT HEX(CAST(JSON_OBJECT('id',LOWER(HEX(id)),'applicationId',LOWER(HEX(application_id)),'uiVersionId',LOWER(HEX(ui_version_id)),'slug',slug,'revision',revision,'status',status,'manifest',manifest_json,'error',last_error) AS BINARY)) FROM application_ui_publications",
    }
    bindings = {
        name: [json.loads(bytes.fromhex(row)) for row in _control_mysql(suite["installed"], query).splitlines() if row]
        for name, query in queries.items()
    }
    publication = result["revalidatedPublication"]["publication"]
    version = next(row for row in bindings["uiVersions"] if row["id"] == uuid.UUID(publication["uiVersionId"]).hex)
    assert version["html"].encode("utf-8") == original["html"].encode("utf-8")
    assert version["htmlHash"] == publication["manifest"]["htmlHash"] == original["htmlHash"]
    assert version["contract"] == publication["manifest"]["contract"]
    bindings.update(
        newPageAuthoringRuns=0,
        newPageModelCalls=0,
        originalSourceRun=source.parent.name,
        replayedWorkflow={"id": case["versionId"], "definition": current["definition"]},
        originalWorkflowEvidence=str(source / "prepared-workflows/S13.json"),
    )
    (suite["directory"] / "original-page-revalidation-bindings.json").write_text(
        json.dumps(bindings, ensure_ascii=False, indent=2)
    )
    result.update(
        originalGenerationRunIds=original["sourceRunIds"],
        originalGenerationRevision=original["sourceRevision"],
        newPageAuthoringRuns=0,
        newPageModelCalls=0,
        definitionExactReplay=True,
        originalSourceRun=source.parent.name,
        originalFailedReportPreserved=True,
        historySeedNotCountedAsRequiredBusinessExecution=True,
        additionalBoundaryPageNotCountedAsRequiredGenerationOrExecution=True,
        additionalOriginalDeliveriesAreRegressionSetupOnly=True,
        directoryPaginationSetupApplicationIds=directory_noise,
    )
    report.write_text(json.dumps(result, ensure_ascii=False, indent=2))


def test_original_ai_delivery_authoring_boundaries(plan9_suite):
    """Repeat every original boundary operation with scoped observer counters."""
    suite = plan9_suite
    replay = json.loads((suite["directory"] / "original-page-revalidation.json").read_text())
    assert replay["passed"] and replay["newPageAuthoringRuns"] == replay["newPageModelCalls"] == 0
    current, other = replay["revalidatedPublication"], replay["boundaryAlternate"]
    verify_page_authoring_scope(suite, current, current["applicationId"], other["applicationId"])
    result = json.loads((suite["directory"] / "page-authoring-boundaries.json").read_text())
    assert result["passed"] and len(result["checks"]) == 16
