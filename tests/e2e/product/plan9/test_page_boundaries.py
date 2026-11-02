"""Use real AI drafts to check revision, actor and publication boundaries without changing their HTML."""

import copy
import hashlib
import json
import time
import uuid
from concurrent.futures import ThreadPoolExecutor

import httpx
import pytest

from tests.e2e.product.next_batch_support import get
from tests.e2e.runtime.test_agent_attachments import _control_mysql, _runtime_mysql

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def active(suite, application):
    listing = get(suite["control"], f"/applications/{application}/ui/publications")
    return listing, next(row for row in listing["publications"] if row["id"] == listing["activePublicationId"])


def test_real_page_authoring_scope_and_stale_commands(plan9_suite):
    suite = plan9_suite
    cases = {row["scene"]: row for row in suite["cases"]}
    pages = json.loads((suite["directory"] / "page-results.json").read_text())
    page = next(row for row in pages if row["scene"] == "S01" and row["round"] == 3 and row["passed"])
    verify_page_authoring_scope(suite, page, cases["S01"]["applicationId"], cases["S02"]["applicationId"])


def verify_page_authoring_scope(suite, page, application, other_application):
    """The same real boundary operations also apply to explicitly replayed AI drafts."""
    report = {"passed": False, "checks": {}}
    control = suite["control"]
    path = f"/api/v1/applications/{application}/ui/drafts/{page['draftId']}"
    draft = control.get(path).json()
    assert draft["html"] and draft["revision"] > 0
    session_path = f"/api/v1/application-ui/sessions/{page['sessionId']}"
    session = control.get(session_path).json()
    snapshot = session["snapshot"]
    before = _runtime_mysql(
        suite["installed"],
        "SELECT COUNT(*) FROM runtime_calls WHERE call_kind IN ('model','compaction') AND source_kind IN ('studio_builder','application_ui_builder')",
    )

    def rejected(label, response, statuses):
        assert response.status_code in statuses, f"{label}: {response.status_code} {response.text}"
        report["checks"][label] = {"status": response.status_code}

    try:
        login = control.post(
            "/api/v1/auth/login",
            auth=None,
            json={"username": suite["employee"]["username"], "password": suite["employee"]["password"].value},
        )
        assert login.status_code == 200
        with httpx.Client(
            base_url=suite["urls"]["web"],
            headers={"Authorization": "Bearer " + login.json()["accessToken"]},
            timeout=60,
            trust_env=False,
        ) as employee:
            rejected("invokeOnlyCannotReadDraft", employee.get(path), {403, 404})
            rejected("invokeOnlyCannotReadCreatorSession", employee.get(session_path), {403, 404})
            rejected(
                "invokeOnlyCannotWriteHtml",
                employee.put(
                    path, json={"saveId": str(uuid.uuid4()), "baseRevision": draft["revision"], "html": draft["html"]}
                ),
                {403, 404},
            )
            rejected(
                "invokeOnlyCannotStartAuthoring",
                employee.post(
                    session_path + "/runs",
                    json={"runId": str(uuid.uuid4()), "prompt": "读取当前页面", "snapshot": snapshot},
                ),
                {403, 404},
            )
        rejected(
            "staleSaveCannotOverwriteAiHtml",
            control.put(
                path, json={"saveId": str(uuid.uuid4()), "baseRevision": draft["revision"] - 1, "html": draft["html"]}
            ),
            {409},
        )
        rejected(
            "crossApplicationCannotBindDraft",
            control.post(
                f"/api/v1/applications/{other_application}/ui/drafts/{page['draftId']}/builder-sessions",
                json={"snapshot": snapshot},
            ),
            {404},
        )
        rejected(
            "wrongEditorCannotReplaceSnapshot",
            control.put(session_path + "/snapshot", json={**snapshot, "editorInstanceId": str(uuid.uuid4())}),
            {409},
        )
        rejected(
            "tamperedHtmlHashCannotReplaceSnapshot",
            control.put(session_path + "/snapshot", json={**snapshot, "htmlHash": "sha256:" + "0" * 64}),
            {409},
        )
        rejected(
            "forgedActorCannotStartRun",
            control.post(
                session_path + "/runs",
                json={
                    "runId": str(uuid.uuid4()),
                    "prompt": "读取当前页面",
                    "snapshot": snapshot,
                    "actorId": suite["employee"]["id"],
                },
            ),
            {400, 422},
        )
        for field, forged in (
            ("profile", "workflow_builder"),
            ("tenantId", str(uuid.uuid4())),
            ("target", {"kind": "workflow_builder", "draftId": page["draftId"]}),
        ):
            rejected(
                "forgedRunField-" + field,
                control.post(
                    session_path + "/runs",
                    json={"runId": str(uuid.uuid4()), "prompt": "读取当前页面", "snapshot": snapshot, field: forged},
                ),
                {400, 422},
            )
        rejected(
            "forgedCommandCannotAcknowledge",
            control.post(
                session_path + "/acknowledgements",
                json={"commandId": str(uuid.uuid4()), "status": "applied", "snapshot": snapshot, "report": None},
            ),
            {404, 409},
        )
        listing, publication = active(suite, application)
        body = {
            "draftId": None,
            "uiVersionId": publication["uiVersionId"],
            "expectedDraftRevision": None,
            "expectedPublicationRevision": listing["revision"],
            "slug": publication["slug"],
            "enabled": True,
        }
        publish_path = f"/api/v1/applications/{application}/ui/publications"
        rejected(
            "stalePublicationCannotReplacePair",
            control.post(publish_path, json={**body, "expectedPublicationRevision": listing["revision"] - 1}),
            {409},
        )
        rejected(
            "pathTraversalSuffixRejected", control.post(publish_path, json={**body, "slug": "../escape"}), {400, 422}
        )
        _, another = active(suite, other_application)
        rejected(
            "anotherApplicationsSuffixProtected",
            control.post(publish_path, json={**body, "slug": another["slug"]}),
            {409},
        )
        assert control.get(path).json() == draft
        assert control.get(session_path).json()["snapshot"] == snapshot
        assert active(suite, application)[0]["activePublicationId"] == listing["activePublicationId"]
        after = _runtime_mysql(
            suite["installed"],
            "SELECT COUNT(*) FROM runtime_calls WHERE call_kind IN ('model','compaction') AND source_kind IN ('studio_builder','application_ui_builder')",
        )
        assert after == before
        employee_id = uuid.UUID(suite["employee"]["id"]).hex
        # Earlier tests intentionally execute other applications as administrators
        # and debug bare workflows. Scope this no-business-effect proof to actual
        # AI-page applications/Bundles, rather than their unrelated namespace work.
        scope_rows = [
            json.loads(bytes.fromhex(row))
            for row in _control_mysql(
                suite["installed"],
                "SELECT DISTINCT HEX(CAST(JSON_OBJECT('applicationId',LOWER(HEX(application_id)),'bundleId',LOWER(HEX(bundle_id))) AS BINARY)) FROM application_ui_drafts",
            ).splitlines()
            if row
        ]
        page_applications = sorted({uuid.UUID(row["applicationId"]).hex for row in scope_rows})
        page_bundles = sorted({uuid.UUID(row["bundleId"]).hex for row in scope_rows})
        assert page_applications and page_bundles
        applications_sql = ",".join("UNHEX('" + value + "')" for value in page_applications)
        bundles_sql = ",".join("UNHEX('" + value + "')" for value in page_bundles)
        privileged_invocations = int(
            _runtime_mysql(
                suite["installed"],
                f"SELECT COUNT(*) FROM application_invocations WHERE caller_type='user' AND caller_id<>UNHEX('{employee_id}') AND application_id IN ({applications_sql})",  # noqa: S608 -- parsed UUID only.
            )
        )
        unassociated_executions = int(
            _runtime_mysql(
                suite["installed"],
                f"SELECT COUNT(*) FROM workflow_executions WHERE parent_execution_id IS NULL AND invocation_id IS NULL AND bundle_id IN ({bundles_sql})",  # noqa: S608 -- all Bundle IDs are parsed UUIDs from authoritative page drafts.
            )
        )
        assert privileged_invocations == 0, (
            "Authoring and synthetic previews must not invoke business as the administrator"
        )
        assert unassociated_executions == 0, "Design operations must not create business Executions"
        report.update(
            passed=True,
            htmlHash=draft["htmlHash"],
            originalRevision=draft["revision"],
            modelCallsBefore=int(before),
            modelCallsAfter=int(after),
            privilegedBusinessInvocations=privileged_invocations,
            unassociatedRootExecutions=unassociated_executions,
            pageApplicationScope=page_applications,
            pageBundleScope=page_bundles,
            modelCallScope=["studio_builder", "application_ui_builder"],
        )
    finally:
        (suite["directory"] / "page-authoring-boundaries.json").write_text(
            json.dumps(report, ensure_ascii=False, indent=2)
        )


