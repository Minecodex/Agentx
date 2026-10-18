"""Application-owned datasets, immutable AI scoring and desktop workflows."""

from __future__ import annotations

import copy
import json
import os
import time
import uuid
from pathlib import Path

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.test_channel_delivery import (
    _development_environment_id,
    _passthrough_workflow,
    _publish_application,
)
from tests.e2e.product.test_live_memory_pipeline import _save_version
from tests.e2e.product.test_model_streaming import _fixture_credential, _upsert_model
from tests.e2e.runtime.test_agent_attachments import _access_token, _runtime_mysql
from tests.e2e.support import run, run_playwright
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def send(client, path, body, *, method="POST", key=None, status=(200, 201, 202)):
    response = client.request(
        method, "/api/v1" + path, json=body, headers={"Idempotency-Key": key or str(uuid.uuid4())}
    )
    assert response.status_code in status, (path, response.status_code, response.text)
    return response.json() if response.content else None


def read(client, path):
    response = client.get("/api/v1" + path)
    assert response.status_code == 200, (path, response.text)
    return response.json()


def finished(client, path, batch):
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        report = read(client, f"{path}/evaluations/{batch}/report")
        if report["status"] in {"completed", "failed", "cancelled"} and all(
            child["status"] in {"completed", "failed", "cancelled"} for child in report["runs"]
        ):
            return report
        time.sleep(0.5)
    raise AssertionError(f"Evaluation did not converge: {report}")


def select_model(client, model):
    setting = read(client, "/platform-ai/model-setting")
    send(
        client,
        "/platform-ai/model-setting",
        {"expectedVersion": setting["version"], "modelAliasId": model["id"]},
        method="PUT",
    )


@pytest.fixture(scope="module")
def application_testing(installed_agentx, service_urls, e2e_providers, run_id):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        workflow = _passthrough_workflow(control, dict(control.headers), f"Application tests {run_id}")
        app = send(
            control,
            "/applications",
            {
                "name": f"Application tests {run_id}",
                "slug": f"app-testing-{run_id}",
                "workflowId": workflow["workflowId"],
                "visibility": "company",
            },
        )
        _publish_application(
            control,
            dict(control.headers),
            workflow,
            app["id"],
            _development_environment_id(control, dict(control.headers)),
        )
        definition = copy.deepcopy(fixture_draft(control, workflow["workflowId"])["definition"])
        definition["nodes"][0]["name"] = "Same contract second version"
        v2 = _save_version(control, workflow["workflowId"], definition)
        definition["start"]["inputs"]["properties"]["region"] = {"type": "string", "enum": ["CN", "US"]}
        definition["start"]["inputs"]["required"].append("region")
        v3 = _save_version(control, workflow["workflowId"], definition)
        definition["nodes"][0]["name"] = "Same new contract fourth version"
        v4 = _save_version(control, workflow["workflowId"], definition)
        credential = _fixture_credential(control, dict(control.headers), me["departmentId"])
        models = []
        for name, upstream in [("judge", "echo-model"), ("invalid", "echo-invalid-judge")]:
            alias = f"app-{name}-{run_id}"
            _upsert_model(
                control,
                dict(control.headers),
                alias,
                e2e_providers["echo_mcp"],
                upstream,
                me["departmentId"],
                credential,
            )
            models.append(read(control, f"/models/aliases?search={alias}")["items"][0])
        select_model(control, models[0])
        role = send(
            control,
            "/roles",
            {
                "code": f"eval_reader_{run_id}".replace("-", "_"),
                "name": f"Evaluation reader {run_id}",
                "dataScope": "own",
                "permissions": [
                    "application:view",
                    "workflow:view",
                    "dataset:view",
                    "evaluation:view",
                    "evaluation_profile:view",
                ],
            },
        )
        username = "eval_reader_" + uuid.uuid4().hex[:10]
        send(
            control,
            "/users",
            {
                "username": username,
                "displayName": "Evaluation reader",
                "departmentId": me["departmentId"],
                "roleId": role["id"],
            },
        )
        login = send(control, "/auth/login", {"username": username, "password": "123456"})
        if login.get("passwordChangeRequired"):
            send(
                control,
                "/auth/change-password",
                {"token": login["changePasswordToken"], "password": "application-reader-password"},
            )
        reader = send(control, "/auth/login", {"username": username, "password": "application-reader-password"})
        empty = send(
            control,
            "/applications",
            {
                "name": "Empty application for reader",
                "slug": f"reader-{uuid.uuid4().hex[:12]}",
                "workflowId": workflow["workflowId"],
                "visibility": "company",
            },
        )
        # Neither model nor credential is granted to the business Workflow.
        yield {
            "control": control,
            "app": app,
            "path": f"/applications/{app['id']}",
            "versions": [workflow["versionId"], v2["id"], v3["id"], v4["id"]],
            "models": models,
            "context": installed_agentx,
            "urls": service_urls,
            "reader": {"username": username, "token": reader["accessToken"], "applicationId": empty["id"]},
        }


