"""Actual external content, workspace isolation and multi-dataset runtime search."""

import json

import httpx
import pytest

from tests.e2e.product.knowledge_agent_support import agent_knowledge_workflow, knowledge_tool_operations, query_tools
from tests.e2e.product.knowledge_native_support import knowledge_workflow, provider_client, seed_lightrag
from tests.e2e.product.knowledge_real_provider_support import knowledge_instances as knowledge_instances
from tests.e2e.product.knowledge_real_provider_support import seed_ragflow
from tests.e2e.product.remaining_support import invoke, start_execution
from tests.e2e.product.studio_builder_support import post
from tests.e2e.runtime.test_agent_attachments import _access_token
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def register(control, owner, name, provider, endpoint, secret, scope=None):
    credential = post(
        control,
        "/credentials",
        {"name": name, "credentialType": "bearer", "secret": secret, "ownerDepartmentId": owner},
    )
    connection = post(
        control,
        "/knowledge/connections",
        {
            "name": name,
            "provider": provider,
            "endpoint": endpoint,
            "credentialId": credential["id"],
            "ownerDepartmentId": owner,
        },
    )
    catalog = control.get(f"/api/v1/knowledge/connections/{connection['id']}/catalog")
    assert catalog.status_code == 200, catalog.text
    query = post(
        control,
        f"/knowledge/connections/{connection['id']}/queries",
        {
            "name": name,
            "description": "External project release codes",
            "scope": scope or catalog.json()[0]["scope"],
            "ownerDepartmentId": owner,
        },
    )
    return connection, query


def retrieve(control, query):
    response = control.post(
        f"/api/v1/knowledge/queries/{query['id']}/retrieval-test",
        json={"query": "水星计划的发布代号是什么", "topK": 20},
    )
    assert response.status_code == 200, response.text
    return response.json()


