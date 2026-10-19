"""Real entry-point closure for progress messages and durable raw input waits."""

# ruff: noqa: S608 -- IDs belong to the isolated acceptance database.

from __future__ import annotations

import copy
import json
import os
import time
import uuid
from pathlib import Path

import httpx
import pytest

from tests.e2e.draft_fixtures import fixture_draft, idempotent_headers
from tests.e2e.product.test_application_evaluations import finished, read, select_model, send
from tests.e2e.product.test_channel_delivery import (
    _development_environment_id,
    _dingtalk_channel,
    _mock_received,
    _passthrough_workflow,
    _publish_application,
    _signed_dingtalk_post,
)
from tests.e2e.product.test_channel_delivery import (
    im_mock as im_mock,
)
from tests.e2e.product.test_live_memory_pipeline import _reference, _save_version
from tests.e2e.product.test_model_streaming import _fixture_credential, _upsert_model
from tests.e2e.runtime.test_agent_attachments import _access_token, _runtime_mysql
from tests.e2e.support import run, run_playwright

pytestmark = [pytest.mark.cluster, pytest.mark.product]


def poll(read_value, matches, *, timeout=90):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        last = read_value()
        if matches(last):
            return last
        time.sleep(0.4)
    raise AssertionError(f"Interaction did not converge: {last}")


def interaction_definition(base, hook_url):
    definition = copy.deepcopy(base)
    exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
    exit_node["parameters"]["outputs"] = {"answer": _reference("ask", ["content"])}
    definition["end"]["outputs"]["answer"]["schema"] = {}
    nodes = [
        ("progress", "reply_message", {"content": {"kind": "literal", "value": "正在执行，请回复确认"}}),
        ("ask", "await_input", {"hookUrl": {"kind": "literal", "value": hook_url}}),
        (
            "intent",
            "if",
            {
                "cases": [
                    {
                        "id": "confirmed",
                        "name": "Confirmed",
                        "logicalOp": "and",
                        "conditions": [
                            {
                                "condition": {
                                    "left": {
                                        "kind": "reference",
                                        "selector": {
                                            "namespace": "item",
                                            "run": {"kind": "current"},
                                            "item": {"kind": "current"},
                                            "path": ["content"],
                                        },
                                        "missingPolicy": {"kind": "error"},
                                    },
                                    "operator": "eq",
                                    "right": {"kind": "literal", "value": "确认"},
                                }
                            }
                        ],
                    }
                ]
            },
        ),
        ("reprompt", "reply_message", {"content": {"kind": "literal", "value": "收到您的内容，请回复确认继续"}}),
    ]
    definition["nodes"] = [exit_node] + [
        {
            "id": identifier,
            "key": identifier,
            "name": identifier,
            "type": kind,
            "typeVersion": 1,
            "parameters": parameters,
            "contextWrites": [],
            "resourceReferences": [],
            "settings": {},
        }
        for identifier, kind, parameters in nodes
    ]
    edges = [
        ("__start__", "main", "progress"),
        ("progress", "main", "ask"),
        ("ask", "main", "intent"),
        ("intent", "case:confirmed", exit_node["id"]),
        ("intent", "else", "reprompt"),
        ("reprompt", "main", "ask"),
    ]
    definition["connections"] = [
        {
            "id": f"interaction-{index}",
            "sourceNodeId": source,
            "sourceHandle": port,
            "targetNodeId": target,
            "targetHandle": "main",
            "order": 0,
        }
        for index, (source, port, target) in enumerate(edges)
    ]
    return definition