def test_batch_contracts_copy_revision_recovery_and_model_isolation(application_testing):
    state = application_testing
    client, path, versions = state["control"], state["path"], state["versions"]
    contracts = read(client, path + "/test-contracts")
    fingerprint = {item["id"]: item["contract"]["fingerprint"] for item in contracts}
    assert fingerprint[versions[0]] == fingerprint[versions[1]]
    assert fingerprint[versions[2]] == fingerprint[versions[3]] != fingerprint[versions[0]]
    dataset = send(client, path + "/datasets", {"name": "Original", "workflowVersionId": versions[0]})
    send(
        client,
        f"{path}/datasets/{dataset['id']}/cases",
        {
            "expectedRevision": 0,
            "name": "Stable origin",
            "input": {"message": "Hello"},
            "expectedOutput": {"answer": "Hello"},
            "tags": ["regression"],
        },
    )
    source = read(client, f"{path}/datasets/{dataset['id']}")
    body = {"name": "New contract", "workflowVersionId": versions[2], "expectedRevision": source["revision"]}
    preview = send(client, f"{path}/datasets/{source['id']}/copy-preview", body)
    assert preview["contractChanged"] and not preview["cases"][0]["referenceConfirmed"]
    assert preview["incompleteCount"] == 1 and any(
        "region" in error["field"] for error in preview["cases"][0]["validationErrors"]
    )
    copied = send(client, f"{path}/datasets/{source['id']}/copies", body)
    assert copied["incompleteCount"] == 1
    copied_case = read(client, f"{path}/datasets/{copied['id']}/cases")[0]
    fill = {
        "expectedRevision": copied["revision"],
        "version": copied_case["version"],
        "name": copied_case["name"],
        "input": {**copied_case["input"], "region": "CN"},
        "expectedOutput": copied_case["expectedOutput"],
        "tags": copied_case["tags"],
        "referenceConfirmed": True,
    }
    send(client, f"{path}/datasets/{copied['id']}/cases/{copied_case['id']}", fill, method="PATCH")
    send(client, f"{path}/datasets/{copied['id']}/cases/{copied_case['id']}", fill, method="PATCH", status=(409,))
    copied = read(client, f"{path}/datasets/{copied['id']}")
    assert copied["incompleteCount"] == 0
    assert read(client, f"{path}/datasets/{source['id']}")["revision"] == source["revision"]
    assert (
        read(client, f"{path}/datasets/{source['id']}/cases")[0]["lineageId"]
        == read(client, f"{path}/datasets/{copied['id']}/cases")[0]["lineageId"]
    )
    profile = send(
        client,
        path + "/evaluation-profiles",
        {
            "name": "Semantic correctness",
            "criteria": "Judge correctness against the reference. Equivalent wording is accepted.",
            "passThreshold": 0.8,
        },
    )
    request = {
        "name": "Two contracts by two versions",
        "profileVersionId": profile["versionId"],
        "selections": [
            {"datasetId": source["id"], "expectedRevision": source["revision"], "workflowVersionIds": versions[:2]},
            {"datasetId": copied["id"], "expectedRevision": copied["revision"], "workflowVersionIds": versions[2:]},
        ],
    }
    wrong = copy.deepcopy(request)
    wrong["selections"][0]["workflowVersionIds"] = versions[2:]
    send(client, path + "/evaluations", wrong, status=(422,))
    assert read(client, path + "/evaluations") == []
    # Unconfigured platform scoring must reject before accepting any batch.
    setting = read(client, "/platform-ai/model-setting")
    send(
        client,
        "/platform-ai/model-setting",
        {"expectedVersion": setting["version"], "modelAliasId": None},
        method="PUT",
    )
    send(client, path + "/evaluations", request, status=(409,))
    assert read(client, path + "/evaluations") == []
    select_model(client, state["models"][0])
    key = str(uuid.uuid4())
    batch = send(client, path + "/evaluations", request, key=key)
    assert send(client, path + "/evaluations", request, key=key)["id"] == batch["id"]
    send(client, path + "/evaluations", {**request, "name": "Conflicting request"}, key=key, status=(409,))
    select_model(client, state["models"][1])
    # Durable dispatch resumes after the API/publisher process is replaced.
    namespace = state["context"]["control_namespace"]
    run(("kubectl", "-n", namespace, "rollout", "restart", "deployment/platform-control"), timeout=60)
    run(("kubectl", "-n", namespace, "rollout", "status", "deployment/platform-control", "--timeout=180s"), timeout=200)
    report = finished(client, path, batch["id"])
    assert report["status"] == "completed", report
    assert report["modelId"] == state["models"][0]["id"] and len(report["runs"]) == 4
    for child in report["runs"]:
        assert child["passedCount"] == 1 and child["errorCount"] == 0, child
        case = child["cases"][0]
        assert case["score"] == 1 and case["reason"] and case["judgeExecutionId"]
        assert case["businessCostMicros"] == 0 and case["judgeCostMicros"] > 0
        assert case["judgeCostByCurrency"]["USD"] > 0
    send(client, f"{path}/evaluations/{batch['id']}/cancel", None)
    assert read(client, f"{path}/evaluations/{batch['id']}/report")["status"] == "completed"
    # Editing a draft cannot mutate a report's accepted immutable data snapshot.
    original_case = read(client, f"{path}/datasets/{source['id']}/cases")[0]
    send(
        client,
        f"{path}/datasets/{source['id']}/cases/{original_case['id']}",
        {
            "expectedRevision": source["revision"],
            "version": original_case["version"],
            "name": "Edited later",
            "input": {"message": "Changed"},
            "expectedOutput": {"answer": "Changed"},
        },
        method="PATCH",
    )
    frozen = read(client, f"{path}/evaluations/{batch['id']}/report")
    assert all(case["input"]["message"] == "Hello" for child in frozen["runs"] for case in child["cases"])
    send(client, path + "/evaluations", request, status=(409,))
    request["selections"][0]["expectedRevision"] += 1
    previous = profile["versionId"]
    profile = send(
        client,
        f"{path}/evaluation-profiles/{profile['id']}",
        {
            "name": profile["name"],
            "criteria": "A changed standard",
            "passThreshold": 0.9,
            "version": profile["version"],
        },
        method="PATCH",
    )
    assert profile["versionId"] != previous
    assert len(read(client, f"{path}/evaluation-profiles/{profile['id']}/versions")) == 2
    assert read(client, f"{path}/evaluations/{batch['id']}/report")["profileVersionId"] == previous
    failed = send(client, path + "/evaluations", {**request, "name": "Judge failure"})
    result = finished(client, path, failed["id"])
    assert result["status"] == "failed" and len(result["runs"]) == 4, result
    assert all(len(child["cases"]) == 1 and child["cases"][0]["judgeExecutionId"] for child in result["runs"]), result
    assert all(
        case["score"] is None and case["status"] == "error" for child in result["runs"] for case in child["cases"]
    )
    select_model(client, state["models"][0])
    cancelled = send(client, path + "/evaluations", {**request, "name": "Cancel pending batch"})
    send(client, f"{path}/evaluations/{cancelled['id']}/cancel", None)
    assert finished(client, path, cancelled["id"])["status"] == "cancelled"
    other = send(
        client,
        "/applications",
        {
            "name": "Other application",
            "slug": f"other-{uuid.uuid4().hex[:10]}",
            "workflowId": state["app"]["workflowId"],
            "visibility": "company",
        },
    )
    assert client.get(f"/api/v1/applications/{other['id']}/datasets/{source['id']}").status_code == 404
    for old in ["/datasets", "/evaluation-profiles", "/evaluations"]:
        assert client.get("/api/v1" + old).status_code == 404
    write_report(
        state["context"],
        "product/application-evaluation-api.json",
        {
            "status": "passed",
            "batchId": batch["id"],
            "report": report,
            "modelAuthorizationSeparate": True,
            "restartAndIdempotency": True,
        },
    )


