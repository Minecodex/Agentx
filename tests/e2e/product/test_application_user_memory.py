"""Real Mem0 isolation for declared API users, platform users and signed channel senders."""

from __future__ import annotations

# ruff: noqa: S608 -- IDs come from the isolated APIs and generated test events.
import copy
import json
import time

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.live_text_support import post
from tests.e2e.product.test_channel_delivery import _dingtalk_channel, _signed_dingtalk_post
from tests.e2e.product.test_live_memory_pipeline import (
    TOOL_INSTRUCTION,
    _assert_written,
    _memory_results,
    _replace_node_id,
    _save_version,
)
from tests.e2e.product.test_live_text_acceptance import message, terminal
from tests.e2e.product.test_provider_integration import _publish_application_deployment
from tests.e2e.runtime.test_agent_attachments import _runtime_mysql, _wait_admission_outbox
from tools.scripts.release.evidence import write_report

pytestmark = [pytest.mark.cluster, pytest.mark.product, pytest.mark.live_model]


def test_real_mem0_uses_application_users_and_stable_nodes(
    installed_agentx,
    service_urls,
    live_providers,
    live_agent_application,
    run_id,
):
    control = live_providers["control"]
    app = live_agent_application
    workflow = app["workflowId"]
    application = app["applicationId"]
    environment = next(item for item in control.get("/api/v1/environments").json() if item["code"] == "development")
    me = control.get("/api/v1/auth/me").json()
    definition = copy.deepcopy(fixture_draft(control, workflow, None)["definition"])
    definition["start"]["inputs"]["properties"]["customerId"] = {"type": "string"}
    agent = next(node for node in definition["nodes"] if node["type"] == "agent")
    agent["parameters"]["sessionPolicy"] = {"mode": "invocation"}
    agent["parameters"]["systemPrompt"] = {"kind": "template", "segments": [{"kind": "text", "text": TOOL_INSTRUCTION}]}
    version = _save_version(control, workflow, definition)

    def deploy(version_id):
        post(
            control,
            f"/workflows/{workflow}/deployments",
            {"contentSource": "version", "environmentId": environment["id"], "workflowVersionId": version_id},
        )
        deployment = _publish_application_deployment(
            control, dict(control.headers), application, version_id, environment["id"], "customerId"
        )
        path = f"/api/v1/applications/{application}/deployments/{deployment['id']}/playground-config"
        config = control.get(path).json()
        saved = control.put(
            path,
            json={
                "expectedVersion": config["version"],
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
            assert time.monotonic() < deadline
            time.sleep(0.5)

    deploy(version["id"])
    keys = [
        post(control, f"/applications/{application}/api-keys", {"name": f"Memory caller {i}"})["secret"]
        for i in range(2)
    ]
    _wait_admission_outbox(installed_agentx, workflow_id=workflow)
    checked = []
    marker = f"sapphire-memory-{run_id}"
    user = me["id"]  # Identical raw platform/API/channel IDs must remain isolated.

    with httpx.Client(base_url=service_urls["runtime"], timeout=180) as gateway:

        def ask(authorization, customer, question):
            gateway.headers["Authorization"] = f"Bearer {authorization}"
            created = gateway.post(
                f"/gateway/v1/applications/{app['applicationSlug']}/sessions",
                json={"input": {} if customer is None else {"customerId": customer}},
                headers={"Idempotency-Key": f"memory-session-{time.time_ns()}"},
            )
            assert created.status_code == 201, created.text
            result = terminal(gateway, message(gateway, created.json()["id"], question))
            assert result["status"] == "completed", result
            checked.append(result)
            return result

        written = ask(keys[0], user, f"请实际调用 memory_write 保存事实：我的私人项目代号是 {marker}。")
        _assert_written(installed_agentx, written)
        recalled = ask(keys[1], user, "请实际调用 memory_recall 查询我的私人项目代号。")
        assert marker in json.dumps(_memory_results(installed_agentx, recalled), ensure_ascii=False)
        other = ask(keys[1], "another-customer", "请实际调用 memory_recall 查询我的私人项目代号。")
        assert _memory_results(installed_agentx, other) == []
        platform = ask(app["token"].value, user, "请实际调用 memory_recall 查询我的私人项目代号。")
        assert platform["endUser"]["source"] == "platform" and _memory_results(installed_agentx, platform) == []
        missing = ask(keys[0], None, "请实际调用 memory_recall 查询我的私人项目代号，工具不可用时说明原因。")
        assert missing["endUser"] is None and missing["userIdentityDiagnostic"] == "MEMORY_USER_ID_UNAVAILABLE"
        no_calls = _runtime_mysql(
            installed_agentx,
            f"SELECT COUNT(*) FROM runtime_calls WHERE execution_id=UUID_TO_BIN('{missing['executionId']}') AND call_kind='memory';",
        )
        assert int(no_calls) == 0, no_calls

        # A renamed key in a new published version still resolves to the same stable node ID.
        renamed = copy.deepcopy(definition)
        next(node for node in renamed["nodes"] if node["type"] == "agent")["key"] = "renamed_memory_agent"
        next_version = _save_version(control, workflow, renamed)
        deploy(next_version["id"])
        across_version = ask(keys[0], user, "请实际调用 memory_recall 查询我的私人项目代号。")
        assert marker in json.dumps(_memory_results(installed_agentx, across_version), ensure_ascii=False)

        # Signed channel messages use Adapter sender IDs, independently of API mapping.
        channel = _dingtalk_channel(control, dict(control.headers), application, reply_enabled=False)
        deploy(next_version["id"])
        event = f"identity-channel-{run_id}"
        incoming = _signed_dingtalk_post(
            gateway,
            channel["path"],
            "e2e-dingtalk-secret",
            {
                "msgId": event,
                "conversationId": "group-must-not-be-a-user",
                "conversationType": "1",
                "senderStaffId": user,
                "senderNick": "nickname-must-not-be-a-user",
                "msgtype": "text",
                "text": {"content": "请实际调用 memory_recall 查询我的私人项目代号。"},
            },
            "https://oapi.dingtalk.com/robot/sendBySession/test",
        )
        assert incoming.status_code in (200, 202), incoming.text
        deadline = time.monotonic() + 180
        while True:
            raw = _runtime_mysql(
                installed_agentx,
                f"SELECT BIN_TO_UUID(id) FROM application_invocations WHERE provider_event_id='{event}';",
            )
            if raw and raw.lower() != "null":
                channel_result = terminal(gateway, raw.strip())
                break
            assert time.monotonic() < deadline
            time.sleep(0.5)
        assert channel_result["status"] == "completed" and channel_result["endUser"]["source"] == "channel", (
            channel_result
        )
        assert (
            channel_result["endUser"]["externalUserId"] == user
            and _memory_results(installed_agentx, channel_result) == []
        )
        checked.append(channel_result)

        # Changing the immutable node ID creates a separate personal memory scope.
        different = copy.deepcopy(renamed)
        old_id = next(node for node in different["nodes"] if node["type"] == "agent")["id"]
        new_id = "different-memory-agent"

        _replace_node_id(different, old_id, new_id)
        different_version = _save_version(control, workflow, different)
        deploy(different_version["id"])
        new_node = ask(keys[0], user, "请实际调用 memory_recall 查询我的私人项目代号。")
        assert _memory_results(installed_agentx, new_node) == []

    write_report(
        installed_agentx,
        "product/application-user-real-memory.json",
        {
            "status": "passed",
            "realMem0": True,
            "crossSessionRecall": True,
            "apiKeysShareUserMemory": True,
            "crossVersionRecall": True,
            "differentNodesIsolated": True,
            "differentUsersIsolated": True,
            "sourcesIsolated": True,
            "channelSenderAutomatic": True,
            "executions": [{"executionId": item["executionId"], "endUser": item["endUser"]} for item in checked],
        },
    )
