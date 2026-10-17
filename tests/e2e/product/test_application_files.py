"""API callers can download explicit output files without obtaining Trace access."""

from __future__ import annotations

import hashlib
import time
import uuid

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.live_text_support import post
from tests.e2e.product.test_channel_delivery import _passthrough_workflow
from tests.e2e.product.test_live_memory_pipeline import _save_version
from tests.e2e.product.test_live_text_acceptance import terminal
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.runtime.test_agent_attachments import _access_token
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.gateway]


def test_application_output_file_download_authorization(installed_agentx, service_urls, run_id):
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, _ = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        workflow = _passthrough_workflow(control, dict(control.headers), f"Output files {run_id}")["workflowId"]
        definition = fixture_draft(control, workflow)["definition"]
        file_schema = {
            "type": "object",
            "properties": {
                "artifactId": {"type": "string", "format": "uuid"},
                "fileName": {"type": "string"},
                "contentType": {"type": "string"},
                "sizeBytes": {"type": "integer", "minimum": 0},
                "sha256": {"type": "string"},
            },
            "required": ["artifactId", "fileName", "contentType", "sizeBytes", "sha256"],
            "additionalProperties": False,
        }
        files_schema = {"type": "array", "items": file_schema, "x-agentx-artifact-array": True}
        definition["start"]["inputs"]["properties"]["files"] = files_schema
        definition["start"]["inputs"]["required"].append("files")
        definition["end"]["outputs"]["files"] = {"schema": files_schema, "required": True, "sensitive": False}
        exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
        exit_node["parameters"]["outputs"]["files"] = {
            "kind": "reference",
            "selector": {"namespace": "inputs", "path": ["files"]},
        }
        version = _save_version(control, workflow, definition)
        environment = next(item for item in control.get("/api/v1/environments").json() if item["code"] == "development")
        applications = []
        for index in range(2):
            app = post(
                control,
                "/applications",
                {
                    "workflowId": workflow,
                    "name": f"Files {index} {run_id}",
                    "slug": f"files-{index}-{run_id}",
                    "visibility": "company",
                },
            )
            _publish_application_deployment(control, dict(control.headers), app["id"], version["id"], environment["id"])
            keys = [
                post(control, f"/applications/{app['id']}/api-keys", {"name": f"File caller {n}"})
                for n in range(2 if index == 0 else 1)
            ]
            applications.append((app, keys))
        app, keys = applications[0]
        with httpx.Client(base_url=service_urls["runtime"], timeout=60) as gateway:
            gateway.headers["Authorization"] = f"Bearer {keys[0]['secret']}"
            content = "订单,金额\n合成测试,558.00\n".encode()
            uploaded = gateway.post(
                "/gateway/v1/artifacts",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                files={"content": ("订单.csv", content, "text/csv")},
            )
            uploaded.raise_for_status()
            file = {**uploaded.json(), "fileName": "订单.csv"}
            editing = control.post(f"/api/v1/workflows/{workflow}/versions/{version['id']}/edit")
            editing.raise_for_status()
            draft = editing.json()["document"]
            assert draft["definition"]["start"]["inputs"]["properties"]["files"]["x-agentx-artifact-array"]
            debug = control.post(
                f"/api/v1/workflows/{workflow}/drafts/{draft['id']}/debug-executions",
                headers={"Idempotency-Key": str(uuid.uuid4())},
                json={
                    "expectedRevision": draft["revision"],
                    "mode": "full",
                    "input": {"message": "draft input file", "files": [file]},
                    "context": {},
                    "overlayIds": [],
                    "sideEffectDecisions": {},
                },
            )
            debug.raise_for_status()
            debug_execution = debug.json()["executionId"]
            deadline = time.monotonic() + 90
            while True:
                detail = control.get(f"/api/v1/executions/{debug_execution}").json()
                if detail["status"] in {"succeeded", "failed"}:
                    assert detail["status"] == "succeeded", detail
                    break
                assert time.monotonic() < deadline
                time.sleep(0.5)
            debug_file = control.get(f"/api/v1/executions/{debug_execution}/artifacts/{file['artifactId']}")
            debug_file.raise_for_status()
            assert debug_file.content == content

            def invoke(files):
                response = gateway.post(
                    f"/gateway/v1/applications/{app['slug']}/invocations",
                    json={"input": {"message": "file download", "files": files}},
                    headers={"Idempotency-Key": str(uuid.uuid4())},
                )
                response.raise_for_status()
                result = terminal(gateway, response.json()["id"])
                assert result["status"] == "completed", result
                return result

            invocation = invoke([file])
            empty = invoke([])
            path = f"/gateway/v1/invocations/{invocation['id']}/artifacts/{file['artifactId']}"
            response = gateway.get(path)
            response.raise_for_status()
            assert response.content == content
            assert response.headers["content-type"] == "text/csv"
            assert response.headers["etag"] == f'"sha256:{hashlib.sha256(content).hexdigest()}"'
            assert gateway.get(path, headers={"Authorization": ""}).status_code == 401
            unrelated_path = f"/gateway/v1/invocations/{empty['id']}/artifacts/{file['artifactId']}"
            assert gateway.get(unrelated_path).status_code == 404
            gateway.headers["Authorization"] = f"Bearer {applications[1][1][0]['secret']}"
            assert gateway.get(path).status_code == 404
            gateway.headers["Authorization"] = f"Bearer {keys[1]['secret']}"
            assert gateway.get(path).content == content
            revoked = control.post(f"/api/v1/applications/{app['id']}/api-keys/{keys[0]['id']}/revoke")
            assert revoked.is_success, revoked.text
            gateway.headers["Authorization"] = f"Bearer {keys[0]['secret']}"
            deadline = time.monotonic() + 60
            while gateway.get(path).status_code != 401:
                assert time.monotonic() < deadline, "Revoked API Key remained able to download files"
                time.sleep(0.5)
        write_report(
            installed_agentx,
            "product/application-file-download.json",
            {
                "status": "passed",
                "invocationId": invocation["id"],
                "artifactId": file["artifactId"],
                "contentVerified": True,
                "draftInputFileReferenceRetained": True,
                "unrelatedInvocationDenied": True,
                "crossApplicationDenied": True,
                "anonymousDenied": True,
                "revokedKeyDenied": True,
                "sameApplicationAlternateKeyAllowed": True,
            },
        )