def test_application_scopes_and_platform_model_reader(application_testing):
    state = application_testing
    client = state["control"]
    private = send(
        client,
        "/applications",
        {
            "name": "Private test application",
            "slug": f"private-{uuid.uuid4().hex[:12]}",
            "workflowId": state["app"]["workflowId"],
            "visibility": "private",
        },
    )
    with httpx.Client(
        base_url=state["urls"]["web"], headers={"Authorization": "Bearer " + state["reader"]["token"]}, timeout=30
    ) as reader:
        model = read(reader, "/platform-ai/model-setting")
        assert model["status"] == "ready" and model["modelAlias"] and not model["canManage"]
        assert (
            reader.put(
                "/api/v1/platform-ai/model-setting", json={"expectedVersion": model["version"], "modelAliasId": None}
            ).status_code
            == 403
        )
        assert read(reader, f"/applications/{state['reader']['applicationId']}/datasets") == []
        assert (
            reader.post(
                f"/api/v1{state['path']}/datasets",
                json={"name": "Forbidden", "workflowVersionId": state["versions"][0]},
            ).status_code
            == 403
        )
        assert reader.get(f"/api/v1/applications/{private['id']}/datasets").status_code == 403
    write_report(
        state["context"],
        "product/application-evaluation-permissions.json",
        {"status": "passed", "readOnlyModel": True, "applicationVisibility": True},
    )


