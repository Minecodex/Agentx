"""Shared real Control/Runtime and authenticated transport fixtures for closure."""

from __future__ import annotations

import copy
import json
import secrets
import socket
import time

import httpx
import pytest
import yaml

from tests.e2e.draft_fixtures import fixture_draft, fixture_draft_id, publish_fixture_version
from tests.e2e.product.live_text_support import Secret, post
from tests.e2e.product.next_batch_support import get, publish_application
from tests.e2e.product.test_model_streaming import _grant_model_to_workflow, _stream_workflow
from tests.e2e.product.user_auth import UserTokenAuth
from tests.e2e.runtime.test_agent_attachments import _access_token
from tests.e2e.support import ROOT, RestartingPortForward, run


def boundary_echo_documents(rendered, image):
    """Own a separate authenticated Echo service without changing shared providers."""
    documents = copy.deepcopy(
        [
            item
            for item in rendered
            if item and item["metadata"]["name"] == "echo-mcp" and item["kind"] in {"Service", "Deployment"}
        ]
    )
    for document in documents:
        document["metadata"]["name"] = "boundary-echo-mcp"
        if document["kind"] == "Service":
            document["spec"]["selector"]["app.kubernetes.io/name"] = "boundary-echo-mcp"
        else:
            document["spec"]["selector"]["matchLabels"]["app.kubernetes.io/name"] = "boundary-echo-mcp"
            pod = document["spec"]["template"]
            pod["metadata"]["labels"]["app.kubernetes.io/name"] = "boundary-echo-mcp"
            container = pod["spec"]["containers"][0]
            container["image"] = image
            container.setdefault("env", []).append(
                {
                    "name": "AGENTX_FIXTURE_MCP_AUTH_TOKEN",
                    "valueFrom": {"secretKeyRef": {"name": "boundary-mcp-auth", "key": "token"}},
                }
            )
    return documents


@pytest.fixture(scope="session")
def boundary_state(installed_agentx, service_urls, run_id):
    namespace = installed_agentx["dependencies_namespace"]
    rendered = run(("kubectl", "kustomize", ROOT / "deploy/kustomize/e2e-fixtures/runtime-providers")).stdout
    config = yaml.safe_load((ROOT / installed_agentx["values"]).read_text())
    images = config["global"]["images"]
    documents = boundary_echo_documents(
        list(yaml.safe_load_all(rendered)), f"{images['registry']}/echo-mcp:{images['tag']}"
    )
    key = secrets.token_urlsafe(24)
    documents.append(
        {"apiVersion": "v1", "kind": "Secret", "metadata": {"name": "boundary-mcp-auth"}, "stringData": {"token": key}}
    )
    run(("kubectl", "-n", namespace, "apply", "-f", "-"), input_text=yaml.safe_dump_all(documents))
    run(
        ("kubectl", "-n", namespace, "rollout", "status", "deployment/boundary-echo-mcp", "--timeout=300s"), timeout=330
    )
    with httpx.Client(base_url=service_urls["web"], timeout=60) as control:
        token, me = _access_token(control)
        token = Secret(token)
        control.auth = UserTokenAuth(service_urls["web"], token)
        control.headers["Authorization"] = f"Bearer {token.value}"
        yield {
            "control": control,
            "me": me,
            "token": token,
            "echo": f"http://boundary-echo-mcp.{namespace}.svc:8090",
            "mcpKey": Secret(key),
            "context": installed_agentx,
            "urls": service_urls,
            "runId": run_id,
        }


def reference(kind, rid, version=None, operation="use", role=None):
    result = {"resourceType": kind, "resourceId": rid, "resourceVersionId": version, "operation": operation}
    if role:
        result["bindingRole"] = role
    return result


def grant(control, kind, rid, department, version=None, operation="use"):
    return post(
        control,
        f"/resources/{kind}/{rid}/grants",
        {"subjectType": "department", "subjectId": department, "resourceVersionId": version, "operation": operation},
    )


def authorize(control, workflow, resource):
    return post(control, f"/workflows/{workflow}/resource-authorizations", resource)


