"""Nested procurement and strict order types, with deterministic unseen expected results."""

from tests.e2e.product.plan9.scenarios import reference
from tests.e2e.product.plan9.workflow_builders import code, object_schema, output_binding, publish


def procurement(control, profile, run_id):
    item = object_schema(
        {
            "name": {"type": "string", "title": "产品名称", "minLength": 1},
            "quantity": {"type": "integer", "title": "数量", "minimum": 1},
            "unitPrice": {"type": "number", "title": "单价", "minimum": 0.01},
        }
    )
    schema = object_schema(
        {
            "department": object_schema(
                {"name": {"type": "string", "title": "采购部门"}, "contact": {"type": "string", "title": "联系人"}}
            ),
            "items": {"type": "array", "title": "采购明细", "minItems": 1, "maxItems": 20, "items": item},
        }
    )
    cases = [
        {
            "department": {"name": "研发一部", "contact": "林晓"},
            "items": [
                {"name": "显示器", "quantity": 2, "unitPrice": 1299.5},
                {"name": "键盘", "quantity": 5, "unitPrice": 179.9},
            ],
        },
        {
            "department": {"name": "仓储部", "contact": "周明"},
            "items": [
                {"name": "标签纸", "quantity": 12, "unitPrice": 23.5},
                {"name": "扫描器", "quantity": 3, "unitPrice": 489},
                {"name": "货架", "quantity": 4, "unitPrice": 629.75},
            ],
        },
    ]
    example = {
        "department": "示例部门",
        "lines": [{"name": "示例产品", "quantity": 1, "unitPrice": 1.25, "subtotal": 1.25}],
        "total": 1.25,
    }
    source = "def main(**inputs):\n    lines = [dict(item, subtotal=round(item['quantity'] * item['unitPrice'], 2)) for item in inputs['items']]\n    return {'department': inputs['department']['name'], 'lines': lines, 'total': round(sum(item['subtotal'] for item in lines), 2)}"
    compute = code(
        "calculate",
        "核算多行采购明细",
        profile,
        {key: reference("inputs", [key]) for key in ("department", "items")},
        source,
        example,
    )
    expected = [
        {
            "department": "研发一部",
            "lines": [dict(cases[0]["items"][0], subtotal=2599.0), dict(cases[0]["items"][1], subtotal=899.5)],
            "total": 3498.5,
        },
        {
            "department": "仓储部",
            "lines": [
                dict(cases[1]["items"][0], subtotal=282.0),
                dict(cases[1]["items"][1], subtotal=1467),
                dict(cases[1]["items"][2], subtotal=2519.0),
            ],
            "total": 4268.0,
        },
    ]
    outputs = {
        "department": {"type": "string", "title": "采购部门"},
        "lines": {
            "type": "array",
            "title": "采购明细及小计",
            "items": object_schema(
                {
                    **item["properties"],
                    "subtotal": {"type": "number", "title": "行小计"},
                }
            ),
        },
        "total": {"type": "number", "title": "采购总额"},
    }
    return publish(
        control,
        run_id,
        "S02",
        "采购清单",
        schema,
        outputs,
        [compute],
        {key: output_binding("calculate", key) for key in outputs},
        cases,
        expected,
    )


def order_validation(control, profile, run_id):
    schema = object_schema(
        {
            "order_id": {"type": "string", "title": "订单编号", "minLength": 3},
            "priority": {"type": "string", "title": "优先级", "enum": ["normal", "urgent"]},
            "insured": {"type": "boolean", "title": "是否投保"},
            "weight_kg": {"type": "number", "title": "重量（千克）", "minimum": 0.1, "maximum": 50},
            "packages": {"type": "integer", "title": "包裹数", "minimum": 1, "maximum": 20},
            "note": {"type": "string", "title": "可选备注", "maxLength": 100},
        },
        ["order_id", "priority", "insured", "weight_kg", "packages"],
    )
    cases = [
        {"order_id": "ORD-HZ-291", "priority": "normal", "insured": False, "weight_kg": 0.1, "packages": 1},
        {
            "order_id": "ORD-SZ-842",
            "priority": "urgent",
            "insured": True,
            "weight_kg": 50,
            "packages": 20,
            "note": "玻璃制品轻放",
        },
    ]
    outputs = {
        "order_id": {"type": "string"},
        "priority": {"type": "string"},
        "insured": {"type": "boolean"},
        "weight_kg": {"type": "number"},
        "packages": {"type": "integer"},
        "note": {"type": "string"},
    }
    bindings = {key: reference("inputs", [key]) for key in outputs}
    bindings["note"]["missingPolicy"] = {"kind": "null"}
    compute = code(
        "validate",
        "校验订单并保留字段类型",
        profile,
        bindings,
        "def main(**inputs):\n    inputs['note'] = inputs.get('note') or ''\n    return inputs",
        {"order_id": "", "priority": "normal", "insured": False, "weight_kg": 1.25, "packages": 1, "note": ""},
    )
    return publish(
        control,
        run_id,
        "S03",
        "订单校验",
        schema,
        outputs,
        [compute],
        {key: output_binding("validate", key) for key in outputs},
        cases,
        [{**case, "note": case.get("note", "")} for case in cases],
    )