def test_selected_case_export_uses_existing_view_permission_and_revision(application_testing):
    state = application_testing
    client, path = state["control"], state["path"]
    dataset = send(client, path + "/datasets", {"name": "Export selections", "workflowVersionId": state["versions"][0]})
    base = f"{path}/datasets/{dataset['id']}"
    for index in range(2):
        send(
            client,
            base + "/cases",
            {
                "expectedRevision": index,
                "name": f"Export case {index}",
                "input": {"message": str(index)},
                "expectedOutput": {"answer": str(index)},
            },
        )
    cases = read(client, base + "/cases")
    with httpx.Client(
        base_url=state["urls"]["web"], headers={"Authorization": "Bearer " + state["reader"]["token"]}, timeout=30
    ) as reader:
        response = reader.post("/api/v1" + base + "/export", json={"expectedRevision": 2, "caseIds": [cases[1]["id"]]})
        assert response.status_code == 200, response.text
        assert [json.loads(line)["name"] for line in response.text.splitlines()] == ["Export case 1"]
        response = reader.post(
            "/api/v1" + base + "/export", json={"expectedRevision": 2, "caseIds": [case["id"] for case in cases]}
        )
        assert response.status_code == 200
        assert len(response.text.splitlines()) == 2
        assert (
            reader.post(
                "/api/v1" + base + "/import", json={"expectedRevision": 2, "format": "jsonl", "content": response.text}
            ).status_code
            == 403
        )
        assert reader.get("/api/v1" + base + "/export").status_code == 405
    send(client, base + "/export", {"expectedRevision": 1, "caseIds": [cases[0]["id"]]}, status=(409,))
    for selection in [[], [cases[0]["id"], str(uuid.uuid4())]]:
        send(client, base + "/export", {"expectedRevision": 2, "caseIds": selection}, status=(422,))
    other_app = f"/applications/{state['reader']['applicationId']}"
    assert (
        client.post(
            f"/api/v1{other_app}/datasets/{dataset['id']}/export",
            json={"expectedRevision": 2, "caseIds": [cases[0]["id"]]},
        ).status_code
        == 404
    )
    assert client.post("/api/v1" + base + "/bulk-fill", json={}).status_code == 404
    assert read(client, base)["revision"] == 2
    write_report(
        state["context"],
        "product/application-dataset-export.json",
        {
            "status": "passed",
            "selection": True,
            "selectAll": True,
            "readerCanExport": True,
            "staleRevisionRejected": True,
        },
    )


