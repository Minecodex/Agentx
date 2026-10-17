"""Execute and inspect knowledge tools through the actual Agent model loop."""

import json

from tests.e2e.product.knowledge_native_support import knowledge_definition
from tests.e2e.product.next_batch_support import get
from tests.e2e.product.remaining_support import authorize, reference, save_version
from tests.e2e.product.studio_builder_support import post
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql


def agent_knowledge_workflow(control, name, queries, model, question):
    workflow = post(control, "/workflows", {"name": name, "visibility": "company"})["id"]
    for query in queries:
        authorize(control, workflow, reference("rag", query["id"], operation="read"))
    model = get(control, f"/models/aliases/{model['id']}")
    model_ref = reference("model", model["id"], model["deploymentId"])
    authorize(control, workflow, model_ref)
    model_ref["bindingRole"] = "model"
    definition = knowledge_definition(control, workflow, queries)
    agent = next(node for node in definition["nodes"] if node["id"] == "retrieve")
    agent.update(type="agent", typeVersion=2, name="Search selected knowledge")
    agent["settings"] = {"timeoutMs": 180000}
    agent["parameters"] = {
        "systemPrompt": {
            "kind": "literal",
            "value": "根据知识工具的业务名称和用途选择来源。必须实际检索后回答，不猜测。只调用与问题相关的工具，每个相关工具最多调用一次。保留检索中的完整发布代号。",
        },
        "userQuestion": {"kind": "literal", "value": question},
        "sessionPolicy": {"mode": "invocation"},
        "maxIterations": 8,
        "maxModelCalls": 8,
        "maxToolCalls": 8,
        "maxTotalTokens": 24000,
        "maxOutputTokens": 2048,
        "maxCost": 2,
        "maxDurationSeconds": 180,
        "limitAction": "fail",
    }
    agent["resourceReferences"] = [
        model_ref,
        *(reference("rag", query["id"], operation="read", role="knowledge") for query in queries),
    ]
    exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
    exit_node["parameters"] = {
        "outputs": {
            "answer": {
                "kind": "reference",
                "selector": {"namespace": "outputs", "sourceNodeId": "retrieve", "path": ["text"]},
            }
        },
        "errorOutputs": {
            key: {"kind": "reference", "selector": {"namespace": "item", "path": [key]}}
            for key in ("code", "message", "details")
        },
    }
    definition["end"]["outputs"] = {"answer": {"schema": {"type": "string"}, "required": True, "sensitive": False}}
    definition["end"]["error"]["outputs"] = {
        key: {"schema": {"type": "object" if key == "details" else "string"}, "required": True, "sensitive": False}
        for key in ("code", "message", "details")
    }
    definition["connections"].append(
        {
            "id": "knowledge-error",
            "sourceNodeId": "retrieve",
            "targetNodeId": exit_node["id"],
            "sourceHandle": "error",
            "targetHandle": "error",
            "order": 0,
        }
    )
    save_version(control, workflow, definition)
    return workflow


def knowledge_tool_operations(context, execution):
    # Execution UUID is returned by the authenticated API, never user SQL.
    # HEX preserves nested JSON through the MySQL CLI's batch escaping.
    rows = _runtime_mysql(
        context,
        f"SELECT HEX(o.operation_json) FROM agent_session_operations o JOIN agent_runs a ON a.attempt_id=o.attempt_id WHERE a.execution_id=UUID_TO_BIN('{execution}') AND JSON_UNQUOTE(JSON_EXTRACT(o.operation_json,'$.operationKind'))='tool' ORDER BY o.created_at,o.operation_id",  # noqa: S608
    )
    return [
        operation
        for line in rows.splitlines()
        if (operation := json.loads(bytes.fromhex(line)))["operationKind"] == "tool"
        and operation["intent"]["name"].startswith("knowledge_search_")
    ]


def query_tools(queries):
    return {f"knowledge_search_{query['id'].replace('-', '')}" for query in queries}