@pytest.fixture(scope="module")
def interaction_app(installed_agentx, service_urls, im_mock, run_id):
    with httpx.Client(base_url=service_urls["web"], timeout=60, trust_env=False) as control:
        token, me = _access_token(control)
        control.headers["Authorization"] = f"Bearer {token}"
        workflow = _passthrough_workflow(control, dict(control.headers), f"Workflow interactions {run_id}")
        hook_path = f"/input-hook/{run_id}-rate-limit"
        definition = interaction_definition(
            fixture_draft(control, workflow["workflowId"])["definition"], im_mock["url"] + hook_path
        )
        version = _save_version(control, workflow["workflowId"], definition)
        workflow["versionId"] = version["id"]
        environment = _development_environment_id(control, dict(control.headers))
        response = control.post(
            f"/api/v1/workflows/{workflow['workflowId']}/deployments",
            headers=idempotent_headers(dict(control.headers)),
            json={"contentSource": "version", "environmentId": environment, "workflowVersionId": version["id"]},
        )
        assert response.status_code in (200, 201), response.text
        app = send(
            control,
            "/applications",
            {
                "name": f"Interactions {run_id}",
                "slug": f"interactions-{run_id}",
                "workflowId": workflow["workflowId"],
                "visibility": "company",
            },
        )
        channel = _dingtalk_channel(control, dict(control.headers), app["id"], False)
        _publish_application(control, dict(control.headers), workflow, app["id"], environment)
        key = send(control, f"/applications/{app['id']}/api-keys", {"name": "Interaction API"})
        with httpx.Client(
            base_url=service_urls["runtime"],
            headers={"Authorization": f"Bearer {key['secret']}"},
            timeout=60,
            trust_env=False,
        ) as gateway:
            yield {
                "control": control,
                "gateway": gateway,
                "token": token,
                "me": me,
                "app": app,
                "workflow": workflow,
                "version": version,
                "definition": definition,
                "environment": environment,
                "channel": channel,
                "hookPath": hook_path,
                "installed": installed_agentx,
                "serviceUrls": service_urls,
                "runId": run_id,
            }


def invoke(state, *, platform=False, replies=None):
    client = state["gateway"]
    headers = {"Idempotency-Key": str(uuid.uuid4())}
    if platform:
        headers["Authorization"] = f"Bearer {state['token']}"
    body = {"input": {"message": "ASR: 开始处理"}, "responseMode": "async"}
    if replies is not None:
        body["waitInputs"] = replies
    response = poll(
        lambda: client.post(f"/gateway/v1/applications/{state['app']['slug']}/invocations", headers=headers, json=body),
        lambda value: value.status_code != 401,
    )
    assert response.status_code in (200, 202), response.text
    return response.json()


def invocation(state, identifier):
    response = state["gateway"].get(f"/gateway/v1/invocations/{identifier}")
    response.raise_for_status()
    return response.json()


def waiting(state, identifier, previous=None):
    def ready(value):
        assert value["status"] not in ("failed", "cancelled"), value
        return bool(value["activeWaits"] and value["activeWaits"][0]["waitId"] != previous)

    return poll(lambda: invocation(state, identifier), ready)


