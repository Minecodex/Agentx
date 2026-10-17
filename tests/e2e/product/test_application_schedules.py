"""Configure typed schedule inputs through the UI and verify a real scheduled execution."""

import json
import os
from pathlib import Path

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.test_channel_delivery import _passthrough_workflow
from tests.e2e.product.test_live_memory_pipeline import _save_version
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def test_application_schedule_inputs(installed_agentx, service_urls, run_id):
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as control:
        token, _ = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        name = f"Schedule inputs {run_id}"
        original = _passthrough_workflow(control, dict(control.headers), name)
        definition = fixture_draft(control, original["workflowId"])["definition"]
        definition["start"]["inputs"]["properties"] = {
            "message": {"type": "string", "title": "消息", "minLength": 2},
            "count": {"type": "integer", "title": "数量", "minimum": 0, "maximum": 5, "default": 0},
            "enabled": {"type": "boolean", "title": "启用选项", "default": False},
            "mode": {"type": "string", "title": "模式", "enum": ["fast", "safe"], "default": "safe"},
            "options": {
                "type": "object",
                "required": ["region"],
                "properties": {"region": {"type": "string", "title": "地区"}},
                "additionalProperties": False,
            },
            "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1},
        }
        version = _save_version(control, original["workflowId"], definition)
        environments = control.get("/api/v1/environments").json()
        environment = next(item for item in environments if item["code"] == "development")
    fixture = Path(installed_agentx["artifact_dir"]) / "schedule-input-fixture.json"
    fixture.write_text(
        json.dumps(
            {"workflowName": name, "workflowVersion": version["versionNumber"], "environment": environment["name"]}
        ),
        encoding="utf-8",
    )
    run_playwright(
        Path(installed_agentx["root"]),
        "application-schedules",
        ("tests/application-schedules.spec.ts",),
        {
            **os.environ,
            "AGENTX_E2E_RUN_ID": run_id,
            "AGENTX_E2E_STAGE": "helm-agentxctl",
            "AGENTX_E2E_BASE_URL": service_urls["web"],
            "AGENTX_E2E_RUNTIME_URL": service_urls["runtime"],
            "AGENTX_SCHEDULE_FIXTURE": str(fixture),
        },
    )
