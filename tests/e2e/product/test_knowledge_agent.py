"""Agent knowledge failure, partial and empty outcomes with a real model."""

import httpx
import pytest

from tests.e2e.product.knowledge_agent_support import agent_knowledge_workflow, knowledge_tool_operations, query_tools
from tests.e2e.product.remaining_support import invoke
from tests.e2e.product.studio_builder_support import post
from tests.e2e.runtime.test_agent_attachments import _access_token, _runtime_mysql
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


@pytest.mark.parametrize(
    ("provider", "search"),
    [
        ("ragflow", "fixture-partial"),
        ("ragflow", "fixture-empty"),
        ("ragflow", "fixture-error"),
        ("lightrag", "fixture-error"),
    ],
)
def test_agent_knowledge_partial_empty_and_failed_error_outlet(
    installed_agentx, service_urls, studio_provider, studio_live_models, run_id, provider, search
):
    with httpx.Client(base_url=service_urls["web"], timeout=90) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        scope = (
            {"kind": "ragflow", "datasetIds": ["dataset-a", "dataset-b"]}
            if provider == "ragflow"
            else {"kind": "lightrag", "workspace": ""}
        )
        connection = post(
            control,
            "/knowledge/connections",
            {
                "name": f"Agent outcomes {provider} {search} {run_id}",
                "provider": provider,
                "endpoint": studio_provider.removesuffix("/v1"),
                "ownerDepartmentId": me["departmentId"],
            },
        )
        query = post(
            control,
            f"/knowledge/connections/{connection['id']}/queries",
            {
                "name": f"Project {provider}",
                "description": "Project launch questions",
                "scope": scope,
                "ownerDepartmentId": me["departmentId"],
            },
        )
        workflow = agent_knowledge_workflow(
            control,
            f"Agent {provider} {search} {run_id}",
            [query],
            studio_live_models["live"],
            f'必须调用知识工具一次，参数 query 严格等于 "{search}"，topK 为 5；禁止修改检索词。得到工具返回后简短报告结果，不重试。',
        )
        failed = search == "fixture-error"
        execution, result = invoke({"control": control}, workflow, expected="failed" if failed else "succeeded")
        operations = knowledge_tool_operations(installed_agentx, execution)
        assert len(operations) == 1 and operations[0]["intent"]["name"] in query_tools([query]), {
            "executionId": execution,
            "result": result,
            "operations": operations,
        }
        assert operations[0]["intent"]["arguments"]["query"] == search, operations
        tool = operations[0]["phase"]["result"]
        payload = tool["structuredResult"]
        model_calls = int(
            _runtime_mysql(
                installed_agentx,
                f"SELECT COUNT(*) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{execution}') AND call_kind='model'",  # noqa: S608
            )
        )
        if failed:
            assert tool["isError"] and tool["terminate"], tool
            assert model_calls == 1, "Knowledge failure must stop the model before it can answer"
            assert payload["code"] == "KNOWLEDGE_ALL_SOURCES_FAILED", payload
            assert result["error"]["outputs"]["code"] == payload["code"], result
            assert result["error"]["outputs"]["details"]["sources"] == payload["sources"], result
            assert all(source["status"] == "failed" and source["error"] for source in payload["sources"]), payload
        else:
            assert not tool["isError"] and not tool["terminate"], tool
            assert model_calls >= 2
            assert payload["partial"] is (search == "fixture-partial"), payload
            assert bool(payload["documents"]) is (search != "fixture-empty"), payload
        assert all(source["queryIds"] == [query["id"]] and "elapsedMs" in source for source in payload["sources"]), (
            payload
        )
    write_report(
        installed_agentx,
        f"product/knowledge-agent-{provider}-{search}.json",
        {
            "status": "passed",
            "realModel": True,
            "provider": provider,
            "query": search,
            "executionId": execution,
            "modelCalls": model_calls,
            "sources": payload["sources"],
            "executionStatus": result["status"],
        },
    )
