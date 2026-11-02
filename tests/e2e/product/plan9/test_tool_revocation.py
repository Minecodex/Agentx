"""A policy change between proposal and approval cannot authorize a new provider write."""

import json
import time
import uuid

import pytest

from tests.e2e.product.next_batch_support import get
from tests.e2e.product.plan9.agent_scenarios import crm_application, crm_tool
from tests.e2e.product.plan9.mcp_support import TERMINAL, application_client, crm_records, decide
from tests.e2e.product.plan9.test_tool_recovery import effect
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_tool_policy_revoked_after_proposal_never_writes(plan9_suite):
    suite, report = plan9_suite, {"passed": False}
    control = suite["control"]
    endpoint = f"http://echo-mcp.{suite['installed']['dependencies_namespace']}.svc.cluster.local:8090"
    owner = get(control, "/auth/me")["departmentId"]
    tool = crm_tool(control, owner, suite["users"]["manager"]["id"], endpoint, suite["runId"] + "-revoke")
    model = get(control, f"/models/aliases/{suite['model']['id']}")
    case = crm_application(control, model, suite["profile"], tool, endpoint, suite["runId"] + "-revoke", "S17")
    case["inputs"][0]["request_id"] += "-revoked-policy"
    try:
        with application_client(suite, case, "Policy revocation") as (client, published, _):
            reference = case["inputs"][0]["request_id"]
            before = len(crm_records(suite, reference))
            created = client.submit(published["toolName"], published["revision"], case["inputs"][0])
            until = time.monotonic() + 300
            while not (pending := client.task(created["taskId"]))["pendingApprovals"]:
                assert pending["state"] not in TERMINAL and time.monotonic() < until, pending
                time.sleep(0.5)
            original = effect(suite, created["taskId"])
            assert len(crm_records(suite, reference)) == before
            response = control.patch(
                f"/api/v1/mcp/tools/{tool['id']}/policy",
                json={
                    "version": tool["version"],
                    "enabled": False,
                    "debugEnabled": False,
                    "timeoutSeconds": 30,
                    "sideEffect": "non_idempotent",
                    "approvalRule": tool["approvalRule"],
                },
            )
            assert response.status_code == 200, response.text
            revision = response.json()["version"]
            tool_hex = uuid.UUID(tool["id"]).hex
            until = time.monotonic() + 60
            while True:
                disabled = _runtime_mysql(
                    suite["installed"],
                    f"SELECT CONCAT(revision,':',enabled) FROM mcp_tool_policy_projection WHERE tool_id=UNHEX('{tool_hex}')",  # noqa: S608 -- parsed UUID only.
                )
                if disabled == f"{revision}:0":
                    break
                assert time.monotonic() < until, disabled
                time.sleep(0.2)
            human = decide(
                suite,
                case,
                pending["pendingApprovals"][0]["approvalId"],
                case["approvalDecisions"][0][0],
                "revoked-policy",
            )
            until = time.monotonic() + 300
            while (final := client.task(created["taskId"]))["state"] not in TERMINAL:
                assert time.monotonic() < until, final
                time.sleep(0.5)
            assert final["state"] == "failed", final
            assert len(crm_records(suite, reference)) == before
            result = effect(suite, created["taskId"])
            assert result["effectId"] == original["effectId"] and result["argumentsHash"] == original["argumentsHash"]
            assert result["settled"] and not result["unknown"], result
            report.update(
                passed=True,
                original=original,
                effect=result,
                final=final,
                policyRevision=revision,
                humanDecision=human,
                writesBefore=before,
                writesAfter=len(crm_records(suite, reference)),
                records=client.records,
            )
    finally:
        (suite["directory"] / "tool-policy-revocation.json").write_text(
            json.dumps(report, ensure_ascii=False, indent=2)
        )
