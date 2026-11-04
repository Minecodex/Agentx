"""One metered provider preflight, included in the same plan10 money/request ledger."""

import json
import os
import ssl
from pathlib import Path

import httpx
import pytest

from tests.e2e.product.live_model_config import model_configuration, model_secret


@pytest.mark.live_model
def test_plan10_explicit_metered_provider():
    config = model_configuration()
    key = model_secret()
    ca = os.environ["AGENTX_E2E_LLM_CA_FILE"]
    with httpx.Client(verify=ssl.create_default_context(cafile=ca), timeout=120, trust_env=False) as client:
        response = client.post(
            f"{config['endpoint']}/chat/completions",
            headers={"Authorization": f"Bearer {key.value}"},
            json={
                "model": config["modelName"],
                "messages": [{"role": "user", "content": "Reply with OK."}],
                "reasoning_effort": "low",
                "max_tokens": 64,
                "stream": True,
                "stream_options": {"include_usage": True},
            },
        )
        directory = Path(".local/artifacts/plan10/implementation-2026-11-02")
        directory.mkdir(parents=True, exist_ok=True)
        events = [
            json.loads(line[5:].strip())
            for line in response.text.splitlines()
            if line.startswith("data:") and line[5:].strip() != "[DONE]"
        ]
        usage = next((item["usage"] for item in reversed(events) if item.get("usage")), None)
        report = {
            "status": response.status_code,
            "model": config["modelName"],
            "usage": usage,
            "passed": response.status_code == 200,
        }
        (directory / "provider-preflight.json").write_text(json.dumps(report, indent=2) + "\n")
        assert response.status_code == 200, f"Metered model preflight returned HTTP {response.status_code}"
        assert isinstance(report["usage"], dict)
