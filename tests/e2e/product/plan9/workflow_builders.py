"""Small builders for formal Definition 8.0 fixtures; execution always stays in Runtime."""

from __future__ import annotations

from tests.e2e.draft_fixtures import fixture_draft
from tests.e2e.product.next_batch_support import get
from tests.e2e.product.plan9.scenarios import connect, reference
from tests.e2e.product.studio_builder_support import post
from tests.e2e.product.test_live_memory_pipeline import _authorize_definition, _save_version
from tests.e2e.product.test_provider_integration import _publish_application_deployment


def literal(value):
    return {"kind": "literal", "value": value}


def node(identifier, kind, title, parameters, **extra):
    return {
        "id": identifier,
        "key": identifier,
        "type": kind,
        "typeVersion": 1,
        "name": title,
        "parameters": parameters,
        "contextWrites": [],
        "resourceReferences": [],
        "settings": {},
        **extra,
    }


def object_schema(properties, required=None):
    return {
        "type": "object",
        "properties": properties,
        "required": list(properties) if required is None else required,
        "additionalProperties": False,
    }


def code(identifier, title, profile, bindings, source, example):
    return node(
        identifier,
        "code",
        title,
        {
            "runner": "python",
            "inputs": {"kind": "object", "fields": bindings},
            "source": source,
            "outputExample": example,
            "networkPolicy": {"mode": "deny", "destinations": []},
        },
        resourceReferences=[{"resourceType": "sandbox_profile", "resourceId": profile["id"], "operation": "use"}],
        settings={"timeoutMs": 180000},
    )


def publish(
    control, run_id, scene, title, inputs, outputs, nodes, bindings, cases, expected, *, connections=None, context=None
):
    workflow = post(control, "/workflows", {"name": f"{scene} {title} {run_id}", "visibility": "company"})["id"]
    definition = fixture_draft(control, workflow)["definition"]
    exit_node = next(value for value in definition["nodes"] if value["type"] == "exit")
    exit_node["parameters"]["outputs"] = bindings
    definition["start"]["inputs"] = inputs
    definition["nodes"] = [*nodes, exit_node]
    definition["connections"] = connections or connect(
        ["__start__", *(value["id"] for value in nodes), exit_node["id"]]
    )
    # Exit IDs are minted by the normal draft initializer.
    for edge in definition["connections"]:
        if edge["targetNodeId"] == "$exit":
            edge["targetNodeId"] = exit_node["id"]
    definition["settings"].update(timeoutMs=900000, activationBudget=200)
    definition["end"] = {
        "completion": "first_return",
        "outputs": {key: {"schema": schema, "required": True, "sensitive": False} for key, schema in outputs.items()},
        "error": {"outputs": {}},
    }
    _authorize_definition(control, workflow, definition)
    working = fixture_draft(control, workflow)
    checked = control.post(
        f"/api/v1/workflows/{workflow}/drafts/{working['id']}/validate",
        json={"definition": definition, "editorDocument": working["editorDocument"]},
    )
    assert checked.status_code == 200, checked.text
    errors = [issue for issue in checked.json()["issues"] if issue["severity"] == "error"]
    assert not errors, f"{scene} workflow validation failed: {errors}"
    version = _save_version(control, workflow, definition)
    assert isinstance(version.get("id"), str), f"{scene} workflow publication failed: {version}"
    app = post(
        control,
        "/applications",
        {
            "workflowId": workflow,
            "name": f"{title} {run_id}",
            "slug": f"p9-{scene.lower()}-{run_id}",
            "visibility": "company",
        },
    )
    environment = next(value for value in get(control, "/environments") if value["code"] == "development")
    deployment = _publish_application_deployment(
        control, dict(control.headers), app["id"], version["id"], environment["id"]
    )
    return {
        "scene": scene,
        "applicationId": app["id"],
        "slug": app["slug"],
        "workflowId": workflow,
        "versionId": version["id"],
        "deploymentId": deployment["id"],
        "inputs": cases,
        "expected": expected,
        **(context or {}),
    }


def output_binding(node_id, key):
    return reference("outputs", ["structuredOutput", key], node_id)


def wait(identifier, title, schema):
    return node(identifier, "await_input", title, {"responseSchema": schema, "timeoutMs": 600000})


def edge(identifier, source, target, port="main", target_port="main", order=0):
    return {
        "id": identifier,
        "sourceNodeId": source,
        "sourceHandle": port,
        "targetNodeId": target,
        "targetHandle": target_port,
        "order": order,
    }
