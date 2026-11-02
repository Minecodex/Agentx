"""Real-provider probes run through the same bounded relay as the full matrix."""

from __future__ import annotations

import json
import os
import ssl
from decimal import Decimal
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.existing_model_support import existing_model as existing_model
from tests.e2e.product.existing_model_support import existing_model_secret as existing_model_secret

pytestmark = [pytest.mark.product, pytest.mark.live_model]


@pytest.fixture
def preflight_client(existing_model, existing_model_secret):
    certificate = os.environ.get("AGENTX_E2E_LLM_CA_FILE")
    if not certificate:
        pytest.fail("AGENTX_E2E_LLM_CA_FILE is required for isolated real-model acceptance", pytrace=False)
    with httpx.Client(
        base_url=httpx.URL(existing_model["endpoint"]).copy_with(path="/"),
        headers={"Authorization": "Bearer " + existing_model_secret.value},
        verify=ssl.create_default_context(cafile=certificate),
        trust_env=False,
        timeout=180,
    ) as client:
        yield client


def _record(run_id: str, name: str, record: dict) -> None:
    directory = Path(__file__).resolve().parents[2] / ".local/artifacts/e2e" / run_id / "model-preflight"
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f"{name}.json").write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n")


def test_real_provider_function_call(preflight_client, existing_model, run_id):
    response = preflight_client.post(
        "/v1/chat/completions",
        json={
            "model": existing_model["modelName"],
            "stream": False,
            "max_tokens": 2048,
            "messages": [
                {
                    "role": "user",
                    "content": "Call echo_result with value 7. This is a synthetic API compatibility test.",
                }
            ],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "echo_result",
                        "description": "Echo a synthetic integer",
                        "parameters": {
                            "type": "object",
                            "properties": {"value": {"type": "integer"}},
                            "required": ["value"],
                            "additionalProperties": False,
                        },
                    },
                }
            ],
            "tool_choice": {"type": "function", "function": {"name": "echo_result"}},
        },
    )
    assert response.status_code == 200, f"Real provider returned HTTP {response.status_code}"
    result = response.json()
    calls = result["choices"][0]["message"].get("tool_calls", [])
    assert len(calls) == 1
    assert calls[0]["function"]["name"] == "echo_result"
    assert json.loads(calls[0]["function"]["arguments"]) == {"value": 7}
    assert result["usage"]["prompt_tokens"] > 0
    assert result["usage"]["completion_tokens"] > 0
    _record(
        run_id,
        "function-call",
        {
            "status": "passed",
            "model": existing_model["modelName"],
            "responseModel": result.get("model"),
            "usage": result["usage"],
            "tool": "echo_result",
            "argumentValue": 7,
        },
    )


def test_real_provider_stream_and_usage(preflight_client, existing_model, run_id):
    pieces = []
    usage = None
    finished = False
    with preflight_client.stream(
        "POST",
        "/v1/chat/completions",
        json={
            "model": existing_model["modelName"],
            "stream": True,
            "stream_options": {"include_usage": True},
            "max_tokens": 2048,
            "messages": [
                {"role": "user", "content": "Reply with the single word READY. This is a synthetic streaming test."}
            ],
        },
    ) as response:
        assert response.status_code == 200, f"Real provider returned HTTP {response.status_code}"
        for line in response.iter_lines():
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                finished = True
                continue
            event = json.loads(data)
            if isinstance(event.get("usage"), dict):
                usage = event["usage"]
            for choice in event.get("choices", []):
                text = choice.get("delta", {}).get("content")
                if text:
                    pieces.append(text)
    assert finished
    assert "READY" in "".join(pieces)
    assert usage and usage["prompt_tokens"] > 0 and usage["completion_tokens"] > 0
    budget = preflight_client.get("/budget")
    assert budget.status_code == 200
    snapshot = budget.json()
    assert not snapshot["halted"]
    assert Decimal(snapshot["spentUsd"]) + Decimal(snapshot["heldUsd"]) <= Decimal("30")
    _record(
        run_id,
        "stream",
        {
            "status": "passed",
            "model": existing_model["modelName"],
            "usage": usage,
            "content": "".join(pieces),
            "budget": snapshot,
        },
    )


@pytest.mark.parametrize("shape", ["object", "parallel_objects", "historical_explicit"])
def test_real_provider_preserves_native_json_wait_response(preflight_client, existing_model, run_id, shape):
    """Probe concrete published wait schemas through the actual provider."""
    answer = {"contact": "王敏", "date": "2026-12-03", "email": "wang.min@example.test"}
    content_schema = {
        "type": "object",
        "additionalProperties": False,
        "required": list(answer),
        "properties": {key: {"type": "string"} for key in answer},
    }
    if shape == "parallel_objects":
        content_schema = {
            "anyOf": [
                content_schema,
                {
                    "type": "object",
                    "additionalProperties": False,
                    "required": ["company", "taxId"],
                    "properties": {"company": {"type": "string"}, "taxId": {"type": "string"}},
                },
            ]
        }
    if shape == "historical_explicit":
        content_schema = {"type": ["object", "string"], "anyOf": [content_schema, {"type": "string"}]}
    response = preflight_client.post(
        "/v1/chat/completions",
        json={
            "model": existing_model["modelName"],
            "stream": False,
            "max_tokens": 2048,
            "reasoning_effort": "low",
            "messages": [
                {
                    "role": "user",
                    "content": f"为当前待办补充交付安排。待办 responseSchema 要求对象，资料：{json.dumps(answer, ensure_ascii=False)}。使用 submit_task_input 发送原生 JSON content，不要把对象编码成字符串。这是独立的协议类型预检，不会执行业务。",
                }
            ],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "submit_task_input",
                        "description": "Provide native JSON content matching the task's responseSchema. Objects and arrays must remain JSON values.",
                        "parameters": {
                            "type": "object",
                            "additionalProperties": False,
                            "required": ["content"],
                            "properties": {"content": content_schema},
                        },
                    },
                }
            ],
            "tool_choice": {"type": "function", "function": {"name": "submit_task_input"}},
        },
    )
    assert response.status_code == 200, f"Real provider returned HTTP {response.status_code}"
    result = response.json()
    call = result["choices"][0]["message"]["tool_calls"]
    assert len(call) == 1 and call[0]["function"]["name"] == "submit_task_input"
    arguments = json.loads(call[0]["function"]["arguments"])
    _record(
        run_id,
        "native-json-wait-" + shape,
        {
            "model": existing_model["modelName"],
            "arguments": arguments,
            "usage": result["usage"],
            "passed": arguments == {"content": answer},
        },
    )
    assert arguments == {"content": answer}, arguments