def test_invocation_validation_and_atomic_dataset_authoring(application_testing):
    state = application_testing
    client, path, version = state["control"], state["path"], state["versions"][0]
    with httpx.Client(base_url=state["urls"]["runtime"], headers=dict(client.headers), timeout=30) as runtime:
        accepted = runtime.post(
            f"/gateway/v1/applications/{state['app']['slug']}/invocations",
            headers={"Idempotency-Key": str(uuid.uuid4())},
            json={"input": {"message": "Ready on entry"}, "responseMode": "async"},
        )
        assert accepted.status_code in (200, 201, 202), accepted.text
        invocation = accepted.json()["id"]
        deadline = time.monotonic() + 60
        while True:
            detail = runtime.get(f"/gateway/v1/invocations/{invocation}").json()
            if detail["status"] == "completed":
                break
            assert time.monotonic() < deadline, detail
            time.sleep(0.2)
    sample = read(client, f"{path}/invocations/{invocation}/test-sample")
    assert sample["validationErrors"] == [] and not sample["referenceConfirmed"]
    draft = {
        "name": "Ready on entry",
        "input": sample["input"],
        "expectedOutput": sample["expectedOutput"],
        "sourceInvocationId": invocation,
        "referenceConfirmed": False,
    }
    validation = send(client, path + "/test-case-validation", {"workflowVersionId": version, **draft})
    assert validation["validationErrors"] == []  # Approval is an explicit final action, not missing data.
    invalid = send(client, path + "/test-case-validation", {"workflowVersionId": version, **draft, "input": {}})
    assert any(
        error["field"] == "input/message" and error["code"] == "REQUIRED" for error in invalid["validationErrors"]
    )
    created = send(
        client,
        path + "/datasets",
        {
            "name": "Atomic invocation",
            "workflowVersionId": version,
            "initialCase": {**draft, "referenceConfirmed": True},
        },
    )
    assert created["caseCount"] == 1 and created["incompleteCount"] == 0 and created["revision"] == 1
    cases = read(client, f"{path}/datasets/{created['id']}/cases")
    assert cases[0]["name"] == draft["name"] and cases[0]["referenceConfirmed"] and not cases[0]["validationErrors"]
    # Insertion failure after inserting the dataset must roll the whole transaction back.
    send(
        client,
        path + "/datasets",
        {"name": "Must roll back", "workflowVersionId": version, "initialCase": {**draft, "name": ""}},
        status=(400, 422),
    )
    send(
        client,
        path + "/datasets",
        {"name": "Must not mismatch", "workflowVersionId": state["versions"][2], "initialCase": draft},
        status=(422,),
    )
    assert not {"Must roll back", "Must not mismatch"} & {item["name"] for item in read(client, path + "/datasets")}
    with httpx.Client(
        base_url=state["urls"]["web"], headers={"Authorization": "Bearer " + state["reader"]["token"]}, timeout=30
    ) as reader:
        response = reader.post(f"/api/v1{path}/test-case-validation", json={"workflowVersionId": version, **draft})
        assert response.status_code == 403
    write_report(
        state["context"],
        "product/invocation-dataset-authoring.json",
        {
            "status": "passed",
            "validationOnEntry": True,
            "atomicCreation": True,
            "rollback": True,
            "sourceContractEnforced": True,
            "readerDenied": True,
        },
    )


