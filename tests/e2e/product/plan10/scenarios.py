"""Published fixture workflows and held-out inputs; no product logic lives in this harness."""

from tests.e2e.product.plan9.approval_scenarios import approval
from tests.e2e.product.plan9.scenarios import reference
from tests.e2e.product.plan9.workflow_builders import edge, literal, node, object_schema, publish, wait


def expenses(control, run_id, reviewer):
    inputs = object_schema(
        {
            "request_id": {"type": "string", "title": "报销单号", "minLength": 3},
            "amount": {"type": "number", "title": "报销金额", "minimum": 0.01, "maximum": 100000},
            "currency": {"type": "string", "enum": ["CNY", "USD"], "title": "币种"},
        }
    )

    content = object_schema({"note": {"type": "string", "title": "补充报销说明", "minLength": 3}})
    common = {
        "request_id": reference("inputs", ["request_id"]),
        "amount": reference("inputs", ["amount"]),
        "currency": reference("inputs", ["currency"]),
        "notes": reference("outputs", ["content"], "notes"),
    }
    outputs = {**inputs["properties"], "notes": content, "decision": {"type": "string"}}
    return publish(
        control,
        run_id,
        "B10",
        "批量费用报销服务台",
        inputs,
        outputs,
        [
            wait("notes", "补充报销说明", content),
            approval("manager", "经理审核报销", reviewer),
            node("rejected", "exit", "报销申请被拒绝", {"outputs": {**common, "decision": literal("rejected")}}),
        ],
        {**common, "decision": literal("approved")},
        [
            {"request_id": "B10-EXP-101", "amount": 128.25, "currency": "CNY"},
            {"request_id": "B10-EXP-202", "amount": 360.50, "currency": "USD"},
        ],
        [],
        connections=[
            edge("a", "__start__", "notes"),
            edge("b", "notes", "manager"),
            edge("c", "manager", "$exit", "decision:approved"),
            edge("d", "manager", "rejected", "decision:rejected"),
        ],
        context={"kind": "expense", "waitInputs": [{"/notes": [{"note": "正式报销补充说明"}]}]},
    )


def export_states(control, run_id, reviewer, profile):
    """Deterministic branches create real states for one mixed CSV snapshot."""
    from tests.e2e.product.plan9.workflow_builders import code

    properties = {
        "request_id": {"type": "string", "minLength": 3},
        "mode": {"type": "string", "enum": ["wait", "approval", "complete", "fail", "unknown", "cancel", "run"]},
        "amount": {"type": "number"},
        "currency": {"type": "string", "enum": ["CNY", "USD"]},
    }
    targets = [
        ("wait", "wait"),
        ("cancel", "wait"),
        ("approval", "review"),
        ("complete", "$exit"),
        ("fail", "fail"),
        ("unknown", "quick"),
    ]
    cases = [
        {
            "id": value,
            "name": value,
            "logicalOp": "and",
            "conditions": [
                {"condition": {"left": reference("inputs", ["mode"]), "operator": "eq", "right": literal(value)}}
            ],
        }
        for value, _ in targets
    ]
    bindings = {"request_id": reference("inputs", ["request_id"])}
    connections = [
        edge("start", "__start__", "route"),
        *[edge(f"route-{value}", "route", target, f"case:{value}") for value, target in targets],
        edge("running", "route", "running", "else"),
        edge("wait-end", "wait", "$exit"),
        edge("review-ok", "review", "$exit", "decision:approved"),
        edge("review-no", "review", "$exit", "decision:rejected"),
        *[edge(f"{value}-end", value, "$exit") for value in ("fail", "quick", "running")],
    ]
    return publish(
        control,
        run_id,
        "M10",
        "混合状态导出",
        object_schema(properties),
        properties,
        [
            node("route", "if", "选择可控业务路径", {"cases": cases}),
            wait("wait", "等待补充", object_schema({"note": {"type": "string"}})),
            approval("review", "等待经理", reviewer),
            code(
                "fail",
                "确定失败",
                profile,
                bindings,
                "def main(**inputs):\n    raise ValueError('Controlled CSV failure')\n",
                {"ok": True},
            ),
            code(
                "quick",
                "完成后注入待核实凭据",
                profile,
                bindings,
                "def main(**inputs):\n    return {'ok': True}\n",
                {"ok": True},
            ),
            code(
                "running",
                "保持处理中",
                profile,
                bindings,
                "def main(**inputs):\n    import time\n    time.sleep(60)\n    return {'ok': True}\n",
                {"ok": True},
            ),
        ],
        {key: reference("inputs", [key]) for key in properties},
        [],
        [],
        connections=connections,
    )
