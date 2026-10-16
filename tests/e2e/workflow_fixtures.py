"""Shared pure workflow definitions for functional and capacity fixtures."""

from copy import deepcopy
from itertools import pairwise


def set_chain_definition(base: dict, nodes: int) -> dict:
    definition = deepcopy(base)
    definition["start"]["inputs"] = {
        "type": "object",
        "properties": {"message": {"type": "string"}},
        "required": ["message"],
        "additionalProperties": False,
    }
    exit_node = next(node for node in definition["nodes"] if node["type"] == "exit")
    exit_node["parameters"]["outputs"] = {
        "answer": {
            "kind": "reference",
            "selector": {
                "namespace": "inputs",
                "run": {"kind": "current"},
                "item": {"kind": "current"},
                "path": ["message"],
            },
            "missingPolicy": {"kind": "error"},
        }
    }
    definition["end"] = {
        "completion": "first_return",
        "outputs": {"answer": {"schema": {"type": "string"}, "required": True, "sensitive": False}},
        "error": {"outputs": {}},
    }
    chain = []
    for index in range(nodes - 1):
        node_id = f"capacity_{index}"
        chain.append(node_id)
        definition["nodes"].append(
            {
                "id": node_id,
                "key": node_id,
                "type": "set",
                "typeVersion": 1,
                "name": node_id,
                "disabled": False,
                "parameters": {"values": {"kind": "object", "fields": {}}, "keepOnlySet": False},
                "settings": {},
                "contextWrites": [],
                "resourceReferences": [],
            }
        )
    chain = ["__start__", *chain, exit_node["id"]]
    definition["connections"] = [
        {
            "id": f"capacity-edge-{index}",
            "sourceNodeId": source,
            "sourceHandle": "main",
            "targetNodeId": target,
            "targetHandle": "main",
            "order": 0,
        }
        for index, (source, target) in enumerate(pairwise(chain))
    ]
    return definition