def test_dataset_file_references_and_snapshot_completeness(application_testing):
    state = application_testing
    client = state["control"]
    workflow = _passthrough_workflow(client, dict(client.headers), "Evaluation file references")
    definition = copy.deepcopy(fixture_draft(client, workflow["workflowId"])["definition"])
    file_schema = {
        "type": "object",
        "properties": {
            "artifactId": {"type": "string", "format": "uuid"},
            "fileName": {"type": "string"},
            "contentType": {"type": "string"},
            "sizeBytes": {"type": "integer"},
            "sha256": {"type": "string"},
        },
        "required": ["artifactId", "fileName", "contentType", "sizeBytes", "sha256"],
        "additionalProperties": False,
    }
    files = {
        "type": "array",
        "items": file_schema,
        "x-agentx-artifact-array": True,
        "x-agentx-content-types": ["text/plain"],
        "x-agentx-max-size-bytes": 10,
    }
    definition["start"]["inputs"]["properties"]["files"] = files
    definition["start"]["inputs"]["required"].append("files")
    definition["end"]["outputs"]["files"] = {"schema": files, "required": True, "sensitive": False}
    next(node for node in definition["nodes"] if node["type"] == "exit")["parameters"]["outputs"]["files"] = {
        "kind": "reference",
        "selector": {"namespace": "inputs", "path": ["files"]},
    }
    version = _save_version(client, workflow["workflowId"], definition)
    app = send(
        client,
        "/applications",
        {
            "name": "File evaluation",
            "slug": f"files-{uuid.uuid4().hex[:12]}",
            "workflowId": workflow["workflowId"],
            "visibility": "company",
        },
    )
    path = f"/applications/{app['id']}"
    uploaded = httpx.post(
        state["urls"]["runtime"] + "/gateway/v1/artifacts",
        headers={"Authorization": client.headers["Authorization"], "Idempotency-Key": str(uuid.uuid4())},
        files={"content": ("reference.txt", b"ok", "text/plain")},
        timeout=30,
    )
    assert uploaded.status_code in (200, 201), uploaded.text
    artifact = {**uploaded.json(), "fileName": "reference.txt"}
    dataset = send(client, path + "/datasets", {"name": "File cases", "workflowVersionId": version["id"]})
    send(
        client,
        f"{path}/datasets/{dataset['id']}/cases",
        {
            "expectedRevision": 0,
            "name": "Opaque file reference",
            "input": {"message": "file", "files": [artifact]},
            "expectedOutput": {"answer": "file", "files": [artifact]},
        },
    )
    assert read(client, f"{path}/datasets/{dataset['id']}")["incompleteCount"] == 0
    profile = send(
        client,
        path + "/evaluation-profiles",
        {
            "name": "File result criteria",
            "criteria": "Compare scalar outputs and reference metadata. Do not parse file bodies.",
            "passThreshold": 0.8,
        },
    )
    request = {
        "name": "File snapshot",
        "profileVersionId": profile["versionId"],
        "selections": [{"datasetId": dataset["id"], "expectedRevision": 1, "workflowVersionIds": [version["id"]]}],
    }
    batch = send(client, path + "/evaluations", request)
    report = finished(client, path, batch["id"])
    assert report["status"] == "completed", report
    artifact_id = uuid.UUID(artifact["artifactId"])
    _runtime_mysql(
        state["context"],
        f"UPDATE artifacts SET deleted_at=UTC_TIMESTAMP(6) WHERE id=UUID_TO_BIN('{artifact_id}');",  # noqa: S608 -- fixture UUID
    )
    assert read(client, f"{path}/datasets/{dataset['id']}")["incompleteCount"] == 1
    validation = send(
        client,
        path + "/test-case-validation",
        {
            "workflowVersionId": version["id"],
            "name": "Expired",
            "input": {"message": "file", "files": [artifact]},
            "expectedOutput": {"answer": "file", "files": [artifact]},
        },
    )
    assert any(error["code"] == "ARTIFACT_UNAVAILABLE" for error in validation["validationErrors"])
    # A reusable data revision must still recheck attachment availability.
    send(client, path + "/evaluations", request, status=(422,))
    assert len(read(client, path + "/evaluations")) == 1
    frozen = read(client, f"{path}/evaluations/{batch['id']}/report")
    assert frozen["runs"][0]["cases"][0]["expectedOutput"]["files"][0]["artifactId"] == str(artifact_id)
    write_report(
        state["context"],
        "product/application-evaluation-files.json",
        {"status": "passed", "batchId": batch["id"], "expiredReferenceRejected": True},
    )


def test_application_testing_desktop(application_testing):
    state = application_testing
    environment = {
        **os.environ,
        "AGENTX_E2E_BASE_URL": state["urls"]["web"],
        "AGENTX_E2E_RUNTIME_URL": state["urls"]["runtime"],
        "AGENTX_E2E_RUN_ID": state["context"]["run_id"],
        "AGENTX_E2E_STAGE": "application-testing",
        "AGENTX_E2E_TEST_APP": state["app"]["id"],
        "AGENTX_E2E_TEST_VERSIONS": json.dumps(state["versions"]),
        "AGENTX_E2E_TEST_READER": state["reader"]["username"],
        "AGENTX_E2E_EMPTY_APP": state["reader"]["applicationId"],
    }
    run_playwright(
        Path(state["context"]["root"]), "application-testing", ("tests/application-testing.spec.ts",), environment
    )
