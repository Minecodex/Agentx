"""MCP and transitive Skill approvals, live tool execution and refusal."""

# ruff: noqa: S608 -- UUIDs in isolated SQL are returned by the typed test API.

from __future__ import annotations

import time

import pytest

from tests.e2e.product.live_text_support import post
from tests.e2e.product.next_batch_support import authenticated, create_user, get, review
from tests.e2e.product.remaining_support import (
    agent_definition,
    authorize,
    grant,
    invoke,
    model_workflow,
    new_model,
    reference,
    report,
    save_version,
    start_execution,
)

pytestmark = [pytest.mark.cluster, pytest.mark.product]


@pytest.fixture(scope="module")
def mcp_package(boundary_state):
    state = boundary_state
    control = state["control"]
    root = state["me"]["departmentId"]
    departments = [
        post(control, "/departments", {"parentId": root, "name": f"Boundary {kind} {state['runId']}"})
        for kind in ("secret", "mcp", "skill")
    ]
    credential = post(
        control,
        "/credentials",
        {
            "name": f"Boundary MCP credential {state['runId']}",
            "credentialType": "bearer",
            "secret": state["mcpKey"].value,
            "ownerDepartmentId": departments[0]["id"],
        },
    )
    grant(control, "credential", credential["id"], departments[1]["id"])
    server = post(
        control,
        "/mcp/servers",
        {
            "name": f"Boundary authenticated MCP {state['runId']}",
            "ownerDepartmentId": departments[1]["id"],
            "transport": {
                "kind": "streamable_http",
                "endpoint": f"{state['echo']}/mcp",
                "bearerCredentialId": credential["id"],
            },
            "configuration": {},
        },
    )
    discovery = post(control, f"/mcp/servers/{server['id']}/discover", {})
    tool = next(t for t in discovery["tools"] if t["name"] == "echo")
    role = post(
        control,
        "/roles",
        {
            "code": f"boundary_editor_{state['runId']}",
            "name": f"Boundary editor {state['runId']}",
            "dataScope": "company",
            "permissions": [
                "workflow:view",
                "workflow:create",
                "workflow:edit",
                "workflow:publish",
                "execution:view",
                "execution:run",
                "trace:view",
                "model:view",
                "credential:view",
                "mcp:view",
                "skill:view",
                "skill:manage",
                "approval:view",
            ],
        },
    )
    editor = create_user(control, f"boundary-editor-{state['runId']}", root, role["id"])
    reviewer_role = next(r for r in get(control, "/roles?pageSize=100")["items"] if r["code"] == "department_admin")
    reviewers = [
        create_user(control, f"boundary-review-{i}-{state['runId']}", d["id"], reviewer_role["id"])
        for i, d in enumerate(departments)
    ]
    return {
        **state,
        "departments": departments,
        "credential": credential,
        "server": server,
        "tool": tool,
        "editor": editor,
        "reviewers": reviewers,
    }


def skill_package(state, suffix, dependencies):
    control = state["control"]
    owner = state["departments"][2]["id"]
    for dependency in dependencies:
        grant(
            control,
            dependency["resourceType"],
            dependency["resourceId"],
            owner,
            dependency.get("resourceVersionId"),
            dependency["operation"],
        )
    if any(d["resourceType"] == "mcp_tool" for d in dependencies):
        grant(control, "mcp_server", state["server"]["id"], owner)
        grant(control, "credential", state["credential"]["id"], owner)
    skill = post(
        control,
        "/skills",
        {
            "name": f"Boundary Skill {suffix} {state['runId']}",
            "alias": f"boundary-{suffix}-{state['runId']}",
            "description": "P3-04 Skill attachment used by the real MCP permission test",
            "ownerDepartmentId": owner,
        },
    )
    version = post(
        control,
        f"/skills/{skill['id']}/versions",
        {"expectedRevision": skill["draftRevision"], "dependencies": dependencies},
    )
    activated = control.patch(
        f"/api/v1/skills/{skill['id']}",
        json={
            "name": skill["name"],
            "alias": skill["alias"],
            "description": skill["description"],
            "status": "active",
            "version": skill["version"],
        },
    )
    assert activated.status_code == 200, activated.text
    return {**activated.json(), "versionId": version["id"]}


