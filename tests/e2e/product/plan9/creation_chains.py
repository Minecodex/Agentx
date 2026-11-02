"""Business requirements for four genuine canvas-AI -> page-AI creation chains."""

from __future__ import annotations

import copy
import json

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.next_batch_support import get
from tests.e2e.product.studio_builder_support import post
from tests.e2e.product.test_live_memory_pipeline import _authorize_definition
from tests.e2e.product.test_provider_integration import _publish_application_deployment

SCENES = ("S08", "S12", "S14", "S19")


def prepare(suite):
    control, blueprints = suite["control"], []
    for scene in SCENES:
        case = next(case for case in suite["cases"] if case["scene"] == scene)
        source = get(control, f"/workflows/{case['workflowId']}/versions/{case['versionId']}")["definition"]
        created = post(
            control, "/workflows", {"name": f"{scene} AI creation {suite['runId']}", "visibility": "company"}
        )
        document = fixture_draft(control, created["id"])
        # Grant existing configured resources before authoring. The Agent still
        # discovers and binds them through its normal read-only resource tools.
        _authorize_definition(control, created["id"], source)
        requirements = {
            "inputSchema": source["start"]["inputs"],
            "outputSchema": {key: value["schema"] for key, value in source["end"]["outputs"].items()},
        }
        prompt = (
            "从当前空白画布创建完整业务工作流。只使用当前画布编辑工具、node_catalog 和 resource_query，先查真实节点协议再配置，保留受保护 Exit。完成所有连接、Start 输入 Schema、End 输出 Schema、实际输出绑定和静态校验。不要执行或发布，不创建假业务结果，不向用户提问。业务数据只在未来运行时提供。业务输入输出必须严格符合："
            + json.dumps(requirements, ensure_ascii=False)
        )
        if scene == "S08":
            prompt += "。收到 request_id 后，使用 await_input 节点等待员工补充 date、contact、email；responseSchema 与 delivery 输出字段一致，校验日期和邮箱。输出原 request_id 和真实等待结果 delivery，不能从初始输入虚构补充值。"
        elif scene == "S12":
            child = next(node for node in source["nodes"] if node["type"] == "sub_workflow")
            prompt += f"。复用已发布资料收集子工作流版本 {child['parameters']['workflowVersionId']}，使用 sub_workflow 节点传入原 request_id。子工作流会真实等待 delivery 资料；父流程输出原 request_id 与子流程实际 delivery。不要把子工作流的等待替换为字面量。"
        elif scene == "S14":
            manager = suite["users"]["manager"]["id"]
            prompt += f"。经理用户 {manager} 审核报销，使用 approval 节点，两个按钮 id 分别 approved/rejected，中文名批准/拒绝，时限600000ms。将真实 decision:approved 与 decision:rejected 端口分别连接到批准和拒绝 Exit；两条分支都正式完成，输出原 request_id、amount 和对应 decision 字符串。不可绕过真实审批。"
        else:
            resources = [reference for node in source["nodes"] for reference in node.get("resourceReferences", [])]
            endpoint = next(node for node in source["nodes"] if node["id"] == "report")["parameters"]["url"]["value"]
            prompt += (
                "。先以 await_input 等待 contact、email（必填，邮箱格式正确）。然后使用真实 Agent v2，挂载已授权模型与 CRM 工具，不能用普通 MCP 节点代替。Agent 将 request_id 映射为 CRM reference，按 customer、grossAmount、discountPercent 真实创建报价；平台具体工具审批会挂起并恢复它。Agent 成功后返回真实 CRM structuredContent 的纯 JSON，禁止伪造 receiptId。接着 Code Python 节点解析该 JSON，输出 receipt_id、customer、total、discount_percent、reference；将 CRM 原始 receiptId/discountPercent 映射为上述下划线字段，另外生成包含这些字段的 summary JSON 字符串。最后 declarative_http GET "
                + endpoint
                + "，query.summary 绑定刚生成的 summary，交付其 files Artifact 数组。应用输出只使用真实凭证和报告文件。可用资源身份（仍须 resource_query 检查绑定与授权）为："
                + json.dumps(resources, ensure_ascii=False)
            )
        blueprints.append(
            {
                "scene": scene,
                "workflowId": created["id"],
                "versionId": created["createdVersionId"],
                "draftId": document["id"],
                "prompt": prompt,
                "attemptLimit": 3,
            }
        )
        if scene == "S08":
            blueprints[-1]["steeringNodeName"] = "交付资料补充"
    path = suite["directory"] / "workflow-ai-requirements.json"
    path.write_text(json.dumps(blueprints, ensure_ascii=False, indent=2))
    return path


