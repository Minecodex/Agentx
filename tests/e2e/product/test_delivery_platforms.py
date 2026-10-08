"""Strict official IM protocols and distinct outbox rows for repeated loop activations."""

from __future__ import annotations

import time
from itertools import pairwise

import httpx
import pytest

from tests.e2e.product import test_channel_delivery as channels
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import run

pytestmark = [pytest.mark.cluster, pytest.mark.product]
im_mock = channels.im_mock


def test_official_protocols_and_loop_delivery_identity(installed_agentx, service_urls, im_mock, run_id):
    context = installed_agentx
    namespace = context["runtime_namespace"]
    run(
        (
            "kubectl",
            "-n",
            namespace,
            "set",
            "env",
            "deployment/workflow-runtime",
            f"AGENTX_DELIVERY_DINGTALK_API_BASE={im_mock['url']}/dingtalk",
            f"AGENTX_DELIVERY_FEISHU_API_BASE={im_mock['url']}/feishu",
            f"AGENTX_DELIVERY_WECOM_API_BASE={im_mock['url']}/wecom",
        ),
        timeout=60,
    )
    run(("kubectl", "-n", namespace, "rollout", "status", "deployment/workflow-runtime", "--timeout=300s"), timeout=330)
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        headers = {"Authorization": f"Bearer {token}"}
        workflow = channels._passthrough_workflow(control, headers, f"Official sends {run_id}")
        created = control.post(
            "/api/v1/applications",
            headers=headers,
            json={
                "name": f"Official sends {run_id}",
                "slug": f"official-{run_id}",
                "workflowId": workflow["workflowId"],
                "visibility": "company",
            },
        )
        assert created.status_code == 201, created.text
        application = created.json()
        configurations = {
            "dingtalk": (
                "stream",
                {"clientId": "e2e-client", "clientSecret": "e2e-client-secret", "robotCode": "e2e-robot"},
            ),
            "feishu": ("stream", {"appId": "e2e-feishu", "appSecret": "e2e-feishu-secret"}),
            "wecom": (
                "callback",
                {
                    "token": "e2e-wecom-token",
                    "encodingAESKey": "a" * 43,
                    "corpId": "e2e-corp",
                    "corpSecret": "e2e?wecom&secret+encoded",
                    "agentId": "1",
                },
            ),
        }
        bindings = {
            provider: channels._channel(control, headers, application["id"], provider, mode, config)
            for provider, (mode, config) in configurations.items()
        }
        draft = control.get(f"/api/v1/workflows/{workflow['workflowId']}/draft", headers=headers).json()
        definition = draft["definition"]
        exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
        for provider, binding in bindings.items():
            node = {
                "id": f"push_{provider}",
                "key": f"push_{provider}",
                "name": f"Push {provider}",
                "type": "send_message",
                "typeVersion": 1,
                "disabled": False,
                "contextWrites": [],
                "resourceReferences": [],
                "settings": {},
                "parameters": {
                    "channelId": binding["id"],
                    "content": f"Official {provider} {run_id}",
                    "senderId": "e2e-sender",
                    "targetConversationId": "e2e-group",
                },
            }
            if provider == "dingtalk":
                node["parentId"] = "repeat"
            definition["nodes"].append(node)
        definition["nodes"].append(
            {
                "id": "repeat",
                "key": "repeat",
                "type": "loop_over_items",
                "typeVersion": 1,
                "name": "Two deliveries",
                "disabled": False,
                "contextWrites": [],
                "resourceReferences": [],
                "settings": {},
                "parameters": {
                    "input": {
                        "kind": "array",
                        "items": [{"kind": "literal", "value": 1}, {"kind": "literal", "value": 2}],
                    },
                    "parallelism": 2,
                    "errorMode": "terminate",
                    "outputSelector": {
                        "kind": "reference",
                        "selector": {
                            "namespace": "outputs",
                            "sourceNodeId": "push_dingtalk",
                            "port": "main",
                            "run": {"kind": "current"},
                            "item": {"kind": "current"},
                            "path": [],
                        },
                        "missingPolicy": {"kind": "error"},
                    },
                },
            }
        )
        chain = ["__start__", "repeat", "push_feishu", "push_wecom", exit_node["id"]]
        definition["connections"] = [
            {
                "id": f"official-edge-{index}",
                "sourceNodeId": source,
                "sourceHandle": "main",
                "targetNodeId": target,
                "targetHandle": "main",
                "order": 0,
            }
            for index, (source, target) in enumerate(pairwise(chain))
        ]
        saved = control.put(
            f"/api/v1/workflows/{workflow['workflowId']}/draft",
            headers=headers,
            json={"expectedRevision": draft["revision"], "definition": definition},
        )
        assert saved.status_code == 200, saved.text
        latest = control.get(f"/api/v1/workflows/{workflow['workflowId']}/draft", headers=headers).json()
        published = control.post(
            f"/api/v1/workflows/{workflow['workflowId']}/versions",
            headers=headers,
            json={"draftRevision": latest["revision"]},
        )
        assert published.status_code == 201, published.text
        workflow["versionId"] = published.json()["id"]
        environment = channels._development_environment_id(control, headers)
        deployed = control.post(
            f"/api/v1/workflows/{workflow['workflowId']}/deployments",
            headers=headers,
            json={"environmentId": environment, "workflowVersionId": workflow["versionId"]},
        )
        assert deployed.status_code in (200, 201), deployed.text
        channels._publish_application(control, headers, workflow, application["id"], environment)
        key = control.post(
            f"/api/v1/applications/{application['id']}/api-keys", headers=headers, json={"name": "Official IM test"}
        )
        assert key.status_code == 201, key.text
        secret = key.json()["secret"]
    with httpx.Client(base_url=service_urls["runtime"], timeout=60) as gateway:
        headers = {"Authorization": f"Bearer {secret}", "Idempotency-Key": f"official-{run_id}"}
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            accepted = gateway.post(
                f"/gateway/v1/applications/{application['slug']}/invocations",
                headers=headers,
                json={"input": {"message": "official protocol test"}, "responseMode": "async"},
            )
            if accepted.status_code == 202:
                break
            assert accepted.status_code in (401, 403, 404), accepted.text
            time.sleep(1)
        assert accepted.status_code == 202, accepted.text
        invocation = accepted.json()["id"]
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            status = gateway.get(f"/gateway/v1/invocations/{invocation}", headers=headers)
            assert status.status_code == 200, status.text
            value = status.json()
            if value["status"] == "completed":
                break
            assert value["status"] not in {"failed", "cancelled"}, value
            time.sleep(1)
        assert value["status"] == "completed", value
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        rows = channels._delivery_rows(context, me["companyId"], value["executionId"])
        if len(rows) == 4 and all(row["status"] == "delivered" for row in rows):
            break
        assert not any(row["status"] == "dead" for row in rows), rows
        time.sleep(1)
    assert len(rows) == 4 and all(row["providerMessageId"] for row in rows), rows
    ding = [row for row in rows if row["provider"] == "dingtalk"]
    assert len(ding) == 2 and len({row["origin"] for row in ding}) == 2, ding
    received = channels._mock_received(context["dependencies_namespace"], "")
    assert any(item["method"] == "GET" and item["path"].startswith("/wecom/cgi-bin/gettoken") for item in received), (
        received
    )
    assert sum("groupMessages/send" in item["path"] and run_id in item["body"]["msgParam"] for item in received) == 2, (
        received
    )
    assert any(
        "/feishu/" in item["path"] and "messages" in item["path"] and isinstance(item["body"]["content"], str)
        for item in received
    ), received
    assert any(
        "/wecom/" in item["path"] and "message/send" in item["path"] and isinstance(item["body"]["agentid"], int)
        for item in received
    ), received
    from tools.scripts.release.evidence import write_report

    write_report(
        context,
        "product/official-im.json",
        {"status": "passed", "deliveries": rows, "distinctLoopActivations": len(ding)},
    )