def workflow_request(state, resource, suffix):
    with authenticated(state["urls"]["web"], state["editor"]["token"]) as editor:
        workflow = post(editor, "/workflows", {"name": f"Boundary {suffix} {state['runId']}", "visibility": "company"})[
            "id"
        ]
        body = {
            **resource,
            "sourceRevision": get(editor, f"/workflows/{workflow}/draft")["revision"],
            "message": "Verify the complete dependency package",
        }
        request = post(editor, f"/workflows/{workflow}/resource-grant-requests", body)
        denied = editor.post(f"/api/v1/workflows/{workflow}/resource-authorizations", json=resource)
        assert denied.status_code == 403, denied.text
    return workflow, request


def workflow_grants(state, workflow, request):
    identity = get(state["control"], f"/workflows/{workflow}")["serviceIdentityId"]
    return [
        g
        for item in request["items"]
        for g in get(state["control"], f"/resources/{item['resourceType']}/{item['resourceId']}/grants")
        if g["subjectId"] == identity
    ]


def approve_all(state, workflow, request):
    assert not workflow_grants(state, workflow, request)
    for index, own in enumerate(request["reviews"]):
        department = own["ownerDepartmentId"]
        i = next(i for i, d in enumerate(state["departments"]) if d["id"] == department)
        with authenticated(state["urls"]["web"], state["reviewers"][i]["token"]) as reviewer:
            view = get(reviewer, f"/resource-grant-requests/{request['id']}")
            assert all(
                (item["name"] is not None) == (item["ownerDepartmentId"] == department) for item in view["items"]
            ), view
            request = review(reviewer, request, department)
        assert request["status"] == ("approved" if index == len(request["reviews"]) - 1 else "pending"), request
        if request["status"] == "pending":
            assert not workflow_grants(state, workflow, request)
    assert len(workflow_grants(state, workflow, request)) == len(request["items"])
    return request


def test_mcp_tool_dependency_approval_executes_and_revocation_refuses(mcp_package):
    state = mcp_package
    tool = state["tool"]
    workflow, request = workflow_request(
        state, reference("mcp_tool", tool["id"], tool["currentVersionId"]), "MCP approval"
    )
    assert {i["resourceType"] for i in request["items"]} == {"mcp_tool", "mcp_server", "credential"}, request
    assert len(request["reviews"]) == 2, request
    final = approve_all(state, workflow, request)
    model = new_model(state, "mcp")
    template = model_workflow(state, model, "MCP template")
    authorize(state["control"], workflow, reference("model", model["id"], model["deploymentId"]))
    definition = agent_definition(state["control"], template["workflowId"], model, tool)
    save_version(state["control"], workflow, definition)
    execution, _ = invoke(state, workflow)
    calls = get(state["control"], f"/executions/{execution}/runtime-details")["calls"]
    assert any(c["callKind"] == "mcp_tool" and c["status"] == "succeeded" for c in calls), calls
    with authenticated(state["urls"]["web"], state["reviewers"][0]["token"]) as unrelated:
        outside = unrelated.get(f"/api/v1/executions/{execution}")
        assert outside.status_code in {403, 404}, outside.text
    credential_grant = next(g for g in workflow_grants(state, workflow, final) if g["resourceType"] == "credential")
    response = state["control"].delete(
        f"/api/v1/resources/credential/{state['credential']['id']}/grants/{credential_grant['id']}"
    )
    assert response.status_code == 204, response.text
    denied = start_execution(state, workflow, {"message": "must be refused after credential revoke"})
    if denied.status_code == 202:
        execution = denied.json()["executionId"]
        deadline = time.monotonic() + 90
        while get(state["control"], f"/executions/{execution}")["status"] not in {"failed", "succeeded"}:
            assert time.monotonic() < deadline
            time.sleep(0.5)
        assert get(state["control"], f"/executions/{execution}")["status"] == "failed"
        assert not any(
            c["callKind"] == "mcp_tool" and c["status"] == "succeeded"
            for c in get(state["control"], f"/executions/{execution}/runtime-details")["calls"]
        )
    else:
        assert denied.status_code in {403, 422}, denied.text
    report(
        state,
        "remaining-mcp-approval",
        {
            "requestId": final["id"],
            "executionId": execution,
            "atomicPackage": True,
            "redaction": True,
            "unrelatedReviewerCannotReadExecution": True,
            "credentialRevocationRefused": True,
        },
    )