def test_api_hook_asr_resume_restart_and_event_stream(interaction_app):
    state = interaction_app
    accepted = invoke(state)
    current = waiting(state, accepted["id"])
    wait = current["activeWaits"][0]
    assert current["messages"][0]["content"] == "正在执行，请回复确认"
    assert current["outputs"] is None
    unauthenticated = httpx.post(
        state["serviceUrls"]["runtime"] + wait["resumeUrl"],
        headers={"Idempotency-Key": "no-api-key"},
        json={"content": "确认"},
        trust_env=False,
    )
    assert unauthenticated.status_code == 401
    observed = poll(
        lambda: _mock_received(state["installed"]["dependencies_namespace"], state["hookPath"]),
        lambda rows: sum(row["body"].get("waitId") == wait["waitId"] for row in rows) >= 3,
    )
    notices = [row["body"] for row in observed if row["body"].get("waitId") == wait["waitId"]]
    assert len({notice["eventId"] for notice in notices}) == 1
    assert notices[-1]["resumeUrl"] == wait["resumeUrl"]
    assert waiting(state, accepted["id"])["activeWaits"][0]["waitId"] == wait["waitId"], "Hook ACK must not resume"
    other = invoke(state)
    other_wait = waiting(state, other["id"])["activeWaits"][0]
    mismatched = state["gateway"].post(
        other_wait["resumeUrl"].replace(other_wait["waitId"], wait["waitId"]),
        headers={"Idempotency-Key": "wrong-invocation"},
        json={"content": "确认"},
    )
    assert mismatched.status_code == 404
    send(state["control"], f"/executions/{other['executionId']}/cancel", None)
    cancelled = poll(lambda: invocation(state, other["id"]), lambda value: value["status"] == "cancelled")
    assert not cancelled["activeWaits"]
    late = state["gateway"].post(other_wait["resumeUrl"], headers={"Idempotency-Key": "late"}, json={"content": "确认"})
    assert late.status_code == 409
    with state["gateway"].stream("GET", f"/gateway/v1/invocations/{accepted['id']}/events") as response:
        response.raise_for_status()
        seen = set()
        for line in response.iter_lines():
            if line.startswith("event:"):
                seen.add(line.partition(":")[2].strip())
            if "workflow.input_required" in seen:
                break
        assert "workflow.message" in seen
    namespace = state["installed"]["runtime_namespace"]
    run(("kubectl", "-n", namespace, "rollout", "restart", "deployment/workflow-runtime"), timeout=60)
    run(("kubectl", "-n", namespace, "rollout", "status", "deployment/workflow-runtime", "--timeout=180s"), timeout=210)
    reply = {"content": "我再考虑一下"}
    headers = {"Idempotency-Key": "voice-first"}
    first = state["gateway"].post(wait["resumeUrl"], headers=headers, json=reply)
    assert first.status_code == 202, first.text
    repeated = state["gateway"].post(wait["resumeUrl"], headers=headers, json=reply)
    assert repeated.json()["receiptId"] == first.json()["receiptId"]
    conflict = state["gateway"].post(wait["resumeUrl"], headers=headers, json={"content": "changed"})
    assert conflict.status_code == 409
    again = waiting(state, accepted["id"], wait["waitId"])
    assert again["executionId"] == accepted["executionId"]
    assert again["activeWaits"][0]["occurrence"] == 2
    assert len(again["messages"]) == 2, "the downstream condition decides to ask again"
    final = state["gateway"].post(
        again["activeWaits"][0]["resumeUrl"], headers={"Idempotency-Key": "voice-second"}, json={"content": "确认"}
    )
    assert final.status_code == 202, final.text
    completed = poll(
        lambda: invocation(state, accepted["id"]), lambda value: value["status"] in ("completed", "failed")
    )
    assert completed["status"] == "completed", completed
    assert completed["outputs"] == {"answer": "确认"}
    assert completed["executionId"] == accepted["executionId"]
    assert not completed["activeWaits"]


def test_platform_presets_and_exhaustion_do_not_send_hooks(interaction_app):
    state = interaction_app
    accepted = invoke(state, platform=True, replies={"/ask": ["我再考虑一下", "确认"]})
    completed = poll(
        lambda: invocation(state, accepted["id"]), lambda value: value["status"] in ("completed", "failed")
    )
    assert completed["status"] == "completed", completed
    rows = _runtime_mysql(
        state["installed"],
        f"SELECT COUNT(*) FROM delivery_outbox WHERE execution_id=UUID_TO_BIN('{accepted['executionId']}') AND provider='input_hook';",
    )
    assert rows.strip() == "0"
    forbidden = state["gateway"].post(
        f"/gateway/v1/applications/{state['app']['slug']}/invocations",
        headers={"Idempotency-Key": str(uuid.uuid4())},
        json={"input": {"message": "x"}, "waitInputs": {"/ask": ["确认"]}},
    )
    assert forbidden.status_code == 400 and forbidden.json()["code"] == "WAIT_PRESET_NOT_ALLOWED"
    missing = invoke(state, platform=True, replies={"/ask": ["not confirmation"]})
    failed = poll(lambda: invocation(state, missing["id"]), lambda value: value["status"] in ("completed", "failed"))
    assert failed["status"] == "failed", failed
    assert "WAIT_INPUT_EXHAUSTED" in json.dumps(failed)


