"""Replayable formal workflows and unseen business cases for plan9 acceptance."""

from __future__ import annotations

from itertools import pairwise

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.next_batch_support import get
from tests.e2e.product.studio_builder_support import post
from tests.e2e.product.test_live_memory_pipeline import _authorize_definition, _save_version
from tests.e2e.product.test_provider_integration import _publish_application_deployment, _sandbox_image_digest


def reference(namespace, path, source=None):
    return {
        "kind": "reference",
        "selector": {
            "namespace": namespace,
            "path": path,
            "run": {"kind": "current"},
            "item": {"kind": "first" if source else "current"},
            **({"sourceNodeId": source, "port": "main"} if source else {}),
        },
        "missingPolicy": {"kind": "error"},
    }


def connect(ids):
    return [
        {
            "id": f"edge-{index}",
            "sourceNodeId": start,
            "sourceHandle": "main",
            "targetNodeId": end,
            "targetHandle": "main",
            "order": 0,
        }
        for index, (start, end) in enumerate(pairwise(ids))
    ]


def sandbox_profile(control, department, run_id):
    return post(
        control,
        "/sandbox-profiles",
        {
            "name": f"Plan9 business calculation {run_id}",
            "description": "Formal deterministic business execution",
            "ownerDepartmentId": department,
            "runner": "python",
            "imageDigest": _sandbox_image_digest(),
            "cpuMillis": 500,
            "memoryBytes": 536870912,
            "pidsLimit": 256,
            "diskBytes": 1073741824,
            "timeoutSeconds": 120,
            "outputLimitBytes": 1048576,
            "networkPolicy": {"defaultAction": "deny", "egressMode": "none"},
        },
    )


def quotation(control, profile, run_id):
    workflow = post(control, "/workflows", {"name": f"S01 客户报价 {run_id}", "visibility": "company"})["id"]
    definition = fixture_draft(control, workflow)["definition"]
    fields = {
        "customer": {"type": "string", "title": "客户名称", "minLength": 1},
        "quantity": {"type": "integer", "title": "采购数量", "minimum": 1, "maximum": 10000},
        "unitPrice": {"type": "number", "title": "单价（元）", "minimum": 0.01, "maximum": 1000000},
        "discountPercent": {"type": "number", "title": "折扣百分比", "minimum": 0, "maximum": 50},
    }
    definition["start"]["inputs"] = {
        "type": "object",
        "properties": fields,
        "required": list(fields),
        "additionalProperties": False,
    }
    calculation = {
        "id": "calculate",
        "key": "calculate",
        "type": "code",
        "typeVersion": 1,
        "name": "计算正式客户报价",
        "parameters": {
            "runner": "python",
            "inputs": {"kind": "object", "fields": {key: reference("inputs", [key]) for key in fields}},
            "source": "def main(**inputs):\n    from decimal import Decimal, ROUND_HALF_UP\n    total = (Decimal(str(inputs['quantity'])) * Decimal(str(inputs['unitPrice'])) * (1 - Decimal(str(inputs['discountPercent'])) / 100)).quantize(Decimal('0.01'), rounding=ROUND_HALF_UP)\n    return {'customer': inputs['customer'], 'total': float(total), 'currency': 'CNY'}",
            "outputExample": {"customer": "示例客户", "total": 100.25, "currency": "CNY"},
            "networkPolicy": {"mode": "deny", "destinations": []},
        },
        "contextWrites": [],
        "resourceReferences": [{"resourceType": "sandbox_profile", "resourceId": profile["id"], "operation": "use"}],
        "settings": {"timeoutMs": 180000},
    }
    exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
    exit_node["parameters"]["outputs"] = {
        key: reference("outputs", ["structuredOutput", key], "calculate") for key in ("customer", "total", "currency")
    }
    definition["nodes"] = [calculation, exit_node]
    definition["connections"] = connect(["__start__", "calculate", exit_node["id"]])
    definition["settings"]["timeoutMs"] = 240000
    definition["end"] = {
        "completion": "first_return",
        "outputs": {
            key: {"schema": schema, "required": True, "sensitive": False}
            for key, schema in {
                "customer": {"type": "string"},
                "total": {"type": "number"},
                "currency": {"type": "string"},
            }.items()
        },
        "error": {"outputs": {}},
    }
    _authorize_definition(control, workflow, definition)
    version = _save_version(control, workflow, definition)
    assert isinstance(version.get("id"), str), f"S01 workflow publication failed: {version}"
    app = post(
        control,
        "/applications",
        {"workflowId": workflow, "name": f"客户报价 {run_id}", "slug": f"p9-s01-{run_id}", "visibility": "company"},
    )
    environment = next(item for item in get(control, "/environments") if item["code"] == "development")
    deployment = _publish_application_deployment(
        control, dict(control.headers), app["id"], version["id"], environment["id"]
    )
    return {
        "scene": "S01",
        "applicationId": app["id"],
        "slug": app["slug"],
        "workflowId": workflow,
        "versionId": version["id"],
        "deploymentId": deployment["id"],
        "inputs": [
            {"customer": "杭州澄明科技", "quantity": 17, "unitPrice": 289.50, "discountPercent": 7.5},
            {"customer": "苏州远航设备", "quantity": 3, "unitPrice": 1280.25, "discountPercent": 12},
        ],
        "expected": [
            {"customer": "杭州澄明科技", "total": 4552.39, "currency": "CNY"},
            {"customer": "苏州远航设备", "total": 3379.86, "currency": "CNY"},
        ],
    }