def bind_wait_input_paths(case, source, generated):
    """Bind held-out values to the immutable AI graph, never to fixture-only node IDs."""

    def escape(value):
        return value.replace("~", "~0").replace("/", "~1")

    bindings = {}
    for original in source["nodes"]:
        if original["type"] not in {"await_input", "sub_workflow"}:
            continue
        candidates = [
            node
            for node in generated["nodes"]
            if node["type"] == original["type"] and not node.get("disabled") and not node.get("parentId")
        ]
        matching_key = [node for node in candidates if node["key"] == original["key"]]
        if matching_key:
            candidates = matching_key
        assert len(candidates) == 1, f"Ambiguous or missing {original['type']} business role"
        actual = candidates[0]
        if original["type"] == "sub_workflow":
            assert actual["parameters"]["workflowVersionId"] == original["parameters"]["workflowVersionId"], (
                "The AI must reuse the specified immutable child workflow"
            )
        else:
            expected = original["parameters"]["responseSchema"]
            received = actual["parameters"]["responseSchema"]
            assert expected.get("type") == received.get("type"), "The input wait changed its business type"
            assert set(expected.get("properties", {})) == set(received.get("properties", {})), (
                "The input wait changed its business fields"
            )
        bindings[f"/{escape(original['id'])}"] = f"/{escape(actual['id'])}"
    mapped = []
    for inputs in case.get("waitInputs", []):
        current = {}
        for path, values in inputs.items():
            matches = [prefix for prefix in bindings if path == prefix or path.startswith(prefix + "/")]
            assert len(matches) == 1, f"No unique immutable workflow role for {path}"
            prefix = matches[0]
            current[bindings[prefix] + path[len(prefix) :]] = values
        mapped.append(current)
    return mapped, [{"fixturePrefix": key, "publishedNodePrefix": value} for key, value in bindings.items()]


def publish_applications(suite, generated, *, iteration=0, directory=None, page_attempt_budget=3):
    control, cases = suite["control"], []
    environment = next(value for value in get(control, "/environments") if value["code"] == "development")
    for result in generated:
        assert result["passed"], result
        original = next(case for case in suite["cases"] if case["scene"] == result["scene"])
        case = copy.deepcopy(original)
        workflow, version = result["workflowId"], result["publishedVersionId"]
        source = get(control, f"/workflows/{original['workflowId']}/versions/{original['versionId']}")["definition"]
        definition = get(control, f"/workflows/{workflow}/versions/{version}")["definition"]
        if case.get("waitInputs"):
            case["waitInputs"], case["waitInputBindings"] = bind_wait_input_paths(case, source, definition)
        application = post(
            control,
            "/applications",
            {
                "workflowId": workflow,
                "name": f"{result['scene']} AI creation chain {suite['runId']}",
                "slug": f"p9-{result['scene'].lower()}-ai{iteration}-{suite['runId']}",
                "visibility": "company",
            },
        )
        deployment = _publish_application_deployment(
            control, dict(control.headers), application["id"], version, environment["id"]
        )
        case.update(
            scene=f"{result['scene']}-chain",
            applicationId=application["id"],
            slug=application["slug"],
            workflowId=workflow,
            versionId=version,
            deploymentId=deployment["id"],
            creationChain=True,
            pageAttemptBudget=page_attempt_budget,
        )
        if result["scene"] == "S08":
            case["steeringTitle"] = "交付安排服务台"
        cases.append(case)
    path = (directory or suite["directory"]) / "creation-chain-scenarios.json"
    path.write_text(json.dumps(cases, ensure_ascii=False, indent=2))
    return path