def new_model(state, suffix, *, upstream="echo-model", price=None, endpoint=None, secret=None):
    control = state["control"]
    department = state["me"]["departmentId"]
    secret = secret or "m5-model-secret"
    credential = post(
        control,
        "/credentials",
        {
            "name": f"Boundary {suffix} {state['runId']}",
            "credentialType": "bearer",
            "secret": secret,
            "ownerDepartmentId": department,
        },
    )
    alias = f"boundary-{suffix}-{state['runId']}"
    model = post(
        control,
        "/models/aliases",
        {
            "connectionName": alias,
            "providerType": "openai_compatible",
            "endpoint": endpoint or f"{state['echo']}/v1",
            "credentialId": credential["id"],
            "ownerDepartmentId": department,
            "alias": alias,
            "modelName": upstream,
            "price": price or {"currency": "USD", "inputPerMillion": "1.25", "outputPerMillion": "2.5"},
        },
    )
    return {**model, "credential": credential}


def model_workflow(state, model, suffix, *, timeout_ms=None):
    control = state["control"]
    workflow = _stream_workflow(control, dict(control.headers), f"Boundary {suffix} {state['runId']}", model["id"])
    _grant_model_to_workflow(control, dict(control.headers), model["alias"], workflow, model["credential"]["id"])
    draft = get(control, f"/workflows/{workflow}/drafts/{fixture_draft_id(control, workflow, None)}")
    node = next(n for n in draft["definition"]["nodes"] if n["type"] == "model")
    node["resourceReferences"][0]["resourceVersionId"] = model["deploymentId"]
    if timeout_ms:
        node["settings"] = {"timeoutMs": timeout_ms}
    version = save_version(control, workflow, draft["definition"])
    return {"workflowId": workflow, "versionId": version["id"]}


def save_version(control, workflow, definition):
    revision = get(control, f"/workflows/{workflow}/drafts/{fixture_draft_id(control, workflow, None)}")["revision"]
    response = control.put(
        f"/api/v1/workflows/{workflow}/drafts/{fixture_draft_id(control, workflow, None)}",
        json={
            "editorDocument": fixture_draft(control, workflow, None)["editorDocument"],
            "expectedRevision": revision,
            "definition": definition,
        },
    )
    assert response.status_code == 200, response.text
    return publish_fixture_version(
        control,
        workflow,
        fixture_draft(control, workflow, None)["id"],
        get(control, f"/workflows/{workflow}/drafts/{fixture_draft_id(control, workflow, None)}")["revision"],
        None,
    ).json()


def agent_definition(control, workflow, model, tool=None, skill=None):
    definition = copy.deepcopy(
        get(control, f"/workflows/{workflow}/drafts/{fixture_draft_id(control, workflow, None)}")["definition"]
    )
    node = next(n for n in definition["nodes"] if n["type"] == "model")
    question = node["parameters"]["userQuestion"]
    node["type"] = "agent"
    node["typeVersion"] = 2
    node["parameters"] = {
        "systemPrompt": {
            "kind": "template",
            "segments": [{"kind": "text", "text": "Use the authorized echo tool then answer. P3-04 Skill attachment"}],
        },
        "userQuestion": question,
        "sessionPolicy": {"mode": "invocation"},
        "maxIterations": 4,
        "maxModelCalls": 4,
        "maxToolCalls": 4,
        "maxTotalTokens": 4000,
        "maxOutputTokens": 512,
        "maxCost": 1,
        "maxDurationSeconds": 90,
        "limitAction": "fail",
    }
    node["resourceReferences"] = [reference("model", model["id"], model["deploymentId"], role="model")]
    if tool:
        node["resourceReferences"].append(reference("mcp_tool", tool["id"], tool["currentVersionId"], role="mcp_tools"))
    if skill:
        node["resourceReferences"].append(reference("skill", skill["id"], skill["versionId"], role="skills"))
    return definition


def start_execution(state, workflow, input_value=None):
    control = state["control"]
    draft = get(control, f"/workflows/{workflow}/drafts/{fixture_draft_id(control, workflow, None)}")
    response = control.post(
        f"/api/v1/workflows/{workflow}/drafts/{fixture_draft_id(control, workflow, None)}/debug-executions",
        json={
            "expectedRevision": draft["revision"],
            "mode": "full",
            "targetNodeId": None,
            "input": input_value or {"message": "boundary proof"},
            "context": {},
            "overlayIds": [],
            "sideEffectDecisions": {
                node["id"]: "execute" for node in draft["definition"]["nodes"] if node["type"] in {"agent", "code"}
            },
            "idempotencyKey": f"boundary-{time.time_ns()}",
        },
    )
    return response


