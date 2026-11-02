"""Real input suspensions: scalar, structured, sequential, conditional, parallel and nested."""

from __future__ import annotations

from tests.e2e.product.plan9.scenarios import reference
from tests.e2e.product.plan9.workflow_builders import (
    code,
    edge,
    literal,
    node,
    object_schema,
    output_binding,
    publish,
    wait,
)

IDENTITY = object_schema({"request_id": {"type": "string", "title": "业务编号", "minLength": 3}})
TEXT = {"type": "string", "title": "补充说明", "minLength": 3, "maxLength": 500}
DELIVERY = object_schema(
    {
        "date": {"type": "string", "format": "date", "title": "交付日期"},
        "contact": {"type": "string", "title": "收货联系人", "minLength": 2},
        "email": {"type": "string", "format": "email", "title": "联系邮箱"},
    }
)


def sequential(control, run_id, scene, title, wait_specs, contents):
    nodes = [node("progress", "reply_message", "告知处理阶段", {"content": literal("信息校验完成，等待补充资料")})]
    nodes += [wait(identifier, label, schema) for identifier, label, schema in wait_specs]
    outputs = {"request_id": {"type": "string"}, **{identifier: schema for identifier, _, schema in wait_specs}}
    bindings = {
        "request_id": reference("inputs", ["request_id"]),
        **{identifier: reference("outputs", ["content"], identifier) for identifier, _, _ in wait_specs},
    }
    cases = [{"request_id": f"{scene}-HZ-732"}, {"request_id": f"{scene}-SZ-865"}]
    expected = [
        {**case, **{identifier: values[0] for identifier, values in content.items()}}
        for case, content in zip(cases, contents, strict=True)
    ]
    return publish(
        control,
        run_id,
        scene,
        title,
        IDENTITY,
        outputs,
        nodes,
        bindings,
        cases,
        expected,
        context={"waitInputs": [{f"/{key}": value for key, value in content.items()} for content in contents]},
    )


def plain_text(control, run_id):
    return sequential(
        control,
        run_id,
        "S07",
        "补充文字说明",
        [("notes", "补充客户需求", TEXT)],
        [{"notes": ["需要附带中文安装说明"]}, {"notes": ["请在出库前进行通电测试"]}],
    )


def delivery(control, run_id):
    return sequential(
        control,
        run_id,
        "S08",
        "交付安排",
        [("delivery", "填写交付日期和联系人", DELIVERY)],
        [
            {"delivery": [{"date": "2026-12-03", "contact": "王敏", "email": "wang.min@example.test"}]},
            {"delivery": [{"date": "2026-12-11", "contact": "赵强", "email": "zhao.qiang@example.test"}]},
        ],
    )


def two_rounds(control, run_id):
    return sequential(
        control,
        run_id,
        "S09",
        "连续补充资料",
        [("delivery", "先填写交付安排", DELIVERY), ("notes", "再补充验收要求", TEXT)],
        [
            {
                "delivery": [{"date": "2026-12-05", "contact": "徐宁", "email": "xu.ning@example.test"}],
                "notes": ["验收时提供设备序列号清单"],
            },
            {
                "delivery": [{"date": "2026-12-15", "contact": "钱丽", "email": "qian.li@example.test"}],
                "notes": ["按合同附件进行抽样验收"],
            },
        ],
    )


def conditional(control, profile, run_id):
    schema = object_schema(
        {
            "request_id": {"type": "string", "title": "业务编号"},
            "missingInfo": {"type": "boolean", "title": "是否缺少补充说明"},
        }
    )
    branch = node(
        "check",
        "if",
        "判断资料是否缺失",
        {
            "cases": [
                {
                    "id": "missing",
                    "name": "缺失资料",
                    "logicalOp": "and",
                    "conditions": [
                        {
                            "condition": {
                                "left": reference("inputs", ["missingInfo"]),
                                "operator": "eq",
                                "right": literal(True),
                            }
                        }
                    ],
                }
            ]
        },
    )
    note = reference("outputs", ["content"], "notes")
    note["missingPolicy"] = {"kind": "null"}
    compute = code(
        "result",
        "保留原始业务和补充资料",
        profile,
        {"request_id": reference("inputs", ["request_id"]), "notes": note},
        "def main(**inputs):\n    return {'request_id': inputs['request_id'], 'notes': inputs.get('notes') or '资料完整，无需补充'}",
        {"request_id": "", "notes": ""},
    )
    return publish(
        control,
        run_id,
        "S10",
        "缺失信息分支",
        schema,
        {"request_id": {"type": "string"}, "notes": {"type": "string"}},
        [branch, wait("notes", "补充缺失说明", TEXT), compute],
        {key: output_binding("result", key) for key in ("request_id", "notes")},
        [{"request_id": "S10-DIRECT-193", "missingInfo": False}, {"request_id": "S10-WAIT-482", "missingInfo": True}],
        [
            {"request_id": "S10-DIRECT-193", "notes": "资料完整，无需补充"},
            {"request_id": "S10-WAIT-482", "notes": "补充客户指定的设备颜色为灰色"},
        ],
        connections=[
            edge("start-check", "__start__", "check"),
            edge("missing-notes", "check", "notes", "case:missing"),
            edge("direct-result", "check", "result", "else"),
            edge("notes-result", "notes", "result"),
            edge("result-exit", "result", "$exit"),
        ],
        context={"waitInputs": [{}, {"/notes": ["补充客户指定的设备颜色为灰色"]}]},
    )


