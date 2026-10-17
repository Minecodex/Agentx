"""Knowledge connection/query acceptance through the standard Kubernetes lifecycle."""

import os
from pathlib import Path

import pytest

from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_knowledge_connection_desktop(installed_agentx, service_urls, studio_provider):
    run_playwright(
        Path(installed_agentx["root"]),
        "knowledge-connections",
        ("tests/knowledge-management.spec.ts",),
        {
            **os.environ,
            "AGENTX_E2E_BASE_URL": service_urls["web"],
            "AGENTX_E2E_RUN_ID": installed_agentx["run_id"],
            "AGENTX_E2E_STAGE": "helm-agentxctl",
            "AGENTX_E2E_KNOWLEDGE_ENDPOINT": studio_provider.removesuffix("/v1"),
        },
    )


def test_multisource_partial_empty_failure_and_frozen_scope(installed_agentx, service_urls, studio_provider, run_id):
    import json

    import httpx

    from tests.e2e.product.knowledge_native_support import knowledge_workflow
    from tests.e2e.product.remaining_support import invoke, start_execution
    from tests.e2e.product.studio_builder_support import post
    from tests.e2e.runtime.test_agent_attachments import _access_token, _runtime_mysql
    from tools.scripts.release.evidence import write_report

    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        owner = me["departmentId"]
        queries = []
        for provider, scope in (
            ("ragflow", {"kind": "ragflow", "datasetIds": ["dataset-a", "dataset-b"]}),
            ("lightrag", {"kind": "lightrag", "workspace": ""}),
        ):
            connection = post(
                control,
                "/knowledge/connections",
                {
                    "name": f"Sources {provider} {run_id}",
                    "provider": provider,
                    "endpoint": studio_provider.removesuffix("/v1"),
                    "ownerDepartmentId": owner,
                },
            )
            query = post(
                control,
                f"/knowledge/connections/{connection['id']}/queries",
                {"name": "Project search", "description": "Launch project", "scope": scope, "ownerDepartmentId": owner},
            )
            queries.append(query)
            if provider == "ragflow":
                overlap = post(
                    control,
                    f"/knowledge/connections/{connection['id']}/queries",
                    {
                        "name": "Support search",
                        "description": "Same dataset, separate grants",
                        "scope": {"kind": "ragflow", "datasetIds": ["dataset-a"]},
                        "ownerDepartmentId": owner,
                    },
                )
                duplicate = control.post(
                    f"/api/v1/knowledge/connections/{connection['id']}/queries",
                    json={
                        "name": query["name"],
                        "description": "Duplicate",
                        "scope": scope,
                        "ownerDepartmentId": owner,
                    },
                )
                assert duplicate.status_code == 409 and duplicate.json()["code"] == "KNOWLEDGE_NAME_EXISTS", (
                    duplicate.text
                )
        queries.append(overlap)
        outcomes = []
        for text, expected in (
            ("fixture-partial", "succeeded"),
            ("fixture-empty", "succeeded"),
            ("fixture-error", "failed"),
        ):
            workflow = knowledge_workflow(control, f"Sources {text} {run_id}", queries, text)
            execution, detail = invoke({"control": control}, workflow, expected=expected)
            if text == "fixture-error":
                assert "KNOWLEDGE_ALL_SOURCES_FAILED" in json.dumps(detail), detail
            else:
                result = detail["output"]
                assert len(result["sources"]) == 3, result
                assert result["partial"] is (text == "fixture-partial"), result
                assert len([s for s in result["sources"] if s["status"] == "failed"]) == (
                    1 if text == "fixture-partial" else 0
                )
                assert any(set(s["queryIds"]) == {queries[0]["id"], overlap["id"]} for s in result["sources"]), result
                if text == "fixture-empty":
                    assert result["documents"] == [] and result["files"] == [] and result["text"] == "", result
                else:
                    assert result["documents"] and all("fusionScore" in d for d in result["documents"])
            outcomes.append({"executionId": execution, "query": text, "status": expected})
        query = queries[0]
        frozen_sql = f"SELECT configuration_json FROM runtime_resource_bindings WHERE resource_kind='rag' AND resource_id=UUID_TO_BIN('{query['id']}') ORDER BY created_at LIMIT 1"  # noqa: S608 -- server-issued UUID
        frozen = json.loads(_runtime_mysql(installed_agentx, frozen_sql))
        assert frozen["scope"] == query["scope"] and frozen["queryVersion"] == query["version"], frozen
        changed = control.patch(
            f"/api/v1/knowledge/queries/{query['id']}",
            json={
                "name": query["name"],
                "description": "Changed after package freeze",
                "scope": {"kind": "ragflow", "datasetIds": ["dataset-b"]},
                "status": "active",
                "ownerDepartmentId": owner,
                "version": query["version"],
            },
        )
        assert changed.status_code == 200, changed.text
        assert json.loads(_runtime_mysql(installed_agentx, frozen_sql)) == frozen
        revised = knowledge_workflow(control, f"Revised scope {run_id}", [changed.json()], "fixture-empty")
        _, revised_detail = invoke({"control": control}, revised)
        assert len(revised_detail["output"]["sources"]) == 1, revised_detail
        assert revised_detail["output"]["sources"][0]["scope"] == changed.json()["scope"], revised_detail
        assert json.loads(_runtime_mysql(installed_agentx, frozen_sql)) == frozen
        blocked = control.delete(f"/api/v1/knowledge/queries/{query['id']}")
        assert blocked.status_code == 409, blocked.text
        # Revoke every grant for this query; admission must refuse before provider requests.
        grants = control.get(f"/api/v1/resources/rag/{query['id']}/grants").json()
        assert grants
        for grant in grants:
            revoked = control.delete(f"/api/v1/resources/rag/{query['id']}/grants/{grant['id']}")
            assert revoked.status_code == 204, revoked.text
        denied = start_execution({"control": control}, workflow)
        assert denied.status_code in {403, 422}, denied.text
        write_report(
            installed_agentx,
            "product/knowledge-multisource.json",
            {
                "status": "passed",
                "outcomes": outcomes,
                "frozenScope": frozen["scope"],
                "queryVersion": frozen["queryVersion"],
                "revocationStatus": denied.status_code,
            },
        )