def test_concurrent_page_publication_has_one_atomic_winner(plan9_suite):
    suite, report = plan9_suite, {"passed": False}
    application = next(row["applicationId"] for row in suite["cases"] if row["scene"] == "S01")
    listing, original = active(suite, application)
    versions = [
        row
        for row in listing["publications"]
        if row["status"] in {"active", "superseded"} and row["uiVersionId"] != original["uiVersionId"]
    ]
    assert versions
    requests = [
        {
            "draftId": None,
            "uiVersionId": value["uiVersionId"],
            "expectedDraftRevision": None,
            "expectedPublicationRevision": listing["revision"],
            "slug": f"p9-race-{suffix}-{suite['runId']}",
            "enabled": True,
        }
        for suffix, value in zip(("a", "b"), (original, versions[-1]), strict=True)
    ]
    try:
        with ThreadPoolExecutor(max_workers=2) as pool:
            responses = list(
                pool.map(
                    lambda body: suite["control"].post(
                        f"/api/v1/applications/{application}/ui/publications", json=body
                    ),
                    requests,
                )
            )
        assert sorted(response.status_code for response in responses) == [200, 409], [
            response.text for response in responses
        ]
        winner_index = next(index for index, response in enumerate(responses) if response.is_success)
        winner = responses[winner_index].json()
        until = time.monotonic() + 120
        while (published := active(suite, application))[0]["activePublicationId"] != winner["id"]:
            assert time.monotonic() < until, published
            time.sleep(0.5)
        current = published[1]
        assert current["uiVersionId"] == requests[winner_index]["uiVersionId"]
        login = suite["control"].post(
            "/api/v1/auth/login",
            auth=None,
            json={"username": suite["employee"]["username"], "password": suite["employee"]["password"].value},
        )
        assert login.status_code == 200
        with httpx.Client(
            base_url=suite["urls"]["runtime"],
            headers={"Authorization": "Bearer " + login.json()["accessToken"]},
            timeout=60,
            trust_env=False,
        ) as employee:
            view = employee.get(f"/gateway/v1/pages/{current['slug']}")
            assert view.status_code == 200, view.text
            view = view.json()
            assert view["publication"]["publicationId"] == current["id"]
            assert view["publication"]["contract"]["bundleId"] == current["manifest"]["contract"]["bundleId"]
            for name, content in (("sdk", "sdkScript"), ("stylesheet", "stylesheet")):
                assert (
                    "sha256:" + hashlib.sha256(view[content].encode()).hexdigest()
                    == current["manifest"][name]["contentHash"]
                )
            assert employee.get(f"/gateway/v1/pages/{original['slug']}").status_code == 404
        report.update(
            passed=True,
            responses=[response.status_code for response in responses],
            winner=copy.deepcopy(current),
            oldSuffixRemoved=True,
            exactSdkAndStylesheet=True,
        )
    finally:
        (suite["directory"] / "page-publication-concurrency.json").write_text(
            json.dumps(report, ensure_ascii=False, indent=2)
        )