def test_real_external_scopes_and_cross_connection_workflow(
    installed_agentx, service_urls, e2e_providers, knowledge_instances, studio_live_models, run_id
):
    instances = knowledge_instances
    datasets = seed_ragflow(installed_agentx, e2e_providers, instances, run_id)
    with httpx.Client(base_url=service_urls["web"], timeout=90) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        owner = me["departmentId"]
        light_queries = []
        light_connections = []
        markers = []
        for service, workspace in (("lightrag", "alpha"), ("lightrag-b", "beta")):
            marker = f"light-{workspace}-{run_id}"
            markers.append(marker)
            with provider_client(
                instances["namespace"], service, 9621, instances["directory"], {"X-API-Key": instances["key"].value}
            ) as native:
                assert native.get("/health").json()["configuration"]["workspace"] == workspace
                seed_lightrag(
                    native, f"水星计划的发布代号是 {marker}。该计划属于 {workspace} 团队。", f"{workspace}.txt"
                )
            connection, query = register(
                control,
                owner,
                f"Real {workspace}",
                "lightrag",
                f"http://{service}.{instances['namespace']}.svc:9621",
                instances["key"].value,
            )
            assert query["scope"] == {"kind": "lightrag", "workspace": workspace}
            forged = control.post(
                f"/api/v1/knowledge/connections/{connection['id']}/queries",
                json={
                    "name": "Forged workspace",
                    "description": "Invalid scope",
                    "scope": {"kind": "lightrag", "workspace": "not-this-instance"},
                    "ownerDepartmentId": owner,
                },
            )
            assert forged.status_code == 422 and forged.json()["code"] == "RAG_SCOPE_UNAVAILABLE", forged.text
            light_queries.append(query)
            light_connections.append(connection)
        for query, marker in zip(light_queries, markers, strict=True):
            result = retrieve(control, query)
            assert marker in result["text"] and all(
                other not in result["text"] for other in markers if other != marker
            ), result
            assert len(result["sources"]) == 1 and result["partial"] is False
        connection, rag = register(
            control,
            owner,
            "Real RAGFlow datasets",
            "ragflow",
            e2e_providers["ragflow"],
            e2e_providers["ragflow_api_key"],
            {"kind": "ragflow", "datasetIds": [item["datasetId"] for item in datasets]},
        )
        rag_result = retrieve(control, rag)
        assert len(rag_result["sources"]) == 2 and all(item["marker"] in rag_result["text"] for item in datasets), (
            rag_result
        )
        overlapping = post(
            control,
            f"/knowledge/connections/{connection['id']}/queries",
            {
                "name": "First team only",
                "description": "Separately authorized team",
                "scope": {"kind": "ragflow", "datasetIds": [datasets[0]["datasetId"]]},
                "ownerDepartmentId": owner,
            },
        )
        isolated = retrieve(control, overlapping)
        assert datasets[0]["marker"] in isolated["text"] and datasets[1]["marker"] not in isolated["text"], isolated
        documents = control.get(f"/api/v1/knowledge/queries/{overlapping['id']}/documents")
        assert documents.status_code == 200 and {d["id"] for d in documents.json()["items"]} == {
            datasets[0]["documentId"]
        }, documents.text
        outside = control.get(
            f"/api/v1/knowledge/queries/{overlapping['id']}/documents", params={"datasetId": datasets[1]["datasetId"]}
        )
        assert outside.status_code == 403
        workflow = knowledge_workflow(control, f"Real multisource {run_id}", [rag, overlapping, *light_queries])
        execution, detail = invoke({"control": control}, workflow)
        result = detail["output"]
        assert len(result["sources"]) == 4 and result["partial"] is False, result
        assert all(marker in result["text"] for marker in [*markers, *(d["marker"] for d in datasets)]), result
        assert any(set(s["queryIds"]) == {rag["id"], overlapping["id"]} for s in result["sources"]), result
        assert all("fusionScore" in hit and "sources" in hit for hit in result["documents"])
        agent_runs = []
        for label, question, selected, expected_markers in (
            ("alpha-only", "只查询 alpha 团队的水星计划发布代号，不要查询其他团队。", [light_queries[0]], [markers[0]]),
            (
                "all-teams",
                "分别查询 alpha 团队、beta 团队以及 RAGFlow 两个数据集中的水星计划发布代号，完整列出四个代号。",
                [*light_queries, rag],
                [*markers, *(item["marker"] for item in datasets)],
            ),
        ):
            agent_workflow = agent_knowledge_workflow(
                control, f"Real Agent {label} {run_id}", [*light_queries, rag], studio_live_models["live"], question
            )
            agent_execution, answer = invoke({"control": control}, agent_workflow)
            operations = knowledge_tool_operations(installed_agentx, agent_execution)
            assert {op["intent"]["name"] for op in operations} == query_tools(selected), operations
            assert len(operations) == len(selected), operations
            assert all(marker in answer["output"]["answer"] for marker in expected_markers), answer
            for operation in operations:
                tool = operation["phase"]["result"]
                assert not tool["isError"] and not tool["terminate"], tool
                assert tool["structuredResult"]["sources"] and tool["structuredResult"]["partial"] is False, tool
            agent_runs.append(
                {
                    "case": label,
                    "executionId": agent_execution,
                    "selectedTools": sorted(query_tools(selected)),
                    "actualToolCalls": len(operations),
                }
            )
        # Published workflow keeps its query config while the provider indexes new content.
        version = light_queries[0]["version"]
        extra = f"externally-added-{run_id}"
        with provider_client(
            instances["namespace"], "lightrag", 9621, instances["directory"], {"X-API-Key": instances["key"].value}
        ) as native:
            seed_lightrag(native, f"水星计划新的发布代号是 {extra}。", "external-update.txt")
        updated = retrieve(control, light_queries[0])
        assert extra in updated["text"], updated
        assert control.get(f"/api/v1/knowledge/queries/{light_queries[0]['id']}").json()["version"] == version
        _, latest = invoke({"control": control}, workflow)
        assert extra in latest["output"]["text"], latest
        credential_id = light_connections[0]["credentialId"]
        grants = control.get(f"/api/v1/resources/credential/{credential_id}/grants").json()
        assert grants, "Authorizing a query must include its credential dependency"
        for grant in grants:
            revoked = control.delete(f"/api/v1/resources/credential/{credential_id}/grants/{grant['id']}")
            assert revoked.status_code == 204, revoked.text
        denied = start_execution({"control": control}, workflow)
        assert denied.status_code in {403, 422}, denied.text
        write_report(
            installed_agentx,
            "product/knowledge-native-scopes.json",
            {
                "status": "passed",
                "executionId": execution,
                "realCpuEmbedding": True,
                "externalNativeSeeding": True,
                "workspaces": ["alpha", "beta"],
                "datasets": datasets,
                "sources": result["sources"],
                "externalAddition": extra,
                "queryVersionUnchanged": version,
                "credentialRevocationStatus": denied.status_code,
                "realModelAgentRuns": agent_runs,
            },
        )
        (instances["directory"] / "retrieval.json").write_text(json.dumps(result, ensure_ascii=False, indent=2))