def test_canonical_nested_skill_expands_and_approves_full_package(mcp_package):
    state = mcp_package
    leaf = skill_package(state, "leaf", [reference("mcp_tool", state["tool"]["id"], state["tool"]["currentVersionId"])])
    parent = skill_package(state, "parent", [reference("skill", leaf["id"], leaf["versionId"])])
    workflow, request = workflow_request(state, reference("skill", parent["id"], parent["versionId"]), "nested Skill")
    assert len(request["items"]) == 5, request
    assert {i["resourceType"] for i in request["items"]} == {"skill", "mcp_tool", "mcp_server", "credential"}, request
    final = approve_all(state, workflow, request)
    model = new_model(state, "skill")
    template = model_workflow(state, model, "Skill template")
    authorize(state["control"], workflow, reference("model", model["id"], model["deploymentId"]))
    save_version(
        state["control"],
        workflow,
        agent_definition(state["control"], template["workflowId"], model, state["tool"], parent),
    )
    execution, _ = invoke(state, workflow)
    calls = get(state["control"], f"/executions/{execution}/runtime-details")["calls"]
    assert any(c["callKind"] == "mcp_tool" and c["status"] == "succeeded" for c in calls), calls
    report(
        state,
        "remaining-nested-skill",
        {"requestId": final["id"], "executionId": execution, "dependencyCount": 5, "reviewDepartments": 3},
    )


def test_skill_dependency_cannot_reuse_another_subject_grant(mcp_package):
    state = mcp_package
    # The MCP owner has a credential grant. That does not authorize an
    # unrelated Skill owner department to publish the same dependency.
    peer = post(
        state["control"],
        "/departments",
        {"parentId": state["me"]["departmentId"], "name": f"Boundary isolated Skill owner {state['runId']}"},
    )
    with authenticated(state["urls"]["web"], state["editor"]["token"]) as editor:
        skill = post(
            editor,
            "/skills",
            {
                "name": f"Boundary attacker {state['runId']}",
                "alias": f"boundary-attacker-{state['runId']}",
                "description": "Cannot borrow a foreign workflow or department grant",
                "ownerDepartmentId": peer["id"],
            },
        )
        response = editor.post(
            f"/api/v1/skills/{skill['id']}/versions",
            json={
                "expectedRevision": skill["draftRevision"],
                "dependencies": [reference("credential", state["credential"]["id"])],
            },
        )
        assert response.status_code in {403, 422}, response.text
        assert get(editor, f"/skills/{skill['id']}/versions") == []
    report(state, "remaining-skill-escalation", {"foreignGrantCannotPublish": True, "noVersionCreated": True})


