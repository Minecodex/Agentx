"""Real configured-model Agents with concrete, non-idempotent CRM effects."""

from __future__ import annotations

import time

from tests.e2e.draft_fixtures import idempotent_headers
from tests.e2e.product.next_batch_support import get
from tests.e2e.product.plan9.file_scenarios import OUTPUT_FILE, report_node
from tests.e2e.product.plan9.scenarios import reference
from tests.e2e.product.plan9.workflow_builders import code, literal, node, object_schema, output_binding, publish, wait
from tests.e2e.product.studio_builder_support import post


def agent(model, prompt, question, *, tool=None, session=False):
    resources = [
        {
            "resourceType": "model",
            "resourceId": model["id"],
            "resourceVersionId": model["deploymentId"],
            "bindingRole": "model",
            "operation": "use",
        }
    ]
    if tool:
        resources.append(
            {
                "resourceType": "mcp_tool",
                "resourceId": tool["id"],
                "resourceVersionId": tool["currentVersionId"],
                "bindingRole": "mcp_tools",
                "operation": "use",
            }
        )
    return node(
        "agent",
        "agent",
        "真实业务 Agent",
        {
            "systemPrompt": literal(prompt),
            "userQuestion": question,
            "sessionPolicy": {"mode": "application_session" if session else "invocation"},
            "maxIterations": 8,
            "maxModelCalls": 8,
            "maxToolCalls": 4,
            "maxTotalTokens": 30000,
            "maxOutputTokens": 4096,
            "maxCost": 1,
            "maxDurationSeconds": 240,
            "limitAction": "fail",
        },
        typeVersion=2,
        resourceReferences=resources,
        settings={"timeoutMs": 300000},
    )


def crm_tool(control, department, manager_id, endpoint, run_id):
    server = post(
        control,
        "/mcp/servers",
        {
            "name": f"Plan9 CRM {run_id}",
            "description": "Count every external write",
            "ownerDepartmentId": department,
            "transport": {"kind": "streamable_http", "endpoint": f"{endpoint}/mcp", "bearerCredentialId": None},
            "configuration": {},
        },
    )
    deadline = time.monotonic() + 30
    while True:
        discovered = control.post(f"/api/v1/mcp/servers/{server['id']}/discover", json={}, headers=idempotent_headers())
        if discovered.status_code == 200:
            break
        assert discovered.status_code == 503 and time.monotonic() < deadline, discovered.text
        # Discovery is read-only at the business provider. Its owned network
        # policy can take a moment to converge after the fixture rollout.
        time.sleep(0.5)
    tools = get(control, f"/mcp/servers/{server['id']}/tools")
    tool = next(
        item
        for item in (tools.get("items", []) if isinstance(tools, dict) else tools)
        if item["name"] == "crm_create_quote"
    )
    response = control.patch(
        f"/api/v1/mcp/tools/{tool['id']}/policy",
        json={
            "version": tool["version"],
            "enabled": True,
            "debugEnabled": False,
            "timeoutSeconds": 30,
            "sideEffect": "non_idempotent",
            "approvalRule": {"userIds": [manager_id], "roleIds": [], "timeoutSeconds": 600},
        },
    )
    assert response.status_code == 200, response.text
    return response.json()