def test_channel_next_reply_and_provider_redelivery_keep_one_invocation(interaction_app):
    state = interaction_app
    channel = state["channel"]
    conversation = f"interaction-{uuid.uuid4().hex}"

    def deliver(event, text, *, sender="voice-user", conversation_id=None):
        return _signed_dingtalk_post(
            state["gateway"],
            f"/gateway/v1/webhooks/{channel['publicId']}",
            "e2e-dingtalk-secret",
            {
                "msgId": event,
                "conversationId": conversation_id or conversation,
                "senderStaffId": sender,
                "conversationType": "1",
                "text": {"content": text},
            },
            f"http://im-mock.{state['installed']['dependencies_namespace']}.svc:8090/dingtalk/interaction-ok",
        )

    started = deliver("voice-start-" + conversation, "开始")
    assert started.status_code in (200, 202), started.text
    identifier = poll(
        lambda: _runtime_mysql(
            state["installed"],
            f"SELECT BIN_TO_UUID(id) FROM application_invocations WHERE application_id=UUID_TO_BIN('{state['app']['id']}') AND provider_event_id='voice-start-{conversation}';",
        ).strip(),
        bool,
    )
    current = waiting(state, identifier)
    for suffix, sender, room in (
        ("other-user", "second-voice-user", conversation),
        ("other-room", "voice-user", conversation + "-other"),
    ):
        event = f"{suffix}-{conversation}"
        response = deliver(event, "开始", sender=sender, conversation_id=room)
        assert response.status_code in (200, 202), response.text
        isolated_id = poll(
            lambda event=event: _runtime_mysql(
                state["installed"],
                f"SELECT BIN_TO_UUID(id) FROM application_invocations WHERE application_id=UUID_TO_BIN('{state['app']['id']}') AND provider_event_id='{event}';",
            ).strip(),
            bool,
        )
        assert isolated_id != identifier
        waiting(state, isolated_id)
        assert waiting(state, identifier)["activeWaits"][0]["waitId"] == current["activeWaits"][0]["waitId"]
        assert deliver(event + "-confirmed", "确认", sender=sender, conversation_id=room).status_code in (200, 202)
        completed_isolated = poll(
            lambda isolated_id=isolated_id: invocation(state, isolated_id),
            lambda value: value["status"] in ("completed", "failed"),
        )
        assert completed_isolated["status"] == "completed", completed_isolated
    first = deliver("voice-continue-" + conversation, "我再考虑一下")
    assert first.status_code in (200, 202), first.text
    again = waiting(state, identifier, current["activeWaits"][0]["waitId"])
    duplicate = deliver("voice-continue-" + conversation, "我再考虑一下")
    assert duplicate.status_code in (200, 202), duplicate.text
    assert waiting(state, identifier)["activeWaits"][0]["waitId"] == again["activeWaits"][0]["waitId"]
    assert deliver("voice-confirm-" + conversation, "确认").status_code in (200, 202)
    completed = poll(lambda: invocation(state, identifier), lambda value: value["status"] in ("completed", "failed"))
    assert completed["status"] == "completed", completed
    assert completed["executionId"] == current["executionId"]
    assert deliver("voice-confirm-" + conversation, "确认").status_code in (200, 202)


