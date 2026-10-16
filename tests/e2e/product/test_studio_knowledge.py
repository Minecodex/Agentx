"""Knowledge canvas and resource completion through the shared E2E lifecycle."""

import json
import os
import time
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.studio_builder_support import post
from tests.e2e.product.test_studio_builder import capture_builder_evidence
from tests.e2e.runtime.test_agent_attachments import (
    _access_token,
    _control_mysql,
    _runtime_mysql,
)
from tests.e2e.support import run, run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def agent_resources(client, owner, endpoint):
    server = post(
        client,
        "/mcp/servers",
        {
            "name": "P8 MCP",
            "ownerDepartmentId": owner,
            "transport": {"kind": "streamable_http", "endpoint": f"{endpoint}/mcp"},
            "configuration": {},
        },
    )
    tool = next(
        item for item in post(client, f"/mcp/servers/{server['id']}/discover", {})["tools"] if item["name"] == "echo"
    )
    connection = post(
        client,
        "/memory/connections",
        {
            "name": "P8 Memory",
            "provider": "mem0",
            "endpoint": f"{endpoint}/memory",
            "ownerDepartmentId": owner,
            "configuration": {},
        },
    )
    memory = post(
        client,
        "/memory/namespaces",
        {
            "name": "P8 Memory",
            "connectionId": connection["id"],
            "externalNamespace": "p8_knowledge",
            "accessMode": "read_write",
            "ownerDepartmentId": owner,
        },
    )
    skill = post(
        client,
        "/skills",
        {
            "name": "P8 Skill",
            "alias": "p8-knowledge-skill",
            "description": "Resource matching test",
            "ownerDepartmentId": owner,
        },
    )
    version = post(
        client, f"/skills/{skill['id']}/versions", {"expectedRevision": skill["draftRevision"], "dependencies": []}
    )
    activated = client.patch(
        f"/api/v1/skills/{skill['id']}",
        json={
            "name": skill["name"],
            "alias": skill["alias"],
            "description": skill["description"],
            "status": "active",
            "version": skill["version"],
        },
    )
    assert activated.status_code == 200, activated.text
    return {"tool": tool["id"], "memory": memory["id"], "skill": skill["id"], "skillVersion": version["id"]}


def test_knowledge_canvas_and_ai_resource_completion(
    installed_agentx, service_urls, studio_live_models, studio_provider, studio_users
):
    studio_models = studio_live_models
    resources = {}
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as client:
        token, me = _access_token(client)
        client.headers.update({"Authorization": f"Bearer {token}"})
        for provider in ("lightrag", "ragflow"):
            name = f"P8 Knowledge {provider}"
            connection = post(
                client,
                "/knowledge/connections",
                {
                    "name": name,
                    "provider": provider,
                    "endpoint": studio_provider.removesuffix("/v1"),
                    "healthPath": "/health",
                    "ownerDepartmentId": me["departmentId"],
                    "configuration": {},
                },
            )
            resources[provider] = post(
                client,
                "/knowledge/resources",
                {
                    "connectionId": connection["id"],
                    "name": name,
                    "externalResourceId": "p8_knowledge",
                    "ownerDepartmentId": me["departmentId"],
                },
            )
        attachments = agent_resources(client, me["departmentId"], studio_provider.removesuffix("/v1"))
    try:
        run_playwright(
            Path(installed_agentx["root"]),
            "studio-knowledge",
            (
                "tests/studio-builder-knowledge.spec.ts",
                "tests/studio-builder-inspectors.spec.ts",
                "tests/workflow-multi-exit.spec.ts",
                "--grep",
                os.environ.get("AGENTX_E2E_PLAYWRIGHT_GREP")
                or "P8 knowledge|inspector fixture knowledge_retrieval|multi exit workflow",
            ),
            {
                **os.environ,
                "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
                "AGENTX_E2E_STAGE": "helm-agentxctl",
                "AGENTX_E2E_BASE_URL": service_urls["web"],
                "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
                "AGENTX_E2E_STUDIO_FIXTURE_ALIAS": studio_models["fixture"]["alias"],
                "AGENTX_E2E_STUDIO_LIVE_ALIAS": studio_models["live"]["alias"],
                "AGENTX_E2E_STUDIO_KNOWLEDGE_LIGHTRAG": resources["lightrag"]["id"],
                "AGENTX_E2E_STUDIO_KNOWLEDGE_RAGFLOW": resources["ragflow"]["id"],
                "AGENTX_E2E_STUDIO_AGENT_RESOURCES": json.dumps(attachments),
            },
        )
    finally:
        capture_builder_evidence(installed_agentx)
    verify_failed_execution_recovery(installed_agentx, service_urls, studio_users)