def invoke(state, workflow, *, expected="succeeded", input_value=None):
    response = start_execution(state, workflow, input_value)
    assert response.status_code == 202, f"{response.status_code}: {response.text}"
    execution = response.json()["executionId"]
    deadline = time.monotonic() + 180
    while True:
        detail = get(state["control"], f"/executions/{execution}")
        if detail["status"] in {"succeeded", "failed", "cancelled"}:
            assert detail["status"] == expected, detail
            return execution, detail
        assert time.monotonic() < deadline, detail
        time.sleep(0.5)


def publish_chat(state, workflow, suffix):
    return publish_application(
        state["control"], workflow["workflowId"], workflow["versionId"], f"boundary-{suffix}-{state['runId']}"
    )


def report(state, name, value):
    from tools.scripts.release.evidence import write_report

    write_report(state["context"], f"product/{name}.json", {"status": "passed", **value})


def profile(control, model, name):
    return post(
        control,
        "/evaluation-profiles",
        {
            "name": name,
            "visibility": "company",
            "aggregation": "all",
            "passThreshold": "1",
            "rules": [
                {
                    "key": "judge",
                    "name": "Judge",
                    "evaluatorType": "llm_judge",
                    "configuration": {
                        "modelId": model["id"],
                        "prompt": "Compare {{actualOutput}} against {{expectedOutput}}. Return JSON passed, score, reason.",
                    },
                    "weight": "1",
                    "required": True,
                }
            ],
        },
    )


def evaluation(control, workflow, profile_value, name, keys=("shared",), *, expected_start=202):
    dataset = post(control, "/datasets", {"name": name, "visibility": "company"})
    post(
        control,
        f"/datasets/{dataset['id']}/import",
        {
            "expectedRevision": dataset["revision"],
            "format": "jsonl",
            "content": "\n".join(
                json.dumps({"caseKey": k, "name": k, "input": {"message": k}, "expectedOutput": {"answer": k}})
                for k in keys
            ),
        },
    )
    version = post(control, f"/datasets/{dataset['id']}/versions", {"expectedRevision": dataset["revision"] + 1})
    result = post(
        control,
        "/evaluations",
        {
            "name": name,
            "workflowVersionId": workflow["versionId"],
            "datasetVersionId": version["id"],
            "evaluationProfileVersionId": profile_value["versionId"],
            "visibility": "company",
            "parameters": {"concurrency": 1},
        },
    )
    started = control.post(f"/api/v1/evaluations/{result['id']}/start")
    assert started.status_code == expected_start, started.text
    if expected_start != 202:
        return result["id"], started.json()
    deadline = time.monotonic() + 240
    while True:
        value = get(control, f"/evaluations/{result['id']}/report")
        if value["run"]["status"] in {"completed", "failed", "cancelled"}:
            return result["id"], value
        assert time.monotonic() < deadline, value
        time.sleep(0.5)


@pytest.fixture(scope="session")
def boundary_gateway_metrics(installed_agentx):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    directory = ROOT / installed_agentx["artifact_dir"]
    process = RestartingPortForward(
        (
            "kubectl",
            "-n",
            installed_agentx["runtime_namespace"],
            "port-forward",
            "deployment/runtime-gateway",
            f"{port}:9092",
        ),
        stdout_path=directory / "boundary-metrics.log",
        stderr_path=directory / "boundary-metrics-error.log",
        health_url=f"http://127.0.0.1:{port}/metrics",
    )
    try:
        url = f"http://127.0.0.1:{port}"
        deadline = time.monotonic() + 30
        while True:
            try:
                response = httpx.get(f"{url}/metrics", timeout=3)
                if response.status_code == 200:
                    break
            except httpx.HTTPError:
                pass
            assert time.monotonic() < deadline, "Gateway metrics did not become available"
            time.sleep(0.25)
        yield url
    finally:
        process.stop()