def test_channel_parallel_wait_conflict_uses_error_exit_and_releases_slot(interaction_app):
    state = interaction_app
    control = state["control"]
    workflow = _passthrough_workflow(control, dict(control.headers), f"Conflicting waits {uuid.uuid4().hex[:8]}")
    definition = copy.deepcopy(fixture_draft(control, workflow["workflowId"])["definition"])
    exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
    exit_node["parameters"]["outputs"] = {}
    definition["end"]["outputs"] = {}
    definition["nodes"] = [exit_node] + [
        {
            "id": name,
            "key": name,
            "name": name,
            "type": "await_input",
            "typeVersion": 1,
            "parameters": {},
            "contextWrites": [],
            "resourceReferences": [],
            "settings": {},
        }
        for name in ("first", "second")
    ]
    definition["connections"] = []
    for index, name in enumerate(("first", "second")):
        for source, port, target, order in (
            ("__start__", "main", name, index),
            (name, "main", exit_node["id"], 0),
            (name, "error", exit_node["id"], 0),
        ):
            definition["connections"].append(
                {
                    "id": f"{source}-{port}-{target}",
                    "sourceNodeId": source,
                    "sourceHandle": port,
                    "targetNodeId": target,
                    "targetHandle": "error" if port == "error" else "main",
                    "order": order,
                }
            )
    draft = fixture_draft(control, workflow["workflowId"])
    validation = send(
        control,
        f"/workflows/{workflow['workflowId']}/drafts/{draft['id']}/validate",
        {
            "definition": definition,
            "editorDocument": draft["editorDocument"],
        },
    )
    assert not [issue for issue in validation["issues"] if issue["severity"] == "error"], validation
    version = _save_version(control, workflow["workflowId"], definition)
    assert "id" in version, version
    workflow["versionId"] = version["id"]
    app = send(
        control,
        "/applications",
        {
            "name": f"Conflicting waits {uuid.uuid4().hex[:8]}",
            "slug": f"wait-conflict-{uuid.uuid4().hex[:8]}",
            "workflowId": workflow["workflowId"],
            "visibility": "company",
        },
    )
    channel = _dingtalk_channel(control, dict(control.headers), app["id"], False)
    _publish_application(control, dict(control.headers), workflow, app["id"], state["environment"])
    event = "parallel-wait-" + uuid.uuid4().hex
    with httpx.Client(base_url=state["serviceUrls"]["runtime"], timeout=60, trust_env=False) as channel_client:
        delivered = _signed_dingtalk_post(
            channel_client,
            f"/gateway/v1/webhooks/{channel['publicId']}",
            "e2e-dingtalk-secret",
            {
                "msgId": event,
                "conversationId": event,
                "senderStaffId": "voice-user",
                "conversationType": "1",
                "text": {"content": "start"},
            },
            f"http://im-mock.{state['installed']['dependencies_namespace']}.svc:8090/dingtalk/conflict-ok",
        )
    assert delivered.status_code in (200, 202), delivered.text
    execution = poll(
        lambda: _runtime_mysql(
            state["installed"],
            f"SELECT BIN_TO_UUID(execution_id) FROM application_invocations WHERE application_id=UUID_TO_BIN('{app['id']}') AND provider_event_id='{event}';",
        ).strip(),
        bool,
    )
    result = poll(
        lambda: read(control, f"/executions/{execution}"), lambda value: value["status"] in ("succeeded", "failed")
    )
    assert result["status"] == "failed", result
    nodes = read(control, f"/executions/{execution}/nodes")["items"]
    assert any(node["errorCode"] == "CHANNEL_WAIT_CONFLICT" for node in nodes), nodes
    history = read(control, f"/executions/{execution}/runtime-details")
    assert len(history["inputWaits"]) == 1 and history["inputWaits"][0]["status"] == "cancelled"
    slots = _runtime_mysql(
        state["installed"],
        f"SELECT COUNT(*) FROM input_waits WHERE execution_id=UUID_TO_BIN('{execution}') AND active_scope IS NOT NULL;",
    )
    assert slots.strip() == "0"