def verify_failed_execution_recovery(installed_agentx, service_urls, studio_users):
    """An isolated lost projection must be restored without rewriting Execution."""
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as client:
        token, _ = _access_token(client)
        client.headers.update({"Authorization": f"Bearer {token}"})
        disabled = client.post(f"/api/v1/users/{studio_users['viewer']['id']}/disable")
        assert disabled.status_code == 204, disabled.text
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            pending = _control_mysql(
                installed_agentx,
                "SELECT COUNT(*) FROM outbox WHERE aggregate_type='runtime_user_admission' AND status IN ('pending','processing','failed')",
            )
            if pending == "0":
                break
            time.sleep(1)
        else:
            raise AssertionError("User admission outbox did not settle before recovery injection")
        expected_snapshot = _control_mysql(
            installed_agentx, "SELECT CONCAT(HEX(id),':',status,':',token_version) FROM users ORDER BY id"
        )
        target = json.loads(
            _runtime_mysql(
                installed_agentx,
                "SELECT JSON_OBJECT('executionId',BIN_TO_UUID(e.id),'workflowId',BIN_TO_UUID(e.workflow_id),'draftId',JSON_UNQUOTE(JSON_EXTRACT(s.execution_context_json,'$.workflow.source.draftId'))) FROM workflow_executions e JOIN execution_snapshots s ON s.execution_id=e.id WHERE e.status='failed' AND e.error_code='PROVIDER_REJECTED' ORDER BY e.created_at DESC LIMIT 1",
            )
        )
        detail_path = f"/api/v1/executions/{target['executionId']}"
        before = client.get(detail_path)
        assert before.status_code == 200, before.text
        expected = int(_control_mysql(installed_agentx, "SELECT COUNT(*) FROM users"))
        _runtime_mysql(installed_agentx, "DELETE FROM runtime_user_admission")
        assert client.get(detail_path).status_code == 403, "Missing projection must not bypass authorization"
        namespace = installed_agentx["control_namespace"]
        run(("kubectl", "-n", namespace, "rollout", "restart", "deployment/platform-control"))
        run(
            ("kubectl", "-n", namespace, "rollout", "status", "deployment/platform-control", "--timeout=180s"),
            timeout=200,
        )
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            if int(_runtime_mysql(installed_agentx, "SELECT COUNT(*) FROM runtime_user_admission")) == expected:
                break
            time.sleep(1)
        else:
            raise AssertionError("User admission projection was not restored")
        restored_snapshot = _runtime_mysql(
            installed_agentx,
            "SELECT CONCAT(HEX(user_id),':',status,':',token_version) FROM runtime_user_admission ORDER BY user_id",
        )
        assert restored_snapshot == expected_snapshot, "Reconciliation must preserve disabled users and token versions"
        after = client.get(detail_path)
        assert after.status_code == 200, after.text
        assert after.json() == before.json(), "Restoring query identity must preserve the execution"
        for suffix in ("/nodes", "/events?after=0&limit=200", "/checkpoints", "/runtime-details"):
            response = client.get(detail_path + suffix)
            assert response.status_code == 200, response.text
        (Path(installed_agentx["artifact_dir"]) / "admission-recovery.json").write_text(
            json.dumps({"target": target, "restoredUsers": expected, "executionUnchanged": True})
        )
    run_playwright(
        Path(installed_agentx["root"]),
        "studio-recovered-execution",
        ("tests/studio-builder-knowledge.spec.ts", "--grep", "P8 recovered execution"),
        {
            **os.environ,
            "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
            "AGENTX_E2E_STAGE": "helm-agentxctl",
            "AGENTX_E2E_BASE_URL": service_urls["web"],
            "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
            "AGENTX_E2E_RECOVERED_EXECUTION": json.dumps(target),
        },
    )