def parallel(control, run_id):
    invoice = object_schema(
        {
            "company": {"type": "string", "title": "发票抬头", "minLength": 2},
            "taxId": {"type": "string", "title": "纳税人识别号", "minLength": 6},
        }
    )
    cases = [{"request_id": "S11-HZ-413"}, {"request_id": "S11-SZ-962"}]
    contents = [
        {
            "/invoice": [{"company": "杭州星海技术有限公司", "taxId": "TAXHZ9132"}],
            "/delivery": [{"date": "2026-12-08", "contact": "唐明", "email": "tang.ming@example.test"}],
        },
        {
            "/invoice": [{"company": "苏州蓝山设备有限公司", "taxId": "TAXSZ6395"}],
            "/delivery": [{"date": "2026-12-18", "contact": "沈燕", "email": "shen.yan@example.test"}],
        },
    ]
    outputs = {"request_id": {"type": "string"}, "invoice": invoice, "delivery": DELIVERY}
    return publish(
        control,
        run_id,
        "S11",
        "并行填写发票和交付",
        IDENTITY,
        outputs,
        [
            wait("invoice", "填写发票信息", invoice),
            wait("delivery", "填写交付安排", DELIVERY),
            node("join", "merge", "等待两份资料", {"mode": "combine_by_position"}),
        ],
        {
            "request_id": reference("inputs", ["request_id"]),
            "invoice": reference("outputs", ["content"], "invoice"),
            "delivery": reference("outputs", ["content"], "delivery"),
        },
        cases,
        [
            {**case, "invoice": content["/invoice"][0], "delivery": content["/delivery"][0]}
            for case, content in zip(cases, contents, strict=True)
        ],
        connections=[
            edge("start-invoice", "__start__", "invoice"),
            edge("start-delivery", "__start__", "delivery", order=1),
            edge("invoice-join", "invoice", "join", target_port="left"),
            edge("delivery-join", "delivery", "join", target_port="right"),
            edge("join-exit", "join", "$exit"),
        ],
        context={"waitInputs": contents, "simultaneousWaits": 2},
    )


def child(control, run_id):
    nested = sequential(
        control,
        run_id,
        "S12child",
        "子流程收集交付资料",
        [("delivery", "子流程交付安排", DELIVERY)],
        [
            {"delivery": [{"date": "2026-12-09", "contact": "吴航", "email": "wu.hang@example.test"}]},
            {"delivery": [{"date": "2026-12-19", "contact": "陈芸", "email": "chen.yun@example.test"}]},
        ],
    )
    cases = [{"request_id": "S12-PARENT-318"}, {"request_id": "S12-PARENT-749"}]
    call = node(
        "collect",
        "sub_workflow",
        "调用资料收集子工作流",
        {
            "workflowVersionId": nested["versionId"],
            "inputs": {"kind": "object", "fields": {"request_id": reference("inputs", ["request_id"])}},
        },
    )
    return publish(
        control,
        run_id,
        "S12",
        "子流程收集资料",
        IDENTITY,
        {"request_id": {"type": "string"}, "delivery": DELIVERY},
        [call],
        {key: reference("outputs", [key], "collect") for key in ("request_id", "delivery")},
        cases,
        [{**case, "delivery": nested["expected"][index]["delivery"]} for index, case in enumerate(cases)],
        context={
            "waitInputs": [{"/collect/delivery": nested["waitInputs"][index]["/delivery"]} for index in range(2)],
            "childWorkflowId": nested["workflowId"],
        },
    )


def loop_review(control, run_id):
    schema = object_schema(
        {
            "request_id": {"type": "string", "title": "复核编号"},
            "items": {
                "type": "array",
                "title": "待复核明细",
                "minItems": 1,
                "maxItems": 5,
                "items": object_schema({"name": {"type": "string", "title": "项目名称"}}),
            },
        }
    )
    loop = node(
        "review",
        "loop_over_items",
        "逐项收集人工复核意见",
        {
            "input": reference("inputs", ["items"]),
            "outputSelector": reference("outputs", ["content"], "opinion"),
            "parallelism": 1,
            "errorMode": "terminate",
        },
    )
    ask = wait("opinion", "填写当前项目复核意见", {"type": "string", "title": "复核意见", "minLength": 2})
    ask["parentId"] = "review"
    cases = [
        {"request_id": "S13-HZ-125", "items": [{"name": "配件数量"}, {"name": "设备外观"}]},
        {"request_id": "S13-SZ-683", "items": [{"name": "发票信息"}, {"name": "包装标签"}, {"name": "保修卡"}]},
    ]
    replies = [["数量一致", "外观完好"], ["信息正确", "标签清晰", "保修卡齐全"]]
    return publish(
        control,
        run_id,
        "S13",
        "循环逐项复核",
        schema,
        {"request_id": {"type": "string"}, "opinions": {"type": "array", "items": {"type": "string"}}},
        [loop, ask],
        {"request_id": reference("inputs", ["request_id"]), "opinions": reference("outputs", ["items"], "review")},
        cases,
        [{"request_id": case["request_id"], "opinions": replies[index]} for index, case in enumerate(cases)],
        connections=[edge("start-review", "__start__", "review"), edge("review-exit", "review", "$exit")],
        context={"waitInputs": [{"/opinion": values} for values in replies], "repeatNode": "opinion"},
    )