def test_manual_subworkflow_uses_qualified_reply_paths(interaction_app):
    state = interaction_app
    control = state["control"]
    parent = _passthrough_workflow(control, dict(control.headers), f"Parent interactions {uuid.uuid4().hex[:8]}")
    definition = copy.deepcopy(fixture_draft(control, parent["workflowId"])["definition"])
    exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
    exit_node["parameters"]["outputs"] = {"answer": _reference("child", ["answer"])}
    definition["end"]["outputs"]["answer"]["schema"] = {}
    definition["nodes"] = [
        exit_node,
        {
            "id": "child",
            "key": "child",
            "name": "Child",
            "type": "sub_workflow",
            "typeVersion": 1,
            "parameters": {
                "workflowVersionId": state["version"]["id"],
                "inputs": {"kind": "object", "fields": {"message": {"kind": "literal", "value": "nested"}}},
            },
            "contextWrites": [],
            "resourceReferences": [],
            "settings": {},
        },
    ]
    definition["connections"] = [
        {
            "id": "start-child",
            "sourceNodeId": "__start__",
            "sourceHandle": "main",
            "targetNodeId": "child",
            "targetHandle": "main",
            "order": 0,
        },
        {
            "id": "child-end",
            "sourceNodeId": "child",
            "sourceHandle": "main",
            "targetNodeId": exit_node["id"],
            "targetHandle": "main",
            "order": 0,
        },
    ]
    version = _save_version(control, parent["workflowId"], definition)
    catalog = send(control, f"/workflows/{parent['workflowId']}/wait-input-nodes", {"definition": definition})
    assert [node["path"] for node in catalog] == ["/child/ask"]
    accepted = send(
        control,
        f"/workflow-versions/{version['id']}/executions",
        {"input": {"message": "nested"}, "waitInputs": {"/child/ask": ["还没准备好", "确认"]}},
    )
    result = poll(
        lambda: read(control, f"/executions/{accepted['executionId']}"),
        lambda value: value["status"] in ("succeeded", "failed"),
    )
    assert result["status"] == "succeeded", result
    history = read(control, f"/executions/{accepted['executionId']}/runtime-details")
    assert [wait["nodePath"] for wait in history["inputWaits"]] == ["/child/ask", "/child/ask"]
    assert [wait["content"] for wait in history["inputWaits"]] == ["还没准备好", "确认"]


def test_multiple_wait_nodes_skip_unselected_branch_and_scope_debug(interaction_app):
    state = interaction_app
    control = state["control"]
    workflow = _passthrough_workflow(control, dict(control.headers), f"Branch replies {uuid.uuid4().hex[:8]}")
    definition = interaction_definition(fixture_draft(control, workflow["workflowId"])["definition"], "")
    exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
    exit_node["parameters"]["outputs"] = {}
    definition["end"]["outputs"] = {}
    template = copy.deepcopy(next(node for node in definition["nodes"] if node["id"] == "ask"))
    for identifier in ("details", "alternate"):
        node = copy.deepcopy(template)
        node.update(id=identifier, key=identifier, name=identifier)
        definition["nodes"].append(node)
    intent = next(node for node in definition["nodes"] if node["id"] == "intent")
    case = copy.deepcopy(intent["parameters"]["cases"][0])
    case.update(id="alternate", name="Alternate")
    case["conditions"][0]["condition"]["right"]["value"] = "alternate"
    intent["parameters"]["cases"].append(case)
    confirmed = next(edge for edge in definition["connections"] if edge["sourceHandle"] == "case:confirmed")
    confirmed["targetNodeId"] = "details"
    for source, port, target in (
        ("intent", "case:alternate", "alternate"),
        ("details", "main", exit_node["id"]),
        ("alternate", "main", exit_node["id"]),
    ):
        definition["connections"].append(
            {
                "id": f"{source}-{target}",
                "sourceNodeId": source,
                "sourceHandle": port,
                "targetNodeId": target,
                "targetHandle": "main",
                "order": 0,
            }
        )
    version = _save_version(control, workflow["workflowId"], definition)
    plan = {"/ask": ["稍等", "确认"], "/details": [{"raw": "details"}], "/alternate": []}
    accepted = send(
        control,
        f"/workflow-versions/{version['id']}/executions",
        {"input": {"message": "branches"}, "waitInputs": plan},
    )
    result = poll(
        lambda: read(control, f"/executions/{accepted['executionId']}"),
        lambda value: value["status"] in ("succeeded", "failed"),
    )
    assert result["status"] == "succeeded", result
    history = read(control, f"/executions/{accepted['executionId']}/runtime-details")
    assert [(wait["nodePath"], wait["occurrence"], wait["content"]) for wait in history["inputWaits"]] == [
        ("/ask", 1, "稍等"),
        ("/ask", 2, "确认"),
        ("/details", 1, {"raw": "details"}),
    ]
    scoped = send(
        control,
        f"/workflows/{workflow['workflowId']}/wait-input-nodes",
        {"definition": definition, "includedNodeIds": ["details"]},
    )
    assert [node["path"] for node in scoped] == ["/details"]
    edit = send(control, f"/workflows/{workflow['workflowId']}/versions/{version['id']}/edit", None)["document"]
    debug = send(
        control,
        f"/workflows/{workflow['workflowId']}/drafts/{edit['id']}/debug-executions",
        {
            "expectedRevision": edit["revision"],
            "mode": "single_node",
            "targetNodeId": "details",
            "input": {"message": "debug"},
            "inputSource": {"kind": "manual", "value": {"message": "debug"}},
            "waitInputs": {"/details": ["debug raw reply"]},
            "sideEffectDecisions": {},
        },
    )
    result = poll(
        lambda: read(control, f"/executions/{debug['executionId']}"),
        lambda value: value["status"] in ("succeeded", "failed"),
    )
    assert result["status"] == "succeeded", result
    history = read(control, f"/executions/{debug['executionId']}/runtime-details")
    assert [wait["content"] for wait in history["inputWaits"]] == ["debug raw reply"]