def crm_application(control, model, profile, tool, endpoint, run_id, scene):
    schema = object_schema(
        {
            "request_id": {"type": "string", "title": "报价编号"},
            "customer": {"type": "string", "title": "客户名称"},
            "grossAmount": {"type": "number", "title": "原价金额", "minimum": 0.01},
            "discountPercent": {"type": "number", "title": "优惠百分比", "minimum": 0, "maximum": 50},
        }
    )
    cases = [
        {"request_id": f"{scene}-HZ-392", "customer": "杭州海川科技", "grossAmount": 1479, "discountPercent": 18},
        {"request_id": f"{scene}-SZ-861", "customer": "苏州启明制造", "grossAmount": 2500, "discountPercent": 25},
    ]
    question = {
        "kind": "template",
        "segments": [{"kind": "text", "text": "请真实创建 CRM 报价。参数："}]
        + [
            segment
            for key in ("request_id", "customer", "grossAmount", "discountPercent")
            for segment in (
                {"kind": "text", "text": f" {key}="},
                {"kind": "reference", "selector": {"namespace": "inputs", "path": [key]}},
            )
        ],
    }
    prompt = "你是报价业务 Agent。必须使用挂载的 CRM 工具真实创建一条报价，request_id 映射到工具 reference，其余字段保持原值。等待审批期间不得另发写入。成功后只回复该工具最后一次成功结果的纯 JSON 对象（structuredContent），必须包含 receiptId、customer、total、discountPercent、reference，不加 Markdown，不伪造凭证。"
    if scene == "S18":
        prompt += " 如果审批被拒绝，读取真实拒绝意见，将 discountPercent 调整为 5，其他参数不变，重新提出一次具体工具操作。再次获批成功后回复新结果。"
    else:
        prompt += " 如果审批被拒绝，应说明未创建，不得自行重新写入。"
    nodes = []
    context = {"crm": True, "generatedFields": ["receipt_id"], "approvalDecisions": []}
    if scene == "S19":
        details = object_schema(
            {
                "contact": {"type": "string", "title": "客户联系人", "minLength": 2},
                "email": {"type": "string", "format": "email", "title": "联系邮箱"},
            }
        )
        nodes.append(wait("details", "补充客户联系资料后再创建报价", details))
        question["segments"] += [
            {"kind": "text", "text": " 已补充的客户资料="},
            {
                "kind": "reference",
                "selector": {
                    "namespace": "outputs",
                    "sourceNodeId": "details",
                    "port": "main",
                    "path": ["content"],
                    "item": {"kind": "first"},
                },
            },
        ]
        context["waitInputs"] = [
            {"/details": [{"contact": "李欣", "email": "li.xin@example.test"}]},
            {"/details": [{"contact": "张毅", "email": "zhang.yi@example.test"}]},
        ]
    nodes.append(agent(model, prompt, question, tool=tool))
    parsed = code(
        "receipt",
        "核验 Agent 返回的真实报价凭证",
        profile,
        {"receipt": reference("outputs", ["text"], "agent")},
        "def main(**inputs):\n    import json\n    receipt = json.loads(inputs['receipt'])\n    for key in ['receiptId', 'customer', 'total', 'discountPercent', 'reference']:\n        assert key in receipt, 'Missing CRM receipt field: ' + key\n    return dict(receipt, summary=json.dumps(receipt, ensure_ascii=False, sort_keys=True))",
        {"receiptId": "", "customer": "", "total": 100.25, "discountPercent": 2.5, "reference": "", "summary": ""},
    )
    nodes.append(parsed)
    outputs = {
        "receipt_id": {"type": "string"},
        "customer": {"type": "string"},
        "total": {"type": "number"},
        "discount_percent": {"type": "number"},
        "reference": {"type": "string"},
    }
    bindings = {
        "receipt_id": output_binding("receipt", "receiptId"),
        "customer": output_binding("receipt", "customer"),
        "total": output_binding("receipt", "total"),
        "discount_percent": output_binding("receipt", "discountPercent"),
        "reference": output_binding("receipt", "reference"),
    }
    if scene == "S19":
        nodes.append(report_node(endpoint, "receipt"))
        outputs["files"] = OUTPUT_FILE
        bindings["files"] = reference("outputs", ["files"], "report")
        context["fileOutputs"] = True
    expected = []
    for case in cases:
        discount = 5 if scene == "S18" else case["discountPercent"]
        expected.append(
            {
                "customer": case["customer"],
                "total": round(case["grossAmount"] * (1 - discount / 100), 2),
                "discount_percent": discount,
                "reference": case["request_id"],
            }
        )
        context["approvalDecisions"].append(
            (
                [
                    {
                        "decision": "rejected",
                        "actor": "manager",
                        "reason": "优惠比例过高，请将 discountPercent 调整为5后重新申请",
                    }
                ]
                if scene == "S18"
                else []
            )
            + [{"decision": "approved", "actor": "manager"}]
        )
    return publish(
        control,
        run_id,
        scene,
        {"S17": "Agent 创建 CRM 报价", "S18": "拒绝后调整优惠", "S19": "补充资料并审批报价文件"}[scene],
        schema,
        outputs,
        nodes,
        bindings,
        cases,
        expected,
        context=context,
    )


def conversation(control, model, run_id):
    schema = object_schema({"message": {"type": "string", "title": "客服消息", "minLength": 1}})
    cases = [
        {"message": "记住我的工单代号为 CASE-HZ-X61，只回复工单代号本身，不加任何标点或解释。"},
        {"message": "记住我的工单代号为 CASE-SZ-Z92，只回复工单代号本身，不加任何标点或解释。"},
    ]
    result = publish(
        control,
        run_id,
        "S06",
        "多轮客服会话",
        schema,
        {"answer": {"type": "string"}},
        [
            agent(
                model,
                "你是客服会话助手。使用当前会话的历史内容回答问题，严格遵守用户要求的回复格式。每个会话只能使用自身记录，禁止猜测或混入其他会话。",
                reference("inputs", ["message"]),
                session=True,
            )
        ],
        {"answer": reference("outputs", ["text"], "agent")},
        cases,
        [{"answer": "CASE-HZ-X61"}, {"answer": "CASE-SZ-Z92"}],
        context={
            "chat": True,
            "followups": [
                [{"message": "我的工单代号是什么?只回复这个代号。", "answer": "CASE-HZ-X61"}],
                [{"message": "我的工单代号是什么?只回复这个代号。", "answer": "CASE-SZ-Z92"}],
            ],
        },
    )
    path = f"/api/v1/applications/{result['applicationId']}/deployments/{result['deploymentId']}/playground-config"
    current = control.get(path).json()
    saved = control.put(
        path,
        json={
            "expectedVersion": current["version"],
            "mapping": {
                "questionInput": "message",
                "fileInput": None,
                "answerOutput": "answer",
                "answerFilesOutput": None,
            },
        },
    )
    assert saved.status_code == 202, saved.text
    deadline = time.monotonic() + 120
    while control.get(path).json()["publishStatus"] != "active":
        assert time.monotonic() < deadline, "Formal conversation contract publication did not settle"
        time.sleep(0.5)
    return result
