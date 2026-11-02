"""Workflow-node approval branches, kept separate from Agent Tool Effect approval fixtures."""

from tests.e2e.product.plan9.scenarios import reference
from tests.e2e.product.plan9.workflow_builders import edge, literal, node, object_schema, publish

INPUT = object_schema(
    {
        "request_id": {"type": "string", "title": "报销或采购编号", "minLength": 3},
        "amount": {"type": "number", "title": "申请金额", "minimum": 0.01, "maximum": 100000},
    }
)
OUTPUT = {"request_id": {"type": "string"}, "amount": {"type": "number"}, "decision": {"type": "string"}}


def approval(identifier, title, user):
    return node(
        identifier,
        "approval",
        title,
        {
            "candidateUserId": user,
            "title": literal(title),
            "description": {
                "kind": "template",
                "segments": [
                    {"kind": "text", "text": "请审核当前业务申请 "},
                    {"kind": "reference", "selector": {"namespace": "inputs", "path": ["request_id"]}},
                ],
            },
            "buttons": [{"id": "approved", "label": "批准"}, {"id": "rejected", "label": "拒绝"}],
            "timeoutMs": 600000,
        },
    )


def bindings(decision):
    return {
        "request_id": reference("inputs", ["request_id"]),
        "amount": reference("inputs", ["amount"]),
        "decision": literal(decision),
    }


def manager(control, run_id, user):
    rejected = node("rejected", "exit", "报销申请被拒绝", {"outputs": bindings("rejected")})
    cases = [{"request_id": "S14-EXP-319", "amount": 287.50}, {"request_id": "S14-EXP-865", "amount": 1689.75}]
    return publish(
        control,
        run_id,
        "S14",
        "经理审批报销",
        INPUT,
        OUTPUT,
        [approval("manager", "经理审核报销申请", user), rejected],
        bindings("approved"),
        cases,
        [{**cases[0], "decision": "approved"}, {**cases[1], "decision": "rejected"}],
        connections=[
            edge("start-manager", "__start__", "manager"),
            edge("approved-exit", "manager", "$exit", "decision:approved"),
            edge("rejected-exit", "manager", "rejected", "decision:rejected"),
        ],
        context={
            "approvalDecisions": [
                [{"nodeId": "manager", "decision": "approved", "actor": "manager"}],
                [{"nodeId": "manager", "decision": "rejected", "actor": "manager"}],
            ]
        },
    )


def manager_finance(control, run_id, manager_id, finance_id):
    rejected = node("rejected", "exit", "审批未通过", {"outputs": bindings("rejected")})
    cases = [{"request_id": "S15-EXP-168", "amount": 5600}, {"request_id": "S15-EXP-729", "amount": 9800}]
    return publish(
        control,
        run_id,
        "S15",
        "经理与财务依次审批",
        INPUT,
        OUTPUT,
        [
            approval("manager", "经理审核业务必要性", manager_id),
            approval("finance", "财务审核费用凭据", finance_id),
            rejected,
        ],
        bindings("approved"),
        cases,
        [{**cases[0], "decision": "approved"}, {**cases[1], "decision": "rejected"}],
        connections=[
            edge("start-manager", "__start__", "manager"),
            edge("manager-yes", "manager", "finance", "decision:approved"),
            edge("manager-no", "manager", "rejected", "decision:rejected"),
            edge("finance-yes", "finance", "$exit", "decision:approved"),
            edge("finance-no", "finance", "rejected", "decision:rejected"),
        ],
        context={
            "approvalDecisions": [
                [
                    {"nodeId": "manager", "decision": "approved", "actor": "manager"},
                    {"nodeId": "finance", "decision": decision, "actor": "finance"},
                ]
                for decision in ("approved", "rejected")
            ]
        },
    )


def threshold(control, run_id, manager_id):
    check = node(
        "threshold",
        "if",
        "超过五千元进入经理审批",
        {
            "cases": [
                {
                    "id": "exceeded",
                    "name": "超过阈值",
                    "logicalOp": "and",
                    "conditions": [
                        {
                            "condition": {
                                "left": reference("inputs", ["amount"]),
                                "operator": "gt",
                                "right": literal(5000),
                            }
                        }
                    ],
                }
            ]
        },
    )
    direct = node("direct", "exit", "阈值以内直接完成", {"outputs": bindings("not_required")})
    cases = [{"request_id": "S16-PO-257", "amount": 4999.99}, {"request_id": "S16-PO-638", "amount": 5000.01}]
    rejected = node("rejected", "exit", "超额采购被拒绝", {"outputs": bindings("rejected")})
    return publish(
        control,
        run_id,
        "S16",
        "超额采购审批",
        INPUT,
        OUTPUT,
        [check, approval("manager", "超额采购经理审批", manager_id), direct, rejected],
        bindings("approved"),
        cases,
        [{**cases[0], "decision": "not_required"}, {**cases[1], "decision": "approved"}],
        connections=[
            edge("start-check", "__start__", "threshold"),
            edge("exceeded", "threshold", "manager", "case:exceeded"),
            edge("direct", "threshold", "direct", "else"),
            edge("approved", "manager", "$exit", "decision:approved"),
            edge("rejected", "manager", "rejected", "decision:rejected"),
        ],
        context={"approvalDecisions": [[], [{"nodeId": "manager", "decision": "approved", "actor": "manager"}]]},
    )