def test_dataset_reply_snapshots_reports_and_schedule_presets(interaction_app, e2e_providers):
    state = interaction_app
    control = state["control"]
    path = f"/applications/{state['app']['id']}"
    credential = _fixture_credential(control, dict(control.headers), state["me"]["departmentId"])
    alias = f"wait-judge-{uuid.uuid4().hex[:8]}"
    _upsert_model(
        control,
        dict(control.headers),
        alias,
        e2e_providers["echo_mcp"],
        "echo-model",
        state["me"]["departmentId"],
        credential,
    )
    select_model(control, read(control, f"/models/aliases?search={alias}")["items"][0])
    dataset = send(
        control,
        path + "/datasets",
        {
            "name": "Raw input sequence",
            "workflowVersionId": state["version"]["id"],
            "initialCase": {
                "name": "ASR rounds",
                "input": {"message": "test"},
                "expectedOutput": {"answer": "确认"},
                "waitInputs": {"/ask": ["稍等", "确认"]},
                "tags": [],
                "referenceConfirmed": True,
            },
        },
    )
    cases = read(control, f"{path}/datasets/{dataset['id']}/cases")
    exported = control.post(
        f"/api/v1{path}/datasets/{dataset['id']}/export",
        json={"expectedRevision": dataset["revision"], "caseIds": [cases[0]["id"]]},
    )
    exported.raise_for_status()
    assert json.loads(exported.text)["waitInputs"] == {"/ask": ["稍等", "确认"]}
    profile = send(
        control,
        path + "/evaluation-profiles",
        {"name": "Interaction judge", "criteria": "Check the answer.", "passThreshold": 0.5},
    )
    batch = send(
        control,
        path + "/evaluations",
        {
            "name": "Interaction evaluation",
            "profileVersionId": profile["versionId"],
            "selections": [
                {
                    "datasetId": dataset["id"],
                    "expectedRevision": dataset["revision"],
                    "workflowVersionIds": [state["version"]["id"]],
                }
            ],
        },
    )
    report = finished(control, path, batch["id"])
    case = report["runs"][0]["cases"][0]
    assert case["actualOutput"] == {"answer": "确认"}, case
    assert [wait["content"] for wait in case["inputWaits"]] == ["稍等", "确认"]
    assert case["messages"][0]["content"] == "正在执行，请回复确认"
    schedule = send(
        control,
        path + "/schedules",
        {
            "name": "Preset input schedule",
            "cronExpression": "*/10 * * * * *",
            "timezone": "Asia/Shanghai",
            "input": {"message": "scheduled"},
            "waitInputs": {"/ask": ["确认"]},
            "misfirePolicy": "fire_once",
        },
    )
    assert schedule["waitInputs"] == {"/ask": ["确认"]}
    _publish_application(control, dict(control.headers), state["workflow"], state["app"]["id"], state["environment"])
    count = poll(
        lambda: _runtime_mysql(
            state["installed"],
            f"SELECT COUNT(*) FROM workflow_executions WHERE application_id=UUID_TO_BIN('{state['app']['id']}') AND trigger_type='schedule' AND status='succeeded';",
        ),
        lambda value: int(value.strip() or 0) > 0,
    )
    assert int(count.strip()) > 0
    hooks = _runtime_mysql(
        state["installed"],
        f"SELECT COUNT(*) FROM delivery_outbox d JOIN workflow_executions e ON e.id=d.execution_id WHERE e.application_id=UUID_TO_BIN('{state['app']['id']}') AND e.trigger_type='schedule' AND d.provider='input_hook';",
    )
    assert hooks.strip() == "0"