def test_stdio_mcp_package_includes_sandbox_version_and_environment_credential(mcp_package):
    from tests.e2e.product.test_provider_integration import _sandbox_image_digest

    state = mcp_package
    control = state["control"]
    owner = state["departments"][2]["id"]
    sandbox = post(
        control,
        "/sandbox-profiles",
        {
            "name": f"Boundary stdio sandbox {state['runId']}",
            "ownerDepartmentId": owner,
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
    version = sandbox["current"]["id"]
    grant(control, "sandbox_profile", sandbox["id"], state["departments"][1]["id"], version)
    server = post(
        control,
        "/mcp/servers",
        {
            "name": f"Boundary stdio MCP {state['runId']}",
            "ownerDepartmentId": state["departments"][1]["id"],
            "transport": {
                "kind": "stdio",
                "command": "/usr/bin/python3",
                "args": ["-u", "-c", "print('isolated permission package')"],
                "environmentCredentialRefs": [{"name": "SAMPLE_TOKEN", "credentialId": state["credential"]["id"]}],
                "runtimeSandbox": {"resourceId": sandbox["id"], "resourceVersionId": version},
            },
            "configuration": {},
        },
    )
    workflow, request = workflow_request(state, reference("mcp_server", server["id"]), "stdio full package")
    assert {i["resourceType"] for i in request["items"]} == {"mcp_server", "sandbox_profile", "credential"}, request
    assert next(i for i in request["items"] if i["resourceType"] == "sandbox_profile")["resourceVersionId"] == version
    assert len(request["reviews"]) == 3, request
    approved = approve_all(state, workflow, request)
    report(
        state,
        "remaining-stdio-package",
        {"requestId": approved["id"], "sandboxVersionPinned": True, "environmentCredentialIncluded": True},
    )


@pytest.mark.parametrize("action", ["cancel", "reject"])
def test_nested_skill_package_cancel_or_reject_never_grants_dependencies(mcp_package, action):
    state = mcp_package
    leaf = skill_package(
        state, f"{action}-leaf", [reference("mcp_tool", state["tool"]["id"], state["tool"]["currentVersionId"])]
    )
    parent = skill_package(state, f"{action}-parent", [reference("skill", leaf["id"], leaf["versionId"])])
    workflow, request = workflow_request(
        state, reference("skill", parent["id"], parent["versionId"]), f"Skill {action}"
    )
    if action == "cancel":
        with authenticated(state["urls"]["web"], state["editor"]["token"]) as editor:
            response = editor.post(
                f"/api/v1/resource-grant-requests/{request['id']}/cancel", json={"expectedVersion": request["version"]}
            )
            assert response.status_code == 200, response.text
            final = response.json()
        assert final["status"] == "cancelled", final
    else:
        own = request["reviews"][0]
        i = next(i for i, d in enumerate(state["departments"]) if d["id"] == own["ownerDepartmentId"])
        with authenticated(state["urls"]["web"], state["reviewers"][i]["token"]) as reviewer:
            final = review(reviewer, request, own["ownerDepartmentId"], "reject")
        assert final["status"] == "rejected", final
    assert not workflow_grants(state, workflow, request)
    report(
        state,
        f"remaining-skill-{action}",
        {"requestId": request["id"], "status": final["status"], "noGrantCreated": True},
    )


@pytest.mark.parametrize("operation", ["read", "write"])
def test_skill_memory_dependency_respects_department_read_only_grant(mcp_package, operation):
    state = mcp_package
    control = state["control"]
    owner, skill_owner = state["departments"][0]["id"], state["departments"][2]["id"]
    connection = post(
        control,
        "/memory/connections",
        {
            "name": f"Boundary read-only memory {operation} {state['runId']}",
            "provider": "mem0",
            "endpoint": f"{state['echo']}/memory",
            "ownerDepartmentId": owner,
            "configuration": {},
        },
    )
    memory = post(
        control,
        "/memory/namespaces",
        {
            "name": f"Boundary read-only namespace {operation} {state['runId']}",
            "connectionId": connection["id"],
            "externalNamespace": f"readonly_{operation}_{state['runId']}",
            "accessMode": "read_write",
            "ownerDepartmentId": owner,
        },
    )
    grant(control, "memory", memory["id"], skill_owner, operation="read")
    skill = post(
        control,
        "/skills",
        {
            "name": f"Boundary memory Skill {operation} {state['runId']}",
            "alias": f"boundary-memory-{operation}-{state['runId']}",
            "description": "A read grant cannot authorize writing",
            "ownerDepartmentId": skill_owner,
        },
    )
    response = control.post(
        f"/api/v1/skills/{skill['id']}/versions",
        json={
            "expectedRevision": skill["draftRevision"],
            "dependencies": [reference("memory", memory["id"], operation=operation)],
        },
    )
    assert response.status_code == (201 if operation == "read" else 403), response.text
    assert len(get(control, f"/skills/{skill['id']}/versions")) == (1 if operation == "read" else 0)
    report(
        state,
        f"remaining-skill-memory-{operation}",
        {"statusCode": response.status_code, "departmentReadOnlyEnforced": True},
    )


@pytest.mark.parametrize("kind", ["mcp_server", "mcp_tool"])
def test_frozen_mcp_version_uses_its_own_credential_dependency(mcp_package, kind):
    from tests.e2e.runtime.test_agent_attachments import _control_mysql

    state = mcp_package
    control = state["control"]
    server = post(
        control,
        "/mcp/servers",
        {
            "name": f"Boundary frozen {kind} {state['runId']}",
            "ownerDepartmentId": state["departments"][1]["id"],
            "transport": {
                "kind": "streamable_http",
                "endpoint": f"{state['echo']}/mcp",
                "bearerCredentialId": state["credential"]["id"],
            },
            "configuration": {},
        },
    )
    tool = next(t for t in post(control, f"/mcp/servers/{server['id']}/discover", {})["tools"] if t["name"] == "echo")
    old_version = _control_mysql(
        state["context"],
        f"SELECT BIN_TO_UUID(server_version_id) FROM mcp_tool_versions WHERE id=UUID_TO_BIN('{tool['currentVersionId']}');",
    )
    other = post(
        control,
        "/credentials",
        {
            "name": f"Boundary future MCP key {kind} {state['runId']}",
            "credentialType": "bearer",
            "secret": "future-invalid-key",
            "ownerDepartmentId": state["departments"][0]["id"],
        },
    )
    grant(control, "credential", other["id"], state["departments"][1]["id"])
    response = control.patch(
        f"/api/v1/mcp/servers/{server['id']}",
        json={
            "name": server["name"],
            "description": server["description"],
            "status": "active",
            "version": server["version"],
            "transport": {**server["transport"], "bearerCredentialId": other["id"]},
            "configuration": {},
        },
    )
    assert response.status_code == 200, response.text
    resource = reference(
        kind,
        server["id"] if kind == "mcp_server" else tool["id"],
        old_version if kind == "mcp_server" else tool["currentVersionId"],
    )
    workflow, request = workflow_request(state, resource, f"frozen {kind}")
    credential_ids = {i["resourceId"] for i in request["items"] if i["resourceType"] == "credential"}
    assert credential_ids == {state["credential"]["id"]}, request
    assert next(i for i in request["items"] if i["resourceType"] == "mcp_server")["resourceVersionId"] == old_version
    approved = approve_all(state, workflow, request)
    execution = None
    if kind == "mcp_tool":
        model = new_model(state, "frozen-mcp")
        template = model_workflow(state, model, "frozen MCP template")
        authorize(control, workflow, reference("model", model["id"], model["deploymentId"]))
        save_version(control, workflow, agent_definition(control, template["workflowId"], model, tool))
        execution, _ = invoke(state, workflow)
        calls = get(control, f"/executions/{execution}/runtime-details")["calls"]
        assert any(c["callKind"] == "mcp_tool" and c["status"] == "succeeded" for c in calls), calls
    report(
        state,
        f"remaining-frozen-{kind}",
        {
            "requestId": approved["id"],
            "oldServerVersionId": old_version,
            "oldCredentialSelected": True,
            "executionId": execution,
        },
    )


@pytest.mark.parametrize("changed", ["server", "tool", "credential"])
def test_mcp_pending_package_stales_on_dependency_change(mcp_package, changed):
    state = mcp_package
    control = state["control"]
    workflow, request = workflow_request(
        state, reference("mcp_tool", state["tool"]["id"], state["tool"]["currentVersionId"]), f"MCP stale {changed}"
    )
    if changed == "credential":
        value = get(control, f"/credentials/{state['credential']['id']}")
        response = control.post(
            f"/api/v1/credentials/{value['id']}/rotate",
            json={"secret": state["mcpKey"].value, "version": value["version"]},
        )
    elif changed == "server":
        value = get(control, f"/mcp/servers/{state['server']['id']}")
        response = control.patch(
            f"/api/v1/mcp/servers/{value['id']}",
            json={
                "name": value["name"],
                "description": value["description"],
                "status": "disabled",
                "transport": value["transport"],
                "version": value["version"],
                "configuration": {},
            },
        )
    else:
        value = next(t for t in get(control, "/mcp/tools?pageSize=100")["items"] if t["id"] == state["tool"]["id"])
        response = control.patch(
            f"/api/v1/mcp/tools/{value['id']}/policy",
            json={
                "enabled": False,
                "debugEnabled": value["debugEnabled"],
                "timeoutSeconds": value["timeoutSeconds"],
                "sideEffect": value["sideEffect"],
                "version": value["version"],
            },
        )
    assert response.status_code == 200, response.text
    try:
        own = request["reviews"][0]
        response = control.post(
            f"/api/v1/resource-grant-requests/{request['id']}/reviews/{own['ownerDepartmentId']}/approve",
            json={"expectedVersion": own["version"], "comment": None},
        )
        assert response.status_code == 409 and response.json()["code"] == "RESOURCE_GRANT_REQUEST_STALE", response.text
        assert get(control, f"/resource-grant-requests/{request['id']}")["status"] == "stale"
    finally:
        if changed == "server":
            value = get(control, f"/mcp/servers/{value['id']}")
            response = control.patch(
                f"/api/v1/mcp/servers/{value['id']}",
                json={
                    "name": value["name"],
                    "description": value["description"],
                    "status": "active",
                    "transport": value["transport"],
                    "version": value["version"],
                    "configuration": {},
                },
            )
            assert response.status_code == 200, response.text
        elif changed == "tool":
            value = next(t for t in get(control, "/mcp/tools?pageSize=100")["items"] if t["id"] == value["id"])
            response = control.patch(
                f"/api/v1/mcp/tools/{value['id']}/policy",
                json={
                    "enabled": True,
                    "debugEnabled": value["debugEnabled"],
                    "timeoutSeconds": value["timeoutSeconds"],
                    "sideEffect": value["sideEffect"],
                    "version": value["version"],
                },
            )
            assert response.status_code == 200, response.text
    assert not workflow_grants(state, workflow, request)
    report(state, f"remaining-mcp-stale-{changed}", {"requestId": request["id"], "noGrantCreated": True})
