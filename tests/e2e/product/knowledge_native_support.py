"""Seed external indexes through provider-native APIs, never through Agentx."""

from __future__ import annotations

import socket
import time
from contextlib import contextmanager

import httpx

from tests.e2e.support import start_process


@contextmanager
def provider_client(namespace, service, port, directory, headers=None, health="/health"):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        local = listener.getsockname()[1]
    directory.mkdir(parents=True, exist_ok=True)
    forward = start_process(
        ("kubectl", "-n", namespace, "port-forward", f"service/{service}", f"{local}:{port}"),
        stdout_path=directory / f"{service}-forward.log",
        stderr_path=directory / f"{service}-forward-error.log",
    )
    try:
        with httpx.Client(base_url=f"http://127.0.0.1:{local}", headers=headers, timeout=120) as client:
            deadline = time.monotonic() + 60
            while True:
                assert forward.process.poll() is None, "Provider port-forward exited"
                try:
                    if client.get(health, timeout=3).status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                assert time.monotonic() < deadline, "Provider did not become ready"
                time.sleep(0.5)
            yield client
    finally:
        forward.stop()


def seed_lightrag(client, content, name):
    inserted = client.post("/documents/text", json={"text": content, "file_source": name})
    assert inserted.status_code == 200, inserted.text
    track = inserted.json()["track_id"]
    deadline = time.monotonic() + 600
    while True:
        response = client.get(f"/documents/track_status/{track}")
        assert response.status_code == 200, response.text
        documents = response.json()["documents"]
        assert not any(item["status"].lower() == "failed" for item in documents), documents
        if documents and all(item["status"].lower() == "processed" for item in documents):
            return documents[0]
        assert time.monotonic() < deadline, documents
        time.sleep(2)


def knowledge_workflow(control, name, queries, query="水星计划的发布代号是什么"):
    from tests.e2e.product.remaining_support import authorize, reference, save_version
    from tests.e2e.product.studio_builder_support import post

    workflow = post(
        control,
        "/workflows",
        {"name": name, "description": "Multi-source knowledge acceptance", "visibility": "company"},
    )
    for item in queries:
        authorize(control, workflow["id"], reference("rag", item["id"], operation="read"))
    definition = knowledge_definition(control, workflow["id"], queries, query)
    version = save_version(control, workflow["id"], definition)
    assert version.get("id"), version
    return workflow["id"]


def knowledge_definition(control, workflow, queries, query="水星计划的发布代号是什么"):
    from tests.e2e.draft_fixtures import fixture_draft
    from tests.e2e.product.remaining_support import reference

    draft = fixture_draft(control, workflow)
    definition = draft["definition"]
    output = {
        "text": {"type": "string"},
        "documents": {"type": "array", "items": {"type": "object"}},
        "files": {"type": "array", "items": {"type": "object"}},
        "partial": {"type": "boolean"},
        "sources": {"type": "array", "items": {"type": "object"}},
    }
    definition["nodes"].append(
        {
            "id": "retrieve",
            "key": "retrieve",
            "name": "Retrieve external knowledge",
            "type": "knowledge_retrieval",
            "typeVersion": 1,
            "parameters": {"query": {"kind": "literal", "value": query}, "topK": 20},
            "contextWrites": [],
            "resourceReferences": [reference("rag", item["id"], operation="read") for item in queries],
        }
    )
    exit_node = next(n for n in definition["nodes"] if n["type"] == "exit")
    exit_node["parameters"] = {
        "outputs": {
            key: {"kind": "reference", "selector": {"namespace": "outputs", "sourceNodeId": "retrieve", "path": [key]}}
            for key in output
        },
        "errorOutputs": {},
    }
    definition["end"]["outputs"] = {
        key: {"schema": schema, "required": True, "sensitive": False} for key, schema in output.items()
    }
    definition["connections"] = [
        {
            "id": f"edge-{index}",
            "sourceNodeId": source,
            "targetNodeId": target,
            "sourceHandle": "main",
            "targetHandle": "main",
            "order": 0,
        }
        for index, (source, target) in enumerate((("__start__", "retrieve"), ("retrieve", exit_node["id"])))
    ]
    return definition