def test_hook_dead_letter_keeps_wait_and_allows_later_input(interaction_app):
    state = interaction_app
    control = state["control"]
    workflow = _passthrough_workflow(control, dict(control.headers), f"Dead Hook {uuid.uuid4().hex[:8]}")
    definition = interaction_definition(
        fixture_draft(control, workflow["workflowId"])["definition"],
        state["definition"]["nodes"][2]["parameters"]["hookUrl"]["value"].replace("rate-limit", "not-found"),
    )
    version = _save_version(control, workflow["workflowId"], definition)
    workflow["versionId"] = version["id"]
    send(
        control,
        f"/workflows/{workflow['workflowId']}/deployments",
        {
            "contentSource": "version",
            "environmentId": state["environment"],
            "workflowVersionId": version["id"],
        },
    )
    app = send(
        control,
        "/applications",
        {
            "name": f"Dead Hook {uuid.uuid4().hex[:8]}",
            "slug": f"dead-hook-{uuid.uuid4().hex[:8]}",
            "workflowId": workflow["workflowId"],
            "visibility": "company",
        },
    )
    _publish_application(control, dict(control.headers), workflow, app["id"], state["environment"])
    key = send(control, f"/applications/{app['id']}/api-keys", {"name": "Dead Hook API"})
    with httpx.Client(
        base_url=state["serviceUrls"]["runtime"],
        headers={"Authorization": f"Bearer {key['secret']}"},
        timeout=60,
        trust_env=False,
    ) as gateway:
        isolated = {**state, "gateway": gateway, "app": app}
        accepted = invoke(isolated)
        wait = waiting(isolated, accepted["id"])["activeWaits"][0]
        deliveries = poll(
            lambda: read(control, f"/deliveries?applicationId={app['id']}&invocationId={accepted['id']}"),
            lambda value: any(item["status"] == "dead" for item in value["items"]),
        )
        assert deliveries["items"][0]["lastErrorCode"] == "WAIT_HOOK_REJECTED"
        assert waiting(isolated, accepted["id"])["activeWaits"][0]["waitId"] == wait["waitId"]
        denied = state["gateway"].post(
            wait["resumeUrl"], headers={"Idempotency-Key": "wrong-app"}, json={"content": "确认"}
        )
        assert denied.status_code in (401, 403, 404)
        submitted = gateway.post(
            wait["resumeUrl"], headers={"Idempotency-Key": "after-dead-hook"}, json={"content": "确认"}
        )
        assert submitted.status_code == 202, submitted.text
        completed = poll(
            lambda: invocation(isolated, accepted["id"]), lambda value: value["status"] in ("completed", "failed")
        )
        assert completed["status"] == "completed", completed


def test_interaction_configuration_and_playground_desktop(interaction_app):
    state = interaction_app
    fixture = Path(state["installed"]["artifact_dir"]) / "workflow-interaction-fixture.json"
    fixture.write_text(
        json.dumps(
            {
                "applicationId": state["app"]["id"],
                "workflowId": state["workflow"]["workflowId"],
                "versionId": state["version"]["id"],
            }
        ),
        encoding="utf-8",
    )
    run_playwright(
        Path(state["installed"]["root"]),
        "workflow-interactions",
        ("tests/workflow-interactions.spec.ts",),
        {
            **os.environ,
            "AGENTX_E2E_RUN_ID": state["runId"],
            "AGENTX_E2E_STAGE": "helm-agentxctl",
            "AGENTX_E2E_BASE_URL": state["serviceUrls"]["web"],
            "AGENTX_E2E_RUNTIME_URL": state["serviceUrls"]["runtime"],
            "AGENTX_INTERACTION_FIXTURE": str(fixture),
        },
    )
